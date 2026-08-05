//! MobaXterm importer: parses `MobaXterm.ini`'s bookmark sections.
//!
//! Sessions live as one INI entry per bookmark:
//! `name= #109#0%host%port%user%...`, where the first number is the
//! session type and the `%`-separated tail its parameters. Folders
//! are separate `[Bookmarks_N]` sections carrying a `SubRep=` name,
//! which becomes a note on each host inside it.
//!
//! Only type 109 (SSH) is mapped: it is the one MobaXterm session
//! type whose layout is unambiguous in the wild, and inventing a
//! mapping for the others would silently produce hosts that dial the
//! wrong thing. Every other bookmark is reported by name instead.
//!
//! Passwords never travel: MobaXterm keeps them in its own encrypted
//! credential store, not in the ini.

use oryxis_core::models::connection::{Connection, ConnectionProtocol};

use super::{DirectHost, DirectImport};

/// MobaXterm's SSH session type.
const TYPE_SSH: &str = "109";

pub(crate) fn parse(text: &str) -> Option<DirectImport> {
    let mut out = DirectImport {
        source_key: "import_mobaxterm_btn",
        hosts: Vec::new(),
        skipped: Vec::new(),
    };
    let mut in_bookmarks = false;
    let mut folder = String::new();
    let mut saw_section = false;

    for raw in text.lines() {
        let line = raw.trim_end_matches('\r').trim();
        if line.is_empty() || line.starts_with(';') {
            continue;
        }
        if let Some(section) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            // `[Bookmarks]`, `[Bookmarks_1]`, ... each open a folder;
            // anything else (Colors, Misc, ...) closes the scope.
            in_bookmarks = section.starts_with("Bookmarks");
            saw_section |= in_bookmarks;
            folder.clear();
            continue;
        }
        if !in_bookmarks {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let (key, value) = (key.trim(), value.trim());
        if key == "SubRep" {
            folder = value.to_string();
            continue;
        }
        // Bookmark entries are the only ones whose value starts with
        // the `#type#` marker; ImgNum and friends never do.
        let Some(rest) = value.strip_prefix('#') else {
            continue;
        };
        let Some((session_type, params)) = rest.split_once('#') else {
            continue;
        };
        if session_type != TYPE_SSH {
            out.skipped.push(key.to_string());
            continue;
        }
        // `0%host%port%user%...`: the leading field is the subtype.
        let fields: Vec<&str> = params.split('%').collect();
        let host = fields.get(1).copied().unwrap_or_default().trim();
        if host.is_empty() {
            out.skipped.push(key.to_string());
            continue;
        }
        let mut conn = Connection::new(key.to_string(), host.to_string());
        conn.protocol = ConnectionProtocol::Ssh;
        conn.port = fields
            .get(2)
            .and_then(|p| p.trim().parse().ok())
            .filter(|p| *p > 0)
            .unwrap_or(22);
        let user = fields.get(3).copied().unwrap_or_default().trim();
        if !user.is_empty() {
            conn.username = Some(user.to_string());
        }
        let mut notes = format!("Imported from MobaXterm (bookmark `{key}`)");
        if !folder.is_empty() {
            notes.push_str(&format!("\nMobaXterm folder: {folder}"));
        }
        conn.notes = Some(notes);
        out.hosts.push(DirectHost { conn, password: None });
    }

    (saw_section && (!out.hosts.is_empty() || !out.skipped.is_empty())).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "[Misc]\n\
        Something=1\n\
        \n\
        [Bookmarks]\n\
        SubRep=\n\
        ImgNum=42\n\
        web1= #109#0%web1.example.com%2222%deploy%%-1%-1%%%22%%0%\n\
        \n\
        [Bookmarks_1]\n\
        SubRep=Prod\n\
        ImgNum=41\n\
        db1= #109#0%db1.example.com%22%root%%-1%\n\
        winbox= #98#1%10.0.0.5%3389%admin%\n";

    #[test]
    fn maps_ssh_bookmarks_and_reports_the_rest() {
        let import = parse(SAMPLE).expect("bookmarks parse");
        assert_eq!(import.hosts.len(), 2);
        assert_eq!(import.skipped, vec!["winbox".to_string()]);

        let web = &import.hosts[0].conn;
        assert_eq!(web.label, "web1");
        assert_eq!(web.hostname, "web1.example.com");
        assert_eq!(web.port, 2222);
        assert_eq!(web.username.as_deref(), Some("deploy"));

        // The folder of the second section rides along as a note.
        let db = &import.hosts[1].conn;
        assert_eq!(db.port, 22);
        assert!(db.notes.as_deref().unwrap().contains("Prod"));
    }

    #[test]
    fn an_ini_without_bookmarks_is_not_ours() {
        assert!(parse("[Colors]\nBackground=0,0,0\n").is_none());
        assert!(parse("").is_none());
    }
}
