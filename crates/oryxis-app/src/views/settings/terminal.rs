//! Settings -> Terminal section view. Split out of views/settings/mod.rs.

use super::*;
use iced::widget::column;

impl Oryxis {
    /// Long-command threshold pick for smart tabs, shown only while the
    /// smart-tabs toggle is on (an off feature hides all of its UI).
    fn smart_tabs_threshold_row(&self) -> Element<'_, Message> {
        if !self.setting_smart_tabs {
            return Space::new().height(0).into();
        }
        column![
            Space::new().height(10),
            self.nav_pick_row(
                crate::i18n::t("smart_tabs_threshold"),
                crate::smart_tabs::threshold_options()
                    .into_iter()
                    .map(|(_, l)| l)
                    .collect::<Vec<_>>(),
                crate::smart_tabs::threshold_label(self.setting_smart_long_secs),
                |s: &String| s.clone(),
                200.0,
                Message::SmartTabsThresholdChanged,
            ),
        ]
        .into()
    }

    /// Sub-row for the command-log folder, shown only while the
    /// live-append toggle is on: the effective folder (default
    /// `~/.oryxis/command-history/`) with a Change button, indented
    /// like the other nested sub-options.
    fn command_history_dir_row(&self) -> Element<'_, Message> {
        if !self.setting_command_history_file {
            return Space::new().height(0).into();
        }
        let indent = if crate::i18n::is_rtl_layout() {
            Padding { right: 22.0, ..Padding::ZERO }
        } else {
            Padding { left: 22.0, ..Padding::ZERO }
        };
        let dir = self.command_history_dir().display().to_string();
        let change = self.settings_nav_slot(
            crate::keynav::RowAction::activate(Message::CommandHistory(CommandHistoryMessage::PickCommandHistoryDir)),
            8.0,
            crate::widgets::styled_button_opt(
                crate::i18n::t("browse"),
                Some(Message::CommandHistory(CommandHistoryMessage::PickCommandHistoryDir)),
                crate::theme::OryxisColors::t().accent,
            ),
        );
        container(
            crate::widgets::dir_row(vec![
                text(dir)
                    .size(12)
                    .color(crate::theme::OryxisColors::t().text_muted)
                    .width(Length::Fill)
                    .into(),
                Space::new().width(10).into(),
                change,
            ])
            .align_y(iced::Alignment::Center),
        )
        .padding(Padding { top: 8.0, ..indent })
        .width(Length::Fill)
        .into()
    }

    /// Row for the ZMODEM download folder: the resolved path (default or
    /// configured) plus a Browse button, and a Reset when a custom folder
    /// is set. Always shown (transfers work regardless of other toggles).
    fn zmodem_download_dir_row(&self) -> Element<'_, Message> {
        let configured = self.setting_zmodem_download_dir.trim();
        let shown = if configured.is_empty() {
            dirs::download_dir()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "~/.oryxis/downloads".to_string())
        } else {
            configured.to_string()
        };
        let browse = self.settings_nav_slot(
            crate::keynav::RowAction::activate(Message::Zmodem(ZmodemMessage::PickZmodemDownloadDir)),
            8.0,
            crate::widgets::styled_button_opt(
                crate::i18n::t("browse"),
                Some(Message::Zmodem(ZmodemMessage::PickZmodemDownloadDir)),
                crate::theme::OryxisColors::t().accent,
            ),
        );
        let mut row = crate::widgets::dir_row(vec![
            column![
                text(crate::i18n::t("zmodem_download_dir"))
                    .size(13)
                    .color(crate::theme::OryxisColors::t().text_secondary),
                Space::new().height(2),
                text(shown)
                    .size(11)
                    .color(crate::theme::OryxisColors::t().text_muted),
            ]
            .width(Length::Fill)
            .into(),
            Space::new().width(10).into(),
            browse,
        ]);
        // Reset-to-default only when a custom folder is set.
        if !configured.is_empty() {
            let reset = self.settings_nav_slot(
                crate::keynav::RowAction::activate(Message::Zmodem(ZmodemMessage::ClearZmodemDownloadDir)),
                8.0,
                crate::widgets::styled_button_opt(
                    crate::i18n::t("reset"),
                    Some(Message::Zmodem(ZmodemMessage::ClearZmodemDownloadDir)),
                    crate::theme::OryxisColors::t().text_muted,
                ),
            );
            row = row.push(Space::new().width(8)).push(reset);
        }
        container(row.align_y(iced::Alignment::Center))
            .padding(Padding { top: 8.0, ..Padding::ZERO })
            .width(Length::Fill)
            .into()
    }

    pub(crate) fn view_settings_terminal(&self) -> Element<'_, Message> {
        // Keyboard rows are recorded in visual order: the sections below
        // are deliberately CONSTRUCTED in the same order they render
        // (recording happens at construction), so keep any new section
        // in its on-screen position.
        self.keynav_settings_reset();
        let mut toggles_col: iced::widget::Column<'_, Message> = column![
            self.nav_toggle_row(crate::i18n::t("copy_on_select"), self.setting_copy_on_select, Message::ToggleCopyOnSelect),
        ];
        // Right-click scheme (PuTTY's Context menu / Paste / Extend). The
        // single authority for the gesture.
        let rc_is_paste =
            self.setting_terminal_right_click == crate::util::RightClickMode::Paste;
        toggles_col = toggles_col.push(Space::new().height(10)).push(self.nav_pick_row(
            crate::i18n::t("terminal_right_click"),
            crate::util::RightClickMode::ALL
                .iter()
                .map(|m| crate::i18n::t(m.label_key()).to_string())
                .collect::<Vec<_>>(),
            crate::i18n::t(self.setting_terminal_right_click.label_key()).to_string(),
            |s: &String| s.clone(),
            200.0,
            Message::TerminalRightClickChanged,
        ));
        // "Copy on right-click" is a sub-option of copy-on-select, and
        // only meaningful when the right-click scheme is Paste (Menu and
        // Extend repurpose the gesture entirely). Hidden otherwise.
        if self.setting_copy_on_select && rc_is_paste {
            let indent = if crate::i18n::is_rtl_layout() {
                Padding { right: 22.0, ..Padding::ZERO }
            } else {
                Padding { left: 22.0, ..Padding::ZERO }
            };
            toggles_col = toggles_col
                .push(Space::new().height(8))
                .push(
                    container(self.nav_toggle_row(
                        crate::i18n::t("copy_requires_right_click"),
                        self.setting_right_click_copy,
                        Message::ToggleRightClickCopy,
                    ))
                    .padding(indent),
                );
        }
        // X11-style middle-click paste (xterm / PuTTY tradition). Its own
        // gesture, so it sits outside the copy-on-select bundle; the
        // paste still routes through the careful-paste / paste-guard
        // checks like every other paste path.
        toggles_col = toggles_col
            .push(Space::new().height(10))
            .push(self.nav_toggle_row(
                crate::i18n::t("middle_click_paste"),
                self.setting_middle_click_paste,
                Message::ToggleMiddleClickPaste,
            ));
        // Careful paste: the multi-line paste guard (line-count preview
        // before anything reaches the session). Default on; the toggle is
        // the power-user opt-out.
        toggles_col = toggles_col
            .push(Space::new().height(10))
            .push(self.nav_toggle_row(
                crate::i18n::t("careful_paste_label"),
                self.setting_careful_paste,
                Message::ToggleCarefulPaste,
            ))
            .push(Space::new().height(4))
            .push(
                text(crate::i18n::t("careful_paste_desc"))
                    .size(11)
                    .color(OryxisColors::t().text_muted),
            );
        // Content heuristics (bidi/invisible, control bytes, curl|sh,
        // homographs): its own switch so the multi-line check and the
        // suspicious-content check opt in/out independently.
        toggles_col = toggles_col
            .push(Space::new().height(10))
            .push(self.nav_toggle_row(
                crate::i18n::t("paste_guard_label"),
                self.setting_paste_guard,
                Message::TogglePasteGuard,
            ))
            .push(Space::new().height(4))
            .push(
                text(crate::i18n::t("paste_guard_desc"))
                    .size(11)
                    .color(OryxisColors::t().text_muted),
            );
        // The whole Behavior group shares one card: selection /
        // clipboard toggles, then the word-delimiter and scrollback
        // sub-blocks (each keeps its 13 px sub-title). Constructed
        // in visual order so the keyboard rows record in order.
        let toggles_col = toggles_col.push(Space::new().height(16));
        let word_delimiters_block = column![
            text(crate::i18n::t("word_delimiters")).size(13).color(OryxisColors::t().text_primary),
            Space::new().height(4),
            text(t("setting_word_delimiters_desc"))
                .size(11).color(OryxisColors::t().text_muted),
            Space::new().height(8),
            dir_row(vec![
                self.settings_nav_slot(
                    crate::keynav::RowAction::input(iced::widget::Id::new("set-terminal-word-delimiters")),
                    10.0,
                    text_input(oryxis_terminal::DEFAULT_WORD_DELIMITERS, &self.setting_word_delimiters)
                        .id(iced::widget::Id::new("set-terminal-word-delimiters"))
                        .on_input(Message::SettingWordDelimitersChanged)
                        .padding(10)
                        .width(240)
                        .style(crate::widgets::rounded_input_style)
                        .align_x(dir_align_x())
                        .into(),
                ),
                Space::new().width(8).into(),
                self.settings_nav_slot(
                    crate::keynav::RowAction::activate(Message::SettingResetWordDelimiters),
                    6.0,
                    styled_button(
                        crate::i18n::t("word_delimiters_reset"),
                        Message::SettingResetWordDelimiters,
                        OryxisColors::t().bg_selected,
                    ),
                ),
            ]).align_y(iced::Alignment::Center),
        ];

        let scrollback_block = column![
            text(crate::i18n::t("scrollback")).size(13).color(OryxisColors::t().text_primary),
            Space::new().height(4),
            text(t("setting_scrollback_desc"))
                .size(11).color(OryxisColors::t().text_muted),
            Space::new().height(8),
            self.settings_nav_slot(
                crate::keynav::RowAction::input(iced::widget::Id::new("set-terminal-scrollback")),
                10.0,
                text_input("10000", &self.setting_scrollback_rows)
                    .id(iced::widget::Id::new("set-terminal-scrollback"))
                    .on_input(Message::SettingScrollbackChanged)
                    .padding(10)
                    .width(240)
                    .style(crate::widgets::rounded_input_style).align_x(dir_align_x())
                    .into(),
            ),
            // PuTTY's two "jump back to the live edge" behaviors, so a user
            // stranded deep in history returns without reaching for the
            // wheel / scrollbar.
            Space::new().height(12),
            self.nav_toggle_row(
                crate::i18n::t("scrollback_reset_keypress"),
                self.setting_scrollback_reset_keypress,
                Message::ToggleScrollbackResetKeypress,
            ),
            Space::new().height(10),
            self.nav_toggle_row(
                crate::i18n::t("scrollback_reset_output"),
                self.setting_scrollback_reset_output,
                Message::ToggleScrollbackResetOutput,
            ),
        ];
        let behavior_section = panel_section(
            toggles_col
                .push(word_delimiters_block)
                .push(Space::new().height(16))
                .push(scrollback_block),
        );

        // Text rendering toggles open the Appearance card (under the
        // Appearance group header, not mixed with clipboard
        // behaviour); the font sub-blocks follow in the same card.
        let text_render_col = column![
            self.nav_toggle_row(crate::i18n::t("bold_bright"), self.setting_bold_is_bright, Message::ToggleBoldIsBright),
            Space::new().height(10),
            self.nav_toggle_row(crate::i18n::t("keyword_highlight"), self.setting_keyword_highlight, Message::ToggleKeywordHighlight),
            Space::new().height(10),
            self.nav_toggle_row(crate::i18n::t("command_history_capture"), self.setting_command_history, Message::ToggleCommandHistory),
            Space::new().height(10),
            self.nav_toggle_row(crate::i18n::t("cmd_history_file"), self.setting_command_history_file, Message::CommandHistory(CommandHistoryMessage::ToggleCommandHistoryFile)),
            self.command_history_dir_row(),
            Space::new().height(10),
            self.zmodem_download_dir_row(),
            Space::new().height(10),
            self.nav_toggle_row(crate::i18n::t("smart_contrast"), self.setting_smart_contrast, Message::ToggleSmartContrast),
            Space::new().height(10),
            self.nav_toggle_row(crate::i18n::t("terminal_auto_title"), crate::state::auto_title_enabled(), Message::ToggleTerminalAutoTitle),
            Space::new().height(10),
            self.nav_pick_row(
                crate::i18n::t("terminal_bell"),
                crate::util::BellMode::ALL
                    .iter()
                    .map(|m| crate::i18n::t(m.label_key()).to_string())
                    .collect::<Vec<_>>(),
                crate::i18n::t(self.setting_bell_mode.label_key()).to_string(),
                |s: &String| s.clone(),
                200.0,
                Message::BellModeChanged,
            ),
            Space::new().height(10),
            self.nav_pick_row(
                crate::i18n::t("terminal_clipboard"),
                crate::util::ClipboardAccess::ALL
                    .iter()
                    .map(|m| crate::i18n::t(m.label_key()).to_string())
                    .collect::<Vec<_>>(),
                crate::i18n::t(self.setting_clipboard_access.label_key()).to_string(),
                |s: &String| s.clone(),
                200.0,
                Message::ClipboardAccessChanged,
            ),
            Space::new().height(10),
            self.nav_pick_row(
                crate::i18n::t("terminal_notification"),
                crate::util::NotificationMode::ALL
                    .iter()
                    .map(|m| crate::i18n::t(m.label_key()).to_string())
                    .collect::<Vec<_>>(),
                crate::i18n::t(self.setting_notification_mode.label_key()).to_string(),
                |s: &String| s.clone(),
                200.0,
                Message::NotificationModeChanged,
            ),
            Space::new().height(10),
            self.nav_toggle_row(crate::i18n::t("smart_tabs"), self.setting_smart_tabs, Message::SettingToggleSmartTabs),
            self.smart_tabs_threshold_row(),
        ];

        // The +/- stepper maps naturally onto the picker action:
        // Left decreases, Right increases the font size.
        let font_size_block = column![
            self.settings_nav_slot(
                crate::keynav::RowAction::picker(
                    Some(Message::TerminalFontSizeDecrease),
                    Some(Message::TerminalFontSizeIncrease),
                ),
                8.0,
                dir_row(vec![
                text(crate::i18n::t("terminal_font_size")).size(13).color(OryxisColors::t().text_primary).into(),
                Space::new().width(Length::Fill).into(),
                button(
                    container(text("\u{2212}").size(14).color(OryxisColors::t().text_primary))
                        .padding(Padding { top: 4.0, right: 10.0, bottom: 4.0, left: 10.0 }),
                )
                .on_press(Message::TerminalFontSizeDecrease)
                .style(|_, status| {
                    let bg = match status {
                        BtnStatus::Hovered => OryxisColors::t().bg_hover,
                        _ => OryxisColors::t().bg_selected,
                    };
                    button::Style {
                        background: Some(Background::Color(bg)),
                        border: Border { radius: Radius::from(4.0), ..Default::default() },
                        ..Default::default()
                    }
                }).into(),
                Space::new().width(8).into(),
                text(format!("{:.0}", self.terminal_font_size)).size(13).color(OryxisColors::t().text_primary).into(),
                Space::new().width(8).into(),
                button(
                    container(text("+").size(14).color(OryxisColors::t().text_primary))
                        .padding(Padding { top: 4.0, right: 10.0, bottom: 4.0, left: 10.0 }),
                )
                .on_press(Message::TerminalFontSizeIncrease)
                .style(|_, status| {
                    let bg = match status {
                        BtnStatus::Hovered => OryxisColors::t().bg_hover,
                        _ => OryxisColors::t().bg_selected,
                    };
                    button::Style {
                        background: Some(Background::Color(bg)),
                        border: Border { radius: Radius::from(4.0), ..Default::default() },
                        ..Default::default()
                    }
                }).into(),
                ]).align_y(iced::Alignment::Center).into(),
            ),
        ];

        // Font picker. The list comes from a fontdb scan of
        // monospace families installed on the system (cached
        // for the process lifetime; rescanning per frame read
        // every font file from disk), with a hardcoded
        // fallback when the scan returns nothing.
        let fonts: &'static [String] = crate::app::enumerate_terminal_fonts();
        // Live sample rendered in the picked font on the active
        // terminal palette: the user can confirm the font exists
        // on their machine and preview the theme at a glance. The
        // font name comes straight from the (`'static`) enumerated
        // list, so `Family::Name` needs no leak.
        let preview_font = fonts
            .iter()
            .find(|f| f.as_str() == self.terminal_font_name)
            .map(|f| iced::Font {
                family: iced::font::Family::Name(f.as_str()),
                ..iced::Font::MONOSPACE
            })
            .unwrap_or(iced::Font::MONOSPACE);
        let active_term_theme = self
            .terminal_theme_override
            .clone()
            .unwrap_or_else(|| crate::theme::AppTheme::active().name().to_string());
        let pal = self
            .terminal_palette_for_name(&active_term_theme)
            .unwrap_or_default();
        let (fg, bg) = (pal.foreground, pal.background);
        let (c_green, c_blue, c_cyan, c_yellow) =
            (pal.ansi[2], pal.ansi[4], pal.ansi[6], pal.ansi[3]);
        let fs = self.terminal_font_size;
        let font_preview = container(
            column![
                text("The quick brown fox 1234567890 {}[]()<>")
                    .font(preview_font).size(fs).color(fg),
                Space::new().height(4),
                dir_row(vec![
                    text("user").font(preview_font).size(fs).color(c_green).into(),
                    text("@").font(preview_font).size(fs).color(fg).into(),
                    text("host").font(preview_font).size(fs).color(c_blue).into(),
                    text(":").font(preview_font).size(fs).color(fg).into(),
                    text("~/dev").font(preview_font).size(fs).color(c_cyan).into(),
                    text("$ ").font(preview_font).size(fs).color(fg).into(),
                    text("git status").font(preview_font).size(fs).color(c_yellow).into(),
                ]),
                Space::new().height(4),
                // Nerd Font glyphs (branch, powerline, home, folder,
                // github, git, code, terminal). Render as tofu boxes
                // if the picked font lacks Nerd Font icon coverage,
                // which is exactly the at-a-glance check we want.
                text("\u{e0a0} \u{e0b0} \u{f015} \u{f07b} \u{f09b} \u{e702} \u{f121} \u{f120}")
                    .font(preview_font).size(fs).color(c_green),
            ],
        )
        .padding(12)
        .width(Length::Fill)
        .style(move |_| container::Style {
            background: Some(Background::Color(bg)),
            border: Border {
                radius: Radius::from(8.0),
                color: OryxisColors::t().border,
                width: 1.0,
            },
            ..Default::default()
        });
        // Left/Right cycle the installed fonts without opening the
        // dropdown; `fonts` is a `'static` slice so cycle_pair borrows
        // it directly.
        let (font_prev, font_next) = crate::keynav::slots::cycle_pair(
            fonts,
            &self.terminal_font_name,
            Message::TerminalFontChanged,
        );
        let font_picker_block = column![
            text(crate::i18n::t("terminal_font")).size(13).color(OryxisColors::t().text_primary),
            Space::new().height(4),
            text(t("setting_font_desc"))
                .size(11).color(OryxisColors::t().text_muted),
            Space::new().height(8),
            self.settings_nav_slot(
                crate::keynav::RowAction::picker(font_prev, font_next),
                8.0,
                pick_list(
                    Some(self.terminal_font_name.clone()),
                    fonts,
                    |s: &String| s.clone(),
                )
                .on_select(Message::TerminalFontChanged)
                .on_open(Message::Navigation(NavigationMessage::PickOpenChanged(true)))
                .on_close(Message::Navigation(NavigationMessage::PickOpenChanged(false)))
                .width(260).padding(10).style(crate::widgets::rounded_pick_list_style)
                .into(),
            ),
            Space::new().height(12),
            font_preview,
        ];
        // One Appearance card: rendering toggles, then the font
        // size stepper and the font picker + live sample. The
        // terminal-theme gallery keeps its own card below (its own
        // sub-theme, and a grid that large reads better boxed
        // separately).
        let appearance_section = panel_section(
            text_render_col
                .push(Space::new().height(16))
                .push(font_size_block)
                .push(Space::new().height(16))
                .push(font_picker_block),
        );

        // Terminal theme picker. First card is the "follow
        // app theme" sentinel (terminal_theme_override = None);
        // the rest are explicit palette previews so the user
        // can compare colours without applying each one. Per-host
        // overrides configured via the icon picker still win
        // over this global pick. Each card is a keyboard row (Enter
        // applies / opens it); built after the font picker so the
        // recording matches the render order.
        let mut theme_cards: Vec<Element<'_, Message>> = Vec::new();
        // The sentinel renders as a real palette card previewing
        // the app-theme-derived palette (every app theme has a
        // same-named terminal palette), instead of the old
        // input-looking box that read as a text field.
        let app_theme_name = crate::theme::AppTheme::active().name();
        let follow_palette = self
            .terminal_palette_for_name(app_theme_name)
            .unwrap_or_default();
        let follow_label =
            format!("{} ({})", t("terminal_theme_follow_app"), app_theme_name);
        theme_cards.push(self.settings_nav_slot(
            crate::keynav::RowAction::activate(Message::TerminalThemeChanged(String::new())),
            10.0,
            crate::widgets::terminal_theme_card(
                follow_palette,
                &follow_label,
                self.terminal_theme_override.is_none(),
                Message::TerminalThemeChanged(String::new()),
            ),
        ));
        for theme in oryxis_terminal::TerminalTheme::ALL.iter() {
            let is_selected = self
                .terminal_theme_override
                .as_deref()
                == Some(theme.name());
            theme_cards.push(self.settings_nav_slot(
                crate::keynav::RowAction::activate(Message::TerminalThemeChanged(
                    theme.name().to_string(),
                )),
                10.0,
                crate::widgets::terminal_theme_card(
                    theme.palette(),
                    theme.name(),
                    is_selected,
                    Message::TerminalThemeChanged(theme.name().to_string()),
                ),
            ));
        }
        // User-defined themes after the built-ins, each with the
        // hover edit / delete affordances. Enter applies the theme
        // (the card's own click action); edit / delete stay
        // hover-only.
        for (idx, ct) in self.custom_terminal_themes.iter().enumerate() {
            let is_selected =
                self.terminal_theme_override.as_deref() == Some(ct.name.as_str());
            let palette = self
                .terminal_palette_for_name(&ct.name)
                .unwrap_or_default();
            theme_cards.push(self.settings_nav_slot(
                crate::keynav::RowAction::activate(Message::TerminalThemeChanged(
                    ct.name.clone(),
                )),
                10.0,
                self.terminal_custom_theme_card(
                    idx,
                    &ct.name,
                    palette,
                    is_selected,
                ),
            ));
        }
        // "+ New custom theme" + "Import" cards last.
        theme_cards.push(self.settings_nav_slot(
            crate::keynav::RowAction::activate(Message::ThemeEditorNew),
            10.0,
            crate::views::settings_themes::terminal_theme_add_card(),
        ));
        theme_cards.push(self.settings_nav_slot(
            crate::keynav::RowAction::activate(Message::ThemeImportOpen),
            10.0,
            crate::views::settings_themes::terminal_theme_import_card(),
        ));
        // 2-column responsive grid for theme cards. Cards still
        // use the existing swatch-+-name layout (the "bolinhas"
        // style); only the row arrangement changes from a single
        // tall column to a side-by-side pair so the picker
        // doesn't dominate the settings panel vertically.
        let theme_grid = crate::widgets::distribute_card_grid(
            theme_cards,
            2,
            8.0,
            8.0,
        );
        let theme_picker_section = panel_section(column![
            text(t("terminal_theme")).size(13).color(OryxisColors::t().text_primary),
            Space::new().height(4),
            text(t("terminal_theme_desc"))
                .size(11).color(OryxisColors::t().text_muted),
            Space::new().height(10),
            theme_grid,
        ]);

        // Grouped under "h2" headers, same pattern as Interface:
        // Behavior (selection, delimiters, scrollback) then
        // Appearance (rendering, font, theme). Connection + logging
        // knobs live in their own sections.
        use crate::widgets::settings_group_header as gh;
        scrollable(
            container(
                column![
                    gh(crate::i18n::t("terminal_group_behavior")),
                    Space::new().height(8),
                    behavior_section,
                    Space::new().height(18),
                    gh(crate::i18n::t("terminal_group_appearance")),
                    Space::new().height(8),
                    appearance_section,
                    Space::new().height(12),
                    theme_picker_section,
                    Space::new().height(18),
                    gh(crate::i18n::t("local_terminals")),
                    Space::new().height(8),
                    self.local_terminals_card(),
                    Space::new().height(24),
                ]
                .width(Length::Fill)
                .align_x(dir_align_x()),
            )
            .padding(Padding { top: 24.0, right: 24.0, bottom: 24.0, left: 24.0 }),
        )
        // Stable id so the keyboard router can keep the selected row
        // in view.
        .id(iced::widget::Id::new("settings-terminal-scroll"))
        .height(Length::Fill)
        .into()
    }
}
