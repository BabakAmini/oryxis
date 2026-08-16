//! Types behind the tmux manager (issue #116).
//!
//! State is keyed by PANE, not by host: every pane owns the live SSH
//! session the listing is read over, so a pane that disconnects drops
//! exactly its own listing and two panes on the same host each answer
//! for the transport they actually hold.

use std::collections::{HashMap, HashSet};
use uuid::Uuid;

/// One session as `tmux list-sessions` reported it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TmuxSession {
    /// The session name, which is also its `-t` target. Remote-supplied
    /// text: quote it (`probe::*_command`) before it reaches any shell.
    pub name: String,
    pub windows: u32,
    /// Number of clients currently attached. tmux reports a count, not
    /// a flag, so a session shared by two terminals reads as 2.
    pub attached: u32,
    /// Unix timestamp of creation, absent on a tmux too old to report it.
    pub created: Option<i64>,
    /// Session group, when this session is grouped with others.
    pub group: Option<String>,
    /// Foreground commands of this session's panes that are NOT a
    /// shell sitting at a prompt (deduplicated, listing order). This is
    /// how a row can say a detached session still has work running
    /// inside it (issue #159 follow-up); empty means every pane is idle
    /// at a shell, or the tmux is too old to report commands.
    pub running: Vec<String>,
}

impl TmuxSession {
    pub(crate) fn is_attached(&self) -> bool {
        self.attached > 0
    }
}

/// What the tab knows about one pane's host.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) enum TmuxStatus {
    /// No probe has run yet for this pane.
    #[default]
    Idle,
    /// A listing is in flight.
    Loading,
    /// tmux is not installed on the host. A distinct state from an
    /// empty list: there is nothing to offer creating here.
    NoTmux,
    /// tmux answered. An empty vector means it is installed but owns no
    /// sessions, which is the state "New session" exists for.
    Ready(Vec<TmuxSession>),
    /// The probe failed (transport died, timeout, refused shell).
    Failed(String),
}

/// Per-pane tmux tab state: the listing plus the small amount of form
/// state the tab carries.
#[derive(Debug, Clone, Default)]
pub(crate) struct PaneTmux {
    pub status: TmuxStatus,
    /// "New session" name field. Empty means tmux picks the name.
    pub new_name: String,
    /// Session name awaiting kill confirmation. A kill is destructive
    /// and unrecoverable, so it never fires from a single click.
    pub confirm_kill: Option<String>,
    /// Inline error from the last action (a refused name, a failed
    /// kill), cleared by the next successful one.
    pub error: Option<String>,
    /// The session this pane is believed to be attached to (issue
    /// #159): set when the tab's own attach is typed, retired when the
    /// pane leaves the alternate screen (the detach signal), and
    /// validated against every listing (a session that is gone or shows
    /// zero clients cannot be this pane's). A hint, not a promise: the
    /// user can switch sessions by hand, and the next listing corrects
    /// it. While set, that session's row renders highlighted and inert
    /// so a click cannot type a command into the session it is already
    /// showing.
    pub attached_to: Option<String>,
}

/// Whether a listing lets the pane KEEP its "attached here" hint
/// (issue #159). Pure so both directions of the rule stay testable:
/// retiring the hint too eagerly re-arms the click that types `tmux
/// attach` into the session it is already showing, and never retiring
/// it leaves the row permanently inert.
///
/// - A session tmux no longer lists cannot be this pane's, whatever the
///   pane draws. Conclusive: the hint goes.
/// - A failed probe proves nothing; the hint stays for the next good
///   listing.
/// - Zero clients is only evidence WITH the pane off the alternate
///   screen. A tmux client draws there, so a pane still on the primary
///   screen is not showing one: the attach never landed (or already
///   detached). While the pane IS on the alternate screen, a
///   zero-client reading is just a listing that raced the attach, which
///   takes a shell round trip to register.
pub(crate) fn hint_survives_listing(
    status: &TmuxStatus,
    name: &str,
    pane_in_alt_screen: bool,
) -> bool {
    match status {
        TmuxStatus::Failed(_) => true,
        TmuxStatus::Ready(sessions) => match sessions.iter().find(|s| s.name == name) {
            None => false,
            Some(s) => s.is_attached() || pane_in_alt_screen,
        },
        // NoTmux / Idle / Loading: no tmux, no session.
        _ => false,
    }
}

/// Every pane's tmux state, plus the in-flight guard.
#[derive(Debug, Default)]
pub(crate) struct TmuxState {
    panes: HashMap<Uuid, PaneTmux>,
    /// Panes with a probe in flight. A slow host is skipped rather than
    /// queueing listings behind each other, the same rule the monitor
    /// probe follows.
    probing: HashSet<Uuid>,
}

impl TmuxState {
    pub(crate) fn get(&self, pane_id: &Uuid) -> Option<&PaneTmux> {
        self.panes.get(pane_id)
    }

    pub(crate) fn entry(&mut self, pane_id: Uuid) -> &mut PaneTmux {
        self.panes.entry(pane_id).or_default()
    }

    /// Claim the probe slot for a pane. `false` means one is already in
    /// flight and this request should be dropped.
    pub(crate) fn begin_probe(&mut self, pane_id: Uuid) -> bool {
        self.probing.insert(pane_id)
    }

    pub(crate) fn end_probe(&mut self, pane_id: &Uuid) {
        self.probing.remove(pane_id);
    }

    /// Drop one pane's listing (disconnect, pane closed). A listing
    /// that outlived its transport would paint sessions nobody can
    /// attach to.
    pub(crate) fn forget(&mut self, pane_id: &Uuid) {
        self.panes.remove(pane_id);
        self.probing.remove(pane_id);
    }

    /// Drop everything (feature turned off, vault locked).
    pub(crate) fn clear(&mut self) {
        self.panes.clear();
        self.probing.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_probe_slot_is_claimed_once() {
        let mut state = TmuxState::default();
        let pane = Uuid::new_v4();
        assert!(state.begin_probe(pane));
        // Second request while the first is in flight: dropped, not
        // queued behind it.
        assert!(!state.begin_probe(pane));
        state.end_probe(&pane);
        assert!(state.begin_probe(pane));
    }

    fn session(name: &str, attached: u32) -> TmuxSession {
        TmuxSession {
            name: name.into(),
            windows: 1,
            attached,
            created: None,
            group: None,
            running: Vec::new(),
        }
    }

    /// Issue #159, the direction that types a command INTO the session:
    /// the typed attach needs a shell round trip to register, so a
    /// listing that raced it reports zero clients. While the pane is on
    /// the alternate screen (a tmux client IS drawing) that reading is
    /// not evidence, and the hint must survive it however long the host
    /// takes to answer.
    #[test]
    fn a_racing_listing_cannot_retire_the_hint() {
        let ready = TmuxStatus::Ready(vec![session("work", 0)]);
        assert!(hint_survives_listing(&ready, "work", true));
    }

    /// The opposite direction, which a timing grace could not unstick:
    /// a typed attach that failed outright leaves the pane on the
    /// PRIMARY screen, so the next listing must retire the hint rather
    /// than leave the row inert forever.
    #[test]
    fn a_failed_attach_retires_the_hint_on_the_next_listing() {
        let ready = TmuxStatus::Ready(vec![session("work", 0)]);
        assert!(!hint_survives_listing(&ready, "work", false));
    }

    #[test]
    fn an_attached_session_keeps_the_hint_either_way() {
        let ready = TmuxStatus::Ready(vec![session("work", 1)]);
        assert!(hint_survives_listing(&ready, "work", true));
        // Our own client is counted even before the pane's first
        // alternate-screen batch lands.
        assert!(hint_survives_listing(&ready, "work", false));
    }

    #[test]
    fn a_vanished_session_retires_the_hint_whatever_the_pane_draws() {
        // Killed under the pane, or renamed: nothing here can be it,
        // even while the pane still draws the dead client's screen.
        let ready = TmuxStatus::Ready(vec![session("other", 1)]);
        assert!(!hint_survives_listing(&ready, "work", true));
        assert!(!hint_survives_listing(&TmuxStatus::NoTmux, "work", true));
    }

    #[test]
    fn a_failed_probe_proves_nothing_and_keeps_the_hint() {
        let failed = TmuxStatus::Failed("timeout".into());
        assert!(hint_survives_listing(&failed, "work", false));
    }

    #[test]
    fn forgetting_a_pane_releases_its_probe_slot() {
        // Otherwise a pane that disconnects mid-probe could never be
        // listed again after reconnecting.
        let mut state = TmuxState::default();
        let pane = Uuid::new_v4();
        state.begin_probe(pane);
        state.entry(pane).new_name = "x".into();
        state.forget(&pane);
        assert!(state.get(&pane).is_none());
        assert!(state.begin_probe(pane));
    }
}
