//! Central resolution of the app's `~/.oryxis` data directory.
//!
//! `ORYXIS_HOME` overrides the home directory the `.oryxis` tree lives
//! under. It exists for the harness sandbox on Windows, where
//! `dirs::home_dir()` is `SHGetKnownFolderPath` (a WinAPI call that
//! ignores `$HOME` / `%USERPROFILE%`), and doubles as a portable-install
//! override everywhere else. It deliberately governs only the app's own
//! data tree: user files outside it (`~/.ssh/config`, `~/.aws`,
//! `~/.Xauthority`, the OS download folder) keep resolving against the
//! real OS home, so a portable vault never relocates the user's own
//! configuration.

use std::ffi::OsString;
use std::path::PathBuf;

/// The home directory the `.oryxis` tree lives under: the `ORYXIS_HOME`
/// override when set and non-empty, the OS home otherwise.
pub fn home_dir() -> Option<PathBuf> {
    resolve_home(std::env::var_os("ORYXIS_HOME"), dirs::home_dir())
}

/// The `~/.oryxis` data directory itself (vault, plugin cache, fonts,
/// logs, tray runtime, agent socket). Not created here; each consumer
/// creates what it needs on demand.
pub fn oryxis_dir() -> Option<PathBuf> {
    home_dir().map(|h| h.join(".oryxis"))
}

/// Pure core of [`home_dir`], split out so the resolution is
/// unit-testable without mutating the process environment (`set_var` is
/// unsafe under Rust 2024, and test binaries run their tests on
/// parallel threads).
fn resolve_home(override_home: Option<OsString>, os_home: Option<PathBuf>) -> Option<PathBuf> {
    override_home
        // An empty export must fall through to the real home, matching
        // `dirs_sys`' own `HOME` handling; otherwise the data dir would
        // land in `./.oryxis` under the process working directory.
        .filter(|h| !h.is_empty())
        .map(PathBuf::from)
        .or(os_home)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `ORYXIS_HOME` overrides the data directory's home. The harness
    /// sandbox depends on this on Windows, where `dirs::home_dir()` is
    /// a WinAPI call that ignores `$HOME` / `%USERPROFILE%`: without
    /// the override a harness run would read and write the REAL
    /// profile's `.oryxis`. Exercised through the pure resolver; the
    /// end-to-end pin lives in `oryxis-vault/tests/oryxis_home.rs`, a
    /// binary with exactly one test and therefore no `getenv` race.
    #[test]
    fn oryxis_home_overrides_home_dir() {
        let sandbox = || Some(OsString::from("/sandbox"));
        let home = || Some(PathBuf::from("/real-home"));
        // The override wins over the OS home.
        assert_eq!(
            resolve_home(sandbox(), home()),
            Some(PathBuf::from("/sandbox"))
        );
        // Unset: the OS home.
        assert_eq!(resolve_home(None, home()), home());
        // An accidental `export ORYXIS_HOME=` falls through to the real
        // home instead of landing the data dir in the working directory.
        assert_eq!(resolve_home(Some(OsString::new()), home()), home());
        // Nothing resolves at all: consumers surface their own errors.
        assert_eq!(resolve_home(Some(OsString::new()), None), None);
    }
}
