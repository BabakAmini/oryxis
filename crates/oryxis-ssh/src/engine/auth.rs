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
        private_key_pem: Option<&str>,
    ) -> Result<(), SshError> {
        let username = connection.username.as_deref().unwrap_or("root");
        let has_pw = password.is_some();
        let has_key = private_key_pem.is_some();
        tracing::info!(
            "Auth for {}@{} method={:?} has_password={} has_key={}",
            username, connection.hostname, connection.auth_method, has_pw, has_key,
        );

        match self
            .do_auth(handle, username, &connection.auth_method, password, private_key_pem)
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
        private_key_pem: Option<&str>,
    ) -> Result<bool, SshError> {
        match auth_method {
            AuthMethod::Auto => {
                let mut tried: Vec<&str> = Vec::new();

                // 1. Try publickey if a key is provided
                if let Some(pem) = private_key_pem {
                    tried.push("publickey");
                    tracing::info!("Auto: trying publickey auth for {}", username);
                    match self.try_publickey_auth(handle, username, pem).await {
                        Ok(true) => return Ok(true),
                        Ok(false) => tracing::info!("Auto: publickey rejected"),
                        Err(e) => tracing::info!("Auto: publickey error: {}", e),
                    }
                }

                // 2. Try agent auth
                tried.push("agent");
                tracing::info!("Auto: trying agent auth for {}", username);
                match self.auth_via_agent(handle, username).await {
                    Ok(true) => return Ok(true),
                    Ok(false) => tracing::info!("Auto: agent had no matching keys"),
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
                let pem = private_key_pem
                    .ok_or_else(|| SshError::Key("No private key selected".into()))?;

                tracing::info!("Trying publickey auth for {}", username);
                if self.try_publickey_auth(handle, username, pem).await? {
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
            AuthMethod::Agent => {
                tracing::info!("Trying agent auth for {}", username);
                match self.auth_via_agent(handle, username).await {
                    Ok(true) => Ok(true),
                    Ok(false) => {
                        if let Some(pw) = password {
                            tracing::info!("Agent auth failed, trying password for {}", username);
                            let res = handle.authenticate_password(username, pw).await?;
                            if res.success() {
                                return Ok(true);
                            }
                        }
                        Err(SshError::Key("Agent auth failed, no keys matched".into()))
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
        pem: &str,
    ) -> Result<bool, SshError> {
        let private_key = russh::keys::decode_secret_key(pem, None)
            .map_err(|e| SshError::Key(format!("Failed to decode key: {}", e)))?;
        let hash = if private_key.algorithm().is_rsa() {
            server_rsa_hash(handle).await
        } else {
            None
        };
        let key = PrivateKeyWithHashAlg::new(Arc::new(private_key), hash);
        let res = handle.authenticate_publickey(username, key).await?;
        Ok(res.success())
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
    /// Authenticate via ssh-agent. Uses Unix socket on Linux/macOS, named pipe on Windows.
    #[cfg(unix)]
    pub(crate) async fn auth_via_agent(
        &self,
        handle: &mut client::Handle<ClientHandler>,
        username: &str,
    ) -> Result<bool, SshError> {
        match russh::keys::agent::client::AgentClient::connect_env().await {
            Ok(mut agent) => {
                let identities = agent
                    .request_identities()
                    .await
                    .map_err(|e| SshError::Key(format!("Agent: {}", e)))?;

                // Server-advertised RSA hash is per-connection, resolved once
                // (not per key) so a multi-key agent doesn't burn MaxAuthTries.
                let rsa_hash = server_rsa_hash(handle).await;
                for identity in identities {
                    let pubkey = identity.public_key().into_owned();
                    let hash = if pubkey.algorithm().is_rsa() { rsa_hash } else { None };
                    if let Ok(res) = handle
                        .authenticate_publickey_with(username, pubkey, hash, &mut agent)
                        .await
                    && res.success() {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
            Err(e) => Err(SshError::Key(format!("ssh-agent not available: {}", e))),
        }
    }

    /// Authenticate via Windows OpenSSH Agent (named pipe).
    #[cfg(windows)]
    pub(crate) async fn auth_via_agent(
        &self,
        handle: &mut client::Handle<ClientHandler>,
        username: &str,
    ) -> Result<bool, SshError> {
        let pipe_path = windows_agent_pipe();
        match russh::keys::agent::client::AgentClient::connect_named_pipe(&pipe_path).await {
            Ok(mut agent) => {
                let identities = agent
                    .request_identities()
                    .await
                    .map_err(|e| SshError::Key(format!("Agent: {}", e)))?;

                // Server-advertised RSA hash is per-connection, resolved once
                // (not per key) so a multi-key agent doesn't burn MaxAuthTries.
                let rsa_hash = server_rsa_hash(handle).await;
                for identity in identities {
                    let pubkey = identity.public_key().into_owned();
                    let hash = if pubkey.algorithm().is_rsa() { rsa_hash } else { None };
                    if let Ok(res) = handle
                        .authenticate_publickey_with(username, pubkey, hash, &mut agent)
                        .await
                    && res.success() {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
            Err(e) => Err(SshError::Key(format!(
                "Windows ssh-agent not available ({}): {}",
                pipe_path, e
            ))),
        }
    }

    pub(crate) async fn authenticate_and_open(
        &self,
        mut handle: client::Handle<ClientHandler>,
        connection: &Connection,
        password: Option<&str>,
        private_key_pem: Option<&str>,
        cols: u32,
        rows: u32,
    ) -> Result<(SshSession, mpsc::UnboundedReceiver<Vec<u8>>), SshError> {
        // Apply the same per-phase timeouts the public 2-step API uses
        //, single-call connects via `connect_with_resolver` were
        // bypassing them, leaving auth/session free to hang on the OS
        // default ceilings. Auth honours the Interactive exemption (human
        // input isn't a network stall) via `authenticate_handle_bounded`.
        let session_timeout = self.session_timeout;
        self.authenticate_handle_bounded(&mut handle, connection, password, private_key_pem)
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
