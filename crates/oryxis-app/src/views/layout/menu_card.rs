//! Overlay menu builders for vault card / list kebab menus. Split out
//! of `render_overlay_menu` in views/layout/menus.rs; each method returns
//! the inner menu `items` Element that `render_overlay_menu` wraps in the
//! shared popover container. Pure relocation, no behavior change.

use super::*;
use iced::widget::column;

impl Oryxis {
    pub(crate) fn build_menu_session_log_actions(&self, idx: usize) -> Element<'_, Message> {
        let log_id = self.session_logs.get(idx).map(|e| e.id);
        let mut col = column![].spacing(2);
        if let Some(log_id) = log_id {
            // .cast replay export pairs with full-detail
            // recording; with simple logs the action is hidden
            // (owner call 2026-07-04), not just degraded.
            if self.setting_session_log_full {
                col = col.push(self.menu_item(
                    iced_fonts::lucide::film(),
                    crate::i18n::t("export_cast_tip"),
                    Message::ExportSessionCast(log_id),
                    OryxisColors::t().text_secondary,
                ));
            }
            col = col.push(self.menu_item(
                iced_fonts::lucide::file_text(),
                crate::i18n::t("export_transcript_tip"),
                Message::ExportSessionTranscript(log_id),
                OryxisColors::t().text_secondary,
            ));
            col = col.push(self.menu_item(
                iced_fonts::lucide::keyboard(),
                crate::i18n::t("export_commands_tip"),
                Message::ExportSessionCommands(log_id),
                OryxisColors::t().text_secondary,
            ));
        }
        col = col.push(self.menu_item(
            iced_fonts::lucide::trash(),
            crate::i18n::t("delete"),
            Message::RequestDeleteSessionLog(idx),
            OryxisColors::t().error,
        ));
        // Honest-export caption: recordings carry the raw
        // bytes, Privacy Mode masking is display-only.
        col = col.push(
            container(
                text(crate::i18n::t("session_export_privacy_note"))
                    .size(10)
                    .color(OryxisColors::t().text_muted),
            )
            .padding(Padding { top: 4.0, right: 12.0, bottom: 2.0, left: 12.0 })
            .width(Length::Fill),
        );
        col.into()
    }

    pub(crate) fn build_menu_host_actions(&self, idx: usize) -> Element<'_, Message> {
        let conn = self.connections.get(idx);
        let cloud_profile_id = conn
            .and_then(|c| c.cloud_ref.as_ref())
            .map(|r| r.profile_id);
        let is_orphan = conn
            .and_then(|c| c.cloud_ref.as_ref())
            .and_then(|r| r.orphaned_at)
            .is_some();
        // SSH-only actions (Share + SFTP mount both ride the SSH
        // subsystem) and the URL scheme depend on the protocol.
        use oryxis_core::models::connection::ConnectionProtocol;
        let protocol = conn.map(|c| c.protocol).unwrap_or(ConnectionProtocol::Ssh);
        let is_ssh_host = protocol == ConnectionProtocol::Ssh;
        let is_rd_host = protocol == ConnectionProtocol::RemoteDesktop;
        let has_url = matches!(
            protocol,
            ConnectionProtocol::Ssh | ConnectionProtocol::Telnet
        );
        let mut items = column![
            self.menu_item(iced_fonts::lucide::play(), crate::i18n::t("connect"), Message::ConnectSsh(idx), OryxisColors::t().success),
            self.menu_item(iced_fonts::lucide::pencil(), crate::i18n::t("edit"), Message::EditConnection(idx), OryxisColors::t().text_secondary),
            self.menu_item(iced_fonts::lucide::copy(), crate::i18n::t("duplicate"), Message::DuplicateConnection(idx), OryxisColors::t().text_secondary),
        ];
        if is_ssh_host {
            items = items
                .push(self.menu_item(iced_fonts::lucide::share(), crate::i18n::t("share"), Message::ShareConnection(idx), OryxisColors::t().text_secondary));
            // SFTP is an optional feature: its entry hides with the
            // toggle, like every other SFTP surface.
            if self.sftp_enabled {
                items = items.push(self.menu_item(iced_fonts::lucide::folder_tree(), crate::i18n::t("open_sftp_tab"), Message::OpenSftpForConnection(idx), OryxisColors::t().text_secondary));
            }
        }
        if has_url {
            items = items.push(self.menu_item(iced_fonts::lucide::link(), crate::i18n::t("copy_ssh_url"), Message::CopyHostSshUrl(idx), OryxisColors::t().text_secondary));
        }
        // Remote-desktop host: Connect (above) already launches the
        // desktop; add an explicit Stop while its tunnel is live.
        if is_rd_host
            && let Some(cid) = conn.map(|c| c.id)
            && self.remote_desktop_forwards.contains_key(&cid)
        {
            items = items.push(self.menu_item(
                iced_fonts::lucide::monitor_x(),
                crate::i18n::t("stop_remote_desktop"),
                Message::StopRemoteDesktop(cid),
                OryxisColors::t().error,
            ));
        }
        if let Some(pid) = cloud_profile_id {
            items = items.push(self.menu_item(
                iced_fonts::lucide::funnel(),
                crate::i18n::t("host_filter_by_profile"),
                Message::HostFilterByCloudProfile(Some(pid)),
                OryxisColors::t().text_secondary,
            ));
        }
        // Orphan hosts get a "Forget" label (semantically
        // closer to "this resource is gone upstream, drop my
        // local record") instead of the generic "Remove".
        // Same `DeleteConnection` action under the hood.
        let (remove_label, remove_icon) = if is_orphan {
            (crate::i18n::t("host_orphan_forget"), iced_fonts::lucide::eraser())
        } else {
            (crate::i18n::t("remove"), iced_fonts::lucide::trash())
        };
        items
            .push(self.menu_item(remove_icon, remove_label, Message::RequestDeleteConnection(idx), OryxisColors::t().error))
            .into()
    }

    pub(crate) fn build_menu_session_group_actions(&self, idx: usize) -> Element<'_, Message> {
        column![
            self.menu_item(iced_fonts::lucide::play(), crate::i18n::t("open_session_group"), Message::OpenSessionGroup(idx), OryxisColors::t().success),
            self.menu_item(iced_fonts::lucide::pencil(), crate::i18n::t("edit"), Message::EditSessionGroup(idx), OryxisColors::t().text_secondary),
            self.menu_item(iced_fonts::lucide::copy(), crate::i18n::t("duplicate"), Message::DuplicateSessionGroup(idx), OryxisColors::t().text_secondary),
            self.menu_item(iced_fonts::lucide::trash(), crate::i18n::t("remove"), Message::RequestDeleteSessionGroup(idx), OryxisColors::t().error),
        ]
        .into()
    }

    pub(crate) fn build_menu_key_actions(&self, idx: usize) -> Element<'_, Message> {
        column![
            self.menu_item(iced_fonts::lucide::pencil(), crate::i18n::t("edit"), Message::EditKey(idx), OryxisColors::t().text_secondary),
            self.menu_item(iced_fonts::lucide::trash(), crate::i18n::t("remove"), Message::RequestDeleteKey(idx), OryxisColors::t().error),
        ].into()
    }

    pub(crate) fn build_menu_identity_actions(&self, idx: usize) -> Element<'_, Message> {
        column![
            self.menu_item(iced_fonts::lucide::pencil(), crate::i18n::t("edit"), Message::EditIdentity(idx), OryxisColors::t().text_secondary),
            self.menu_item(iced_fonts::lucide::trash(), crate::i18n::t("remove"), Message::RequestDeleteIdentity(idx), OryxisColors::t().error),
        ].into()
    }

    pub(crate) fn build_menu_snippet_actions(&self, idx: usize) -> Element<'_, Message> {
        column![
            self.menu_item(iced_fonts::lucide::pencil(), crate::i18n::t("edit"), Message::EditSnippet(idx), OryxisColors::t().text_secondary),
            self.menu_item(iced_fonts::lucide::trash(), crate::i18n::t("delete"), Message::RequestDeleteSnippet(idx), OryxisColors::t().error),
        ].into()
    }

    pub(crate) fn build_menu_keychain_add(&self) -> Element<'_, Message> {
        column![
            self.menu_item(iced_fonts::lucide::key_round(), crate::i18n::t("import_key"), Message::ShowKeyPanel, OryxisColors::t().text_secondary),
            self.menu_item(iced_fonts::lucide::user(), crate::i18n::t("new_identity"), Message::ShowIdentityPanel, OryxisColors::t().text_secondary),
        ].into()
    }

    pub(crate) fn build_menu_folder_actions(&self, gid: uuid::Uuid) -> Element<'_, Message> {
        // Folders that hold cloud-imported hosts used to hide
        // their rename / delete actions to protect the
        // import-by-label dedupe. The decoupling work in v0.7
        // moved import targets to an explicit picker, so
        // renaming or moving the auto folder no longer breaks
        // anything (worst case the next Auto import creates a
        // sibling). Surface the standard actions instead.
        column![
            self.menu_item(iced_fonts::lucide::pencil(), crate::i18n::t("edit"), Message::EditGroup(gid), OryxisColors::t().accent),
            self.menu_item(iced_fonts::lucide::trash(), crate::i18n::t("delete"), Message::StartDeleteFolder(gid), OryxisColors::t().error),
        ].into()
    }

    pub(crate) fn build_menu_dynamic_group_actions(&self, id: uuid::Uuid) -> Element<'_, Message> {
        column![
            self.menu_item(iced_fonts::lucide::pencil(), crate::i18n::t("edit"), Message::EditDynamicGroup(id), OryxisColors::t().accent),
            // Rename = friendly display label only. The
            // cloud_query (cluster/service/container) and the
            // import-dedupe key never look at it, so renaming
            // is safe and the subtitle keeps surfacing the
            // original ECS path.
            self.menu_item(iced_fonts::lucide::text_cursor_input(), crate::i18n::t("rename"), Message::StartRenameFolder(id), OryxisColors::t().text_secondary),
            self.menu_item(iced_fonts::lucide::trash(), crate::i18n::t("delete"), Message::DeleteDynamicGroup(id), OryxisColors::t().error),
        ].into()
    }

    pub(crate) fn build_menu_cloud_profile_actions(&self, id: uuid::Uuid) -> Element<'_, Message> {
        column![
            self.menu_item(iced_fonts::lucide::pencil(), crate::i18n::t("edit"), Message::ShowCloudForm(Some(id)), OryxisColors::t().text_secondary),
            self.menu_item(iced_fonts::lucide::refresh_cw(), crate::i18n::t("cloud_profile_sync"), Message::CloudProfileSync(id), OryxisColors::t().accent),
            self.menu_item(iced_fonts::lucide::trash(), crate::i18n::t("delete"), Message::DeleteCloudProfile(id), OryxisColors::t().error),
        ].into()
    }

    pub(crate) fn build_menu_cloud_provider_picker(&self) -> Element<'_, Message> {
        // The "+ Host ▾" add menu. Offers importing a `.oryxis`
        // file (a full vault export or a single shared host),
        // importing an OpenSSH `~/.ssh/config`, exporting the
        // current view, then one entry per configured cloud
        // profile for discovery. Import / export live here so
        // they're reachable from where hosts are managed
        // instead of being buried in Settings.
        let mut items = column![
            self.menu_item(
                iced_fonts::lucide::download(),
                crate::i18n::t("import_from_file"),
                Message::ImportVault,
                OryxisColors::t().text_secondary,
            ),
            self.menu_item(
                iced_fonts::lucide::file_code(),
                crate::i18n::t("import_ssh_config_btn"),
                Message::ImportSshConfig,
                OryxisColors::t().text_secondary,
            ),
        ];
        // Add a remote-desktop host (RDP/VNC), only when the opt-in
        // feature is enabled so it stays out of the light-user menu.
        if self.remote_desktop_enabled {
            items = items.push(self.menu_item(
                iced_fonts::lucide::monitor(),
                crate::i18n::t("add_remote_desktop"),
                Message::ShowNewRemoteDesktop,
                OryxisColors::t().text_secondary,
            ));
        }
        // Export hosts: opens the share dialog with a per-folder
        // include/exclude checklist (keys-off by default), unlike
        // the full-vault export in Settings. Pre-scoped to the
        // active folder when one is open.
        if !self.connections.is_empty() {
            items = items.push(self.menu_item(
                iced_fonts::lucide::upload(),
                crate::i18n::t("export_hosts"),
                Message::ShowExportHosts(self.active_group),
                OryxisColors::t().text_secondary,
            ));
        }
        // Only profiles whose provider plugin is installed can
        // run discovery; hide the rest (they'd fail with a
        // "binary not found" wall) until the plugin is back.
        for cp in self
            .cloud_profiles
            .iter()
            .filter(|p| self.cloud_provider_installed(&p.provider))
        {
            let (glyph, brand) = crate::os_icon::provider_icon(
                &cp.provider,
                OryxisColors::t().accent,
            );
            items = items.push(self.menu_item(
                glyph,
                cp.label.as_str(),
                Message::ShowCloudDiscover(cp.id),
                brand,
            ));
        }
        items.into()
    }
}
