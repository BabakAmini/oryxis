//! Starting a transfer for ONE item: upload, download (including the
//! "download to..." destination pick) and local duplicate.
//!
//! Each of these ends by handing a queue to the runner in `queue`, or by
//! raising a conflict for `conflict` to answer. Nothing here loops.

#![allow(clippy::result_large_err)]

use iced::Task;

use crate::app::{SftpMessage, Message, Oryxis};
use crate::sftp_helpers::{
    parent_path, remote_cp, remote_join, unique_name_in_local_dir,
    unique_name_in_remote_dir, upload_one, UploadOutcome,
};
use super::SftpSides;

impl Oryxis {
    pub(super) fn handle_sftp_single(
        &mut self,
        message: SftpMessage,
        sides: SftpSides,
    ) -> Result<Task<Message>, SftpMessage> {
        let SftpSides { remote: remote_side, local: local_side, owner: _ } = sides;
        match message {
            SftpMessage::SftpUpload(local_path) => {
                self.sftp.row_menu = None;
                if self.sftp_upload_blocked_by_zip(remote_side) {
                    return Ok(Task::none());
                }
                let Some(client) = self.sftp.pane(remote_side).client.clone() else {
                    self.sftp.pane_mut(remote_side).error = Some(crate::i18n::t("sftp_not_connected").to_string());
                    return Ok(Task::none());
                };
                let remote_dir = self
                    .sftp
                    .upload_dest_override
                    .take()
                    .unwrap_or_else(|| self.sftp.pane(remote_side).remote_path.clone());
                let temp_name = self.prefs.sftp_upload_temp_name;
                Ok(Task::perform(
                    async move {
                        let basename = local_path
                            .file_name()
                            .and_then(|s| s.to_str())
                            .ok_or_else(|| "invalid filename".to_string())?
                            .to_string();
                        let entries = client
                            .list_dir(&remote_dir)
                            .await
                            .map_err(|e| e.to_string())?;
                        // Existence check up front: hand back to the
                        // user via overwrite modal if the name is taken,
                        // otherwise stream the file and finish silently.
                        let conflict = entries.iter().find(|e| e.name == basename);
                        if let Some(existing) = conflict {
                            let src_size = tokio::fs::metadata(&local_path)
                                .await
                                .map(|m| m.len())
                                .unwrap_or(0);
                            return Ok::<UploadOutcome, String>(UploadOutcome::Conflict(
                                crate::state::OverwritePrompt {
                                    src: local_path.to_string_lossy().into_owned(),
                                    dst_dir: remote_dir,
                                    basename,
                                    src_size,
                                    dst_size: existing.size,
                                    direction: crate::state::OverwriteDirection::Upload,
                                    multi: false,
                                    apply_to_all: false,
                                },
                            ));
                        }
                        let target = remote_join(&remote_dir, &basename);
                        upload_one(&client, &local_path, &target, temp_name, None).await?;
                        Ok(UploadOutcome::Done(remote_dir))
                    },
                    move |result| match result {
                        Ok(UploadOutcome::Done(reload)) => {
                            Message::Sftp(SftpMessage::SftpNavigateRemote(remote_side, reload))
                        }
                        Ok(UploadOutcome::Conflict(prompt)) => Message::Sftp(SftpMessage::SftpAskOverwrite(prompt)),
                        Err(e) => Message::Sftp(SftpMessage::SftpOpResult(remote_side, e, true)),
                    },
                ))
            }
            SftpMessage::SftpDownload(remote_path) => {
                self.sftp.row_menu = None;
                if let Some(ask) = self.sftp_ask_download_dir(SftpMessage::SftpDownload(
                    remote_path.clone(),
                )) {
                    return Ok(ask);
                }
                let Some(client) = self.sftp.pane(remote_side).client.clone() else {
                    self.sftp.pane_mut(remote_side).error = Some(crate::i18n::t("sftp_not_connected").to_string());
                    return Ok(Task::none());
                };
                let local_dir = self
                    .sftp
                    .download_dest_override
                    .take()
                    .unwrap_or_else(|| self.sftp.pane(local_side).local_path.clone());
                Ok(Task::perform(
                    async move {
                        let basename = remote_path
                            .rsplit('/')
                            .find(|s| !s.is_empty())
                            .unwrap_or(&remote_path)
                            .to_string();
                        let target = local_dir.join(&basename);
                        // A name already taken is the user's call, not
                        // ours: same four answers the upload side offers,
                        // including Duplicate, which is what this path
                        // used to do silently.
                        if let Ok(existing) = tokio::fs::metadata(&target).await {
                            let src_size = client
                                .stat(&remote_path)
                                .await
                                .map(|s| s.size)
                                .unwrap_or(0);
                            return Ok::<_, String>(Some(crate::state::OverwritePrompt {
                                src: remote_path,
                                dst_dir: local_dir.to_string_lossy().into_owned(),
                                basename,
                                src_size,
                                dst_size: existing.len(),
                                direction: crate::state::OverwriteDirection::Download,
                                multi: false,
                                apply_to_all: false,
                            }));
                        }
                        client
                            // Single file: one extra stat is negligible.
                            .download_to(&remote_path, &target, None)
                            .await
                            .map_err(|e| e.to_string())?;
                        Ok(None)
                    },
                    move |result| match result {
                        Ok(None) => Message::Sftp(SftpMessage::SftpRefreshLocal(local_side)),
                        Ok(Some(prompt)) => Message::Sftp(SftpMessage::SftpAskOverwrite(prompt)),
                        Err(e) => Message::Sftp(SftpMessage::SftpOpResult(remote_side, e, true)),
                    },
                ))
            }
            SftpMessage::SftpDownloadTo(then) => {
                self.sftp.row_menu = None;
                // Explicit ask, so an override left over from a drop onto
                // a folder must not short-circuit it.
                self.sftp.download_dest_override = None;
                Ok(self.sftp_pick_download_dir(*then))
            }
            SftpMessage::SftpDownloadDestPicked(dir, then) => {
                let Some(dir) = dir else {
                    // Cancelled: nothing was touched, in particular the
                    // override stays unset so the next download still asks.
                    return Ok(Task::none());
                };
                self.sftp.download_dest_override = Some(dir);
                Ok(Task::done(Message::Sftp(*then)))
            }
            SftpMessage::SftpDuplicate(side, path) => {
                self.sftp.row_menu = None;
                if !self.sftp.pane(side).is_remote {
                        let src = std::path::PathBuf::from(&path);
                        let parent = match src.parent() {
                            Some(p) => p.to_path_buf(),
                            None => {
                                self.sftp.pane_mut(side).error = Some("Cannot duplicate root".into());
                                return Ok(Task::none());
                            }
                        };
                        let basename = src
                            .file_name()
                            .and_then(|s| s.to_str())
                            .unwrap_or("untitled")
                            .to_string();
                        let unique = unique_name_in_local_dir(&parent, &basename);
                        let dest = parent.join(&unique);
                        // The copy can be multi-GB; run it off the event
                        // loop instead of freezing update() for the
                        // duration, mirroring the remote branch below.
                        Ok(Task::perform(
                            tokio::task::spawn_blocking(move || std::fs::copy(&src, &dest)),
                            move |res| match res {
                                Ok(Ok(_)) => Message::Sftp(SftpMessage::SftpRefreshLocal(side)),
                                Ok(Err(e)) => Message::Sftp(SftpMessage::SftpOpResult(side, format!("copy: {e}"), true)),
                                Err(e) => Message::Sftp(SftpMessage::SftpOpResult(side, format!("copy: {e}"), true)),
                            },
                        ))
                } else {
                        let Some(client) = self.sftp.pane(side).client.clone() else {
                            return Ok(Task::none());
                        };
                        let parent = parent_path(&path);
                        let basename = path
                            .rsplit('/')
                            .find(|s| !s.is_empty())
                            .unwrap_or(&path)
                            .to_string();
                        let reload = self.sftp.pane(side).remote_path.clone();
                        let src = path.clone();
                        Ok(Task::perform(
                            async move {
                                let unique =
                                    unique_name_in_remote_dir(&client, &parent, &basename)
                                        .await?;
                                let dest = remote_join(&parent, &unique);
                                // `cp -- src dst`, same exec channel trick
                                // we used for `rm -rf`. Using -- prevents
                                // dashes in names from being parsed as flags.
                                remote_cp(&client, &src, &dest, false).await?;
                                Ok::<String, String>(reload)
                            },
                            move |result| match result {
                                Ok(reload) => Message::Sftp(SftpMessage::SftpNavigateRemote(side, reload)),
                                Err(e) => Message::Sftp(SftpMessage::SftpOpResult(side, e, true)),
                            },
                        ))
                }
            }
            // Every arm above returns, so the match IS the return
            // value here rather than a statement before one.
            m => Err(m),
        }
    }
}
