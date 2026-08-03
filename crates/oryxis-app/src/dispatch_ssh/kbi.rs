//! Keyboard-interactive (2FA / OTP) prompts, split out of
//! `dispatch_ssh`: the prompt modal lifecycle (open / input / submit /
//! cancel) and the quick-connect identity / key auth switch that
//! pairs with the TOTP autofill flow. Called from `handle_ssh`.

#![allow(clippy::result_large_err)]

use iced::Task;

use crate::app::{SshMessage, Message, Oryxis};

impl Oryxis {
    pub(super) fn handle_ssh_kbi(
        &mut self,
        message: SshMessage,
    ) -> Result<Task<Message>, SshMessage> {
        match message {
            SshMessage::SshKbiPrompt(quick, query) => {
                // One empty answer buffer per prompt, parallel to query.prompts.
                self.kbi_inputs = vec![String::new(); query.prompts.len()];
                self.pending_kbi_prompt = Some(query);
                // Quick-connect prompts carry their entry id, which unlocks
                // the saved identity / key selector in the modal.
                self.pending_kbi_quick = quick;
                // Land focus in the first prompt field so OTP entry is
                // type-and-Enter without a click.
                return Ok(iced::widget::operation::focus(iced::widget::Id::new(
                    crate::state::KBI_FIRST_INPUT_ID,
                )));
            }
            SshMessage::SshKbiInput(idx, value) => {
                if let Some(slot) = self.kbi_inputs.get_mut(idx) {
                    *slot = value.into_inner();
                }
            }
            SshMessage::SshKbiSubmit => {
                let answers = std::mem::take(&mut self.kbi_inputs);
                self.pending_kbi_prompt = None;
                self.pending_kbi_quick = None;
                if let Some(ref tx) = self.kbi_response_tx {
                    let _ = tx.try_send(Some(answers));
                }
            }
            SshMessage::SshKbiCancel => {
                self.pending_kbi_prompt = None;
                self.pending_kbi_quick = None;
                self.kbi_inputs.clear();
                if let Some(ref tx) = self.kbi_response_tx {
                    let _ = tx.try_send(None);
                }
            }
            SshMessage::QuickAuthSwitch(quick_id, choice) => {
                // Mutate the ephemeral entry so this retry and every later
                // reconnect of the tab carry the picked identity / key. The
                // auth method stays Auto: the ladder tries the new material
                // first and still falls back to the interactive prompt if
                // the server rejects it.
                let Some(entry) = self.quick_connects.get_mut(&quick_id) else {
                    return Ok(Task::none());
                };
                match choice {
                    crate::state::QuickAuthChoice::Identity(iid) => {
                        entry.conn.identity_id = Some(iid);
                        entry.conn.key_id = None;
                        // The identity's username becomes the login user when
                        // it carries one (the selector label shows it); the
                        // typed username stays otherwise.
                        if let Some(u) = self
                            .identities
                            .iter()
                            .find(|i| i.id == iid)
                            .and_then(|i| i.username.clone())
                            .filter(|u| !u.trim().is_empty())
                        {
                            entry.conn.username = Some(u);
                        }
                    }
                    crate::state::QuickAuthChoice::Key(kid) => {
                        entry.conn.key_id = Some(kid);
                        entry.conn.identity_id = None;
                    }
                }
                if self.pending_kbi_prompt.take().is_some() {
                    // Mid-prompt: cancel the parked auth attempt. The engine
                    // fails with "Authentication cancelled"; the resulting
                    // error is consumed by `SshError` / `PaneConnectError`
                    // (via `pending_auth_switch`) as an immediate retry
                    // instead of a surfaced failure.
                    self.kbi_inputs.clear();
                    self.pending_kbi_quick = None;
                    self.pending_auth_switch = Some(quick_id);
                    if let Some(ref tx) = self.kbi_response_tx {
                        let _ = tx.try_send(None);
                    }
                } else if self.connecting.as_ref().is_some_and(|p| {
                    p.origin == crate::state::ProgressOrigin::Quick(quick_id) && p.failed
                }) {
                    // Failed-connect screen: the old stream is already dead,
                    // retry directly with the mutated entry.
                    return Ok(self.update(Message::Ssh(SshMessage::SshRetry)));
                }
            }

            m => return Err(m),
        }
        Ok(Task::none())
    }
}
