//! Settings -> Shortcuts section view. Split out of views/settings/mod.rs.

use super::*;
use iced::widget::column;

impl Oryxis {
    pub(crate) fn view_settings_shortcuts(&self) -> Element<'_, Message> {
        use crate::hotkeys::{default_bindings, HotkeyAction};
        // Keyboard rows are recorded in visual order.
        self.keynav_settings_reset();
        let defaults = default_bindings();

        // Header: title + hint + global reset button.
        let header = column![
                                text(crate::i18n::t("hotkey_edit_hint"))
                .size(11)
                .color(OryxisColors::t().text_muted),
            Space::new().height(10),
            self.settings_nav_slot_labeled(
                crate::i18n::t("hotkey_reset_all"),
                crate::keynav::RowAction::activate(Message::Settings(SettingsMessage::ResetAllHotkeys)),
                6.0,
                styled_button(
                    crate::i18n::t("hotkey_reset_all"),
                    Message::Settings(SettingsMessage::ResetAllHotkeys),
                    OryxisColors::t().bg_selected,
                ),
            ),
            Space::new().height(16),
        ]
        .width(Length::Fill)
        .align_x(dir_align_x());

        let mut rows_col = column![header]
            .spacing(8)
            .width(Length::Fill)
            .align_x(dir_align_x());

        for action in HotkeyAction::all() {
            // The row is not one nav slot: it records a slot per chord
            // chip, plus the add chip and the reset button. Enter on a
            // chip starts a capture for THAT chord.
            rows_col = rows_col.push(self.hotkey_editor_row(*action, defaults.get(action)));
        }

        // Read-only footer for the one terminal gesture that isn't a
        // chord and so can't live in the table above: Ctrl+Wheel zoom
        // is handled in the scroll event. Terminal copy / paste /
        // select-all used to sit here too, as read-only rows, back when
        // they were hard-coded in the widget and the dispatcher; they
        // are ordinary editable actions now (#75).
        let static_rows = column![
            Space::new().height(20),
            text(crate::i18n::t("hotkey_terminal_handled"))
                .size(11)
                .color(OryxisColors::t().text_muted),
            Space::new().height(8),
            shortcut_row(
                vec![key_badge("Ctrl"), key_badge("Wheel")],
                crate::i18n::t("font_zoom_wheel"),
            ),
        ]
        .spacing(8)
        .width(Length::Fill)
        .align_x(dir_align_x());
        rows_col = rows_col.push(static_rows);

        scrollable(
            container(rows_col)
                .padding(Padding { top: 24.0, right: 24.0, bottom: 24.0, left: 24.0 }),
        )
        // Stable id so the keyboard router can keep the selected row
        // in view.
        .id(iced::widget::Id::new("settings-shortcuts-scroll"))
        .height(Length::Fill)
        .into()
    }

    /// One chord chip in the Shortcuts editor: the badge cluster on a
    /// clickable surface that starts a capture for `slot`. `chord` is
    /// `None` for the trailing add button.
    ///
    /// Records its own keynav slot, so callers must build chips in
    /// display order (build order is record order).
    fn hotkey_chip(
        &self,
        action: crate::hotkeys::HotkeyAction,
        slot: crate::hotkeys::HotkeySlot,
        chord: Option<crate::hotkeys::HotkeyBinding>,
        recording: bool,
        empty_row: bool,
    ) -> Element<'_, Message> {
        let idx = self.settings_nav_record(crate::keynav::RowAction::activate(
            Message::Settings(SettingsMessage::StartEditingHotkey(action, slot)),
        ));
        let inner: Element<'_, Message> = if recording {
            // Capture state: paint with the high-contrast `button_text`
            // foreground, the readable pairing for the `button_bg`
            // surface this button already uses. Painting accent-on-bg
            // here washed the placeholder out against the dark button.
            text(crate::i18n::t("hotkey_press_a_key"))
                .size(12)
                .color(OryxisColors::t().button_text)
                .into()
        } else if let Some(b) = chord {
            // For family actions the suffix badge is rendered with a
            // distinct muted style so the user sees at a glance which
            // slot is fixed.
            let labels = b.badges();
            let n = labels.len();
            let primary_editable = action.primary_editable();
            let badges: Vec<Element<'_, Message>> = labels
                .into_iter()
                .enumerate()
                .map(|(i, lbl)| {
                    let is_suffix = i == n - 1;
                    if is_suffix && !primary_editable {
                        // Fixed-suffix badge: same solid pill as the
                        // modifiers so it stays legible, but with a
                        // dashed-feel via a tinted border + muted
                        // text. The earlier alpha-40 background
                        // washed out completely against the dark
                        // button surface; this keeps the visual
                        // distinction without losing contrast.
                        container(
                            text(lbl)
                                .size(11)
                                .color(OryxisColors::t().text_secondary),
                        )
                        .padding(Padding {
                            top: 3.0,
                            right: 6.0,
                            bottom: 3.0,
                            left: 6.0,
                        })
                        .style(|_| container::Style {
                            background: Some(Background::Color(OryxisColors::t().bg_selected)),
                            border: Border {
                                radius: Radius::from(4.0),
                                color: OryxisColors::t().border,
                                width: 1.0,
                            },
                            ..Default::default()
                        })
                        .into()
                    } else {
                        key_badge_owned(lbl)
                    }
                })
                .collect();
            iced::widget::Row::with_children(badges)
                .spacing(4)
                .align_y(iced::Alignment::Center)
                .into()
        } else if empty_row {
            // Nothing bound at all, gestures included (`empty_row` is
            // false when a live mouse gesture badge precedes this chip):
            // the add chip carries the unbound placeholder, so the row
            // still reads as one affordance rather than a bare "+" next
            // to nothing.
            text(crate::i18n::t("hotkey_unbound"))
                .size(11)
                .color(OryxisColors::t().text_muted)
                .into()
        } else {
            text("+")
                .size(13)
                .color(OryxisColors::t().text_muted)
                .into()
        };

        let btn = button(inner)
            .on_press(Message::Settings(SettingsMessage::StartEditingHotkey(action, slot)))
            .style(move |_, status| {
                let bg = match status {
                    BtnStatus::Hovered => OryxisColors::t().button_bg_hover,
                    _ => OryxisColors::t().button_bg,
                };
                // The chip being recorded gets an accent border so it
                // reads "pending input" against its siblings.
                let border_color = if recording {
                    OryxisColors::t().accent
                } else {
                    OryxisColors::t().border
                };
                button::Style {
                    background: Some(Background::Color(bg)),
                    border: Border {
                        radius: Radius::from(6.0),
                        color: border_color,
                        width: 1.0,
                    },
                    ..Default::default()
                }
            });
        self.settings_nav_ring_at(idx, 6.0, btn.into())
    }

    /// Name of the built-in MOUSE gesture that performs `action`, when it
    /// has one AND that gesture is currently enabled. Lets a chord-less
    /// row say what does drive the action instead of "(unbound)", which
    /// reads as a broken feature.
    ///
    /// Gated on the live setting on purpose: a user who turned the
    /// gesture off really has nothing bound, and the row should say so
    /// rather than point at a disabled affordance.
    fn action_live_gesture(&self, action: crate::hotkeys::HotkeyAction) -> Option<&'static str> {
        match action {
            // X11 PRIMARY paste. Middle-click is the convention's native
            // gesture, which is why this action ships without a chord.
            crate::hotkeys::HotkeyAction::TerminalPasteSelection
                if self.setting_middle_click_paste =>
            {
                Some(crate::i18n::t("gesture_middle_click"))
            }
            _ => None,
        }
    }

    /// Single row in the Shortcuts editor list. Renders one chip per
    /// bound chord (click to re-record it, Delete while recording to
    /// drop it), a trailing add chip, and a reset button only when the
    /// chords differ from the factory ones, so the user can spot
    /// overrides at a glance.
    ///
    /// Actions carry a LIST of chords, not one: `Ctrl+Shift+V` and
    /// `Shift+Insert` are both factory paste chords. Each chord gets
    /// its own bordered chip precisely so two chords never read as one
    /// long run of badges.
    pub(crate) fn hotkey_editor_row(
        &self,
        action: crate::hotkeys::HotkeyAction,
        default: Option<&crate::hotkeys::HotkeyBindings>,
    ) -> Element<'_, Message> {
        use crate::hotkeys::{HotkeyBindings, HotkeySlot};
        let fallback = HotkeyBindings::default();
        let binds = self.hotkey_bindings.get(&action).unwrap_or(&fallback);
        let is_overridden = default.is_some_and(|d| d != binds);
        let editing = self
            .editing_hotkey
            .filter(|(a, _)| *a == action)
            .map(|(_, s)| s);

        let mut chips: Vec<Element<'_, Message>> = Vec::with_capacity(binds.len() + 2);
        // A built-in MOUSE gesture renders first, as a key badge like the
        // chord pills, so it reads at the same contrast on every theme.
        // Not a chip: it is not recordable and not resettable, the
        // gesture's own setting governs it, so it takes no click.
        if let Some(gesture) = self.action_live_gesture(action) {
            chips.push(key_badge_owned(gesture.to_string()));
        }
        chips.extend(binds.iter().enumerate().map(|(i, chord)| {
            let slot = HotkeySlot::Replace(i);
            self.hotkey_chip(action, slot, Some(*chord), editing == Some(slot), false)
        }));
        chips.push(self.hotkey_chip(
            action,
            HotkeySlot::Add,
            None,
            editing == Some(HotkeySlot::Add),
            binds.is_empty() && self.action_live_gesture(action).is_none(),
        ));

        let pills_box = container(
            iced::widget::Row::with_children(chips)
                .spacing(6)
                .align_y(iced::Alignment::Center),
        )
        .width(260)
        .align_x(dir_align_x());

        let label = text(crate::i18n::t(action.label_key()))
            .size(13)
            .color(OryxisColors::t().text_secondary);

        // Recorded after the chips: build order is record order, and
        // reset sits at the trailing edge of the row.
        let reset_el: Element<'_, Message> = if is_overridden {
            let btn = button(
                text(crate::i18n::t("hotkey_reset"))
                    .size(11)
                    .color(OryxisColors::t().text_muted),
            )
            .on_press(Message::Settings(SettingsMessage::ResetHotkey(action)))
            .style(|_, status| {
                let bg = match status {
                    BtnStatus::Hovered => Some(Background::Color(OryxisColors::t().button_bg_hover)),
                    _ => None,
                };
                button::Style {
                    background: bg,
                    border: Border {
                        radius: Radius::from(4.0),
                        ..Default::default()
                    },
                    ..Default::default()
                }
            });
            self.settings_nav_slot(
                crate::keynav::RowAction::activate(Message::Settings(SettingsMessage::ResetHotkey(action))),
                4.0,
                btn.into(),
            )
        } else {
            Space::new().into()
        };

        dir_row(vec![
            pills_box.into(),
            label.into(),
            Space::new().width(Length::Fill).into(),
            reset_el,
        ])
        .align_y(iced::Alignment::Center)
        .into()
    }
}

/// Owned-label variant of `widgets::key_badge`. The editor builds
/// labels at runtime from `HotkeyBinding::badges()` so we can't reuse
/// the `&'a str` shape directly without leaking.
fn key_badge_owned(label: String) -> Element<'static, Message> {
    container(text(label).size(11).color(OryxisColors::t().text_primary))
        .padding(Padding {
            top: 3.0,
            right: 6.0,
            bottom: 3.0,
            left: 6.0,
        })
        .style(|_| container::Style {
            background: Some(Background::Color(OryxisColors::t().bg_selected)),
            border: Border {
                radius: Radius::from(4.0),
                ..Default::default()
            },
            ..Default::default()
        })
        .into()
}
