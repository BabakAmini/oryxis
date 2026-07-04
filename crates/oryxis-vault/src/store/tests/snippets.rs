use super::*;

#[test]
fn save_and_list_snippets() {
    let vault = unlocked_vault();
    let s = Snippet::new("restart-nginx", "sudo systemctl restart nginx");
    vault.save_snippet(&s).unwrap();

    let snippets = vault.list_snippets().unwrap();
    assert_eq!(snippets.len(), 1);
    assert_eq!(snippets[0].command, "sudo systemctl restart nginx");
}


#[test]
fn delete_snippet() {
    let vault = unlocked_vault();
    let s = Snippet::new("temp", "echo hi");
    vault.save_snippet(&s).unwrap();
    vault.delete_snippet(&s.id).unwrap();
    assert_eq!(vault.list_snippets().unwrap().len(), 0);
}

// ── Known Hosts ──


#[test]
fn snippet_has_updated_at() {
    let vault = unlocked_vault();
    let s = Snippet::new("test", "echo hi");
    assert!(s.updated_at.timestamp() > 0);
    vault.save_snippet(&s).unwrap();

    let snippets = vault.list_snippets().unwrap();
    assert_eq!(snippets.len(), 1);
    assert!(snippets[0].updated_at.timestamp() > 0);
}


#[test]
fn snippet_group_and_tags_roundtrip() {
    let vault = unlocked_vault();
    let mut s = Snippet::new("deploy", "make deploy");
    s.group = Some("devops".to_string());
    s.tags = vec!["prod".to_string(), "web".to_string()];
    vault.save_snippet(&s).unwrap();

    let back = &vault.list_snippets().unwrap()[0];
    assert_eq!(back.group.as_deref(), Some("devops"));
    assert_eq!(back.tags, vec!["prod", "web"]);

    // Clearing the group persists as NULL, not an empty string.
    let mut s2 = back.clone();
    s2.group = None;
    vault.save_snippet(&s2).unwrap();
    assert_eq!(vault.list_snippets().unwrap()[0].group, None);
}
