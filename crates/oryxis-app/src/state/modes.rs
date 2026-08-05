//! Top-level UI modes (split out of `state.rs`).

use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum VaultState {
    #[default]
    Loading,
    NeedSetup,
    Locked,
    Unlocked,
}

/// Active tab inside the terminal-side panel. `Chat` is only reachable
/// when AI is enabled; the dispatch falls back to `Snippets` otherwise.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TerminalSidebarTab {
    #[default]
    Chat,
    Snippets,
    /// Per-host command history (top frequent + recent), captured by the
    /// OSC 133 / input-mirror pipeline.
    History,
    /// Remote file browser for the focused pane's SSH session (an SFTP
    /// channel multiplexed on the live handle), with follow-cwd via the
    /// OSC 7 the terminal already captures. SSH-only: the tab button is
    /// hidden (and the dispatch falls back to `Snippets`) when the pane
    /// has no SSH transport.
    Files,
    /// Agentless resource monitor for the focused pane's host: CPU /
    /// memory / load / disk / network read from `/proc` over the live
    /// session (issue #83). SSH-only and opt-in per host, like Files.
    Monitor,
    /// Per-host appearance/behavior settings for the focused pane's
    /// connection, edited live with the terminal visible alongside.
    HostConfig,
}

impl TerminalSidebarTab {
    /// Every tab, in strip order. Backs the "Default sidebar tab"
    /// picker (issue #85).
    pub const ALL: [TerminalSidebarTab; 6] = [
        TerminalSidebarTab::Chat,
        TerminalSidebarTab::Snippets,
        TerminalSidebarTab::History,
        TerminalSidebarTab::Files,
        TerminalSidebarTab::Monitor,
        TerminalSidebarTab::HostConfig,
    ];

    /// Stable code persisted in the `sidebar_default_tab` setting.
    pub fn code(self) -> &'static str {
        match self {
            TerminalSidebarTab::Chat => "chat",
            TerminalSidebarTab::Snippets => "snippets",
            TerminalSidebarTab::History => "history",
            TerminalSidebarTab::Files => "files",
            TerminalSidebarTab::Monitor => "monitor",
            TerminalSidebarTab::HostConfig => "hostconfig",
        }
    }

    /// Parse a persisted code back to a tab; unknown codes (and the
    /// "last opened" sentinel) return `None`.
    pub fn from_code(code: &str) -> Option<TerminalSidebarTab> {
        TerminalSidebarTab::ALL.into_iter().find(|t| t.code() == code)
    }

    /// i18n key for this tab's label, reusing the tab-strip tooltip
    /// keys so the picker and the strip never drift.
    pub fn label_key(self) -> &'static str {
        match self {
            TerminalSidebarTab::Chat => "tab_tip_chat",
            TerminalSidebarTab::Snippets => "snippets",
            TerminalSidebarTab::History => "tab_tip_history",
            TerminalSidebarTab::Files => "tab_tip_files",
            TerminalSidebarTab::Monitor => "tab_tip_monitor",
            TerminalSidebarTab::HostConfig => "tab_tip_host_config",
        }
    }
}

/// Identifies a secret text field whose reveal/eye toggle is on. One
/// shared enum + a `HashSet` in app state instead of a bool per field,
/// so adding the eye to a new password input is a one-variant change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SecretField {
    /// Inline proxy password in the host editor.
    ProxyPassword,
    /// Password on the Share (portable export) dialog.
    SharePassword,
    /// AI assistant API key (Settings > AI).
    AiApiKey,
    /// New master password (Settings > Security).
    VaultNewPassword,
    /// Confirm new master password (Settings > Security).
    VaultConfirmPassword,
    /// Current master password in the change-password form (Settings > Security).
    VaultCurrentPassword,
    /// Portable export password (Settings > Security).
    ExportPassword,
    /// Portable import password (Settings > Security).
    ImportPassword,
    /// Sync signaling token (Settings > Sync).
    SyncSignalingToken,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    Dashboard,
    Terminal,
    Keys,
    Snippets,
    PortForwarding,
    /// Cloud-account CRUD. Promoted to a top-level vault surface
    /// (sub-nav pill / sidebar entry); the Cloud Sync settings block
    /// stays behind in Settings.
    Cloud,
    /// Proxy-identity CRUD. Promoted to a top-level vault surface.
    Proxies,
    /// Known-host management. Promoted back to a top-level vault
    /// surface alongside Cloud / Proxies (was a SettingsSection in
    /// v0.7).
    KnownHosts,
    History,
    Sftp,
    Settings,
    /// Multi-host monitor dashboard (issue #95): live vitals across
    /// every opted-in host. Not a sub-nav pill: entered through the
    /// Hosts toolbar's monitor icon, which only renders while the
    /// master `host_monitoring` toggle is on (optional-features rule).
    Monitoring,
}

/// One row in the Plugins panel: a cloud-provider plugin and its
/// install / update state. Cloud providers ship as downloaded
/// subprocess plugins (see `crate::plugins`); this is the UI-side
/// view of one.
#[derive(Debug, Clone)]
pub struct PluginUiEntry {
    /// Provider id, matches `CloudProvider::id()` (`"aws"`, ...).
    pub provider_id: String,
    /// Human-readable name shown in the panel.
    pub display_name: String,
    /// Current install / update state.
    pub status: PluginUiStatus,
    /// Per-plugin auto-update override, resolved against the global
    /// default when the panel loads.
    pub auto_update: bool,
    /// User-pinned version. When set, the updater won't move off it.
    pub pinned_version: Option<String>,
    /// Downloaded binaries exist in the plugin cache (or, for MCP,
    /// the launcher copy). Lets a dev build still offer "remove
    /// downloaded files" for the cache it shadows.
    pub cached_install: bool,
    /// Last successfully fetched manifest. Drives the install modal's
    /// size / changelog. `None` until a check runs (and on every
    /// machine until the manifest host exists, see PR 6).
    pub manifest: Option<crate::plugins::PluginManifest>,
}

/// Install / update lifecycle state for a [`PluginUiEntry`].
#[derive(Debug, Clone, PartialEq)]
pub enum PluginUiStatus {
    /// No binary on disk and no dev build, the plugin must be
    /// downloaded before its provider can be used.
    NotInstalled,
    /// Running from a freshly-built `target/debug` binary (the dev
    /// loop). No version directory, no manifest involved.
    DevBuild,
    /// Installed from the cache at this version.
    Installed(String),
    /// Installed, and the manifest advertises a newer compatible
    /// version.
    UpdateAvailable { current: String, latest: String },
    /// A manifest fetch is in flight.
    Checking,
    /// A binary download + verify is in flight (indeterminate).
    Downloading,
    /// The last check / install failed; carries a user-facing message.
    Failed(String),
}

/// Cloud provider picked in the wizard. AWS authenticates via named
/// profile / access key / SSO; Kubernetes via a kubeconfig; GCP via the
/// already-authenticated `gcloud` CLI (scoped to an optional project);
/// Azure via the already-authenticated `az` CLI (scoped to an optional
/// subscription).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CloudProviderChoice {
    #[default]
    Aws,
    K8s,
    Gcp,
    Azure,
}

/// Which kind of `PodSelector` a K8s dynamic group's editor produces.
/// `Labels` takes a `k=v,k=v` string; the rest take a single resource
/// name (the resolver expands it to that workload's / pod's selector).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum K8sSelectorKind {
    #[default]
    Labels,
    Deployment,
    StatefulSet,
    Name,
}

impl K8sSelectorKind {
    pub const ALL: [K8sSelectorKind; 4] = [
        K8sSelectorKind::Labels,
        K8sSelectorKind::Deployment,
        K8sSelectorKind::StatefulSet,
        K8sSelectorKind::Name,
    ];
}

impl std::fmt::Display for K8sSelectorKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            K8sSelectorKind::Labels => "Labels",
            K8sSelectorKind::Deployment => "Deployment",
            K8sSelectorKind::StatefulSet => "StatefulSet",
            K8sSelectorKind::Name => "Pod name",
        })
    }
}

impl CloudProviderChoice {
    pub fn id(self) -> &'static str {
        match self {
            Self::Aws => "aws",
            Self::K8s => "k8s",
            Self::Gcp => "gcp",
            Self::Azure => "azure",
        }
    }

    pub fn from_id(s: &str) -> Self {
        match s {
            "k8s" => Self::K8s,
            "gcp" => Self::Gcp,
            "azure" => Self::Azure,
            _ => Self::Aws,
        }
    }
}

/// Auth strategy chosen in the wizard. Only `Profile` is implemented in
/// v0.6 PR 3; the other variants render disabled with a hint and route
/// to `CloudError::Unsupported` if somehow selected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CloudAuthChoice {
    #[default]
    Profile,
    AccessKey,
    Sso,
    Kubeconfig,
    /// GCP: the ambient `gcloud` login (`gcloud auth login`); no secret
    /// stored, just an optional project scope.
    GcloudCli,
    /// Azure: the ambient `az` login (`az login`); no secret stored, just
    /// an optional subscription scope.
    AzCli,
}

impl CloudAuthChoice {
    pub fn id(self) -> &'static str {
        match self {
            Self::Profile => "profile",
            Self::AccessKey => "access_key",
            Self::Sso => "sso",
            Self::Kubeconfig => "kubeconfig",
            Self::GcloudCli => "gcloud",
            Self::AzCli => "az",
        }
    }

    pub fn from_id(s: &str) -> Self {
        match s {
            "access_key" => Self::AccessKey,
            "sso" => Self::Sso,
            "kubeconfig" => Self::Kubeconfig,
            "gcloud" => Self::GcloudCli,
            "az" => Self::AzCli,
            _ => Self::Profile,
        }
    }
}

/// Live state of the "Test credentials" button in the wizard.
#[derive(Debug, Clone, Default)]
pub enum CloudTestState {
    #[default]
    Idle,
    Running,
    Ok,
    Failed(String),
}

/// State of the wizard's "Discover & pick" panel, owns the in-flight
/// or completed discovery result so the user can scroll/select without
/// re-hitting the cloud.
#[derive(Debug, Clone, Default)]
pub enum CloudDiscoverState {
    #[default]
    Idle,
    Running,
    Loaded(oryxis_cloud::DiscoveryResult),
    Failed(String),
}


/// Per-dynamic-group resolve state. Lives in a `HashMap<group_id, _>`
/// on `Oryxis` so opening one group doesn't blow away another's
/// cached resolve. TTL handling lives on the call site.
#[derive(Debug, Clone)]
pub enum DynamicGroupState {
    Loading,
    Loaded {
        hosts: Vec<oryxis_cloud::DiscoveredHost>,
        // When this list was fetched. `OpenGroup` compares against
        // `Utc::now()` and re-resolves past the cache TTL so a recycled
        // ECS task doesn't sit as a dead row until a manual Refresh.
        fetched_at: chrono::DateTime<chrono::Utc>,
    },
    Failed(String),
}

/// One mDNS-discovered peer the user could pair with. Lives in
/// `Oryxis.sync_discovered`, deduped by `device_id`, rebuilt as
/// `SyncEngineEvent::PeerDiscovered` arrives.
#[derive(Debug, Clone)]
pub(crate) struct DiscoveredPeerInfo {
    pub device_id: Uuid,
    pub device_name: String,
    pub addr: std::net::SocketAddr,
}

/// Which pairing sub-view the Sync settings panel is showing. The hosted
/// code and the join inputs live alongside this in
/// [`SyncPairingForm`](super::SyncPairingForm) on `Oryxis.sync_pairing`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum SyncPairingState {
    /// Default: just the two "Host" / "Join" entry buttons.
    #[default]
    Idle,
    /// This device is hosting a code, waiting for a peer to join.
    Hosting,
    /// This device is entering another device's code + address.
    Joining,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum SettingsSection {
    Terminal,
    /// SSH connection behaviour shared across hosts: keepalive
    /// interval, auto-reconnect, OS detection. Split out of the
    /// Terminal section, which had grown into a grab-bag of terminal
    /// display, connection and logging knobs.
    Connection,
    Sftp,
    /// Host monitoring config (issue #83), shown only while the
    /// monitoring feature is enabled in Features & Plugins.
    Monitoring,
    AI,
    /// Visual + layout preferences. Absorbs the legacy "Theme" section
    /// and adds toggles for status bar visibility and (in later PRs)
    /// layout mode, tab close button position, host icon style, etc.
    Interface,
    /// MCP server (Model Context Protocol). Was bundled into the
    /// installer in 0.6 and lived inside the Security section; in
    /// 0.7 it's distributed as a plugin and gets its own section
    /// in the Settings sidebar so the setup-guide affordances and
    /// the enable toggle aren't buried.
    Mcp,
    Shortcuts,
    Security,
    Sync,
    /// SSH agent server configuration: per-signature confirm, external
    /// key adds, the OpenSSH pipe alias (Windows) and the socket path +
    /// setup snippets. The enable toggle stays on the Features screen
    /// (like AI / SFTP / Sync); this section only appears while the
    /// agent is enabled.
    Agent,
    /// Cloud Sync preferences (auto-refresh interval, orphan
    /// auto-archive). The cloud *account* CRUD moved to the top-level
    /// `View::Cloud` surface; this section keeps only the sync knobs.
    Cloud,
    /// Cloud provider plugins management: install, update, uninstall
    /// the subprocess plugins each cloud provider runs as. Sits next
    /// to `Cloud` because every cloud account here needs a matching
    /// plugin to actually function.
    Plugins,
    /// Troubleshooting surface: the debug-logging file toggle and the
    /// environment report to paste into GitHub issues. Sits between the
    /// feature sections and About; nothing here is everyday config.
    Advanced,
    About,
}

impl SettingsSection {
    /// Stable id of the section's content scrollable. Static literals
    /// because the fork's `widget::Id::new` only takes `&'static str`.
    /// The keyboard router snaps these to keep the selected row in
    /// view; each section view sets the same id on its scrollable.
    pub(crate) fn scroll_id(self) -> &'static str {
        match self {
            SettingsSection::Terminal => "settings-terminal-scroll",
            SettingsSection::Connection => "settings-connection-scroll",
            SettingsSection::Sftp => "settings-sftp-scroll",
            SettingsSection::Monitoring => "settings-monitoring-scroll",
            SettingsSection::AI => "settings-ai-scroll",
            SettingsSection::Interface => "settings-interface-scroll",
            SettingsSection::Mcp => "settings-mcp-scroll",
            SettingsSection::Shortcuts => "settings-shortcuts-scroll",
            SettingsSection::Security => "settings-security-scroll",
            SettingsSection::Sync => "settings-sync-scroll",
            SettingsSection::Agent => "settings-agent-scroll",
            SettingsSection::Cloud => "settings-cloud-scroll",
            SettingsSection::Plugins => "settings-plugins-scroll",
            SettingsSection::Advanced => "settings-advanced-scroll",
            SettingsSection::About => "settings-about-scroll",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::TerminalSidebarTab;

    #[test]
    fn sidebar_tab_code_roundtrips_and_rejects_sentinels() {
        // Every tab survives code -> from_code so the persisted
        // `sidebar_default_tab` setting resolves back exactly (issue #85).
        for t in TerminalSidebarTab::ALL {
            assert_eq!(TerminalSidebarTab::from_code(t.code()), Some(t));
        }
        // The "last opened" sentinel and any junk resolve to None (keep
        // the last tab), never a wrong tab.
        assert_eq!(TerminalSidebarTab::from_code("last"), None);
        assert_eq!(TerminalSidebarTab::from_code(""), None);
        assert_eq!(TerminalSidebarTab::from_code("bogus"), None);
    }
}
