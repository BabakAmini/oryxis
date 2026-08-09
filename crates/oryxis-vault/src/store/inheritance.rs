//! Group settings inheritance (D4): what a connection's settings
//! actually resolve to once its groups have had their say.
//!
//! A host field that is unset walks up the `parent_id` chain and takes
//! the first ancestor that sets it; nothing found means the app default
//! answers, exactly as before. Resolution is PER PARAMETER, so a group
//! that only sets the proxy leaves the theme to be answered elsewhere:
//! all-or-nothing inheritance would force a user to repeat settings
//! they only wanted to share one of.
//!
//! Nothing here is stored. The chain is walked live on every read, so
//! editing a group is immediately true for every host inside it and
//! there is no copied value to go stale (the `customized_fields`
//! machinery on cloud reimport is the opposite trade, and deliberately
//! so: that one preserves what a user typed against a REMOTE source of
//! truth).

use super::*;
use oryxis_core::models::connection::{Connection, EnvVar, ProxyConfig};

/// Where a resolved value came from, so the editor can say "inherited
/// from <group>" instead of pretending the host set it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// The host names it itself.
    Host,
    /// A group in the host's ancestry, nearest first.
    Group(Uuid),
}

/// A connection's settings after its groups have been applied. Each
/// field carries its `Origin` so the UI can grey what is inherited and
/// offer to clear an override back to it.
#[derive(Debug, Clone, Default)]
pub struct EffectiveConfig {
    pub username: Option<(String, Origin)>,
    pub identity_id: Option<(Uuid, Origin)>,
    /// Hydrated exactly like `resolve_proxy` does it, password
    /// included.
    pub proxy: Option<(ProxyConfig, Origin)>,
    /// Merged rather than chosen: root-first, nearer scopes overriding
    /// by NAME, host last. See `merge_env`.
    pub env_vars: Vec<EnvVar>,
    pub terminal_theme: Option<(String, Origin)>,
    pub startup_snippet_id: Option<(Uuid, Origin)>,
}

/// Depth ceiling for the ancestry walk. Far above any real nesting, so
/// it never truncates valid data; it exists only so a corrupted chain
/// cannot spin forever.
const MAX_DEPTH: usize = 64;

impl VaultStore {
    /// The ancestry of `group_id`, nearest first.
    ///
    /// Cycle-safe by the same construction `Group::subtree_ids` uses:
    /// the visited set IS the loop guard. A parent loop is not
    /// hypothetical, two devices can each re-parent one of a pair and
    /// LWW merges both edges, so the walk must terminate on data no
    /// user could have created by hand. It returns what it has rather
    /// than an error, because a corrupt hierarchy must never be the
    /// reason a host cannot connect.
    fn ancestry<'a>(&self, groups: &'a [Group], group_id: Uuid) -> Vec<&'a Group> {
        let mut chain = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let mut cursor = Some(group_id);
        while let Some(id) = cursor {
            if chain.len() >= MAX_DEPTH {
                tracing::warn!(
                    group = %id,
                    "group ancestry hit the depth cap; resolving with what was walked so far"
                );
                break;
            }
            if !seen.insert(id) {
                tracing::warn!(
                    group = %id,
                    "group ancestry loops; resolving with what was walked so far"
                );
                break;
            }
            let Some(group) = groups.iter().find(|g| g.id == id) else {
                // Dangling parent: mid-sync the ancestor's own record
                // may simply not have arrived yet. Stop, don't fail.
                break;
            };
            chain.push(group);
            cursor = group.parent_id;
        }
        chain
    }

    /// Resolve `conn` against its group chain.
    ///
    /// `groups` is passed in rather than listed here because this runs
    /// inside `view()`: the app already holds the list, and a query per
    /// frame per host would be a database read in the render path.
    ///
    /// Note what is NOT resolved: the port. A group's port applies when
    /// a host is CREATED inside it, never at connect time, so a host
    /// that works today cannot change destination because a group
    /// gained a default later.
    pub fn resolve_effective(
        &self,
        conn: &Connection,
        groups: &[Group],
    ) -> Result<EffectiveConfig, VaultError> {
        let chain: Vec<&Group> = match conn.group_id {
            Some(gid) => self.ancestry(groups, gid),
            None => Vec::new(),
        };

        let mut effective = EffectiveConfig {
            username: conn
                .username
                .clone()
                .filter(|u| !u.is_empty())
                .map(|u| (u, Origin::Host)),
            identity_id: conn.identity_id.map(|id| (id, Origin::Host)),
            terminal_theme: conn
                .terminal_theme
                .clone()
                .filter(|t| !t.is_empty())
                .map(|t| (t, Origin::Host)),
            startup_snippet_id: conn.startup_snippet_id.map(|id| (id, Origin::Host)),
            // The host's own proxy keeps every rule `resolve_proxy`
            // already documents (identity over inline, dangling id ->
            // None with a warning). Layering rather than replacing is
            // what keeps those callers correct.
            proxy: self.resolve_proxy(conn)?.map(|p| (p, Origin::Host)),
            env_vars: Vec::new(),
        };

        for group in &chain {
            let Some(defaults) = group.defaults.as_ref() else {
                continue;
            };
            let origin = Origin::Group(group.id);
            if effective.username.is_none()
                && let Some(u) = defaults.username.clone().filter(|u| !u.is_empty())
            {
                effective.username = Some((u, origin));
            }
            if effective.identity_id.is_none()
                && let Some(id) = defaults.identity_id
            {
                // Two gates, both about not eclipsing what the host can
                // already do. (1) Credentials are ONE parameter family:
                // the engine's credential resolution takes the identity
                // branch wholesale, so inheriting an identity onto a
                // host that stores its own password or names its own
                // key would silently disable those the day the group
                // gains a default. (2) Same forgiveness as the proxy
                // identity below (and the editor hint, which already
                // skips it): a deleted identity names nothing, and a
                // dangling reference would ALSO take the credential
                // branch and resolve to no credentials at all.
                let host_answers_credentials =
                    conn.key_id.is_some() || self.connection_has_password(&conn.id).unwrap_or(false);
                if host_answers_credentials {
                    // Nothing to inherit: the host's own credentials
                    // stand, exactly as if the group had no default.
                } else if self.identity_exists(&id)? {
                    effective.identity_id = Some((id, origin));
                } else {
                    tracing::warn!(
                        identity = %id,
                        group = %group.id,
                        "group default identity not found, leaving the host on its own credentials"
                    );
                }
            }
            if effective.terminal_theme.is_none()
                && let Some(t) = defaults.terminal_theme.clone().filter(|t| !t.is_empty())
            {
                effective.terminal_theme = Some((t, origin));
            }
            if effective.startup_snippet_id.is_none()
                && let Some(id) = defaults.startup_snippet_id
            {
                effective.startup_snippet_id = Some((id, origin));
            }
            if effective.proxy.is_none()
                && let Some(pid) = defaults.proxy_identity_id
            {
                // Same forgiveness as `resolve_proxy`: an identity the
                // user deleted leaves the host proxy-less with a
                // warning rather than breaking every host under the
                // group that referenced it.
                match self.get_proxy_identity(&pid)? {
                    Some(ident) => {
                        let password = self.get_proxy_identity_password(&pid).ok().flatten();
                        effective.proxy = Some((
                            ProxyConfig {
                                proxy_type: ident.proxy_type,
                                host: ident.host,
                                port: ident.port,
                                username: ident.username,
                                password,
                            },
                            origin,
                        ));
                    }
                    None => tracing::warn!(
                        proxy_identity = %pid,
                        group = %group.id,
                        "group default proxy identity not found, leaving the host without a proxy"
                    ),
                }
            }
        }

        effective.env_vars = merge_env(&chain, &conn.env_vars);
        Ok(effective)
    }

    /// Collapse [`resolve_effective`](Self::resolve_effective) onto the
    /// working copy an engine dials: the effective proxy lands on
    /// `conn.proxy` (the only proxy field engines read), an inherited
    /// username / identity fills the empty fields, and the merged env
    /// set replaces the host's own.
    ///
    /// `identities` also answers the username an identity carries: the
    /// SSH engine reads `connection.username` and falls back to "root",
    /// it never looks inside an identity, so a host that names no user
    /// and resolves to an identity takes the identity's username here.
    ///
    /// Resolution failing must not stop a connect that would otherwise
    /// work: the fallback is the host's own proxy, exactly the pre-D4
    /// behaviour. ONE implementation for every consumer (the app's
    /// dial sites and the MCP server), so the two can't drift.
    pub fn apply_effective(
        &self,
        conn: &mut Connection,
        groups: &[Group],
        identities: &[oryxis_core::models::Identity],
    ) {
        let effective = match self.resolve_effective(conn, groups) {
            Ok(effective) => effective,
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "group inheritance failed, using the host's own settings"
                );
                conn.proxy = self.resolve_proxy(conn).ok().flatten();
                return;
            }
        };
        conn.proxy = effective.proxy.map(|(p, _)| p);
        if conn.username.as_deref().unwrap_or_default().is_empty() {
            conn.username = effective.username.map(|(u, _)| u);
        }
        if conn.identity_id.is_none() {
            conn.identity_id = effective.identity_id.map(|(id, _)| id);
        }
        if conn.username.as_deref().unwrap_or_default().is_empty()
            && let Some(iid) = conn.identity_id
        {
            conn.username = identities
                .iter()
                .find(|i| i.id == iid)
                .and_then(|i| i.username.clone())
                .filter(|u| !u.is_empty());
        }
        // Already merged by name with the host winning, so this is the
        // whole set rather than an override.
        conn.env_vars = effective.env_vars;
    }
}

/// Environment variables are MERGED, not chosen.
///
/// `Vec<EnvVar>` has no "unset" to distinguish from "empty", so the
/// override rule other fields use cannot apply: a host with no
/// variables would otherwise mean "override the group with nothing",
/// and the group's variables would vanish the moment a host defined
/// one of its own. Merging by name keeps both, and a host that names
/// the same variable wins, which is the only reading in which a host
/// can still say no to an inherited value.
///
/// `chain` is nearest-first, so it is applied in REVERSE: the farthest
/// ancestor lays the base and nearer scopes overwrite it, host last.
fn merge_env(chain: &[&Group], host: &[EnvVar]) -> Vec<EnvVar> {
    let mut merged: Vec<EnvVar> = Vec::new();
    let mut put = |var: &EnvVar| {
        if let Some(existing) = merged.iter_mut().find(|e| e.key == var.key) {
            existing.value = var.value.clone();
        } else {
            merged.push(var.clone());
        }
    };
    for group in chain.iter().rev() {
        if let Some(defaults) = group.defaults.as_ref() {
            for var in &defaults.env_vars {
                put(var);
            }
        }
    }
    for var in host {
        put(var);
    }
    merged
}
