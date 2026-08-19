//! SecureCRT importer: one session `.ini` from the `Sessions`
//! directory.
//!
//! Values carry a type prefix: `S:"Hostname"=host` (string),
//! `D:"[SSH2] Port"=00000016` (hex dword), `B:"..."=00000001` (bool).
//! Passwords (`S:"Password V2"=02:...`) are encrypted with a
//! per-install key and never travel; the host imports with a note.
//!
//! One file is one session, so the Import hub's folder mode is the
//! practical entry point (point it at the SecureCRT `Sessions` tree).

use oryxis_core::models::connection::{Connection, ConnectionProtocol};

use super::{DirectHost, DirectImport};

pub(crate) fn parse(text: &str, file_stem: &str) -> Option<DirectImport> {
    // `S:"Key"=value` lines, flattened (SecureCRT session files are
    // one flat block).
    let mut entries: Vec<(String, String)> = Vec::new();
    for raw in text.lines() {
        let line = raw.trim_end_matches('\r').trim();
        let Some(rest) = line
            .strip_prefix("S:")
            .or_else(|| line.strip_prefix("D:"))
            .or_else(|| line.strip_prefix("B:"))
        else {
            continue;
        };
        let Some(rest) = rest.strip_prefix('"') else {
            continue;
        };
        let Some(quote) = rest.find('"') else {
            continue;
        };
        let key = rest[..quote].to_string();
        let Some(value) = rest[quote + 1..].strip_prefix('=') else {
            continue;
        };
        entries.push((key, value.trim().to_string()));
    }
    if entries.is_empty() {
        return None;
    }
    let get = |key: &str| -> Option<String> {
        entries
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(key))
            .map(|(_, v)| v.clone())
            .filter(|v| !v.is_empty())
    };
    // Dwords are hex, zero-padded.
    let get_dword =
        |key: &str| -> Option<u32> { get(key).and_then(|v| u32::from_str_radix(&v, 16).ok()) };

    let protocol_text = get("Protocol Name").unwrap_or_default().to_ascii_uppercase();
    let mut out = DirectImport {
        source_key: "import_securecrt_btn",
        hosts: Vec::new(),
        skipped: Vec::new(),
    };
    let (protocol, default_port, port_key) = match protocol_text.as_str() {
        "SSH2" => (ConnectionProtocol::Ssh, 22, "[SSH2] Port"),
        // SSH1 sessions dial the same transport; the engine only
        // speaks SSH2, which every modern server wants anyway.
        "SSH1" => (ConnectionProtocol::Ssh, 22, "[SSH1] Port"),
        "TELNET" => (ConnectionProtocol::Telnet, 23, "Port"),
        // A bare TCP socket, SecureCRT's own name for it. Its port is
        // per console server, so the session's value is the only one
        // worth taking.
        "RAW" => (ConnectionProtocol::Raw, 0, "Port"),
        _ => {
            out.skipped.push(file_stem.to_string());
            return Some(out);
        }
    };
    let Some(host) = get("Hostname") else {
        out.skipped.push(file_stem.to_string());
        return Some(out);
    };

    let mut conn = Connection::new(file_stem.to_string(), host);
    conn.protocol = protocol;
    conn.port = get_dword(port_key)
        .or_else(|| get_dword("Port"))
        .filter(|p| *p > 0 && *p <= u16::MAX as u32)
        .map(|p| p as u16)
        .unwrap_or(default_port);
    if let Some(user) = get("Username") {
        conn.username = Some(user);
    }
    let mut notes = format!("Imported from SecureCRT (session `{file_stem}`)");
    if get("Password V2").is_some() || get("Password").is_some() {
        notes.push_str("\nSecureCRT encrypts stored passwords; set it manually");
    }
    if let Some(fw) = get("Firewall Name")
        && fw != "None"
    {
        // Proxy/firewall entries live in the global config, not here,
        // so the reference is preserved rather than resolved.
        notes.push_str(&format!("\nSecureCRT firewall: {fw}"));
    }
    conn.notes = Some(notes);
    out.hosts.push(DirectHost { conn, password: None });
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "S:\"Protocol Name\"=SSH2\r\n\
        S:\"Hostname\"=db.example.com\r\n\
        D:\"[SSH2] Port\"=000008ae\r\n\
        S:\"Username\"=oracle\r\n\
        S:\"Password V2\"=02:abcdef\r\n\
        S:\"Firewall Name\"=Session:corp-proxy\r\n\
        B:\"Auth Prompts in Window\"=00000001\r\n";

    #[test]
    fn maps_a_session_with_hex_port() {
        let import = parse(SAMPLE, "db-prod").expect("session parses");
        let c = &import.hosts[0].conn;
        assert_eq!(c.label, "db-prod");
        assert_eq!(c.hostname, "db.example.com");
        assert_eq!(c.port, 0x8ae);
        assert_eq!(c.username.as_deref(), Some("oracle"));
        assert!(import.hosts[0].password.is_none());
        let notes = c.notes.as_deref().unwrap();
        assert!(notes.contains("encrypts"));
        assert!(notes.contains("corp-proxy"));
    }

    #[test]
    fn foreign_protocol_and_non_sessions() {
        let rlogin = "S:\"Protocol Name\"=RLogin\r\nS:\"Hostname\"=h\r\n";
        let import = parse(rlogin, "old").unwrap();
        assert!(import.hosts.is_empty());
        assert_eq!(import.skipped, vec!["old".to_string()]);

        assert!(parse("just text\n", "x").is_none());
    }
}
