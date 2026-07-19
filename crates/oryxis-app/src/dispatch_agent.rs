//! `Oryxis::handle_agent`: the ssh-agent server settings toggles, the
//! per-signature confirm flow, and the setup-snippet copy actions
//! (B1). The wire protocol and listener live in `agent_server`; this is
//! the app-side glue.

#![allow(clippy::result_large_err)]

use iced::Task;

use crate::app::{AgentMessage, Message, Oryxis};
use crate::state::AgentSnippetKind;

impl Oryxis {
    pub(crate) fn handle_agent(&mut self, message: AgentMessage) -> Task<Message> {
        match message {
            AgentMessage::AgentServerToggled(on) => {
                self.agent.error = None;
                if on {
                    return self.start_agent_server();
                }
                self.stop_agent_server();
                self.agent.enabled = false;
                self.persist_setting("agent_server_enabled", "false");
            }
            AgentMessage::AgentConfirmToggled(on) => {
                self.agent.confirm = on;
                self.persist_setting(
                    "agent_server_confirm",
                    if on { "true" } else { "false" },
                );
                // The confirm setting is read at listener start; restart
                // the runtime so the change takes effect immediately.
                if self.agent.enabled {
                    self.stop_agent_server();
                    return self.start_agent_server();
                }
            }
            AgentMessage::AgentAllowAddToggled(on) => {
                self.agent.allow_add = on;
                self.persist_setting(
                    "agent_server_allow_add",
                    if on { "true" } else { "false" },
                );
                // Baked into the source at spawn, same as confirm.
                if self.agent.enabled {
                    self.stop_agent_server();
                    return self.start_agent_server();
                }
            }
            AgentMessage::AgentOpensshPipeToggled(on) => {
                self.agent.openssh_pipe = on;
                self.persist_setting(
                    "agent_server_openssh_pipe",
                    if on { "true" } else { "false" },
                );
                if self.agent.enabled {
                    self.stop_agent_server();
                    return self.start_agent_server();
                }
            }
            AgentMessage::KeyExposeViaAgentToggled(id) => {
                if let Some(vault) = &self.vault
                    && let Some(key) = self.keys.iter_mut().find(|k| k.id == id)
                {
                    key.expose_via_agent = !key.expose_via_agent;
                    key.updated_at = chrono::Utc::now();
                    if let Err(e) = vault.save_key(key, None) {
                        tracing::warn!(target = "oryxis::agent", error = %e, "persist expose flag");
                    }
                }
            }
            AgentMessage::AgentConfirmAsk(card) => {
                // The state machine auto-approves a granted key, queues
                // behind a live prompt, or shows it (returning the seq to
                // arm the auto-dismiss timer for).
                return match self.agent.on_confirm_ask(card) {
                    Some(seq) => confirm_timeout_task(seq),
                    None => Task::none(),
                };
            }
            AgentMessage::AgentConfirmToggleAlways => {
                self.agent.confirm_always = !self.agent.confirm_always;
            }
            AgentMessage::AgentConfirmDecision { allow, always } => {
                self.agent.decide_confirm(allow, always);
                return self.advance_confirm_queue();
            }
            AgentMessage::AgentConfirmTimedOut(seq) => {
                if self.agent.confirm_timed_out(seq) {
                    return self.advance_confirm_queue();
                }
            }
            AgentMessage::CopyAgentPath => {
                if let Some(path) = crate::agent_server::listener_socket_display() {
                    return iced::clipboard::write(path).discard();
                }
            }
            AgentMessage::CopyAgentSnippet(kind) => {
                if let Some(snippet) = self.agent_snippet(kind) {
                    return iced::clipboard::write(snippet).discard();
                }
            }
        }
        Task::none()
    }

    /// Start the agent runtime, wiring its confirm receiver into the
    /// update loop. Reverts the toggle on a bind failure.
    pub(crate) fn start_agent_server(&mut self) -> Task<Message> {
        if self.agent.runtime.is_some() {
            self.agent.enabled = true;
            return Task::none();
        }
        let Some(vault) = &self.vault else {
            return Task::none();
        };
        let db_path = vault.db_path().to_path_buf();
        let master_password = self.master_password.clone();
        match crate::agent_server::AgentRuntime::spawn(
            &db_path,
            master_password.as_deref(),
            self.agent.confirm,
            self.agent.allow_add,
            self.agent.openssh_pipe,
        ) {
            Ok((runtime, confirm_rx)) => {
                // A busy OpenSSH alias is non-fatal: the main listener
                // runs; the inline note under the alias toggle says why
                // zero-config discovery is not on.
                self.agent.alias_error = runtime.alias_error.clone();
                self.agent.runtime = Some(runtime);
                self.agent.enabled = true;
                self.agent.error = None;
                self.persist_setting("agent_server_enabled", "true");
                // The channel always exists: even with the global
                // confirm off, keys added under a CONFIRM constraint
                // prompt through it.
                let stream = tokio_stream::wrappers::UnboundedReceiverStream::new(confirm_rx);
                Task::stream(stream).map(|ask| {
                    Message::Agent(AgentMessage::AgentConfirmAsk(crate::state::AgentConfirmCard {
                        key_comment: ask.key_comment,
                        key_fingerprint: ask.key_fingerprint,
                        peer: ask.peer,
                        responder: std::sync::Arc::new(std::sync::Mutex::new(Some(ask.respond))),
                    }))
                })
            }
            Err(e) => {
                self.agent.enabled = false;
                self.agent.error = Some(e);
                self.agent.alias_error = None;
                Task::none()
            }
        }
    }

    /// Start the agent at unlock/boot time if the user left it enabled
    /// but the runtime is not up yet. Returns the confirm stream Task
    /// (or none). Called from the vault-unlock arms.
    pub(crate) fn agent_boot_task(&mut self) -> Task<Message> {
        if self.agent.enabled && self.agent.runtime.is_none() {
            self.start_agent_server()
        } else {
            Task::none()
        }
    }

    /// Promote the next queued confirm (if any) to the on-screen prompt,
    /// arming its auto-dismiss timer. The state machine owns the policy
    /// (grant skips, seq bump); this just lifts the seq into a Task.
    pub(crate) fn advance_confirm_queue(&mut self) -> Task<Message> {
        match self.agent.advance_confirm_queue() {
            Some(seq) => confirm_timeout_task(seq),
            None => Task::none(),
        }
    }

    /// Stop the runtime (aborts the listener, removes the socket) and
    /// sweep the confirm state / session grants.
    pub(crate) fn stop_agent_server(&mut self) {
        if let Some(runtime) = self.agent.runtime.take() {
            runtime.shutdown();
        }
        self.agent.alias_error = None;
        self.agent.deny_all_and_clear_grants();
    }

    /// Flip the agent's key gate on a soft/hard vault lock (keys go
    /// dark, the listener stays up). No-op when the agent is off.
    pub(crate) fn agent_on_lock(&mut self) {
        if let Some(runtime) = &self.agent.runtime {
            runtime.lock();
        }
        // No prompt (on screen or queued) can be answered against a
        // locked vault: deny them all and drop the grants.
        self.agent.deny_all_and_clear_grants();
    }

    /// Re-unlock the agent's dedicated handle after the vault unlocks.
    pub(crate) fn agent_on_unlock(&mut self) {
        if let Some(runtime) = &self.agent.runtime {
            runtime.unlock(self.master_password.as_deref());
        }
    }

    /// The confirm card's "remember this session" checkbox state.
    pub(crate) fn agent_confirm_always(&self) -> bool {
        self.agent.confirm_always
    }

    /// The generated setup snippet for `kind`, or `None` off unix.
    fn agent_snippet(&self, kind: AgentSnippetKind) -> Option<String> {
        let path = crate::agent_server::listener_socket_display()?;
        Some(match kind {
            AgentSnippetKind::ShellEnv => format!("export SSH_AUTH_SOCK=\"{path}\""),
            AgentSnippetKind::SshConfig => format!("Host *\n  IdentityAgent {path}"),
        })
    }
}

/// How long the on-screen prompt waits before it denies + dismisses
/// itself, matching the sign side's `CONFIRM_TIMEOUT` so the modal
/// never outlives the request it stands for.
const CONFIRM_UI_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// The auto-dismiss timer for the prompt tagged `seq`.
fn confirm_timeout_task(seq: u64) -> Task<Message> {
    Task::perform(
        async move { tokio::time::sleep(CONFIRM_UI_TIMEOUT).await },
        move |()| Message::Agent(AgentMessage::AgentConfirmTimedOut(seq)),
    )
}
