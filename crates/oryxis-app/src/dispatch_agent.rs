//! `Oryxis::handle_agent`: the ssh-agent server settings toggles, the
//! per-signature confirm flow, and the setup-snippet copy actions
//! (B1). The wire protocol and listener live in `agent_server`; this is
//! the app-side glue.

#![allow(clippy::result_large_err)]

use iced::Task;

use crate::app::{Message, Oryxis};
use crate::state::AgentSnippetKind;

impl Oryxis {
    pub(crate) fn handle_agent(&mut self, message: Message) -> Result<Task<Message>, Message> {
        match message {
            Message::AgentServerToggled(on) => {
                self.agent.error = None;
                if on {
                    return Ok(self.start_agent_server());
                }
                self.stop_agent_server();
                self.agent.enabled = false;
                self.persist_setting("agent_server_enabled", "false");
            }
            Message::AgentConfirmToggled(on) => {
                self.agent.confirm = on;
                self.persist_setting(
                    "agent_server_confirm",
                    if on { "true" } else { "false" },
                );
                // The confirm setting is read at listener start; restart
                // the runtime so the change takes effect immediately.
                if self.agent.enabled {
                    self.stop_agent_server();
                    return Ok(self.start_agent_server());
                }
            }
            Message::KeyExposeViaAgentToggled(id) => {
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
            Message::AgentConfirmAsk(card) => {
                // The state machine auto-approves a granted key, queues
                // behind a live prompt, or shows it (returning the seq to
                // arm the auto-dismiss timer for).
                return Ok(match self.agent.on_confirm_ask(card) {
                    Some(seq) => confirm_timeout_task(seq),
                    None => Task::none(),
                });
            }
            Message::AgentConfirmToggleAlways => {
                self.agent.confirm_always = !self.agent.confirm_always;
            }
            Message::AgentConfirmDecision { allow, always } => {
                self.agent.decide_confirm(allow, always);
                return Ok(self.advance_confirm_queue());
            }
            Message::AgentConfirmTimedOut(seq) => {
                if self.agent.confirm_timed_out(seq) {
                    return Ok(self.advance_confirm_queue());
                }
            }
            Message::CopyAgentPath => {
                if let Some(path) = crate::agent_server::listener_socket_display() {
                    return Ok(iced::clipboard::write(path).discard());
                }
            }
            Message::CopyAgentSnippet(kind) => {
                if let Some(snippet) = self.agent_snippet(kind) {
                    return Ok(iced::clipboard::write(snippet).discard());
                }
            }
            other => return Err(other),
        }
        Ok(Task::none())
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
        ) {
            Ok((runtime, confirm_rx)) => {
                self.agent.runtime = Some(runtime);
                self.agent.enabled = true;
                self.agent.error = None;
                self.persist_setting("agent_server_enabled", "true");
                if let Some(rx) = confirm_rx {
                    let stream = tokio_stream::wrappers::UnboundedReceiverStream::new(rx);
                    return Task::stream(stream).map(|ask| {
                        Message::AgentConfirmAsk(crate::state::AgentConfirmCard {
                            key_comment: ask.key_comment,
                            key_fingerprint: ask.key_fingerprint,
                            peer: ask.peer,
                            responder: std::sync::Arc::new(std::sync::Mutex::new(Some(
                                ask.respond,
                            ))),
                        })
                    });
                }
                Task::none()
            }
            Err(e) => {
                self.agent.enabled = false;
                self.agent.error = Some(e);
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
        move |()| Message::AgentConfirmTimedOut(seq),
    )
}
