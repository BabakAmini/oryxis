//! Async driver that runs one ZMODEM transfer to completion.
//!
//! `zmodem2` is a synchronous poll/submit state machine; this wraps it
//! in a tokio task that pumps its [`Action`]s against the pane's
//! transport (the `WriteWire` bytes go out through the session's input
//! sender, exactly where a keystroke would) and the local filesystem
//! (`WriteFile` for a download, `ReadFile` for an upload). Wire input
//! arrives on a channel the app fills with the bytes it diverts from
//! the terminal; progress and the terminal result flow back on another.
//!
//! Design constraints that shaped this:
//!
//! - **No blocking dialogs mid-flight.** ZMODEM peers time out, so the
//!   destination (a download folder) and the source (an upload file)
//!   are decided by the caller BEFORE the driver starts; the driver
//!   never stops to ask.
//! - **`submit_wire` may take only part of a chunk** (heapless internal
//!   buffer), so unconsumed wire is retained and re-submitted rather
//!   than dropped.
//! - **Abort is cooperative**: a shared flag lets the app request a
//!   `ZCAN`; the machine then emits its cancel bytes and ends with
//!   `Aborted`, so the remote tears down cleanly instead of hanging.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use tokio::sync::mpsc;
use zmodem2::{Action, Event, FileInfo, Position, Receiver, Sender};

use crate::Direction;

/// Where a transfer's bytes come from / go to. Chosen by the caller up
/// front (see the no-blocking-dialogs note above).
#[derive(Debug, Clone)]
pub enum TransferSpec {
    /// Download: save each incoming file into this directory under its
    /// advertised (sanitized) name.
    Download { dest_dir: PathBuf },
    /// Upload: send this single local file.
    Upload { source: PathBuf },
}

/// Progress and terminal outcome, streamed to the app.
#[derive(Debug, Clone)]
pub enum Progress {
    /// A file began; `size` is the advertised total when known.
    Started { name: String, size: Option<u64> },
    /// Cumulative bytes moved for the current file.
    Advanced { transferred: u64, total: Option<u64> },
    /// One file finished; `path` is where a download landed.
    FileDone { name: String, path: Option<PathBuf> },
    /// The whole session finished successfully.
    Completed,
    /// The session was cancelled (by us or the peer).
    Aborted,
    /// The transfer failed; the string is a human-readable reason.
    Error(String),
}

/// Handles the app passes in to drive one transfer.
pub struct TransferIo {
    /// Bytes diverted from the terminal (the detector's `wire` first,
    /// then every subsequent pane output batch). Closing it (drop) is
    /// treated as a disconnect and aborts the transfer.
    pub wire_in: mpsc::UnboundedReceiver<Vec<u8>>,
    /// The pane transport's input sender: protocol replies to the peer.
    pub wire_out: mpsc::UnboundedSender<Vec<u8>>,
    /// Progress + outcome sink.
    pub progress: mpsc::UnboundedSender<Progress>,
    /// Set by the app to request cancellation.
    pub abort: Arc<AtomicBool>,
}

/// Run a transfer to completion. Consumes `first_wire` (the detector's
/// initial bytes) before awaiting more on `io.wire_in`. Always emits a
/// terminal `Progress` (`Completed` / `Aborted` / `Error`) before
/// returning, so the app can reliably leave divert mode.
pub async fn run(direction: Direction, spec: TransferSpec, first_wire: Vec<u8>, mut io: TransferIo) {
    let result = match direction {
        Direction::Download => run_download(spec, first_wire, &mut io).await,
        Direction::Upload => run_upload(spec, first_wire, &mut io).await,
    };
    let terminal = match result {
        Ok(Outcome::Completed) => Progress::Completed,
        Ok(Outcome::Aborted) => Progress::Aborted,
        Err(e) => Progress::Error(e),
    };
    let _ = io.progress.send(terminal);
}

enum Outcome {
    Completed,
    Aborted,
}

/// One decoded step, copied out of the borrowed `Action` so the machine
/// can be mutated again in the same loop turn.
enum Step {
    WriteWire(Vec<u8>),
    WriteFile(Vec<u8>),
    ReadFile { offset: u32, max_len: usize },
    Started { name: Vec<u8>, size: Option<u64> },
    FileDone,
    Completed,
    Aborted,
    Idle,
}

fn decode(action: Action<'_>) -> Step {
    match action {
        Action::WriteWire(b) => Step::WriteWire(b.to_vec()),
        Action::WriteFile(b) => Step::WriteFile(b.to_vec()),
        Action::ReadFile { offset, max_len } => Step::ReadFile {
            offset: offset.get(),
            max_len,
        },
        Action::Event(Event::FileStarted(info)) => Step::Started {
            name: info.name.to_vec(),
            size: info.size.map(|p| u64::from(p.get())),
        },
        Action::Event(Event::FileCompleted) => Step::FileDone,
        Action::Event(Event::SessionCompleted) => Step::Completed,
        Action::Event(Event::Aborted) => Step::Aborted,
        Action::Idle => Step::Idle,
        // `Action` / `Event` are `#[non_exhaustive]`; treat anything new
        // as idle so a crate bump can't silently break the loop.
        _ => Step::Idle,
    }
}

async fn run_download(
    spec: TransferSpec,
    first_wire: Vec<u8>,
    io: &mut TransferIo,
) -> Result<Outcome, String> {
    let TransferSpec::Download { dest_dir } = spec else {
        return Err("download driver given a non-download spec".into());
    };
    let mut receiver = Receiver::new().map_err(|e| format!("zmodem init: {e:?}"))?;
    let mut pending = first_wire;
    let mut dest: Option<tokio::fs::File> = None;
    let mut dest_path: Option<PathBuf> = None;
    let mut name = String::new();
    let mut transferred: u64 = 0;
    let mut total: Option<u64> = None;

    loop {
        if io.abort.load(Ordering::Relaxed) {
            let _ = receiver.abort();
        }
        match decode(receiver.poll()) {
            Step::WriteWire(bytes) => {
                let n = bytes.len();
                let _ = io.wire_out.send(bytes);
                receiver.wire_written(n);
            }
            Step::WriteFile(bytes) => {
                let n = bytes.len();
                if let Some(file) = dest.as_mut() {
                    file.write_all(&bytes)
                        .await
                        .map_err(|e| format!("write: {e}"))?;
                }
                transferred += n as u64;
                receiver
                    .file_written(n)
                    .map_err(|e| format!("zmodem file: {e:?}"))?;
                let _ = io.progress.send(Progress::Advanced { transferred, total });
            }
            Step::ReadFile { .. } => {
                return Err("receiver asked to read a file (protocol error)".into());
            }
            Step::Started { name: raw, size } => {
                let safe = sanitize_name(&raw);
                let path = dest_dir.join(&safe);
                let file = tokio::fs::File::create(&path)
                    .await
                    .map_err(|e| format!("create {}: {e}", path.display()))?;
                dest = Some(file);
                dest_path = Some(path);
                name = safe;
                total = size;
                transferred = 0;
                let _ = io.progress.send(Progress::Started {
                    name: name.clone(),
                    size,
                });
            }
            Step::FileDone => {
                if let Some(mut file) = dest.take() {
                    file.flush().await.map_err(|e| format!("flush: {e}"))?;
                }
                let _ = io.progress.send(Progress::FileDone {
                    name: name.clone(),
                    path: dest_path.take(),
                });
            }
            Step::Completed => return Ok(Outcome::Completed),
            Step::Aborted => return Ok(Outcome::Aborted),
            Step::Idle => {
                if pending.is_empty() {
                    match io.wire_in.recv().await {
                        Some(bytes) => pending = bytes,
                        None => return Err("connection closed during transfer".into()),
                    }
                }
                let consumed = receiver
                    .submit_wire(&pending)
                    .map_err(|e| format!("zmodem wire: {e:?}"))?;
                pending.drain(..consumed);
            }
        }
    }
}

async fn run_upload(
    spec: TransferSpec,
    first_wire: Vec<u8>,
    io: &mut TransferIo,
) -> Result<Outcome, String> {
    let TransferSpec::Upload { source } = spec else {
        return Err("upload driver given a non-upload spec".into());
    };
    let mut file = tokio::fs::File::open(&source)
        .await
        .map_err(|e| format!("open {}: {e}", source.display()))?;
    let size = file.metadata().await.map_err(|e| format!("stat: {e}"))?.len();
    let size32 = u32::try_from(size).map_err(|_| "file larger than 4 GiB (ZMODEM limit)")?;
    let name = source
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "file".into());

    let mut sender = Sender::new().map_err(|e| format!("zmodem init: {e:?}"))?;
    sender
        .start_file(FileInfo::new(name.as_bytes(), Some(Position::new(size32))))
        .map_err(|e| format!("zmodem start_file: {e:?}"))?;
    // Request session end after this single file; honored when the file
    // completes (the machine queues ZFIN then).
    sender.finish().map_err(|e| format!("zmodem finish: {e:?}"))?;
    let _ = io.progress.send(Progress::Started {
        name: name.clone(),
        size: Some(size),
    });

    let mut pending = first_wire;

    loop {
        if io.abort.load(Ordering::Relaxed) {
            sender.abort();
        }
        match decode(sender.poll()) {
            Step::WriteWire(bytes) => {
                let n = bytes.len();
                let _ = io.wire_out.send(bytes);
                sender.wire_written(n);
            }
            Step::ReadFile { offset, max_len } => {
                file.seek(std::io::SeekFrom::Start(u64::from(offset)))
                    .await
                    .map_err(|e| format!("seek: {e}"))?;
                let mut buf = vec![0u8; max_len];
                let read = file.read(&mut buf).await.map_err(|e| format!("read: {e}"))?;
                buf.truncate(read);
                if read == 0 {
                    return Err("unexpected end of file during upload".into());
                }
                sender
                    .submit_file(&buf)
                    .map_err(|e| format!("zmodem submit: {e:?}"))?;
                let _ = io.progress.send(Progress::Advanced {
                    transferred: u64::from(offset) + read as u64,
                    total: Some(size),
                });
            }
            Step::WriteFile(_) => {
                return Err("sender asked to write a file (protocol error)".into());
            }
            Step::Started { .. } => {}
            Step::FileDone => {
                let _ = io.progress.send(Progress::FileDone {
                    name: name.clone(),
                    path: None,
                });
            }
            Step::Completed => return Ok(Outcome::Completed),
            Step::Aborted => return Ok(Outcome::Aborted),
            Step::Idle => {
                if pending.is_empty() {
                    match io.wire_in.recv().await {
                        Some(bytes) => pending = bytes,
                        None => return Err("connection closed during transfer".into()),
                    }
                }
                let consumed = sender
                    .submit_wire(&pending)
                    .map_err(|e| format!("zmodem wire: {e:?}"))?;
                pending.drain(..consumed);
            }
        }
    }
}

/// Strip a sender-advertised name down to a safe basename: no directory
/// components, no `..`, no leading dots that could hide the file. A
/// hostile or careless peer must not be able to write outside the
/// chosen download folder.
fn sanitize_name(raw: &[u8]) -> String {
    let lossy = String::from_utf8_lossy(raw);
    let base = lossy
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or("")
        .trim_matches(|c: char| c == '.' || c.is_whitespace() || c.is_control());
    if base.is_empty() || base == ".." {
        "received.bin".to_string()
    } else {
        base.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_strips_path_traversal_and_separators() {
        assert_eq!(sanitize_name(b"../../etc/passwd"), "passwd");
        assert_eq!(sanitize_name(b"C:\\Windows\\System32\\evil.dll"), "evil.dll");
        assert_eq!(sanitize_name(b"report.pdf"), "report.pdf");
        assert_eq!(sanitize_name(b"/abs/log.txt"), "log.txt");
        assert_eq!(sanitize_name(b""), "received.bin");
        assert_eq!(sanitize_name(b"..."), "received.bin");
        assert_eq!(sanitize_name(b"/"), "received.bin");
    }

    /// End-to-end oracle: drive our upload driver and our download
    /// driver against each other over crossed channels (each plays the
    /// role lrzsz would on the wire) and assert the file round-trips.
    /// This validates the whole poll/submit loop, the finish() timing,
    /// the ReadFile/WriteFile handling and partial-wire retry, none of
    /// which the crate's own one-sided tests cover.
    #[test]
    fn loopback_upload_to_download_round_trips_a_file() {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let dir = std::env::temp_dir().join(format!("oryxis-zm-{}", std::process::id()));
            let _ = tokio::fs::create_dir_all(&dir).await;
            let src = dir.join("payload.bin");
            let dest_dir = dir.join("incoming");
            let _ = tokio::fs::create_dir_all(&dest_dir).await;
            // A payload big enough to span several subpackets and force
            // multiple ReadFile/WriteFile round trips, with control-ish
            // bytes (0x11 XON, 0x18 ZDLE, 0x0d CR) that ZMODEM escapes.
            let payload: Vec<u8> = (0..8192u32).map(|i| (i % 251) as u8).collect();
            tokio::fs::write(&src, &payload).await.unwrap();

            let (up2down_tx, up2down_rx) = mpsc::unbounded_channel();
            let (down2up_tx, down2up_rx) = mpsc::unbounded_channel();
            let (p_up_tx, _p_up_rx) = mpsc::unbounded_channel();
            let (p_down_tx, mut p_down_rx) = mpsc::unbounded_channel();
            let abort = Arc::new(AtomicBool::new(false));

            let up = tokio::spawn(run(
                Direction::Upload,
                TransferSpec::Upload { source: src.clone() },
                Vec::new(),
                TransferIo {
                    wire_in: down2up_rx,
                    wire_out: up2down_tx,
                    progress: p_up_tx,
                    abort: abort.clone(),
                },
            ));
            let down = tokio::spawn(run(
                Direction::Download,
                TransferSpec::Download {
                    dest_dir: dest_dir.clone(),
                },
                Vec::new(),
                TransferIo {
                    wire_in: up2down_rx,
                    wire_out: down2up_tx,
                    progress: p_down_tx,
                    abort,
                },
            ));

            let both = async {
                up.await.unwrap();
                down.await.unwrap();
            };
            tokio::time::timeout(std::time::Duration::from_secs(20), both)
                .await
                .expect("transfer deadlocked");

            // The download side must have reported Completed.
            let mut completed = false;
            let mut saved: Option<PathBuf> = None;
            while let Ok(p) = p_down_rx.try_recv() {
                match p {
                    Progress::Completed => completed = true,
                    Progress::FileDone { path, .. } => saved = path,
                    Progress::Error(e) => panic!("download error: {e}"),
                    _ => {}
                }
            }
            assert!(completed, "download never completed");
            let saved = saved.expect("no saved path reported");
            let got = tokio::fs::read(&saved).await.unwrap();
            assert_eq!(got, payload, "round-tripped bytes differ");

            let _ = tokio::fs::remove_dir_all(&dir).await;
        });
    }
}
