//! Well-known endpoints of the Oryxis ssh-agent server.
//!
//! Single source of truth shared by the server side
//! (`oryxis-app::agent_server`, which binds these) and the client side
//! (`oryxis-ssh`, which includes them as fallback candidates when
//! authenticating via agent). Both names are part of the product
//! contract: users point `SSH_AUTH_SOCK` / `IdentityAgent` at them, so
//! they are frozen like a wire format.

/// The fixed Windows named pipe the agent server binds. The standard
/// `\\.\pipe\openssh-ssh-agent` name is only ever taken via the opt-in
/// alias setting; this one is always ours while the server runs.
pub const WINDOWS_AGENT_PIPE: &str = r"\\.\pipe\oryxis-ssh-agent";

/// `~/.oryxis/agent.sock`, the fixed Unix socket path the user points
/// `SSH_AUTH_SOCK` at. `None` only when the home directory cannot be
/// resolved.
pub fn unix_agent_socket_path() -> Option<std::path::PathBuf> {
    Some(crate::paths::oryxis_dir()?.join("agent.sock"))
}
