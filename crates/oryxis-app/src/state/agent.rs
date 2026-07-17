//! SSH-agent server UI + runtime state (B1). Off by default; the
//! runtime is `Some` only while the feature toggle is on and the vault
//! is available.

use std::collections::{HashSet, VecDeque};
use std::sync::{Arc, Mutex};

/// A pending per-signature confirmation shown to the user. Holds the
/// oneshot responder inside an `Arc<Mutex<Option<_>>>` so the message
/// carrying it stays `Clone` (a bare `oneshot::Sender` is not).
#[derive(Clone, Debug)]
pub(crate) struct AgentConfirmCard {
    pub key_comment: String,
    pub key_fingerprint: String,
    pub peer: Option<String>,
    /// Taken and fired exactly once on the user's decision.
    pub responder: Arc<Mutex<Option<tokio::sync::oneshot::Sender<bool>>>>,
}

impl AgentConfirmCard {
    /// Fire the responder exactly once (taking it out of the shared
    /// slot). A dropped receiver just means the sign side already timed
    /// out, which is fine.
    pub fn respond(&self, allow: bool) {
        if let Ok(mut slot) = self.responder.lock()
            && let Some(tx) = slot.take()
        {
            let _ = tx.send(allow);
        }
    }

    /// Whether the sign side is still waiting on this card. A queued card
    /// whose receiver was dropped (its 60s sign-side timeout already
    /// fired) must not be promoted to the screen: the prompt would ask
    /// about a request that already failed, and an "always" click would
    /// still record a session grant for it.
    pub fn is_live(&self) -> bool {
        self.responder
            .lock()
            .ok()
            .and_then(|slot| slot.as_ref().map(|tx| !tx.is_closed()))
            .unwrap_or(false)
    }
}

/// Which generated setup snippet a Copy button targets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AgentSnippetKind {
    /// `export SSH_AUTH_SOCK=...` for a shell profile.
    ShellEnv,
    /// `Host *\n  IdentityAgent <path>` for ~/.ssh/config.
    SshConfig,
}

#[derive(Default)]
pub(crate) struct AgentState {
    /// Persisted `agent_server_enabled` (default false). Mirrors the
    /// live runtime's presence.
    pub enabled: bool,
    /// Persisted `agent_server_confirm` (default true): prompt on every
    /// signature.
    pub confirm: bool,
    /// Persisted `agent_server_allow_add` (default false): accept keys
    /// pushed in by external tools (KeePassXC et al) into an in-memory
    /// roster, swept on lock / exit.
    pub allow_add: bool,
    /// Persisted `agent_server_openssh_pipe` (default false, Windows
    /// only): also serve the standard `\\.\pipe\openssh-ssh-agent`
    /// name when it is free.
    pub openssh_pipe: bool,
    /// The live runtime while the feature is on.
    pub runtime: Option<crate::agent_server::AgentRuntime>,
    /// A bind / start error, shown inline under the toggle (and reverts
    /// the toggle).
    pub error: Option<String>,
    /// The OpenSSH alias pipe could not be taken (typically the real
    /// agent service owns it); the main listener still runs. Shown
    /// inline under the alias toggle.
    pub alias_error: Option<String>,
    /// The confirm prompt currently on screen, if any.
    pub pending_confirm: Option<AgentConfirmCard>,
    /// Confirm prompts that arrived while another was on screen, shown
    /// one after another (concurrent sign requests must not clobber each
    /// other into a silent deny). Drained on lock and on toggle-off.
    pub confirm_queue: VecDeque<AgentConfirmCard>,
    /// Monotonic tag for the on-screen prompt, so a stale auto-dismiss
    /// timer only clears the card it was armed for.
    pub confirm_seq: u64,
    /// The "remember this key this session" checkbox state for the
    /// on-screen prompt.
    pub confirm_always: bool,
    /// "Always allow this key this session" grants, keyed by SHA-256
    /// fingerprint. Swept on lock and on toggle-off.
    pub session_grants: HashSet<String>,
}

impl AgentState {
    /// A freshly arrived confirm ask. Returns `Some(seq)` when it becomes
    /// the on-screen prompt (the caller arms its auto-dismiss timer for
    /// that seq); `None` when it was auto-approved by a session grant or
    /// queued behind a live prompt.
    pub fn on_confirm_ask(&mut self, card: AgentConfirmCard) -> Option<u64> {
        if self.session_grants.contains(&card.key_fingerprint) {
            card.respond(true);
            return None;
        }
        // A prompt is already up: queue rather than clobber it (a dropped
        // card resolves to deny, silently failing a concurrent sign).
        if self.pending_confirm.is_some() {
            self.confirm_queue.push_back(card);
            return None;
        }
        Some(self.show_confirm(card))
    }

    /// Arm `card` as the on-screen prompt and return its fresh seq.
    fn show_confirm(&mut self, card: AgentConfirmCard) -> u64 {
        self.confirm_always = false;
        self.confirm_seq = self.confirm_seq.wrapping_add(1);
        self.pending_confirm = Some(card);
        self.confirm_seq
    }

    /// Promote the next queued prompt, auto-approving any whose key was
    /// granted for the session in the meantime. `Some(seq)` when one is
    /// shown (arm its timer), `None` when nothing is left. No-op while a
    /// prompt is already on screen.
    pub fn advance_confirm_queue(&mut self) -> Option<u64> {
        if self.pending_confirm.is_some() {
            return None;
        }
        while let Some(card) = self.confirm_queue.pop_front() {
            // The sign side already gave up on this one; drop it instead
            // of prompting for a dead request (and recording its grant).
            if !card.is_live() {
                continue;
            }
            if self.session_grants.contains(&card.key_fingerprint) {
                card.respond(true);
                continue;
            }
            return Some(self.show_confirm(card));
        }
        None
    }

    /// Resolve the on-screen prompt with the user's decision; `always`
    /// grants the key for the rest of the session.
    pub fn decide_confirm(&mut self, allow: bool, always: bool) {
        if let Some(card) = self.pending_confirm.take() {
            if allow && always {
                self.session_grants.insert(card.key_fingerprint.clone());
            }
            card.respond(allow);
        }
    }

    /// The auto-dismiss timer for `seq` fired: deny and drop the prompt
    /// iff it is still the one on screen (a newer prompt has a higher
    /// seq and survives). Returns whether it cleared a prompt.
    pub fn confirm_timed_out(&mut self, seq: u64) -> bool {
        if self.confirm_seq == seq
            && let Some(card) = self.pending_confirm.take()
        {
            card.respond(false);
            return true;
        }
        false
    }

    /// Deny + drop the on-screen prompt and every queued one, then clear
    /// the session grants (vault lock or feature off). Every responder
    /// resolves to `false`, the safe default.
    pub fn deny_all_and_clear_grants(&mut self) {
        if let Some(card) = self.pending_confirm.take() {
            card.respond(false);
        }
        for card in self.confirm_queue.drain(..) {
            card.respond(false);
        }
        self.session_grants.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::oneshot;

    /// Build a card for `fingerprint` plus the receiver that observes
    /// its eventual decision.
    fn card(fingerprint: &str) -> (AgentConfirmCard, oneshot::Receiver<bool>) {
        let (tx, rx) = oneshot::channel();
        let card = AgentConfirmCard {
            key_comment: "k".into(),
            key_fingerprint: fingerprint.into(),
            peer: None,
            responder: Arc::new(Mutex::new(Some(tx))),
        };
        (card, rx)
    }

    /// The decision a receiver saw: `Some(bool)` if responded, `None` if
    /// the sender was dropped (should never happen in these flows).
    fn decision(rx: &mut oneshot::Receiver<bool>) -> Option<bool> {
        rx.try_recv().ok()
    }

    #[test]
    fn second_ask_queues_instead_of_clobbering() {
        let mut st = AgentState::default();
        let (a, mut rx_a) = card("fp-a");
        let (b, mut rx_b) = card("fp-b");

        // First shows; second queues (both must eventually get answered,
        // the whole point of the queue).
        assert_eq!(st.on_confirm_ask(a), Some(1));
        assert!(st.on_confirm_ask(b).is_none());
        assert_eq!(st.confirm_queue.len(), 1);
        assert_eq!(decision(&mut rx_a), None, "A still pending");
        assert_eq!(decision(&mut rx_b), None, "B queued, not denied");

        // Answer A: B is promoted with a new seq, still unanswered.
        st.decide_confirm(true, false);
        assert_eq!(decision(&mut rx_a), Some(true));
        assert_eq!(st.advance_confirm_queue(), Some(2));
        assert_eq!(st.pending_confirm.as_ref().unwrap().key_fingerprint, "fp-b");
        assert_eq!(decision(&mut rx_b), None);

        st.decide_confirm(false, false);
        assert_eq!(decision(&mut rx_b), Some(false));
    }

    #[test]
    fn always_grant_auto_approves_queued_same_key() {
        let mut st = AgentState::default();
        let (a, mut rx_a) = card("fp");
        let (b, mut rx_b) = card("fp"); // same key, queued behind A

        assert_eq!(st.on_confirm_ask(a), Some(1));
        st.on_confirm_ask(b);

        // Allow A "for the session": promoting the queue must auto-approve
        // B (same fingerprint) without a second prompt.
        st.decide_confirm(true, true);
        assert_eq!(decision(&mut rx_a), Some(true));
        assert_eq!(st.advance_confirm_queue(), None, "B auto-approved, nothing to show");
        assert_eq!(decision(&mut rx_b), Some(true));
        assert!(st.pending_confirm.is_none());
    }

    #[test]
    fn stale_timeout_leaves_newer_prompt_alone() {
        let mut st = AgentState::default();
        let (a, mut rx_a) = card("fp-a");
        assert_eq!(st.on_confirm_ask(a), Some(1));

        // Answer A, then a fresh prompt B takes seq 2.
        st.decide_confirm(false, false);
        let _ = decision(&mut rx_a);
        let (b, mut rx_b) = card("fp-b");
        assert_eq!(st.on_confirm_ask(b), Some(2));

        // A's stale timer (seq 1) must not touch B.
        assert!(!st.confirm_timed_out(1));
        assert!(st.pending_confirm.is_some());
        assert_eq!(decision(&mut rx_b), None);

        // B's own timer denies + clears it.
        assert!(st.confirm_timed_out(2));
        assert_eq!(decision(&mut rx_b), Some(false));
    }

    #[test]
    fn deny_all_denies_screen_and_queue() {
        let mut st = AgentState::default();
        let (a, mut rx_a) = card("fp-a");
        let (b, mut rx_b) = card("fp-b");
        let (c, mut rx_c) = card("fp-c");
        st.on_confirm_ask(a);
        st.on_confirm_ask(b);
        st.on_confirm_ask(c);
        st.session_grants.insert("fp-x".into());

        st.deny_all_and_clear_grants();
        assert_eq!(decision(&mut rx_a), Some(false));
        assert_eq!(decision(&mut rx_b), Some(false));
        assert_eq!(decision(&mut rx_c), Some(false));
        assert!(st.pending_confirm.is_none());
        assert!(st.confirm_queue.is_empty());
        assert!(st.session_grants.is_empty());
    }

    #[test]
    fn session_grant_auto_approves_without_prompt() {
        let mut st = AgentState::default();
        st.session_grants.insert("fp".into());
        let (a, mut rx_a) = card("fp");
        assert_eq!(st.on_confirm_ask(a), None, "granted key never prompts");
        assert_eq!(decision(&mut rx_a), Some(true));
        assert!(st.pending_confirm.is_none());
    }
}
