//! AI settings + AI chat sidebar + terminal-sidebar tab controls,
//! wrapped by [`crate::messages::Message::Ai`]. Handled by `Oryxis::handle_ai`.

use iced::widget::text_editor;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub enum AiMessage {
    SelectTerminalSidebarTab(crate::state::TerminalSidebarTab),
    SidebarSnippetSearchChanged(String),
    /// Toggle the Snippets-tab sort popover.
    ToggleSidebarSort,
    /// Toggle the Snippets-tab search field (autofocuses on open, clears
    /// the needle on close).
    ToggleSidebarSearch,
    ToggleAiEnabled,
    /// Toggle "let reasoning models think before answering". Off by default:
    /// the chain-of-thought is billed and never shown, and it is billed
    /// twice once providers require it replayed (see `ChatStreamReasoning`).
    ToggleAiReasoning,
    /// Toggle saving conversations to the vault. On by default; turning it
    /// off stops new turns from being written and leaves what is already
    /// stored alone (the History screen keeps its own delete).
    ToggleAiSaveHistory,
    AiProviderChanged(String),
    AiModelChanged(String),
    AiApiKeyChanged(super::Redacted),
    AiApiUrlChanged(String),
    AiSystemPromptAction(text_editor::Action),
    SaveAiApiKey,
    ToggleChatSidebar,
    ChatInputAction(text_editor::Action),
    ChatScrolled(f32),
    ChatResetConversation,
    ChatSidebarResizeStart,
    ChatSidebarResizeStop,
    /// User picked a Plan / Ask / Auto mode from the chat sidebar. Applies to
    /// the active tab's conversation and is persisted as the default for new
    /// tabs (`ai_default_mode`).
    ChatModeChanged(crate::state::ChatMode),
    SendChat,
    /// Incremental text delta from the streaming AI response. Appended
    /// to the origin tab's active assistant bubble so the user sees tokens
    /// land as they're generated. `tab_id` routes the delta to the tab that
    /// started the stream, not `active_tab`, so switching tabs mid-stream
    /// can't corrupt another conversation or run a command on the wrong host.
    ChatStreamChunk { tab_id: Uuid, delta: String },
    /// Incremental chain-of-thought delta (DeepSeek thinking mode), routed
    /// like `ChatStreamChunk` but accumulated apart from the answer: it is
    /// never rendered, and exists because the provider demands it back on
    /// the conversation's next request (issue #105).
    ChatStreamReasoning { tab_id: Uuid, delta: String },
    /// Terminal sentinel for `ChatStreamChunk`, clears the loading
    /// state and finalises the message (markdown re-parse, scroll snap)
    /// on the origin tab.
    ChatStreamDone { tab_id: Uuid },
    ChatError { tab_id: Uuid, error: String },
    /// User clicked Stop while the assistant was streaming or
    /// auto-running tools. Aborts the in-flight chat task (and the
    /// detached tool-followup pipeline it feeds) so a runaway
    /// tool loop can be interrupted by hand.
    ChatStop,
    /// Re-send the last user message, used by the Retry button on an
    /// error bubble. Pops the most recent error and replays.
    ChatRetry,
    /// Run a command in the origin tab's terminal, recording a running `Tool`
    /// exchange. `tab_id` pins it to the tab that owns the conversation so a
    /// tool call never lands in the wrong session; `risk` is carried so the
    /// reconstructed `tool_use` block reports the model's classification.
    ChatToolExec {
        tab_id: Uuid,
        command: String,
        risk: String,
        /// Gemini's per-call `thoughtSignature`, threaded from the proposal
        /// through every gate (auto-exec, judge, user approval) so it reaches
        /// the `ToolExchange` that the replay reads. Losing it here would
        /// 400 the request after the command runs, which is the worst place
        /// to fail. See `crate::ai::ToolUseMsg::thought_signature`.
        thought_signature: Option<String>,
    },
    /// The terminal poll for a running tool exchange finished. Persists the
    /// captured `output` onto the exchange with id `tool_id` (pairing it with
    /// its `tool_use`), then fires the analysis followup. Re-introduced as the
    /// hook that makes tool results durable in history for real tool blocks.
    ChatToolResult { tab_id: Uuid, tool_id: String, output: String },
    /// AI proposed a tool call. Carries the command + `risk` it
    /// self-classified ("safe" / "risky") plus the origin `tab_id`. Safe
    /// commands still have to clear the independent auto-exec judge before
    /// running unattended; risky ones (and ones the model failed to
    /// classify) are queued as a `PendingTool` bubble with RUN / ALWAYS RUN
    /// / DENY buttons.
    ChatToolProposed {
        tab_id: Uuid,
        command: String,
        risk: String,
        /// Gemini's opaque per-call `thoughtSignature`, carried through to
        /// the `ToolExchange` so the replay can echo it back. `None` on the
        /// other providers. See `crate::ai::ToolUseMsg::thought_signature`.
        thought_signature: Option<String>,
    },
    /// The independent safety judge (or a mode/loop guard) declined to
    /// auto-run a command. Surface it for explicit approval like a risky one.
    ChatToolGuardBlocked {
        tab_id: Uuid,
        command: String,
        /// Gemini's per-call `thoughtSignature`, carried so the pending
        /// bubble this raises can hand it back when the user approves.
        thought_signature: Option<String>,
    },
    /// User clicked RUN on a pending tool prompt, execute once.
    ChatToolApprove(String),
    /// User clicked ALWAYS RUN, add this command's first token to the
    /// tab's allow-list and execute now.
    ChatToolApproveAlways(String),
    /// User clicked DENY on a pending tool prompt, drop the bubble and
    /// record the refusal so the next turn tells the model it was declined.
    ChatToolDeny(String),
}
