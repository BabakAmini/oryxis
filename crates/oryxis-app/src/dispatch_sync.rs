//! `Oryxis::handle_sync`: settings-panel-independent dispatch arms for the
//! sync area, split out of dispatch.rs. Returns `Err(message)` for anything
//! it doesn't claim so the try_handler! chain falls through.
#![allow(clippy::result_large_err)]
#![allow(clippy::collapsible_if)]
#![allow(clippy::collapsible_match)]
#![allow(clippy::too_many_lines)]

use iced::Task;

use crate::app::{Message, Oryxis, SyncMessage};

impl Oryxis {
    pub(crate) fn handle_sync(
        &mut self,
        message: SyncMessage,
    ) -> Task<Message> {
        match message {
            // ── Sync ──
            SyncMessage::ToggleEnabled => {
                self.sync.enabled = !self.sync.enabled;
                if let Some(vault) = &self.vault {
                    let _ = vault.set_setting("sync_enabled", if self.sync.enabled { "true" } else { "false" });
                }
                // SFTP transport has no background engine: enabling just
                // persists the flag (the cadence subscription picks it up);
                // disabling clears any stale status.
                if !self.sync_uses_p2p() {
                    self.sync.status = Some(
                        crate::i18n::t(if self.sync.enabled {
                            "sync_status_enabled"
                        } else {
                            "sync_status_stopped"
                        })
                        .to_string(),
                    );
                } else if self.sync.enabled {
                    return self.start_sync_engine();
                } else {
                    self.stop_sync_engine();
                    self.sync.status =
                        Some(crate::i18n::t("sync_status_stopped").to_string());
                }
            }
            SyncMessage::TogglePasswords => {
                self.sync.passwords = !self.sync.passwords;
                if let Some(vault) = &self.vault {
                    let _ = vault.set_setting(
                        "sync_passwords",
                        if self.sync.passwords { "true" } else { "false" },
                    );
                }
            }
            SyncMessage::ModeChanged(v) => {
                self.sync.mode = v.clone();
                if let Some(vault) = &self.vault {
                    let _ = vault.set_setting("sync_mode", &v);
                }
            }
            SyncMessage::DeviceNameChanged(v) => {
                self.sync.device_name = v.clone();
                if let Some(vault) = &self.vault {
                    let _ = vault.set_setting("sync_device_name", &v);
                }
            }
            SyncMessage::SignalingUrlChanged(v) => {
                self.sync.signaling_url = v.clone();
                if let Some(vault) = &self.vault {
                    let _ = vault.set_setting("sync_signaling_url", &v);
                }
            }
            SyncMessage::SignalingTokenChanged(v) => {
                let v = v.into_inner();
                self.sync.signaling_token = v.clone();
                if let Some(vault) = &self.vault {
                    let _ = vault.set_setting("sync_signaling_token", &v);
                }
            }
            SyncMessage::RelayUrlChanged(v) => {
                self.sync.relay_url = v.clone();
                if let Some(vault) = &self.vault {
                    let _ = vault.set_setting("sync_relay_url", &v);
                }
            }
            SyncMessage::ListenPortChanged(v) => {
                self.sync.listen_port = v.clone();
                if let Some(vault) = &self.vault {
                    let _ = vault.set_setting("sync_listen_port", &v);
                }
            }
            SyncMessage::WizardToggle => {
                let w = &mut self.sync.relay_wizard;
                w.open = !w.open;
                if w.open && w.token.is_empty() {
                    w.token = fresh_relay_token();
                }
            }
            SyncMessage::WizardDomainChanged(v) => {
                self.sync.relay_wizard.domain = v;
                self.sync.relay_wizard.result = None;
                // Editing the endpoint invalidates any in-flight probe:
                // with the snapshot gone, its result is discarded on
                // arrival instead of persisting a never-tested value.
                self.sync.relay_wizard.testing_snapshot = None;
            }
            SyncMessage::WizardPortChanged(v) => {
                self.sync.relay_wizard.port = v;
                self.sync.relay_wizard.result = None;
                self.sync.relay_wizard.testing_snapshot = None;
            }
            SyncMessage::WizardFormatChanged(f) => {
                self.sync.relay_wizard.format = f;
            }
            SyncMessage::WizardRegenToken => {
                self.sync.relay_wizard.token = fresh_relay_token();
                self.sync.relay_wizard.result = None;
                self.sync.relay_wizard.testing_snapshot = None;
            }
            SyncMessage::WizardTest => {
                let Some(base) = self.sync.relay_wizard.base_url() else {
                    return Task::none();
                };
                if self.sync.relay_wizard.testing {
                    return Task::none();
                }
                self.sync.relay_wizard.testing = true;
                self.sync.relay_wizard.result = None;
                // Snapshot what this probe actually tests. The result
                // handler adopts these values, never the live form, so
                // whatever the user types during the 8s probe window is
                // never persisted untested.
                self.sync.relay_wizard.testing_snapshot =
                    Some((base.clone(), self.sync.relay_wizard.token.clone()));
                return Task::perform(
                    async move {
                        // The wizard sets up a self-hosted `oryxis-relay`
                        // binary, whose `/healthz` is unauthenticated
                        // (the Worker backend authenticates every route,
                        // so it is not a wizard target). Probe reachability
                        // + TLS without leaking the token, but require the
                        // relay to identify itself: a parked domain or CDN
                        // will answer 2xx on any path, and adopting it
                        // would silently overwrite a working endpoint. The
                        // `x-oryxis-relay` header is the positive signal;
                        // an exact `ok` body is the fallback for relays
                        // that predate the header.
                        let client = reqwest::Client::builder()
                            .timeout(std::time::Duration::from_secs(8))
                            .build()
                            .map_err(|e| e.to_string())?;
                        let mut resp = client
                            .get(format!("{base}/healthz"))
                            .send()
                            .await
                            .map_err(|e| e.to_string())?;
                        if !resp.status().is_success() {
                            return Err(format!("HTTP {}", resp.status()));
                        }
                        // The header alone confirms an Oryxis relay; short
                        // circuit so a confirmed relay's body is never read.
                        if resp.headers().contains_key("x-oryxis-relay") {
                            return Ok(());
                        }
                        // Fallback for relays predating the header: the body
                        // must be exactly "ok". Cap the read at 4 KiB so a
                        // mistyped or hostile URL streaming a huge body can't
                        // OOM the app (the 8s timeout bounds time, not bytes).
                        let mut body = Vec::new();
                        while let Some(chunk) =
                            resp.chunk().await.map_err(|e| e.to_string())?
                        {
                            if body.len() + chunk.len() > 4096 {
                                return Err(crate::i18n::t("sync_relay_not_recognized")
                                    .to_string());
                            }
                            body.extend_from_slice(&chunk);
                        }
                        if String::from_utf8_lossy(&body).trim() == "ok" {
                            Ok(())
                        } else {
                            Err(crate::i18n::t("sync_relay_not_recognized").to_string())
                        }
                    },
                    |v| Message::Sync(SyncMessage::WizardTestResult(v)),
                );
            }
            SyncMessage::WizardTestResult(r) => {
                self.sync.relay_wizard.testing = false;
                // The probe tested the SNAPSHOT taken when it started,
                // not whatever the form says now. A domain / port /
                // token edit during the probe clears the snapshot; in
                // that case the result describes a stale endpoint, so
                // drop it entirely (no success or failure line) and
                // let the user re-test the edited values.
                let Some((base, token)) =
                    self.sync.relay_wizard.testing_snapshot.take()
                else {
                    self.sync.relay_wizard.result = None;
                    return Task::none();
                };
                let ok = r.is_ok();
                self.sync.relay_wizard.result = Some(r);
                if ok {
                    // Reachable: adopt the relay as this device's
                    // signaling endpoint, exactly as if the user had
                    // filled the Advanced fields by hand. Adopt the
                    // probed snapshot, never the live form.
                    self.sync.signaling_url = base.clone();
                    self.sync.signaling_token = token.clone();
                    if let Some(vault) = &self.vault {
                        let _ = vault.set_setting("sync_signaling_url", &base);
                        let _ =
                            vault.set_setting("sync_signaling_token", &token);
                    }
                    // A running engine keeps its old config; bounce
                    // it so the new endpoint takes effect now.
                    if self.sync.engine_running {
                        self.stop_sync_engine();
                        return self.start_sync_engine();
                    }
                }
            }
            SyncMessage::StartPairing => {
                // Host a real pairing code on the engine. The engine
                // also emits `PairingCodeGenerated`, but we set the
                // code + state here directly so the UI flips instantly.
                if let Some(runtime) = &self.sync.runtime {
                    let handle = runtime.handle();
                    let code = handle.start_hosting_pairing();
                    let link = handle.pairing_link(&code);
                    self.sync.pairing.link = Some(link);
                    self.sync.pairing.code = Some(code);
                    self.sync.pairing.state = crate::state::SyncPairingState::Hosting;
                } else {
                    self.sync.status =
                        Some(crate::i18n::t("sync_status_disabled").to_string());
                }
            }
            SyncMessage::CancelHostingPairing => {
                if let Some(runtime) = &self.sync.runtime {
                    runtime.handle().cancel_hosting_pairing();
                }
                self.sync.pairing.code = None;
                self.sync.pairing.link = None;
                self.sync.pairing.state = crate::state::SyncPairingState::Idle;
            }
            SyncMessage::JoinPairingRequested => {
                self.sync.pairing.state = crate::state::SyncPairingState::Joining;
                self.sync.pairing.join_code_input.clear();
                self.sync.pairing.join_target_input.clear();
                self.sync.pairing.join_link_input.clear();
            }
            SyncMessage::JoinCodeChanged(v) => {
                self.sync.pairing.join_code_input = v;
            }
            SyncMessage::JoinTargetChanged(v) => {
                self.sync.pairing.join_target_input = v;
            }
            SyncMessage::JoinLinkChanged(v) => {
                self.sync.pairing.join_link_input = v;
            }
            SyncMessage::JoinPairingCancel => {
                self.sync.pairing.state = crate::state::SyncPairingState::Idle;
            }
            SyncMessage::PairWithDiscovered(device_id) => {
                if let Some(peer) = self
                    .sync.discovered
                    .iter()
                    .find(|p| p.device_id == device_id)
                {
                    self.sync.pairing.state = crate::state::SyncPairingState::Joining;
                    self.sync.pairing.join_code_input.clear();
                    self.sync.pairing.join_link_input.clear();
                    self.sync.pairing.join_target_input = peer.addr.to_string();
                }
            }
            SyncMessage::JoinPairingByLink => {
                let Some(runtime) = &self.sync.runtime else {
                    self.sync.status =
                        Some(crate::i18n::t("sync_status_disabled").to_string());
                    return Task::none();
                };
                let link = self.sync.pairing.join_link_input.trim().to_string();
                if oryxis_sync::parse_pairing_link(&link).is_none() {
                    self.sync.status = Some(
                        crate::i18n::t("sync_pairing_bad_link").to_string(),
                    );
                    return Task::none();
                }
                let handle = runtime.handle();
                // Keep at Joining so the inline status + form stay
                // visible; the PairingCompleted / PairingFailed event
                // handler decides whether to drop back to Idle.
                self.sync.status =
                    Some(crate::i18n::t("sync_pairing_connecting").to_string());
                return Task::perform(
                    async move {
                        let _ = handle.join_pairing_remote(&link).await;
                    },
                    |()| Message::NoOp,
                );
            }
            SyncMessage::JoinPairingConnect => {
                let Some(runtime) = &self.sync.runtime else {
                    self.sync.status =
                        Some(crate::i18n::t("sync_status_disabled").to_string());
                    return Task::none();
                };
                let code = self.sync.pairing.join_code_input.trim().to_string();
                if code.len() != 6 || !code.chars().all(|c| c.is_ascii_digit()) {
                    self.sync.status =
                        Some(crate::i18n::t("sync_pairing_invalid_code").to_string());
                    return Task::none();
                }
                let addr: std::net::SocketAddr =
                    match self.sync.pairing.join_target_input.trim().parse() {
                        Ok(a) => a,
                        Err(_) => {
                            self.sync.status = Some(
                                crate::i18n::t("sync_pairing_bad_address").to_string(),
                            );
                            return Task::none();
                        }
                    };
                let handle = runtime.handle();
                // Keep at Joining so the inline status + form stay
                // visible while the handshake runs; the PairingCompleted
                // event flips back to Idle, PairingFailed stays put so
                // the user can fix the code/addr and retry.
                self.sync.status =
                    Some(crate::i18n::t("sync_pairing_connecting").to_string());
                // join_pairing emits PairingCompleted / PairingFailed,
                // which the SyncEngineEvent arm turns into UI state.
                return Task::perform(
                    async move {
                        let _ = handle.join_pairing(addr, code).await;
                    },
                    |()| Message::NoOp,
                );
            }
            SyncMessage::UnpairDevice(peer_id) => {
                if let Some(vault) = &self.vault {
                    let _ = vault.delete_sync_peer(&peer_id);
                    self.sync.peers = vault.list_sync_peers().unwrap_or_default();
                }
            }
            SyncMessage::Now => {
                // SFTP transport: a manual round goes through the
                // snapshot path, not the P2P engine.
                if self.sync.transport == "sftp" {
                    return self.run_sftp_sync_round();
                }
                if self.sync.in_progress {
                    // Defensive: shouldn't fire because the UI swaps
                    // Sync Now for Cancel while a sync is running,
                    // but if a stray click does land, ignore it.
                    return Task::none();
                }
                if let Some(runtime) = &self.sync.runtime {
                    let handle = runtime.handle();
                    let (abort_tx, abort_rx) = tokio::sync::oneshot::channel::<()>();
                    self.sync.abort_tx = Some(abort_tx);
                    self.sync.in_progress = true;
                    self.sync.status =
                        Some(crate::i18n::t("sync_status_syncing").to_string());
                    // Race the sync against a 90s timeout AND the
                    // abort channel. Whichever fires first wins; the
                    // sync future is dropped, which closes the QUIC
                    // connection mid-handshake (quinn cleans up).
                    return Task::perform(
                        async move {
                            tokio::select! {
                                r = tokio::time::timeout(
                                    std::time::Duration::from_secs(90),
                                    handle.sync_now(),
                                ) => match r {
                                    Ok(Ok(())) => Ok(()),
                                    Ok(Err(e)) => Err(format!("{e}")),
                                    Err(_) => Err("__timeout__".into()),
                                },
                                _ = abort_rx => Err("__cancelled__".into()),
                            }
                        },
                        |v| Message::Sync(SyncMessage::NowFinished(v)),
                    );
                }
                self.sync.status =
                    Some(crate::i18n::t("sync_status_disabled").to_string());
            }
            SyncMessage::CancelInProgress => {
                if let Some(tx) = self.sync.abort_tx.take() {
                    let _ = tx.send(());
                }
                // Don't clear `sync_in_progress` here: the Task lands
                // back as `SyncNowFinished(Err("__cancelled__"))` and
                // clears it there, so the Cancel button stays visible
                // until the cancellation actually settles.
            }
            SyncMessage::EngineSpawned => {
                let Some(mut rx) = self.sync.pending_engine.take() else {
                    // `stop_sync_engine` abandoned the spawn: its oneshot
                    // receiver disappeared, the task's `send` failed and
                    // the fresh runtime dropped (stopping the engine's
                    // background tasks). Nothing to adopt.
                    return Task::none();
                };
                match rx.try_recv() {
                    Ok(Ok((runtime, event_rx))) => {
                        self.sync.runtime = Some(runtime);
                        self.sync.engine_running = true;
                        self.sync.status = Some(crate::i18n::t("sync_status_running").to_string());
                        let stream = tokio_stream::wrappers::UnboundedReceiverStream::new(event_rx);
                        return Task::stream(stream)
                            .map(|v| Message::Sync(SyncMessage::EngineEvent(v)));
                    }
                    Ok(Err(e)) => {
                        self.sync.engine_running = false;
                        self.sync.status =
                            Some(format!("{}: {e}", crate::i18n::t("sync_status_failed"),));
                        tracing::warn!("sync engine failed to start: {e}");
                        return Task::none();
                    }
                    Err(_) => {
                        // The spawn task panicked before sending; there
                        // is nothing to adopt and `engine_running` was
                        // never set.
                        return Task::none();
                    }
                }
            }
            SyncMessage::TransportChanged(v) => {
                let mut tasks = Vec::new();
                if v != self.sync.transport {
                    // Leaving P2P: tear the engine down so QUIC/mDNS stop.
                    // Entering P2P (and enabled): bring it up.
                    self.sync.transport = v.clone();
                    if let Some(vault) = &self.vault {
                        let _ = vault.set_setting("sync_transport", &v);
                    }
                    // Every transport's status, not just SFTP's: the
                    // old line was written when there were two, so
                    // switching away and back showed the previous
                    // transport's "Synced, N records" as if it were
                    // this one's.
                    self.sync.status = None;
                    self.sync.sftp.status = None;
                    self.sync.folder.status = None;
                    self.sync.git.status = None;
                    self.sync.webdav.status = None;
                    if !self.sync_uses_p2p() {
                        self.stop_sync_engine();
                    } else if self.sync.enabled {
                        tasks.push(self.start_sync_engine());
                    }
                }
                // The git card's availability probe is a subprocess
                // spawn: run it off the UI thread (see
                // `dispatch_git_sync::git_availability_task`), whenever
                // the git card can be on screen.
                if self.sync.transport == "git" {
                    tasks.push(crate::dispatch_git_sync::git_availability_task());
                }
                return if tasks.is_empty() {
                    Task::none()
                } else {
                    Task::batch(tasks)
                };
            }
            SyncMessage::GitAvailabilityChecked(available) => {
                self.sync.git.git_available = Some(available);
            }
            SyncMessage::SftpHostChanged(id) => {
                self.sync.sftp.host_id = Some(id);
                self.sync.sftp.picker_open = false;
                self.sync.sftp.picker_search.clear();
                if let Some(vault) = &self.vault {
                    let _ = vault.set_setting("sync_sftp_host_id", &id.to_string());
                }
            }
            SyncMessage::SftpHostPickerOpen => {
                self.sync.sftp.picker_open = true;
                self.sync.sftp.picker_search.clear();
            }
            SyncMessage::SftpHostPickerClose => {
                self.sync.sftp.picker_open = false;
                self.sync.sftp.picker_search.clear();
            }
            SyncMessage::SftpHostPickerSearch(v) => {
                self.sync.sftp.picker_search = v;
            }
            SyncMessage::WebdavUrlChanged(v) => {
                self.sync.webdav.url = v.clone();
                if let Some(vault) = &self.vault {
                    let _ = vault.set_setting("sync_webdav_url", &v);
                }
            }
            SyncMessage::WebdavUserChanged(v) => {
                self.sync.webdav.user = v.clone();
                if let Some(vault) = &self.vault {
                    let _ = vault.set_setting("sync_webdav_user", &v);
                }
            }
            SyncMessage::WebdavPasswordChanged(v) => {
                let v = v.into_inner();
                self.sync.webdav.password = v.clone();
                if let Some(vault) = &self.vault {
                    // Its own encrypted setting, not the group
                    // passphrase: this one is an account credential on
                    // one server, and every device has a different one.
                    let _ = vault.set_sync_webdav_password(&v);
                }
            }
            SyncMessage::WebdavPassphraseChanged(v) => {
                let v = v.into_inner();
                self.sync.webdav.passphrase = v.clone();
                if let Some(vault) = &self.vault {
                    // One group secret across every snapshot transport.
                    let _ = vault.set_sync_sftp_passphrase(&v);
                }
            }
            SyncMessage::WebdavSyncNow => return self.run_webdav_sync_round(),
            SyncMessage::WebdavRoundFinished(result) => {
                self.sync.webdav.in_progress = false;
                if result.is_ok() {
                    // The round merged on its own `VaultStore` handle,
                    // so the in-memory lists are stale until this.
                    self.load_data_from_vault();
                }
                self.sync.webdav.status = Some(match result {
                    Ok(pulled) => Ok(crate::i18n::t("snapshot_sync_ok")
                        .replace("{n}", &pulled.to_string())),
                    Err(e) => Err(e),
                });
            }
            SyncMessage::GitRemoteChanged(v) => {
                self.sync.git.remote = v.clone();
                if let Some(vault) = &self.vault {
                    let _ = vault.set_setting("sync_git_remote", &v);
                }
            }
            SyncMessage::GitPassphraseChanged(v) => {
                let v = v.into_inner();
                self.sync.git.passphrase = v.clone();
                if let Some(vault) = &self.vault {
                    // One group secret across every snapshot transport.
                    let _ = vault.set_sync_sftp_passphrase(&v);
                }
            }
            SyncMessage::GitSyncNow => return self.run_git_sync_round(),
            SyncMessage::GitRoundFinished(result) => {
                self.sync.git.in_progress = false;
                if result.is_ok() {
                    // A completed round proves `git` is on PATH; keep
                    // the card's cached probe current without re-spawning
                    // on every frame.
                    self.sync.git.git_available = Some(true);
                    // The round merged on its OWN `VaultStore` handle,
                    // so every in-memory list is stale: whatever it
                    // pulled is on disk and invisible until this
                    // reload. Same reason `SftpDone` does it.
                    self.load_data_from_vault();
                }
                self.sync.git.status = Some(match result {
                    Ok(pulled) => Ok(crate::i18n::t("snapshot_sync_ok")
                        .replace("{n}", &pulled.to_string())),
                    Err(e) => Err(e),
                });
            }
            SyncMessage::FolderPathChanged(v) => {
                self.sync.folder.path = v.clone();
                if let Some(vault) = &self.vault {
                    let _ = vault.set_setting("sync_folder_path", &v);
                }
            }
            SyncMessage::FolderPassphraseChanged(v) => {
                let v = v.into_inner();
                self.sync.folder.passphrase = v.clone();
                if let Some(vault) = &self.vault {
                    // Same encrypted setting the SFTP transport uses:
                    // it is the same group secret, and keeping two
                    // copies would let them drift into two groups.
                    let _ = vault.set_sync_sftp_passphrase(&v);
                }
            }
            SyncMessage::FolderPickDirectory => {
                return Task::perform(
                    async {
                        rfd::AsyncFileDialog::new()
                            .pick_folder()
                            .await
                            .map(|h| h.path().to_string_lossy().into_owned())
                    },
                    |picked| Message::Sync(SyncMessage::FolderDirectoryPicked(picked)),
                );
            }
            SyncMessage::FolderDirectoryPicked(picked) => {
                // A cancel is silent: the user closing a file dialog is
                // not an event worth reporting.
                if let Some(path) = picked {
                    self.sync.folder.path = path.clone();
                    if let Some(vault) = &self.vault {
                        let _ = vault.set_setting("sync_folder_path", &path);
                    }
                }
            }
            SyncMessage::FolderSyncNow => return self.run_folder_sync_round(),
            SyncMessage::FolderRoundFinished(result) => {
                self.sync.folder.in_progress = false;
                if result.is_ok() {
                    // The round merged on its OWN `VaultStore` handle,
                    // so every in-memory list is stale: whatever it
                    // pulled is on disk and invisible until this
                    // reload. Same reason `SftpDone` does it.
                    self.load_data_from_vault();
                }
                self.sync.folder.status = Some(match result {
                    Ok(pulled) => Ok(crate::i18n::t("snapshot_sync_ok")
                        .replace("{n}", &pulled.to_string())),
                    Err(e) => Err(e),
                });
            }
            SyncMessage::SftpPathChanged(v) => {
                self.sync.sftp.remote_path = v.clone();
                if let Some(vault) = &self.vault {
                    let _ = vault.set_setting("sync_sftp_remote_path", &v);
                }
            }
            SyncMessage::SftpPassphraseChanged(v) => {
                let v = v.into_inner();
                self.sync.sftp.passphrase = v.clone();
                if let Some(vault) = &self.vault {
                    let _ = vault.set_sync_sftp_passphrase(&v);
                }
            }
            SyncMessage::SnapshotTick => {
                // Auto-cadence tick. Re-checks the conditions the
                // subscription mounted on, because the subscription is
                // rebuilt a frame LATE: a user who switches to manual
                // (or to P2P) mid-interval would otherwise get one more
                // round after the change. The per-transport
                // `in_progress` guard keeps a slow round from stacking.
                if !self.sync.enabled
                    || self.sync.mode != "auto"
                    || self.vault_ui.state != crate::state::VaultState::Unlocked
                {
                    return Task::none();
                }
                match self.sync.transport.as_str() {
                    "sftp" if !self.sync.sftp.in_progress => {
                        return self.run_sftp_sync_round();
                    }
                    "folder" if !self.sync.folder.in_progress => {
                        return self.run_folder_sync_round();
                    }
                    "git" if !self.sync.git.in_progress => {
                        return self.run_git_sync_round();
                    }
                    "webdav" if !self.sync.webdav.in_progress => {
                        return self.run_webdav_sync_round();
                    }
                    _ => {}
                }
            }
            SyncMessage::SftpDone(result) => {
                self.sync.sftp.in_progress = false;
                if result.is_ok() {
                    // The merge ran on a separate vault handle, so the
                    // in-memory lists are stale: reload to reflect it.
                    self.load_data_from_vault();
                }
                self.sync.sftp.status = Some(result);
            }
            SyncMessage::NowFinished(result) => {
                self.sync.in_progress = false;
                self.sync.abort_tx = None;
                match result {
                    Ok(()) => {}
                    Err(e) if e == "__cancelled__" => {
                        self.sync.status = Some(
                            crate::i18n::t("sync_status_cancelled").to_string(),
                        );
                    }
                    Err(e) if e == "__timeout__" => {
                        self.sync.status = Some(
                            crate::i18n::t("sync_status_timeout").to_string(),
                        );
                    }
                    Err(e) => {
                        self.sync.status = Some(format!(
                            "{}: {e}",
                            crate::i18n::t("sync_status_failed"),
                        ));
                    }
                }
                // Per-peer outcomes already arrived as SyncEngineEvent;
                // refresh the peer list so last_synced_at is current.
                if let Some(vault) = &self.vault {
                    self.sync.peers = vault.list_sync_peers().unwrap_or_default();
                }
            }
            SyncMessage::EngineEvent(event) => {
                use oryxis_sync::SyncEvent;
                match event {
                    SyncEvent::PeerDiscovered { device_id, device_name, addr, .. } => {
                        // Dedup by device_id: an mDNS browse can
                        // republish the same peer (different network
                        // interface, restart, etc.). Last writer wins
                        // on the address so a roaming peer's entry
                        // tracks its new ip:port.
                        let info = crate::state::DiscoveredPeerInfo {
                            device_id,
                            device_name,
                            addr,
                        };
                        if let Some(existing) = self
                            .sync.discovered
                            .iter_mut()
                            .find(|p| p.device_id == device_id)
                        {
                            *existing = info;
                        } else {
                            self.sync.discovered.push(info);
                        }
                    }
                    SyncEvent::PairingCodeGenerated { code } => {
                        self.sync.pairing.code = Some(code);
                    }
                    SyncEvent::PairingCompleted { device_name, .. } => {
                        self.sync.status = Some(format!(
                            "{} {device_name}",
                            crate::i18n::t("sync_paired_with"),
                        ));
                        // Pairing done on either side: close the modal
                        // sub-view, drop the hosted code / link / QR,
                        // and refresh the peer list.
                        self.sync.pairing.state =
                            crate::state::SyncPairingState::Idle;
                        self.sync.pairing.code = None;
                        self.sync.pairing.link = None;
                        if let Some(vault) = &self.vault {
                            self.sync.peers =
                                vault.list_sync_peers().unwrap_or_default();
                        }
                    }
                    SyncEvent::PairingFailed { reason } => {
                        self.sync.status = Some(format!(
                            "{}: {reason}",
                            crate::i18n::t("sync_pairing_failed"),
                        ));
                        // Stay in whichever sub-view triggered the
                        // pairing so the user sees the error in
                        // context and can fix + retry without
                        // re-entering everything. Host-side: clear
                        // the code/link since the single-shot was
                        // consumed even on failure.
                        if self.sync.pairing.state
                            == crate::state::SyncPairingState::Hosting
                        {
                            self.sync.pairing.code = None;
                            self.sync.pairing.link = None;
                            self.sync.pairing.state =
                                crate::state::SyncPairingState::Idle;
                        }
                    }
                    SyncEvent::SyncStarted { .. } => {
                        self.sync.status =
                            Some(crate::i18n::t("sync_status_syncing").to_string());
                    }
                    SyncEvent::SyncCompleted { pushed, pulled, .. } => {
                        self.sync.status = Some(format!(
                            "{} (+{pushed} / -{pulled})",
                            crate::i18n::t("sync_status_done"),
                        ));
                        if let Some(vault) = &self.vault {
                            self.sync.peers =
                                vault.list_sync_peers().unwrap_or_default();
                        }
                    }
                    SyncEvent::SyncFailed { error, .. } => {
                        self.sync.status = Some(format!(
                            "{}: {error}",
                            crate::i18n::t("sync_status_failed"),
                        ));
                    }
                    SyncEvent::PeerOnline { .. } | SyncEvent::PeerOffline { .. } => {}
                    SyncEvent::SignalingRegistered { ip, port } => {
                        // Confirms cross-network pairing is reachable
                        // at this address. Until this fires the host is
                        // LAN-only (or signaling failed silently). The
                        // `(n)` counter bumps on every refresh so the
                        // user sees heartbeats land even when the IP
                        // is stable.
                        self.sync.signaling_tick =
                            self.sync.signaling_tick.saturating_add(1);
                        self.sync.signaling_last = Some(Ok(format!("{ip}:{port}")));
                        self.sync.status = Some(format!(
                            "{} ({}): {ip}:{port}",
                            crate::i18n::t("sync_status_signaling_registered"),
                            self.sync.signaling_tick,
                        ));
                    }
                    SyncEvent::SignalingFailed { reason } => {
                        self.sync.signaling_last = Some(Err(reason.clone()));
                        self.sync.status = Some(format!(
                            "{}: {reason}",
                            crate::i18n::t("sync_status_signaling_failed"),
                        ));
                    }
                    SyncEvent::VersionMismatch {
                        peer_version,
                        local_version,
                        ..
                    } => {
                        self.sync.status = Some(format!(
                            "{}: peer v{peer_version}, local v{local_version}",
                            crate::i18n::t("sync_status_version_mismatch"),
                        ));
                    }
                    SyncEvent::PeerStaleWarning { days_since_sync, .. } => {
                        self.sync.status = Some(format!(
                            "{} ({}d)",
                            crate::i18n::t("sync_status_peer_stale"),
                            days_since_sync,
                        ));
                    }
                }
            }
        }
        Task::none()
    }
}

/// Random bearer token for a wizard-generated relay: two UUIDs' worth
/// of hex (256 bits), long enough that the token is never the weak
/// link (the relay token only gates "can talk to the relay"; payloads
/// stay end-to-end encrypted above it).
fn fresh_relay_token() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}
