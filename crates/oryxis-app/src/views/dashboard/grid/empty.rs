//! Dashboard grid: empty state. Split out of views/dashboard/grid/mod.rs.

use super::*;
use iced::widget::column;

/// Width of the whole centered block (input, Continue, the "or"
/// divider and every secondary action), so the column reads as one
/// object instead of a stack of differently sized parts.
const BLOCK_WIDTH: f32 = 380.0;

impl Oryxis {
    /// Centered empty state shown when no hosts/groups/session groups exist.
    ///
    /// No toolbar here: with an empty vault there is nothing to search,
    /// sort, filter or re-layout, and the add menu's entries render as
    /// real buttons below instead (same catalog, `add_host_actions`).
    /// Matches the other empty vault views (see `view_history`).
    pub(crate) fn dashboard_empty_state(&self) -> Element<'_, Message> {
        // Termius-style empty state, centered "Create host" with input
        let has_input = !self.quick_host_input.is_empty();
        let btn_bg = if has_input { OryxisColors::t().success } else { OryxisColors::t().bg_surface };
        // An explicit connect target (user@, a port, an IP literal)
        // makes Enter / the button quick-connect directly instead of
        // opening the editor (issue #97, see `QuickHostContinue`), so
        // the button must say so instead of lying with "Continue".
        let connects_directly = oryxis_core::ssh_target::SshTarget::parse(
            self.quick_host_input.trim(),
        )
        .is_some_and(|t| t.is_explicit())
            && self.quick_connect_target(self.quick_host_input.trim()).is_some();

        // The toolbar's recording is from a previous frame (a host was
        // just deleted); the toolbar isn't on screen, so drop it and
        // blank the anchor cells its dropdowns would point at. The
        // content zone was cleared by the caller and is re-recorded
        // below, in display order.
        self.keynav_toolbar_reset();
        self.keynav_toolbar_zero_trigger_bounds();

        let mut items: Vec<Element<'_, Message>> = vec![
            // Icon
            container(iced_fonts::lucide::server().size(32).color(OryxisColors::t().text_muted))
                .padding(16)
                .style(|_| container::Style {
                    background: Some(Background::Color(OryxisColors::t().bg_surface)),
                    border: Border { radius: Radius::from(12.0), ..Default::default() },
                    ..Default::default()
                })
                .into(),
            Space::new().height(20).into(),
            text(crate::i18n::t("create_host_title"))
                .size(20)
                .color(OryxisColors::t().text_primary)
                .into(),
            Space::new().height(8).into(),
            text(crate::i18n::t("create_host_desc")).size(13).color(OryxisColors::t().text_muted).into(),
            Space::new().height(24).into(),
            // Hostname input. Enter on its keyboard row focuses it (the
            // id), typing then submits with the same Enter.
            self.content_action_slot(
                crate::keynav::RowAction::input(iced::widget::Id::new(QUICK_HOST_INPUT_ID)),
                8.0,
                text_input(t("type_ip_or_hostname"), &self.quick_host_input)
                    .id(QUICK_HOST_INPUT_ID)
                    .on_input(|v| Message::Navigation(NavigationMessage::QuickHostInput(v)))
                    .on_submit(Message::Navigation(NavigationMessage::QuickHostContinue))
                    .padding(14)
                    .width(BLOCK_WIDTH)
                    .style(crate::widgets::rounded_input_style)
                    .align_x(dir_align_x())
                    .into(),
            ),
            Space::new().height(12).into(),
            // Continue button
            self.content_action_slot(
                crate::keynav::RowAction::activate(Message::Navigation(NavigationMessage::QuickHostContinue)),
                8.0,
                button(
                    container(
                        text(crate::i18n::t(if connects_directly {
                            "connect"
                        } else {
                            "continue_btn"
                        }))
                        .size(14)
                        .color(OryxisColors::t().text_primary),
                    )
                    .padding(Padding { top: 12.0, right: 0.0, bottom: 12.0, left: 0.0 })
                    .width(BLOCK_WIDTH)
                    .center_x(BLOCK_WIDTH),
                )
                .on_press(Message::Navigation(NavigationMessage::QuickHostContinue))
                .width(BLOCK_WIDTH)
                .style(move |_, status| {
                    // Hover / press lift the fill a step in both states
                    // (idle surface and the input-filled success fill).
                    let bg = match status {
                        BtnStatus::Hovered | BtnStatus::Pressed if has_input => {
                            OryxisColors::t().accent_hover
                        }
                        BtnStatus::Hovered | BtnStatus::Pressed => OryxisColors::t().bg_hover,
                        _ => btn_bg,
                    };
                    button::Style {
                        background: Some(Background::Color(bg)),
                        border: Border { radius: Radius::from(8.0), ..Default::default() },
                        ..Default::default()
                    }
                })
                .into(),
            ),
        ];

        // Secondary paths: the "+ Host ▾" menu's own entries, as
        // buttons. A dropdown on an otherwise blank screen hides the
        // only other ways in (import, cloud discovery) behind a chevron
        // the first-run user has no reason to click.
        let actions = self.add_host_actions();
        if !actions.is_empty() {
            items.push(Space::new().height(24).into());
            items.push(or_divider());
            items.push(Space::new().height(16).into());
            for action in actions {
                items.push(self.content_action_slot(
                    crate::keynav::RowAction::activate(action.msg.clone()),
                    8.0,
                    secondary_action_button(action),
                ));
                items.push(Space::new().height(8).into());
            }
        }

        let empty_state = container(column(items).align_x(iced::Alignment::Center)).center(Length::Fill);

        column![empty_state].width(Length::Fill).height(Length::Fill).into()
    }
}

/// Text-input id of the quick-host field, shared by the widget and its
/// keyboard row (`RowAction::input` focuses by id).
const QUICK_HOST_INPUT_ID: &str = "empty-quick-host";

/// A hairline rule with the "or" label centered in it, separating the
/// primary create path from the secondary ones.
fn or_divider<'a>() -> Element<'a, Message> {
    let rule = || {
        container(Space::new().width(Length::Fill).height(1))
            .width(Length::Fill)
            .style(|_| container::Style {
                background: Some(Background::Color(OryxisColors::t().border)),
                ..Default::default()
            })
    };
    // Symmetric, so it needs no direction awareness.
    container(
        iced::widget::row![
            rule(),
            container(text(t("or_separator")).size(12).color(OryxisColors::t().text_muted))
                .padding(Padding { top: 0.0, right: 10.0, bottom: 0.0, left: 10.0 }),
            rule(),
        ]
        .align_y(iced::Alignment::Center),
    )
    .width(BLOCK_WIDTH)
    .into()
}

/// One secondary action: outlined, muted, deliberately quieter than
/// the success-filled Continue above it.
fn secondary_action_button(action: crate::views::add_actions::AddHostAction<'_>) -> Element<'_, Message> {
    let crate::views::add_actions::AddHostAction { icon, label, msg, color } = action;
    button(
        container(
            dir_row(vec![
                icon.view(14.0, color),
                Space::new().width(8).into(),
                text(label).size(13).color(OryxisColors::t().text_secondary).into(),
            ])
            .align_y(iced::Alignment::Center),
        )
        .width(BLOCK_WIDTH)
        .center_x(BLOCK_WIDTH),
    )
    .on_press(msg)
    .width(BLOCK_WIDTH)
    .padding(Padding { top: 10.0, right: 12.0, bottom: 10.0, left: 12.0 })
    .style(|_, status| {
        let bg = match status {
            BtnStatus::Hovered => OryxisColors::t().bg_hover,
            BtnStatus::Pressed => OryxisColors::t().bg_selected,
            _ => Color::TRANSPARENT,
        };
        button::Style {
            background: Some(Background::Color(bg)),
            border: Border {
                radius: Radius::from(8.0),
                width: 1.0,
                color: OryxisColors::t().border,
            },
            ..Default::default()
        }
    })
    .into()
}
