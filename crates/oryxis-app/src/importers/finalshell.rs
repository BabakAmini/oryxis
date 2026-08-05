//! FinalShell importer: the per-connection JSON files FinalShell
//! keeps under `~/.finalshell/conn/` (one object per connection,
//! plus `*_folder.json` entries for the tree).
//!
//! Passwords are stored under FinalShell's own obfuscation and are
//! deliberately not decoded: unlike WinSCP's documented scheme, the
//! FinalShell one has changed between releases, and a wrong guess
//! would import a corrupted secret. The host imports with a note.
//!
//! One file is one connection, so the Import hub's folder mode is
//! the practical entry point.

use oryxis_core::models::connection::{Connection, ConnectionProtocol};

use super::{DirectHost, DirectImport};

pub(crate) fn parse(text: &str, file_stem: &str) -> Option<DirectImport> {
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    let obj = value.as_object()?;
    // Folder entries carry no endpoint; they are not an error, just
    // not hosts.
    let host = obj
        .get("host")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|h| !h.is_empty())?;

    let name = obj
        .get("name")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .unwrap_or(file_stem);

    let mut out = DirectImport {
        source_key: "import_finalshell_btn",
        hosts: Vec::new(),
        skipped: Vec::new(),
    };
    // FinalShell's numeric protocol tag: 1 = SSH, 2 = telnet in the
    // builds that carry it; a missing tag means SSH (the default a
    // "new connection" creates).
    let protocol = match obj.get("protocol").and_then(|v| v.as_i64()) {
        None | Some(1) => ConnectionProtocol::Ssh,
        Some(2) => ConnectionProtocol::Telnet,
        Some(_) => {
            out.skipped.push(name.to_string());
            return Some(out);
        }
    };

    let mut conn = Connection::new(name.to_string(), host.to_string());
    conn.protocol = protocol;
    conn.port = obj
        .get("port")
        .and_then(|v| v.as_u64())
        .filter(|p| *p > 0 && *p <= u16::MAX as u64)
        .map(|p| p as u16)
        .unwrap_or(match protocol {
            ConnectionProtocol::Telnet => 23,
            _ => 22,
        });
    if let Some(user) = obj
        .get("user_name")
        .or_else(|| obj.get("userName"))
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|u| !u.is_empty())
    {
        conn.username = Some(user.to_string());
    }
    let mut notes = format!("Imported from FinalShell (connection `{name}`)");
    if obj
        .get("password")
        .and_then(|v| v.as_str())
        .is_some_and(|p| !p.is_empty())
    {
        notes.push_str("\nFinalShell obfuscates stored passwords; set it manually");
    }
    conn.notes = Some(notes);
    out.hosts.push(DirectHost { conn, password: None });
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_a_connection_json() {
        let json = r#"{"id":"abc","name":"prod-app","host":"10.1.2.3","port":2200,
            "user_name":"ubuntu","password":"AABBCC","protocol":1}"#;
        let import = parse(json, "abc").expect("connection parses");
        let c = &import.hosts[0].conn;
        assert_eq!(c.label, "prod-app");
        assert_eq!(c.hostname, "10.1.2.3");
        assert_eq!(c.port, 2200);
        assert_eq!(c.username.as_deref(), Some("ubuntu"));
        assert!(import.hosts[0].password.is_none());
        assert!(c.notes.as_deref().unwrap().contains("obfuscates"));
    }

    #[test]
    fn defaults_and_non_hosts() {
        // No port / no protocol: SSH on 22.
        let json = r#"{"name":"x","host":"h"}"#;
        let import = parse(json, "f").unwrap();
        assert_eq!(import.hosts[0].conn.port, 22);
        assert_eq!(import.hosts[0].conn.protocol, ConnectionProtocol::Ssh);

        // Folder entries (no host) and non-JSON are simply not ours.
        assert!(parse(r#"{"name":"Prod","folder":true}"#, "f").is_none());
        assert!(parse("nope", "f").is_none());
    }
}
