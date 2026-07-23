use super::*;

impl SshEngine {
    // -----------------------------------------------------------------------
    // Authentication
    // -----------------------------------------------------------------------

    /// Authenticate on a handle (used for both direct and jump host connections).
    pub(crate) async fn authenticate_handle(
        &self,
        handle: &mut client::Handle<ClientHandler>,
        connection: &Connection,
        password: Option<&str>,
        key_material: Option<KeyMaterial<'_>>,
    ) -> Result<(), SshError> {
        let username = connection.username.as_deref().unwrap_or("root");
        let has_pw = password.is_some();
        let has_key = key_material.is_some();
        tracing::info!(
            "Auth for {}@{} method={:?} has_password={} has_key={}",
            username, connection.hostname, connection.auth_method, has_pw, has_key,
        );

        match self
            .do_auth(handle, username, &connection.auth_method, password, key_material)
            .await
        {
            Ok(true) => {
                tracing::info!("Authenticated as {} on {}", username, connection.hostname);
                Ok(())
            }
            Ok(false) => Err(SshError::Key(format!(
                "Auth rejected for \"{}\" (method: {:?}, password: {}, key: {})",
                username, connection.auth_method, has_pw, has_key,
            ))),
            Err(e) => Err(e),
        }
    }

    pub(crate) async fn do_auth(
        &self,
        handle: &mut client::Handle<ClientHandler>,
        username: &str,
        auth_method: &AuthMethod,
        password: Option<&str>,
        key_material: Option<KeyMaterial<'_>>,
    ) -> Result<bool, SshError> {
        match auth_method {
            AuthMethod::Auto => {
                let mut tried: Vec<&str> = Vec::new();

                // 1. Try publickey if a key is provided
                if let Some(km) = key_material {
                    tried.push("publickey");
                    tracing::info!("Auto: trying publickey auth for {}", username);
                    match self.try_publickey_auth(handle, username, km).await {
                        Ok(true) => return Ok(true),
                        Ok(false) => tracing::info!("Auto: publickey rejected"),
                        Err(e) => tracing::info!("Auto: publickey error: {}", e),
                    }
                }

                // 2. Try agent auth
                tried.push("agent");
                tracing::info!("Auto: trying agent auth for {}", username);
                match self.auth_via_agent(handle, username).await {
                    Ok(AgentAuthOutcome::Authenticated) => return Ok(true),
                    Ok(AgentAuthOutcome::NoMatch(tally)) => {
                        tracing::info!("Auto: agent had no matching keys ({tally})")
                    }
                    Err(e) => tracing::info!("Auto: agent unavailable: {}", e),
                }

                // 3. Try password if available
                if let Some(pw) = password {
                    tried.push("password");
                    tracing::info!("Auto: trying password auth for {}", username);
                    match handle.authenticate_password(username, pw).await {
                        Ok(res) if res.success() => return Ok(true),
                        Ok(_) => tracing::info!("Auto: password rejected"),
                        Err(e) => tracing::info!("Auto: password error: {}", e),
                    }

                    // 4. Try keyboard-interactive with password. Silent
                    // (use_callback = false): it only reaches here after
                    // password already failed, so a prompt at the tail of a
                    // saved Auto host would be surprising. The user picks
                    // AuthMethod::Interactive when they want the modal; the
                    // quick-connect opt-in below is the one exception.
                    tried.push("keyboard-interactive");
                    tracing::info!("Auto: trying keyboard-interactive auth for {}", username);
                    if matches!(
                        self.try_keyboard_interactive(handle, username, Some(pw), false).await?,
                        KbiOutcome::Success
                    ) {
                        return Ok(true);
                    }
                }

                // 5. Quick-connect fallback (`with_auto_interactive_fallback`):
                // surface the interactive prompt the way OpenSSH would once
                // every silent method has failed, instead of erroring out.
                if self.auto_interactive_fallback && self.kbi_ask_tx.is_some() {
                    tried.push("keyboard-interactive (prompt)");
                    tracing::info!("Auto: trying prompted keyboard-interactive auth for {}", username);
                    match self.try_keyboard_interactive(handle, username, password, true).await? {
                        KbiOutcome::Success => return Ok(true),
                        // An explicit cancel ends the attempt; chaining a
                        // second modal after it would fight the user.
                        KbiOutcome::Cancelled => {
                            return Err(SshError::Key("Authentication cancelled".into()));
                        }
                        // The server may not offer keyboard-interactive at
                        // all (password-only sshd): one prompted password
                        // attempt before giving up.
                        KbiOutcome::Rejected => {
                            tried.push("password (prompt)");
                            tracing::info!("Auto: trying prompted password auth for {}", username);
                            match self.prompt_password_once(None).await {
                                Some(pw) => {
                                    let res = tokio::time::timeout(
                                        self.auth_timeout,
                                        handle.authenticate_password(username, &pw),
                                    )
                                    .await
                                    .map_err(|_| {
                                        SshError::ConnectionFailed(format!(
                                            "auth timed out after {}s",
                                            self.auth_timeout.as_secs()
                                        ))
                                    })??;
                                    if res.success() {
                                        return Ok(true);
                                    }
                                }
                                None => {
                                    return Err(SshError::Key(
                                        "Authentication cancelled".into(),
                                    ));
                                }
                            }
                        }
                    }
                }

                Err(SshError::Key(format!(
                    "All auto auth methods failed for \"{}\". Tried: {}",
                    username,
                    tried.join(", ")
                )))
            }
            AuthMethod::Password => {
                let pw = password.ok_or(SshError::AuthFailed)?;
                tracing::info!("Trying password auth for {}", username);
                let res = handle.authenticate_password(username, pw).await?;
                if !res.success() {
                    return Err(SshError::Key("Password rejected by server".into()));
                }
                Ok(true)
            }
            AuthMethod::Key => {
                let km = key_material
                    .ok_or_else(|| SshError::Key("No private key selected".into()))?;

                // Strictly the bare key (B2.1): the user picked "Key", so an
                // attached certificate is never offered here. `Certificate`
                // is the cert-only method and `Auto` the smart one.
                let km = KeyMaterial::plain(km.private_pem);

                tracing::info!("Trying publickey auth for {}", username);
                if self.try_publickey_auth(handle, username, km).await? {
                    return Ok(true);
                }

                // Key was rejected, try password as fallback if available
                if let Some(pw) = password {
                    tracing::info!("Key rejected, trying password fallback for {}", username);
                    let res = handle.authenticate_password(username, pw).await?;
                    if res.success() {
                        return Ok(true);
                    }
                    return Err(SshError::Key("Both key and password rejected by server".into()));
                }

                Err(SshError::Key("Public key rejected by server".into()))
            }
            AuthMethod::Certificate => {
                // Certificate-only (B2.1): offer the attached OpenSSH user
                // certificate and nothing else. Unlike the degrade-friendly
                // `try_publickey_auth`, everything here is a hard error: the
                // user asked for exactly this credential, so a missing or
                // unusable cert must surface instead of silently landing on
                // a different auth path.
                let km = key_material
                    .ok_or_else(|| SshError::Key("No key selected".into()))?;
                let cert_line = km.certificate.ok_or_else(|| {
                    SshError::Key("The selected key has no attached certificate".into())
                })?;

                let private_key = russh::keys::decode_secret_key(km.private_pem, None)
                    .map_err(|e| SshError::Key(format!("Failed to decode key: {}", e)))?;
                let private_key = Arc::new(private_key);

                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                let cert = match check_certificate(cert_line, &private_key, now) {
                    CertCheck::Unusable(why) => {
                        return Err(SshError::Key(format!("Certificate unusable: {}", why)));
                    }
                    CertCheck::Offer { cert, expired } => {
                        if expired {
                            // Advisory only: the server's clock is authoritative.
                            tracing::warn!(
                                "Certificate for {} is expired; offering anyway",
                                username,
                            );
                        }
                        cert
                    }
                };
                tracing::info!("Trying certificate auth for {}", username);
                let res = handle
                    .authenticate_openssh_cert(username, private_key, *cert)
                    .await?;
                if !res.success() {
                    return Err(SshError::Key("Certificate rejected by server".into()));
                }
                Ok(true)
            }
            AuthMethod::Agent => {
                tracing::info!("Trying agent auth for {}", username);
                match self.auth_via_agent(handle, username).await {
                    Ok(AgentAuthOutcome::Authenticated) => Ok(true),
                    Ok(AgentAuthOutcome::NoMatch(tally)) => {
                        if let Some(pw) = password {
                            tracing::info!("Agent auth failed, trying password for {}", username);
                            let res = handle.authenticate_password(username, pw).await?;
                            if res.success() {
                                return Ok(true);
                            }
                        }
                        Err(SshError::Key(format!(
                            "Agent auth failed, no keys matched ({tally})"
                        )))
                    }
                    Err(e) => {
                        if let Some(pw) = password {
                            tracing::info!("Agent unavailable ({}), trying password for {}", e, username);
                            let res = handle.authenticate_password(username, pw).await?;
                            if res.success() {
                                return Ok(true);
                            }
                        }
                        Err(e)
                    }
                }
            }
            AuthMethod::Interactive => {
                tracing::info!("Trying keyboard-interactive auth for {}", username);
                match self.try_keyboard_interactive(handle, username, password, true).await? {
                    KbiOutcome::Success => Ok(true),
                    // Rejection and cancel both surfaced the same error
                    // before the outcome split; keep that behavior.
                    KbiOutcome::Rejected | KbiOutcome::Cancelled => {
                        Err(SshError::Key("Keyboard-interactive auth rejected".into()))
                    }
                }
            }
            AuthMethod::PasswordPrompt => {
                // Ask the UI for the password (never stored). The human wait
                // is unbounded; only the network exchange below is capped so
                // a server wedging after the user types can't hang forever.
                let pw = self
                    .prompt_password_once(password)
                    .await
                    .ok_or_else(|| SshError::Key("Password entry cancelled".into()))?;
                tracing::info!("Trying prompted password auth for {}", username);
                let res = tokio::time::timeout(
                    self.auth_timeout,
                    handle.authenticate_password(username, &pw),
                )
                .await
                .map_err(|_| {
                    SshError::ConnectionFailed(format!(
                        "auth timed out after {}s",
                        self.auth_timeout.as_secs()
                    ))
                })??;
                if !res.success() {
                    return Err(SshError::Key("Password rejected by server".into()));
                }
                Ok(true)
            }
        }
    }

    /// Ask the UI for a password once, for `AuthMethod::PasswordPrompt`.
    ///
    /// Sends a single-field, non-echoed prompt through `kbi_ask_tx` (the
    /// same bridge keyboard-interactive uses) and returns the typed value.
    /// Returns `None` when the user cancels or the UI bridge is gone.
    /// Headless callers (no `kbi_ask_tx`) fall back to `fallback_pw`, so
    /// MCP / boot port-forwards still authenticate without a modal.
    pub(crate) async fn prompt_password_once(&self, fallback_pw: Option<&str>) -> Option<String> {
        let Some(tx) = self.kbi_ask_tx.as_ref() else {
            return fallback_pw.map(|s| s.to_string());
        };
        let query = KbiQuery {
            name: self
                .pw_prompt_title
                .clone()
                .unwrap_or_else(|| "Enter Password".to_string()),
            instructions: String::new(),
            prompts: vec![KbiPromptField {
                prompt: self
                    .pw_prompt_label
                    .clone()
                    .unwrap_or_else(|| "Password".to_string()),
                echo: false,
            }],
        };
        let (resp_tx, resp_rx) = tokio::sync::oneshot::channel();
        if tx.send((query, resp_tx)).await.is_err() {
            // UI bridge dropped: treat as cancellation.
            return None;
        }
        match resp_rx.await {
            Ok(Some(mut answers)) => answers.drain(..).next(),
            // User cancelled, or the responder was dropped.
            Ok(None) | Err(_) => None,
        }
    }

    /// Try publickey auth, signing RSA keys with the hash the server actually
    /// accepts (`server_rsa_hash`) so legacy `ssh-rsa` / SHA-1 servers still
    /// authenticate instead of the client insisting on rsa-sha2-256.
    pub(crate) async fn try_publickey_auth(
        &self,
        handle: &mut client::Handle<ClientHandler>,
        username: &str,
        material: KeyMaterial<'_>,
    ) -> Result<bool, SshError> {
        let private_key = russh::keys::decode_secret_key(material.private_pem, None)
            .map_err(|e| SshError::Key(format!("Failed to decode key: {}", e)))?;
        let private_key = Arc::new(private_key);

        // If a certificate is attached, offer it first. Anything wrong with
        // the cert itself (unparseable, not this key's cert) degrades to a
        // plain publickey attempt instead of failing the whole auth: a bad
        // cert must never brick a host that could still authenticate with
        // the bare key. This matters because `AuthMethod::Key` propagates a
        // returned `Err` (skipping its password fallback), so cert trouble
        // is signalled by falling through, never by `Err`. Only a decode or
        // transport failure is a real `Err` here.
        if let Some(cert_line) = material.certificate {
            match self
                .try_certificate_auth(handle, username, &private_key, cert_line)
                .await?
            {
                Some(true) => return Ok(true),
                // Offered but the server rejected the cert, or the cert was
                // unusable: fall through to the bare key (OpenSSH treats
                // the cert and the plain key as separate identities).
                Some(false) | None => {
                    tracing::info!("Falling back to bare public key for {}", username);
                }
            }
        }

        // Plain publickey, signing RSA with the hash the server accepts.
        let hash = if private_key.algorithm().is_rsa() {
            server_rsa_hash(handle).await
        } else {
            None
        };
        let key = PrivateKeyWithHashAlg::new(private_key, hash);
        let res = handle.authenticate_publickey(username, key).await?;
        Ok(res.success())
    }

    /// Offer an OpenSSH certificate during publickey auth. Returns:
    /// - `Ok(Some(true))` the server accepted the certificate;
    /// - `Ok(Some(false))` offered, but the server rejected it;
    /// - `Ok(None)` the cert is unusable (unparseable, or it does not certify
    ///   this key) so the caller should try the bare key;
    /// - `Err(..)` a transport failure (propagated like plain auth).
    ///
    /// Expiry is advisory only: the server's clock is authoritative, so an
    /// expired cert is logged as a warning and still offered.
    async fn try_certificate_auth(
        &self,
        handle: &mut client::Handle<ClientHandler>,
        username: &str,
        private_key: &Arc<russh::keys::PrivateKey>,
        cert_line: &str,
    ) -> Result<Option<bool>, SshError> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let cert = match check_certificate(cert_line, private_key, now) {
            CertCheck::Unusable(why) => {
                tracing::warn!("Attached certificate unusable ({why}); using bare key");
                return Ok(None);
            }
            CertCheck::Offer { cert, expired } => {
                if expired {
                    tracing::warn!(
                        "Certificate for {} is expired; offering anyway (the server clock is authoritative)",
                        username,
                    );
                }
                cert
            }
        };
        let res = handle
            .authenticate_openssh_cert(username, private_key.clone(), *cert)
            .await?;
        Ok(Some(res.success()))
    }

    /// Drive a keyboard-interactive exchange to completion.
    ///
    /// `_start` is called once, then we loop on `_respond` round by round
    /// until the server returns `Success` or `Failure` (a single auth can
    /// span several `InfoRequest` rounds, e.g. password then OTP). The loop
    /// is bounded so a misbehaving server can't pop prompts forever.
    ///
    /// Each round's answers come from one of three sources, in order:
    /// - `use_callback` + a `kbi_ask_tx` channel: surface the prompts to the
    ///   UI and wait for typed answers. The user cancelling (`None`) aborts
    ///   the auth cleanly (`Cancelled`).
    /// - otherwise `fallback_pw`: answer every prompt with the stored
    ///   password (the Auto path, and the headless degrade path).
    /// - neither available: fail cleanly (`Rejected`).
    ///
    /// A round carrying zero prompts is answered with an empty response, so
    /// servers that send an informational-only `InfoRequest` keep advancing.
    ///
    /// The `Rejected` / `Cancelled` split matters to the quick-connect Auto
    /// fallback: a server refusal may still fall through to a prompted
    /// password attempt, while an explicit user cancel must never chain a
    /// second modal.
    pub(crate) async fn try_keyboard_interactive(
        &self,
        handle: &mut client::Handle<ClientHandler>,
        username: &str,
        fallback_pw: Option<&str>,
        use_callback: bool,
    ) -> Result<KbiOutcome, SshError> {
        // Cap on the number of challenge rounds. Real flows use 1-2; this is
        // just a backstop against a server that loops InfoRequest forever.
        const MAX_ROUNDS: usize = 16;

        // The outer auth-stage timeout is skipped for Interactive (it would
        // abort while the user types an OTP), so bound the individual network
        // round-trips here instead. The human-input wait below stays
        // unbounded but cancellable.
        let net_timeout = self.auth_timeout;
        let net_err = || {
            SshError::ConnectionFailed(format!(
                "keyboard-interactive server response timed out after {}s",
                net_timeout.as_secs()
            ))
        };

        let mut resp = tokio::time::timeout(
            net_timeout,
            handle.authenticate_keyboard_interactive_start(username, None::<String>),
        )
        .await
        .map_err(|_| net_err())??;

        // Guard for the TOTP autofill: only the FIRST OTP-looking round of
        // an attempt is answered automatically. A second one means the
        // server rejected the code (bad secret, clock drift), so the manual
        // modal takes over instead of feeding the same wrong code forever.
        let mut totp_used = false;

        for _ in 0..MAX_ROUNDS {
            let (name, instructions, prompts) = match resp {
                client::KeyboardInteractiveAuthResponse::Success => {
                    return Ok(KbiOutcome::Success);
                }
                client::KeyboardInteractiveAuthResponse::Failure { .. } => {
                    return Ok(KbiOutcome::Rejected);
                }
                client::KeyboardInteractiveAuthResponse::InfoRequest {
                    name,
                    instructions,
                    prompts,
                } => (name, instructions, prompts),
            };

            let autofill = if totp_used {
                None
            } else {
                autofill_kbi_round(
                    self.totp.as_ref(),
                    prompts.iter().map(|p| p.prompt.as_str()),
                    fallback_pw,
                )
            };

            let answers: Vec<String> = if prompts.is_empty() {
                Vec::new()
            } else if let Some(answers) = autofill {
                totp_used = true;
                answers
            } else if use_callback && let Some(tx) = self.kbi_ask_tx.as_ref() {
                let query = KbiQuery {
                    name,
                    instructions,
                    prompts: prompts
                        .iter()
                        .map(|p| KbiPromptField {
                            prompt: p.prompt.clone(),
                            echo: p.echo,
                        })
                        .collect(),
                };
                let (resp_tx, resp_rx) = tokio::sync::oneshot::channel();
                if tx.send((query, resp_tx)).await.is_err() {
                    // UI bridge is gone; treat as cancellation.
                    return Ok(KbiOutcome::Cancelled);
                }
                match resp_rx.await {
                    Ok(Some(answers)) => answers,
                    // User cancelled, or the responder dropped: abort cleanly.
                    Ok(None) | Err(_) => return Ok(KbiOutcome::Cancelled),
                }
            } else if let Some(pw) = fallback_pw {
                prompts.iter().map(|_| pw.to_string()).collect()
            } else {
                return Ok(KbiOutcome::Rejected);
            };

            resp = tokio::time::timeout(
                net_timeout,
                handle.authenticate_keyboard_interactive_respond(answers),
            )
            .await
            .map_err(|_| net_err())??;
        }

        tracing::warn!("keyboard-interactive exceeded {} rounds, giving up", MAX_ROUNDS);
        Ok(KbiOutcome::Rejected)
    }

    /// Authenticate and open a PTY session on the handle.
    /// Authenticate via ssh-agent. Uses Unix sockets on Linux/macOS,
    /// named pipes on Windows, trying EVERY discovered agent in order
    /// (issue #98): a live Pageant-style pipe serving zero keys (a
    /// locked KeePassXC keeps its pipe open) must not shadow the
    /// OpenSSH pipe that holds the working key. Keys already offered
    /// by an earlier agent are skipped so overlapping rosters can't
    /// burn the server's MaxAuthTries, and the per-agent tally rides
    /// the NoMatch outcome into the connection log so an empty agent
    /// is visible instead of a bare "no keys matched".
    #[cfg(unix)]
    pub(crate) async fn auth_via_agent(
        &self,
        handle: &mut client::Handle<ClientHandler>,
        username: &str,
    ) -> Result<AgentAuthOutcome, SshError> {
        let candidates = super::agent::unix_agent_sock_candidates(
            std::env::var("SSH_AUTH_SOCK").ok(),
            oryxis_core::agent_paths::unix_agent_socket_path(),
        );
        if candidates.is_empty() {
            return Err(SshError::Key(
                "ssh-agent not available: SSH_AUTH_SOCK is not set".into(),
            ));
        }
        let mut offered: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut report: Vec<String> = Vec::new();
        let mut connected_any = false;
        let mut last_err: Option<String> = None;
        for path in &candidates {
            let display = path.display().to_string();
            match russh::keys::agent::client::AgentClient::connect_uds(path).await {
                Ok(mut agent) => {
                    connected_any = true;
                    match agent.request_identities().await {
                        Ok(identities) => {
                            report.push(format!("{}: {} key(s)", display, identities.len()));
                            let fresh = filter_fresh_identities(identities, &mut offered);
                            if fresh.is_empty() {
                                continue;
                            }
                            if self
                                .try_agent_identities(handle, username, fresh, &mut agent)
                                .await?
                            {
                                return Ok(AgentAuthOutcome::Authenticated);
                            }
                        }
                        Err(e) => report.push(format!("{}: error {}", display, e)),
                    }
                }
                Err(e) => {
                    report.push(format!("{}: unavailable", display));
                    last_err = Some(format!("{}: {}", display, e));
                }
            }
        }
        if !connected_any {
            return Err(SshError::Key(format!(
                "ssh-agent not available: {}",
                last_err.unwrap_or_else(|| "no agent socket found".into()),
            )));
        }
        Ok(AgentAuthOutcome::NoMatch(report.join("; ")))
    }

    /// Authenticate via Windows ssh-agents (named pipes). Same
    /// fallback-chain contract as the Unix variant above.
    #[cfg(windows)]
    pub(crate) async fn auth_via_agent(
        &self,
        handle: &mut client::Handle<ClientHandler>,
        username: &str,
    ) -> Result<AgentAuthOutcome, SshError> {
        let candidates = super::agent::agent_pipe_candidates();
        let mut offered: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut report: Vec<String> = Vec::new();
        let mut connected_any = false;
        let mut last_err: Option<String> = None;
        for pipe_path in &candidates {
            // The `\\.\pipe\` prefix is noise in a user-facing tally.
            let display = pipe_path.trim_start_matches(r"\\.\pipe\");
            match russh::keys::agent::client::AgentClient::connect_named_pipe(pipe_path).await
            {
                Ok(mut agent) => {
                    connected_any = true;
                    match agent.request_identities().await {
                        Ok(identities) => {
                            report.push(format!("{}: {} key(s)", display, identities.len()));
                            let fresh = filter_fresh_identities(identities, &mut offered);
                            if fresh.is_empty() {
                                continue;
                            }
                            if self
                                .try_agent_identities(handle, username, fresh, &mut agent)
                                .await?
                            {
                                return Ok(AgentAuthOutcome::Authenticated);
                            }
                        }
                        Err(e) => report.push(format!("{}: error {}", display, e)),
                    }
                }
                Err(e) => {
                    report.push(format!("{}: unavailable", display));
                    last_err = Some(format!("{}: {}", display, e));
                }
            }
        }
        if !connected_any {
            return Err(SshError::Key(format!(
                "Windows ssh-agent not available: {}",
                last_err.unwrap_or_else(|| "no agent pipe found".into()),
            )));
        }
        Ok(AgentAuthOutcome::NoMatch(report.join("; ")))
    }

    /// The shared agent-auth loop: order the identities (the host's
    /// pinned key first, B3), then try each until one succeeds. NOTE:
    /// callers iterate several agents (see `auth_via_agent`); this
    /// runs one agent's roster.
    /// Certificate identities (an sk- cert loaded via `ssh-add`, or any
    /// agent-held cert) are offered as certificates; plain keys as
    /// publickey. The agent does the signing either way, so security-key
    /// signatures (authenticator flags + counter) pass through opaquely.
    async fn try_agent_identities<S>(
        &self,
        handle: &mut client::Handle<ClientHandler>,
        username: &str,
        identities: Vec<russh::keys::agent::AgentIdentity>,
        agent: &mut S,
    ) -> Result<bool, SshError>
    where
        S: russh::Signer,
    {
        // Server-advertised RSA hash is per-connection, resolved once
        // (not per key) so a multi-key agent doesn't burn MaxAuthTries.
        let rsa_hash = server_rsa_hash(handle).await;
        for identity in
            select_agent_identities(identities, self.pinned_agent_key.as_ref())
        {
            let pubkey = identity.public_key().into_owned();
            let hash = if pubkey.algorithm().is_rsa() { rsa_hash } else { None };
            let res = match identity {
                russh::keys::agent::AgentIdentity::Certificate { certificate, .. } => {
                    handle
                        .authenticate_certificate_with(username, certificate, hash, agent)
                        .await
                }
                russh::keys::agent::AgentIdentity::PublicKey { .. } => {
                    handle
                        .authenticate_publickey_with(username, pubkey, hash, agent)
                        .await
                }
            };
            if let Ok(res) = res
                && res.success()
            {
                return Ok(true);
            }
        }
        Ok(false)
    }

    pub(crate) async fn authenticate_and_open(
        &self,
        mut handle: client::Handle<ClientHandler>,
        connection: &Connection,
        password: Option<&str>,
        key_material: Option<KeyMaterial<'_>>,
        cols: u32,
        rows: u32,
    ) -> Result<(SshSession, mpsc::UnboundedReceiver<Vec<u8>>), SshError> {
        // Apply the same per-phase timeouts the public 2-step API uses
        //, single-call connects via `connect_with_resolver` were
        // bypassing them, leaving auth/session free to hang on the OS
        // default ceilings. Auth honours the Interactive exemption (human
        // input isn't a network stall) via `authenticate_handle_bounded`.
        let session_timeout = self.session_timeout;
        self.authenticate_handle_bounded(&mut handle, connection, password, key_material)
            .await?;
        let listeners = bind_port_forward_listeners(&connection.port_forwards).await?;
        let (mut session, rx) = tokio::time::timeout(
            session_timeout,
            self.open_pty_session(handle, cols, rows, listeners),
        )
        .await
        .map_err(|_| {
            SshError::ConnectionFailed(format!(
                "session open timed out after {}s",
                session_timeout.as_secs()
            ))
        })??;
        session.sftp_open_timeout = session_timeout;
        Ok((session, rx))
    }
}

/// Result of the multi-agent auth sweep in `auth_via_agent`. `NoMatch`
/// carries the per-agent key tally (endpoint: N key(s), ...) so the
/// surfaced error explains WHICH agent had nothing instead of a bare
/// "no keys matched" (issue #98).
pub(crate) enum AgentAuthOutcome {
    Authenticated,
    NoMatch(String),
}

/// Drop identities whose (kind, fingerprint) was already offered by an
/// earlier agent in the sweep, recording the survivors. Certificates
/// and plain keys with the same underlying key are DIFFERENT offers
/// (cert auth vs publickey auth), hence the kind tag.
fn filter_fresh_identities(
    identities: Vec<russh::keys::agent::AgentIdentity>,
    offered: &mut std::collections::HashSet<String>,
) -> Vec<russh::keys::agent::AgentIdentity> {
    identities
        .into_iter()
        .filter(|identity| {
            let kind = match identity {
                russh::keys::agent::AgentIdentity::Certificate { .. } => "cert",
                russh::keys::agent::AgentIdentity::PublicKey { .. } => "key",
            };
            let tag = format!(
                "{}:{}",
                kind,
                identity
                    .public_key()
                    .fingerprint(russh::keys::HashAlg::Sha256)
            );
            offered.insert(tag)
        })
        .collect()
}

/// Order agent identities so the pinned key (the host's referenced vault
/// key, B3) is offered FIRST, preserving the try-all fallback after it.
/// Comparison is on key data, so a certificate identity whose underlying
/// key matches the pin also sorts first. A pin matching nothing (dangling
/// `key_id`, key not loaded in the agent) leaves the order untouched.
/// Pure, so it unit-tests without an agent socket.
fn select_agent_identities(
    identities: Vec<russh::keys::agent::AgentIdentity>,
    pinned: Option<&russh::keys::PublicKey>,
) -> Vec<russh::keys::agent::AgentIdentity> {
    let Some(pinned) = pinned else {
        return identities;
    };
    let (mut matching, rest): (Vec<_>, Vec<_>) = identities
        .into_iter()
        .partition(|id| id.public_key().key_data() == pinned.key_data());
    matching.extend(rest);
    matching
}

/// The result of validating an attached certificate against its private
/// key, before any network round-trip. Pure so it is unit-testable
/// without a live server (the `authenticate_openssh_cert` call is not).
enum CertCheck {
    /// Parsed and certifies this key; offer it. `expired` drives an
    /// advisory warning only (the server's clock is authoritative). The
    /// certificate is boxed (it dwarfs the `Unusable` variant).
    Offer {
        cert: Box<russh::keys::Certificate>,
        expired: bool,
    },
    /// Unusable (unparseable, or it does not certify this key): the
    /// caller should fall back to the bare public key.
    Unusable(&'static str),
}

/// Validate `cert_line` against `private_key` at wall-clock `now_unix`
/// (0 = unknown, skips the expiry check). Never fails: a bad cert is a
/// `Unusable`, so the auth path can always degrade to the plain key.
fn check_certificate(
    cert_line: &str,
    private_key: &russh::keys::PrivateKey,
    now_unix: u64,
) -> CertCheck {
    let cert = match russh::keys::Certificate::from_openssh(cert_line) {
        Ok(c) => c,
        Err(_) => return CertCheck::Unusable("unparseable"),
    };
    // The certificate must certify exactly this private key.
    if cert.public_key() != private_key.public_key().key_data() {
        return CertCheck::Unusable("does not match the private key");
    }
    let expired = now_unix != 0 && cert.valid_before() != 0 && now_unix > cert.valid_before();
    CertCheck::Offer { cert: Box::new(cert), expired }
}

#[cfg(test)]
mod cert_tests {
    use super::{check_certificate, CertCheck};
    use russh::keys::ssh_key::{certificate, Algorithm, PrivateKey};

    /// A CA-signed user certificate for `user_key`, valid across `now`,
    /// as its OpenSSH public line.
    fn make_cert(user_key: &PrivateKey, valid_before: u64) -> String {
        let ca = PrivateKey::random(&mut rand010::rng(), Algorithm::Ed25519).unwrap();
        let mut builder = certificate::Builder::new_with_random_nonce(
            &mut rand010::rng(),
            user_key.public_key(),
            0, // valid_after: the beginning of time
            valid_before,
        )
        .unwrap();
        builder.serial(1).unwrap();
        builder.key_id("t").unwrap();
        builder.cert_type(certificate::CertType::User).unwrap();
        builder.valid_principal("tester").unwrap();
        builder.sign(&ca).unwrap().to_openssh().unwrap()
    }

    #[test]
    fn matching_cert_is_offered() {
        let key = PrivateKey::random(&mut rand010::rng(), Algorithm::Ed25519).unwrap();
        let cert = make_cert(&key, 4_000_000_000); // far future
        match check_certificate(&cert, &key, 1_700_000_000) {
            CertCheck::Offer { expired, .. } => assert!(!expired),
            CertCheck::Unusable(w) => panic!("expected offer, got {w}"),
        }
    }

    #[test]
    fn expired_cert_is_still_offered_flagged() {
        let key = PrivateKey::random(&mut rand010::rng(), Algorithm::Ed25519).unwrap();
        let cert = make_cert(&key, 1_000); // long past
        match check_certificate(&cert, &key, 1_700_000_000) {
            CertCheck::Offer { expired, .. } => assert!(expired, "should flag expiry"),
            CertCheck::Unusable(w) => panic!("expired cert must still be offered, got {w}"),
        }
    }

    #[test]
    fn cert_for_another_key_is_unusable() {
        let key = PrivateKey::random(&mut rand010::rng(), Algorithm::Ed25519).unwrap();
        let other = PrivateKey::random(&mut rand010::rng(), Algorithm::Ed25519).unwrap();
        let cert = make_cert(&other, 4_000_000_000); // certifies `other`, not `key`
        assert!(matches!(
            check_certificate(&cert, &key, 1_700_000_000),
            CertCheck::Unusable(_)
        ));
    }

    #[test]
    fn garbage_cert_line_is_unusable() {
        let key = PrivateKey::random(&mut rand010::rng(), Algorithm::Ed25519).unwrap();
        assert!(matches!(
            check_certificate("not a certificate", &key, 0),
            CertCheck::Unusable(_)
        ));
    }
}

#[cfg(test)]
mod agent_pin_tests {
    use super::select_agent_identities;
    use russh::keys::agent::AgentIdentity;
    use russh::keys::PublicKey;

    // Public security-key fixture from the ssh-key crate's test suite
    // (public material only, nothing secret).
    const SK_ED25519_PUB: &str = "sk-ssh-ed25519@openssh.com AAAAGnNrLXNzaC1lZDI1NTE5QG9wZW5zc2guY29tAAAAICFo/k5LU8863u66YC9eUO2170QduohPURkQnbLa/dczAAAABHNzaDo= user@example.com";

    fn plain(seed: u8) -> AgentIdentity {
        // Deterministic distinct Ed25519 keys derived from a seed byte.
        use russh::keys::ssh_key;
        let secret = ssh_key::private::Ed25519Keypair::from_seed(&[seed; 32]);
        AgentIdentity::PublicKey {
            key: PublicKey::new(ssh_key::public::KeyData::Ed25519(secret.public), ""),
            comment: format!("key-{seed}"),
        }
    }

    fn sk_identity() -> AgentIdentity {
        AgentIdentity::from(PublicKey::from_openssh(SK_ED25519_PUB).unwrap())
    }

    fn labels(ids: &[AgentIdentity]) -> Vec<String> {
        ids.iter().map(|i| i.comment().to_string()).collect()
    }

    #[test]
    fn no_pin_keeps_order() {
        let ids = vec![plain(1), plain(2), sk_identity()];
        let expect = labels(&ids);
        let ordered = select_agent_identities(ids, None);
        assert_eq!(labels(&ordered), expect);
    }

    #[test]
    fn pinned_identity_moves_first_and_rest_follow() {
        let pinned = PublicKey::from_openssh(SK_ED25519_PUB).unwrap();
        let ids = vec![plain(1), plain(2), sk_identity(), plain(3)];
        let ordered = select_agent_identities(ids, Some(&pinned));
        assert_eq!(ordered.len(), 4);
        assert_eq!(
            ordered[0].public_key().key_data(),
            pinned.key_data(),
            "pinned identity must be offered first"
        );
        // Try-all fallback preserved in original relative order.
        assert_eq!(labels(&ordered)[1..], ["key-1", "key-2", "key-3"]);
    }

    #[test]
    fn dangling_pin_leaves_order_untouched() {
        let pinned = PublicKey::from_openssh(SK_ED25519_PUB).unwrap();
        let ids = vec![plain(1), plain(2)];
        let expect = labels(&ids);
        let ordered = select_agent_identities(ids, Some(&pinned));
        assert_eq!(labels(&ordered), expect);
    }
}
