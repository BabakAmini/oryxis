//! RDP/VNC-over-SSH launcher. A one-click card action that opens a `-L`
//! tunnel through the host's SSH connection to its RDP/VNC service and
//! spawns the OS-native client at the local end.
//!
//! The tunnel (`ForwardSession`) is a MANAGED forward stored on the app,
//! kept alive until relaunch / vault lock / app close, NOT tied to the
//! client's process: several clients (`open rdp://`, single-instance
//! Remmina, mstsc handoff) return immediately, so process-exit teardown
//! would kill the tunnel before the desktop connects.
//!
//! The client spawn is a fire-and-forget leaf with no automated
//! coverage (no headless RDP/VNC client exists); it needs manual QA.
//! The command RESOLUTION (`crate::remote_desktop::resolve_command`) is
//! a pure, unit-tested function.

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
            Message::RemoteDesktopReady(conn_id, res) => {
                match res {
                    Ok((session, port)) => {
                        // Relaunch replaces any prior tunnel for this host;
                        // dropping the old `Arc` cancels it.
                        self.remote_desktop_forwards.insert(conn_id, session);
                        self.toast = Some(
                            crate::i18n::t("remote_desktop_opening")
                                .replace("{port}", &port.to_string()),
                        );
                    }
                    Err(e) => self.toast = Some(e),
                }
                Ok(Task::none())
            }
            Message::StopRemoteDesktop(conn_id) => {
                if let Some(session) = self.remote_desktop_forwards.remove(&conn_id) {
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

    /// Build and fire the launch: resolve credentials like every other
    /// SSH connect, open the ephemeral `-L` forward, then spawn the
    /// client (or surface a "nothing installed" hint).
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

        Task::perform(
            async move {
                let engine = SshEngine::new()
                    .with_host_key_check(host_key_check)
                    .with_strict_host_key(true)
                    .with_totp_secret(totp_secret.as_deref())
                    .with_keepalive(keepalive)
                    .with_algorithm_overrides(
                        conn.ciphers.clone(),
                        conn.kex.clone(),
                        conn.macs.clone(),
                        conn.host_key_algorithms.clone(),
                    );
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
                match resolve_command(kind, std::env::consts::OS, port, username.as_deref(), &program_on_path) {
                    Ok(cmd) => {
                        match std::process::Command::new(&cmd.program).args(&cmd.args).spawn() {
                            Ok(_child) => Ok((Arc::new(session), port)),
                            Err(e) => {
                                // Client found but failed to launch: drop the
                                // tunnel so it doesn't linger unusable.
                                session.cancel().await;
                                Err(format!("{}: {e}", cmd.program))
                            }
                        }
                    }
                    Err(no) => {
                        session.cancel().await;
                        Err(format!(
                            "{} ({})",
                            crate::i18n::t("remote_desktop_no_client"),
                            no.looked_for.join(", ")
                        ))
                    }
                }
            },
            move |res| Message::RemoteDesktopReady(conn_id, res),
        )
    }
}
