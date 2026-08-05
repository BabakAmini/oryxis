//! CSV / Termius importer: one header-mapped table reader serving
//! both the generic "spreadsheet of hosts" case and Termius' own
//! export (which is a CSV of hosts without secrets).
//!
//! Columns are matched by header NAME, case- and separator-
//! insensitive (`Host Name`, `hostname`, `host_name` all hit the
//! same slot), so a hand-made sheet and a vendor export read the
//! same way and column ORDER never matters. A row needs a hostname
//! (or a label that doubles as one); everything else is optional.
//!
//! Secrets: a `password` column is honored because a user who typed
//! their passwords into a spreadsheet clearly wants them imported,
//! and they land in the vault's encrypted column. Termius' own export
//! carries none, so that path simply never populates it.

use oryxis_core::models::connection::{AuthMethod, Connection, ConnectionProtocol};

use super::{DirectHost, DirectImport};

/// Which connection field a header maps to. Anything unrecognized is
/// ignored rather than guessed.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Column {
    Label,
    Hostname,
    Port,
    Username,
    Password,
    Group,
    Tags,
    Notes,
    Protocol,
    Ignored,
}

/// Header aliases, in the normalized form (lowercase, only
/// alphanumerics kept). Termius' export headers are included next to
/// the obvious generic ones.
fn column_of(header: &str) -> Column {
    let key: String = header
        .chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect();
    match key.as_str() {
        "label" | "name" | "alias" | "title" | "sessionname" => Column::Label,
        "hostname" | "host" | "address" | "ip" | "hostaddress" => Column::Hostname,
        "port" | "sshport" => Column::Port,
        "username" | "user" | "login" | "sshusername" => Column::Username,
        "password" | "sshpassword" => Column::Password,
        "group" | "folder" | "groupname" | "path" => Column::Group,
        "tags" | "labels" => Column::Tags,
        "notes" | "note" | "comment" | "description" => Column::Notes,
        "protocol" | "type" => Column::Protocol,
        _ => Column::Ignored,
    }
}

/// Parse a CSV table. Returns `None` when the text has no header row
/// that maps to anything useful, which is what keeps detection from
/// claiming arbitrary text files.
pub(crate) fn parse(text: &str) -> Option<DirectImport> {
    let mut rows = read_rows(text);
    if rows.is_empty() {
        return None;
    }
    let header = rows.remove(0);
    let columns: Vec<Column> = header.iter().map(|h| column_of(h)).collect();
    // A usable table needs at least somewhere to read an address
    // from: a hostname column, or a label column we can fall back on.
    if !columns
        .iter()
        .any(|c| matches!(c, Column::Hostname | Column::Label))
    {
        return None;
    }

    let mut out = DirectImport {
        source_key: "import_csv_btn",
        hosts: Vec::new(),
        skipped: Vec::new(),
    };
    for (line_no, row) in rows.iter().enumerate() {
        if row.iter().all(|cell| cell.trim().is_empty()) {
            continue;
        }
        let get = |want: Column| -> String {
            columns
                .iter()
                .position(|c| *c == want)
                .and_then(|i| row.get(i))
                .map(|s| s.trim().to_string())
                .unwrap_or_default()
        };
        let label = get(Column::Label);
        let hostname = get(Column::Hostname);
        // Either column can stand in for the other, which is what
        // makes a two-column sheet (name,address) and a one-column
        // list of hostnames both work.
        let (label, hostname) = match (label.is_empty(), hostname.is_empty()) {
            (false, false) => (label, hostname),
            (true, false) => (hostname.clone(), hostname),
            (false, true) => (label.clone(), label),
            (true, true) => {
                // Row with nothing to connect to: name it by line so
                // the user can find it in their file.
                out.skipped.push(format!("line {}", line_no + 2));
                continue;
            }
        };

        let protocol_text = get(Column::Protocol).to_ascii_lowercase();
        let (protocol, default_port) = match protocol_text.as_str() {
            "" | "ssh" | "sftp" | "scp" => (ConnectionProtocol::Ssh, 22),
            "telnet" => (ConnectionProtocol::Telnet, 23),
            _ => {
                out.skipped.push(label);
                continue;
            }
        };

        let mut conn = Connection::new(label, hostname);
        conn.protocol = protocol;
        conn.port = get(Column::Port).parse().unwrap_or(default_port);
        let username = get(Column::Username);
        if !username.is_empty() {
            conn.username = Some(username);
        }
        let tags = get(Column::Tags);
        if !tags.is_empty() {
            conn.tags = tags
                .split([',', ';'])
                .map(|t| t.trim().to_string())
                .filter(|t| !t.is_empty())
                .collect();
        }
        // The group column is preserved as a note rather than
        // silently creating folders: group creation is a decision,
        // and the preview only promises hosts.
        let mut notes = get(Column::Notes);
        let group = get(Column::Group);
        if !group.is_empty() {
            if !notes.is_empty() {
                notes.push('\n');
            }
            notes.push_str(&format!("Group: {group}"));
        }
        conn.notes = (!notes.is_empty()).then_some(notes);

        let password = Some(get(Column::Password)).filter(|p| !p.is_empty());
        if password.is_some() {
            conn.auth_method = AuthMethod::Password;
        }
        out.hosts.push(DirectHost { conn, password });
    }
    (!out.hosts.is_empty() || !out.skipped.is_empty()).then_some(out)
}

/// Minimal RFC 4180 reader: quoted fields, doubled quotes inside
/// them, embedded newlines, and a comma-or-semicolon delimiter
/// sniffed from the header (European exports use `;`).
fn read_rows(text: &str) -> Vec<Vec<String>> {
    let delimiter = sniff_delimiter(text);
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut row: Vec<String> = Vec::new();
    let mut field = String::new();
    let mut in_quotes = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if in_quotes {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    chars.next();
                    field.push('"');
                } else {
                    in_quotes = false;
                }
            } else {
                field.push(c);
            }
            continue;
        }
        match c {
            '"' if field.is_empty() => in_quotes = true,
            c if c == delimiter => row.push(std::mem::take(&mut field)),
            '\r' => {}
            '\n' => {
                row.push(std::mem::take(&mut field));
                rows.push(std::mem::take(&mut row));
            }
            c => field.push(c),
        }
    }
    if !field.is_empty() || !row.is_empty() {
        row.push(field);
        rows.push(row);
    }
    // A trailing newline leaves an empty final row.
    rows.retain(|r| !(r.len() == 1 && r[0].trim().is_empty()));
    rows
}

/// Delimiter of the first line: whichever of `,` / `;` / tab appears
/// more often outside quotes.
fn sniff_delimiter(text: &str) -> char {
    let first = text.lines().next().unwrap_or_default();
    let count = |d: char| first.matches(d).count();
    let (mut best, mut best_n) = (',', count(','));
    for d in [';', '\t'] {
        if count(d) > best_n {
            best = d;
            best_n = count(d);
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_mapping_is_order_and_style_insensitive() {
        let csv = "Session Name,SSH Username,Host Name,SSH Port\n\
                   web,deploy,web.example.com,2222\n";
        let import = parse(csv).expect("table parses");
        assert_eq!(import.hosts.len(), 1);
        let c = &import.hosts[0].conn;
        assert_eq!(c.label, "web");
        assert_eq!(c.hostname, "web.example.com");
        assert_eq!(c.port, 2222);
        assert_eq!(c.username.as_deref(), Some("deploy"));
    }

    #[test]
    fn quotes_semicolons_tags_and_passwords() {
        // European-style semicolons, a quoted field with a comma and
        // an escaped quote, tags, and a password column.
        let csv = "name;host;tags;notes;password\n\
                   db;db1.corp;\"prod,db\";\"say \"\"hi\"\", then wait\";hunter2\n";
        let import = parse(csv).expect("table parses");
        let host = &import.hosts[0];
        assert_eq!(host.conn.tags, vec!["prod".to_string(), "db".to_string()]);
        assert_eq!(
            host.conn.notes.as_deref(),
            Some("say \"hi\", then wait")
        );
        assert_eq!(host.password.as_deref(), Some("hunter2"));
        assert_eq!(host.conn.auth_method, AuthMethod::Password);
    }

    #[test]
    fn one_column_of_hostnames_still_works() {
        let csv = "hostname\nweb1.example.com\nweb2.example.com\n";
        let import = parse(csv).expect("table parses");
        assert_eq!(import.hosts.len(), 2);
        // Label falls back to the address.
        assert_eq!(import.hosts[0].conn.label, "web1.example.com");
    }

    #[test]
    fn rows_without_an_address_and_foreign_protocols_are_reported() {
        let csv = "name,host,protocol\n\
                   ok,h1,ssh\n\
                   ,,\n\
                   rdp-box,h2,rdp\n\
                   ,,ssh\n";
        let import = parse(csv).expect("table parses");
        assert_eq!(import.hosts.len(), 1);
        // The blank-but-present row is named by its file line, the
        // RDP row by its label.
        assert_eq!(
            import.skipped,
            vec!["rdp-box".to_string(), "line 5".to_string()]
        );
    }

    #[test]
    fn group_column_lands_in_notes() {
        let csv = "name,host,folder\napp,a.corp,Prod/Web\n";
        let import = parse(csv).unwrap();
        assert!(import.hosts[0]
            .conn
            .notes
            .as_deref()
            .unwrap()
            .contains("Prod/Web"));
    }

    #[test]
    fn a_table_with_no_usable_header_is_not_a_host_list() {
        assert!(parse("total,average\n1,2\n").is_none());
        assert!(parse("").is_none());
    }
}
