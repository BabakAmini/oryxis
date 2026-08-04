//! End-to-end integration tests for the plain SSH path (auth, exec,
//! PTY shell, detect_os) against a real OpenSSH server in a throwaway
//! container.
//!
//! Same Docker / `--ignored` rules as `sftp_integration.rs`. Each test
//! spins its own container so they parallelise cleanly.

use std::sync::Arc;
use std::time::Duration;

use oryxis_core::models::connection::{AuthMethod, Connection};
use oryxis_core::models::port_forward_rule::{ForwardKind, PortForwardRule};
use oryxis_ssh::{HostKeyStatus, SshEngine};
use testcontainers::{
    core::{ContainerPort, IntoContainerPort, WaitFor},
    runners::AsyncRunner,
    GenericImage, ImageExt,
};

const TEST_USER: &str = "tester";
const TEST_PASS: &str = "testpass123";

/// Ephemeral ed25519 keypair used only by the pubkey-auth tests below.
/// Public half is fed to the linuxserver/openssh-server container via
/// the `PUBLIC_KEY` env var; the private half is handed to russh as PEM.
/// Generated with `ssh-keygen -t ed25519 -N "" -C oryxis-test`. Has no
/// authority on any real machine, committing it here is fine.
const TEST_PUBKEY: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIHqXz+0CmwH1pGs+5hWVBcqRQmED5a1tJ5Umb1vp0cW8 oryxis-test";
const TEST_PRIVKEY: &str = "-----BEGIN OPENSSH PRIVATE KEY-----
b3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQAAAAAAAAABAAAAMwAAAAtzc2gtZW
QyNTUxOQAAACB6l8/tApsB9aRrPuYVlQXKkUJhA+WtbSeVJm9b6dHFvAAAAJChC7l8oQu5
fAAAAAtzc2gtZWQyNTUxOQAAACB6l8/tApsB9aRrPuYVlQXKkUJhA+WtbSeVJm9b6dHFvA
AAAED+kh0/9HXyIxhyVOboYST/QHB9Uswr4KfyjtmwkwUOHXqXz+0CmwH1pGs+5hWVBcqR
QmED5a1tJ5Umb1vp0cW8AAAAC29yeXhpcy10ZXN0AQI=
-----END OPENSSH PRIVATE KEY-----
";

/// Spin up sshd in `linuxserver/openssh-server`. Mirrors the helper in
/// `sftp_integration.rs`; if `pubkey` is set, the container also accepts
/// the embedded test public key.
async fn start_sshd(
    pubkey: bool,
) -> (
    Connection,
    String,
    testcontainers::ContainerAsync<GenericImage>,
) {
    let mut image = GenericImage::new("linuxserver/openssh-server", "latest")
        .with_exposed_port(ContainerPort::Tcp(2222))
        // The image prints "sshd is listening on port 2222" *before*
        // it's actually bound and ready, so we wait for the very last
        // init line which fires only after sshd is reachable.
        .with_wait_for(WaitFor::message_on_stdout("[ls.io-init] done."))
        .with_env_var("PUID", "1000")
        .with_env_var("PGID", "1000")
        .with_env_var("PASSWORD_ACCESS", "true")
        .with_env_var("USER_NAME", TEST_USER)
        .with_env_var("USER_PASSWORD", TEST_PASS)
        .with_env_var("SUDO_ACCESS", "false");
    if pubkey {
        image = image.with_env_var("PUBLIC_KEY", TEST_PUBKEY);
    }
    let container = image
        .start()
        .await
        .expect("docker daemon must be running");
    let port = container
        .get_host_port_ipv4(2222.tcp())
        .await
        .expect("port mapping");
    let host = container.get_host().await.expect("host").to_string();
    let mut conn = Connection::new("test", host);
    conn.port = port;
    conn.username = Some(TEST_USER.to_string());
    conn.auth_method = AuthMethod::Password;
    (conn, TEST_PASS.to_string(), container)
}

fn engine() -> SshEngine {
    SshEngine::new()
        .with_host_key_check(Arc::new(|_, _, _, _| HostKeyStatus::Known))
        .with_connect_timeout(Duration::from_secs(20))
        .with_auth_timeout(Duration::from_secs(20))
        .with_session_timeout(Duration::from_secs(20))
}

/// Drive the 3-stage connect (transport / auth / exec) so we can call
/// `exec_command` instead of opening a shell. The convenience wrapper
/// `engine.connect` always opens a PTY; for exec we go the long way.
async fn exec_with_password(
    conn: &Connection,
    password: &str,
    command: &str,
) -> oryxis_ssh::ExecResult {
    let engine = engine();
    let mut handle = engine
        .establish_transport(conn, None)
        .await
        .expect("transport");
    engine
        .do_authenticate(&mut handle, conn, Some(password), None)
        .await
        .expect("authenticate");
    engine
        .exec_command(handle, command, Duration::from_secs(20))
        .await
        .expect("exec_command")
}

#[tokio::test]
#[ignore = "requires Docker, run with --ignored"]
async fn password_auth_runs_exec_command() {
    let (conn, password, _container) = start_sshd(false).await;
    let result = exec_with_password(&conn, &password, "echo hello-from-oryxis").await;
    assert_eq!(result.exit_code, 0);
    assert_eq!(result.stdout.trim(), "hello-from-oryxis");
    assert!(result.stderr.is_empty());
}

#[tokio::test]
#[ignore = "requires Docker, run with --ignored"]
async fn pubkey_auth_runs_exec_command() {
    let (mut conn, _password, _container) = start_sshd(true).await;
    conn.auth_method = AuthMethod::Key;
    let engine = engine();
    let mut handle = engine
        .establish_transport(&conn, None)
        .await
        .expect("transport");
    engine
        .do_authenticate(&mut handle, &conn, None, Some(oryxis_ssh::KeyMaterial::plain(TEST_PRIVKEY)))
        .await
        .expect("authenticate via pubkey");
    let result = engine
        .exec_command(handle, "id -un", Duration::from_secs(20))
        .await
        .expect("exec_command");
    assert_eq!(result.exit_code, 0);
    assert_eq!(result.stdout.trim(), TEST_USER);
}

/// Issue #125 against genuine sshd: `AuthenticationMethods
/// publickey,password` makes sshd answer the ACCEPTED publickey with
/// RFC 4252 partial success and demand the password on top. The
/// pre-fix engine reported that as "Public key rejected by server"; the
/// continuation must chain the stored password and open a session. The
/// russh in-process server can't emit partial success on this path
/// (see `src/partial_auth_tests.rs`), hence real sshd here.
#[tokio::test]
#[ignore = "requires Docker, run with --ignored"]
async fn partial_success_publickey_then_password_against_real_sshd() {
    use testcontainers::CopyTargetOptions;

    // linuxserver custom-cont-init script: runs after the image's own
    // init has generated sshd_config, before sshd starts.
    let script = "#!/bin/bash\n\
        echo 'AuthenticationMethods publickey,password' >> /config/sshd/sshd_config\n";
    let container = GenericImage::new("linuxserver/openssh-server", "latest")
        .with_exposed_port(ContainerPort::Tcp(2222))
        .with_wait_for(WaitFor::message_on_stdout("[ls.io-init] done."))
        .with_env_var("PUID", "1000")
        .with_env_var("PGID", "1000")
        .with_env_var("PASSWORD_ACCESS", "true")
        .with_env_var("USER_NAME", TEST_USER)
        .with_env_var("USER_PASSWORD", TEST_PASS)
        .with_env_var("SUDO_ACCESS", "false")
        .with_env_var("PUBLIC_KEY", TEST_PUBKEY)
        .with_copy_to(
            CopyTargetOptions::new("/custom-cont-init.d/20-auth-methods.sh").with_mode(0o755),
            script.as_bytes().to_vec(),
        )
        .start()
        .await
        .expect("docker daemon must be running");
    let port = container
        .get_host_port_ipv4(2222.tcp())
        .await
        .expect("port mapping");
    let host = container.get_host().await.expect("host").to_string();
    let mut conn = Connection::new("test", host);
    conn.port = port;
    conn.username = Some(TEST_USER.to_string());
    conn.auth_method = AuthMethod::Key;

    let engine = engine();
    let mut handle = engine
        .establish_transport(&conn, None)
        .await
        .expect("transport");
    engine
        .do_authenticate(
            &mut handle,
            &conn,
            Some(TEST_PASS),
            Some(oryxis_ssh::KeyMaterial::plain(TEST_PRIVKEY)),
        )
        .await
        .expect("publickey partial success + password continuation must authenticate");
    let result = engine
        .exec_command(handle, "id -un", Duration::from_secs(20))
        .await
        .expect("exec_command");
    assert_eq!(result.exit_code, 0);
    assert_eq!(result.stdout.trim(), TEST_USER);
}

#[tokio::test]
#[ignore = "requires Docker, run with --ignored"]
async fn exec_command_propagates_nonzero_exit() {
    let (conn, password, _container) = start_sshd(false).await;
    let result = exec_with_password(&conn, &password, "exit 42").await;
    assert_eq!(result.exit_code, 42);
}

#[tokio::test]
#[ignore = "requires Docker, run with --ignored"]
async fn exec_command_separates_stdout_and_stderr() {
    let (conn, password, _container) = start_sshd(false).await;
    let result = exec_with_password(
        &conn,
        &password,
        "echo on-stdout; echo on-stderr 1>&2",
    )
    .await;
    assert_eq!(result.exit_code, 0);
    assert!(result.stdout.contains("on-stdout"));
    assert!(result.stderr.contains("on-stderr"));
    // Cross-check: stdout should NOT carry the stderr line and vice
    // versa, confirms the ExtendedData (ext=1) split worked.
    assert!(!result.stdout.contains("on-stderr"));
    assert!(!result.stderr.contains("on-stdout"));
}

#[tokio::test]
#[ignore = "requires Docker, run with --ignored"]
async fn wrong_password_yields_error() {
    let (conn, _password, _container) = start_sshd(false).await;
    let engine = engine();
    let mut handle = engine
        .establish_transport(&conn, None)
        .await
        .expect("transport");
    let err = engine
        .do_authenticate(&mut handle, &conn, Some("definitely-not-the-password"), None)
        .await
        .expect_err("auth must fail");
    // The exact message is provider-dependent; we only assert that the
    // call surfaced an error rather than silently succeeding.
    let _ = err;
}

#[tokio::test]
#[ignore = "requires Docker, run with --ignored"]
async fn pty_session_round_trips_input_to_output() {
    // Sanity check on the interactive shell path: open a PTY, write a
    // command terminated by newline, and confirm the prompt echoes
    // both the command we typed and its output back to us.
    let (conn, password, _container) = start_sshd(false).await;
    let engine = engine();
    let (session, mut rx) = engine
        .connect(&conn, Some(&password), None, 80, 24)
        .await
        .expect("connect");
    session.write(b"echo pty-marker-xyz\n").expect("write");

    // Drain output until we see our marker or hit a generous timeout.
    // The PTY echoes both the typed command and its output, so we check
    // for the literal output token (without "echo " prefix) appearing
    // on its own line.
    let mut buf = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    let saw_marker = loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            break false;
        }
        match tokio::time::timeout(remaining, rx.recv()).await {
            Ok(Some(chunk)) => {
                buf.extend_from_slice(&chunk);
                let text = String::from_utf8_lossy(&buf);
                // Look for the marker on a line that isn't the echoed
                // command itself (which always carries `echo `).
                if text
                    .lines()
                    .any(|l| l.contains("pty-marker-xyz") && !l.contains("echo "))
                {
                    break true;
                }
            }
            Ok(None) => break false,
            Err(_) => break false,
        }
    };
    assert!(
        saw_marker,
        "expected pty output to include the marker, got: {:?}",
        String::from_utf8_lossy(&buf)
    );
}

#[tokio::test]
#[ignore = "requires Docker, run with --ignored"]
async fn custom_terminal_type_probe_never_blocks_connect() {
    // Issue #88 regression guard for the pre-PTY terminfo probe: with a
    // custom TERM configured the connect gains an exec round trip before
    // the PTY request, and it must never hang or fail the connect, on
    // any host shape (this Alpine-based image has no `infocmp`, so the
    // probe here lands on the inconclusive path; hosts with a full
    // ncurses exercise present/fallback instead). The probe outcomes
    // themselves are unit-tested in `engine::terminfo`; what only a
    // live sshd can prove is the channel choreography. The shell's TERM
    // must agree with whatever the probe decided.
    let (conn, password, _container) = start_sshd(false).await;
    let engine = engine().with_terminal_type(Some("tmux-256color".to_string()));
    let (session, mut rx) = engine
        .connect(&conn, Some(&password), None, 80, 24)
        .await
        .expect("connect with custom TERM");
    let expected_term = match session.term_fallback() {
        Some(fb) => fb.used.as_deref().unwrap_or("tmux-256color").to_string(),
        None => "tmux-256color".to_string(),
    };
    session.write(b"echo term-is-$TERM\n").expect("write");

    let marker = format!("term-is-{expected_term}");
    let mut buf = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    let saw_marker = loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            break false;
        }
        match tokio::time::timeout(remaining, rx.recv()).await {
            Ok(Some(chunk)) => {
                buf.extend_from_slice(&chunk);
                let text = String::from_utf8_lossy(&buf);
                if text
                    .lines()
                    .any(|l| l.contains(&marker) && !l.contains("echo "))
                {
                    break true;
                }
            }
            Ok(None) => break false,
            Err(_) => break false,
        }
    };
    assert!(
        saw_marker,
        "expected TERM {expected_term:?} in pty output, got: {:?}",
        String::from_utf8_lossy(&buf)
    );
}

#[tokio::test]
#[ignore = "requires Docker, run with --ignored"]
async fn pty_session_resize_is_not_fatal() {
    // resize() is fire-and-forget, we just want to confirm it doesn't
    // panic and the session stays alive afterwards.
    let (conn, password, _container) = start_sshd(false).await;
    let engine = engine();
    let (session, _rx) = engine
        .connect(&conn, Some(&password), None, 80, 24)
        .await
        .expect("connect");
    session.resize(120, 40);
    session.resize(200, 60);
    // Tiny grace period so the window-change request can hit the wire
    // before we tear down, an immediate drop sometimes truncates the
    // last channel message and produces a misleading "channel closed"
    // log line.
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(session.is_alive());
}

#[tokio::test]
#[ignore = "requires Docker, run with --ignored"]
async fn agent_forwarding_sets_remote_ssh_auth_sock() {
    // When the engine is configured with `with_agent_forwarding(true)`,
    // sshd inside the container should create a unix socket and export
    // its path as `SSH_AUTH_SOCK` to the user's shell. We don't need a
    // real local agent for this assertion, the env var is set on the
    // remote side as soon as the channel-level request is accepted.
    let (conn, password, _container) = start_sshd(false).await;
    let engine = engine().with_agent_forwarding(true);
    let (session, mut rx) = engine
        .connect(&conn, Some(&password), None, 80, 24)
        .await
        .expect("connect");
    // Sleep briefly so the shell prompt is fully drawn before we type
    //, otherwise the marker can interleave with motd / prompt output.
    tokio::time::sleep(Duration::from_millis(500)).await;
    session
        .write(b"echo SOCK=[$SSH_AUTH_SOCK]\n")
        .expect("write");

    let mut buf = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    let saw_socket = loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            break false;
        }
        match tokio::time::timeout(remaining, rx.recv()).await {
            Ok(Some(chunk)) => {
                buf.extend_from_slice(&chunk);
                let text = String::from_utf8_lossy(&buf);
                // Look for `SOCK=[/...` on a line that isn't the
                // echoed command (the typed line carries the literal
                // `$SSH_AUTH_SOCK`, not its expansion).
                if text.lines().any(|l| {
                    l.contains("SOCK=[/") && !l.contains("$SSH_AUTH_SOCK")
                }) {
                    break true;
                }
            }
            Ok(None) | Err(_) => break false,
        }
    };
    assert!(
        saw_socket,
        "expected SSH_AUTH_SOCK to be set on the remote shell when forwarding is on, got: {:?}",
        String::from_utf8_lossy(&buf)
    );
}

#[tokio::test]
#[ignore = "requires Docker, run with --ignored"]
async fn agent_forwarding_off_leaves_remote_socket_unset() {
    // Mirror of the previous test, without forwarding, the remote
    // shell shouldn't have `SSH_AUTH_SOCK` set (the whole point of
    // OpenSSH's default `ForwardAgent no` is that opting in is explicit).
    let (conn, password, _container) = start_sshd(false).await;
    let engine = engine(); // forwarding off (default)
    let (session, mut rx) = engine
        .connect(&conn, Some(&password), None, 80, 24)
        .await
        .expect("connect");
    tokio::time::sleep(Duration::from_millis(500)).await;
    session
        .write(b"echo SOCK=[$SSH_AUTH_SOCK]\n")
        .expect("write");

    let mut buf = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    let saw_empty = loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            break false;
        }
        match tokio::time::timeout(remaining, rx.recv()).await {
            Ok(Some(chunk)) => {
                buf.extend_from_slice(&chunk);
                let text = String::from_utf8_lossy(&buf);
                if text.lines().any(|l| {
                    l.contains("SOCK=[]") && !l.contains("$SSH_AUTH_SOCK")
                }) {
                    break true;
                }
            }
            Ok(None) | Err(_) => break false,
        }
    };
    assert!(
        saw_empty,
        "expected SSH_AUTH_SOCK to be unset without forwarding, got: {:?}",
        String::from_utf8_lossy(&buf)
    );
}

#[tokio::test]
#[ignore = "requires Docker, run with --ignored"]
async fn local_forward_tunnels_and_frees_listener_on_cancel() {
    use tokio::io::AsyncReadExt;

    // Reserve a free local port by binding+dropping a probe listener, so
    // the forward binds a port we can predict and re-check after teardown.
    let listen_port = {
        let probe = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("probe bind");
        probe.local_addr().expect("probe addr").port()
    };

    let (conn, password, container) = start_sshd(false).await;

    // The linuxserver image ships `AllowTcpForwarding no`; flip it on and
    // reload sshd so `direct-tcpip` channels are permitted, otherwise the
    // forward would fail for an environmental reason, not a code one.
    container
        .exec(testcontainers::core::ExecCommand::new([
            "sh",
            "-c",
            "sed -i 's/AllowTcpForwarding no/AllowTcpForwarding yes/' \
             /config/sshd/sshd_config /etc/ssh/sshd_config 2>/dev/null; \
             pkill -HUP sshd 2>/dev/null; true",
        ]))
        .await
        .expect("enable tcp forwarding");
    // Give sshd a moment to re-read its config after the HUP.
    tokio::time::sleep(Duration::from_secs(2)).await;

    // `-L` rule targeting the container's own sshd (reachable from the
    // server as 127.0.0.1:2222). Tunnelling there means a TCP connect to
    // our local listener should read back the remote sshd's SSH banner.
    let mut rule = PortForwardRule::new("tunnel", ForwardKind::Local, uuid::Uuid::new_v4());
    rule.listen_host = "127.0.0.1".into();
    rule.listen_port = listen_port;
    rule.target_host = "127.0.0.1".into();
    rule.target_port = 2222;

    let engine = engine();
    let session = engine
        .connect_forward(&conn, Some(&password), None, &rule, None)
        .await
        .expect("forward up");
    assert!(session.is_alive());

    // Connect through the local forwarded port and read the tunneled
    // sshd banner, end-to-end proof the tunnel carries bytes.
    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", listen_port))
        .await
        .expect("connect to forwarded port");
    let mut buf = [0u8; 64];
    let n = tokio::time::timeout(Duration::from_secs(15), stream.read(&mut buf))
        .await
        .expect("read banner within timeout")
        .expect("read banner");
    let banner = String::from_utf8_lossy(&buf[..n]);
    assert!(
        banner.starts_with("SSH-2.0"),
        "expected tunneled SSH banner, got: {banner:?}"
    );
    drop(stream);

    // Cancel and confirm the listener is actually released (the core
    // spine guarantee: toggle-off is real cancellation, not a detached
    // task that keeps the port bound).
    session.cancel().await;
    drop(session);
    let freed = {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        loop {
            if tokio::net::TcpListener::bind(("127.0.0.1", listen_port))
                .await
                .is_ok()
            {
                break true;
            }
            if tokio::time::Instant::now() >= deadline {
                break false;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    };
    assert!(freed, "listener port {listen_port} was not freed after cancel");
}

#[tokio::test]
#[ignore = "requires Docker, run with --ignored"]
async fn shared_connection_carries_multiple_forwards() {
    use testcontainers::core::ExecCommand;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    const MARKER: &str = "HELLO-VIA-SHARED-CONN";

    // Two predictable local ports for the `-L` rules.
    let (port_a, port_b) = {
        let a = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("probe bind");
        let b = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("probe bind");
        (
            a.local_addr().expect("probe addr").port(),
            b.local_addr().expect("probe addr").port(),
        )
    };
    // Local target + server-side port for the `-R` rule.
    let target = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("target bind");
    let target_port = target.local_addr().expect("target addr").port();
    tokio::spawn(async move {
        while let Ok((mut sock, _)) = target.accept().await {
            let _ = sock.write_all(MARKER.as_bytes()).await;
            let _ = sock.flush().await;
            let _ = sock.shutdown().await;
        }
    });
    let server_port = {
        let probe = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("probe bind");
        probe.local_addr().expect("probe addr").port()
    };

    let (conn, password, container) = start_sshd(false).await;
    container
        .exec(ExecCommand::new([
            "sh",
            "-c",
            "sed -i 's/AllowTcpForwarding no/AllowTcpForwarding yes/' \
             /config/sshd/sshd_config /etc/ssh/sshd_config 2>/dev/null; \
             pkill -HUP sshd 2>/dev/null; true",
        ]))
        .await
        .expect("enable tcp forwarding");
    tokio::time::sleep(Duration::from_secs(2)).await;

    // ONE dial for the host (issue #126); every rule attaches onto it.
    let engine = engine();
    let fconn = engine
        .connect_forward_conn(&conn, Some(&password), None, None)
        .await
        .expect("shared forward connection up");
    assert!(fconn.is_alive());

    let mut rule_a = PortForwardRule::new("a", ForwardKind::Local, uuid::Uuid::new_v4());
    rule_a.listen_host = "127.0.0.1".into();
    rule_a.listen_port = port_a;
    rule_a.target_host = "127.0.0.1".into();
    rule_a.target_port = 2222;
    let mut rule_b = rule_a.clone();
    rule_b.id = uuid::Uuid::new_v4();
    rule_b.listen_port = port_b;
    let mut rule_r = PortForwardRule::new("r", ForwardKind::Remote, uuid::Uuid::new_v4());
    rule_r.listen_host = "127.0.0.1".into();
    rule_r.listen_port = server_port;
    rule_r.target_host = "127.0.0.1".into();
    rule_r.target_port = target_port;

    let session_a = fconn.attach(&rule_a).await.expect("-L a up");
    let session_b = fconn.attach(&rule_b).await.expect("-L b up");
    let session_r = fconn.attach(&rule_r).await.expect("-R up");

    // Both `-L` listeners tunnel over the one connection.
    for port in [port_a, port_b] {
        let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .expect("connect to forwarded port");
        let mut buf = [0u8; 64];
        let n = tokio::time::timeout(Duration::from_secs(15), stream.read(&mut buf))
            .await
            .expect("read banner within timeout")
            .expect("read banner");
        let banner = String::from_utf8_lossy(&buf[..n]);
        assert!(
            banner.starts_with("SSH-2.0"),
            "expected tunneled SSH banner on {port}, got: {banner:?}"
        );
    }

    // The `-R` bind routes inbound channels to ITS drain (the demux by
    // (address, port)), not to a sibling's.
    let mut exec = container
        .exec(ExecCommand::new([
            "sh",
            "-c",
            &format!("nc -w 5 127.0.0.1 {server_port}"),
        ]))
        .await
        .expect("exec nc");
    let out = exec.stdout_to_vec().await.expect("nc stdout");
    let got = String::from_utf8_lossy(&out);
    assert!(
        got.contains(MARKER),
        "expected the local target marker via the shared -R, got: {got:?}"
    );

    // Cancelling one rule tears down only ITS listener; siblings keep
    // tunneling on the same connection.
    session_a.cancel().await;
    drop(session_a);
    let freed = {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        loop {
            if tokio::net::TcpListener::bind(("127.0.0.1", port_a))
                .await
                .is_ok()
            {
                break true;
            }
            if tokio::time::Instant::now() >= deadline {
                break false;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    };
    assert!(freed, "port {port_a} was not freed after cancelling rule a");

    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port_b))
        .await
        .expect("connect to surviving forward");
    let mut buf = [0u8; 64];
    let n = tokio::time::timeout(Duration::from_secs(15), stream.read(&mut buf))
        .await
        .expect("read banner within timeout")
        .expect("read banner");
    assert!(
        String::from_utf8_lossy(&buf[..n]).starts_with("SSH-2.0"),
        "sibling forward must survive a rule's cancel"
    );
    assert!(session_b.is_alive());

    session_r.cancel().await;
    session_b.cancel().await;
}

#[tokio::test]
#[ignore = "requires Docker, run with --ignored"]
async fn remote_forward_delivers_inbound_to_local_target() {
    use testcontainers::core::ExecCommand;
    use tokio::io::AsyncWriteExt;

    const MARKER: &str = "HELLO-FROM-LOCAL-TARGET";

    // Local target: a listener in THIS process. A `-R` forward should tunnel
    // a connection made *inside the server* back here, so when the container
    // connects to the server-side port, our listener writes the marker and
    // the container reads it back.
    let target = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("target bind");
    let target_port = target.local_addr().expect("target addr").port();
    tokio::spawn(async move {
        while let Ok((mut sock, _)) = target.accept().await {
            let _ = sock.write_all(MARKER.as_bytes()).await;
            let _ = sock.flush().await;
            let _ = sock.shutdown().await;
        }
    });

    // Pick a free server-side port (predict it on the container's loopback).
    let server_port = {
        let probe = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("probe bind");
        probe.local_addr().expect("probe addr").port()
    };

    let (conn, password, container) = start_sshd(false).await;
    container
        .exec(ExecCommand::new([
            "sh",
            "-c",
            "sed -i 's/AllowTcpForwarding no/AllowTcpForwarding yes/' \
             /config/sshd/sshd_config /etc/ssh/sshd_config 2>/dev/null; \
             pkill -HUP sshd 2>/dev/null; true",
        ]))
        .await
        .expect("enable tcp forwarding");
    tokio::time::sleep(Duration::from_secs(2)).await;

    // `-R`: server binds 127.0.0.1:server_port and tunnels back to our
    // local target.
    let mut rule = PortForwardRule::new("reverse", ForwardKind::Remote, uuid::Uuid::new_v4());
    rule.listen_host = "127.0.0.1".into();
    rule.listen_port = server_port;
    rule.target_host = "127.0.0.1".into();
    rule.target_port = target_port;

    let engine = engine();
    let session = engine
        .connect_forward(&conn, Some(&password), None, &rule, None)
        .await
        .expect("remote forward up");
    assert!(session.is_alive());

    // From inside the container, connect to the server-side listener and
    // read what comes back, which should be our local target's marker.
    let mut exec = container
        .exec(ExecCommand::new([
            "sh",
            "-c",
            &format!("nc -w 5 127.0.0.1 {server_port}"),
        ]))
        .await
        .expect("exec nc");
    let out = exec.stdout_to_vec().await.expect("nc stdout");
    let got = String::from_utf8_lossy(&out);
    assert!(
        got.contains(MARKER),
        "expected the local target marker tunneled back through -R, got: {got:?}"
    );

    // Clean teardown also releases the server-side listener.
    session.cancel().await;
}

#[tokio::test]
#[ignore = "requires Docker, run with --ignored"]
async fn dynamic_socks_forward_connects_through_proxy() {
    use testcontainers::core::ExecCommand;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let listen_port = {
        let probe = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("probe bind");
        probe.local_addr().expect("probe addr").port()
    };

    let (conn, password, container) = start_sshd(false).await;
    container
        .exec(ExecCommand::new([
            "sh",
            "-c",
            "sed -i 's/AllowTcpForwarding no/AllowTcpForwarding yes/' \
             /config/sshd/sshd_config /etc/ssh/sshd_config 2>/dev/null; \
             pkill -HUP sshd 2>/dev/null; true",
        ]))
        .await
        .expect("enable tcp forwarding");
    tokio::time::sleep(Duration::from_secs(2)).await;

    // `-D`: a local SOCKS5 listener. The destination is chosen per SOCKS
    // request, so the rule has no fixed target.
    let mut rule = PortForwardRule::new("socks", ForwardKind::Dynamic, uuid::Uuid::new_v4());
    rule.listen_host = "127.0.0.1".into();
    rule.listen_port = listen_port;

    let engine = engine();
    let session = engine
        .connect_forward(&conn, Some(&password), None, &rule, None)
        .await
        .expect("dynamic forward up");
    assert!(session.is_alive());

    // Minimal SOCKS5 client: ask the proxy to CONNECT to the container's
    // own sshd (127.0.0.1:2222, reachable from the server), then read the
    // SSH banner that comes back through the tunnel.
    let mut s = tokio::net::TcpStream::connect(("127.0.0.1", listen_port))
        .await
        .expect("connect to socks listener");
    // Greeting: VER=5, 1 method, no-auth(0x00).
    s.write_all(&[0x05, 0x01, 0x00]).await.expect("greeting");
    let mut method = [0u8; 2];
    s.read_exact(&mut method).await.expect("method reply");
    assert_eq!(method, [0x05, 0x00], "expected no-auth method selected");
    // CONNECT request: VER CMD RSV ATYP=IPv4 127.0.0.1 :2222.
    s.write_all(&[0x05, 0x01, 0x00, 0x01, 127, 0, 0, 1, 0x08, 0xAE])
        .await
        .expect("connect request");
    let mut reply = [0u8; 10]; // VER REP RSV ATYP=1 + 4 addr + 2 port
    s.read_exact(&mut reply).await.expect("connect reply");
    assert_eq!(reply[1], 0x00, "expected SOCKS5 success reply, got {reply:?}");

    let mut buf = [0u8; 64];
    let n = tokio::time::timeout(Duration::from_secs(15), s.read(&mut buf))
        .await
        .expect("banner within timeout")
        .expect("read banner");
    let banner = String::from_utf8_lossy(&buf[..n]);
    assert!(
        banner.starts_with("SSH-2.0"),
        "expected tunneled SSH banner via SOCKS5, got: {banner:?}"
    );

    session.cancel().await;
}

#[tokio::test]
#[ignore = "requires Docker, run with --ignored"]
async fn detect_os_returns_a_value() {
    // The image is Alpine-based; we don't pin the exact string because
    // it depends on which uname/lsb-release path detect_os hits inside
    // the container. We only assert the call resolves to *something*
    // non-empty within the timeout.
    let (conn, password, _container) = start_sshd(false).await;
    let engine = engine();
    let (session, _rx) = engine
        .connect(&conn, Some(&password), None, 80, 24)
        .await
        .expect("connect");
    let os = session.detect_os().await;
    assert!(os.is_some(), "expected detect_os to return Some(_)");
    let os = os.unwrap();
    assert!(!os.is_empty(), "detect_os returned an empty string");
}

/// `exec_capture` is what the Monitor tab's kill action (issue #96)
/// runs on, and it is the only side-channel call that needs the exit
/// status, the stderr AND a stdin payload. All three are checked
/// against a real sshd, since a half-open channel or a swallowed exit
/// status only shows up over the wire.
#[tokio::test]
#[ignore = "requires Docker, run with --ignored"]
async fn exec_capture_reports_status_streams_and_feeds_stdin() {
    let (conn, password, _container) = start_sshd(false).await;
    let engine = engine();
    let (session, _rx) = engine
        .connect(&conn, Some(&password), None, 80, 24)
        .await
        .expect("connect");
    let timeout = Duration::from_secs(20);

    // Success: stdout only, exit 0.
    let ok = session
        .exec_capture("echo out", None, timeout)
        .await
        .expect("exec_capture");
    assert_eq!(ok.exit_code, 0);
    assert_eq!(ok.stdout.trim(), "out");
    assert!(ok.stderr.is_empty());

    // Failure: the streams stay separate and the real status survives
    // (the `probe` path would have thrown both away).
    let failed = session
        .exec_capture("sh -c 'echo boom >&2; exit 7'", None, timeout)
        .await
        .expect("exec_capture");
    assert_eq!(failed.exit_code, 7);
    assert_eq!(failed.stderr.trim(), "boom");
    assert!(failed.stdout.is_empty());

    // The `sudo -S` mechanism: bytes must reach the command's stdin and
    // the EOF must arrive, or a reader like sudo blocks until timeout.
    let piped = session
        .exec_capture("cat", Some(b"secret-on-stdin\n".to_vec()), timeout)
        .await
        .expect("exec_capture");
    assert_eq!(piped.exit_code, 0);
    assert_eq!(piped.stdout.trim(), "secret-on-stdin");

    // A command that never reads stdin must still finish, rather than
    // wedging on the unread payload.
    let ignored = session
        .exec_capture("echo ignored", Some(b"unread\n".to_vec()), timeout)
        .await
        .expect("exec_capture");
    assert_eq!(ignored.exit_code, 0);
    assert_eq!(ignored.stdout.trim(), "ignored");

    // A command that outlives the cap reports nothing rather than
    // hanging the caller.
    assert!(
        session
            .exec_capture("sleep 5", None, Duration::from_millis(300))
            .await
            .is_none()
    );
}

/// The kill pipeline's first step re-reads the host's listening
/// sockets with the very command the monitor probe uses. This pins
/// that the command runs on a real (BusyBox) host and names a socket
/// the login user owns, which is what makes an unescalated kill
/// possible at all.
#[tokio::test]
#[ignore = "requires Docker, run with --ignored"]
async fn the_socket_probe_names_a_process_the_login_user_owns() {
    let (conn, password, _container) = start_sshd(false).await;
    let engine = engine();
    let (session, _rx) = engine
        .connect(&conn, Some(&password), None, 80, 24)
        .await
        .expect("connect");

    // A listener owned by the login user, so `-p` is allowed to name
    // it. Backgrounded with its own redirects so the exec channel
    // doesn't wait on it.
    let spawn = session
        .exec_capture(
            "sh -c 'nc -l -p 45678 >/dev/null 2>&1 & echo $!'",
            None,
            Duration::from_secs(20),
        )
        .await
        .expect("exec_capture");
    assert_eq!(spawn.exit_code, 0, "stderr: {}", spawn.stderr);
    let pid: u32 = spawn.stdout.trim().parse().expect("a pid on stdout");

    let sockets = session
        .exec_capture(
            "sh -c 'ss -tulnp 2>/dev/null || netstat -tulnp 2>/dev/null'",
            None,
            Duration::from_secs(20),
        )
        .await
        .expect("exec_capture");
    assert!(
        sockets.stdout.contains("45678"),
        "the probe should see our listener; got: {}",
        sockets.stdout
    );

    // And the signal itself, in the exact shape `kill_command` builds.
    let killed = session
        .exec_capture(
            &format!("sh -c 'kill -s TERM {pid}'"),
            None,
            Duration::from_secs(20),
        )
        .await
        .expect("exec_capture");
    assert_eq!(killed.exit_code, 0, "stderr: {}", killed.stderr);
}
