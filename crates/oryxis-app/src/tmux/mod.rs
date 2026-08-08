//! tmux session manager (issue #116).
//!
//! A terminal-sidebar tab that lists the tmux sessions running on the
//! focused pane's host, and creates, attaches to and kills them.
//!
//! Nothing is installed on the host, no rc file is written and nothing
//! is injected into the shell: the listing, the create and the kill all
//! run tmux itself on an exec channel multiplexed on the pane's live
//! SSH session, the same way the monitor probe and the SFTP channel do.
//! Attaching is the one action that reaches the user's own shell,
//! because it has to: it is the command they would type, sent when they
//! click, into the pane the tab sits beside.
//!
//! Every session name is text the REMOTE HOST printed, so it is quoted
//! (`oryxis_archive::quote::sh_quote`) at every boundary, and a name
//! carrying a line break is refused rather than quoted.
//!
//! The feature is off by default and hides ALL of its UI when off
//! (`prefs.tmux_manager`), per the optional-features rule.

pub(crate) mod model;
pub(crate) mod probe;

pub(crate) use model::TmuxState;
