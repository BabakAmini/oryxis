pub mod backend;
pub mod highlight_rules;
pub mod host_clipboard;
pub mod input_tracker;
pub mod osc;
pub mod prompt_detect;
pub mod screen_title;
pub mod trigger;
pub mod widget;
pub mod pty;
pub mod colors;
pub mod mouse;

pub use backend::{set_clipboard_access, set_default_scrollback, TerminalBackend, DEFAULT_WORD_DELIMITERS};
pub use host_clipboard::{take_clipboard_requests, ClipboardRequest, ClipboardSink};
pub use input_tracker::{InputTracker, SubmittedLine};
pub use highlight_rules::{parse_hex_color, CompiledRule, CompiledRules};
pub use osc::{PositionedShellMark, Progress, ShellMark};
pub use prompt_detect::PasswordPrompt;
pub use trigger::TriggerHit;
pub use colors::{TerminalPalette, TerminalTheme};
pub use widget::{
    ime_caret_rect, ipv4_is_private_or_loopback, ipv6_is_local, looks_like_ipv6,
    take_privacy_mask_drawn, wrap_paste, BackgroundImage, BgFit, HoveredLink, NetHud,
    PrivacyClasses, RightClickAction, TerminalState, TerminalView,
};
pub use pty::PtyHandle;

/// DECRST/DECSET sequence the app feeds a pane the moment a fresh
/// session attaches (`Oryxis::wire_connected_pane`): clears the
/// emulator modes a previous session's apps may have left armed —
/// mouse tracking (1000/1002/1003), SGR mouse encoding (1006),
/// bracketed paste (2004) — and re-shows a cursor the old app hid.
/// A fresh shell never re-issues those requests, so leaving them
/// armed made the widget keep synthesizing mouse reports into a
/// shell that did not ask for them, whose echo of those reports
/// landed on screen as garbage after a reconnect.
pub const SESSION_ATTACH_MODE_RESET: &[u8] =
    b"\x1b[?1000;1002;1003;1006l\x1b[?2004l\x1b[?25h";

// The backend exposes `Term` and grid types in its public surface
// (`TerminalBackend::term`), so consumers that inspect the grid (the
// app's session player tests, the harness) need the crate's types
// without pinning their own copy of the dependency.
pub use alacritty_terminal;
