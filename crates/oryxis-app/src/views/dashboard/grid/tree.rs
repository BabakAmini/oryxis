//! Dashboard grid: TREE view mode (issue #102). The mRemoteNG shape
//! at dashboard scale: every level visible at once, folders fold in
//! place (sharing the terminal-sidebar tree's expansion set), no
//! drill-down for manual folders. Rows ARE the grid/list cards -
//! `manual_folder_card`, `dashboard_host_card`, `session_group_card` -
//! indented per level, so hover kebabs, right-click menus, privacy
//! redaction and the vault keynav ring all come along unchanged.
//!
//! Construction order is display order on purpose: the keynav section
//! is recorded from the returned tuples, and the Menu-key anchor rides
//! the ringed card's `bounds_reporter` (see `apply_card_wash`).

use super::*;

/// Indent per tree level, applied on the leading edge (via `dir_row`,
/// so it mirrors under RTL). Card-sized: the 18 px sidebar step reads
/// as nothing next to 56 px rows.
const INDENT: f32 = 28.0;

impl Oryxis {
    /// Every row of the tree, top to bottom, as the same
    /// `(element, color, DashNavItem)` tuples the grid emits.
    pub(crate) fn dashboard_tree_cards(&self) -> Vec<(Element<'_, Message>, Color, DashNavItem)> {
        let search_lower = self.host_search.to_lowercase();
        let searching = !search_lower.trim().is_empty();
        let hidden_profiles = self.hidden_cloud_profile_ids();
        // Provider-hiding: same classification as the grid (dynamic
        // group of a hidden profile, manual folder with no visible
        // content while some plugin is missing).
        let hidden_groups: std::collections::HashSet<Uuid> = if hidden_profiles.is_empty() {
            std::collections::HashSet::new()
        } else {
            let mut has_visible_conn: std::collections::HashSet<Uuid> =
                std::collections::HashSet::new();
            for c in &self.connections {
                if let Some(gid) = c.group_id
                    && !c
                        .cloud_ref
                        .as_ref()
                        .is_some_and(|r| hidden_profiles.contains(&r.profile_id))
                {
                    has_visible_conn.insert(gid);
                }
            }
            let mut memo: std::collections::HashMap<Uuid, bool> =
                std::collections::HashMap::new();
            let mut set: std::collections::HashSet<Uuid> = std::collections::HashSet::new();
            for g in &self.groups {
                let hide = if let Some(q) = g.cloud_query.as_ref() {
                    hidden_profiles.contains(&q.profile_id)
                } else {
                    !group_has_visible_content(
                        g.id,
                        &self.groups,
                        &has_visible_conn,
                        &hidden_profiles,
                        &mut memo,
                    )
                };
                if hide {
                    set.insert(g.id);
                }
            }
            set
        };
        let cloud_filter_groups: Option<std::collections::HashSet<Uuid>> = self
            .host_filter_cloud_profile
            .map(|pid| self.groups_containing_cloud_profile(pid));
        let tag_filter_groups: Option<std::collections::HashSet<Uuid>> =
            self.groups_containing_filtered_tags();
        // Count + brand maps, the same pre-pass the folder cards use.
        let mut direct_host_count: std::collections::HashMap<Uuid, usize> =
            std::collections::HashMap::new();
        let mut first_cloud_profile: std::collections::HashMap<Uuid, Uuid> =
            std::collections::HashMap::new();
        for conn in &self.connections {
            if let Some(cgid) = conn.group_id {
                if conn
                    .cloud_ref
                    .as_ref()
                    .is_some_and(|r| hidden_profiles.contains(&r.profile_id))
                {
                    continue;
                }
                *direct_host_count.entry(cgid).or_insert(0) += 1;
                if let Some(cref) = conn.cloud_ref.as_ref() {
                    first_cloud_profile.entry(cgid).or_insert(cref.profile_id);
                }
            }
        }
        let mut nested_group_count: std::collections::HashMap<Uuid, usize> =
            std::collections::HashMap::new();
        let mut child_query_brand: std::collections::HashMap<Uuid, &'static str> =
            std::collections::HashMap::new();
        for g in &self.groups {
            if let Some(pgid) = g.parent_id {
                if hidden_groups.contains(&g.id) {
                    continue;
                }
                *nested_group_count.entry(pgid).or_insert(0) += 1;
                if let Some(q) = g.cloud_query.as_ref() {
                    child_query_brand.entry(pgid).or_insert(match q.kind {
                        oryxis_core::models::cloud::CloudQueryKind::EcsTasks { .. } => "ecs",
                        oryxis_core::models::cloud::CloudQueryKind::K8sPods { .. } => {
                            "kubernetes"
                        }
                    });
                }
            }
        }
        let infer_brand = |gid: &Uuid| -> Option<&'static str> {
            child_query_brand.get(gid).copied().or_else(|| {
                first_cloud_profile.get(gid).and_then(|pid| {
                    self.cloud_profiles
                        .iter()
                        .find(|p| p.id == *pid)
                        .map(|p| match p.provider.as_str() {
                            "aws" => "aws",
                            "k8s" | "kubernetes" => "kubernetes",
                            _ => "cloud",
                        })
                })
            })
        };
        let privacy_terms = self.privacy_terms();

        // Which host indices pass the non-search filters (provider
        // hiding, cloud-profile chip, tag filter). The search filter
        // is applied in the walk, where a matching ancestor short-
        // circuits it (a folder that matches shows all its children).
        let host_passes = |i: usize| -> bool {
            let conn = &self.connections[i];
            if conn
                .cloud_ref
                .as_ref()
                .is_some_and(|r| hidden_profiles.contains(&r.profile_id))
            {
                return false;
            }
            if let Some(filter_pid) = self.host_filter_cloud_profile
                && conn.cloud_ref.as_ref().map(|r| r.profile_id) != Some(filter_pid)
            {
                return false;
            }
            if !self.host_filter_tags.is_empty()
                && !conn.tags.iter().any(|tg| {
                    self.host_filter_tags.iter().any(|f| f.eq_ignore_ascii_case(tg))
                })
            {
                return false;
            }
            true
        };
        let host_search_match = |i: usize| -> bool {
            let conn = &self.connections[i];
            !searching
                || conn.label.to_lowercase().contains(&search_lower)
                || conn.hostname.to_lowercase().contains(&search_lower)
                || conn.tags.iter().any(|tg| tg.to_lowercase().contains(&search_lower))
        };
        let group_passes = |g: &oryxis_core::models::Group| -> bool {
            !hidden_groups.contains(&g.id)
                && cloud_filter_groups.as_ref().is_none_or(|v| v.contains(&g.id))
                && tag_filter_groups.as_ref().is_none_or(|v| v.contains(&g.id))
        };

        // Search visibility per group is decided BEFORE any card is
        // built (construction must follow display order: the keynav
        // section is recorded from the emitted tuples); see
        // `search_visible_entry` below. Memoised across the walk.
        let mut search_memo: std::collections::HashMap<Uuid, bool> =
            std::collections::HashMap::new();

        let mut rows: Vec<(Element<'_, Message>, Color, DashNavItem)> = Vec::new();
        let mut visited: std::collections::HashSet<Uuid> = std::collections::HashSet::new();

        // Roots: parentless groups plus broken-ancestry ones (dangling
        // or cyclic parents degrade to root, the dashboard policy).
        let mut roots: Vec<usize> = (0..self.groups.len())
            .filter(|&i| {
                let g = &self.groups[i];
                g.parent_id.is_none()
                    || !oryxis_core::models::Group::is_reachable_from_root(&self.groups, g.id)
            })
            .collect();
        self.hosts_sort.sort_items(
            &mut roots,
            |&i| self.groups[i].label.clone(),
            |&i| self.groups[i].created_at,
        );
        for i in roots {
            self.tree_walk_group(
                &mut rows,
                &self.groups[i],
                0,
                searching,
                &search_lower,
                &mut search_memo,
                &group_passes,
                &host_passes,
                &host_search_match,
                &infer_brand,
                &direct_host_count,
                &nested_group_count,
                &privacy_terms,
                &mut visited,
            );
        }

        // Root session groups (no folder, or a dangling folder id).
        let group_exists = |gid: Uuid| self.groups.iter().any(|g| g.id == gid);
        let mut root_sessions: Vec<usize> = (0..self.session_groups.len())
            .filter(|&i| {
                self.session_groups[i]
                    .group_id
                    .filter(|gid| group_exists(*gid))
                    .is_none()
            })
            .collect();
        self.hosts_sort.sort_items(
            &mut root_sessions,
            |&i| self.session_groups[i].label.clone(),
            |&i| self.session_groups[i].created_at,
        );
        for i in root_sessions {
            let sg = &self.session_groups[i];
            if searching && !sg.label.to_lowercase().contains(&search_lower) {
                continue;
            }
            let (el, color) = self.session_group_card(i, sg);
            rows.push((indent_card(el, 0), color, DashNavItem::SessionGroup(i)));
        }

        // Root hosts: no group, or a group id that no longer resolves.
        let mut root_hosts: Vec<usize> = (0..self.connections.len())
            .filter(|&i| {
                self.connections[i]
                    .group_id
                    .filter(|gid| group_exists(*gid))
                    .is_none()
                    && host_passes(i)
                    && host_search_match(i)
            })
            .collect();
        self.hosts_sort.sort_items(
            &mut root_hosts,
            |&i| self.connections[i].label.clone(),
            |&i| self.connections[i].created_at,
        );
        for i in root_hosts {
            let (el, color) = self.dashboard_host_card(i, &privacy_terms);
            rows.push((indent_card(el, 0), color, DashNavItem::Host(i)));
        }
        rows
    }

    /// Emit one group's row and, when expanded, its subtree - strictly
    /// in display order (subfolders, session groups, hosts).
    #[allow(clippy::too_many_arguments)]
    fn tree_walk_group<'a>(
        &'a self,
        rows: &mut Vec<(Element<'a, Message>, Color, DashNavItem)>,
        group: &'a oryxis_core::models::Group,
        depth: usize,
        searching: bool,
        search_lower: &str,
        search_memo: &mut std::collections::HashMap<Uuid, bool>,
        group_passes: &dyn Fn(&oryxis_core::models::Group) -> bool,
        host_passes: &dyn Fn(usize) -> bool,
        host_search_match: &dyn Fn(usize) -> bool,
        infer_brand: &dyn Fn(&Uuid) -> Option<&'static str>,
        direct_host_count: &std::collections::HashMap<Uuid, usize>,
        nested_group_count: &std::collections::HashMap<Uuid, usize>,
        privacy_terms: &[String],
        visited: &mut std::collections::HashSet<Uuid>,
    ) {
        if !visited.insert(group.id) {
            return;
        }
        if !group_passes(group) {
            return;
        }
        let label_match =
            !searching || group.label.to_lowercase().contains(search_lower);
        if searching
            && !label_match
            && !search_visible_entry(self, group.id, search_lower, search_memo)
        {
            return;
        }
        let gid = group.id;

        if let Some(query) = group.cloud_query.as_ref() {
            // Dynamic (ECS / K8s) groups keep their drill-down: the
            // dedicated cloud-group screen (task list, refresh,
            // transport) is richer than inline rows at this scale.
            rows.push((
                self.tree_dynamic_group_row(group, query, depth),
                OryxisColors::t().accent,
                DashNavItem::Group(gid),
            ));
            return;
        }

        let expanded = searching || self.hosts_tree_expanded.contains(&gid);
        let direct_hosts = direct_host_count.get(&gid).copied().unwrap_or(0);
        let nested_groups = nested_group_count.get(&gid).copied().unwrap_or(0);
        let count_text = crate::i18n::host_count(direct_hosts + nested_groups);
        let (el, color) =
            self.manual_folder_card(group, count_text, infer_brand(&gid), Some(expanded));
        rows.push((indent_card(el, depth), color, DashNavItem::Group(gid)));
        if !expanded {
            return;
        }

        let mut children: Vec<usize> = (0..self.groups.len())
            .filter(|&i| self.groups[i].parent_id == Some(gid))
            .collect();
        self.hosts_sort.sort_items(
            &mut children,
            |&i| self.groups[i].label.clone(),
            |&i| self.groups[i].created_at,
        );
        for i in children {
            self.tree_walk_group(
                rows,
                &self.groups[i],
                depth + 1,
                searching,
                search_lower,
                search_memo,
                group_passes,
                host_passes,
                host_search_match,
                infer_brand,
                direct_host_count,
                nested_group_count,
                privacy_terms,
                visited,
            );
        }

        let mut sessions: Vec<usize> = (0..self.session_groups.len())
            .filter(|&i| self.session_groups[i].group_id == Some(gid))
            .collect();
        self.hosts_sort.sort_items(
            &mut sessions,
            |&i| self.session_groups[i].label.clone(),
            |&i| self.session_groups[i].created_at,
        );
        for i in sessions {
            let sg = &self.session_groups[i];
            if searching
                && !label_match
                && !sg.label.to_lowercase().contains(search_lower)
            {
                continue;
            }
            let (el, color) = self.session_group_card(i, sg);
            rows.push((indent_card(el, depth + 1), color, DashNavItem::SessionGroup(i)));
        }

        let mut hosts: Vec<usize> = (0..self.connections.len())
            .filter(|&i| {
                self.connections[i].group_id == Some(gid)
                    && host_passes(i)
                    && (label_match || host_search_match(i))
            })
            .collect();
        self.hosts_sort.sort_items(
            &mut hosts,
            |&i| self.connections[i].label.clone(),
            |&i| self.connections[i].created_at,
        );
        for i in hosts {
            let (el, color) = self.dashboard_host_card(i, privacy_terms);
            rows.push((indent_card(el, depth + 1), color, DashNavItem::Host(i)));
        }
    }

    /// A dynamic (cloud-query) group as a tree row: brand chip, label,
    /// query subtitle, hover kebab, drill-in chevron. Press opens the
    /// dedicated cloud-group screen, same as the card.
    fn tree_dynamic_group_row<'a>(
        &'a self,
        group: &'a oryxis_core::models::Group,
        query: &'a oryxis_core::models::cloud::CloudQuery,
        depth: usize,
    ) -> Element<'a, Message> {
        let gid = group.id;
        let subtitle = match &query.kind {
            oryxis_core::models::cloud::CloudQueryKind::EcsTasks { cluster, .. } => {
                format!("ECS · {cluster}")
            }
            oryxis_core::models::cloud::CloudQueryKind::K8sPods {
                context, namespace, ..
            } => format!("K8s · {context}/{namespace}"),
        };
        let query_brand: &str = match query.kind {
            oryxis_core::models::cloud::CloudQueryKind::EcsTasks { .. } => "ecs",
            oryxis_core::models::cloud::CloudQueryKind::K8sPods { .. } => "kubernetes",
        };
        let icon_id: &str =
            group.icon.as_deref().filter(|s| !s.is_empty()).unwrap_or(query_brand);
        let folder_glyph = crate::os_icon::custom_icon_glyph(icon_id);
        let folder_bg = group
            .color
            .as_deref()
            .and_then(crate::os_icon::parse_hex_color)
            .unwrap_or_else(|| {
                crate::os_icon::provider_icon(icon_id, OryxisColors::t().accent).1
            });
        let host_style =
            crate::widgets::resolve_host_icon_style(None, &self.prefs.default_host_icon);
        let icon_box = crate::widgets::host_icon(
            host_style,
            folder_bg,
            &group.label,
            Some(folder_glyph.view(18.0, Color::WHITE)),
            32.0,
        );
        let rtl = crate::i18n::is_rtl_layout();
        let show_dots = self.hover.dynamic_group_card == Some(gid);
        let trailing: Element<'_, Message> = if show_dots {
            crate::widgets::card_kebab_button(
                OryxisColors::t().text_muted,
                true,
                Message::Cloud(CloudMessage::ShowDynamicGroupCardMenu(gid)),
            )
            .into()
        } else {
            let chevron = if rtl {
                iced_fonts::lucide::chevron_left()
            } else {
                iced_fonts::lucide::chevron_right()
            };
            container(chevron.size(14).color(OryxisColors::t().text_muted))
                .center_x(Length::Fixed(22.0))
                .center_y(Length::Fixed(22.0))
                .into()
        };
        let card_padding = if rtl {
            Padding { top: 8.0, right: 8.0, bottom: 8.0, left: 24.0 }
        } else {
            Padding { top: 8.0, right: 24.0, bottom: 8.0, left: 8.0 }
        };
        let card = button(
            container(
                dir_row(vec![
                    icon_box,
                    Space::new().width(10).into(),
                    iced::widget::column![
                        text(group.label.clone())
                            .size(13)
                            .color(OryxisColors::t().text_primary)
                            .wrapping(iced::widget::text::Wrapping::None),
                        text(subtitle)
                            .size(10)
                            .color(OryxisColors::t().text_muted)
                            .wrapping(iced::widget::text::Wrapping::None),
                    ]
                    .width(Length::Fill)
                    .align_x(crate::widgets::dir_align_x())
                    .clip(true)
                    .into(),
                ])
                .align_y(iced::Alignment::Center),
            )
            .padding(card_padding),
        )
        .on_press(Message::Navigation(NavigationMessage::OpenGroup(gid)))
        .width(Length::Fill)
        .style(|_, status| {
            let (bg, bc, bw) = match status {
                BtnStatus::Hovered => {
                    (OryxisColors::t().bg_hover, OryxisColors::t().accent, 1.5)
                }
                BtnStatus::Pressed => {
                    (OryxisColors::t().bg_selected, OryxisColors::t().accent, 2.0)
                }
                _ => (OryxisColors::t().bg_surface, OryxisColors::t().border, 1.0),
            };
            button::Style {
                background: Some(Background::Color(bg)),
                border: Border { radius: Radius::from(10.0), color: bc, width: bw },
                ..Default::default()
            }
        });
        let dots_align = if rtl {
            iced::alignment::Horizontal::Left
        } else {
            iced::alignment::Horizontal::Right
        };
        let dots_pad = if rtl {
            Padding { top: 0.0, right: 0.0, bottom: 0.0, left: 4.0 }
        } else {
            Padding { top: 0.0, right: 4.0, bottom: 0.0, left: 0.0 }
        };
        let overlay = container(trailing)
            .width(Length::Fill)
            .height(Length::Fill)
            .align_x(dots_align)
            .align_y(iced::alignment::Vertical::Center)
            .padding(dots_pad);
        let stacked: Element<'_, Message> =
            iced::widget::Stack::new().push(card).push(overlay).into();
        let wrapped = MouseArea::new(stacked)
            .on_enter(Message::Cloud(CloudMessage::DynamicGroupCardHovered(gid)))
            .on_exit(Message::Cloud(CloudMessage::DynamicGroupCardUnhovered(gid)))
            .on_right_press(Message::Cloud(CloudMessage::ShowDynamicGroupCardMenu(gid)));
        indent_card(
            Element::from(container(wrapped).width(Length::Fill).clip(true)),
            depth,
        )
    }
}

/// Entry shim so the memoised recursion can be called with `&self`
/// borrows already split (the walk holds `rows` mutably).
fn search_visible_entry(
    app: &Oryxis,
    gid: Uuid,
    search_lower: &str,
    memo: &mut std::collections::HashMap<Uuid, bool>,
) -> bool {
    fn rec(
        app: &Oryxis,
        gid: Uuid,
        search_lower: &str,
        memo: &mut std::collections::HashMap<Uuid, bool>,
    ) -> bool {
        if let Some(&v) = memo.get(&gid) {
            return v;
        }
        memo.insert(gid, false);
        let Some(group) = app.groups.iter().find(|g| g.id == gid) else {
            return false;
        };
        let v = group.label.to_lowercase().contains(search_lower)
            || app.connections.iter().any(|c| {
                c.group_id == Some(gid)
                    && (c.label.to_lowercase().contains(search_lower)
                        || c.hostname.to_lowercase().contains(search_lower)
                        || c.tags.iter().any(|tg| tg.to_lowercase().contains(search_lower)))
            })
            || app.session_groups.iter().any(|sg| {
                sg.group_id == Some(gid) && sg.label.to_lowercase().contains(search_lower)
            })
            || app
                .groups
                .iter()
                .filter(|g| g.parent_id == Some(gid))
                .any(|g| rec(app, g.id, search_lower, memo));
        memo.insert(gid, v);
        v
    }
    rec(app, gid, search_lower, memo)
}

/// Leading indent for a tree row, mirrored under RTL like every other
/// leading-edge inset.
fn indent_card<'a>(card: Element<'a, Message>, depth: usize) -> Element<'a, Message> {
    if depth == 0 {
        return card;
    }
    dir_row(vec![
        Space::new().width(depth as f32 * INDENT).into(),
        card,
    ])
    .into()
}
