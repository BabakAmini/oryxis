//! Terminal view + AI chat sidebar.

use std::sync::Arc;

use iced::border::Radius;
use iced::widget::{
    button, canvas, column, container, row, scrollable, text, MouseArea, Space,
};
use iced::widget::button::Status as BtnStatus;
use iced::{Background, Border, Color, Element, Length, Padding};

use oryxis_terminal::widget::TerminalView;

use crate::app::{Message, Oryxis};
use crate::i18n::t;
use crate::state::TerminalTab;
use crate::theme::OryxisColors;
use crate::widgets::dir_row;

impl Oryxis {
    pub(crate) fn view_terminal(&self) -> Element<'_, Message> {
        let chat_visible = self.active_tab
            .and_then(|idx| self.tabs.get(idx))
            .map(|tab| tab.chat_visible)
            .unwrap_or(false);

        let terminal_area: Element<'_, Message> = if let Some(tab_idx) = self.active_tab {
            if let Some(tab) = self.tabs.get(tab_idx) {
                // Render the tab's panes through a `pane_grid`. With one
                // pane this is visually identical to the old single canvas;
                // splits add cells. Each cell gets a focus border (only
                // visible once there's more than one pane) and the grid
                // wires click-to-focus + drag-to-resize.
                let focused = tab.focused;
                let multipane = tab.pane_grid.panes.len() > 1;
                let grid = iced::widget::pane_grid(&tab.pane_grid, move |pane, pane_data, _max| {
                    let is_focused = pane == focused;
                    // The focus border only shows when there's more than one
                    // pane; the mouse-report gate uses real focus regardless.
                    let show_border = multipane && is_focused;
                    let border_color = if show_border {
                        OryxisColors::t().accent
                    } else {
                        OryxisColors::t().border
                    };
                    iced::widget::pane_grid::Content::new(
                        container(self.render_pane_canvas(pane_data, is_focused))
                            .width(Length::Fill)
                            .height(Length::Fill)
                            .style(move |_| container::Style {
                                border: Border {
                                    color: border_color,
                                    width: if multipane { 1.0 } else { 0.0 },
                                    radius: Radius::from(0.0),
                                },
                                ..Default::default()
                            }),
                    )
                })
                .on_click(Message::FocusPane)
                .on_resize(8, Message::ResizePane)
                .spacing(if multipane { 4 } else { 0 })
                .width(Length::Fill)
                .height(Length::Fill);

                // The AI/sidebar toggle now lives in the tab bar (panel
                // button right of `+`), so the terminal canvas no longer
                // carries its own floating sparkle overlay.
                let term_with_toggle: Element<'_, Message> = grid.into();

                // The session-group editor renders here, as a sibling of the
                // grid inside the terminal area, the same way the chat sidebar
                // does. Wrapping the whole terminal container from outside
                // (view_content) instead left the canvas eating clicks meant
                // for the panel, so keep it inside.
                if chat_visible || self.show_session_group_panel {
                    let mut children = vec![term_with_toggle];
                    if chat_visible {
                        children.push(self.view_terminal_sidebar(tab));
                    }
                    if self.show_session_group_panel {
                        children.push(self.view_session_group_panel());
                    }
                    dir_row(children)
                        .width(Length::Fill)
                        .height(Length::Fill)
                        .into()
                } else {
                    term_with_toggle
                }
            } else {
                container(text(t("no_active_session")).size(14).color(OryxisColors::t().text_muted))
                    .center(Length::Fill).into()
            }
        } else {
            container(text(t("no_active_session")).size(14).color(OryxisColors::t().text_muted))
                .center(Length::Fill).into()
        };

        let base = container(terminal_area)
            .width(Length::Fill)
            .height(Length::Fill)
            .style(|_| container::Style {
                background: Some(Background::Color(OryxisColors::t().terminal_bg)),
                ..Default::default()
            });
        // Floating overlay over the terminal area (shown whether or not
        // the chat sidebar is open): the ZMODEM transfer card. The toast
        // chip is NOT layered here: it mounts at the window root
        // (`root_view.rs`), so notifications raised while the user sits
        // in the Dashboard / Settings views are visible too.
        let mut stack = iced::widget::Stack::new().push(base);
        if let Some(zm) = self.zmodem_overlay() {
            stack = stack.push(zm);
        }
        stack.into()
    }

    /// Bottom-center transfer card over the terminal while the active
    /// pane is running a ZMODEM transfer: direction, file name, byte
    /// progress, a bar (when the size is known) and a Cancel button.
    /// `None` when no transfer is active.
    fn zmodem_overlay(&self) -> Option<Element<'_, Message>> {
        let pane = self.active_tab.and_then(|i| self.tabs.get(i)).map(|t| t.active())?;
        let zm = pane.zmodem.as_ref()?;
        let pane_id = pane.id;

        let verb = match zm.direction {
            oryxis_zmodem::Direction::Download => t("zmodem_downloading"),
            oryxis_zmodem::Direction::Upload => t("zmodem_uploading"),
        };
        let name = zm.file_name.as_deref().unwrap_or("…");
        let bytes_line = match zm.total {
            Some(total) => format!("{} / {}", fmt_bytes(zm.transferred), fmt_bytes(total)),
            None => fmt_bytes(zm.transferred),
        };
        let header = dir_row(vec![
            text(format!("{verb} {name}"))
                .size(12)
                .color(OryxisColors::t().text_primary)
                .into(),
            Space::new().width(Length::Fill).into(),
            text(bytes_line).size(11).color(OryxisColors::t().text_muted).into(),
        ])
        .align_y(iced::Alignment::Center);

        let mut body = column![header].spacing(6).width(Length::Fixed(320.0));
        if let Some(total) = zm.total.filter(|t| *t > 0) {
            let frac = (zm.transferred as f32 / total as f32).clamp(0.0, 1.0);
            body = body.push(iced::widget::progress_bar(0.0..=1.0, frac));
        }
        let cancel = button(text(t("cancel")).size(11).color(OryxisColors::t().text_primary))
            .on_press(Message::ZmodemCancel(pane_id))
            .padding(Padding { top: 4.0, right: 10.0, bottom: 4.0, left: 10.0 })
            .style(|_, status| {
                let bg = match status {
                    iced::widget::button::Status::Hovered
                    | iced::widget::button::Status::Pressed => OryxisColors::t().bg_hover,
                    _ => OryxisColors::t().bg_surface,
                };
                iced::widget::button::Style {
                    background: Some(Background::Color(bg)),
                    border: Border {
                        radius: Radius::from(6.0),
                        color: OryxisColors::t().border,
                        width: 1.0,
                    },
                    ..Default::default()
                }
            });
        body = body.push(
            container(cancel)
                .width(Length::Fill)
                .align_x(iced::alignment::Horizontal::Right),
        );

        let card = container(body)
            .padding(Padding { top: 10.0, right: 12.0, bottom: 10.0, left: 12.0 })
            .style(|_| container::Style {
                background: Some(Background::Color(Color {
                    a: 0.97,
                    ..OryxisColors::t().bg_selected
                })),
                border: Border {
                    radius: Radius::from(8.0),
                    color: OryxisColors::t().accent,
                    width: 1.0,
                },
                ..Default::default()
            });
        Some(
            container(
                column![
                    Space::new().height(Length::Fill),
                    container(card)
                        .width(Length::Fill)
                        .align_x(iced::alignment::Horizontal::Center),
                    Space::new().height(Length::Fixed(84.0)),
                ]
                .width(Length::Fill)
                .height(Length::Fill),
            )
            .width(Length::Fill)
            .height(Length::Fill)
            .into(),
        )
    }

    /// Bottom-center toast chip, or `None` when no toast is pending.
    /// Mounted at the window root (`root_view.rs`) so it floats over
    /// every unlocked view, not just the terminal; the chat sidebar no
    /// longer renders its own copy (that only showed while it was open).
    pub(crate) fn toast_overlay(&self) -> Option<Element<'_, Message>> {
        let text_ = self.toast.as_ref()?;
        let chip = container(
            text(text_.clone()).size(11).color(OryxisColors::t().text_primary),
        )
        .padding(Padding { top: 5.0, right: 12.0, bottom: 5.0, left: 12.0 })
        .style(|_| container::Style {
            background: Some(Background::Color(Color {
                a: 0.95,
                ..OryxisColors::t().bg_selected
            })),
            border: Border {
                radius: Radius::from(8.0),
                color: OryxisColors::t().border,
                width: 1.0,
            },
            ..Default::default()
        });
        // Clicking the chip dismisses it immediately. Only the chip is
        // interactive; the surrounding Fill stays transparent to clicks so it
        // never steals input from the terminal underneath.
        let chip = MouseArea::new(chip)
            .on_press(Message::ToastClear)
            .interaction(iced::mouse::Interaction::Pointer);
        Some(
            container(
                column![
                    Space::new().height(Length::Fill),
                    container(chip)
                        .width(Length::Fill)
                        .align_x(iced::alignment::Horizontal::Center),
                    Space::new().height(Length::Fixed(48.0)),
                ]
                .width(Length::Fill)
                .height(Length::Fill),
            )
            .width(Length::Fill)
            .height(Length::Fill)
            .into(),
        )
    }

    /// Build the terminal canvas for one pane, applying the global font /
    /// rendering settings. Shared by every `pane_grid` cell. `is_focused`
    /// gates mouse-tracking reports so a focus-click on an inactive pane
    /// doesn't inject a stray report.
    fn render_pane_canvas<'a>(
        &'a self,
        pane: &'a crate::state::Pane,
        is_focused: bool,
    ) -> Element<'a, Message> {
        let mut term_view = TerminalView::new(Arc::clone(&pane.terminal))
            .focused(is_focused)
            .with_bell_flash(pane.bell_flash)
            .with_font_size(self.terminal_font_size)
            .with_font_name(&self.terminal_font_name)
            .with_copy_on_select(self.setting_copy_on_select)
            .with_right_click_copy(self.setting_right_click_copy)
            .with_middle_click_paste(self.setting_middle_click_paste)
            .with_right_click_action(self.setting_terminal_right_click.to_widget())
            .with_bold_is_bright(self.setting_bold_is_bright)
            .with_keyword_highlight(self.setting_keyword_highlight)
            .with_performance(self.setting_performance_mode)
            .with_perf_overlay(self.setting_perf_overlay)
            .with_privacy(self.privacy_active_for_label(&pane.label))
            .with_privacy_terms(&self.privacy_terms())
            .with_smart_contrast(self.setting_smart_contrast)
            .with_word_delimiters(&self.setting_word_delimiters)
            .on_font_size_increase(Message::TerminalFontSizeIncrease)
            .on_font_size_decrease(Message::TerminalFontSizeDecrease)
            .on_paste_request(Message::TerminalPasteFromClipboard)
            .on_terminal_input(Message::TerminalInput)
            .on_link_opened(Message::TerminalLinkOpened);
        // Context menu (right-click scheme = Menu): carry the clicked
        // pane's id so Copy All / Clear Scrollback target the right pane,
        // not just the focused one.
        if self.setting_terminal_right_click == crate::util::RightClickMode::Menu {
            let pane_id = pane.id;
            term_view = term_view
                .on_context_menu(move |x, y, _sel| Message::ShowTerminalContextMenu(pane_id, x, y));
        }
        // Wire the teaching hints only while they should still show for
        // this pane, so the widget stops emitting once HintMode::Once has
        // retired them (and never emits under Never).
        if self.setting_hint_mode.should_show(pane.mouse_hint_shown) {
            term_view = term_view.on_mouse_capture_hint(|| Message::TerminalMouseCaptureHint);
        }
        if self.setting_hint_mode.should_show(pane.link_hint_shown) {
            term_view = term_view.on_link_click_hint(|| Message::TerminalLinkClickHint);
        }
        // Wrap the canvas so the focused pane asks the OS to enable its IME.
        // The terminal is a canvas (not a text_input), so without this winit
        // keeps the IME disabled and CJK input can't be switched on.
        let term_canvas = canvas(term_view)
            .width(Length::Fill)
            .height(Length::Fill);
        crate::widgets::ime_host(
            term_canvas,
            is_focused,
            Arc::clone(&pane.terminal),
            self.terminal_font_size,
            self.terminal_font_name.clone(),
        )
    }

    pub(crate) fn view_terminal_sidebar<'a>(&'a self, tab: &'a TerminalTab) -> Element<'a, Message> {
        use crate::state::TerminalSidebarTab as STab;
        // Fresh sidebar-row recording every frame, BEFORE any tab body
        // is built: each tab records its keyboard rows while rendering,
        // so a stale list from the previous frame must never leak in.
        self.sidebar_nav_reset();
        // Chat is only reachable when AI is enabled; otherwise the active
        // tab effectively falls back to Snippets.
        let active = if self.terminal_sidebar_tab == STab::Chat && !self.ai.enabled {
            STab::Snippets
        } else {
            self.terminal_sidebar_tab
        };

        // ── Tab strip ──
        // Icon tabs on the leading edge; contextual Reset (Chat only) and
        // the Close X on the trailing edge, same affordance as the chrome.
        let mut strip: Vec<Element<'_, Message>> = Vec::new();
        if self.ai.enabled {
            strip.push(sidebar_tab_btn(
                iced_fonts::lucide::sparkles(),
                active == STab::Chat,
                Message::SelectTerminalSidebarTab(STab::Chat),
                t("tab_tip_chat"),
            ));
        }
        strip.push(sidebar_tab_btn(
            iced_fonts::lucide::code(),
            active == STab::Snippets,
            Message::SelectTerminalSidebarTab(STab::Snippets),
            t("snippets"),
        ));
        strip.push(sidebar_tab_btn(
            iced_fonts::lucide::history(),
            active == STab::History,
            Message::SelectTerminalSidebarTab(STab::History),
            t("tab_tip_history"),
        ));
        strip.push(sidebar_tab_btn(
            iced_fonts::lucide::cog(),
            active == STab::HostConfig,
            Message::SelectTerminalSidebarTab(STab::HostConfig),
            t("tab_tip_host_config"),
        ));
        strip.push(Space::new().width(Length::Fill).into());
        // The trailing header actions (Reset on Chat, Close always) join
        // the Tab walk, recorded FIRST (the strip renders above every tab
        // body) under the active tab's tag. The four tab icons stay off
        // the walk: the FocusSidebarList hotkey already cycles them.
        if active == STab::Chat {
            strip.push(self.sidebar_nav_slot(
                crate::keynav::SidebarRow::button(Message::ChatResetConversation),
                active,
                6.0,
                icon_tooltip(
                    chat_header_btn(
                        iced_fonts::lucide::rotate_ccw(),
                        Message::ChatResetConversation,
                    ),
                    t("chat_reset_tip"),
                ),
            ));
            strip.push(Space::new().width(4).into());
        }
        strip.push(self.sidebar_nav_slot(
            crate::keynav::SidebarRow::button(Message::ToggleChatSidebar),
            active,
            6.0,
            icon_tooltip(
                chat_header_btn(iced_fonts::lucide::x(), Message::ToggleChatSidebar),
                t("close"),
            ),
        ));

        let header = container(
            dir_row(strip)
                .width(Length::Fill)
                .align_y(iced::Alignment::Center),
        )
        .padding(Padding { top: 8.0, right: 8.0, bottom: 8.0, left: 8.0 })
        .width(Length::Fill);

        let header_separator = container(Space::new().height(1))
            .width(Length::Fill)
            .style(|_| container::Style {
                background: Some(Background::Color(OryxisColors::t().border)),
                ..Default::default()
            });

        // 4 px draggable handle on the left edge, clicking starts a
        // resize, the global mouse-move handler in app.rs follows the
        // cursor, and the global mouse-up stops the drag.
        let resize_handle: Element<'_, Message> = MouseArea::new(
            container(Space::new().width(Length::Fixed(4.0)).height(Length::Fill))
                .width(Length::Fixed(4.0))
                .height(Length::Fill)
                .style(|_| container::Style {
                    background: Some(Background::Color(OryxisColors::t().border)),
                    ..Default::default()
                }),
        )
        .on_press(Message::ChatSidebarResizeStart)
        .interaction(iced::mouse::Interaction::ResizingHorizontally)
        .into();

        // ── Assemble sidebar ──
        // Tab bodies are built lazily inside the match: building an
        // inactive tab's body would record its keyboard rows into the
        // per-frame sidebar recording (see `sidebar_nav_reset` above).
        let content: Element<'_, Message> = match active {
            STab::Chat => self.chat_tab_body(tab),
            STab::Snippets => self.snippets_tab_content(),
            STab::History => self.history_tab_content(),
            STab::HostConfig => self.host_config_tab_content(tab),
        };
        let panel_column = column![header, header_separator, content]
            .width(Length::Fill)
            .height(Length::Fill);

        // The toast now floats over the whole terminal view (see
        // `view_terminal` / `toast_overlay`), not just this sidebar, so it
        // shows even when the chat panel is closed.
        let panel = container(panel_column)
        .width(Length::Fill)
        .height(Length::Fill)
        .style(|_| container::Style {
            background: Some(Background::Color(OryxisColors::t().bg_primary)),
            ..Default::default()
        });

        container(
            row![resize_handle, panel]
                .width(Length::Fill)
                .height(Length::Fill),
        )
        .width(Length::Fixed(self.chat_sidebar_width))
        .height(Length::Fill)
        .into()
    }

    /// Chat tab body: the message list, the floating Stop pill, the
    /// Plan / Ask / Auto mode picker and the message editor. Split out
    /// of `view_terminal_sidebar` so it only renders (and records its
    /// keyboard rows) when the Chat tab is the active one.
    fn chat_tab_body<'a>(&'a self, tab: &'a TerminalTab) -> Element<'a, Message> {
        // ── Messages list ──
        let mut messages_col = column![].spacing(8).padding(Padding { top: 8.0, right: 12.0, bottom: 8.0, left: 12.0 });

        if tab.chat_history.is_empty() {
            messages_col = messages_col.push(
                container(
                    column![
                        iced_fonts::lucide::sparkles().size(24).color(OryxisColors::t().text_muted),
                        Space::new().height(8),
                        text(t("ask_ai_session")).size(12).color(OryxisColors::t().text_muted),
                    ]
                    .align_x(iced::Alignment::Center),
                )
                .center_x(Length::Fill)
                .padding(Padding { top: 40.0, right: 0.0, bottom: 0.0, left: 0.0 }),
            );
        } else {
            // Markdown settings are identical for every assistant
            // bubble, so build them once per sidebar render instead of
            // re-deriving the style from the theme per message.
            let md_settings = self.chat_markdown_settings();
            for msg in &tab.chat_history {
                // Skip empty assistant placeholders, they exist as
                // staging slots for streaming chunks; an empty one is
                // either pre-first-token (covered by the "Thinking..."
                // bubble below) or a stream that ended before any text
                // arrived (e.g. straight to a tool call). Either way,
                // an empty padded box would just look like a glitch.
                if msg.role == crate::state::ChatRole::Assistant
                    && msg.content.is_empty()
                {
                    continue;
                }
                let bubble = self.view_chat_message(msg, md_settings);
                messages_col = messages_col.push(bubble);
            }
        }

        // Hide the "Thinking..." indicator once the model has started
        // streaming visible text, the streaming bubble itself is the
        // signal of activity, and showing both reads as a stutter.
        let actively_streaming = tab
            .chat_history
            .last()
            .map(|m| m.role == crate::state::ChatRole::Assistant && !m.content.is_empty())
            .unwrap_or(false);
        if tab.chat_loading && !actively_streaming {
            messages_col = messages_col.push(
                container(
                    text(t("thinking")).size(12).color(OryxisColors::t().text_muted),
                )
                .padding(Padding { top: 4.0, right: 8.0, bottom: 4.0, left: 8.0 })
                .style(|_| container::Style {
                    background: Some(Background::Color(OryxisColors::t().bg_surface)),
                    border: Border { radius: Radius::from(8.0), ..Default::default() },
                    ..Default::default()
                }),
            );
        }

        let messages_scroll = scrollable(messages_col)
            .id(iced::widget::Id::new("chat-scroll"))
            .on_scroll(|viewport| Message::ChatScrolled(viewport.relative_offset().y))
            .width(Length::Fill)
            .height(Length::Fill);

        // ── Input area ──
        let input_separator = container(Space::new().height(1))
            .width(Length::Fill)
            .style(|_| container::Style {
                background: Some(Background::Color(OryxisColors::t().border)),
                ..Default::default()
            });

        // Multi-line input, grows with content up to ~6 lines (~150 px),
        // then scrolls internally. Enter sends the message; Shift+Enter
        // inserts a newline. No send button, every chat-style UI uses
        // Enter today, so the arrow was just visual noise.
        let chat_editor = iced::widget::text_editor(&self.chat_input)
            // Programmatic focus target for the FocusSidebarList hotkey's
            // Chat stop (the fork's text_editor is operation::Focusable).
            .id(iced::widget::Id::new("chat-input"))
            .placeholder(t("ask_ai"))
            .on_action(Message::ChatInputAction)
            .padding(10)
            .height(Length::Shrink)
            .key_binding(|key_press| {
                use iced::keyboard::{key::Named, Key};
                use iced::widget::text_editor::{Binding, KeyPress};
                let KeyPress { key, modifiers, .. } = &key_press;
                if matches!(key, Key::Named(Named::Enter)) && !modifiers.shift() {
                    return Some(Binding::Custom(Message::SendChat));
                }
                Binding::from_key_press(key_press)
            })
            .style(|_theme, status| {
                let c = OryxisColors::t();
                let (border_color, border_width) = match status {
                    iced::widget::text_editor::Status::Focused { .. } => (c.accent, 1.5),
                    _ => (c.border, 1.0),
                };
                iced::widget::text_editor::Style {
                    background: Background::Color(c.bg_surface),
                    border: Border {
                        radius: Radius::from(crate::widgets::INPUT_RADIUS),
                        width: border_width,
                        color: border_color,
                    },
                    placeholder: c.text_muted,
                    value: c.text_primary,
                    selection: c.accent,
                }
            });

        // Plan / Ask / Auto picker, sitting just above the input so the
        // active mode is visible while typing. Reflects (and sets) THIS
        // tab's mode. Recorded as a picker row (Left/Right cycle the
        // modes) BEFORE the editor, matching the display order.
        let mode_row = {
            use crate::state::ChatMode;
            let (prev, next) = crate::keynav::slots::cycle_pair(
                &[ChatMode::Plan, ChatMode::Ask, ChatMode::Auto],
                &tab.chat_mode,
                Message::ChatModeChanged,
            );
            container(
                dir_row(vec![self.sidebar_nav_slot(
                    crate::keynav::SidebarRow::picker(prev, next),
                    crate::state::TerminalSidebarTab::Chat,
                    6.0,
                    crate::views::sidebar_chat::chat_mode_picker(tab.chat_mode),
                )])
                .width(Length::Fill)
                .align_y(iced::Alignment::Center),
            )
            .padding(Padding { top: 6.0, right: 12.0, bottom: 0.0, left: 12.0 })
            .width(Length::Fill)
            .align_x(crate::widgets::dir_align_x())
        };

        // The editor is an input row in the sidebar Tab walk (real
        // focus via its "chat-input" id; its own key_binding keeps
        // Enter = send).
        let input_row = container(
            self.sidebar_nav_slot(
                crate::keynav::SidebarRow::input(iced::widget::Id::new("chat-input")),
                crate::state::TerminalSidebarTab::Chat,
                crate::widgets::INPUT_RADIUS,
                container(chat_editor).height(Length::Shrink.max(150.0)).into(),
            ),
        )
        .padding(Padding { top: 8.0, right: 12.0, bottom: 12.0, left: 12.0 })
        .width(Length::Fill);

        // Persistent reminder that the assistant runs commands on the
        // live server (some auto-execute), sitting just above the input.
        let chat_disclaimer = container(
            text(t("ai_chat_disclaimer"))
                .size(10)
                .color(OryxisColors::t().text_muted),
        )
        .padding(Padding { top: 6.0, right: 12.0, bottom: 0.0, left: 12.0 })
        .width(Length::Fill)
        .align_x(crate::widgets::dir_align_x());

        // While a chat task is in flight (streaming a reply or auto-running
        // a tool chain) offer an explicit Stop, floating over the bottom of
        // the message list (not inline) so it stays reachable without pushing
        // the conversation around. It aborts the live task so a runaway tool
        // loop can be halted by hand, without closing the panel. Per-tab:
        // shown only when THIS tab has work in flight.
        let stop_overlay: Option<Element<'_, Message>> = tab.chat_task.is_some().then(|| {
            let pill = button(
                dir_row(vec![
                    iced_fonts::lucide::circle_stop()
                        .size(12)
                        .color(OryxisColors::t().text_primary)
                        .into(),
                    text(t("chat_stop"))
                        .size(11)
                        .color(OryxisColors::t().text_primary)
                        .into(),
                ])
                .spacing(6)
                .align_y(iced::Alignment::Center),
            )
            .padding(Padding { top: 5.0, right: 14.0, bottom: 5.0, left: 14.0 })
            .on_press(Message::ChatStop)
            .style(|_, status| {
                let c = OryxisColors::t();
                let bg = match status {
                    BtnStatus::Hovered => c.button_bg_hover,
                    _ => c.button_bg,
                };
                button::Style {
                    background: Some(Background::Color(bg)),
                    text_color: c.text_primary,
                    border: Border {
                        radius: Radius::from(16.0),
                        width: 1.0,
                        color: c.border,
                    },
                    // A soft shadow lifts the pill off the messages behind it.
                    shadow: iced::Shadow {
                        color: Color { a: 0.25, ..Color::BLACK },
                        offset: iced::Vector::new(0.0, 2.0),
                        blur_radius: 8.0,
                    },
                    ..Default::default()
                }
            });
            // Pin to bottom-center of the message area, floating above the
            // separator/input.
            container(pill)
                .width(Length::Fill)
                .height(Length::Fill)
                .align_x(iced::alignment::Horizontal::Center)
                .align_y(iced::alignment::Vertical::Bottom)
                .padding(Padding { top: 0.0, right: 0.0, bottom: 10.0, left: 0.0 })
                .into()
        });

        // Base is the scrollable message list; the Stop pill (when present)
        // floats over its bottom edge via a Stack.
        let messages_area: Element<'_, Message> = match stop_overlay {
            Some(overlay) => iced::widget::Stack::new()
                .push(messages_scroll)
                .push(overlay)
                .into(),
            None => messages_scroll.into(),
        };

        column![messages_area, input_separator, mode_row, chat_disclaimer, input_row]
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }
}

/// One icon tab in the sidebar's tab strip. Active tab gets an accent
/// glyph on a faint accent wash; inactive is muted and transparent.
fn sidebar_tab_btn<'a>(
    icon: iced::widget::Text<'a>,
    active: bool,
    msg: Message,
    tip: &'a str,
) -> Element<'a, Message> {
    let color = if active { OryxisColors::t().accent } else { OryxisColors::t().text_muted };
    let btn = button(
        container(icon.size(15).color(color))
            .center_x(Length::Fixed(34.0))
            .center_y(Length::Fixed(28.0)),
    )
    .padding(0)
    .on_press(msg)
    .style(move |_, status| {
        // Selected tab keeps its accent tint; an unselected tab fills with
        // bg_hover on hover/press for clear pointer feedback.
        let bg = if active {
            Color { a: 0.15, ..OryxisColors::t().accent }
        } else {
            match status {
                BtnStatus::Hovered | BtnStatus::Pressed => OryxisColors::t().bg_hover,
                _ => Color::TRANSPARENT,
            }
        };
        button::Style {
            background: Some(Background::Color(bg)),
            border: Border { radius: Radius::from(6.0), ..Default::default() },
            ..Default::default()
        }
    });
    icon_tooltip(btn.into(), tip)
}

/// Wrap an icon control in a small bottom-anchored tooltip, the shared
/// affordance for the sidebar tab strip and close affordances.
pub(crate) fn icon_tooltip<'a>(inner: Element<'a, Message>, tip: &'a str) -> Element<'a, Message> {
    iced::widget::tooltip(
        inner,
        container(text(tip).size(11).color(OryxisColors::t().text_primary))
            .padding(Padding { top: 4.0, right: 8.0, bottom: 4.0, left: 8.0 })
            .style(|_| container::Style {
                background: Some(Background::Color(OryxisColors::t().bg_surface)),
                border: Border {
                    radius: Radius::from(6.0),
                    color: OryxisColors::t().border,
                    width: 1.0,
                },
                ..Default::default()
            }),
        iced::widget::tooltip::Position::Bottom,
    )
    .into()
}


/// Compact human-readable byte count for the transfer overlay
/// (1 decimal past KB; integers stay integers).
fn fmt_bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = n as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{n} {}", UNITS[0])
    } else {
        format!("{v:.1} {}", UNITS[u])
    }
}

pub(crate) fn chat_header_btn<'a>(
    icon: iced::widget::Text<'a>,
    msg: Message,
) -> Element<'a, Message> {
    button(
        container(icon.size(13).color(OryxisColors::t().text_muted))
            .center_x(Length::Fixed(28.0))
            .center_y(Length::Fixed(24.0)),
    )
    .padding(0)
    .on_press(msg)
    .style(|_, status| {
        // Fill with bg_hover on hover/press so close/reset/action icons
        // give the same pointer feedback as the rest of the chrome.
        let bg = match status {
            BtnStatus::Hovered | BtnStatus::Pressed => OryxisColors::t().bg_hover,
            _ => Color::TRANSPARENT,
        };
        button::Style {
            background: Some(Background::Color(bg)),
            border: Border { radius: Radius::from(4.0), ..Default::default() },
            ..Default::default()
        }
    })
    .into()
}
