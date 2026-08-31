//! Docker manager.
//!
//! A terminal-sidebar tab that lists Docker containers and images on
//! the focused pane's host, starts/stops/restarts containers, and
//! manages docker-compose projects detected in the shell's working
//! directory.
//!
//! Everything runs docker itself on an exec channel multiplexed on the
//! pane's live SSH session, same approach as the tmux manager. Nothing
//! is installed on the host.
//!
//! The feature is off by default and hides ALL of its UI when off
//! (`prefs.docker_manager`), per the optional-features rule.

pub(crate) mod model;
pub(crate) mod probe;

pub(crate) use model::DockerState;
