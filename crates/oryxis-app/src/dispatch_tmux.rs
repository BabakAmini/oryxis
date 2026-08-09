//! tmux session manager dispatch (issue #116).
//!
//! Three of the four actions ride an exec channel multiplexed on the
//! pane's live SSH session (list, create, kill), so they never touch
//! the user's shell. Attach is the exception and has to be: it is the
//! command the user would type, so it is typed, into the pane the tab
//! sits beside, on their click.

use std::time::Duration;

use iced::Task;
use uuid::Uuid;

use crate::app::{Message, Oryxis, TmuxMessage};
use crate::tmux::model::TmuxStatus;
use crate::tmux::probe;

/// A listing is a single short command; a host that cannot answer in
/// this long is a host the tab should report on rather than wait for.
const TMUX_TIMEOUT: Duration = Duration::from_secs(8);

impl Oryxis {
    pub(crate) fn handle_tmux(&mut self, message: TmuxMessage) -> Task<Message> {
        match message {
            TmuxMessage::Refresh(pane_id) => self.tmux_list(pane_id),
            TmuxMessage::Listed(pane_id, result) => {
                self.tmux.end_probe(&pane_id);
                // The pane may have disconnected or closed while the
                // listing was in flight; `forget` dropped its entry, and
                // re-creating one here would paint sessions read over a
                // transport that no longer exists.
                if self.tmux.get(&pane_id).is_none() {
                    return Task::none();
                }
                let entry = self.tmux.entry(pane_id);
                entry.status = match result {
                    Ok(payload) => match probe::parse_listing(&payload) {
                        probe::Listing::NoTmux => TmuxStatus::NoTmux,
                        probe::Listing::Sessions(sessions) => TmuxStatus::Ready(sessions),
                    },
                    Err(e) => TmuxStatus::Failed(e),
                };
                Task::none()
            }
            TmuxMessage::Attach(tab_idx, pane_id, name) => self.tmux_attach(tab_idx, pane_id, name),
            TmuxMessage::NewNameChanged(pane_id, value) => {
                self.tmux.entry(pane_id).new_name = value;
                Task::none()
            }
            TmuxMessage::Create(pane_id) => self.tmux_create(pane_id),
            TmuxMessage::AskKill(pane_id, name) => {
                let entry = self.tmux.entry(pane_id);
                entry.confirm_kill = Some(name);
                entry.error = None;
                Task::none()
            }
            TmuxMessage::CancelKill(pane_id) => {
                self.tmux.entry(pane_id).confirm_kill = None;
                Task::none()
            }
            TmuxMessage::ConfirmKill(pane_id) => self.tmux_kill(pane_id),
            TmuxMessage::RowHovered(idx) => {
                self.hover.tmux_row = Some(idx);
                Task::none()
            }
            TmuxMessage::RowExit(idx) => {
                // Guarded clear: see `HoverState::leave`.
                self.hover.leave_tmux_row(idx);
                Task::none()
            }
            TmuxMessage::ActionDone(pane_id, result) => {
                if self.tmux.get(&pane_id).is_none() {
                    return Task::none();
                }
                match result {
                    // The host's answer is the listing, not our
                    // optimism: re-read rather than editing the list we
                    // are holding.
                    Ok(()) => {
                        self.tmux.entry(pane_id).error = None;
                        self.tmux_list(pane_id)
                    }
                    Err(e) => {
                        self.tmux.entry(pane_id).error = Some(e);
                        Task::none()
                    }
                }
            }
        }
    }

    /// Run the listing for a pane, if it still holds a live SSH session.
    fn tmux_list(&mut self, pane_id: Uuid) -> Task<Message> {
        let Some(session) = self.tmux_session_for_pane(pane_id) else {
            return Task::none();
        };
        // A slow host is skipped rather than queueing listings behind
        // each other: the previous one still holds a channel.
        if !self.tmux.begin_probe(pane_id) {
            return Task::none();
        }
        // Keep whatever is on screen while refreshing an existing list,
        // so the rows don't blink away on every action; only a first
        // load shows the spinner.
        let entry = self.tmux.entry(pane_id);
        if matches!(entry.status, TmuxStatus::Idle) {
            entry.status = TmuxStatus::Loading;
        }
        let command = probe::list_sessions_command();
        Task::perform(
            async move {
                match session.probe(&command, TMUX_TIMEOUT).await {
                    Some(payload) => Ok(payload),
                    None => Err(crate::i18n::t("tmux_probe_failed").to_string()),
                }
            },
            move |result| Message::Tmux(TmuxMessage::Listed(pane_id, result)),
        )
    }

    /// Type `tmux attach -t <name>` into the pane the tab belongs to.
    ///
    /// This is the one action that reaches the user's own shell. The
    /// name came off the host, so it is quoted (and a name carrying a
    /// line break is refused rather than quoted, which would otherwise
    /// let the host run a second command here).
    fn tmux_attach(&mut self, tab_idx: usize, pane_id: Uuid, name: String) -> Task<Message> {
        // A modal on screen owns the keyboard, and the fork's
        // `text_input` returns its `on_submit` binding for Enter before
        // the focus gate, so an Enter meant for the modal can reach
        // this handler.
        if self.any_modal_blocks_input() {
            return Task::none();
        }
        let command = match probe::attach_command(&name) {
            Ok(cmd) => cmd,
            Err(e) => {
                self.tmux.entry(pane_id).error = Some(e.to_string());
                return Task::none();
            }
        };
        self.tmux.entry(pane_id).error = None;
        self.write_ring_injection_to_tab(tab_idx, format!("{command}\n").as_bytes());
        Task::none()
    }

    /// Create a DETACHED session over the exec channel. Detached so it
    /// never fights the pane's own PTY; the user attaches afterwards
    /// with the same gesture any other session takes.
    fn tmux_create(&mut self, pane_id: Uuid) -> Task<Message> {
        // Same fork Enter hazard as `tmux_attach`: the new-name field
        // carries `on_submit`, so an Enter meant for a modal on screen
        // would otherwise create a session on the remote host.
        if self.any_modal_blocks_input() {
            return Task::none();
        }
        let Some(session) = self.tmux_session_for_pane(pane_id) else {
            return Task::none();
        };
        let name = self.tmux.entry(pane_id).new_name.trim().to_string();
        // An empty name is not an error: tmux numbers the session
        // itself, which is what `tmux new -d` does on the command line.
        let command = if name.is_empty() {
            "tmux new-session -d".to_string()
        } else {
            match probe::new_session_command(&name) {
                Ok(cmd) => cmd,
                Err(e) => {
                    self.tmux.entry(pane_id).error = Some(e.to_string());
                    return Task::none();
                }
            }
        };
        self.tmux.entry(pane_id).new_name.clear();
        self.tmux_run_action(pane_id, session, command)
    }

    /// Kill the session parked by `AskKill`.
    fn tmux_kill(&mut self, pane_id: Uuid) -> Task<Message> {
        let Some(name) = self.tmux.entry(pane_id).confirm_kill.take() else {
            return Task::none();
        };
        let Some(session) = self.tmux_session_for_pane(pane_id) else {
            return Task::none();
        };
        let command = match probe::kill_session_command(&name) {
            Ok(cmd) => cmd,
            Err(e) => {
                self.tmux.entry(pane_id).error = Some(e.to_string());
                return Task::none();
            }
        };
        self.tmux_run_action(pane_id, session, command)
    }

    /// Shared tail of create / kill: run the command on an exec channel
    /// and report through `ActionDone`, which re-lists on success.
    fn tmux_run_action(
        &mut self,
        pane_id: Uuid,
        session: std::sync::Arc<oryxis_ssh::SshSession>,
        command: String,
    ) -> Task<Message> {
        Task::perform(
            async move {
                match session.probe(&command, TMUX_TIMEOUT).await {
                    // tmux prints nothing on success and a reason on
                    // failure, so a non-empty answer IS the error. It
                    // is the host's own wording, which beats a generic
                    // string that hides "duplicate session: work".
                    Some(output) if output.trim().is_empty() => Ok(()),
                    Some(output) => Err(output.trim().to_string()),
                    None => Err(crate::i18n::t("tmux_action_failed").to_string()),
                }
            },
            move |result| Message::Tmux(TmuxMessage::ActionDone(pane_id, result)),
        )
    }

    /// The live SSH session behind a pane, when the feature is on and
    /// the transport is still up. `None` for local / serial / telnet
    /// panes and for dead sessions.
    pub(crate) fn tmux_session_for_pane(
        &self,
        pane_id: Uuid,
    ) -> Option<std::sync::Arc<oryxis_ssh::SshSession>> {
        if !self.prefs.tmux_manager {
            return None;
        }
        let pane = self
            .tabs
            .iter()
            .find_map(|tab| tab.pane_grid.panes.values().find(|p| p.id == pane_id))?;
        let ssh = pane.session.as_ref().and_then(|s| s.ssh())?;
        ssh.is_alive().then(|| ssh.clone())
    }

    /// True while the tmux tab is the visible sidebar tab, which is what
    /// arms the on-open listing: the manager never probes a screen
    /// nobody is looking at.
    pub(crate) fn tmux_tab_visible(&self) -> bool {
        self.sidebar_tab_shown(crate::state::TerminalSidebarTab::Tmux)
    }

    /// Idempotent "the tmux tab is on screen for this pane" sync, called
    /// from every entry point that can reveal it (tab selected, pane
    /// focused, sidebar opened). Lists once per pane; a refresh after
    /// that is the user's own action.
    pub(crate) fn tmux_sync(&mut self) -> Task<Message> {
        if !self.tmux_tab_visible() {
            return Task::none();
        }
        let Some(pane_id) = self.active_pane_mut().map(|p| p.id) else {
            return Task::none();
        };
        if self.tmux_session_for_pane(pane_id).is_none() {
            return Task::none();
        }
        // Only a pane that has never been listed triggers a probe:
        // re-entering the tab must not re-hit the host every time.
        if matches!(
            self.tmux.get(&pane_id).map(|e| &e.status),
            None | Some(TmuxStatus::Idle)
        ) {
            // Materialize the entry first: `Listed` drops results for
            // panes it cannot find, and `tmux_list` only creates one
            // when it gets past the session check.
            self.tmux.entry(pane_id);
            return self.tmux_list(pane_id);
        }
        Task::none()
    }

    /// Drop a pane's listing on disconnect / close. A list that outlived
    /// its transport would offer attaches that cannot happen.
    pub(crate) fn tmux_reset_pane(&mut self, pane_id: &Uuid) {
        self.tmux.forget(pane_id);
    }

    /// Drop every listing (feature turned off, vault locked).
    pub(crate) fn tmux_reset_all(&mut self) {
        self.tmux.clear();
    }
}
