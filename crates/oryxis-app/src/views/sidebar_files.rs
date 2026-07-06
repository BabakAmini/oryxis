//! Files sidebar tab: a compact remote browser for the focused pane's
//! SSH session (an SFTP channel multiplexed on the live handle). No
//! host header, the host is the tab's own; just the current path, the
//! follow-cwd pin, hidden/refresh/expand actions and the entry list.
//! Rows follow the History tab's conventions: hover-revealed floating
//! action, click = primary (folders navigate, files copy their path),
//! all recorded into the sidebar keynav layer.

use iced::border::Radius;
use iced::widget::{column, container, text, MouseArea, Space};
use iced::{Background, Border, Element, Length, Padding};

use super::terminal::chat_header_btn;
use crate::app::{Message, Oryxis};
use crate::dispatch_sidebar_files::{files_join, files_parent_dir};
use crate::i18n::t;
use crate::state::TerminalSidebarTab;
use crate::theme::OryxisColors;
use crate::widgets::dir_row;

impl Oryxis {
    pub(crate) fn files_tab_content<'a>(
        &'a self,
        tab: &'a crate::state::TerminalTab,
    ) -> Element<'a, Message> {
        let pane = tab.active();
        let files = &pane.files;

        // Disconnected mid-view (the tab button hides on the next
        // frame; this covers the one where it hasn't yet).
        if pane.session.as_ref().and_then(|s| s.ssh()).is_none() {
            return sidebar_placeholder(t("files_no_session"));
        }

        // ── Header: path + follow pin + hidden / refresh / expand ──
        // Actions recorded in display order (the strip's Close came
        // first, recorded by `view_terminal_sidebar`).
        let stab = TerminalSidebarTab::Files;
        let follow = files.follow();
        // The path is clickable (owner QA: manage the directory by
        // typing, like the SFTP pane's breadcrumb): click swaps the
        // label for a text input; Enter commits (canonicalize + list),
        // Esc via the sidebar router cancels.
        let path_el: Element<'_, Message> = if let Some(editing) = &files.path_editing {
            self.sidebar_nav_slot(
                crate::keynav::SidebarRow::input(iced::widget::Id::new("sidebar-files-path")),
                stab,
                crate::widgets::INPUT_RADIUS,
                iced::widget::text_input("/", editing)
                    .id(iced::widget::Id::new("sidebar-files-path"))
                    .on_input(Message::SidebarFilesEditPath)
                    .on_submit(Message::SidebarFilesCommitPath)
                    .padding(4)
                    .size(12)
                    .font(iced::Font::MONOSPACE)
                    .style(crate::widgets::rounded_input_style)
                    .width(Length::Fill)
                    .into(),
            )
        } else {
            let path_label = if files.path.is_empty() {
                String::from("…")
            } else {
                truncate_path_leading(&files.path, 34)
            };
            let label = MouseArea::new(
                text(path_label)
                    .size(12)
                    .font(iced::Font::MONOSPACE)
                    .color(OryxisColors::t().text_secondary)
                    .width(Length::Fill),
            )
            .on_press(Message::SidebarFilesStartEditPath)
            .interaction(iced::mouse::Interaction::Text);
            self.sidebar_nav_slot(
                crate::keynav::SidebarRow::button(Message::SidebarFilesStartEditPath),
                stab,
                6.0,
                label.into(),
            )
        };
        let pin_btn = self.sidebar_nav_slot(
            crate::keynav::SidebarRow::button(Message::SidebarFilesToggleFollow),
            stab,
            6.0,
            action_btn(
                if follow {
                    iced_fonts::lucide::pin().color(OryxisColors::t().accent)
                } else {
                    iced_fonts::lucide::pin_off()
                },
                Message::SidebarFilesToggleFollow,
                // While following, the tooltip names the cwd SOURCE so a
                // test tells OSC 7 (exact) from the title fallback
                // (heuristic) apart at a glance.
                if !follow {
                    t("files_follow_off_tip")
                } else if pane.cwd_from_osc7 {
                    t("files_follow_on_osc7_tip")
                } else {
                    t("files_follow_on_title_tip")
                },
            ),
        );
        let hidden_btn = self.sidebar_nav_slot(
            crate::keynav::SidebarRow::button(Message::SidebarFilesToggleHidden),
            stab,
            6.0,
            action_btn(
                if files.show_hidden {
                    iced_fonts::lucide::eye()
                } else {
                    iced_fonts::lucide::eye_off()
                },
                Message::SidebarFilesToggleHidden,
                if files.show_hidden { t("hide_hidden_files") } else { t("show_hidden_files") },
            ),
        );
        let refresh_btn = self.sidebar_nav_slot(
            crate::keynav::SidebarRow::button(Message::SidebarFilesRefresh),
            stab,
            6.0,
            action_btn(
                iced_fonts::lucide::rotate_cw(),
                Message::SidebarFilesRefresh,
                t("refresh"),
            ),
        );
        let expand_btn = self.sidebar_nav_slot(
            crate::keynav::SidebarRow::button(Message::SidebarFilesExpand),
            stab,
            6.0,
            action_btn(
                iced_fonts::lucide::folder_tree(),
                Message::SidebarFilesExpand,
                t("open_sftp_session_here"),
            ),
        );
        let header = container(
            dir_row(vec![
                path_el,
                Space::new().width(4).into(),
                pin_btn,
                hidden_btn,
                refresh_btn,
                expand_btn,
            ])
            .align_y(iced::Alignment::Center),
        )
        .padding(Padding { top: 10.0, right: 10.0, bottom: 8.0, left: 12.0 })
        .width(Length::Fill);

        // ── Body ──
        let body: Element<'_, Message> = if let Some(err) = &files.error {
            column![
                sidebar_placeholder(err),
                container(
                    self.sidebar_nav_slot(
                        crate::keynav::SidebarRow::button(Message::SidebarFilesRefresh),
                        stab,
                        6.0,
                        action_btn(
                            iced_fonts::lucide::rotate_cw(),
                            Message::SidebarFilesRefresh,
                            t("retry"),
                        ),
                    )
                )
                .center_x(Length::Fill)
                .padding(Padding { top: 8.0, right: 0.0, bottom: 0.0, left: 0.0 }),
            ]
            .into()
        } else if files.client.is_none() || (files.loading && files.entries.is_empty()) {
            sidebar_placeholder(t("files_mounting"))
        } else {
            let mut list = column![]
                .spacing(4)
                .padding(Padding { top: 0.0, right: 12.0, bottom: 12.0, left: 12.0 });
            let mut pos = 0usize;
            // Inline "new file / new folder" input at the top of the
            // list (Enter creates, Esc via the sidebar router cancels).
            if let Some((kind, input)) = &files.new_entry {
                let icon = match kind {
                    crate::state::SftpEntryKind::Folder => iced_fonts::lucide::folder_plus(),
                    crate::state::SftpEntryKind::File => iced_fonts::lucide::file_plus(),
                };
                let field = iced::widget::text_input(t("name"), input)
                    .id(iced::widget::Id::new("sidebar-files-new"))
                    .on_input(Message::SidebarFilesNewEntryInput)
                    .on_submit(Message::SidebarFilesNewEntryCommit)
                    .padding(6)
                    .size(12)
                    .style(crate::widgets::rounded_input_style);
                let row = dir_row(vec![
                    icon.size(13).color(OryxisColors::t().accent).into(),
                    Space::new().width(8).into(),
                    field.into(),
                ])
                .align_y(iced::Alignment::Center);
                list = list.push(self.sidebar_nav_slot(
                    crate::keynav::SidebarRow::input(iced::widget::Id::new(
                        "sidebar-files-new",
                    )),
                    TerminalSidebarTab::Files,
                    crate::widgets::INPUT_RADIUS,
                    container(row)
                        .padding(Padding { top: 2.0, right: 0.0, bottom: 2.0, left: 2.0 })
                        .into(),
                ));
                pos += 1;
            }
            // ".." row, hidden at the root.
            if let Some(parent) = files_parent_dir(&files.path) {
                list = list.push(self.files_row(
                    "..",
                    true,
                    false,
                    0,
                    Message::SidebarFilesNavigate(parent),
                    None,
                    pos,
                ));
                pos += 1;
            }
            let mut any = false;
            for entry in &files.entries {
                if !files.show_hidden && entry.name.starts_with('.') {
                    continue;
                }
                any = true;
                let full = files_join(&files.path, &entry.name);
                // Inline rename swaps this row's label for an input
                // (Enter commits, Esc via the sidebar router cancels).
                if let Some((rpath, rinput)) = &files.rename
                    && rpath == &full
                {
                    let field = iced::widget::text_input("", rinput)
                        .id(iced::widget::Id::new("sidebar-files-rename"))
                        .on_input(Message::SidebarFilesRenameInput)
                        .on_submit(Message::SidebarFilesRenameCommit)
                        .padding(6)
                        .size(12)
                        .style(crate::widgets::rounded_input_style);
                    let row = dir_row(vec![
                        crate::views::sftp::file_icon(
                            &entry.name,
                            entry.is_dir,
                            entry.is_symlink,
                        )
                        .into(),
                        Space::new().width(8).into(),
                        field.into(),
                    ])
                    .align_y(iced::Alignment::Center);
                    list = list.push(self.sidebar_nav_slot(
                        crate::keynav::SidebarRow::input(iced::widget::Id::new(
                            "sidebar-files-rename",
                        )),
                        TerminalSidebarTab::Files,
                        crate::widgets::INPUT_RADIUS,
                        container(row)
                            .padding(Padding {
                                top: 2.0,
                                right: 0.0,
                                bottom: 2.0,
                                left: 2.0,
                            })
                            .into(),
                    ));
                    pos += 1;
                    continue;
                }
                let primary = if entry.is_dir {
                    Message::SidebarFilesNavigate(full.clone())
                } else {
                    // Files: the row's primary is Copy path (toast
                    // feedback); heavier actions live in the context
                    // menu (right-click) and the full SFTP session.
                    Message::SftpCopyPath(full.clone())
                };
                list = list.push(self.files_row(
                    &entry.name,
                    entry.is_dir,
                    entry.is_symlink,
                    entry.size,
                    primary,
                    Some(full),
                    pos,
                ));
                pos += 1;
            }
            if !any {
                list = list.push(sidebar_placeholder(t("files_empty")));
            }
            // Shared id with the Snippets / History lists (only one
            // renders): the sidebar keynav router snaps the ringed row
            // into view by it. Right-clicking the empty area opens the
            // directory-level menu (rows consume their own right-press
            // first).
            let scroll = iced::widget::scrollable(list)
                .id(iced::widget::Id::new("sidebar-list-scroll"))
                .width(Length::Fill)
                .height(Length::Fill);
            MouseArea::new(scroll)
                .on_right_press(Message::ShowSidebarFilesBackgroundMenu)
                .into()
        };

        column![header, body]
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    /// One browser row, recorded into the sidebar keynav layer (Enter =
    /// the row's primary: folders navigate, files copy their path).
    /// `full_path` enables the hover-revealed Copy path action; the
    /// ".." row has none.
    #[allow(clippy::too_many_arguments)]
    fn files_row<'a>(
        &'a self,
        name: &'a str,
        is_dir: bool,
        is_symlink: bool,
        size: u64,
        primary: Message,
        full_path: Option<String>,
        pos: usize,
    ) -> Element<'a, Message> {
        let c = OryxisColors::t();
        let hovered = self.hovered_files_row == Some(pos);

        let mut cells: Vec<Element<'a, Message>> = vec![
            crate::views::sftp::file_icon(name, is_dir, is_symlink).into(),
            Space::new().width(8).into(),
            text(name)
                .size(12)
                .color(c.text_primary)
                .wrapping(iced::widget::text::Wrapping::None)
                .width(Length::Fill)
                .into(),
        ];
        if !is_dir {
            cells.push(Space::new().width(6).into());
            cells.push(
                text(crate::views::sftp::format_size(size))
                    .size(11)
                    .color(c.text_muted)
                    .into(),
            );
        }
        let card = container(dir_row(cells).align_y(iced::Alignment::Center))
            .padding(Padding { top: 6.0, right: 10.0, bottom: 6.0, left: 10.0 })
            .width(Length::Fill)
            .style(|_| container::Style {
                background: Some(Background::Color(OryxisColors::t().bg_surface)),
                border: Border { radius: Radius::from(6.0), ..Default::default() },
                ..Default::default()
            });

        // Hover-revealed floating Copy path (the card-action convention;
        // the ring border stays the keyboard affordance).
        let row_el: Element<'a, Message> = match (&full_path, hovered) {
            (Some(full), true) => {
                let actions = container(action_btn(
                    iced_fonts::lucide::clipboard_copy(),
                    Message::SftpCopyPath(full.clone()),
                    t("copy_path"),
                ))
                .padding(Padding { top: 2.0, right: 4.0, bottom: 2.0, left: 4.0 })
                .style(|_| container::Style {
                    background: Some(Background::Color(OryxisColors::t().bg_selected)),
                    border: Border { radius: Radius::from(6.0), ..Default::default() },
                    ..Default::default()
                });
                let overlay = container(actions)
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .align_x(iced::alignment::Horizontal::Right)
                    .align_y(iced::alignment::Vertical::Center)
                    .padding(Padding { top: 0.0, right: 4.0, bottom: 0.0, left: 0.0 });
                iced::widget::Stack::new().push(card).push(overlay).into()
            }
            _ => card.into(),
        };

        let mut area = MouseArea::new(row_el)
            .on_enter(Message::SidebarFilesRowHovered(pos))
            .on_exit(Message::SidebarFilesRowUnhovered)
            .on_press(primary.clone())
            .interaction(iced::mouse::Interaction::Pointer);
        // Right-click opens the row's context menu (Open / Open SFTP
        // session here / Copy path / Copy name); the ".." row has none.
        if let Some(full) = &full_path {
            area = area
                .on_right_press(Message::ShowSidebarFilesRowMenu(full.clone(), is_dir));
        }

        self.sidebar_nav_slot(
            crate::keynav::SidebarRow::button(primary),
            TerminalSidebarTab::Files,
            6.0,
            area.into(),
        )
    }
}

/// Centered muted text for the empty / mounting / error states.
fn sidebar_placeholder(label: &str) -> Element<'_, Message> {
    container(text(label).size(12).color(OryxisColors::t().text_muted))
        .center_x(Length::Fill)
        .padding(Padding { top: 40.0, right: 12.0, bottom: 0.0, left: 12.0 })
        .width(Length::Fill)
        .into()
}

/// An icon action with a tooltip (same chrome as the History-row actions).
fn action_btn<'a>(
    icon: iced::widget::Text<'a>,
    msg: Message,
    tip: &'a str,
) -> Element<'a, Message> {
    iced::widget::tooltip(
        chat_header_btn(icon, msg),
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
        iced::widget::tooltip::Position::Top,
    )
    .into()
}

/// Truncate an absolute path from the LEADING side so the tail (the
/// directory the user is in) stays visible: `…/var/www/html`.
fn truncate_path_leading(path: &str, max_chars: usize) -> String {
    let count = path.chars().count();
    if count <= max_chars {
        return path.to_string();
    }
    let tail: String = path
        .chars()
        .skip(count - max_chars.saturating_sub(1))
        .collect();
    // Cut at the next separator so the head component isn't half a name.
    match tail.find('/') {
        Some(idx) => format!("…{}", &tail[idx..]),
        None => format!("…{tail}"),
    }
}
