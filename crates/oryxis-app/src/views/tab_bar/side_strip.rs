//! Side-docked (left / right) vertical tab strip (issue #87).
//!
//! When `tab_bar_position` is `left` or `right` the tabs stack as a
//! vertical list on that window edge, between the slim top chrome bar
//! and the status bar. Every chip is rendered by the same
//! `strip_tab_element` the horizontal strips use, at one uniform row
//! width; compact pinned chips pack several per row (Edge-style pinned
//! grid). Left / right are physical edges by user choice, so RTL never
//! flips the strip; the chips' inner rows still mirror through
//! `dir_row` like everywhere else.

use super::*;

/// Total width of the side-docked strip, gutters included.
pub(crate) const SIDE_STRIP_WIDTH: f32 = 216.0;
/// Uniform row width of every tab chip inside the strip (the strip
/// minus its 8px side gutters).
pub(crate) const SIDE_TAB_WIDTH: f32 = SIDE_STRIP_WIDTH - 16.0;
/// Rendered height of one strip row: the chip's TAB_HEIGHT content box
/// plus the button's default 5px top/bottom paddings.
const SIDE_ROW_HEIGHT: f32 = TAB_HEIGHT + 10.0;
/// Window-space y where the strip's content starts: the slim top
/// chrome bar, its 1px separator and the strip's own top gutter.
/// `main_layout` must keep the side layout in sync with this so the
/// drag ghost tracks the cursor.
const SIDE_STRIP_TOP: f32 = BAR_HEIGHT + 1.0 + 6.0;

impl Oryxis {
    /// The vertical tab strip for the left / right docked layout:
    /// Home area tab, then the unified pinned-first tab order (compact
    /// pinned chips packed into rows), then `+`; `⋯` joins a docked
    /// footer once the list overflows the viewport.
    pub(crate) fn view_side_tab_strip(&self) -> Element<'_, Message> {
        let compact_pins = self.setting_pinned_tab_style == "compact";
        let solid_fill =
            self.setting_tab_fill_style == "solid" || self.setting_performance_mode;
        let dragging_any = self.tab_drag.map(|d| d.active).unwrap_or(false);
        let ctx = StripCtx {
            privacy_terms: self.privacy_terms(),
            close_on_right: self.setting_tab_close_button_side == "right",
            compact_pins,
            solid_fill,
            dragging_any,
            // Uniform rows: the drag width IS the row width, so the
            // live-slide never changes the strip geometry.
            drag_uniform_w: SIDE_TAB_WIDTH,
            uniform_w: Some(SIDE_TAB_WIDTH),
            session_widths: Vec::new(),
        };

        let mut items: Vec<Element<'_, Message>> = Vec::new();
        {
            // Icon-only Home tab, same square as the horizontal strip
            // (see `tab_strip_bar` for why Settings stays out).
            let nav_active = self.active_tab.is_none();
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
            items.push(area_tab(
                "",
                iced_fonts::lucide::house(),
                View::Dashboard,
                nav_active && in_vault_area,
                solid_fill,
            ));
        }

        // Consecutive compact pinned chips pack into rows of CHIP_W
        // chips; everything else stacks one chip per row. `strip_order`
        // is pinned-first, so with the compact style the chips form one
        // grid at the top of the list.
        let chips_per_row = ((SIDE_TAB_WIDTH + TAB_SPACING)
            / (CHIP_W + TAB_SPACING))
            .floor()
            .max(1.0) as usize;
        let mut chip_row: Vec<Element<'_, Message>> = Vec::new();
        let mut row_count = 1usize; // the Home tab above
        for (is_sftp, idx) in self.strip_order() {
            let el = self.strip_tab_element(&ctx, is_sftp, idx);
            if compact_pins && self.strip_entry_pinned(is_sftp, idx) {
                chip_row.push(el);
                if chip_row.len() == chips_per_row {
                    items.push(
                        row(std::mem::take(&mut chip_row))
                            .spacing(TAB_SPACING)
                            .into(),
                    );
                    row_count += 1;
                }
            } else {
                if !chip_row.is_empty() {
                    items.push(
                        row(std::mem::take(&mut chip_row))
                            .spacing(TAB_SPACING)
                            .into(),
                    );
                    row_count += 1;
                }
                items.push(el);
                row_count += 1;
            }
        }
        if !chip_row.is_empty() {
            items.push(row(chip_row).spacing(TAB_SPACING).into());
            row_count += 1;
        }

        // Overflow: the rows (plus the trailing `+`) don't fit the
        // strip's viewport, so the `+` docks into a fixed footer with
        // the `⋯` jump button and the list alone scrolls. Mirrors the
        // horizontal strip's docked-plus / scroll-mode pair; vertical
        // rows never compress, so one trigger covers both.
        let viewport_h = (self.window_size.height - SIDE_STRIP_TOP - 40.0).max(120.0);
        let content_h = (row_count as f32 + 1.0) * (SIDE_ROW_HEIGHT + TAB_SPACING);
        let overflow = content_h > viewport_h;

        // Same `+` affordance as the horizontal strip: bounds reported
        // for the split-menu anchor, drag-to-end drop target.
        let plus_btn: Element<'_, Message> = MouseArea::new(crate::widgets::bounds_reporter(
            new_tab_btn(!overflow),
            self.plus_btn_bounds.clone(),
        ))
        .on_enter(Message::Tabs(TabsMessage::TabDragToEnd))
        .into();
        let mut footer: Option<Element<'_, Message>> = None;
        if overflow {
            footer = Some(
                row(vec![plus_btn, Space::new().width(2).into(), tab_jump_btn()])
                    .align_y(iced::Alignment::Center)
                    .into(),
            );
        } else {
            items.push(plus_btn);
        }

        // Vertical scrollable, scrollbar zeroed out like the horizontal
        // strip (the wheel still scrolls it natively).
        let strip_scroll = scrollable(
            iced::widget::Column::with_children(items).spacing(TAB_SPACING),
        )
        .id(iced::widget::Id::new("tab-scroll"))
        .direction(scrollable::Direction::Vertical(
            scrollable::Scrollbar::new().width(0.0).scroller_width(0.0),
        ))
        .width(Length::Fill)
        .height(Length::Fill);

        let mut inner = iced::widget::Column::new().push(strip_scroll);
        if let Some(footer) = footer {
            inner = inner.push(footer);
        }

        // Strip surface: the accent wash runs top -> bottom here (the
        // horizontal bars wash along their leading edge), fading toward
        // the status bar; same gate and tint as `tab_bar_background`.
        let bar_base = OryxisColors::t().bg_sidebar;
        let bar_bg = if self.setting_tab_accent_wash {
            let washed = crate::theme::mix(bar_base, self.top_accent_tint(), 0.16);
            Background::Gradient(iced::Gradient::Linear(
                iced::gradient::Linear::new(iced::Radians(std::f32::consts::PI))
                    .add_stop(0.0, washed)
                    .add_stop(0.9, bar_base),
            ))
        } else {
            Background::Color(bar_base)
        };
        let bar: Element<'_, Message> = container(inner)
            .padding(Padding { top: 6.0, right: 8.0, bottom: 6.0, left: 8.0 })
            .width(Length::Fixed(SIDE_STRIP_WIDTH))
            .height(Length::Fill)
            .style(move |_| container::Style {
                background: Some(bar_bg),
                ..Default::default()
            })
            .into();

        // Floating drag ghost, tracking the cursor's y (the horizontal
        // bars track x). Non-interactive so the tab MouseAreas below
        // keep receiving the hover events that drive the live-slide.
        if let Some((ghost, _ghost_w)) =
            self.strip_drag_ghost_el(SIDE_TAB_WIDTH, compact_pins, &ctx.privacy_terms)
        {
            let gy = (self.mouse_position.y - SIDE_STRIP_TOP - SIDE_ROW_HEIGHT / 2.0)
                .max(0.0);
            let positioned: Element<'_, Message> = iced::widget::Column::new()
                .push(Space::new().height(gy))
                .push(
                    iced::widget::Row::new()
                        .push(Space::new().width(8.0))
                        .push(ghost),
                )
                .into();
            return iced::widget::Stack::new()
                .push(bar)
                .push(positioned)
                .width(Length::Fixed(SIDE_STRIP_WIDTH))
                .height(Length::Fill)
                .into();
        }
        bar
    }
}
