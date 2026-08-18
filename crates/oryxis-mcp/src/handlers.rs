use serde_json::{json, Value};
use uuid::Uuid;

use oryxis_ssh::SshEngine;
use oryxis_vault::VaultStore;

/// Host-key verification against the vault's known-host pins, the same
/// rows the app's accept-and-save flow writes. Mirrors
/// `oryxis-app`'s `connect_methods::make_host_key_check`: a different
/// offered algorithm reads as `Unknown` (verify + accept) rather than a
/// "Changed" MITM warning, since the pin is per (host, port, key_type).
///
/// Paired with `with_strict_host_key(true)` at the call site, because
/// this is a headless dialer with no UI to raise a fingerprint prompt.
/// Without both, `ClientHandler::check_server_key` falls through to its
/// legacy arm and returns `Ok(true)` for ANY server key, which hands the
/// stored password and a live TOTP code to whoever answers the dial.
/// Every other dial site in the workspace wires this; this one is the
/// odd one out and the same policy the boot port-forward and SFTP-sync
/// dials already use applies here.
fn make_host_key_check(vault: &VaultStore) -> oryxis_ssh::HostKeyCheckCallback {
    let pinned = vault.list_known_hosts().unwrap_or_default();
    std::sync::Arc::new(move |host, port, key_type, fingerprint| {
        if let Some(existing) = pinned
            .iter()
            .find(|h| h.hostname == host && h.port == port && h.key_type == key_type)
        {
            if existing.fingerprint != fingerprint {
                return oryxis_ssh::HostKeyStatus::Changed {
                    old_fingerprint: existing.fingerprint.clone(),
                };
            }
            return oryxis_ssh::HostKeyStatus::Known;
        }
        oryxis_ssh::HostKeyStatus::Unknown
    })
}

pub fn handle_list_hosts(vault: &VaultStore, params: Option<&Value>) -> Result<Value, String> {
    let conns = vault.list_mcp_connections().map_err(|e| e.to_string())?;

    let group_filter = params
        .and_then(|p| p.get("group_id"))
        .and_then(|v| v.as_str())
        .and_then(|s| Uuid::parse_str(s).ok());

    let tag_filter = params
        .and_then(|p| p.get("tag"))
        .and_then(|v| v.as_str());

    let hosts: Vec<Value> = conns
        .iter()
        .filter(|c| {
            if let Some(gid) = group_filter {
                if c.group_id != Some(gid) {
                    return false;
                }
            }
            if let Some(tag) = tag_filter {
                if !c.tags.iter().any(|t| t == tag) {
                    return false;
                }
            }
            true
        })
        .map(|c| {
            json!({
                "id": c.id.to_string(),
                "label": c.label,
                "hostname": c.hostname,
                "port": c.port,
                "username": c.username,
                "auth_method": format!("{:?}", c.auth_method),
                "group_id": c.group_id.map(|g| g.to_string()),
                "tags": c.tags,
                "notes": c.notes,
                "last_used": c.last_used.map(|d| d.to_rfc3339()),
            })
        })
        .collect();

    Ok(json!(hosts))
}

pub fn handle_list_groups(vault: &VaultStore) -> Result<Value, String> {
    let groups = vault.list_groups().map_err(|e| e.to_string())?;
    let result: Vec<Value> = groups
        .iter()
        .map(|g| {
            json!({
                "id": g.id.to_string(),
                "label": g.label,
                "parent_id": g.parent_id.map(|p| p.to_string()),
                "color": g.color,
                "icon": g.icon,
            })
        })
        .collect();
    Ok(json!(result))
}

pub fn handle_list_keys(vault: &VaultStore) -> Result<Value, String> {
    let keys = vault.list_keys().map_err(|e| e.to_string())?;
    let result: Vec<Value> = keys
        .iter()
        .map(|k| {
            json!({
                "id": k.id.to_string(),
                "label": k.label,
                "fingerprint": k.fingerprint,
                "algorithm": format!("{}", k.algorithm),
                "has_passphrase": k.has_passphrase,
            })
        })
        .collect();
    Ok(json!(result))
}

pub fn handle_get_host(vault: &VaultStore, params: Option<&Value>) -> Result<Value, String> {
    let id_str = params
        .and_then(|p| p.get("id"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| "Missing required parameter: id".to_string())?;

    let id = Uuid::parse_str(id_str).map_err(|_| "Invalid UUID".to_string())?;

    let conns = vault.list_mcp_connections().map_err(|e| e.to_string())?;
    let conn = conns
        .iter()
        .find(|c| c.id == id)
        .ok_or_else(|| "Host not found or not MCP-enabled".to_string())?;

    Ok(json!({
        "id": conn.id.to_string(),
        "label": conn.label,
        "hostname": conn.hostname,
        "port": conn.port,
        "username": conn.username,
        "auth_method": format!("{:?}", conn.auth_method),
        "group_id": conn.group_id.map(|g| g.to_string()),
        "identity_id": conn.identity_id.map(|i| i.to_string()),
        "key_id": conn.key_id.map(|k| k.to_string()),
        "tags": conn.tags,
        "notes": conn.notes,
        "color": conn.color,
        "last_used": conn.last_used.map(|d| d.to_rfc3339()),
        "created_at": conn.created_at.to_rfc3339(),
        "updated_at": conn.updated_at.to_rfc3339(),
    }))
}

pub async fn handle_ssh_execute(
    vault: &VaultStore,
    params: Option<&Value>,
) -> Result<Value, String> {
    let id_str = params
        .and_then(|p| p.get("id"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| "Missing required parameter: id".to_string())?;
    let command = params
        .and_then(|p| p.get("command"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| "Missing required parameter: command".to_string())?;
    let timeout_secs = params
        .and_then(|p| p.get("timeout_secs"))
        .and_then(|v| v.as_u64())
        .unwrap_or(30)
        .min(300);

    let id = Uuid::parse_str(id_str).map_err(|_| "Invalid UUID".to_string())?;

    // Find connection
    let conns = vault.list_mcp_connections().map_err(|e| e.to_string())?;
    let conn = conns
        .iter()
        .find(|c| c.id == id)
        .ok_or_else(|| "Host not found or not MCP-enabled".to_string())?;

    // Group inheritance (D4), the SAME collapse every app dial site
    // applies (`VaultStore::apply_effective`): the effective proxy lands
    // on `conn.proxy`, an inherited username / identity fills the empty
    // fields. Skipping it here is how a headless dial once authenticated
    // differently (as "root") than a tab to the very same host.
    let groups = vault.list_groups().unwrap_or_default();
    let identities = vault.list_identities().unwrap_or_default();
    let mut conn = conn.clone();
    vault.apply_effective(&mut conn, &groups, &identities);
    let conn = &conn;

    // The certificate (B2) is resolved from the SAME key as the pem, so
    // it can never pair with the wrong key.
    let all_keys = vault.list_keys().unwrap_or_default();
    let cert_for = |kid: &uuid::Uuid| -> Option<String> {
        all_keys
            .iter()
            .find(|k| k.id == *kid)
            .and_then(|k| k.certificate.clone())
    };

    // Resolve credentials
    let password = vault.get_connection_password(&conn.id).unwrap_or(None);
    let private_key = conn
        .key_id
        .and_then(|kid| vault.get_key_private(&kid).ok().flatten());
    let conn_cert = conn.key_id.as_ref().and_then(&cert_for);

    // If identity linked (the host's own or a group-inherited one), get
    // identity credentials
    let (ident_password, ident_key, ident_cert) = if let Some(iid) = conn.identity_id {
        let ident_pw = vault.get_identity_password(&iid).unwrap_or(None);
        let ident_key_id = identities.iter().find(|i| i.id == iid).and_then(|i| i.key_id);
        let ident_pk = ident_key_id.and_then(|kid| vault.get_key_private(&kid).ok().flatten());
        let ident_cert = ident_key_id.as_ref().and_then(&cert_for);
        (ident_pw, ident_pk, ident_cert)
    } else {
        (None, None, None)
    };

    let final_password = password.or(ident_password);
    // Certificate follows the key that wins (conn-preferred, same as the
    // key), so the pair never desyncs.
    let (final_key, final_cert) = if private_key.is_some() {
        (private_key, conn_cert)
    } else {
        (ident_key, ident_cert)
    };
    // The disk key fills a still-empty slot, exactly as the app's
    // `resolve_credentials` does: a host that authenticates in the UI
    // must not fail here for want of a key source. Its certificate is
    // the `<key>-cert.pub` sibling, so the pair still describes ONE key.
    let (final_key, final_cert) = match final_key {
        Some(pem) => (Some(pem), final_cert),
        None if matches!(
            conn.auth_method,
            oryxis_core::models::connection::AuthMethod::Key
                | oryxis_core::models::connection::AuthMethod::Auto
                | oryxis_core::models::connection::AuthMethod::Certificate
        ) =>
        {
            match oryxis_vault::resolve_disk_key(conn.use_disk_key, conn.identity_file.as_deref())
                .material()
            {
                Some((pem, disk_cert)) => (Some(pem), disk_cert),
                None => (None, final_cert),
            }
        }
        None => (None, final_cert),
    };
    let username = conn.username.clone().unwrap_or_else(|| "root".into());

    // Build a temporary Connection with resolved username for auth. The
    // effective proxy is already collapsed onto `conn.proxy` by
    // `apply_effective` above (re-resolving here would overwrite a
    // group-inherited proxy with the host's own).
    let mut auth_conn = conn.clone();
    auth_conn.username = Some(username);

    // Build engine and connect. Honor any per-host legacy-algorithm
    // overrides the user pinned in the app (MCP is headless, so there is
    // no interactive fallback dialog, only the pinned settings apply).
    // The stored TOTP secret rides along for the same reason: an
    // OTP-gated host is unreachable headlessly without the autofill.
    let totp_secret = vault
        .get_connection_totp_secret(&conn.id)
        .ok()
        .flatten();
    // Agent-auth pin (B3): the referenced key's public line (connection
    // key preferred, then the identity's), offered first when the Auto
    // ladder reaches agent auth.
    let pinned_agent = conn
        .key_id
        .or_else(|| {
            conn.identity_id.and_then(|iid| {
                vault
                    .list_identities()
                    .unwrap_or_default()
                    .iter()
                    .find(|i| i.id == iid)
                    .and_then(|i| i.key_id)
            })
        })
        .and_then(|kid| all_keys.iter().find(|k| k.id == kid))
        .map(|k| k.public_key.clone())
        .filter(|p| !p.trim().is_empty());
    // Command-proxy approval, resolved from the vault's own list and
    // answered with no UI in the loop, because there is none here.
    // Same authority and same helper the app's unattended dials use, so
    // a host that runs over MCP is exactly a host the user approved in
    // the app, never one a sync peer wrote into the vault.
    let trusted_proxy_commands = vault
        .list_trusted_proxy_commands()
        .map(|list| list.into_iter().map(|t| t.fingerprint).collect())
        .unwrap_or_default();
    let engine = SshEngine::new()
        // Verify the server key against the vault's pins and reject
        // unknown/changed ones: there is no terminal here to surface a
        // fingerprint prompt, so a host reached over MCP must already
        // have been trusted interactively in the app.
        .with_host_key_check(make_host_key_check(vault))
        .with_strict_host_key(true)
        .with_proxy_command_ask(oryxis_ssh::trusted_only_proxy_command_ask(
            trusted_proxy_commands,
        ))
        .with_totp_secret(totp_secret.as_deref())
        .with_address_family(auth_conn.address_family)
        .with_pinned_agent_key(pinned_agent.as_deref())
        .with_algorithm_overrides(
            auth_conn.ciphers.clone(),
            auth_conn.kex.clone(),
            auth_conn.macs.clone(),
            auth_conn.host_key_algorithms.clone(),
        );

    let mut handle = engine
        .establish_transport(&auth_conn, None)
        .await
        .map_err(|e| format!("Connection failed: {}", e))?;

    engine
        .do_authenticate(
            &mut handle,
            &auth_conn,
            final_password.as_deref(),
            final_key
                .as_deref()
                .map(|pem| oryxis_ssh::KeyMaterial::new(pem, final_cert.as_deref())),
        )
        .await
        .map_err(|e| format!("Authentication failed: {}", e))?;

    let timeout = std::time::Duration::from_secs(timeout_secs);
    let result = engine
        .exec_command(handle, command, timeout)
        .await
        .map_err(|e| format!("Execution failed: {}", e))?;

    Ok(json!({
        "exit_code": result.exit_code,
        "stdout": result.stdout,
        "stderr": result.stderr,
    }))
}
