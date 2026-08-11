use super::*;
use oryxis_core::models::connection::{Connection, EnvVar};
use oryxis_core::models::group::{Group, GroupDefaults};
use oryxis_core::models::key::{KeyAlgorithm, SshKey};
use oryxis_core::models::known_host::KnownHost;
use oryxis_core::models::log_entry::{LogEntry, LogEvent};
use oryxis_core::models::login_script::LoginScript;
use oryxis_core::models::session_group::{
    PaneLayout, PaneMember, PaneSource, SessionGroup, SplitAxis,
};
use oryxis_core::models::snippet::Snippet;
use tempfile::NamedTempFile;

fn temp_vault() -> VaultStore {
    let tmp = NamedTempFile::new().unwrap();
    let path = tmp.path().to_path_buf();
    // Keep the file alive by leaking it (tests are short-lived)
    std::mem::forget(tmp);
    VaultStore::open(&path).unwrap()
}

fn unlocked_vault() -> VaultStore {
    let mut vault = temp_vault();
    vault.set_master_password("testpass123").unwrap();
    vault
}

mod chat;
mod cloud;
mod command_history;
mod connections;
mod core_crypto;
mod forwarding;
mod groups;
mod identities;
mod inheritance;
mod keys;
mod logs;
mod portable;
mod portable_hardening;
mod settings;
mod snippets;
mod sync;

/// `ORYXIS_HOME` overrides the vault's home directory. The harness
/// sandbox depends on this on Windows, where `dirs::home_dir()` is a
/// WinAPI call that ignores `$HOME` / `%USERPROFILE%`: without the
/// override a harness run would open (and migrate) the REAL profile's
/// vault. Exercised through the pure resolver because this binary runs
/// its tests on parallel threads, where `set_var` racing any `getenv`
/// (`tempfile` reads `TMPDIR` on every tempdir) is undefined behavior;
/// the end-to-end pin lives in `tests/oryxis_home.rs`, a binary with
/// exactly one test and therefore no such race.
#[test]
fn oryxis_home_overrides_vault_home() {
    use std::ffi::OsString;

    let sandbox = || Some(OsString::from("/sandbox"));
    let home = || Some(PathBuf::from("/real-home"));
    // The override wins over the OS home.
    assert_eq!(
        super::vault_home(sandbox(), home()),
        Some(PathBuf::from("/sandbox"))
    );
    // Unset: the OS home.
    assert_eq!(super::vault_home(None, home()), home());
    // An accidental `export ORYXIS_HOME=` falls through to the real
    // home instead of landing the vault in the working directory.
    assert_eq!(super::vault_home(Some(OsString::new()), home()), home());
    // Nothing resolves at all: `open_default` surfaces an error.
    assert_eq!(super::vault_home(Some(OsString::new()), None), None);
}
