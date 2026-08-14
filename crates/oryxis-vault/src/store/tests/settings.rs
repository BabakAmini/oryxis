use super::*;

#[test]
fn settings_round_trip_string_value() {
    let vault = temp_vault();
    vault.set_setting("scrollback_rows", "10000").unwrap();
    assert_eq!(
        vault.get_setting("scrollback_rows").unwrap().as_deref(),
        Some("10000"),
    );
}


#[test]
fn sync_sftp_passphrase_round_trips_encrypted() {
    let mut vault = temp_vault();
    vault.set_master_password("master").unwrap();
    vault.set_sync_sftp_passphrase("hunter2-group-secret").unwrap();
    assert_eq!(
        vault.get_sync_sftp_passphrase().unwrap().as_deref(),
        Some("hunter2-group-secret"),
    );
    // The stored column must be ciphertext, never the plaintext
    // passphrase, mirroring the proxy/AI-key leak invariants.
    let raw = vault.get_setting("sync_sftp_passphrase").unwrap().unwrap();
    assert!(
        !raw.contains("hunter2-group-secret"),
        "passphrase must not sit in plaintext in the settings column"
    );
}

#[test]
fn sync_sftp_passphrase_empty_clears_row() {
    let mut vault = temp_vault();
    vault.set_master_password("master").unwrap();
    vault.set_sync_sftp_passphrase("secret").unwrap();
    // Clearing deletes the row (rather than storing ""), so a later
    // key-rotation re-encrypt pass never chokes on an empty ciphertext.
    vault.set_sync_sftp_passphrase("").unwrap();
    assert_eq!(vault.get_sync_sftp_passphrase().unwrap(), None);
    assert_eq!(vault.get_setting("sync_sftp_passphrase").unwrap(), None);
}

/// The passphrase must survive a close-and-reopen of the vault: the
/// git / SFTP / folder / WebDAV transports derive the SAME snapshot key
/// from the field that a later session hydrates from storage. A lossy
/// round-trip here would surface as "Decryption failed (wrong key?)"
/// after every app restart even though the user typed the same phrase.
#[test]
fn sync_sftp_passphrase_survives_close_and_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("vault.db");
    {
        let mut vault = VaultStore::open(&db).unwrap();
        vault.set_master_password("master").unwrap();
        vault.set_sync_sftp_passphrase("hunter2-group-secret").unwrap();
    }
    // Simulate an app restart: a brand-new handle, same file.
    let mut vault = VaultStore::open(&db).unwrap();
    vault.unlock("master").unwrap();
    assert_eq!(
        vault.get_sync_sftp_passphrase().unwrap().as_deref(),
        Some("hunter2-group-secret"),
        "the passphrase must round-trip through a restart"
    );
    // And the derived snapshot key must be identical, which is the
    // actual contract the sync transports rely on.
    let before = super::super::derive_sync_secret("hunter2-group-secret").unwrap();
    let after = super::super::derive_sync_secret(
        &vault.get_sync_sftp_passphrase().unwrap().unwrap(),
    )
    .unwrap();
    assert_eq!(before, after);
}

#[test]
fn files_recent_folders_round_trip_encrypted() {
    let mut vault = temp_vault();
    vault.set_master_password("master").unwrap();
    let json = r#"{"6f1a":["/srv/acme-client/deploy"]}"#;
    vault.set_files_recent_folders(json).unwrap();
    assert_eq!(vault.get_files_recent_folders().unwrap().as_deref(), Some(json));
    // The settings table is read WITHOUT unlocking, so a plain row would
    // hand every host's directory layout to anyone holding the file.
    let raw = vault.get_setting("files_recent_folders").unwrap().unwrap();
    assert!(
        !raw.contains("acme-client"),
        "the folder history must not sit in plaintext in the settings column"
    );
}

#[test]
fn files_recent_folders_drops_a_pre_encryption_row() {
    let mut vault = temp_vault();
    vault.set_master_password("master").unwrap();
    // What a build before the encryption change left behind: plain JSON.
    // Reading must report absence AND remove the row, or the plaintext
    // this guards against would survive the upgrade.
    vault
        .set_setting("files_recent_folders", r#"{"6f1a":["/srv/acme-client"]}"#)
        .unwrap();
    assert_eq!(vault.get_files_recent_folders().unwrap(), None);
    assert_eq!(vault.get_setting("files_recent_folders").unwrap(), None);
}

#[test]
fn settings_get_unset_returns_none() {
    let vault = temp_vault();
    // No tests should pollute global state, but defensively
    // assert that a fresh vault has nothing under our key.
    assert_eq!(
        vault.get_setting("never_set_anywhere").unwrap(),
        None,
    );
}


#[test]
fn settings_overwrite_replaces_value() {
    let vault = temp_vault();
    vault.set_setting("ai_provider", "anthropic").unwrap();
    vault.set_setting("ai_provider", "openai").unwrap();
    assert_eq!(
        vault.get_setting("ai_provider").unwrap().as_deref(),
        Some("openai"),
    );
}


#[test]
fn settings_persist_across_reopen() {
    // Opening the vault file twice (different `VaultStore`
    // instances backed by the same SQLite file) must yield the
    // same setting value, the bug we're guarding against is
    // someone adding a transient cache that doesn't write through.
    use tempfile::NamedTempFile;
    let tmp = NamedTempFile::new().unwrap();
    let path = tmp.path().to_path_buf();
    {
        let v = VaultStore::open(&path).unwrap();
        v.set_setting("sftp_concurrency", "4").unwrap();
    }
    let v2 = VaultStore::open(&path).unwrap();
    assert_eq!(
        v2.get_setting("sftp_concurrency").unwrap().as_deref(),
        Some("4"),
    );
}


#[test]
fn settings_handle_unicode_values() {
    // The settings panel exposes `ai_system_prompt` as a free-form
    // string the user can fill with anything. Make sure we round-
    // trip non-ASCII without mangling.
    let vault = temp_vault();
    let value = "Olá 🚀, system prompt with emojis";
    vault.set_setting("ai_system_prompt", value).unwrap();
    assert_eq!(
        vault.get_setting("ai_system_prompt").unwrap().as_deref(),
        Some(value),
    );
}


#[test]
fn settings_independent_keys_dont_collide() {
    let vault = temp_vault();
    vault.set_setting("a", "1").unwrap();
    vault.set_setting("b", "2").unwrap();
    vault.set_setting("c", "3").unwrap();
    assert_eq!(vault.get_setting("a").unwrap().as_deref(), Some("1"));
    assert_eq!(vault.get_setting("b").unwrap().as_deref(), Some("2"));
    assert_eq!(vault.get_setting("c").unwrap().as_deref(), Some("3"));
}

// ── Cloud Profiles ──

