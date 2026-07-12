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

#[cfg(unix)]
mod unix {
    use std::path::PathBuf;
    use std::sync::Arc;

    use tokio::net::UnixListener;

    use super::super::protocol::serve_connection;
    use super::super::source::AgentKeySource;

    /// `~/.oryxis/agent.sock`, the fixed path the user points
    /// `SSH_AUTH_SOCK` at.
    pub(crate) fn agent_socket_path() -> Option<PathBuf> {
        Some(dirs::home_dir()?.join(".oryxis").join("agent.sock"))
    }

    /// Bind the socket and accept forever, serving each connection with
    /// `source`. Returns on bind failure (surfaced to the toggle) or
    /// when the task is aborted (toggle off / shutdown).
    pub(crate) async fn serve_unix<K>(source: Arc<K>) -> std::io::Result<()>
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
            tokio::spawn(async move {
                if let Err(e) = serve_connection(stream, source.as_ref()).await {
                    tracing::debug!(target = "oryxis::agent", error = %e, "connection ended");
                }
            });
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
                serve_connection(stream, src.as_ref()).await.unwrap();
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
