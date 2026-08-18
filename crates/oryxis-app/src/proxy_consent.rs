//! Answering the engine's command-proxy approval question on dials that
//! nobody is watching.
//!
//! A `ProxyType::Command` proxy is spawned locally, before the SSH
//! handshake, from data that may have arrived over sync or through an
//! imported file, so `oryxis-ssh` refuses to spawn one unless something
//! approves it (see `engine::connect::proxy_command`). On a connect the
//! user triggered, that something is the modal: the engine's query rides
//! the dial's stream up to `SshProxyCommandVerify`, which looks the line
//! up in the vault and only asks when it has never been approved here.
//!
//! Boot-time auto-start forwards, snapshot sync and the monitor
//! dashboard have no such stream and no user to ask. They still must run
//! a line the user DID approve (otherwise the gate would quietly break
//! every legitimate `ProxyCommand` host the moment it is dialed in the
//! background), so they answer from a snapshot of the approvals taken on
//! the UI thread before the task is spawned, and refuse everything else.
//!
//! The snapshot is exactly that: `trusted_proxy_commands` in the vault
//! stays the authority, this is a copy taken per dial because
//! `VaultStore` owns a `rusqlite::Connection` and cannot cross into the
//! task. Racing a revocation therefore costs at most one already-started
//! dial, which is the same window the modal path has once its prompt is
//! on screen.
//!
//! Answering FROM the snapshot lives in `oryxis-ssh`
//! (`trusted_only_proxy_command_ask`) rather than here, so the MCP
//! server, which dials the same vault with no UI at all, cannot drift
//! from the app on what counts as approved.

use std::collections::HashSet;

use crate::app::Oryxis;

impl Oryxis {
    /// Approve a command proxy the local user just TYPED, in the host
    /// editor's proxy fields.
    ///
    /// Typing a line and then being asked whether you meant it is not a
    /// security decision, it is a dialog people learn to dismiss, which
    /// is exactly how a gate stops protecting anything. What the gate is
    /// for is a line nobody here typed.
    ///
    /// IMPORTS are deliberately not that, and none of them call this:
    /// `~/.ssh/config`, PuTTY and WinSCP all hand over a file the user
    /// picked but did not necessarily read, and "a colleague's config"
    /// is the same attack with a different courier. They cost one prompt
    /// on the first dial of each imported host, which shows the very
    /// line the import never made them look at. The `.oryxis` import and
    /// sync's `apply_records` are the same story and equally absent.
    ///
    /// A no-op for every other proxy kind, so callers can hand it
    /// whatever the form produced.
    pub(crate) fn trust_authored_proxy_command(
        &self,
        proxy: Option<&oryxis_core::models::connection::ProxyConfig>,
        label: &str,
    ) {
        let Some(oryxis_core::models::connection::ProxyType::Command(cmd)) =
            proxy.map(|p| &p.proxy_type)
        else {
            return;
        };
        if let Some(vault) = &self.vault
            && let Err(e) = vault.trust_proxy_command(cmd, label)
        {
            tracing::warn!("failed to record an authored command proxy: {e}");
        }
    }

    /// Fingerprints of every command proxy this device has approved.
    ///
    /// A locked or absent vault yields an empty set, which refuses
    /// everything: the auto-start sweep runs right after unlock, and a
    /// vault that cannot answer must never be read as approval.
    pub(crate) fn trusted_proxy_commands(&self) -> HashSet<String> {
        self.vault
            .as_ref()
            .and_then(|v| v.list_trusted_proxy_commands().ok())
            .map(|list| list.into_iter().map(|t| t.fingerprint).collect())
            .unwrap_or_default()
    }
}
