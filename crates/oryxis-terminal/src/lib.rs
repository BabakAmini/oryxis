pub mod backend;
pub mod input_tracker;
pub mod osc;
pub mod screen_title;
pub mod widget;
pub mod pty;
pub mod colors;
pub mod mouse;

pub use backend::{set_clipboard_access, set_default_scrollback, TerminalBackend, DEFAULT_WORD_DELIMITERS};
pub use input_tracker::{InputTracker, SubmittedLine};
pub use osc::{PositionedShellMark, Progress, ShellMark};
pub use colors::{TerminalPalette, TerminalTheme};
pub use widget::{
    ime_caret_rect, ipv4_is_private_or_loopback, ipv6_is_local, looks_like_ipv6,
    take_privacy_mask_drawn, wrap_paste, HoveredLink, NetHud,
    PrivacyClasses, RightClickAction, TerminalState, TerminalView,
};
pub use pty::PtyHandle;

// The backend exposes `Term` and grid types in its public surface
// (`TerminalBackend::term`), so consumers that inspect the grid (the
// app's session player tests, the harness) need the crate's types
// without pinning their own copy of the dependency.
pub use alacritty_terminal;
