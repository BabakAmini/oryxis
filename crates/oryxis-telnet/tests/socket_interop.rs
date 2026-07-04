//! Real-socket interop for the client -> server path. The crate's
//! in-src test already drives a real `TcpListener` for negotiation +
//! TTYPE subneg + login autofill + server-to-client data; this adds
//! the missing half: that terminal input is put on the wire in valid
//! Telnet form (IAC doubled, the Enter key's CR mapped to CR LF), by
//! parsing it back through a minimal server-side IAC un-escaper.

use oryxis_telnet::{TelnetConfig, TelnetSession};
use tokio::io::AsyncReadExt;
use tokio::net::TcpListener;

const IAC: u8 = 255;
const SB: u8 = 250;
const SE: u8 = 240;

/// Strip Telnet framing from a raw server-received stream, returning the
/// application bytes: `IAC IAC` -> one `0xFF`, `IAC SB ... IAC SE` and
/// `IAC <WILL/WONT/DO/DONT> <opt>` dropped, other `IAC <cmd>` dropped.
fn unescape(raw: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < raw.len() {
        if raw[i] != IAC {
            out.push(raw[i]);
            i += 1;
            continue;
        }
        match raw.get(i + 1) {
            Some(&IAC) => {
                out.push(IAC);
                i += 2;
            }
            Some(&SB) => {
                // Skip to IAC SE.
                i += 2;
                while i < raw.len() && !(raw[i] == IAC && raw.get(i + 1) == Some(&SE)) {
                    i += 1;
                }
                i += 2;
            }
            Some(&(251..=254)) => i += 3, // WILL/WONT/DO/DONT + option
            Some(_) => i += 2,            // other 2-byte command
            None => i += 1,
        }
    }
    out
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn terminal_input_is_encoded_for_the_wire() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    // Server: accept, then read the whole client stream until it hangs
    // up, and hand back the un-escaped application bytes.
    let server = tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
        let mut raw = Vec::new();
        let mut buf = [0u8; 1024];
        loop {
            match sock.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => raw.extend_from_slice(&buf[..n]),
            }
        }
        unescape(&raw)
    });

    let (session, _rx) = TelnetSession::connect(TelnetConfig {
        host: addr.ip().to_string(),
        port: addr.port(),
        ..TelnetConfig::default()
    })
    .await
    .unwrap();

    // A literal 0xFF (must be IAC-doubled on the wire) surrounded by
    // text, then an Enter (bare CR, must become CR LF).
    session.write(&[b'x', 0xFF, b'y', b'\r']).unwrap();
    // Give the writer task time to flush, then close so the server's
    // read loop ends and returns the collected bytes.
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    session.close();

    let app = tokio::time::timeout(std::time::Duration::from_secs(5), server)
        .await
        .expect("server never finished")
        .unwrap();

    // The un-escaped stream must contain exactly the encoded form:
    // the 0xFF survived (proving it was doubled) and CR became CR LF.
    let expected: &[u8] = &[b'x', 0xFF, b'y', b'\r', b'\n'];
    assert!(
        app.windows(expected.len()).any(|w| w == expected),
        "input not correctly Telnet-encoded on the wire: {app:?}"
    );
}
