//! Xshell / Xftp importer: one `.xsh` (or `.xfp`) session file.
//!
//! The file is an INI whose `[CONNECTION]` section carries the
//! endpoint and `[CONNECTION:AUTHENTICATION]` the user name. Xshell
//! encrypts stored passwords with a per-install key, so passwords
//! never travel; the host imports with a note instead.
//!
//! One file is one session by design, which is why the Import hub's
//! folder mode matters here: pointing it at the Xshell `Sessions`
//! directory brings the whole tree in at once.

use oryxis_core::models::connection::{Connection, ConnectionProtocol};

use super::{DirectHost, DirectImport};

pub(crate) fn parse(text: &str, file_stem: &str) -> Option<DirectImport> {
    let sections = super::ini::sections(text);
    let get = |section: &str, key: &str| -> Option<String> {
        sections
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(section))
            .and_then(|(_, entries)| {
                entries
                    .iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case(key))
                    .map(|(_, v)| v.trim().to_string())
            })
            .filter(|v| !v.is_empty())
    };

    // A session file always has one of these two blocks; without
    // either it is somebody else's INI.
    if !sections.iter().any(|(name, _)| {
        name.eq_ignore_ascii_case("CONNECTION")
            || name.eq_ignore_ascii_case("SessionInfo")
    }) {
        return None;
    }

    let protocol_text = get("CONNECTION", "Protocol")
        .or_else(|| get("SessionInfo", "Protocol"))
        .unwrap_or_default()
        .to_ascii_uppercase();
    let name = get("SessionInfo", "Description")
        .filter(|d| !d.is_empty())
        .unwrap_or_else(|| file_stem.to_string());

    let mut out = DirectImport {
        source_key: "import_xshell_btn",
        hosts: Vec::new(),
        skipped: Vec::new(),
    };
    let (protocol, default_port) = match protocol_text.as_str() {
        // SFTP sessions (.xfp) are SSH transports too.
        "SSH" | "SFTP" | "" => (ConnectionProtocol::Ssh, 22),
        "TELNET" => (ConnectionProtocol::Telnet, 23),
        "SERIAL" => {
            // The port path lives elsewhere per Xshell version; not
            // worth guessing, so report rather than mis-map.
            out.skipped.push(name);
            return Some(out);
        }
        _ => {
            out.skipped.push(name);
            return Some(out);
        }
    };

    let Some(host) = get("CONNECTION", "Host").or_else(|| get("SessionInfo", "Host"))
    else {
        out.skipped.push(name);
        return Some(out);
    };

    let mut conn = Connection::new(name.clone(), host);
    conn.protocol = protocol;
    conn.port = get("CONNECTION", "Port")
        .or_else(|| get("SessionInfo", "Port"))
        .and_then(|p| p.parse().ok())
        .unwrap_or(default_port);
    if let Some(user) = get("CONNECTION:AUTHENTICATION", "UserName")
        .or_else(|| get("CONNECTION", "UserName"))
    {
        conn.username = Some(user);
    }
    let mut notes = format!("Imported from Xshell (session `{name}`)");
    if get("CONNECTION:AUTHENTICATION", "Password").is_some() {
        notes.push_str("\nXshell encrypts stored passwords; set it manually");
    }
    conn.notes = Some(notes);
    out.hosts.push(DirectHost { conn, password: None });
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "[SessionInfo]\r\n\
        Version=5.2\r\n\
        Description=Prod web\r\n\
        \r\n\
        [CONNECTION]\r\n\
        Protocol=SSH\r\n\
        Host=web.example.com\r\n\
        Port=2222\r\n\
        \r\n\
        [CONNECTION:AUTHENTICATION]\r\n\
        UserName=deploy\r\n\
        Password=abcdef0123\r\n";

    #[test]
    fn maps_a_session_file() {
        let import = parse(SAMPLE, "web").expect("session parses");
        let c = &import.hosts[0].conn;
        // The description wins over the file name when present.
        assert_eq!(c.label, "Prod web");
        assert_eq!(c.hostname, "web.example.com");
        assert_eq!(c.port, 2222);
        assert_eq!(c.username.as_deref(), Some("deploy"));
        assert_eq!(c.protocol, ConnectionProtocol::Ssh);
        // The encrypted password is acknowledged, never guessed.
        assert!(import.hosts[0].password.is_none());
        assert!(c.notes.as_deref().unwrap().contains("encrypts"));
    }

    #[test]
    fn file_name_is_the_fallback_label() {
        let text = "[CONNECTION]\nProtocol=SSH\nHost=h\n";
        let import = parse(text, "my-box").unwrap();
        assert_eq!(import.hosts[0].conn.label, "my-box");
        assert_eq!(import.hosts[0].conn.port, 22);
    }

    #[test]
    fn foreign_protocols_report_instead_of_mapping() {
        let text = "[SessionInfo]\nDescription=ftp\n[CONNECTION]\nProtocol=FTP\nHost=h\n";
        let import = parse(text, "x").unwrap();
        assert!(import.hosts.is_empty());
        assert_eq!(import.skipped, vec!["ftp".to_string()]);
    }

    #[test]
    fn other_inis_are_not_sessions() {
        assert!(parse("[Colors]\nBackground=0\n", "x").is_none());
    }
}
