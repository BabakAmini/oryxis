//! Host editor: the Terminal card (theme / icon / encoding / TERM
//! appearance tile, session logging, privacy-mode override).
use super::*;
use iced::widget::column;

impl Oryxis {
    pub(super) fn hp_appearance_items(&self) -> Element<'_, Message> {
        // ── Section: Terminal appearance ──
        // A single "click to open picker" tile that mirrors the
        // current pick (palette swatches if a specific theme is set,
        // a plain "inherit" row otherwise). The full picker lives in
        // its own modal so this section stays compact.
        // Themed preview tile: shows the chosen per-host palette, or the
        // inherited global theme when there's no override, so the row is
        // always a real preview instead of a bare "use global" dropdown.
        // Click opens the full picker modal.
        // Resolve the override (built-in OR custom) to a palette for the
        // preview swatch; fall back to the inherited global when there's no
        // override (or the named custom theme was deleted).
        let override_name = self
            .editor_form
            .terminal_theme
            .as_deref()
            .filter(|name| self.terminal_palette_for_name(name).is_some());
        let (preview_palette, theme_label) = match override_name {
            Some(name) => (
                self.terminal_palette_for_name(name).unwrap(),
                name.to_string(),
            ),
            None => (
                self.resolve_global_terminal_palette(),
                format!(
                    "{} ({})",
                    crate::i18n::t("terminal_theme_inherit_global"),
                    self.resolve_global_terminal_theme_name()
                ),
            ),
        };
        let theme_trigger: Element<'_, Message> = self.panel_nav_slot(
            crate::keynav::RowAction::activate(Message::EditorOpenThemePicker),
            8.0,
            terminal_theme_trigger(preview_palette, theme_label),
        );

        // Per-host icon shape override. The "Use default" entry maps to
        // an empty string which clears the override (resolved to the
        // global default_host_icon at render time).
        // Tokens drive the picker value (same pattern as Settings
        // -> Interface). Empty string is the "use default" token; the
        // dispatcher treats it as a None override on the form field.
        let icon_options = vec![
            String::new(),
            "circular".to_string(),
            "square".to_string(),
            "rounded".to_string(),
            "outline".to_string(),
            "initials".to_string(),
        ];
        let icon_selected = self.editor_form.icon_style.clone().unwrap_or_default();
        let icon_picker = pick_list(
            Some(icon_selected),
            icon_options,
            |s: &String| {
                let key = match s.as_str() {
                    "circular" => "icon_circular",
                    "square" => "icon_square",
                    "rounded" => "icon_rounded",
                    "outline" => "icon_outline",
                    "initials" => "icon_initials",
                    _ => "icon_use_default",
                };
                crate::i18n::t(key).to_string()
            },
        )
        .on_select(Message::EditorIconStyleChanged)
        .id(iced::widget::Id::new("editor-pick-icon-style"))
        .on_open(Message::Navigation(NavigationMessage::PickOpenChanged(true)))
        .on_close(Message::Navigation(NavigationMessage::PickOpenChanged(false)))
        .width(170)
        .padding(10)
        .style(crate::widgets::rounded_pick_list_style);
        // Focusable select (Tab + Enter/Space, widget-owned keys).
        let icon_row: Element<'_, Message> = dir_row(vec![
            text(crate::i18n::t("host_icon_style")).size(13).color(OryxisColors::t().text_secondary).into(),
            Space::new().width(Length::Fill).into(),
            self.panel_nav_slot(
                crate::keynav::RowAction::input(iced::widget::Id::new("editor-pick-icon-style")),
                crate::widgets::INPUT_RADIUS,
                icon_picker.into(),
            ),
        ]).align_y(iced::Alignment::Center).into();

        // Per-host terminal encoding. "UTF-8" is the default (stored as
        // None); the rest are encoding_rs labels the SSH engine transcodes.
        let encoding_options: Vec<String> = [
            "UTF-8", "Big5", "GBK", "gb18030", "Shift_JIS", "EUC-JP",
            "EUC-KR", "ISO-8859-1", "ISO-8859-15", "windows-1251",
            "windows-1252", "KOI8-R",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let encoding_selected = self
            .editor_form
            .encoding
            .clone()
            .unwrap_or_else(|| "UTF-8".to_string());
        let encoding_picker = pick_list(Some(encoding_selected), encoding_options, |s: &String| s.clone())
            .on_select(Message::EditorEncodingChanged)
            .id(iced::widget::Id::new("editor-pick-encoding"))
            .on_open(Message::Navigation(NavigationMessage::PickOpenChanged(true)))
            .on_close(Message::Navigation(NavigationMessage::PickOpenChanged(false)))
            .width(170)
            .padding(10)
            .style(crate::widgets::rounded_pick_list_style);
        // Focusable select, same treatment as the icon row.
        let encoding_row: Element<'_, Message> = dir_row(vec![
            text(crate::i18n::t("host_encoding")).size(13).color(OryxisColors::t().text_secondary).into(),
            Space::new().width(Length::Fill).into(),
            self.panel_nav_slot(
                crate::keynav::RowAction::input(iced::widget::Id::new("editor-pick-encoding")),
                crate::widgets::INPUT_RADIUS,
                encoding_picker.into(),
            ),
        ]).align_y(iced::Alignment::Center).into();

        // Per-host TERM. "xterm-256color" is the default (stored as None);
        // the rest are fallbacks for hosts whose terminfo trips on it.
        let term_options: Vec<String> = [
            "xterm-256color", "xterm", "screen-256color", "tmux-256color",
            "screen", "linux", "vt220", "vt100", "ansi",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let term_selected = self
            .editor_form
            .terminal_type
            .clone()
            .unwrap_or_else(|| "xterm-256color".to_string());
        let term_picker = pick_list(Some(term_selected), term_options, |s: &String| s.clone())
            .on_select(Message::EditorTerminalTypeChanged)
            .id(iced::widget::Id::new("editor-pick-term"))
            .on_open(Message::Navigation(NavigationMessage::PickOpenChanged(true)))
            .on_close(Message::Navigation(NavigationMessage::PickOpenChanged(false)))
            .width(170)
            .padding(10)
            .style(crate::widgets::rounded_pick_list_style);
        // Focusable select, same treatment as the icon row.
        let term_row: Element<'_, Message> = dir_row(vec![
            text(crate::i18n::t("host_terminal_type")).size(13).color(OryxisColors::t().text_secondary).into(),
            Space::new().width(Length::Fill).into(),
            self.panel_nav_slot(
                crate::keynav::RowAction::input(iced::widget::Id::new("editor-pick-term")),
                crate::widgets::INPUT_RADIUS,
                term_picker.into(),
            ),
        ]).align_y(iced::Alignment::Center).into();

        // Terminal card body: the theme keeps its full-width preview tile
        // (it's a live swatch, not a plain dropdown); icon and encoding
        // are compact inline rows (label left, picker right) like Auth
        // Method, so the section reads tight instead of three stacked
        // label+description blocks.
        let appearance_items = column![
            text(crate::i18n::t("terminal_theme"))
                .size(13)
                .color(OryxisColors::t().text_secondary),
            Space::new().height(8),
            theme_trigger,
            Space::new().height(14),
            icon_row,
            Space::new().height(12),
            encoding_row,
            Space::new().height(12),
            term_row,
        ];
        appearance_items.into()
    }

    pub(super) fn hp_row_session_logging(&self) -> Element<'_, Message> {
        // Session logging (universal -> Terminal). Tri-state: Default
        // (inherit global) / On / Off. Enter/Space cycles the state.
        let row_session_logging: Element<'_, Message> = self.panel_nav_slot(
            crate::keynav::RowAction::activate(Message::EditorCycleSessionLogging),
            8.0,
            container(
                dir_row(vec![
                    iced_fonts::lucide::file_text().size(14).color(OryxisColors::t().text_muted).into(),
                    Space::new().width(10).into(),
                    text(t("session_logging")).size(13).color(OryxisColors::t().text_secondary).into(),
                    Space::new().width(Length::Fill).into(),
                    {
                        let (label_key, bg) = match self.editor_form.session_logging {
                            None => ("session_log_default", OryxisColors::t().bg_hover),
                            Some(true) => ("session_log_on", OryxisColors::t().success),
                            Some(false) => ("session_log_off", OryxisColors::t().error),
                        };
                        let fg = crate::theme::contrast_text_for(bg);
                        button(text(t(label_key)).size(12).color(fg))
                            .on_press(Message::EditorCycleSessionLogging)
                            .style(move |_theme, _status| button::Style {
                                background: Some(Background::Color(bg)),
                                border: Border { radius: Radius::from(4.0), ..Default::default() },
                                text_color: fg,
                                ..Default::default()
                            })
                            .into()
                    },
                ]).align_y(iced::Alignment::Center)
            )
            .padding(Padding { top: 8.0, right: 0.0, bottom: 8.0, left: 0.0 }).into(),
        );
        row_session_logging
    }

    pub(super) fn hp_row_privacy_mode(&self) -> Element<'_, Message> {
        // Per-host Privacy Mode override: Default (inherit global) / On
        // (always hide sensitive data for this host) / Off (never hide).
        let privacy_mode_selected = match self.editor_form.privacy_mode {
            Some(true) => t("host_privacy_mode_on"),
            Some(false) => t("host_privacy_mode_off"),
            None => t("host_privacy_mode_default"),
        }
        .to_string();
        let privacy_mode_options = vec![
            t("host_privacy_mode_default").to_string(),
            t("host_privacy_mode_on").to_string(),
            t("host_privacy_mode_off").to_string(),
        ];
        // Focusable select (Tab + Enter/Space, widget-owned keys).
        let row_privacy_mode: Element<'_, Message> = panel_option_row(
            iced_fonts::lucide::eye_off(),
            t("host_privacy_mode"),
            self.panel_nav_slot(
                crate::keynav::RowAction::input(iced::widget::Id::new("editor-pick-privacy-mode")),
                crate::widgets::INPUT_RADIUS,
                pick_list(Some(privacy_mode_selected), privacy_mode_options, |s: &String| s.clone())
                    .on_select(Message::EditorPrivacyModeChanged)
                    .id(iced::widget::Id::new("editor-pick-privacy-mode"))
                    .on_open(Message::Navigation(NavigationMessage::PickOpenChanged(true)))
                    .on_close(Message::Navigation(NavigationMessage::PickOpenChanged(false)))
                    .width(120)
                    .padding(10)
                    .style(crate::widgets::rounded_pick_list_style)
                    .into(),
            ),
        );
        row_privacy_mode
    }

    /// A keyboard-navigable pick_list row for the Advanced-terminal
    /// section (icon + label + widget-owned select), mirroring the
    /// privacy-mode row.
    fn hp_quirk_pick_row<'a>(
        &self,
        icon: iced::widget::Text<'a>,
        label: &'a str,
        id: &'static str,
        selected: String,
        options: Vec<String>,
        on_select: fn(String) -> Message,
    ) -> Element<'a, Message> {
        panel_option_row(
            icon,
            label,
            self.panel_nav_slot(
                crate::keynav::RowAction::input(iced::widget::Id::new(id)),
                crate::widgets::INPUT_RADIUS,
                pick_list(Some(selected), options, |s: &String| s.clone())
                    .on_select(on_select)
                    .id(iced::widget::Id::new(id))
                    .on_open(Message::Navigation(NavigationMessage::PickOpenChanged(true)))
                    .on_close(Message::Navigation(NavigationMessage::PickOpenChanged(false)))
                    // Wide enough for the longest label ("Control-? (127)").
                    .width(160.0)
                    .padding(10)
                    .style(crate::widgets::rounded_pick_list_style)
                    .into(),
            ),
        )
    }

    /// An on/off toggle row for the Advanced-terminal section (mirrors
    /// the agent-forwarding toggle: click / Enter flips it).
    fn hp_quirk_toggle_row<'a>(
        &self,
        icon: iced::widget::Text<'a>,
        label: &'a str,
        on: bool,
        msg: fn(bool) -> Message,
    ) -> Element<'a, Message> {
        let toggle_msg = msg(!on);
        self.panel_nav_slot(
            crate::keynav::RowAction::activate(toggle_msg.clone()),
            8.0,
            container(
                dir_row(vec![
                    icon.size(14).color(OryxisColors::t().text_muted).into(),
                    Space::new().width(10).into(),
                    text(label).size(13).color(OryxisColors::t().text_secondary).into(),
                    Space::new().width(Length::Fill).into(),
                    {
                        let bg = if on { OryxisColors::t().success } else { OryxisColors::t().bg_hover };
                        let fg = crate::theme::contrast_text_for(bg);
                        button(
                            text(if on { t("toggle_on") } else { t("toggle_off") })
                                .size(12)
                                .color(fg),
                        )
                        .on_press(toggle_msg)
                        .style(move |_theme, _status| button::Style {
                            background: Some(Background::Color(bg)),
                            border: Border { radius: Radius::from(4.0), ..Default::default() },
                            text_color: fg,
                            ..Default::default()
                        })
                        .into()
                    },
                ])
                .align_y(iced::Alignment::Center),
            )
            .padding(Padding { top: 8.0, right: 0.0, bottom: 8.0, left: 0.0 })
            .into(),
        )
    }

    /// C5 "Advanced terminal" section: per-host legacy keyboard modes
    /// (backspace / home-end / function keys) and feature toggles
    /// (mouse reporting, title changes, OSC 52 clipboard, SSH rekey
    /// limit). Only rendered for terminal protocols (`is_terminal`);
    /// RDP/VNC hosts never call this.
    pub(super) fn hp_advanced_terminal_items(&self) -> Element<'_, Message> {
        use oryxis_core::models::terminal_quirks::{
            BackspaceMode, FunctionKeyMode, HomeEndMode, OptionAsMeta,
        };
        let q = &self.editor_form.quirks;

        let backspace_row = self.hp_quirk_pick_row(
            iced_fonts::lucide::delete(),
            t("quirks_backspace"),
            "editor-pick-quirk-backspace",
            crate::util::quirk_backspace_label(q.backspace),
            vec![
                crate::util::quirk_backspace_label(BackspaceMode::Del127),
                crate::util::quirk_backspace_label(BackspaceMode::CtrlH),
            ],
            Message::EditorQuirkBackspaceChanged,
        );

        let home_end_row = self.hp_quirk_pick_row(
            iced_fonts::lucide::move_horizontal(),
            t("quirks_home_end"),
            "editor-pick-quirk-homeend",
            crate::util::quirk_home_end_label(q.home_end),
            vec![
                crate::util::quirk_home_end_label(HomeEndMode::Standard),
                crate::util::quirk_home_end_label(HomeEndMode::Rxvt),
            ],
            Message::EditorQuirkHomeEndChanged,
        );

        let fn_keys_row = self.hp_quirk_pick_row(
            iced_fonts::lucide::keyboard(),
            t("quirks_fn_keys"),
            "editor-pick-quirk-fnkeys",
            crate::util::quirk_fn_keys_label(q.function_keys),
            vec![
                crate::util::quirk_fn_keys_label(FunctionKeyMode::Xterm),
                crate::util::quirk_fn_keys_label(FunctionKeyMode::LinuxConsole),
                crate::util::quirk_fn_keys_label(FunctionKeyMode::Vt400),
                crate::util::quirk_fn_keys_label(FunctionKeyMode::Rxvt),
            ],
            Message::EditorQuirkFnKeysChanged,
        );

        let mouse_row = self.hp_quirk_toggle_row(
            iced_fonts::lucide::mouse_pointer_click(),
            t("quirks_mouse_reporting"),
            !q.disable_mouse_reporting,
            Message::EditorQuirkMouseReportingChanged,
        );
        let title_row = self.hp_quirk_toggle_row(
            iced_fonts::lucide::r#type(),
            t("quirks_title_change"),
            !q.disable_title_change,
            Message::EditorQuirkTitleChangeChanged,
        );

        let osc52_selected = match q.osc52 {
            Some(oryxis_core::models::terminal_quirks::Osc52Override::On) => t("quirks_osc52_on"),
            Some(oryxis_core::models::terminal_quirks::Osc52Override::Off) => t("quirks_osc52_off"),
            None => t("quirks_osc52_default"),
        }
        .to_string();
        let osc52_row = self.hp_quirk_pick_row(
            iced_fonts::lucide::clipboard(),
            t("quirks_osc52"),
            "editor-pick-quirk-osc52",
            osc52_selected,
            vec![
                t("quirks_osc52_default").to_string(),
                t("quirks_osc52_on").to_string(),
                t("quirks_osc52_off").to_string(),
            ],
            Message::EditorQuirkOsc52Changed,
        );

        // macOS Option-as-Meta (issue #80). Shown on every platform: the
        // vault syncs across OSes, so a host edited on Linux/Windows must
        // still be able to carry the quirk its Mac replica applies; the
        // "(macOS)" in the label says where it takes effect.
        let option_meta_row = self.hp_quirk_pick_row(
            iced_fonts::lucide::option(),
            t("quirks_option_meta"),
            "editor-pick-quirk-optionmeta",
            crate::util::quirk_option_as_meta_label(q.option_as_meta),
            vec![
                crate::util::quirk_option_as_meta_label(OptionAsMeta::None),
                crate::util::quirk_option_as_meta_label(OptionAsMeta::OnlyLeft),
                crate::util::quirk_option_as_meta_label(OptionAsMeta::OnlyRight),
                crate::util::quirk_option_as_meta_label(OptionAsMeta::Both),
            ],
            Message::EditorQuirkOptionAsMetaChanged,
        );

        // Rekey limit: a small numeric text input (empty = russh default).
        let rekey_input: Element<'_, Message> = self.panel_nav_slot(
            crate::keynav::RowAction::input(iced::widget::Id::new("editor-quirk-rekey")),
            crate::widgets::INPUT_RADIUS,
            text_input(t("quirks_rekey_hint"), &self.editor_form.rekey_limit_mb)
                .id(iced::widget::Id::new("editor-quirk-rekey"))
                .on_input(Message::EditorQuirkRekeyChanged)
                .width(120)
                .padding(8)
                .style(crate::widgets::rounded_input_style)
                .into(),
        );
        let rekey_row = panel_option_row(
            iced_fonts::lucide::refresh_cw(),
            t("quirks_rekey_limit"),
            rekey_input,
        );

        column![
            section_header(t("quirks_section_title")),
            Space::new().height(2),
            text(t("quirks_applies_next_connect"))
                .size(11)
                .color(OryxisColors::t().text_muted),
            Space::new().height(4),
            backspace_row,
            home_end_row,
            fn_keys_row,
            mouse_row,
            title_row,
            osc52_row,
            option_meta_row,
            rekey_row,
        ]
        .spacing(2)
        .into()
    }
}
