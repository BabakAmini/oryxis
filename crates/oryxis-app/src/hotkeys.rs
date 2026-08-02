//! Editable keyboard binding model.
//!
//! Each `HotkeyAction` is something the user can trigger from the
//! keyboard (open settings, switch tab, close active tab, ...). A
//! `HotkeyBinding` pairs a modifier set with a `PrimaryKey`; the
//! `match_event` helper turns an incoming iced KeyPressed into an
//! optional `FamilyMatch` which the dispatcher inspects to build the
//! final `Message`.
//!
//! Families (`Digit1to9`, `ArrowLeftRight`) are bindings where the
//! suffix isn't editable, mirroring Termius's "Ctrl + [1...9]" row.
//! Only their modifier set can change.

use std::collections::HashMap;
use std::fmt::Write;

use iced::keyboard::{self, key::Named, Key, Modifiers};

/// Stable identifier for every editable action. Persisted to the
/// settings table as `hotkey_<snake_case_name>` so renames are
/// breaking changes; treat the variant order as append-only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HotkeyAction {
    // Navigation / global pickers
    ShowNewTabPicker,
    ShowTabJump,
    /// Command palette (C4): fuzzy search over every action. Global,
    /// so no `terminal_only` / `vault_only` gate.
    ShowCommandPalette,
    OpenLocalShell,
    NewWindow,
    CloseActiveTab,
    OpenPortForwards,
    OpenSettings,
    FocusViewSearch,
    /// Open a new SFTP browser tab.
    OpenSftp,
    // Tab strip
    SwitchToTabSlot,   // family: Ctrl + digit 1..9
    CycleTabs,         // family: Alt + ArrowLeft/Right
    // Window
    ToggleFullscreen,
    // Font zoom (the three discrete keys; wheel zoom isn't editable)
    FontZoomIn,
    FontZoomOut,
    FontZoomReset,
    // Terminal split panes. These only fire while the terminal view is
    // focused (`terminal_only`); elsewhere the key is left free.
    SplitPaneVertical,
    SplitPaneHorizontal,
    FocusPaneLeft,
    FocusPaneRight,
    FocusPaneUp,
    FocusPaneDown,
    /// Expand the focused pane to the whole tab, and back. The layout is
    /// untouched while zoomed, so restoring puts every pane back exactly
    /// where it was.
    ToggleMaximizePane,
    /// Ring the terminal-sidebar list rows (Snippets / History):
    /// opens the sidebar when closed, cycles the two list tabs on
    /// repeat. Terminal-only, like the split-pane family.
    FocusSidebarList,
    /// Open/close the terminal sidebar for the focused tab.
    ToggleSidebar,
    /// Hybrid tab (issue #61): flip the focused SSH tab between its
    /// terminal and its host's files (full SFTP surface).
    ToggleTabFiles,
    /// Broadcast input (C2): arm / disarm fan-out of keystrokes to every
    /// pane of the focused tab. Terminal-scoped; Ctrl+Shift+U by default.
    ToggleBroadcastInput,
    /// Jump to a vault section by position. Family: Ctrl+Shift +
    /// digit 1..8 (Hosts, Keychain, Snippets, Port Forwarding,
    /// Logs, Cloud Accounts, Proxies, Known Hosts); 9 is spare.
    VaultSectionSlot,
    // Vault-area section cycling (Hosts -> Keychain -> ... in sub-nav
    // order). Only fire in the vault area (`vault_only`); inside a
    // terminal tab the key is left free for TUI apps.
    VaultSectionPrev,
    VaultSectionNext,
    // Vault entity creation. Each opens its editor panel, navigating
    // to the owning vault section first (the panels only render
    // there). Appended per the order contract above.
    /// Open the new-host editor (Hosts section).
    NewHost,
    /// Open the key import panel (Keychain).
    NewKey,
    /// Open the new-identity panel (Keychain).
    NewIdentity,
    // Terminal clipboard + scrollback (#75). These were hard-coded
    // chords until they moved into this table, which is why they sit
    // at the end of the append-only order despite being among the
    // oldest behaviours in the app.
    /// Copy the terminal selection. Handled inside the terminal widget
    /// (it owns the selection), so the resolved chords are pushed down
    /// to it rather than dispatched here.
    TerminalCopy,
    /// Paste the clipboard into the focused pane. Handled in the
    /// dispatcher, which is the only layer that can reach an SSH
    /// session.
    TerminalPaste,
    /// Paste the X11 PRIMARY selection (the text of the last completed
    /// selection, remembered independently of the highlight) into the
    /// focused pane: the keyboard twin of middle-click. Widget-side like
    /// `TerminalCopy`, because PRIMARY lives in the canvas state; the
    /// widget hands the text over and the dispatcher pastes it.
    /// Factory inputs: Shift+Insert (the xterm / kitty / Alacritty
    /// convention) and the middle mouse button.
    TerminalPasteSelection,
    /// Select the whole terminal buffer. Widget-side, like `TerminalCopy`.
    TerminalSelectAll,
    /// Page the scrollback up. Widget-side (it owns `scroll_offset`).
    ScrollbackPageUp,
    /// Page the scrollback down. Widget-side, like `ScrollbackPageUp`.
    ScrollbackPageDown,
    /// Privacy Mode session override (issue #78): flip a volatile
    /// forced-on/off state that sits above the global setting AND the
    /// per-host overrides, for "I'm about to share my screen" moments.
    /// Never persisted. Global (not `terminal_only`): the vault
    /// surfaces mask too.
    TogglePrivacyMode,
    /// Ad-hoc quick connect (issue #99): open the "+ Host" editor
    /// empty, where "Connect without saving" runs a session that is
    /// never persisted. Global: unlike `NewHost`'s bare Ctrl+N this
    /// ships with a Shift chord, so it also fires inside a terminal.
    ShowQuickConnect,
    /// Reconnect the focused tab's session: the tab context menu's
    /// "Reconnect" entry on a chord. Works on live tabs too (a
    /// "restart this host"), same handler either way. Terminal-only:
    /// there is no focused tab to reconnect anywhere else.
    ReconnectTab,
}

impl HotkeyAction {
    /// All actions in display order. Used by the Settings panel to
    /// iterate without forgetting one.
    pub fn all() -> &'static [HotkeyAction] {
        use HotkeyAction::*;
        &[
            ShowNewTabPicker,
            ShowTabJump,
            ShowCommandPalette,
            OpenLocalShell,
            NewWindow,
            NewHost,
            ShowQuickConnect,
            NewKey,
            NewIdentity,
            ReconnectTab,
            CloseActiveTab,
            OpenPortForwards,
            OpenSettings,
            FocusViewSearch,
            OpenSftp,
            SwitchToTabSlot,
            CycleTabs,
            ToggleFullscreen,
            FontZoomIn,
            FontZoomOut,
            FontZoomReset,
            SplitPaneVertical,
            SplitPaneHorizontal,
            FocusPaneLeft,
            FocusPaneRight,
            FocusPaneUp,
            FocusPaneDown,
            ToggleMaximizePane,
            FocusSidebarList,
            ToggleSidebar,
            ToggleTabFiles,
            ToggleBroadcastInput,
            TogglePrivacyMode,
            TerminalCopy,
            TerminalPaste,
            TerminalPasteSelection,
            TerminalSelectAll,
            ScrollbackPageUp,
            ScrollbackPageDown,
            VaultSectionSlot,
            VaultSectionPrev,
            VaultSectionNext,
        ]
    }

    /// Stable snake_case id used in the settings key
    /// (`hotkey_show_new_tab_picker`, ...). Must not change after a
    /// release ships.
    pub fn id(self) -> &'static str {
        use HotkeyAction::*;
        match self {
            ShowNewTabPicker => "show_new_tab_picker",
            ShowTabJump => "show_tab_jump",
            ShowCommandPalette => "show_command_palette",
            OpenLocalShell => "open_local_shell",
            NewWindow => "new_window",
            CloseActiveTab => "close_active_tab",
            OpenPortForwards => "open_port_forwards",
            OpenSettings => "open_settings",
            FocusViewSearch => "focus_view_search",
            OpenSftp => "open_sftp",
            SwitchToTabSlot => "switch_to_tab_slot",
            CycleTabs => "cycle_tabs",
            ToggleFullscreen => "toggle_fullscreen",
            FontZoomIn => "font_zoom_in",
            FontZoomOut => "font_zoom_out",
            FontZoomReset => "font_zoom_reset",
            SplitPaneVertical => "split_pane_vertical",
            SplitPaneHorizontal => "split_pane_horizontal",
            FocusPaneLeft => "focus_pane_left",
            FocusPaneRight => "focus_pane_right",
            FocusPaneUp => "focus_pane_up",
            FocusPaneDown => "focus_pane_down",
            ToggleMaximizePane => "toggle_maximize_pane",
            FocusSidebarList => "focus_sidebar_list",
            ToggleSidebar => "toggle_sidebar",
            ToggleTabFiles => "toggle_tab_files",
            ToggleBroadcastInput => "toggle_broadcast_input",
            TogglePrivacyMode => "toggle_privacy_mode",
            VaultSectionSlot => "vault_section_slot",
            VaultSectionPrev => "vault_section_prev",
            VaultSectionNext => "vault_section_next",
            NewHost => "new_host",
            ShowQuickConnect => "show_quick_connect",
            ReconnectTab => "reconnect_tab",
            NewKey => "new_key",
            NewIdentity => "new_identity",
            TerminalCopy => "terminal_copy",
            TerminalPaste => "terminal_paste",
            TerminalPasteSelection => "terminal_paste_selection",
            TerminalSelectAll => "terminal_select_all",
            ScrollbackPageUp => "scrollback_page_up",
            ScrollbackPageDown => "scrollback_page_down",
        }
    }

    /// i18n key for the action's display label.
    pub fn label_key(self) -> &'static str {
        use HotkeyAction::*;
        match self {
            ShowNewTabPicker => "hotkey_show_new_tab_picker",
            ShowTabJump => "hotkey_show_tab_jump",
            ShowCommandPalette => "hotkey_show_command_palette",
            OpenLocalShell => "hotkey_open_local_shell",
            NewWindow => "hotkey_new_window",
            CloseActiveTab => "hotkey_close_active_tab",
            OpenPortForwards => "hotkey_open_port_forwards",
            OpenSettings => "hotkey_open_settings",
            FocusViewSearch => "hotkey_focus_view_search",
            OpenSftp => "hotkey_open_sftp",
            SwitchToTabSlot => "hotkey_switch_to_tab_slot",
            CycleTabs => "hotkey_cycle_tabs",
            ToggleFullscreen => "hotkey_toggle_fullscreen",
            FontZoomIn => "hotkey_font_zoom_in",
            FontZoomOut => "hotkey_font_zoom_out",
            FontZoomReset => "hotkey_font_zoom_reset",
            // Reuse the context-menu split labels (already translated in
            // all 17 languages) rather than minting parallel keys.
            SplitPaneVertical => "split_side_by_side",
            SplitPaneHorizontal => "split_stacked",
            FocusPaneLeft => "hotkey_focus_pane_left",
            FocusPaneRight => "hotkey_focus_pane_right",
            FocusPaneUp => "hotkey_focus_pane_up",
            FocusPaneDown => "hotkey_focus_pane_down",
            ToggleMaximizePane => "hotkey_toggle_maximize_pane",
            FocusSidebarList => "hotkey_focus_sidebar_list",
            ToggleSidebar => "hotkey_toggle_sidebar",
            ToggleTabFiles => "hotkey_toggle_tab_files",
            ToggleBroadcastInput => "hotkey_toggle_broadcast_input",
            TogglePrivacyMode => "hotkey_toggle_privacy_mode",
            VaultSectionSlot => "hotkey_vault_section_slot",
            VaultSectionPrev => "hotkey_vault_section_prev",
            VaultSectionNext => "hotkey_vault_section_next",
            // Reuse the vault-area button labels (already translated
            // in all 17 languages), same as the split-pane pair.
            NewHost => "new_host",
            ShowQuickConnect => "quick_connect",
            // Reuses the tab context menu's entry label, same pattern.
            ReconnectTab => "reconnect",
            NewKey => "import_key",
            NewIdentity => "new_identity",
            // Reuse the terminal context-menu labels (already
            // translated in all 23 languages) rather than minting
            // parallel keys, same as the split-pane pair above.
            TerminalCopy => "terminal_copy",
            TerminalPaste => "terminal_paste",
            TerminalPasteSelection => "hotkey_terminal_paste_selection",
            TerminalSelectAll => "select_all",
            ScrollbackPageUp => "hotkey_scrollback_page_up",
            ScrollbackPageDown => "hotkey_scrollback_page_down",
        }
    }

    /// Whether the action only applies while the terminal view is
    /// focused. The dispatch loop skips these elsewhere so the key
    /// stays free in other views (and doesn't swallow the event).
    pub fn terminal_only(self) -> bool {
        use HotkeyAction::*;
        matches!(
            self,
            SplitPaneVertical
                | SplitPaneHorizontal
                | FocusPaneLeft
                | FocusPaneRight
                | FocusPaneUp
                | FocusPaneDown
                | ToggleMaximizePane
                | FocusSidebarList
                | ToggleSidebar
                | ToggleTabFiles
                | ToggleBroadcastInput
                | ReconnectTab
                | TerminalCopy
                | TerminalPaste
                | TerminalPasteSelection
                | TerminalSelectAll
                | ScrollbackPageUp
                | ScrollbackPageDown
        )
    }

    /// Actions the terminal WIDGET performs from its own canvas state
    /// (selection, scroll offset), which `dispatch_hotkey_action` only
    /// swallows. They fire from a real keystroke reaching the widget,
    /// never from a `RunHotkeyAction` message, so the command palette
    /// (which dispatches the message) must not list them: a click would
    /// silently do nothing.
    pub fn widget_dispatched(self) -> bool {
        use HotkeyAction::*;
        matches!(
            self,
            TerminalCopy
                | TerminalPasteSelection
                | TerminalSelectAll
                | ScrollbackPageUp
                | ScrollbackPageDown
        )
    }

    /// Whether the action only applies in the vault area (Home and
    /// its sub-sections). The dispatch loop skips these elsewhere,
    /// leaving the key free: Ctrl+PageUp/Down inside a terminal tab
    /// belongs to the TUI running there, not to Oryxis.
    pub fn vault_only(self) -> bool {
        matches!(
            self,
            HotkeyAction::VaultSectionPrev | HotkeyAction::VaultSectionNext
        )
    }

    /// Whether the primary key (suffix) is editable. Family actions
    /// are modifier-only; everything else accepts any single primary.
    pub fn primary_editable(self) -> bool {
        !matches!(
            self,
            HotkeyAction::SwitchToTabSlot
                | HotkeyAction::CycleTabs
                | HotkeyAction::VaultSectionSlot
        )
    }

    /// Whether ANY mouse button may be bound to this action, which is
    /// what the Shortcuts chip's placeholder announces.
    ///
    /// A family action edits its modifiers only, so it has no primary
    /// slot for a button to occupy; everything else takes at least the
    /// side buttons.
    pub fn accepts_mouse(self) -> bool {
        self.primary_editable()
    }

    /// Whether THIS button may be bound to this action.
    ///
    /// Side buttons are free window-wide (see
    /// [`MouseButton::is_side_button`]), so they carry any action. The
    /// wheel click is only ever read inside the terminal canvas, so an
    /// action that never fires there could not fire from one either:
    /// `terminal_only` IS that set, which is why this derives from it
    /// rather than listing actions twice.
    pub fn accepts_mouse_button(self, button: MouseButton) -> bool {
        self.accepts_mouse() && (button.is_side_button() || self.terminal_only())
    }

    /// Which layer runs a mouse binding on this action.
    ///
    /// The single authority for the split, called by BOTH sides
    /// (`views::terminal::terminal_mouse_resolver` and
    /// `shortcuts::dispatch_mouse_binding`) precisely so they cannot
    /// drift: the two see the same press, so a pair claimed twice fires
    /// twice and a pair claimed by neither is a dead button.
    pub fn mouse_binding_owner(self, button: MouseButton) -> MouseBindingOwner {
        if self.widget_dispatched() || !button.is_side_button() {
            // The five canvas-state gestures can only run in the widget,
            // whatever the button; and the wheel click is only readable
            // over the canvas in the first place.
            MouseBindingOwner::Widget
        } else {
            // A side button is free window-wide, so the app runs it and
            // the gesture works outside a terminal too.
            MouseBindingOwner::App
        }
    }
}

/// Which layer performs a mouse binding. See
/// [`HotkeyAction::mouse_binding_owner`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseBindingOwner {
    /// The terminal widget, over its own canvas.
    Widget,
    /// The app's global press handler, anywhere in the window.
    App,
}

/// A mouse button that can stand in for the primary key of a binding.
///
/// Left and Right are deliberately NOT in this set. Both are the
/// terminal canvas's own gestures (select / the PuTTY right-click
/// scheme), and binding either would take a gesture away from the
/// terminal with no way back. Everything else is fair game: none of
/// these buttons produce text, so a bare mouse binding is a chord on
/// its own, no modifier required.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    Middle,
    Back,
    Forward,
    /// Any further button the OS reports by index (thumb buttons past
    /// Back / Forward, tilt-wheel clicks, ...).
    Other(u16),
}

impl MouseButton {
    /// The bindable subset of iced's button set. `None` for Left and
    /// Right, the two the terminal keeps for itself.
    pub fn from_iced(button: iced::mouse::Button) -> Option<Self> {
        match button {
            iced::mouse::Button::Middle => Some(Self::Middle),
            iced::mouse::Button::Back => Some(Self::Back),
            iced::mouse::Button::Forward => Some(Self::Forward),
            iced::mouse::Button::Other(n) => Some(Self::Other(n)),
            iced::mouse::Button::Left | iced::mouse::Button::Right => None,
        }
    }

    /// Settings-table token. The `mouse_` prefix is what keeps these
    /// clear of every other primary: no `Named` name, punctuation token
    /// or single alphanumeric char can collide with it, so `parse` can
    /// try mouse buttons first without shadowing anything.
    pub fn token(self) -> String {
        match self {
            Self::Middle => "mouse_middle".into(),
            Self::Back => "mouse_back".into(),
            Self::Forward => "mouse_forward".into(),
            Self::Other(n) => format!("mouse_{n}"),
        }
    }

    /// Reverse of [`MouseButton::token`].
    pub fn parse_token(s: &str) -> Option<Self> {
        match s {
            "mouse_middle" => Some(Self::Middle),
            "mouse_back" => Some(Self::Back),
            "mouse_forward" => Some(Self::Forward),
            other => other
                .strip_prefix("mouse_")
                .and_then(|n| n.parse::<u16>().ok())
                .map(Self::Other),
        }
    }

    /// Whether this is a SIDE button (the thumb pair and anything past
    /// it), as opposed to the wheel click.
    ///
    /// The distinction decides where the button may fire. Nothing in
    /// the app reacts to a side button: iced's `button` / `scrollable`
    /// / `text_input` all act on the primary, and the terminal canvas
    /// claims primary / secondary / middle. So a side button is free
    /// window-wide and can carry any action.
    ///
    /// The wheel click is not free: the canvas spends it on mouse
    /// reports and the X11 paste, and a middle click over a list or a
    /// scrollbar is a gesture users expect elsewhere too. It stays
    /// terminal-scoped.
    pub fn is_side_button(self) -> bool {
        !matches!(self, Self::Middle)
    }

    /// User-facing badge label, translated. Deliberately short: it
    /// shares a chip with the modifier badges.
    pub fn label(self) -> String {
        match self {
            Self::Middle => crate::i18n::t("mouse_btn_middle").to_string(),
            Self::Back => crate::i18n::t("mouse_btn_back").to_string(),
            Self::Forward => crate::i18n::t("mouse_btn_forward").to_string(),
            Self::Other(n) => crate::i18n::t("mouse_btn_other").replace("{n}", &n.to_string()),
        }
    }
}

/// The non-modifier half of a binding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrimaryKey {
    /// A printable character, ASCII case-insensitive (`'k'` matches
    /// both `"k"` and `"K"`).
    Char(char),
    /// A named key (F11, Escape, ',', '=', ...). Stored as
    /// `iced::keyboard::key::Named` plus an optional character
    /// fallback for punctuation.
    Named(Named),
    /// Single-char punctuation that iced reports as `Character` not
    /// `Named` (`,`, `=`, `-`, `+`). Kept as a distinct variant from
    /// `Char` because the editor needs to know it's punctuation when
    /// rendering the badge.
    Punct(&'static str),
    /// Family: any digit 1..9. Suffix isn't editable.
    Digit1to9,
    /// Family: ArrowLeft or ArrowRight. Suffix isn't editable.
    ArrowLeftRight,
    /// A mouse button, optionally with modifiers. Only fires inside the
    /// terminal canvas (that is the one surface where a click can't
    /// belong to a widget), so only `HotkeyAction::accepts_mouse`
    /// actions may hold one.
    Mouse(MouseButton),
}

/// What `HotkeyBinding::match_event` returns: `None` if the event
/// didn't match this binding; `Some(FamilyMatch)` if it did, carrying
/// any extracted payload from the family variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FamilyMatch {
    /// Plain match, no payload.
    Plain,
    /// Digit family matched digit `n` (1..=9).
    Digit(u8),
    /// Arrow family matched left arrow.
    ArrowLeft,
    /// Arrow family matched right arrow.
    ArrowRight,
}

/// A modifier set + primary key. `Modifiers` from iced isn't stored
/// directly so we can `PartialEq` and serialize it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HotkeyBinding {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    pub logo: bool,
    pub primary: PrimaryKey,
}

impl HotkeyBinding {
    /// Returns `Some(FamilyMatch)` when this binding fires for the
    /// given event, otherwise `None`. Modifier match is exact (a
    /// binding with no Shift won't fire when Shift is held), this
    /// avoids the `Ctrl+1` / `Ctrl+!` confusion on US layouts.
    pub fn match_event(&self, key: &Key, modifiers: &Modifiers) -> Option<FamilyMatch> {
        if modifiers.control() != self.ctrl
            || modifiers.shift() != self.shift
            || modifiers.alt() != self.alt
            || modifiers.logo() != self.logo
        {
            return None;
        }
        match self.primary {
            PrimaryKey::Char(c) => match key {
                Key::Character(s) => {
                    let s = s.as_str();
                    if s.len() == 1 && s.eq_ignore_ascii_case(&c.to_string()) {
                        Some(FamilyMatch::Plain)
                    } else {
                        None
                    }
                }
                _ => None,
            },
            PrimaryKey::Named(n) => match key {
                Key::Named(actual) if *actual == n => Some(FamilyMatch::Plain),
                _ => None,
            },
            PrimaryKey::Punct(p) => match key {
                Key::Character(s) if s.as_str() == p => Some(FamilyMatch::Plain),
                _ => None,
            },
            PrimaryKey::Digit1to9 => match key {
                Key::Character(s) => s
                    .as_str()
                    .chars()
                    .next()
                    .and_then(|ch| ch.to_digit(10))
                    .filter(|d| (1..=9).contains(d))
                    .map(|d| FamilyMatch::Digit(d as u8)),
                _ => None,
            },
            PrimaryKey::ArrowLeftRight => match key {
                Key::Named(Named::ArrowLeft) => Some(FamilyMatch::ArrowLeft),
                Key::Named(Named::ArrowRight) => Some(FamilyMatch::ArrowRight),
                _ => None,
            },
            // No keystroke can ever produce a mouse binding.
            PrimaryKey::Mouse(_) => None,
        }
    }

    /// Mouse twin of [`HotkeyBinding::match_event`]. Modifier match is
    /// exact for the same reason: `Ctrl+Middle` and a bare middle click
    /// are different bindings, so one must not fire for the other.
    pub fn match_mouse(&self, button: MouseButton, modifiers: &Modifiers) -> bool {
        self.primary == PrimaryKey::Mouse(button)
            && modifiers.control() == self.ctrl
            && modifiers.shift() == self.shift
            && modifiers.alt() == self.alt
            && modifiers.logo() == self.logo
    }

    /// Whether the primary is a mouse button.
    pub fn is_mouse(&self) -> bool {
        matches!(self.primary, PrimaryKey::Mouse(_))
    }

    /// Whether the binding is valid for the editor: it must carry at
    /// least one of Ctrl / Alt / Logo, otherwise it would silently
    /// intercept the user's typing.
    ///
    /// Shift is not a modifier for this purpose on a primary that
    /// produces text (`Shift+a` is just an uppercase A), but it is on
    /// a primary that never can: `Shift+Insert` (paste) and
    /// `Shift+PageUp` (scrollback) are how every mainstream terminal
    /// spells those chords, and neither steals a keystroke the user
    /// could have typed.
    pub fn is_safe(&self) -> bool {
        // A mouse button never types anything, and the bindable set
        // already excludes the two buttons the terminal owns, so a bare
        // mouse binding steals nothing.
        if self.is_mouse() {
            return true;
        }
        if self.ctrl || self.alt || self.logo {
            return true;
        }
        // A function key is a chord on its own (F11 = fullscreen).
        if self.is_function_key_primary() {
            return true;
        }
        // Otherwise Shift is required, and only on a primary that can't
        // be typed. Modifier-free Insert / Delete / arrows stay
        // unbindable: the PTY wants them, and leaving them out keeps
        // Delete free as the capture editor's "remove this chord" key.
        self.shift && self.is_non_text_primary()
    }

    /// `true` when this binding looks like a sequence the terminal
    /// shell normally consumes itself: Ctrl + printable character with
    /// no other modifier. Examples: Ctrl+L = clear, Ctrl+P = history
    /// prev, Ctrl+K = readline kill, Ctrl+[ = Escape byte. Ctrl+Shift+X
    /// is NOT included because shells don't interpret it as a control
    /// byte. Used by the dispatcher to suppress app-level handling
    /// when the terminal view is focused.
    pub fn is_terminal_control_sequence(&self) -> bool {
        if !self.ctrl || self.alt || self.logo || self.shift {
            return false;
        }
        match self.primary {
            PrimaryKey::Char(c) => c.is_ascii_alphanumeric(),
            // Only the punctuation keys that genuinely produce control
            // bytes via the kernel's tty layer get suppressed. The
            // wider Punct set (`,`, `=`, `-`, `.`, `;`, `/`) doesn't
            // map to anything readline or the shell consumes, so the
            // default bindings on those (OpenSettings, FontZoomIn,
            // FontZoomOut) must continue to fire inside the terminal.
            // The accepted set mirrors the C0 escapes a US/QWERTY shell
            // actually generates: Ctrl+[ = ESC, Ctrl+\ = FS,
            // Ctrl+] = GS.
            PrimaryKey::Punct("[") => true,
            PrimaryKey::Punct("\\") => true,
            PrimaryKey::Punct("]") => true,
            _ => false,
        }
    }

    /// `true` when the primary is F1..F12. The only primaries a
    /// modifier-free binding may target.
    fn is_function_key_primary(&self) -> bool {
        matches!(
            self.primary,
            PrimaryKey::Named(
                Named::F1
                    | Named::F2
                    | Named::F3
                    | Named::F4
                    | Named::F5
                    | Named::F6
                    | Named::F7
                    | Named::F8
                    | Named::F9
                    | Named::F10
                    | Named::F11
                    | Named::F12
            )
        )
    }

    /// `true` when the primary can never produce text on its own: the
    /// function keys plus the navigation / editing block. These are the
    /// only primaries where a bare Shift is a real chord rather than
    /// uppercase typing.
    ///
    /// Escape / Enter / Tab / Backspace / Space are deliberately out:
    /// they produce bytes the shell consumes, so binding them (even
    /// with Shift) would eat input the PTY needs.
    fn is_non_text_primary(&self) -> bool {
        matches!(
            self.primary,
            PrimaryKey::Named(
                Named::F1
                    | Named::F2
                    | Named::F3
                    | Named::F4
                    | Named::F5
                    | Named::F6
                    | Named::F7
                    | Named::F8
                    | Named::F9
                    | Named::F10
                    | Named::F11
                    | Named::F12
                    | Named::Insert
                    | Named::Delete
                    | Named::Home
                    | Named::End
                    | Named::PageUp
                    | Named::PageDown
                    | Named::ArrowUp
                    | Named::ArrowDown
                    | Named::ArrowLeft
                    | Named::ArrowRight
            )
        )
    }

    /// Serialize for the settings table: `"ctrl+shift+n"` /
    /// `"alt+arrows"` / `"f11"`. Lowercase, plus-separated, modifiers
    /// in canonical order so a round-trip never reformats.
    pub fn serialize(&self) -> String {
        let mut out = String::new();
        if self.ctrl {
            out.push_str("ctrl+");
        }
        if self.shift {
            out.push_str("shift+");
        }
        if self.alt {
            out.push_str("alt+");
        }
        if self.logo {
            out.push_str("logo+");
        }
        match self.primary {
            PrimaryKey::Char(c) => {
                let _ = write!(out, "{}", c.to_ascii_lowercase());
            }
            PrimaryKey::Named(n) => out.push_str(named_to_str(n)),
            PrimaryKey::Punct(p) => out.push_str(p),
            PrimaryKey::Digit1to9 => out.push_str("digit"),
            PrimaryKey::ArrowLeftRight => out.push_str("arrows"),
            PrimaryKey::Mouse(b) => out.push_str(&b.token()),
        }
        out
    }

    /// Reverse of `serialize`. Returns `None` for malformed input or
    /// unknown tokens (the caller falls back to the default binding).
    pub fn parse(s: &str) -> Option<Self> {
        let mut ctrl = false;
        let mut shift = false;
        let mut alt = false;
        let mut logo = false;
        let parts: Vec<&str> = s.split('+').collect();
        let (mods, primary_str) = parts.split_at(parts.len().saturating_sub(1));
        let primary_str = primary_str.first()?;
        for m in mods {
            match *m {
                "ctrl" => ctrl = true,
                "shift" => shift = true,
                "alt" => alt = true,
                "logo" => logo = true,
                _ => return None,
            }
        }
        let primary = match *primary_str {
            "digit" => PrimaryKey::Digit1to9,
            "arrows" => PrimaryKey::ArrowLeftRight,
            "," | "." | ";" | "=" | "-" | "+" | "/" | "\\" | "[" | "]" => {
                // Static slice lookup keeps the &'static str alive.
                match *primary_str {
                    "," => PrimaryKey::Punct(","),
                    "." => PrimaryKey::Punct("."),
                    ";" => PrimaryKey::Punct(";"),
                    "=" => PrimaryKey::Punct("="),
                    "-" => PrimaryKey::Punct("-"),
                    "+" => PrimaryKey::Punct("+"),
                    "/" => PrimaryKey::Punct("/"),
                    "\\" => PrimaryKey::Punct("\\"),
                    "[" => PrimaryKey::Punct("["),
                    "]" => PrimaryKey::Punct("]"),
                    _ => unreachable!(),
                }
            }
            other => {
                // Mouse tokens first: they are `mouse_`-prefixed, so
                // they can't shadow a named key or a single char, and
                // checking them here keeps the fallback chain honest.
                if let Some(button) = MouseButton::parse_token(other) {
                    PrimaryKey::Mouse(button)
                } else if let Some(named) = str_to_named(other) {
                    PrimaryKey::Named(named)
                } else if other.len() == 1
                    && other
                        .chars()
                        .next()
                        .is_some_and(|c| c.is_ascii_alphanumeric())
                {
                    // Digit chars (0..=9) round-trip as Char; the
                    // `digit` family token is reserved for the 1..9
                    // suffix variant of `SwitchToTabSlot`.
                    PrimaryKey::Char(other.chars().next().unwrap().to_ascii_lowercase())
                } else {
                    return None;
                }
            }
        };
        Some(HotkeyBinding {
            ctrl,
            shift,
            alt,
            logo,
            primary,
        })
    }

    /// Returns the user-facing badges for the binding (e.g.
    /// `["Ctrl", "Shift", "N"]`). Family suffixes render as their
    /// fixed glyph token (`"1...9"`, `"←/→"`).
    pub fn badges(&self) -> Vec<String> {
        let mut out = Vec::new();
        if self.ctrl {
            out.push("Ctrl".into());
        }
        if self.shift {
            out.push("Shift".into());
        }
        if self.alt {
            out.push("Alt".into());
        }
        if self.logo {
            // Render as Win on Windows / Linux, ⌘ on macOS. iced
            // doesn't expose the host OS at this layer so we pick
            // the cross-platform "Super" token.
            out.push("Super".into());
        }
        let primary = match self.primary {
            PrimaryKey::Char(c) => c.to_ascii_uppercase().to_string(),
            PrimaryKey::Named(n) => named_to_str(n).to_uppercase(),
            PrimaryKey::Punct(p) => p.to_string(),
            PrimaryKey::Digit1to9 => "1...9".into(),
            PrimaryKey::ArrowLeftRight => "←/→".into(),
            PrimaryKey::Mouse(b) => b.label(),
        };
        out.push(primary);
        out
    }
}

fn named_to_str(n: Named) -> &'static str {
    match n {
        Named::Escape => "esc",
        Named::Enter => "enter",
        Named::Tab => "tab",
        Named::Backspace => "backspace",
        Named::Delete => "del",
        Named::Insert => "ins",
        Named::Home => "home",
        Named::End => "end",
        Named::PageUp => "pgup",
        Named::PageDown => "pgdn",
        Named::ArrowUp => "up",
        Named::ArrowDown => "down",
        Named::ArrowLeft => "left",
        Named::ArrowRight => "right",
        Named::Space => "space",
        Named::F1 => "f1",
        Named::F2 => "f2",
        Named::F3 => "f3",
        Named::F4 => "f4",
        Named::F5 => "f5",
        Named::F6 => "f6",
        Named::F7 => "f7",
        Named::F8 => "f8",
        Named::F9 => "f9",
        Named::F10 => "f10",
        Named::F11 => "f11",
        Named::F12 => "f12",
        _ => "?",
    }
}

fn str_to_named(s: &str) -> Option<Named> {
    Some(match s {
        "esc" => Named::Escape,
        "enter" => Named::Enter,
        "tab" => Named::Tab,
        "backspace" => Named::Backspace,
        "del" => Named::Delete,
        "ins" => Named::Insert,
        "home" => Named::Home,
        "end" => Named::End,
        "pgup" => Named::PageUp,
        "pgdn" => Named::PageDown,
        "up" => Named::ArrowUp,
        "down" => Named::ArrowDown,
        "left" => Named::ArrowLeft,
        "right" => Named::ArrowRight,
        "space" => Named::Space,
        "f1" => Named::F1,
        "f2" => Named::F2,
        "f3" => Named::F3,
        "f4" => Named::F4,
        "f5" => Named::F5,
        "f6" => Named::F6,
        "f7" => Named::F7,
        "f8" => Named::F8,
        "f9" => Named::F9,
        "f10" => Named::F10,
        "f11" => Named::F11,
        "f12" => Named::F12,
        _ => return None,
    })
}

/// Builds a `HotkeyBinding` from a captured iced KeyPressed event,
/// or `None` if the event can't be turned into a safe binding (no
/// modifier and not a function key). Used by capture mode in the
/// Settings → Shortcuts editor.
pub fn binding_from_event(
    key: &Key,
    modifiers: &Modifiers,
    primary_editable: bool,
) -> Option<HotkeyBinding> {
    // For family bindings (modifier-only edit) we ignore the primary
    // and just take the modifier set; the caller substitutes the
    // existing primary back in. The editor passes `primary_editable
    // = false` for those.
    let primary_opt: Option<PrimaryKey> = if primary_editable {
        match key {
            Key::Character(s) => {
                let txt = s.as_str();
                if txt.len() == 1 {
                    let ch = txt.chars().next().unwrap();
                    if ch.is_ascii_alphanumeric() {
                        Some(PrimaryKey::Char(ch.to_ascii_lowercase()))
                    } else {
                        // Single source of truth for the punctuation
                        // accept-list: the match returning Some(s) IS
                        // both the membership check and the
                        // &'static str mapping. Adding a new punct
                        // means one new arm, not two synced lists.
                        match ch {
                            ',' => Some(PrimaryKey::Punct(",")),
                            '.' => Some(PrimaryKey::Punct(".")),
                            ';' => Some(PrimaryKey::Punct(";")),
                            '=' => Some(PrimaryKey::Punct("=")),
                            '-' => Some(PrimaryKey::Punct("-")),
                            '+' => Some(PrimaryKey::Punct("+")),
                            '/' => Some(PrimaryKey::Punct("/")),
                            '\\' => Some(PrimaryKey::Punct("\\")),
                            '[' => Some(PrimaryKey::Punct("[")),
                            ']' => Some(PrimaryKey::Punct("]")),
                            _ => None,
                        }
                    }
                } else {
                    None
                }
            }
            Key::Named(n) => Some(PrimaryKey::Named(*n)),
            _ => None,
        }
    } else {
        None
    };

    if primary_editable {
        // Without a recognised primary there is nothing to bind. The
        // old fallback to `PrimaryKey::Char('?')` produced a row that
        // passed `is_safe()` but no real key event ever reproduced,
        // so the binding was silently dead. Returning `None` here
        // keeps the capture in "press a key" state.
        let primary = primary_opt?;
        let binding = HotkeyBinding {
            ctrl: modifiers.control(),
            shift: modifiers.shift(),
            alt: modifiers.alt(),
            logo: modifiers.logo(),
            primary,
        };
        if !binding.is_safe() {
            return None;
        }
        Some(binding)
    } else {
        // Family captures keep the existing primary (a digit, an
        // arrow, etc.) and only swap the modifiers. The user must
        // still pick at least one of Ctrl / Alt / Logo, otherwise
        // any bare digit press would hijack tab switching. The
        // primary isn't read from the event here, so a missing
        // `primary_opt` is fine, fall back to a placeholder that
        // the caller's existing `family` field overrides.
        let binding = HotkeyBinding {
            ctrl: modifiers.control(),
            shift: modifiers.shift(),
            alt: modifiers.alt(),
            logo: modifiers.logo(),
            primary: primary_opt.unwrap_or(PrimaryKey::Digit1to9),
        };
        if !binding.ctrl && !binding.alt && !binding.logo {
            return None;
        }
        Some(binding)
    }
}

/// The bare middle-click chord: `TerminalPasteSelection`'s second
/// factory input, and exactly what Settings > Terminal's "middle-click
/// paste" toggle adds to / removes from the binding table.
pub fn middle_click_chord() -> HotkeyBinding {
    HotkeyBinding {
        ctrl: false,
        shift: false,
        alt: false,
        logo: false,
        primary: PrimaryKey::Mouse(MouseButton::Middle),
    }
}

/// Mouse twin of [`binding_from_event`]: turns a captured button press
/// into a binding, or `None` when the button isn't bindable (Left and
/// Right, which the terminal canvas keeps).
///
/// No `is_safe` check is needed: every button that survives
/// `MouseButton::from_iced` is safe by construction (see
/// [`HotkeyBinding::is_safe`]).
pub fn binding_from_mouse(button: iced::mouse::Button, modifiers: &Modifiers) -> Option<HotkeyBinding> {
    Some(HotkeyBinding {
        ctrl: modifiers.control(),
        shift: modifiers.shift(),
        alt: modifiers.alt(),
        logo: modifiers.logo(),
        primary: PrimaryKey::Mouse(MouseButton::from_iced(button)?),
    })
}

/// Settings-table token for "the user deliberately unbound this
/// action", as opposed to `""`, which means "no override, use the
/// factory chords". Not a parseable chord (`HotkeyBinding::parse`
/// rejects it: no such primary), so it can never collide with a real
/// binding.
const UNBOUND: &str = "none";

/// Which chord of an action's list an edit is aimed at. The Shortcuts
/// editor renders one chip per chord plus a trailing add button, so a
/// capture has to carry the target alongside the action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeySlot {
    /// Overwrite the chord at this index.
    Replace(usize),
    /// Append a chord.
    Add,
}

/// The ordered chords bound to one action.
///
/// `primary()` (index 0) is the canonical chord: it is what the
/// command palette, the tab strip and every tooltip show. The rest are
/// equal-footing alternates that fire the same action. Order is
/// display order, and `push` appends, so the factory chord stays the
/// visible one unless the user removes it.
///
/// Several actions need more than one chord out of the box, which is
/// why this is a list rather than a single binding: `Ctrl+Shift+V` and
/// `Shift+Insert` are both standard paste chords, and dropping either
/// would break muscle memory that every other terminal honours.
///
/// An empty list means "deliberately unbound", which is distinct from
/// the action being absent from the map (no stored row, so the factory
/// default applies).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HotkeyBindings(Vec<HotkeyBinding>);

impl HotkeyBindings {
    /// One chord. The common case for actions with no alternate.
    pub fn single(binding: HotkeyBinding) -> Self {
        Self(vec![binding])
    }

    /// Several chords, in display order. Duplicates are dropped so the
    /// caller doesn't have to care.
    pub fn many(bindings: impl IntoIterator<Item = HotkeyBinding>) -> Self {
        let mut out = Self::default();
        for b in bindings {
            out.push(b);
        }
        out
    }

    /// The chord shown in the UI, or `None` when unbound.
    pub fn primary(&self) -> Option<HotkeyBinding> {
        self.0.first().copied()
    }

    /// Badges for the chord shown in the UI. `None` when unbound, so
    /// callers rendering a hint drop the row rather than printing an
    /// empty chord.
    pub fn badges(&self) -> Option<Vec<String>> {
        self.primary().map(|b| b.badges())
    }

    pub fn iter(&self) -> impl Iterator<Item = &HotkeyBinding> {
        self.0.iter()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn contains(&self, binding: &HotkeyBinding) -> bool {
        self.0.contains(binding)
    }

    /// Append a chord, ignoring an exact duplicate.
    pub fn push(&mut self, binding: HotkeyBinding) {
        if !self.contains(&binding) {
            self.0.push(binding);
        }
    }

    /// Write a chord into `slot`, appending when the slot is `Add` or
    /// points past the end (a stale index from a row that shrank under
    /// the editor). Any other copy of the chord in this list is dropped
    /// first, so a list can never hold the same chord twice.
    pub fn set(&mut self, slot: HotkeySlot, binding: HotkeyBinding) {
        match slot {
            HotkeySlot::Replace(i) if i < self.0.len() => {
                self.0[i] = binding;
                let mut seen = false;
                self.0.retain(|b| {
                    if *b == binding {
                        let first = !seen;
                        seen = true;
                        return first;
                    }
                    true
                });
            }
            _ => self.push(binding),
        }
    }

    /// Drop one chord. Returns whether it was there.
    pub fn remove(&mut self, binding: &HotkeyBinding) -> bool {
        let before = self.0.len();
        self.0.retain(|b| b != binding);
        self.0.len() != before
    }

    /// First chord that fires for this event, if any. Chords are
    /// modifier-exact (see `HotkeyBinding::match_event`), so at most
    /// one can match and the order only decides which is checked first.
    pub fn match_event(&self, key: &Key, modifiers: &Modifiers) -> Option<FamilyMatch> {
        self.match_event_where(key, modifiers, |_| true)
    }

    /// Whether any chord in this list is the given mouse button with
    /// exactly these modifiers.
    pub fn match_mouse(&self, button: MouseButton, modifiers: &Modifiers) -> bool {
        self.0.iter().any(|b| b.match_mouse(button, modifiers))
    }

    /// The mouse chords in this list, in display order.
    pub fn mouse_chords(&self) -> impl Iterator<Item = &HotkeyBinding> {
        self.0.iter().filter(|b| b.is_mouse())
    }

    /// `match_event`, restricted to the chords `accept` keeps.
    ///
    /// The PTY gate is per-chord, never per-action: an action bound to
    /// both `Ctrl+Shift+V` and `Ctrl+V` must keep the first inside a
    /// terminal while yielding the second to the shell. Deciding for
    /// the whole action would either steal `^V` (if any chord being
    /// safe rescued the rest) or kill `Ctrl+Shift+V` too (if any chord
    /// being a control sequence condemned the rest).
    pub fn match_event_where(
        &self,
        key: &Key,
        modifiers: &Modifiers,
        mut accept: impl FnMut(&HotkeyBinding) -> bool,
    ) -> Option<FamilyMatch> {
        self.0
            .iter()
            .filter(|b| accept(b))
            .find_map(|b| b.match_event(key, modifiers))
    }

    /// Serialize for the settings table: chords space-separated, each
    /// in `HotkeyBinding::serialize` form (`"ctrl+shift+v shift+ins"`).
    ///
    /// The separator is a space and must stay one: every other
    /// candidate is a bindable primary. `,` is `OpenSettings`'s factory
    /// chord and `;` `.` `=` `-` `+` `/` `\` `[` `]` are all in the
    /// `Punct` set, and all of them serialize literally. A space can
    /// never appear inside a chord, because `Named::Space` serializes
    /// to the word `"space"`.
    pub fn serialize(&self) -> String {
        if self.0.is_empty() {
            return UNBOUND.to_string();
        }
        self.0
            .iter()
            .map(|b| b.serialize())
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// Reverse of `serialize`. `None` means "no usable override", and
    /// the caller keeps the factory chords.
    ///
    /// Three cases the settings table can hold:
    ///
    /// * `""` is *no override* (what `ResetHotkey` writes to drop a
    ///   row back to factory), so it returns `None`.
    /// * `UNBOUND` is a *deliberate* unbind and returns an empty list.
    ///   It needs its own token precisely because `""` was already
    ///   taken: the single-binding model wrote `""` for both and could
    ///   not tell them apart, so an unbound action silently regained
    ///   its factory chord on the next boot.
    /// * anything else is a chord list.
    ///
    /// Unparseable chords are skipped rather than failing the whole
    /// row, so a value written by a newer build (one that knows a key
    /// name this build doesn't) degrades to the chords this build
    /// understands instead of resetting the action. A row where
    /// nothing parses at all is malformed, not an unbind.
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim();
        if s.is_empty() {
            return None;
        }
        if s == UNBOUND {
            return Some(Self::default());
        }
        let parsed = Self::many(s.split_whitespace().filter_map(HotkeyBinding::parse));
        if parsed.is_empty() {
            return None;
        }
        Some(parsed)
    }
}

impl<'a> IntoIterator for &'a HotkeyBindings {
    type Item = &'a HotkeyBinding;
    type IntoIter = std::slice::Iter<'a, HotkeyBinding>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

/// Map from action to its current chords (default or user override).
pub type HotkeyMap = HashMap<HotkeyAction, HotkeyBindings>;

/// Hardcoded factory defaults. Settings overrides land on top of
/// this map in `boot.rs::load_data_from_vault`.
///
/// macOS swaps Ctrl for Logo (Cmd) on the primary actions to match
/// the platform convention (Termius, VSCode, Safari all use Cmd
/// for new-tab / close-tab / find / etc on macOS). Modifier-only
/// fields are still settable in the editor so a user who wants
/// Ctrl-everywhere on macOS can rebind.
pub fn default_bindings() -> HotkeyMap {
    use HotkeyAction::*;
    use PrimaryKey::*;
    let mut m = HotkeyMap::new();
    let put = |m: &mut HotkeyMap, a, ctrl, shift, alt, logo, p| {
        m.insert(
            a,
            HotkeyBindings::single(HotkeyBinding {
                ctrl,
                shift,
                alt,
                logo,
                primary: p,
            }),
        );
    };
    // Same as `put`, for the actions that ship with alternates.
    let put_many = |m: &mut HotkeyMap, a, chords: &[(bool, bool, bool, bool, PrimaryKey)]| {
        m.insert(
            a,
            HotkeyBindings::many(chords.iter().map(|&(ctrl, shift, alt, logo, primary)| {
                HotkeyBinding {
                    ctrl,
                    shift,
                    alt,
                    logo,
                    primary,
                }
            })),
        );
    };
    // Platform-primary modifier: Cmd (logo) on macOS, Ctrl elsewhere.
    let mac = cfg!(target_os = "macos");
    let primary_ctrl = !mac;
    let primary_logo = mac;
    // Ctrl+Shift+T (Cmd+Shift+T on macOS): the terminal-world "new tab"
    // chord (GNOME Terminal / Windows Terminal), which is exactly what
    // the saved-host picker opens. NOT plain Ctrl+K as before: a bare
    // Ctrl+letter is a terminal control sequence (Ctrl+K = readline
    // kill-line) that is_terminal_control_sequence() leaves with the
    // PTY, so the picker was unreachable from inside a live terminal.
    // The Shift lifts it out of that gate.
    put(&mut m, ShowNewTabPicker, primary_ctrl, true, false, primary_logo, Char('t'));
    // Ctrl+Shift+J (Cmd+Shift+J on macOS), not plain Ctrl+J: a bare
    // Ctrl+letter is a terminal control sequence (Ctrl+J IS line feed,
    // emacs/readline accept-line) and is_terminal_control_sequence()
    // suppresses it inside the terminal view, which is exactly where
    // jumping between tabs is most useful (issue #100). The Shift lifts
    // it out of that gate, same rationale as OpenLocalShell below.
    put(&mut m, ShowTabJump, primary_ctrl, true, false, primary_logo, Char('j'));
    // Ctrl+Shift+P (Cmd+Shift+P on macOS), the VS Code convention. Plain
    // Ctrl+P is OpenPortForwards and a bare Ctrl+letter is a PTY control
    // sequence anyway; the Shift both frees it from that gate and clears
    // the OpenPortForwards binding (modifier match is exact).
    put(&mut m, ShowCommandPalette, primary_ctrl, true, false, primary_logo, Char('p'));
    // Ctrl+Shift+L (Cmd+Shift+L on macOS), not plain Ctrl+L: a bare
    // Ctrl+letter is a terminal control sequence (Ctrl+L = clear) and
    // is_terminal_control_sequence() suppresses it inside the terminal
    // view, which is exactly where opening a local shell is useful. The
    // Shift modifier lifts it out of that gate so the action fires
    // everywhere while plain Ctrl+L still reaches the PTY to clear.
    put(&mut m, OpenLocalShell, primary_ctrl, true, false, primary_logo, Char('l'));
    put(&mut m, NewWindow, primary_ctrl, true, false, primary_logo, Char('n'));
    // Ctrl+N (Cmd+N on macOS): the Termius new-host convention. A bare
    // Ctrl+letter, so inside a terminal it stays with the PTY (readline
    // next-history), like Ctrl+K / Ctrl+P above.
    put(&mut m, NewHost, primary_ctrl, false, false, primary_logo, Char('n'));
    // Ctrl+Shift+G (Cmd+Shift+G on macOS), issue #99. Moved off
    // Ctrl+Shift+T: that is the terminal-world "new tab" chord, which
    // the saved-host picker (the primary new-tab action) now owns; an
    // ad-hoc quick connect is the secondary path, so "G" for "go to a
    // host". NOT Q despite the "Quick" mnemonic: Cmd+Shift+Q is the
    // macOS system Log Out chord, and every other Oryxis default
    // teaches Cmd+Shift+<letter> muscle memory on the Mac, so binding
    // Quick there would invite the exact wrong-modifier slip into
    // logout. The Shift lifts it out of the terminal control-sequence
    // gate so it fires from inside a live terminal too.
    put(&mut m, ShowQuickConnect, primary_ctrl, true, false, primary_logo, Char('g'));
    // Ctrl+Shift+R (Cmd+Shift+R on macOS): reconnect the focused tab,
    // the browser-reload mnemonic. NOT plain Ctrl+R: that is readline
    // reverse-history-search, which the control-sequence gate rightly
    // leaves with the PTY; the Shift lifts the chord out of that gate
    // so it fires from inside a live terminal, where a dropped session
    // is actually noticed.
    put(&mut m, ReconnectTab, primary_ctrl, true, false, primary_logo, Char('r'));
    // Keychain pair on Ctrl+Shift (Cmd+Shift on macOS): Shift lifts
    // both out of the terminal control-sequence gate; K mirrors the
    // key mnemonic (exact-modifier matching keeps it clear of the
    // plain Ctrl+K picker), I the identity one.
    put(&mut m, NewKey, primary_ctrl, true, false, primary_logo, Char('k'));
    put(&mut m, NewIdentity, primary_ctrl, true, false, primary_logo, Char('i'));
    put(&mut m, CloseActiveTab, primary_ctrl, true, false, primary_logo, Char('w'));
    put(&mut m, OpenPortForwards, primary_ctrl, false, false, primary_logo, Char('p'));
    put(&mut m, OpenSettings, primary_ctrl, false, false, primary_logo, Punct(","));
    put(&mut m, FocusViewSearch, primary_ctrl, false, false, primary_logo, Char('f'));
    // Ctrl+Shift+E (Cmd+Shift+E on macOS): Shift lifts it out of the terminal
    // control-sequence gate, same rationale as OpenLocalShell. Configurable.
    put(&mut m, OpenSftp, primary_ctrl, true, false, primary_logo, Char('e'));
    put(&mut m, SwitchToTabSlot, primary_ctrl, false, false, primary_logo, Digit1to9);
    put(&mut m, CycleTabs, false, false, true, false, ArrowLeftRight);
    put(&mut m, ToggleFullscreen, false, false, false, false, Named(keyboard::key::Named::F11));
    put(&mut m, FontZoomIn, primary_ctrl, false, false, primary_logo, Punct("="));
    put(&mut m, FontZoomOut, primary_ctrl, false, false, primary_logo, Punct("-"));
    put(&mut m, FontZoomReset, primary_ctrl, false, false, primary_logo, Char('0'));
    // Terminal split panes. Ctrl+Shift (Cmd+Shift on macOS): Shift lifts
    // these out of the terminal control-sequence gate and the directional
    // arrows out of cursor-key reach. Vertical split is on D ("divide")
    // because Ctrl+Shift+E is the OpenSftp binding above; O keeps the
    // GNOME Terminal stacked-split convention.
    put(&mut m, SplitPaneVertical, primary_ctrl, true, false, primary_logo, Char('d'));
    put(&mut m, SplitPaneHorizontal, primary_ctrl, true, false, primary_logo, Char('o'));
    put(&mut m, FocusPaneLeft, primary_ctrl, true, false, primary_logo, Named(keyboard::key::Named::ArrowLeft));
    put(&mut m, FocusPaneRight, primary_ctrl, true, false, primary_logo, Named(keyboard::key::Named::ArrowRight));
    put(&mut m, FocusPaneUp, primary_ctrl, true, false, primary_logo, Named(keyboard::key::Named::ArrowUp));
    put(&mut m, FocusPaneDown, primary_ctrl, true, false, primary_logo, Named(keyboard::key::Named::ArrowDown));
    // Ctrl+Shift+Z (Cmd+Shift+Z on macOS): Z for zoom, the tmux
    // `prefix z` convention for exactly this toggle. Shift lifts it out
    // of the terminal control-sequence gate.
    put(&mut m, ToggleMaximizePane, primary_ctrl, true, false, primary_logo, Char('z'));
    // Ctrl+Shift+H (Cmd+Shift+H on macOS): Shift lifts it out of the
    // terminal control-sequence gate (plain Ctrl+H is backspace on the
    // PTY), H for the History/lists sidebar. Rebindable like the rest.
    put(&mut m, FocusSidebarList, primary_ctrl, true, false, primary_logo, Char('h'));
    // Ctrl+Shift+B (Cmd+Shift+B on macOS): the VS Code toggle-sidebar
    // convention; Shift lifts it out of the control-sequence gate.
    put(&mut m, ToggleSidebar, primary_ctrl, true, false, primary_logo, Char('b'));
    // Ctrl+Shift+F (Cmd+Shift+F on macOS): flip the focused SSH tab
    // between Terminal and Files. Shift lifts it out of the terminal
    // control-sequence gate (plain Ctrl+F is readline forward-char /
    // the app's FocusViewSearch elsewhere).
    put(&mut m, ToggleTabFiles, primary_ctrl, true, false, primary_logo, Char('f'));
    // Ctrl+Shift+U (Cmd+Shift+U on macOS): arm / disarm broadcast input
    // across the tab's panes. Shift lifts it out of the terminal
    // control-sequence gate (plain Ctrl+U is readline kill-line).
    put(&mut m, ToggleBroadcastInput, primary_ctrl, true, false, primary_logo, Char('u'));
    // Ctrl+Shift+M (Cmd+Shift+M on macOS): Privacy Mode session
    // override (issue #78). Shift lifts it out of the terminal
    // control-sequence gate (plain Ctrl+M IS carriage return). Users
    // who want an F-key can rebind (F11=fullscreen is the precedent);
    // F1-F10 stay with TUI apps by default.
    put(&mut m, TogglePrivacyMode, primary_ctrl, true, false, primary_logo, Char('m'));
    // Terminal clipboard (#75). Deliberately NOT built from the
    // primary_ctrl / primary_logo pair: the platforms disagree on more
    // than which modifier to use. Elsewhere the convention is
    // Ctrl+Shift+X, because plain Ctrl+X is a control sequence the
    // shell wants (is_terminal_control_sequence). macOS has no such
    // idiom and uses bare Cmd.
    //
    // Paste also ships Shift+Insert, and only paste. That chord has real
    // pedigree (X11 tradition; PuTTY documents it as an alternative to
    // its right-click paste) and it is modifier-neutral, so it rides
    // along on macOS rather than being dropped. There is deliberately NO
    // Ctrl+Insert copy to match it: the symmetry is tempting and wrong.
    // PuTTY's own docs say copy is mouse-only by default, and GNOME
    // Terminal documents only Ctrl+Shift+C. Anyone who wants it can bind
    // it, which is the whole point of this table.
    //
    // Plain Ctrl+V is NOT a paste chord. It is the literal-next byte
    // (vim visual block, readline quoted-insert), and binding it here
    // would take it away from the PTY with no way back.
    if mac {
        put_many(&mut m, TerminalCopy, &[(false, false, false, true, Char('c'))]);
        put_many(&mut m, TerminalPaste, &[(false, false, false, true, Char('v'))]);
        put_many(&mut m, TerminalSelectAll, &[(false, false, false, true, Char('a'))]);
    } else {
        put_many(&mut m, TerminalCopy, &[(true, true, false, false, Char('c'))]);
        put_many(&mut m, TerminalPaste, &[(true, true, false, false, Char('v'))]);
        put_many(&mut m, TerminalSelectAll, &[(true, true, false, false, Char('a'))]);
    }
    // Shift+Insert pastes the PRIMARY selection, not the clipboard: the
    // xterm default, kitty's `paste_from_selection` and Alacritty's
    // `PasteSelection` all bind exactly this chord to exactly this
    // buffer, so this IS the convention, on every platform (our PRIMARY
    // is app-internal, so it exists off X11 too). The widget falls back
    // to a clipboard paste when the pane never had a selection or under
    // copy_on_select (the PuTTY single-buffer model), so the chord
    // still always pastes something, matching what it did while it was
    // a plain paste chord.
    //
    // The middle mouse button is the SECOND factory input, and it is an
    // ordinary chord in this list rather than a separate setting: the
    // binding table is the single authority for the gesture, so
    // rebinding it (or moving it onto a thumb button) is the same edit
    // as any other. Settings > Terminal's "middle-click paste" toggle is
    // a shortcut for adding / removing exactly this chord.
    put_many(
        &mut m,
        TerminalPasteSelection,
        &[
            (false, true, false, false, Named(keyboard::key::Named::Insert)),
            (false, false, false, false, Mouse(MouseButton::Middle)),
        ],
    );
    // Scrollback paging. Shift+PageUp/PageDown on every platform: it is
    // universal across terminals and macOS has no competing idiom. The
    // widget yields these to the PTY on the alternate screen, where
    // there is no scrollback and the running app owns the key.
    put(&mut m, ScrollbackPageUp, false, true, false, false, Named(keyboard::key::Named::PageUp));
    put(&mut m, ScrollbackPageDown, false, true, false, false, Named(keyboard::key::Named::PageDown));
    // Ctrl+Shift+digit (Cmd+Shift on macOS): the vault-section
    // jump family, one digit per burger-menu VAULT entry. Shift
    // keeps it clear of the Ctrl+digit tab slots.
    put(&mut m, VaultSectionSlot, primary_ctrl, true, false, primary_logo, Digit1to9);
    // Vault section cycling. Plain Ctrl on every platform (the
    // browser/IDE tab-strip convention; Cmd+PageUp/Down has no macOS
    // precedent), rebindable like everything else.
    put(&mut m, VaultSectionPrev, true, false, false, false, Named(keyboard::key::Named::PageUp));
    put(&mut m, VaultSectionNext, true, false, false, false, Named(keyboard::key::Named::PageDown));
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_serialize_parse() {
        let defaults = default_bindings();
        for binds in defaults.values() {
            // Every chord on its own.
            for binding in binds.iter() {
                let s = binding.serialize();
                let parsed = HotkeyBinding::parse(&s)
                    .unwrap_or_else(|| panic!("parse failed for {s}"));
                assert_eq!(
                    *binding, parsed,
                    "round-trip mismatch for {s}: {binding:?} != {parsed:?}"
                );
            }
            // And the whole list, which is what the settings row holds.
            let s = binds.serialize();
            let parsed = HotkeyBindings::parse(&s)
                .unwrap_or_else(|| panic!("list parse failed for {s:?}"));
            assert_eq!(*binds, parsed, "list round-trip mismatch for {s:?}");
        }
    }

    /// A chord must never serialize to something that the list
    /// separator (a space) would split. If this ever fails, a new
    /// `PrimaryKey` is serializing to a multi-word token and every
    /// stored multi-chord row silently loses chords.
    #[test]
    fn no_default_chord_serializes_with_a_space() {
        for binds in default_bindings().values() {
            for b in binds.iter() {
                let s = b.serialize();
                assert!(
                    !s.contains(char::is_whitespace),
                    "chord {b:?} serializes to {s:?}, which the list separator would split"
                );
            }
        }
    }

    #[test]
    fn vault_section_defaults_parse_from_settings_strings() {
        // The serialized form users end up with in the settings table
        // must parse back to the exact default bindings.
        let defaults = default_bindings();
        for (action, expected) in [
            (HotkeyAction::VaultSectionPrev, "ctrl+pgup"),
            (HotkeyAction::VaultSectionNext, "ctrl+pgdn"),
        ] {
            let b = defaults.get(&action).expect("default missing");
            assert_eq!(b.serialize(), expected);
            assert_eq!(HotkeyBindings::parse(expected).as_ref(), Some(b));
        }
    }

    /// The three settings-row states are distinct. `""` (no override)
    /// must not read as an unbind, or `ResetHotkey` would unbind instead
    /// of restoring the factory chord; `UNBOUND` must not read as "no
    /// override", or an unbind would silently come back on next boot,
    /// which is exactly what the single-binding model did.
    #[test]
    fn empty_row_is_not_an_unbind() {
        assert_eq!(HotkeyBindings::parse(""), None);
        assert_eq!(HotkeyBindings::parse("   "), None);
        assert_eq!(HotkeyBindings::parse(UNBOUND), Some(HotkeyBindings::default()));
        assert_eq!(HotkeyBindings::default().serialize(), UNBOUND);
        // Round-trips as a real state, not by accident.
        assert_eq!(
            HotkeyBindings::parse(&HotkeyBindings::default().serialize()),
            Some(HotkeyBindings::default())
        );
        // A row that parses to nothing usable is malformed, not an
        // unbind: the caller keeps the factory chords.
        assert_eq!(HotkeyBindings::parse("wat nonsense"), None);
    }

    /// A row written by a newer build (one that knows a key name this
    /// build doesn't) degrades to the chords this build understands
    /// rather than resetting the action to factory.
    #[test]
    fn unknown_chord_in_a_row_is_skipped_not_fatal() {
        let parsed = HotkeyBindings::parse("ctrl+shift+v futurekey shift+ins")
            .expect("should keep the chords it understands");
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed.serialize(), "ctrl+shift+v shift+ins");
    }

    /// The chords #73 asked for. `Shift+Insert` (paste) and
    /// `Shift+PageUp` (scrollback) carry no Ctrl/Alt/Logo, so they only
    /// clear `is_safe` because Shift counts on a primary that can't be
    /// typed. If that regresses, the capture editor silently rejects the
    /// chord every other terminal ships.
    #[test]
    fn shift_plus_non_text_key_is_bindable() {
        let shift_ins = HotkeyBinding {
            ctrl: false,
            shift: true,
            alt: false,
            logo: false,
            primary: PrimaryKey::Named(Named::Insert),
        };
        assert!(shift_ins.is_safe());
        assert_eq!(shift_ins.serialize(), "shift+ins");
        assert_eq!(HotkeyBinding::parse("shift+ins"), Some(shift_ins));
        // Never suppressed as a shell control sequence (no Ctrl).
        assert!(!shift_ins.is_terminal_control_sequence());

        let shift_pgup = HotkeyBinding {
            primary: PrimaryKey::Named(Named::PageUp),
            ..shift_ins
        };
        assert!(shift_pgup.is_safe());

        // Shift alone on a typable primary is still just uppercase.
        let shift_a = HotkeyBinding {
            primary: PrimaryKey::Char('a'),
            ..shift_ins
        };
        assert!(!shift_a.is_safe());
        // Modifier-free navigation keys stay unbindable: the PTY wants
        // them, and bare Delete is the capture editor's remove gesture.
        let bare_del = HotkeyBinding {
            shift: false,
            primary: PrimaryKey::Named(Named::Delete),
            ..shift_ins
        };
        assert!(!bare_del.is_safe());
    }

    /// Factory paste must carry BOTH conventional chords, and must not
    /// carry plain Ctrl+V: that is the shell's literal-next byte (vim
    /// visual block, readline quoted-insert), and binding it would take
    /// it from the PTY with no way back.
    #[test]
    fn paste_defaults_follow_the_terminal_convention() {
        let defaults = default_bindings();
        let paste = defaults.get(&HotkeyAction::TerminalPaste).expect("paste bound");
        let ctrl_v = HotkeyBinding {
            ctrl: true,
            shift: false,
            alt: false,
            logo: false,
            primary: PrimaryKey::Char('v'),
        };
        assert!(!paste.contains(&ctrl_v), "Ctrl+V must stay with the PTY");
        // Shift+Insert is NOT a clipboard-paste chord: xterm binds it to
        // the PRIMARY selection, kitty ships it as `paste_from_selection`
        // and Alacritty as `PasteSelection`, so it belongs to the
        // separate `TerminalPasteSelection` action. Leaving it on plain
        // paste as well would double-fire the chord (the widget resolver
        // and the app router both matching it).
        let shift_ins = HotkeyBinding {
            ctrl: false,
            shift: true,
            alt: false,
            logo: false,
            primary: PrimaryKey::Named(Named::Insert),
        };
        assert!(!paste.contains(&shift_ins), "Shift+Insert belongs to PasteSelection");
        assert_eq!(paste.len(), 1);
        let paste_sel = defaults
            .get(&HotkeyAction::TerminalPasteSelection)
            .expect("paste-selection bound");
        assert!(paste_sel.contains(&shift_ins), "the xterm/kitty/Alacritty chord");
        // Plus the X11 middle click, the gesture's other half. It is a
        // chord in this list rather than a setting of its own, which is
        // what makes it rebindable in Settings > Shortcuts.
        assert!(
            paste_sel.contains(&middle_click_chord()),
            "middle click is a factory input for PRIMARY paste"
        );
        assert_eq!(paste_sel.len(), 2);

        // Shift+Insert paste has real pedigree (X11; PuTTY documents it).
        // A matching Ctrl+Insert copy does NOT: PuTTY's docs say copy is
        // mouse-only by default and GNOME Terminal documents only
        // Ctrl+Shift+C. Shipping it "for symmetry" would be inventing a
        // convention. It stays bindable, just not factory.
        let copy = defaults.get(&HotkeyAction::TerminalCopy).expect("copy bound");
        let ctrl_ins = HotkeyBinding {
            ctrl: true,
            shift: false,
            alt: false,
            logo: false,
            primary: PrimaryKey::Named(Named::Insert),
        };
        assert!(!copy.contains(&ctrl_ins), "Ctrl+Insert copy is not a real convention");
        assert_eq!(copy.len(), 1);

        // No factory chord anywhere is a bare Ctrl+letter that the shell
        // would want, which is what keeps the whole clipboard set clear
        // of the is_terminal_control_sequence gate.
        for action in [
            HotkeyAction::TerminalCopy,
            HotkeyAction::TerminalPaste,
            HotkeyAction::TerminalSelectAll,
            HotkeyAction::ScrollbackPageUp,
            HotkeyAction::ScrollbackPageDown,
        ] {
            for b in defaults.get(&action).expect("bound").iter() {
                assert!(
                    !b.is_terminal_control_sequence(),
                    "{action:?} chord {b:?} would be swallowed by the PTY gate"
                );
                assert!(b.is_safe(), "{action:?} chord {b:?} is not recordable");
            }
        }
    }

    /// Every action must be reachable from the editor, and every stored
    /// row keyed by a stable id. A new action that forgets `all()` is
    /// invisible in Settings and the palette; one that forgets `id()`
    /// panics. Cheap guard, since both are hand-maintained tables.
    #[test]
    fn every_action_is_listed_and_has_a_unique_id() {
        let all = HotkeyAction::all();
        let mut ids: Vec<&str> = all.iter().map(|a| a.id()).collect();
        ids.sort_unstable();
        let before = ids.len();
        ids.dedup();
        assert_eq!(before, ids.len(), "duplicate hotkey id");
        for a in [
            HotkeyAction::TerminalCopy,
            HotkeyAction::TerminalPaste,
            HotkeyAction::TerminalPasteSelection,
            HotkeyAction::TerminalSelectAll,
            HotkeyAction::ScrollbackPageUp,
            HotkeyAction::ScrollbackPageDown,
        ] {
            assert!(all.contains(&a), "{a:?} missing from all()");
            assert!(a.terminal_only(), "{a:?} must not fire outside a terminal");
        }
    }

    #[test]
    fn paste_selection_is_widget_dispatched_and_listed() {
        assert!(HotkeyAction::all().contains(&HotkeyAction::TerminalPasteSelection));
        // Performed by the widget (PRIMARY lives in the canvas state), so
        // the command palette must not list it: a palette row dispatches
        // `RunHotkeyAction`, which never reaches the canvas, and the row
        // would silently do nothing.
        assert!(HotkeyAction::TerminalPasteSelection.widget_dispatched());
        // Paste, by contrast, IS dispatched app-side (it has to reach an
        // SSH session), so it stays clickable in the palette. Guards the
        // pair against being lumped together by a later edit.
        assert!(!HotkeyAction::TerminalPaste.widget_dispatched());
    }

    #[test]
    fn family_match_extracts_digit() {
        let b = HotkeyBinding {
            ctrl: true,
            shift: false,
            alt: false,
            logo: false,
            primary: PrimaryKey::Digit1to9,
        };
        let mods = Modifiers::CTRL;
        let key = Key::Character("3".into());
        assert_eq!(b.match_event(&key, &mods), Some(FamilyMatch::Digit(3)));
        let bad = Key::Character("0".into());
        assert_eq!(b.match_event(&bad, &mods), None);
    }

    #[test]
    fn family_match_extracts_arrow() {
        let b = HotkeyBinding {
            ctrl: false,
            shift: false,
            alt: true,
            logo: false,
            primary: PrimaryKey::ArrowLeftRight,
        };
        let mods = Modifiers::ALT;
        assert_eq!(
            b.match_event(&Key::Named(Named::ArrowRight), &mods),
            Some(FamilyMatch::ArrowRight),
        );
        assert_eq!(
            b.match_event(&Key::Named(Named::ArrowLeft), &mods),
            Some(FamilyMatch::ArrowLeft),
        );
        assert_eq!(b.match_event(&Key::Named(Named::ArrowUp), &mods), None);
    }

    #[test]
    fn shift_diff_blocks_match() {
        // Ctrl+K binding should NOT fire on Ctrl+Shift+K, the editor
        // exact-matches modifiers so the two combos can be bound to
        // different actions.
        let b = HotkeyBinding {
            ctrl: true,
            shift: false,
            alt: false,
            logo: false,
            primary: PrimaryKey::Char('k'),
        };
        assert_eq!(
            b.match_event(&Key::Character("k".into()), &(Modifiers::CTRL | Modifiers::SHIFT)),
            None
        );
        assert_eq!(
            b.match_event(&Key::Character("k".into()), &Modifiers::CTRL),
            Some(FamilyMatch::Plain)
        );
    }

    #[test]
    fn punct_keys_are_not_terminal_control_unless_c0() {
        // Regression: Ctrl+, / Ctrl+= / Ctrl+- used to be silently
        // suppressed inside the terminal view because the gate
        // accepted every Punct. They map to no control byte; the
        // default bindings (OpenSettings, FontZoomIn, FontZoomOut)
        // must fire even when the focus is on the embedded terminal.
        for &p in &[",", "=", "-", ".", ";", "/"] {
            let b = HotkeyBinding {
                ctrl: true,
                shift: false,
                alt: false,
                logo: false,
                primary: PrimaryKey::Punct(p),
            };
            assert!(
                !b.is_terminal_control_sequence(),
                "Ctrl+{p} should not be a terminal control sequence"
            );
        }
    }

    #[test]
    fn punct_keys_that_map_to_c0_are_terminal_control() {
        // Ctrl+[ = ESC, Ctrl+\ = FS, Ctrl+] = GS are real C0 escapes
        // a shell consumes via the tty layer, so the dispatcher should
        // continue to suppress them inside the terminal view.
        for &p in &["[", "\\", "]"] {
            let b = HotkeyBinding {
                ctrl: true,
                shift: false,
                alt: false,
                logo: false,
                primary: PrimaryKey::Punct(p),
            };
            assert!(
                b.is_terminal_control_sequence(),
                "Ctrl+{p} should be a terminal control sequence"
            );
        }
    }

    #[test]
    fn safe_requires_modifier_or_function_key() {
        let unsafe_binding = HotkeyBinding {
            ctrl: false,
            shift: false,
            alt: false,
            logo: false,
            primary: PrimaryKey::Char('a'),
        };
        assert!(!unsafe_binding.is_safe());

        let f_key = HotkeyBinding {
            ctrl: false,
            shift: false,
            alt: false,
            logo: false,
            primary: PrimaryKey::Named(Named::F11),
        };
        assert!(f_key.is_safe());

        let ctrl_a = HotkeyBinding {
            ctrl: true,
            shift: false,
            alt: false,
            logo: false,
            primary: PrimaryKey::Char('a'),
        };
        assert!(ctrl_a.is_safe());
    }

    /// Does the Insert chord actually FIRE? `is_safe` / `parse` passing
    /// only proves the editor would accept the chord, not that a real
    /// key event reaches the action. Since the Shift+Insert move to
    /// PasteSelection this also pins that plain paste does NOT match it,
    /// or one keystroke would fire both actions.
    #[test]
    fn insert_chords_match_a_real_key_event() {
        let defaults = default_bindings();
        let ins = Key::Named(Named::Insert);

        let mut m = Modifiers::default();
        m.set(Modifiers::SHIFT, true);
        let paste_sel = defaults
            .get(&HotkeyAction::TerminalPasteSelection)
            .expect("bound");
        assert_eq!(
            paste_sel.match_event(&ins, &m),
            Some(FamilyMatch::Plain),
            "Shift+Insert must fire the PRIMARY paste"
        );
        let paste = defaults.get(&HotkeyAction::TerminalPaste).expect("bound");
        assert_eq!(
            paste.match_event(&ins, &m),
            None,
            "plain paste must not shadow the PasteSelection chord"
        );

        // Modifier match is exact: a bare Insert fires nothing.
        let none = Modifiers::default();
        assert_eq!(paste_sel.match_event(&ins, &none), None);
    }

    fn mouse(button: MouseButton) -> HotkeyBinding {
        HotkeyBinding {
            ctrl: false,
            shift: false,
            alt: false,
            logo: false,
            primary: PrimaryKey::Mouse(button),
        }
    }

    /// Every bindable button round-trips through the settings table,
    /// modifiers included. A regression here silently drops a user's
    /// mouse binding on the next boot.
    #[test]
    fn mouse_bindings_round_trip() {
        for button in [
            MouseButton::Middle,
            MouseButton::Back,
            MouseButton::Forward,
            MouseButton::Other(8),
            MouseButton::Other(0),
        ] {
            let bare = mouse(button);
            let s = bare.serialize();
            assert!(!s.contains(char::is_whitespace), "{s:?} would split the list");
            assert_eq!(HotkeyBinding::parse(&s), Some(bare), "bare {button:?}");

            let modded = HotkeyBinding { ctrl: true, shift: true, ..bare };
            let s = modded.serialize();
            assert_eq!(HotkeyBinding::parse(&s), Some(modded), "modified {button:?}");
        }
        // And as part of a multi-chord row, which is how paste-selection
        // actually stores it.
        let row = HotkeyBindings::many([
            HotkeyBinding {
                ctrl: false,
                shift: true,
                alt: false,
                logo: false,
                primary: PrimaryKey::Named(Named::Insert),
            },
            middle_click_chord(),
        ]);
        assert_eq!(row.serialize(), "shift+ins mouse_middle");
        assert_eq!(HotkeyBindings::parse("shift+ins mouse_middle"), Some(row));
    }

    /// The `mouse_` prefix is the whole reason `parse` can try buttons
    /// first. If a named key or punctuation token ever collided, one of
    /// the two would become unparseable.
    #[test]
    fn mouse_tokens_never_collide_with_key_primaries() {
        for token in ["mouse_middle", "mouse_back", "mouse_forward", "mouse_8"] {
            assert!(str_to_named(token).is_none(), "{token} shadows a named key");
            assert!(
                matches!(
                    HotkeyBinding::parse(token).map(|b| b.primary),
                    Some(PrimaryKey::Mouse(_))
                ),
                "{token} must parse as a mouse button"
            );
        }
        // Not every `mouse_*` string is a button: an unknown suffix is
        // malformed, not a silent Other(0).
        assert_eq!(MouseButton::parse_token("mouse_wat"), None);
        assert_eq!(MouseButton::parse_token("middle"), None);
    }

    /// Left and Right stay with the terminal canvas (select / the
    /// right-click scheme), so neither can ever become a binding.
    #[test]
    fn left_and_right_are_never_bindable() {
        let none = Modifiers::default();
        for button in [iced::mouse::Button::Left, iced::mouse::Button::Right] {
            assert_eq!(MouseButton::from_iced(button), None, "{button:?}");
            assert_eq!(binding_from_mouse(button, &none), None, "{button:?}");
        }
        assert_eq!(
            binding_from_mouse(iced::mouse::Button::Middle, &none),
            Some(middle_click_chord())
        );
    }

    /// A bare mouse binding needs no modifier (it can't be typed), and
    /// modifier matching is exact in both directions.
    #[test]
    fn mouse_matching_is_modifier_exact() {
        let bare = mouse(MouseButton::Middle);
        assert!(bare.is_safe(), "a mouse button is a chord on its own");
        // Never suppressed as a shell control sequence, whatever the
        // modifiers: the PTY has no byte for a mouse binding.
        assert!(!bare.is_terminal_control_sequence());
        assert!(!HotkeyBinding { ctrl: true, ..bare }.is_terminal_control_sequence());

        let none = Modifiers::default();
        let mut ctrl = Modifiers::default();
        ctrl.set(Modifiers::CTRL, true);

        assert!(bare.match_mouse(MouseButton::Middle, &none));
        assert!(!bare.match_mouse(MouseButton::Middle, &ctrl), "Ctrl+Middle is a different binding");
        assert!(!bare.match_mouse(MouseButton::Back, &none), "wrong button");

        let ctrl_middle = HotkeyBinding { ctrl: true, ..bare };
        assert!(ctrl_middle.match_mouse(MouseButton::Middle, &ctrl));
        assert!(!ctrl_middle.match_mouse(MouseButton::Middle, &none));
    }

    /// The two halves never cross: no keystroke fires a mouse binding,
    /// and no button fires a chord.
    #[test]
    fn mouse_and_keyboard_bindings_never_cross() {
        let none = Modifiers::default();
        let bare = HotkeyBindings::single(mouse(MouseButton::Middle));
        for key in [Key::Named(Named::Insert), Key::Character("v".into())] {
            assert_eq!(bare.match_event(&key, &none), None, "{key:?}");
        }
        let defaults = default_bindings();
        let copy = defaults.get(&HotkeyAction::TerminalCopy).expect("bound");
        assert!(!copy.match_mouse(MouseButton::Middle, &none));
    }

    /// Side buttons are free window-wide, so they bind to anything with
    /// an editable primary. The WHEEL CLICK is only ever read inside the
    /// canvas, so it stays on terminal actions: offering it elsewhere
    /// would record a binding that could never fire.
    #[test]
    fn side_buttons_bind_anywhere_the_wheel_click_does_not() {
        assert!(MouseButton::Back.is_side_button());
        assert!(MouseButton::Forward.is_side_button());
        assert!(MouseButton::Other(8).is_side_button());
        assert!(!MouseButton::Middle.is_side_button());

        for action in HotkeyAction::all() {
            assert_eq!(action.accepts_mouse(), action.primary_editable(), "{}", action.id());
            for side in [MouseButton::Back, MouseButton::Forward, MouseButton::Other(9)] {
                assert_eq!(
                    action.accepts_mouse_button(side),
                    action.primary_editable(),
                    "{} / {side:?}",
                    action.id()
                );
            }
            assert_eq!(
                action.accepts_mouse_button(MouseButton::Middle),
                action.primary_editable() && action.terminal_only(),
                "{}",
                action.id()
            );
        }
        // A family action has no primary slot for a button at all.
        assert!(!HotkeyAction::SwitchToTabSlot.accepts_mouse());
        assert!(!HotkeyAction::SwitchToTabSlot.accepts_mouse_button(MouseButton::Back));
        // The two concrete cases the design turns on.
        assert!(HotkeyAction::TerminalPasteSelection.accepts_mouse_button(MouseButton::Middle));
        assert!(HotkeyAction::CloseActiveTab.accepts_mouse_button(MouseButton::Back));
        assert!(!HotkeyAction::CloseActiveTab.accepts_mouse_button(MouseButton::Middle));
    }

    /// Exactly one layer claims each (action, button) pair. Both the
    /// widget resolver and the global press handler gate on this, so a
    /// pair claimed twice would fire twice and a pair claimed by neither
    /// would be a dead button.
    #[test]
    fn every_mouse_binding_has_exactly_one_owner() {
        use MouseBindingOwner::*;
        for action in HotkeyAction::all() {
            for button in [
                MouseButton::Middle,
                MouseButton::Back,
                MouseButton::Forward,
                MouseButton::Other(8),
            ] {
                let owner = action.mouse_binding_owner(button);
                // The canvas-state gestures are never the app's, at any
                // button: `RunHotkeyAction` only swallows them.
                if action.widget_dispatched() {
                    assert_eq!(owner, Widget, "{} / {button:?}", action.id());
                }
                // The wheel click is only ever readable over the canvas.
                if !button.is_side_button() {
                    assert_eq!(owner, Widget, "{} / {button:?}", action.id());
                }
                // A side button on a non-canvas action must reach the
                // app, or binding one outside the terminal is a no-op.
                if button.is_side_button() && !action.widget_dispatched() {
                    assert_eq!(owner, App, "{} / {button:?}", action.id());
                }
            }
        }
        // The case the whole split exists for: Back closes a tab from
        // anywhere, and it is the app that runs it.
        assert_eq!(
            HotkeyAction::CloseActiveTab.mouse_binding_owner(MouseButton::Back),
            App
        );
        assert_eq!(
            HotkeyAction::TerminalCopy.mouse_binding_owner(MouseButton::Back),
            Widget
        );
    }

    /// Every factory mouse chord sits on an action that accepts one,
    /// and `mouse_chords` sees exactly those.
    #[test]
    fn factory_mouse_chords_are_on_mouse_capable_actions() {
        let defaults = default_bindings();
        let mut found = 0;
        for (action, binds) in defaults.iter() {
            for chord in binds.mouse_chords() {
                found += 1;
                let PrimaryKey::Mouse(button) = chord.primary else {
                    unreachable!("mouse_chords yielded a keyboard chord")
                };
                assert!(
                    action.accepts_mouse_button(button),
                    "{} ships a mouse chord it can never fire",
                    action.id()
                );
            }
        }
        assert_eq!(found, 1, "middle-click paste is the only factory gesture");
    }
}
