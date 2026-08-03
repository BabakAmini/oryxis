//! Global shortcuts: the keyboard and mouse routers, plus what they aim
//! at.
//!
//! Was one 1526-line `impl Oryxis`, which had become where things landed
//! when no other file claimed them (spawning a child window is the
//! clearest example). Split by who asks:
//!
//! - [`keyboard`]: keypress -> binding -> action.
//! - [`mouse`]: bindable buttons and their owners.
//! - [`modals`]: which surface owns the keyboard at all.
//! - [`targets`]: what a shortcut resolves to, and its label.
//! - [`process`]: launching another Oryxis (not a shortcut; see there).

mod targets;
mod modals;
mod process;
mod keyboard;
mod mouse;

use iced::Task;

use crate::app::Message;

/// Dispatch a `Message::ToastClear` after `secs` seconds.
///
/// Lives on the module rather than in one of the routers: the capture
/// branch and `dispatch_terminal` both reach for it, and it is the only
/// thing in here that is not a method on `Oryxis`.
pub(crate) fn toast_clear_after_secs(secs: u64) -> Task<Message> {
    Task::perform(
        async move {
            tokio::time::sleep(std::time::Duration::from_secs(secs)).await;
        },
        |_| Message::ToastClear,
    )
}
