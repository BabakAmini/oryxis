//! `impl Oryxis` block for SSH-connect plumbing, credential resolution,
//! jump-host resolver assembly, and the host-key verification callback.
//! Pulled out of `app.rs` to keep the main module from drifting past
//! ten thousand lines.

use std::sync::{Arc, Mutex};

use oryxis_core::models::connection::{AuthMethod, Connection};

use crate::app::Oryxis;

/// Whether this host's auth method ever offers a private key of its
/// own. `Agent` is absent: its key lives in the agent process, and a
/// local PEM would be a second credential the user did not pick.
pub(crate) fn conn_uses_key(conn: &Connection) -> bool {
    matches!(
        conn.auth_method,
        AuthMethod::Key | AuthMethod::Auto | AuthMethod::Certificate
    )
}

impl Oryxis {
    /// The OpenSSH certificate attached to key `kid`, if any (B2). Read
    /// from the in-memory key list; resolved alongside the private key so
    /// a certificate can never be paired with the wrong key.
    pub(crate) fn key_certificate(&self, kid: &uuid::Uuid) -> Option<String> {
        self.keys
            .iter()
            .find(|k| k.id == *kid)
            .and_then(|k| k.certificate.clone())
    }

    /// The public line of the vault key this connection references
    /// (its own `key_id`, or the linked identity's), for the agent-auth
    /// pin (B3): agent auth offers a matching agent identity first. Any
    /// key qualifies, not only security keys; a dangling reference is
    /// simply no pin (mirrors the dangling proxy-identity rule).
    pub(crate) fn pinned_agent_public(&self, conn: &Connection) -> Option<String> {
        let kid = conn.key_id.or_else(|| {
            conn.identity_id.and_then(|iid| {
                self.identities.iter().find(|i| i.id == iid).and_then(|i| i.key_id)
            })
        })?;
        self.keys
            .iter()
            .find(|k| k.id == kid)
            .map(|k| k.public_key.clone())
            .filter(|p| !p.trim().is_empty())
    }

    /// Resolve `(password, private_key_pem, certificate)` for a connection,
    /// same rules as `|v| Message::Ssh(SshMessage::ConnectSsh(v))`: prefer identity-linked
    /// credentials, fall back to per-connection vault entries. The
    /// certificate is resolved from the SAME key as the pem.
    ///
    /// A host with `use_disk_key` fills a still-empty key slot from
    /// `~/.ssh` (`oryxis_vault::resolve_disk_key`). It runs LAST on
    /// purpose: the vault is the app's own answer to "which key is this
    /// host's", and the disk is only ever the gap it leaves. Jump hops
    /// route through here too (`make_jump_resolver` calls this per hop),
    /// so a bastion reads its OWN `identity_file`, never the target's.
    pub(crate) fn resolve_credentials(
        &self,
        conn: &Connection,
    ) -> (Option<String>, Option<String>, Option<String>) {
        let (pw, pk, cert) = self.resolve_vault_credentials(conn);
        match pk {
            Some(pem) => (pw, Some(pem), cert),
            // Same gate as the vault key below: on a method that never
            // offers a key, reading one off disk would be work nobody
            // asked for (and a file access per connect).
            //
            // The certificate comes from the disk key too (its
            // `<key>-cert.pub` sibling), never from the vault: the pair
            // must always describe ONE key, which is the whole reason
            // `KeyMaterial` bundles them.
            None if conn_uses_key(conn) => {
                match oryxis_vault::resolve_disk_key(
                    conn.use_disk_key,
                    conn.identity_file.as_deref(),
                )
                .material()
                {
                    Some((pem, disk_cert)) => (pw, Some(pem), disk_cert),
                    None => (pw, None, cert),
                }
            }
            None => (pw, None, cert),
        }
    }

    /// The vault half of `resolve_credentials`, unchanged: identity-linked
    /// credentials first, per-connection vault entries second.
    fn resolve_vault_credentials(
        &self,
        conn: &Connection,
    ) -> (Option<String>, Option<String>, Option<String>) {
        if let Some(iid) = conn.identity_id {
            let id_pw = self
                .vault
                .as_ref()
                .and_then(|v| v.get_identity_password(&iid).ok().flatten());
            let kid = self
                .identities
                .iter()
                .find(|i| i.id == iid)
                .and_then(|i| i.key_id);
            let id_key = kid.and_then(|kid| {
                self.vault
                    .as_ref()
                    .and_then(|v| v.get_key_private(&kid).ok().flatten())
            });
            let id_cert = kid.and_then(|kid| self.key_certificate(&kid));
            (id_pw, id_key, id_cert)
        } else {
            let pw = self
                .vault
                .as_ref()
                .and_then(|v| v.get_connection_password(&conn.id).ok().flatten());
            let (pk, cert) = if conn_uses_key(conn) {
                let pk = conn.key_id.and_then(|kid| {
                    self.vault
                        .as_ref()
                        .and_then(|v| v.get_key_private(&kid).ok().flatten())
                });
                let cert = conn.key_id.and_then(|kid| self.key_certificate(&kid));
                (pk, cert)
            } else {
                (None, None)
            };
            (pw, pk, cert)
        }
    }

    /// Expand nested hop routes onto the connect working copy (issue
    /// #184): a hop that itself sits behind a jump chain is reached
    /// through its own route first, the way OpenSSH follows a hop's
    /// `ProxyJump` recursively. The engine keeps dialing a flat list;
    /// this rewrite is what makes the list the full route, and it runs
    /// on the working copy only, never on a row that is saved back.
    pub(crate) fn expand_jump_chain(&self, conn: &mut Connection) {
        if conn.jump_chain.is_empty() {
            return;
        }
        conn.jump_chain = expanded_jump_chain(conn.id, &conn.jump_chain, &self.connections);
    }

    /// Build a `ConnectionResolver` covering the jump-host chain of the
    /// given connection. `None` when there's no chain.
    ///
    /// Takes the working copy mutably because it first expands nested
    /// hop routes onto `jump_chain` (see
    /// [`expand_jump_chain`](Self::expand_jump_chain)): the expansion
    /// lives here so no dial site can forget it, and the connect
    /// progress hop count reads the expanded route for free.
    ///
    /// The engine authenticates each hop from the resolver's OWN rows
    /// (`connect_via_jump_hosts` reads username, port, algorithms off
    /// them), so every hop gets the same working copy a direct connect
    /// would dial: group inheritance (D4) applied, the effective proxy
    /// collapsed onto `proxy`, an inherited identity's username filling
    /// an empty field. Hop credentials mirror `resolve_credentials`
    /// (identity-linked first, per-connection fields second) for the
    /// same reason: a bastion must not authenticate differently
    /// depending on whether it is dialed directly or as a hop.
    pub(crate) fn make_jump_resolver(
        &self,
        conn: &mut Connection,
    ) -> Option<oryxis_ssh::ConnectionResolver> {
        self.expand_jump_chain(conn);
        if conn.jump_chain.is_empty() {
            return None;
        }
        // Only the jump-chain hosts are ever looked up by the engine, so
        // the resolver carries just those RESOLVED rows rather than a
        // clone of the whole vault (wasted work on large vaults).
        let mut connections = Vec::with_capacity(conn.jump_chain.len());
        let mut passwords = std::collections::HashMap::new();
        let mut keys = std::collections::HashMap::new();
        let mut certificates = std::collections::HashMap::new();
        let mut proxies = std::collections::HashMap::new();
        for jid in &conn.jump_chain {
            // A dangling hop id stays a resolver miss, reported by the
            // engine as "jump host not found" like before.
            let Some(hop) = self.connections.iter().find(|c| c.id == *jid) else {
                continue;
            };
            let mut hop = hop.clone();
            self.apply_group_inheritance(&mut hop);
            let (pw, pk, cert) = self.resolve_credentials(&hop);
            if let Some(pw) = pw {
                passwords.insert(*jid, pw);
            }
            if let Some(pk) = pk {
                keys.insert(*jid, pk);
            }
            if let Some(cert) = cert {
                certificates.insert(*jid, cert);
            }
            // Only matters for the first jump (later hops travel inside
            // the tunnel) but we hydrate every jump's entry, cheap and
            // keeps the resolver self-contained.
            if let Some(p) = hop.proxy.clone() {
                proxies.insert(*jid, p);
            }
            connections.push(hop);
        }
        Some(oryxis_ssh::ConnectionResolver {
            connections,
            passwords,
            private_keys: keys,
            certificates,
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

    /// Canonical `ssh://` URL for a saved host ("Copy SSH URL" card
    /// action). Mirrors connect-time username resolution (the linked
    /// identity's username fills an empty field); the default port 22 is
    /// omitted and IPv6 hosts take brackets, both via `SshTarget`.
    pub(crate) fn host_ssh_url(&self, conn: &Connection) -> String {
        use oryxis_core::models::connection::ConnectionProtocol;
        // Serial has no network URL; hand back the bare port path so the
        // copy action still yields something meaningful (the caller only
        // offers this on SSH/Telnet hosts, but stay honest if reached).
        // Local reaches no endpoint at all, so its label is the only
        // truthful answer.
        if conn.protocol == ConnectionProtocol::Serial {
            return conn.hostname.clone();
        }
        if conn.protocol == ConnectionProtocol::Local {
            return conn.label.clone();
        }
        let username = conn.username.clone().or_else(|| {
            conn.identity_id.and_then(|iid| {
                self.identities
                    .iter()
                    .find(|i| i.id == iid)
                    .and_then(|i| i.username.clone())
            })
        });
        // Telnet's default port is 23, SSH's is 22: omit the port only
        // when it matches the scheme's default. RemoteDesktop hosts carry
        // an rdp/vnc endpoint; use the kind's scheme (the copy action is
        // only offered on SSH/Telnet, but stay honest if reached).
        let (scheme, default_port) = match conn.protocol {
            // `telnets` when TLS is on, so pasting the URL back into
            // quick connect restores the tunnel and not a cleartext
            // session on the same port.
            ConnectionProtocol::Telnet => {
                match conn.telnet.map(|t| t.tls).unwrap_or(false) {
                    true => ("telnets", 992),
                    false => ("telnet", 23),
                }
            }
            // Raw has no conventional port, so `default_port` is one it
            // can never equal: the port always shows, which is the only
            // way the URL round-trips (`raw://host` alone is not a
            // target).
            ConnectionProtocol::Raw => ("raw", 0),
            ConnectionProtocol::RemoteDesktop => match conn.rd_kind {
                oryxis_core::models::remote_desktop::RemoteDesktopKind::Rdp => ("rdp", 3389),
                oryxis_core::models::remote_desktop::RemoteDesktopKind::Vnc => ("vnc", 5900),
            },
            ConnectionProtocol::Ssh
            | ConnectionProtocol::Serial
            | ConnectionProtocol::Local => ("ssh", 22),
        };
        let target = oryxis_core::ssh_target::SshTarget {
            username,
            // A shareable URL never carries the stored password, and
            // `canonical` would not render one anyway.
            password: None,
            host: conn.hostname.clone(),
            port: (conn.port != default_port).then_some(conn.port),
        };
        format!("{scheme}://{}", target.canonical())
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
        use oryxis_core::models::connection::ConnectionProtocol as Proto;
        self.quick_connect_target_as(input, Proto::Ssh)
    }

    /// The dashboard's quick connect, where the card's protocol badges
    /// are on screen and can answer for a line that named no scheme.
    ///
    /// Separate from [`quick_connect_target`](Self::quick_connect_target)
    /// on purpose: the badge belongs to that one card. Reading the
    /// field everywhere would let a Telnet pick made on the dashboard
    /// silently apply in the new-tab picker and the tab-jump palette,
    /// where there is no badge to see it or to undo it.
    pub(crate) fn dashboard_quick_connect_target(&self, input: &str) -> Option<Connection> {
        self.quick_connect_target_as(input, self.quick_connect_protocol)
    }

    /// Shared body: `fallback` is the protocol for a line that named no
    /// `scheme://`. A typed scheme always wins over it.
    fn quick_connect_target_as(
        &self,
        input: &str,
        fallback: oryxis_core::models::connection::ConnectionProtocol,
    ) -> Option<Connection> {
        use oryxis_core::models::connection::ConnectionProtocol as Proto;
        use oryxis_core::quick_target::{QuickEndpoint, QuickTarget};
        let parsed = QuickTarget::parse(input)?;
        let protocol = parsed.protocol.unwrap_or(fallback);
        let target = match parsed.endpoint {
            // A serial device is its own kind of target: no user, no
            // port, and the line parameters (baud) ride the connection.
            QuickEndpoint::Serial { device, baud } => {
                let mut conn = Connection::new(device.clone(), &device);
                conn.protocol = Proto::Serial;
                let mut params = oryxis_core::models::serial::SerialParams::default();
                if let Some(baud) = baud {
                    params.baud = baud;
                }
                conn.serial = Some(params);
                return Some(conn);
            }
            QuickEndpoint::Network(target) => target,
        };
        // Raw needs a port and has no default worth inventing (console
        // servers number their lines per vendor), so a Raw target
        // without one is not offered rather than dialled somewhere
        // arbitrary.
        if protocol == Proto::Raw && target.port.is_none() {
            return None;
        }
        let needle = target.host.to_lowercase();
        let matches_saved = self.connections.iter().any(|c| {
            c.label.to_lowercase().contains(&needle)
                || c.hostname.to_lowercase().contains(&needle)
        });
        // A typed scheme IS the explicit marker: `telnet://web01` says
        // connect, whatever else the vault happens to hold under that
        // name.
        if parsed.protocol.is_none() && !quick_connect_offerable(&target, matches_saved) {
            return None;
        }
        // Raw and Serial authenticate nobody, so they never borrow the
        // local OS user the SSH/Telnet default fills in.
        let username = match protocol.uses_credentials() {
            true => target.username.clone().or_else(oryxis_core::ssh_target::local_username),
            false => None,
        };
        let resolved = oryxis_core::ssh_target::SshTarget {
            username: username.clone(),
            ..target
        };
        let mut conn = Connection::new(resolved.canonical(), &resolved.host);
        conn.protocol = protocol;
        conn.port = resolved
            .port
            .or_else(|| protocol.default_port())
            .unwrap_or(conn.port);
        conn.username = username;
        if parsed.tls {
            conn.telnet = Some(oryxis_core::models::telnet::TelnetOptions {
                tls: true,
                // An ad-hoc dial never skips verification: the escape is
                // a per-host decision made in the editor, on a host the
                // user chose to keep.
                tls_insecure: false,
            });
        }
        Some(conn)
    }

    /// Which protocols the quick-connect card offers as badges for the
    /// current input, and which one is selected.
    ///
    /// Only shown while the typed text names no scheme: with
    /// `telnet://` in front of it the question is already answered, and
    /// a picker that could contradict the text would be a second
    /// source of truth. Raw appears only once the text carries a port,
    /// because that is the one thing it cannot do without.
    pub(crate) fn quick_connect_badges(
        &self,
        input: &str,
    ) -> Option<(Vec<oryxis_core::models::connection::ConnectionProtocol>, oryxis_core::models::connection::ConnectionProtocol)>
    {
        use oryxis_core::models::connection::ConnectionProtocol as Proto;
        use oryxis_core::quick_target::{QuickEndpoint, QuickTarget};
        let parsed = QuickTarget::parse(input)?;
        if parsed.protocol.is_some() {
            return None;
        }
        let QuickEndpoint::Network(target) = &parsed.endpoint else {
            return None;
        };
        let mut options = vec![Proto::Ssh, Proto::Telnet];
        if target.port.is_some() {
            options.push(Proto::Raw);
        }
        let selected = if options.contains(&self.quick_connect_protocol) {
            self.quick_connect_protocol
        } else {
            Proto::Ssh
        };
        Some((options, selected))
    }
}

/// Flatten `chain` into the full dial route, following each hop's own
/// `jump_chain` recursively (issue #184): each hop contributes its own
/// route immediately before itself, so a bastion that sits behind a
/// bastion is reached the way OpenSSH reaches a `ProxyJump` hop whose
/// config names another jump. Pure over the connection list so it
/// unit-tests without an app.
///
/// `target_id` seeds the visited set, and a hop already routed earlier
/// in the list is never routed twice: a cycle (or a hop naming the
/// target itself) degrades to dialing the hop directly instead of
/// recursing forever. Dangling ids stay in the route verbatim, so the
/// engine still reports "jump host not found" for them like before.
pub(crate) fn expanded_jump_chain(
    target_id: uuid::Uuid,
    chain: &[uuid::Uuid],
    connections: &[Connection],
) -> Vec<uuid::Uuid> {
    fn push_route(
        id: uuid::Uuid,
        connections: &[Connection],
        visited: &mut std::collections::HashSet<uuid::Uuid>,
        out: &mut Vec<uuid::Uuid>,
    ) {
        if !visited.insert(id) {
            return;
        }
        if let Some(hop) = connections.iter().find(|c| c.id == id) {
            for pre in &hop.jump_chain {
                push_route(*pre, connections, visited, out);
            }
        }
        out.push(id);
    }
    let mut visited = std::collections::HashSet::new();
    visited.insert(target_id);
    let mut out = Vec::new();
    for id in chain {
        push_route(*id, connections, &mut visited, &mut out);
    }
    out
}

/// Pure gate for offering quick connect (free of `self` so it unit-tests):
/// explicit targets (a username, a port, an IP-literal host) always offer;
/// a bare hostname offers only when it matches no saved host, so ordinary
/// label searches never grow a spurious quick-connect row.
pub(crate) fn quick_connect_offerable(
    target: &oryxis_core::ssh_target::SshTarget,
    matches_saved_host: bool,
) -> bool {
    target.is_explicit() || !matches_saved_host
}

#[cfg(test)]
mod tests {
    use super::{expanded_jump_chain, quick_connect_offerable};
    use oryxis_core::models::connection::Connection;
    use oryxis_core::ssh_target::SshTarget;
    use uuid::Uuid;

    fn host(label: &str, chain: Vec<Uuid>) -> Connection {
        let mut c = Connection::new(label.to_string(), label);
        c.jump_chain = chain;
        c
    }

    #[test]
    fn nested_hop_route_expands_before_the_hop() {
        // C jumps via B, and B itself sits behind A (issue #184): the
        // dial route must reach B through A, the way OpenSSH follows
        // B's own ProxyJump.
        let a = host("a", vec![]);
        let b = host("b", vec![a.id]);
        let target = Uuid::new_v4();
        let route = expanded_jump_chain(target, &[b.id], &[a.clone(), b.clone()]);
        assert_eq!(route, vec![a.id, b.id]);

        // One level deeper: A behind Z.
        let z = host("z", vec![]);
        let a2 = host("a2", vec![z.id]);
        let b2 = host("b2", vec![a2.id]);
        let route = expanded_jump_chain(target, &[b2.id], &[z.clone(), a2.clone(), b2.clone()]);
        assert_eq!(route, vec![z.id, a2.id, b2.id]);
    }

    #[test]
    fn hop_already_routed_is_not_dialed_twice() {
        // The final host lists the full route by hand (today's
        // workaround) AND B names A itself: A must appear once.
        let a = host("a", vec![]);
        let b = host("b", vec![a.id]);
        let target = Uuid::new_v4();
        let route = expanded_jump_chain(target, &[a.id, b.id], &[a.clone(), b.clone()]);
        assert_eq!(route, vec![a.id, b.id]);
    }

    #[test]
    fn cycles_degrade_to_a_direct_dial_of_the_hop() {
        // B's own chain names the target: expanding must not recurse
        // into it, the hop is dialed directly.
        let target = Uuid::new_v4();
        let b = host("b", vec![target]);
        let route = expanded_jump_chain(target, &[b.id], std::slice::from_ref(&b));
        assert_eq!(route, vec![b.id]);

        // A and B name each other: the walk terminates and every hop
        // still appears exactly once.
        let mut a = host("a", vec![]);
        let mut b = host("b", vec![]);
        a.jump_chain = vec![b.id];
        b.jump_chain = vec![a.id];
        let route = expanded_jump_chain(target, &[b.id], &[a.clone(), b.clone()]);
        assert_eq!(route, vec![a.id, b.id]);

        // The chain naming the target itself contributes nothing.
        let route = expanded_jump_chain(target, &[target], &[]);
        assert!(route.is_empty());
    }

    #[test]
    fn dangling_hop_ids_survive_verbatim() {
        // A hop id with no row (deleted host, unsynced peer) stays in
        // the route so the engine reports it, exactly like before.
        let ghost = Uuid::new_v4();
        let b = host("b", vec![ghost]);
        let target = Uuid::new_v4();
        let route = expanded_jump_chain(target, &[b.id], std::slice::from_ref(&b));
        assert_eq!(route, vec![ghost, b.id]);
    }

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
