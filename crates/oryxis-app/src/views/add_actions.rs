//! The "add a host" action catalog: the single source of truth behind
//! both the toolbar's "+ Host ▾" dropdown
//! (`build_menu_cloud_provider_picker`) and the first-run empty state's
//! action buttons (`dashboard_empty_state`). Two surfaces, one list, so
//! a new entry can never land on one and be forgotten on the other.

use iced::Color;

use crate::app::{EditorMessage, CloudMessage, ShareMessage, TabsMessage, Message, Oryxis};
use crate::os_icon::BrandIcon;
use crate::theme::OryxisColors;

/// One entry of the add catalog: how it looks, what it says, what it
/// fires. `label` borrows from `Oryxis` so cloud entries can carry the
/// profile's own name.
pub(crate) struct AddHostAction<'a> {
    pub(crate) icon: BrandIcon,
    pub(crate) label: &'a str,
    pub(crate) msg: Message,
    /// Icon tint: the brand color for cloud providers, a neutral
    /// secondary for the built-in actions.
    pub(crate) color: Color,
}

impl Oryxis {
    /// Every "add a host" action available right now, in display order:
    /// import a `.oryxis` file (a full vault export or a single shared
    /// host), import an OpenSSH `~/.ssh/config`, add a remote-desktop
    /// host (only with the opt-in feature on), export the current view
    /// (only with hosts to export), then one discovery entry per
    /// configured cloud profile. Import / export live here so they're
    /// reachable from where hosts are managed instead of being buried
    /// in Settings.
    pub(crate) fn add_host_actions(&self) -> Vec<AddHostAction<'_>> {
        let secondary = OryxisColors::t().text_secondary;
        let mut actions = vec![
            AddHostAction {
                icon: iced_fonts::lucide::download().into(),
                label: crate::i18n::t("import_from_file"),
                msg: Message::Share(ShareMessage::ImportVault),
                color: secondary,
            },
            AddHostAction {
                icon: iced_fonts::lucide::file_code().into(),
                label: crate::i18n::t("import_ssh_config_btn"),
                msg: Message::Share(ShareMessage::ImportSshConfig),
                color: secondary,
            },
        ];
        // Group creation, context-symmetric and always the leading
        // entry. Inside a manual folder it's "New subgroup" (a child of
        // the open folder): the folder kebab offers the same action from
        // the parent view, this covers creating one while the folder
        // itself is open (its own card, and thus its kebab, isn't
        // visible there). At the vault root it's "New group" (a fresh
        // top-level folder), so an empty group can be born here instead
        // of only by typing a new name in the host editor's group combo.
        // Dynamic groups derive their contents from the cloud query, so
        // they take neither: no manual children, and their toolbar shows
        // Discover rather than this add menu.
        match self.active_group {
            Some(gid)
                if self
                    .groups
                    .iter()
                    .any(|g| g.id == gid && g.cloud_query.is_none()) =>
            {
                actions.insert(
                    0,
                    AddHostAction {
                        icon: iced_fonts::lucide::folder_plus().into(),
                        label: crate::i18n::t("new_subgroup"),
                        msg: Message::Tabs(TabsMessage::NewSubgroup(gid)),
                        color: secondary,
                    },
                );
            }
            None => {
                actions.insert(
                    0,
                    AddHostAction {
                        icon: iced_fonts::lucide::folder_plus().into(),
                        label: crate::i18n::t("new_group"),
                        msg: Message::Tabs(TabsMessage::NewGroup),
                        color: secondary,
                    },
                );
            }
            _ => {}
        }
        // Remote-desktop hosts (RDP/VNC) stay out of the light-user
        // list until the opt-in feature is enabled.
        if self.remote_desktop_enabled {
            actions.push(AddHostAction {
                icon: iced_fonts::lucide::monitor().into(),
                label: crate::i18n::t("add_remote_desktop"),
                msg: Message::Editor(EditorMessage::ShowNewRemoteDesktop),
                color: secondary,
            });
        }
        // Export hosts: opens the share dialog with a per-folder
        // include/exclude checklist (keys-off by default), unlike the
        // full-vault export in Settings. Pre-scoped to the active
        // folder when one is open. Nothing to share with an empty
        // vault, so the entry only exists once a host does.
        if !self.connections.is_empty() {
            actions.push(AddHostAction {
                icon: iced_fonts::lucide::upload().into(),
                label: crate::i18n::t("export_hosts"),
                msg: Message::Share(ShareMessage::ShowExportHosts(self.active_group)),
                color: secondary,
            });
        }
        // Only profiles whose provider plugin is installed can run
        // discovery; hide the rest (they'd fail with a "binary not
        // found" wall) until the plugin is back.
        for cp in self
            .cloud_profiles
            .iter()
            .filter(|p| self.cloud_provider_installed(&p.provider))
        {
            let (icon, brand) =
                crate::os_icon::provider_icon(&cp.provider, OryxisColors::t().accent);
            actions.push(AddHostAction {
                icon,
                label: cp.label.as_str(),
                msg: Message::Cloud(CloudMessage::ShowCloudDiscover(cp.id)),
                color: brand,
            });
        }
        actions
    }
}
