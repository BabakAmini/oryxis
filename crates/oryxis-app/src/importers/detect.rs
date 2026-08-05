//! Format auto-detection for the one Import entry (owner call: a
//! single standardized import, the app figures out what the file is).
//!
//! Detection is content-based, never extension-based: an Oryxis
//! export announces itself with its magic, a regedit export with its
//! header line (then the hive paths inside say whose sessions these
//! are - a full HKCU export can carry PuTTY AND WinSCP, which merges
//! into one batch), a portable WinSCP.ini with its `[Sessions\...]`
//! sections, MobaXterm.ini with its `[Bookmarks]`, an mRemoteNG
//! confCons.xml with its namespace, and the per-session formats
//! (Xshell, SecureCRT, FinalShell) plus CSV by shape.
//!
//! `file_stem` is the picked file's name without extension: the
//! per-session formats have no field for the session name, so the
//! file name IS the label there.

use super::{
    csv, finalshell, mobaxterm, putty, regfile, securecrt, winscp, xshell,
    DirectHost, DirectImport,
};

/// What the picked file turned out to be.
pub(crate) enum Detected {
    /// An `.oryxis` portable export: route to the vault-import dialog.
    OryxisExport,
    /// An OpenSSH config: route to the ssh_config flow (it has its own
    /// alias-linking pass). Carries the decoded text.
    SshConfig(String),
    /// A third-party batch, parsed and ready for the shared preview.
    Foreign(DirectImport),
    /// A confCons.xml: parsed lazily by the caller because it may
    /// need the file password (the hub holds the bytes and asks).
    MRemoteNg,
    /// Nothing recognizable.
    Unknown,
}

pub(crate) fn detect(bytes: &[u8], file_stem: &str) -> Detected {
    if oryxis_vault::is_valid_export(bytes) {
        return Detected::OryxisExport;
    }
    let text = regfile::decode_reg_bytes(bytes);
    let trimmed = text.trim_start_matches('\u{feff}').trim_start();

    // mRemoteNG confCons.xml: the namespaced root is unmistakable.
    if text.contains("mremoteng.org") || text.contains("<mrng:Connections") {
        return Detected::MRemoteNg;
    }

    if trimmed.starts_with("Windows Registry Editor")
        || trimmed.starts_with("REGEDIT4")
    {
        // Whose hive(s)? A full-registry export can hold PuTTY, KiTTY
        // and WinSCP at once; merge so the user sees one combined
        // preview instead of picking a side.
        let has_putty = text.contains("\\SimonTatham\\PuTTY\\Sessions\\")
            || text.contains("\\9bis.com\\KiTTY\\Sessions\\");
        let has_winscp = text.contains("\\WinSCP 2\\Sessions\\");
        let mut hosts: Vec<DirectHost> = Vec::new();
        let mut skipped: Vec<String> = Vec::new();
        if has_putty {
            let import = putty::parse_reg(&text);
            hosts.extend(
                import
                    .connections
                    .into_iter()
                    .map(|conn| DirectHost { conn, password: None }),
            );
            skipped.extend(import.skipped);
        }
        if has_winscp {
            let import = winscp::parse(&text);
            hosts.extend(import.hosts);
            skipped.extend(import.skipped);
        }
        let source_key = match (has_putty, has_winscp) {
            (true, false) => "import_putty_btn",
            (false, true) => "import_winscp_btn",
            _ => "import_hub_title",
        };
        return Detected::Foreign(DirectImport {
            source_key,
            hosts,
            skipped,
        });
    }

    // Portable WinSCP.ini: its session sections are unmistakable.
    if text.contains("[Sessions\\") {
        let import = winscp::parse(&text);
        return Detected::Foreign(DirectImport {
            source_key: "import_winscp_btn",
            hosts: import.hosts,
            skipped: import.skipped,
        });
    }

    // MobaXterm.ini bookmarks.
    if text.contains("[Bookmarks")
        && let Some(import) = mobaxterm::parse(&text)
    {
        return Detected::Foreign(import);
    }

    // Per-session files: Xshell / SecureCRT INIs and FinalShell JSON.
    if let Some(import) = xshell::parse(&text, file_stem) {
        return Detected::Foreign(import);
    }
    if let Some(import) = securecrt::parse(&text, file_stem) {
        return Detected::Foreign(import);
    }
    if trimmed.starts_with('{')
        && let Some(import) = finalshell::parse(&text, file_stem)
    {
        return Detected::Foreign(import);
    }

    // OpenSSH config heuristic: at least one `Host`/`Match`-family
    // block header outside comments. `Host` alone would also match a
    // hosts-file-ish text, so require a known directive too when the
    // file has more than one line.
    let mut has_host_block = false;
    let mut has_directive = false;
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let lower = line.to_ascii_lowercase();
        if lower.starts_with("host ") || lower.starts_with("host\t") {
            has_host_block = true;
        } else if [
            "hostname", "port ", "port=", "user ", "user=", "identityfile",
            "proxyjump", "proxycommand", "forwardagent",
        ]
        .iter()
        .any(|d| lower.starts_with(d))
        {
            has_directive = true;
        }
    }
    if has_host_block && (has_directive || text.lines().count() <= 2) {
        return Detected::SshConfig(text);
    }

    // CSV last: its shape (a header row that maps to host fields) is
    // the loosest signal, so every stricter format gets first refusal.
    if let Some(import) = csv::parse(&text) {
        return Detected::Foreign(import);
    }

    Detected::Unknown
}

/// Import every recognizable session file under a directory, one
/// merged batch (the per-session formats - Xshell, SecureCRT,
/// FinalShell - are one file per host, so a folder IS the unit the
/// user thinks in). Recurses, bounded so a mis-picked directory can
/// never walk a whole disk.
pub(crate) fn scan_folder(root: &std::path::Path) -> DirectImport {
    /// Files a session tree could plausibly hold; a huge file is
    /// something else entirely.
    const MAX_FILE_BYTES: u64 = 4 * 1024 * 1024;
    const MAX_FILES: usize = 2_000;
    const MAX_DEPTH: usize = 6;

    let mut out = DirectImport {
        source_key: "import_hub_title",
        hosts: Vec::new(),
        skipped: Vec::new(),
    };
    let mut seen = 0usize;
    let mut stack: Vec<(std::path::PathBuf, usize)> = vec![(root.to_path_buf(), 0)];
    while let Some((dir, depth)) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            if seen >= MAX_FILES {
                return out;
            }
            let path = entry.path();
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            if meta.is_dir() {
                if depth < MAX_DEPTH {
                    stack.push((path, depth + 1));
                }
                continue;
            }
            if meta.len() > MAX_FILE_BYTES {
                continue;
            }
            seen += 1;
            let Ok(bytes) = std::fs::read(&path) else {
                continue;
            };
            let stem = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("session");
            // Only direct batches merge: a vault export or an ssh
            // config inside a session folder is a different flow and
            // is left for the file picker.
            if let Detected::Foreign(import) = detect(&bytes, stem) {
                out.hosts.extend(import.hosts);
                out.skipped.extend(import.skipped);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_each_family() {
        // regedit header + PuTTY hive.
        let putty = "Windows Registry Editor Version 5.00\r\n\r\n[HKEY_CURRENT_USER\\Software\\SimonTatham\\PuTTY\\Sessions\\a]\r\n\"HostName\"=\"h\"\r\n\"Protocol\"=\"ssh\"\r\n";
        assert!(matches!(
            detect(putty.as_bytes(), "f"),
            Detected::Foreign(d) if d.source_key == "import_putty_btn" && d.hosts.len() == 1
        ));

        // Portable WinSCP.ini.
        let ini = "[Sessions\\site]\nHostName=h\nUserName=u\n";
        assert!(matches!(
            detect(ini.as_bytes(), "f"),
            Detected::Foreign(d) if d.source_key == "import_winscp_btn" && d.hosts.len() == 1
        ));

        // A combined HKCU export merges both hives into one batch.
        let both = "Windows Registry Editor Version 5.00\r\n\r\n\
            [HKEY_CURRENT_USER\\Software\\SimonTatham\\PuTTY\\Sessions\\a]\r\n\"HostName\"=\"h\"\r\n\"Protocol\"=\"ssh\"\r\n\r\n\
            [HKEY_CURRENT_USER\\Software\\Martin Prikryl\\WinSCP 2\\Sessions\\b]\r\n\"HostName\"=\"w\"\r\n";
        assert!(matches!(
            detect(both.as_bytes(), "f"),
            Detected::Foreign(d) if d.source_key == "import_hub_title" && d.hosts.len() == 2
        ));

        // mRemoteNG defers to the password-aware path.
        let mrng = "<?xml version=\"1.0\"?>\n<mrng:Connections xmlns:mrng=\"http://mremoteng.org\" Name=\"Connections\"></mrng:Connections>";
        assert!(matches!(detect(mrng.as_bytes(), "f"), Detected::MRemoteNg));

        // MobaXterm bookmarks.
        let moba = "[Bookmarks]\nSubRep=\nweb= #109#0%h%22%u%\n";
        assert!(matches!(
            detect(moba.as_bytes(), "f"),
            Detected::Foreign(d) if d.source_key == "import_mobaxterm_btn"
        ));

        // Xshell session, SecureCRT session, FinalShell connection.
        let xsh = "[CONNECTION]\nProtocol=SSH\nHost=h\nPort=22\n";
        assert!(matches!(
            detect(xsh.as_bytes(), "box"),
            Detected::Foreign(d) if d.source_key == "import_xshell_btn"
        ));
        let crt = "S:\"Protocol Name\"=SSH2\r\nS:\"Hostname\"=h\r\n";
        assert!(matches!(
            detect(crt.as_bytes(), "box"),
            Detected::Foreign(d) if d.source_key == "import_securecrt_btn"
        ));
        let fs = r#"{"name":"x","host":"h","port":22}"#;
        assert!(matches!(
            detect(fs.as_bytes(), "box"),
            Detected::Foreign(d) if d.source_key == "import_finalshell_btn"
        ));

        // OpenSSH config.
        let ssh = "Host web\n  HostName web.example.com\n  User deploy\n";
        assert!(matches!(detect(ssh.as_bytes(), "f"), Detected::SshConfig(_)));

        // CSV of hosts.
        let csv = "name,host,user\nweb,web.corp,deploy\n";
        assert!(matches!(
            detect(csv.as_bytes(), "f"),
            Detected::Foreign(d) if d.source_key == "import_csv_btn"
        ));

        // Garbage stays unknown instead of guessing.
        assert!(matches!(detect(b"not a config at all", "f"), Detected::Unknown));
        assert!(matches!(detect(&[0u8, 159, 146, 150], "f"), Detected::Unknown));
    }
}
