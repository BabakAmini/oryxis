use super::*;

impl SshEngine {
    /// Connect to a remote host with full pipeline support:
    /// - Direct TCP connection
    /// - SOCKS4/5 proxy
    /// - HTTP CONNECT proxy
    /// - ProxyCommand (spawn process as transport)
    /// - Jump hosts (chained SSH connections via direct-tcpip channels)
    pub async fn connect(
        &self,
        connection: &Connection,
        password: Option<&str>,
        private_key_pem: Option<&str>,
        cols: u32,
        rows: u32,
    ) -> Result<(SshSession, mpsc::UnboundedReceiver<Vec<u8>>), SshError> {
        self.connect_with_resolver(connection, password, private_key_pem, cols, rows, None)
            .await
    }

    /// Connect with a resolver for jump host credentials. Wraps the
    /// transport setup in `connect_timeout` so the SFTP picker (which
    /// goes through here) doesn't fall through to the kernel's ~127s
    /// SYN-retransmit ceiling on unreachable hosts.
    /// Establish the raw TCP+SSH transport handle: jump chain first, then
    /// a proxy, else a direct dial, all under the connect timeout so an
    /// unreachable host fails fast instead of hanging on SYN retransmits.
    /// Shared by `connect_with_resolver` and `establish_transport`.
    pub(crate) async fn dial(
        &self,
        connection: &Connection,
        resolver: Option<&ConnectionResolver>,
    ) -> Result<client::Handle<ClientHandler>, SshError> {
        let target_host = &connection.hostname;
        let target_port = connection.port;
        // Brackets bare IPv6 literals; hostnames/IPv4 pass through.
        let addr = oryxis_core::net::host_port(target_host, target_port);
        let connect_timeout = self.connect_timeout;

        tracing::info!(
            "SSH connecting to {} (timeout: {}s)",
            addr,
            connect_timeout.as_secs()
        );

        let connect_fut = async {
            if !connection.jump_chain.is_empty() {
                self.connect_via_jump_hosts(connection, resolver, &addr).await
            } else if let Some(proxy) = &connection.proxy {
                self.connect_via_proxy(proxy, target_host, target_port, self.address_family)
                    .await
            } else {
                let config = self.make_config();
                let handler = self.make_handler(target_host, target_port);
                // Dial ourselves (instead of `client::connect`) so the
                // socket honors the address-family preference and gets
                // TCP_NODELAY before the SSH handshake starts.
                let stream = self.dial_tcp(&addr, self.address_family).await?;
                client::connect_stream(config, stream, handler)
                    .await
                    .map_err(|e| {
                        // Keep the structured negotiation failure (already an
                        // `SshError::Russh(NoCommonAlgo)` via the handler's
                        // `From`) so the UI can offer the legacy-algorithm
                        // fallback instead of a dead-end error string.
                        if e.negotiation_failure().is_some() {
                            e
                        } else {
                            SshError::ConnectionFailed(format!("{}: {}", addr, e))
                        }
                    })
            }
        };
        tokio::time::timeout(connect_timeout, connect_fut)
            .await
            .map_err(|_| {
                SshError::ConnectionFailed(format!(
                    "{}: timed out after {}s",
                    addr,
                    connect_timeout.as_secs()
                ))
            })?
    }

    pub async fn connect_with_resolver(
        &self,
        connection: &Connection,
        password: Option<&str>,
        private_key_pem: Option<&str>,
        cols: u32,
        rows: u32,
        resolver: Option<&ConnectionResolver>,
    ) -> Result<(SshSession, mpsc::UnboundedReceiver<Vec<u8>>), SshError> {
        let handle = self.dial(connection, resolver).await?;

        self.authenticate_and_open(handle, connection, password, private_key_pem, cols, rows)
            .await
    }

    /// Step 1: Establish TCP transport (direct, proxy, or jump host).
    /// Returns an opaque handle after successful TCP connection + SSH handshake + host key verification.
    ///
    /// Wrapped in a 15-second timeout so unreachable hosts fail fast instead of
    /// hanging on TCP SYN retransmits (Linux default: ~127s for SYN retries).
    pub async fn establish_transport(
        &self,
        connection: &Connection,
        resolver: Option<&ConnectionResolver>,
    ) -> Result<SshHandle, SshError> {
        let handle = self.dial(connection, resolver).await?;
        Ok(SshHandle(handle))
    }

    /// Step 2: Authenticate on an established handle. Configurable
    /// timeout (default 30s) so a misbehaving server wedging mid-
    /// handshake can't hang the connect flow forever.
    pub async fn do_authenticate(
        &self,
        handle: &mut SshHandle,
        connection: &Connection,
        password: Option<&str>,
        private_key_pem: Option<&str>,
    ) -> Result<(), SshError> {
        self.authenticate_handle_bounded(&mut handle.0, connection, password, private_key_pem)
            .await
    }

    /// Run `authenticate_handle` under the auth-stage timeout, EXCEPT for
    /// `AuthMethod::Interactive`. Interactive parks on human input (reading
    /// a prompt, fetching an OTP from a phone), which routinely exceeds any
    /// sane network bound, so the blanket `auth_timeout` would abort the very
    /// 2FA flow it's meant to protect. For Interactive the network
    /// round-trips are bounded individually inside `try_keyboard_interactive`
    /// instead, so a misbehaving server is still capped while a slow human is
    /// not. The user can always cancel the prompt to fail the auth cleanly.
    pub(crate) async fn authenticate_handle_bounded(
        &self,
        handle: &mut client::Handle<ClientHandler>,
        connection: &Connection,
        password: Option<&str>,
        private_key_pem: Option<&str>,
    ) -> Result<(), SshError> {
        // Interactive and PasswordPrompt both park on human input, which
        // routinely exceeds any network bound. Their network round-trips
        // are capped individually inside the auth path instead, so the
        // blanket `auth_timeout` is skipped here for both. Auto joins them
        // when the quick-connect interactive fallback can prompt: its tail
        // may park on the same modal.
        let may_prompt = self.auto_interactive_fallback && self.kbi_ask_tx.is_some();
        if matches!(
            connection.auth_method,
            AuthMethod::Interactive | AuthMethod::PasswordPrompt
        ) || (connection.auth_method == AuthMethod::Auto && may_prompt)
        {
            return self
                .authenticate_handle(handle, connection, password, private_key_pem)
                .await;
        }
        let auth_timeout = self.auth_timeout;
        tokio::time::timeout(
            auth_timeout,
            self.authenticate_handle(handle, connection, password, private_key_pem),
        )
        .await
        .map_err(|_| {
            SshError::ConnectionFailed(format!(
                "auth timed out after {}s",
                auth_timeout.as_secs()
            ))
        })?
    }

    /// Step 3: Open PTY session on an authenticated handle. The session
    /// timeout (default 10s) covers the channel-open + pty-request +
    /// shell-request chain.
    pub async fn open_session(
        &self,
        handle: SshHandle,
        cols: u32,
        rows: u32,
        port_forwards: &[PortForward],
    ) -> Result<(SshSession, mpsc::UnboundedReceiver<Vec<u8>>), SshError> {
        let session_timeout = self.session_timeout;
        let listeners = bind_port_forward_listeners(port_forwards).await?;
        tokio::time::timeout(
            session_timeout,
            self.open_pty_session(handle.0, cols, rows, listeners),
        )
        .await
        .map_err(|_| {
            SshError::ConnectionFailed(format!(
                "session open timed out after {}s",
                session_timeout.as_secs()
            ))
        })?
        .map(|(mut session, rx)| {
            // Propagate the SFTP-open timeout so siblings opened later
            // honour the same configured limit.
            session.sftp_open_timeout = session_timeout;
            (session, rx)
        })
    }

    /// Open a standalone port forward (no PTY). Runs the same transport +
    /// auth ladder as a terminal connect, then binds the forward listener
    /// instead of requesting a shell. The returned `ForwardSession` holds
    /// the connection open until cancelled.
    ///
    /// Consumes `self` because a remote (`-R`) forward must install the
    /// inbound-channel sink on the handler *before* the transport (and thus
    /// the handler) is created.
    /// Open a `-L` forward on an OS-assigned ephemeral local port and
    /// report the port back, so a caller (the RDP/VNC launcher) can
    /// point a client at `127.0.0.1:<port>` with no bind race: the
    /// listener owns the port before we return it. The returned
    /// `ForwardSession` keeps the tunnel up until dropped / cancelled;
    /// its lifetime is deliberately independent of any client process.
    pub async fn connect_local_forward_ephemeral(
        self,
        connection: &Connection,
        password: Option<&str>,
        private_key_pem: Option<&str>,
        target_host: &str,
        target_port: u16,
        resolver: Option<&ConnectionResolver>,
    ) -> Result<(ForwardSession, u16), SshError> {
        let (cancel_tx, cancel_rx) = tokio::sync::watch::channel(false);
        let cancel_tx = Arc::new(cancel_tx);
        let mut handle = self.establish_transport(connection, resolver).await?;
        self.do_authenticate(&mut handle, connection, password, private_key_pem)
            .await?;
        let shared = Arc::new(tokio::sync::Mutex::new(handle.0));

        // Port 0 -> the OS picks a free port; read it back from the bound
        // listener before spawning, so what we return is what's bound.
        let listener = bind_forward_listener("127.0.0.1", 0).await?;
        let local_port = listener
            .local_addr()
            .map_err(|e| SshError::Channel(format!("forward local_addr: {e}")))?
            .port();
        // Auto-close: unlike a saved `-L` rule, this tunnel exists only to
        // carry one desktop session. Once it has served a connection and then
        // sits idle (client window closed), tear it down so the SSH handle
        // doesn't linger. Independent of any client process, so it works
        // uniformly for blocking viewers (xfreerdp) and handoff launchers
        // (`open rdp://`, remmina) alike.
        let task = spawn_autoclose_local_forward_task(
            listener,
            Arc::clone(&shared),
            target_host.to_string(),
            target_port,
            local_port,
            cancel_rx,
            Arc::clone(&cancel_tx),
            RD_TUNNEL_IDLE_GRACE,
        );
        tracing::info!(
            "forward(-L ephemeral) 127.0.0.1:{} -> {}:{} up",
            local_port, target_host, target_port
        );
        Ok((
            ForwardSession {
                handle: shared,
                cancel_tx,
                _tasks: vec![task],
                remote_bind: None,
            },
            local_port,
        ))
    }

    pub async fn connect_forward(
        mut self,
        connection: &Connection,
        password: Option<&str>,
        private_key_pem: Option<&str>,
        rule: &PortForwardRule,
        resolver: Option<&ConnectionResolver>,
    ) -> Result<ForwardSession, SshError> {
        let (cancel_tx, cancel_rx) = tokio::sync::watch::channel(false);
        let cancel_tx = Arc::new(cancel_tx);

        // Remote forwards need the handler to route inbound `forwarded-tcpip`
        // channels, so wire the sink before `establish_transport` builds it.
        let remote_rx = if rule.kind == ForwardKind::Remote {
            let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
            self.forwarded_channel_sink = Some(tx);
            Some(rx)
        } else {
            None
        };

        let mut handle = self.establish_transport(connection, resolver).await?;
        self.do_authenticate(&mut handle, connection, password, private_key_pem)
            .await?;
        let shared = Arc::new(tokio::sync::Mutex::new(handle.0));

        match rule.kind {
            ForwardKind::Local => {
                let listener =
                    bind_forward_listener(&rule.listen_host, rule.listen_port).await?;
                let task = spawn_local_forward_task(
                    listener,
                    Arc::clone(&shared),
                    rule.target_host.clone(),
                    rule.target_port,
                    rule.listen_port,
                    cancel_rx,
                );
                tracing::info!(
                    "forward(-L) {}:{} -> {}:{} up",
                    rule.listen_host, rule.listen_port, rule.target_host, rule.target_port
                );
                Ok(ForwardSession {
                    handle: shared,
                    cancel_tx,
                    _tasks: vec![task],
                    remote_bind: None,
                })
            }
            ForwardKind::Remote => {
                // Ask the server to listen on `listen_host:listen_port` and
                // tunnel inbound connections back to us. A denied request
                // (e.g. `AllowTcpForwarding no`) fails the toggle.
                {
                    let h = shared.lock().await;
                    h.tcpip_forward(rule.listen_host.clone(), rule.listen_port as u32)
                        .await
                        .map_err(|e| {
                            SshError::Channel(format!("remote forward request denied: {e}"))
                        })?;
                }
                let mut rx = remote_rx.expect("remote sink set above for -R");
                let target_host = rule.target_host.clone();
                let target_port = rule.target_port;
                let mut cancel = cancel_rx;
                let task = tokio::spawn(async move {
                    loop {
                        tokio::select! {
                            _ = cancel.changed() => break,
                            ch = rx.recv() => match ch {
                                Some(channel) => {
                                    let th = target_host.clone();
                                    let child_cancel = cancel.clone();
                                    tokio::spawn(async move {
                                        bridge_channel_to_target(
                                            channel, th, target_port, child_cancel,
                                        )
                                        .await;
                                    });
                                }
                                None => break,
                            },
                        }
                    }
                });
                tracing::info!(
                    "forward(-R) server {}:{} -> local {}:{} up",
                    rule.listen_host, rule.listen_port, rule.target_host, rule.target_port
                );
                Ok(ForwardSession {
                    handle: shared,
                    cancel_tx,
                    _tasks: vec![task],
                    remote_bind: Some((rule.listen_host.clone(), rule.listen_port)),
                })
            }
            ForwardKind::Dynamic => {
                let listener =
                    bind_forward_listener(&rule.listen_host, rule.listen_port).await?;
                let task = spawn_dynamic_forward_task(
                    listener,
                    Arc::clone(&shared),
                    rule.listen_port,
                    cancel_rx,
                );
                tracing::info!(
                    "forward(-D) SOCKS5 {}:{} up",
                    rule.listen_host, rule.listen_port
                );
                Ok(ForwardSession {
                    handle: shared,
                    cancel_tx,
                    _tasks: vec![task],
                    remote_bind: None,
                })
            }
        }
    }

    // -----------------------------------------------------------------------
    // Transport resolvers
    // -----------------------------------------------------------------------

    /// Connect via SOCKS or HTTP proxy. `family` governs the socket to
    /// the PROXY (the only dial this machine makes on this path); it is
    /// the target connection's preference, or the bastion's when the
    /// proxied hop is a jump chain's first host.
    pub(crate) async fn connect_via_proxy(
        &self,
        proxy: &ProxyConfig,
        target_host: &str,
        target_port: u16,
        family: AddressFamily,
    ) -> Result<client::Handle<ClientHandler>, SshError> {
        let proxy_addr = oryxis_core::net::host_port(&proxy.host, proxy.port);
        tracing::info!("Connecting via {:?} proxy at {}", proxy.proxy_type, proxy_addr);

        match &proxy.proxy_type {
            ProxyType::Socks5 => {
                // Dial the proxy ourselves (family + TCP_NODELAY), then
                // run the SOCKS handshake over the prepared socket.
                let socket = self
                    .dial_tcp(&proxy_addr, family)
                    .await
                    .map_err(|e| SshError::Proxy(format!("SOCKS5 proxy connect: {}", e)))?;
                let stream = if let Some(user) = &proxy.username {
                    // SOCKS5 username/password auth (RFC 1929). Password
                    // is hydrated from the vault before this call; if
                    // the user configured no password, send an empty
                    // one, the proxy may still accept it.
                    tokio_socks::tcp::Socks5Stream::connect_with_password_and_socket(
                        socket,
                        (target_host, target_port),
                        user.as_str(),
                        proxy.password.as_deref().unwrap_or(""),
                    )
                    .await
                    .map_err(|e| SshError::Proxy(format!("SOCKS5 auth: {}", e)))?
                } else {
                    tokio_socks::tcp::Socks5Stream::connect_with_socket(
                        socket,
                        (target_host, target_port),
                    )
                    .await
                    .map_err(|e| SshError::Proxy(format!("SOCKS5: {}", e)))?
                };

                let config = self.make_config();
                client::connect_stream(config, stream, self.make_handler(target_host, target_port))
                    .await
                    .map_err(|e| SshError::Proxy(format!("SSH over SOCKS5: {}", e)))
            }
            ProxyType::Socks4 => {
                let socket = self
                    .dial_tcp(&proxy_addr, family)
                    .await
                    .map_err(|e| SshError::Proxy(format!("SOCKS4 proxy connect: {}", e)))?;
                let stream = if let Some(user) = &proxy.username {
                    tokio_socks::tcp::Socks4Stream::connect_with_userid_and_socket(
                        socket,
                        (target_host, target_port),
                        user.as_str(),
                    )
                    .await
                    .map_err(|e| SshError::Proxy(format!("SOCKS4: {}", e)))?
                } else {
                    tokio_socks::tcp::Socks4Stream::connect_with_socket(
                        socket,
                        (target_host, target_port),
                    )
                    .await
                    .map_err(|e| SshError::Proxy(format!("SOCKS4: {}", e)))?
                };

                let config = self.make_config();
                client::connect_stream(config, stream, self.make_handler(target_host, target_port))
                    .await
                    .map_err(|e| SshError::Proxy(format!("SSH over SOCKS4: {}", e)))
            }
            ProxyType::Http => {
                let stream = self
                    .http_connect_tunnel(
                        &proxy_addr,
                        target_host,
                        target_port,
                        proxy.username.as_deref(),
                        proxy.password.as_deref(),
                        family,
                    )
                    .await?;

                let config = self.make_config();
                client::connect_stream(config, stream, self.make_handler(target_host, target_port))
                    .await
                    .map_err(|e| SshError::Proxy(format!("SSH over HTTP CONNECT: {}", e)))
            }
            ProxyType::Command(cmd) => {
                let stream = self.proxy_command(cmd).await?;

                let config = self.make_config();
                client::connect_stream(config, stream, self.make_handler(target_host, target_port))
                    .await
                    .map_err(|e| SshError::Proxy(format!("SSH over ProxyCommand: {}", e)))
            }
        }
    }

    /// HTTP CONNECT tunnel, establish a TCP tunnel through an HTTP proxy.
    /// Supports Basic auth (RFC 7617) when `username` is provided.
    pub(crate) async fn http_connect_tunnel(
        &self,
        proxy_addr: &str,
        target_host: &str,
        target_port: u16,
        username: Option<&str>,
        password: Option<&str>,
        family: AddressFamily,
    ) -> Result<TcpStream, SshError> {
        let mut stream = self
            .dial_tcp(proxy_addr, family)
            .await
            .map_err(|e| SshError::Proxy(format!("HTTP proxy connect: {}", e)))?;

        let connect_req = build_http_connect_request(target_host, target_port, username, password);

        stream
            .write_all(connect_req.as_bytes())
            .await
            .map_err(|e| SshError::Proxy(format!("HTTP CONNECT write: {}", e)))?;

        // Read until end-of-headers ("\r\n\r\n"). A single read() typically
        // delivers the whole CONNECT response on first packet, but a hostile
        // or chunked proxy may split it, loop until we have headers or hit
        // a 16 KiB cap (HTTP requests this small never exceed that).
        let mut buf = Vec::with_capacity(1024);
        let mut chunk = [0u8; 1024];
        loop {
            let n = tokio::io::AsyncReadExt::read(&mut stream, &mut chunk)
                .await
                .map_err(|e| SshError::Proxy(format!("HTTP CONNECT read: {}", e)))?;
            if n == 0 {
                return Err(SshError::Proxy(
                    "HTTP CONNECT: proxy closed before response".into(),
                ));
            }
            buf.extend_from_slice(&chunk[..n]);
            if buf.windows(4).any(|w| w == b"\r\n\r\n") || buf.len() > 16 * 1024 {
                break;
            }
        }

        match parse_http_status(&buf) {
            Some(200) => {
                tracing::info!("HTTP CONNECT tunnel established");
                Ok(stream)
            }
            Some(407) => Err(SshError::Proxy(
                "HTTP CONNECT failed: 407 Proxy Authentication Required".into(),
            )),
            Some(code) => Err(SshError::Proxy(format!(
                "HTTP CONNECT failed: status {}",
                code
            ))),
            None => Err(SshError::Proxy(format!(
                "HTTP CONNECT failed: unparseable response \"{}\"",
                String::from_utf8_lossy(&buf).lines().next().unwrap_or("")
            ))),
        }
    }

    /// ProxyCommand, spawn a process and use its stdin/stdout as transport.
    pub(crate) async fn proxy_command(
        &self,
        cmd: &str,
    ) -> Result<impl AsyncRead + AsyncWrite + Unpin + Send + 'static, SshError> {
        tracing::info!("ProxyCommand: {}", cmd);

        let mut child = TokioCommand::new("sh")
            .arg("-c")
            .arg(cmd)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| SshError::Proxy(format!("ProxyCommand spawn: {}", e)))?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| SshError::Proxy("ProxyCommand: no stdin".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| SshError::Proxy("ProxyCommand: no stdout".into()))?;

        Ok(tokio::io::join(stdout, stdin))
    }

    /// Connect via jump hosts (SSH tunneling through bastion hosts).
    pub(crate) async fn connect_via_jump_hosts(
        &self,
        connection: &Connection,
        resolver: Option<&ConnectionResolver>,
        final_addr: &str,
    ) -> Result<client::Handle<ClientHandler>, SshError> {
        let resolver = resolver.ok_or_else(|| {
            SshError::JumpHost("Jump hosts require a connection resolver".into())
        })?;

        tracing::info!(
            "Connecting via {} jump host(s)",
            connection.jump_chain.len()
        );

        // Connect to the first jump host. If the jump itself sits
        // behind a proxy, dial via that proxy, only the *first* hop
        // does, since subsequent hops travel inside the SSH tunnel.
        let first_jump_id = connection.jump_chain[0];
        let first_jump = resolver
            .connections
            .iter()
            .find(|c| c.id == first_jump_id)
            .ok_or_else(|| SshError::JumpHost("First jump host not found".into()))?;

        let first_addr = oryxis_core::net::host_port(&first_jump.hostname, first_jump.port);
        let mut current_handle = if let Some(first_proxy) = resolver.proxies.get(&first_jump_id) {
            tracing::info!(
                "First jump host {} sits behind {:?} proxy",
                first_addr,
                first_proxy.proxy_type
            );
            self.connect_via_proxy(
                first_proxy,
                &first_jump.hostname,
                first_jump.port,
                first_jump.address_family,
            )
            .await
            .map_err(|e| SshError::JumpHost(format!("Jump host {} via proxy: {}", first_addr, e)))?
        } else {
            let config = self.make_config();
            let handler = self.make_handler(&first_jump.hostname, first_jump.port);
            // The socket goes to the BASTION, so its address-family
            // preference (not the target's) governs this dial.
            let stream = self.dial_tcp(&first_addr, first_jump.address_family).await
                .map_err(|e| SshError::JumpHost(format!("Jump host {}: {}", first_addr, e)))?;
            client::connect_stream(config, stream, handler)
                .await
                .map_err(|e| SshError::JumpHost(format!("Jump host {}: {}", first_addr, e)))?
        };

        // Authenticate on first jump host
        let first_pw = resolver.passwords.get(&first_jump_id);
        let first_key = resolver.private_keys.get(&first_jump_id);
        self.authenticate_handle(
            &mut current_handle,
            first_jump,
            first_pw.map(String::as_str),
            first_key.map(String::as_str),
        )
        .await?;

        // Chain through remaining jump hosts
        for i in 1..connection.jump_chain.len() {
            let jump_id = connection.jump_chain[i];
            let jump = resolver
                .connections
                .iter()
                .find(|c| c.id == jump_id)
                .ok_or_else(|| SshError::JumpHost(format!("Jump host {} not found", jump_id)))?;

            // Open a direct-tcpip channel through current host to next hop
            let channel = current_handle
                .channel_open_direct_tcpip(
                    jump.hostname.clone(),
                    jump.port as u32,
                    "127.0.0.1",
                    0,
                )
                .await
                .map_err(|e| SshError::JumpHost(format!("direct-tcpip to {}: {}", jump.hostname, e)))?;

            let stream = channel.into_stream();
            let config = self.make_config();
            let handler = self.make_handler(&jump.hostname, jump.port);
            current_handle = client::connect_stream(config, stream, handler)
                .await
                .map_err(|e| SshError::JumpHost(format!("SSH handshake via jump: {}", e)))?;

            let jump_pw = resolver.passwords.get(&jump_id);
            let jump_key = resolver.private_keys.get(&jump_id);
            self.authenticate_handle(
                &mut current_handle,
                jump,
                jump_pw.map(String::as_str),
                jump_key.map(String::as_str),
            )
            .await?;
        }

        // Open direct-tcpip channel to final target through the last jump host
        let (target_host, target_port) = parse_addr(final_addr)?;
        let channel = current_handle
            .channel_open_direct_tcpip(target_host.clone(), target_port, "127.0.0.1", 0)
            .await
            .map_err(|e| SshError::JumpHost(format!("direct-tcpip to target {}: {}", final_addr, e)))?;

        let stream = channel.into_stream();
        let config = self.make_config();
        let handler = self.make_handler(&target_host, target_port as u16);
        client::connect_stream(config, stream, handler)
            .await
            .map_err(|e| SshError::JumpHost(format!("SSH handshake to target: {}", e)))
    }

}
