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

    /// Walk the `parent_id` chain from `gid` and report whether it
    /// terminates at a REAL root (a group whose `parent_id` is `None`).
    /// Returns `false` when the chain hits a dangling parent (an id no
    /// longer present in `groups`), revisits a group (a sync-merged
    /// parent cycle: device A sets G1.parent = G2 while device B sets
    /// G2.parent = G1, and LWW merges both into a loop), or exceeds a
    /// sane depth cap. Cycle-safe: the visited set is the loop guard;
    /// the depth cap is a belt-and-suspenders ceiling that sits far
    /// above any real folder nesting, so it never detaches valid data.
    ///
    /// The dashboard root pass uses this to degrade any group whose
    /// ancestry is broken (dangling OR cyclic) to rendering AT root, so
    /// the group (and the hosts inside it) stay reachable, editable and
    /// deletable instead of being trapped inside an unreachable loop.
    /// A group with no parent is trivially root-reachable (it IS a
    /// root); the root pass renders those at root through the plain
    /// `parent_id.is_none()` branch, so this only distinguishes
    /// well-nested subgroups (`true`) from broken ones (`false`).
    pub fn is_reachable_from_root(groups: &[Group], gid: Uuid) -> bool {
        const MAX_DEPTH: usize = 1024;
        let mut seen = std::collections::HashSet::new();
        let mut cursor = Some(gid);
        let mut depth = 0usize;
        while let Some(id) = cursor {
            if depth >= MAX_DEPTH {
                // Pathologically deep chain: treat as broken.
                return false;
            }
            depth += 1;
            if !seen.insert(id) {
                // Revisited a group: the chain loops, no real root.
                return false;
            }
            let Some(g) = groups.iter().find(|g| g.id == id) else {
                // Dangling parent id: the chain never reaches a root.
                return false;
            };
            match g.parent_id {
                None => return true,
                Some(p) => cursor = Some(p),
            }
        }
        false
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

    /// Materialise a breadcrumb `path` ("Prod / NewTeam") as a group
    /// chain, CREATING only the missing segments. This is the create
    /// half of [`Group::resolve_path_or_label`]: callers resolve an
    /// existing path/label first, then fall here to build a typed value
    /// that matched nothing.
    ///
    /// The path is split on the " / " separator; each segment is
    /// trimmed and empty segments are dropped. Walking from the root,
    /// every segment reuses the first existing MANUAL group
    /// (`cloud_query.is_none()`) with that exact label under the running
    /// parent, or creates a fresh group parented to it. New groups are
    /// appended to `groups` (so later segments and repeat calls see
    /// them) AND collected into `created` for the caller to persist to
    /// the vault. Returns the deepest segment's id, or `None` when the
    /// path has no non-empty segment.
    ///
    /// Because every created group's label is a single trimmed segment,
    /// no created label can ever contain " / ", so a group created here
    /// can never later impersonate a real breadcrumb path (the reason a
    /// free-text "Prod / NewTeam" must not become one root group named
    /// with the separator inside it). Reusing existing segments by
    /// (label, parent) also makes creating the same path twice
    /// idempotent, so no duplicate folders accrete. Cycle-safe: the walk
    /// only ever descends into groups it just created or matched by
    /// label under the running parent, never following a `parent_id`
    /// link, so cyclic sync-merged data can't loop it.
    pub fn create_path(
        groups: &mut Vec<Group>,
        path: &str,
        created: &mut Vec<Group>,
    ) -> Option<Uuid> {
        let mut parent: Option<Uuid> = None;
        let mut last: Option<Uuid> = None;
        for segment in path.split(" / ") {
            let label = segment.trim();
            if label.is_empty() {
                continue;
            }
            // Reuse an existing manual folder at this level (same label
            // under the running parent), else create one. Matching on
            // the running parent (not just the label) is what makes
            // "Prod / Web" create a NEW Web under Prod even when a
            // root-level Web already exists.
            let existing = groups
                .iter()
                .find(|g| {
                    g.cloud_query.is_none() && g.parent_id == parent && g.label == label
                })
                .map(|g| g.id);
            let id = match existing {
                Some(id) => id,
                None => {
                    let mut g = Group::new(label);
                    g.parent_id = parent;
                    let id = g.id;
                    created.push(g.clone());
                    groups.push(g);
                    id
                }
            };
            parent = Some(id);
            last = Some(id);
        }
        last
    }

    /// Find-or-create a group from a free-text Parent Group combo value.
    /// Tries [`Group::resolve_path_or_label`] first (an existing full
    /// path or bare label, no exclusions); on no match it materialises
    /// the value as a breadcrumb PATH via [`Group::create_path`], so
    /// "Prod / NewTeam" builds the nested chain instead of a single root
    /// group literally named with the separator inside it (which would
    /// then collide byte-for-byte with a genuine breadcrumb path). New
    /// groups are appended to `groups` and collected into `created` for
    /// the caller to persist. Returns `None` for a blank value (root).
    pub fn resolve_or_create_path(
        groups: &mut Vec<Group>,
        input: &str,
        created: &mut Vec<Group>,
    ) -> Option<Uuid> {
        if let Some(id) =
            Group::resolve_path_or_label(groups, input, &std::collections::HashSet::new())
        {
            return Some(id);
        }
        Group::create_path(groups, input, created)
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
    fn is_reachable_from_root_clean_chain() {
        let root = Group::new("root");
        let child = child_of("child", root.id);
        let grandchild = child_of("grandchild", child.id);
        let groups = vec![root.clone(), child.clone(), grandchild.clone()];
        assert!(Group::is_reachable_from_root(&groups, root.id));
        assert!(Group::is_reachable_from_root(&groups, child.id));
        assert!(Group::is_reachable_from_root(&groups, grandchild.id));
    }

    #[test]
    fn is_reachable_from_root_dangling_parent() {
        // The parent id points at a group that no longer exists (deleted
        // on another device before this one synced).
        let mut orphan = Group::new("Orphan");
        orphan.parent_id = Some(Uuid::new_v4());
        let groups = vec![orphan.clone()];
        assert!(!Group::is_reachable_from_root(&groups, orphan.id));
    }

    #[test]
    fn is_reachable_from_root_two_node_cycle() {
        // A -> B -> A, the classic concurrent-reparent merge.
        let mut a = Group::new("a");
        let mut b = Group::new("b");
        a.parent_id = Some(b.id);
        b.parent_id = Some(a.id);
        let groups = vec![a.clone(), b.clone()];
        assert!(!Group::is_reachable_from_root(&groups, a.id));
        assert!(!Group::is_reachable_from_root(&groups, b.id));
    }

    #[test]
    fn is_reachable_from_root_three_node_cycle() {
        // A -> B -> C -> A.
        let mut a = Group::new("a");
        let mut b = Group::new("b");
        let mut c = Group::new("c");
        a.parent_id = Some(b.id);
        b.parent_id = Some(c.id);
        c.parent_id = Some(a.id);
        let groups = vec![a.clone(), b.clone(), c.clone()];
        assert!(!Group::is_reachable_from_root(&groups, a.id));
        assert!(!Group::is_reachable_from_root(&groups, b.id));
        assert!(!Group::is_reachable_from_root(&groups, c.id));
    }

    #[test]
    fn is_reachable_from_root_self_parent() {
        // id == parent_id: a one-node loop, also unreachable today.
        let mut a = Group::new("a");
        a.parent_id = Some(a.id);
        let groups = vec![a.clone()];
        assert!(!Group::is_reachable_from_root(&groups, a.id));
    }

    #[test]
    fn is_reachable_from_root_node_hanging_off_a_cycle() {
        // C -> A -> B -> A: C itself is not on the loop, but its chain
        // never terminates at a real root, so it degrades to root too
        // (drives the descendant-promotion decision in the grid).
        let mut a = Group::new("a");
        let mut b = Group::new("b");
        a.parent_id = Some(b.id);
        b.parent_id = Some(a.id);
        let c = child_of("c", a.id);
        let groups = vec![a.clone(), b.clone(), c.clone()];
        assert!(!Group::is_reachable_from_root(&groups, c.id));
    }

    #[test]
    fn create_path_builds_the_nested_chain() {
        let mut groups: Vec<Group> = Vec::new();
        let mut created: Vec<Group> = Vec::new();
        let id = Group::create_path(&mut groups, "Prod / NewTeam", &mut created).unwrap();
        // Two groups created, wired parent -> child.
        assert_eq!(created.len(), 2);
        assert_eq!(groups.len(), 2);
        let leaf = groups.iter().find(|g| g.id == id).unwrap();
        assert_eq!(leaf.label, "NewTeam");
        let parent = groups.iter().find(|g| Some(g.id) == leaf.parent_id).unwrap();
        assert_eq!(parent.label, "Prod");
        assert_eq!(parent.parent_id, None);
        // The full breadcrumb path round-trips.
        assert_eq!(Group::path_of(&groups, id), "Prod / NewTeam");
    }

    #[test]
    fn create_path_never_puts_the_separator_in_a_label() {
        // The whole point of MINOR-2/5: a free-text value with the
        // separator must not become one group whose own label contains
        // " / " (which would impersonate a real path).
        let mut groups: Vec<Group> = Vec::new();
        let mut created: Vec<Group> = Vec::new();
        Group::create_path(&mut groups, "A / B / C", &mut created);
        assert_eq!(groups.len(), 3);
        assert!(groups.iter().all(|g| !g.label.contains(" / ")));
    }

    #[test]
    fn create_path_is_idempotent() {
        // Creating the same path twice reuses every segment, no dupes.
        let mut groups: Vec<Group> = Vec::new();
        let mut created = Vec::new();
        let first = Group::create_path(&mut groups, "Prod / NewTeam", &mut created).unwrap();
        created.clear();
        let second = Group::create_path(&mut groups, "Prod / NewTeam", &mut created).unwrap();
        assert_eq!(first, second);
        assert!(created.is_empty());
        assert_eq!(groups.len(), 2);
    }

    #[test]
    fn create_path_reuses_by_parent_not_just_label() {
        // A root "Web" exists; "Prod / Web" must create a NEW Web UNDER
        // Prod rather than adopting the unrelated root one.
        let root_web = Group::new("Web");
        let root_web_id = root_web.id;
        let mut groups = vec![root_web];
        let mut created = Vec::new();
        let leaf = Group::create_path(&mut groups, "Prod / Web", &mut created).unwrap();
        assert_ne!(leaf, root_web_id);
        let leaf_g = groups.iter().find(|g| g.id == leaf).unwrap();
        assert_eq!(leaf_g.label, "Web");
        let parent = groups.iter().find(|g| Some(g.id) == leaf_g.parent_id).unwrap();
        assert_eq!(parent.label, "Prod");
        // Prod + its Web created; the root Web is untouched.
        assert_eq!(created.len(), 2);
        assert_eq!(Group::path_of(&groups, root_web_id), "Web");
        assert_eq!(Group::path_of(&groups, leaf), "Prod / Web");
    }

    #[test]
    fn create_path_blank_and_empty_segments_yield_none() {
        let mut groups: Vec<Group> = Vec::new();
        let mut created = Vec::new();
        assert_eq!(Group::create_path(&mut groups, "   ", &mut created), None);
        assert_eq!(Group::create_path(&mut groups, " /  / ", &mut created), None);
        assert!(groups.is_empty());
        assert!(created.is_empty());
    }

    #[test]
    fn create_path_skips_empty_interior_segments() {
        // A stray double separator collapses instead of minting a blank
        // folder in the middle of the chain.
        let mut groups: Vec<Group> = Vec::new();
        let mut created = Vec::new();
        let id = Group::create_path(&mut groups, "Prod /  / Web", &mut created).unwrap();
        assert_eq!(groups.len(), 2);
        assert_eq!(Group::path_of(&groups, id), "Prod / Web");
    }

    #[test]
    fn resolve_or_create_path_prefers_existing() {
        let root = Group::new("Prod");
        let child = child_of("Frontend", root.id);
        let mut groups = vec![root.clone(), child.clone()];
        let mut created = Vec::new();
        // Existing full path resolves, nothing is created.
        assert_eq!(
            Group::resolve_or_create_path(&mut groups, "Prod / Frontend", &mut created),
            Some(child.id)
        );
        assert!(created.is_empty());
        assert_eq!(groups.len(), 2);
        // A brand-new path is materialised as a nested chain.
        let id =
            Group::resolve_or_create_path(&mut groups, "Prod / Backend", &mut created).unwrap();
        assert_eq!(Group::path_of(&groups, id), "Prod / Backend");
        // "Prod" was reused, only "Backend" created.
        assert_eq!(created.len(), 1);
        assert_eq!(created[0].label, "Backend");
    }

    #[test]
    fn resolve_or_create_path_blank_is_root() {
        let mut groups: Vec<Group> = Vec::new();
        let mut created = Vec::new();
        assert_eq!(Group::resolve_or_create_path(&mut groups, "  ", &mut created), None);
        assert!(groups.is_empty());
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
