//! Known-hosts management (delete one / clear all, with confirms), wrapped by [`crate::messages::Message::KnownHost`]. Handled by `Oryxis::handle_known_hosts`.

#[derive(Debug, Clone)]
pub enum KnownHostMessage {
    /// Open the confirm dialog before deleting a single known host.
    RequestDeleteKnownHost(usize),
    DeleteKnownHost(usize),
    /// Open the confirm dialog before clearing every known host.
    RequestClearAllKnownHosts,
    ClearAllKnownHosts,
}
