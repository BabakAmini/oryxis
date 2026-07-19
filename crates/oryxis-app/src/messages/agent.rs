//! SSH-agent server settings, confirm prompts and per-key expose toggles, wrapped by [`crate::messages::Message::Agent`]. Handled by `Oryxis::handle_agent`.

#[derive(Debug, Clone)]
pub enum AgentMessage {
    /// Settings > Features & Plugins: turn the agent on/off.
    AgentServerToggled(bool),
    /// Toggle the per-signature confirm prompt.
    AgentConfirmToggled(bool),
    /// Toggle accepting external ADD/REMOVE (KeePassXC et al) into the
    /// in-memory roster.
    AgentAllowAddToggled(bool),
    /// Toggle also serving the standard OpenSSH agent pipe name when
    /// free (Windows only).
    AgentOpensshPipeToggled(bool),
    /// A per-key "expose via agent" toggle on the keychain card.
    KeyExposeViaAgentToggled(uuid::Uuid),
    /// A confirm prompt arrived from the agent runtime (carries the
    /// oneshot responder inside an Arc<Mutex<>> so Message stays Clone).
    AgentConfirmAsk(crate::state::AgentConfirmCard),
    /// The user's confirm decision; `always` grants the key for the
    /// session.
    AgentConfirmDecision { allow: bool, always: bool },
    /// Toggle the confirm card's "remember this session" checkbox.
    AgentConfirmToggleAlways,
    /// The on-screen confirm prompt tagged with this seq has sat
    /// unanswered past its deadline: deny it and drop the dead modal.
    AgentConfirmTimedOut(u64),
    /// Copy the socket path / a setup snippet from the settings block.
    CopyAgentPath,
    CopyAgentSnippet(crate::state::AgentSnippetKind),
}
