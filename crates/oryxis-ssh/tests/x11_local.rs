//! Live X11-forwarding test against a LOCAL sshd and a LOCAL X server.
//!
//! Unlike the container-based tests in `ssh_integration.rs`, this one
//! cannot be hermetic: forwarding X11 needs a real X display on the
//! machine running the test, which no container image can provide.
//! It is therefore `#[ignore]`d and driven by env vars.
//!
//! What it proves that no unit test can: that the `x11-req` is accepted,
//! that sshd exports a working `DISPLAY`, that the X11 channel comes
//! back, that the cookie swap produces a setup request the local X
//! server ACCEPTS, and that bytes flow both ways.
//!
//! Run it with a throwaway sshd (no root, no changes to the system
//! config), e.g.:
//!
//! ```bash
//! cd "$(mktemp -d)"
//! ssh-keygen -q -t ed25519 -f hostkey -N ""
//! ssh-keygen -q -t ed25519 -f clientkey -N ""
//! cat clientkey.pub > authorized_keys && chmod 600 authorized_keys
//! /usr/sbin/sshd -f /dev/null -h "$PWD/hostkey" -p 2222 \
//!     -o "X11Forwarding yes" -o "UsePAM no" -o "StrictModes no" \
//!     -o "AuthorizedKeysFile $PWD/authorized_keys" -D -e &
//!
//! ORYXIS_X11_PORT=2222 \
//! ORYXIS_X11_KEY="$PWD/clientkey" \
//! ORYXIS_X11_PROBE=/path/to/x11probe.py \
//!   cargo test -p oryxis-ssh --test x11_local -- --ignored --nocapture
//! ```

use std::sync::Arc;
use std::time::Duration;

use oryxis_core::models::connection::{AuthMethod, Connection};
use oryxis_ssh::{HostKeyStatus, KeyMaterial, SshEngine};

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

#[tokio::test]
#[ignore = "needs a local sshd with X11Forwarding plus a real X display"]
async fn x11_forwarding_reaches_the_local_display() {
    let (Some(port), Some(key_path), Some(probe)) = (
        env("ORYXIS_X11_PORT"),
        env("ORYXIS_X11_KEY"),
        env("ORYXIS_X11_PROBE"),
    ) else {
        panic!("set ORYXIS_X11_PORT / ORYXIS_X11_KEY / ORYXIS_X11_PROBE (see the module docs)");
    };
    let private_pem = std::fs::read_to_string(&key_path).expect("read the client key");
    // The engine's own tracing is the only window into which X11 branch
    // was taken (resolved display, auth mode, per-channel rejects).
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "oryxis_ssh=debug".into()),
        )
        .try_init();

    let mut conn = Connection::new("x11-local", "127.0.0.1");
    conn.port = port.parse().expect("ORYXIS_X11_PORT must be a number");
    conn.username = Some(whoami());
    conn.auth_method = AuthMethod::Key;

    let engine = SshEngine::new()
        .with_host_key_check(Arc::new(|_, _, _, _| HostKeyStatus::Known))
        .with_connect_timeout(Duration::from_secs(20))
        .with_auth_timeout(Duration::from_secs(20))
        .with_session_timeout(Duration::from_secs(20))
        .with_x11_forwarding(true);

    let (session, mut output) = engine
        .connect(&conn, None, Some(KeyMaterial::plain(&private_pem)), 80, 24)
        .await
        .expect("connect with X11 forwarding");

    // A marker brackets the probe output so we never match text that the
    // login shell happened to print.
    //
    // The marker is SPLIT in the command text (`"X11_PROBE""_DONE_"`) so
    // the PTY's echo of the command line does not itself contain the
    // marker: matching your own echo makes the test pass before the
    // probe has even run.
    session
        .write(format!("python3 {probe}; echo \"X11_PROBE\"\"_DONE_$?\"\n").as_bytes())
        .expect("write the probe command");

    let mut seen = String::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    let why = loop {
        match tokio::time::timeout_at(deadline, output.recv()).await {
            Ok(Some(chunk)) => {
                seen.push_str(&String::from_utf8_lossy(&chunk));
                if seen.contains("X11_PROBE_DONE_") {
                    break "probe finished";
                }
            }
            Ok(None) => break "the session channel CLOSED",
            Err(_) => break "timed out waiting for output",
        }
    };
    session.close();

    println!("---- stopped because: {why} ----");

    println!("---- remote output ----\n{seen}\n-----------------------");
    assert!(
        seen.contains("X11 OK"),
        "the probe never reached the local X display; output was:\n{seen}"
    );
    assert!(
        seen.contains("X11_PROBE_DONE_0"),
        "the probe exited non-zero; output was:\n{seen}"
    );
}

fn whoami() -> String {
    env("USER").or_else(|| env("USERNAME")).unwrap_or_else(|| "root".into())
}
