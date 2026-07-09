use crate::app::Oryxis;
use oryxis_vault::VaultStore;

impl Oryxis {
    /// One-shot migration of legacy inline `Connection.port_forwards` into
    /// standalone `PortForwardRule` rows (always `Local`, `auto_start =
    /// false`). The legacy field is left intact, it still raises forwards
    /// alongside the terminal session, the new rules just make the same
    /// tunnels runnable on their own. Gated by a settings flag so it runs
    /// exactly once.
    pub(super) fn migrate_port_forwards(&mut self, vault: &oryxis_vault::store::VaultStore) {
        if vault
            .get_setting("port_forwards_migrated")
            .ok()
            .flatten()
            .as_deref()
            == Some("true")
        {
            return;
        }
        let rules = legacy_forwards_to_rules(&self.connections);
        let mut created = 0usize;
        for rule in &rules {
            match vault.save_port_forward_rule(rule) {
                Ok(()) => created += 1,
                Err(e) => tracing::warn!("port-forward migration: save failed: {e}"),
            }
        }
        let _ = vault.set_setting("port_forwards_migrated", "true");
        if created > 0 {
            tracing::info!("migrated {created} legacy port forward(s) into standalone rules");
            self.port_forward_rules = vault.list_port_forward_rules().unwrap_or_default();
        }
    }

    /// Idempotent normalization of cloud-backed groups in the vault.
    ///
    /// Two legacy issues are fixed here:
    ///   1. **Wrong icon string**, early imports stored `"cloud"` /
    ///      `"si:aws"` on the provider folder and `"si:aws"` on the
    ///      ECS dynamic group. The brand-icon registry now expects
    ///      canonical ids (`"aws"`, `"ecs"`, `"kubernetes"`).
    ///   2. **Flat layout**, early imports left dynamic groups at
    ///      root with `parent_id = None`. We now nest them under the
    ///      provider folder.
    ///
    /// This walks `self.groups` once, mutates rows that need it, and
    /// rewrites them via `save_group` (no-op if nothing changed).
    pub(super) fn migrate_legacy_cloud_layout(&mut self, vault: &oryxis_vault::store::VaultStore) {
        // Snapshot so we can mutate the vault while iterating logic.
        let groups_snapshot = self.groups.clone();
        let profiles = self.cloud_profiles.clone();

        // Provider folders → canonical icon. A provider folder is any
        // group whose label matches a profile's label *and* has no
        // `cloud_query` itself (so we don't conflate it with a
        // dynamic group named the same).
        for g in &groups_snapshot {
            if g.cloud_query.is_some() {
                continue;
            }
            let Some(matching_profile) =
                profiles.iter().find(|p| p.label == g.label)
            else {
                continue;
            };
            let canonical = matching_profile.provider.as_str();
            let needs_update = g
                .icon
                .as_deref()
                .map(|cur| cur != canonical)
                .unwrap_or(true);
            if needs_update {
                let mut updated = g.clone();
                updated.icon = Some(canonical.to_string());
                let _ = vault.save_group(&updated);
            }
        }

        // Dynamic groups → canonical icon + parented under their
        // provider folder.
        for g in &groups_snapshot {
            let Some(query) = g.cloud_query.as_ref() else {
                continue;
            };
            let canonical_icon = match query.kind {
                oryxis_core::models::cloud::CloudQueryKind::EcsTasks { .. } => "ecs",
                oryxis_core::models::cloud::CloudQueryKind::K8sPods { .. } => "kubernetes",
            };

            // Find the provider folder this dynamic group should live
            // under (= the manual folder named after the profile).
            // Re-fetch from the freshly-mutated vault list so a folder
            // we just renamed in pass 1 above still resolves.
            let parent_id = profiles
                .iter()
                .find(|p| p.id == query.profile_id)
                .and_then(|p| {
                    groups_snapshot
                        .iter()
                        .find(|gg| gg.label == p.label && gg.cloud_query.is_none())
                        .map(|gg| gg.id)
                });

            let icon_wrong = g
                .icon
                .as_deref()
                .map(|cur| cur != canonical_icon)
                .unwrap_or(true);
            let parent_wrong = parent_id.is_some() && g.parent_id != parent_id;

            if icon_wrong || parent_wrong {
                let mut updated = g.clone();
                updated.icon = Some(canonical_icon.to_string());
                if let Some(pid) = parent_id {
                    updated.parent_id = Some(pid);
                }
                let _ = vault.save_group(&updated);
            }
        }

        // Re-pull groups so the in-memory state matches what we just
        // wrote (icons + parent ids).
        self.groups = vault.list_groups().unwrap_or_default();
    }
}

/// Re-home every group whose parent is not a valid manual folder onto its
/// nearest manual-folder ancestor (or root). A dynamic (cloud_query) group
/// is never a container, and a parent that no longer exists is dangling: a
/// child left pointing at either renders nowhere while still counting as
/// imported. The walk skips dynamic ancestors and resolves a missing id (or
/// running off the top) to root. Persists only the rows that actually move,
/// so it's a cheap no-op once the hierarchy is clean. A `visited` set guards
/// against a parent cycle in corrupt data.
pub(super) fn repair_group_parents(
    groups: &mut [oryxis_core::models::Group],
    vault: &VaultStore,
) {
    // id -> (parent_id, is_manual_folder)
    let index: std::collections::HashMap<uuid::Uuid, (Option<uuid::Uuid>, bool)> =
        groups
            .iter()
            .map(|g| (g.id, (g.parent_id, g.cloud_query.is_none())))
            .collect();
    let fixes: Vec<(uuid::Uuid, Option<uuid::Uuid>)> = groups
        .iter()
        .filter_map(|g| {
            let resolved = nearest_manual_parent(g.parent_id, &index);
            (resolved != g.parent_id).then_some((g.id, resolved))
        })
        .collect();
    for (gid, new_parent) in fixes {
        if let Some(g) = groups.iter_mut().find(|g| g.id == gid) {
            g.parent_id = new_parent;
            g.updated_at = chrono::Utc::now();
            let _ = vault.save_group(g);
        }
    }
}

/// Walk up from `parent` to the nearest manual-folder ancestor in `index`
/// (`id -> (parent_id, is_manual_folder)`). Dynamic ancestors are skipped;
/// a missing id, a `None` parent, or a cycle resolves to root (`None`). Pure
/// so the resolution rule is unit-testable without a vault.
fn nearest_manual_parent(
    parent: Option<uuid::Uuid>,
    index: &std::collections::HashMap<uuid::Uuid, (Option<uuid::Uuid>, bool)>,
) -> Option<uuid::Uuid> {
    let mut cur = parent;
    let mut visited: std::collections::HashSet<uuid::Uuid> =
        std::collections::HashSet::new();
    loop {
        let pid = cur?;
        if !visited.insert(pid) {
            return None; // cycle: bail to root
        }
        match index.get(&pid) {
            None => return None,                  // dangling parent
            Some((_, true)) => return Some(pid),  // manual folder: valid container
            Some((grandparent, false)) => cur = *grandparent, // dynamic: skip upward
        }
    }
}

/// Pure mapping from legacy inline `Connection.port_forwards` to standalone
/// `PortForwardRule`s. Every legacy forward is Local, binds `127.0.0.1` on
/// its old `local_port`, targets the old `remote_host:remote_port`, and is
/// created with `auto_start = false`. Kept separate from the vault I/O so
/// the mapping is unit-testable.
fn legacy_forwards_to_rules(
    conns: &[oryxis_core::models::connection::Connection],
) -> Vec<oryxis_core::models::port_forward_rule::PortForwardRule> {
    use oryxis_core::models::port_forward_rule::{ForwardKind, PortForwardRule};
    let mut rules = Vec::new();
    for conn in conns {
        for pf in &conn.port_forwards {
            let mut rule = PortForwardRule::new(
                format!("{} :{}", conn.label, pf.local_port),
                ForwardKind::Local,
                conn.id,
            );
            rule.listen_host = "127.0.0.1".into();
            rule.listen_port = pf.local_port;
            rule.target_host = pf.remote_host.clone();
            rule.target_port = pf.remote_port;
            rule.auto_start = false;
            rules.push(rule);
        }
    }
    rules
}

#[cfg(test)]
mod port_forward_migration_tests {
    use super::legacy_forwards_to_rules;
    use oryxis_core::models::connection::{Connection, PortForward};
    use oryxis_core::models::port_forward_rule::ForwardKind;

    #[test]
    fn maps_each_legacy_forward_to_a_local_rule() {
        let mut conn = Connection::new("db-box", "10.0.0.1");
        conn.port_forwards = vec![
            PortForward { local_port: 5432, remote_host: "127.0.0.1".into(), remote_port: 5432 },
            PortForward { local_port: 6379, remote_host: "cache.internal".into(), remote_port: 6379 },
        ];
        let other = Connection::new("no-forwards", "10.0.0.2");

        let rules = legacy_forwards_to_rules(&[conn.clone(), other]);

        // Two forwards on one connection, none on the other.
        assert_eq!(rules.len(), 2);
        for r in &rules {
            assert_eq!(r.kind, ForwardKind::Local);
            assert_eq!(r.host_id, conn.id);
            assert_eq!(r.listen_host, "127.0.0.1");
            assert!(!r.auto_start);
        }
        assert_eq!(rules[0].listen_port, 5432);
        assert_eq!(rules[0].target_host, "127.0.0.1");
        assert_eq!(rules[0].target_port, 5432);
        assert_eq!(rules[1].listen_port, 6379);
        assert_eq!(rules[1].target_host, "cache.internal");
        assert_eq!(rules[1].target_port, 6379);
    }

    #[test]
    fn no_forwards_yields_no_rules() {
        let conn = Connection::new("plain", "10.0.0.3");
        assert!(legacy_forwards_to_rules(&[conn]).is_empty());
    }
}

#[cfg(test)]
mod group_parent_repair_tests {
    use super::nearest_manual_parent;
    use std::collections::HashMap;
    use uuid::Uuid;

    // (parent_id, is_manual_folder)
    type Index = HashMap<Uuid, (Option<Uuid>, bool)>;

    #[test]
    fn manual_parent_is_kept() {
        let folder = Uuid::new_v4();
        let mut idx: Index = HashMap::new();
        idx.insert(folder, (None, true)); // manual folder at root
        assert_eq!(nearest_manual_parent(Some(folder), &idx), Some(folder));
    }

    #[test]
    fn root_parent_stays_root() {
        let idx: Index = HashMap::new();
        assert_eq!(nearest_manual_parent(None, &idx), None);
    }

    #[test]
    fn dangling_parent_resolves_to_root() {
        let missing = Uuid::new_v4();
        let idx: Index = HashMap::new(); // id not present
        assert_eq!(nearest_manual_parent(Some(missing), &idx), None);
    }

    #[test]
    fn dynamic_parent_is_skipped_up_to_manual_folder() {
        // The exact shape of the reported bug: a dynamic group renamed
        // under a folder whose label collides with a sibling dynamic
        // group, so it ends up parented on the dynamic one. It must be
        // re-homed on the manual folder above the dynamic parent.
        let folder = Uuid::new_v4(); // manual "ECS Example"
        let dyn_group = Uuid::new_v4(); // dynamic "ECS Example", child of folder
        let mut idx: Index = HashMap::new();
        idx.insert(folder, (None, true));
        idx.insert(dyn_group, (Some(folder), false));
        assert_eq!(nearest_manual_parent(Some(dyn_group), &idx), Some(folder));
    }

    #[test]
    fn all_dynamic_ancestors_resolve_to_root() {
        let dyn_a = Uuid::new_v4();
        let dyn_b = Uuid::new_v4();
        let mut idx: Index = HashMap::new();
        idx.insert(dyn_a, (None, false)); // dynamic at root
        idx.insert(dyn_b, (Some(dyn_a), false)); // dynamic under dynamic
        assert_eq!(nearest_manual_parent(Some(dyn_b), &idx), None);
    }

    #[test]
    fn parent_cycle_bails_to_root() {
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let mut idx: Index = HashMap::new();
        // Two dynamic groups pointing at each other: corrupt, must not loop.
        idx.insert(a, (Some(b), false));
        idx.insert(b, (Some(a), false));
        assert_eq!(nearest_manual_parent(Some(a), &idx), None);
    }
}
