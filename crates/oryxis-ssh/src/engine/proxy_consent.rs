//! Answering [`ProxyCommandQuery`] without a user.
//!
//! `proxy_command` refuses to spawn anything unless an approval channel
//! says yes, which is right for a line that may have arrived over sync,
//! and wrong for the many dials that legitimately run one with nobody
//! watching: boot-time auto-start forwards, snapshot sync rounds, the
//! monitor dashboard, the MCP server. Those still have the answer, they
//! just have no way to ASK it: the approvals live in the vault, and the
//! vault handle cannot cross into the dial task.
//!
//! So they take a snapshot of the approved fingerprints first and let
//! this responder answer from it. Every consumer shares one
//! implementation of "approved means the fingerprint is in the set",
//! which is what keeps the app's and the MCP server's answers identical
//! for the same vault, exactly like `resolve_disk_key` does for keys.

use std::collections::HashSet;

use oryxis_core::models::connection::proxy_command_fingerprint;

use super::{ProxyCommandAskSender, ProxyCommandQuery};

/// Build an approval channel that answers from `trusted` alone: a
/// command whose fingerprint is in the set spawns, anything else is
/// refused.
///
/// An empty set therefore refuses everything, which is the correct
/// reading of a locked or unreadable vault.
pub fn trusted_only_proxy_command_ask(trusted: HashSet<String>) -> ProxyCommandAskSender {
    let (tx, mut rx) = tokio::sync::mpsc::channel::<(
        ProxyCommandQuery,
        tokio::sync::oneshot::Sender<bool>,
    )>(1);
    tokio::spawn(async move {
        while let Some((query, resp_tx)) = rx.recv().await {
            let approved = trusted.contains(&proxy_command_fingerprint(&query.command));
            if !approved {
                tracing::warn!(
                    "refusing an unapproved command proxy for {}:{} on an unattended dial",
                    query.target_host,
                    query.target_port
                );
            }
            let _ = resp_tx.send(approved);
        }
    });
    tx
}
