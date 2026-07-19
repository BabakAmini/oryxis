//! Settings screen, terminal, AI, theme, shortcuts, security, sync, about.

pub(crate) use iced::border::Radius;
pub(crate) use iced::widget::{button, checkbox, container, pick_list, scrollable, text, text_input, Space};
// `column` carries both a fn and a `column!` macro; re-exporting it through the
// `use super::*` glob makes the macro ambiguous in the section submodules, so it
// is imported directly here and in each section file instead.
use iced::widget::column;
pub(crate) use iced::widget::button::Status as BtnStatus;
pub(crate) use iced::{Background, Border, Color, Element, Length, Padding};

pub(crate) use crate::app::{SettingsMessage, McpMessage, NavigationMessage, CommandHistoryMessage, UpdateMessage, ProxyIdentityMessage, AgentMessage, ZmodemMessage, Message, Oryxis, VaultMessage, AiMessage, ShareMessage, SyncMessage, NAV_RAIL_WIDTH_EXPANDED};
pub(crate) use crate::i18n::t;
pub(crate) use crate::state::SettingsSection;
pub(crate) use crate::theme::OryxisColors;
pub(crate) use crate::widgets::{
    dir_align_x, dir_row, key_badge, panel_field, panel_section, settings_row, shortcut_row,
    styled_button, styled_button_opt,
};

// Per-section view methods, split into sibling files.
mod about;
mod advanced;
mod agent;
mod ai;
mod connection;
mod host_picker;
mod interface;
mod local_terminals;
mod mcp;
mod previews;
mod proxies;
mod security;
mod sftp;
mod shortcuts;
mod sync;
mod terminal;

// `host_badge` is shared with the section submodules through the
// `use super::*` glob (the Sync section renders it in its host picker
// trigger), so it is re-exported here.
pub(crate) use host_picker::host_badge;
use host_picker::sync_host_picker_modal;

impl Oryxis {
    pub(crate) fn view_settings(&self) -> Element<'_, Message> {
        // ── Settings sidebar ──
        let settings_sidebar = {
            // Order: most-touched at the top (visual + everyday
            // configuration), then per-feature toggles, then network
            // resources, then plugin / system / about. The previous
            // order was historical (followed the implementation
            // sequence) and didn't reflect how users actually move
            // through the panel.
            // Core sections, then the "feature plugin" sections (AI /
            // MCP / SFTP / Sync / SSH Agent / Cloud Sync) which only
            // appear once the feature is enabled on the Plugins screen,
            // then About. The
            // enable/disable toggles live on the Plugins screen, not here.
            // The list (with its feature gating) is shared with the
            // command palette's "Settings: X" rows via this helper.
            let items = self.settings_section_items();
            // Record the visible section list for the keyboard router
            // (the SubNav zone here): the set is dynamic (feature
            // toggles hide sections), so it must come from this exact
            // list, not the enum.
            *self.keynav.subnav_items.borrow_mut() = items
                .iter()
                .map(|(_, s)| crate::keynav::NavItem::SettingsSection(*s))
                .collect();
            let kb_sel = match self.keynav.selected_in(crate::keynav::FocusZone::SubNav) {
                Some(crate::keynav::NavItem::SettingsSection(s)) => Some(s),
                _ => None,
            };
            let mut col = column![]
                .spacing(4)
                .padding(Padding { top: 8.0, right: 8.0, bottom: 8.0, left: 8.0 });

            for (label, section) in items {
                let is_active = self.settings_section == section;
                let kb_selected = kb_sel == Some(section);
                let bg = if is_active {
                    Color { a: 0.15, ..OryxisColors::t().accent }
                } else {
                    Color::TRANSPARENT
                };
                let fg = if is_active {
                    OryxisColors::t().accent
                } else {
                    OryxisColors::t().text_secondary
                };
                let btn: Element<'_, Message> = button(
                    container(text(label).size(13).color(fg))
                        .width(Length::Fill)
                        .align_x(crate::widgets::dir_align_x())
                        .padding(Padding { top: 12.0, right: 16.0, bottom: 12.0, left: 16.0 }),
                )
                .on_press(Message::Settings(SettingsMessage::ChangeSettingsSection(section)))
                // Zero the button's default padding so the container's
                // 16/12 is the exact content inset.
                .padding(0)
                .width(Length::Fill)
                .style(move |_, status| {
                    let hover_bg = match status {
                        BtnStatus::Hovered if !is_active => Color::from_rgba(1.0, 1.0, 1.0, 0.08),
                        BtnStatus::Pressed => Color { a: 0.25, ..OryxisColors::t().accent },
                        // Keyboard selection reads on active rows too
                        // (border alone vanishes on the accent tint).
                        _ if kb_selected && is_active => Color { a: 0.30, ..OryxisColors::t().accent },
                        _ if kb_selected => Color::from_rgba(1.0, 1.0, 1.0, 0.10),
                        _ => bg,
                    };
                    button::Style {
                        background: Some(Background::Color(hover_bg)),
                        border: Border {
                            radius: Radius::from(10.0),
                            color: if kb_selected {
                                OryxisColors::t().accent
                            } else {
                                Color::TRANSPARENT
                            },
                            width: if kb_selected { 2.0 } else { 0.0 },
                        },
                        ..Default::default()
                    }
                })
                .into();
                col = col.push(btn);
            }

            // Wrap the section list in a scrollable so a short window
            // doesn't clip the bottom entries (About / Plugins were
            // disappearing when the height dropped below ~520 px).
            // Width matches the main vertical nav rail; no side hairline
            // so it reads as the same sidebar surface.
            container(
                scrollable(col)
                    // Stable id so the keyboard router can keep the
                    // selected section in view on short windows.
                    .id(iced::widget::Id::new("settings-sidebar-scroll"))
                    .height(Length::Fill),
            )
            .width(NAV_RAIL_WIDTH_EXPANDED)
            .height(Length::Fill)
            .style(|_| container::Style {
                background: Some(Background::Color(OryxisColors::t().bg_sidebar)),
                ..Default::default()
            })
        };

        // ── Settings content ──
        let settings_content: Element<'_, Message> = match self.settings_section {
            SettingsSection::Terminal => self.view_settings_terminal(),

            SettingsSection::Connection => self.view_settings_connection(),

            SettingsSection::Sftp => self.view_settings_sftp(),

            SettingsSection::AI => self.view_settings_ai(),

            SettingsSection::Interface => self.view_settings_interface(),

            SettingsSection::Shortcuts => self.view_settings_shortcuts(),

            SettingsSection::Security => self.view_settings_security(),

            SettingsSection::Sync => self.view_settings_sync(),

            SettingsSection::Agent => self.view_settings_agent(),

            SettingsSection::Advanced => self.view_settings_advanced(),
            SettingsSection::About => self.view_settings_about(),
            SettingsSection::Cloud => self.view_cloud_sync_settings(),
            SettingsSection::Plugins => self.view_plugins_panel(),
            SettingsSection::Mcp => self.view_settings_mcp(),
        };

        let layout = container(crate::widgets::dir_row(vec![
            settings_sidebar.into(),
            container(settings_content)
                .width(Length::Fill)
                .height(Length::Fill)
                .into(),
        ]))
        .width(Length::Fill)
        .height(Length::Fill);

        // Overlay the SFTP-sync host picker modal across the whole page
        // when open (same scrim + centered dialog pattern as the SFTP
        // file browser's picker).
        if self.sync.sftp.picker_open {
            iced::widget::Stack::new()
                .push(layout)
                .push(sync_host_picker_modal(self))
                .width(Length::Fill)
                .height(Length::Fill)
                .into()
        } else {
            layout.into()
        }
    }
}

/// i18n key for an export/import category's checkbox label.
pub(crate) fn category_label_key(c: oryxis_vault::ExportCategory) -> &'static str {
    use oryxis_vault::ExportCategory as C;
    match c {
        C::Connections => "cat_connections",
        C::Groups => "cat_groups",
        C::Keys => "cat_keys",
        C::Identities => "cat_identities",
        C::ProxyIdentities => "cat_proxies",
        C::CloudProfiles => "cat_cloud_profiles",
        C::Snippets => "cat_snippets",
        C::KnownHosts => "cat_known_hosts",
        C::PortForwardRules => "cat_port_forwards",
        C::SessionGroups => "cat_session_layouts",
        C::Settings => "cat_settings",
    }
}
