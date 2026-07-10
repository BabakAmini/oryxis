//! SFTP view helpers: formatting. Split out of views/sftp/mod.rs.

use super::*;
/// POSIX rwx string for a permission/mode value, e.g. "drwxr-xr-x".
/// The leading type char comes from the row's dir/symlink flags; only
/// the low 12 bits of `mode` carry the rwx + setuid/setgid/sticky bits.
pub(crate) fn format_perms(mode: Option<u32>, is_dir: bool, is_symlink: bool) -> String {
    let Some(m) = mode else {
        return "-".to_string();
    };
    let type_char = if is_symlink {
        'l'
    } else if is_dir {
        'd'
    } else {
        '-'
    };
    let rwx = |bits: u32| {
        format!(
            "{}{}{}",
            if bits & 0o4 != 0 { 'r' } else { '-' },
            if bits & 0o2 != 0 { 'w' } else { '-' },
            if bits & 0o1 != 0 { 'x' } else { '-' },
        )
    };
    format!(
        "{}{}{}{}",
        type_char,
        rwx((m >> 6) & 0o7),
        rwx((m >> 3) & 0o7),
        rwx(m & 0o7),
    )
}

/// "uid:gid" owner string, with a dash when neither side is known
/// (Windows local entries, or a server that omits owner attributes).
pub(crate) fn format_owner(uid: Option<u32>, gid: Option<u32>) -> String {
    match (uid, gid) {
        (Some(u), Some(g)) => format!("{u}:{g}"),
        (Some(u), None) => u.to_string(),
        (None, Some(g)) => format!(":{g}"),
        (None, None) => "-".to_string(),
    }
}

/// Value for the Type column: folders / symlinks keep their friendly label;
/// files show the MIME type guessed from the extension (`application/
/// octet-stream` when unknown or extensionless).
pub(crate) fn format_kind(name: &str, is_dir: bool, is_symlink: bool) -> String {
    if is_symlink {
        return t("sftp_type_symlink").to_string();
    }
    if is_dir {
        return t("sftp_type_folder").to_string();
    }
    match name.rsplit_once('.') {
        Some((stem, ext)) if !ext.is_empty() && !stem.is_empty() => {
            mime_for_ext(&ext.to_ascii_lowercase()).to_string()
        }
        _ => "application/octet-stream".to_string(),
    }
}

/// MIME type for a (lowercased) file extension. The comprehensive
/// [`crate::mime_types`] table (embedded from mime-db) covers the long tail;
/// `dev_mime_override` wins first for source-code / dev extensions that
/// mime-db gets wrong (e.g. `.rs`, `.ts`) or doesn't list (`.go`, `.vue`).
/// Anything unknown falls back to `application/octet-stream`.
pub(crate) fn mime_for_ext(ext: &str) -> &'static str {
    dev_mime_override(ext)
        .or_else(|| crate::mime_types::lookup(ext))
        .unwrap_or("application/octet-stream")
}

/// Source-code / dev extensions where mime-db is wrong or missing. Returns
/// `None` for everything else so the embedded mime-db table answers.
pub(crate) fn dev_mime_override(ext: &str) -> Option<&'static str> {
    Some(match ext {
        "rs" => "text/x-rust",
        "go" => "text/x-go",
        "py" | "pyw" | "pyi" => "text/x-python",
        "rb" => "text/x-ruby",
        // mime-db maps .ts to MPEG transport stream; in a code tree it's
        // overwhelmingly TypeScript.
        "ts" | "tsx" | "mts" | "cts" => "application/typescript",
        "jsx" => "text/jsx",
        "mjs" | "cjs" => "text/javascript",
        "kt" | "kts" => "text/x-kotlin",
        "swift" => "text/x-swift",
        "cs" => "text/x-csharp",
        "cpp" | "cc" | "cxx" | "hpp" | "hxx" => "text/x-c++",
        "vue" => "text/x-vue",
        "svelte" => "text/x-svelte",
        "astro" => "text/x-astro",
        "dockerfile" => "text/x-dockerfile",
        "env" => "text/plain",
        "ex" | "exs" => "text/x-elixir",
        "erl" => "text/x-erlang",
        "hs" => "text/x-haskell",
        "clj" | "cljs" | "cljc" => "text/x-clojure",
        "scala" | "sc" => "text/x-scala",
        "dart" => "application/dart",
        "zig" => "text/x-zig",
        "nim" => "text/x-nim",
        "proto" => "text/x-protobuf",
        "tf" | "tfvars" => "text/x-terraform",
        "gradle" => "text/x-gradle",
        _ => return None,
    })
}

/// Rough px width of `s` at the given font size, used only to decide whether
/// a Name cell is truncated (and so warrants a hover tooltip). The UI font is
/// proportional, so this is an estimate biased slightly high (~0.55em average
/// advance) to avoid attaching tooltips to names that actually fit.
pub(crate) fn approx_text_width(s: &str, size: f32) -> f32 {
    s.chars().count() as f32 * size * 0.55
}

pub(crate) fn format_modified_local(modified: Option<std::time::SystemTime>) -> String {
    let Some(t) = modified else { return String::new() };
    // NOT `DateTime::from(SystemTime)`: that conversion panics when the
    // timestamp falls outside chrono's representable range, and corrupt
    // NTFS mtimes really do land there (an invalid FILETIME reads as
    // year 30828+), which took the whole app down just for listing the
    // folder that contained the file. Convert checked; garbage renders
    // as a dash instead of a crash.
    let (secs, nanos) = match t.duration_since(std::time::UNIX_EPOCH) {
        Ok(d) => (i64::try_from(d.as_secs()).ok(), d.subsec_nanos()),
        // Pre-epoch: negative seconds with nanos counted forward from
        // the previous whole second, chrono's convention.
        Err(e) => {
            let d = e.duration();
            let s = i64::try_from(d.as_secs()).ok();
            if d.subsec_nanos() == 0 {
                (s.and_then(i64::checked_neg), 0)
            } else {
                (
                    s.and_then(|s| s.checked_add(1)).and_then(i64::checked_neg),
                    1_000_000_000 - d.subsec_nanos(),
                )
            }
        }
    };
    secs.and_then(|s| chrono::DateTime::<chrono::Utc>::from_timestamp(s, nanos))
        .map(|dt| dt.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_else(|| "-".to_string())
}

pub(crate) fn format_modified_remote(mtime: Option<u32>) -> String {
    let Some(secs) = mtime else { return String::new() };
    match chrono::DateTime::<chrono::Utc>::from_timestamp(secs as i64, 0) {
        Some(dt) => dt
            .with_timezone(&chrono::Local)
            .format("%Y-%m-%d %H:%M")
            .to_string(),
        None => String::new(),
    }
}

pub(crate) fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut idx = 0;
    while value >= 1024.0 && idx < UNITS.len() - 1 {
        value /= 1024.0;
        idx += 1;
    }
    if idx == 0 {
        format!("{} {}", bytes, UNITS[0])
    } else {
        format!("{:.1} {}", value, UNITS[idx])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, UNIX_EPOCH};

    /// Corrupt NTFS mtimes (invalid FILETIMEs read as year 30828+) land
    /// outside chrono's range; formatting must degrade to a dash, not
    /// panic mid-render (this crashed the whole app on listing such a
    /// folder, issue #63).
    #[test]
    fn format_modified_local_survives_out_of_range_mtimes() {
        // ~year 318,000, past chrono's +262,143 ceiling. Some platforms
        // can't even represent it in SystemTime; nothing to assert there.
        if let Some(t) = UNIX_EPOCH.checked_add(Duration::from_secs(10_000_000_000_000)) {
            assert_eq!(format_modified_local(Some(t)), "-");
        }
        // A sane modern date still formats.
        let recent = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        assert!(format_modified_local(Some(recent)).starts_with("2023-11-1"));
        // Pre-epoch is a real date, not garbage: 1901 formats too.
        if let Some(t) = UNIX_EPOCH.checked_sub(Duration::from_secs(2_147_483_648)) {
            assert!(format_modified_local(Some(t)).starts_with("1901-"));
        }
        assert_eq!(format_modified_local(None), "");
    }
}
