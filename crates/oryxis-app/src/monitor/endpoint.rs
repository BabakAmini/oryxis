//! Which MACHINE a monitored connection reads (issue #156).
//!
//! Several vault rows routinely point at one server: `root@srv`,
//! `deploy@srv`, `app@srv`. Keying the sample window on the ROW gave
//! each of them its own probe on its own stagger slot, so the same
//! machine reported three different CPU figures at three different
//! instants, and cost three logins to say it. The window is keyed on
//! the machine instead: one probe, one ring, every card that reaches
//! that machine reading it.
//!
//! The key is deliberately conservative, for the same reason
//! [`crate::ssh_reuse::ReuseKey`] is: a miss costs a second probe, a
//! false match paints one machine's vitals on another machine's card.
//! It is also PURE (a function of the row alone, no vault access), so
//! it can run at read sites that render every frame.

use uuid::Uuid;

use oryxis_core::models::Connection;

use crate::app::Oryxis;

/// Identity of the machine a connection monitors.
///
/// The username is deliberately absent: it is exactly what these rows
/// differ in, and `/proc` reads the same for all of them. Everything
/// that decides WHERE the connection lands is in, via [`route_digest`].
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct MonitorKey {
    pub host: String,
    pub port: u16,
    pub route: u64,
}

impl MonitorKey {
    pub(crate) fn new(conn: &Connection) -> Self {
        Self {
            // Hostnames are case-insensitive and a stray space in the
            // field is a typo, not a second machine.
            host: conn.hostname.trim().to_lowercase(),
            port: conn.port,
            route: route_digest(conn),
        }
    }
}

/// Fold everything that decides where a connection lands into one
/// value, so two rows only share a window when they cannot be two
/// different machines.
///
/// The proxy PASSWORD is excluded (it changes how a dial authenticates,
/// never where it lands), and so are the algorithm overrides, which
/// [`crate::ssh_reuse::route_digest`]'s sibling includes: negotiating a
/// different cipher cannot move the server, and splitting on it would
/// give one machine two windows.
///
/// `group_id` is in as a STAND-IN for group inheritance (D4). The only
/// route-shaping field a group supplies is the proxy, and resolving it
/// properly means `resolve_proxy`, i.e. a vault read for the proxy
/// password, which cannot run at a read site that renders every frame.
/// Hashing the group instead under-shares (two rows on one machine in
/// two different groups keep separate windows, as they do today) and
/// can never over-share, which is the direction that stays correct: a
/// group whose proxy points into another network makes the same private
/// address a different machine.
fn route_digest(conn: &Connection) -> u64 {
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
    conn.proxy_identity_id.hash(&mut h);
    conn.group_id.hash(&mut h);
    format!("{:?}", conn.address_family).hash(&mut h);
    h.finish()
}

/// Fold rows into one entry per machine, keeping the rows' own order:
/// the first row of a machine decides where its entry sits, so the
/// dashboard's grid order (and the probe stagger derived from it) is
/// as stable as the card list it comes from.
pub(crate) fn group_by_machine(rows: Vec<(MonitorKey, Uuid)>) -> Vec<(MonitorKey, Vec<Uuid>)> {
    let mut groups: Vec<(MonitorKey, Vec<Uuid>)> = Vec::new();
    for (key, id) in rows {
        match groups.iter_mut().find(|(k, _)| *k == key) {
            Some((_, members)) => members.push(id),
            None => groups.push((key, vec![id])),
        }
    }
    groups
}

/// The disk selection of a machine, folded from its rows' own.
///
/// One window per machine can only carry one selection, and the UNION
/// is the only choice that never drops a mount a user explicitly asked
/// to watch on one of the rows. `None` is Auto, which already means
/// "every mount the probe's rules kept", so one Auto row absorbs the
/// rest. With a single row per machine (the norm) this is that row's
/// own selection, exactly as before issue #156.
pub(crate) fn union_disk_patterns<'a>(
    rows: impl Iterator<Item = Option<&'a Vec<String>>>,
) -> Option<Vec<String>> {
    let mut union: Vec<String> = Vec::new();
    for patterns in rows {
        let patterns = patterns?;
        for p in patterns {
            if !union.iter().any(|u| u == p) {
                union.push(p.clone());
            }
        }
    }
    Some(union)
}

impl Oryxis {
    /// The monitor key of a saved host, or `None` when the row is gone
    /// (deleted mid-probe, a stale id in a message still in flight).
    pub(crate) fn monitor_key(&self, conn_id: &Uuid) -> Option<MonitorKey> {
        self.connections
            .iter()
            .find(|c| c.id == *conn_id)
            .map(MonitorKey::new)
    }

    /// The window of the machine behind a host: what every surface
    /// showing that host reads, and the reason cards on one server
    /// can't disagree (issue #156).
    pub(crate) fn monitor_series(
        &self,
        conn_id: &Uuid,
    ) -> Option<&crate::monitor::ring::HostSeries> {
        self.monitor.series.get(&self.monitor_key(conn_id)?)
    }

    /// Its newest sample, the shape most read sites want.
    pub(crate) fn monitor_sample(
        &self,
        conn_id: &Uuid,
    ) -> Option<&crate::monitor::model::Sample> {
        self.monitor_series(conn_id)?.latest()
    }

    /// Every host that monitors this machine, in label order (the same
    /// order the dashboard lays its cards out, so the member elected to
    /// sample is stable across re-renders and reboots).
    ///
    /// Only opted-in rows count: a host nobody asked to monitor must
    /// not be the one dialed, and its disk selection is not part of the
    /// machine's.
    pub(crate) fn monitor_key_members(&self, key: &MonitorKey) -> Vec<Uuid> {
        let mut members: Vec<(String, Uuid)> = self
            .connections
            .iter()
            .filter(|c| self.monitor_conn_opted_in(c) && MonitorKey::new(c) == *key)
            .map(|c| (c.label.to_lowercase(), c.id))
            .collect();
        members.sort();
        members.into_iter().map(|(_, id)| id).collect()
    }

    /// The disk selection applied to a machine's window: the union of
    /// what its monitoring rows asked for (see [`union_disk_patterns`]).
    pub(crate) fn monitor_disk_patterns(&self, key: &MonitorKey) -> Option<Vec<String>> {
        let members = self.monitor_key_members(key);
        union_disk_patterns(members.iter().map(|id| {
            self.connections
                .iter()
                .find(|c| c.id == *id)
                .and_then(|c| c.monitor_disks.as_ref())
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oryxis_core::models::connection::{ProxyConfig, ProxyType};

    fn conn(host: &str, user: &str) -> Connection {
        let mut c = Connection::new("label", host);
        c.username = Some(user.to_string());
        c
    }

    /// The issue's own case: three rows, one server, three users. They
    /// read the same `/proc`, so they share one window.
    #[test]
    fn the_same_server_under_different_users_is_one_machine() {
        let a = MonitorKey::new(&conn("srv.example", "root"));
        let b = MonitorKey::new(&conn("srv.example", "deploy"));
        let c = MonitorKey::new(&conn("srv.example", "app"));
        assert_eq!(a, b);
        assert_eq!(b, c);
    }

    /// Case and stray whitespace in the host field are typing, not a
    /// second machine.
    #[test]
    fn the_hostname_is_normalised() {
        assert_eq!(
            MonitorKey::new(&conn("SRV.Example", "root")),
            MonitorKey::new(&conn(" srv.example ", "deploy"))
        );
    }

    /// Different endpoints stay apart, port included: two sshd's on one
    /// box can be two containers.
    #[test]
    fn a_different_endpoint_is_a_different_machine() {
        let base = MonitorKey::new(&conn("srv.example", "root"));
        assert_ne!(base, MonitorKey::new(&conn("other.example", "root")));
        let mut porty = conn("srv.example", "root");
        porty.port = 2222;
        assert_ne!(base, MonitorKey::new(&porty));
    }

    /// With bastion-relative addressing one private name behind two
    /// routes IS two machines, and a shared window would paint one of
    /// them with the other's vitals.
    #[test]
    fn a_different_route_is_a_different_machine() {
        let base = MonitorKey::new(&conn("10.0.0.5", "root"));

        let mut jumped = conn("10.0.0.5", "root");
        jumped.jump_chain = vec![Uuid::new_v4()];
        assert_ne!(base, MonitorKey::new(&jumped));

        let mut proxied = conn("10.0.0.5", "root");
        proxied.proxy = Some(ProxyConfig {
            proxy_type: ProxyType::Socks5,
            host: "bastion-b".into(),
            port: 1080,
            username: None,
            password: None,
        });
        assert_ne!(base, MonitorKey::new(&proxied));

        let mut by_identity = conn("10.0.0.5", "root");
        by_identity.proxy_identity_id = Some(Uuid::new_v4());
        assert_ne!(base, MonitorKey::new(&by_identity));

        // The group can supply the proxy (D4), so rows in different
        // groups never share a window. Keep this: "optimising" it away
        // is how one machine's numbers end up on another's card.
        let mut grouped = conn("10.0.0.5", "root");
        grouped.group_id = Some(Uuid::new_v4());
        assert_ne!(base, MonitorKey::new(&grouped));
    }

    /// The proxy password authenticates the hop; it does not move the
    /// server. Saving one must not split a machine's window in two.
    #[test]
    fn a_proxy_password_keeps_the_machine() {
        let with_pw = |pw: Option<&str>| {
            let mut c = conn("10.0.0.5", "root");
            c.proxy = Some(ProxyConfig {
                proxy_type: ProxyType::Socks5,
                host: "bastion-a".into(),
                port: 1080,
                username: Some("proxyuser".into()),
                password: pw.map(|s| s.to_string()),
            });
            c
        };
        assert_eq!(
            MonitorKey::new(&with_pw(None)),
            MonitorKey::new(&with_pw(Some("s3cret")))
        );
    }

    /// The fleet folds to ONE entry per machine, sitting where its
    /// first row sat: that entry is what gets a link and a probe, and
    /// its position is the stagger slot.
    #[test]
    fn rows_fold_into_one_entry_per_machine() {
        let (srv, other) = (
            MonitorKey::new(&conn("srv.example", "root")),
            MonitorKey::new(&conn("other.example", "root")),
        );
        let ids: Vec<Uuid> = (0..4).map(|_| Uuid::new_v4()).collect();
        let groups = group_by_machine(vec![
            (srv.clone(), ids[0]),
            (other.clone(), ids[1]),
            (srv.clone(), ids[2]),
            (srv.clone(), ids[3]),
        ]);
        assert_eq!(
            groups,
            vec![
                (srv, vec![ids[0], ids[2], ids[3]]),
                (other, vec![ids[1]]),
            ]
        );
    }

    /// The machine's disk selection is the union of its rows', so a
    /// mount one row asked to watch is never dropped because a sibling
    /// row didn't list it.
    #[test]
    fn disk_selections_union_across_the_rows() {
        let data = Some(vec!["/data".to_string()]);
        let var = Some(vec!["/var".to_string(), "/data".to_string()]);
        assert_eq!(
            union_disk_patterns([data.as_ref(), var.as_ref()].into_iter()),
            Some(vec!["/data".to_string(), "/var".to_string()])
        );
        // One Auto row means every mount: Auto is already the superset,
        // and narrowing it would hide mounts that row asked to see.
        assert_eq!(union_disk_patterns([data.as_ref(), None].into_iter()), None);
        // A machine whose only row reports nothing keeps that answer:
        // Custom-with-nothing is deliberate (issue #135), not Auto.
        assert_eq!(
            union_disk_patterns([Some(&Vec::new())].into_iter()),
            Some(Vec::new())
        );
    }

    /// Algorithm overrides change what the handshake negotiates, never
    /// which machine answers: splitting on them would give one server
    /// two windows and put the issue's bug straight back.
    #[test]
    fn algorithm_overrides_keep_the_machine() {
        let mut pinned = conn("srv.example", "root");
        pinned.kex = Some(vec!["curve25519-sha256".into()]);
        pinned.ciphers = Some(vec!["aes256-gcm@openssh.com".into()]);
        assert_eq!(
            MonitorKey::new(&conn("srv.example", "deploy")),
            MonitorKey::new(&pinned)
        );
    }
}
