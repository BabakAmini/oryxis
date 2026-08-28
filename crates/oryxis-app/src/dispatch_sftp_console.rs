//! Turning a freshly connected SSH session into an SFTP console
//! (issue #188).
//!
//! Same shape and same reasoning as the mosh handover next door: this
//! runs at the point where EVERY dial path converges rather than at the
//! sites that mint an SSH transport, so a dial site added later inherits
//! it and cannot be written without it. Landing here also means the
//! whole SSH connect experience is reused as it stands, host keys and
//! password prompts and proxy consent and the expanded jump chain and
//! all: a console host IS an SSH host right up to this line.
//!
//! Unlike mosh, the SSH session is KEPT. The console's channel rides it,
//! and it is what answers whether the link is still there, so the
//! session lives exactly as long as the console does. It may also be a
//! link a terminal tab is holding (the reuse pool hands out the same
//! transport), which is why closing the console never closes it.

use std::sync::Arc;

use oryxis_ssh::SshSession;
use oryxis_ssh::sftp_shell::SftpShellSession;

use crate::app::Oryxis;
use crate::messages::{Message, SshMessage, TerminalMessage};
use crate::state::{PanePurpose, TerminalTransport};

impl Oryxis {
    /// Open an SFTP console for `conn`, in a tab of its own.
    ///
    /// There is ONE path here whether or not a session to that host is
    /// already up, and that is the point. `start_ssh_tab` consults the
    /// reuse pool before it dials (F2), so an open terminal tab to the
    /// same host lends its connection and the console costs no handshake
    /// at all, while a cold open gets the full connect experience.
    /// Branching on "is there a session" would have meant writing the
    /// cold path twice.
    ///
    /// The console gets its own TAB rather than riding the one it was
    /// opened from. A `get` of four gigabytes must not end because
    /// somebody closed the shell they started it from.
    pub(crate) fn open_sftp_console(
        &mut self,
        conn: oryxis_core::models::Connection,
        start_dir: Option<String>,
    ) -> iced::Task<Message> {
        // Checked again here even though every caller filters already,
        // because what is at stake is a flag that only the SSH dial path
        // consumes: setting it for a host that will not take that path
        // leaves it armed for whatever opens next.
        if !Self::host_can_console(&conn) {
            return iced::Task::none();
        }
        // Consumed by `start_ssh_tab` when it builds the pane, and by
        // `begin_sftp_console` when the dial lands. One-shot, like every
        // other hint of this shape in the app.
        self.pending_console_purpose = true;
        self.pending_console_dir = start_dir;
        let origin = crate::state::ProgressOrigin::Saved(conn.id);
        self.start_ssh_tab(conn, origin)
    }

    /// The console entry for the tab at `idx`: its host, and the working
    /// directory its shell had reached.
    ///
    /// `None` when the tab has no saved host behind it. A quick-connect
    /// tab is deliberately included only when it resolves, for the same
    /// reason quick hosts stay out of pins: their credentials live in a
    /// store that does not outlive the session.
    pub(crate) fn tab_console_target(
        &self,
        idx: usize,
    ) -> Option<(oryxis_core::models::Connection, Option<String>)> {
        let tab = self.tabs.get(idx)?;
        let pane = tab.active();
        // A console needs a live SSH session to multiplex on, or a host
        // it can dial. A mosh pane has neither: it let its SSH go on
        // purpose, which is why asking it for files opens a tab of its
        // own rather than a surface beside it.
        let conn_id = match pane.origin {
            crate::state::PaneOrigin::Host(id) => id,
            _ => return None,
        };
        let conn = self.connections.iter().find(|c| c.id == conn_id)?.clone();
        if !Self::host_can_console(&conn) {
            return None;
        }
        // The shell's own directory, when it reported one. This is the
        // trick SecureCRT's SFTP tab needs an escape sequence for; OSC 7
        // already told us.
        let dir = pane.cwd.clone().filter(|d| d.starts_with('/'));
        Some((conn, dir))
    }

    /// Whether a host can carry an SFTP console at all.
    ///
    /// Two exclusions, and both are about the dial landing somewhere
    /// else than where the console waits:
    ///
    /// - **Not SSH.** `start_ssh_tab` forwards every other protocol to
    ///   its own connect path, none of which reaches `SshConnected`, so
    ///   the console would never open AND the one-shot purpose flag
    ///   would never be consumed. The next ordinary SSH tab would then
    ///   be born a console: a hint that outlived its request, which is
    ///   the failure this app documents in three other places.
    /// - **mosh.** A mosh host branches one line ABOVE the console in
    ///   `SshConnected`, deliberately, because mosh closes the SSH
    ///   session it is handed. So asking for a console on one would
    ///   silently deliver a mosh shell instead. `transport.ssh()`
    ///   already keeps the entry off an OPEN mosh tab; this is what
    ///   keeps it off the host card, where there is no tab to ask.
    pub(crate) fn host_can_console(conn: &oryxis_core::models::Connection) -> bool {
        conn.protocol == oryxis_core::models::connection::ConnectionProtocol::Ssh
            && conn.mosh.is_none()
    }

    /// What a pane's session is for. `Shell` for anything that never
    /// asked to be anything else, which is every pane but the ones the
    /// console opened.
    pub(crate) fn pane_purpose(&self, pane_id: uuid::Uuid) -> PanePurpose {
        self.tabs
            .iter()
            .find_map(|t| t.pane_grid.panes.values().find(|p| p.id == pane_id))
            .map(|p| p.purpose)
            .unwrap_or_default()
    }

    /// Open the SFTP subsystem on `ssh` and hand the pane a console over
    /// it.
    ///
    /// The starting directory is the shell's own working directory when
    /// one is known (a console opened beside a live tab inherits where
    /// that tab had navigated, which is the one thing SecureCRT's own
    /// SFTP tab needs an escape sequence to do), and the session's home
    /// otherwise.
    pub(crate) fn begin_sftp_console(
        &mut self,
        pane_id: uuid::Uuid,
        ssh: Arc<SshSession>,
    ) -> iced::Task<Message> {
        let cols = self
            .tabs
            .iter()
            .find_map(|t| t.pane_grid.panes.values().find(|p| p.id == pane_id))
            .and_then(|p| p.terminal.lock().ok().map(|t| t.cols()))
            .map_or(80, |c| c.max(1));
        let label = self
            .tabs
            .iter()
            .find_map(|t| t.pane_grid.panes.values().find(|p| p.id == pane_id))
            .map(|p| p.label.clone())
            .unwrap_or_else(|| "host".to_string());
        let start_dir = self.pending_console_dir.take();
        let local_cwd = std::env::current_dir().unwrap_or_else(|_| {
            std::env::var_os("HOME")
                .or_else(|| std::env::var_os("USERPROFILE"))
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| std::path::PathBuf::from("."))
        });

        let stream = iced::stream::channel::<Message>(
            128,
            move |mut sender: iced::futures::channel::mpsc::Sender<Message>| async move {
                use iced::futures::SinkExt as _;
                let _ = sender
                    .send(Message::Ssh(SshMessage::SshProgress(
                        pane_id,
                        crate::state::ConnectionStep::OpeningSession,
                        crate::i18n::t("sftp_console_opening").to_string(),
                    )))
                    .await;

                let client = match ssh.open_sftp().await {
                    Ok(client) => client,
                    Err(error) => {
                        let _ = sender
                            .send(Message::Ssh(SshMessage::SshError(
                                pane_id,
                                crate::i18n::t("sftp_console_error_open")
                                    .replace("{reason}", &error.to_string()),
                            )))
                            .await;
                        return;
                    }
                };

                // The home is resolved once, here, so `cd` with no
                // argument and a leading `~` both have an answer without
                // a round trip later. A server that will not canonicalize
                // `.` leaves the console rooted at `/`, which is wrong
                // but navigable, rather than failing to open at all.
                let home = client
                    .canonicalize(".")
                    .await
                    .unwrap_or_else(|_| "/".to_string());
                // Wipe what SSH left on the pane. The dial opened a
                // shell of its own on the way in, so by now the pane is
                // carrying a login banner ("Last login: ...") and a
                // prompt that belong to a session the user never asked
                // to see and that is about to be closed. Left there they
                // would sit above the console's own banner for the rest
                // of its life.
                //
                // Word for word the reason the mosh handover clears at
                // the same point, and placed the same way: after the
                // round trip that opened the subsystem, by which time
                // the shell has finished saying its piece, and before
                // the console prints anything of its own.
                let _ = sender
                    .send(Message::Terminal(TerminalMessage::PtyOutput(
                        pane_id,
                        b"\x1b[H\x1b[2J\x1b[3J\x1b[m".to_vec(),
                    )))
                    .await;

                let (session, mut rx) = SftpShellSession::spawn(
                    Arc::clone(&ssh),
                    client,
                    home,
                    local_cwd,
                    cols,
                    label,
                );
                if let Some(dir) = start_dir {
                    // Delivered as typed input rather than as a
                    // constructor argument: it goes through the same
                    // parse, resolution and error reporting a user's own
                    // `cd` does, so a directory that has vanished since
                    // the shell was there says so instead of leaving the
                    // console silently somewhere else.
                    let _ = session.write(format!("cd {dir}\r").as_bytes());
                }

                let transport = TerminalTransport::SftpShell(Arc::new(session));
                let _ = sender
                    .send(Message::Ssh(SshMessage::SshConnected(pane_id, transport)))
                    .await;

                // The dial opened a SHELL channel on the way here, with a
                // PTY and a login banner and a prompt, and its byte
                // stream is pointed at this very pane. Nobody is going to
                // read it: the pane renders the console now. Left running
                // it would interleave a login banner and a shell prompt
                // into the console's output, so it is let go.
                //
                // Closing the SESSION does not close the CONNECTION: the
                // transport is reference-counted and the console's SFTP
                // channel rides it, so what dies here is one channel that
                // had no reader. The `Arc<SshSession>` the console holds
                // is what keeps that transport alive.
                //
                // Done AFTER the transport was published, not before,
                // because the dying shell stream ends with a
                // `SshDisconnected` for this pane. The handler discards
                // one whose pane already has a live transport, and this
                // ordering is what guarantees it finds one. Same shape as
                // the mosh handover, which closes its SSH for a different
                // reason and relies on the same rule.
                ssh.close();

                while let Some(data) = rx.recv().await {
                    if sender
                        .send(Message::Terminal(TerminalMessage::PtyOutput(pane_id, data)))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
                let _ = sender
                    .send(Message::Ssh(SshMessage::SshDisconnected(pane_id)))
                    .await;
            },
        );

        iced::Task::stream(stream)
    }
}
