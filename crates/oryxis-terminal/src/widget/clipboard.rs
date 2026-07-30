/// Bracketed-paste start marker (`ESC [ 200 ~`).
const PASTE_START: &[u8] = b"\x1b[200~";
/// Bracketed-paste end marker (`ESC [ 201 ~`).
const PASTE_END: &[u8] = b"\x1b[201~";

/// Prepare clipboard text for writing to a terminal session.
///
/// Line endings are normalized to bare CR (`\r`), the byte the Enter key
/// sends, in both modes. A CRLF clipboard (Windows editors, files with DOS
/// endings) would otherwise deliver BOTH bytes per line break, and every
/// consumer treats each one as its own break (readline's accept-line binds
/// `\r` and `\n`, vim insert mode breaks on both, a cooked tty maps `\r` to
/// `\n` via ICRNL and keeps the `\n`), so every pasted line gained a blank
/// line (issue #60). kitty / Windows Terminal / PuTTY / xterm all send `\r`
/// for pasted newlines, inside and outside the bracket.
///
/// When `bracketed` is true (the focused app enabled DECSET 2004), wrap the
/// payload in `ESC [ 200 ~` ... `ESC [ 201 ~` so readline / TUI programs
/// (bash, zsh, Codex CLI, ...) treat the whole block as one paste and only
/// submit when the user presses Enter, instead of one submit per embedded
/// newline. Any marker already present in the clipboard is stripped first so
/// the payload can't prematurely close (or reopen) the bracket.
pub fn wrap_paste(text: &str, bracketed: bool) -> Vec<u8> {
    let normalized = text.replace("\r\n", "\r").replace('\n', "\r");
    if !bracketed {
        return normalized.into_bytes();
    }
    let sanitized = normalized.replace("\x1b[200~", "").replace("\x1b[201~", "");
    let mut out = Vec::with_capacity(sanitized.len() + PASTE_START.len() + PASTE_END.len());
    out.extend_from_slice(PASTE_START);
    out.extend_from_slice(sanitized.as_bytes());
    out.extend_from_slice(PASTE_END);
    out
}

/// Queue `text` for the host to put on the system clipboard. Shared by the
/// copy-on-select, right-click-copy and Ctrl+Shift+C paths so the three sites
/// stay in sync.
///
/// This crate never talks to the system clipboard itself: the host performs
/// every operation through the iced runtime, which keeps one clipboard access
/// in flight per process. See [`crate::host_clipboard`] for the crash that
/// bought that rule.
pub(crate) fn set_clipboard_text(text: &str) {
    crate::host_clipboard::write_text(text);
}

/// Best-effort spawn of the OS default handler for a URL. Runs detached; the
/// terminal widget never blocks on it and errors are swallowed, a failed
/// launch just means nothing happens visibly, same as any other click miss.
pub(crate) fn open_url(url: &str) {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW so the `cmd /C start` shim doesn't flash a
        // console window on the GUI-subsystem app.
        let _ = std::process::Command::new("cmd")
            .args(["/C", "start", "", url])
            .creation_flags(0x0800_0000)
            .spawn();
    }
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open").arg(url).spawn();
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let _ = std::process::Command::new("xdg-open").arg(url).spawn();
    }
}

#[cfg(test)]
mod paste_tests {
    use super::wrap_paste;

    #[test]
    fn unwrapped_when_mode_disabled_with_cr_line_endings() {
        let out = wrap_paste("line one\nline two\n", false);
        assert_eq!(out, b"line one\rline two\r");
    }

    #[test]
    fn wraps_when_mode_enabled() {
        let out = wrap_paste("hello\nworld", true);
        assert_eq!(out, b"\x1b[200~hello\rworld\x1b[201~");
    }

    /// A CRLF pair is ONE line break and must collapse to a single CR;
    /// forwarding both bytes doubles every pasted line on the receiving
    /// side (readline, vim, cooked ttys all break on each byte).
    #[test]
    fn crlf_collapses_to_single_cr() {
        assert_eq!(wrap_paste("a\r\nb\r\n", false), b"a\rb\r");
        assert_eq!(wrap_paste("a\r\nb", true), b"\x1b[200~a\rb\x1b[201~");
    }

    /// A bare CR already matches what Enter sends and passes through as-is
    /// (it must not merge with a following normalized `\n`-turned-CR).
    #[test]
    fn mixed_endings_normalize_per_break() {
        assert_eq!(wrap_paste("a\rb\nc\r\nd", false), b"a\rb\rc\rd");
    }

    #[test]
    fn strips_embedded_markers_so_payload_cannot_break_out() {
        // A clipboard carrying its own bracket markers must not be able to
        // close the bracket early or open a nested one.
        let out = wrap_paste("a\x1b[201~b\x1b[200~c", true);
        assert_eq!(out, b"\x1b[200~abc\x1b[201~");
    }
}
