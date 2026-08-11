//! End-to-end pin for the `ORYXIS_HOME` vault redirect (the harness
//! sandbox depends on it on Windows, where `dirs::home_dir()` is a
//! WinAPI call that ignores `$HOME` / `%USERPROFILE%`).
//!
//! This lives in its own integration-test binary on purpose: the
//! override can only be proven end to end by mutating the process
//! environment, and under Rust 2024 `set_var` is unsafe because it
//! races any concurrent `getenv`. A binary with exactly one test has
//! no other thread to race, which is what makes the call sound here;
//! the resolution logic itself (override wins, empty means unset) is
//! unit-tested in `oryxis_core::paths` without touching the
//! environment.

use oryxis_vault::VaultStore;

#[test]
fn open_default_honors_oryxis_home() {
    let dir = tempfile::tempdir().unwrap();
    // SAFETY: this binary contains exactly this one test, so no other
    // thread reads the environment while it is mutated.
    unsafe { std::env::set_var("ORYXIS_HOME", dir.path()) };
    let vault = VaultStore::open_default().unwrap();
    drop(vault); // release the SQLite handle before asserting
    assert!(dir.path().join(".oryxis").join("vault.db").exists());
}
