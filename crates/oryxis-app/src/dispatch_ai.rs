//! `Oryxis::handle_ai`, match arms for the AI side of the app:
//! provider/model/api-key Settings panel knobs, and the chat sidebar
//! conversation flow (send, receive, retry, tool exec).

#![allow(clippy::result_large_err)]

use iced::widget::text_editor;
use iced::Task;

use std::sync::Arc;

use uuid::Uuid;

use crate::app::{SftpMessage, AiMessage, Message, Oryxis};
use crate::state::{ChatMessage, ChatRole};
use crate::util::chat_scroll_to_end;

/// Cap on consecutive AI-auto-executed commands per user turn. Past this,
/// further auto-exec is refused and the command is surfaced for explicit
/// approval. A backstop for runaway loops of *different* commands;
/// exact-repeat loops are caught earlier by `chat_auto_run_history`.
const CHAT_AUTO_RUN_STREAK_MAX: usize = 12;

/// Snapshot the last `n_lines` of a terminal pane's visible grid as text
/// for the AI context. Trailing whitespace is trimmed per line (alacritty
/// pads empty cells with spaces, which would otherwise inflate the token
/// count for no signal). When `privacy_terms` is `Some`, every line is run
/// through the same redaction the live terminal and session-log viewer use,
/// so IPs / hostnames / usernames the user chose to hide never leave the
/// machine in an AI request. Free function (not a method) so it works both
/// synchronously and inside the detached tool-followup `tokio::spawn`, which
/// can't borrow `self`. Returns an empty string if the lock is poisoned.
fn capture_terminal_context(
    terminal: &std::sync::Mutex<crate::state::TerminalState>,
    n_lines: usize,
    privacy_terms: Option<&[String]>,
    privacy_classes: oryxis_terminal::PrivacyClasses,
) -> String {
    let Ok(state) = terminal.lock() else {
        return String::new();
    };
    // `tail_text` reads the last N rows including scrollback history (already
    // trimmed, wide-char-aware), so a command whose output scrolled off the
    // visible screen still reaches the model. Redact per line under Privacy.
    state
        .tail_text(n_lines)
        .into_iter()
        .map(|l| match privacy_terms {
            Some(terms) => crate::widgets::redact_for_display(&l, terms, privacy_classes),
            None => l,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Reconstruct the provider-agnostic message list from chat history,
/// rebuilding native `tool_use` / `tool_result` pairs from completed `Tool`
/// exchanges. Rules that keep the request valid on every provider:
///
/// - A completed `Tool` exchange (output `Some`) merges its `tool_use` into
///   the immediately-preceding assistant text turn (the canonical single
///   `[text, tool_use]` turn), or emits a standalone assistant `tool_use`
///   turn when there is no preamble; then a `tool` turn carries the result.
/// - A `Tool` exchange still running (output `None`) is emitted as flat text,
///   so an in-flight command never leaves a dangling `tool_use` that would
///   400 the next request.
/// - `Error` / `PendingTool` bubbles and empty assistant placeholders are
///   dropped. `System` notes ride along as plain user text.
///
/// Pure (no `self`), so the per-provider shaping is unit-tested end to end.
fn build_provider_messages(history: &[ChatMessage]) -> Vec<crate::ai::ChatMsg> {
    use crate::ai::{ChatMsg, ToolResultMsg, ToolUseMsg};
    let mut out: Vec<ChatMsg> = Vec::new();
    for m in history {
        match m.role {
            ChatRole::Error | ChatRole::PendingTool => {}
            ChatRole::Assistant => {
                if !m.content.is_empty() {
                    let mut a = ChatMsg::text("assistant", &m.content);
                    // Carry the turn's chain-of-thought back to the
                    // provider; DeepSeek's thinking mode 400s the request
                    // when a prior assistant turn arrives without it (#105).
                    if !m.reasoning.is_empty() {
                        a.reasoning = Some(m.reasoning.clone());
                    }
                    out.push(a);
                }
            }
            ChatRole::User | ChatRole::System => {
                out.push(ChatMsg::text("user", &m.content));
            }
            ChatRole::Tool => {
                let Some(tx) = &m.tool else { continue };
                match &tx.output {
                    // Still running: flat text, no structured tool_use.
                    None => out.push(ChatMsg::text(
                        "user",
                        format!("[Running command: {}]", tx.command),
                    )),
                    Some(output) => {
                        let tu = ToolUseMsg {
                            id: tx.id.clone(),
                            command: tx.command.clone(),
                            risk: tx.risk.clone(),
                            // Gemini signs each function call and demands the
                            // signature back on the next request; we rebuild
                            // the call from name+args, so it has to ride here.
                            thought_signature: tx.thought_signature.clone(),
                        };
                        // Merge into the preceding assistant text turn if any,
                        // else emit a standalone assistant tool_use turn.
                        if let Some(last) = out.last_mut()
                            && last.role == "assistant"
                            && last.tool_use.is_none()
                            && last.tool_result.is_none()
                        {
                            last.tool_use = Some(tu);
                        } else {
                            let mut a = ChatMsg::text("assistant", "");
                            a.tool_use = Some(tu);
                            out.push(a);
                        }
                        out.push(ChatMsg::tool_result(ToolResultMsg {
                            id: tx.id.clone(),
                            output: output.clone(),
                        }));
                    }
                }
            }
        }
    }
    out
}

/// A bubble asking the user to approve a proposed command.
///
/// It carries the proposed call, not just its text, so Gemini's per-call
/// `thoughtSignature` survives the wait for an answer: the approve paths
/// rebuild the exchange from this bubble, and a signature lost here would
/// 400 the request AFTER the command already ran. `build_provider_messages`
/// skips `PendingTool` entirely, so nothing here reaches a request until the
/// command actually executes.
fn pending_tool_bubble(command: String, thought_signature: Option<String>) -> ChatMessage {
    let mut bubble = ChatMessage::text(ChatRole::PendingTool, command.clone());
    bubble.tool = Some(crate::state::ToolExchange {
        // The real id is minted when the command runs; this bubble is only
        // ever read for its command + signature.
        id: String::new(),
        command,
        risk: "risky".into(),
        output: None,
        thought_signature,
    });
    bubble
}

/// Flags that decide how a proposed AI tool call is gated. Pulled out of
/// `Oryxis` state so the decision is a pure function (see
/// [`classify_tool_gate`]) and can be unit-tested.
struct ToolGateInput {
    /// The tab's chat mode (`Plan` / `Ask` / `Auto`), the top-level branch.
    mode: crate::state::ChatMode,
    /// First token is on the tab's "always run" allow-list.
    allowed: bool,
    /// Command chains / pipes / redirects / substitutes (e.g. `ls; rm`).
    has_chaining: bool,
    /// The model self-classified the command as `safe`.
    risk_safe: bool,
    /// Command matches the deterministic catastrophic-command floor.
    obviously_destructive: bool,
    /// The per-turn auto-run streak has reached `CHAT_AUTO_RUN_STREAK_MAX`.
    streak_exceeded: bool,
    /// This exact command was already auto-run earlier in the turn.
    already_auto_ran: bool,
}

/// What to do with a proposed tool call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ToolGate {
    /// Run it immediately, no prompt.
    AutoExec,
    /// Surface it for explicit user approval (loop guard or destructive floor).
    Confirm,
    /// Hand to the independent auto-exec judge.
    Judge,
    /// Queue a pending-tool bubble (risky / unclassified).
    Prompt,
}

/// Decide how a proposed tool call is gated, given the tab's chat mode.
///
/// The allow-list bypass runs first in every mode: a command whose exact
/// name the user marked "always run" is honored (still subject to the loop
/// backstop) whether the tab is in Plan, Ask, or Auto, because that decision
/// was an explicit per-command grant. After that the mode decides:
///
/// - **Ask**: nothing runs unattended; every non-allow-listed command is a
///   `Prompt`.
/// - **Plan**: read-only investigation may auto-run (same floor + judge path
///   as Auto), but any write / destructive / unclassified command is
///   surfaced (`Prompt`) instead of executed, so the session stays read-only
///   by default. The user can still click RUN on a surfaced write.
/// - **Auto**: the full pipeline (destructive floor + loop guards collapse
///   to `Confirm`, model-claimed safe goes to the `Judge`, everything else
///   `Prompt`).
fn classify_tool_gate(i: ToolGateInput) -> ToolGate {
    use crate::state::ChatMode;
    // Allow-listed simple command: runs unattended in any mode, but even a
    // trusted command can't loop forever, so the streak backstop still
    // applies. Exact-repeat is deliberately NOT applied to allow-listed
    // commands (the user may legitimately want `ls` run more than once).
    if i.allowed && !i.has_chaining {
        return if i.streak_exceeded {
            ToolGate::Confirm
        } else {
            ToolGate::AutoExec
        };
    }
    match i.mode {
        // Every command needs an explicit click.
        ChatMode::Ask => ToolGate::Prompt,
        // Read-only auto-runs (through the same guards as Auto); anything
        // that isn't cleanly read-only is surfaced rather than executed.
        ChatMode::Plan => {
            if i.risk_safe
                && !i.obviously_destructive
                && !i.streak_exceeded
                && !i.already_auto_ran
            {
                ToolGate::Judge
            } else {
                ToolGate::Prompt
            }
        }
        ChatMode::Auto => {
            // Deterministic catastrophic-command floor: always prompt.
            if i.risk_safe && i.obviously_destructive {
                return ToolGate::Confirm;
            }
            // Loop guards on the judge path: a repeated command or an
            // over-long streak is refused auto-exec and surfaced instead.
            // This is what breaks the runaway loop with no user action.
            if i.risk_safe && (i.streak_exceeded || i.already_auto_ran) {
                return ToolGate::Confirm;
            }
            // Model-claimed safe and nothing objected: let the judge decide.
            if i.risk_safe {
                return ToolGate::Judge;
            }
            // Risky or unclassified: explicit prompt.
            ToolGate::Prompt
        }
    }
}

impl Oryxis {
    /// Index of the tab with this id, if it still exists. The chat pipeline
    /// routes by id (not `active_tab`) so switching or closing tabs
    /// mid-stream can never land a delta or a command on the wrong session.
    fn chat_tab_index(&self, id: Uuid) -> Option<usize> {
        self.tabs.iter().position(|t| t._id == id)
    }

    /// Abort the in-flight chat stream on the tab with `id` (if any) and
    /// forget its handle. Aborting the iced stream drops the receiver
    /// feeding it, which makes the detached tool-followup task's `tx.send`
    /// fail so it stops too. No-op if the tab is gone or idle.
    pub(crate) fn abort_chat_task_for(&mut self, id: Uuid) {
        if let Some(idx) = self.chat_tab_index(id)
            && let Some(handle) = self.tabs[idx].chat_task.take()
        {
            handle.abort();
        }
    }

    /// Abort the active tab's chat stream and clear its loading flag. The
    /// user-gesture stop path (Stop button, closing the sidebar, reset):
    /// those always act on the tab the user is looking at.
    pub(crate) fn abort_active_chat_task(&mut self) {
        if let Some(idx) = self.active_tab
            && let Some(tab) = self.tabs.get_mut(idx)
        {
            if let Some(handle) = tab.chat_task.take() {
                handle.abort();
            }
            tab.chat_loading = false;
        }
    }

    /// Replace a tab's tracked chat task: abort whatever was running on it,
    /// make the new task abortable, store its handle on the tab, and return
    /// the wrapped task to hand back to iced. Funnel every chat-stream /
    /// judge task through this (keyed by origin tab id) so a single Stop /
    /// close / reset cancels the right conversation's live work, and a chat
    /// on one tab never cancels another's. Passthrough if the tab is gone.
    fn track_chat_task_for(&mut self, id: Uuid, task: Task<Message>) -> Task<Message> {
        self.abort_chat_task_for(id);
        let (task, handle) = task.abortable();
        if let Some(idx) = self.chat_tab_index(id) {
            self.tabs[idx].chat_task = Some(handle);
        }
        task
    }

    /// Clear the per-turn auto-exec guard state on the active tab. Called
    /// whenever the user retakes control (sends a message, resets, or
    /// explicitly approves a command) so a fresh turn starts with a clean
    /// streak and repeat history.
    fn reset_chat_auto_run_guard(&mut self) {
        if let Some(idx) = self.active_tab
            && let Some(tab) = self.tabs.get_mut(idx)
        {
            tab.chat_auto_run_history.clear();
            tab.chat_auto_run_streak = 0;
        }
    }

    /// Spawn a fresh assistant stream for tab `idx` over its CURRENT chat
    /// history, marking the tab loading, pushing the empty assistant
    /// placeholder the deltas land in, and returning the tracked stream task.
    /// Shared by `SendChat` (after it pushes the user message) and `ChatRetry`
    /// (which resumes over the existing history, including tool-output System
    /// bubbles, without needing a trailing user message). Injects the current
    /// terminal output (redacted under Privacy Mode) as leading context.
    fn spawn_chat_stream_for(&mut self, idx: usize) -> Task<Message> {
        // Resolve everything that needs an immutable borrow of `self` up
        // front (tab id for routing, terminal handle, Privacy-Mode inputs,
        // provider config) so the mutable history push doesn't collide.
        let tab_id = self.tabs[idx]._id;
        let terminal = Arc::clone(&self.tabs[idx].active().terminal);
        let pane_label = self.tabs[idx].active().label.clone();
        let privacy_terms = self
            .privacy_active_for_label(&pane_label)
            .then(|| self.privacy_terms());
        let api_key = self
            .vault
            .as_ref()
            .and_then(|v| v.get_ai_api_key().ok().flatten())
            .unwrap_or_default();
        let extra_prompt = self
            .vault
            .as_ref()
            .and_then(|v| v.get_setting("ai_system_prompt").ok().flatten());
        let config = crate::ai::AiConfig {
            provider: self.ai.provider.clone(),
            model: self.ai.model.clone(),
            api_key,
            api_url: if self.ai.api_url.is_empty() {
                None
            } else {
                Some(self.ai.api_url.clone())
            },
            system_prompt: extra_prompt,
            reasoning: self.ai.reasoning,
        };

        // Snapshot the last ~50 lines of terminal output for context,
        // redacted when Privacy Mode is active for this pane.
        let terminal_context = capture_terminal_context(
            &terminal,
            50,
            privacy_terms.as_deref(),
            self.privacy_classes(),
        );

        let tab = &mut self.tabs[idx];
        tab.chat_loading = true;

        // Build messages: inject current terminal output as a leading user
        // turn, then the reconstructed history (with native tool blocks).
        let mut messages: Vec<crate::ai::ChatMsg> = Vec::new();
        if !terminal_context.is_empty() {
            messages.push(crate::ai::ChatMsg::text(
                "user",
                format!(
                    "[Current terminal output (last ~50 lines)]\n```\n{}\n```",
                    terminal_context
                ),
            ));
            messages.push(crate::ai::ChatMsg::text(
                "assistant",
                "I can see the terminal output. How can I help?",
            ));
        }
        messages.extend(build_provider_messages(&tab.chat_history));

        // Insert an empty assistant placeholder so streamed text deltas have
        // a bubble to land in.
        tab.chat_history
            .push(ChatMessage::text(ChatRole::Assistant, String::new()));

        let stream_task = Task::stream(crate::ai::send_chat_stream(config, messages)).map(
            move |chunk| match chunk {
                crate::ai::StreamChunk::Text(delta) => Message::Ai(AiMessage::ChatStreamChunk { tab_id, delta }),
                crate::ai::StreamChunk::Reasoning(delta) => {
                    Message::Ai(AiMessage::ChatStreamReasoning { tab_id, delta })
                }
                crate::ai::StreamChunk::ToolUse { command, risk, thought_signature } => {
                    Message::Ai(AiMessage::ChatToolProposed {
                        tab_id,
                        command,
                        risk,
                        thought_signature,
                    })
                }
                crate::ai::StreamChunk::Done => Message::Ai(AiMessage::ChatStreamDone { tab_id }),
                crate::ai::StreamChunk::Error(error) => Message::Ai(AiMessage::ChatError { tab_id, error }),
            },
        );
        // Track the stream so Stop / close / reset can abort it.
        self.track_chat_task_for(tab_id, stream_task)
    }

    pub(crate) fn handle_ai(
        &mut self,
        message: AiMessage,
    ) -> Task<Message> {
        match message {
            // ── AI settings ──
            AiMessage::ToggleAiEnabled => {
                self.ai.enabled = !self.ai.enabled;
                if let Some(vault) = &self.vault {
                    let _ = vault.set_setting("ai_enabled", if self.ai.enabled { "true" } else { "false" });
                }
            }
            AiMessage::ToggleAiReasoning => {
                self.ai.reasoning = !self.ai.reasoning;
                if let Some(vault) = &self.vault {
                    let _ = vault.set_setting(
                        "ai_reasoning",
                        if self.ai.reasoning { "true" } else { "false" },
                    );
                }
            }
            AiMessage::AiProviderChanged(provider) => {
                // Accept either a display name (from the dropdown) or the
                // internal id. Fall back to keeping the current provider if
                // the value can't be resolved.
                let info = crate::ai::provider_from_display(&provider)
                    .unwrap_or_else(|| crate::ai::provider_info(&provider));
                self.ai.provider = info.id.to_string();
                if let Some(vault) = &self.vault {
                    let _ = vault.set_setting("ai_provider", &self.ai.provider);
                }
                // Suggest the provider's default model when the user hasn't
                // picked one. For Custom we keep whatever model is set.
                if !info.default_model.is_empty() {
                    self.ai.model = info.default_model.into();
                    if let Some(vault) = &self.vault {
                        let _ = vault.set_setting("ai_model", &self.ai.model);
                    }
                }
                // Presets always use their bundled URL; clear any stale
                // override so Save doesn't carry it across providers.
                if info.kind != crate::ai::ProviderKind::Custom {
                    self.ai.api_url.clear();
                    if let Some(vault) = &self.vault {
                        let _ = vault.set_setting("ai_api_url", "");
                    }
                }
            }
            AiMessage::AiModelChanged(model) => {
                self.ai.model = model;
                if let Some(vault) = &self.vault {
                    let _ = vault.set_setting("ai_model", &self.ai.model);
                }
            }
            AiMessage::AiApiKeyChanged(key) => {
                self.ai.api_key = key;
            }
            AiMessage::AiApiUrlChanged(url) => {
                self.ai.api_url = url;
                if let Some(vault) = &self.vault {
                    let _ = vault.set_setting("ai_api_url", &self.ai.api_url);
                }
            }
            AiMessage::AiSystemPromptAction(action) => {
                let was_edit = action.is_edit();
                self.ai.system_prompt.perform(action);
                if was_edit
                    && let Some(vault) = &self.vault
                {
                    let _ = vault.set_setting("ai_system_prompt", &self.ai.system_prompt.text());
                }
            }
            AiMessage::SaveAiApiKey => {
                if !self.ai.api_key.is_empty()
                    && let Some(vault) = &self.vault
                    && vault.set_ai_api_key(&self.ai.api_key).is_ok() {
                        self.ai.api_key.clear();
                        self.ai.api_key_set = true;
                }
            }

            // ── AI chat sidebar ──
            AiMessage::ToggleChatSidebar => {
                let toggled_to = if let Some(idx) = self.active_tab
                    && let Some(tab) = self.tabs.get_mut(idx)
                {
                    tab.chat_visible = !tab.chat_visible;
                    Some(tab.chat_visible)
                } else {
                    None
                };
                if toggled_to == Some(true) {
                    // Opening: land on the configured default tab
                    // (issue #85), resolved against what the pane offers
                    // so a gated default (Files/Monitor with no SSH, Chat
                    // with AI off) never opens an empty panel. "Last
                    // opened" (`None`) keeps the remembered tab, only
                    // applying the legacy Chat->Snippets fallback so the
                    // remembered tab survives a temporary loss of its gate.
                    use crate::state::TerminalSidebarTab;
                    if let Some(default) = self.setting_sidebar_default_tab {
                        self.terminal_sidebar_tab =
                            self.resolve_available_sidebar_tab(default);
                    } else if !self.ai.enabled
                        && self.terminal_sidebar_tab == TerminalSidebarTab::Chat
                    {
                        self.terminal_sidebar_tab = TerminalSidebarTab::Snippets;
                    }
                    // Opening onto the Files tab: mount / catch up to the
                    // shell's cwd (no-op on every other tab).
                    return self.sidebar_files_sync();
                }
                if toggled_to == Some(false) {
                    // Closing the panel is the user's "stop it" gesture (the
                    // reported bug: a runaway tool loop kept running after the
                    // sidebar was closed). Cancel any live chat work so it
                    // doesn't keep executing commands in the background.
                    self.abort_active_chat_task();
                    // A closed sidebar can't keep a keynav ring: it would
                    // silently swallow Enter/arrows meant for the terminal.
                    // Same for the dropdown gate: a HostConfig pick_list
                    // open at close time unmounts without on_close.
                    self.keynav.sidebar_selected = None;
                    self.keynav.pick_open = false;
                }
            }
            AiMessage::SelectTerminalSidebarTab(tab) => {
                // A HostConfig dropdown open when the sidebar tab swaps
                // unmounts without on_close; drop the gate with it.
                self.keynav.pick_open = false;
                // Leaving the Files tab is a blur for its path edit; a
                // stale full-width input waiting behind the tab switch
                // would read as broken on return.
                self.close_files_path_edit();
                self.terminal_sidebar_tab = tab;
                if tab == crate::state::TerminalSidebarTab::History {
                    self.refresh_command_history();
                    // Owner call: entering History lands the keyboard in
                    // its search field. No-op on the empty state, whose
                    // frame renders no such input.
                    return iced::widget::operation::focus(iced::widget::Id::new(
                        "sidebar-history-search",
                    ));
                }
                if tab == crate::state::TerminalSidebarTab::Files {
                    // Mount the pane's SFTP channel (first open) or catch
                    // up to the shell's cwd.
                    return self.sidebar_files_sync();
                }
            }
            AiMessage::SidebarSnippetSearchChanged(v) => {
                self.sidebar_snippet_search = v;
            }
            AiMessage::ToggleSidebarSort => {
                self.sidebar_sort_open = !self.sidebar_sort_open;
                if self.sidebar_sort_open {
                    self.sidebar_search_open = false;
                }
            }
            AiMessage::ToggleSidebarSearch => {
                self.sidebar_search_open = !self.sidebar_search_open;
                self.sidebar_sort_open = false;
                if self.sidebar_search_open {
                    return iced::widget::operation::focus(iced::widget::Id::new(
                        "sidebar-snippet-search",
                    ));
                }
                // Collapsing clears the needle so the list shows everything.
                self.sidebar_snippet_search.clear();
            }
            AiMessage::ChatInputAction(action) => {
                self.chat_input.perform(action);
            }
            AiMessage::ChatScrolled(relative_y) => {
                // Strict end check (not "near end"), relative_offset.y
                // becomes 1.0 when the user is exactly at the bottom.
                // Tiny epsilon covers f32 rounding from the layout pass.
                self.chat_scroll_at_bottom = relative_y >= 0.999;
            }
            AiMessage::ChatResetConversation => {
                // Cancel any in-flight stream first, otherwise the detached
                // tool-followup task would keep running and re-populate the
                // history we're about to clear. Acts on the active tab (the
                // conversation the user is looking at).
                self.abort_active_chat_task();
                self.reset_chat_auto_run_guard();
                if let Some(idx) = self.active_tab {
                    if let Some(tab) = self.tabs.get_mut(idx) {
                        tab.chat_history.clear();
                    }
                    // Whatever was already saved stays saved and readable on
                    // the History screen; the next exchange just starts its
                    // own conversation instead of appending to the one the
                    // user cleared.
                    self.detach_saved_chat(idx);
                }
                self.chat_scroll_at_bottom = true;
            }
            AiMessage::ChatModeChanged(mode) => {
                // Apply to the active tab's conversation and remember it as
                // the default for new tabs (process-wide default + setting).
                if let Some(idx) = self.active_tab
                    && let Some(tab) = self.tabs.get_mut(idx)
                {
                    tab.chat_mode = mode;
                }
                crate::state::set_default_chat_mode(mode);
                if let Some(vault) = &self.vault {
                    let _ = vault.set_setting("ai_default_mode", mode.as_setting());
                }
            }
            AiMessage::ChatSidebarResizeStart => {
                // Capture cursor x and current width, the MouseMoved
                // handler computes the delta against these.
                self.chat_sidebar_drag = Some((self.mouse_position.x, self.chat_sidebar_width));
            }
            AiMessage::ChatSidebarResizeStop => {
                self.chat_sidebar_drag = None;
                // The same global Left-release ends an SFTP divider drag;
                // persist the final ratio so it survives a relaunch.
                if self.sftp_split_drag.take().is_some() {
                    self.persist_setting(
                        "sftp_split_ratio",
                        &format!("{:.4}", self.sftp_split_ratio),
                    );
                }
                // Same Left-release ends a log-panel resize; persist the
                // final height so it survives a relaunch.
                if self.sftp_log_drag.take().is_some() {
                    self.persist_setting(
                        "sftp_log_height",
                        &format!("{:.0}", self.sftp.log_height),
                    );
                }
                // End a column resize: the width was updated live, so just
                // re-seed the template and persist.
                if let Some((side, _, _, _)) = self.sftp_col_resize.take() {
                    self.sftp_columns_template = self.sftp.pane(side).columns.clone();
                    self.persist_sftp_columns();
                }
                // A tab released over the content area merges into the
                // tab showing there instead of reordering (issue #112).
                // Runs first and consumes the drag on success, so the
                // reorder path below sees nothing left to do. Nothing is
                // proposed unless the cursor sits on a split anchor, so
                // an ordinary reorder release falls straight through.
                self.merge_dragged_tab_if_proposed();
                // Ends a tab reorder drag. The live-slide already moved
                // the tab into place during the drag (see TabHovered); on
                // drop we just persist the new pinned order (if the dragged
                // tab is pinned) and clear. A plain click (never promoted to
                // `active`) clears with no persist. Runs BEFORE any early
                // return below: a release that also finished a column
                // sort / SFTP drag / armed a rename used to skip this,
                // leaving the ghost chip stuck on screen (field report).
                if let Some(drag) = self.tab_drag.take()
                    && drag.active
                {
                    // Persist when the dragged tab (terminal or SFTP) is pinned,
                    // so the rearranged pinned order survives a relaunch.
                    let pinned = self
                        .tabs
                        .iter()
                        .find(|t| t._id == drag.from_id)
                        .map(|t| t.pinned)
                        .or_else(|| {
                            self.sftp_tabs
                                .iter()
                                .find(|t| t.id == drag.from_id)
                                .map(|t| t.pinned)
                        })
                        .unwrap_or(false);
                    if pinned {
                        self.persist_pinned_tabs();
                    }
                }
                // End a column reorder. If the drag went active, move the
                // dragged column before whichever header the cursor is over;
                // a release without movement is a plain click that sorts.
                if let Some(drag) = self.sftp_col_drag.take() {
                    let hovered = self.sftp_hovered_col;
                    self.sftp_hovered_col = None;
                    if drag.active {
                        // Name is never a drop target: nothing can be dropped
                        // onto/before it (so it shows no drop effect and keeps
                        // its slot). It can still be dragged elsewhere itself.
                        if let Some((hside, hcol)) = hovered
                            && hside == drag.side
                            && hcol != drag.col
                            && hcol != crate::state::SftpColumn::Name
                        {
                            self.sftp.pane_mut(drag.side).columns.reorder(drag.col, hcol);
                            self.sftp_columns_template =
                                self.sftp.pane(drag.side).columns.clone();
                            self.persist_sftp_columns();
                        }
                    } else if let Some(sort_col) = drag.col.sort_column() {
                        return Task::done(Message::Sftp(SftpMessage::SftpSort(drag.side, sort_col)));
                    }
                }
                // Same global Left-release event also ends an internal
                // SFTP drag. If the drag was active, dispatch the transfer;
                // otherwise it was a plain click, which may have armed a
                // slow-click rename (set on the press in SftpSelectRow).
                if let Some(drag) = self.sftp.drag.take()
                    && drag.active
                {
                    self.sftp.pending_rename = None;
                    return self.handle_internal_drag_drop(drag);
                }
                if self.sftp.pending_rename.is_some() {
                    return self.defer_slow_rename();
                }
            }
            AiMessage::SendChat => {
                let input = self.chat_input.text().trim().to_string();
                if input.is_empty() || !self.ai.enabled {
                    return Task::none();
                }
                let Some(idx) = self.active_tab else {
                    return Task::none();
                };
                if idx >= self.tabs.len() {
                    return Task::none();
                }

                let tab = &mut self.tabs[idx];
                tab.chat_history
                    .push(ChatMessage::text(ChatRole::User, input));
                // A fresh user turn clears the auto-exec guard so the
                // streak / repeat history from the previous turn
                // doesn't bleed into this one.
                tab.chat_auto_run_history.clear();
                tab.chat_auto_run_streak = 0;
                self.chat_input = text_editor::Content::new();
                // Sending a message snaps focus back to the latest
                // exchange, so the next assistant response should
                // also follow (until the user scrolls up again).
                self.chat_scroll_at_bottom = true;

                let stream_task = self.spawn_chat_stream_for(idx);
                return Task::batch(vec![chat_scroll_to_end(), stream_task]);
            }
            AiMessage::ChatStreamChunk { tab_id, delta } => {
                // Route to the origin tab by id (not `active_tab`): the user
                // may have switched tabs while this stream keeps running.
                let is_active = self.chat_tab_index(tab_id) == self.active_tab;
                if let Some(idx) = self.chat_tab_index(tab_id) {
                    // Markdown parse is O(content), so re-parsing on every
                    // token makes a long streamed reply O(n^2). Throttle to
                    // ~10 parses/s off the tab's own last-parse clock (a
                    // shared static would never throttle when two tabs stream
                    // at once); `ChatStreamDone` does the final parse.
                    let now = std::time::Instant::now();
                    let due = self.tabs[idx]
                        .chat_last_md_parse
                        .map(|t| now.duration_since(t).as_millis() >= 100)
                        .unwrap_or(true);
                    if let Some(last) = self.tabs[idx].chat_history.last_mut()
                        && last.role == ChatRole::Assistant
                    {
                        last.content.push_str(&delta);
                        if due {
                            last.parsed_md =
                                iced::widget::markdown::parse(&last.content).collect();
                        }
                    }
                    if due {
                        self.tabs[idx].chat_last_md_parse = Some(now);
                    }
                }
                // Only follow the scroll when the streaming tab is the one
                // on screen; a background stream must not yank the view.
                if is_active && self.chat_scroll_at_bottom {
                    return chat_scroll_to_end();
                }
            }
            AiMessage::ChatStreamReasoning { tab_id, delta } => {
                // Accumulate onto the same assistant bubble the text deltas
                // feed, but in its own field: this is not part of the answer
                // and is never rendered. No markdown re-parse and no scroll
                // follow, so a long chain-of-thought costs nothing on screen.
                if let Some(idx) = self.chat_tab_index(tab_id)
                    && let Some(last) = self.tabs[idx].chat_history.last_mut()
                    && last.role == ChatRole::Assistant
                {
                    last.reasoning.push_str(&delta);
                }
            }
            AiMessage::ChatStreamDone { tab_id } => {
                let is_active = self.chat_tab_index(tab_id) == self.active_tab;
                if let Some(idx) = self.chat_tab_index(tab_id) {
                    // Final parse so the rendered markdown can't lag behind
                    // the throttled streaming parses above.
                    if let Some(last) = self.tabs[idx].chat_history.last_mut()
                        && last.role == ChatRole::Assistant
                    {
                        last.parsed_md =
                            iced::widget::markdown::parse(&last.content).collect();
                    }
                    // Empty assistant placeholders are filtered out at the
                    // view layer and excluded from the message-builder when
                    // we send to the model, so we don't try to pop them
                    // here. (Popping was racy when a tool followup pushed
                    // its own placeholder before the original stream's Done
                    // arrived.)
                    // The stream finished on its own; drop the now-spent
                    // abort handle so a later Stop doesn't cancel nothing.
                    self.tabs[idx].chat_task = None;
                    self.tabs[idx].chat_loading = false;
                    // The reply is complete: a settle point where saving the
                    // conversation cannot capture a half-streamed answer.
                    self.flush_chat_history(idx);
                }
                if is_active && self.chat_scroll_at_bottom {
                    return chat_scroll_to_end();
                }
            }
            AiMessage::ChatStop => {
                // User asked to stop. Abort the active tab's live stream (and
                // the detached tool-followup pipeline it feeds) and freeze
                // the auto-exec guard where it is, so nothing else runs until
                // the user sends the next message.
                self.abort_active_chat_task();
            }
            AiMessage::ChatError { tab_id, error } => {
                // Provider/network failures get their own role so the
                // bubble can render with an error treatment + Retry,
                // instead of being indistinguishable from a real reply.
                let is_active = self.chat_tab_index(tab_id) == self.active_tab;
                if let Some(idx) = self.chat_tab_index(tab_id) {
                    // If the stream errored before the model wrote any
                    // text, drop the empty assistant placeholder so we
                    // don't render a blank bubble above the error.
                    if let Some(last) = self.tabs[idx].chat_history.last()
                        && last.role == ChatRole::Assistant
                        && last.content.is_empty()
                    {
                        self.tabs[idx].chat_history.pop();
                    }
                    self.tabs[idx]
                        .chat_history
                        .push(ChatMessage::text(ChatRole::Error, error));
                    self.tabs[idx].chat_task = None;
                    self.tabs[idx].chat_loading = false;
                }
                if is_active && self.chat_scroll_at_bottom {
                    return chat_scroll_to_end();
                }
            }
            AiMessage::ChatRetry => {
                // Strip the trailing error bubble plus any partial-stream
                // assistant remnants (empty placeholders, or text that
                // arrived before the error), then resume a fresh assistant
                // stream over whatever remains. Unlike the old flow this does
                // NOT require a trailing user message: when the error followed
                // a tool execution the history ends on a System bubble (the
                // command + its output), and the stream continues from there
                // (#3). It also does not pop the user message, so nothing is
                // lost if the remaining history isn't what we expected.
                let Some(idx) = self.active_tab else {
                    return Task::none();
                };
                if idx >= self.tabs.len() {
                    return Task::none();
                }
                {
                    let tab = &mut self.tabs[idx];
                    while matches!(
                        tab.chat_history.last().map(|m| &m.role),
                        Some(ChatRole::Error) | Some(ChatRole::Assistant)
                    ) {
                        tab.chat_history.pop();
                    }
                    // Nothing left to respond to (whole conversation was error
                    // remnants): don't spawn an empty stream.
                    if tab.chat_history.is_empty() {
                        tab.chat_loading = false;
                        return Task::none();
                    }
                }
                self.chat_scroll_at_bottom = true;
                let stream_task = self.spawn_chat_stream_for(idx);
                return Task::batch(vec![chat_scroll_to_end(), stream_task]);
            }
            AiMessage::ChatToolProposed { tab_id, command, risk, thought_signature } => {
                // Gate the tool call against the ORIGIN tab (routed by id, so
                // switching tabs mid-stream can't run a command on the wrong
                // host). Mode + allow-list + loop guards all read from that
                // tab. If it's gone (closed mid-stream), drop the proposal.
                let Some(idx) = self.chat_tab_index(tab_id) else {
                    return Task::none();
                };
                let first_token = command
                    .split_whitespace()
                    .next()
                    .unwrap_or("")
                    .to_string();
                let tab = &self.tabs[idx];
                let allowed = tab
                    .chat_always_run_commands
                    .iter()
                    .any(|c| c == &first_token);
                // Loop guards, read from the origin tab's per-turn auto-exec
                // state. `streak_exceeded` catches a long run of *different*
                // auto-executed commands; `already_auto_ran` catches the
                // model re-proposing the exact same command (the reported
                // `docker --version` loop). Both convert an auto-exec into a
                // confirmation prompt so the loop can't run unattended.
                let streak_exceeded = tab.chat_auto_run_streak >= CHAT_AUTO_RUN_STREAK_MAX;
                let already_auto_ran = tab.chat_auto_run_history.iter().any(|c| c == &command);
                let mode = tab.chat_mode;
                // Decide how this tool call is gated. The branching (mode,
                // allow-list bypass, destructive floor, loop guards, judge,
                // prompt) is a pure function of these flags so it can be
                // unit-tested without a live `Oryxis`, see `classify_tool_gate`.
                let gate = classify_tool_gate(ToolGateInput {
                    mode,
                    allowed,
                    has_chaining: crate::ai::has_shell_chaining(&command),
                    risk_safe: risk == "safe",
                    obviously_destructive: crate::ai::is_obviously_destructive(&command),
                    streak_exceeded,
                    already_auto_ran,
                });
                match gate {
                    // Allow-listed simple command under the streak cap: run now.
                    ToolGate::AutoExec => {
                        return Task::done(Message::Ai(AiMessage::ChatToolExec {
                            tab_id,
                            command,
                            risk,
                            thought_signature,
                        }));
                    }
                    // Loop guard tripped, or the deterministic destructive
                    // floor fired: surface it for explicit approval instead of
                    // running it unattended. This is what stops the reported
                    // runaway loop on its own.
                    ToolGate::Confirm => {
                        return Task::done(Message::Ai(AiMessage::ChatToolGuardBlocked {
                            tab_id,
                            command,
                            thought_signature,
                        }));
                    }
                    // Model-claimed `safe` and nothing above objected: hand to
                    // the independent auto-exec judge, which can only escalate
                    // to a prompt, never approve, and fails safe on error.
                    ToolGate::Judge => {
                        let api_key = self
                            .vault
                            .as_ref()
                            .and_then(|v| v.get_ai_api_key().ok().flatten())
                            .unwrap_or_default();
                        let config = crate::ai::AiConfig {
                            provider: self.ai.provider.clone(),
                            model: self.ai.model.clone(),
                            api_key,
                            api_url: if self.ai.api_url.is_empty() {
                                None
                            } else {
                                Some(self.ai.api_url.clone())
                            },
                            system_prompt: None,
                            reasoning: self.ai.reasoning,
                        };
                        self.tabs[idx].chat_loading = true;
                        let cmd_for_judge = command.clone();
                        let risk_for_exec = risk.clone();
                        let sig_for_exec = thought_signature.clone();
                        let sig_for_block = thought_signature.clone();
                        let judge = Task::perform(
                            crate::ai::judge_auto_exec(config, cmd_for_judge),
                            move |allow| {
                                if allow {
                                    Message::Ai(AiMessage::ChatToolExec {
                                        tab_id,
                                        command: command.clone(),
                                        risk: risk_for_exec.clone(),
                                        thought_signature: sig_for_exec.clone(),
                                    })
                                } else {
                                    Message::Ai(AiMessage::ChatToolGuardBlocked {
                                        tab_id,
                                        command: command.clone(),
                                        thought_signature: sig_for_block.clone(),
                                    })
                                }
                            },
                        );
                        // Track the judge call so Stop cancels a pending
                        // auto-exec decision before it can fire ChatToolExec
                        // and run a command behind the user's back.
                        return self.track_chat_task_for(tab_id, judge);
                    }
                    // Risky / unclassified: fall through to the pending bubble.
                    ToolGate::Prompt => {}
                }
                // Drop the empty assistant placeholder if the model went
                // straight to a tool call without any text.
                if let Some(last) = self.tabs[idx].chat_history.last()
                    && last.role == ChatRole::Assistant
                    && last.content.is_empty()
                {
                    self.tabs[idx].chat_history.pop();
                }
                self.tabs[idx]
                    .chat_history
                    .push(pending_tool_bubble(command, thought_signature));
                self.tabs[idx].chat_loading = false;
                if Some(idx) == self.active_tab && self.chat_scroll_at_bottom {
                    return chat_scroll_to_end();
                }
            }
            AiMessage::ChatToolGuardBlocked { tab_id, command, thought_signature } => {
                // A mode / loop guard or the independent judge declined to
                // auto-run this command, so surface it on the origin tab for
                // explicit approval exactly like a risky one.
                let Some(idx) = self.chat_tab_index(tab_id) else {
                    return Task::none();
                };
                if let Some(last) = self.tabs[idx].chat_history.last()
                    && last.role == ChatRole::Assistant
                    && last.content.is_empty()
                {
                    self.tabs[idx].chat_history.pop();
                }
                self.tabs[idx]
                    .chat_history
                    .push(pending_tool_bubble(command, thought_signature));
                self.tabs[idx].chat_loading = false;
                if Some(idx) == self.active_tab && self.chat_scroll_at_bottom {
                    return chat_scroll_to_end();
                }
            }
            AiMessage::ChatToolApprove(command) => {
                // RUN on a pending prompt in the active tab. Only pop the
                // trailing bubble when it's the pending prompt for THIS exact
                // command: a Play click on an older code block must not
                // silently swallow a different command still awaiting a
                // decision (#4).
                let Some(idx) = self.active_tab else {
                    return Task::none();
                };
                let tab_id = self.tabs.get(idx).map(|t| t._id);
                // Lift the proposal's signature off the bubble before it goes:
                // Gemini needs it back on the request that follows the run.
                let mut thought_signature = None;
                if let Some(tab) = self.tabs.get_mut(idx)
                    && let Some(last) = tab.chat_history.last()
                    && last.role == ChatRole::PendingTool
                    && last.content == command
                {
                    thought_signature =
                        last.tool.as_ref().and_then(|t| t.thought_signature.clone());
                    tab.chat_history.pop();
                }
                // The user just retook control, so a command they approve
                // starts a fresh auto-exec chain (clears the streak / repeat
                // history the loop guard accumulated).
                self.reset_chat_auto_run_guard();
                if let Some(tab_id) = tab_id {
                    // User-approved (was surfaced as needing confirmation), so
                    // record it as `risky` in the reconstructed tool_use.
                    return Task::done(Message::Ai(AiMessage::ChatToolExec {
                        tab_id,
                        command,
                        risk: "risky".into(),
                        thought_signature,
                    }));
                }
            }
            AiMessage::ChatToolApproveAlways(command) => {
                let Some(idx) = self.active_tab else {
                    return Task::none();
                };
                let tab_id = self.tabs.get(idx).map(|t| t._id);
                let first_token = command
                    .split_whitespace()
                    .next()
                    .unwrap_or("")
                    .to_string();
                let mut thought_signature = None;
                if let Some(tab) = self.tabs.get_mut(idx) {
                    if !first_token.is_empty()
                        && !tab
                            .chat_always_run_commands
                            .iter()
                            .any(|c| c == &first_token)
                    {
                        tab.chat_always_run_commands.push(first_token);
                    }
                    if let Some(last) = tab.chat_history.last()
                        && last.role == ChatRole::PendingTool
                        && last.content == command
                    {
                        // Same lift as the RUN path: the signature has to
                        // outlive the bubble that carried it.
                        thought_signature =
                            last.tool.as_ref().and_then(|t| t.thought_signature.clone());
                        tab.chat_history.pop();
                    }
                    // User retook control: start a fresh auto-exec chain.
                    tab.chat_auto_run_history.clear();
                    tab.chat_auto_run_streak = 0;
                }
                if let Some(tab_id) = tab_id {
                    return Task::done(Message::Ai(AiMessage::ChatToolExec {
                        tab_id,
                        command,
                        risk: "risky".into(),
                        thought_signature,
                    }));
                }
            }
            AiMessage::ChatToolDeny(command) => {
                // User said no. Drop the pending bubble and record the
                // refusal so the next user turn tells the model the command
                // was declined (otherwise it tends to re-propose the same
                // one). No stream is spawned; the user can also just type.
                if let Some(idx) = self.active_tab
                    && let Some(tab) = self.tabs.get_mut(idx)
                {
                    if let Some(last) = tab.chat_history.last()
                        && last.role == ChatRole::PendingTool
                        && last.content == command
                    {
                        tab.chat_history.pop();
                    }
                    tab.chat_history.push(ChatMessage::text(
                        ChatRole::System,
                        format!("[user declined to run: {command}]"),
                    ));
                    tab.chat_loading = false;
                }
            }
            AiMessage::ChatToolExec { tab_id, command, risk, thought_signature } => {
                // Run a command in the ORIGIN tab's terminal (routed by id so
                // a tool call can never land in another session). We record a
                // *running* Tool exchange now; the poll delivers `ChatToolResult`
                // which persists the output and fires the analysis followup.
                let Some(idx) = self.chat_tab_index(tab_id) else {
                    return Task::none();
                };
                let terminal = Arc::clone(&self.tabs[idx].active().terminal);
                let pane_label = self.tabs[idx].active().label.clone();
                let privacy_terms = self
                    .privacy_active_for_label(&pane_label)
                    .then(|| self.privacy_terms());
                // Captured before the detached poll below: the spawn
                // can't borrow `self` (same reason the terms are).
                let privacy_classes = self.privacy_classes();

                // Write the command, clearing any half-typed prompt line
                // first (Ctrl+U) so the AI's command isn't concatenated onto
                // stray user input. `write_ok` is false when the session is
                // gone, so we surface that instead of feeding back a stale
                // screen as if it were the command's output.
                let mut bytes = Vec::with_capacity(command.len() + 2);
                bytes.push(0x15); // Ctrl+U: discard a partially-typed line
                bytes.extend_from_slice(command.as_bytes());
                bytes.push(b'\n');
                let write_ok = {
                    let pane = self.tabs[idx].active();
                    if let Some(ref session) = pane.session {
                        session.write(&bytes).is_ok()
                    } else if let Ok(mut state) = pane.terminal.lock() {
                        state.write(&bytes);
                        true
                    } else {
                        false
                    }
                };
                if write_ok {
                    // AI-run commands are commands executed on the host:
                    // mirror them into the history capture like any other
                    // input (the direct write above preserves `write_ok`).
                    self.feed_input_capture(idx, &bytes);
                    // Their output belongs on screen too, so leave the
                    // scrollback the way typed input does (issue #111),
                    // gated on the tab still being the visible one (see
                    // `snap_tab_to_live_edge`).
                    self.snap_tab_to_live_edge(idx);
                }

                let tab = &mut self.tabs[idx];
                // Record this execution against the per-turn loop guard. A
                // later proposal of the same command (or one past the streak
                // cap) is then refused auto-exec by `ChatToolProposed`.
                // User-approval paths reset this first, so the count only ever
                // reflects commands run since the user last took control. Cap
                // the history length so a long agentic turn can't grow it
                // without bound; the repeated command stays within the window.
                tab.chat_auto_run_history.push(command.clone());
                if tab.chat_auto_run_history.len() > 50 {
                    tab.chat_auto_run_history.remove(0);
                }
                tab.chat_auto_run_streak += 1;

                if !write_ok {
                    // Session torn down: don't spawn a poll that would capture
                    // the dead screen and hand it to the model.
                    tab.chat_history.push(ChatMessage::text(
                        ChatRole::System,
                        "[terminal session is not connected; command not sent]",
                    ));
                    tab.chat_loading = false;
                    if Some(idx) == self.active_tab && self.chat_scroll_at_bottom {
                        return chat_scroll_to_end();
                    }
                    return Task::none();
                }

                // Push a *running* Tool exchange (output `None`). Until the
                // poll fills it in, the message builder renders it as flat
                // text, so an interrupted command never dangles a `tool_use`.
                let tool_id = format!("toolu_{}", Uuid::new_v4().simple());
                tab.chat_history.push(ChatMessage {
                    role: ChatRole::Tool,
                    content: format!("$ {}", command),
                    parsed_md: Vec::new(),
                    reasoning: String::new(),
                    tool: Some(crate::state::ToolExchange {
                        id: tool_id.clone(),
                        command: command.clone(),
                        risk,
                        output: None,
                        thought_signature,
                    }),
                });
                tab.chat_loading = true;

                // Poll the terminal off-thread until output stabilizes (no
                // change for 800ms) or a 15s cap, then deliver the captured
                // text as `ChatToolResult`. Redaction + trim + scrollback are
                // handled by the shared capture helper.
                let poll = Task::perform(
                    async move {
                        let poll_interval = std::time::Duration::from_millis(300);
                        let stable_threshold = std::time::Duration::from_millis(800);
                        let max_wait = std::time::Duration::from_secs(15);
                        let start_time = std::time::Instant::now();
                        let mut last_snapshot = String::new();
                        let mut stable_since = std::time::Instant::now();
                        let mut timed_out = false;
                        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                        loop {
                            let snapshot = capture_terminal_context(
                                &terminal,
                                40,
                                privacy_terms.as_deref(),
                                privacy_classes,
                            );
                            if snapshot != last_snapshot {
                                last_snapshot = snapshot;
                                stable_since = std::time::Instant::now();
                            } else if stable_since.elapsed() >= stable_threshold {
                                break;
                            }
                            if start_time.elapsed() >= max_wait {
                                timed_out = true;
                                break;
                            }
                            tokio::time::sleep(poll_interval).await;
                        }
                        // A command still producing output at the 15s cap
                        // (long build, `tail -f`, a TUI) would otherwise look
                        // "done" to the model; flag the partial capture.
                        if timed_out {
                            last_snapshot.push_str(
                                "\n[Note: command may still be running; output captured after 15s]",
                            );
                        }
                        last_snapshot
                    },
                    move |output| Message::Ai(AiMessage::ChatToolResult { tab_id, tool_id, output }),
                );
                // Track the poll so Stop / close / reset can cancel a command
                // whose output hasn't come back yet.
                return self.track_chat_task_for(tab_id, poll);
            }
            AiMessage::ChatToolResult { tab_id, tool_id, output } => {
                // Persist the captured output onto its running Tool exchange
                // (pairing it with the tool_use), then fire the analysis
                // followup from the now-complete history.
                let Some(idx) = self.chat_tab_index(tab_id) else {
                    return Task::none();
                };
                let mut attached = false;
                for m in self.tabs[idx].chat_history.iter_mut().rev() {
                    if let Some(t) = m.tool.as_mut()
                        && t.id == tool_id
                        && t.output.is_none()
                    {
                        t.output = Some(output);
                        attached = true;
                        break;
                    }
                }
                if !attached {
                    // The exchange was reset / cleared while polling; nothing
                    // to continue from.
                    self.tabs[idx].chat_loading = false;
                    return Task::none();
                }
                // The exchange is complete (command ran, output attached), so
                // it can be saved before the followup starts streaming over
                // it. Without this a conversation that ends on a tool call
                // would lose the call itself.
                self.flush_chat_history(idx);
                let followup = self.spawn_chat_stream_for(idx);
                return Task::batch(vec![chat_scroll_to_end(), followup]);
            }
        }
        Task::none()
    }
}

#[cfg(test)]
mod tests {
    use super::{classify_tool_gate, ToolGate, ToolGateInput};
    use crate::state::ChatMode;

    /// Convenience builder: Auto mode, everything else off (a plain
    /// risky/unclassified call). Individual tests override the fields they
    /// exercise via struct-update syntax.
    fn input() -> ToolGateInput {
        ToolGateInput {
            mode: ChatMode::Auto,
            allowed: false,
            has_chaining: false,
            risk_safe: false,
            obviously_destructive: false,
            streak_exceeded: false,
            already_auto_ran: false,
        }
    }

    #[test]
    fn unclassified_command_prompts() {
        assert_eq!(classify_tool_gate(input()), ToolGate::Prompt);
    }

    #[test]
    fn safe_command_goes_to_judge() {
        let gate = classify_tool_gate(ToolGateInput {
            risk_safe: true,
            ..input()
        });
        assert_eq!(gate, ToolGate::Judge);
    }

    #[test]
    fn allow_listed_simple_command_auto_execs() {
        let gate = classify_tool_gate(ToolGateInput {
            allowed: true,
            ..input()
        });
        assert_eq!(gate, ToolGate::AutoExec);
    }

    #[test]
    fn allow_listed_chained_command_falls_through_to_prompt() {
        // A trusted first token can't smuggle a chained command past the gate.
        let gate = classify_tool_gate(ToolGateInput {
            allowed: true,
            has_chaining: true,
            ..input()
        });
        assert_eq!(gate, ToolGate::Prompt);
    }

    #[test]
    fn repeated_safe_command_is_refused_auto_exec() {
        // The reported bug: the model re-proposing `docker --version`. A
        // safe command already auto-run this turn must be surfaced, not
        // auto-run again, so the loop can't continue unattended.
        let gate = classify_tool_gate(ToolGateInput {
            risk_safe: true,
            already_auto_ran: true,
            ..input()
        });
        assert_eq!(gate, ToolGate::Confirm);
    }

    #[test]
    fn over_long_streak_is_refused_even_when_safe() {
        // Backstop for a run of *different* safe commands.
        let gate = classify_tool_gate(ToolGateInput {
            risk_safe: true,
            streak_exceeded: true,
            ..input()
        });
        assert_eq!(gate, ToolGate::Confirm);
    }

    #[test]
    fn streak_cap_also_blocks_allow_listed_commands() {
        // A loop of an always-run command is still a loop.
        let gate = classify_tool_gate(ToolGateInput {
            allowed: true,
            streak_exceeded: true,
            ..input()
        });
        assert_eq!(gate, ToolGate::Confirm);
    }

    #[test]
    fn allow_listed_repeat_without_streak_still_auto_execs() {
        // Exact-repeat is NOT applied to allow-listed commands: the user
        // may legitimately want `ls` run more than once in a turn.
        let gate = classify_tool_gate(ToolGateInput {
            allowed: true,
            already_auto_ran: true,
            ..input()
        });
        assert_eq!(gate, ToolGate::AutoExec);
    }

    #[test]
    fn destructive_safe_command_is_refused_before_judge() {
        // The deterministic floor fires regardless of the judge.
        let gate = classify_tool_gate(ToolGateInput {
            risk_safe: true,
            obviously_destructive: true,
            ..input()
        });
        assert_eq!(gate, ToolGate::Confirm);
    }

    // ── Mode branches ──

    #[test]
    fn ask_mode_prompts_even_for_safe_commands() {
        // Ask never auto-runs; a model-claimed safe command still needs a
        // click instead of going to the judge.
        let gate = classify_tool_gate(ToolGateInput {
            mode: ChatMode::Ask,
            risk_safe: true,
            ..input()
        });
        assert_eq!(gate, ToolGate::Prompt);
    }

    #[test]
    fn ask_mode_still_honors_allow_list() {
        // An explicit "always run X" grant applies in every mode.
        let gate = classify_tool_gate(ToolGateInput {
            mode: ChatMode::Ask,
            allowed: true,
            ..input()
        });
        assert_eq!(gate, ToolGate::AutoExec);
    }

    #[test]
    fn plan_mode_lets_read_only_reach_the_judge() {
        // Plan allows investigation: a safe, non-destructive command goes to
        // the judge just like Auto.
        let gate = classify_tool_gate(ToolGateInput {
            mode: ChatMode::Plan,
            risk_safe: true,
            ..input()
        });
        assert_eq!(gate, ToolGate::Judge);
    }

    #[test]
    fn plan_mode_surfaces_writes_instead_of_running_them() {
        // A risky / unclassified (write) command is surfaced, never
        // auto-run, so a Plan session stays read-only by default.
        let gate = classify_tool_gate(ToolGateInput {
            mode: ChatMode::Plan,
            risk_safe: false,
            ..input()
        });
        assert_eq!(gate, ToolGate::Prompt);
    }

    #[test]
    fn plan_mode_surfaces_destructive_safe_commands() {
        // Even a model-claimed-safe command that trips the destructive floor
        // is surfaced under Plan, not sent to the judge.
        let gate = classify_tool_gate(ToolGateInput {
            mode: ChatMode::Plan,
            risk_safe: true,
            obviously_destructive: true,
            ..input()
        });
        assert_eq!(gate, ToolGate::Prompt);
    }

    // ── Message reconstruction (build_provider_messages) ──

    use super::build_provider_messages;
    use crate::state::{ChatMessage, ChatRole, ToolExchange};

    fn tool_msg(id: &str, command: &str, risk: &str, output: Option<&str>) -> ChatMessage {
        ChatMessage {
            role: ChatRole::Tool,
            content: format!("$ {command}"),
            parsed_md: Vec::new(),
            reasoning: String::new(),
            tool: Some(ToolExchange {
                id: id.into(),
                command: command.into(),
                risk: risk.into(),
                output: output.map(str::to_string),
                thought_signature: None,
            }),
        }
    }

    #[test]
    fn completed_tool_merges_preamble_then_emits_result() {
        // Assistant preamble + completed tool → ONE assistant turn carrying
        // [text, tool_use], then a `tool` turn with the matching result.
        let history = vec![
            ChatMessage::text(ChatRole::Assistant, "Let me check."),
            tool_msg("t1", "df -h", "safe", Some("Filesystem ... 40% /")),
        ];
        let out = build_provider_messages(&history);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].role, "assistant");
        assert_eq!(out[0].content.as_str(), Some("Let me check."));
        let tu = out[0].tool_use.as_ref().expect("tool_use on assistant turn");
        assert_eq!(tu.id, "t1");
        assert_eq!(tu.command, "df -h");
        assert_eq!(tu.risk, "safe");
        assert_eq!(out[1].role, "tool");
        let tr = out[1].tool_result.as_ref().expect("tool_result turn");
        assert_eq!(tr.id, "t1"); // pairs with the tool_use id
        assert_eq!(tr.output, "Filesystem ... 40% /");
    }

    #[test]
    fn completed_tool_without_preamble_is_standalone() {
        // No assistant text before the tool → a standalone assistant turn
        // carrying just the tool_use (empty text), then the result.
        let history = vec![
            ChatMessage::text(ChatRole::User, "how much disk is free?"),
            tool_msg("t2", "df -h", "safe", Some("ok")),
        ];
        let out = build_provider_messages(&history);
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].role, "user");
        assert_eq!(out[1].role, "assistant");
        assert_eq!(out[1].content.as_str(), Some(""));
        assert!(out[1].tool_use.is_some());
        assert_eq!(out[2].role, "tool");
    }

    #[test]
    fn running_tool_is_flat_never_a_dangling_tool_use() {
        // A still-running exchange (output None) must NOT produce a tool_use
        // block, or the next request would 400 on a dangling tool_use.
        let history = vec![
            ChatMessage::text(ChatRole::Assistant, "Checking."),
            tool_msg("t3", "sleep 30", "safe", None),
        ];
        let out = build_provider_messages(&history);
        assert!(out.iter().all(|m| m.tool_use.is_none() && m.tool_result.is_none()));
        assert_eq!(out.last().unwrap().role, "user");
        assert_eq!(
            out.last().unwrap().content.as_str(),
            Some("[Running command: sleep 30]")
        );
    }

    #[test]
    fn deny_and_error_and_pending_leave_no_tool_use() {
        // The not-run paths stay flat: a declined command is a System note, and
        // Error / PendingTool bubbles are dropped entirely. No tool_use leaks.
        let history = vec![
            ChatMessage::text(ChatRole::Assistant, "I could run this."),
            ChatMessage::text(ChatRole::PendingTool, "rm -rf /"),
            ChatMessage::text(ChatRole::System, "[user declined to run: rm -rf /]"),
            ChatMessage::text(ChatRole::Error, "network blip"),
            ChatMessage::text(ChatRole::User, "never mind"),
        ];
        let out = build_provider_messages(&history);
        assert!(out.iter().all(|m| m.tool_use.is_none() && m.tool_result.is_none()));
        // assistant text + declined-note(as user) + user follow-up; the
        // PendingTool and Error bubbles are gone.
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].role, "assistant");
        assert_eq!(out[1].role, "user");
        assert_eq!(out[2].role, "user");
    }

    #[test]
    fn two_tools_back_to_back_stay_paired() {
        // A second tool right after a result (no text between) gets its own
        // standalone assistant turn: every tool_use has exactly one following
        // tool_result, so the sequence is valid.
        let history = vec![
            ChatMessage::text(ChatRole::Assistant, "First."),
            tool_msg("a", "ls", "safe", Some("file1")),
            tool_msg("b", "pwd", "safe", Some("/home")),
        ];
        let out = build_provider_messages(&history);
        // assistant[First,tool a] / tool a-result / assistant[tool b] / tool b-result
        assert_eq!(out.len(), 4);
        assert_eq!(out[0].tool_use.as_ref().unwrap().id, "a");
        assert_eq!(out[1].tool_result.as_ref().unwrap().id, "a");
        assert!(out[2].tool_use.is_some() && out[2].content.as_str() == Some(""));
        assert_eq!(out[2].tool_use.as_ref().unwrap().id, "b");
        assert_eq!(out[3].tool_result.as_ref().unwrap().id, "b");
    }
}
