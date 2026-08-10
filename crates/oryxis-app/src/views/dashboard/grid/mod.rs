//! Dashboard main content, the responsive grid of folder cards, host
//! cards, and dynamic-group cards plus the two early-return paths
//! (zero connections, dynamic-group view). The biggest chunk of
//! `view_dashboard`, lifted here so the orchestrator stays thin.
//!
//! Returns the full `main_content` (toolbar + search + status + body).
//! The mod-level `view_dashboard` only wraps it with the right-side
//! panel slot.

pub(crate) use iced::border::Radius;
pub(crate) use iced::widget::button::Status as BtnStatus;
pub(crate) use iced::widget::{button, container, scrollable, text, text_input, MouseArea, Space};
use iced::widget::column;
pub(crate) use uuid::Uuid;
pub(crate) use iced::{Background, Border, Color, Element, Length, Padding};

pub(crate) use crate::app::{TabsMessage, SshMessage, CloudMessage, NavigationMessage, DashNavItem, Message, Oryxis, SessionGroupMessage, CARD_WIDTH};
pub(crate) use crate::i18n::t;
pub(crate) use crate::os_icon::BrandIcon;
pub(crate) use crate::theme::OryxisColors;
pub(crate) use crate::widgets::{card_grid_columns, dir_align_x, dir_row, distribute_card_grid};

/// Count the leaf panes in a saved session-group layout (for the card
/// subtitle).
pub(crate) fn count_leaves(layout: &oryxis_core::models::PaneLayout) -> usize {
    match layout {
        oryxis_core::models::PaneLayout::Split { a, b, .. } => {
            count_leaves(a) + count_leaves(b)
        }
        oryxis_core::models::PaneLayout::Leaf(_) => 1,
    }
}

/// True when group `gid` has any *visible* content once the hosts /
/// dynamic groups of uninstalled cloud providers are filtered out.
/// Visible content = a direct connection that isn't from a hidden
/// provider, or a child group that is itself visible (a non-hidden
/// dynamic group, or a folder that recurses to visible content). Used
/// to drop provider folders that go empty after a plugin is removed
/// while keeping folders that still hold manual or installed-provider
/// hosts. Memoised; the pre-seeded `false` doubles as cycle guard.
pub(crate) fn group_has_visible_content(
    gid: Uuid,
    groups: &[oryxis_core::models::Group],
    has_visible_conn: &std::collections::HashSet<Uuid>,
    hidden_profiles: &std::collections::HashSet<Uuid>,
    memo: &mut std::collections::HashMap<Uuid, bool>,
) -> bool {
    if let Some(&v) = memo.get(&gid) {
        return v;
    }
    memo.insert(gid, false);
    let mut visible = has_visible_conn.contains(&gid);
    if !visible {
        for child in groups.iter().filter(|g| g.parent_id == Some(gid)) {
            let child_visible = if let Some(q) = child.cloud_query.as_ref() {
                !hidden_profiles.contains(&q.profile_id)
            } else {
                group_has_visible_content(
                    child.id,
                    groups,
                    has_visible_conn,
                    hidden_profiles,
                    memo,
                )
            };
            if child_visible {
                visible = true;
                break;
            }
        }
    }
    memo.insert(gid, visible);
    visible
}

/// Sibling of `group_has_visible_content`: whether the subtree holds
/// any content from an UNINSTALLED provider (a hidden cloud host or a
/// hidden dynamic group). Together the two classify "phantom" provider
/// folders (hidden content, nothing visible). A genuinely empty manual
/// folder has neither flag, so it is NOT phantom and stays pickable as
/// a parent even while some plugin is missing. Memoised; the
/// pre-seeded `false` doubles as cycle guard.
pub(crate) fn group_has_hidden_content(
    gid: Uuid,
    groups: &[oryxis_core::models::Group],
    has_hidden_conn: &std::collections::HashSet<Uuid>,
    hidden_profiles: &std::collections::HashSet<Uuid>,
    memo: &mut std::collections::HashMap<Uuid, bool>,
) -> bool {
    if let Some(&v) = memo.get(&gid) {
        return v;
    }
    memo.insert(gid, false);
    let mut hidden = has_hidden_conn.contains(&gid);
    if !hidden {
        for child in groups.iter().filter(|g| g.parent_id == Some(gid)) {
            let child_hidden = if let Some(q) = child.cloud_query.as_ref() {
                hidden_profiles.contains(&q.profile_id)
            } else {
                group_has_hidden_content(
                    child.id,
                    groups,
                    has_hidden_conn,
                    hidden_profiles,
                    memo,
                )
            };
            if child_hidden {
                hidden = true;
                break;
            }
        }
    }
    memo.insert(gid, hidden);
    hidden
}


/// Map (card, accent-colour, nav-item) tuples to renderable cards: apply
/// the shared `widgets::card_accent_wash` when `glass` is on, then draw
/// the keyboard-selection ring on the item matching `selected`. A free fn
/// (not a closure) so the input/output lifetimes stay tied.
pub(crate) fn apply_card_wash<'a>(
    cards: Vec<(Element<'a, Message>, Color, DashNavItem)>,
    glass: bool,
    selected: Option<DashNavItem>,
    ring_bounds: crate::widgets::BoundsCell,
) -> Vec<Element<'a, Message>> {
    cards
        .into_iter()
        .map(|(el, c, nav)| {
            let el = if glass {
                crate::widgets::card_accent_wash(el, c)
            } else {
                el
            };
            // Ring wrapper on EVERY card (transparent when unringed,
            // see select_ring_opt); the bounds_reporter, invisible to
            // the widget tree, wraps only the ringed card so the Menu
            // key can anchor the context menu at its kebab corner.
            let ringed = selected == Some(nav);
            let el = crate::widgets::select_ring_opt(
                el,
                10.0,
                ringed.then(|| crate::theme::OryxisColors::t().accent),
            );
            if ringed {
                crate::widgets::bounds_reporter(el, ring_bounds.clone())
            } else {
                el
            }
        })
        .collect()
}

// Card/section view methods, split into sibling files.
mod cloud;
mod empty;
mod group;
mod host;
mod session;
mod tree;

impl Oryxis {
    /// Build the set of groups whose subtree contains at least one host
    /// or nested dynamic group whose cloud origin matches `profile_id`.
    /// Used by the cloud-profile filter chip so a parent folder stays
    /// visible when only its descendants match. Each match marks its
    /// whole ancestor chain in one upward walk, so the full set costs
    /// one pass over connections + groups per view call instead of a
    /// recursive subtree scan per folder card per frame.
    pub(crate) fn groups_containing_cloud_profile(
        &self,
        profile_id: Uuid,
    ) -> std::collections::HashSet<Uuid> {
        let parent_of: std::collections::HashMap<Uuid, Option<Uuid>> =
            self.groups.iter().map(|g| (g.id, g.parent_id)).collect();
        let mut set = std::collections::HashSet::new();
        // Walk up the parent chain marking every ancestor. The insert
        // check doubles as cycle protection should upstream data ever
        // hold a parent loop.
        let mark_up = |start: Option<Uuid>,
                       set: &mut std::collections::HashSet<Uuid>| {
            let mut cur = start;
            while let Some(g) = cur {
                if !set.insert(g) {
                    break;
                }
                cur = parent_of.get(&g).copied().flatten();
            }
        };
        for conn in &self.connections {
            if conn.cloud_ref.as_ref().map(|r| r.profile_id) == Some(profile_id) {
                mark_up(conn.group_id, &mut set);
            }
        }
        for g in &self.groups {
            if g.cloud_query.as_ref().is_some_and(|q| q.profile_id == profile_id) {
                // A matching dynamic group makes its *ancestors*
                // visible; the dynamic card itself renders through the
                // dedicated dynamic-group pass below.
                mark_up(g.parent_id, &mut set);
            }
        }
        set
    }

    /// Subtree-match set for the dashboard tag filter: every group
    /// whose descendants include a host carrying at least one selected
    /// tag (ancestors marked in one upward walk, same approach as
    /// `groups_containing_cloud_profile`). `None` while the filter is
    /// off so callers can skip the check entirely.
    pub(crate) fn groups_containing_filtered_tags(
        &self,
    ) -> Option<std::collections::HashSet<Uuid>> {
        if self.host_filter_tags.is_empty() {
            return None;
        }
        let parent_of: std::collections::HashMap<Uuid, Option<Uuid>> =
            self.groups.iter().map(|g| (g.id, g.parent_id)).collect();
        let mut set = std::collections::HashSet::new();
        for conn in &self.connections {
            let matches = conn.tags.iter().any(|tg| {
                self.host_filter_tags.iter().any(|f| f.eq_ignore_ascii_case(tg))
            });
            if !matches {
                continue;
            }
            let mut cur = conn.group_id;
            while let Some(g) = cur {
                if !set.insert(g) {
                    break;
                }
                cur = parent_of.get(&g).copied().flatten();
            }
        }
        Some(set)
    }

    /// Manual group ids eligible for the parent-group pickers (host
    /// editor combo, Settings default-group). Every manual folder
    /// qualifies, including empty ones (a freshly created subgroup
    /// must be a pickable destination before anything lives in it),
    /// EXCEPT phantom provider folders: folders whose subtree holds
    /// content from an uninstalled provider plugin and nothing
    /// visible, which the dashboard hides too. Dynamic `cloud_query`
    /// groups are excluded outright: they're auto-managed, never
    /// valid parents.
    pub(crate) fn visible_group_ids(&self) -> std::collections::HashSet<Uuid> {
        let hidden_profiles = self.hidden_cloud_profile_ids();
        let mut set: std::collections::HashSet<Uuid> = std::collections::HashSet::new();
        if hidden_profiles.is_empty() {
            // Common case: no plugin is missing, so no folder can be
            // phantom; every manual group is pickable.
            for g in &self.groups {
                if g.cloud_query.is_none() {
                    set.insert(g.id);
                }
            }
            return set;
        }
        let mut has_visible_conn: std::collections::HashSet<Uuid> =
            std::collections::HashSet::new();
        let mut has_hidden_conn: std::collections::HashSet<Uuid> =
            std::collections::HashSet::new();
        for c in &self.connections {
            if let Some(gid) = c.group_id {
                if c.cloud_ref
                    .as_ref()
                    .is_some_and(|r| hidden_profiles.contains(&r.profile_id))
                {
                    has_hidden_conn.insert(gid);
                } else {
                    has_visible_conn.insert(gid);
                }
            }
        }
        let mut vis_memo: std::collections::HashMap<Uuid, bool> =
            std::collections::HashMap::new();
        let mut hid_memo: std::collections::HashMap<Uuid, bool> =
            std::collections::HashMap::new();
        for g in &self.groups {
            if g.cloud_query.is_some() {
                continue;
            }
            let visible = group_has_visible_content(
                g.id,
                &self.groups,
                &has_visible_conn,
                &hidden_profiles,
                &mut vis_memo,
            );
            let hidden = group_has_hidden_content(
                g.id,
                &self.groups,
                &has_hidden_conn,
                &hidden_profiles,
                &mut hid_memo,
            );
            // Phantom = only-hidden content. Empty folders have
            // neither flag and stay pickable.
            if visible || !hidden {
                set.insert(g.id);
            }
        }
        set
    }


    /// The host cards currently shown on the dashboard, as absolute
    /// indices into `self.connections`, in display order (group + search +
    /// cloud-profile filters applied, then the user's sort). Shared by the
    /// grid renderer and the keyboard-selection navigation so Tab / arrows
    /// move through exactly what's on screen.
    pub(crate) fn dashboard_host_order(&self) -> Vec<usize> {
        let at_root = self.active_group.is_none();
        let flatten = self.flatten_hosts && at_root;
        let search_lower = self.host_search.to_lowercase();
        let hidden_profiles = self.hidden_cloud_profile_ids();
        let mut host_order: Vec<usize> = (0..self.connections.len())
            .filter(|&i| {
                let conn = &self.connections[i];
                if conn
                    .cloud_ref
                    .as_ref()
                    .is_some_and(|r| hidden_profiles.contains(&r.profile_id))
                {
                    return false;
                }
                if let Some(gid) = self.active_group {
                    if conn.group_id != Some(gid) {
                        return false;
                    }
                } else if conn.group_id.is_some() && !flatten {
                    return false;
                }
                if !search_lower.is_empty()
                    && !conn.label.to_lowercase().contains(&search_lower)
                    && !conn.hostname.to_lowercase().contains(&search_lower)
                    && !conn
                        .tags
                        .iter()
                        .any(|tg| tg.to_lowercase().contains(&search_lower))
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
            })
            .collect();
        self.hosts_sort.sort_items(
            &mut host_order,
            |&i| self.connections[i].label.clone(),
            |&i| self.connections[i].created_at,
        );
        host_order
    }

    /// True on a first-run vault: nothing saved anywhere, so the
    /// dashboard renders `dashboard_empty_state` (no toolbar, no
    /// search field). Read outside the view too, by
    /// `active_view_search_id`, so the keyboard never tries to focus a
    /// search field this screen doesn't build.
    pub(crate) fn dashboard_is_empty(&self) -> bool {
        self.connections.is_empty() && self.groups.is_empty() && self.session_groups.is_empty()
    }

    pub(super) fn dashboard_main_content(&self) -> Element<'_, Message> {
        if self.dashboard_is_empty() {
            // Nothing navigable; keep the keyboard order in sync. The
            // empty state builds no toolbar, so it resets that
            // recording itself.
            self.keynav_clear_content();
            return self.dashboard_empty_state();
        }

        let toolbar = self.dashboard_toolbar();

        // ── Search bar ──
        // The host search lives in the dashboard toolbar
        // (`vault_search_field`) now, so the legacy full-width bar here
        // collapses to a zero-height spacer.
        let search_bar: Element<'_, Message> = Space::new().into();

        // The host editor's validation error renders inside the
        // editor panel itself (`host_panel::view_host_panel`) right
        // above the Save button. Slot reserved for future list-level
        // statuses.
        let status: Element<'_, Message> = Space::new().into();
        let at_root = self.active_group.is_none();
        let flatten = self.flatten_hosts && at_root;

        if let Some(gid) = self.active_group
            && let Some(group) = self.groups.iter().find(|g| g.id == gid)
            && let Some(query) = group.cloud_query.as_ref()
        {
            // Clear the grid recording first, then let the dynamic
            // cloud-group view record its own keyboard rows (connectable
            // tasks / pods, the failed-state retry) into the cleared
            // lists via `content_action_slot`. States with nothing
            // actionable (pending / loading / empty) record nothing, so
            // the clear stands and stale grid items from the previous
            // frame can't be activated from inside the group.
            self.keynav_clear_content();
            return self.dashboard_cloud_group_view(gid, query);
        }

        // Tree mode builds its own depth-aware walk (grid/tree.rs);
        // the flat collectors stay empty so nothing below double
        // renders.
        let tree_mode = self.prefs.host_view_mode == crate::state::HostViewMode::Tree;
        let (group_cards, host_cards) = if tree_mode {
            (Vec::new(), Vec::new())
        } else {
            (self.dashboard_group_cards(), self.dashboard_host_cards())
        };

        // Column count adapts to current window width minus the visible
        // chrome (left nav + optional right panel + horizontal padding).
        // Re-derived on every view() so resizing the window or toggling
        // the side panel reflows the cards into the new column count.
        let nav_width = self.vault_rail_width();
        let panel_open = self.cloud_discover.visible || self.panels.host_panel;
        let panel_width = if panel_open { crate::app::PANEL_WIDTH } else { 0.0 };
        // A side-docked tab strip (issue #87) narrows the content band
        // like the other grids; without it the math yields one column
        // too many and the card row clips at the edge.
        let available = (self.window_size.width
            - nav_width
            - panel_width
            - self.side_strip_reserve()
            - 48.0)
            .max(0.0);
        // List and tree modes force a single column; otherwise the
        // grid reflows responsively to the available width.
        let cols = if self.prefs.host_view_mode == crate::state::HostViewMode::Grid {
            card_grid_columns(available, CARD_WIDTH, 12.0)
        } else {
            1
        };

        // Section header (Termius-style "Groups" / "Hosts" labels).
        // Only rendered in flatten mode at root, where the user can
        // see both lists side-by-side.
        // Wrap the label in a width-fill container so it lines up
        // with the card grid's leading edge. The plain `text` widget
        // shrinks to content and the column's `align_x` pushes the
        // shrunk box around in a way that doesn't always coincide
        // with the card border; making the container Fill anchors it
        // explicitly to the leading edge of the row. Also mirrors
        // Keychain's section_title vertical padding (4 px top, 8 px
        // bottom) so the section labels sit at the same offset
        // relative to the search bar as they do in the Keychain.
        let section_header = |label_key: &'static str| -> Element<'_, Message> {
            container(
                container(
                    text(t(label_key))
                        .size(14)
                        .color(OryxisColors::t().text_muted),
                )
                .width(Length::Fill)
                .align_x(crate::widgets::dir_align_x()),
            )
            .padding(Padding { top: 4.0, right: 0.0, bottom: 8.0, left: 0.0 })
            .into()
        };

        // Saved session groups that live in the current folder. The
        // enumerate index is absolute (into `self.session_groups`), which is
        // what Open/Edit/Delete expect. Tree mode emits them inside
        // its own walk, at their folder's level.
        let session_group_cards: Vec<(Element<'_, Message>, Color, DashNavItem)> = if tree_mode
        {
            Vec::new()
        } else {
            self.session_groups
                .iter()
                .enumerate()
                .filter(|(_, g)| g.group_id == self.active_group)
                .map(|(i, g)| {
                    let (el, color) = self.session_group_card(i, g);
                    (el, color, DashNavItem::SessionGroup(i))
                })
                .collect()
        };

        // Per the `card_accent_glass` setting: glass on → each card gets
        // the soft per-colour wash; off → cards stay pure (just the
        // element, no overlay).
        let glass = self.prefs.card_accent_glass;
        let selected = match self.keynav.selected_in(crate::keynav::FocusZone::Content) {
            Some(crate::keynav::NavItem::Dash(d)) => Some(d),
            _ => None,
        };

        // List mode (cols == 1) renders History-style rows: full-width
        // rounded cards with a small gap. Grid mode keeps the roomier
        // 12px gutters. Tree mode is dense on purpose - a hairline gap
        // keeps its indent guide lines reading as near-continuous.
        let gap = match self.prefs.host_view_mode {
            crate::state::HostViewMode::Grid => 12.0,
            crate::state::HostViewMode::List => 8.0,
            crate::state::HostViewMode::Tree => 2.0,
        };

        let mut content_rows: Vec<Element<'_, Message>> = Vec::new();
        let tree_cards = if tree_mode { self.dashboard_tree_cards() } else { Vec::new() };
        // Record the keyboard-navigation order as visual rows (groups rows
        // then hosts rows, each chunked to the column count) so the key
        // handler can move the selection in 2-D without re-deriving the
        // group order. Groups + session groups share the groups section.
        // Tree mode is one linear section instead: one item per row,
        // in exactly the walk's display order.
        if tree_mode {
            self.keynav_set_content_sections(vec![tree_cards
                .iter()
                .map(|(_, _, n)| vec![crate::keynav::NavItem::Dash(*n)])
                .collect()]);
        } else {
            let cw = cols.max(1);
            let group_nav: Vec<DashNavItem> = group_cards
                .iter()
                .chain(session_group_cards.iter())
                .map(|(_, _, n)| *n)
                .collect();
            let host_nav: Vec<DashNavItem> =
                host_cards.iter().map(|(_, _, n)| *n).collect();
            let dash_row = |c: &[DashNavItem]| {
                c.iter().map(|&n| crate::keynav::NavItem::Dash(n)).collect()
            };
            // Two Tab sections (Groups, then Hosts); arrows still flow
            // continuously across both.
            self.keynav_set_content_sections(vec![
                group_nav.chunks(cw).map(dash_row).collect(),
                host_nav.chunks(cw).map(dash_row).collect(),
            ]);
        }

        // Search filtered everything out but the query parses as an
        // ad-hoc `user@host[:port]` target: offer a centered quick-connect
        // card instead of a bare no-results gap (mirrors the picker row
        // and the toolbar's "Enter to connect" hint).
        if group_cards.is_empty()
            && session_group_cards.is_empty()
            && host_cards.is_empty()
            && tree_cards.is_empty()
            && !self.host_search.trim().is_empty()
            && let Some(conn) = self.quick_connect_target(&self.host_search)
        {
            content_rows.push(self.quick_connect_card(conn));
        }
        if tree_mode {
            let washed =
                apply_card_wash(tree_cards, glass, selected, self.keynav.ring_bounds.clone());
            content_rows.push(distribute_card_grid(washed, 1, gap, gap));
        } else if flatten {
            // Session groups live under the same "Groups" section as host
            // groups (they're both group-shaped entries), instead of a
            // separate "Session Groups" section. Host groups come first.
            if !group_cards.is_empty() || !session_group_cards.is_empty() {
                // `section_header` already carries its own 4/8 vertical
                // padding (mirroring Keychain), so no extra Space below.
                content_rows.push(section_header("groups_section"));
                let mut grouped = group_cards;
                grouped.extend(session_group_cards);
                let grouped = apply_card_wash(grouped, glass, selected, self.keynav.ring_bounds.clone());
                content_rows.push(distribute_card_grid(grouped, cols, gap, gap));
                content_rows.push(Space::new().height(20).into());
            }
            if !host_cards.is_empty() {
                content_rows.push(section_header("hosts_section"));
                let host_cards = apply_card_wash(host_cards, glass, selected, self.keynav.ring_bounds.clone());
                content_rows.push(distribute_card_grid(host_cards, cols, gap, gap));
            }
        } else {
            // Legacy: groups, then session groups, then hosts, in one grid.
            let mut combined = group_cards;
            combined.extend(session_group_cards);
            combined.extend(host_cards);
            let combined = apply_card_wash(combined, glass, selected, self.keynav.ring_bounds.clone());
            content_rows.push(distribute_card_grid(combined, cols, gap, gap));
        }

        // Each grid row holds up to 3 fixed-width cards; once the row
        // is narrower than the available column width, the column's
        // cross-axis alignment decides whether the row sticks to the
        // leading or trailing edge. Use `dir_align_x()` so cards begin
        // from the trailing edge of the LTR layout (= leading edge of
        // the RTL layout), keeping them aligned with the toolbar title
        // / actions on the same side.
        // The column needs `Length::Fill` for `align_x` to have any
        // slack to align inside, without it the column shrinks to
        // content and the rows still hug the leading edge.
        let grid = scrollable(
            column(content_rows)
                .width(Length::Fill)
                .padding(Padding { top: 0.0, right: 24.0, bottom: 24.0, left: 24.0 })
                .align_x(crate::widgets::dir_align_x()),
        )
        .id(iced::widget::Id::new("dashboard-grid-scroll"))
        .height(Length::Fill);

        // Cloud-profile filter chip, only rendered while a filter is
        // active. Sits between search and the grid so the user always
        // has a visible way to clear it. Picks the brand glyph and
        // colour from the active profile's provider so AWS reads
        // orange, K8s blue, etc.
        let filter_chip: Element<'_, Message> = if let Some(filter_pid) =
            self.host_filter_cloud_profile
        {
            let profile = self.cloud_profiles.iter().find(|p| p.id == filter_pid);
            let profile_label = profile.map(|p| p.label.clone()).unwrap_or_default();
            let provider = profile.map(|p| p.provider.as_str()).unwrap_or("cloud");
            let brand_key = match provider {
                "aws" => "aws",
                "k8s" | "kubernetes" => "kubernetes",
                _ => "cloud",
            };
            let (brand_glyph, brand_color) =
                crate::os_icon::provider_icon(brand_key, OryxisColors::t().accent);
            let bg_color = brand_color;
            let chip = container(
                dir_row(vec![
                    brand_glyph.view(12.0, brand_color),
                    Space::new().width(6).into(),
                    text(crate::i18n::t("host_filter_active"))
                        .size(11)
                        .color(OryxisColors::t().text_muted)
                        .into(),
                    Space::new().width(4).into(),
                    text(profile_label)
                        .size(11)
                        .color(OryxisColors::t().text_primary)
                        .into(),
                    Space::new().width(6).into(),
                    button(
                        text("\u{00D7}")
                            .size(13)
                            .color(OryxisColors::t().text_muted),
                    )
                    .on_press(Message::Navigation(NavigationMessage::HostFilterByCloudProfile(None)))
                    .padding(Padding {
                        top: 0.0,
                        right: 6.0,
                        bottom: 0.0,
                        left: 6.0,
                    })
                    .style(|_, _| button::Style {
                        background: None,
                        ..Default::default()
                    })
                    .into(),
                ])
                .align_y(iced::Alignment::Center),
            )
            .padding(Padding {
                top: 4.0,
                right: 4.0,
                bottom: 4.0,
                left: 10.0,
            })
            .style(move |_| container::Style {
                background: Some(Background::Color(Color {
                    a: 0.12,
                    ..bg_color
                })),
                border: Border {
                    radius: Radius::from(14.0),
                    color: Color { a: 0.30, ..bg_color },
                    width: 1.0,
                },
                ..Default::default()
            });
            container(chip)
                .padding(Padding {
                    top: 0.0,
                    right: 24.0,
                    bottom: 8.0,
                    left: 24.0,
                })
                .align_x(dir_align_x())
                .width(Length::Fill)
                .into()
        } else {
            Space::new().into()
        };

        let main_content = column![toolbar, search_bar, filter_chip, status, grid]
            .width(Length::Fill)
            .height(Length::Fill);
        main_content.into()
    }

    /// Centered ad-hoc quick-connect card, shown when the host search
    /// matches nothing but parses as a `user@host[:port]` target. The
    /// whole card is a button (hover + press feedback per convention)
    /// dispatching `QuickConnect`; nothing is saved to the vault.
    fn quick_connect_card(
        &self,
        conn: oryxis_core::models::Connection,
    ) -> Element<'_, Message> {
        let label = conn.label.clone();
        let card = button(
            iced::widget::Column::with_children(vec![
                iced_fonts::lucide::zap()
                    .size(28)
                    .color(OryxisColors::t().accent)
                    .into(),
                Space::new().height(10).into(),
                text(format!("{}: {}", t("quick_connect"), label))
                    .size(15)
                    .color(OryxisColors::t().text_primary)
                    .into(),
                Space::new().height(4).into(),
                text(t("quick_connect_not_saved"))
                    .size(12)
                    .color(OryxisColors::t().text_muted)
                    .into(),
                Space::new().height(8).into(),
                container(
                    text(t("quick_connect_hint"))
                        .size(11)
                        .color(OryxisColors::t().accent),
                )
                .padding(Padding { top: 2.0, right: 8.0, bottom: 2.0, left: 8.0 })
                .style(|_| container::Style {
                    background: Some(Background::Color(OryxisColors::t().bg_hover)),
                    border: Border {
                        radius: Radius::from(4.0),
                        ..Default::default()
                    },
                    ..Default::default()
                })
                .into(),
            ])
            .align_x(iced::alignment::Horizontal::Center),
        )
        .on_press(Message::Ssh(SshMessage::QuickConnect(Box::new(
            crate::state::QuickConnectEntry::bare(conn),
        ))))
        .padding(Padding { top: 24.0, right: 32.0, bottom: 24.0, left: 32.0 })
        .style(|_, status| {
            let (bg, bc) = match status {
                BtnStatus::Hovered => (OryxisColors::t().bg_hover, OryxisColors::t().accent),
                BtnStatus::Pressed => {
                    (OryxisColors::t().bg_selected, OryxisColors::t().accent)
                }
                _ => (OryxisColors::t().bg_surface, OryxisColors::t().border),
            };
            button::Style {
                background: Some(Background::Color(bg)),
                border: Border {
                    radius: Radius::from(10.0),
                    color: bc,
                    width: 1.0,
                },
                ..Default::default()
            }
        });
        container(card)
            .width(Length::Fill)
            .center_x(Length::Fill)
            .padding(Padding { top: 32.0, right: 0.0, bottom: 0.0, left: 0.0 })
            .into()
    }
}

/// Coloured pill rendering a short status string. Background uses the
/// caller-provided accent (success / warning / error / muted) at low
/// alpha so the pill reads as a chip on either light or dark surfaces
/// without fighting the row's own border.
pub(crate) fn status_pill_widget(label: String, accent: Color) -> Element<'static, Message> {
    container(text(label).size(10).color(accent))
        .padding(Padding { top: 2.0, right: 8.0, bottom: 2.0, left: 8.0 })
        .style(move |_| container::Style {
            background: Some(Background::Color(Color { a: 0.15, ..accent })),
            border: Border {
                radius: Radius::from(6.0),
                color: Color { a: 0.30, ..accent },
                width: 1.0,
            },
            ..Default::default()
        })
        .into()
}

/// Compact "5m ago" / "2h ago" / "3d ago" formatter. Negative or
/// zero-second deltas collapse to "now" so freshly-started tasks read
/// cleanly. Values past 30 days fall through to a plain ISO date so
/// older orphans don't claim impossibly large hour counts.
pub(crate) fn relative_time_ago(t: chrono::DateTime<chrono::Utc>) -> String {
    let now = chrono::Utc::now();
    let delta = now.signed_duration_since(t);
    let secs = delta.num_seconds();
    if secs < 5 {
        return "now".to_string();
    }
    if secs < 60 {
        return format!("{secs}s ago");
    }
    let mins = delta.num_minutes();
    if mins < 60 {
        return format!("{mins}m ago");
    }
    let hours = delta.num_hours();
    if hours < 48 {
        return format!("{hours}h ago");
    }
    let days = delta.num_days();
    if days < 30 {
        return format!("{days}d ago");
    }
    // Absolute fallback for old timestamps: show the date in the user's
    // local timezone, not UTC.
    t.with_timezone(&chrono::Local).format("%Y-%m-%d").to_string()
}

#[cfg(test)]
mod hidden_cloud_tests {
    //! Folder visibility recursion that drives hiding provider folders
    //! once their plugin is removed. A folder stays visible while it
    //! holds any non-hidden content (a manual host or a host/group from
    //! an installed provider), and goes hidden once every descendant is
    //! from an uninstalled provider.
    use super::group_has_visible_content;
    use oryxis_core::models::cloud::{
        CloudQuery, CloudQueryKind, ConnectionTemplate, TransportKind,
    };
    use oryxis_core::models::Group;
    use std::collections::{HashMap, HashSet};
    use uuid::Uuid;

    fn folder(parent: Option<Uuid>) -> Group {
        let mut g = Group::new("folder");
        g.parent_id = parent;
        g
    }

    fn dyn_group(parent: Option<Uuid>, profile: Uuid) -> Group {
        let mut g = Group::new("dyn");
        g.parent_id = parent;
        g.cloud_query = Some(CloudQuery {
            profile_id: profile,
            kind: CloudQueryKind::EcsTasks {
                cluster: "c".into(),
                service: "s".into(),
                container: String::new(),
            },
            template: ConnectionTemplate::new(TransportKind::EcsExec),
        });
        g
    }

    fn visible(gid: Uuid, groups: &[Group], visible_conn: &[Uuid], hidden: &[Uuid]) -> bool {
        let has_visible_conn: HashSet<Uuid> = visible_conn.iter().copied().collect();
        let hidden_profiles: HashSet<Uuid> = hidden.iter().copied().collect();
        let mut memo = HashMap::new();
        group_has_visible_content(gid, groups, &has_visible_conn, &hidden_profiles, &mut memo)
    }

    #[test]
    fn folder_with_only_hidden_dynamic_child_is_hidden() {
        let p = Uuid::new_v4();
        let f = folder(None);
        let groups = vec![f.clone(), dyn_group(Some(f.id), p)];
        assert!(!visible(f.id, &groups, &[], &[p]));
    }

    #[test]
    fn folder_with_installed_dynamic_child_is_visible() {
        let p = Uuid::new_v4();
        let f = folder(None);
        let groups = vec![f.clone(), dyn_group(Some(f.id), p)];
        // p not in the hidden set => its provider is installed.
        assert!(visible(f.id, &groups, &[], &[]));
    }

    #[test]
    fn folder_with_manual_host_survives_a_hidden_child() {
        let p = Uuid::new_v4();
        let f = folder(None);
        let groups = vec![f.clone(), dyn_group(Some(f.id), p)];
        // f holds a visible (non-cloud) connection.
        assert!(visible(f.id, &groups, &[f.id], &[p]));
    }

    #[test]
    fn two_level_nest_resolves_through_the_recursion() {
        let p = Uuid::new_v4();
        let f = folder(None);
        let s = folder(Some(f.id));
        let groups = vec![f.clone(), s.clone(), dyn_group(Some(s.id), p)];
        assert!(!visible(f.id, &groups, &[], &[p]));
        assert!(visible(f.id, &groups, &[], &[]));
    }

    #[test]
    fn folder_with_no_visible_content_is_hidden() {
        // Hidden hosts are excluded from `has_visible_conn` upstream, so
        // a folder whose only host is hidden reaches here with nothing.
        let f = folder(None);
        let groups = vec![f.clone()];
        assert!(!visible(f.id, &groups, &[], &[]));
    }
}
