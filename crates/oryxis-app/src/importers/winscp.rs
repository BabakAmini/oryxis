//! WinSCP site importer: parses either a portable `WinSCP.ini` or a
//! `.reg` export of `HKCU\Software\Martin Prikryl\WinSCP 2\Sessions`
//! (same keys, two containers) into [`DirectHost`]s.
//!
//! Passwords: WinSCP stores site passwords under a documented,
//! reversible obfuscation (the "simple" PWALG, keyed on
//! username+host) unless the user set a WinSCP master password, in
//! which case the value is real cryptography and undecodable without
//! it. The scheme carries its own validity check (the decoded text
//! must start with the key), so a master-password blob decodes to
//! garbage, fails the check, and the site imports without a password
//! plus a note, never with a corrupted one. Importing the decodable
//! ones is the plan's call: it is the user's own data and the main
//! migration pain; they land straight in the vault's encrypted
//! column and only ever live in memory in between.
//!
//! Only SFTP/SCP sites map (they ride SSH); FTP, WebDAV and S3 sites
//! have no Oryxis transport and are reported by name.

use oryxis_core::models::connection::{
    AuthMethod, Connection, ProxyConfig, ProxyType,
};

use super::regfile::{decode_session_name, split_reg_line};
use super::DirectHost;

/// One parsed `[Sessions\...]` block, before mapping.
#[derive(Debug, Clone, Default)]
struct WinScpSite {
    name: String,
    host_name: Option<String>,
    port: Option<u16>,
    user: Option<String>,
    password_blob: Option<String>,
    fs_protocol: Option<u32>,
    public_key_file: Option<String>,
    agent_fwd: bool,
    proxy_method: u32,
    proxy_host: Option<String>,
    proxy_port: Option<u16>,
    proxy_username: Option<String>,
    proxy_telnet_command: Option<String>,
}

// No Debug on purpose: `DirectHost` carries a decoded password, and a
// stray debug print must not be able to put it in a log line.
#[derive(Default)]
pub struct WinScpImport {
    pub hosts: Vec<DirectHost>,
    pub skipped: Vec<String>,
}

/// Parse either container: the `.reg` export announces itself on the
/// first line; anything else is treated as the INI shape.
pub fn parse(text: &str) -> WinScpImport {
    let sites = if text.trim_start().starts_with("Windows Registry Editor") {
        parse_reg_sites(text)
    } else {
        parse_ini_sites(text)
    };

    let mut out = WinScpImport::default();
    for site in sites {
        // "Default%20Settings" is WinSCP's template pseudo-site, like
        // PuTTY's.
        if site.name.is_empty() || site.name == "Default Settings" {
            continue;
        }
        match to_host(&site) {
            Some(host) => out.hosts.push(host),
            None => out.skipped.push(site.name.clone()),
        }
    }
    out
}

fn parse_reg_sites(text: &str) -> Vec<WinScpSite> {
    const SESSIONS_MARKER: &str = "\\WinSCP 2\\Sessions\\";
    let mut sites: Vec<WinScpSite> = Vec::new();
    let mut current: Option<WinScpSite> = None;
    for raw in text.lines() {
        let line = raw.trim_end_matches('\r').trim();
        if let Some(path) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            if let Some(prev) = current.take() {
                sites.push(prev);
            }
            current = path.find(SESSIONS_MARKER).map(|at| WinScpSite {
                name: decode_session_name(&path[at + SESSIONS_MARKER.len()..]),
                ..Default::default()
            });
            continue;
        }
        let Some(site) = current.as_mut() else {
            continue;
        };
        let Some((key, value)) = split_reg_line(line) else {
            continue;
        };
        apply(site, key, value.as_str(), value.as_dword());
    }
    if let Some(prev) = current.take() {
        sites.push(prev);
    }
    sites
}

fn parse_ini_sites(text: &str) -> Vec<WinScpSite> {
    const SECTION_PREFIX: &str = "Sessions\\";
    let mut sites: Vec<WinScpSite> = Vec::new();
    let mut current: Option<WinScpSite> = None;
    for raw in text.lines() {
        let line = raw.trim_end_matches('\r').trim();
        if line.is_empty() || line.starts_with(';') {
            continue;
        }
        if let Some(section) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            if let Some(prev) = current.take() {
                sites.push(prev);
            }
            current = section.strip_prefix(SECTION_PREFIX).map(|name| WinScpSite {
                name: decode_session_name(name),
                ..Default::default()
            });
            continue;
        }
        let Some(site) = current.as_mut() else {
            continue;
        };
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        // INI numbers are plain decimal where the registry uses dwords.
        apply(site, key, Some(value), value.parse::<u32>().ok());
    }
    if let Some(prev) = current.take() {
        sites.push(prev);
    }
    sites
}

/// One site key, container-agnostic: `text` is the raw string value
/// (both containers), `number` its numeric reading where one exists.
fn apply(site: &mut WinScpSite, key: &str, text: Option<&str>, number: Option<u32>) {
    match key {
        "HostName" => site.host_name = text.map(str::to_string),
        "PortNumber" => site.port = number.map(|d| d as u16),
        "UserName" => site.user = text.map(str::to_string),
        "Password" => {
            site.password_blob = text.filter(|s| !s.is_empty()).map(str::to_string)
        }
        "FSProtocol" => site.fs_protocol = number,
        "PublicKeyFile" => {
            site.public_key_file = text.filter(|s| !s.is_empty()).map(str::to_string)
        }
        "AgentFwd" => site.agent_fwd = number == Some(1),
        "ProxyMethod" => site.proxy_method = number.unwrap_or(0),
        "ProxyHost" => site.proxy_host = text.map(str::to_string),
        "ProxyPort" => site.proxy_port = number.map(|d| d as u16),
        "ProxyUsername" => {
            site.proxy_username = text.filter(|s| !s.is_empty()).map(str::to_string)
        }
        "ProxyTelnetCommand" => {
            site.proxy_telnet_command =
                text.filter(|s| !s.is_empty()).map(str::to_string)
        }
        _ => {}
    }
}

/// Map one site; `None` when the protocol has no Oryxis analogue.
fn to_host(site: &WinScpSite) -> Option<DirectHost> {
    // FSProtocol: 0 SFTP (SCP fallback), 1 SCP, 2 SFTP-only ride SSH;
    // 5 FTP, 6 WebDAV, 7 S3 do not. Missing means the default (SFTP).
    match site.fs_protocol.unwrap_or(0) {
        0..=2 => {}
        _ => return None,
    }
    let host = site.host_name.clone().unwrap_or_default();
    if host.is_empty() {
        return None;
    }

    let mut conn = Connection::new(site.name.clone(), host.clone());
    conn.port = site.port.unwrap_or(22);
    if let Some(user) = &site.user
        && !user.is_empty()
    {
        conn.username = Some(user.clone());
    }
    conn.agent_forwarding = site.agent_fwd;

    // Proxy codes are PuTTY's (WinSCP inherits its connection layer):
    // 0 none, 1 SOCKS4, 2 SOCKS5, 3 HTTP, 4 Telnet, 5 Local.
    let proxy_type = match site.proxy_method {
        1 => Some(ProxyType::Socks4),
        2 => Some(ProxyType::Socks5),
        3 => Some(ProxyType::Http),
        4 | 5 => site.proxy_telnet_command.clone().map(ProxyType::Command),
        _ => None,
    };
    if let Some(proxy_type) = proxy_type {
        let is_command = matches!(proxy_type, ProxyType::Command(_));
        conn.proxy = Some(ProxyConfig {
            proxy_type,
            host: if is_command {
                String::new()
            } else {
                site.proxy_host.clone().unwrap_or_default()
            },
            port: if is_command { 0 } else { site.proxy_port.unwrap_or(0) },
            username: if is_command { None } else { site.proxy_username.clone() },
            password: None,
        });
    }

    let mut notes = format!("Imported from WinSCP (site `{}`)", site.name);
    if let Some(ppk) = &site.public_key_file {
        conn.auth_method = AuthMethod::Key;
        notes.push_str(&format!("\nWinSCP key file: {ppk}"));
    }

    // Password: decode the reversible scheme; a failure means a
    // WinSCP master password (or corruption), which is worth saying.
    let password = site.password_blob.as_deref().and_then(|blob| {
        decode_password(
            site.host_name.as_deref().unwrap_or_default(),
            site.user.as_deref().unwrap_or_default(),
            blob,
        )
    });
    if password.is_some() {
        if conn.auth_method == AuthMethod::Auto {
            conn.auth_method = AuthMethod::Password;
        }
    } else if site.password_blob.is_some() {
        notes.push_str(
            "\nStored password could not be decoded (WinSCP master password?), set it manually",
        );
    }
    conn.notes = Some(notes);
    Some(DirectHost { conn, password })
}

const PW_MAGIC: u8 = 0xA3;
const PW_FLAG: u8 = 0xFF;

/// Decode WinSCP's "simple" password obfuscation: a hex string where
/// each byte is `!(plain ^ 0xA3)`, framed by a flag, a length and a
/// junk offset, with `username + hostname` prepended to the plaintext
/// as an integrity key. Documented and implemented publicly many
/// times over; the key-prefix check is the validity gate that rejects
/// master-password blobs.
fn decode_password(host: &str, user: &str, blob: &str) -> Option<String> {
    let mut digits = blob.trim().chars();
    let mut next = || -> Option<u8> {
        let hi = digits.next()?.to_digit(16)? as u8;
        let lo = digits.next()?.to_digit(16)? as u8;
        Some(!((hi << 4) | lo) ^ PW_MAGIC)
    };

    let flag = next()?;
    let length = if flag == PW_FLAG {
        let _unused = next()?;
        next()?
    } else {
        flag
    };
    let offset = next()?;
    for _ in 0..offset {
        let _ = next()?;
    }
    let mut clear = Vec::with_capacity(length as usize);
    for _ in 0..length {
        clear.push(next()?);
    }
    let clear = String::from_utf8(clear).ok()?;
    if flag == PW_FLAG {
        let key = format!("{user}{host}");
        return clear.strip_prefix(key.as_str()).map(str::to_string);
    }
    Some(clear)
}

#[cfg(test)]
mod tests {
    use super::*;
    use oryxis_core::models::connection::ConnectionProtocol;

    /// The inverse of `decode_password`, used to build test vectors:
    /// flag, junk byte, length, zero offset, then `key + password`.
    fn encode_password(host: &str, user: &str, password: &str) -> String {
        let plain = format!("{user}{host}{password}");
        let mut bytes = vec![PW_FLAG, 0x00, plain.len() as u8, 0x00];
        bytes.extend_from_slice(plain.as_bytes());
        bytes
            .iter()
            .map(|b| format!("{:02X}", !(b ^ PW_MAGIC)))
            .collect()
    }

    #[test]
    fn password_roundtrip_and_master_password_rejection() {
        let blob = encode_password("db1.example.com", "root", "hunter2");
        assert_eq!(
            decode_password("db1.example.com", "root", &blob).as_deref(),
            Some("hunter2"),
        );
        // A blob scrambled for ANOTHER key (what a master-password
        // AES blob effectively is to this decoder) fails the prefix
        // check instead of returning garbage.
        assert_eq!(decode_password("other.host", "root", &blob), None);
        // Truncated / non-hex blobs fail cleanly too.
        assert_eq!(decode_password("db1.example.com", "root", "F3A9"), None);
        assert_eq!(decode_password("db1.example.com", "root", "zz"), None);
    }

    #[test]
    fn ini_sites_map_with_passwords_and_skips() {
        let blob = encode_password("sftp.example.com", "deploy", "s3cret");
        let ini = format!(
            "[Configuration]\nRandomSeedFile=x\n\n\
             [Sessions\\prod%20sftp]\nHostName=sftp.example.com\nPortNumber=2222\n\
             UserName=deploy\nPassword={blob}\nFSProtocol=0\nAgentFwd=1\n\n\
             [Sessions\\bucket]\nHostName=s3.amazonaws.com\nFSProtocol=7\n\n\
             [Sessions\\Default%20Settings]\nHostName=\n"
        );
        let import = parse(&ini);
        assert_eq!(import.hosts.len(), 1);
        assert_eq!(import.skipped, vec!["bucket".to_string()]);
        let host = &import.hosts[0];
        assert_eq!(host.conn.label, "prod sftp");
        assert_eq!(host.conn.hostname, "sftp.example.com");
        assert_eq!(host.conn.port, 2222);
        assert_eq!(host.conn.username.as_deref(), Some("deploy"));
        assert_eq!(host.conn.protocol, ConnectionProtocol::Ssh);
        assert!(host.conn.agent_forwarding);
        assert_eq!(host.conn.auth_method, AuthMethod::Password);
        assert_eq!(host.password.as_deref(), Some("s3cret"));
    }

    #[test]
    fn reg_export_and_undecodable_password() {
        let reg = "Windows Registry Editor Version 5.00\r\n\r\n\
            [HKEY_CURRENT_USER\\Software\\Martin Prikryl\\WinSCP 2\\Sessions\\edge]\r\n\
            \"HostName\"=\"edge.example.com\"\r\n\
            \"PortNumber\"=dword:00000016\r\n\
            \"UserName\"=\"ops\"\r\n\
            \"Password\"=\"DEADBEEF\"\r\n\
            \"ProxyMethod\"=dword:00000003\r\n\
            \"ProxyHost\"=\"proxy.corp\"\r\n\
            \"ProxyPort\"=dword:00000bb8\r\n";
        let import = parse(reg);
        assert_eq!(import.hosts.len(), 1);
        let host = &import.hosts[0];
        assert_eq!(host.conn.port, 22);
        // Undecodable blob: no password, and the note says why.
        assert_eq!(host.password, None);
        assert!(host.conn.notes.as_deref().unwrap().contains("master password"));
        let proxy = host.conn.proxy.as_ref().expect("proxy");
        assert_eq!(proxy.proxy_type, ProxyType::Http);
        assert_eq!(proxy.host, "proxy.corp");
        assert_eq!(proxy.port, 3000);
    }
}
