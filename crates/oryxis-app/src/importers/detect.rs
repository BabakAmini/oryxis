//! Format auto-detection for the one Import entry (owner call: a
//! single standardized import, the app figures out what the file is).
//!
//! Detection is content-based, never extension-based: an Oryxis
//! export announces itself with its magic, a regedit export with its
//! header line (then the hive paths inside say whose sessions these
//! are - a full HKCU export can carry PuTTY AND WinSCP, which merges
//! into one batch), a portable WinSCP.ini with its `[Sessions\...]`
//! sections, and an OpenSSH config with its `Host` directives.

use super::{putty, regfile, winscp, DirectHost, DirectImport};

/// What the picked file turned out to be.
pub(crate) enum Detected {
    /// An `.oryxis` portable export: route to the vault-import dialog.
    OryxisExport,
    /// An OpenSSH config: route to the ssh_config flow (it has its own
    /// alias-linking pass). Carries the decoded text.
    SshConfig(String),
    /// A third-party batch, parsed and ready for the shared preview.
    Foreign(DirectImport),
    /// Nothing recognizable.
    Unknown,
}

pub(crate) fn detect(bytes: &[u8]) -> Detected {
    if oryxis_vault::is_valid_export(bytes) {
        return Detected::OryxisExport;
    }
    let text = regfile::decode_reg_bytes(bytes);
    let trimmed = text.trim_start_matches('\u{feff}').trim_start();

    if trimmed.starts_with("Windows Registry Editor")
        || trimmed.starts_with("REGEDIT4")
    {
        // Whose hive(s)? A full-registry export can hold both; merge
        // so the user sees one combined preview instead of picking a
        // side.
        let has_putty = text.contains("\\SimonTatham\\PuTTY\\Sessions\\");
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

    Detected::Unknown
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_each_family() {
        // regedit header + PuTTY hive.
        let putty = "Windows Registry Editor Version 5.00\r\n\r\n[HKEY_CURRENT_USER\\Software\\SimonTatham\\PuTTY\\Sessions\\a]\r\n\"HostName\"=\"h\"\r\n\"Protocol\"=\"ssh\"\r\n";
        assert!(matches!(
            detect(putty.as_bytes()),
            Detected::Foreign(d) if d.source_key == "import_putty_btn" && d.hosts.len() == 1
        ));

        // Portable WinSCP.ini.
        let ini = "[Sessions\\site]\nHostName=h\nUserName=u\n";
        assert!(matches!(
            detect(ini.as_bytes()),
            Detected::Foreign(d) if d.source_key == "import_winscp_btn" && d.hosts.len() == 1
        ));

        // A combined HKCU export merges both hives into one batch.
        let both = "Windows Registry Editor Version 5.00\r\n\r\n\
            [HKEY_CURRENT_USER\\Software\\SimonTatham\\PuTTY\\Sessions\\a]\r\n\"HostName\"=\"h\"\r\n\"Protocol\"=\"ssh\"\r\n\r\n\
            [HKEY_CURRENT_USER\\Software\\Martin Prikryl\\WinSCP 2\\Sessions\\b]\r\n\"HostName\"=\"w\"\r\n";
        assert!(matches!(
            detect(both.as_bytes()),
            Detected::Foreign(d) if d.source_key == "import_hub_title" && d.hosts.len() == 2
        ));

        // OpenSSH config.
        let ssh = "Host web\n  HostName web.example.com\n  User deploy\n";
        assert!(matches!(detect(ssh.as_bytes()), Detected::SshConfig(_)));

        // Garbage stays unknown instead of guessing.
        assert!(matches!(detect(b"not a config at all"), Detected::Unknown));
        assert!(matches!(detect(&[0u8, 159, 146, 150]), Detected::Unknown));
    }
}
