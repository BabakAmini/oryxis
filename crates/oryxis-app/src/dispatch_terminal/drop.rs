//! OS drag-and-drop onto the terminal (#106): buffering, routing, and
//! the SFTP half of the upload.
//!
//! `FileDropped` is a global window event that `subscription.rs` funnels
//! into `SftpMessage::SftpFileDropped`; the SFTP surface keeps first
//! claim on it, and this module is the fallback when that surface is not
//! visible (`sftp_surface_visible()` false, or no SFTP tab open at all).
//! A hybrid Files tab therefore keeps the full dual-pane drop behavior:
//! it IS the SFTP surface while shown.
//!
//! Routing, per target pane:
//!
//! | Transport      | Shell cwd (OSC 7)  | Files       | Folders        |
//! |----------------|--------------------|-------------|----------------|
//! | SSH            | exact              | SFTP to cwd | SFTP to cwd    |
//! | SSH            | unknown/heuristic  | ZMODEM      | toast, skipped |
//! | Telnet/Serial  | n/a                | ZMODEM      | toast, skipped |
//! | local shell    | n/a                | paste path  | paste path     |
//!
//! The OSC 7 gate is strict on purpose: the title-parse cwd fallback is
//! a heuristic and often `~`-relative, and a guessed directory means
//! uploading to the wrong place. ZMODEM's `rz` lands in the shell's real
//! cwd without us knowing it, which is exactly why it stays the fallback
//! for files. Folders can't ride ZMODEM at all (the protocol streams
//! file contents, it cannot create directory trees).
//!
//! The ZMODEM half is intentionally thin here: stash the sources on the
//! pane, type `rz -y`, and let the existing detector flow
//! (`begin_zmodem_transfer`) consume them instead of opening the file
//! picker. The transfer only ever starts when the detector has SEEN the
//! remote receiver start, so the detect-timeout can never abort a
//! running transfer; it merely clears the stash when `rz` demonstrably
//! never ran (#106's fixed 1-second watchdog aborted every upload that
//! outlived it, and mislabeled the abort as "lrzsz not installed").

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use iced::Task;
use iced::futures::SinkExt;
use uuid::Uuid;

use crate::app::{Message, Oryxis, TerminalMessage, ZmodemMessage};
use crate::state::{DropProgress, DropUploadPane, PromptState};

/// Debounce that coalesces a multi-file OS drop (delivered as one
/// `FileDropped` per file) into a single flush, mirroring the SFTP
/// panel's own burst window.
const DROP_FLUSH_DEBOUNCE_MS: u64 = 150;

/// How long the ZMODEM detector gets to see the remote `rz` start after
/// the app types `rz -y`. This bounds one command round trip (echo +
/// spawn), never any transfer, so a generous value costs nothing: the
/// only thing that happens at expiry is clearing the stash and telling
/// the user why.
const RZ_DETECT_TIMEOUT_SECS: u64 = 4;

impl Oryxis {
    /// Buffer one dropped path; the first file of the gesture arms the
    /// debounce flush. Called from the SFTP drop handler's fallback
    /// paths, so it only ever sees drops the SFTP surface declined.
    pub(crate) fn buffer_terminal_drop(&mut self, path: PathBuf) -> Task<Message> {
        let first = self.pending_terminal_drops.is_empty();
        self.pending_terminal_drops.push(path);
        if !first {
            return Task::none();
        }
        Task::perform(
            async {
                tokio::time::sleep(std::time::Duration::from_millis(DROP_FLUSH_DEBOUNCE_MS)).await;
            },
            |_| Message::Terminal(TerminalMessage::TerminalDropFlush),
        )
    }

    /// Route the buffered drop to the pane under the cursor. See the
    /// module docs for the routing table.
    pub(crate) fn handle_terminal_drop_flush(&mut self) -> Task<Message> {
        let paths = std::mem::take(&mut self.pending_terminal_drops);
        if paths.is_empty() {
            return Task::none();
        }
        // Only a visible terminal accepts drops: with the vault (or any
        // other view) on screen there is no pane under the cursor, and
        // uploading into a BACKGROUND tab is the #106 behavior this
        // replaces. The SFTP surfaces already claimed their drops
        // upstream.
        if self.active_view != crate::state::View::Terminal {
            return Task::none();
        }
        let Some(tab_idx) = self.active_tab else {
            return Task::none();
        };
        let Some(tab) = self.tabs.get(tab_idx) else {
            return Task::none();
        };
        let tab_id = tab._id;
        // The pane whose last-drawn canvas rect contains the cursor,
        // falling back to the focused pane: `FileDropped` carries no
        // position, and some platforms stop delivering CursorMoved while
        // the OS owns the drag, so the last known position can be stale.
        // The fallback is never worse than what #106 shipped (it always
        // used the focused pane).
        let rects: Vec<(Uuid, iced::Rectangle)> = tab
            .pane_grid
            .panes
            .values()
            .map(|p| (p.id, p.bounds.get()))
            .collect();
        let pane_id =
            pane_under(self.mouse_position, &rects).unwrap_or_else(|| tab.active().id);
        let Some(pane) = tab.pane_grid.panes.values().find(|p| p.id == pane_id) else {
            return Task::none();
        };

        // One transfer per pane at a time. The running one's card is on
        // screen, so the state is visible rather than silently dropped.
        if pane.zmodem.is_some() || pane.drop_upload.is_some() || !pane.pending_drop_sources.is_empty() {
            return Task::none();
        }

        // Broken symlinks and unreadable entries ride the FILE branch on
        // purpose: the transport's own open() error is the honest report,
        // where #106's `is_file()` partition silently misfiled them as
        // folders.
        let (dirs, files): (Vec<PathBuf>, Vec<PathBuf>) =
            paths.into_iter().partition(|p| p.is_dir());

        let Some(session) = pane.session.as_ref() else {
            // Local shell: paste the quoted paths, the convention every
            // terminal follows for drops. Riding the paste path keeps
            // bracketed paste and the paste guards.
            let line = typed_drop_line(&files, &dirs);
            if !line.is_empty() {
                self.paste_text_into_tab(tab_id, &line);
            }
            return Task::none();
        };

        // Sidebar Files hybrid: with the browser on screen, its current
        // directory is what the user is LOOKING at (manual navigation
        // included), and the mounted client is proof SFTP works, so it
        // outranks the shell cwd. Focused pane only: the sidebar renders
        // the FOCUSED pane's browser, so for any other pane of a split
        // that directory is not the one on screen.
        let sidebar_dir = (pane_id == tab.active().id
            && self.effective_sidebar_tab() == Some(crate::state::TerminalSidebarTab::Files)
            && pane.files.client.is_some()
            && !pane.files.path.is_empty())
        .then(|| pane.files.path.clone());

        // SSH with a visible browser directory or an exact shell cwd:
        // SFTP, files and folders alike, on a subsystem channel of the
        // live handle. Out-of-band: nothing is typed, so this is safe
        // even with a full-screen program up.
        if let Some(ssh) = session.ssh()
            && let Some(dest) = sidebar_dir
                .or_else(|| pane.cwd_from_osc7.then(|| pane.cwd.clone()).flatten())
        {
            let ssh = Arc::clone(ssh);
            return self.begin_drop_sftp_upload(pane_id, ssh, dest, files, dirs);
        }

        // ZMODEM fallback (SSH without exact cwd, Telnet, serial).
        let mut toast: Option<&'static str> = None;
        if !dirs.is_empty() {
            // rz cannot create directory trees; say so instead of
            // vanishing (the files of a mixed drop still go).
            toast = Some("terminal_drop_folder_no_cwd");
        }
        if files.is_empty() {
            if let Some(key) = toast {
                self.set_toast(crate::i18n::t(key).to_string());
            }
            return Task::none();
        }
        // `rz -y` is a typed command: refuse while a command is known to
        // be running, or the line lands inside vim/less/htop. NoIntegration
        // stays permissive: without OSC 133 there is no busy signal, and
        // refusing would break every host without shell integration.
        if matches!(pane.prompt, PromptState::Busy) {
            self.set_toast(crate::i18n::t("terminal_drop_busy").to_string());
            return Task::none();
        }
        let wire = session.write_sender();
        if let Some(key) = toast {
            self.set_toast(crate::i18n::t(key).to_string());
        }
        if let Some(pane) = self.pane_by_id_mut(pane_id) {
            pane.pending_drop_sources = files;
        }
        // Straight to the transport's write sender, NOT
        // `write_input_to_tab`: broadcast input fans that out to every
        // pane in the tab, which would start rz on hosts nobody dropped
        // onto.
        let _ = wire.send(b"rz -y\r".to_vec());
        Task::perform(
            async {
                tokio::time::sleep(std::time::Duration::from_secs(RZ_DETECT_TIMEOUT_SECS)).await;
            },
            move |_| Message::Zmodem(ZmodemMessage::ZmodemDropRzTimeout(pane_id)),
        )
    }

    /// Start the SFTP half: pre-walk the sources (so the total is known
    /// up front), then upload sequentially on a fresh subsystem channel,
    /// streaming progress back as messages. Top-level entries get
    /// collision-free names via `unique_name_in_remote_dir`, the same
    /// never-clobber rule the SFTP panel's folder upload uses; children
    /// live under a fresh root, so they cannot conflict.
    fn begin_drop_sftp_upload(
        &mut self,
        pane_id: Uuid,
        ssh: Arc<oryxis_ssh::SshSession>,
        dest_dir: String,
        files: Vec<PathBuf>,
        dirs: Vec<PathBuf>,
    ) -> Task<Message> {
        let abort = Arc::new(AtomicBool::new(false));
        if let Some(pane) = self.pane_by_id_mut(pane_id) {
            pane.drop_upload = Some(DropUploadPane {
                file_name: None,
                batch: None,
                transferred: 0,
                total: None,
                abort: Arc::clone(&abort),
                dest_dir: dest_dir.clone(),
            });
        } else {
            return Task::none();
        }

        let stream = iced::stream::channel::<Message>(
            64,
            move |out: iced::futures::channel::mpsc::Sender<Message>| async move {
                // Every exit path reports exactly one terminal event;
                // the handler clears the card and toasts from it.
                let send = |p: DropProgress| {
                    let msg = Message::Terminal(TerminalMessage::TerminalDropProgress(pane_id, p));
                    let mut out = out.clone();
                    async move { out.send(msg).await.is_ok() }
                };

                let client = match ssh.open_sftp().await {
                    Ok(c) => c,
                    Err(e) => {
                        send(DropProgress::Failed(e.to_string())).await;
                        return;
                    }
                };

                // Top-level plan: (source, unique remote destination,
                // is_dir), then the recursive queue per folder.
                let mut entries: Vec<(PathBuf, String, bool)> = Vec::new();
                for (src, is_dir) in files
                    .iter()
                    .map(|f| (f.clone(), false))
                    .chain(dirs.iter().map(|d| (d.clone(), true)))
                {
                    let Some(base) = src.file_name().map(|s| s.to_string_lossy().into_owned())
                    else {
                        continue;
                    };
                    let unique = match crate::sftp_helpers::unique_name_in_remote_dir(
                        &client, &dest_dir, &base,
                    )
                    .await
                    {
                        Ok(u) => u,
                        Err(e) => {
                            send(DropProgress::Failed(e)).await;
                            return;
                        }
                    };
                    let dst = crate::sftp_helpers::remote_join(&dest_dir, &unique);
                    entries.push((src, dst, is_dir));
                }
                if entries.is_empty() {
                    send(DropProgress::Done).await;
                    return;
                }

                // Expand folders into their full item queues now, so the
                // byte total covers everything before the first write.
                // (top-level display name, items in parent-first order)
                let mut plans: Vec<(String, Vec<crate::state::TransferItem>)> = Vec::new();
                let mut total: u64 = 0;
                for (src, dst, is_dir) in entries {
                    let name = dst.rsplit('/').next().unwrap_or(&dst).to_string();
                    let mut items = Vec::new();
                    if is_dir {
                        items.push(crate::state::TransferItem {
                            src: src.to_string_lossy().into_owned(),
                            dst: dst.clone(),
                            is_dir: true,
                            size: None,
                        });
                        let mut queue = std::collections::VecDeque::new();
                        if let Err(e) =
                            crate::sftp_helpers::walk_local_for_upload(&src, &dst, &mut queue)
                        {
                            send(DropProgress::Failed(e)).await;
                            return;
                        }
                        total += queue.iter().filter_map(|i| i.size).sum::<u64>();
                        items.extend(queue);
                    } else {
                        let size = tokio::fs::metadata(&src).await.map(|m| m.len()).ok();
                        total += size.unwrap_or(0);
                        items.push(crate::state::TransferItem {
                            src: src.to_string_lossy().into_owned(),
                            dst,
                            is_dir: false,
                            size,
                        });
                    }
                    plans.push((name, items));
                }
                if !send(DropProgress::Plan { total }).await {
                    return;
                }

                let of = plans.len();
                let mut completed: u64 = 0;
                for (index, (name, items)) in plans.into_iter().enumerate() {
                    if !send(DropProgress::Entry {
                        name,
                        index: index + 1,
                        of,
                    })
                    .await
                    {
                        return;
                    }
                    for item in items {
                        if abort.load(Ordering::Relaxed) {
                            send(DropProgress::Cancelled).await;
                            return;
                        }
                        if item.is_dir {
                            // Merge-tolerant like the panel's uploader: a
                            // racing mkdir is fine, anything else poisons
                            // every child below it, so surface it now.
                            if let Err(e) = client.create_dir(&item.dst).await
                                && client.list_dir(&item.dst).await.is_err()
                            {
                                send(DropProgress::Failed(e.to_string())).await;
                                return;
                            }
                            continue;
                        }
                        let counter = Arc::new(AtomicU64::new(0));
                        let src = PathBuf::from(&item.src);
                        let mut up = Box::pin(client.upload_from_progress(
                            &src,
                            &item.dst,
                            Some(Arc::clone(&counter)),
                        ));
                        let mut tick =
                            tokio::time::interval(std::time::Duration::from_millis(120));
                        let finished = loop {
                            tokio::select! {
                                r = &mut up => break r,
                                _ = tick.tick() => {
                                    if abort.load(Ordering::Relaxed) {
                                        // Kill the in-flight write and
                                        // sweep the partial so a cancel
                                        // never masquerades as a
                                        // complete file.
                                        drop(up);
                                        let _ = client.remove_file(&item.dst).await;
                                        send(DropProgress::Cancelled).await;
                                        return;
                                    }
                                    if !send(DropProgress::Advanced {
                                        transferred: completed + counter.load(Ordering::Relaxed),
                                    })
                                    .await
                                    {
                                        return;
                                    }
                                }
                            }
                        };
                        if let Err(e) = finished {
                            send(DropProgress::Failed(format!("{}: {e}", item.dst))).await;
                            return;
                        }
                        completed += item.size.unwrap_or_else(|| counter.load(Ordering::Relaxed));
                        if !send(DropProgress::Advanced {
                            transferred: completed,
                        })
                        .await
                        {
                            return;
                        }
                    }
                }
                send(DropProgress::Done).await;
            },
        );
        Task::stream(stream)
    }

    /// Apply a streamed drop-upload event to the pane's card; terminal
    /// events clear it and toast the outcome.
    pub(crate) fn handle_terminal_drop_progress(
        &mut self,
        pane_id: Uuid,
        progress: DropProgress,
    ) -> Task<Message> {
        let mut toast: Option<String> = None;
        let mut refresh: Option<Task<Message>> = None;
        if let Some(pane) = self.pane_by_id_mut(pane_id) {
            match progress {
                DropProgress::Plan { total } => {
                    if let Some(up) = pane.drop_upload.as_mut() {
                        up.total = Some(total);
                    }
                }
                DropProgress::Entry { name, index, of } => {
                    if let Some(up) = pane.drop_upload.as_mut() {
                        up.file_name = Some(name);
                        // A single-entry drop needs no "(1/1)" noise,
                        // matching the ZMODEM card's single-file rule.
                        up.batch = (of > 1).then_some((index, of));
                    }
                }
                DropProgress::Advanced { transferred } => {
                    if let Some(up) = pane.drop_upload.as_mut() {
                        up.transferred = transferred;
                    }
                }
                DropProgress::Done => {
                    let up = pane.drop_upload.take();
                    toast = Some(crate::i18n::t("zmodem_complete").to_string());
                    // Sidebar Files showing the directory we just landed
                    // in: re-list it so the new entries appear without a
                    // manual refresh. Pinned to this pane's browser (not
                    // `SidebarFilesRefresh`, which acts on the ACTIVE
                    // pane: focus can have moved during a long upload).
                    if let Some(up) = up
                        && let Some(client) = pane.files.client.clone()
                        && pane.files.path == up.dest_dir
                    {
                        pane.files.loading = true;
                        let seq = pane.files.next_req();
                        refresh = Some(crate::dispatch_sidebar_files::list_dir_task(
                            client,
                            up.dest_dir,
                            pane_id,
                            seq,
                        ));
                    }
                }
                DropProgress::Cancelled => {
                    pane.drop_upload = None;
                    toast = Some(crate::i18n::t("zmodem_cancelled").to_string());
                }
                DropProgress::Failed(e) => {
                    pane.drop_upload = None;
                    toast = Some(format!("{}: {e}", crate::i18n::t("zmodem_failed")));
                }
            }
        }
        if let Some(t) = toast {
            self.set_toast(t);
        }
        refresh.unwrap_or_else(Task::none)
    }
}

/// The pane whose last-drawn rect contains `point`. `None` when the
/// point misses every pane (stale cursor position, drop outside the
/// grid), which the caller resolves to the focused pane.
fn pane_under(point: iced::Point, rects: &[(Uuid, iced::Rectangle)]) -> Option<Uuid> {
    rects
        .iter()
        .find(|(_, r)| r.contains(point))
        .map(|(id, _)| *id)
}

/// The line a local-shell drop pastes: every path shell-quoted, space
/// separated, with a trailing space so the user can keep typing. Paths
/// come from the OS, so quoting here is convenience (spaces, quotes),
/// not a security boundary; a path whose name defeats quoting (a line
/// break, on POSIX) is skipped rather than typed broken.
fn typed_drop_line(files: &[PathBuf], dirs: &[PathBuf]) -> String {
    let mut line = String::new();
    for p in files.iter().chain(dirs.iter()) {
        let raw = p.display().to_string();
        #[cfg(windows)]
        let quoted = if raw.contains(' ') {
            // Windows paths cannot contain `"`, so plain wrapping is
            // exact for any real path.
            Some(format!("\"{raw}\""))
        } else {
            Some(raw)
        };
        #[cfg(not(windows))]
        let quoted = if raw
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '.' | '-' | '_' | '~' | '+'))
        {
            Some(raw)
        } else {
            oryxis_archive::quote::sh_quote(&raw).ok()
        };
        if let Some(q) = quoted {
            line.push_str(&q);
            line.push(' ');
        }
    }
    line
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: f32, y: f32, w: f32, h: f32) -> iced::Rectangle {
        iced::Rectangle::new(iced::Point::new(x, y), iced::Size::new(w, h))
    }

    #[test]
    fn pane_under_picks_the_containing_rect() {
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let rects = vec![(a, rect(0.0, 0.0, 100.0, 100.0)), (b, rect(100.0, 0.0, 100.0, 100.0))];
        assert_eq!(pane_under(iced::Point::new(50.0, 50.0), &rects), Some(a));
        assert_eq!(pane_under(iced::Point::new(150.0, 50.0), &rects), Some(b));
        // A miss (stale cursor, drop on the chrome) resolves to None so
        // the caller falls back to the focused pane.
        assert_eq!(pane_under(iced::Point::new(250.0, 50.0), &rects), None);
    }

    #[cfg(not(windows))]
    #[test]
    fn typed_drop_line_quotes_what_needs_it() {
        let plain = PathBuf::from("/tmp/plain-1.txt");
        let spaced = PathBuf::from("/tmp/with space.txt");
        let quoted = PathBuf::from("/tmp/it's.txt");
        let line = typed_drop_line(&[plain, spaced, quoted], &[]);
        assert_eq!(
            line,
            "/tmp/plain-1.txt '/tmp/with space.txt' '/tmp/it'\\''s.txt' "
        );
        // A newline defeats POSIX single quotes in csh-family shells;
        // the path is skipped, never typed broken.
        let broken = PathBuf::from("/tmp/a\nb");
        assert_eq!(typed_drop_line(&[broken], &[]), "");
    }
}
