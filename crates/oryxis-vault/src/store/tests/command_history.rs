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
