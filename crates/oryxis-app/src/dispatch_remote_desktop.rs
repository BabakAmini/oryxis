//! RDP/VNC-over-SSH launcher. A one-click card action that opens a `-L`
//! tunnel through the host's SSH connection to its RDP/VNC service and
//! spawns the OS-native client at the local end.
//!
//! The tunnel (`ForwardSession`) is a MANAGED forward stored on the app.
//! It is NOT tied to the client's process (`open rdp://`, single-instance
//! Remmina and mstsc can return immediately, so process-exit teardown
//! would kill the tunnel before the desktop connects). Instead it
//! self-closes once it has served a connection and then goes idle (the
//! desktop client disconnected): the engine's ephemeral forward runs an
//! idle watcher (`spawn_autoclose_local_forward_task`), and when it fires
//! the stream below emits `RemoteDesktopClientClosed` so the app drops its
//! bookkeeping entry. Vault lock / app close clear the map outright.
//!
//! First-time hosts prompt for host-key verification exactly like a normal
//! connect: the launch wires the same `SshHostKeyVerify` modal bridge the
//! terminal connect uses, so a host that isn't in `known_hosts` yet no
//! longer fails outright (the old behaviour, which forced you to open a
//! terminal to the host first to trust its key).
//!
//! The client spawn is a fire-and-forget leaf with no automated coverage
//! (no headless RDP/VNC client exists); it needs manual QA. The command
//! RESOLUTION (`crate::remote_desktop::resolve_command`) is a pure,
//! unit-tested function.

#![allow(clippy::result_large_err)]

use std::sync::Arc;

use iced::Task;
use oryxis_ssh::SshEngine;

use crate::app::{Message, Oryxis};
use crate::remote_desktop::{program_on_path, resolve_command};

impl Oryxis {
    pub(crate) fn handle_remote_desktop(
        &mut self,
        message: Message,
    ) -> Result<Task<Message>, Message> {
        match message {
            Message::OpenRemoteDesktop(idx) => Ok(self.open_remote_desktop(idx)),
            Message::RemoteDesktopReady(conn_id, seq, res) => {
                match res {
                    Ok((session, port)) => {
                        // Replace any prior tunnel for this host. The old
                        // entry holds the sole strong `Arc`, so dropping it
                        // here fires its cancellation (and its own stream
                        // emits a now-stale `ClientClosed` we ignore by seq).
                        self.remote_desktop_forwards
                            .insert(conn_id, (seq, session));
                        self.toast = Some(
                            crate::i18n::t("remote_desktop_opening")
                                .replace("{port}", &port.to_string()),
                        );
                    }
                    Err(e) => self.toast = Some(e),
                }
                Ok(Task::none())
            }
            Message::RemoteDesktopClientClosed(conn_id, seq) => {
                // The tunnel closed on its own. Drop the entry only if it is
                // still the one this stream owns: a superseded launch (Stop +
                // relaunch) must not evict the newer tunnel.
                if self
                    .remote_desktop_forwards
                    .get(&conn_id)
                    .is_some_and(|(s, _)| *s == seq)
                {
                    self.remote_desktop_forwards.remove(&conn_id);
                }
                Ok(Task::none())
            }
            Message::StopRemoteDesktop(conn_id) => {
                if let Some((_, session)) = self.remote_desktop_forwards.remove(&conn_id) {
                    return Ok(Task::perform(
                        async move { session.cancel().await },
                        |_| Message::NoOp,
                    ));
                }
                Ok(Task::none())
            }
            m => Err(m),
        }
    }

    /// Build and fire the launch: resolve credentials like every other SSH
    /// connect, open the ephemeral `-L` forward (prompting for the host key
    /// if unknown), then spawn the client (or surface a "nothing installed"
    /// hint).
    fn open_remote_desktop(&mut self, idx: usize) -> Task<Message> {
        self.card_context_menu = None;
        self.overlay = None;
        let Some(mut conn) = self.connections.get(idx).cloned() else {
            return Task::none();
        };
        use oryxis_core::models::connection::ConnectionProtocol;
        let Some(rd) = conn.remote_desktop.clone() else {
            return Task::none();
        };
        if conn.protocol != ConnectionProtocol::Ssh {
            return Task::none();
        }

        if let Some(vault) = self.vault.as_ref() {
            conn.proxy = vault.resolve_proxy(&conn).ok().flatten();
        }
        let (password, private_key) = self.resolve_forward_credentials(&conn);
        let totp_secret = self
            .vault
            .as_ref()
            .and_then(|v| v.get_connection_totp_secret(&conn.id).ok().flatten());
        let resolver = self.build_jump_resolver(&conn);
        let host_key_check = self.build_host_key_check();
        let keepalive = self.effective_keepalive(&conn);
        // The RDP username can prefill the client (FreeRDP `/u:`); reuse
        // the connect-time resolution (linked identity fills an empty).
        let username = conn.username.clone().or_else(|| {
            conn.identity_id.and_then(|iid| {
                self.identities
                    .iter()
                    .find(|i| i.id == iid)
                    .and_then(|i| i.username.clone())
            })
        });
        let conn_id = conn.id;
        let kind = rd.kind;
        let target_host = rd.target_host.clone();
        let target_port = rd.target_port;
        let algo_ciphers = conn.ciphers.clone();
        let algo_kex = conn.kex.clone();
        let algo_macs = conn.macs.clone();
        let algo_host_keys = conn.host_key_algorithms.clone();

        // Launch generation, so a stale result / self-close from a
        // superseded launch can't clobber a newer tunnel for this host.
        self.remote_desktop_seq += 1;
        let seq = self.remote_desktop_seq;

        // Host-key + keyboard-interactive bridges: the engine asks over
        // `hk_ask` / `kbi_ask`, the stream surfaces the shared modals, and
        // the answers come back on the response channels the existing
        // `SshHostKey*` / `SshKbi*` handlers already drive. This inherits
        // the documented single-response-channel limitation (fine here: a
        // launch is a foreground, one-at-a-time user action).
        let (hk_ask_tx, mut hk_ask_rx) = tokio::sync::mpsc::channel::<(
            oryxis_ssh::HostKeyQuery,
            tokio::sync::oneshot::Sender<bool>,
        )>(1);
        let (hk_resp_tx, mut hk_resp_rx) = tokio::sync::mpsc::channel::<bool>(1);
        self.host_key_response_tx = Some(hk_resp_tx);

        let (kbi_ask_tx, mut kbi_ask_rx) = tokio::sync::mpsc::channel::<(
            oryxis_ssh::KbiQuery,
            tokio::sync::oneshot::Sender<Option<Vec<String>>>,
        )>(1);
        let (kbi_resp_tx, mut kbi_resp_rx) =
            tokio::sync::mpsc::channel::<Option<Vec<String>>>(1);
        self.kbi_response_tx = Some(kbi_resp_tx);

        let stream = iced::stream::channel::<Message>(
            8,
            move |mut sender: iced::futures::channel::mpsc::Sender<Message>| async move {
                use iced::futures::SinkExt;

                let engine = SshEngine::new()
                    .with_host_key_check(host_key_check)
                    .with_host_key_ask(hk_ask_tx)
                    .with_kbi_ask(kbi_ask_tx)
                    .with_totp_secret(totp_secret.as_deref())
                    .with_password_prompt_labels(
                        crate::i18n::t("auth_password_prompt_title").to_string(),
                        crate::i18n::t("password").to_string(),
                    )
                    .with_keepalive(keepalive)
                    .with_strict_host_key(true)
                    .with_algorithm_overrides(
                        algo_ciphers,
                        algo_kex,
                        algo_macs,
                        algo_host_keys,
                    );

                let mut hk_sender = sender.clone();
                let _hk_bridge = tokio::spawn(async move {
                    while let Some((query, resp_tx)) = hk_ask_rx.recv().await {
                        let _ = hk_sender.send(Message::SshHostKeyVerify(query)).await;
                        let accepted = hk_resp_rx.recv().await.unwrap_or(false);
                        let _ = resp_tx.send(accepted);
                    }
                });

                let mut kbi_sender = sender.clone();
                let _kbi_bridge = tokio::spawn(async move {
                    while let Some((query, resp_tx)) = kbi_ask_rx.recv().await {
                        let _ = kbi_sender.send(Message::SshKbiPrompt(None, query)).await;
                        let answers = kbi_resp_rx.recv().await.unwrap_or(None);
                        let _ = resp_tx.send(answers);
                    }
                });

                let outcome: Result<(Arc<oryxis_ssh::ForwardSession>, u16), String> = async {
                    let (session, port) = engine
                        .connect_local_forward_ephemeral(
                            &conn,
                            password.as_deref(),
                            private_key.as_deref(),
                            &target_host,
                            target_port,
                            resolver.as_ref(),
                        )
                        .await
                        .map_err(|e| {
                            format!("{}: {e}", crate::i18n::t("remote_desktop_tunnel_failed"))
                        })?;

                    // The tunnel is up; point a client at its local end.
                    match resolve_command(
                        kind,
                        std::env::consts::OS,
                        port,
                        username.as_deref(),
                        &program_on_path,
                    ) {
                        Ok(cmd) => match std::process::Command::new(&cmd.program)
                            .args(&cmd.args)
                            .spawn()
                        {
                            Ok(_child) => Ok((Arc::new(session), port)),
                            Err(e) => {
                                // Client found but failed to launch: drop the
                                // tunnel so it doesn't linger unusable.
                                session.cancel().await;
                                Err(format!("{}: {e}", cmd.program))
                            }
                        },
                        Err(no) => {
                            session.cancel().await;
                            Err(format!(
                                "{} ({})",
                                crate::i18n::t("remote_desktop_no_client"),
                                no.looked_for.join(", ")
                            ))
                        }
                    }
                }
                .await;

                match outcome {
                    Ok((session, port)) => {
                        // Watch for the tunnel closing (idle auto-close, owner
                        // Stop, or drop) BEFORE handing the sole `Arc` to the
                        // app, so we can tell it to drop the entry. The watch
                        // receiver does not keep the `ForwardSession` alive.
                        let mut closed = session.subscribe_cancel();
                        let _ = sender
                            .send(Message::RemoteDesktopReady(conn_id, seq, Ok((session, port))))
                            .await;
                        let _ = closed.wait_for(|&c| c).await;
                        let _ = sender
                            .send(Message::RemoteDesktopClientClosed(conn_id, seq))
                            .await;
                    }
                    Err(msg) => {
                        let _ = sender
                            .send(Message::RemoteDesktopReady(conn_id, seq, Err(msg)))
                            .await;
                    }
                }
            },
        );

        Task::stream(stream)
    }
}
