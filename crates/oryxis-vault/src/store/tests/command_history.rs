use super::*;
use uuid::Uuid;

#[test]
fn record_inserts_then_bumps_use_count() {
    let vault = unlocked_vault();
    let host = Uuid::new_v4();

    vault.record_command(&host, "ls -la").unwrap();
    vault.record_command(&host, "ls -la").unwrap();
    vault.record_command(&host, "pwd").unwrap();

    let entries = vault.list_command_history(&host).unwrap();
    assert_eq!(entries.len(), 2);
    let ls = entries.iter().find(|e| e.command == "ls -la").unwrap();
    assert_eq!(ls.use_count, 2);
    let pwd = entries.iter().find(|e| e.command == "pwd").unwrap();
    assert_eq!(pwd.use_count, 1);
}

#[test]
fn list_orders_most_recent_first_and_scopes_per_host() {
    let vault = unlocked_vault();
    let a = Uuid::new_v4();
    let b = Uuid::new_v4();

    vault.record_command(&a, "first").unwrap();
    // RFC3339 carries sub-second precision, but two writes can land inside
    // the same tick; force distinct timestamps for a deterministic order.
    std::thread::sleep(std::time::Duration::from_millis(5));
    vault.record_command(&a, "second").unwrap();
    vault.record_command(&b, "other-host").unwrap();

    let entries = vault.list_command_history(&a).unwrap();
    assert_eq!(
        entries.iter().map(|e| e.command.as_str()).collect::<Vec<_>>(),
        vec!["second", "first"],
    );
    let entries_b = vault.list_command_history(&b).unwrap();
    assert_eq!(entries_b.len(), 1);
    assert_eq!(entries_b[0].command, "other-host");
}

#[test]
fn rerunning_an_old_command_moves_it_to_the_front() {
    let vault = unlocked_vault();
    let host = Uuid::new_v4();

    vault.record_command(&host, "old").unwrap();
    std::thread::sleep(std::time::Duration::from_millis(5));
    vault.record_command(&host, "new").unwrap();
    std::thread::sleep(std::time::Duration::from_millis(5));
    vault.record_command(&host, "old").unwrap();

    let entries = vault.list_command_history(&host).unwrap();
    assert_eq!(entries[0].command, "old");
    assert_eq!(entries[0].use_count, 2);
}

#[test]
fn per_host_cap_drops_least_recently_used() {
    let vault = unlocked_vault();
    let host = Uuid::new_v4();

    // The first insert is the LRU victim once the cap overflows.
    vault.record_command(&host, "victim").unwrap();
    std::thread::sleep(std::time::Duration::from_millis(5));
    for i in 0..200 {
        vault.record_command(&host, &format!("cmd-{i}")).unwrap();
    }

    let entries = vault.list_command_history(&host).unwrap();
    assert_eq!(entries.len(), 200, "cap must hold at 200 rows per host");
    assert!(
        !entries.iter().any(|e| e.command == "victim"),
        "the least-recently-used row must be the one pruned"
    );
}

#[test]
fn delete_entry_and_clear_host() {
    let vault = unlocked_vault();
    let host = Uuid::new_v4();

    vault.record_command(&host, "keep").unwrap();
    vault.record_command(&host, "drop").unwrap();
    let entries = vault.list_command_history(&host).unwrap();
    let drop_id = entries.iter().find(|e| e.command == "drop").unwrap().id;

    vault.delete_command_history_entry(&drop_id).unwrap();
    let entries = vault.list_command_history(&host).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].command, "keep");

    vault.clear_command_history(&host).unwrap();
    assert!(vault.list_command_history(&host).unwrap().is_empty());
}

/// Critical: a captured command line can carry echoed inline secrets
/// (`mysql -pS3cret`), so the text must never sit in a plaintext column.
/// The `command` column carries only the keyed dedup hash; the text
/// lives sealed in `command_enc`.
#[test]
fn command_text_is_encrypted_at_rest() {
    let vault = unlocked_vault();
    let host = Uuid::new_v4();
    let secret_cmd = "mysql -u root -pS3cretMarker777";

    vault.record_command(&host, secret_cmd).unwrap();

    let (raw_cmd, raw_enc): (String, Option<Vec<u8>>) = vault
        .db
        .query_row(
            "SELECT command, command_enc FROM command_history WHERE connection_id = ?1",
            params![host.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert!(
        !raw_cmd.contains("S3cretMarker777"),
        "command text leaked into the plaintext command column: {raw_cmd}"
    );
    let enc = raw_enc.expect("sealed command blob must be present");
    assert!(
        !String::from_utf8_lossy(&enc).contains("S3cretMarker777"),
        "command text leaked unencrypted into command_enc"
    );

    // Round-trip through the API still yields the original text.
    let entries = vault.list_command_history(&host).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].command, secret_cmd);
}

/// Rows written by pre-encryption versions (plaintext `command`, NULL
/// `command_enc`) are sealed on first unlocked use, keep their counters,
/// and dedup against new recordings of the same command.
#[test]
fn legacy_plaintext_rows_migrate_and_dedup() {
    let vault = unlocked_vault();
    let host = Uuid::new_v4();

    // Simulate a row from an older build: plaintext command, no blob.
    vault
        .db
        .execute(
            "INSERT INTO command_history
                 (id, connection_id, command, use_count, last_used_at, created_at)
             VALUES (?1, ?2, 'sudo systemctl restart nginx', 3, ?3, ?3)",
            params![
                Uuid::new_v4().to_string(),
                host.to_string(),
                chrono::Utc::now().to_rfc3339()
            ],
        )
        .unwrap();

    // Recording the same command must migrate the legacy row first and
    // then bump it, not insert a duplicate.
    vault
        .record_command(&host, "sudo systemctl restart nginx")
        .unwrap();

    let entries = vault.list_command_history(&host).unwrap();
    assert_eq!(entries.len(), 1, "legacy row must dedup with the new record");
    assert_eq!(entries[0].command, "sudo systemctl restart nginx");
    assert_eq!(entries[0].use_count, 4, "migration must keep the counter");

    // And the plaintext is gone from the table.
    let leftovers: i64 = vault
        .db
        .query_row(
            "SELECT COUNT(*) FROM command_history
             WHERE command_enc IS NULL OR command LIKE '%nginx%'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(leftovers, 0, "plaintext command survived the migration");
}

/// A vault reset must not leave a command trail behind (the DROP list in
/// `destroy_and_recreate` has to cover this table like every other).
#[test]
fn destroy_and_recreate_wipes_command_history() {
    let mut vault = unlocked_vault();
    let host = Uuid::new_v4();
    vault.record_command(&host, "ls -la").unwrap();

    vault.destroy_and_recreate().unwrap();

    let rows: i64 = vault
        .db
        .query_row("SELECT COUNT(*) FROM command_history", [], |row| row.get(0))
        .unwrap();
    assert_eq!(rows, 0, "command history survived a vault destroy");
}

#[test]
fn deleting_the_connection_cascades_its_history() {
    let vault = unlocked_vault();
    let conn = Connection::new("hist-host", "hist.example.com");
    vault.save_connection(&conn, None).unwrap();
    vault.record_command(&conn.id, "ls").unwrap();

    vault.delete_connection(&conn.id).unwrap();

    assert!(
        vault.list_command_history(&conn.id).unwrap().is_empty(),
        "host deletion must not leave a command trail behind"
    );
}

#[test]
fn search_command_history_maps_commands_to_hosts() {
    let vault = unlocked_vault();
    let a = Uuid::new_v4();
    let b = Uuid::new_v4();
    let c = Uuid::new_v4();
    vault.record_command(&a, "kubectl get pods").unwrap();
    std::thread::sleep(std::time::Duration::from_millis(5));
    vault.record_command(&a, "kubectl logs api").unwrap();
    vault.record_command(&b, "docker ps").unwrap();
    vault.record_command(&c, "Kubectl drain node-1").unwrap();

    // Case-insensitive, one hit per host, most recently used first.
    let hits = vault.search_command_history("kubectl").unwrap();
    assert_eq!(hits.len(), 2);
    let host_a = hits.iter().find(|(id, _)| *id == a).unwrap();
    assert_eq!(host_a.1, "kubectl logs api");
    let host_c = hits.iter().find(|(id, _)| *id == c).unwrap();
    assert_eq!(host_c.1, "Kubectl drain node-1");

    assert!(vault.search_command_history("terraform").unwrap().is_empty());
    assert!(vault.search_command_history("").unwrap().is_empty());
}
