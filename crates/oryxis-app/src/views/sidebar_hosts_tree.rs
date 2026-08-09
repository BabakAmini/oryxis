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
            self.tree_group_rows(&mut rows, group, 0, &needle, &mut visited);
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
    /// `rows`. Returns whether anything was appended, which under a
    /// search is what tells the parent that a descendant matched.
    fn tree_group_rows<'a>(
        &'a self,
        rows: &mut Vec<Element<'a, Message>>,
        group: &'a Group,
        depth: usize,
        needle: &str,
        visited: &mut std::collections::HashSet<Uuid>,
    ) -> bool {
        if !visited.insert(group.id) {
            return false;
        }
        let searching = !needle.is_empty();
        let expanded = searching || self.hosts_tree_expanded.contains(&group.id);

        let mut children: Vec<&Group> = self
            .groups
            .iter()
            .filter(|g| g.parent_id == Some(group.id))
            .collect();
        sort_groups(&mut children);
        let mut hosts: Vec<(usize, &oryxis_core::models::Connection)> = self
            .connections
            .iter()
            .enumerate()
            .filter(|(_, c)| c.group_id == Some(group.id))
            .collect();
        sort_hosts(&mut hosts);

        // Build the subtree first: under a search the group only shows
        // when its own label matches or something below it does.
        let mut subtree: Vec<Element<'a, Message>> = Vec::new();
        let mut any_below = false;
        for child in children {
            any_below |= self.tree_group_rows(&mut subtree, child, depth + 1, needle, visited);
        }
        let label_match = !searching || group.label.to_lowercase().contains(needle);
        for (idx, conn) in hosts {
            // A matching group shows its whole host list; otherwise
            // only the hosts that match themselves.
            if label_match || host_matches(conn, needle) {
                subtree.push(self.tree_host_row(idx, conn, depth + 1));
                any_below = true;
            }
        }
        if searching && !label_match && !any_below {
            return false;
        }

        rows.push(self.tree_group_row(group, depth, expanded));
        if expanded {
            rows.append(&mut subtree);
        }
        true
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
        let folder: Element<'a, Message> = match group.icon.as_deref().filter(|s| !s.is_empty()) {
            Some(icon_id) => crate::os_icon::custom_icon_glyph(icon_id).view(14.0, tint),
            None if expanded => iced_fonts::lucide::folder_open().size(14).color(tint).into(),
            None => iced_fonts::lucide::folder().size(14).color(tint).into(),
        };
        let subtree_hosts = self.tree_subtree_host_count(group.id);
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
