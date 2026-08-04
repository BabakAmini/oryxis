//! Hermetic integration tests for RFC 4252 partial-success auth (2FA,
//! issue #125). Stands up an in-process russh server that mimics the
//! Bitvise / sshd `AuthenticationMethods` flow: the first factor is
//! verified, the server answers "partial success, proceed with
//! keyboard-interactive", and a keyboard-interactive round challenges
//! for a TOTP verification code. Drives the real `SshEngine` against it
//! over loopback TCP.
//!
//! Server-side limitation that shapes these tests: russh 0.62's server
//! unconditionally clears `partial_success` on the password / publickey
//! / none reject paths (`auth_request.partial_success = false;` right
//! after honoring the handler's verdict in `server/encrypted.rs`), so
//! the only partial success a russh server can actually put on the wire
//! is the keyboard-interactive response path. The first factor here is
//! therefore a keyboard-interactive password round; the CLIENT-side
//! continuation code under test is the same one the password and
//! publickey first factors feed (`finish_partial_auth`), and the real
//! password-first / publickey-first exchanges are covered end-to-end
//! against genuine sshd by `partial_auth_against_real_sshd` in
//! `tests/ssh_integration.rs`.
//!
//! Neither maintainer runs a compound-auth server, so the path is
//! validated here instead.

use std::borrow::Cow;
use std::sync::Arc;
use std::time::Duration;

use crate::sftp_harness::HARNESS_HOST_KEY;
use crate::{HostKeyCheckCallback, HostKeyStatus, KbiAskSender, SshEngine};
use oryxis_core::models::connection::{AuthMethod, Connection};
use oryxis_core::totp::Totp;
use russh::server::{Auth, Response};
use russh::{MethodKind, MethodSet};

const TEST_USER: &str = "tester";
const TEST_PASS: &str = "correct-horse";
/// Base32 TOTP secret shared by the fake server and the client's vault
/// side. Test fixture only, never used anywhere real.
const TOTP_SECRET: &str = "JBSWY3DPEHPK3PXP";

/// True when `code` is the secret's TOTP for the current 30s window or
/// an adjacent one (standard +-1 step tolerance, so a test crossing a
/// window boundary between generation and verification can't flake).
fn totp_code_ok(code: &str) -> bool {
    let totp = Totp::parse(TOTP_SECRET).expect("fixture secret parses");
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock after epoch")
        .as_secs();
    [now.saturating_sub(30), now, now + 30]
        .iter()
        .any(|t| totp.code_at(*t) == code)
}

/// Per-connection server handler enforcing the two-factor order. The
/// first keyboard-interactive exchange asks for the password and, on
/// the correct answer, replies PARTIAL SUCCESS with
/// keyboard-interactive as the follow-up; the second exchange asks for
/// the verification code and only accepts a valid TOTP.
struct TwoFactorHandler {
    password_ok: bool,
}

impl russh::server::Handler for TwoFactorHandler {
    type Error = russh::Error;

    async fn auth_keyboard_interactive<'a>(
        &'a mut self,
        user: &str,
        _submethods: &str,
        response: Option<Response<'a>>,
    ) -> Result<Auth, Self::Error> {
        if user != TEST_USER {
            return Ok(Auth::reject());
        }
        let Some(resp) = response else {
            // Exchange start: challenge for whichever factor is next.
            let prompt = if self.password_ok { "Verification code: " } else { "Password: " };
            return Ok(Auth::Partial {
                name: Cow::Borrowed("Two-factor authentication"),
                instructions: Cow::Borrowed(""),
                prompts: Cow::Owned(vec![(Cow::Borrowed(prompt), false)]),
            });
        };
        let answers: Vec<String> = resp
            .map(|bytes| String::from_utf8_lossy(&bytes).to_string())
            .collect();
        let answer = answers.first().map(String::as_str).unwrap_or_default();
        if self.password_ok {
            if totp_code_ok(answer) {
                Ok(Auth::Accept)
            } else {
                Ok(Auth::reject())
            }
        } else if answer == TEST_PASS {
            self.password_ok = true;
            // The wire-visible RFC 4252 partial success under test.
            Ok(Auth::Reject {
                proceed_with_methods: Some(MethodSet::from(
                    &[MethodKind::KeyboardInteractive][..],
                )),
                partial_success: true,
            })
        } else {
            Ok(Auth::reject())
        }
    }
}

/// Spawn the loopback two-factor server; returns the bound port. Loops
/// on accept with a fresh handler (fresh factor state) per connection.
async fn spawn_two_factor_server() -> u16 {
    use russh::keys::PrivateKey;

    let mut config = russh::server::Config::default();
    config
        .keys
        .push(PrivateKey::from_openssh(HARNESS_HOST_KEY).expect("parse host key"));
    // Rejection pacing is anti-bruteforce theater on loopback; partial
    // success rides the rejection path, so zero it to keep tests fast.
    config.auth_rejection_time = Duration::ZERO;
    config.auth_rejection_time_initial = Some(Duration::ZERO);
    let config = Arc::new(config);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind loopback");
    let port = listener.local_addr().expect("local_addr").port();

    tokio::spawn(async move {
        while let Ok((socket, _)) = listener.accept().await {
            let config = config.clone();
            tokio::spawn(async move {
                let handler = TwoFactorHandler { password_ok: false };
                if let Ok(running) = russh::server::run_stream(config, socket, handler).await {
                    let _ = running.await;
                }
            });
        }
    });

    port
}

fn loopback_conn(port: u16) -> Connection {
    let mut conn = Connection::new("partial-auth-test", "127.0.0.1");
    conn.port = port;
    conn.username = Some(TEST_USER.to_string());
    conn.auth_method = AuthMethod::Interactive;
    conn
}

fn engine() -> SshEngine {
    let accept_all: HostKeyCheckCallback = Arc::new(|_, _, _, _| HostKeyStatus::Known);
    SshEngine::new()
        .with_host_key_check(accept_all)
        .with_connect_timeout(Duration::from_secs(10))
        .with_auth_timeout(Duration::from_secs(10))
}

/// Issue #125's shape with a stored secret: the first factor is
/// accepted, the server demands more, and the TOTP autofill answers the
/// verification-code round silently. Before the fix the partial success
/// was reported as a plain rejection and the connect died.
#[tokio::test]
async fn first_factor_then_totp_autofill_authenticates() {
    let port = spawn_two_factor_server().await;
    let conn = loopback_conn(port);
    let engine = engine().with_totp_secret(Some(TOTP_SECRET));

    let mut handle = engine
        .establish_transport(&conn, None)
        .await
        .expect("transport");
    engine
        .do_authenticate(&mut handle, &conn, Some(TEST_PASS), None)
        .await
        .expect("first factor + TOTP continuation must authenticate");
}

/// No stored secret, but a UI channel: the verification-code round must
/// surface as a keyboard-interactive prompt (what OpenSSH does), and
/// the typed answer completes the auth.
#[tokio::test]
async fn first_factor_then_prompted_code_authenticates() {
    let port = spawn_two_factor_server().await;
    let conn = loopback_conn(port);

    let (tx, mut rx) = tokio::sync::mpsc::channel(4);
    let kbi_tx: KbiAskSender = tx;
    // Answer each round the way a user reading the prompts would.
    tokio::spawn(async move {
        while let Some((query, resp_tx)) = rx.recv().await {
            let answers: Vec<String> = query
                .prompts
                .iter()
                .map(|p| {
                    if p.prompt.contains("Password") {
                        TEST_PASS.to_string()
                    } else {
                        assert!(
                            p.prompt.contains("Verification code"),
                            "unexpected prompt surfaced to the UI: {}",
                            p.prompt,
                        );
                        Totp::parse(TOTP_SECRET).expect("fixture parses").code_now()
                    }
                })
                .collect();
            let _ = resp_tx.send(Some(answers));
        }
    });
    let engine = engine().with_kbi_ask(kbi_tx);

    let mut handle = engine
        .establish_transport(&conn, None)
        .await
        .expect("transport");
    engine
        .do_authenticate(&mut handle, &conn, None, None)
        .await
        .expect("first factor + prompted code must authenticate");
}

/// No second-factor source at all (no secret, no UI): the auth must
/// fail with the honest "requires additional authentication" error.
/// The pre-fix behavior reported the ACCEPTED first factor as rejected.
#[tokio::test]
async fn missing_second_factor_fails_honestly() {
    let port = spawn_two_factor_server().await;
    let conn = loopback_conn(port);
    let engine = engine();

    let mut handle = engine
        .establish_transport(&conn, None)
        .await
        .expect("transport");
    let err = engine
        .do_authenticate(&mut handle, &conn, Some(TEST_PASS), None)
        .await
        .expect_err("no second factor available, auth must fail");
    let msg = err.to_string();
    assert!(
        msg.contains("additional authentication"),
        "error must name the missing second factor, got: {msg}",
    );
    assert!(
        !msg.contains("rejected"),
        "an accepted first factor must not be reported as rejected, got: {msg}",
    );
}
