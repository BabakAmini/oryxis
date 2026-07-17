//! GIF export of session recordings (issue #71): resolve + spawn the
//! `oryxis-gif` plugin binary (a thin agg wrapper distributed through
//! the plugin pipeline, like `oryxis-mcp`). The app writes the
//! recording as a temporary `.cast` (the same asciicast v3 the export
//! action produces, terminal theme embedded in the header), hands the
//! plugin the input/output paths, and reports the outcome as a toast.
//! Unlike cloud plugins there is no JSON-RPC protocol: the contract is
//! just the CLI (`oryxis-gif <in.cast> <out.gif>`), so only the
//! distribution half of the plugin subsystem applies.

use std::path::PathBuf;

use crate::plugins::cache;

/// The plugin's provider id in the manifest / cache / Plugins panel.
pub(crate) const PROVIDER_ID: &str = "gif";

/// Resolve the plugin binary: a freshly-built `target/debug` sibling
/// wins in debug builds (the dev loop, mirroring
/// `PluginProvider::resolve_binary`), otherwise the active cached
/// version. `None` = not installed, the caller opens the install
/// modal.
pub(crate) fn resolve_binary() -> Option<PathBuf> {
    #[cfg(debug_assertions)]
    {
        if let Ok(exe) = std::env::current_exe()
            && let Some(dir) = exe.parent()
        {
            let dev = dir.join(cache::binary_name(PROVIDER_ID));
            if dev.exists() {
                return Some(dev);
            }
        }
    }
    cache::current_binary(PROVIDER_ID).ok().flatten()
}

/// Render `cast_body` to `output` with the plugin at `binary`. Writes
/// the cast to a unique temp file, spawns the plugin, and cleans the
/// temp up regardless of outcome. Returns the output path (display
/// form) on success and a one-line error otherwise. GIF rendering is
/// CPU-bound and can take a while on long recordings; the caller keeps
/// the UI responsive by running this inside a `Task`.
pub(crate) async fn render(
    binary: PathBuf,
    cast_body: String,
    output: PathBuf,
) -> Result<String, String> {
    let cast_path = std::env::temp_dir()
        .join(format!("oryxis-gif-{}.cast", uuid::Uuid::new_v4()));
    tokio::fs::write(&cast_path, cast_body)
        .await
        .map_err(|e| format!("temp cast write: {e}"))?;

    let result = run_plugin(&binary, &cast_path, &output).await;
    // The recording's raw bytes shouldn't outlive the render in the
    // OS temp dir (same privacy posture as the sync temp files).
    let _ = tokio::fs::remove_file(&cast_path).await;
    result.map(|()| output.display().to_string())
}

async fn run_plugin(
    binary: &PathBuf,
    cast: &PathBuf,
    output: &PathBuf,
) -> Result<(), String> {
    let mut cmd = tokio::process::Command::new(binary);
    cmd.arg(cast).arg(output);
    #[cfg(windows)]
    {
        // No console flash over the GUI while the renderer runs.
        use std::os::windows::process::CommandExt as _;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let out = cmd
        .output()
        .await
        .map_err(|e| format!("spawn {}: {e}", binary.display()))?;
    if out.status.success() {
        return Ok(());
    }
    // The plugin prints a single `oryxis-gif: <cause>` line on stderr;
    // surface its last non-empty line so agg's root cause reaches the
    // toast instead of a bare exit code.
    let stderr = String::from_utf8_lossy(&out.stderr);
    let cause = stderr
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("plugin exited with an error")
        .trim()
        .to_string();
    Err(cause)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The render path must clean its temp cast and surface the
    /// plugin's stderr on failure. A shell script stands in for the
    /// plugin so the test needs no real agg build; it records the cast
    /// path it was handed so the cleanup assertion targets exactly
    /// that file (counting the shared OS temp dir instead was racy:
    /// the sibling test's own temp cast made the count flap).
    #[cfg(unix)]
    #[tokio::test]
    async fn render_reports_stderr_and_cleans_temp() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let seen = dir.path().join("seen-cast-path");
        let fake = dir.path().join("fake-plugin");
        std::fs::write(
            &fake,
            format!(
                "#!/bin/sh\nprintf '%s' \"$1\" > '{}'\necho 'oryxis-gif: boom' >&2\nexit 1\n",
                seen.display()
            ),
        )
        .unwrap();
        let mut perms = std::fs::metadata(&fake).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&fake, perms).unwrap();

        let out = dir.path().join("out.gif");
        let err = render(fake, "{}".into(), out).await.unwrap_err();
        assert_eq!(err, "oryxis-gif: boom");
        let cast_path = std::fs::read_to_string(&seen).unwrap();
        assert!(
            cast_path.contains("oryxis-gif-"),
            "plugin must receive the temp cast path, got {cast_path}"
        );
        assert!(
            !std::path::Path::new(&cast_path).exists(),
            "temp cast must be cleaned up"
        );
    }

    /// Success path: the fake plugin writes the output file; render
    /// returns its display path.
    #[cfg(unix)]
    #[tokio::test]
    async fn render_returns_output_path_on_success() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let fake = dir.path().join("fake-plugin");
        // `$2` is the output path argument the app passes.
        std::fs::write(&fake, "#!/bin/sh\necho gif > \"$2\"\n").unwrap();
        let mut perms = std::fs::metadata(&fake).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&fake, perms).unwrap();

        let out = dir.path().join("out.gif");
        let path = render(fake, "{}".into(), out.clone()).await.unwrap();
        assert_eq!(path, out.display().to_string());
        assert!(out.exists());
    }
}
