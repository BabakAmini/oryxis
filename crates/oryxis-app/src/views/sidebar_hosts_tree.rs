//! Hosts sidebar tab (issue #102): an mRemoteNG-style tree of the
//! vault's groups and hosts. Folders expand/collapse in place (nested
//! to any depth, the #102 sub-group work), a click on a host opens a
//! session in a new tab, and the search shows every match with its
//! ancestor chain force-expanded. Session-independent by design: the
//! tab needs no live transport, so a region holding only this tab is
//! always available.

use iced::border::Radius;
use iced::widget::{column, container, text, Space};
use iced::{Background, Border, Color, Element, Length, Padding};
use uuid::Uuid;

use oryxis_core::models::Group;

use crate::app::{AiMessage, Message, Oryxis, SshMessage};
use crate::i18n::t;
use crate::theme::OryxisColors;
use crate::widgets::dir_row;

const STAB: crate::state::TerminalSidebarTab = crate::state::TerminalSidebarTab::HostsTree;

/// Indent per tree level, applied on the leading edge (via `dir_row`,
/// so it mirrors under RTL).
const INDENT: f32 = 14.0;

impl Oryxis {
    pub(crate) fn hosts_tree_tab_content(&self) -> Element<'_, Message> {
        // Focus target for the sidebar hotkey / Ctrl+F (entering the
        // tree lands the keyboard here), and an input row in the Tab
        // walk.
        let search = self.sidebar_nav_slot(
            crate::keynav::SidebarRow::input(iced::widget::Id::new("sidebar-hosts-search")),
            STAB,
            crate::widgets::INPUT_RADIUS,
            iced::widget::text_input(t("search"), &self.hosts_tree_search)
                .id(iced::widget::Id::new("sidebar-hosts-search"))
                .on_input(|v| Message::Ai(AiMessage::HostsTreeSearchChanged(v)))
                .padding(8)
                .size(13)
                .style(crate::widgets::rounded_input_style)
                .into(),
        );
        let header = container(
            dir_row(vec![search]).align_y(iced::Alignment::Center),
        )
        .padding(Padding { top: 10.0, right: 12.0, bottom: 8.0, left: 12.0 })
        .width(Length::Fill);

        if self.connections.is_empty() && self.groups.is_empty() {
            return column![header, placeholder(t("hosts_tree_empty"))]
                .width(Length::Fill)
                .height(Length::Fill)
                .into();
        }

        let needle = self.hosts_tree_search.trim().to_lowercase();
        let mut rows: Vec<Element<'_, Message>> = Vec::new();
        // Dynamic (cloud-query) groups whose provider plugin isn't
        // installed are invisible, dashboard parity.
        let hidden_profiles = self.hidden_cloud_profile_ids();
        // Which groups earn a row this frame, decided BEFORE any row
        // is built: the keynav recording happens at construction time,
        // so rows must be built strictly in display order (a subtree
        // materialised early, or built-then-discarded for a collapsed
        // folder, records phantom indices the keyboard then acts on -
        // that shipped as Enter connecting a host that wasn't even on
        // screen).
        let visible = self.tree_visibility(&needle, &hidden_profiles);
        // Sync LWW merges can leave dangling parents and cycles; a
        // group whose chain doesn't reach a root degrades to root
        // (same policy as the dashboard), and the visited set keeps a
        // cycle from recursing forever.
        let mut visited: std::collections::HashSet<Uuid> = std::collections::HashSet::new();
        let mut roots: Vec<&Group> = self
            .groups
            .iter()
            .filter(|g| {
                g.parent_id.is_none() || !Group::is_reachable_from_root(&self.groups, g.id)
            })
            .collect();
        sort_groups(&mut roots);
        for group in roots {
            self.tree_group_rows(&mut rows, group, 0, &needle, &visible, &mut visited);
        }
        // Root hosts: no group, or a group id that no longer resolves.
        let group_exists =
            |gid: Uuid| self.groups.iter().any(|g| g.id == gid);
        let mut root_hosts: Vec<(usize, &oryxis_core::models::Connection)> = self
            .connections
            .iter()
            .enumerate()
            .filter(|(_, c)| c.group_id.filter(|gid| group_exists(*gid)).is_none())
            .collect();
        sort_hosts(&mut root_hosts);
        for (idx, conn) in root_hosts {
            if host_matches(conn, &needle) {
                rows.push(self.tree_host_row(idx, conn, 0));
            }
        }

        if rows.is_empty() {
            rows.push(placeholder(t("no_matches")));
        }

        let list = column(rows)
            .spacing(2)
            .padding(Padding { top: 0.0, right: 12.0, bottom: 12.0, left: 12.0 });
        let body = iced::widget::scrollable(list)
            .id(crate::keynav::sidebar_scroll_id(STAB))
            .width(Length::Fill)
            .height(Length::Fill);
        column![header, body]
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    /// Append one group's row (and, when expanded, its subtree) to
    /// `rows`, strictly in DISPLAY order: the keynav layer records a
    /// row the moment it is built, so nothing is ever built ahead of
    /// its on-screen position and nothing built is ever discarded.
    /// Whether a branch shows at all was decided up front by
    /// `tree_visibility`.
    fn tree_group_rows<'a>(
        &'a self,
        rows: &mut Vec<Element<'a, Message>>,
        group: &'a Group,
        depth: usize,
        needle: &str,
        visible: &std::collections::HashMap<Uuid, bool>,
        visited: &mut std::collections::HashSet<Uuid>,
    ) {
        if !visited.insert(group.id) {
            return;
        }
        if !visible.get(&group.id).copied().unwrap_or(false) {
            return;
        }
        let searching = !needle.is_empty();
        let expanded = searching || self.hosts_tree_expanded.contains(&group.id);
        rows.push(self.tree_group_row(group, depth, expanded));
        if !expanded {
            return;
        }

        let mut children: Vec<&Group> = self
            .groups
            .iter()
            .filter(|g| g.parent_id == Some(group.id))
            .collect();
        sort_groups(&mut children);
        for child in children {
            self.tree_group_rows(rows, child, depth + 1, needle, visible, visited);
        }

        let label_match = !searching || group.label.to_lowercase().contains(needle);
        let mut hosts: Vec<(usize, &oryxis_core::models::Connection)> = self
            .connections
            .iter()
            .enumerate()
            .filter(|(_, c)| c.group_id == Some(group.id))
            .collect();
        sort_hosts(&mut hosts);
        for (idx, conn) in hosts {
            // A matching group shows its whole host list; otherwise
            // only the hosts that match themselves.
            if label_match || host_matches(conn, needle) {
                rows.push(self.tree_host_row(idx, conn, depth + 1));
            }
        }

        if group.cloud_query.is_some() {
            // The resolved ECS tasks / K8s pods (or the pending /
            // loading / failed state) render as this folder's
            // children. The resolve itself fires from the expand
            // click (`HostsTreeToggleGroup`), never from view().
            let (mut dyn_rows, _) =
                self.tree_dynamic_rows(group, depth + 1, needle, label_match);
            rows.append(&mut dyn_rows);
        }
    }

    /// Which groups earn a row for this frame's needle, computed
    /// WITHOUT building any widget (see `tree_group_rows` for why
    /// construction must follow display order). Memoised recursion in
    /// the `group_has_visible_content` style; the pre-seeded `false`
    /// doubles as the cycle guard.
    ///
    /// The rules, per group:
    /// - dynamic (cloud-query): hidden provider = never; otherwise
    ///   shown, under a search only when its label or a resolved
    ///   task/pod matches.
    /// - manual: needs a saved host or a visible dynamic group
    ///   somewhere below (an empty folder has nothing to connect to,
    ///   owner ask); under a search additionally its label, one of
    ///   its hosts, or a descendant must match.
    fn tree_visibility(
        &self,
        needle: &str,
        hidden_profiles: &std::collections::HashSet<Uuid>,
    ) -> std::collections::HashMap<Uuid, bool> {
        fn visible(
            app: &Oryxis,
            gid: Uuid,
            needle: &str,
            hidden_profiles: &std::collections::HashSet<Uuid>,
            memo: &mut std::collections::HashMap<Uuid, bool>,
        ) -> bool {
            if let Some(&v) = memo.get(&gid) {
                return v;
            }
            memo.insert(gid, false);
            let Some(group) = app.groups.iter().find(|g| g.id == gid) else {
                return false;
            };
            let searching = !needle.is_empty();
            let v = if let Some(q) = group.cloud_query.as_ref() {
                if hidden_profiles.contains(&q.profile_id) {
                    false
                } else if !searching {
                    true
                } else {
                    group.label.to_lowercase().contains(needle)
                        || app.tree_dynamic_host_matches(gid, needle)
                }
            } else {
                let has_content = app.tree_subtree_host_count(gid) > 0
                    || app.tree_subtree_has_dynamic(gid, hidden_profiles);
                if !has_content {
                    false
                } else if !searching {
                    true
                } else {
                    group.label.to_lowercase().contains(needle)
                        || app
                            .connections
                            .iter()
                            .any(|c| c.group_id == Some(gid) && host_matches(c, needle))
                        || app
                            .groups
                            .iter()
                            .filter(|g| g.parent_id == Some(gid))
                            .any(|g| visible(app, g.id, needle, hidden_profiles, memo))
                }
            };
            memo.insert(gid, v);
            v
        }
        let mut memo = std::collections::HashMap::new();
        for g in &self.groups {
            visible(self, g.id, needle, hidden_profiles, &mut memo);
        }
        memo
    }

    /// Whether any RESOLVED task/pod of a dynamic group matches the
    /// needle (unresolved groups can't answer, so they only match by
    /// label).
    fn tree_dynamic_host_matches(&self, gid: Uuid, needle: &str) -> bool {
        match self.cloud_dynamic_group_state.get(&gid) {
            Some(crate::state::DynamicGroupState::Loaded { hosts, .. }) => {
                hosts.iter().any(|h| {
                    h.resource_id.to_lowercase().contains(needle)
                        || h.container_name
                            .as_deref()
                            .is_some_and(|n| n.to_lowercase().contains(needle))
                })
            }
            _ => false,
        }
    }

    /// One folder row: chevron + folder glyph (the group's custom icon
    /// and colour when set) + label + subtree host count. Click (or
    /// Enter on the ring) toggles the expansion.
    fn tree_group_row<'a>(
        &'a self,
        group: &'a Group,
        depth: usize,
        expanded: bool,
    ) -> Element<'a, Message> {
        let c = OryxisColors::t();
        let chevron = if expanded {
            iced_fonts::lucide::chevron_down()
        } else if crate::i18n::is_rtl_layout() {
            iced_fonts::lucide::chevron_left()
        } else {
            iced_fonts::lucide::chevron_right()
        };
        let tint = group
            .color
            .as_deref()
            .and_then(crate::os_icon::parse_hex_color)
            .unwrap_or(c.text_muted);
        // Icon precedence mirrors the dynamic-group cards: an explicit
        // user icon wins, then the query-derived brand (ecs /
        // kubernetes), then the plain folder glyphs.
        let brand: Option<&str> = group.cloud_query.as_ref().map(|q| match q.kind {
            oryxis_core::models::cloud::CloudQueryKind::EcsTasks { .. } => "ecs",
            oryxis_core::models::cloud::CloudQueryKind::K8sPods { .. } => "kubernetes",
        });
        let icon_id = group.icon.as_deref().filter(|s| !s.is_empty()).or(brand);
        let folder: Element<'a, Message> = match icon_id {
            Some(icon_id) => crate::os_icon::custom_icon_glyph(icon_id).view(14.0, tint),
            None if expanded => iced_fonts::lucide::folder_open().size(14).color(tint).into(),
            None => iced_fonts::lucide::folder().size(14).color(tint).into(),
        };
        // Dynamic groups count what the resolve brought back (nothing
        // before the first expand); manual folders count their
        // subtree's saved hosts.
        let subtree_hosts = if group.cloud_query.is_some() {
            match self.cloud_dynamic_group_state.get(&group.id) {
                Some(crate::state::DynamicGroupState::Loaded { hosts, .. }) => hosts.len(),
                _ => 0,
            }
        } else {
            self.tree_subtree_host_count(group.id)
        };
        let mut items: Vec<Element<'a, Message>> = vec![
            Space::new().width(depth as f32 * INDENT).into(),
            chevron.size(12).color(c.text_muted).into(),
            Space::new().width(4).into(),
            folder,
            Space::new().width(6).into(),
            text(group.label.as_str())
                .size(12)
                .color(c.text_primary)
                .width(Length::Fill)
                .wrapping(iced::widget::text::Wrapping::None)
                .into(),
        ];
        if subtree_hosts > 0 {
            items.push(Space::new().width(6).into());
            items.push(text(subtree_hosts.to_string()).size(11).color(c.text_muted).into());
        }
        let msg = Message::Ai(AiMessage::HostsTreeToggleGroup(group.id));
        self.sidebar_nav_slot(
            crate::keynav::SidebarRow::list_button(msg.clone()),
            STAB,
            6.0,
            tree_row_button(items, msg),
        )
    }

    /// One host row: live-session dot (a tab is connected to this
    /// host), protocol glyph, label, and the address when the global
    /// "show host address" preference is on. Click (or Enter on the
    /// ring) opens a session, the same message as the dashboard card.
    fn tree_host_row<'a>(
        &'a self,
        idx: usize,
        conn: &'a oryxis_core::models::Connection,
        depth: usize,
    ) -> Element<'a, Message> {
        let c = OryxisColors::t();
        let live = self.tabs.iter().any(|t| {
            t.pane_grid
                .panes
                .values()
                .any(|p| p.saved_conn_id() == Some(conn.id) && p.session.is_some())
        });
        let mut items: Vec<Element<'a, Message>> = vec![
            Space::new().width(depth as f32 * INDENT + 16.0).into(),
            iced_fonts::lucide::server()
                .size(13)
                .color(if live { c.success } else { c.text_muted })
                .into(),
            Space::new().width(6).into(),
            text(conn.label.as_str())
                .size(12)
                .color(c.text_primary)
                .wrapping(iced::widget::text::Wrapping::None)
                .into(),
        ];
        if live {
            // The tint alone can read as a themed icon; the dot makes
            // "connected" unambiguous at a glance.
            items.push(Space::new().width(5).into());
            items.push(
                container(Space::new().width(6).height(6))
                    .style(|_| container::Style {
                        background: Some(Background::Color(OryxisColors::t().success)),
                        border: Border { radius: Radius::from(3.0), ..Default::default() },
                        ..Default::default()
                    })
                    .into(),
            );
        }
        items.push(Space::new().width(Length::Fill).into());
        if self.prefs.show_host_address {
            items.push(
                text(conn.hostname.as_str())
                    .size(11)
                    .color(c.text_muted)
                    .wrapping(iced::widget::text::Wrapping::None)
                    .into(),
            );
        }
        let msg = Message::Ssh(SshMessage::ConnectSsh(idx));
        self.sidebar_nav_slot(
            crate::keynav::SidebarRow::list_button(msg.clone()),
            STAB,
            6.0,
            tree_row_button(items, msg),
        )
    }

    /// Hosts anywhere in a group's subtree (cycle-safe via
    /// `Group::subtree_ids`), the folder row's count badge.
    fn tree_subtree_host_count(&self, gid: Uuid) -> usize {
        let ids = Group::subtree_ids(&self.groups, gid);
        self.connections
            .iter()
            .filter(|c| c.group_id.is_some_and(|g| ids.contains(&g)))
            .count()
    }

    /// Whether a group's subtree holds any VISIBLE dynamic
    /// (cloud-query) group, which keeps an otherwise host-less branch
    /// on screen: the dynamic contents only exist after a resolve.
    fn tree_subtree_has_dynamic(
        &self,
        gid: Uuid,
        hidden_profiles: &std::collections::HashSet<Uuid>,
    ) -> bool {
        let ids = Group::subtree_ids(&self.groups, gid);
        self.groups.iter().any(|g| {
            ids.contains(&g.id)
                && g.cloud_query
                    .as_ref()
                    .is_some_and(|q| !hidden_profiles.contains(&q.profile_id))
        })
    }

    /// A dynamic group's children: the resolved ECS tasks / K8s pods,
    /// or the pending / loading / failed state as informational rows.
    /// Returns the rows plus whether any resolved host matched the
    /// search needle (`group_matched` short-circuits the filter: a
    /// matching folder shows everything it has, like manual groups).
    /// Connect messages mirror the dashboard grid / new-tab picker
    /// verbatim, container fallback included.
    fn tree_dynamic_rows<'a>(
        &'a self,
        group: &'a Group,
        depth: usize,
        needle: &str,
        group_matched: bool,
    ) -> (Vec<Element<'a, Message>>, bool) {
        use crate::app::CloudMessage;
        use crate::state::DynamicGroupState;
        let gid = group.id;
        let (ecs_container, k8s_namespace) = match group.cloud_query.as_ref().map(|q| &q.kind) {
            Some(oryxis_core::models::cloud::CloudQueryKind::EcsTasks { container, .. }) => {
                (container.clone(), None)
            }
            Some(oryxis_core::models::cloud::CloudQueryKind::K8sPods { namespace, .. }) => {
                (String::new(), Some(namespace.clone()))
            }
            None => return (Vec::new(), false),
        };

        match self.cloud_dynamic_group_state.get(&gid) {
            None => (vec![tree_info_row(t("cloud_dynamic_group_pending"), depth)], false),
            Some(DynamicGroupState::Loading) => {
                (vec![tree_info_row(t("cloud_discover_running"), depth)], false)
            }
            Some(DynamicGroupState::Failed(msg)) => {
                let retry_msg = Message::Cloud(CloudMessage::DynamicGroupResolve(gid));
                let retry_items: Vec<Element<'a, Message>> = vec![
                    Space::new().width(depth as f32 * INDENT + 16.0).into(),
                    iced_fonts::lucide::refresh_cw()
                        .size(13)
                        .color(OryxisColors::t().text_primary)
                        .into(),
                    Space::new().width(6).into(),
                    text(t("cloud_discover_refresh"))
                        .size(12)
                        .color(OryxisColors::t().text_primary)
                        .into(),
                ];
                let retry = self.sidebar_nav_slot(
                    crate::keynav::SidebarRow::list_button(retry_msg.clone()),
                    STAB,
                    6.0,
                    tree_row_button(retry_items, retry_msg),
                );
                (
                    vec![
                        tree_info_row_owned(
                            format!("{}: {msg}", t("cloud_test_failed")),
                            depth,
                        ),
                        retry,
                    ],
                    false,
                )
            }
            Some(DynamicGroupState::Loaded { hosts, .. }) => {
                let mut rows: Vec<Element<'a, Message>> = Vec::new();
                let mut matched = false;
                // Dynamic hosts have no saved Connection, so the
                // global Privacy Mode default decides the redaction
                // (issue #78); filtering stays on the RAW strings.
                let privacy_terms = self.privacy_terms();
                let redact = |s: &str| {
                    if self.privacy_global_active() {
                        crate::widgets::redact_for_display(
                            s,
                            &privacy_terms,
                            self.privacy_classes(),
                        )
                    } else {
                        s.to_string()
                    }
                };
                for h in hosts {
                    let primary = match &h.container_name {
                        Some(name) if !name.is_empty() => name.clone(),
                        _ => h.resource_id.clone(),
                    };
                    if !group_matched
                        && !needle.is_empty()
                        && !primary.to_lowercase().contains(needle)
                        && !h.resource_id.to_lowercase().contains(needle)
                    {
                        continue;
                    }
                    matched = true;
                    let status_upper: Option<String> =
                        h.status.as_deref().map(|s| s.to_ascii_uppercase());
                    // Only RUNNING (or unknown) tasks can be exec'd
                    // into; a PENDING / STOPPED one yields an opaque
                    // error on click.
                    let connectable =
                        matches!(status_upper.as_deref(), Some("RUNNING") | None);
                    let msg = match &k8s_namespace {
                        Some(ns) => Message::Cloud(CloudMessage::ConnectKubectlExecPod {
                            group_id: gid,
                            namespace: ns.clone(),
                            pod: h.resource_id.clone(),
                            container: h.container_name.clone().unwrap_or_default(),
                        }),
                        None => Message::Cloud(CloudMessage::ConnectEcsExecTask {
                            group_id: gid,
                            task_id: h.resource_id.clone(),
                            task_label: h.label.clone(),
                            container: h
                                .container_name
                                .clone()
                                .unwrap_or_else(|| ecs_container.clone()),
                        }),
                    };
                    let c = OryxisColors::t();
                    let mut items: Vec<Element<'a, Message>> = vec![
                        Space::new().width(depth as f32 * INDENT + 16.0).into(),
                        iced_fonts::lucide::cloud()
                            .size(13)
                            .color(if connectable { c.text_muted } else { c.border })
                            .into(),
                        Space::new().width(6).into(),
                        text(redact(&primary))
                            .size(12)
                            .color(if connectable { c.text_primary } else { c.text_muted })
                            .wrapping(iced::widget::text::Wrapping::None)
                            .into(),
                    ];
                    if let Some(status) = status_upper.as_deref().filter(|s| *s != "RUNNING")
                    {
                        items.push(Space::new().width(6).into());
                        items.push(
                            text(status.to_string()).size(10).color(c.text_muted).into(),
                        );
                    }
                    items.push(Space::new().width(Length::Fill).into());
                    let row = tree_row_button(items, msg.clone());
                    // Non-connectable tasks stay unrecorded so the
                    // keyboard never lands on a dead row (the click
                    // still explains itself via the error path).
                    rows.push(if connectable {
                        self.sidebar_nav_slot(
                            crate::keynav::SidebarRow::list_button(msg),
                            STAB,
                            6.0,
                            row,
                        )
                    } else {
                        row
                    });
                }
                if rows.is_empty() {
                    rows.push(tree_info_row(
                        if needle.is_empty() || group_matched {
                            t("cloud_dynamic_group_no_tasks")
                        } else {
                            t("no_matches")
                        },
                        depth,
                    ));
                }
                (rows, matched)
            }
        }
    }
}

/// Shared row chrome: full-width flat button with hover / press
/// feedback (the button-feedback convention; no flat rows).
fn tree_row_button<'a>(
    items: Vec<Element<'a, Message>>,
    msg: Message,
) -> Element<'a, Message> {
    iced::widget::button(
        dir_row(items)
            .align_y(iced::Alignment::Center)
            .width(Length::Fill),
    )
    .on_press(msg)
    .padding(Padding { top: 5.0, right: 6.0, bottom: 5.0, left: 6.0 })
    .width(Length::Fill)
    .style(|_, status| {
        let bg = match status {
            iced::widget::button::Status::Hovered | iced::widget::button::Status::Pressed => {
                OryxisColors::t().bg_hover
            }
            _ => Color::TRANSPARENT,
        };
        iced::widget::button::Style {
            background: Some(Background::Color(bg)),
            border: Border { radius: Radius::from(6.0), ..Default::default() },
            ..Default::default()
        }
    })
    .into()
}

/// Muted, indented informational row for a dynamic group's transient
/// states (pending / loading / failed / no tasks). Not recorded: the
/// keyboard has nothing to do on it.
fn tree_info_row(label: &str, depth: usize) -> Element<'_, Message> {
    tree_info_row_owned(label.to_string(), depth)
}

fn tree_info_row_owned<'a>(label: String, depth: usize) -> Element<'a, Message> {
    container(
        dir_row(vec![
            Space::new().width(depth as f32 * INDENT + 16.0).into(),
            text(label)
                .size(11)
                .color(OryxisColors::t().text_muted)
                .wrapping(iced::widget::text::Wrapping::None)
                .into(),
        ])
        .align_y(iced::Alignment::Center),
    )
    .padding(Padding { top: 5.0, right: 6.0, bottom: 5.0, left: 6.0 })
    .width(Length::Fill)
    .into()
}

/// Centered muted text for the empty / no-matches states.
fn placeholder(label: &str) -> Element<'_, Message> {
    container(text(label).size(12).color(OryxisColors::t().text_muted))
        .center_x(Length::Fill)
        .padding(Padding { top: 40.0, right: 12.0, bottom: 0.0, left: 12.0 })
        .width(Length::Fill)
        .into()
}

/// Folders sort by their explicit order first, then A-Z, mirroring
/// the dashboard.
fn sort_groups(groups: &mut [&Group]) {
    groups.sort_by(|a, b| {
        a.sort_order
            .cmp(&b.sort_order)
            .then_with(|| a.label.to_lowercase().cmp(&b.label.to_lowercase()))
    });
}

/// Hosts sort A-Z by label (indices ride along untouched: they are
/// positions in `Oryxis::connections`, which `ConnectSsh` consumes).
fn sort_hosts(hosts: &mut [(usize, &oryxis_core::models::Connection)]) {
    hosts.sort_by_key(|(_, c)| c.label.to_lowercase());
}

/// Whether a host row survives the search needle (empty = everything).
fn host_matches(conn: &oryxis_core::models::Connection, needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    conn.label.to_lowercase().contains(needle)
        || conn.hostname.to_lowercase().contains(needle)
        || conn
            .username
            .as_deref()
            .is_some_and(|u| u.to_lowercase().contains(needle))
}
