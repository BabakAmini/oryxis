//! Tab bar + window chrome.
//!
//! Tabs render as pill-shaped chips with an OS-coloured icon badge on the
//! left that morphs into an X on hover/active (Termius-style close
//! affordance). The right-hand cluster, `[+]`, `[⋯]`, then the window
//! chrome (minimize / maximize / close), is pinned to the window edge and
//! never gets pushed off when many tabs are open. Tabs themselves shrink
//! uniformly to a minimum width as the bar fills, while the active tab
//! keeps its natural width so its label stays fully readable.

pub(crate) use iced::border::Radius;
pub(crate) use iced::widget::button::Status as BtnStatus;
pub(crate) use iced::widget::{button, container, row, scrollable, text, MouseArea, Space};
pub(crate) use iced::{Background, Border, Color, Element, Length, Padding};

pub(crate) use crate::app::{Message, Oryxis};
pub(crate) use crate::state::View;
pub(crate) use crate::theme::{OryxisColors, SYSTEM_UI_SEMIBOLD};

pub(crate) const TAB_HEIGHT: f32 = 26.0;
pub(crate) const TAB_ICON_SLOT: f32 = 16.0;
/// Fixed width of a compact (Chrome-style) pinned tab chip.
pub(crate) const CHIP_W: f32 = 38.0;

/// Maximum width a tab claims when it has the room. Sized to fit a typical
/// label like "user@hostname.example.com" without truncation. The active
/// tab always uses this; inactives only when there's space.
pub(crate) const TAB_NATURAL_WIDTH: f32 = 200.0;
/// Floor below which we don't shrink, once a tab gets this narrow the
/// label is mostly ellipses anyway, and going lower kills hit-target ergonomics.
/// Picked to fit "OS-badge + ~8 chars + ellipsis" comfortably.
pub(crate) const TAB_MIN_WIDTH: f32 = 110.0;

/// Approximate per-character width at the tab label's font/size combo
/// (12 px SemiBold). Used to figure out how many chars fit in a compact
/// tab before the truncation kicks in.
pub(crate) const TAB_CHAR_WIDTH: f32 = 7.0;

/// Spacing between tabs, extracted into a constant so the width math
/// can subtract it without drifting from the actual `row.spacing()`.
// Tabs separate by their own internal padding; the strip adds only a hairline
// gap so adjacent hover/active fills don't visually merge. (padding + 6px gap +
// padding read as too much space, especially between compact pinned chips.)
pub(crate) const TAB_SPACING: f32 = 1.0;

/// Total height of the tab bar. Sized to comfortably fit a session tab
/// (whose inner row already includes the OS-icon badge at 18 px plus
/// padding) without feeling cramped, and tall enough that the chrome
/// buttons read as proper hit targets when filled corner-to-corner.
pub(crate) const BAR_HEIGHT: f32 = 40.0;

pub(crate) const SIDEBAR_TOGGLE_WIDTH: f32 = 28.0;
// `+` and `⋯` (jump-to) live in the right cluster next to the chrome
// buttons, so they share the chrome width, gives the whole strip a
// uniform 46×BAR_HEIGHT cell rhythm.
pub(crate) const PLUS_BUTTON_WIDTH: f32 = 46.0;
pub(crate) const DOTS_BUTTON_WIDTH: f32 = 46.0;
pub(crate) const SIDEBAR_BUTTON_WIDTH: f32 = 46.0;
pub(crate) const CHROME_BUTTON_WIDTH: f32 = 46.0;
pub(crate) const CHROME_TOTAL_WIDTH: f32 = CHROME_BUTTON_WIDTH * 3.0;

impl Oryxis {
    /// The default top bar: burger + tab strip + `+`/`⋯` + side-panel
    /// toggle + window chrome, all in one row.
    pub(crate) fn view_tab_bar(&self) -> Element<'_, Message> {
        self.tab_strip_bar(false)
    }

    /// Bottom-docked variant of the strip (Settings -> Interface -> Tab
    /// bar position): just the tabs, the `+` and the `⋯` jump button.
    /// The window chrome stays in `view_top_chrome_bar`.
    pub(crate) fn view_bottom_tab_strip(&self) -> Element<'_, Message> {
        self.tab_strip_bar(true)
    }

    /// Slim top bar for the bottom-docked layout: burger, an empty
    /// window-drag area, the side-panel toggle and the chrome buttons.
    /// Keeps the titlebar affordances (drag, double-click maximize,
    /// minimize / maximize / close) at the top of the window where every
    /// OS puts them, while the tabs live at the bottom.
    pub(crate) fn view_top_chrome_bar(&self) -> Element<'_, Message> {
        let drag_area: Element<'_, Message> = MouseArea::new(
            container(Space::new())
                .width(Length::Fill)
                .height(Length::Fixed(BAR_HEIGHT)),
        )
        .on_press(Message::WindowDrag)
        .on_double_click(Message::WindowMaximizeToggle)
        .into();

        let mut cluster_items: Vec<Element<'_, Message>> = Vec::new();
        if self.active_tab.is_some() {
            cluster_items.push(sidebar_btn());
            cluster_items.push(Space::new().width(2).into());
        }
        cluster_items.push(self.window_chrome_row().into());
        let right_cluster: Element<'_, Message> = crate::widgets::dir_row(cluster_items)
            .align_y(iced::Alignment::Center)
            .into();

        let leading: Vec<Element<'_, Message>> = vec![
            burger_menu_btn(self.show_burger_menu),
            Space::new().width(1).height(TAB_HEIGHT).into(),
            drag_area,
            right_cluster,
        ];
        let bar_bg = self.tab_bar_background();
        container(
            crate::widgets::dir_row(leading).align_y(iced::Alignment::Center),
        )
        .width(Length::Fill)
        .height(Length::Fixed(BAR_HEIGHT))
        .style(move |_| container::Style {
            background: Some(bar_bg),
            ..Default::default()
        })
        .into()
    }

    /// Window controls (minimize / maximize-restore / close) in their own
    /// dir_row so the close button ends up on the leading edge under RTL,
    /// matching how macOS and GNOME flip traffic-light buttons when the
    /// locale flips. Shared by the combined top bar and the slim chrome
    /// bar of the bottom-docked layout.
    fn window_chrome_row(&self) -> iced::widget::Row<'_, Message> {
        let max_icon = if self.window_maximized {
            iced_fonts::codicon::chrome_restore()
        } else {
            iced_fonts::codicon::chrome_maximize()
        };
        crate::widgets::dir_row(vec![
            window_btn(
                iced_fonts::codicon::chrome_minimize(),
                Message::WindowMinimize,
                OryxisColors::t().text_secondary,
            ),
            window_btn(
                max_icon,
                Message::WindowMaximizeToggle,
                OryxisColors::t().text_secondary,
            ),
            window_btn(
                iced_fonts::codicon::chrome_close(),
                Message::WindowClose,
                OryxisColors::t().error,
            ),
        ])
        .align_y(iced::Alignment::Center)
    }

    /// Shared bar background: the accent wash (tinted leading edge fading
    /// to the bar surface) when enabled, else the flat sidebar surface.
    /// Used by the combined top bar, the slim chrome bar and the
    /// bottom-docked strip so the chrome reads as one material.
    fn tab_bar_background(&self) -> Background {
        let bar_base = OryxisColors::t().bg_sidebar;
        if self.setting_tab_accent_wash {
            let washed = crate::theme::mix(bar_base, self.top_accent_tint(), 0.16);
            Background::Gradient(iced::Gradient::Linear(
                iced::gradient::Linear::new(iced::Radians(std::f32::consts::FRAC_PI_2))
                    .add_stop(0.0, washed)
                    .add_stop(0.9, bar_base),
            ))
        } else {
            Background::Color(bar_base)
        }
    }

    /// Build the tab strip bar. `bottom == false` is the classic combined
    /// top bar (burger + tabs + right cluster with chrome); `bottom ==
    /// true` renders only the strip pieces (tabs, `+`, `⋯`), the chrome
    /// half living in `view_top_chrome_bar` instead.
    fn tab_strip_bar(&self, bottom: bool) -> Element<'_, Message> {
        let n_tabs = self.tabs.len();
        let active_idx = self.active_tab;

        // For compaction we need a rough estimate of the strip's width
        // (active tab natural, inactives shrink to fit). The exact
        // value isn't critical, `scrollable` is the safety net for
        // any miscalculation. Subtract everything else on the row.
        // RIGHT_CLUSTER_WIDTH = +(28) + 2 + ⋯(28) + 2 + chrome(3*46)
        const RIGHT_CLUSTER_WIDTH: f32 = SIDEBAR_BUTTON_WIDTH
            + 2.0
            + PLUS_BUTTON_WIDTH
            + 2.0
            + DOTS_BUTTON_WIDTH
            + 2.0
            + CHROME_TOTAL_WIDTH;
        // Workspace mode prepends area tabs (Hosts, SFTP) that consume
        // strip width before the connection tabs even start; subtract
        // a rough estimate so the connection-tab allocator and the
        // scroll_mode trigger see the actual budget. Each area tab is
        // roughly icon(16) + gap(6) + label(~50) + padding(20) ~= 90 px.
        let area_tab_count = 1 + (self.sftp_enabled as u32);
        const AREA_TAB_APPROX_WIDTH: f32 = 100.0;
        let area_tabs_total = area_tab_count as f32
            * (AREA_TAB_APPROX_WIDTH + TAB_SPACING);
        // Burger menu button (SIDEBAR_TOGGLE_WIDTH) lives on the
        // leading edge.
        let burger_width = SIDEBAR_TOGGLE_WIDTH;
        // Logo removed from the top strip; no width reserved for it.
        let logo_width = 0.0;
        // Bottom-docked strip: the burger + chrome live in the slim top
        // bar, so the tabs get (almost) the full window width; only the
        // `+` / `⋯` companions still share the row.
        let reserved = if bottom {
            PLUS_BUTTON_WIDTH + 2.0 + DOTS_BUTTON_WIDTH
        } else {
            burger_width + logo_width + RIGHT_CLUSTER_WIDTH
        };
        let approx_strip_width = (self.window_size.width
            - reserved
            - area_tabs_total
            - 12.0)
            .max(120.0);

        // Per-tab width allocation. Inactive tabs hug their own label
        // (clamped to [MIN, NATURAL]); the active tab claims the full
        // NATURAL width so focusing it visibly "fattens" the chip
        // (JetBrains-style). When the combined widths overflow the strip
        // the inactive tabs shrink proportionally toward MIN (the
        // scrollable is the final safety net). Compact pinned chips are
        // fixed at CHIP_W and don't participate in the flexible sizing.
        let close_on_right = self.setting_tab_close_button_side == "right";
        let compact_pins = self.setting_pinned_tab_style == "compact";
        let mut session_widths = vec![TAB_MIN_WIDTH; n_tabs];
        let mut max_inactive_content = TAB_MIN_WIDTH;
        for (i, tab) in self.tabs.iter().enumerate() {
            if tab.pinned && compact_pins {
                session_widths[i] = CHIP_W;
            } else if active_idx == Some(i) {
                session_widths[i] = TAB_NATURAL_WIDTH;
            } else {
                let cw = tab_content_width(
                    tab.display_label(self.tab_auto_title(tab)),
                    close_on_right,
                    tab.pane_count() > 1,
                );
                session_widths[i] = cw;
                max_inactive_content = max_inactive_content.max(cw);
            }
        }
        let n_f = n_tabs as f32;
        let total_spacing = TAB_SPACING * (n_f - 1.0).max(0.0);
        let desired_total: f32 = session_widths.iter().sum::<f32>() + total_spacing;
        // Scroll-mode trigger (brings in the `⋯` jump button): the tabs
        // at their desired widths plus spacing wouldn't fit the strip.
        // Computed from the same per-tab widths the strip actually
        // renders, so the button doesn't pop in while everything still
        // fits.
        let scroll_mode = desired_total > approx_strip_width;
        // Overflow shrink: pull the inactive tabs proportionally toward
        // MIN so the strip stays packed before the scrollable has to
        // scroll. The active tab keeps its NATURAL width; compact pins
        // keep CHIP_W.
        if desired_total > approx_strip_width {
            let overflow = desired_total - approx_strip_width;
            let shrinkable: f32 = (0..n_tabs)
                .filter(|&i| {
                    !(self.tabs[i].pinned && compact_pins) && active_idx != Some(i)
                })
                .map(|i| (session_widths[i] - TAB_MIN_WIDTH).max(0.0))
                .sum();
            if shrinkable > 0.0 {
                let ratio = ((shrinkable - overflow) / shrinkable).clamp(0.0, 1.0);
                for (i, w) in session_widths.iter_mut().enumerate().take(n_tabs) {
                    if (self.tabs[i].pinned && compact_pins) || active_idx == Some(i) {
                        continue;
                    }
                    *w = TAB_MIN_WIDTH + (*w - TAB_MIN_WIDTH) * ratio;
                }
            }
        }
        // Uniform width used while a tab is mid-drag, so the strip
        // geometry stays stable as the dragged slot slides (the
        // active/inactive width difference otherwise bounces the seam).
        // Sized to the widest inactive content so no label clips.
        let drag_uniform_w = max_inactive_content.clamp(TAB_MIN_WIDTH, TAB_NATURAL_WIDTH);
        // True overflow: even at TAB_MIN_WIDTH (compact pins at CHIP_W)
        // the tabs don't fit, so the scrollable actually scrolls. This
        // is the trigger that docks the "+" at the strip edge; the
        // softer `scroll_mode` above (tabs merely compressed below
        // natural) only brings in the `⋯` jump button. Without the
        // distinction the "+" jumped to the right cluster as soon as
        // three tabs compressed, long before anything scrolled.
        let pin_n = if compact_pins {
            self.tabs.iter().filter(|t| t.pinned).count()
        } else {
            0
        } as f32;
        let reg_n = n_f - pin_n;
        let min_total = pin_n * (CHIP_W + TAB_SPACING)
            + reg_n * TAB_MIN_WIDTH
            + (reg_n - 1.0).max(0.0) * TAB_SPACING;
        let strip_overflow = min_total > approx_strip_width;

        let mut tab_items: Vec<Element<'_, Message>> = Vec::new();
        // Active-tab fill style: gradient (default) or a flat accent tint.
        // Computed once and threaded into every tab/chip renderer so the
        // choice applies uniformly across session, SFTP and area tabs.
        // Performance mode forces the flat tint: the gradient is a
        // per-pixel shader in the software renderer, the flat tint a
        // single solid fill.
        let solid_fill = self.setting_tab_fill_style == "solid" || self.setting_performance_mode;

        // The navigation areas (Hosts and SFTP) live as top-level tabs
        // before the connection tabs. Settings stays out of the strip on
        // purpose - it lives in the burger menu so it doesn't take a
        // permanent slot.
        {
            let nav_active = self.active_tab.is_none();
            // The "Hosts" area tab stays selected across every vault
            // sub-section (Hosts, Keychain, Snippets, Port Forwarding,
            // History), not just the Hosts grid. Those sub-sections
            // live under the contextual sub-nav of this same area, so
            // mirror the `in_vault_area` family used in `layout.rs`.
            let in_vault_area = matches!(
                self.active_view,
                View::Dashboard
                    | View::Keys
                    | View::Snippets
                    | View::PortForwarding
                    | View::Cloud
                    | View::Proxies
                    | View::KnownHosts
                    | View::History
            );
            // Icon-only Home tab: the vault identity / switcher now
            // lives on the contextual sub-nav (the "Personal" chip),
            // so this tab is just the route back to the vault surface.
            tab_items.push(area_tab(
                "",
                iced_fonts::lucide::house(),
                View::Dashboard,
                nav_active && in_vault_area,
                solid_fill,
            ));
        }

        // Terminal and SFTP tabs share one strip, pinned-first across BOTH
        // kinds (so an unpinned SFTP tab never jumps ahead of a pinned
        // terminal). `false` = terminal index into `self.tabs`, `true` = SFTP
        // index into `self.sftp_tabs`. Within a pin partition, terminals come
        // before SFTP tabs (cross-type drag-interleave is a later refinement).
        let sftp_surface = self.active_tab.is_none() && self.active_view == View::Sftp;
        // While a drag is active every tab renders at the inactive width so
        // the strip geometry is uniform. The active-vs-inactive width
        // difference otherwise shifts positions on each live-slide swap and
        // bounces the dragged tab back and forth over a seam.
        let dragging_any = self.tab_drag.map(|d| d.active).unwrap_or(false);
        // Display order follows `tab_order` (the authoritative, drag-reorderable
        // unified order), partitioned pinned-first across both kinds. Each
        // `TabRef` maps to its current storage index. SFTP refs are skipped
        // when the SFTP feature is off.
        for (is_sftp, idx) in self.strip_order() {
            if is_sftp {
                let tab = &self.sftp_tabs[idx];
                let is_active = sftp_surface && self.active_sftp == Some(idx);
                // The mounted host (matched by the tab label = host name) drives
                // the badge icon + color, same as the terminal tabs.
                let detected_os = self.tab_detected_os(&tab.label);
                // Active-tab accent: the host's custom color if set, else the
                // OS brand color (so an Ubuntu tab "breathes" orange), else the
                // global accent for an empty (no-host) tab.
                let host_accent = self
                    .connections
                    .iter()
                    .find(|c| c.label == tab.label)
                    .and_then(|c| c.custom_color.as_deref().or(c.color.as_deref()))
                    .and_then(crate::widgets::parse_hex_color)
                    .or_else(|| {
                        detected_os.as_deref().map(|os| {
                            crate::os_icon::resolve_icon(Some(os), OryxisColors::t().accent).1
                        })
                    });
                // Width mirrors the terminal model: NATURAL when active,
                // content-hugged otherwise, uniform during a drag.
                let width = if dragging_any {
                    drag_uniform_w
                } else if is_active {
                    TAB_NATURAL_WIDTH
                } else {
                    tab_content_width(tab.display_label(), close_on_right, false)
                };
                // The dragged tab floats as a ghost (below); leave a same-width
                // gap here that the other tabs slide around, like terminal tabs.
                let is_dragging = self
                    .tab_drag
                    .filter(|d| d.active)
                    .map(|d| d.from_id == tab.id)
                    .unwrap_or(false);
                if is_dragging {
                    let gap_w = if compact_pins && tab.pinned { CHIP_W } else { width };
                    tab_items.push(Space::new().width(gap_w).height(TAB_HEIGHT).into());
                } else if compact_pins && tab.pinned {
                    tab_items.push(sftp_pinned_chip(idx, is_active, host_accent, solid_fill));
                } else {
                    tab_items.push(sftp_session_tab(idx, tab.display_label(), is_active, width, host_accent, tab.pinned, solid_fill));
                }
                continue;
            }
            let tab = &self.tabs[idx];
            let is_active = active_idx == Some(idx);
            let is_hovered = self.hovered_tab == Some(idx);
            // Reorder drag: the dragged tab gets the accent outline so the
            // user sees which one they picked up.
            let is_dragging = self
                .tab_drag
                .filter(|d| d.active)
                .map(|d| d.from_id == tab._id)
                .unwrap_or(false);
            // A split tab shows the focused pane's label + icon; a single
            // pane shows the tab's own label. Lookups (accent, OS badge)
            // key on the automatic label so a custom rename stays
            // display-only.
            let display_label = tab.display_label(self.tab_auto_title(tab));
            let base_label = tab
                .auto_label(self.tab_auto_title(tab))
                .trim_end_matches(" (disconnected)");
            let detected_os = self.tab_detected_os(base_label);
            // During a drag every tab is uniform (drag width); otherwise
            // each tab uses its own allocated width (active = NATURAL,
            // inactive = content-hugged, possibly shrunk under overflow).
            let width = if dragging_any {
                drag_uniform_w
            } else {
                session_widths[idx]
            };
            // Per-host accent override: when this tab points at a
            // saved connection that has a custom `color`, tint the
            // active-tab fill and the tab text with that color
            // (JetBrains-style "respiração"). Otherwise the active
            // tab keeps the global accent.
            let host_accent: Option<Color> = self.connections.iter()
                .find(|c| c.label == base_label)
                // `custom_color` is what the icon picker writes (the
                // user-chosen accent). The legacy `color` field is a
                // dead column today but stays as a fallback so any
                // future code path that fills it still works.
                .and_then(|c| c.custom_color.as_deref().or(c.color.as_deref()))
                .and_then(crate::widgets::parse_hex_color)
                // Auto mode (no custom color): fall back to the detected
                // OS brand color so an Ubuntu tab "breathes" orange,
                // matching the OS badge glyph and the dashboard card.
                // Mirrors the SFTP tab above.
                .or_else(|| {
                    detected_os.as_deref().map(|os| {
                        crate::os_icon::resolve_icon(Some(os), OryxisColors::t().accent).1
                    })
                })
                // Cloud-transport tabs (`ECS · ...`, `SSM · ...`,
                // `K8s · ...`) don't match any saved Connection by
                // label, so the per-host color lookup above returns
                // None and the active-tab gradient falls back to the
                // global accent. Derive a brand-coloured accent from
                // the tab label prefix instead so the tab "breathes"
                // the parent dynamic-group color (AWS orange / K8s
                // blue / etc.) the same way a per-host accent does.
                .or_else(|| {
                    crate::os_icon::tab_label_cloud_brand(base_label).map(|brand| {
                        crate::os_icon::provider_icon(brand, OryxisColors::t().accent).1
                    })
                });
            // Tabs always render the badge as a rounded square,
            // independent of the per-host override and the global
            // `default_host_icon` setting. Circular badges read as
            // pills inside the narrow tab strip and the variable
            // shape disrupts the row's vertical rhythm; locking the
            // tab shape keeps the strip uniform while leaving the
            // dashboard card free to honour the user's choice.
            let host_icon_style = crate::widgets::HostIconStyle::Rounded;
            // Connection-state dot color. Connecting beats every other
            // signal because a tab that's currently dialing isn't yet
            // "disconnected" in the user's mental model. Local-shell
            // tabs (no SSH session, not labeled disconnected) get no
            // dot, the OS badge already says what they are.
            let status_dot: Option<Color> = if self.setting_show_tab_status_dot {
                let is_connecting = self.connecting.as_ref().map(|cp| cp.tab_idx) == Some(idx);
                let is_disconnected = tab.label.ends_with(" (disconnected)");
                let is_remote = tab.active().session.is_some();
                if is_connecting {
                    Some(OryxisColors::t().warning)
                } else if is_disconnected {
                    Some(OryxisColors::t().error)
                } else if is_remote {
                    Some(OryxisColors::t().success)
                } else {
                    None
                }
            } else {
                None
            };
            // Smart-tabs attention dot (top-right corner of the badge):
            // the highest-priority cause across the tab's panes. Viewing
            // the tab clears the state, so an active watched tab never
            // carries one.
            let attention_dot: Option<Color> = if self.setting_smart_tabs {
                tab.pane_grid
                    .panes
                    .values()
                    .filter_map(|p| p.attention)
                    .max()
                    .map(|a| match a {
                        crate::smart_tabs::TabAttention::Activity => {
                            OryxisColors::t().accent
                        }
                        crate::smart_tabs::TabAttention::FinishedOk => {
                            OryxisColors::t().success
                        }
                        crate::smart_tabs::TabAttention::FinishedFail => {
                            OryxisColors::t().error
                        }
                    })
            } else {
                None
            };
            // Session-group tabs carry the group's own icon + color.
            let session_group = tab
                .session_group_id
                .and_then(|id| self.session_groups.iter().find(|g| g.id == id));
            let sg_custom_color = session_group
                .and_then(|g| g.color.as_deref())
                .and_then(crate::widgets::parse_hex_color);
            let sg_custom_icon = session_group
                .and_then(|g| g.icon_style.as_deref())
                .filter(|s| !s.is_empty());
            // Local-terminal appearance override: a curated entry (matched
            // by label) can carry an explicit icon / color chosen in the
            // Settings card, which wins over the OS hint so the tab chip
            // reflects what the user picked. Session-group icon/color still
            // take precedence (a grouped tab is the group's identity).
            let is_local_pane =
                matches!(tab.active().origin, crate::state::PaneOrigin::Local(_));
            let lt_entry = if is_local_pane {
                self.local_terminals
                    .as_deref()
                    .and_then(|list| list.iter().find(|e| e.label == base_label))
            } else {
                None
            };
            let lt_icon = lt_entry.and_then(|e| e.icon.as_deref());
            let lt_color = lt_entry
                .and_then(|e| e.color.as_deref())
                .and_then(crate::widgets::parse_hex_color);
            let tab_icon = sg_custom_icon.or(lt_icon);
            let tab_badge_color = sg_custom_color.or(lt_color);
            let tab_accent = sg_custom_color.or(lt_color).or(host_accent);
            if is_dragging {
                // The dragged tab floats as a ghost following the cursor
                // (built below); leave a same-width gap here that the other
                // tabs slide around as the reorder happens.
                let gap_w = if tab.pinned && compact_pins { CHIP_W } else { width };
                tab_items.push(
                    Space::new()
                        .width(gap_w)
                        .height(TAB_HEIGHT)
                        .into(),
                );
            } else if tab.pinned && compact_pins {
                // Chrome-style: icon-only chip, fixed width, stuck left.
                tab_items.push(pinned_tab_chip(
                    idx,
                    detected_os.as_deref(),
                    is_active,
                    tab_accent,
                    host_icon_style,
                    tab_icon,
                    tab_badge_color,
                    status_dot,
                    attention_dot,
                    solid_fill,
                ));
            } else {
                // Hybrid mode glyph (issue #61): SSH tabs carry a small
                // state chip (>_ terminal / folder files) that toggles the
                // tab between its terminal and its host's files. Hidden
                // for local / Telnet / serial panes; kept while in Files
                // mode even if the session drops, so the way back to the
                // terminal never disappears.
                // An in-Files-mode tab always keeps the glyph (the way
                // back), even if SFTP gets toggled off or the session
                // drops; otherwise the affordance requires the feature
                // on and a live SSH session.
                let files_mode = (tab.files_mode
                    || (self.sftp_enabled
                        && tab
                            .active()
                            .session
                            .as_ref()
                            .and_then(|s| s.ssh())
                            .is_some()))
                .then_some(tab.files_mode);
                tab_items.push(session_tab(
                    idx,
                    display_label,
                    tab.pane_count(),
                    is_active,
                    is_hovered,
                    detected_os.as_deref(),
                    width,
                    self.setting_tab_close_button_side == "right",
                    status_dot,
                    attention_dot,
                    tab_accent,
                    host_icon_style,
                    tab_icon,
                    tab_badge_color,
                    tab.pinned,
                    solid_fill,
                    tab.active().progress,
                    files_mode,
                ));
            }
        }

        // "+" trails the last tab, browser-style (issue #38). Only when
        // the strip TRULY overflows (tabs at min width still don't fit,
        // so the scrollable scrolls) it docks at the strip's trailing
        // edge instead, just before the right cluster, so it can never
        // scroll out of reach with the tabs.
        // Wrapped in a MouseArea so entering the `+` during an active tab
        // drag drops the dragged tab at the end of its partition (the trailing
        // slot the live-slide can't reach). The handler no-ops when no drag is
        // in flight, so normal `+` clicks are unaffected.
        let plus_btn: Element<'_, Message> = MouseArea::new(crate::widgets::bounds_reporter(
            new_tab_btn(!strip_overflow),
            self.plus_btn_bounds.clone(),
        ))
        .on_enter(Message::TabDragToEnd)
        .into();
        let mut docked_plus: Option<Element<'_, Message>> = None;
        if strip_overflow {
            docked_plus = Some(plus_btn);
        } else {
            tab_items.push(plus_btn);
        }

        // The tab strip lives in an auto-width container, Length::Fill
        // so the row gives it whatever's left after the sidebar toggle
        // and right cluster claim their Shrink widths. The scrollable
        // inside is the safety net: tabs that don't fit at min width
        // overflow into a horizontal scroll (mouse-wheel works, the
        // scrollbar itself is zeroed out so it's invisible). The
        // surrounding MouseArea makes the empty area of the strip a
        // window-drag handle, since we no longer have a separate drag
        // sibling in the row.
        let tab_strip_inner = scrollable(
            row(tab_items)
                .spacing(TAB_SPACING)
                .align_y(iced::Alignment::Center),
        )
        .id(iced::widget::Id::new("tab-scroll"))
        .direction(scrollable::Direction::Horizontal(
            scrollable::Scrollbar::new()
                .width(0.0)
                .scroller_width(0.0),
        ))
        .width(Length::Fill)
        .height(Length::Fixed(BAR_HEIGHT));

        // Top mode: no leading padding (the burger already carries its own
        // right padding, an extra strip margin just read as a gap before
        // the first tab). Bottom mode: nothing claims the leading edge, so
        // the strip gets its own small gutter before the Home tab, and one
        // extra px on top so the tabs don't hug the accent hairline that
        // now sits directly above them.
        let strip_padding = if bottom {
            // The gutter hugs the leading edge, which flips under RTL
            // (dir_row renders the Home tab on the right there).
            if crate::i18n::is_rtl_layout() {
                Padding { top: 5.0, right: 8.0, bottom: 4.0, left: 0.0 }
            } else {
                Padding { top: 5.0, right: 0.0, bottom: 4.0, left: 8.0 }
            }
        } else {
            Padding { top: 4.0, right: 0.0, bottom: 4.0, left: 0.0 }
        };
        let tab_strip: Element<'_, Message> = MouseArea::new(
            container(tab_strip_inner)
                .width(Length::Fill)
                .padding(strip_padding),
        )
        .on_press(Message::WindowDrag)
        // Native title-bar convention: double-click the drag area to
        // toggle maximize.
        .on_double_click(Message::WindowMaximizeToggle)
        // Vertical wheel translates to horizontal scroll on the tab
        // strip. The horizontal scrollable inside doesn't capture a
        // pure-y wheel event (iced only steers wheel along the
        // direction the scrollable can actually scroll), so this
        // MouseArea picks it up and routes a scroll_by command.
        .on_scroll(|delta| {
            let y = match delta {
                iced::mouse::ScrollDelta::Lines { y, .. } => y * 60.0,
                iced::mouse::ScrollDelta::Pixels { y, .. } => y,
            };
            Message::TabBarWheel(y)
        })
        .into();

        // `⋯` jump-to button, shown only when the strip is compressed
        // (scroll mode). In the combined bar it heads the right cluster;
        // in the bottom-docked strip it trails the tabs directly.
        let dots_btn: Option<Element<'_, Message>> =
            if scroll_mode { Some(tab_jump_btn()) } else { None };

        // Row composition. Combined top bar: [burger] [tab_strip(Fill)]
        // [docked +?] [right cluster: ⋯? / side-panel / chrome]. Bottom-
        // docked strip: [tab_strip(Fill)] [docked +?] [⋯?], the burger and
        // chrome live in the slim top bar. Burger / cluster are
        // Length::Shrink so iced gives them their content width first;
        // tab_strip is the remaining Fill area in between. `dir_row` flips
        // the row under RTL so the leading-edge controls always sit next
        // to the sidebar (which the outer layout also flips).
        let mut leading: Vec<Element<'_, Message>> = Vec::new();
        if !bottom {
            // Burger menu on the far leading edge: its dropdown lists every
            // vault destination + global actions. Leading breathing space is
            // the burger button's own left padding (not a margin), so the gap
            // is part of its clickable / hover area.
            leading.push(burger_menu_btn(self.show_burger_menu));
            // 1 px breather between the burger and the first area tab (home).
            leading.push(Space::new().width(1).height(TAB_HEIGHT).into());
        }
        leading.push(tab_strip);
        if let Some(plus) = docked_plus {
            leading.push(plus);
        }
        if bottom {
            if let Some(dots) = dots_btn {
                leading.push(dots);
            }
        } else {
            // The right cluster sits on the trailing edge of the tab bar.
            // Build it in reading order ([extras] then chrome) and let
            // `dir_row` flip the order in RTL so chrome lands on the
            // outer edge there too.
            let mut cluster_items: Vec<Element<'_, Message>> = Vec::new();
            if let Some(dots) = dots_btn {
                cluster_items.push(dots);
                cluster_items.push(Space::new().width(2).into());
            }
            // The side-panel toggle (Chat / Snippets / History) only makes
            // sense inside a connection tab, so skip it on the navigation
            // views where there's no terminal session to attach a panel to.
            if self.active_tab.is_some() {
                cluster_items.push(sidebar_btn());
                cluster_items.push(Space::new().width(2).into());
            }
            cluster_items.push(self.window_chrome_row().into());
            let right_cluster: Element<'_, Message> = crate::widgets::dir_row(cluster_items)
                .align_y(iced::Alignment::Center)
                .into();
            leading.push(right_cluster);
        }

        // Whole-bar accent wash: a tinted leading edge fading back to
        // the bar surface, same direction as the card accent wash + the
        // bottom hairline. Gated on `setting_tab_accent_wash`, and
        // breathes the active tab's colour via `top_accent_tint`. Both
        // gradient stops are opaque, so the tab buttons render normally.
        let bar_bg = self.tab_bar_background();
        let bar: Element<'_, Message> = container(
            crate::widgets::dir_row(leading)
                .align_y(iced::Alignment::Center),
        )
        .width(Length::Fill)
        .height(Length::Fixed(BAR_HEIGHT))
        .style(move |_| container::Style {
            background: Some(bar_bg),
            ..Default::default()
        })
        .into();

        // Floating ghost of the tab being dragged. The bar spans the window's
        // top-left, so window-space cursor x maps directly to bar-local x. The
        // ghost is a plain (non-interactive) container, so the tab MouseAreas
        // underneath still receive the hover events that drive the live-slide.
        let drag_ghost_el: Option<(Element<'_, Message>, f32)> = if dragging_any
            && let Some(drag) = self.tab_drag
        {
            if let Some(tab) = self.tabs.iter().find(|t| t._id == drag.from_id) {
                let base_label = tab
                    .display_label(self.tab_auto_title(tab))
                    .trim_end_matches(" (disconnected)")
                    .to_string();
                let lookup_label = tab
                    .auto_label(self.tab_auto_title(tab))
                    .trim_end_matches(" (disconnected)")
                    .to_string();
                let detected_os = self.tab_detected_os(&lookup_label);
                let compact = tab.pinned && compact_pins;
                let session_group = tab
                    .session_group_id
                    .and_then(|id| self.session_groups.iter().find(|g| g.id == id));
                let sg_color = session_group
                    .and_then(|g| g.color.as_deref())
                    .and_then(crate::widgets::parse_hex_color);
                let sg_icon = session_group
                    .and_then(|g| g.icon_style.as_deref())
                    .filter(|s| !s.is_empty())
                    .map(|s| s.to_string());
                let accent = sg_color.unwrap_or_else(|| OryxisColors::t().accent);
                let ghost_w = if compact { CHIP_W } else { drag_uniform_w };
                Some((
                    drag_ghost(base_label, detected_os, compact, ghost_w, accent, sg_icon, sg_color),
                    ghost_w,
                ))
            } else if let Some(sftp_tab) = self.sftp_tabs.iter().find(|t| t.id == drag.from_id) {
                let detected_os = self.tab_detected_os(&sftp_tab.label);
                let accent = self
                    .connections
                    .iter()
                    .find(|c| c.label == sftp_tab.label)
                    .and_then(|c| c.custom_color.as_deref().or(c.color.as_deref()))
                    .and_then(crate::widgets::parse_hex_color)
                    .or_else(|| {
                        detected_os.as_deref().map(|os| {
                            crate::os_icon::resolve_icon(Some(os), OryxisColors::t().accent).1
                        })
                    })
                    .unwrap_or_else(|| OryxisColors::t().accent);
                let compact = sftp_tab.pinned && compact_pins;
                let ghost_w = if compact { CHIP_W } else { drag_uniform_w };
                Some((sftp_drag_ghost(sftp_tab.display_label().to_string(), compact, ghost_w, accent), ghost_w))
            } else {
                None
            }
        } else {
            None
        };
        if let Some((ghost, ghost_w)) = drag_ghost_el {
            let gx = (self.mouse_position.x - ghost_w / 2.0).max(0.0);
            let positioned: Element<'_, Message> = iced::widget::Column::new()
                .push(Space::new().height(7.0))
                .push(
                    iced::widget::Row::new()
                        .push(Space::new().width(gx))
                        .push(ghost),
                )
                .into();
            return iced::widget::Stack::new()
                .push(bar)
                .push(positioned)
                .width(Length::Fill)
                .height(Length::Fixed(BAR_HEIGHT))
                .into();
        }
        bar
    }
}

impl Oryxis {
    /// Build a task that snaps the tab strip's scrollable so the active
    /// tab is roughly centered in the visible area. Called whenever a
    /// new tab gets focused (manual select, opening a local shell,
    /// connecting an SSH session, etc.), without this the new tab
    /// can land off-screen when the strip is in scroll mode.
    /// Resolve the OS / brand icon hint for a tab from its (de-suffixed)
    /// label: a saved connection's detected OS, else a local-shell hint, else
    /// the cloud brand parsed from an `ECS · ...` / `K8s · ...` prefix.
    /// Effective auto-title (OSC 0/2) decision for a tab: the focused host's
    /// per-host `Connection.auto_title` override wins over the global
    /// `terminal_auto_title` setting; local shells and hosts with no override
    /// fall back to the global. Resolved live so editing a host updates its
    /// open tabs without a reconnect.
    pub(crate) fn tab_auto_title(&self, tab: &crate::state::TerminalTab) -> bool {
        if let crate::state::PaneOrigin::Host(id) = &tab.active().origin
            && let Some(conn) = self.connections.iter().find(|c| c.id == *id)
            && let Some(over) = conn.auto_title
        {
            return over;
        }
        crate::state::auto_title_enabled()
    }

    pub(crate) fn tab_detected_os(&self, base_label: &str) -> Option<String> {
        // Saved hosts first, then quick-connect entries (their detection
        // result lives in memory only), then the local/cloud hints.
        self.any_connection_by_label(base_label)
            .and_then(|c| c.detected_os.clone())
            .or_else(|| crate::os_icon::local_shell_os_hint(base_label))
            .or_else(|| {
                crate::os_icon::tab_label_cloud_brand(base_label).map(|s| s.to_string())
            })
    }

    /// Display order of the unified tab strip: pinned-first over the
    /// drag-reorderable `tab_order`, across both terminal and SFTP kinds.
    /// Each entry is `(is_sftp, storage_index)`. SFTP refs are dropped when
    /// the SFTP feature is off. Shared by `view_tab_bar` (rendering) and
    /// `tab_scroll_to_active` (offset math) so the two can't drift.
    pub(crate) fn strip_order(&self) -> Vec<(bool, usize)> {
        let pinned_of = |r: &crate::state::TabRef| -> bool {
            match r {
                crate::state::TabRef::Terminal(id) => {
                    self.tabs.iter().find(|t| t._id == *id).map(|t| t.pinned).unwrap_or(false)
                }
                crate::state::TabRef::Sftp(id) => {
                    self.sftp_tabs.iter().find(|t| t.id == *id).map(|t| t.pinned).unwrap_or(false)
                }
            }
        };
        let to_entry = |r: &crate::state::TabRef| -> Option<(bool, usize)> {
            match r {
                crate::state::TabRef::Terminal(id) => {
                    self.tabs.iter().position(|t| t._id == *id).map(|i| (false, i))
                }
                crate::state::TabRef::Sftp(id) => {
                    if !self.sftp_enabled {
                        return None;
                    }
                    self.sftp_tabs.iter().position(|t| t.id == *id).map(|i| (true, i))
                }
            }
        };
        let mut order: Vec<(bool, usize)> = Vec::new();
        order.extend(self.tab_order.iter().filter(|r| pinned_of(r)).filter_map(to_entry));
        order.extend(self.tab_order.iter().filter(|r| !pinned_of(r)).filter_map(to_entry));
        order
    }

    pub(crate) fn tab_scroll_to_active(&self) -> iced::Task<Message> {
        let Some(active_idx) = self.active_tab else {
            return iced::Task::none();
        };
        if self.tabs.is_empty() {
            return iced::Task::none();
        }
        // Mirror the layout math in view_tab_bar so the offsets line up,
        // including the burger button + area tabs that Workspace mode
        // prepends to the strip.
        const RIGHT_CLUSTER_WIDTH: f32 = SIDEBAR_BUTTON_WIDTH
            + 2.0
            + PLUS_BUTTON_WIDTH
            + 2.0
            + DOTS_BUTTON_WIDTH
            + 2.0
            + CHROME_TOTAL_WIDTH;
        let area_tab_count = 1 + (self.sftp_enabled as u32);
        const AREA_TAB_APPROX_WIDTH: f32 = 100.0;
        let area_tabs_total = area_tab_count as f32
            * (AREA_TAB_APPROX_WIDTH + TAB_SPACING);
        let burger_width = SIDEBAR_TOGGLE_WIDTH;
        // Logo removed from the top strip; no width reserved for it.
        let logo_width = 0.0;
        // Same per-position reserve as `tab_strip_bar`: the bottom-docked
        // strip has no burger / chrome sharing its row.
        let reserved = if tab_bar_bottom() {
            PLUS_BUTTON_WIDTH + 2.0 + DOTS_BUTTON_WIDTH
        } else {
            burger_width + logo_width + RIGHT_CLUSTER_WIDTH
        };
        let approx_strip_width = (self.window_size.width
            - reserved
            - area_tabs_total
            - 12.0)
            .max(120.0);
        let (active_w, inactive_w) =
            allocate_tab_widths(self.tabs.len(), approx_strip_width);
        // Sum widths of all tabs that come before the active one, plus
        // the spacing between them. The strip renders pinned-first over
        // the drag-reorderable tab_order, not in storage order, so use
        // the active tab's actual display position (else a reorder or pin
        // would center the wrong tab). Width is still approximated
        // uniformly here, as before; only the ordering is corrected.
        let preceding = self
            .strip_order()
            .iter()
            .position(|&(is_sftp, i)| !is_sftp && i == active_idx)
            .unwrap_or(active_idx) as f32;
        let mut x = preceding * (inactive_w + TAB_SPACING);
        // Center active in viewport instead of left-aligning so the
        // user has context (the previous + next tabs visible too).
        x = (x - approx_strip_width / 2.0 + active_w / 2.0).max(0.0);
        iced::widget::operation::scroll_to(
            iced::widget::Id::new("tab-scroll"),
            iced::widget::scrollable::AbsoluteOffset { x, y: 0.0 },
        )
    }
}

// Tab-bar helper fns split into themed sibling files.
mod buttons;
mod ghosts;
mod sizing;
mod tabs;

pub(crate) use buttons::*;
pub(crate) use ghosts::*;
pub(crate) use sizing::*;
pub(crate) use tabs::*;
