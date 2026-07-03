//! `impl Oryxis` block for SSH-connect plumbing, credential resolution,
//! jump-host resolver assembly, and the host-key verification callback.
//! Pulled out of `app.rs` to keep the main module from drifting past
//! ten thousand lines.

use std::sync::{Arc, Mutex};

use oryxis_core::models::connection::{AuthMethod, Connection};

use crate::app::Oryxis;

impl Oryxis {
    /// Resolve `(password, private_key_pem)` for a connection, same
    /// rules as `Message::ConnectSsh`: prefer identity-linked credentials,
    /// fall back to per-connection vault entries.
    pub(crate) fn resolve_credentials(
        &self,
        conn: &Connection,
    ) -> (Option<String>, Option<String>) {
        if let Some(iid) = conn.identity_id {
            let id_pw = self
                .vault
                .as_ref()
                .and_then(|v| v.get_identity_password(&iid).ok().flatten());
            let identity = self.identities.iter().find(|i| i.id == iid);
            let id_key = identity.and_then(|i| i.key_id).and_then(|kid| {
                self.vault
                    .as_ref()
                    .and_then(|v| v.get_key_private(&kid).ok().flatten())
            });
            (id_pw, id_key)
        } else {
            let pw = self
                .vault
                .as_ref()
                .and_then(|v| v.get_connection_password(&conn.id).ok().flatten());
            let pk = if conn.auth_method == AuthMethod::Key
                || conn.auth_method == AuthMethod::Auto
            {
                conn.key_id.and_then(|kid| {
                    self.vault
                        .as_ref()
                        .and_then(|v| v.get_key_private(&kid).ok().flatten())
                })
            } else {
                None
            };
            (pw, pk)
        }
    }

    /// Build a `ConnectionResolver` covering the jump-host chain of the
    /// given connection. `None` when there's no chain.
    pub(crate) fn make_jump_resolver(
        &self,
        conn: &Connection,
    ) -> Option<oryxis_ssh::ConnectionResolver> {
        if conn.jump_chain.is_empty() {
            return None;
        }
        let mut passwords = std::collections::HashMap::new();
        let mut keys = std::collections::HashMap::new();
        let mut proxies = std::collections::HashMap::new();
        for jid in &conn.jump_chain {
            if let Some(vault) = &self.vault
                && let Ok(Some(pw)) = vault.get_connection_password(jid)
            {
                passwords.insert(*jid, pw);
            }
            if let Some(jconn) = self.connections.iter().find(|c| c.id == *jid)
                && let Some(kid) = jconn.key_id
                && let Some(vault) = &self.vault
                && let Ok(Some(pk)) = vault.get_key_private(&kid)
            {
                keys.insert(*jid, pk);
            }
            // Resolve the jump host's effective proxy (identity-based or
            // inline) so the engine's first-hop dial can route through it.
            // Only matters for the first jump but we hydrate every jump's
            // entry, cheap and keeps the resolver self-contained.
            if let Some(jconn) = self.connections.iter().find(|c| c.id == *jid)
                && let Some(vault) = &self.vault
                && let Ok(Some(p)) = vault.resolve_proxy(jconn)
            {
                proxies.insert(*jid, p);
            }
        }
        Some(oryxis_ssh::ConnectionResolver {
            connections: self.connections.clone(),
            passwords,
            private_keys: keys,
            proxies,
        })
    }

    /// Build the host-key verification callback against the in-memory
    /// `known_hosts` snapshot. Read-only, known-host writes still happen
    /// in the connect handler itself.
    pub(crate) fn make_host_key_check(&self) -> oryxis_ssh::HostKeyCheckCallback {
        let snapshot = Arc::new(Mutex::new(self.known_hosts.clone()));
        Arc::new(move |host, port, key_type, fingerprint| {
            let hosts = match snapshot.lock() {
                Ok(g) => g,
                Err(p) => p.into_inner(),
            };
            // Per (host, port, key_type): a different offered algorithm is
            // Unknown (verify + accept), not a "Changed" MITM warning.
            if let Some(existing) = hosts
                .iter()
                .find(|h| h.hostname == host && h.port == port && h.key_type == key_type)
            {
                if existing.fingerprint != fingerprint {
                    return oryxis_ssh::HostKeyStatus::Changed {
                        old_fingerprint: existing.fingerprint.clone(),
                    };
                }
                return oryxis_ssh::HostKeyStatus::Known;
            }
            oryxis_ssh::HostKeyStatus::Unknown
        })
    }

    /// Find a connection by its display label, looking at saved hosts
    /// first and quick-connect entries second (so a label collision always
    /// resolves to the vault-backed host). The label-keyed reconnect and
    /// status paths use this to cover ad-hoc tabs too.
    pub(crate) fn any_connection_by_label(&self, label: &str) -> Option<&Connection> {
        self.connections
            .iter()
            .find(|c| c.label == label)
            .or_else(|| {
                self.quick_connects
                    .values()
                    .map(|e| &e.conn)
                    .find(|c| c.label == label)
            })
    }

    /// Drop quick-connect entries no longer referenced by any pane or by
    /// an in-flight connection progress. Called after closing tabs/panes
    /// so typed credentials don't outlive the session that used them.
    pub(crate) fn prune_quick_connects(&mut self) {
        if self.quick_connects.is_empty() {
            return;
        }
        let mut live: std::collections::HashSet<uuid::Uuid> = std::collections::HashSet::new();
        for tab in &self.tabs {
            for pane in tab.pane_grid.panes.values() {
                if let crate::state::PaneOrigin::QuickHost(id) = &pane.origin {
                    live.insert(*id);
                }
            }
        }
        if let Some(progress) = &self.connecting
            && let crate::state::ProgressOrigin::Quick(id) = progress.origin
        {
            live.insert(id);
        }
        self.quick_connects.retain(|id, _| live.contains(id));
    }

    /// Overlay a quick-connect entry's typed credentials on top of the
    /// vault hydration (which always misses for ephemeral ids): password,
    /// TOTP secret, and the inline-proxy password `resolve_proxy` cannot
    /// know. Vault-sourced values (a linked identity, a saved proxy
    /// identity) keep precedence, matching saved-host semantics.
    pub(crate) fn apply_quick_entry_secrets(
        &self,
        quick_id: uuid::Uuid,
        conn: &mut Connection,
        password: &mut Option<String>,
        totp_secret: &mut Option<String>,
    ) {
        let Some(entry) = self.quick_connects.get(&quick_id) else {
            return;
        };
        if password.is_none() {
            *password = entry.password.clone();
        }
        if totp_secret.is_none() {
            *totp_secret = entry.totp_secret.clone();
        }
        if conn.proxy_identity_id.is_none()
            && let Some(proxy) = conn.proxy.as_mut()
            && proxy.password.is_none()
        {
            proxy.password = entry.proxy_password.clone();
        }
    }

    /// Parse the given input as an ad-hoc quick-connect target and build
    /// the ephemeral `Connection` for it. `None` when the input is not
    /// offered as a target: it must parse AND carry an explicit marker
    /// (`@`, a port, an IP literal) or be a bare hostname matching no
    /// saved host, so ordinary label searches never grow a spurious
    /// quick-connect row.
    pub(crate) fn quick_connect_target(&self, input: &str) -> Option<Connection> {
        let target = oryxis_core::ssh_target::SshTarget::parse(input)?;
        let needle = target.host.to_lowercase();
        let matches_saved = self.connections.iter().any(|c| {
            c.label.to_lowercase().contains(&needle)
                || c.hostname.to_lowercase().contains(&needle)
        });
        if !quick_connect_offerable(&target, matches_saved) {
            return None;
        }
        let username = target
            .username
            .clone()
            .or_else(oryxis_core::ssh_target::local_username);
        let resolved = oryxis_core::ssh_target::SshTarget {
            username: username.clone(),
            ..target
        };
        let mut conn = Connection::new(resolved.canonical(), &resolved.host);
        if let Some(port) = resolved.port {
            conn.port = port;
        }
        conn.username = username;
        Some(conn)
    }
}

/// Pure gate for offering quick connect (free of `self` so it unit-tests):
/// explicit targets (a username, a port, an IP-literal host) always offer;
/// a bare hostname offers only when it matches no saved host, so ordinary
/// label searches never grow a spurious quick-connect row.
pub(crate) fn quick_connect_offerable(
    target: &oryxis_core::ssh_target::SshTarget,
    matches_saved_host: bool,
) -> bool {
    target.username.is_some()
        || target.port.is_some()
        || target.host_is_ip_literal()
        || !matches_saved_host
}

#[cfg(test)]
mod tests {
    use super::quick_connect_offerable;
    use oryxis_core::ssh_target::SshTarget;

    fn parsed(s: &str) -> SshTarget {
        SshTarget::parse(s).expect("test input must parse")
    }

    #[test]
    fn explicit_targets_always_offer() {
        // A username, a port, or an IP literal is an unambiguous connect
        // intent, even when a saved host also matches the text.
        for s in ["root@web01", "web01:2222", "10.0.0.5", "::1"] {
            assert!(quick_connect_offerable(&parsed(s), true), "{s}");
            assert!(quick_connect_offerable(&parsed(s), false), "{s}");
        }
    }

    #[test]
    fn bare_hostname_defers_to_saved_matches() {
        // Typing a plain word is a search first: only offer the ad-hoc
        // row when nothing saved matches it.
        let t = parsed("staging");
        assert!(!quick_connect_offerable(&t, true));
        assert!(quick_connect_offerable(&t, false));
    }
}
