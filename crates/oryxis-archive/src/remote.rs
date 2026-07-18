//! Synthesis of the shell commands that run archive operations on the
//! REMOTE host over an SSH exec channel.
//!
//! Nothing here talks to the network: callers probe the host with
//! [`probe_commands`], feed the combined output to
//! [`parse_probe_output`], and then ask for concrete command strings.
//! Every interpolated path goes through [`crate::quote`], and POSIX
//! commands are additionally wrapped in `sh -c '...'` so they parse the
//! same regardless of the user's login shell (fish/csh included).

use crate::ArchiveError;
use crate::names::ArchiveKind;
use crate::quote::{sh_quote, win_quote};

/// Which command syntax the remote host's exec channel expects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteShell {
    /// Any POSIX-ish host (Linux, macOS, BSDs). Commands are wrapped in
    /// `sh -c` so the user's login shell only has to pass one
    /// single-quoted literal through.
    Posix,
    /// Windows OpenSSH (cmd.exe or PowerShell default shell). Only
    /// plain external commands with double-quoted arguments are
    /// emitted: no built-ins, no operators, so both shells parse them
    /// identically.
    Windows,
}

/// Which archive tools the probe found on the host.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ArchiveTools {
    pub tar: bool,
    /// `tar` is libarchive's bsdtar (macOS, Windows, some Linux),
    /// which unlike GNU tar can read AND write zip.
    pub bsdtar: bool,
    pub unzip: bool,
    pub zip: bool,
}

impl ArchiveTools {
    /// True when at least one operation is possible, i.e. the archive
    /// menu section is worth showing at all.
    pub fn any(self) -> bool {
        self.tar || self.unzip || self.zip
    }
}

/// The probe command(s) to run once per session. POSIX needs a single
/// exec; Windows needs two because cmd.exe and PowerShell share no
/// command separator (`&` is a background operator in PowerShell 7).
pub fn probe_commands(shell: RemoteShell) -> &'static [&'static str] {
    match shell {
        RemoteShell::Posix => &[
            // Prints one line per tool found, then tar's version line
            // (to tell bsdtar from GNU tar). Wrapped in `sh -c` for
            // login-shell independence; `command -v` is POSIX.
            "sh -c 'for t in tar unzip zip; do command -v -- \"$t\" >/dev/null 2>&1 && printf \"%s\\n\" \"$t\"; done; tar --version 2>/dev/null | head -n 1'",
        ],
        RemoteShell::Windows => &["where.exe tar unzip zip", "tar --version"],
    }
}

/// Parse the concatenated stdout of the probe command(s). Tolerant of
/// both probe styles: bare tool names (POSIX) and full `where.exe`
/// paths (`C:\Windows\System32\tar.exe`).
pub fn parse_probe_output(stdout: &str) -> ArchiveTools {
    let mut tools = ArchiveTools::default();
    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let lower = line.to_ascii_lowercase();
        if lower.contains("bsdtar") || lower.contains("libarchive") {
            tools.tar = true;
            tools.bsdtar = true;
            continue;
        }
        // Bare name (POSIX probe) or where.exe path: match on the
        // final path component, with or without `.exe`.
        let base = lower
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or(&lower)
            .trim_end_matches(".exe");
        match base {
            "tar" => tools.tar = true,
            "unzip" => tools.unzip = true,
            "zip" => tools.zip = true,
            _ => {}
        }
    }
    tools
}

/// Convert an SFTP-style absolute path on a Windows server into the
/// form Win32 tools accept: `/C:/Users/x` -> `C:/Users/x`. Forward
/// slashes are fine for tar.exe/unzip. Non-drive paths pass through.
pub fn windows_native_path(sftp_path: &str) -> String {
    let bytes = sftp_path.as_bytes();
    if bytes.len() >= 3
        && bytes[0] == b'/'
        && bytes[1].is_ascii_alphabetic()
        && bytes[2] == b':'
    {
        sftp_path[1..].to_string()
    } else {
        sftp_path.to_string()
    }
}

/// Can this host extract archives of `kind`?
pub fn can_extract(shell: RemoteShell, tools: ArchiveTools, kind: ArchiveKind) -> bool {
    match kind {
        // GNU tar cannot read zip; bsdtar and unzip can.
        ArchiveKind::Zip => tools.unzip || tools.bsdtar || (shell == RemoteShell::Windows && tools.tar),
        _ => tools.tar,
    }
}

/// Can this host create archives of `kind`? Only zip and tar.gz are
/// offered for creation.
pub fn can_compress(shell: RemoteShell, tools: ArchiveTools, kind: ArchiveKind) -> bool {
    match kind {
        ArchiveKind::Zip => tools.zip || tools.bsdtar || (shell == RemoteShell::Windows && tools.tar),
        ArchiveKind::TarGz => tools.tar,
        _ => false,
    }
}

/// Whether the extract command these params synthesize invokes `unzip`.
/// Unlike `tar`, `unzip` exits 1 on benign warnings (trailing garbage,
/// extra bytes) while still extracting everything, so the caller maps
/// exit code 1 to success ONLY for unzip. This is the single source of
/// truth for that decision; it must track [`extract_command`]'s tool
/// choice byte for byte (the `extract_uses_unzip_matches_command` test
/// pins them together).
pub fn extract_uses_unzip(shell: RemoteShell, tools: ArchiveTools, kind: ArchiveKind) -> bool {
    if !matches!(kind, ArchiveKind::Zip) {
        return false;
    }
    match shell {
        // POSIX prefers unzip for zip; without it, bsdtar (`tar -xf`).
        RemoteShell::Posix => tools.unzip,
        // Windows tar.exe (bsdtar) reads zip, so it wins when present;
        // unzip is the fallback.
        RemoteShell::Windows => !tools.tar,
    }
}

/// Wrap a POSIX command so the user's login shell only parses one
/// single-quoted literal (fish and csh diverge from POSIX on loops,
/// `&&`, and escaping; `sh -c '<literal>'` is common ground).
fn wrap_posix(inner: &str) -> Result<String, ArchiveError> {
    Ok(format!("sh -c {}", sh_quote(inner)?))
}

/// Build the command that extracts `archive_abs` into the (already
/// created by the caller, via SFTP mkdir) directory `dest_abs`. Both
/// are absolute paths as the SFTP layer sees them.
pub fn extract_command(
    shell: RemoteShell,
    tools: ArchiveTools,
    kind: ArchiveKind,
    archive_abs: &str,
    dest_abs: &str,
) -> Result<String, ArchiveError> {
    if !can_extract(shell, tools, kind) {
        return Err(ArchiveError::Unsupported(format!(
            "no remote tool available to extract {}",
            kind.extension()
        )));
    }
    match shell {
        RemoteShell::Posix => {
            let a = sh_quote(archive_abs)?;
            let d = sh_quote(dest_abs)?;
            let inner = match kind {
                ArchiveKind::Zip if tools.unzip => format!("unzip -o -qq {a} -d {d}"),
                // bsdtar reads zip with plain -xf.
                ArchiveKind::Zip => format!("tar -xf {a} -C {d}"),
                // Explicit -z: universal (busybox included).
                ArchiveKind::TarGz => format!("tar -xzf {a} -C {d}"),
                // GNU tar and bsdtar both auto-detect bz2/xz/zst on
                // read with plain -xf; a host whose tar lacks the
                // codec reports it on stderr, which we surface.
                ArchiveKind::Tar
                | ArchiveKind::TarBz2
                | ArchiveKind::TarXz
                | ArchiveKind::TarZst => format!("tar -xf {a} -C {d}"),
            };
            wrap_posix(&inner)
        }
        RemoteShell::Windows => {
            let a = win_quote(&windows_native_path(archive_abs))?;
            let d = win_quote(&windows_native_path(dest_abs))?;
            Ok(match kind {
                // Windows tar.exe is bsdtar: one syntax for everything.
                _ if tools.tar => format!("tar -xf {a} -C {d}"),
                ArchiveKind::Zip => format!("unzip -o -qq {a} -d {d}"),
                _ => unreachable!("can_extract gated"),
            })
        }
    }
}

/// Build the command that creates archive `out_name` (a bare file name;
/// the caller pre-computed uniqueness) inside directory `cwd_abs`,
/// containing `items` (names relative to `cwd_abs`, straight from the
/// directory listing).
pub fn compress_command(
    shell: RemoteShell,
    tools: ArchiveTools,
    kind: ArchiveKind,
    cwd_abs: &str,
    out_name: &str,
    items: &[String],
) -> Result<String, ArchiveError> {
    if !can_compress(shell, tools, kind) {
        return Err(ArchiveError::Unsupported(format!(
            "no remote tool available to create {}",
            kind.extension()
        )));
    }
    if items.is_empty() {
        return Err(ArchiveError::Unsupported("nothing to compress".into()));
    }
    // `./` prefix keeps a leading-dash name from parsing as an option
    // (tar and zip both store the name without the prefix).
    let rel = |name: &str| format!("./{name}");
    match shell {
        RemoteShell::Posix => {
            let cwd = sh_quote(cwd_abs)?;
            let inner = match kind {
                ArchiveKind::Zip if tools.zip => {
                    // `zip` has no -C, so cd first; `-r` recurses, `-q`
                    // keeps the exec output to errors only.
                    let out = sh_quote(&rel(out_name))?;
                    let list = quoted_list(items, rel, sh_quote)?;
                    format!("cd {cwd} && zip -r -q {out} {list}")
                }
                ArchiveKind::Zip => {
                    // bsdtar: -a picks the zip writer from the output
                    // extension; -C scopes the item names.
                    let out = sh_quote(&format!("{}/{}", cwd_abs.trim_end_matches('/'), out_name))?;
                    let list = quoted_list(items, rel, sh_quote)?;
                    format!("tar -a -cf {out} -C {cwd} {list}")
                }
                ArchiveKind::TarGz => {
                    let out = sh_quote(&format!("{}/{}", cwd_abs.trim_end_matches('/'), out_name))?;
                    let list = quoted_list(items, rel, sh_quote)?;
                    format!("tar -czf {out} -C {cwd} {list}")
                }
                _ => unreachable!("can_compress gated"),
            };
            wrap_posix(&inner)
        }
        RemoteShell::Windows => {
            let cwd_native = windows_native_path(cwd_abs);
            let cwd = win_quote(&cwd_native)?;
            let out = win_quote(&format!(
                "{}/{}",
                cwd_native.trim_end_matches('/'),
                out_name
            ))?;
            let list = quoted_list(items, rel, win_quote)?;
            Ok(match kind {
                ArchiveKind::Zip => format!("tar -a -cf {out} -C {cwd} {list}"),
                ArchiveKind::TarGz => format!("tar -czf {out} -C {cwd} {list}"),
                _ => unreachable!("can_compress gated"),
            })
        }
    }
}

/// Quote a list of item names with the given quoting fn, space-joined.
fn quoted_list(
    items: &[String],
    rel: impl Fn(&str) -> String,
    quote: impl Fn(&str) -> Result<String, ArchiveError>,
) -> Result<String, ArchiveError> {
    let mut parts = Vec::with_capacity(items.len());
    for item in items {
        parts.push(quote(&rel(item))?);
    }
    Ok(parts.join(" "))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gnu() -> ArchiveTools {
        ArchiveTools { tar: true, bsdtar: false, unzip: true, zip: true }
    }

    fn bsd_only() -> ArchiveTools {
        ArchiveTools { tar: true, bsdtar: true, unzip: false, zip: false }
    }

    #[test]
    fn probe_parse_posix() {
        let out = "tar\nunzip\ntar (GNU tar) 1.34\n";
        let t = parse_probe_output(out);
        assert!(t.tar && t.unzip && !t.zip && !t.bsdtar);
    }

    #[test]
    fn probe_parse_bsdtar() {
        let out = "tar\nbsdtar 3.7.2 - libarchive 3.7.2 zlib/1.3\n";
        let t = parse_probe_output(out);
        assert!(t.tar && t.bsdtar);
    }

    #[test]
    fn probe_parse_windows_where() {
        let out = "C:\\Windows\\System32\\tar.exe\r\nC:\\Tools\\unzip.EXE\r\nbsdtar 3.5.2 - libarchive 3.5.2\r\n";
        let t = parse_probe_output(out);
        assert!(t.tar && t.unzip && t.bsdtar && !t.zip);
    }

    #[test]
    fn native_path_strips_drive_slash() {
        assert_eq!(windows_native_path("/C:/Users/me"), "C:/Users/me");
        assert_eq!(windows_native_path("/d:/x"), "d:/x");
        assert_eq!(windows_native_path("/home/me"), "/home/me");
    }

    #[test]
    fn capability_matrix() {
        use ArchiveKind::*;
        let gnu_no_unzip = ArchiveTools { tar: true, ..Default::default() };
        // GNU tar alone cannot touch zip.
        assert!(!can_extract(RemoteShell::Posix, gnu_no_unzip, Zip));
        assert!(can_extract(RemoteShell::Posix, gnu_no_unzip, TarGz));
        assert!(can_extract(RemoteShell::Posix, bsd_only(), Zip));
        assert!(can_extract(RemoteShell::Windows, gnu_no_unzip, Zip));
        assert!(can_compress(RemoteShell::Posix, bsd_only(), Zip));
        assert!(!can_compress(RemoteShell::Posix, gnu_no_unzip, Zip));
        assert!(!can_compress(RemoteShell::Posix, gnu(), Tar));
    }

    #[test]
    fn extract_uses_unzip_matches_command() {
        // The exit-code policy in the app keys off `extract_uses_unzip`;
        // it must agree with the tool `extract_command` actually picks for
        // every supported (shell, tools, kind). Paths here are chosen with
        // no "unzip" substring so the command scan is an honest oracle.
        use ArchiveKind::*;
        let shells = [RemoteShell::Posix, RemoteShell::Windows];
        let kinds = [Zip, TarGz, Tar, TarBz2, TarXz, TarZst];
        let tool_sets = [
            ArchiveTools { tar: true, bsdtar: false, unzip: true, zip: true },
            ArchiveTools { tar: true, bsdtar: true, unzip: false, zip: false },
            ArchiveTools { tar: false, bsdtar: true, unzip: false, zip: false },
            ArchiveTools { tar: false, bsdtar: false, unzip: true, zip: false },
            ArchiveTools { tar: true, bsdtar: false, unzip: false, zip: false },
        ];
        for shell in shells {
            for &kind in &kinds {
                for tools in tool_sets {
                    if !can_extract(shell, tools, kind) {
                        continue;
                    }
                    let cmd =
                        extract_command(shell, tools, kind, "/a/plain.arc", "/a/dest").unwrap();
                    assert_eq!(
                        cmd.contains("unzip "),
                        extract_uses_unzip(shell, tools, kind),
                        "shell={shell:?} kind={kind:?} tools={tools:?} cmd={cmd}"
                    );
                }
            }
        }
    }

    #[test]
    fn extract_zip_prefers_unzip() {
        let cmd = extract_command(
            RemoteShell::Posix,
            gnu(),
            ArchiveKind::Zip,
            "/srv/data/it's here.zip",
            "/srv/data/it's here",
        )
        .unwrap();
        assert_eq!(
            cmd,
            "sh -c 'unzip -o -qq '\\''/srv/data/it'\\''\\'\\'''\\''s here.zip'\\'' -d '\\''/srv/data/it'\\''\\'\\'''\\''s here'\\'''"
        );
    }

    #[test]
    fn extract_zip_bsdtar_fallback() {
        let cmd = extract_command(
            RemoteShell::Posix,
            bsd_only(),
            ArchiveKind::Zip,
            "/a/b.zip",
            "/a/b",
        )
        .unwrap();
        assert_eq!(cmd, "sh -c 'tar -xf '\\''/a/b.zip'\\'' -C '\\''/a/b'\\'''");
    }

    #[test]
    fn extract_targz_posix() {
        let cmd = extract_command(
            RemoteShell::Posix,
            gnu(),
            ArchiveKind::TarGz,
            "/a/x.tar.gz",
            "/a/x",
        )
        .unwrap();
        assert_eq!(cmd, "sh -c 'tar -xzf '\\''/a/x.tar.gz'\\'' -C '\\''/a/x'\\'''");
    }

    #[test]
    fn extract_windows_uses_native_paths() {
        let t = ArchiveTools { tar: true, bsdtar: true, ..Default::default() };
        let cmd = extract_command(
            RemoteShell::Windows,
            t,
            ArchiveKind::Zip,
            "/C:/Users/me/a.zip",
            "/C:/Users/me/a",
        )
        .unwrap();
        assert_eq!(cmd, "tar -xf \"C:/Users/me/a.zip\" -C \"C:/Users/me/a\"");
    }

    #[test]
    fn compress_zip_with_zip_tool() {
        let cmd = compress_command(
            RemoteShell::Posix,
            gnu(),
            ArchiveKind::Zip,
            "/srv/www",
            "site.zip",
            &["index.html".into(), "-weird dir".into()],
        )
        .unwrap();
        assert_eq!(
            cmd,
            "sh -c 'cd '\\''/srv/www'\\'' && zip -r -q '\\''./site.zip'\\'' '\\''./index.html'\\'' '\\''./-weird dir'\\'''"
        );
    }

    #[test]
    fn compress_targz_uses_dash_c() {
        let cmd = compress_command(
            RemoteShell::Posix,
            gnu(),
            ArchiveKind::TarGz,
            "/srv/www/",
            "site.tar.gz",
            &["app".into()],
        )
        .unwrap();
        assert_eq!(
            cmd,
            "sh -c 'tar -czf '\\''/srv/www/site.tar.gz'\\'' -C '\\''/srv/www/'\\'' '\\''./app'\\'''"
        );
    }

    #[test]
    fn compress_windows_zip_via_bsdtar() {
        let t = ArchiveTools { tar: true, bsdtar: true, ..Default::default() };
        let cmd = compress_command(
            RemoteShell::Windows,
            t,
            ArchiveKind::Zip,
            "/C:/data",
            "out.zip",
            &["docs".into()],
        )
        .unwrap();
        assert_eq!(cmd, "tar -a -cf \"C:/data/out.zip\" -C \"C:/data\" \"./docs\"");
    }

    #[test]
    fn hostile_name_stays_inert() {
        // The classic: a listing entry whose name embeds a command
        // substitution. It must come out as a plain literal.
        let cmd = compress_command(
            RemoteShell::Posix,
            gnu(),
            ArchiveKind::TarGz,
            "/tmp",
            "out.tar.gz",
            &["$(reboot).txt".into()],
        )
        .unwrap();
        assert!(cmd.contains("'\\''./$(reboot).txt'\\''"));
        // And a newline-bearing name is refused outright.
        assert!(
            compress_command(
                RemoteShell::Posix,
                gnu(),
                ArchiveKind::TarGz,
                "/tmp",
                "out.tar.gz",
                &["a\nb".into()],
            )
            .is_err()
        );
    }

    /// End-to-end: run the synthesized POSIX commands through a real
    /// `sh` (the same thing the exec channel hands to the login shell)
    /// and check the quoting survives contact with hostile names.
    /// `tar` is present on every unix CI runner; unix-only by nature.
    #[cfg(unix)]
    #[test]
    fn posix_synthesis_executes_for_real() {
        let tmp = tempfile::tempdir().unwrap();
        let cwd = tmp.path().join("src dir");
        std::fs::create_dir(&cwd).unwrap();
        let hostile = "$(touch pwned) 'quoted'.txt";
        std::fs::write(cwd.join(hostile), b"payload").unwrap();
        let tools = ArchiveTools { tar: true, ..Default::default() };
        let run = |cmd: &str| {
            let out = std::process::Command::new("sh")
                .arg("-c")
                .arg(cmd)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "command failed: {cmd}\nstderr: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        };
        let compress = compress_command(
            RemoteShell::Posix,
            tools,
            ArchiveKind::TarGz,
            cwd.to_str().unwrap(),
            "out.tar.gz",
            &[hostile.to_string()],
        )
        .unwrap();
        run(&compress);
        let archive = cwd.join("out.tar.gz");
        assert!(archive.exists());
        let dest = tmp.path().join("dest dir");
        std::fs::create_dir(&dest).unwrap();
        let extract = extract_command(
            RemoteShell::Posix,
            tools,
            ArchiveKind::TarGz,
            archive.to_str().unwrap(),
            dest.to_str().unwrap(),
        )
        .unwrap();
        run(&extract);
        assert_eq!(std::fs::read(dest.join(hostile)).unwrap(), b"payload");
        assert!(
            !std::path::Path::new("pwned").exists()
                && !cwd.join("pwned").exists()
                && !dest.join("pwned").exists(),
            "command substitution in a file name must never execute"
        );
    }

    #[test]
    fn windows_rejects_percent_names() {
        let t = ArchiveTools { tar: true, ..Default::default() };
        assert!(
            extract_command(
                RemoteShell::Windows,
                t,
                ArchiveKind::Zip,
                "/C:/a/%TEMP%.zip",
                "/C:/a/dest",
            )
            .is_err()
        );
    }
}
