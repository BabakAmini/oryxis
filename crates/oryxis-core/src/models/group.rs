use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::cloud::CloudQuery;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Group {
    pub id: Uuid,
    pub label: String,
    pub parent_id: Option<Uuid>,
    pub color: Option<String>,
    pub icon: Option<String>,
    pub sort_order: i32,
    pub is_shared: bool,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    /// When set, the group's children are not stored in the vault but
    /// resolved on each expand by a `CloudProvider::resolve_query`
    /// call. Manual groups have `None`. Dynamic groups must not also
    /// have manually-attached children, the contents are 100% derived
    /// from the query.
    #[serde(default)]
    pub cloud_query: Option<CloudQuery>,
}

impl Group {
    /// Collect `root` plus every descendant reachable through
    /// `parent_id` links. Used by re-parenting UIs to exclude a
    /// group's own subtree from the valid-parent candidates (a group
    /// nested under its own descendant would orphan the whole
    /// subtree). Cycle-safe: sync merges could theoretically land a
    /// parent loop in the data, so membership in the result set
    /// doubles as the visited guard.
    pub fn subtree_ids(groups: &[Group], root: Uuid) -> std::collections::HashSet<Uuid> {
        let mut set = std::collections::HashSet::from([root]);
        let mut frontier = vec![root];
        while let Some(gid) = frontier.pop() {
            for child in groups.iter().filter(|g| g.parent_id == Some(gid)) {
                if set.insert(child.id) {
                    frontier.push(child.id);
                }
            }
        }
        set
    }

    /// Breadcrumb path of a group from its root ancestor, labels
    /// joined with " / " ("Prod / Frontend / API"). This is what the
    /// Parent Group combos display so same-named folders under
    /// different parents stay distinguishable. Cycle-safe: a repeated
    /// ancestor stops the walk (sync merges could land a parent loop),
    /// and a dangling parent id just ends the chain (renders as root).
    pub fn path_of(groups: &[Group], gid: Uuid) -> String {
        let mut labels: Vec<&str> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let mut cursor = Some(gid);
        while let Some(id) = cursor {
            if !seen.insert(id) {
                break;
            }
            let Some(g) = groups.iter().find(|g| g.id == id) else {
                break;
            };
            labels.push(&g.label);
            cursor = g.parent_id;
        }
        labels.reverse();
        labels.join(" / ")
    }

    /// Resolve a Parent Group combo value to a group id. The combos
    /// display full paths, so an exact path match wins; a bare label
    /// (typed by hand) falls back to the first label match, preserving
    /// the historical find-by-label semantics. Only manual folders
    /// qualify (`cloud_query.is_none()`), and `excluded` removes a
    /// subtree from the candidates (the re-parent cycle guard). An
    /// empty / unmatched input returns `None` (root).
    pub fn resolve_path_or_label(
        groups: &[Group],
        input: &str,
        excluded: &std::collections::HashSet<Uuid>,
    ) -> Option<Uuid> {
        let input = input.trim();
        if input.is_empty() {
            return None;
        }
        let candidates = || {
            groups
                .iter()
                .filter(|g| g.cloud_query.is_none() && !excluded.contains(&g.id))
        };
        candidates()
            .find(|g| Group::path_of(groups, g.id) == input)
            .or_else(|| candidates().find(|g| g.label == input))
            .map(|g| g.id)
    }

    pub fn new(label: impl Into<String>) -> Self {
        let now = chrono::Utc::now();
        Self {
            id: Uuid::new_v4(),
            label: label.into(),
            parent_id: None,
            color: None,
            icon: None,
            sort_order: 0,
            is_shared: false,
            created_at: now,
            updated_at: now,
            cloud_query: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn child_of(label: &str, parent: Uuid) -> Group {
        let mut g = Group::new(label);
        g.parent_id = Some(parent);
        g
    }

    #[test]
    fn subtree_ids_collects_root_and_descendants() {
        let root = Group::new("root");
        let child = child_of("child", root.id);
        let grandchild = child_of("grandchild", child.id);
        let stranger = Group::new("stranger");
        let groups = vec![root.clone(), child.clone(), grandchild.clone(), stranger.clone()];

        let set = Group::subtree_ids(&groups, root.id);
        assert_eq!(set.len(), 3);
        assert!(set.contains(&root.id));
        assert!(set.contains(&child.id));
        assert!(set.contains(&grandchild.id));
        assert!(!set.contains(&stranger.id));
    }

    #[test]
    fn subtree_ids_leaf_is_just_itself() {
        let root = Group::new("root");
        let child = child_of("child", root.id);
        let groups = vec![root, child.clone()];
        assert_eq!(Group::subtree_ids(&groups, child.id).len(), 1);
    }

    #[test]
    fn path_of_walks_the_parent_chain() {
        let root = Group::new("Prod");
        let child = child_of("Frontend", root.id);
        let grandchild = child_of("API", child.id);
        let groups = vec![root.clone(), child.clone(), grandchild.clone()];
        assert_eq!(Group::path_of(&groups, root.id), "Prod");
        assert_eq!(Group::path_of(&groups, child.id), "Prod / Frontend");
        assert_eq!(Group::path_of(&groups, grandchild.id), "Prod / Frontend / API");
    }

    #[test]
    fn path_of_dangling_parent_renders_from_the_break() {
        let mut orphan = Group::new("Orphan");
        orphan.parent_id = Some(Uuid::new_v4());
        let groups = vec![orphan.clone()];
        assert_eq!(Group::path_of(&groups, orphan.id), "Orphan");
    }

    #[test]
    fn path_of_survives_parent_cycles() {
        let mut a = Group::new("a");
        let mut b = Group::new("b");
        a.parent_id = Some(b.id);
        b.parent_id = Some(a.id);
        let groups = vec![a.clone(), b.clone()];
        // The exact prefix depends on where the cycle breaks; it must
        // terminate and end at the queried group.
        assert!(Group::path_of(&groups, a.id).ends_with("a"));
    }

    #[test]
    fn resolve_prefers_path_then_label() {
        let root = Group::new("Prod");
        let staging = Group::new("Staging");
        let fe_prod = child_of("Frontend", root.id);
        let fe_staging = child_of("Frontend", staging.id);
        let groups = vec![root.clone(), staging.clone(), fe_prod.clone(), fe_staging.clone()];
        let none = std::collections::HashSet::new();

        // Full paths disambiguate the two same-named subgroups.
        assert_eq!(
            Group::resolve_path_or_label(&groups, "Staging / Frontend", &none),
            Some(fe_staging.id)
        );
        // A bare label keeps first-match semantics.
        assert_eq!(
            Group::resolve_path_or_label(&groups, "Frontend", &none),
            Some(fe_prod.id)
        );
        // Empty and unknown inputs resolve to root.
        assert_eq!(Group::resolve_path_or_label(&groups, "  ", &none), None);
        assert_eq!(Group::resolve_path_or_label(&groups, "Nope", &none), None);
    }

    #[test]
    fn resolve_skips_excluded_and_dynamic_groups() {
        let root = Group::new("Prod");
        let child = child_of("Frontend", root.id);
        let mut dynamic = Group::new("Dyn");
        dynamic.cloud_query = Some(CloudQuery {
            profile_id: Uuid::new_v4(),
            kind: super::super::cloud::CloudQueryKind::EcsTasks {
                cluster: String::new(),
                service: String::new(),
                container: String::new(),
            },
            template: super::super::cloud::ConnectionTemplate::new(
                super::super::cloud::TransportKind::EcsExec,
            ),
        });
        let groups = vec![root.clone(), child.clone(), dynamic.clone()];

        let excluded = Group::subtree_ids(&groups, root.id);
        assert_eq!(Group::resolve_path_or_label(&groups, "Prod", &excluded), None);
        assert_eq!(
            Group::resolve_path_or_label(&groups, "Dyn", &std::collections::HashSet::new()),
            None
        );
    }

    #[test]
    fn subtree_ids_survives_parent_cycles() {
        // A sync merge could land A -> B -> A; the walk must terminate.
        let mut a = Group::new("a");
        let mut b = Group::new("b");
        a.parent_id = Some(b.id);
        b.parent_id = Some(a.id);
        let groups = vec![a.clone(), b.clone()];
        let set = Group::subtree_ids(&groups, a.id);
        assert!(set.contains(&a.id));
        assert!(set.contains(&b.id));
        assert_eq!(set.len(), 2);
    }
}
