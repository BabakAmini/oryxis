//! Shell quoting for remote exec commands.
//!
//! Every file name interpolated into an exec command comes from a
//! remote directory listing, i.e. from a potentially hostile server, so
//! quoting is a security boundary here, not cosmetics: a file named
//! `$(rm -rf ~).zip` must stay a literal argument.

use crate::ArchiveError;

/// Quote a string as a single-quoted POSIX shell literal. Embedded
/// single quotes use the standard `'\''` splice (close, escaped quote,
/// reopen), which every POSIX shell (and csh, for the login-shell case)
/// parses as one word.
///
/// Newlines are rejected rather than quoted: they survive single quotes
/// in `sh` but terminate the command in `csh`-family login shells, and
/// no legitimate archive workflow involves a newline in a file name.
pub fn sh_quote(s: &str) -> Result<String, ArchiveError> {
    if s.contains('\n') || s.contains('\r') {
        return Err(ArchiveError::UnsafeName(format!(
            "name contains a line break and cannot be used in a remote command: {s:?}"
        )));
    }
    Ok(format!("'{}'", s.replace('\'', "'\\''")))
}

/// Quote a string as a double-quoted argument for a Windows OpenSSH
/// exec (cmd.exe or PowerShell default shells both accept plain
/// double-quoted external-command arguments).
///
/// Windows file names can never contain `"`, `<`, `>`, `|`, `*`, `?` or
/// control characters, so those are rejected outright (their presence
/// means the string is not a real Windows path). `%` is also rejected:
/// cmd.exe expands `%VAR%` inside double quotes and offers no reliable
/// command-line escape for it.
pub fn win_quote(s: &str) -> Result<String, ArchiveError> {
    let bad = |what: &str| {
        Err(ArchiveError::UnsafeName(format!(
            "name contains {what} and cannot be used in a remote Windows command: {s:?}"
        )))
    };
    if s.chars().any(|c| c.is_control()) {
        return bad("control characters");
    }
    if s.contains('"') {
        return bad("a double quote");
    }
    if s.contains('%') {
        return bad("a percent sign (cmd.exe variable expansion)");
    }
    Ok(format!("\"{s}\""))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sh_quote_plain() {
        assert_eq!(sh_quote("file.zip").unwrap(), "'file.zip'");
        assert_eq!(sh_quote("with space.tar.gz").unwrap(), "'with space.tar.gz'");
    }

    #[test]
    fn sh_quote_hostile_names_stay_literal() {
        assert_eq!(
            sh_quote("$(rm -rf ~).zip").unwrap(),
            "'$(rm -rf ~).zip'"
        );
        assert_eq!(sh_quote("a`b;c&d|e").unwrap(), "'a`b;c&d|e'");
    }

    #[test]
    fn sh_quote_embedded_single_quote() {
        // ' -> '\'' : close, literal quote, reopen.
        assert_eq!(sh_quote("it's.zip").unwrap(), "'it'\\''s.zip'");
        assert_eq!(sh_quote("''").unwrap(), "''\\'''\\'''");
    }

    #[test]
    fn sh_quote_rejects_line_breaks() {
        assert!(sh_quote("a\nb").is_err());
        assert!(sh_quote("a\rb").is_err());
    }

    #[test]
    fn win_quote_plain() {
        assert_eq!(win_quote("C:/Users/me/file.zip").unwrap(), "\"C:/Users/me/file.zip\"");
        assert_eq!(win_quote("with space.zip").unwrap(), "\"with space.zip\"");
    }

    #[test]
    fn win_quote_rejects_expansion_and_quotes() {
        assert!(win_quote("%TEMP%.zip").is_err());
        assert!(win_quote("a\"b.zip").is_err());
        assert!(win_quote("a\nb.zip").is_err());
        assert!(win_quote("a\u{1b}b.zip").is_err());
    }
}
