use super::*;
use uuid::Uuid;

fn conv(vault: &VaultStore, label: &str) -> Uuid {
    let id = Uuid::new_v4();
    vault
        .upsert_chat_conversation(&id, None, None, label, "anthropic", "claude-sonnet-5")
        .unwrap();
    id
}

#[test]
fn a_conversation_round_trips_its_turns_in_order() {
    let vault = unlocked_vault();
    let host = Uuid::new_v4();
    let log = Uuid::new_v4();
    let id = Uuid::new_v4();
    vault
        .upsert_chat_conversation(&id, Some(&host), Some(&log), "prod-db", "openai", "gpt-4o")
        .unwrap();

    vault.append_chat_message(&id, "user", "why is disk full?", None).unwrap();
    vault
        .append_chat_message(&id, "assistant", "let me look", None)
        .unwrap();
    vault
        .append_chat_message(&id, "tool", "$ df -h", Some(r#"{"command":"df -h"}"#))
        .unwrap();

    let msgs = vault.chat_messages(&id).unwrap();
    assert_eq!(msgs.len(), 3, "every turn comes back");
    assert_eq!(msgs[0].role, "user");
    assert_eq!(msgs[0].content, "why is disk full?");
    assert_eq!(msgs[1].content, "let me look");
    assert_eq!(msgs[2].tool_json.as_deref(), Some(r#"{"command":"df -h"}"#));
    assert!(msgs[0].tool_json.is_none(), "a plain turn carries no tool");

    let list = vault.list_chat_conversations().unwrap();
    let entry = list.iter().find(|c| c.id == id).unwrap();
    assert_eq!(entry.connection_id, Some(host));
    assert_eq!(entry.session_log_id, Some(log));
    assert_eq!(entry.message_count, 3, "counted without loading the turns");
    assert_eq!(entry.model, "gpt-4o");
}

/// The whole point of keeping this apart from `session_logs`: a chat is
/// saved whether or not the session was being recorded, because recording
/// is opt-in per host. A conversation with no log id is still a first-class
/// row, and a local shell has no connection id either.
#[test]
fn a_conversation_saves_without_a_recording_or_a_host() {
    let vault = unlocked_vault();
    let id = conv(&vault, "Local Shell");
    vault.append_chat_message(&id, "user", "hello", None).unwrap();

    let list = vault.list_chat_conversations().unwrap();
    let entry = list.iter().find(|c| c.id == id).unwrap();
    assert!(entry.session_log_id.is_none());
    assert!(entry.connection_id.is_none());
    assert_eq!(entry.message_count, 1);
}

/// Structural leak test, the same discipline as
/// `proxy_password_does_not_leak_into_proxy_column`: a chat turn quotes
/// terminal output and command lines, so the plaintext must never be
/// readable in the database file.
#[test]
fn turn_text_is_never_stored_in_plaintext() {
    let vault = unlocked_vault();
    let id = conv(&vault, "prod");
    vault
        .append_chat_message(
            &id,
            "assistant",
            "the root password is hunter2",
            Some(r#"{"command":"cat /etc/shadow"}"#),
        )
        .unwrap();

    // Scan the raw columns, not the API.
    let mut stmt = vault
        .db
        .prepare("SELECT content_enc, tool_enc FROM chat_messages")
        .unwrap();
    let rows: Vec<(Vec<u8>, Option<Vec<u8>>)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    assert_eq!(rows.len(), 1);
    for (content, tool) in &rows {
        let haystack = String::from_utf8_lossy(content).to_string()
            + &tool.as_deref().map(String::from_utf8_lossy).unwrap_or_default();
        assert!(!haystack.contains("hunter2"), "reply text leaked");
        assert!(!haystack.contains("/etc/shadow"), "tool command leaked");
    }
}

#[test]
fn listing_is_most_recently_active_first() {
    let vault = unlocked_vault();
    let older = conv(&vault, "older");
    // RFC3339 carries sub-second precision, but two writes can land in the
    // same tick; force distinct stamps for a deterministic order.
    std::thread::sleep(std::time::Duration::from_millis(5));
    let newer = conv(&vault, "newer");

    let list = vault.list_chat_conversations().unwrap();
    let pos = |id: Uuid| list.iter().position(|c| c.id == id).unwrap();
    assert!(pos(newer) < pos(older));

    // A turn on the older one makes it the most recent again.
    std::thread::sleep(std::time::Duration::from_millis(5));
    vault.append_chat_message(&older, "user", "back to this", None).unwrap();
    let list = vault.list_chat_conversations().unwrap();
    let pos = |id: Uuid| list.iter().position(|c| c.id == id).unwrap();
    assert!(pos(older) < pos(newer), "a new turn revives its conversation");
}

/// Re-saving keeps the conversation's identity and origin time, and only
/// moves what actually changed (a renamed tab, a switched model).
#[test]
fn upsert_updates_metadata_without_duplicating_or_resetting_start() {
    let vault = unlocked_vault();
    let id = conv(&vault, "old name");
    let started = vault
        .list_chat_conversations()
        .unwrap()
        .into_iter()
        .find(|c| c.id == id)
        .unwrap()
        .started_at;

    std::thread::sleep(std::time::Duration::from_millis(5));
    vault
        .upsert_chat_conversation(&id, None, None, "new name", "deepseek", "deepseek-v4-flash")
        .unwrap();

    let list = vault.list_chat_conversations().unwrap();
    assert_eq!(list.iter().filter(|c| c.id == id).count(), 1, "no duplicate row");
    let entry = list.iter().find(|c| c.id == id).unwrap();
    assert_eq!(entry.label, "new name");
    assert_eq!(entry.model, "deepseek-v4-flash");
    assert_eq!(entry.started_at, started, "origin time is preserved");
    assert!(entry.updated_at > started, "recency moved");
}

#[test]
fn deleting_a_conversation_takes_its_turns_with_it() {
    let vault = unlocked_vault();
    let keep = conv(&vault, "keep");
    let drop = conv(&vault, "drop");
    vault.append_chat_message(&keep, "user", "kept", None).unwrap();
    vault.append_chat_message(&drop, "user", "dropped", None).unwrap();

    vault.delete_chat_conversation(&drop).unwrap();

    assert!(vault.chat_messages(&drop).unwrap().is_empty());
    assert_eq!(vault.chat_messages(&keep).unwrap().len(), 1, "neighbour untouched");
    let list = vault.list_chat_conversations().unwrap();
    assert!(list.iter().all(|c| c.id != drop));
}

/// Deleting a host must not leave its conversations dangling against an id
/// that no longer resolves.
#[test]
fn deleting_a_connection_sweeps_only_its_conversations() {
    let vault = unlocked_vault();
    let host_a = Uuid::new_v4();
    let host_b = Uuid::new_v4();
    let a = Uuid::new_v4();
    let b = Uuid::new_v4();
    vault
        .upsert_chat_conversation(&a, Some(&host_a), None, "a", "openai", "gpt-4o")
        .unwrap();
    vault
        .upsert_chat_conversation(&b, Some(&host_b), None, "b", "openai", "gpt-4o")
        .unwrap();
    vault.append_chat_message(&a, "user", "on a", None).unwrap();
    vault.append_chat_message(&b, "user", "on b", None).unwrap();

    vault.delete_chat_conversations_for_connection(&host_a).unwrap();

    let list = vault.list_chat_conversations().unwrap();
    assert!(list.iter().all(|c| c.id != a));
    assert_eq!(list.iter().filter(|c| c.id == b).count(), 1);
    assert!(vault.chat_messages(&a).unwrap().is_empty(), "turns went too");
    assert_eq!(vault.chat_messages(&b).unwrap().len(), 1);
}

#[test]
fn clear_removes_every_conversation_and_turn() {
    let vault = unlocked_vault();
    let a = conv(&vault, "a");
    vault.append_chat_message(&a, "user", "x", None).unwrap();

    vault.clear_chat_conversations().unwrap();

    assert!(vault.list_chat_conversations().unwrap().is_empty());
    assert!(vault.chat_messages(&a).unwrap().is_empty());
}
