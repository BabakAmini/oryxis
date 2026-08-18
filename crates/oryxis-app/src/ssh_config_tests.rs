//! Tests for the ssh_config parser. Covers the syntactic shapes we
//! actually see in real configs (per-line, indented, `=` separator,
//! quoted values, wildcard hosts, comments).

use super::*;

#[test]
fn parses_simple_host_block() {
    let input = "
Host bastion
    HostName bastion.example.com
    User admin
    Port 2222
    IdentityFile ~/.ssh/id_ed25519
";
    let hosts = parse(input);
    assert_eq!(hosts.len(), 1);
    let h = &hosts[0];
    assert_eq!(h.alias, "bastion");
    assert_eq!(h.hostname.as_deref(), Some("bastion.example.com"));
    assert_eq!(h.port, Some(2222));
    assert_eq!(h.user.as_deref(), Some("admin"));
    assert_eq!(h.identity_files.len(), 1);
}

#[test]
fn skips_wildcard_hosts() {
    // The `Host *` block is the global default in real configs
    // never importable as a concrete server.
    let input = "
Host *
    ServerAliveInterval 30

Host real
    HostName 10.0.0.1
";
    let hosts = parse(input);
    assert_eq!(hosts.len(), 1);
    assert_eq!(hosts[0].alias, "real");
}

#[test]
fn supports_equals_separator_and_quoted_values() {
    let input = "
Host quoted
    HostName=\"db.example.com\"
    Port=3306
";
    let hosts = parse(input);
    assert_eq!(hosts.len(), 1);
    assert_eq!(hosts[0].hostname.as_deref(), Some("db.example.com"));
    assert_eq!(hosts[0].port, Some(3306));
}

#[test]
fn ignores_comments_and_blanks() {
    let input = "
# this is a comment
Host alpha
    # inline-style
    HostName alpha.local

Host beta
    HostName beta.local
";
    let hosts = parse(input);
    assert_eq!(hosts.len(), 2);
    assert_eq!(hosts[0].alias, "alpha");
    assert_eq!(hosts[1].alias, "beta");
}

#[test]
fn captures_proxy_jump_chain() {
    let input = "
Host inner
    HostName 10.1.1.1
    ProxyJump bastion
";
    let hosts = parse(input);
    assert_eq!(hosts[0].proxy_jump.as_deref(), Some("bastion"));
}

#[test]
fn proxy_jump_takes_only_first_hop() {
    // OpenSSH allows comma-separated chains; for v1 import we keep
    // only the first hop so alias linking stays 1-1.
    let input = "
Host inner
    HostName 10.1.1.1
    ProxyJump alpha,beta,gamma
";
    let hosts = parse(input);
    assert_eq!(hosts[0].proxy_jump.as_deref(), Some("alpha"));
}

#[test]
fn captures_proxy_command_verbatim() {
    // Placeholders like %h / %p must be preserved verbatim, they're
    // expanded by the user's shell at connect time.
    let input = "
Host tunneled
    HostName 10.0.0.5
    ProxyCommand ssh -W %h:%p bastion
";
    let hosts = parse(input);
    assert_eq!(
        hosts[0].proxy_command.as_deref(),
        Some("ssh -W %h:%p bastion")
    );
}

#[test]
fn address_family_directive_maps_to_the_preference() {
    use oryxis_core::models::connection::AddressFamily;
    let input = "
Host v4only
    HostName a.example.com
    AddressFamily inet
Host v6only
    HostName b.example.com
    AddressFamily inet6
Host anyhost
    HostName c.example.com
    AddressFamily any
Host unset
    HostName d.example.com
";
    let hosts = parse(input);
    assert_eq!(hosts.len(), 4);
    assert_eq!(to_connection(&hosts[0]).address_family, AddressFamily::V4);
    assert_eq!(to_connection(&hosts[1]).address_family, AddressFamily::V6);
    assert_eq!(to_connection(&hosts[2]).address_family, AddressFamily::Auto);
    assert_eq!(to_connection(&hosts[3]).address_family, AddressFamily::Auto);
}

#[test]
fn proxy_command_maps_to_command_proxy_type() {
    use oryxis_core::models::connection::ProxyType;
    let host = SshConfigHost {
        alias: "h".into(),
        hostname: Some("h.example.com".into()),
        port: None,
        user: None,
        identity_files: Vec::new(),
        proxy_jump: None,
        proxy_command: Some("nc -X connect -x corp:8080 %h %p".into()),
        forward_agent: false,
        forward_x11: false,
        address_family: Default::default(),
    };
    let conn = to_connection(&host);
    let proxy = conn.proxy.expect("proxy_command should produce inline proxy");
    match proxy.proxy_type {
        ProxyType::Command(cmd) => {
            assert_eq!(cmd, "nc -X connect -x corp:8080 %h %p");
        }
        other => panic!("expected ProxyType::Command, got {:?}", other),
    }
}

#[test]
fn link_proxy_jumps_resolves_alias_to_id() {
    let input = "
Host bastion
    HostName 198.51.100.1

Host inner
    HostName 10.0.0.5
    ProxyJump bastion
";
    let parsed = parse(input);
    let mut conns: Vec<_> = parsed.iter().map(to_connection).collect();
    link_proxy_jumps(&parsed, &mut conns);

    let bastion_id = conns.iter().find(|c| c.label == "bastion").unwrap().id;
    let inner = conns.iter().find(|c| c.label == "inner").unwrap();
    assert_eq!(inner.jump_chain, vec![bastion_id]);
}

#[test]
fn link_proxy_jumps_records_unresolved_alias_in_notes() {
    let input = "
Host orphan
    HostName 10.0.0.5
    ProxyJump ghost
";
    let parsed = parse(input);
    let mut conns: Vec<_> = parsed.iter().map(to_connection).collect();
    link_proxy_jumps(&parsed, &mut conns);

    let orphan = &conns[0];
    assert!(orphan.jump_chain.is_empty());
    let notes = orphan.notes.as_deref().unwrap_or("");
    assert!(
        notes.contains("ghost") && notes.contains("not resolved"),
        "notes should flag unresolved alias: {notes:?}"
    );
}

#[test]
fn captures_forward_agent_only_when_yes() {
    // OpenSSH defaults to `ForwardAgent no`; only an explicit `yes`
    // flips agent forwarding on, so the parser should treat any other
    // value (or its absence) as off.
    let input = "
Host with-fwd
    HostName fwd.example.com
    ForwardAgent yes

Host without-fwd
    HostName plain.example.com

Host explicit-no
    HostName off.example.com
    ForwardAgent no
";
    let hosts = parse(input);
    let by_alias = |a: &str| hosts.iter().find(|h| h.alias == a).unwrap();
    assert!(by_alias("with-fwd").forward_agent);
    assert!(!by_alias("without-fwd").forward_agent);
    assert!(!by_alias("explicit-no").forward_agent);
    // Round-trip onto Connection, flag should propagate.
    let conn = to_connection(by_alias("with-fwd"));
    assert!(conn.agent_forwarding);
}

#[test]
fn imports_forward_x11_directives() {
    // Both spellings enable it; we only implement trusted forwarding,
    // so plain `ForwardX11` maps onto the same flag.
    let input = "
Host plain
    HostName a.example.com
    ForwardX11 yes

Host trusted
    HostName b.example.com
    ForwardX11Trusted yes

Host mixed
    HostName c.example.com
    ForwardX11 yes
    ForwardX11Trusted no

Host off
    HostName d.example.com
    ForwardX11 no

Host silent
    HostName e.example.com
";
    let hosts = parse(input);
    let by_alias = |a: &str| hosts.iter().find(|h| h.alias == a).unwrap();
    assert!(by_alias("plain").forward_x11);
    assert!(by_alias("trusted").forward_x11);
    // `|=` semantics: an explicit `no` on the OTHER directive must not
    // cancel a `yes`, in either order.
    assert!(by_alias("mixed").forward_x11);
    assert!(!by_alias("off").forward_x11);
    assert!(!by_alias("silent").forward_x11);
    // Round-trip onto Connection, flag should propagate.
    assert!(to_connection(by_alias("plain")).x11_forwarding);
    assert!(!to_connection(by_alias("silent")).x11_forwarding);
}

#[test]
fn first_alias_wins_for_multi_alias_host() {
    // `Host a b c`, pick the first as the canonical name.
    let input = "
Host primary alt1 alt2
    HostName primary.example.com
";
    let hosts = parse(input);
    assert_eq!(hosts[0].alias, "primary");
}

#[test]
fn to_connection_maps_fields() {
    let input = "
Host srv
    HostName srv.local
    User wilson
    Port 22
    IdentityFile ~/.ssh/id_ed25519
";
    let hosts = parse(input);
    let conn = to_connection(&hosts[0]);
    assert_eq!(conn.label, "srv");
    assert_eq!(conn.hostname, "srv.local");
    assert_eq!(conn.username.as_deref(), Some("wilson"));
    assert_eq!(conn.port, 22);
    assert_eq!(conn.auth_method, oryxis_core::models::connection::AuthMethod::Key);
    // Notes carries the import provenance.
    assert!(conn.notes.as_deref().unwrap_or("").contains("ssh_config"));
}

#[test]
fn an_identity_file_becomes_the_hosts_disk_key() {
    let input = "
Host srv
    HostName srv.local
    IdentityFile ~/.ssh/work_ed25519
";
    let conn = to_connection(&parse(input)[0]);
    // The whole point of carrying the path: before this, the import
    // flagged the host as Key and dropped the file, so it arrived
    // unconnectable with nothing on it to explain why.
    assert!(conn.use_disk_key);
    let path = conn.identity_file.expect("identity file carried over");
    assert!(path.ends_with("work_ed25519"), "{path}");
    // Expanded, not the literal `~/...`, so it resolves the same from
    // any working directory.
    assert!(!path.starts_with('~'), "{path}");
}

#[test]
fn multiple_identity_files_keep_the_first_and_record_the_rest() {
    let input = "
Host srv
    HostName srv.local
    IdentityFile ~/.ssh/first_key
    IdentityFile ~/.ssh/second_key
";
    let hosts = parse(input);
    // OpenSSH accumulates the directive; a single field would have kept
    // `second_key` and quietly changed which key the host offers.
    assert_eq!(hosts[0].identity_files.len(), 2);
    let conn = to_connection(&hosts[0]);
    assert!(conn.identity_file.as_deref().unwrap_or("").ends_with("first_key"));
    let notes = conn.notes.unwrap_or_default();
    assert!(notes.contains("second_key"), "{notes}");
    assert!(!notes.contains("first_key"), "{notes}");
}

#[test]
fn a_host_without_an_identity_file_leaves_the_disk_source_off() {
    let input = "
Host srv
    HostName srv.local
";
    let conn = to_connection(&parse(input)[0]);
    // Importing a config is not consent to offer whatever sits in
    // `~/.ssh` to a host whose block never mentioned a key.
    assert!(!conn.use_disk_key);
    assert!(conn.identity_file.is_none());
}

#[test]
fn to_connection_falls_back_to_alias_when_no_hostname() {
    let input = "
Host bare
    User root
";
    let hosts = parse(input);
    let conn = to_connection(&hosts[0]);
    // No HostName given → use alias as the address. Common for short
    // SSH config aliases that happen to be valid hostnames already.
    assert_eq!(conn.hostname, "bare");
    assert_eq!(conn.auth_method, oryxis_core::models::connection::AuthMethod::Auto);
}

#[test]
fn handles_indentation_variants() {
    // SSH config tolerates tab-indented, no-indent, mixed, parser
    // should match.
    let input = "Host noindent\nHostName x.local\n\nHost tabindent\n\tHostName y.local\n";
    let hosts = parse(input);
    assert_eq!(hosts.len(), 2);
    assert_eq!(hosts[0].hostname.as_deref(), Some("x.local"));
    assert_eq!(hosts[1].hostname.as_deref(), Some("y.local"));
}

// ---------------------------------------------------------------------------
// Property-based tests
// ---------------------------------------------------------------------------

use proptest::prelude::*;

proptest! {
    #[test]
    fn prop_parse_never_panics(input in ".{0,500}") {
        // Pure smoke, random input should never crash the parser.
        // Real ssh_config files are user-edited and we get whatever
        // they wrote.
        let _ = parse(&input);
    }

    #[test]
    fn prop_parse_block_count_matches_host_lines(
        // Generate N "Host alias\n" prefix lines, no body. Parser
        // should report exactly N concrete hosts (none wildcarded).
        aliases in proptest::collection::vec("[a-z][a-z0-9-]{0,15}", 1..8),
    ) {
        let mut input = String::new();
        for alias in &aliases {
            input.push_str(&format!("Host {}\n", alias));
        }
        let hosts = parse(&input);
        prop_assert_eq!(hosts.len(), aliases.len());
        for (i, alias) in aliases.iter().enumerate() {
            prop_assert_eq!(&hosts[i].alias, alias);
        }
    }

    #[test]
    fn prop_wildcard_aliases_always_skipped(
        suffix in "[a-z][a-z0-9-]{0,10}",
    ) {
        let inputs = [
            "Host *\n  HostName x\n".to_string(),
            format!("Host *.{}\n  HostName x\n", suffix),
            format!("Host {}?\n  HostName x\n", suffix),
        ];
        for input in inputs {
            let hosts = parse(&input);
            prop_assert!(hosts.is_empty(), "expected wildcard to be skipped: {input}");
        }
    }

    #[test]
    fn prop_to_connection_label_matches_alias(
        alias in "[a-z][a-zA-Z0-9_.-]{0,30}",
        port in 1u16..65535,
    ) {
        let host = SshConfigHost {
            alias: alias.clone(),
            hostname: Some("h.example.com".into()),
            port: Some(port),
            user: Some("u".into()),
            identity_files: Vec::new(),
            proxy_jump: None,
            proxy_command: None,
            forward_agent: false,
            forward_x11: false,
            address_family: Default::default(),
        };
        let conn = to_connection(&host);
        // Invariant: label always carries the alias verbatim, it's
        // the user-facing identifier.
        prop_assert_eq!(conn.label, alias);
        prop_assert_eq!(conn.port, port);
    }
}
