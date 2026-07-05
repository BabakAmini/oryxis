use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::cloud::CloudRef;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Connection {
    pub id: Uuid,
    pub label: String,
    pub hostname: String,
    pub port: u16,
    /// Wire protocol for this host. Defaults to SSH so every payload
    /// written before the field existed (old vaults, sync peers,
    /// portable exports) keeps meaning exactly what it meant. Telnet
    /// hosts reuse the same encrypted password column and ride sync /
    /// export unchanged; the editor swaps to a reduced form and the
    /// terminal pane picks the matching transport at connect.
    #[serde(default)]
    pub protocol: ConnectionProtocol,
    /// Serial-line parameters, meaningful only when `protocol` is
    /// `Serial` (the port path itself reuses `hostname`). `None` on
    /// every non-serial host and on legacy payloads; the connect path
    /// falls back to `SerialParams::default()` (9600 8N1).
    #[serde(default)]
    pub serial: Option<super::serial::SerialParams>,
    /// Remote-desktop kind (RDP vs VNC). Meaningful only when `protocol`
    /// is `RemoteDesktop`; ignored otherwise. `#[serde(default)]` -> RDP
    /// on legacy payloads.
    #[serde(default)]
    pub rd_kind: super::remote_desktop::RemoteDesktopKind,
    /// Optional SSH host to tunnel the remote-desktop connection through.
    /// `Some(id)` routes through that connection's SSH session (the
    /// launcher opens an ephemeral `-L` forward to `hostname:port`);
    /// `None` connects the desktop endpoint directly. A dangling id
    /// resolves to direct with a warning, never an error (mirrors
    /// `proxy_identity_id`). Meaningful only for `RemoteDesktop`.
    #[serde(default)]
    pub rd_gateway_id: Option<Uuid>,
    /// Outbound address-family preference for this host's dials
    /// (PuTTY's Auto / IPv4 / IPv6). Applies to the direct dial, the
    /// proxy dial and the first jump hop, everything that opens a real
    /// socket from this machine. `#[serde(default)]` -> Auto on legacy
    /// payloads, and sync / portable export ride the serde field.
    #[serde(default)]
    pub address_family: AddressFamily,
    pub username: Option<String>,
    pub auth_method: AuthMethod,
    pub key_id: Option<Uuid>,
    pub identity_id: Option<Uuid>,
    pub group_id: Option<Uuid>,
    pub jump_chain: Vec<Uuid>,
    pub proxy: Option<ProxyConfig>,
    /// Reference to a saved `ProxyIdentity`. When set, takes precedence
    /// over the inline `proxy` field, the SSH engine resolves the
    /// identity (via the vault) and ignores `proxy`. `None` falls back
    /// to inline. Cleared on cascade when the identity is deleted.
    #[serde(default)]
    pub proxy_identity_id: Option<Uuid>,
    pub tags: Vec<String>,
    pub notes: Option<String>,
    /// Accent color for the host. Persisted as a hex string ("#RRGGBB").
    /// Used by the dynamic accent system to tint the chrome / tab
    /// indicator when this host is active, and as the fill / border
    /// color for the host icon. `None` falls back to the global accent.
    pub color: Option<String>,
    #[serde(default)]
    pub port_forwards: Vec<PortForward>,
    /// Environment variables sent to the remote shell via SSH `setenv`
    /// before the shell starts. Note most `sshd` only accept `LC_*` /
    /// `LANG_*` unless `AcceptEnv` is widened.
    #[serde(default)]
    pub env_vars: Vec<EnvVar>,
    /// Per-host character encoding label (e.g. `"Big5"`). `None` = UTF-8.
    /// Drives PTY transcoding in the SSH engine for legacy charsets.
    #[serde(default)]
    pub encoding: Option<String>,
    /// Per-host terminal type sent to the server as `TERM` when requesting the
    /// PTY (e.g. `"xterm"`, `"linux"`, `"vt100"`). `None` = `xterm-256color`.
    /// Lets the user pick a fallback for hosts whose terminfo trips on the
    /// default (older boxes, some `mc` / curses setups).
    #[serde(default)]
    pub terminal_type: Option<String>,
    pub mcp_enabled: bool,
    pub last_used: Option<chrono::DateTime<chrono::Utc>>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    /// Detected remote OS id, populated the first time we successfully SSH
    /// into this host and the OS-detection setting is enabled. Values are
    /// lowercase `ID=` from `/etc/os-release` for Linux (ubuntu / debian /
    /// alpine / rhel / fedora / arch / amzn / centos / rocky / alma / suse)
    /// or `uname -s` lowercased for non-Linux (darwin / freebsd / openbsd /
    /// netbsd). `None` means unknown, show the generic server icon.
    #[serde(default)]
    pub detected_os: Option<String>,
    /// User-chosen icon id (overrides the auto-detected one). When present,
    /// the OS-detection probe is skipped and the stored icon / color are
    /// used verbatim on host cards / tabs / editor.
    #[serde(default)]
    pub custom_icon: Option<String>,
    /// User-chosen icon-background color as a hex string (e.g. `#E95420`).
    /// Paired with `custom_icon`.
    #[serde(default)]
    pub custom_color: Option<String>,
    /// Forward the local ssh-agent socket to the remote shell. When
    /// enabled, after the session channel is open we send an
    /// `auth-agent-req@openssh.com` request; sshd then sets
    /// `SSH_AUTH_SOCK` on the remote side and tunnels back any reads
    /// from that socket through this SSH transport. Lets the user
    /// `ssh hostB` from inside hostA without staging keys remotely.
    #[serde(default)]
    pub agent_forwarding: bool,
    /// Per-host terminal palette override. When set, takes precedence
    /// over the global `terminal_theme_override` setting and the app
    /// theme fallback. Stored as `TerminalTheme::name()` (e.g.
    /// "Dracula", "Monokai") so the value survives palette additions
    /// without a migration. `None` falls through to the global pick.
    #[serde(default)]
    pub terminal_theme: Option<String>,
    /// Set on hosts imported from a cloud profile (EC2 in v0.6). Carries
    /// the stable resource handle so the connect path can re-resolve
    /// hostname / pick the right transport on each session. `None` for
    /// manually-added hosts.
    #[serde(default)]
    pub cloud_ref: Option<CloudRef>,
    /// Sent to the remote shell right after the session opens. Used to
    /// escape minimal entry shells (`exec bash` on ECS / distroless) or
    /// to drop into a specific working directory. `None` skips the step.
    #[serde(default)]
    pub initial_command: Option<String>,
    /// When set, the startup command is resolved live from this snippet's
    /// body at connect time (so editing the snippet updates every host
    /// that references it). Takes precedence over `initial_command`; a
    /// dangling id (snippet deleted) resolves to no command, never an
    /// error. `None` means the startup command is the literal
    /// `initial_command` (custom) or nothing.
    #[serde(default)]
    pub startup_snippet_id: Option<Uuid>,
    /// Per-host SSH keepalive override (seconds). `None` inherits the
    /// global `keepalive_interval` setting. `Some(0)` explicitly disables
    /// keepalive on this host even when the global default is non-zero.
    /// `Some(n)` overrides the global with `n` seconds.
    #[serde(default)]
    pub keepalive_interval: Option<u32>,
    /// Per-host override for showing the shell-set window title (OSC 0/2) in
    /// the tab strip. `None` inherits the global `terminal_auto_title`
    /// setting; `Some(true)` always shows the shell title for this host,
    /// `Some(false)` always keeps this host's curated label.
    #[serde(default)]
    pub auto_title: Option<bool>,
    /// Shape to use when rendering this host's icon in cards / tabs /
    /// sidebar. Valid values: `"circular"`, `"square"`, `"outline"`,
    /// `"initials"`. `None` falls back to the global
    /// `default_host_icon` setting (default `"circular"` in v0.7).
    /// Stored as a String to keep the wire / sync payload identical for
    /// older peers that never saw the field.
    #[serde(default)]
    pub icon_style: Option<String>,
    /// Names of fields the user has explicitly overridden after this
    /// host was imported from a cloud provider. Reimport / refresh
    /// flows consult this list before overwriting a field with the
    /// upstream value: if the field name appears here, the user's value
    /// wins. Empty on manually-added hosts and on freshly-imported
    /// cloud hosts. Today only `label`, `hostname`, and `username` are
    /// tracked since those are the fields AWS discovery actually
    /// pushes; the structure stays open-ended so future providers can
    /// flag more without a schema change.
    #[serde(default)]
    pub customized_fields: Vec<String>,
    /// Per-host override for terminal session recording. `None` follows
    /// the global `session_logging` setting; `Some(true)` always records
    /// this host (even when the global toggle is off); `Some(false)`
    /// never records it (even when the global toggle is on).
    #[serde(default)]
    pub session_logging: Option<bool>,
    /// Per-host SSH algorithm overrides, one list per negotiation
    /// category. `None` = `Auto` (russh's safe defaults, untouched);
    /// `Some(list)` pins exactly those algorithm names (in order) for the
    /// category, which is how a user reaches legacy servers that only
    /// offer cbc / sha1 / dh-group1. Names are the on-the-wire strings
    /// (e.g. `"aes256-cbc"`, `"diffie-hellman-group14-sha1"`,
    /// `"hmac-sha1"`, `"ssh-rsa"`); unknown names are ignored by the
    /// engine. Stored as plain strings so the sync / export payload stays
    /// identical for older peers that never saw the fields.
    #[serde(default)]
    pub ciphers: Option<Vec<String>>,
    #[serde(default)]
    pub kex: Option<Vec<String>>,
    #[serde(default)]
    pub macs: Option<Vec<String>>,
    #[serde(default)]
    pub host_key_algorithms: Option<Vec<String>>,
    /// Per-host override for Privacy Mode (auto-hide sensitive data:
    /// host / ip / user / port / proxy on cards and logs, plus IP and
    /// `user@host` prompt tokens in the terminal). `None` follows the
    /// global `privacy_mode` setting; `Some(true)` always hides for this
    /// host (even when the global toggle is off); `Some(false)` never
    /// hides it (even when the global toggle is on).
    #[serde(default)]
    pub privacy_mode: Option<bool>,
}

impl Connection {
    pub fn new(label: impl Into<String>, hostname: impl Into<String>) -> Self {
        let now = chrono::Utc::now();
        Self {
            id: Uuid::new_v4(),
            label: label.into(),
            hostname: hostname.into(),
            port: 22,
            protocol: ConnectionProtocol::Ssh,
            serial: None,
            rd_kind: super::remote_desktop::RemoteDesktopKind::default(),
            rd_gateway_id: None,
            address_family: AddressFamily::default(),
            username: None,
            auth_method: AuthMethod::Auto,
            key_id: None,
            identity_id: None,
            group_id: None,
            jump_chain: Vec::new(),
            port_forwards: Vec::new(),
            env_vars: Vec::new(),
            encoding: None,
            terminal_type: None,
            proxy: None,
            proxy_identity_id: None,
            tags: Vec::new(),
            notes: None,
            color: None,
            mcp_enabled: true,
            last_used: None,
            created_at: now,
            updated_at: now,
            detected_os: None,
            custom_icon: None,
            custom_color: None,
            agent_forwarding: false,
            terminal_theme: None,
            cloud_ref: None,
            initial_command: None,
            startup_snippet_id: None,
            keepalive_interval: None,
            auto_title: None,
            icon_style: None,
            customized_fields: Vec::new(),
            session_logging: None,
            ciphers: None,
            kex: None,
            macs: None,
            host_key_algorithms: None,
            privacy_mode: None,
        }
    }
}

/// Which wire protocol a connection speaks. One selector per host, not
/// a per-host stack of protocols: the whole `Connection` model is
/// single-endpoint, so a host that needs both SSH and Telnet is two
/// hosts. Serialized as a plain string variant so older peers that
/// never saw the field simply ignore it on receive and omit it on send
/// (covered by the legacy-payload test below).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum ConnectionProtocol {
    #[default]
    Ssh,
    Telnet,
    /// Local serial line (no network). The port path lives in
    /// `hostname`; line parameters live in `Connection.serial`.
    Serial,
    /// Remote desktop (RDP/VNC). Unlike the others this is NOT a terminal
    /// transport: it opens no pane. `hostname`/`port` are the desktop
    /// endpoint and `username`/`password` its login; `rd_kind` picks
    /// RDP vs VNC and `rd_gateway_id` optionally routes the connection
    /// through an SSH host (the tunnel). The connect action launches the
    /// OS-native client instead of opening a terminal.
    RemoteDesktop,
}

impl ConnectionProtocol {
    /// Conventional TCP port, used by the host editor to swap the
    /// numeric-port default when the picker changes (22 <-> 23).
    /// `None` for `Serial`, which has no network port (the editor
    /// hides the numeric field entirely). `RemoteDesktop` reports the
    /// RDP default (3389); the kind picker refines it to VNC's 5900.
    pub fn default_port(self) -> Option<u16> {
        match self {
            ConnectionProtocol::Ssh => Some(22),
            ConnectionProtocol::Telnet => Some(23),
            ConnectionProtocol::Serial => None,
            ConnectionProtocol::RemoteDesktop => Some(3389),
        }
    }

    /// Whether this protocol drives a terminal pane (SSH/Telnet/Serial).
    /// `RemoteDesktop` does not, it launches an external client, so the
    /// terminal / SFTP / MCP paths must exclude it.
    pub fn is_terminal(self) -> bool {
        !matches!(self, ConnectionProtocol::RemoteDesktop)
    }
}

// Display feeds the host editor's pick_list mapper directly (the fork's
// 4-step pick_list API renders via `|p| p.to_string()`).
impl std::fmt::Display for ConnectionProtocol {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConnectionProtocol::Ssh => write!(f, "SSH"),
            ConnectionProtocol::Telnet => write!(f, "Telnet"),
            ConnectionProtocol::Serial => write!(f, "Serial"),
            ConnectionProtocol::RemoteDesktop => write!(f, "Remote Desktop"),
        }
    }
}

/// Address-family preference for outbound dials (PuTTY's Auto / IPv4 /
/// IPv6 setting). `Auto` takes the resolver's order; `V4` / `V6` keep
/// only that family's addresses and fail honestly when the name
/// resolves to none of them.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum AddressFamily {
    #[default]
    Auto,
    V4,
    V6,
}

// Display feeds the host editor's pick_list mapper.
impl std::fmt::Display for AddressFamily {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AddressFamily::Auto => write!(f, "Auto"),
            AddressFamily::V4 => write!(f, "IPv4"),
            AddressFamily::V6 => write!(f, "IPv6"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum AuthMethod {
    #[default]
    Auto,
    Password,
    Key,
    Agent,
    Interactive,
    /// Password auth where the password is never stored: the app prompts
    /// for it at every connect and feeds the typed value straight to the
    /// server. Falls back to any caller-provided password when no UI
    /// prompt channel is wired (headless / MCP).
    PasswordPrompt,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PortForward {
    pub local_port: u16,
    pub remote_host: String,
    pub remote_port: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EnvVar {
    pub key: String,
    pub value: String,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct ProxyConfig {
    pub proxy_type: ProxyType,
    pub host: String,
    pub port: u16,
    pub username: Option<String>,
    /// Proxy password. Hydrated in-memory by the vault
    /// (`get_proxy_password`) right before connect. Marked `serde(skip)`
    /// so it never lands in the `proxy` column (which is plaintext JSON)
    ///, the credential lives in the encrypted `proxy_password` column.
    #[serde(skip)]
    pub password: Option<String>,
}

// Hand-written so a hydrated proxy formatted with `{:?}` (logs, error
// chains) can never print the password.
impl std::fmt::Debug for ProxyConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProxyConfig")
            .field("proxy_type", &self.proxy_type)
            .field("host", &self.host)
            .field("port", &self.port)
            .field("username", &self.username)
            .field("password", &self.password.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ProxyType {
    Socks5,
    Socks4,
    Http,
    Command(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_connection_defaults() {
        let conn = Connection::new("test", "10.0.0.1");
        assert_eq!(conn.label, "test");
        assert_eq!(conn.hostname, "10.0.0.1");
        assert_eq!(conn.port, 22);
        assert_eq!(conn.auth_method, AuthMethod::Auto);
        assert!(conn.username.is_none());
        assert!(conn.jump_chain.is_empty());
        assert!(conn.proxy.is_none());
    }

    #[test]
    fn connection_serialization_roundtrip() {
        let mut conn = Connection::new("prod", "server.example.com");
        conn.username = Some("deploy".into());
        conn.auth_method = AuthMethod::Key;
        conn.tags = vec!["production".into(), "web".into()];

        let json = serde_json::to_string(&conn).unwrap();
        let deserialized: Connection = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.label, "prod");
        assert_eq!(deserialized.hostname, "server.example.com");
        assert_eq!(deserialized.username, Some("deploy".into()));
        assert_eq!(deserialized.auth_method, AuthMethod::Key);
        assert_eq!(deserialized.tags.len(), 2);
    }

    #[test]
    fn proxy_config_serialization() {
        let proxy = ProxyConfig {
            proxy_type: ProxyType::Socks5,
            host: "proxy.local".into(),
            port: 1080,
            username: Some("user".into()),
            password: None,
        };

        let json = serde_json::to_string(&proxy).unwrap();
        let de: ProxyConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(de.proxy_type, ProxyType::Socks5);
        assert_eq!(de.port, 1080);
        assert_eq!(de.username.as_deref(), Some("user"));
        assert!(de.password.is_none());
    }

    /// `password` is `serde(skip)`, it must not appear in serialized
    /// JSON nor be read back. This guards against credential leaks via
    /// the plaintext `proxy` column.
    #[test]
    fn proxy_config_password_is_not_serialized() {
        let proxy = ProxyConfig {
            proxy_type: ProxyType::Http,
            host: "proxy.local".into(),
            port: 8080,
            username: Some("u".into()),
            password: Some("topsecret".into()),
        };

        let json = serde_json::to_string(&proxy).unwrap();
        assert!(
            !json.contains("topsecret"),
            "password leaked into ProxyConfig JSON: {json}"
        );
        assert!(
            !json.contains("password"),
            "password key should not appear at all: {json}"
        );

        let de: ProxyConfig = serde_json::from_str(&json).unwrap();
        assert!(de.password.is_none());
    }

    /// Legacy peers (sync wire) and old portable exports never carried
    /// the `keepalive_interval` field. Receiving such a payload must
    /// deserialize cleanly with the field defaulting to `None` (= inherit
    /// global). Without `#[serde(default)]` on the field, this would
    /// regress the moment a v1 peer talks to a v2 peer.
    #[test]
    fn keepalive_interval_legacy_payload_defaults_to_none() {
        let conn = Connection::new("legacy", "10.0.0.1");
        let mut value = serde_json::to_value(&conn).unwrap();
        // Simulate a payload from a peer that never knew about the field.
        value.as_object_mut().unwrap().remove("keepalive_interval");
        let de: Connection = serde_json::from_value(value).unwrap();
        assert_eq!(de.keepalive_interval, None);
    }

    /// Same contract for the protocol selector: a payload written
    /// before Telnet existed carries no `protocol` key and must land as
    /// `Ssh`, because that is what every pre-existing host is.
    #[test]
    fn protocol_legacy_payload_defaults_to_ssh() {
        let conn = Connection::new("legacy", "10.0.0.1");
        let mut value = serde_json::to_value(&conn).unwrap();
        value.as_object_mut().unwrap().remove("protocol");
        let de: Connection = serde_json::from_value(value).unwrap();
        assert_eq!(de.protocol, ConnectionProtocol::Ssh);
    }

    /// Same contract for the address-family preference: a payload from
    /// before the field existed must land as `Auto` (the behavior every
    /// pre-existing host had).
    #[test]
    fn address_family_legacy_payload_defaults_to_auto() {
        let conn = Connection::new("legacy", "10.0.0.1");
        let mut value = serde_json::to_value(&conn).unwrap();
        value.as_object_mut().unwrap().remove("address_family");
        let de: Connection = serde_json::from_value(value).unwrap();
        assert_eq!(de.address_family, AddressFamily::Auto);
    }

    #[test]
    fn address_family_round_trips() {
        let mut conn = Connection::new("v6-host", "host.example");
        conn.address_family = AddressFamily::V6;
        let json = serde_json::to_string(&conn).unwrap();
        let de: Connection = serde_json::from_str(&json).unwrap();
        assert_eq!(de.address_family, AddressFamily::V6);
    }

    #[test]
    fn protocol_telnet_round_trips() {
        let mut conn = Connection::new("router", "192.168.0.1");
        conn.protocol = ConnectionProtocol::Telnet;
        conn.port = 23;
        let json = serde_json::to_string(&conn).unwrap();
        let de: Connection = serde_json::from_str(&json).unwrap();
        assert_eq!(de.protocol, ConnectionProtocol::Telnet);
    }

    #[test]
    fn keepalive_interval_round_trip() {
        let mut conn = Connection::new("h", "1.2.3.4");
        conn.keepalive_interval = Some(45);
        let json = serde_json::to_string(&conn).unwrap();
        let de: Connection = serde_json::from_str(&json).unwrap();
        assert_eq!(de.keepalive_interval, Some(45));

        // Explicit zero must round-trip distinctly from None, they have
        // different semantics (per-host disable vs. inherit global).
        conn.keepalive_interval = Some(0);
        let json = serde_json::to_string(&conn).unwrap();
        let de: Connection = serde_json::from_str(&json).unwrap();
        assert_eq!(de.keepalive_interval, Some(0));
    }

    /// The legacy-cipher override fields are newest of all; a peer / export
    /// without them must default every category to `None` (= Auto).
    #[test]
    fn algorithm_overrides_legacy_payload_defaults_to_none() {
        let conn = Connection::new("legacy", "10.0.0.1");
        let mut value = serde_json::to_value(&conn).unwrap();
        let obj = value.as_object_mut().unwrap();
        for f in ["ciphers", "kex", "macs", "host_key_algorithms"] {
            obj.remove(f);
        }
        let de: Connection = serde_json::from_value(value).unwrap();
        assert_eq!(de.ciphers, None);
        assert_eq!(de.kex, None);
        assert_eq!(de.macs, None);
        assert_eq!(de.host_key_algorithms, None);
    }

    #[test]
    fn algorithm_overrides_round_trip() {
        let mut conn = Connection::new("h", "1.2.3.4");
        conn.ciphers = Some(vec!["aes256-cbc".into(), "3des-cbc".into()]);
        conn.kex = Some(vec!["diffie-hellman-group14-sha1".into()]);
        let json = serde_json::to_string(&conn).unwrap();
        let de: Connection = serde_json::from_str(&json).unwrap();
        assert_eq!(de.ciphers.as_deref(), Some(&["aes256-cbc".to_string(), "3des-cbc".to_string()][..]));
        assert_eq!(de.kex.as_deref(), Some(&["diffie-hellman-group14-sha1".to_string()][..]));
        assert_eq!(de.macs, None);
    }

    #[test]
    fn auto_title_legacy_payload_defaults_to_none() {
        let conn = Connection::new("legacy", "10.0.0.1");
        let mut value = serde_json::to_value(&conn).unwrap();
        value.as_object_mut().unwrap().remove("auto_title");
        let de: Connection = serde_json::from_value(value).unwrap();
        assert_eq!(de.auto_title, None);
    }

    #[test]
    fn terminal_type_legacy_payload_defaults_to_none() {
        let conn = Connection::new("legacy", "10.0.0.1");
        let mut value = serde_json::to_value(&conn).unwrap();
        value.as_object_mut().unwrap().remove("terminal_type");
        let de: Connection = serde_json::from_value(value).unwrap();
        assert_eq!(de.terminal_type, None);
    }

    #[test]
    fn auto_title_round_trip() {
        let mut conn = Connection::new("h", "1.2.3.4");
        // None (inherit), Some(true) (force on), Some(false) (force off) must
        // each round-trip distinctly, they have different semantics.
        for v in [None, Some(true), Some(false)] {
            conn.auto_title = v;
            let json = serde_json::to_string(&conn).unwrap();
            let de: Connection = serde_json::from_str(&json).unwrap();
            assert_eq!(de.auto_title, v);
        }
    }

    #[test]
    fn privacy_mode_legacy_payload_defaults_to_none() {
        let conn = Connection::new("legacy", "10.0.0.1");
        let mut value = serde_json::to_value(&conn).unwrap();
        value.as_object_mut().unwrap().remove("privacy_mode");
        let de: Connection = serde_json::from_value(value).unwrap();
        assert_eq!(de.privacy_mode, None);
    }

    #[test]
    fn privacy_mode_round_trip() {
        let mut conn = Connection::new("h", "1.2.3.4");
        // None (inherit), Some(true) (force on), Some(false) (force off) must
        // each round-trip distinctly, they have different semantics.
        for v in [None, Some(true), Some(false)] {
            conn.privacy_mode = v;
            let json = serde_json::to_string(&conn).unwrap();
            let de: Connection = serde_json::from_str(&json).unwrap();
            assert_eq!(de.privacy_mode, v);
        }
    }

    #[test]
    fn auth_method_variants() {
        assert_eq!(serde_json::to_string(&AuthMethod::Auto).unwrap(), "\"Auto\"");
        assert_eq!(serde_json::to_string(&AuthMethod::Password).unwrap(), "\"Password\"");
        assert_eq!(serde_json::to_string(&AuthMethod::Key).unwrap(), "\"Key\"");
        assert_eq!(serde_json::to_string(&AuthMethod::Agent).unwrap(), "\"Agent\"");
        assert_eq!(serde_json::to_string(&AuthMethod::Interactive).unwrap(), "\"Interactive\"");
        assert_eq!(serde_json::to_string(&AuthMethod::PasswordPrompt).unwrap(), "\"PasswordPrompt\"");
    }
}
