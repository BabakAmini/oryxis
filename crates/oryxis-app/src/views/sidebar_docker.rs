//! Docker sidebar tab: containers, images, and compose projects on the
//! focused pane's host.
//!
//! The probe, the start/stop/restart/rm, and the compose up/down all
//! run docker itself on an exec channel over the pane's live session,
//! so nothing is installed on the host.

use iced::border::Radius;
use iced::widget::button::Status as BtnStatus;
use iced::widget::{button, column, container, text, Space};
use iced::{Background, Border, Color, Element, Length, Padding};

use crate::app::{DockerMessage, Message, Oryxis};
use crate::docker::model::{DockerData, DockerPanel, DockerStatus};
use crate::i18n::t;
use crate::state::TerminalSidebarTab;
use crate::theme::OryxisColors;
use crate::widgets::dir_row;

impl Oryxis {
    pub(crate) fn docker_tab_content<'a>(
        &'a self,
        tab: &'a crate::state::TerminalTab,
    ) -> Element<'a, Message> {
        let Some(_tab_idx) = self.active_tab else {
            return placeholder(t("docker_no_session"));
        };
        if tab.active().session.as_ref().and_then(|s| s.ssh()).is_none() {
            return placeholder(t("docker_no_session"));
        }
        let pane_id = tab.active().id;
        let entry = self.docker.get(&pane_id);
        let status = entry.map(|e| &e.status).unwrap_or(&DockerStatus::Idle);

        let mut body = column![]
            .spacing(8)
            .padding(Padding { top: 10.0, right: 10.0, bottom: 12.0, left: 10.0 })
            .width(Length::Fill);

        body = body.push(self.docker_header(pane_id));

        let panel = entry.map(|e| e.panel).unwrap_or(DockerPanel::Containers);
        body = body.push(self.docker_panel_tabs(pane_id, panel));

        match status {
            DockerStatus::Idle | DockerStatus::Loading => {
                body = body.push(hint(t("docker_listing")));
            }
            DockerStatus::NoDocker => {
                body = body.push(hint(t("docker_not_installed")));
            }
            DockerStatus::Failed(e) => {
                body = body.push(hint(e));
            }
            DockerStatus::Ready(data) => match panel {
                DockerPanel::Containers => {
                    body = body.push(self.docker_containers_panel(pane_id, data, entry));
                }
                DockerPanel::Images => {
                    body = body.push(self.docker_images_panel(pane_id, data, entry));
                }
                DockerPanel::Compose => {
                    body = body.push(self.docker_compose_panel(pane_id, data, entry));
                }
            },
        }

        if let Some(err) = entry.and_then(|e| e.error.as_deref()) {
            body = body.push(
                container(text(err.to_string()).size(11).color(OryxisColors::t().error))
                    .padding(Padding { top: 2.0, right: 4.0, bottom: 0.0, left: 4.0 }),
            );
        }

        iced::widget::scrollable(body)
            .id(crate::keynav::sidebar_scroll_id(crate::state::TerminalSidebarTab::Docker))
            .height(Length::Fill)
            .into()
    }

    fn docker_header(&self, pane_id: uuid::Uuid) -> Element<'_, Message> {
        let refresh = crate::views::terminal::icon_tooltip(
            button(
                iced_fonts::lucide::refresh_cw()
                    .size(13)
                    .color(OryxisColors::t().text_secondary),
            )
            .on_press(Message::Docker(DockerMessage::Refresh(pane_id)))
            .padding(Padding { top: 4.0, right: 6.0, bottom: 4.0, left: 6.0 })
            .style(icon_btn_style)
            .into(),
            t("docker_refresh"),
        );
        dir_row(vec![
            text(t("docker_title"))
                .size(11)
                .color(OryxisColors::t().text_secondary)
                .into(),
            Space::new().width(Length::Fill).into(),
            self.sidebar_nav_slot(
                crate::keynav::SidebarRow::button(Message::Docker(
                    DockerMessage::Refresh(pane_id),
                )),
                TerminalSidebarTab::Docker,
                6.0,
                refresh,
            ),
        ])
        .align_y(iced::Alignment::Center)
        .width(Length::Fill)
        .into()
    }

    fn docker_panel_tabs(
        &self,
        pane_id: uuid::Uuid,
        current: DockerPanel,
    ) -> Element<'_, Message> {
        let items: Vec<(&str, DockerPanel)> = vec![
            (t("docker_containers"), DockerPanel::Containers),
            (t("docker_images"), DockerPanel::Images),
            (t("docker_compose"), DockerPanel::Compose),
        ];
        let mut btns: Vec<Element<'_, Message>> = Vec::new();
        for (label, panel) in items {
            let active = current == panel;
            let code = match panel {
                DockerPanel::Containers => 0u8,
                DockerPanel::Images => 1,
                DockerPanel::Compose => 2,
            };
            let msg = Message::Docker(DockerMessage::SwitchPanel(pane_id, code));
            btns.push(
                button(
                    text(label.to_string())
                        .size(11)
                        .color(if active {
                            OryxisColors::t().accent
                        } else {
                            OryxisColors::t().text_muted
                        }),
                )
                .on_press(msg)
                .padding(Padding { top: 3.0, right: 8.0, bottom: 3.0, left: 8.0 })
                .style(if active {
                    tab_active_style as fn(&iced::Theme, BtnStatus) -> button::Style
                } else {
                    tab_inactive_style
                })
                .into(),
            );
        }

        dir_row(btns)
            .spacing(4)
            .width(Length::Fill)
            .into()
    }

    fn docker_containers_panel<'a>(
        &'a self,
        pane_id: uuid::Uuid,
        data: &'a DockerData,
        entry: Option<&'a crate::docker::model::PaneDocker>,
    ) -> Element<'a, Message> {
        let filter = entry.map(|e| e.container_filter.as_str()).unwrap_or("");
        let confirm_stop = entry.and_then(|e| e.confirm_stop.as_deref());
        let confirm_remove = entry.and_then(|e| e.confirm_remove.as_deref());

        let mut col = column![].spacing(4).width(Length::Fill);

        let filter_input = iced::widget::text_input(t("docker_filter"), filter)
            .on_input(move |v| Message::Docker(DockerMessage::ContainerFilterChanged(pane_id, v)))
            .size(11)
            .padding(Padding { top: 3.0, right: 6.0, bottom: 3.0, left: 6.0 });
        col = col.push(filter_input);

        let filtered: Vec<_> = data
            .containers
            .iter()
            .filter(|c| {
                filter.is_empty()
                    || c.name.to_lowercase().contains(&filter.to_lowercase())
                    || c.image.to_lowercase().contains(&filter.to_lowercase())
            })
            .collect();

        if filtered.is_empty() {
            col = col.push(hint(t("docker_no_containers")));
        }

        for c in &filtered {
            if confirm_stop == Some(c.name.as_str()) {
                col = col.push(self.docker_action_confirm(
                    pane_id,
                    format!("Stop \u{201c}{}\u{201d}?", c.name),
                    Message::Docker(DockerMessage::ConfirmStop(pane_id)),
                    Message::Docker(DockerMessage::CancelStop(pane_id)),
                    t("docker_stop").to_string(),
                ));
            } else if confirm_remove == Some(c.name.as_str()) {
                col = col.push(self.docker_action_confirm(
                    pane_id,
                    format!("Remove \u{201c}{}\u{201d}?", c.name),
                    Message::Docker(DockerMessage::ConfirmRemove(pane_id)),
                    Message::Docker(DockerMessage::CancelRemove(pane_id)),
                    t("docker_remove").to_string(),
                ));
            } else {
                col = col.push(self.docker_container_row(pane_id, c));
            }
        }

        col.into()
    }

    fn docker_container_row<'a>(
        &'a self,
        pane_id: uuid::Uuid,
        container: &'a crate::docker::model::DockerContainer,
    ) -> Element<'a, Message> {
        let running = container.state == "running";
        let state_color = if running {
            OryxisColors::t().accent
        } else {
            OryxisColors::t().text_muted
        };

        let meta: Vec<Element<'a, Message>> = vec![
            text(container.image.clone())
                .size(10)
                .color(OryxisColors::t().text_muted)
                .into(),
            Space::new().width(6).into(),
            text(if running {
                t("docker_running")
            } else {
                t("docker_stopped")
            })
            .size(10)
            .color(state_color)
            .into(),
        ];

        let row_body = column![
            text(container.name.clone())
                .size(12)
                .color(OryxisColors::t().text_primary),
            dir_row(meta).align_y(iced::Alignment::Center),
        ]
        .spacing(2)
        .width(Length::Fill);

        let mut actions: Vec<Element<'a, Message>> = Vec::new();

        if running {
            actions.push(icon_action_btn(
                iced_fonts::lucide::square(),
                t("docker_stop"),
                Message::Docker(DockerMessage::AskStop(pane_id, container.name.clone())),
                OryxisColors::t().warning,
            ));
            actions.push(icon_action_btn(
                iced_fonts::lucide::refresh_cw(),
                t("docker_restart"),
                Message::Docker(DockerMessage::RestartContainer(
                    pane_id,
                    container.name.clone(),
                )),
                OryxisColors::t().accent,
            ));
        } else {
            actions.push(icon_action_btn(
                iced_fonts::lucide::play(),
                t("docker_start"),
                Message::Docker(DockerMessage::StartContainer(
                    pane_id,
                    container.name.clone(),
                )),
                OryxisColors::t().accent,
            ));
        }

        actions.push(icon_action_btn(
            iced_fonts::lucide::trash(),
            t("docker_remove"),
            Message::Docker(DockerMessage::AskRemove(pane_id, container.name.clone())),
            OryxisColors::t().error,
        ));

        let action_row = dir_row(actions).spacing(2);

        let stack_body = column![row_body]
            .width(Length::Fill)
            .push(
                dir_row(vec![
                    Space::new().width(Length::Fill).into(),
                    action_row.into(),
                ])
                .width(Length::Fill),
            );

        let row = button(stack_body)
            .padding(Padding { top: 6.0, right: 8.0, bottom: 6.0, left: 8.0 })
            .width(Length::Fill)
            .style(row_btn_style);

        self.sidebar_nav_slot(
            crate::keynav::SidebarRow::button(Message::NoOp),
            TerminalSidebarTab::Docker,
            6.0,
            row.into(),
        )
    }

    fn docker_action_confirm(
        &self,
        _pane_id: uuid::Uuid,
        message: String,
        confirm: Message,
        cancel: Message,
        action_label: String,
    ) -> Element<'_, Message> {
        container(
            column![
                text(message)
                    .size(11)
                    .color(OryxisColors::t().text_primary),
                dir_row(vec![
                    self.sidebar_nav_slot(
                        crate::keynav::SidebarRow::button(confirm.clone()),
                        TerminalSidebarTab::Docker,
                        6.0,
                        button(text(action_label).size(12).color(Color::WHITE))
                            .on_press(confirm)
                            .padding(Padding { top: 6.0, right: 12.0, bottom: 6.0, left: 12.0 })
                            .style(|_: &iced::Theme, _: BtnStatus| button::Style {
                                background: Some(Background::Color(OryxisColors::t().error)),
                                border: Border { radius: Radius::from(6.0), ..Default::default() },
                                ..Default::default()
                            })
                            .into(),
                    ),
                    Space::new().width(6).into(),
                    self.sidebar_nav_slot(
                        crate::keynav::SidebarRow::button(cancel.clone()),
                        TerminalSidebarTab::Docker,
                        6.0,
                        crate::widgets::styled_button(
                            t("cancel"),
                            cancel,
                            OryxisColors::t().text_secondary,
                        ),
                    ),
                ])
                .align_y(iced::Alignment::Center),
            ]
            .spacing(6),
        )
        .padding(Padding { top: 8.0, right: 8.0, bottom: 8.0, left: 8.0 })
        .width(Length::Fill)
        .style(|_| container::Style {
            background: Some(Background::Color(OryxisColors::t().bg_surface)),
            border: Border {
                radius: Radius::from(6.0),
                color: OryxisColors::t().error,
                width: 1.0,
            },
            ..Default::default()
        })
        .into()
    }

    fn docker_images_panel<'a>(
        &'a self,
        pane_id: uuid::Uuid,
        data: &'a DockerData,
        entry: Option<&'a crate::docker::model::PaneDocker>,
    ) -> Element<'a, Message> {
        let filter = entry.map(|e| e.image_filter.as_str()).unwrap_or("");

        let mut col = column![].spacing(4).width(Length::Fill);

        let filter_input = iced::widget::text_input(t("docker_filter"), filter)
            .on_input(move |v| Message::Docker(DockerMessage::ImageFilterChanged(pane_id, v)))
            .size(11)
            .padding(Padding { top: 3.0, right: 6.0, bottom: 3.0, left: 6.0 });
        col = col.push(filter_input);

        let filtered: Vec<_> = data
            .images
            .iter()
            .filter(|img| {
                filter.is_empty()
                    || img.repository.to_lowercase().contains(&filter.to_lowercase())
                    || img.tag.to_lowercase().contains(&filter.to_lowercase())
            })
            .collect();

        if filtered.is_empty() {
            col = col.push(hint(t("docker_no_images")));
        }

        for img in &filtered {
            let row_body = column![
                text(format!("{}:{}", img.repository, img.tag))
                    .size(12)
                    .color(OryxisColors::t().text_primary),
                dir_row(vec![
                    text(img.size.clone())
                        .size(10)
                        .color(OryxisColors::t().text_muted)
                        .into(),
                    Space::new().width(6).into(),
                    text(img.id.chars().take(12).collect::<String>())
                        .size(10)
                        .color(OryxisColors::t().text_muted)
                        .into(),
                ])
                .align_y(iced::Alignment::Center),
            ]
            .spacing(2)
            .width(Length::Fill);

            let row = button(row_body)
                .padding(Padding { top: 6.0, right: 8.0, bottom: 6.0, left: 8.0 })
                .width(Length::Fill)
                .style(row_btn_style);

            col = col.push(row);
        }

        col.into()
    }

    fn docker_compose_panel<'a>(
        &'a self,
        pane_id: uuid::Uuid,
        data: &'a DockerData,
        entry: Option<&'a crate::docker::model::PaneDocker>,
    ) -> Element<'a, Message> {
        let confirm_down = entry.and_then(|e| e.confirm_compose_down.as_deref());

        let mut col = column![].spacing(4).width(Length::Fill);

        if data.compose_projects.is_empty() {
            col = col.push(hint(t("docker_no_compose")));
        }

        for project in &data.compose_projects {
            if confirm_down == Some(project.file_path.as_str()) {
                col = col.push(self.docker_action_confirm(
                    pane_id,
                    "Bring down this compose project?".to_string(),
                    Message::Docker(DockerMessage::ConfirmComposeDown(pane_id)),
                    Message::Docker(DockerMessage::CancelComposeDown(pane_id)),
                    t("docker_compose_down").to_string(),
                ));
            } else {
                let file_label = std::path::Path::new(&project.file_path)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or(&project.file_path);

                let up_msg = Message::Docker(DockerMessage::ComposeUp(
                    pane_id,
                    project.file_path.clone(),
                ));
                let down_msg = Message::Docker(DockerMessage::AskComposeDown(
                    pane_id,
                    project.file_path.clone(),
                ));

                let row_body = column![
                    text(file_label.to_string())
                        .size(12)
                        .color(OryxisColors::t().text_primary),
                    text(project.file_path.clone())
                        .size(10)
                        .color(OryxisColors::t().text_muted),
                ]
                .spacing(2)
                .width(Length::Fill);

                let actions = dir_row(vec![
                    icon_action_btn(
                        iced_fonts::lucide::play(),
                        t("docker_compose_up"),
                        up_msg,
                        OryxisColors::t().accent,
                    ),
                    icon_action_btn(
                        iced_fonts::lucide::square(),
                        t("docker_compose_down"),
                        down_msg,
                        OryxisColors::t().error,
                    ),
                ])
                .spacing(2);

                let full_row = column![row_body]
                    .width(Length::Fill)
                    .push(
                        dir_row(vec![
                            Space::new().width(Length::Fill).into(),
                            actions.into(),
                        ])
                        .width(Length::Fill),
                    );

                let row = button(full_row)
                    .padding(Padding { top: 6.0, right: 8.0, bottom: 6.0, left: 8.0 })
                    .width(Length::Fill)
                    .style(row_btn_style);

                col = col.push(self.sidebar_nav_slot(
                    crate::keynav::SidebarRow::button(Message::NoOp),
                    TerminalSidebarTab::Docker,
                    6.0,
                    row.into(),
                ));
            }
        }

        col.into()
    }
}

// ── Helper widgets ──

fn hint(label: &str) -> Element<'_, Message> {
    container(text(label.to_string()).size(11).color(OryxisColors::t().text_muted))
        .padding(Padding { top: 10.0, right: 4.0, bottom: 4.0, left: 4.0 })
        .width(Length::Fill)
        .into()
}

fn placeholder(label: &str) -> Element<'_, Message> {
    container(text(label.to_string()).size(12).color(OryxisColors::t().text_muted))
        .center_x(Length::Fill)
        .padding(Padding { top: 40.0, right: 12.0, bottom: 0.0, left: 12.0 })
        .width(Length::Fill)
        .into()
}

fn icon_action_btn<'a>(
    icon: iced::widget::Text<'a>,
    label: &'a str,
    msg: Message,
    color: Color,
) -> Element<'a, Message> {
    crate::views::terminal::icon_tooltip(
        button(icon.size(12).color(color))
            .on_press(msg)
            .padding(Padding { top: 3.0, right: 5.0, bottom: 3.0, left: 5.0 })
            .style(icon_btn_style)
            .into(),
        label,
    )
}

fn row_btn_style(_: &iced::Theme, status: BtnStatus) -> button::Style {
    let bg = match status {
        BtnStatus::Hovered => OryxisColors::t().bg_hover,
        BtnStatus::Pressed => OryxisColors::t().bg_selected,
        _ => Color::TRANSPARENT,
    };
    button::Style {
        background: Some(Background::Color(bg)),
        border: Border { radius: Radius::from(6.0), ..Default::default() },
        ..Default::default()
    }
}

fn icon_btn_style(_: &iced::Theme, status: BtnStatus) -> button::Style {
    let bg = match status {
        BtnStatus::Hovered => OryxisColors::t().bg_hover,
        BtnStatus::Pressed => OryxisColors::t().bg_selected,
        _ => Color::TRANSPARENT,
    };
    button::Style {
        background: Some(Background::Color(bg)),
        border: Border { radius: Radius::from(6.0), ..Default::default() },
        ..Default::default()
    }
}

fn tab_active_style(_: &iced::Theme, _: BtnStatus) -> button::Style {
    button::Style {
        background: Some(Background::Color(OryxisColors::t().bg_surface)),
        border: Border {
            radius: Radius::from(4.0),
            ..Default::default()
        },
        ..Default::default()
    }
}

fn tab_inactive_style(_: &iced::Theme, status: BtnStatus) -> button::Style {
    let bg = match status {
        BtnStatus::Hovered => OryxisColors::t().bg_hover,
        _ => Color::TRANSPARENT,
    };
    button::Style {
        background: Some(Background::Color(bg)),
        border: Border { radius: Radius::from(4.0), ..Default::default() },
        ..Default::default()
    }
}
