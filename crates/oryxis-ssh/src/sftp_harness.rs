//! In-process SSH + SFTP server for tests.
//!
//! Wires a real russh client to a real russh-sftp server over an
//! in-memory `tokio::io::duplex` stream: no TCP, no Docker, no network,
//! so it runs in a plain `cargo test` everywhere including CI. Unlike the
//! in-memory unit fakes, this drives the actual protocol path (handshake,
//! channel, sftp subsystem, request-id multiplexing), so it is the real
//! check that `SftpClient`'s concurrent streaming reassembles correctly
//! against a server, not just against a stub.
//!
//! The server filesystem lives entirely in a `HashMap`; only the handful
//! of operations the streaming paths touch are implemented.

#![cfg(test)]

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

use russh::server::{Auth, Msg, Session};
use russh::{Channel, ChannelId};
use russh_sftp::protocol::{
    Attrs, Data, File, FileAttributes, Handle, Name, OpenFlags, Status, StatusCode, Version,
};

use crate::engine::{ClientHandler, SharedHandle};
use crate::sftp::SftpClient;

/// Throwaway ed25519 host key for the in-process server (generated once
/// with `ssh-keygen`, never used anywhere real). Avoids pulling an RNG
/// path into the test.
pub(crate) const HARNESS_HOST_KEY: &str = "-----BEGIN OPENSSH PRIVATE KEY-----
b3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQAAAAAAAAABAAAAMwAAAAtzc2gtZW
QyNTUxOQAAACBlvoDcBf/w9DbcBLuL2Rj1Lvv7QsEoUz4BIn2EjAQ7tgAAAJDSAIzt0gCM
7QAAAAtzc2gtZWQyNTUxOQAAACBlvoDcBf/w9DbcBLuL2Rj1Lvv7QsEoUz4BIn2EjAQ7tg
AAAEAfVzLcxRas90R8PzqxnURWULsvE8T9Z/naok4PjYsemmW+gNwF//D0NtwEu4vZGPUu
+/tCwShTPgEifYSMBDu2AAAAB2hhcm5lc3MBAgMEBQY=
-----END OPENSSH PRIVATE KEY-----
";

// ---------------------------------------------------------------------------
// In-memory filesystem
// ---------------------------------------------------------------------------

#[derive(Default)]
struct Fs {
    /// Absolute path -> file contents.
    files: HashMap<String, Vec<u8>>,
    /// Open handle id -> the path it refers to.
    handles: HashMap<String, String>,
    next_handle: u64,
}

type SharedFs = Arc<Mutex<Fs>>;

fn ok_status(id: u32) -> Status {
    Status {
        id,
        status_code: StatusCode::Ok,
        error_message: "Ok".to_string(),
        language_tag: "en-US".to_string(),
    }
}

/// Collapse a POSIX path to one canonical spelling: empty and `.` become
/// root, repeated separators fold, `.` segments drop and `..` pops the
/// segment before it (never above root). What a server's `realpath`
/// answers for a symlink-free tree.
fn normalize_posix(path: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    for seg in path.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                out.pop();
            }
            s => out.push(s),
        }
    }
    if out.is_empty() {
        "/".to_string()
    } else {
        format!("/{}", out.join("/"))
    }
}

// ---------------------------------------------------------------------------
// SFTP subsystem handler (in-memory)
// ---------------------------------------------------------------------------

struct SftpHandler {
    fs: SharedFs,
    version: Option<u32>,
}

impl russh_sftp::server::Handler for SftpHandler {
    type Error = StatusCode;

    fn unimplemented(&self) -> StatusCode {
        StatusCode::OpUnsupported
    }

    async fn init(
        &mut self,
        version: u32,
        _extensions: HashMap<String, String>,
    ) -> Result<Version, StatusCode> {
        self.version = Some(version);
        Ok(Version::new())
    }

    async fn realpath(&mut self, id: u32, path: String) -> Result<Name, StatusCode> {
        // Normalise the way a real server does: collapse repeated
        // separators and resolve `.` / `..` segments, so two different
        // spellings of one file answer with one identity. There are no
        // symlinks in this filesystem, so that is the whole of it.
        Ok(Name {
            id,
            files: vec![File::dummy(normalize_posix(&path))],
        })
    }

    async fn open(
        &mut self,
        id: u32,
        filename: String,
        pflags: OpenFlags,
        _attrs: FileAttributes,
    ) -> Result<Handle, StatusCode> {
        let mut fs = self.fs.lock().await;
        let exists = fs.files.contains_key(&filename);
        if pflags.contains(OpenFlags::READ) && !exists {
            return Err(StatusCode::NoSuchFile);
        }
        if pflags.contains(OpenFlags::TRUNCATE)
            || (pflags.contains(OpenFlags::CREATE) && !exists)
        {
            fs.files.insert(filename.clone(), Vec::new());
        }
        fs.next_handle += 1;
        let hid = format!("h{}", fs.next_handle);
        fs.handles.insert(hid.clone(), filename);
        Ok(Handle { id, handle: hid })
    }

    async fn close(&mut self, id: u32, handle: String) -> Result<Status, StatusCode> {
        self.fs.lock().await.handles.remove(&handle);
        Ok(ok_status(id))
    }

    async fn read(
        &mut self,
        id: u32,
        handle: String,
        offset: u64,
        len: u32,
    ) -> Result<Data, StatusCode> {
        let fs = self.fs.lock().await;
        let path = fs.handles.get(&handle).ok_or(StatusCode::Failure)?;
        let data = fs.files.get(path).ok_or(StatusCode::NoSuchFile)?;
        let off = offset as usize;
        if off >= data.len() {
            // Clean EOF: the client maps this to a 0-byte read.
            return Err(StatusCode::Eof);
        }
        let end = (off + len as usize).min(data.len());
        Ok(Data {
            id,
            data: data[off..end].to_vec(),
        })
    }

    async fn write(
        &mut self,
        id: u32,
        handle: String,
        offset: u64,
        data: Vec<u8>,
    ) -> Result<Status, StatusCode> {
        let mut fs = self.fs.lock().await;
        let path = fs.handles.get(&handle).ok_or(StatusCode::Failure)?.clone();
        let buf = fs.files.entry(path).or_default();
        let off = offset as usize;
        let end = off + data.len();
        if buf.len() < end {
            buf.resize(end, 0);
        }
        buf[off..end].copy_from_slice(&data);
        Ok(ok_status(id))
    }

    async fn stat(&mut self, id: u32, path: String) -> Result<Attrs, StatusCode> {
        let fs = self.fs.lock().await;
        let size = fs
            .files
            .get(&path)
            .map(|d| d.len() as u64)
            .ok_or(StatusCode::NoSuchFile)?;
        Ok(attrs_with_size(id, size))
    }

    async fn fstat(&mut self, id: u32, handle: String) -> Result<Attrs, StatusCode> {
        let fs = self.fs.lock().await;
        let path = fs.handles.get(&handle).ok_or(StatusCode::Failure)?;
        let size = fs.files.get(path).map(|d| d.len() as u64).unwrap_or(0);
        Ok(attrs_with_size(id, size))
    }
}

fn attrs_with_size(id: u32, size: u64) -> Attrs {
    let mut attrs = FileAttributes::empty();
    attrs.size = Some(size);
    Attrs { id, attrs }
}

// ---------------------------------------------------------------------------
// SSH server handler: accept everything, hand the sftp subsystem channel
// to the in-memory SFTP handler.
// ---------------------------------------------------------------------------

struct SshHarness {
    channels: Arc<Mutex<HashMap<ChannelId, Channel<Msg>>>>,
    fs: SharedFs,
}

impl russh::server::Handler for SshHarness {
    type Error = russh::Error;

    async fn auth_none(&mut self, _user: &str) -> Result<Auth, russh::Error> {
        Ok(Auth::Accept)
    }

    async fn auth_password(&mut self, _user: &str, _password: &str) -> Result<Auth, russh::Error> {
        Ok(Auth::Accept)
    }

    async fn channel_open_session(
        &mut self,
        channel: Channel<Msg>,
        reply: russh::server::ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), russh::Error> {
        reply.accept().await;
        self.channels.lock().await.insert(channel.id(), channel);
        Ok(())
    }

    async fn subsystem_request(
        &mut self,
        channel_id: ChannelId,
        name: &str,
        session: &mut Session,
    ) -> Result<(), russh::Error> {
        if name == "sftp" {
            let channel = self
                .channels
                .lock()
                .await
                .remove(&channel_id)
                .expect("channel opened before subsystem request");
            session.channel_success(channel_id)?;
            let handler = SftpHandler {
                fs: self.fs.clone(),
                version: None,
            };
            // `run` spawns its own task and returns promptly.
            russh_sftp::server::run(channel.into_stream(), handler).await;
        } else {
            session.channel_failure(channel_id)?;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Harness entry point
// ---------------------------------------------------------------------------

/// Stand up the in-process server and return a connected [`SftpClient`]
/// plus a handle to the server's filesystem (for seeding / inspecting).
async fn connect_in_memory() -> (SftpClient, SharedFs) {
    use russh::keys::PrivateKey;

    let fs: SharedFs = Arc::new(Mutex::new(Fs::default()));
    let (client_io, server_io) = tokio::io::duplex(64 * 1024);

    // Server side. A fixed throwaway host key (generated once with
    // ssh-keygen) so the harness needs no RNG dependency; the client
    // trusts any key (see `ClientHandler::test_accept_all`).
    let mut server_config = russh::server::Config::default();
    let host_key = PrivateKey::from_openssh(HARNESS_HOST_KEY).expect("parse host key");
    server_config.keys.push(host_key);
    let server_config = Arc::new(server_config);
    let server_handler = SshHarness {
        channels: Arc::new(Mutex::new(HashMap::new())),
        fs: fs.clone(),
    };
    tokio::spawn(async move {
        if let Ok(running) =
            russh::server::run_stream(server_config, server_io, server_handler).await
        {
            let _ = running.await;
        }
    });

    // Client side.
    let client_config = Arc::new(russh::client::Config::default());
    let mut handle =
        russh::client::connect_stream(client_config, client_io, ClientHandler::test_accept_all())
            .await
            .expect("client connect_stream");
    let auth = handle
        .authenticate_password("tester", "tester")
        .await
        .expect("authenticate_password");
    assert!(auth.success(), "harness auth rejected");

    // Open the sftp subsystem the same way `engine::open_sftp` does.
    let timeout = std::time::Duration::from_secs(10);
    let shared: SharedHandle = Arc::new(Mutex::new(handle));
    let session = {
        let h = shared.lock().await;
        let channel = h.channel_open_session().await.expect("channel_open_session");
        channel
            .request_subsystem(true, "sftp")
            .await
            .expect("request_subsystem");
        russh_sftp::client::SftpSession::new(channel.into_stream())
            .await
            .expect("SftpSession::new")
    };
    let client = SftpClient::new(session, shared, timeout);
    (client, fs)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Round-trip a payload local -> remote -> local through the real
/// protocol and assert byte-identical. `size` chosen by the caller to hit
/// either the single-handle path or the windowed path.
async fn round_trip_through_server(size: usize) {
    let (client, _fs) = connect_in_memory().await;

    let payload: Vec<u8> = (0..size).map(|i| (i % 251) as u8).collect();
    let tmp = std::env::temp_dir();
    let pid = std::process::id();
    let src = tmp.join(format!("oryxis-harness-src-{pid}-{size}.bin"));
    let dst = tmp.join(format!("oryxis-harness-dst-{pid}-{size}.bin"));
    std::fs::write(&src, &payload).expect("write src");

    let remote = "/file.bin";
    client.upload_from(&src, remote).await.expect("upload_from");
    let stat = client.stat(remote).await.expect("stat");
    assert_eq!(stat.size, size as u64, "remote size mismatch");

    client
        .download_to(remote, &dst, None)
        .await
        .expect("download_to");
    let got = std::fs::read(&dst).expect("read dst");
    assert_eq!(got.len(), size, "size mismatch after round trip");
    assert_eq!(got, payload, "byte mismatch after round trip");

    let _ = std::fs::remove_file(&src);
    let _ = std::fs::remove_file(&dst);
}

#[tokio::test]
async fn harness_small_round_trip() {
    // Below STREAM_THRESHOLD: single-handle sequential path.
    round_trip_through_server(64 * 1024).await;
}

#[tokio::test]
async fn harness_windowed_round_trip() {
    // Above STREAM_THRESHOLD (8 MiB): both directions use the sliding
    // window, so this is the real-server check of concurrent single-handle
    // reassembly and the request-id multiplexing the throughput win
    // depends on.
    round_trip_through_server(10 * 1024 * 1024).await;
}

#[tokio::test]
async fn harness_threshold_boundary() {
    // Exactly at the threshold: first size that takes the windowed path.
    round_trip_through_server(8 * 1024 * 1024).await;
}

#[tokio::test]
async fn harness_relay_between_servers() {
    // Two independent in-process servers. Seed a file on the source, relay
    // it to the destination through the client, verify it landed intact.
    // This is the end-to-end server-to-server path (read from A, write to
    // B, bytes through this process).
    let (src, src_fs) = connect_in_memory().await;
    let (dst, dst_fs) = connect_in_memory().await;

    let payload: Vec<u8> = (0..600 * 1024).map(|i| (i % 251) as u8).collect();
    src_fs
        .lock()
        .await
        .files
        .insert("/source.bin".to_string(), payload.clone());

    src.relay_to("/source.bin", &dst, "/dest.bin", None)
        .await
        .expect("relay_to");

    let landed = dst_fs
        .lock()
        .await
        .files
        .get("/dest.bin")
        .cloned()
        .expect("dest file present after relay");
    assert_eq!(landed, payload, "relayed bytes mismatch");
}

#[tokio::test]
async fn harness_relay_onto_itself_is_refused() {
    // The destination is opened WRITE | CREATE | TRUNCATE before a byte
    // is read, so a self-relay would empty the file and the failure
    // cleanup would then remove it. Two SFTP clients on ONE connection
    // (what `open_sibling` gives, and what two panes mounted on one host
    // through a shared session are) must be refused before the open.
    let (src, fs) = connect_in_memory().await;
    let sibling = src.open_sibling().await.expect("open sibling");
    assert!(
        src.shares_session_with(&sibling),
        "a sibling must be recognised as the same connection"
    );

    let payload: Vec<u8> = (0..64 * 1024).map(|i| (i % 251) as u8).collect();
    fs.lock()
        .await
        .files
        .insert("/only-copy.bin".to_string(), payload.clone());

    let err = src
        .relay_to("/only-copy.bin", &sibling, "/only-copy.bin", None)
        .await
        .expect_err("relaying a file onto itself must be refused");
    assert!(
        err.to_string().contains("same file"),
        "unexpected refusal message: {err}"
    );

    let survived = fs
        .lock()
        .await
        .files
        .get("/only-copy.bin")
        .cloned()
        .expect("the file must still exist after the refusal");
    assert_eq!(survived, payload, "the refused relay damaged the file");
}

#[tokio::test]
async fn harness_relay_onto_itself_spelled_differently_is_refused() {
    // Same file, two spellings. The guard compares resolved identities,
    // not strings, so `/sub/../data.bin` must be caught as `/data.bin`.
    let (src, fs) = connect_in_memory().await;
    let sibling = src.open_sibling().await.expect("open sibling");

    let payload: Vec<u8> = (0..32 * 1024).map(|i| (i % 251) as u8).collect();
    fs.lock()
        .await
        .files
        .insert("/data.bin".to_string(), payload.clone());

    let err = src
        .relay_to("/data.bin", &sibling, "/sub/../data.bin", None)
        .await
        .expect_err("a differently spelled self-relay must be refused");
    assert!(
        err.to_string().contains("same file"),
        "unexpected refusal message: {err}"
    );
    assert_eq!(
        fs.lock().await.files.get("/data.bin"),
        Some(&payload),
        "the refused relay damaged the file"
    );
}

#[tokio::test]
async fn harness_relay_between_paths_on_one_host_is_allowed() {
    // The guard is on the resolved FILE, never on the host. Copying
    // between two locations on one server is a legitimate move (the
    // reporter's case was two users on one machine) and must still work.
    let (src, fs) = connect_in_memory().await;
    let sibling = src.open_sibling().await.expect("open sibling");

    let payload: Vec<u8> = (0..48 * 1024).map(|i| (i % 251) as u8).collect();
    fs.lock()
        .await
        .files
        .insert("/home/a/report.bin".to_string(), payload.clone());

    src.relay_to("/home/a/report.bin", &sibling, "/home/b/report.bin", None)
        .await
        .expect("same-host relay to a different path must be allowed");

    let guard = fs.lock().await;
    assert_eq!(
        guard.files.get("/home/b/report.bin"),
        Some(&payload),
        "the copy did not land"
    );
    assert_eq!(
        guard.files.get("/home/a/report.bin"),
        Some(&payload),
        "a relay must not disturb its source"
    );
}

#[tokio::test]
async fn harness_relay_same_path_across_hosts_is_allowed() {
    // Two independent servers, identical path on both. Nothing about the
    // path makes these the same file, and refusing here would break the
    // most ordinary relay there is (`/etc/app.conf` from staging to
    // prod).
    let (src, src_fs) = connect_in_memory().await;
    let (dst, dst_fs) = connect_in_memory().await;
    assert!(
        !src.shares_session_with(&dst),
        "two separate connections must not be seen as one"
    );

    let payload: Vec<u8> = (0..16 * 1024).map(|i| (i % 251) as u8).collect();
    src_fs
        .lock()
        .await
        .files
        .insert("/etc/app.conf".to_string(), payload.clone());

    src.relay_to("/etc/app.conf", &dst, "/etc/app.conf", None)
        .await
        .expect("same path on two different hosts must be allowed");

    assert_eq!(
        dst_fs.lock().await.files.get("/etc/app.conf"),
        Some(&payload),
        "the cross-host relay did not land"
    );
}

#[tokio::test]
async fn harness_relay_windowed_stale_hint() {
    // Server-to-server relay above the window threshold (10 MiB), with a
    // size hint that is too small (8 MiB) but still >= threshold. The
    // windowed branch must fstat the source for the true size and relay
    // the WHOLE file to the destination, not truncate to the hint.
    let (src, src_fs) = connect_in_memory().await;
    let (dst, dst_fs) = connect_in_memory().await;

    let actual_size = 10 * 1024 * 1024usize;
    let payload: Vec<u8> = (0..actual_size).map(|i| (i % 251) as u8).collect();
    src_fs
        .lock()
        .await
        .files
        .insert("/big-source.bin".to_string(), payload.clone());

    src.relay_to(
        "/big-source.bin",
        &dst,
        "/big-dest.bin",
        Some(8 * 1024 * 1024),
    )
    .await
    .expect("windowed relay with stale hint");

    let landed = dst_fs
        .lock()
        .await
        .files
        .get("/big-dest.bin")
        .cloned()
        .expect("dest file present after windowed relay");
    assert_eq!(landed.len(), actual_size, "stale hint truncated the relay");
    assert_eq!(landed, payload, "windowed relay corrupted the file");
}

#[tokio::test]
async fn harness_stale_hint_not_truncated() {
    // A size hint smaller than the real file (file grew since the dir
    // walk) but still >= threshold: the windowed branch must fstat the
    // open handle for the true size and download the WHOLE file, not
    // truncate to the stale hint. Without the fstat this silently
    // truncates and "succeeds".
    let (client, fs) = connect_in_memory().await;
    let actual_size = 10 * 1024 * 1024usize;
    let payload: Vec<u8> = (0..actual_size).map(|i| (i % 251) as u8).collect();
    fs.lock()
        .await
        .files
        .insert("/grew.bin".to_string(), payload.clone());

    let tmp = std::env::temp_dir();
    let dst = tmp.join(format!("oryxis-stalehint-{}.bin", std::process::id()));
    // Hint is 8 MiB (>= threshold, so windowed) but the file is 10 MiB.
    let stale_hint = Some(8 * 1024 * 1024u64);
    client
        .download_to("/grew.bin", &dst, stale_hint)
        .await
        .expect("download_to with stale hint");
    let got = std::fs::read(&dst).expect("read dst");
    assert_eq!(got.len(), actual_size, "stale hint truncated the download");
    assert_eq!(got, payload, "stale hint corrupted the download");
    let _ = std::fs::remove_file(&dst);
}

#[tokio::test]
async fn harness_ranged_reads() {
    // The zip-browse primitive: positioned reads over the real protocol
    // path, including spans that need several concurrent requests and
    // the EOF clamp.
    let (client, fs) = connect_in_memory().await;
    let size = 2 * 1024 * 1024usize;
    let payload: Vec<u8> = (0..size).map(|i| (i % 251) as u8).collect();
    fs.lock()
        .await
        .files
        .insert("/big.zip".to_string(), payload.clone());

    let file = client.open_ranged("/big.zip").await.expect("open_ranged");
    assert_eq!(file.len(), size as u64);
    // Small tail read (the EOCD probe pattern).
    let tail = file.read_at(size as u64 - 100, 100).await.expect("tail");
    assert_eq!(tail, &payload[size - 100..]);
    // A middle span larger than one 255 KiB request (concurrent fan-out).
    let mid = file.read_at(300_000, 600_000).await.expect("mid");
    assert_eq!(mid, &payload[300_000..900_000]);
    // Clamped at EOF; fully past EOF is empty.
    let clamp = file.read_at(size as u64 - 10, 50).await.expect("clamp");
    assert_eq!(clamp, &payload[size - 10..]);
    assert!(
        file.read_at(size as u64, 10).await.expect("past").is_empty()
    );
    file.close().await;
}
