//! Sidebar tmux session manager messages, issue #116.
//!
//! Every variant shared the `Tmux` prefix the wrapper already supplies,
//! so the prefix is stripped (`clippy::enum_variant_names`), like the
//! `sync` / `player` / `tray` / `onboarding` domains.

use uuid::Uuid;

#[derive(Debug, Clone)]
pub enum TmuxMessage {
    /// List the sessions on the pane's host. Fired when the tab becomes
    /// visible and from the Refresh action; there is no recurring tick,
    /// because a session list changes on user action, not on its own.
    Refresh(Uuid),
    /// A listing came back for that pane: the raw payload, or an error
    /// to surface in the tab.
    Listed(Uuid, Result<String, String>),
    /// Attach to a session: the command the user would type, sent into
    /// the pane the tab sits beside. Carries the pane AND the tab index
    /// captured when the row was built, so a tab switch between click
    /// and delivery can't land the line in someone else's shell. When
    /// the pane is already attached to another session the handler
    /// switches out-of-band instead (see [`Self::SwitchClients`]): a
    /// typed line would queue behind whatever command runs inside the
    /// current session.
    Attach(usize, Uuid, String),
    /// Continuation of [`Self::Attach`] while attached elsewhere: the
    /// `list-clients` payload for the CURRENT session came back, naming
    /// the client tty the switch must move. The tab index rides along
    /// for the fallback (no client found = the hint was stale, attach
    /// the ordinary typed way).
    SwitchClients(usize, Uuid, String, Result<String, String>),
    /// The out-of-band `switch-client` finished for that pane and
    /// target session: `Ok(())` moves the attach hint and re-lists
    /// immediately, `Err` surfaces the host's wording inline.
    SwitchDone(Uuid, String, Result<(), String>),
    /// "New session" name field.
    NewNameChanged(Uuid, String),
    /// The name field's `on_submit` fired. The fork's `text_input` runs
    /// that binding on ANY Enter, focused or not, so an Enter typed into
    /// the TERMINAL lands here too (issue #160: every command run with
    /// the tab open minted a session on the host). The handler resolves
    /// which widget iced actually has focused before acting.
    Submitted(Uuid),
    /// Continuation of [`Self::Submitted`] once `find_focused` answered:
    /// only an Enter pressed in the name field itself creates a session.
    SubmittedFocus(Uuid, Option<iced::widget::Id>),
    /// Create a detached session with the typed name (empty = let tmux
    /// name it). Detached so it never fights the pane's own PTY. Sent by
    /// the + button and the keynav ring, which both take a deliberate
    /// activation; the Enter path goes through [`Self::Submitted`].
    Create(Uuid),
    /// Ask to kill a session. Parks the confirmation; nothing reaches
    /// the host until it is confirmed.
    AskKill(Uuid, String),
    /// Run the parked kill.
    ConfirmKill(Uuid),
    /// Dismiss the confirmation without touching the host.
    CancelKill(Uuid),
    /// A create / kill finished: `Ok(())` re-lists, `Err` surfaces
    /// inline and leaves the previous listing on screen.
    ActionDone(Uuid, Result<(), String>),
    /// Pointer entered a session row: reveals its floating kill action.
    RowHovered(usize),
    /// Pointer left one. Clears THROUGH `HoverState::leave_tmux_row`,
    /// never `= None`: crossing rows publishes both events in the same
    /// frame in build order, so an unconditional clear would wipe the
    /// hover the arriving row just gained.
    RowExit(usize),
}
