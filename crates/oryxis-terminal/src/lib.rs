pub mod backend;
pub mod input_tracker;
pub mod osc;
pub mod widget;
pub mod pty;
pub mod colors;
pub mod mouse;

pub use backend::{set_clipboard_access, set_default_scrollback, TerminalBackend, DEFAULT_WORD_DELIMITERS};
pub use input_tracker::{InputTracker, SubmittedLine};
pub use osc::{PositionedShellMark, Progress, ShellMark};
pub use colors::{TerminalPalette, TerminalTheme};
pub use widget::{
    ime_caret_rect, ipv4_is_private_or_loopback, looks_like_ipv6, quad_dot_is_version_like,
    wrap_paste, NetHud, RightClickAction, TerminalState,
    TerminalView,
};
pub use pty::PtyHandle;
