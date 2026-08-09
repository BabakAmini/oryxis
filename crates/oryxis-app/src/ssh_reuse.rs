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
}

impl ReuseKey {
    pub(crate) fn new(origin: Uuid, conn: &oryxis_core::models::Connection) -> Self {
        Self {
            origin,
            host: conn.hostname.clone(),
            port: conn.port,
            username: conn.username.clone().unwrap_or_default(),
        }
    }
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
    pub(crate) fn prune_transport_pool(&mut self) {
        self.ssh_transport_pool
            .retain(|_, weak| weak.strong_count() > 0);
    }

    /// The reuse key for a pane, from what it is ACTUALLY connected to.
    ///
    /// The connection is RESOLVED first, exactly as the connect path
    /// resolves it. That is not a detail: a host inheriting its user
    /// from a group has an empty `username` in the vault row and
    /// authenticates as the inherited one, so keying the registration
    /// off the raw row and the lookup off the resolved copy produced
    /// two different keys and reuse never matched. Both sides go
    /// through the same resolution now, which is the only way they
    /// cannot drift.
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
}
