//! PuTTY session importer: parses a `.reg` export of
//! `HKEY_CURRENT_USER\Software\SimonTatham\PuTTY\Sessions` into
//! [`Connection`]s (issue D2, first importer).
//!
//! The `.reg` route works on every platform (regedit exports it in
//! one step on Windows; the file also travels to a Linux/macOS
//! machine trivially), which is why it ships first; reading the live
//! registry directly is a Windows-only convenience on the roadmap.
//!
//! Only the directives Oryxis can represent are mapped; everything
//! else is ignored. Sessions whose `Protocol` has no Oryxis
//! equivalent (raw, rlogin, supdup) are skipped and reported by name
//! in the parse result so the preview can say so instead of silently
//! shrinking. PuTTY does not store passwords, so there are none to
//! carry; a `PublicKeyFile` (.ppk) becomes a note plus
//! `AuthMethod::Key`, mirroring the ssh_config importer's decision to
//! let the user finish the key link in the editor (the .ppk file may
//! not even exist on this machine).

use oryxis_core::models::connection::{
    AuthMethod, Connection, ConnectionProtocol, ProxyConfig, ProxyType,
};
use oryxis_core::models::serial::{
    SerialFlowControl, SerialParams, SerialParity, SerialStopBits,
};

/// One parsed session block, before mapping.
#[derive(Debug, Clone, Default)]
struct PuttySession {
    name: String,
    host_name: Option<String>,
    port: Option<u16>,
    user: Option<String>,
    protocol: Option<String>,
    public_key_file: Option<String>,
    agent_fwd: bool,
    x11_forward: bool,
    proxy_method: u32,
    proxy_host: Option<String>,
    proxy_port: Option<u16>,
    proxy_username: Option<String>,
    proxy_telnet_command: Option<String>,
    serial_line: Option<String>,
    serial_speed: Option<u32>,
    serial_data_bits: Option<u8>,
    serial_parity: Option<u32>,
    serial_stop_halfbits: Option<u32>,
    serial_flow: Option<u32>,
}

/// Everything a parse produced: the mapped connections plus the
/// sessions that had to be skipped (unsupported protocol, or no host
/// to connect to), by name, so the UI can be honest about the
/// difference between "file had nothing" and "file had 12 sessions,
/// 2 of them raw-mode".
#[derive(Debug, Default)]
pub struct PuttyImport {
    pub connections: Vec<Connection>,
    pub skipped: Vec<String>,
}

use super::regfile::{decode_session_name, split_reg_line};

/// Parse a `.reg` export into sessions and map them. Tolerant by
/// design: unknown keys are ignored, malformed lines are skipped, and
/// only blocks under the PuTTY `Sessions` path are considered (a full
/// `HKCU\Software\SimonTatham` export carries jump lists and host key
/// caches too).
pub fn parse_reg(text: &str) -> PuttyImport {
    const SESSIONS_MARKER: &str = "\\SimonTatham\\PuTTY\\Sessions\\";

    let mut sessions: Vec<PuttySession> = Vec::new();
    let mut current: Option<PuttySession> = None;
    for raw in text.lines() {
        let line = raw.trim_end_matches('\r').trim();
        if let Some(path) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            // New block: close the previous session (if any) and open
            // one when the path is a session of the PuTTY hive.
            if let Some(prev) = current.take() {
                sessions.push(prev);
            }
            current = path.find(SESSIONS_MARKER).map(|at| PuttySession {
                name: decode_session_name(&path[at + SESSIONS_MARKER.len()..]),
                ..Default::default()
            });
            continue;
        }
        let Some(session) = current.as_mut() else {
            continue;
        };
        let Some((key, value)) = split_reg_line(line) else {
            continue;
        };
        match key {
            "HostName" => session.host_name = value.as_str().map(str::to_string),
            "PortNumber" => session.port = value.as_dword().map(|d| d as u16),
            "UserName" => session.user = value.as_str().map(str::to_string),
            "Protocol" => session.protocol = value.as_str().map(str::to_string),
            "PublicKeyFile" => {
                session.public_key_file =
                    value.as_str().filter(|s| !s.is_empty()).map(str::to_string)
            }
            "AgentFwd" => session.agent_fwd = value.as_dword() == Some(1),
            "X11Forward" => session.x11_forward = value.as_dword() == Some(1),
            "ProxyMethod" => session.proxy_method = value.as_dword().unwrap_or(0),
            "ProxyHost" => session.proxy_host = value.as_str().map(str::to_string),
            "ProxyPort" => session.proxy_port = value.as_dword().map(|d| d as u16),
            "ProxyUsername" => {
                session.proxy_username =
                    value.as_str().filter(|s| !s.is_empty()).map(str::to_string)
            }
            "ProxyTelnetCommand" => {
                session.proxy_telnet_command =
                    value.as_str().filter(|s| !s.is_empty()).map(str::to_string)
            }
            "SerialLine" => session.serial_line = value.as_str().map(str::to_string),
            "SerialSpeed" => session.serial_speed = value.as_dword(),
            "SerialDataBits" => {
                session.serial_data_bits = value.as_dword().map(|d| d as u8)
            }
            "SerialParity" => session.serial_parity = value.as_dword(),
            "SerialStopHalfbits" => session.serial_stop_halfbits = value.as_dword(),
            "SerialFlowControl" => session.serial_flow = value.as_dword(),
            _ => {}
        }
    }
    if let Some(prev) = current.take() {
        sessions.push(prev);
    }

    let mut out = PuttyImport::default();
    for session in sessions {
        // "Default Settings" is PuTTY's template pseudo-session, the
        // exact analogue of the ssh_config wildcard blocks we skip.
        if session.name.is_empty() || session.name == "Default Settings" {
            continue;
        }
        match to_connection(&session) {
            Some(conn) => out.connections.push(conn),
            None => out.skipped.push(session.name.clone()),
        }
    }
    out
}

/// Map one session; `None` when the protocol has no Oryxis analogue.
fn to_connection(s: &PuttySession) -> Option<Connection> {
    let protocol = s.protocol.as_deref().unwrap_or("ssh");
    let (proto, default_port) = match protocol {
        "ssh" => (ConnectionProtocol::Ssh, 22),
        "telnet" => (ConnectionProtocol::Telnet, 23),
        "serial" => (ConnectionProtocol::Serial, 0),
        _ => return None,
    };

    let mut conn = match proto {
        ConnectionProtocol::Serial => {
            // The port path lives in `hostname` for serial hosts.
            let line = s.serial_line.clone().unwrap_or_default();
            if line.is_empty() {
                return None;
            }
            let mut conn = Connection::new(s.name.clone(), line);
            let mut params = SerialParams {
                baud: s.serial_speed.unwrap_or(9600),
                ..Default::default()
            };
            if let Some(bits @ 5..=8) = s.serial_data_bits {
                params.data_bits = bits;
            }
            // PuTTY: 0 none, 1 odd, 2 even, 3 mark, 4 space. Mark and
            // space have no analogue here; they fall back to the
            // default rather than silently meaning something else.
            params.parity = match s.serial_parity {
                Some(1) => SerialParity::Odd,
                Some(2) => SerialParity::Even,
                _ => SerialParity::None,
            };
            // PuTTY stores HALF bits: 2 = one stop bit, 4 = two. The
            // odd 1.5 (3) has no analogue; nearest wins.
            params.stop_bits = match s.serial_stop_halfbits {
                Some(4) => SerialStopBits::Two,
                _ => SerialStopBits::One,
            };
            // PuTTY: 0 none, 1 XON/XOFF, 2 RTS/CTS, 3 DSR/DTR (no
            // analogue, defaults to none).
            params.flow_control = match s.serial_flow {
                Some(1) => SerialFlowControl::Software,
                Some(2) => SerialFlowControl::Hardware,
                _ => SerialFlowControl::None,
            };
            conn.serial = Some(params);
            conn
        }
        _ => {
            let host = s.host_name.clone().unwrap_or_default();
            if host.is_empty() {
                // A session saved with no host is a half-filled dialog,
                // not a server.
                return None;
            }
            let mut conn = Connection::new(s.name.clone(), host);
            conn.port = s.port.unwrap_or(default_port);
            conn
        }
    };
    conn.protocol = proto;
    if let Some(user) = &s.user
        && !user.is_empty()
    {
        conn.username = Some(user.clone());
    }
    conn.agent_forwarding = s.agent_fwd;
    conn.x11_forwarding = s.x11_forward;

    // Proxy: PuTTY ProxyMethod 0 none, 1 SOCKS4, 2 SOCKS5, 3 HTTP,
    // 4 Telnet, 5 Local. Telnet/Local both run the ProxyTelnetCommand
    // through a subprocess on PuTTY's side, which is exactly our
    // `Command` proxy.
    let proxy_type = match s.proxy_method {
        1 => Some(ProxyType::Socks4),
        2 => Some(ProxyType::Socks5),
        3 => Some(ProxyType::Http),
        4 | 5 => s
            .proxy_telnet_command
            .clone()
            .map(ProxyType::Command),
        _ => None,
    };
    if let Some(proxy_type) = proxy_type {
        let is_command = matches!(proxy_type, ProxyType::Command(_));
        conn.proxy = Some(ProxyConfig {
            proxy_type,
            host: if is_command {
                String::new()
            } else {
                s.proxy_host.clone().unwrap_or_default()
            },
            port: if is_command {
                0
            } else {
                s.proxy_port.unwrap_or(0)
            },
            username: if is_command {
                None
            } else {
                s.proxy_username.clone()
            },
            password: None,
        });
    }

    let mut notes = format!("Imported from PuTTY (session `{}`)", s.name);
    if let Some(ppk) = &s.public_key_file {
        // Lean Key like the ssh_config importer; the .ppk may not even
        // exist on this machine, so the actual key import (Keychain >
        // Import Key reads .ppk) stays a deliberate user step.
        conn.auth_method = AuthMethod::Key;
        notes.push_str(&format!("\nPuTTY key file: {ppk}"));
    }
    conn.notes = Some(notes);
    Some(conn)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"Windows Registry Editor Version 5.00

[HKEY_CURRENT_USER\Software\SimonTatham\PuTTY\Sessions\Default%20Settings]
"HostName"=""
"Protocol"="ssh"

[HKEY_CURRENT_USER\Software\SimonTatham\PuTTY\Sessions\prod%20web%2f1]
"HostName"="web1.example.com"
"PortNumber"=dword:00000826
"UserName"="deploy"
"Protocol"="ssh"
"AgentFwd"=dword:00000001
"PublicKeyFile"="C:\\keys\\deploy.ppk"
"ProxyMethod"=dword:00000002
"ProxyHost"="socks.corp"
"ProxyPort"=dword:00000438
"ProxyUsername"="proxyuser"

[HKEY_CURRENT_USER\Software\SimonTatham\PuTTY\Sessions\switch]
"HostName"="10.0.0.2"
"Protocol"="telnet"
"PortNumber"=dword:00000017

[HKEY_CURRENT_USER\Software\SimonTatham\PuTTY\Sessions\console]
"Protocol"="serial"
"SerialLine"="COM3"
"SerialSpeed"=dword:0001c200
"SerialDataBits"=dword:00000007
"SerialParity"=dword:00000002
"SerialStopHalfbits"=dword:00000004
"SerialFlowControl"=dword:00000002

[HKEY_CURRENT_USER\Software\SimonTatham\PuTTY\Sessions\weird]
"HostName"="legacy"
"Protocol"="rlogin"

[HKEY_CURRENT_USER\Software\SimonTatham\PuTTY\Jumplist]
"Recent sessions"="prod"
"#;

    #[test]
    fn parses_sessions_and_skips_the_rest() {
        let import = parse_reg(SAMPLE);
        // Default Settings is silently dropped (template), rlogin is
        // reported as skipped, the Jumplist block never opens a
        // session.
        assert_eq!(import.connections.len(), 3);
        assert_eq!(import.skipped, vec!["weird".to_string()]);
    }

    #[test]
    fn ssh_session_maps_every_field() {
        let import = parse_reg(SAMPLE);
        let c = &import.connections[0];
        // %20 and %2f decode in the registry-escaped session name.
        assert_eq!(c.label, "prod web/1");
        assert_eq!(c.hostname, "web1.example.com");
        assert_eq!(c.port, 0x826);
        assert_eq!(c.username.as_deref(), Some("deploy"));
        assert!(c.agent_forwarding);
        assert_eq!(c.auth_method, AuthMethod::Key);
        let proxy = c.proxy.as_ref().expect("proxy imported");
        assert_eq!(proxy.proxy_type, ProxyType::Socks5);
        assert_eq!(proxy.host, "socks.corp");
        assert_eq!(proxy.port, 0x438);
        assert_eq!(proxy.username.as_deref(), Some("proxyuser"));
        // The .ppk path is preserved for the manual key link.
        assert!(c.notes.as_deref().unwrap().contains("deploy.ppk"));
    }

    #[test]
    fn telnet_and_serial_sessions_map_their_protocols() {
        let import = parse_reg(SAMPLE);
        let telnet = &import.connections[1];
        assert_eq!(telnet.protocol, ConnectionProtocol::Telnet);
        assert_eq!(telnet.hostname, "10.0.0.2");
        assert_eq!(telnet.port, 23);

        let serial = &import.connections[2];
        assert_eq!(serial.protocol, ConnectionProtocol::Serial);
        assert_eq!(serial.hostname, "COM3");
        let params = serial.serial.as_ref().expect("serial params");
        assert_eq!(params.baud, 115_200);
        assert_eq!(params.data_bits, 7);
        assert_eq!(params.parity, SerialParity::Even);
        assert_eq!(params.stop_bits, SerialStopBits::Two);
        assert_eq!(params.flow_control, SerialFlowControl::Hardware);
    }

    #[test]
    fn utf16le_reg_export_decodes() {
        let text = "[HKEY_CURRENT_USER\\Software\\SimonTatham\\PuTTY\\Sessions\\u16]\r\n\"HostName\"=\"härtel\"\r\n\"Protocol\"=\"ssh\"\r\n";
        let mut bytes: Vec<u8> = vec![0xFF, 0xFE];
        for unit in text.encode_utf16() {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
        let decoded = crate::importers::regfile::decode_reg_bytes(&bytes);
        let import = parse_reg(&decoded);
        assert_eq!(import.connections.len(), 1);
        assert_eq!(import.connections[0].hostname, "härtel");
    }

    #[test]
    fn a_session_with_no_host_is_not_a_server() {
        let text = "[HKEY_CURRENT_USER\\Software\\SimonTatham\\PuTTY\\Sessions\\empty]\n\"Protocol\"=\"ssh\"\n";
        let import = parse_reg(text);
        assert!(import.connections.is_empty());
        // It still shows up as skipped: the file DID carry a session
        // by that name, and silence would read as data loss.
        assert_eq!(import.skipped, vec!["empty".to_string()]);
    }
}
