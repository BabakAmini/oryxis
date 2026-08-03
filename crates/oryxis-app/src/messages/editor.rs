//! Host editor form: fields, quirks, serial, proxy, chain editor, env vars, port forwards.

use iced::widget::text_editor;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub enum EditorMessage {
    EditorOpenThemePicker,
    EditorCloseThemePicker,
    /// Empty string == "inherit the global theme".
    EditorTerminalThemeChanged(String),
    /// Cloud transport pick (only meaningful when editing a cloud-imported host).
    EditorCloudTransportChanged(oryxis_core::models::cloud::TransportKind),
    /// Per-host initial command, sent as keystrokes after the shell
    /// opens. Empty = none. Useful for hosts that drop into `/bin/sh`
    /// when you really want `bash`.
    EditorInitialCommandChanged(text_editor::Action),
    /// Set the per-host icon shape override. Empty string clears the
    /// override (falls back to the global `default_host_icon`).
    EditorIconStyleChanged(String),
    EditorEncodingChanged(String),
    /// Per-host TERM name picked in the host editor.
    EditorTerminalTypeChanged(String),
    /// Empty string == "inherit the global keepalive setting".
    /// "0" == explicitly disabled on this host; any positive integer
    /// is the per-host override in seconds. Sanitized to digits-only.
    EditorKeepaliveChanged(String),
    /// Directory a fresh SFTP mount of this host lands in, as typed.
    /// Empty == the login directory (the default).
    EditorSftpInitialPathChanged(String),
    /// Per-host auto-title (OSC 0/2) selection from the host editor pick:
    /// the localized "Default / Show / Hide" label.
    EditorAutoTitleChanged(String),
    /// Per-host Privacy Mode selection from the host editor pick: the
    /// localized "Default / On / Off" label.
    EditorPrivacyModeChanged(String),
    EditorSidebarAutoOpenChanged(String),
    /// Backspace mode pick (localized "Control-? (127)" / "Control-H (8)").
    EditorQuirkBackspaceChanged(String),
    /// Home/End mode pick (localized "Standard" / "rxvt").
    EditorQuirkHomeEndChanged(String),
    /// Function-key mode pick (localized Xterm / Linux / VT400 / rxvt).
    EditorQuirkFnKeysChanged(String),
    /// "Report mouse to remote" toggle (off = `disable_mouse_reporting`).
    EditorQuirkMouseReportingChanged(bool),
    /// "Allow remote title changes" toggle (off = `disable_title_change`).
    EditorQuirkTitleChangeChanged(bool),
    /// OSC 52 clipboard-write override pick (localized Default / On / Off).
    EditorQuirkOsc52Changed(String),
    /// macOS Option-as-Meta pick (localized Off / Left / Right / Both;
    /// issue #80: the default composes characters like every macOS
    /// terminal, Meta is the readline/emacs opt-in).
    EditorQuirkOptionAsMetaChanged(String),
    /// Per-host SSH rekey limit (MB) text input.
    EditorQuirkRekeyChanged(String),
    /// Toggle a per-host SSH algorithm category between Auto (None) and a
    /// custom pinned list (seeded from the safe defaults).
    EditorAlgoSetAuto(crate::state::AlgoCategory, bool),
    /// Add/remove one algorithm name in a category's pinned list.
    EditorAlgoToggle(crate::state::AlgoCategory, String),
    ShowNewConnection,
    /// Open the host editor seeded as a RemoteDesktop host ("Add remote
    /// desktop" in the + Host menu; only shown when the feature toggle is on).
    ShowNewRemoteDesktop,
    EditConnection(usize),
    EditorLabelChanged(String),
    /// Host editor: comma-separated tags field.
    EditorTagsChanged(String),
    EditorHostnameChanged(String),
    /// Host editor: the wire-protocol picker (SSH / Telnet). Switching
    /// swaps the reduced form and, when the port still holds the old
    /// protocol's default, retargets it (22 <-> 23).
    EditorProtocolChanged(oryxis_core::models::connection::ConnectionProtocol),
    EditorSerialBaudChanged(u32),
    EditorSerialDataBitsChanged(u8),
    EditorSerialParityChanged(oryxis_core::models::serial::SerialParity),
    EditorSerialStopBitsChanged(oryxis_core::models::serial::SerialStopBits),
    EditorSerialFlowChanged(oryxis_core::models::serial::SerialFlowControl),
    EditorSerialLineEndingChanged(oryxis_core::models::serial::SerialLineEnding),
    EditorSerialLocalEchoToggled,
    EditorRdKindChanged(oryxis_core::models::remote_desktop::RemoteDesktopKind),
    EditorRdGatewayChanged(Option<uuid::Uuid>),
    /// Address-family preference picked in the host editor (SSH > Network).
    EditorAddressFamilyChanged(oryxis_core::models::connection::AddressFamily),
    EditorPortChanged(String),
    EditorUsernameChanged(String),
    EditorPasswordChanged(super::Redacted),
    EditorAuthMethodChanged(String),
    EditorGroupChanged(String),
    EditorKeyChanged(String),
    OpenChainEditor,
    CloseChainEditor,
    /// Switch the chain editor into "add a hop" mode (host picker).
    ChainEditorStartAdd,
    /// Back out of "add a hop" mode to the chain list.
    ChainEditorCancelAdd,
    ChainEditorSearchChanged(String),
    /// Append the selected connection as the next hop.
    ChainEditorAddHop(Uuid),
    ChainEditorRemoveHop(usize),
    ChainEditorMoveHopUp(usize),
    ChainEditorMoveHopDown(usize),
    EditorProxyKindChanged(crate::state::ProxyKind),
    EditorProxyHostChanged(String),
    EditorProxyPortChanged(String),
    EditorProxyUsernameChanged(String),
    EditorProxyPasswordChanged(super::Redacted),
    EditorProxyCommandChanged(String),
    EditorTogglePasswordVisibility,
    /// TOTP secret (2FA) field: value edit + eye toggle. Tri-state save
    /// mirrors the password field (untouched preserves the stored secret).
    EditorTotpChanged(super::Redacted),
    EditorToggleTotpVisibility,
    EditorUseTotpToggled,
    EditorSave,
    /// Connect using the current editor form WITHOUT persisting anything:
    /// builds an ephemeral quick-connect entry (typed credentials ride in
    /// memory) and dispatches `QuickConnect`. New-host flow only.
    EditorConnectWithoutSaving,
    EditorCancel,
    /// Ask for confirmation before removing a host. Confirming dispatches
    /// `DeleteConnection`. Destructive removals are routed through a confirm
    /// dialog so a stray click can't silently drop a host.
    RequestDeleteConnection(usize),
    DeleteConnection(usize),
    DuplicateConnection(usize),
    /// Open the host editor prefilled from the quick-connect entry so the
    /// user can persist it as a regular host.
    SaveQuickHost(Uuid),
    /// Same prefill, but as the temporary-host edit flow (from the
    /// connect progress screen): Connect (without saving) is the primary
    /// footer action, Save the secondary.
    EditQuickHost(Uuid),
    /// Live per-host edits from the Host config sidebar tab. Each mutates
    /// the focused pane's connection, persists immediately, and (for the
    /// theme) repaints the running terminal for instant preview.
    HostConfigThemeChanged(String),
    HostConfigEncodingChanged(String),
    HostConfigTerminalTypeChanged(String),
    HostConfigAutoTitleChanged(String),
    /// Host editor startup-command source changed (the picker label:
    /// the None sentinel, the Custom sentinel, or a snippet label).
    EditorStartupChoiceChanged(String),
    /// The Initial Command / Snippet combo gained focus; clears its
    /// typed value so the dropdown opens on the full list.
    EditorStartupComboOpened,
    /// The SSH Key combo gained focus; clears its typed value so the
    /// dropdown opens on the full list.
    EditorKeyComboOpened,
    EditorIdentityChanged(String),
    EditorAddPortForward,
    EditorRemovePortForward(usize),
    EditorPortFwdLocalPortChanged(usize, String),
    EditorPortFwdRemoteHostChanged(usize, String),
    EditorPortFwdRemotePortChanged(usize, String),
    EditorAddEnvVar,
    EditorRemoveEnvVar(usize),
    EditorEnvVarKeyChanged(usize, String),
    EditorEnvVarValueChanged(usize, String),
    EditorToggleAgentForwarding,
    EditorToggleX11Forwarding,
    EditorToggleMcpEnabled,
    /// SSH > Integration: flip the per-host agentless monitoring opt-in
    /// (issue #83).
    EditorToggleMonitorEnabled,
    /// Cycle the per-host session-recording override: Default -> On -> Off.
    EditorCycleSessionLogging,
}
