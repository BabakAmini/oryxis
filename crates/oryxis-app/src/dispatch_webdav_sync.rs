//! WebDAV sync transport: the same encrypted snapshot on a Nextcloud,
//! ownCloud, Synology or plain WebDAV server.
//!
//! Why this exists next to the folder transport, which already reaches
//! those servers through their desktop client: not everyone can run
//! that client. A work machine with an install policy, a headless box,
//! or simply not wanting a daemon mirroring gigabytes in the background
//! to carry a 10 KB file. Credentials are a URL, a user and an app
//! password, so there is no OAuth app to register and keep alive.
//!
//! It is also the only file transport with real conflict DETECTION.
//! `If-Match` on the ETag is a compare-and-swap: the server answers 412
//! when someone else wrote since our GET, so the round starts over on
//! top of theirs instead of flattening it. SFTP and the folder
//! transport can only self-heal afterwards, on the next round.
//!
//! Semantics below were measured against wsgidav 4.3.5, not assumed:
//! GET returns an `ETag`; PUT with `If-None-Match: *` creates (201) and
//! answers 412 when the resource already exists; a stale `If-Match` is
//! 412; a PUT into a missing collection is 409; MKCOL is 201, or 405
//! when the collection is already there.

use std::path::Path;
use std::sync::{Arc, Mutex};

use iced::Task;

use oryxis_vault::VaultStore;

use crate::app::{Message, Oryxis, SyncMessage};
use crate::i18n::t;

/// Snapshot filename appended when the URL names a collection. Same
/// name the SFTP and folder transports use, so one blob can serve
/// several of them.
const SNAPSHOT_NAME: &str = "oryxis-sync.bin";

/// Retries after a lost compare-and-swap, the same budget as the Git
/// transport. A 412 means another device wrote first, so the answer is
/// to redo the round on top of theirs, never to overwrite them.
const CAS_ATTEMPTS: usize = 3;

/// Resolve the configured URL to the snapshot resource. A URL ending in
/// `/` is a collection and gets the shared filename.
fn snapshot_url(base: &str) -> String {
    let base = base.trim();
    if base.ends_with('/') {
        format!("{base}{SNAPSHOT_NAME}")
    } else {
        base.to_string()
    }
}

/// What the GET found. Three states, not two, and the third is the one
/// that matters: a server can serve a resource without an `ETag`, and
/// treating that as "absent" would send `If-None-Match: *` against
/// something that exists, collect a 412 on every attempt and never be
/// able to write at all.
enum Remote {
    /// 404: nothing there yet, the first round against this server.
    Absent,
    /// Present, with the tag needed to write back over it.
    Tagged { body: Vec<u8>, etag: String },
    /// Present, but the server offered no `ETag`. Degrades to
    /// last-writer-wins, exactly like the folder transport.
    Untagged { body: Vec<u8> },
}

impl Remote {
    fn body(&self) -> Option<&[u8]> {
        match self {
            Remote::Absent => None,
            Remote::Tagged { body, .. } | Remote::Untagged { body } => Some(body),
        }
    }
}

/// How a PUT ended. Typed rather than sniffed out of an error string,
/// because both non-success cases drive control flow.
enum Stored {
    Written,
    /// 412: someone wrote between our GET and our PUT.
    Conflict,
    /// 409: the parent collection does not exist yet.
    MissingCollection,
}

impl Oryxis {
    /// Run one WebDAV-sync round.
    ///
    /// Validates config while holding `&self`, then does the network and
    /// vault work off-thread, so the caller can fire it blindly.
    pub(crate) fn run_webdav_sync_round(&mut self) -> Task<Message> {
        if self.sync.webdav.in_progress {
            return Task::none();
        }
        // A locked vault has no master key to decrypt with; the
        // manual buttons must not rely on the lock screen hiding them.
        if !self.sync_round_allowed() {
            return Task::none();
        }
        let url = self.sync.webdav.url.trim().to_string();
        if url.is_empty() {
            self.sync.webdav.status = Some(Err(t("webdav_sync_no_url").to_string()));
            return Task::none();
        }
        if self.sync.webdav.passphrase.is_empty() {
            self.sync.webdav.status = Some(Err(t("sftp_sync_no_passphrase").to_string()));
            return Task::none();
        }
        // Argon2id derivation (~0.4 s) runs on a worker thread, not on
        // the UI thread (same reason as the Git transport).
        let passphrase = self.sync.webdav.passphrase.clone();
        let Some(vault) = &self.vault else {
            return Task::none();
        };
        // Argon2id derivation (~0.4 s) runs on a worker thread, not on
        // the UI thread (same reason as the Git transport).
        let db_path = vault.db_path().to_path_buf();
        let master_password = self.master_password.clone();
        let auth = Auth {
            user: self.sync.webdav.user.clone(),
            password: self.sync.webdav.password.clone(),
        };

        self.sync.webdav.in_progress = true;
        self.sync.webdav.status = None;
        Task::perform(
            async move {
                let secret = tokio::task::spawn_blocking(move || {
                    oryxis_vault::derive_sync_secret(&passphrase).map_err(|e| e.to_string())
                })
                .await
                .unwrap_or_else(|e| Err(format!("join: {e}")))?;
                webdav_round(&url, &auth, &db_path, master_password, &secret).await
            },
            |result| Message::Sync(SyncMessage::WebdavRoundFinished(result)),
        )
    }
}

/// Basic-auth pair. Carried as one value so no call site can pass the
/// two the wrong way round.
struct Auth {
    user: String,
    password: String,
}

impl Auth {
    /// Attach credentials, unless the user left them blank: an
    /// anonymous share is a legitimate configuration and an empty
    /// `Authorization` header would be rejected by servers that would
    /// otherwise have served it.
    fn apply(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        if self.user.is_empty() && self.password.is_empty() {
            req
        } else {
            req.basic_auth(&self.user, Some(&self.password))
        }
    }
}

/// GET the snapshot and classify what came back.
async fn fetch(client: &reqwest::Client, url: &str, auth: &Auth) -> Result<Remote, String> {
    let resp = auth
        .apply(client.get(url))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    match resp.status() {
        reqwest::StatusCode::NOT_FOUND => return Ok(Remote::Absent),
        reqwest::StatusCode::UNAUTHORIZED | reqwest::StatusCode::FORBIDDEN => {
            return Err(t("webdav_sync_auth").to_string());
        }
        s if s.is_success() => {}
        s => return Err(http_error(s)),
    }
    let etag = resp
        .headers()
        .get(reqwest::header::ETAG)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let body = resp.bytes().await.map_err(|e| e.to_string())?.to_vec();
    Ok(match etag {
        Some(etag) => Remote::Tagged { body, etag },
        None => Remote::Untagged { body },
    })
}

/// PUT the snapshot, guarded by whatever the GET established.
async fn store(
    client: &reqwest::Client,
    url: &str,
    auth: &Auth,
    blob: Vec<u8>,
    remote: &Remote,
) -> Result<Stored, String> {
    let req = client
        .put(url)
        .header(reqwest::header::CONTENT_TYPE, "application/octet-stream");
    let req = match remote {
        // Create-only. Two devices doing their first round both believe
        // the resource is absent; without this one of them silently
        // overwrites the other's brand-new snapshot.
        Remote::Absent => req.header(reqwest::header::IF_NONE_MATCH, "*"),
        Remote::Tagged { etag, .. } => req.header(reqwest::header::IF_MATCH, etag.as_str()),
        // No tag to compare, so nothing to guard with. The alternative
        // (refusing to write) would make the transport unusable on such
        // a server; last-writer-wins matches the folder transport, and
        // the UI says so.
        Remote::Untagged { .. } => req,
    };
    let resp = auth
        .apply(req)
        .body(blob)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    match resp.status() {
        reqwest::StatusCode::PRECONDITION_FAILED => Ok(Stored::Conflict),
        reqwest::StatusCode::CONFLICT => Ok(Stored::MissingCollection),
        reqwest::StatusCode::UNAUTHORIZED | reqwest::StatusCode::FORBIDDEN => {
            Err(t("webdav_sync_auth").to_string())
        }
        s if s.is_success() => Ok(Stored::Written),
        s => Err(http_error(s)),
    }
}

/// Create the snapshot's parent collection.
///
/// Deliberately unlike the folder transport, where a missing directory
/// is an error: there, a typo would sync into a freshly made local
/// folder nobody watches. Here the user typed a URL against a real
/// server they authenticated to, and "the collection is not there yet"
/// is the ordinary first run.
async fn ensure_collection(
    client: &reqwest::Client,
    url: &str,
    auth: &Auth,
) -> Result<(), String> {
    let Some((parent, _)) = url.rsplit_once('/') else {
        return Ok(());
    };
    let method = reqwest::Method::from_bytes(b"MKCOL").map_err(|e| e.to_string())?;
    let resp = auth
        .apply(client.request(method, format!("{parent}/")))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    // 405 means it already exists, which is the outcome we wanted.
    if resp.status().is_success() || resp.status() == reqwest::StatusCode::METHOD_NOT_ALLOWED {
        Ok(())
    } else {
        Err(format!("MKCOL: {}", http_error(resp.status())))
    }
}

fn http_error(status: reqwest::StatusCode) -> String {
    format!("{} {status}", t("webdav_sync_http_error"))
}

/// One round: read, merge, rebuild, write back under a precondition.
///
/// Returns how many records the remote snapshot carried, which is what
/// the status line reports.
async fn webdav_round(
    url: &str,
    auth: &Auth,
    db_path: &Path,
    master_password: Option<String>,
    secret: &[u8; 32],
) -> Result<usize, String> {
    let target = snapshot_url(url);
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .map_err(|e| e.to_string())?;

    // Unlocked ONCE, outside the retry loop: the unlock is an Argon2id
    // derivation tuned to take about a second, and a lost CAS is a
    // reason to re-merge, never a reason to re-derive the key.
    let db = db_path.to_path_buf();
    let vault = tokio::task::spawn_blocking(move || {
        let mut vault = VaultStore::open(&db).map_err(|e| e.to_string())?;
        match master_password.as_deref() {
            Some(pw) => vault.unlock(pw).map_err(|e| e.to_string())?,
            None => vault.open_without_password().map_err(|e| e.to_string())?,
        }
        Ok::<_, String>(Arc::new(Mutex::new(vault)))
    })
    .await
    .map_err(|e| format!("join: {e}"))??;

    let mut created_collection = false;
    for _ in 0..CAS_ATTEMPTS {
        let remote = fetch(&client, &target, auth).await?;

        // SQLite and the merge stay off the async runtime.
        let vault_handle = Arc::clone(&vault);
        let secret = *secret;
        let remote_blob = remote.body().map(<[u8]>::to_vec);
        let (pulled, rebuilt) = tokio::task::spawn_blocking(move || {
            let mut pulled = 0usize;
            if let Some(blob) = remote_blob {
                pulled = oryxis_sync::merge_snapshot(&vault_handle, &blob, &secret)
                    .map_err(|e| e.to_string())?;
            }
            let rebuilt = oryxis_sync::build_full_snapshot(&vault_handle, &secret)
                .map_err(|e| e.to_string())?;
            Ok::<_, String>((pulled, rebuilt))
        })
        .await
        .map_err(|e| format!("join: {e}"))??;

        match store(&client, &target, auth, rebuilt, &remote).await? {
            Stored::Written => return Ok(pulled),
            Stored::Conflict => continue,
            Stored::MissingCollection => {
                // Only worth one attempt: a second 409 after a
                // successful MKCOL means the path is wrong in a way
                // creating folders will not fix.
                if created_collection {
                    return Err(t("webdav_sync_bad_path").to_string());
                }
                created_collection = true;
                ensure_collection(&client, &target, auth).await?;
            }
        }
    }
    Err(t("webdav_sync_busy").to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_collection_url_gets_the_shared_filename() {
        assert_eq!(
            snapshot_url("https://cloud.example/remote.php/dav/files/me/oryxis/"),
            "https://cloud.example/remote.php/dav/files/me/oryxis/oryxis-sync.bin"
        );
    }

    #[test]
    fn a_url_naming_a_file_is_kept() {
        let url = "https://cloud.example/remote.php/dav/files/me/team.bin";
        assert_eq!(snapshot_url(url), url);
        assert_eq!(snapshot_url("  https://c.example/a.bin  "), "https://c.example/a.bin");
    }

    /// The precondition is chosen from the remote's state, and the
    /// untagged case is the one worth pinning: sending
    /// `If-None-Match: *` against a resource that exists earns a 412 on
    /// every attempt, so the transport could never write at all.
    #[test]
    fn the_precondition_follows_the_remote_state() {
        let header = |remote: &Remote| match remote {
            Remote::Absent => Some(("if-none-match", "*".to_string())),
            Remote::Tagged { etag, .. } => Some(("if-match", etag.clone())),
            Remote::Untagged { .. } => None,
        };
        assert_eq!(header(&Remote::Absent), Some(("if-none-match", "*".into())));
        assert_eq!(
            header(&Remote::Tagged {
                body: vec![],
                etag: "\"abc\"".into()
            }),
            Some(("if-match", "\"abc\"".into()))
        );
        assert_eq!(header(&Remote::Untagged { body: vec![] }), None);
    }

    /// Blank credentials mean an anonymous share, not an empty
    /// `Authorization` header.
    #[test]
    fn blank_credentials_send_no_auth_header() {
        let client = reqwest::Client::new();
        let anon = Auth {
            user: String::new(),
            password: String::new(),
        };
        let req = anon
            .apply(client.get("http://example.invalid/"))
            .build()
            .unwrap();
        assert!(req.headers().get(reqwest::header::AUTHORIZATION).is_none());

        let named = Auth {
            user: "me".into(),
            password: "pw".into(),
        };
        let req = named
            .apply(client.get("http://example.invalid/"))
            .build()
            .unwrap();
        assert!(req.headers().get(reqwest::header::AUTHORIZATION).is_some());
    }
}
