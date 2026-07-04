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

use crate::app::{Message, Oryxis};
use crate::state::ZmodemPane;

impl Oryxis {
    /// Directory downloads land in: the `zmodem_download_dir` setting
    /// when set and non-empty, else the OS Downloads dir, else
    /// `~/.oryxis/downloads`. Created on demand.
    fn zmodem_download_dir(&self) -> std::path::PathBuf {
        let configured = self.setting_zmodem_download_dir.trim();
        if !configured.is_empty() {
            return std::path::PathBuf::from(configured);
        }
        dirs::download_dir()
            .or_else(dirs::home_dir)
            .unwrap_or_else(|| std::path::PathBuf::from("."))
            .join(if dirs::download_dir().is_some() {
                ""
            } else {
                ".oryxis/downloads"
            })
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
        // The transport whose input sender carries protocol replies.
        let Some(wire_out) = self
            .pane_by_id(pane_id)
            .and_then(|p| p.session.as_ref())
            .map(|s| s.write_sender())
        else {
            // No transport (local shell): nothing to run the protocol on.
            return Task::none();
        };

        let (wire_tx, wire_in) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
        let (progress_tx, mut progress_rx) = tokio::sync::mpsc::unbounded_channel::<Progress>();
        let abort = Arc::new(AtomicBool::new(false));

        // Seed the divert with the detector's first wire bytes, then flip
        // the pane into transfer mode so every later batch follows.
        let _ = wire_tx.send(first_wire);
        if let Some(pane) = self.pane_by_id_mut(pane_id) {
            pane.zmodem = Some(ZmodemPane {
                direction,
                wire_tx,
                abort: abort.clone(),
                file_name: None,
                transferred: 0,
                total: None,
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
                    Direction::Download => Some(TransferSpec::Download { dest_dir }),
                    Direction::Upload => {
                        match rfd::AsyncFileDialog::new().pick_file().await {
                            Some(handle) => Some(TransferSpec::Upload {
                                source: handle.path().to_path_buf(),
                            }),
                            None => {
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
                            if out.send(Message::ZmodemProgress(pane_id, p)).await.is_err() {
                                break;
                            }
                        }
                    }
                    None => {
                        let _ = out.send(Message::ZmodemProgress(pane_id, Progress::Aborted)).await;
                    }
                }
            },
        );

        Task::stream(stream)
    }

    /// Handle a streamed transfer event: update the overlay state and,
    /// on a terminal event, tear the divert down (resuming the terminal)
    /// and toast the outcome.
    pub(crate) fn handle_zmodem(&mut self, message: Message) -> Result<Task<Message>, Message> {
        match message {
            Message::ZmodemProgress(pane_id, progress) => {
                let Some(pane) = self.pane_by_id_mut(pane_id) else {
                    return Ok(Task::none());
                };
                match progress {
                    Progress::Started { name, size } => {
                        if let Some(zm) = pane.zmodem.as_mut() {
                            zm.file_name = Some(name);
                            zm.total = size;
                            zm.transferred = 0;
                        }
                    }
                    Progress::Advanced { transferred, total } => {
                        if let Some(zm) = pane.zmodem.as_mut() {
                            zm.transferred = transferred;
                            zm.total = total;
                        }
                    }
                    Progress::FileDone { .. } => {}
                    Progress::Completed => {
                        pane.zmodem = None;
                        self.toast = Some(crate::i18n::t("zmodem_complete").to_string());
                    }
                    Progress::Aborted => {
                        pane.zmodem = None;
                        self.toast = Some(crate::i18n::t("zmodem_cancelled").to_string());
                    }
                    Progress::Error(e) => {
                        pane.zmodem = None;
                        self.toast = Some(format!("{}: {e}", crate::i18n::t("zmodem_failed")));
                    }
                }
                Ok(Task::none())
            }
            Message::PickZmodemDownloadDir => Ok(Task::perform(
                tokio::task::spawn_blocking(|| {
                    rfd::FileDialog::new()
                        .set_title("ZMODEM download folder")
                        .pick_folder()
                        .map(|p| p.display().to_string())
                }),
                |res| Message::ZmodemDownloadDirPicked(res.ok().flatten()),
            )),
            Message::ZmodemDownloadDirPicked(dir) => {
                if let Some(dir) = dir {
                    self.persist_setting("zmodem_download_dir", &dir);
                    self.setting_zmodem_download_dir = dir;
                }
                Ok(Task::none())
            }
            Message::ClearZmodemDownloadDir => {
                self.persist_setting("zmodem_download_dir", "");
                self.setting_zmodem_download_dir = String::new();
                Ok(Task::none())
            }
            Message::ZmodemCancel(pane_id) => {
                if let Some(pane) = self.pane_by_id_mut(pane_id)
                    && let Some(zm) = pane.zmodem.as_ref()
                {
                    // Cooperative: the driver sees the flag, emits ZCAN,
                    // and ends with `Aborted`, which clears the divert.
                    zm.abort.store(true, Ordering::Relaxed);
                }
                Ok(Task::none())
            }
            m => Err(m),
        }
    }
}
