//! Folder sync transport: one reconcile round against an encrypted
//! snapshot file in a local directory.
//!
//! Same shape as the SFTP transport and the same blob, minus the
//! network: read, LWW-merge into the vault, rebuild, write back. The
//! merge core lives in `oryxis-sync` (`merge_snapshot` /
//! `build_full_snapshot`), so this file only moves bytes.
//!
//! The directory is whatever the OS already mounts, which is the point.
//! A cloud client's folder (OneDrive, Google Drive, Dropbox, iCloud), a
//! network share, an external disk, a Syncthing directory: every one of
//! those becomes a sync destination without a line of provider code, no
//! OAuth, no client secret embedded in the binary, and no app
//! registration to keep alive. It is also the cheapest transport to
//! reason about, because the filesystem gives us the atomic rename the
//! SFTP path has to ask the server for.
//!
//! What the user must know, and the UI says: two machines writing the
//! same cloud-mirrored file is read-modify-write without a strong
//! compare-and-swap, exactly like the SFTP transport. The cloud client
//! may also produce its own "conflicted copy" that Oryxis never sees.
//! LWW heals the ordinary race on the next round (delay, not loss); for
//! two machines on one network, P2P is the better answer.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use iced::Task;

use oryxis_vault::VaultStore;

use crate::app::{Message, Oryxis, SyncMessage};
use crate::i18n::t;

/// Snapshot filename appended when the user points at a directory.
/// Deliberately the same constant the SFTP transport uses, so a user
/// can aim both at one blob if they ever want to.
const DEFAULT_SNAPSHOT_NAME: &str = "oryxis-sync.bin";

/// Resolve the configured path to the snapshot FILE.
///
/// A directory (the common case: someone picks their OneDrive folder)
/// gets the default filename appended; a path that already names a file
/// is used as-is. An existing directory is detected on disk rather than
/// by a trailing separator, because a folder picker rarely returns one.
fn snapshot_path(input: &str) -> PathBuf {
    let path = Path::new(input.trim());
    if path.is_dir() || input.ends_with('/') || input.ends_with('\\') {
        path.join(DEFAULT_SNAPSHOT_NAME)
    } else {
        path.to_path_buf()
    }
}

impl Oryxis {
    /// Run one folder-sync round.
    ///
    /// Validates config while holding `&self`, then does the file work
    /// off-thread. Returns `Task::none()` with an inline status on any
    /// precondition failure, so the caller can fire it blindly.
    pub(crate) fn run_folder_sync_round(&mut self) -> Task<Message> {
        if self.sync.folder.in_progress {
            return Task::none();
        }
        // A locked vault has no master key to decrypt with; the
        // manual buttons must not rely on the lock screen hiding them.
        if !self.sync_round_allowed() {
            return Task::none();
        }
        let input = self.sync.folder.path.trim().to_string();
        if input.is_empty() {
            self.sync.folder.status = Some(Err(t("folder_sync_no_path").to_string()));
            return Task::none();
        }
        let Some(vault) = &self.vault else {
            return Task::none();
        };
        // Group key from STORAGE, not the form field: the four snapshot
        // transports share one `sync_sftp_passphrase` row, and a sibling
        // transport's edit can leave this form stale. Sealing with a
        // stale value pushes a snapshot the next session cannot decrypt
        // (see `run_git_sync_round`).
        let passphrase = match vault.get_sync_sftp_passphrase() {
            Ok(Some(p)) => p,
            Ok(None) => {
                self.sync.folder.status = Some(Err(t("sftp_sync_no_passphrase").to_string()));
                return Task::none();
            }
            Err(e) => {
                self.sync.folder.status = Some(Err(e.to_string()));
                return Task::none();
            }
        };
        // Argon2id derivation (~0.4 s) runs inside the blocking task,
        // not on the UI thread (same reason as the Git transport).
        let db_path = vault.db_path().to_path_buf();
        let master_password = self.master_password.clone();

        // Per-device temp-file tag, the same guard the SFTP transport
        // documents: two devices sharing one temp name would interleave
        // their writes into a single corrupt blob that every device
        // then fails to merge, wedging sync permanently. On a
        // cloud-mirrored folder this matters MORE than over SFTP,
        // because the cloud client is a third writer in the directory.
        // Its own key, so a vault that syncs both ways keeps the two
        // transports' tags apart.
        let device_tag = match vault
            .get_setting("folder_sync_device_tag")
            .ok()
            .flatten()
            .filter(|s| !s.is_empty())
        {
            Some(tag) => tag,
            None => {
                let tag = uuid::Uuid::new_v4().to_string();
                let _ = vault.set_setting("folder_sync_device_tag", &tag);
                tag
            }
        };

        self.sync.folder.in_progress = true;
        self.sync.folder.status = None;
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    let secret = oryxis_vault::derive_sync_secret(&passphrase)
                        .map_err(|e| e.to_string())?;
                    folder_round(&input, &db_path, master_password, &secret, &device_tag)
                })
                .await
                .unwrap_or_else(|e| Err(format!("join: {e}")))
            },
            |result| Message::Sync(SyncMessage::FolderRoundFinished(result)),
        )
    }
}

/// The round itself, on a blocking thread: read, merge, rebuild, write.
///
/// Returns how many records the remote snapshot carried, which is what
/// the status line reports.
fn folder_round(
    input: &str,
    db_path: &Path,
    master_password: Option<String>,
    secret: &[u8; 32],
    device_tag: &str,
) -> Result<usize, String> {
    let target = snapshot_path(input);
    let Some(dir) = target.parent() else {
        return Err(t("folder_sync_no_path").to_string());
    };
    // A path whose directory does not exist is a typo (or an unmounted
    // drive), not something to create silently: writing the vault into
    // a freshly-made directory nobody is watching looks like success
    // and syncs with nothing.
    if !dir.as_os_str().is_empty() && !dir.is_dir() {
        return Err(t("folder_sync_missing_dir").to_string());
    }

    let mut vault = VaultStore::open(db_path).map_err(|e| e.to_string())?;
    match master_password.as_deref() {
        Some(pw) => vault.unlock(pw).map_err(|e| e.to_string())?,
        None => vault.open_without_password().map_err(|e| e.to_string())?,
    }
    let vault = Arc::new(Mutex::new(vault));

    // Merge whatever is already there. A missing file is the first
    // round, not an error.
    let mut pulled = 0usize;
    if target.is_file() {
        let blob = std::fs::read(&target).map_err(|e| e.to_string())?;
        pulled = oryxis_sync::merge_snapshot(&vault, &blob, secret)
            .map_err(|e| e.to_string())?;
    }

    // Rebuild from the merged vault and install it atomically. The temp
    // file goes in the TARGET's directory on purpose: `rename` is only
    // atomic within one filesystem, and a temp in the system temp dir
    // would silently degrade to copy+delete across mounts, which is the
    // window where another device reads half a snapshot.
    let snapshot = oryxis_sync::build_full_snapshot(&vault, secret)
        .map_err(|e| e.to_string())?;
    let tmp = target.with_file_name(format!(
        "{}.tmp.{device_tag}",
        target
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| DEFAULT_SNAPSHOT_NAME.to_string())
    ));
    std::fs::write(&tmp, &snapshot).map_err(|e| e.to_string())?;
    if let Err(e) = std::fs::rename(&tmp, &target) {
        // Leave nothing behind for the cloud client to upload as a
        // stray file.
        let _ = std::fs::remove_file(&tmp);
        return Err(e.to_string());
    }
    Ok(pulled)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_directory_gets_the_shared_snapshot_name() {
        let dir = tempfile::tempdir().unwrap();
        let resolved = snapshot_path(dir.path().to_str().unwrap());
        assert_eq!(resolved.file_name().unwrap(), DEFAULT_SNAPSHOT_NAME);
        assert_eq!(resolved.parent().unwrap(), dir.path());
    }

    /// A path that names a file is honoured as typed, so someone
    /// keeping two groups in one folder can give them separate blobs.
    #[test]
    fn an_explicit_filename_is_kept() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("team.bin");
        let resolved = snapshot_path(file.to_str().unwrap());
        assert_eq!(resolved, file);
    }

    /// The round trips through the same blob the SFTP transport uses,
    /// so a second device reading the file sees the first device's
    /// vault. This also pins the atomic-install path: the temp must be
    /// gone and only the target left.
    #[test]
    fn a_round_writes_a_snapshot_the_next_round_can_merge() {
        let dir = tempfile::tempdir().unwrap();
        let vault_file = dir.path().join("vault.db");
        {
            let mut v = VaultStore::open(&vault_file).unwrap();
            v.set_master_password("pw").unwrap();
            let mut conn = oryxis_core::models::Connection::new("web", "example.com");
            conn.username = Some("deploy".into());
            v.save_connection(&conn, None).unwrap();
        }
        let secret = oryxis_vault::derive_sync_secret("group-passphrase").unwrap();
        let pulled = folder_round(
            dir.path().to_str().unwrap(),
            &vault_file,
            Some("pw".to_string()),
            &secret,
            "device-a",
        )
        .expect("the round completed");
        // First round: nothing to pull, everything to push.
        assert_eq!(pulled, 0);

        let target = dir.path().join(DEFAULT_SNAPSHOT_NAME);
        assert!(target.is_file(), "the snapshot was installed");
        // No temp file survives a successful install.
        let strays: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains(".tmp."))
            .collect();
        assert!(strays.is_empty(), "temp files left behind: {strays:?}");

        // A SECOND vault pointed at the same folder absorbs the first
        // one's records, which is the whole contract.
        let other_vault = dir.path().join("other.db");
        {
            let mut v = VaultStore::open(&other_vault).unwrap();
            v.set_master_password("pw2").unwrap();
        }
        let pulled = folder_round(
            dir.path().to_str().unwrap(),
            &other_vault,
            Some("pw2".to_string()),
            &secret,
            "device-b",
        )
        .expect("the second round completed");
        assert!(pulled > 0, "the second device merged the first one's records");
        let v = VaultStore::open(&other_vault).unwrap();
        let mut v = v;
        v.unlock("pw2").unwrap();
        assert!(
            v.list_connections().unwrap().iter().any(|c| c.label == "web"),
            "the host crossed over"
        );
    }

    /// A directory that does not exist is a typo or an unmounted drive.
    /// Creating it would look like success while syncing with nothing.
    #[test]
    fn a_missing_directory_is_an_error_not_a_mkdir() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("not-mounted").join("snap.bin");
        let vault_file = dir.path().join("vault.db");
        {
            let mut v = VaultStore::open(&vault_file).unwrap();
            v.set_master_password("pw").unwrap();
        }
        let secret = oryxis_vault::derive_sync_secret("p").unwrap();
        let err = folder_round(
            missing.to_str().unwrap(),
            &vault_file,
            Some("pw".to_string()),
            &secret,
            "device-a",
        )
        .unwrap_err();
        assert!(!err.is_empty());
        assert!(!missing.exists(), "nothing was created");
    }
}
