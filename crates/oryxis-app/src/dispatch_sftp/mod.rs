//! `Oryxis::handle_sftp`, match arms for the SFTP pane: navigation,
//! filtering, transfers (upload/download/duplicate), property edits,
//! row interactions, drag-and-drop, edit-in-place. The single biggest
//! domain in the dispatch table.
//!
//! Pane operations are side-addressed: a `SftpPaneSide` (Left / Right)
//! names which pane, and the handler branches on `pane(side).is_remote`
//! to choose filesystem vs SFTP behaviour, so either pane can be Local
//! or a remote host.
//!
//! The router fans `Message` variants out to per-area submodules:
//!
//! - `hosts`    : mount / connect / session-reuse / remount / retry,
//!   plus the host picker.
//! - `tabs`     : SFTP tab lifecycle (select / close / pin / menu).
//! - `listing`  : navigation + listings (remote / local / up), the
//!   path-bar edit flow, sort / filter / hidden toggles.
//! - `layout`   : pane menus, column toggles / resize / auto-fit,
//!   split and log resizes.
//! - `entries`  : rename / delete / new-entry flows.
//! - `selection`: row clicks / selection, drag arming, type-ahead and
//!   the SFTP keyboard handling.

#![allow(clippy::result_large_err)]

mod entries;
mod hosts;
mod layout;
mod listing;
mod selection;
mod tabs;

use iced::Task;

use crate::app::{Message, Oryxis};
use crate::sftp_helpers::parent_path;

/// First listing of a freshly mounted SFTP client: try the caller's
/// preferred directory (the sidebar Files promotion, a saved path)
/// first, falling back to the home directory when it doesn't resolve
/// or list (deleted, no permission), so a stale hint degrades to the
/// normal mount instead of an error.
pub(crate) async fn initial_remote_listing(
    client: &oryxis_ssh::SftpClient,
    hint: Option<String>,
) -> Result<(String, Vec<oryxis_ssh::SftpEntry>), String> {
    if let Some(h) = hint
        && let Ok(path) = client.canonicalize(&h).await
        && let Ok(entries) = client.list_dir(&path).await
    {
        return Ok((path, entries));
    }
    let path = client.canonicalize(".").await.unwrap_or_else(|_| "/".to_string());
    let entries = client.list_dir(&path).await.map_err(|e| e.to_string())?;
    Ok((path, entries))
}

impl Oryxis {
    /// Apply the in-progress inline rename (Enter, or a click outside the
    /// input). Logs on success; a remote rename runs async and re-lists the
    /// directory via `SftpRenamed`. No-op when nothing is being renamed or
    /// the new name is blank. Does not touch `swallow_next_activate` (the
    /// keyboard-commit path sets that itself).
    fn commit_rename(&mut self) -> Task<Message> {
        let Some(rn) = self.sftp.rename.take() else {
            return Task::none();
        };
        let new_name = rn.input.trim().to_string();
        // One plain path component (rejects empty, ".", ".." and
        // separators): "." / ".." would rename onto the directory itself
        // or its parent, a separator would silently relocate the entry.
        if !crate::sftp_helpers::is_safe_remote_entry_name(&new_name) {
            return Task::none();
        }
        // Unchanged name: close the editor silently. The commit also fires
        // on any click outside the input, and a remote SSH_FXP_RENAME onto
        // its own path fails with SSH_FX_FAILURE (the target exists), so
        // without this a no-op edit surfaces a spurious "Failure" error.
        let unchanged = if self.sftp.pane(rn.side).is_remote {
            rn.original_path.rsplit('/').next() == Some(new_name.as_str())
        } else {
            std::path::Path::new(&rn.original_path)
                .file_name()
                .is_some_and(|n| n == std::ffi::OsStr::new(&new_name))
        };
        if unchanged {
            return Task::none();
        }
        if !self.sftp.pane(rn.side).is_remote {
            let original = std::path::PathBuf::from(&rn.original_path);
            let Some(parent) = original.parent().map(|p| p.to_path_buf()) else {
                self.sftp.pane_mut(rn.side).error = Some("Cannot rename root".into());
                return Task::none();
            };
            let dest = parent.join(&new_name);
            match std::fs::rename(&original, &dest) {
                Ok(()) => self.push_sftp_log(
                    crate::state::SftpLogLevel::Ok,
                    format!("{} {}", crate::i18n::t("sftp_log_renamed"), new_name),
                ),
                Err(e) => self.sftp.pane_mut(rn.side).error = Some(e.to_string()),
            }
            self.refresh_sftp_local(rn.side);
            Task::none()
        } else {
            let Some(client) = self.sftp.pane(rn.side).client.clone() else {
                return Task::none();
            };
            let parent = parent_path(&rn.original_path);
            let dest = if parent == "/" {
                format!("/{}", new_name)
            } else {
                format!("{}/{}", parent.trim_end_matches('/'), new_name)
            };
            let from = rn.original_path;
            let side = rn.side;
            let reload_path = self.sftp.pane(side).remote_path.clone();
            Task::perform(
                async move { client.rename(&from, &dest).await.map_err(|e| e.to_string()) },
                move |result| match result {
                    Ok(()) => Message::SftpRenamed(side, reload_path.clone(), new_name.clone()),
                    Err(e) => Message::SftpOpResult(side, e, true),
                },
            )
        }
    }

    /// Dispatch an SFTP `Message` to the matching submodule handler.
    /// Each submodule returns `Err(message)` for variants it doesn't
    /// handle so the chain falls through to the next; the final `Err`
    /// propagates back to `dispatch::update` so the other handlers
    /// (or the inline match) get their turn.
    pub(crate) fn handle_sftp(
        &mut self,
        message: Message,
    ) -> Result<Task<Message>, Message> {
        let message = match self.handle_sftp_hosts(message) {
            Ok(task) => return Ok(task),
            Err(m) => m,
        };
        let message = match self.handle_sftp_tabs(message) {
            Ok(task) => return Ok(task),
            Err(m) => m,
        };
        let message = match self.handle_sftp_listing(message) {
            Ok(task) => return Ok(task),
            Err(m) => m,
        };
        let message = match self.handle_sftp_layout(message) {
            Ok(task) => return Ok(task),
            Err(m) => m,
        };
        let message = match self.handle_sftp_entries(message) {
            Ok(task) => return Ok(task),
            Err(m) => m,
        };
        let message = match self.handle_sftp_selection(message) {
            Ok(task) => return Ok(task),
            Err(m) => m,
        };
        Err(message)
    }
}
