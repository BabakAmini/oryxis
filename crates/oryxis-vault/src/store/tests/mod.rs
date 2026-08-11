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

/// `ORYXIS_HOME` overrides the vault location. The harness sandbox
/// depends on this on Windows, where `dirs::home_dir()` is a WinAPI
/// call that ignores `$HOME` / `%USERPROFILE%` — without the override
/// a harness run would open (and migrate) the REAL profile's vault.
/// (The empty-value fallthrough is documented on `open_default`; it is
/// not testable without opening a real vault, which a unit test must
/// never do.)
#[test]
fn open_default_honors_oryxis_home() {
    let dir = tempfile::tempdir().unwrap();
    let sandbox = dir.path().join(".oryxis");
    // SAFETY: single-threaded test; no other test in this binary reads
    // ORYXIS_HOME (grep), so the env mutation cannot race a real-home
    // open. Edition 2024 makes set_var/remove_var unsafe.
    unsafe { std::env::set_var("ORYXIS_HOME", dir.path()) };
    let vault = VaultStore::open_default().unwrap();
    drop(vault); // release the SQLite handle before asserting
    assert!(sandbox.join("vault.db").exists());
    unsafe { std::env::remove_var("ORYXIS_HOME") };
}
