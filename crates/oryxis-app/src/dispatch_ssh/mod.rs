//! `Oryxis::handle_ssh`, match arms for the SSH connection lifecycle
//! (connect, progress streaming, disconnect, errors) plus the shared
//! session-recording gates. The router fans `Message` variants out to
//! per-area submodules:
//!
//! - `connect`     : `start_ssh_tab` (with its `ConnectPlan`
//!   resolution), the split-pane / quick-connect spawn paths and the
//!   pane-entry helpers around them.
//! - `hostkey`     : host-key verification prompts + the
//!   no-common-algorithm / legacy-algorithm fallback dialog.
//! - `kbi`         : keyboard-interactive (2FA / OTP) prompts + the
//!   quick-connect identity / key auth switch.
//! - `session_log` : the session-logging / connection-history settings
//!   toggles and the logs retention window.

// Domain handlers return `Err(Message)` to pass an unclaimed message
// back up the chain. The Message enum is large (~200 bytes) but
// boxing it would force every handler-call site to allocate; the
// pattern is intentional, allow the lint.
#![allow(clippy::result_large_err)]

mod connect;
mod hostkey;
mod kbi;
mod session_log;

use iced::Task;

use std::sync::Arc;
use uuid::Uuid;

use oryxis_ssh::SshSession;

use crate::app::{SshMessage, Message, Oryxis};
use crate::state::View;

impl Oryxis {
    /// Whether a new session should be recorded to the vault. A per-host
    /// `Connection.session_logging` override wins; otherwise the global
    /// `session_logging` setting decides. Panes without a saved
    /// connection (cloud / SSM / local) fall through to the global value.
    pub(crate) fn should_record_session(
        &self,
        conn: Option<&oryxis_core::models::connection::Connection>,
    ) -> bool {
        conn.and_then(|c| c.session_logging)
            .unwrap_or(self.setting_session_logging)
    }

    /// Whether connection events (connect / disconnect / auth failure /
    /// error) should be written to the vault log. Gated by the global
    /// `connection_history` setting (off by default).
    pub(crate) fn should_record_history(&self) -> bool {
        self.setting_connection_history
    }

    /// Dispatch an SSH-lifecycle `Message` to the matching submodule
    /// handler. Each submodule returns `Err(message)` for variants it
    /// doesn't handle so the chain falls through to the next; the core
    /// connect arms live in the match below, and the final `Err`
    /// propagates back to `dispatch::update` so the other handlers
    /// (or the inline match) get their turn.
    pub(crate) fn handle_ssh(
        &mut self,
        message: Message,
    ) -> Result<Task<Message>, Message> {
        let message = match self.handle_ssh_hostkey(message) {
            Ok(task) => return Ok(task),
            Err(m) => m,
        };
        let message = match self.handle_ssh_kbi(message) {
            Ok(task) => return Ok(task),
            Err(m) => m,
        };
        let message = match self.handle_ssh_session_log(message) {
            Ok(task) => return Ok(task),
            Err(m) => m,
        };
        match message {
            // -- SSH connection --
            Message::Ssh(SshMessage::ConnectSsh(idx)) => {
                self.card_context_menu = None;
                self.overlay = None;
                // Close the new-tab picker if the connection was picked there.
                self.show_new_tab_picker = false;
                // If this pick is filling a split pane (not a new tab),
                // route to the per-pane connect path instead.
                if let Some((tab_idx, target, axis)) = self.pending_pane_split.take() {
                    return Ok(self.connect_ssh_into_pane(idx, tab_idx, target, axis));
                }
                if let Some(conn) = self.connections.get(idx).cloned() {
                    return Ok(
                        self.start_ssh_tab(conn, crate::state::ProgressOrigin::Saved(idx))
                    );
                }
            }
            Message::Ssh(SshMessage::QuickConnect(entry)) => {
                self.card_context_menu = None;
                self.overlay = None;
                self.show_new_tab_picker = false;
                // Reuse an existing entry for the same id: a retry after the
                // legacy-algorithm dialog must see its in-place mutations.
                // First connects insert the incoming entry.
                let id = entry.conn.id;
                let conn = self
                    .quick_connects
                    .entry(id)
                    .or_insert_with(|| *entry)
                    .conn
                    .clone();
                if let Some((tab_idx, target, axis)) = self.pending_pane_split.take() {
                    return Ok(self.quick_connect_into_pane(id, tab_idx, target, axis));
                }
                return Ok(self.start_ssh_tab(conn, crate::state::ProgressOrigin::Quick(id)));
            }
            Message::Ssh(SshMessage::SshProgress(step, log)) => {
                if let Some(ref mut progress) = self.connecting {
                    progress.step = step;
                    progress.logs.push((step, log));
                }
            }
            Message::Ssh(SshMessage::SshConnected(pane_id, session)) => {
                let mut detect_for: Option<(Uuid, Arc<SshSession>)> = None;
                if let Some(tab_idx) = self.pane_tab_index(pane_id) {
                    let label = self.tabs[tab_idx].label.clone();
                    // Attach the session to the specific pane that connected
                    // and forward future viewport resizes to the server so
                    // remote `top`/`vim` re-layout instead of overflowing.
                    if let Some(pane) = self.tabs[tab_idx].pane_by_id_mut(pane_id) {
                        pane.session = Some(session.clone());
                        if let Ok(mut state) = pane.terminal.lock() {
                            // Serial has no viewport, so no resize sender;
                            // SSH/Telnet forward window changes to the peer.
                            if let Some(rtx) = session.resize_sender() {
                                state.set_remote_resize_sender(rtx);
                            }
                            // Query replies (cursor position, DECRQM, ...) must
                            // reach the remote: programs block waiting for them
                            // (issue #48, docker compose's raw-mode prompt).
                            state.set_remote_reply_sender(session.write_sender());
                            session.resize(state.cols(), state.rows());
                        }
                    }
                    // Startup command, fired as keystrokes right after the
                    // session is wired. The SSH channel buffers input until
                    // the shell is ready, so the line lands cleanly; the
                    // newline triggers `Enter` on the remote.
                    //
                    // A session-group per-pane script (keyed by pane_id) wins
                    // over the host's own `initial_command`. The fallback is
                    // resolved via the pane's origin rather than the tab label
                    // so it stays correct for group tabs (whose label is the
                    // group name) and for two panes sharing one host.
                    // A live snippet reference (its body, looked up now so
                    // snippet edits propagate) wins over the literal
                    // `initial_command`; a dangling snippet id resolves to
                    // nothing, never an error.
                    let (startup_snip, startup_lit) = self
                        .pane_origin_connection(pane_id)
                        .map(|c| (c.startup_snippet_id, c.initial_command.clone()))
                        .unwrap_or((None, None));
                    let fallback_cmd = match startup_snip {
                        Some(id) => self
                            .snippets
                            .iter()
                            .find(|s| s.id == id)
                            .map(|s| s.command.clone()),
                        None => startup_lit,
                    };
                    let initial = self
                        .pane_script_overrides
                        .remove(&pane_id)
                        .filter(|s| !s.trim().is_empty())
                        .or(fallback_cmd)
                        .filter(|s| !s.trim().is_empty());
                    if let Some(cmd) = initial {
                        let payload = format!("{cmd}\n");
                        if let Err(e) = session.write(payload.as_bytes()) {
                            tracing::warn!(
                                target = "oryxis::dispatch_ssh",
                                error = %e,
                                "failed to send startup command"
                            );
                        } else {
                            tracing::info!(
                                target = "oryxis::dispatch_ssh",
                                bytes = payload.len(),
                                "sent startup command after session ready"
                            );
                        }
                    }
                    // Force-OSC7 (opt-in): inject a PROMPT_COMMAND that
                    // emits OSC 7 on every prompt, so the terminal Files
                    // sidebar follows the exact cwd instead of relying on
                    // the window-title heuristic. bash/zsh syntax; a
                    // shell without PROMPT_COMMAND (fish/sh) just ignores
                    // the assignment. Prepends to any existing value so
                    // the user's own PROMPT_COMMAND still runs. The setup
                    // block erases its own echo (see OSC7_PROMPT_INJECT),
                    // so nothing is left on screen.
                    if self.setting_sftp_force_osc7
                        && let Some(ssh) = session.ssh()
                    {
                        if let Err(e) =
                            ssh.write(crate::state::OSC7_PROMPT_INJECT.as_bytes())
                        {
                            tracing::warn!(
                                target = "oryxis::dispatch_ssh",
                                error = %e,
                                "failed to inject OSC 7 PROMPT_COMMAND"
                            );
                        } else if let Some(pane) =
                            self.tabs[tab_idx].pane_by_id_mut(pane_id)
                        {
                            pane.osc7_injected = true;
                        }
                    }
                    tracing::info!("SSH connected: {}", label);
                    if self.should_record_history()
                        && let Some(vault) = &self.vault {
                        let entry = oryxis_core::models::log_entry::LogEntry::new(
                            &label, &label, oryxis_core::models::log_entry::LogEvent::Connected, "Session established",
                        );
                        let _ = vault.add_log(&entry);
                    }
                    // Reset the auto-reconnect counter for this connection.
                    // Quick-connect hosts resolve through the same label
                    // lookup (saved hosts win a collision), so their
                    // counters reset and OS detection covers them too.
                    let connected = self.any_connection_by_label(&label).map(|conn| {
                        (
                            conn.id,
                            conn.custom_icon.is_some() || conn.custom_color.is_some(),
                            conn.detected_os.is_none(),
                        )
                    });
                    if let Some((conn_id, has_custom, os_unknown)) = connected {
                        self.reconnect_counters.remove(&conn_id);
                        // Queue silent OS detection only if:
                        //   - the feature is enabled,
                        //   - we haven't detected this host before (runs once),
                        //   - and the user hasn't set a custom icon override.
                        // OS detection execs over SSH; Telnet panes skip it
                        // (their icon stays the generic server glyph).
                        if self.setting_os_detection
                            && os_unknown
                            && !has_custom
                            && let Some(ssh) = session.ssh()
                        {
                            detect_for = Some((conn_id, ssh.clone()));
                        }
                    }
                }
                // Clear progress, show terminal, but ONLY if this completion
                // is the connect the card is tracking. A split-pane or
                // background connect completing, or a stale completion from a
                // dial the user cancelled via "Edit host" (whose tab is
                // gone), must not wipe an unrelated Home connect's card.
                if self
                    .connecting
                    .as_ref()
                    .is_some_and(|c| c.pane_id == pane_id)
                {
                    self.connecting = None;
                }

                // A visible sidebar Files browser waiting on this session
                // (reconnect with the tab open) can mount now; without
                // this it would sit on the "Opening SFTP" placeholder
                // until the next pane/tab interaction. No-op otherwise.
                let files_sync = self.sidebar_files_sync();
                // Same idea for the tab's hybrid Files surface (visible
                // or parked): its mount died with the old session, so an
                // in-place reconnect remounts it on the fresh handle at
                // the same directory (issue #63). No-op when nothing was
                // mounted or the mount is still alive.
                let hybrid_sftp = match (self.pane_tab_index(pane_id), session.ssh()) {
                    (Some(t_idx), Some(ssh)) => {
                        let ssh = ssh.clone();
                        self.hybrid_sftp_remount_dead(t_idx, pane_id, &ssh)
                    }
                    _ => Task::none(),
                };
                if let Some((conn_id, sess)) = detect_for {
                    return Ok(Task::batch([
                        files_sync,
                        hybrid_sftp,
                        Task::perform(
                            async move { (conn_id, sess.detect_os().await) },
                            |(id, os)| Message::Ssh(SshMessage::OsDetected(id, os)),
                        ),
                    ]));
                }
                return Ok(Task::batch([files_sync, hybrid_sftp]));
            }
            Message::Ssh(SshMessage::OsDetected(conn_id, os)) => {
                // Persist + update in-memory list so the icon refreshes.
                // Quick-connect hosts update in memory only (tab badge,
                // save-host prefill); nothing is written to the vault.
                if let Some(conn) = self.connections.iter_mut().find(|c| c.id == conn_id) {
                    conn.detected_os = os.clone();
                    if let Some(vault) = &self.vault {
                        let _ = vault.set_detected_os(&conn_id, os.as_deref());
                    }
                } else if let Some(entry) = self.quick_connects.get_mut(&conn_id) {
                    entry.conn.detected_os = os.clone();
                }
                tracing::info!("OS detected for {}: {:?}", conn_id, os);
            }
            Message::SettingToggleOsDetection => {
                self.setting_os_detection = !self.setting_os_detection;
                self.persist_setting(
                    "os_detection",
                    if self.setting_os_detection { "true" } else { "false" },
                );
            }
            Message::Ssh(SshMessage::SshDisconnected(pane_id)) => {
                // Persist whatever this pane recorded before we mark the
                // log ended; otherwise the tail of the session is lost.
                self.flush_session_logs_final();
                if let Some(tab_idx) = self.pane_tab_index(pane_id) {
                    let label = self.tabs[tab_idx].label.replace(" (disconnected)", "");
                    // Clear the disconnected pane's session + end its log.
                    let log_id = self.tabs[tab_idx].pane_by_id_mut(pane_id).and_then(|p| {
                        // Close (not just drop) the dead session: SFTP
                        // mounts hold their own Arc clones, so dropping
                        // the pane's alone would leak the writer/quality
                        // tasks and keep `is_alive()` true on a session
                        // whose transport is gone (the reader exiting is
                        // what delivered this message). `close()` is
                        // idempotent and the polite disconnect it sends
                        // is a no-op on a dead transport.
                        if let Some(session) = p.session.take() {
                            session.close();
                        }
                        // A transfer in flight loses its transport here.
                        // Dropping the `ZmodemPane` drops its `wire_tx`, so
                        // the driver's `wire_in` closes, it returns an error,
                        // and the pane resumes (typable) instead of being
                        // stranded as a dead sink behind a frozen card.
                        p.zmodem = None;
                        // A dead transport voids any in-flight command
                        // timing: the reconnect prompt would otherwise
                        // "finish" it with a duration spanning the outage.
                        p.running_cmd = None;
                        p.last_submitted = None;
                        // The sidebar Files channel died with the session;
                        // a reconnect remounts lazily (preferences kept).
                        p.files.reset_for_disconnect();
                        // A fresh shell on reconnect needs the OSC 7
                        // inject again.
                        p.osc7_injected = false;
                        p.session_log_id
                    });
                    if let Some(log_id) = log_id
                        && let Some(vault) = &self.vault
                    {
                        let _ = vault.end_session_log(&log_id);
                    }
                    if self.should_record_history()
                        && let Some(vault) = &self.vault {
                        let entry = oryxis_core::models::log_entry::LogEntry::new(
                            &label, &label, oryxis_core::models::log_entry::LogEvent::Disconnected, "Session ended",
                        );
                        let _ = vault.add_log(&entry);
                    }
                    // Refresh session logs list (count + current page)
                    if let Some(vault) = &self.vault {
                        self.session_logs_total =
                            vault.count_session_logs().unwrap_or(0);
                        self.session_logs = vault
                            .list_session_logs_page(self.session_logs_page * 50, 50)
                            .unwrap_or_default();
                    }
                    // The tab-level "(disconnected)" relabel + idle toast +
                    // auto-reconnect only make sense when the tab IS this one
                    // session. A split tab has live sibling panes, relabeling
                    // it would make `AutoReconnectTick` rebuild the whole tab
                    // (`ReconnectTab` removes it), nuking the siblings. So for
                    // a multi-pane tab we just note the disconnect inside the
                    // pane and leave the tab alone.
                    if self.tabs[tab_idx].pane_grid.panes.len() > 1 {
                        if let Some(pane) = self.tabs[tab_idx].pane_by_id_mut(pane_id)
                            && let Ok(mut state) = pane.terminal.lock()
                        {
                            state.process(b"\r\n[disconnected]\r\n");
                        }
                        return Ok(Task::none());
                    }
                    self.tabs[tab_idx].label = format!("{} (disconnected)", label);
                    // Surface the disconnect to the user. Without this the
                    // terminal just goes silent and the silent auto-reconnect
                    // (up to 30s later) feels like the shell mysteriously
                    // reset itself. A second toast fires from `ReconnectTab`
                    // when the actual reconnect attempt starts, so the
                    // wording here is intentionally past-tense only.
                    self.set_toast(crate::i18n::t("disconnected_idle").to_string());
                    return Ok(Task::perform(
                        async {
                            tokio::time::sleep(std::time::Duration::from_millis(2500)).await;
                        },
                        |_| Message::ToastClear,
                    ));
                }
            }
            Message::Ssh(SshMessage::SshCloseProgress) => {
                // Close connection progress, remove the tab
                if let Some(ref progress) = self.connecting {
                    let tab_idx = progress.tab_idx;
                    if tab_idx < self.tabs.len() {
                        self.tabs.remove(tab_idx);
                        self.adjust_last_terminal_tab_after_remove(tab_idx);
                    }
                }
                self.connecting = None;
                // A parked identity/key switch dies with its connect.
                self.pending_auth_switch = None;
                self.active_tab = None;
                self.active_view = View::Dashboard;
            }
            Message::Ssh(SshMessage::SshEditFromProgress) => {
                if let Some(ref progress) = self.connecting {
                    let origin = progress.origin;
                    let tab_idx = progress.tab_idx;
                    // A still-live connect (quick hosts offer Edit in every
                    // state, not just failure) is parked on a prompt or mid
                    // dial. Answer any pending ask so the engine isn't left
                    // hanging on its oneshot, and arm the one-shot swallow
                    // for the error that cancel provokes, else it lands
                    // inside the editor as `host_panel_error`.
                    if !progress.failed {
                        if self.pending_kbi_prompt.is_some() {
                            self.pending_kbi_prompt = None;
                            self.pending_kbi_quick = None;
                            self.kbi_inputs.clear();
                            if let Some(ref tx) = self.kbi_response_tx {
                                let _ = tx.try_send(None);
                            }
                        }
                        if self.pending_host_key.is_some() {
                            self.pending_host_key = None;
                            if let Some(ref tx) = self.host_key_response_tx {
                                let _ = tx.try_send(false);
                            }
                        }
                        self.pending_edit_cancel = true;
                    }
                    self.connecting = None;
                    // The switch parked for this connect dies with it.
                    self.pending_auth_switch = None;
                    if tab_idx < self.tabs.len() {
                        self.tabs.remove(tab_idx);
                        self.adjust_last_terminal_tab_after_remove(tab_idx);
                    }
                    self.active_tab = None;
                    self.active_view = View::Dashboard;
                    return Ok(match origin {
                        crate::state::ProgressOrigin::Saved(idx) => {
                            self.update(Message::EditConnection(idx))
                        }
                        // Ad-hoc host: edit the TEMPORARY entry; the editor
                        // opens with Connect (without saving) as the primary
                        // action, Save as the explicit opt-in.
                        crate::state::ProgressOrigin::Quick(id) => {
                            self.update(Message::EditQuickHost(id))
                        }
                    });
                }
            }
            Message::Ssh(SshMessage::SshRetry) => {
                if let Some(ref progress) = self.connecting {
                    let origin = progress.origin;
                    let tab_idx = progress.tab_idx;
                    self.connecting = None;
                    if tab_idx < self.tabs.len() {
                        self.tabs.remove(tab_idx);
                        self.adjust_last_terminal_tab_after_remove(tab_idx);
                    }
                    self.active_tab = None;
                    return Ok(match origin {
                        crate::state::ProgressOrigin::Saved(idx) => {
                            self.update(Message::Ssh(SshMessage::ConnectSsh(idx)))
                        }
                        crate::state::ProgressOrigin::Quick(id) => {
                            match self.quick_connects.get(&id).cloned() {
                                Some(entry) => self
                                    .update(Message::Ssh(SshMessage::QuickConnect(Box::new(entry)))),
                                None => Task::none(),
                            }
                        }
                    });
                }
            }
            Message::Ssh(SshMessage::PaneConnectError(pane_id, msg)) => {
                // Identity / key switch on a split-pane quick connect: the
                // error is the cancel we provoked, reconnect the same pane
                // in place with the mutated entry.
                if let Some(qid) = self.pending_auth_switch
                    && let Some(tab_idx) = self.pane_tab_index(pane_id)
                    && self.tabs[tab_idx].pane_by_id_mut(pane_id).is_some_and(|p| {
                        matches!(
                            p.origin,
                            crate::state::PaneOrigin::QuickHost(q) if q == qid
                        )
                    })
                {
                    self.pending_auth_switch = None;
                    if let Some(pane) = self.tabs[tab_idx].pane_by_id_mut(pane_id)
                        && let Ok(mut state) = pane.terminal.lock()
                    {
                        state.process(
                            b"\r\nRetrying with the selected identity...\r\n",
                        );
                    }
                    return Ok(self.spawn_ssh_for_pane_quick(qid, tab_idx, pane_id));
                }
                // Surface the failure inside the pane that was connecting.
                if let Some(pane) = self
                    .tabs
                    .iter()
                    .flat_map(|t| t.pane_grid.panes.values())
                    .find(|p| p.id == pane_id)
                    && let Ok(mut state) = pane.terminal.lock()
                {
                    state.process(format!("\r\nConnection failed: {msg}\r\n").as_bytes());
                }
                // A failed *in-place reconnect* (single-pane tab whose label
                // matches a saved host) must fall back to the "(disconnected)"
                // state so `AutoReconnectTick` keeps retrying up to
                // `max_reconnect_attempts`. Split tabs (>1 pane) share this
                // message but stay connected via their live sibling panes;
                // session-group tabs carry the group name (no matching host),
                // so neither gets relabeled.
                if let Some(tab_idx) = self.pane_tab_index(pane_id)
                    && self.tabs[tab_idx].pane_grid.panes.len() == 1
                    && !self.tabs[tab_idx].label.ends_with(" (disconnected)")
                {
                    let label = self.tabs[tab_idx].label.clone();
                    // Quick-connect hosts join the retry loop too: their
                    // entry resolves by label like a saved host.
                    if self.any_connection_by_label(&label).is_some() {
                        self.tabs[tab_idx].label = format!("{label} (disconnected)");
                    }
                }
                tracing::error!("pane SSH connect failed: {msg}");
            }
            Message::Ssh(SshMessage::SshBanner(text)) => {
                // Progress-card copy, so legal notices / MFA instructions
                // are readable while the auth prompts are up. Multiple
                // banners concatenate, but CAPPED: banners are
                // unauthenticated input, and an unbounded concat would
                // hand a hostile server a memory + per-frame-redaction
                // lever. 8 KiB shows any real notice; the terminal copy
                // below (scrollback-bounded) carries the overflow.
                const BANNER_CAP: usize = 8 * 1024;
                // A whitespace-only banner must not materialize an empty
                // card block (or an empty scrollback write below).
                if text.trim().is_empty() {
                    return Ok(Task::none());
                }
                if let Some(p) = &mut self.connecting {
                    let slot = p.banner.get_or_insert_with(String::new);
                    if slot.len() < BANNER_CAP {
                        if !slot.is_empty() {
                            slot.push('\n');
                        }
                        slot.push_str(text.trim_end());
                        if slot.len() > BANNER_CAP {
                            let mut cut = BANNER_CAP;
                            while !slot.is_char_boundary(cut) {
                                cut -= 1;
                            }
                            slot.truncate(cut);
                            slot.push('\u{2026}');
                        }
                    }
                }
                // Terminal copy: lands in scrollback, so the banner is
                // still reviewable after the card closes (PuTTY prints
                // it in the terminal). The emulator wants CRLF.
                if let Some(tab_idx) = self.connecting.as_ref().map(|p| p.tab_idx)
                    && let Some(tab) = self.tabs.get(tab_idx)
                    && let Ok(mut state) = tab.active().terminal.lock()
                {
                    let normalized = text.replace("\r\n", "\n").replace('\n', "\r\n");
                    state.process(normalized.as_bytes());
                }
            }
            Message::Ssh(SshMessage::SshPaneBanner(pane_id, text)) => {
                // Split-pane connect: no progress card, straight to the
                // pane's terminal.
                if text.trim().is_empty() {
                    return Ok(Task::none());
                }
                if let Some(pane) = self.pane_by_id_mut(pane_id)
                    && let Ok(mut state) = pane.terminal.lock()
                {
                    let normalized = text.replace("\r\n", "\n").replace('\n', "\r\n");
                    state.process(normalized.as_bytes());
                }
            }
            Message::Ssh(SshMessage::SshError(err)) => {
                // A cancel provoked by the identity / key switch: retry with
                // the mutated entry instead of surfacing the failure. The
                // guard on the progress origin keeps an (unlikely) stale flag
                // from hijacking an unrelated connect's error.
                if let Some(qid) = self.pending_auth_switch
                    && self.connecting.as_ref().is_some_and(|p| {
                        p.origin == crate::state::ProgressOrigin::Quick(qid)
                    })
                {
                    self.pending_auth_switch = None;
                    return Ok(self.update(Message::Ssh(SshMessage::SshRetry)));
                }
                // A cancel provoked by "Edit host" mid-connect: the card is
                // already gone and the editor is open, so this error is
                // expected teardown noise. Requiring `connecting == None`
                // keeps a fresh connect's genuine error from being eaten.
                if self.pending_edit_cancel && self.connecting.is_none() {
                    self.pending_edit_cancel = false;
                    tracing::debug!("swallowing edit-host-provoked connect error: {err}");
                    return Ok(Task::none());
                }
                tracing::error!("SSH error: {}", err);
                if self.should_record_history()
                    && let Some(vault) = &self.vault {
                    let label = self.connecting.as_ref().map(|p| p.label.as_str()).unwrap_or("unknown");
                    let entry = oryxis_core::models::log_entry::LogEntry::new(
                        label, label, oryxis_core::models::log_entry::LogEvent::Error, &err,
                    );
                    let _ = vault.add_log(&entry);
                }
                // Empty-agent diagnostics (B3): when the auth failure
                // touched the agent and the host's referenced key is a
                // security key, the almost-certain cause is that the sk-
                // identity was never added to the OS agent. Append the
                // localized hint so the fix is one line away.
                let err = {
                    let sk_pinned = self
                        .connecting
                        .as_ref()
                        .and_then(|p| self.any_connection_by_label(&p.label))
                        .and_then(|c| {
                            c.key_id.or_else(|| {
                                c.identity_id.and_then(|iid| {
                                    self.identities
                                        .iter()
                                        .find(|i| i.id == iid)
                                        .and_then(|i| i.key_id)
                                })
                            })
                        })
                        .and_then(|kid| self.keys.iter().find(|k| k.id == kid))
                        .is_some_and(|k| k.algorithm.is_security_key());
                    if sk_pinned && err.to_lowercase().contains("agent") {
                        format!("{err}\n{}", crate::i18n::t("sk_agent_hint"))
                    } else {
                        err
                    }
                };
                // Mark progress as failed (keep the view open with logs)
                if let Some(ref mut progress) = self.connecting {
                    progress.failed = true;
                    progress.logs.push((progress.step, format!("Error: {}", err)));
                } else {
                    self.host_panel_error = Some(format!("SSH: {}", err));
                }
            }

            m => return Err(m),
        }
        Ok(Task::none())
    }
}
