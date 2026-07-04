use super::*;

#[test]
fn session_log_roundtrips_appended_chunks() {
    let vault = unlocked_vault();
    let log_id = Uuid::new_v4();
    let conn_id = Uuid::new_v4();
    vault.create_session_log(&log_id, &conn_id, "host-a").unwrap();
    vault.append_session_data(&log_id, b"first chunk\n", None).unwrap();
    vault.append_session_data(&log_id, b"second chunk\n", None).unwrap();
    let data = vault.get_session_data(&log_id).unwrap().unwrap();
    assert_eq!(data, b"first chunk\nsecond chunk\n");
}


#[test]
fn session_log_chunks_are_not_stored_in_the_clear() {
    let vault = unlocked_vault();
    let log_id = Uuid::new_v4();
    let conn_id = Uuid::new_v4();
    let marker = b"TOP-SECRET-OUTPUT-MARKER";
    vault.create_session_log(&log_id, &conn_id, "host-a").unwrap();
    vault.append_session_data(&log_id, marker, None).unwrap();
    // Structural check straight against the column: the stored blob
    // must not contain the recorded bytes.
    let raw: Vec<u8> = vault
        .db
        .query_row(
            "SELECT data FROM session_log_chunks WHERE log_id = ?1",
            params![log_id.to_string()],
            |row| row.get(0),
        )
        .unwrap();
    assert!(
        !raw.windows(marker.len()).any(|w| w == marker),
        "recorded output stored in the clear"
    );
    // And it still reads back through the API.
    let data = vault.get_session_data(&log_id).unwrap().unwrap();
    assert_eq!(data, marker);
}


#[test]
fn session_log_chunks_survive_master_password_change() {
    let mut vault = unlocked_vault();
    let log_id = Uuid::new_v4();
    let conn_id = Uuid::new_v4();
    vault.create_session_log(&log_id, &conn_id, "host-a").unwrap();
    vault.append_session_data(&log_id, b"before change\n", None).unwrap();
    vault.set_user_password("brand-new-password").unwrap();
    // Drop the cached content key to force a re-unwrap with the new
    // master key, as a fresh process would.
    *vault.session_log_key.lock().unwrap() = None;
    vault.append_session_data(&log_id, b"after change\n", None).unwrap();
    let data = vault.get_session_data(&log_id).unwrap().unwrap();
    assert_eq!(data, b"before change\nafter change\n");
}


#[test]
fn session_log_chunks_concatenate_in_append_order() {
    let vault = unlocked_vault();
    let log_id = Uuid::new_v4();
    let conn_id = Uuid::new_v4();
    vault
        .create_session_log(&log_id, &conn_id, "web-01")
        .unwrap();

    // Append in three writes; the recorded stream must read back as
    // the exact byte-for-byte concatenation, in order.
    vault.append_session_data(&log_id, b"$ apt update\n", None).unwrap();
    vault.append_session_data(&log_id, b"Hit:1 http://deb\n", None).unwrap();
    vault.append_session_data(&log_id, b"Reading package lists\n", None).unwrap();
    // Empty appends are no-ops, never a stray zero-length chunk.
    vault.append_session_data(&log_id, b"", None).unwrap();

    let data = vault.get_session_data(&log_id).unwrap().unwrap();
    assert_eq!(
        data,
        b"$ apt update\nHit:1 http://deb\nReading package lists\n"
    );

    // Metadata size reflects the stored chunk bytes; each sealed
    // chunk carries a nonce + AEAD tag on top of the recording.
    let entry = vault
        .list_session_logs()
        .unwrap()
        .into_iter()
        .find(|e| e.id == log_id)
        .expect("log listed");
    assert_eq!(entry.data_size, data.len() + 3 * (NONCE_LEN + 16));
}


#[test]
fn session_log_reads_legacy_inline_blob_then_chunks() {
    let vault = unlocked_vault();
    let log_id = Uuid::new_v4();
    let conn_id = Uuid::new_v4();
    vault
        .create_session_log(&log_id, &conn_id, "legacy")
        .unwrap();
    // Simulate a row recorded before the chunk migration: bytes live
    // in the inline `data` column. New appends go to chunks; the read
    // path must stitch legacy-prefix + chunks.
    vault
        .db
        .execute(
            "UPDATE session_logs SET data = ?1 WHERE id = ?2",
            params![b"OLD".to_vec(), log_id.to_string()],
        )
        .unwrap();
    vault.append_session_data(&log_id, b"NEW", None).unwrap();

    let data = vault.get_session_data(&log_id).unwrap().unwrap();
    assert_eq!(data, b"OLDNEW");

    let entry = vault
        .list_session_logs()
        .unwrap()
        .into_iter()
        .find(|e| e.id == log_id)
        .unwrap();
    // Inline legacy bytes are raw; the appended chunk is sealed.
    assert_eq!(entry.data_size, 3 + 3 + NONCE_LEN + 16);
}


#[test]
fn deleting_session_log_drops_its_chunks() {
    let vault = unlocked_vault();
    let log_id = Uuid::new_v4();
    vault
        .create_session_log(&log_id, &Uuid::new_v4(), "doomed")
        .unwrap();
    vault.append_session_data(&log_id, b"transient", None).unwrap();

    vault.delete_session_log(&log_id).unwrap();

    // No orphan chunks left behind.
    let orphans: i64 = vault
        .db
        .query_row(
            "SELECT COUNT(*) FROM session_log_chunks WHERE log_id = ?1",
            params![log_id.to_string()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(orphans, 0);
    assert!(vault.get_session_data(&log_id).is_err());
}


#[test]
fn add_and_list_logs() {
    let vault = unlocked_vault();
    let entry = LogEntry::new("prod-web", "192.168.1.10", LogEvent::Connected, "OK");
    vault.add_log(&entry).unwrap();

    let logs = vault.list_logs(10).unwrap();
    assert_eq!(logs.len(), 1);
    assert_eq!(logs[0].connection_label, "prod-web");
}


#[test]
fn logs_ordered_by_timestamp_desc() {
    let vault = unlocked_vault();
    vault.add_log(&LogEntry::new("first", "h1", LogEvent::Connected, "")).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(10));
    vault.add_log(&LogEntry::new("second", "h2", LogEvent::Disconnected, "")).unwrap();

    let logs = vault.list_logs(10).unwrap();
    assert_eq!(logs[0].connection_label, "second"); // most recent first
}


#[test]
fn clear_logs() {
    let vault = unlocked_vault();
    vault.add_log(&LogEntry::new("x", "y", LogEvent::Error, "fail")).unwrap();
    vault.add_log(&LogEntry::new("a", "b", LogEvent::Connected, "ok")).unwrap();
    vault.clear_logs().unwrap();
    assert_eq!(vault.list_logs(100).unwrap().len(), 0);
}


#[test]
fn logs_limit_works() {
    let vault = unlocked_vault();
    for i in 0..20 {
        vault.add_log(&LogEntry::new(&format!("conn-{}", i), "h", LogEvent::Connected, "")).unwrap();
    }
    let logs = vault.list_logs(5).unwrap();
    assert_eq!(logs.len(), 5);
}

// ── MCP enabled field ──


#[test]
fn session_events_carry_offsets_and_kinds() {
    let vault = unlocked_vault();
    let log_id = Uuid::new_v4();
    let conn_id = Uuid::new_v4();
    vault.create_session_log(&log_id, &conn_id, "host-a").unwrap();
    vault.append_session_resize(&log_id, 0, 120, 30).unwrap();
    vault.append_session_data(&log_id, b"$ ls\n", Some(150)).unwrap();
    vault.append_session_data(&log_id, b"README\n", Some(400)).unwrap();
    vault.append_session_resize(&log_id, 900, 100, 40).unwrap();

    let events = vault.get_session_events(&log_id).unwrap();
    assert_eq!(events.len(), 4);
    assert_eq!(events[0].kind, 'r');
    assert_eq!(events[0].offset_ms, Some(0));
    assert_eq!(events[0].data, b"120x30");
    assert_eq!(events[1].kind, 'o');
    assert_eq!(events[1].offset_ms, Some(150));
    assert_eq!(events[1].data, b"$ ls\n");
    assert_eq!(events[3].kind, 'r');
    assert_eq!(events[3].data, b"100x40");
}

#[test]
fn resize_events_stay_out_of_the_byte_stream() {
    // `get_session_data` reconstructs what the terminal received;
    // resize rows are replay metadata and must never leak into it.
    let vault = unlocked_vault();
    let log_id = Uuid::new_v4();
    let conn_id = Uuid::new_v4();
    vault.create_session_log(&log_id, &conn_id, "host-a").unwrap();
    vault.append_session_data(&log_id, b"before ", Some(10)).unwrap();
    vault.append_session_resize(&log_id, 20, 90, 25).unwrap();
    vault.append_session_data(&log_id, b"after\n", Some(30)).unwrap();
    let data = vault.get_session_data(&log_id).unwrap().unwrap();
    assert_eq!(data, b"before after\n");
}

#[test]
fn untimed_chunks_read_back_with_no_offset() {
    // Pre-migration rows have NULL offsets; the export layer gives
    // them a fixed replay delta, the store must not invent timing.
    let vault = unlocked_vault();
    let log_id = Uuid::new_v4();
    let conn_id = Uuid::new_v4();
    vault.create_session_log(&log_id, &conn_id, "host-a").unwrap();
    vault.append_session_data(&log_id, b"legacy\n", None).unwrap();
    let events = vault.get_session_events(&log_id).unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].offset_ms, None);
    assert_eq!(events[0].kind, 'o');
}
