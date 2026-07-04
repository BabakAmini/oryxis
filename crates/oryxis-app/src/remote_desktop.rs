//! Resolving which OS-native client to spawn for an RDP/VNC-over-SSH
//! launch, and doing the spawn.
//!
//! The RESOLUTION (which program + args for this OS / kind, given what's
//! installed) is a pure function so it can be unit-tested per platform
//! without the clients present. The SPAWN is a thin, fire-and-forget
//! leaf: the launched client is detached (many clients, `open rdp://`,
//! single-instance Remmina, return immediately), so the SSH `-L` tunnel
//! is NOT tied to the client's process lifetime, it lives as a managed
//! forward until the user stops it. The spawn path has no automated
//! coverage (no headless RDP client exists); it needs manual QA.

use oryxis_core::models::remote_desktop::RemoteDesktopKind;

/// The address the client connects to: always the local end of the
/// SSH tunnel.
const LOCAL: &str = "127.0.0.1";

/// A resolved client invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LaunchCommand {
    pub program: String,
    pub args: Vec<String>,
}

/// No suitable client was found; carries the binaries we looked for so
/// the UI can tell the user what to install.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NoClient {
    pub looked_for: Vec<String>,
}

/// Ordered client candidates for (`os`, `kind`), most-preferred first.
/// `os` is `std::env::consts::OS`; injected so the table is testable on
/// any host. URL-scheme launchers (`open`, `mstsc`) are effectively
/// always present, so they're a single candidate; Linux lists several
/// real viewers and the first installed one wins.
fn candidates(kind: RemoteDesktopKind, os: &str, port: u16, username: Option<&str>) -> Vec<LaunchCommand> {
    let endpoint = format!("{LOCAL}:{port}");
    let cmd = |program: &str, args: Vec<String>| LaunchCommand {
        program: program.to_string(),
        args,
    };
    match (os, kind) {
        // Windows: built-in mstsc for RDP; no built-in VNC, try common
        // third-party viewers.
        ("windows", RemoteDesktopKind::Rdp) => {
            vec![cmd("mstsc", vec![format!("/v:{endpoint}")])]
        }
        ("windows", RemoteDesktopKind::Vnc) => vec![
            cmd("vncviewer", vec![endpoint.clone()]),
            cmd("tvnviewer", vec![endpoint.clone()]),
        ],
        // macOS: the URL schemes are handled by Microsoft Remote Desktop
        // (rdp://) and built-in Screen Sharing (vnc://) via `open`.
        ("macos", RemoteDesktopKind::Rdp) => {
            vec![cmd("open", vec![format!("rdp://{endpoint}")])]
        }
        ("macos", RemoteDesktopKind::Vnc) => {
            vec![cmd("open", vec![format!("vnc://{endpoint}")])]
        }
        // Linux / other unix: real viewers, first installed wins.
        (_, RemoteDesktopKind::Rdp) => {
            let mut freerdp = vec![format!("/v:{endpoint}")];
            if let Some(u) = username.filter(|u| !u.trim().is_empty()) {
                freerdp.push(format!("/u:{u}"));
            }
            vec![
                cmd("xfreerdp", freerdp),
                cmd("remmina", vec!["-c".into(), format!("rdp://{endpoint}")]),
            ]
        }
        (_, RemoteDesktopKind::Vnc) => vec![
            cmd("vncviewer", vec![endpoint.clone()]),
            cmd("remmina", vec!["-c".into(), format!("vnc://{endpoint}")]),
            cmd("vinagre", vec![endpoint.clone()]),
            cmd("xtightvncviewer", vec![endpoint.clone()]),
        ],
    }
}

/// Pick the first candidate whose program `is_available`. Returns the
/// full looked-for list on failure so the UI can name what to install.
pub(crate) fn resolve_command(
    kind: RemoteDesktopKind,
    os: &str,
    port: u16,
    username: Option<&str>,
    is_available: &dyn Fn(&str) -> bool,
) -> Result<LaunchCommand, NoClient> {
    let cands = candidates(kind, os, port, username);
    for c in &cands {
        if is_available(&c.program) {
            return Ok(c.clone());
        }
    }
    Err(NoClient {
        looked_for: cands.into_iter().map(|c| c.program).collect(),
    })
}

/// Whether `program` can be executed (found on PATH). Cheap: a
/// `Command::new(program)` spawn probe would run it; instead we scan
/// PATH entries, which is what a shell does and never executes anything.
pub(crate) fn program_on_path(program: &str) -> bool {
    // Windows binaries usually carry an extension; try the common ones.
    #[cfg(windows)]
    let names: Vec<String> = vec![
        program.to_string(),
        format!("{program}.exe"),
        format!("{program}.com"),
    ];
    #[cfg(not(windows))]
    let names: Vec<String> = vec![program.to_string()];

    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|dir| {
        names.iter().any(|n| {
            let candidate = dir.join(n);
            candidate.is_file()
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // Everything installed.
    fn all(_: &str) -> bool {
        true
    }

    #[test]
    fn windows_rdp_uses_mstsc() {
        let c = resolve_command(RemoteDesktopKind::Rdp, "windows", 55001, None, &all).unwrap();
        assert_eq!(c.program, "mstsc");
        assert_eq!(c.args, vec!["/v:127.0.0.1:55001"]);
    }

    #[test]
    fn macos_uses_url_schemes() {
        let rdp = resolve_command(RemoteDesktopKind::Rdp, "macos", 5001, None, &all).unwrap();
        assert_eq!(rdp.program, "open");
        assert_eq!(rdp.args, vec!["rdp://127.0.0.1:5001"]);
        let vnc = resolve_command(RemoteDesktopKind::Vnc, "macos", 5002, None, &all).unwrap();
        assert_eq!(vnc.args, vec!["vnc://127.0.0.1:5002"]);
    }

    #[test]
    fn linux_rdp_prefers_freerdp_and_passes_user() {
        let c = resolve_command(
            RemoteDesktopKind::Rdp,
            "linux",
            3390,
            Some("admin"),
            &all,
        )
        .unwrap();
        assert_eq!(c.program, "xfreerdp");
        assert_eq!(c.args, vec!["/v:127.0.0.1:3390", "/u:admin"]);
    }

    #[test]
    fn linux_falls_back_to_remmina_when_freerdp_missing() {
        let only_remmina = |p: &str| p == "remmina";
        let c = resolve_command(
            RemoteDesktopKind::Rdp,
            "linux",
            3390,
            None,
            &only_remmina,
        )
        .unwrap();
        assert_eq!(c.program, "remmina");
        assert_eq!(c.args, vec!["-c", "rdp://127.0.0.1:3390"]);
    }

    #[test]
    fn linux_vnc_walks_the_viewer_list() {
        let only_vinagre = |p: &str| p == "vinagre";
        let c = resolve_command(RemoteDesktopKind::Vnc, "linux", 5901, None, &only_vinagre).unwrap();
        assert_eq!(c.program, "vinagre");
    }

    #[test]
    fn nothing_installed_reports_what_to_get() {
        let none = |_: &str| false;
        let err = resolve_command(RemoteDesktopKind::Rdp, "linux", 3389, None, &none).unwrap_err();
        assert!(err.looked_for.contains(&"xfreerdp".to_string()));
        assert!(err.looked_for.contains(&"remmina".to_string()));
    }

    #[test]
    fn empty_username_is_omitted() {
        let c = resolve_command(RemoteDesktopKind::Rdp, "linux", 3389, Some("  "), &all).unwrap();
        assert_eq!(c.args, vec!["/v:127.0.0.1:3389"]);
    }
}
