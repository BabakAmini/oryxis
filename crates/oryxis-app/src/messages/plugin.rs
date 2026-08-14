//! Cloud-provider plugin install / update / uninstall lifecycle, wrapped by [`crate::messages::Message::Plugin`]. Handled by `Oryxis::handle_plugins`.

#[derive(Debug, Clone)]
pub enum PluginMessage {
    /// Global auto-update toggle (applies to every plugin without an
    /// explicit per-plugin override).
    PluginToggleGlobalAutoUpdate(bool),
    /// Per-plugin auto-update override.
    PluginToggleAutoUpdate(String, bool),
    /// Fetch the hosted manifest for a provider and compare against
    /// the installed version.
    PluginCheckUpdates(String),
    /// Header action: run the update check for every installed plugin.
    PluginCheckAllUpdates,
    /// Toggle the kebab menu on a plugin row (secondary actions:
    /// check for updates, auto-update override, uninstall).
    ShowPluginMenu(String),
    /// Manifest fetch finished, `Ok` carries the parsed manifest.
    PluginManifestFetched(String, Result<Box<crate::plugins::PluginManifest>, String>),
    /// Open / close the first-use install opt-in modal for a provider.
    ShowPluginInstallModal(String),
    HidePluginInstallModal,
    /// The unreachable-host hint's escape hatch (discussion #163):
    /// close the modal and land on Settings > Advanced with the
    /// Download mirror row revealed.
    OpenMirrorSetting,
    /// Begin downloading + installing the best compatible version.
    PluginInstall(String),
    /// Install finished, `Ok` carries the installed version string.
    PluginInstallDone(String, Result<String, String>),
    /// Remove a provider's cached binaries.
    PluginUninstall(String),
    /// Confirmed from the uninstall dialog: actually remove the
    /// cached binaries (and the MCP launcher copy for `mcp`).
    PluginUninstallConfirmed(String),
}
