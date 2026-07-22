//! Platform accept loops for the agent socket. Unix half (Phase 1);
//! the Windows named-pipe half lands in Phase 3.
//!
//! Unix: a `UnixListener` at `~/.oryxis/agent.sock`, parent dir 0700,
//! stale socket unlinked before bind (after a liveness probe so we
//! never clobber a live agent), socket file 0600 after bind. Each
//! accepted connection is its own task over [`serve_connection`].

// Re-exported for Phase 2's AgentRuntime; unused until then.
#[cfg(unix)]
#[allow(unused_imports)]
pub(crate) use unix::{agent_socket_path, serve_unix};

#[cfg(windows)]
#[allow(unused_imports)]
pub(crate) use windows::{
    agent_pipe_name, create_first_instance, openssh_pipe_name, serve_pipe,
};

#[cfg(unix)]
mod unix {
    use std::path::PathBuf;
    use std::sync::Arc;

    use tokio::net::UnixListener;

    use super::super::protocol::{serve_connection, ConfirmMode};
    use super::super::source::AgentKeySource;

    /// `~/.oryxis/agent.sock`, the fixed path the user points
    /// `SSH_AUTH_SOCK` at. Canonical name lives in
    /// `oryxis_core::agent_paths` (shared with the client-side agent
    /// candidate list in oryxis-ssh).
    pub(crate) fn agent_socket_path() -> Option<PathBuf> {
        oryxis_core::agent_paths::unix_agent_socket_path()
    }

    /// Bind the socket and accept forever, serving each connection with
    /// `source` under the `confirm` policy. Returns on bind failure
    /// (surfaced to the toggle) or when the task is aborted (toggle
    /// off / shutdown).
    pub(crate) async fn serve_unix<K>(
        source: Arc<K>,
        confirm: ConfirmMode,
    ) -> std::io::Result<()>
    where
        K: AgentKeySource + 'static,
    {
        use std::os::unix::fs::PermissionsExt;

        let path = agent_socket_path()
            .ok_or_else(|| std::io::Error::other("no home directory"))?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
            // 0700 on the ~/.oryxis dir (best effort; the vault dir
            // already lives here).
            let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
        }

        // A stale socket file from a crash blocks bind. Only unlink it
        // when nothing live answers, so we never steal a running
        // agent's socket.
        if path.exists() {
            if socket_is_live(&path).await {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::AddrInUse,
                    "another agent is already listening on the socket",
                ));
            }
            std::fs::remove_file(&path)?;
        }

        let listener = UnixListener::bind(&path)?;
        // 0600: only the owner can talk to the signing oracle.
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;

        loop {
            let (stream, _addr) = listener.accept().await?;
            let source = source.clone();
            let confirm = confirm.clone();
            // Best-effort requesting process (SO_PEERCRED pid -> name);
            // shown on the confirm card when resolvable.
            let peer = stream.peer_cred().ok().and_then(|c| peer_name(c.pid()));
            tokio::spawn(async move {
                if let Err(e) =
                    serve_connection(stream, source.as_ref(), &confirm, peer.as_deref()).await
                {
                    tracing::debug!(target = "oryxis::agent", error = %e, "connection ended");
                }
            });
        }
    }

    /// Resolve a pid to a process name via `/proc/<pid>/comm` (Linux)
    /// or `ps` (other unix). Best-effort and racy by design; the
    /// confirm card shows it only when it resolves.
    fn peer_name(pid: Option<i32>) -> Option<String> {
        let pid = pid?;
        #[cfg(target_os = "linux")]
        {
            let comm = std::fs::read_to_string(format!("/proc/{pid}/comm")).ok()?;
            let name = comm.trim();
            (!name.is_empty()).then(|| format!("{name} (pid {pid})"))
        }
        #[cfg(not(target_os = "linux"))]
        {
            let out = std::process::Command::new("ps")
                .args(["-p", &pid.to_string(), "-o", "comm="])
                .output()
                .ok()?;
            let name = String::from_utf8_lossy(&out.stdout).trim().to_string();
            (!name.is_empty()).then(|| format!("{name} (pid {pid})"))
        }
    }

    /// Cheap liveness probe: connect and send a REQUEST_IDENTITIES; a
    /// live agent answers, a stale socket file refuses the connection.
    /// Short timeout so a hung peer can't deadlock the bind.
    async fn socket_is_live(path: &std::path::Path) -> bool {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let Ok(connect) =
            tokio::time::timeout(std::time::Duration::from_millis(300), tokio::net::UnixStream::connect(path))
                .await
        else {
            return false;
        };
        let Ok(mut stream) = connect else {
            return false;
        };
        // `uint32 len=1, byte REQUEST_IDENTITIES(11)`.
        if stream.write_all(&[0, 0, 0, 1, 11]).await.is_err() {
            return false;
        }
        let mut len = [0u8; 4];
        matches!(
            tokio::time::timeout(std::time::Duration::from_millis(300), stream.read_exact(&mut len)).await,
            Ok(Ok(_))
        )
    }

    #[cfg(test)]
    mod tests {
        use super::super::super::source::mock::MockKeySource;
        use super::*;
        use std::os::unix::fs::PermissionsExt;

        #[tokio::test]
        async fn binds_0600_and_serves() {
            use russh::keys::agent::client::AgentClient;

            let dir = tempfile::tempdir().unwrap();
            let sock = dir.path().join("agent.sock");

            let source = Arc::new(MockKeySource::new(vec![]));
            let src = source.clone();
            let sock2 = sock.clone();
            let server = tokio::spawn(async move {
                let listener = tokio::net::UnixListener::bind(&sock2).unwrap();
                std::fs::set_permissions(&sock2, std::fs::Permissions::from_mode(0o600)).unwrap();
                let (stream, _) = listener.accept().await.unwrap();
                serve_connection(stream, src.as_ref(), &ConfirmMode::default(), None)
                    .await
                    .unwrap();
            });

            // Give the bind a moment, then check perms + drive it.
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            let mode = std::fs::metadata(&sock).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "socket must be owner-only");

            let stream = tokio::net::UnixStream::connect(&sock).await.unwrap();
            let mut client = AgentClient::connect(stream);
            assert!(client.request_identities().await.unwrap().is_empty());
            drop(client);
            let _ = server.await;
        }

        #[tokio::test]
        async fn stale_socket_probe_reports_dead() {
            let dir = tempfile::tempdir().unwrap();
            let sock = dir.path().join("stale.sock");
            // A plain file that isn't a live socket.
            std::fs::write(&sock, b"stale").unwrap();
            assert!(!socket_is_live(&sock).await);
        }
    }
}

/// Windows named-pipe half (Phase 3). A pipe at `\\.\pipe\oryxis-ssh-agent`
/// with a per-user DACL (only the current user's SID gets access), the
/// creator using `first_pipe_instance(true)` so a squatter can't attach,
/// and `reject_remote_clients` left at its default `true` so the signing
/// oracle is never reachable over SMB.
///
/// The security-critical part (the DACL actually restricting access) is
/// the one thing a Linux cross-check can't prove; see the QA note in
/// `mod.rs`. Acceptance test: a second local user connecting to the pipe
/// must be DENIED.
#[cfg(windows)]
mod windows {
    use std::os::windows::io::AsRawHandle;
    use std::sync::Arc;

    use tokio::net::windows::named_pipe::{NamedPipeServer, ServerOptions};

    use super::super::protocol::{serve_connection, ConfirmMode};
    use super::super::source::AgentKeySource;

    /// The fixed pipe the user points `IdentityAgent` at. Our own name;
    /// the OpenSSH one below is only ever taken via the opt-in alias.
    /// Canonical name lives in `oryxis_core::agent_paths` (shared with
    /// the client-side agent candidate list in oryxis-ssh).
    pub(crate) fn agent_pipe_name() -> Option<String> {
        Some(oryxis_core::agent_paths::WINDOWS_AGENT_PIPE.to_string())
    }

    /// The pipe the Windows OpenSSH agent service owns when it runs,
    /// and the only name tools with a hardcoded agent target (KeePassXC
    /// in "OpenSSH" mode, stock `ssh.exe` with no config) ever dial.
    /// Served ONLY behind the `agent_server_openssh_pipe` opt-in, and
    /// only when the name is free; the pre-bind probe refuses to fight
    /// a running service for it.
    pub(crate) fn openssh_pipe_name() -> String {
        r"\\.\pipe\openssh-ssh-agent".to_string()
    }

    /// Create the anti-squat FIRST pipe instance (`first_pipe_instance(true)`,
    /// which fails rather than attaches if a hostile squatter already owns
    /// the name). Done by the caller, synchronously, BEFORE the toggle is
    /// confirmed: a bind failure then reverts the toggle with a clear error
    /// instead of leaving it on with a dead listener (there is no
    /// probe-then-bind TOCTOU because this IS the bind). Creating the server
    /// needs the same entered-runtime context `tokio::spawn` needs, which
    /// the caller already has.
    pub(crate) fn create_first_instance(name: &str) -> std::io::Result<NamedPipeServer> {
        create_instance(name, true)
    }

    /// Serve the pipe, driving the accept loop off the pre-created `first`
    /// instance. Returns on a create error or when the task is aborted.
    /// Called once for the Oryxis pipe and, behind the opt-in, once more
    /// for the OpenSSH alias.
    pub(crate) async fn serve_pipe<K>(
        name: String,
        first: NamedPipeServer,
        source: Arc<K>,
        confirm: ConfirmMode,
    ) -> std::io::Result<()>
    where
        K: AgentKeySource + 'static,
    {
        let mut server = first;
        loop {
            server.connect().await?;
            let connected = server;
            // Open the NEXT instance before serving, so a second client is
            // never refused while the first is handled.
            server = create_instance(&name, false)?;

            let source = source.clone();
            let confirm = confirm.clone();
            let peer = client_peer(&connected);
            tokio::spawn(async move {
                if let Err(e) =
                    serve_connection(connected, source.as_ref(), &confirm, peer.as_deref()).await
                {
                    tracing::debug!(target = "oryxis::agent", error = %e, "connection ended");
                }
            });
        }
    }

    /// One pipe instance restricted to the current user. `first` fails if
    /// the name already exists (anti-squat); later instances open the
    /// existing pipe object.
    fn create_instance(name: &str, first: bool) -> std::io::Result<NamedPipeServer> {
        use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;

        let sd = UserOnlySd::new()?;
        let mut sa = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: sd.0,
            bInheritHandle: 0,
        };
        let mut opts = ServerOptions::new();
        opts.first_pipe_instance(first);
        // `reject_remote_clients` defaults to true: never expose the
        // signing oracle over SMB. Do not flip it off.
        //
        // SAFETY: `sa` and the descriptor it points at outlive this call;
        // the kernel copies the descriptor into the pipe object, so freeing
        // it when `sd` drops right after is correct.
        let server = unsafe {
            opts.create_with_security_attributes_raw(
                name,
                &mut sa as *mut _ as *mut std::ffi::c_void,
            )
        }?;
        Ok(server)
    }

    /// The connected client's pid, formatted for the confirm card.
    /// Best-effort: `None` when the query fails. Pid only for now (the exe
    /// name needs a second, heavier query; deferred to a follow-up).
    fn client_peer(server: &NamedPipeServer) -> Option<String> {
        use windows_sys::Win32::System::Pipes::GetNamedPipeClientProcessId;
        let mut pid: u32 = 0;
        // SAFETY: `server` owns a valid pipe handle for the duration.
        let ok = unsafe { GetNamedPipeClientProcessId(server.as_raw_handle(), &mut pid) };
        (ok != 0 && pid != 0).then(|| format!("pid {pid}"))
    }

    /// A security descriptor granting the current user full control and
    /// nobody else (a protected DACL). Frees itself on drop; it only needs
    /// to outlive the `CreateNamedPipe` call that copies it.
    struct UserOnlySd(*mut std::ffi::c_void);

    impl UserOnlySd {
        fn new() -> std::io::Result<Self> {
            use windows_sys::Win32::Security::Authorization::ConvertStringSecurityDescriptorToSecurityDescriptorW;

            let sid = current_user_sid_string()?;
            // D:P = DACL, Protected (blocks inherited ACEs); (A;;GA;;;<sid>)
            // = Allow GENERIC_ALL to the user SID. GA (not GR|GW) is
            // required: opening a SECOND concurrent instance needs
            // FILE_CREATE_PIPE_INSTANCE, which GENERIC_ALL includes and a
            // read/write-only grant does not. Do not narrow this.
            let sddl = format!("D:P(A;;GA;;;{sid})");
            let wsddl: Vec<u16> = sddl.encode_utf16().chain(std::iter::once(0)).collect();
            let mut psd: *mut std::ffi::c_void = std::ptr::null_mut();
            // SAFETY: `wsddl` is a valid NUL-terminated wide string; `psd`
            // receives an owned descriptor, freed in `Drop` with LocalFree.
            // `1` is SDDL_REVISION_1.
            let ok = unsafe {
                ConvertStringSecurityDescriptorToSecurityDescriptorW(
                    wsddl.as_ptr(),
                    1,
                    &mut psd,
                    std::ptr::null_mut(),
                )
            };
            if ok == 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(Self(psd))
        }
    }

    impl Drop for UserOnlySd {
        fn drop(&mut self) {
            if !self.0.is_null() {
                // SAFETY: `self.0` came from Convert...W and is freed with
                // LocalFree exactly once.
                unsafe {
                    windows_sys::Win32::Foundation::LocalFree(self.0);
                }
            }
        }
    }

    /// Closes an owned token handle on drop.
    struct HandleGuard(windows_sys::Win32::Foundation::HANDLE);
    impl Drop for HandleGuard {
        fn drop(&mut self) {
            // SAFETY: `self.0` is a valid handle from OpenProcessToken.
            unsafe {
                windows_sys::Win32::Foundation::CloseHandle(self.0);
            }
        }
    }

    /// Read a NUL-terminated wide string into an owned `String`.
    ///
    /// SAFETY: `ptr` must point at a valid NUL-terminated wide string.
    unsafe fn wide_to_string(ptr: *const u16) -> String {
        let mut len = 0isize;
        while unsafe { *ptr.offset(len) } != 0 {
            len += 1;
        }
        let slice = unsafe { std::slice::from_raw_parts(ptr, len as usize) };
        String::from_utf16_lossy(slice)
    }

    /// The current process user's SID as an SDDL string (`S-1-5-21-...`).
    fn current_user_sid_string() -> std::io::Result<String> {
        use windows_sys::Win32::Foundation::{LocalFree, HANDLE};
        use windows_sys::Win32::Security::Authorization::ConvertSidToStringSidW;
        use windows_sys::Win32::Security::{GetTokenInformation, TokenUser, TOKEN_QUERY, TOKEN_USER};
        use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

        // SAFETY: each raw call is checked; the token handle is closed by
        // `HandleGuard` on every path and the SID string via LocalFree.
        unsafe {
            let mut token: HANDLE = std::ptr::null_mut();
            if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
                return Err(std::io::Error::last_os_error());
            }
            let _guard = HandleGuard(token);

            // First call sizes the buffer: it returns FALSE with
            // ERROR_INSUFFICIENT_BUFFER, which is expected, not an error.
            let mut len: u32 = 0;
            GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &mut len);
            if len == 0 {
                return Err(std::io::Error::last_os_error());
            }
            let mut buf = vec![0u8; len as usize];
            if GetTokenInformation(
                token,
                TokenUser,
                buf.as_mut_ptr() as *mut std::ffi::c_void,
                len,
                &mut len,
            ) == 0
            {
                return Err(std::io::Error::last_os_error());
            }
            let token_user = &*(buf.as_ptr() as *const TOKEN_USER);
            let mut str_ptr: *mut u16 = std::ptr::null_mut();
            if ConvertSidToStringSidW(token_user.User.Sid, &mut str_ptr) == 0 {
                return Err(std::io::Error::last_os_error());
            }
            let s = wide_to_string(str_ptr);
            LocalFree(str_ptr as *mut std::ffi::c_void);
            Ok(s)
        }
    }
}
