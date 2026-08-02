//! In-terminal ZMODEM transfer wiring (the app half of `oryxis-zmodem`).
//!
//! The detector lives in the `PtyOutput` path (`dispatch_terminal.rs`);
//! this module owns starting a transfer once detected, streaming its
//! progress back as messages, and tearing the divert down when it ends.
//!
//! Divert model: while `pane.zmodem` is `Some`, `PtyOutput` for the pane
//! is routed into the driver's wire channel instead of the emulator, and
//! keyboard input is suppressed. The driver writes protocol replies
//! straight to the pane transport's input sender (where a keystroke
//! would go). Exactly one terminal `Progress` (Completed / Aborted /
//! Error) is guaranteed, and it clears `pane.zmodem`, resuming the
//! terminal, so a transfer can never strand the pane as a dead sink.

#![allow(clippy::result_large_err)]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use iced::Task;
use iced::futures::SinkExt;
use uuid::Uuid;

use oryxis_zmodem::{Direction, Progress, TransferIo, TransferSpec};

use crate::app::{TerminalMessage, ZmodemMessage, Message, Oryxis};
use crate::state::{TerminalTransport, ZmodemPane};

impl Oryxis {
    /// Directory downloads land in: the `zmodem_download_dir` setting
    /// when set and non-empty, else the OS Downloads dir, else
    /// `~/.oryxis/downloads`. Created on demand.
    fn zmodem_download_dir(&self) -> std::path::PathBuf {
        let configured = self.setting_zmodem_download_dir.trim();
        if !configured.is_empty() {
            return std::path::PathBuf::from(configured);
        }
        if let Some(dir) = dirs::download_dir() {
            return dir;
        }
        dirs::home_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("."))
            .join(".oryxis")
            .join("downloads")
    }

    /// Open / close the Telnet inbound raw window for a ZMODEM transfer
    /// on `pane_id`. No-op for SSH (8-bit clean) and serial (already the
    /// raw wire); Telnet needs both halves of the raw contract, or a
    /// non-UTF-8 host's charset decoder corrupts every inbound frame.
    pub(crate) fn set_zmodem_binary_inbound(&self, pane_id: Uuid, on: bool) {
        if let Some(TerminalTransport::Telnet(t)) = self
            .pane_by_id(pane_id)
            .and_then(|p| p.session.as_ref())
        {
            t.set_binary_inbound(on);
        }
    }

    /// Begin a ZMODEM transfer on `pane_id` after the detector fired.
    /// Sets up the divert (so subsequent `PtyOutput` for the pane feeds
    /// the driver) and returns a task that runs the transfer and streams
    /// its progress. `first_wire` is the detector's initial bytes.
    pub(crate) fn begin_zmodem_transfer(
        &mut self,
        pane_id: Uuid,
        direction: Direction,
        first_wire: Vec<u8>,
    ) -> Task<Message> {
        // The transport channel that carries the protocol replies.
        // ZMODEM frames are raw bytes, not keystrokes: Telnet's generic
        // input path charset-transcodes and maps line endings (which
        // corrupts binary frames), so it gets the IAC-doubling-only raw
        // sender; serial's `write_sender` is already the raw, echo-free
        // wire path; an SSH PTY channel is 8-bit clean as-is.
        let Some((wire_out, is_serial)) = self
            .pane_by_id(pane_id)
            .and_then(|p| p.session.as_ref())
            .map(|s| {
                // Serial is byte-rate-limited: it needs a small streaming
                // window so the upload watchdog never mistakes a slow drain
                // for a dead peer (see the driver's window constants).
                let is_serial = matches!(s, TerminalTransport::Serial(_));
                let wire_out = match s {
                    TerminalTransport::Telnet(t) => t.raw_write_sender(),
                    other => other.write_sender(),
                };
                (wire_out, is_serial)
            })
        else {
            // No transport (local shell): nothing to run the protocol on.
            return Task::none();
        };
        let streaming_window = if is_serial {
            oryxis_zmodem::SERIAL_STREAMING_WINDOW
        } else {
            oryxis_zmodem::DEFAULT_STREAMING_WINDOW
        };

        let (wire_tx, wire_in) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
        let (progress_tx, mut progress_rx) = tokio::sync::mpsc::unbounded_channel::<Progress>();
        let abort = Arc::new(AtomicBool::new(false));

        // An OS drop staged its sources and typed `rz -y` (drop.rs): the
        // detection firing IS the proof the receiver started, so consume
        // the stash and skip the picker. Taken regardless of direction so
        // a stale stash can never leak into an unrelated later `sz`.
        let preset_sources: Option<Vec<std::path::PathBuf>> =
            self.pane_by_id_mut(pane_id).and_then(|p| {
                if p.pending_drop_sources.is_empty() {
                    None
                } else {
                    Some(std::mem::take(&mut p.pending_drop_sources))
                }
            })
            .filter(|_| direction == Direction::Upload);

        // Telnet: open the inbound raw window before any protocol frame
        // can arrive; closed again wherever the divert is torn down.
        self.set_zmodem_binary_inbound(pane_id, true);
        // Seed the divert with the detector's first wire bytes, then flip
        // the pane into transfer mode so every later batch follows.
        let _ = wire_tx.send(first_wire);
        if let Some(pane) = self.pane_by_id_mut(pane_id) {
            pane.zmodem = Some(ZmodemPane {
                direction,
                wire_tx,
                abort: abort.clone(),
                file_name: None,
                batch: None,
                transferred: 0,
                total: None,
                late: Vec::new(),
            });
        } else {
            return Task::none();
        }

        let dest_dir = self.zmodem_download_dir();
        let io = TransferIo {
            wire_in,
            wire_out: wire_out.clone(),
            progress: progress_tx,
            abort,
        };

        // The stream owns the driver: for a download it runs straight
        // away; for an upload it first asks (async, non-blocking) which
        // file to send, cancelling the remote cleanly if declined.
        let stream = iced::stream::channel::<Message>(
            64,
            move |mut out: iced::futures::channel::mpsc::Sender<Message>| async move {
                let spec = match direction {
                    Direction::Download => {
                        // The destination (a configured folder or the
                        // default `~/Downloads` / `~/.oryxis/downloads`)
                        // may not exist yet; the driver's `File::create`
                        // would fail without its parent. Make it first.
                        if let Err(e) = tokio::fs::create_dir_all(&dest_dir).await {
                            let _ = out
                                .send(Message::Zmodem(ZmodemMessage::ZmodemProgress(
                                    pane_id,
                                    Progress::Error(format!(
                                        "download folder {}: {e}",
                                        dest_dir.display()
                                    )),
                                )))
                                .await;
                            return;
                        }
                        Some(TransferSpec::Download { dest_dir })
                    }
                    Direction::Upload if preset_sources.is_some() => {
                        // OS drop: the sources were chosen by the drop
                        // gesture itself, no picker. `is_some` checked in
                        // the guard, so the expect can never fire.
                        Some(TransferSpec::Upload {
                            sources: preset_sources.expect("guarded by the match arm"),
                            streaming_window,
                        })
                    }
                    Direction::Upload => {
                        // Multi-select: every picked file goes out in
                        // one ZMODEM session, in order.
                        match rfd::AsyncFileDialog::new().pick_files().await {
                            Some(handles) if !handles.is_empty() => Some(TransferSpec::Upload {
                                sources: handles
                                    .iter()
                                    .map(|h| h.path().to_path_buf())
                                    .collect(),
                                streaming_window,
                            }),
                            _ => {
                                // Declined: cancel the waiting remote `rz`
                                // so it doesn't hang, and end the transfer.
                                let _ = wire_out.send(oryxis_zmodem::CANCEL.to_vec());
                                None
                            }
                        }
                    }
                };
                match spec {
                    Some(spec) => {
                        // Run the driver; it drops `progress` (via `io`)
                        // when done, closing `progress_rx` below.
                        tokio::spawn(oryxis_zmodem::run(direction, spec, Vec::new(), io));
                        while let Some(p) = progress_rx.recv().await {
                            if out.send(Message::Zmodem(ZmodemMessage::ZmodemProgress(pane_id, p))).await.is_err() {
                                break;
                            }
                        }
                    }
                    None => {
                        let _ = out.send(Message::Zmodem(ZmodemMessage::ZmodemProgress(pane_id, Progress::Aborted))).await;
                    }
                }
            },
        );

        Task::stream(stream)
    }

    /// Handle a streamed transfer event: update the overlay state and,
    /// on a terminal event, tear the divert down (resuming the terminal)
    /// and toast the outcome.
    pub(crate) fn handle_zmodem(&mut self, message: ZmodemMessage) -> Task<Message> {
        match message {
            ZmodemMessage::ZmodemProgress(pane_id, progress) => {
                // Terminal events tear the divert down and replay any
                // output the transfer no longer owns: the driver's
                // `trailing` (bytes past the peer's "OO" sign-off),
                // then whatever landed on the dead wire channel while
                // this message was in flight (`late`), in arrival
                // order. Replaying synchronously through the normal
                // `PtyOutput` path keeps rendering, logging and
                // detection identical to live output, and nothing else
                // can interleave (the divert is cleared right here).
                let mut toast: Option<String> = None;
                let mut replay: Vec<u8> = Vec::new();
                let mut divert_closed = false;
                {
                    let Some(pane) = self.pane_by_id_mut(pane_id) else {
                        return Task::none();
                    };
                    match progress {
                        Progress::Started { name, size, batch } => {
                            if let Some(zm) = pane.zmodem.as_mut() {
                                zm.file_name = Some(name);
                                zm.total = size;
                                zm.transferred = 0;
                                zm.batch = batch;
                            }
                        }
                        Progress::Advanced { transferred, total } => {
                            if let Some(zm) = pane.zmodem.as_mut() {
                                zm.transferred = transferred;
                                zm.total = total;
                            }
                        }
                        Progress::FileDone { .. } => {}
                        Progress::Completed { trailing } => {
                            replay = trailing;
                            if let Some(zm) = pane.zmodem.take() {
                                replay.extend(zm.late);
                            }
                            toast = Some(crate::i18n::t("zmodem_complete").to_string());
                            divert_closed = true;
                        }
                        Progress::Aborted => {
                            if let Some(zm) = pane.zmodem.take() {
                                replay = zm.late;
                            }
                            toast = Some(crate::i18n::t("zmodem_cancelled").to_string());
                            divert_closed = true;
                        }
                        Progress::Error(e) => {
                            if let Some(zm) = pane.zmodem.take() {
                                replay = zm.late;
                            }
                            toast = Some(format!("{}: {e}", crate::i18n::t("zmodem_failed")));
                            divert_closed = true;
                        }
                    }
                }
                if divert_closed {
                    // Close the Telnet inbound raw window with the divert.
                    // The replayed trailing bytes skip the charset decode
                    // (they were captured raw); a non-ASCII prompt tail may
                    // render mojibake once, which beats corrupting the
                    // whole transfer.
                    self.set_zmodem_binary_inbound(pane_id, false);
                }
                if let Some(text) = toast {
                    self.set_toast(text);
                }
                if replay.is_empty() {
                    Task::none()
                } else {
                    self.update(Message::Terminal(TerminalMessage::PtyOutput(pane_id, replay)))
                }
            }
            ZmodemMessage::PickZmodemDownloadDir => Task::perform(
                tokio::task::spawn_blocking(|| {
                    rfd::FileDialog::new()
                        .set_title(crate::i18n::t("default_download_dir"))
                        .pick_folder()
                        .map(|p| p.display().to_string())
                }),
                |res| Message::Zmodem(ZmodemMessage::ZmodemDownloadDirPicked(res.ok().flatten())),
            ),
            ZmodemMessage::ZmodemDownloadDirPicked(dir) => {
                if let Some(dir) = dir {
                    self.persist_setting("zmodem_download_dir", &dir);
                    self.setting_zmodem_download_dir = dir;
                }
                Task::none()
            }
            ZmodemMessage::ClearZmodemDownloadDir => {
                self.persist_setting("zmodem_download_dir", "");
                self.setting_zmodem_download_dir = String::new();
                Task::none()
            }
            ZmodemMessage::ZmodemCancel(pane_id) => {
                if let Some(pane) = self.pane_by_id_mut(pane_id)
                    && let Some(zm) = pane.zmodem.as_ref()
                {
                    // Cooperative cancel: raise the flag, then wake the
                    // driver with an empty wire chunk in case it is
                    // parked on a silent peer's recv (an empty chunk is
                    // the driver's documented wake-up). It sends the
                    // CANCEL sequence and ends with `Aborted`, which
                    // clears the divert.
                    zm.abort.store(true, Ordering::Relaxed);
                    let _ = zm.wire_tx.send(Vec::new());
                }
                Task::none()
            }
            ZmodemMessage::ZmodemDropRzTimeout(pane_id) => {
                // Sources still staged = the detector never saw the
                // remote receiver start within the window: no lrzsz, or
                // the `rz -y` landed inside a full-screen program. Clear
                // and explain. When the transfer did start, the stash was
                // consumed by `begin_zmodem_transfer` and this is a no-op,
                // so the timeout can never touch a running transfer.
                if let Some(pane) = self.pane_by_id_mut(pane_id)
                    && !pane.pending_drop_sources.is_empty()
                {
                    pane.pending_drop_sources.clear();
                    self.set_toast(crate::i18n::t("terminal_drop_no_rz").to_string());
                }
                Task::none()
            }
        }
    }
}
