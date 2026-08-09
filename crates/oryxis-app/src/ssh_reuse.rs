//! ControlMaster-style connection reuse (F2): a second tab to a host
//! that is already open rides the connection the first one holds.
//!
//! The pool stores `Weak` references only. It must never be the reason
//! a connection stays alive: the sessions own the transport, and when
//! the last one closes the link goes with it and the pool entry becomes
//! a dead weight that the next lookup prunes. A pool of `Arc`s would
//! keep every host ever opened connected until the app exits.
//!
//! Every failure here is silent and falls through to a fresh dial. A
//! reused connection is an optimisation; a user who cannot open a tab
//! because the optimisation broke would rightly call that a bug.

use std::collections::HashMap;
use std::sync::{Arc, Weak};

use uuid::Uuid;

use crate::app::Oryxis;

/// What makes two tabs eligible to share one connection.
///
/// The vault row alone is not enough: the host editor can change the
/// endpoint or the user between two opens, and the second tab must not
/// land on the first one's connection to a different machine. The
/// endpoint alone is not enough either: two `Connection`s to the same
/// box can differ in jump chain, proxy, algorithms or keepalive, none
/// of which are visible in host:port, so they get separate links.
///
/// The result is deliberately conservative. A miss costs one handshake;
/// a false match costs the user a tab on the wrong machine.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct ReuseKey {
    /// The vault row (or the quick-connect entry) this tab came from.
    /// Quick-connect ids are `Uuid`s from a different store, so they
    /// can never collide with a saved host's.
    pub origin: Uuid,
    pub host: String,
    pub port: u16,
    /// Resolved at connect time, so a host inheriting its user from a
    /// group keys on what it actually authenticates as.
    pub username: String,
    /// Everything else that decides WHERE the link lands and what it
    /// negotiates: jump chain, effective proxy, algorithm overrides,
    /// address family. host:port alone cannot see any of it, and with
    /// bastion-relative addressing (one private name reachable through
    /// two different bastions) an edited route IS a different machine,
    /// with no host-key prompt possible on a reused channel. See
    /// [`route_digest`].
    pub route: u64,
}

impl ReuseKey {
    pub(crate) fn new(origin: Uuid, conn: &oryxis_core::models::Connection) -> Self {
        Self {
            origin,
            host: conn.hostname.clone(),
            port: conn.port,
            username: conn.username.clone().unwrap_or_default(),
            route: route_digest(conn),
        }
    }
}

/// Fold the route-shaping fields of a RESOLVED connection into one
/// value. Hashed rather than stored so the key stays cheap to clone
/// and compare; in-memory only, so the hasher needs no cross-run
/// stability. The proxy PASSWORD is deliberately excluded: it changes
/// how a dial authenticates, never where it lands, and including it
/// would make a saved-password edit look like a different route.
fn route_digest(conn: &oryxis_core::models::Connection) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    conn.jump_chain.hash(&mut h);
    match &conn.proxy {
        Some(p) => {
            1u8.hash(&mut h);
            // `ProxyType` carries data (`Command(cmd)`), so Debug is
            // the cheapest faithful encoding of the whole variant.
            format!("{:?}", p.proxy_type).hash(&mut h);
            p.host.hash(&mut h);
            p.port.hash(&mut h);
            p.username.hash(&mut h);
        }
        None => 0u8.hash(&mut h),
    }
    conn.ciphers.hash(&mut h);
    conn.kex.hash(&mut h);
    conn.macs.hash(&mut h);
    conn.host_key_algorithms.hash(&mut h);
    format!("{:?}", conn.address_family).hash(&mut h);
    h.finish()
}

impl Oryxis {
    /// A live connection for this key, or `None` to dial fresh.
    ///
    /// Prunes as it goes: an entry whose transport is gone, or whose
    /// link has stopped answering, is removed rather than left to be
    /// retried on every open.
    pub(crate) fn pooled_transport(
        &mut self,
        key: &ReuseKey,
    ) -> Option<Arc<oryxis_ssh::SshTransport>> {
        if !self.prefs.ssh_connection_reuse {
            return None;
        }
        let entry = self.ssh_transport_pool.get(key)?;
        let Some(transport) = entry.upgrade() else {
            self.ssh_transport_pool.remove(key);
            return None;
        };
        // `looks_healthy` is a cheap "probably fine" read of the probe
        // window, not a round trip: reuse must not add latency to the
        // very thing it exists to speed up. A connection that lies here
        // fails at channel-open instead, which also falls back.
        if !transport.looks_healthy() {
            self.ssh_transport_pool.remove(key);
            return None;
        }
        Some(transport)
    }

    /// Remember a freshly dialled connection so the next tab to the
    /// same host can ride it.
    pub(crate) fn remember_transport(&mut self, key: ReuseKey, session: &oryxis_ssh::SshSession) {
        if !self.prefs.ssh_connection_reuse {
            return;
        }
        self.ssh_transport_pool
            .insert(key, Arc::downgrade(session.transport()));
    }

    /// Drop entries whose transport is gone. Called on disconnect, so
    /// the map does not grow one dead `Weak` per host per app run.
    /// Sweeps the pending dial-time keys the same way: an entry whose
    /// pane no longer exists belongs to a dial that will never report
    /// `SshConnected`.
    pub(crate) fn prune_transport_pool(&mut self) {
        self.ssh_transport_pool
            .retain(|_, weak| weak.strong_count() > 0);
        let live: std::collections::HashSet<Uuid> = self
            .tabs
            .iter()
            .flat_map(|t| t.pane_grid.panes.values().map(|p| p.id))
            .collect();
        self.pending_reuse_keys.retain(|id, _| live.contains(id));
    }

    /// The reuse key for a pane recomputed from the CURRENT row: the
    /// removal fallback when a failed reuse finds no pending dial-time
    /// key. Registration never uses this (an edit while a dial is in
    /// flight would re-key the old transport under the new row); it
    /// consumes the key minted at dial time from `pending_reuse_keys`.
    ///
    /// The connection is RESOLVED first, exactly as the connect path
    /// resolves it, so the recompute matches what the dial keyed on
    /// whenever the row has not changed in between.
    ///
    /// Local, serial and ephemeral panes have nothing to key on.
    pub(crate) fn reuse_key_for_pane(&self, pane_id: Uuid) -> Option<ReuseKey> {
        let pane = self
            .tabs
            .iter()
            .find_map(|tab| tab.pane_grid.panes.values().find(|p| p.id == pane_id))?;
        let origin = match pane.origin {
            crate::state::PaneOrigin::Host(id) => id,
            crate::state::PaneOrigin::QuickHost(id) => id,
            _ => return None,
        };
        let mut conn = self.pane_origin_connection(pane_id)?.clone();
        self.apply_group_inheritance(&mut conn);
        Some(ReuseKey::new(origin, &conn))
    }

    /// Whether the focused pane's connection carries more than one
    /// session, i.e. closing this tab will NOT close the link and a
    /// drop WILL take the others with it. The UI says so, because a
    /// shared connection is a user-visible fact: when it dies, every
    /// tab on it dies at the same moment, and without a word from the
    /// app that reads as several tabs breaking at once for no reason.
    pub(crate) fn pane_shares_connection(&self, pane: &crate::state::Pane) -> bool {
        pane.session
            .as_ref()
            .and_then(|t| t.ssh())
            .is_some_and(|s| s.transport_owners() > 1)
    }
}

/// The pool's storage type, kept here next to the semantics it obeys.
pub(crate) type TransportPool = HashMap<ReuseKey, Weak<oryxis_ssh::SshTransport>>;

#[cfg(test)]
mod tests {
    use super::*;
    use oryxis_core::models::Connection;

    fn conn(host: &str, port: u16, user: Option<&str>) -> Connection {
        let mut c = Connection::new("label", host);
        c.port = port;
        c.username = user.map(|u| u.to_string());
        c
    }

    #[test]
    fn the_same_row_and_endpoint_is_the_same_key() {
        let id = Uuid::new_v4();
        assert_eq!(
            ReuseKey::new(id, &conn("example.com", 22, Some("deploy"))),
            ReuseKey::new(id, &conn("example.com", 22, Some("deploy")))
        );
    }

    /// The editor can change a host's endpoint or user between two
    /// opens. Keying on the vault row alone would put the second tab on
    /// the first one's connection to somewhere else entirely.
    #[test]
    fn an_edited_endpoint_or_user_is_a_different_key() {
        let id = Uuid::new_v4();
        let base = ReuseKey::new(id, &conn("example.com", 22, Some("deploy")));
        assert_ne!(base, ReuseKey::new(id, &conn("example.com", 2222, Some("deploy"))));
        assert_ne!(base, ReuseKey::new(id, &conn("other.example", 22, Some("deploy"))));
        assert_ne!(base, ReuseKey::new(id, &conn("example.com", 22, Some("root"))));
        assert_ne!(base, ReuseKey::new(id, &conn("example.com", 22, None)));
    }

    /// Two vault rows can point at one machine and still differ in jump
    /// chain, proxy or algorithms, none of which show up in host:port.
    /// They get separate connections.
    #[test]
    fn two_rows_to_one_box_do_not_share() {
        let a = ReuseKey::new(Uuid::new_v4(), &conn("example.com", 22, Some("deploy")));
        let b = ReuseKey::new(Uuid::new_v4(), &conn("example.com", 22, Some("deploy")));
        assert_ne!(a, b);
    }

    /// Editing the ROUTE of one row between two opens is a different
    /// key too: with bastion-relative addressing the same private name
    /// through another jump chain or proxy is a different machine, and
    /// a reused channel can never raise a host-key prompt to say so.
    #[test]
    fn an_edited_route_is_a_different_key() {
        let id = Uuid::new_v4();
        let base = ReuseKey::new(id, &conn("10.0.0.5", 22, Some("deploy")));

        let mut jumped = conn("10.0.0.5", 22, Some("deploy"));
        jumped.jump_chain = vec![Uuid::new_v4()];
        assert_ne!(base, ReuseKey::new(id, &jumped));

        let mut proxied = conn("10.0.0.5", 22, Some("deploy"));
        proxied.proxy = Some(oryxis_core::models::connection::ProxyConfig {
            proxy_type: oryxis_core::models::connection::ProxyType::Socks5,
            host: "bastion-b".into(),
            port: 1080,
            username: None,
            password: None,
        });
        assert_ne!(base, ReuseKey::new(id, &proxied));

        let mut pinned = conn("10.0.0.5", 22, Some("deploy"));
        pinned.kex = Some(vec!["diffie-hellman-group14-sha1".into()]);
        assert_ne!(base, ReuseKey::new(id, &pinned));
    }

    /// The proxy PASSWORD is not part of the route: it changes how a
    /// dial authenticates, never where it lands, so saving a new proxy
    /// password must not orphan the live connection's pool entry.
    #[test]
    fn a_proxy_password_change_keeps_the_key() {
        let id = Uuid::new_v4();
        let with_pw = |pw: Option<&str>| {
            let mut c = conn("10.0.0.5", 22, Some("deploy"));
            c.proxy = Some(oryxis_core::models::connection::ProxyConfig {
                proxy_type: oryxis_core::models::connection::ProxyType::Socks5,
                host: "bastion-a".into(),
                port: 1080,
                username: Some("proxyuser".into()),
                password: pw.map(|s| s.to_string()),
            });
            c
        };
        assert_eq!(
            ReuseKey::new(id, &with_pw(None)),
            ReuseKey::new(id, &with_pw(Some("s3cret")))
        );
    }
}
