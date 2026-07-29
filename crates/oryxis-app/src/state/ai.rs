//! AI assistant feature state: provider/model/key settings plus the editable
//! system prompt. Grouped off the `Oryxis` god-struct as part of the
//! modules-by-feature direction (field grouping only).

use iced::widget::text_editor;

/// How the assistant is allowed to run commands in a terminal session.
/// Chosen per-tab (travels with the conversation) and seeded from the
/// global `ai_default_mode` setting. The gate ([`crate::dispatch_ai`]
/// `classify_tool_gate`) branches on this before any command runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum ChatMode {
    /// Read-only investigation. The model may run introspection commands
    /// (subject to the same floor + judge as `Auto`) but every
    /// state-changing command is blocked and surfaced, never executed.
    /// For "figure out what's wrong and propose a fix" without touching
    /// anything.
    Plan,
    /// Every command, safe or not, needs explicit RUN / ALWAYS RUN / DENY.
    /// No unattended auto-exec; the allow-list still lets a command the
    /// user marked "always run" through.
    Ask,
    /// The full pipeline: allow-list + deterministic floor + independent
    /// judge auto-execute read-only commands; risky / destructive ones are
    /// surfaced for approval.
    #[default]
    Auto,
}

impl ChatMode {
    /// Stable string used to persist the mode in the settings table.
    pub(crate) fn as_setting(self) -> &'static str {
        match self {
            ChatMode::Plan => "plan",
            ChatMode::Ask => "ask",
            ChatMode::Auto => "auto",
        }
    }

    /// Parse a persisted setting value back to a mode, defaulting to `Auto`
    /// for an empty or unrecognized value.
    pub(crate) fn from_setting(s: &str) -> Self {
        match s {
            "plan" => ChatMode::Plan,
            "ask" => ChatMode::Ask,
            _ => ChatMode::Auto,
        }
    }

    /// i18n key for the mode's short label (picker + pill).
    pub(crate) fn label_key(self) -> &'static str {
        match self {
            ChatMode::Plan => "ai_mode_plan",
            ChatMode::Ask => "ai_mode_ask",
            ChatMode::Auto => "ai_mode_auto",
        }
    }
}

/// All AI-assistant settings + the editable system-prompt buffer. The
/// scalar settings hydrate from the `settings` table on boot; `api_key_set`
/// mirrors whether an encrypted key exists, and `system_prompt` holds the
/// live `text_editor` buffer (which is why this struct is not `Clone`).
#[derive(Debug)]
pub(crate) struct AiState {
    /// Whether the AI assistant sidebar is enabled.
    pub(crate) enabled: bool,
    /// Provider id, e.g. `"anthropic"` / `"openai"`.
    pub(crate) provider: String,
    /// Model id sent with each request.
    pub(crate) model: String,
    /// In-memory API key while editing the field. The persisted copy is
    /// encrypted per-field in the vault (the `set_user_password` machinery).
    pub(crate) api_key: String,
    /// Mirrors whether an encrypted key is stored, for the masked UI.
    pub(crate) api_key_set: bool,
    /// Optional override base URL for OpenAI-compatible endpoints.
    pub(crate) api_url: String,
    /// Editable system-prompt buffer. `text_editor::Content` is not `Clone`,
    /// so it lives here rather than in a cloneable form struct.
    pub(crate) system_prompt: text_editor::Content,
    /// Let reasoning models think before answering. Off by default because
    /// the chain-of-thought is billed to the user and never shown: it is
    /// output tokens on the turn that produces it, and input tokens on
    /// every later request of the conversation, since providers like
    /// DeepSeek require it replayed (see `ChatMsg::reasoning`). Turning it
    /// on simply lets each provider's own default stand.
    pub(crate) reasoning: bool,
}

impl Default for AiState {
    fn default() -> Self {
        Self {
            enabled: false,
            provider: "anthropic".into(),
            model: "claude-sonnet-5".into(),
            api_key: String::new(),
            api_key_set: false,
            api_url: String::new(),
            system_prompt: text_editor::Content::new(),
            reasoning: false,
        }
    }
}
