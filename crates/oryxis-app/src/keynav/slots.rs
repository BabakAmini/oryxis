//! Recording slots for the modal / settings / side-panel keyboard
//! layers (iteration 2 of the focus-zone framework).
//!
//! These surfaces fire arbitrary `Message`s per row and `Message` is
//! `Clone` but not `PartialEq`, so unlike the vault zones (semantic
//! `NavItem` ids) the selection here is INDEX-based: each surface
//! records its actionable rows in render order every frame, and the
//! routers clamp a stale index instead of chasing identity. A
//! `ModalSurface` tag on the selection makes a surface swap (menu
//! closes, another opens) drop the selection for free.

use std::cell::RefCell;

use crate::app::Message;

/// One keyboard-actionable row/button recorded during view().
///
/// `activate`: dispatched by Enter / Space. `prev` / `next`: fired by
/// Left / Right on picker rows (the on_select message the dropdown
/// would produce for the neighboring option). `focus`: Enter focuses
/// this text input instead of dispatching (row mode hands the
/// keyboard back to iced's real focus).
#[derive(Default, Clone)]
pub(crate) struct RowAction {
    pub(crate) activate: Option<Message>,
    pub(crate) prev: Option<Message>,
    pub(crate) next: Option<Message>,
    pub(crate) focus: Option<iced::widget::Id>,
}

impl RowAction {
    /// A plain button / toggle / menu row.
    pub(crate) fn activate(msg: Message) -> Self {
        Self { activate: Some(msg), ..Default::default() }
    }

    /// A pick_list row: Left/Right cycle, Enter is a consumed no-op.
    pub(crate) fn picker(prev: Option<Message>, next: Option<Message>) -> Self {
        Self { prev, next, ..Default::default() }
    }

    /// A text-input row: Enter focuses the input.
    pub(crate) fn input(id: iced::widget::Id) -> Self {
        Self { focus: Some(id), ..Default::default() }
    }
}

/// Identity of the surface a modal-layer selection belongs to. A
/// selection carrying a stale tag counts as no selection, so closing
/// one menu and opening another can never dispatch a row from the
/// previous surface; no cleanup hooks needed at the ~50 open/close
/// sites.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ModalSurface {
    Modal(crate::state::Modal),
    /// Anchored dropdown/kebab menus; the discriminant distinguishes
    /// menu kinds without requiring PartialEq on the payload.
    Overlay(std::mem::Discriminant<crate::state::OverlayContent>),
    Burger,
}

/// Selection + per-frame row recording for the modal layer.
#[derive(Default)]
pub(crate) struct ModalNavState {
    /// Explicitly selected row, tagged by its owner surface.
    pub(crate) selected: Option<(ModalSurface, usize)>,
    /// Actions recorded by the active modal/menu during view().
    pub(crate) items: RefCell<Vec<RowAction>>,
    /// Row the surface marked as its default (confirm dialogs mark
    /// the action button; menus mark their first row).
    pub(crate) default: std::cell::Cell<Option<usize>>,
}

/// prev/next messages for a picker row: the on_select message the
/// dropdown would fire for the neighboring option, wrapping at the
/// ends. Call sites know both the options and the current value, so
/// the pair is prepared at render time and stored in the RowAction.
pub(crate) fn cycle_pair<T: Clone + PartialEq>(
    options: &[T],
    current: &T,
    mk: impl Fn(T) -> Message,
) -> (Option<Message>, Option<Message>) {
    let n = options.len();
    let Some(i) = options.iter().position(|o| o == current) else {
        // Unknown current value: both arrows land on the first option
        // so the control becomes coherent instead of dead.
        let first = options.first().cloned().map(&mk);
        return (first.clone(), first);
    };
    if n < 2 {
        return (None, None);
    }
    (
        Some(mk(options[(i + n - 1) % n].clone())),
        Some(mk(options[(i + 1) % n].clone())),
    )
}

impl crate::app::Oryxis {
    /// Clear the modal-layer recording. Every navigable modal / menu
    /// view calls this first (only one such surface renders per
    /// frame, topmost-wins like `close_topmost_modal`).
    pub(crate) fn modal_nav_reset(&self) {
        self.keynav.modal.items.borrow_mut().clear();
        self.keynav.modal.default.set(None);
    }

    /// The row index the keyboard currently points at: the explicit
    /// selection when its surface tag matches and the index still
    /// exists (clamped), else the surface default.
    pub(crate) fn modal_nav_effective(&self, surface: ModalSurface) -> Option<usize> {
        use super::movement::clamp_index;
        let len = self.keynav.modal.items.borrow().len();
        match self.keynav.modal.selected {
            Some((tag, idx)) if tag == surface => clamp_index(idx, len),
            _ => self.keynav.modal.default.get().and_then(|d| clamp_index(d, len)),
        }
    }

    /// Record one actionable row and ring it when selected. `radius`
    /// matches the row's own corner radius; `contrast` picks the
    /// text_primary ring for accent/danger-filled buttons (an accent
    /// ring vanishes into them, same rationale as the toolbar ring).
    pub(crate) fn modal_nav_slot<'a>(
        &self,
        action: RowAction,
        radius: f32,
        contrast: bool,
        el: iced::Element<'a, Message>,
    ) -> iced::Element<'a, Message> {
        let idx = {
            let mut items = self.keynav.modal.items.borrow_mut();
            items.push(action);
            items.len() - 1
        };
        // Hover converges the ring with the mouse position.
        let el: iced::Element<'a, Message> = iced::widget::MouseArea::new(el)
            .on_enter(Message::ModalNavHover(idx))
            .into();
        // RAW index comparison, no clamping: this runs mid-recording,
        // when the list is still partial, and clamping a selection of
        // e.g. 3 against a 1-long list would ring EVERY row on its
        // way in (each row briefly "is" the last one). The router
        // clamps when it acts; the ring only matches exact indices.
        let surface = self.modal_nav_surface().map(|(s, _)| s);
        let selected = match self.keynav.modal.selected {
            Some((tag, i)) if Some(tag) == surface => Some(i),
            _ => self.keynav.modal.default.get(),
        } == Some(idx);
        if selected {
            let color = if contrast {
                crate::theme::OryxisColors::t().text_primary
            } else {
                crate::theme::OryxisColors::t().accent
            };
            crate::widgets::select_ring_colored(el, radius, color)
        } else {
            el
        }
    }

    /// `modal_nav_slot` that also marks this row as the surface
    /// default (confirm dialogs call it on their action button).
    pub(crate) fn modal_nav_slot_default<'a>(
        &self,
        action: RowAction,
        radius: f32,
        contrast: bool,
        el: iced::Element<'a, Message>,
    ) -> iced::Element<'a, Message> {
        let next_idx = self.keynav.modal.items.borrow().len();
        self.keynav.modal.default.set(Some(next_idx));
        self.modal_nav_slot(action, radius, contrast, el)
    }

    /// Clear the side-panel row recording. The panel view calls this
    /// at the top of its render pass, then records its actionable
    /// rows through `panel_nav_slot`.
    pub(crate) fn panel_nav_reset(&self) {
        self.keynav.panel_items.borrow_mut().clear();
    }

    /// Drop the panel row-mode state entirely (selection + remembered
    /// position). Called wherever the host editor opens or closes so
    /// a stale ring can never survive across editor sessions.
    pub(crate) fn panel_nav_clear(&mut self) {
        self.keynav.panel_selected = None;
        self.keynav.panel_last_row.set(None);
        // A dropdown can't survive its panel: if the panel unmounts
        // while a pick_list menu was open, the widget never gets to
        // publish on_close, so drop the flag here too.
        self.keynav.pick_open = false;
    }

    /// Record one actionable side-panel row and ring it when it is
    /// the current selection. Same `RowAction` vocabulary as the
    /// Settings rows (activate / picker prev+next / input focus).
    /// Input rows never draw the ring: Tab gives them real iced
    /// focus and the field's own focused border is the indicator.
    pub(crate) fn panel_nav_slot<'a>(
        &self,
        action: RowAction,
        radius: f32,
        el: iced::Element<'a, Message>,
    ) -> iced::Element<'a, Message> {
        let is_input = action.focus.is_some();
        let idx = {
            let mut items = self.keynav.panel_items.borrow_mut();
            items.push(action);
            items.len() - 1
        };
        if !is_input && self.keynav.panel_selected == Some(idx) {
            crate::widgets::select_ring_radius(el, radius)
        } else {
            el
        }
    }

    /// Recording wrapper over `widgets::context_menu_item`: same row,
    /// registered for Up/Down + Enter in the open menu. The free fn
    /// stays for non-navigable uses (the hover-only split popover).
    pub(crate) fn menu_item<'a>(
        &self,
        icon: impl Into<crate::os_icon::BrandIcon>,
        label: &'a str,
        msg: Message,
        color: iced::Color,
    ) -> iced::Element<'a, Message> {
        self.modal_nav_slot(
            RowAction::activate(msg.clone()),
            4.0,
            false,
            crate::widgets::context_menu_item(icon, label, msg, color),
        )
    }

    /// Clear the Settings content recording. Each Settings section
    /// view calls this at the top of its render pass, then records
    /// its actionable rows through `settings_nav_slot`.
    pub(crate) fn keynav_settings_reset(&self) {
        self.keynav.settings_row_actions.borrow_mut().clear();
        self.keynav.content_rows.borrow_mut().clear();
        *self.keynav.content_section_starts.borrow_mut() = vec![0];
    }

    /// Record one actionable Settings content row (single-column) and
    /// ring it when selected. Read-only rows are simply not recorded,
    /// so arrows only stop on things Enter/Space/Left/Right can act
    /// on. `radius` matches the row's own corner radius.
    pub(crate) fn settings_nav_slot<'a>(
        &self,
        action: RowAction,
        radius: f32,
        el: iced::Element<'a, Message>,
    ) -> iced::Element<'a, Message> {
        let idx = {
            let mut actions = self.keynav.settings_row_actions.borrow_mut();
            actions.push(action);
            actions.len() - 1
        };
        let item = super::NavItem::SettingsRow(idx);
        self.keynav.content_rows.borrow_mut().push(vec![item]);
        if self.keynav.selected_in(super::FocusZone::Content) == Some(item) {
            crate::widgets::select_ring_radius(el, radius)
        } else {
            el
        }
    }

    /// Ring a keyboard-selected content CARD and report its on-screen
    /// rect into `keynav.ring_bounds`, so the Menu key can anchor the
    /// card's context menu at the card (kebab corner) instead of the
    /// mouse position. Only the single ringed element writes the cell
    /// per frame.
    pub(crate) fn keynav_ring_content<'a>(
        &self,
        el: iced::Element<'a, Message>,
    ) -> iced::Element<'a, Message> {
        crate::widgets::bounds_reporter(
            crate::widgets::select_ring(el),
            self.keynav.ring_bounds.clone(),
        )
    }

    /// The anchor for the next kebab-menu open: the Menu key's ring
    /// anchor when set (one-shot), else the live mouse position.
    pub(crate) fn keynav_take_menu_anchor(&mut self) -> (f32, f32) {
        self.keynav
            .menu_anchor
            .take()
            .unwrap_or((self.mouse_position.x, self.mouse_position.y))
    }

    /// Record one generic content-action row (single-column): used by
    /// content surfaces whose rows fire arbitrary messages, like the
    /// dynamic cloud-group task list. The caller clears via
    /// `keynav_clear_content` at the top of its render pass.
    pub(crate) fn content_action_slot<'a>(
        &self,
        action: RowAction,
        radius: f32,
        el: iced::Element<'a, Message>,
    ) -> iced::Element<'a, Message> {
        let idx = {
            let mut actions = self.keynav.content_actions.borrow_mut();
            actions.push(action);
            actions.len() - 1
        };
        let item = super::NavItem::ContentAction(idx);
        self.keynav.content_rows.borrow_mut().push(vec![item]);
        if self.keynav.selected_in(super::FocusZone::Content) == Some(item) {
            crate::widgets::select_ring_radius(el, radius)
        } else {
            el
        }
    }

    /// Recording toggle row for Settings content: same visual as
    /// `widgets::toggle_row`, plus Enter/Space flipping it from the
    /// keyboard.
    pub(crate) fn nav_toggle_row<'a>(
        &self,
        label: &'a str,
        value: bool,
        msg: Message,
    ) -> iced::Element<'a, Message> {
        self.settings_nav_slot(
            RowAction::activate(msg.clone()),
            8.0,
            crate::widgets::toggle_row(label, value, msg),
        )
    }

    /// Recording picker row for Settings content: the standard
    /// "label ... pick_list" line, with Left/Right cycling the
    /// options without opening the dropdown. (Settings keeps the
    /// ring-and-cycle model: Up/Down stay row navigation here, unlike
    /// the side panels where pickers are Tab-focusable inputs.)
    pub(crate) fn nav_pick_row<'a, D, F>(
        &self,
        label: &'a str,
        options: Vec<String>,
        selected: String,
        display: D,
        width: f32,
        on_change: F,
    ) -> iced::Element<'a, Message>
    where
        D: Fn(&String) -> String + 'a,
        F: Fn(String) -> Message + Clone + 'a,
    {
        let (prev, next) = cycle_pair(&options, &selected, on_change.clone());
        // The ring hugs the pick_list itself, not the whole row: it
        // marks WHICH control Left/Right act on (user feedback).
        let picker = self.settings_nav_slot(
            RowAction::picker(prev, next),
            crate::widgets::INPUT_RADIUS,
            iced::widget::pick_list(Some(selected), options, display)
                .on_select(on_change)
                // Mouse-opened dropdowns arm the same key guard the
                // focusable panel pickers use, so Esc closes the menu
                // instead of falling through to the app routers.
                .on_open(Message::PickOpenChanged(true))
                .on_close(Message::PickOpenChanged(false))
                .width(width)
                .padding(10)
                .style(crate::widgets::rounded_pick_list_style)
                .into(),
        );
        crate::widgets::dir_row(vec![
            iced::widget::text(label)
                .size(13)
                .color(crate::theme::OryxisColors::t().text_primary)
                .into(),
            iced::widget::Space::new().width(iced::Length::Fill).into(),
            picker,
        ])
        .align_y(iced::Alignment::Center)
        .into()
    }

    /// Recording wrapper over `widgets::sort_menu_row`.
    pub(crate) fn sort_row(
        &self,
        kind: crate::state::SortMenuKind,
        sort: crate::state::ListSort,
        icon: iced::widget::Text<'static, iced::Theme, iced::Renderer>,
        label_key: &'static str,
        is_active: bool,
    ) -> iced::Element<'static, Message> {
        self.modal_nav_slot(
            RowAction::activate(Message::SetListSort(kind, sort)),
            4.0,
            false,
            crate::widgets::sort_menu_row(kind, sort, icon, label_key, is_active),
        )
    }
}
