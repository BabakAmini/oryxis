//! `Oryxis::handle_port_forwards`, match arms for the standalone port
//! forward entity: CRUD on `PortForwardRule`, and the runtime on/off
//! toggle that opens / tears down a dedicated PTY-less SSH session.
//!
//! Kept separate from `dispatch_ssh.rs` (terminal sessions) so the two
//! lifecycles don't tangle. A forward holds its connection open with no
//! shell; turning it off drops the `ForwardSession`, which cancels the
//! tunnel.

// Domain handlers return `Err(Message)` to pass an unclaimed message
// back up the chain. See the note in `dispatch_ssh.rs`.
#![allow(clippy::result_large_err)]

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use iced::futures::SinkExt;
use iced::Task;
use uuid::Uuid;

use oryxis_core::models::connection::{AuthMethod, Connection};
use oryxis_core::models::port_forward_rule::PortForwardRule;
use oryxis_ssh::{ConnectionResolver, ForwardSession, HostKeyQuery, SshEngine};

use crate::app::{SshMessage, PortForwardMessage, Message, Oryxis};

/// Items streamed out of an interactive (manual-toggle) forward connect:
/// either a host-key question for the UI modal, or the final result.
enum PfStreamMsg {
    HostKey(HostKeyQuery),
    Done(Result<Arc<ForwardSession>, String>),
    NoCommonAlgo {
        category: oryxis_ssh::NegCategory,
        server_offers: Vec<String>,
    },
}

/// Retry bookkeeping for an `auto_start` forward that is down. `next_at` is
/// the earliest wall-clock instant to re-attempt; `attempts` is how many
/// re-attempts have been issued so far, driving the backoff. The attempt
/// count is never capped, only the interval is: an `auto_start` forward is
/// meant to stay up, so it keeps trying (cheaply) until the key/network
/// comes back, rather than giving up like the SSH-tab reconnect does.
#[derive(Debug, Clone)]
pub(crate) struct PfRetry {
    pub next_at: Instant,
    pub attempts: u32,
}

/// Backoff for the Nth retry: 15s, 30s, 60s, then a 120s ceiling. Cheap
/// enough to poll a dead endpoint indefinitely (≤ ~720 attempts/day) yet
/// snappy enough that a forward comes up seconds after its key lands.
fn pf_retry_backoff(attempts: u32) -> Duration {
    let secs = 15u64.saturating_mul(1u64 << attempts.min(3));
    Duration::from_secs(secs.min(120))
}

impl Oryxis {
    pub(crate) fn handle_port_forwards(
        &mut self,
        message: PortForwardMessage,
    ) -> Task<Message> {
        match message {
            // -- Editor panel --
            PortForwardMessage::ShowPortForwardPanel => {
                self.overlay = None;
                self.show_port_forward_panel = true;
                self.port_forward_form.editing_id = None;
                self.port_forward_form.label.clear();
                self.port_forward_form.kind = oryxis_core::models::port_forward_rule::ForwardKind::Local;
                // Default the host to the first connection so the picker
                // isn't empty on a fresh rule.
                self.port_forward_form.host_id = self.connections.first().map(|c| c.id);
                self.port_forward_form.listen_host = "127.0.0.1".into();
                self.port_forward_form.listen_port.clear();
                self.port_forward_form.target_host.clear();
                self.port_forward_form.target_port.clear();
                self.port_forward_form.auto_start = false;
                self.port_forward_form.error = None;
            }
            PortForwardMessage::HidePortForwardPanel => {
                self.show_port_forward_panel = false;
            }
            PortForwardMessage::PfLabelChanged(v) => self.port_forward_form.label = v,
            PortForwardMessage::PfKindChanged(k) => self.port_forward_form.kind = k,
            PortForwardMessage::PfHostChanged(id) => self.port_forward_form.host_id = Some(id),
            PortForwardMessage::PfListenHostChanged(v) => self.port_forward_form.listen_host = v,
            PortForwardMessage::PfListenPortChanged(v) => {
                self.port_forward_form.listen_port = v.chars().filter(|c| c.is_ascii_digit()).collect();
            }
            PortForwardMessage::PfTargetHostChanged(v) => self.port_forward_form.target_host = v,
            PortForwardMessage::PfTargetPortChanged(v) => {
                self.port_forward_form.target_port = v.chars().filter(|c| c.is_ascii_digit()).collect();
            }
            PortForwardMessage::PfAutoStartToggled(v) => self.port_forward_form.auto_start = v,
            PortForwardMessage::EditPortForwardRule(idx) => {
                if let Some(rule) = self.port_forward_rules.get(idx) {
                    self.show_port_forward_panel = true;
                    self.port_forward_form.editing_id = Some(rule.id);
                    self.port_forward_form.label = rule.label.clone();
                    self.port_forward_form.kind = rule.kind;
                    self.port_forward_form.host_id = Some(rule.host_id);
                    self.port_forward_form.listen_host = rule.listen_host.clone();
                    self.port_forward_form.listen_port = rule.listen_port.to_string();
                    self.port_forward_form.target_host = rule.target_host.clone();
                    self.port_forward_form.target_port = rule.target_port.to_string();
                    self.port_forward_form.auto_start = rule.auto_start;
                    self.port_forward_form.error = None;
                }
            }
            PortForwardMessage::SavePortForwardRule => {
                if let Some(err) = self.save_port_forward_rule() {
                    self.port_forward_form.error = Some(err);
                } else {
                    self.show_port_forward_panel = false;
                    self.port_forward_form.error = None;
                    self.load_data_from_vault();
                }
            }
            PortForwardMessage::DeletePortForwardRule(idx) => {
                if let Some(rule) = self.port_forward_rules.get(idx) {
                    let id = rule.id;
                    // Tear down a live forward before the rule disappears.
                    self.active_forwards.remove(&id);
                    self.port_forward_starting.remove(&id);
                    self.port_forward_retry.remove(&id);
                    if let Some(vault) = &self.vault {
                        let _ = vault.delete_port_forward_rule(&id);
                        self.show_port_forward_panel = false;
                        self.load_data_from_vault();
                    }
                }
            }

            // -- Runtime on/off --
            PortForwardMessage::StartPortForward(id) => {
                return self.start_port_forward(id, false);
            }
            PortForwardMessage::StopPortForward(id) => {
                self.port_forward_starting.remove(&id);
                // The user turned it off: stop any self-healing retry so an
                // auto_start rule the user explicitly stopped never
                // resurrects on the next tick.
                self.port_forward_retry.remove(&id);
                // Await `cancel()` so a remote (`-R`) forward also releases
                // its server-side listener via `cancel_tcpip_forward`, not
                // just the local tasks that Drop would stop. Dropping the
                // last `Arc` afterwards tears the rest down.
                if let Some(session) = self.active_forwards.remove(&id) {
                    return Task::perform(
                        async move { session.cancel().await },
                        |_| Message::PortForward(PortForwardMessage::PortForwardLivenessTick),
                    );
                }
            }
            PortForwardMessage::PortForwardStarted(id, res) => {
                // `remove` returns false when StopPortForward already pulled
                // this id from the in-flight set, i.e. the user toggled the
                // rule off while the connect was still running. In that case
                // honor the stop and drop the freshly-made session rather than
                // silently re-activating a forward the user turned off.
                let was_starting = self.port_forward_starting.remove(&id);
                match res {
                    Ok(session) => {
                        // Guard against a delete or stop that landed while the
                        // connect was in flight: if the rule is gone, or a stop
                        // was requested, drop the session so it doesn't linger
                        // with no UI to stop (or against the user's intent).
                        if was_starting && self.port_forward_rules.iter().any(|r| r.id == id) {
                            self.active_forwards.insert(id, session);
                            // Came up: clear any retry so a later drop starts
                            // the backoff fresh from the shortest interval.
                            self.port_forward_retry.remove(&id);
                            self.port_forward_form.error = None;
                        } else {
                            drop(session);
                        }
                    }
                    Err(e) => {
                        // First/foreground failure surfaces the error. An
                        // auto_start rule additionally enters the retry loop
                        // so a transient failure (SSH key not loaded yet,
                        // network down) self-heals instead of staying dead.
                        let already_retrying = self.port_forward_retry.contains_key(&id);
                        self.pf_mark_retry_pending(id);
                        // Stay silent on background retries: the amber
                        // "Retrying…" chip already carries the signal, and the
                        // single shared error field would otherwise clobber
                        // across rows on every tick.
                        if !already_retrying {
                            self.port_forward_form.error = Some(e);
                        }
                    }
                }
            }
            PortForwardMessage::PortForwardLivenessTick => {
                // Drop forwards whose underlying connection has died so the
                // per-row toggle reflects reality instead of lying "on".
                let dead: Vec<Uuid> = self
                    .active_forwards
                    .iter()
                    .filter(|(_, s)| !s.is_alive())
                    .map(|(id, _)| *id)
                    .collect();
                for id in dead {
                    self.active_forwards.remove(&id);
                    // An auto_start forward that dropped should climb back
                    // up on its own (network loss / server closed the
                    // connection); a manual one just goes off.
                    self.pf_mark_retry_pending(id);
                    tracing::info!("port forward {id} connection dropped, toggled off");
                }
            }
            PortForwardMessage::PortForwardRetryTick => {
                return self.handle_port_forward_retry_tick();
            }
            PortForwardMessage::PortForwardCardHovered(idx) => {
                self.hovered_port_forward_card = Some(idx);
            }
            PortForwardMessage::PortForwardCardUnhovered => {
                self.hovered_port_forward_card = None;
            }
            PortForwardMessage::PortForwardSearchChanged(v) => self.port_forward_search = v,
        }
        Task::none()
    }

    /// Validate the editor draft and persist it. Returns `Some(error)` on
    /// a validation failure (left in the panel), `None` on success.
    fn save_port_forward_rule(&mut self) -> Option<String> {
        let label = self.port_forward_form.label.trim();
        if label.is_empty() {
            return Some(crate::i18n::t("pf_err_required").to_string());
        }
        let Some(host_id) = self.port_forward_form.host_id else {
            return Some(crate::i18n::t("pf_err_host").to_string());
        };
        if !self.connections.iter().any(|c| c.id == host_id) {
            return Some(crate::i18n::t("pf_err_host").to_string());
        }
        let Some(listen_port) = parse_port(&self.port_forward_form.listen_port) else {
            return Some(crate::i18n::t("pf_err_port").to_string());
        };
        let (target_host, target_port) = if self.port_forward_form.kind.has_target() {
            let th = self.port_forward_form.target_host.trim();
            if th.is_empty() {
                return Some(crate::i18n::t("pf_err_required").to_string());
            }
            let Some(tp) = parse_port(&self.port_forward_form.target_port) else {
                return Some(crate::i18n::t("pf_err_port").to_string());
            };
            (th.to_string(), tp)
        } else {
            (String::new(), 0)
        };

        let mut rule = if let Some(id) = self.port_forward_form.editing_id {
            self.port_forward_rules
                .iter()
                .find(|r| r.id == id)
                .cloned()
                .unwrap_or_else(|| PortForwardRule::new("", self.port_forward_form.kind, host_id))
        } else {
            PortForwardRule::new("", self.port_forward_form.kind, host_id)
        };
        rule.label = label.to_string();
        rule.kind = self.port_forward_form.kind;
        rule.host_id = host_id;
        rule.listen_host = self.port_forward_form.listen_host.trim().to_string();
        rule.listen_port = listen_port;
        rule.target_host = target_host;
        rule.target_port = target_port;
        rule.auto_start = self.port_forward_form.auto_start;
        rule.updated_at = chrono::Utc::now();

        let vault = self.vault.as_ref()?;
        match vault.save_port_forward_rule(&rule) {
            Ok(()) => None,
            Err(e) => Some(e.to_string()),
        }
    }

    /// Open a dedicated PTY-less SSH session for the rule and bind its
    /// listener.
    ///
    /// Host-key policy splits on `boot_auto_start`: a boot/unlock auto-start
    /// runs **known-only** (strict, silent), so a host whose key isn't
    /// already trusted just fails to off instead of popping a modal storm
    /// before the window is even ready. A manual toggle, by contrast, wires
    /// the same host-key prompt the terminal uses, so the user can trust a
    /// new key on the spot.
    pub(crate) fn start_port_forward(&mut self, id: Uuid, boot_auto_start: bool) -> Task<Message> {
        if self.active_forwards.contains_key(&id) || self.port_forward_starting.contains(&id) {
            return Task::none();
        }
        let Some(rule) = self.port_forward_rules.iter().find(|r| r.id == id).cloned() else {
            return Task::none();
        };
        let Some(mut conn) = self
            .connections
            .iter()
            .find(|c| c.id == rule.host_id)
            .cloned()
        else {
            self.port_forward_form.error = Some(crate::i18n::t("pf_err_host").to_string());
            return Task::none();
        };

        // Resolve the effective proxy onto `conn.proxy` (engine reads only
        // that field), mirroring the terminal connect path.
        if let Some(vault) = self.vault.as_ref() {
            conn.proxy = vault.resolve_proxy(&conn).ok().flatten();
        }
        let (password, private_key, certificate) = self.resolve_forward_credentials(&conn);
        // Agent-auth pin (B3), same rule as the tab connect.
        let pinned_agent = self.pinned_agent_public(&conn);
        let totp_secret = self
            .vault
            .as_ref()
            .and_then(|v| v.get_connection_totp_secret(&conn.id).ok().flatten());
        let resolver = self.build_jump_resolver(&conn);
        let host_key_check = self.build_host_key_check();
        let keepalive = self.effective_keepalive(&conn);
        self.port_forward_starting.insert(id);

        if boot_auto_start {
            tracing::info!("auto-starting port forward {} ({})", rule.label, id);
            return Task::perform(
                async move {
                    let engine = SshEngine::new()
                        .with_host_key_check(host_key_check)
                        .with_strict_host_key(true)
                        .with_totp_secret(totp_secret.as_deref())
                        .with_keepalive(keepalive)
                        .with_address_family(conn.address_family)
                        .with_rekey_limit_mb(conn.rekey_limit_mb)
                        .with_pinned_agent_key(pinned_agent.as_deref())
                        .with_algorithm_overrides(
                            conn.ciphers.clone(),
                            conn.kex.clone(),
                            conn.macs.clone(),
                            conn.host_key_algorithms.clone(),
                        );
                    engine
                        .connect_forward(
                            &conn,
                            password.as_deref(),
                            private_key
                                .as_deref()
                                .map(|pem| oryxis_ssh::KeyMaterial::new(pem, certificate.as_deref())),
                            &rule,
                            resolver.as_ref(),
                        )
                        .await
                        .map(Arc::new)
                        .map_err(|e| e.to_string())
                },
                move |res| Message::PortForward(PortForwardMessage::PortForwardStarted(id, res)),
            );
        }

        // Manual toggle: reuse the terminal's host-key ask machinery. The
        // engine sends unknown/changed keys to `hk_ask`; the bridge forwards
        // them to the shared host-key modal and waits for the user's answer
        // on `hk_resp` (driven by the existing SshHostKey* handlers).
        let (hk_ask_tx, mut hk_ask_rx) = tokio::sync::mpsc::channel::<(
            HostKeyQuery,
            tokio::sync::oneshot::Sender<bool>,
        )>(1);
        let (hk_resp_tx, mut hk_resp_rx) = tokio::sync::mpsc::channel::<bool>(1);
        self.host_key_response_tx = Some(hk_resp_tx);

        // Captured for the map closure (conn moves into the producer); the
        // retry re-runs this same port-forward start.
        let pf_conn_id = conn.id;
        let stream = iced::stream::channel::<PfStreamMsg>(8, move |mut sender: iced::futures::channel::mpsc::Sender<PfStreamMsg>| async move {
            let engine = SshEngine::new()
                .with_host_key_check(host_key_check)
                .with_host_key_ask(hk_ask_tx)
                .with_totp_secret(totp_secret.as_deref())
                .with_keepalive(keepalive)
                .with_address_family(conn.address_family)
                .with_rekey_limit_mb(conn.rekey_limit_mb)
                .with_pinned_agent_key(pinned_agent.as_deref())
                .with_algorithm_overrides(
                    conn.ciphers.clone(),
                    conn.kex.clone(),
                    conn.macs.clone(),
                    conn.host_key_algorithms.clone(),
                );

            let mut sender_clone = sender.clone();
            let _bridge = tokio::spawn(async move {
                while let Some((query, resp_tx)) = hk_ask_rx.recv().await {
                    let _ = sender_clone.send(PfStreamMsg::HostKey(query)).await;
                    let accepted = hk_resp_rx.recv().await.unwrap_or(false);
                    let _ = resp_tx.send(accepted);
                }
            });

            match engine
                .connect_forward(
                    &conn,
                    password.as_deref(),
                    private_key
                        .as_deref()
                        .map(|pem| oryxis_ssh::KeyMaterial::new(pem, certificate.as_deref())),
                    &rule,
                    resolver.as_ref(),
                )
                .await
            {
                Ok(session) => {
                    let _ = sender.send(PfStreamMsg::Done(Ok(Arc::new(session)))).await;
                }
                Err(e) => {
                    if let Some(nf) = e.negotiation_failure() {
                        let _ = sender
                            .send(PfStreamMsg::NoCommonAlgo {
                                category: nf.category,
                                server_offers: nf.server_offers,
                            })
                            .await;
                    } else {
                        let _ = sender.send(PfStreamMsg::Done(Err(e.to_string()))).await;
                    }
                }
            }
        });

        Task::stream(stream).map(move |m| match m {
            PfStreamMsg::HostKey(q) => Message::Ssh(SshMessage::SshHostKeyVerify(q)),
            PfStreamMsg::Done(r) => Message::PortForward(PortForwardMessage::PortForwardStarted(id, r)),
            PfStreamMsg::NoCommonAlgo { category, server_offers } => Message::Ssh(SshMessage::SshNoCommonAlgo {
                conn_id: pf_conn_id,
                category,
                server_offers,
                retry: Box::new(Message::PortForward(PortForwardMessage::StartPortForward(id))),
            }),
        })
    }

    /// Start every rule marked `auto_start`. Called once after the vault is
    /// unlocked (boot or `VaultUnlock`). Returns the connect tasks to batch
    /// into the caller's task list.
    pub(crate) fn auto_start_port_forwards(&mut self) -> Vec<Task<Message>> {
        let ids: Vec<Uuid> = self
            .port_forward_rules
            .iter()
            .filter(|r| r.auto_start)
            .map(|r| r.id)
            .collect();
        ids.into_iter()
            .map(|id| self.start_port_forward(id, true))
            .collect()
    }

    /// Mark an `auto_start` rule as failed/dropped and schedule its first
    /// re-attempt. No-op for a rule that isn't `auto_start` (nothing opted
    /// it into self-healing) or that already has a pending retry (`or_insert`
    /// so a repeated failure never resets a backoff that's already climbing).
    fn pf_mark_retry_pending(&mut self, id: Uuid) {
        let is_auto = self
            .port_forward_rules
            .iter()
            .any(|r| r.id == id && r.auto_start);
        if !is_auto {
            return;
        }
        self.port_forward_retry.entry(id).or_insert_with(|| PfRetry {
            next_at: Instant::now() + pf_retry_backoff(0),
            attempts: 0,
        });
    }

    /// Re-attempt every `auto_start` rule whose backoff has elapsed. Driven
    /// by the `PortForwardRetryTick` subscription, which only mounts while
    /// `port_forward_retry` is non-empty and the vault is unlocked. Prunes
    /// stale entries (rule deleted, `auto_start` cleared, or already up) so
    /// the subscription unmounts once nothing is pending.
    fn handle_port_forward_retry_tick(&mut self) -> Task<Message> {
        let now = Instant::now();
        let ids: Vec<Uuid> = self.port_forward_retry.keys().copied().collect();
        let mut due = Vec::new();
        for id in ids {
            let still_auto = self
                .port_forward_rules
                .iter()
                .any(|r| r.id == id && r.auto_start);
            if !still_auto || self.active_forwards.contains_key(&id) {
                self.port_forward_retry.remove(&id);
                continue;
            }
            // An attempt is already in flight (or the connect just landed);
            // don't stack a second one. `start_port_forward` also guards
            // this, but skipping here keeps the backoff honest.
            if self.port_forward_starting.contains(&id) {
                continue;
            }
            if self
                .port_forward_retry
                .get(&id)
                .is_some_and(|r| r.next_at <= now)
            {
                due.push(id);
            }
        }

        let mut tasks = Vec::new();
        for id in due {
            // Advance the backoff BEFORE issuing: a failure that lands after
            // this tick keeps climbing, and a success clears the entry.
            if let Some(retry) = self.port_forward_retry.get_mut(&id) {
                retry.attempts = retry.attempts.saturating_add(1);
                retry.next_at = now + pf_retry_backoff(retry.attempts);
            }
            tracing::info!("retrying auto-start port forward {id}");
            tasks.push(self.start_port_forward(id, true));
        }
        Task::batch(tasks)
    }

    /// Resolve password + private key for a connection, preferring a linked
    /// identity over inline fields. Mirrors the terminal connect path in
    /// `dispatch_ssh.rs`.
    pub(crate) fn resolve_forward_credentials(
        &self,
        conn: &Connection,
    ) -> (Option<String>, Option<String>, Option<String>) {
        if let Some(iid) = conn.identity_id {
            let id_pw = self
                .vault
                .as_ref()
                .and_then(|v| v.get_identity_password(&iid).ok().flatten());
            let kid = self
                .identities
                .iter()
                .find(|i| i.id == iid)
                .and_then(|i| i.key_id);
            let id_key = kid.and_then(|kid| {
                self.vault
                    .as_ref()
                    .and_then(|v| v.get_key_private(&kid).ok().flatten())
            });
            let id_cert = kid.and_then(|kid| self.key_certificate(&kid));
            (id_pw, id_key, id_cert)
        } else {
            let pw = self
                .vault
                .as_ref()
                .and_then(|v| v.get_connection_password(&conn.id).ok().flatten());
            let (pk, cert) = if matches!(
                conn.auth_method,
                AuthMethod::Key | AuthMethod::Auto | AuthMethod::Certificate
            ) {
                let pk = conn.key_id.and_then(|kid| {
                    self.vault
                        .as_ref()
                        .and_then(|v| v.get_key_private(&kid).ok().flatten())
                });
                let cert = conn.key_id.and_then(|kid| self.key_certificate(&kid));
                (pk, cert)
            } else {
                (None, None)
            };
            (pw, pk, cert)
        }
    }

    /// Build the jump-host resolver (hydrated passwords / keys / proxies)
    /// for a connection, or `None` when it has no jump chain.
    pub(crate) fn build_jump_resolver(&self, conn: &Connection) -> Option<ConnectionResolver> {
        if conn.jump_chain.is_empty() {
            return None;
        }
        let mut passwords = std::collections::HashMap::new();
        let mut keys = std::collections::HashMap::new();
        let mut certificates = std::collections::HashMap::new();
        let mut proxies = std::collections::HashMap::new();
        for jid in &conn.jump_chain {
            if let Some(vault) = &self.vault
                && let Ok(Some(pw)) = vault.get_connection_password(jid)
            {
                passwords.insert(*jid, pw);
            }
            if let Some(jconn) = self.connections.iter().find(|c| c.id == *jid) {
                if let Some(kid) = jconn.key_id {
                    if let Some(vault) = &self.vault
                        && let Ok(Some(pk)) = vault.get_key_private(&kid)
                    {
                        keys.insert(*jid, pk);
                    }
                    if let Some(cert) = self.key_certificate(&kid) {
                        certificates.insert(*jid, cert);
                    }
                }
                if let Some(vault) = &self.vault
                    && let Ok(Some(p)) = vault.resolve_proxy(jconn)
                {
                    proxies.insert(*jid, p);
                }
            }
        }
        Some(ConnectionResolver {
            certificates,
            connections: self.connections.clone(),
            passwords,
            private_keys: keys,
            proxies,
        })
    }

    /// Build a host-key check callback over a snapshot of known hosts.
    /// Forwards run it strict (unknown / changed → reject) since there is
    /// no terminal modal to prompt; the user trusts a host by connecting a
    /// terminal to it first.
    pub(crate) fn build_host_key_check(&self) -> oryxis_ssh::HostKeyCheckCallback {
        let snapshot = Arc::new(Mutex::new(self.known_hosts.clone()));
        Arc::new(move |host: &str, port: u16, key_type: &str, fingerprint: &str| {
            let hosts = match snapshot.lock() {
                Ok(g) => g,
                Err(poison) => poison.into_inner(),
            };
            // Per (host, port, key_type): a different offered algorithm is
            // Unknown (verify + accept), not a "Changed" MITM warning.
            if let Some(existing) = hosts
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
}

/// Parse a 1..=65535 port from the editor's digit-filtered string.
fn parse_port(s: &str) -> Option<u16> {
    match s.trim().parse::<u16>() {
        Ok(p) if p > 0 => Some(p),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::pf_retry_backoff;
    use std::time::Duration;

    #[test]
    fn backoff_climbs_then_caps_at_120s() {
        assert_eq!(pf_retry_backoff(0), Duration::from_secs(15));
        assert_eq!(pf_retry_backoff(1), Duration::from_secs(30));
        assert_eq!(pf_retry_backoff(2), Duration::from_secs(60));
        assert_eq!(pf_retry_backoff(3), Duration::from_secs(120));
        // The ceiling holds for every further attempt, and the bounded
        // shift (`attempts.min(3)`) means a huge count can't overflow.
        assert_eq!(pf_retry_backoff(4), Duration::from_secs(120));
        assert_eq!(pf_retry_backoff(50), Duration::from_secs(120));
        assert_eq!(pf_retry_backoff(u32::MAX), Duration::from_secs(120));
    }
}
