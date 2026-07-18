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
//! - **Abort is cooperative but must never wait on the peer**: a shared
//!   flag lets the app request cancellation, and an empty chunk on
//!   `wire_in` wakes a driver parked on a silent peer's `recv` so the
//!   flag is actually seen. On abort the driver writes the canonical
//!   `CANCEL` sequence itself (`zmodem2`'s `abort()` only queues the
//!   `Aborted` event; it stages no wire bytes), so the remote tears
//!   down cleanly instead of waiting out its timeout.
//! - **Completion is not the end of the wire.** `poll()` yields events
//!   before queued wire bytes, so `SessionCompleted` arrives while the
//!   final handshake (the receiver's ZFIN reply, the sender's "OO") is
//!   still queued. The loops keep pumping until `Idle` before
//!   returning; a download then also absorbs the peer's trailing "OO".
//!   Returning on the event alone left the remote `sz` retrying ZFIN
//!   against silence, holding its tty for ~20 s of timeouts (issue
//!   #77). A local error sends `CANCEL` for the same reason: the peer
//!   must never be left to wait out its own clock.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt, BufReader, BufWriter};
use tokio::sync::mpsc;
use zmodem2::{Action, Event, FileInfo, Position, Receiver, Sender};

use crate::Direction;

/// Floor between two streamed `Advanced` reports. Unthrottled, one
/// report per ~1 KiB subpacket turns a large transfer into hundreds of
/// thousands of UI messages; 100 ms keeps the overlay smooth while the
/// exact figure always lands with the final per-file report.
const PROGRESS_INTERVAL: std::time::Duration = std::time::Duration::from_millis(100);

/// Buffered file I/O capacity. Every `tokio::fs` operation dispatches
/// to the blocking pool, so unbuffered per-subpacket I/O costs one
/// dispatch (plus syscall) per ~1 KiB; this amortizes it to ~4 per MiB.
const FILE_BUF_SIZE: usize = 256 * 1024;

/// How long a wait for peer bytes runs before the state machine's
/// `timeout()` is poked (re-queueing its handshake frame when one is
/// pending), and how many consecutive silent windows are tolerated
/// before the peer is declared gone (60 s total). SSH delivers
/// reliably, so this only fires when the remote lrzsz process died or
/// never really engaged; erroring out releases the diverted pane
/// instead of parking it until disconnect.
const RECV_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
const RECV_SILENT_WINDOWS: u32 = 6;

/// Suffix for in-flight downloads. The finished file only appears
/// under its real name once complete (FileDone renames it), and a
/// leftover part file after a crash, cancel or disconnect is the
/// anchor the next transfer of the same file resumes from.
pub const PART_SUFFIX: &str = ".oryxis-part";

/// Where a transfer's bytes come from / go to. Chosen by the caller up
/// front (see the no-blocking-dialogs note above).
#[derive(Debug, Clone)]
pub enum TransferSpec {
    /// Download: save each incoming file into this directory under its
    /// advertised (sanitized) name.
    Download { dest_dir: PathBuf },
    /// Upload: send these local files, in order, in one session.
    Upload { sources: Vec<PathBuf> },
}

/// Progress and terminal outcome, streamed to the app.
#[derive(Debug, Clone)]
pub enum Progress {
    /// A file began; `size` is the advertised total when known.
    /// `batch` is `(k, n)` on a multi-file upload ("file k of n");
    /// `None` for single files and for downloads (a ZMODEM sender
    /// never announces how many files follow).
    Started {
        name: String,
        size: Option<u64>,
        batch: Option<(usize, usize)>,
    },
    /// Cumulative bytes moved for the current file.
    Advanced { transferred: u64, total: Option<u64> },
    /// One file finished; `path` is where a download landed.
    FileDone { name: String, path: Option<PathBuf> },
    /// The whole session finished successfully. `trailing` carries any
    /// post-protocol bytes captured while confirming the peer's final
    /// "OO" (e.g. a fast shell prompt coalesced into the same read);
    /// they belong to the terminal, not the transfer.
    Completed { trailing: Vec<u8> },
    /// The session was cancelled (by us or the peer).
    Aborted,
    /// The transfer failed; the string is a human-readable reason.
    Error(String),
}

/// Handles the app passes in to drive one transfer.
pub struct TransferIo {
    /// Bytes diverted from the terminal (the detector's `wire` first,
    /// then every subsequent pane output batch). An EMPTY chunk is a
    /// pure wake-up carrying no wire bytes: the cancel path sends one
    /// so a driver blocked on a silent peer's `recv` notices the abort
    /// flag. Closing it (drop) is treated as a disconnect and aborts
    /// the transfer.
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
        Ok(Outcome::Completed(trailing)) => Progress::Completed { trailing },
        Ok(Outcome::Aborted) => Progress::Aborted,
        Err(e) => {
            // A local failure (disk, protocol) strands the peer
            // mid-session; without a cancel it holds the tty until its
            // own timeouts expire, the same freeze an unanswered ZFIN
            // causes. Release it now; if the wire is already gone the
            // send fails harmlessly.
            let _ = io.wire_out.send(crate::CANCEL.to_vec());
            Progress::Error(e)
        }
    };
    let _ = io.progress.send(terminal);
}

enum Outcome {
    /// Success; carries post-protocol bytes for the terminal (see
    /// [`Progress::Completed`]).
    Completed(Vec<u8>),
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
    // Advertise nonstop I/O (zero buffer length) plus CANOVIO: the pane
    // transport is SSH/Telnet/serial with its own flow control, and the
    // driver drains wire into the (buffered) file as fast as it arrives,
    // so there is no reason to make `sz` stop for a ZACK round trip
    // every buffer's worth of data; on links with real latency that
    // pacing, not bandwidth, dominates the transfer time.
    let mut receiver =
        Receiver::with_flow_control(0, true).map_err(|e| format!("zmodem init: {e:?}"))?;
    // Each announced file is answered manually so a leftover part file
    // can be resumed from its offset instead of always ZRPOS(0).
    receiver.set_manual_file_accept(true);
    let mut pending = first_wire;
    let mut dest: Option<BufWriter<tokio::fs::File>> = None;
    let mut dest_path: Option<PathBuf> = None;
    let mut name = String::new();
    let mut transferred: u64 = 0;
    let mut total: Option<u64> = None;
    let mut aborting = false;
    let mut finished = false;
    // Consecutive RECV_TIMEOUT windows with no wire bytes; reset on any
    // data. Bounds how long a vanished peer can park the receive loop.
    let mut silent_windows: u32 = 0;
    let mut last_progress = std::time::Instant::now();

    loop {
        if !aborting && io.abort.load(Ordering::Relaxed) {
            aborting = true;
            // `zmodem2`'s abort() only queues the Aborted event; the
            // cancel bytes the peer needs are ours to send.
            let _ = io.wire_out.send(crate::CANCEL.to_vec());
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
                // A correct sender writes exactly the size it announced in
                // ZFILE, so a stream that runs past it is a protocol
                // violation, not a valid transfer. Reject it when the size
                // is known: this bounds an auto-armed download against a
                // hostile host trying to overrun the disk, and keeps the
                // fork's u32 file position from wrapping past 4 GiB. An
                // unannounced size (`total == None`) has no ceiling to
                // check here.
                if let Some(t) = total
                    && t > 0
                    && transferred.saturating_add(n as u64) > t
                {
                    return Err(format!(
                        "sender exceeded the advertised size ({t} bytes); aborting"
                    ));
                }
                if let Some(file) = dest.as_mut() {
                    file.write_all(&bytes)
                        .await
                        .map_err(|e| format!("write: {e}"))?;
                }
                transferred += n as u64;
                receiver
                    .file_written(n)
                    .map_err(|e| format!("zmodem file: {e:?}"))?;
                if last_progress.elapsed() >= PROGRESS_INTERVAL {
                    last_progress = std::time::Instant::now();
                    let _ = io.progress.send(Progress::Advanced { transferred, total });
                }
            }
            Step::ReadFile { .. } => {
                return Err("receiver asked to read a file (protocol error)".into());
            }
            Step::Started { name: raw, size } => {
                let safe = sanitize_name(&raw);
                let part = dest_dir.join(format!("{safe}{PART_SUFFIX}"));
                // Resume decision, `rz -r` semantics (length based): an
                // existing part no larger than the advertised total
                // continues where it left off; len == total still goes
                // through the resume path so a crash between the last
                // byte and the rename heals (sz answers ZRPOS(total)
                // with an immediate ZEOF and the rename runs below).
                let advertised = size.unwrap_or(0);
                // `symlink_metadata` does NOT follow a link, so a hostile
                // pre-planted `<name>.oryxis-part` symlink (pointing at
                // ~/.bashrc, say) is seen as a symlink and never resumed
                // THROUGH: only a real file we wrote reaches the append
                // path below. Anything else at that path (symlink, dir) is
                // stale or hostile, so drop it and restart fresh under the
                // create_new (O_EXCL) path.
                let existing = match tokio::fs::symlink_metadata(&part).await {
                    Ok(m) if m.file_type().is_file() => Some(m.len()),
                    Ok(_) => {
                        let _ = tokio::fs::remove_file(&part).await;
                        None
                    }
                    Err(_) => None,
                };
                let resume_at = match existing {
                    Some(len) if advertised > 0 && len > 0 && len <= advertised => len,
                    _ => 0,
                };
                let resume_at32 = u32::try_from(resume_at)
                    .map_err(|_| "partial larger than 4 GiB (ZMODEM limit)".to_string())?;
                let file = if resume_at > 0 {
                    // Resuming an in-flight part of the same name is
                    // indistinguishable from resuming a dead one
                    // without OS file locks (the `rz -r` trade-off);
                    // fresh parts below stay create_new-protected.
                    tokio::fs::OpenOptions::new()
                        .append(true)
                        .open(&part)
                        .await
                        .map_err(|e| format!("open {}: {e}", part.display()))?
                } else {
                    match tokio::fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(&part)
                        .await
                    {
                        Ok(file) => file,
                        // A stale part (larger than the advertised
                        // size, or a zero-byte leftover) restarts.
                        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                            tokio::fs::OpenOptions::new()
                                .write(true)
                                .truncate(true)
                                .open(&part)
                                .await
                                .map_err(|e| format!("open {}: {e}", part.display()))?
                        }
                        Err(e) => return Err(format!("create {}: {e}", part.display())),
                    }
                };
                dest = Some(BufWriter::with_capacity(FILE_BUF_SIZE, file));
                name = safe;
                dest_path = Some(part);
                total = size;
                transferred = resume_at;
                // The acceptance answer (ZRPOS) can only be queued into
                // an empty outgoing buffer; flush whatever is pending
                // (e.g. the between-files ZACK volley) first.
                loop {
                    match decode(receiver.poll()) {
                        Step::WriteWire(bytes) => {
                            let n = bytes.len();
                            let _ = io.wire_out.send(bytes);
                            receiver.wire_written(n);
                        }
                        Step::Idle => break,
                        _ => return Err("unexpected step while accepting a file".into()),
                    }
                }
                receiver
                    .accept_file_at(resume_at32)
                    .map_err(|e| format!("zmodem accept: {e:?}"))?;
                let _ = io.progress.send(Progress::Started {
                    name: name.clone(),
                    size,
                    batch: None,
                });
                if resume_at > 0 {
                    // Let the overlay start at the resumed percentage
                    // instead of zero.
                    let _ = io.progress.send(Progress::Advanced { transferred, total });
                }
            }
            Step::FileDone => {
                if let Some(mut file) = dest.take() {
                    file.flush().await.map_err(|e| format!("flush: {e}"))?;
                }
                // The finished part surfaces under its real name only
                // now; a collision gets the browser-style " (N)".
                let final_path = match dest_path.take() {
                    Some(part) => Some(finalize_download(&part, &dest_dir, &name).await?),
                    None => None,
                };
                if let Some(path) = final_path.as_ref()
                    && let Some(on_disk) = path.file_name()
                {
                    name = on_disk.to_string_lossy().into_owned();
                }
                // Streamed reports are time-strided; snap the overlay
                // to the exact figure before closing the file out.
                let _ = io.progress.send(Progress::Advanced { transferred, total });
                let _ = io.progress.send(Progress::FileDone {
                    name: name.clone(),
                    path: final_path,
                });
            }
            Step::Completed => {
                // The machine queued its ZFIN reply BEHIND this event
                // (`poll` yields events before wire bytes); returning
                // now would strand the reply and leave the remote `sz`
                // retrying ZFIN for ~20 s (issue #77). Keep pumping so
                // the reply drains, then leave through `Idle`.
                finished = true;
            }
            Step::Aborted => return Ok(Outcome::Aborted),
            Step::Idle => {
                if finished {
                    // Idle after the completion event: the ZFIN reply
                    // is on the wire. The peer answers it with "OO"
                    // and only then frees its tty; absorb that so it
                    // doesn't print as stray text at the next prompt.
                    let trailing =
                        swallow_over_and_out(&mut io.wire_in, std::mem::take(&mut pending)).await;
                    return Ok(Outcome::Completed(trailing));
                }
                if pending.is_empty() {
                    // Once the cancel went out, a torn-down peer may
                    // never speak again; exit instead of parking on
                    // wire that will never come. (Normally the Aborted
                    // event above ends the loop first; this is the
                    // backstop.)
                    if aborting {
                        return Ok(Outcome::Aborted);
                    }
                    match tokio::time::timeout(RECV_TIMEOUT, io.wire_in.recv()).await {
                        Ok(Some(bytes)) => {
                            silent_windows = 0;
                            pending = bytes;
                        }
                        Ok(None) => return Err("connection closed during transfer".into()),
                        Err(_) => {
                            silent_windows += 1;
                            if silent_windows >= RECV_SILENT_WINDOWS {
                                return Err("peer stopped responding".into());
                            }
                            // Re-queue the handshake volley if one is
                            // pending; the next loop turn flushes it.
                            receiver
                                .timeout()
                                .map_err(|e| format!("zmodem timeout: {e:?}"))?;
                            continue;
                        }
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

/// After a download's ZFIN reply, the remote `sz` sends the two-byte
/// "OO" ("over and out") sign-off and exits. Those bytes are protocol,
/// not terminal output; wait briefly for them so they don't render.
/// Anything received beyond them (a fast shell prompt coalesced into
/// the same read) is returned for the app to hand to the emulator. A
/// peer that never signs off (killed, non-lrzsz) just runs the wait
/// out; nothing else can arrive in that window since the pane is still
/// diverted here.
async fn swallow_over_and_out(
    wire_in: &mut mpsc::UnboundedReceiver<Vec<u8>>,
    mut pending: Vec<u8>,
) -> Vec<u8> {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(1000);
    let mut seen = 0u8;
    loop {
        let chunk = if pending.is_empty() {
            match tokio::time::timeout_at(deadline, wire_in.recv()).await {
                Ok(Some(chunk)) => chunk,
                // Timeout or disconnect: no sign-off is coming.
                _ => return Vec::new(),
            }
        } else {
            std::mem::take(&mut pending)
        };
        for (i, &b) in chunk.iter().enumerate() {
            match b {
                b'O' if seen < 2 => seen += 1,
                // Framing residue ahead of the "OO": flow control and
                // the ZHEX line terminator, whose CR / LF / XON may
                // arrive with the high bit set (lrzsz ends hex headers
                // with 0x0d 0x8a 0x11).
                0x11 | 0x13 | b'\r' | b'\n' | 0x8d | 0x8a | 0x91 if seen == 0 => {}
                // First byte past the sign-off (or something that is
                // not one): the terminal's from here on.
                _ => return chunk[i..].to_vec(),
            }
        }
        if seen == 2 {
            return Vec::new();
        }
    }
}

/// One upload source: a buffered reader plus its bookkeeping. The
/// read-ahead buffer matters because the machine requests sequential
/// ~1 KiB slices; seeking per request would defeat it and pay two
/// blocking-pool dispatches per subpacket, so a real seek only happens
/// on a retransmission rewind or a receiver-driven resume offset.
struct UploadFile {
    reader: BufReader<tokio::fs::File>,
    pos: u64,
    size: u64,
    size32: u32,
    name: String,
}

impl UploadFile {
    async fn open(path: &std::path::Path) -> Result<Self, String> {
        let file = tokio::fs::File::open(path)
            .await
            .map_err(|e| format!("open {}: {e}", path.display()))?;
        let size = file.metadata().await.map_err(|e| format!("stat: {e}"))?.len();
        let size32 = u32::try_from(size)
            .map_err(|_| format!("{}: larger than 4 GiB (ZMODEM limit)", path.display()))?;
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "file".into());
        Ok(Self {
            reader: BufReader::with_capacity(FILE_BUF_SIZE, file),
            pos: 0,
            size,
            size32,
            name,
        })
    }
}

async fn run_upload(
    spec: TransferSpec,
    first_wire: Vec<u8>,
    io: &mut TransferIo,
) -> Result<Outcome, String> {
    let TransferSpec::Upload { sources } = spec else {
        return Err("upload driver given a non-upload spec".into());
    };
    if sources.is_empty() {
        return Err("no files to send".into());
    }
    let total_files = sources.len();
    let mut queue = sources.into_iter().enumerate();
    // `(k, n)` shown by the overlay; suppressed for single files.
    let batch_of = move |index: usize| (total_files > 1).then_some((index + 1, total_files));

    let mut sender = Sender::new().map_err(|e| format!("zmodem init: {e:?}"))?;
    // When the remote `rz` allows streaming (lrzsz advertises a zero
    // buffer with CANOVIO), widen the default 10-subpacket window to
    // 1024 (1 MiB between ZACK waits). Not usize::MAX: the wire-out
    // channel is unbounded with no backpressure signal from the
    // transport, so the periodic acknowledgement is what keeps at most
    // ~1 MiB of file data in flight in memory on a slow link.
    sender.set_streaming_window(1024);

    let (index, first) = queue.next().expect("sources checked non-empty");
    let mut current = UploadFile::open(&first).await?;
    sender
        .start_file(FileInfo::new(
            current.name.as_bytes(),
            Some(Position::new(current.size32)),
        ))
        .map_err(|e| format!("zmodem start_file: {e:?}"))?;
    // Requesting the session end when the LAST file starts makes the
    // machine queue ZFIN right after that file completes; earlier
    // files roll straight into the next ZFILE instead.
    if index + 1 == total_files {
        sender.finish().map_err(|e| format!("zmodem finish: {e:?}"))?;
    }
    let _ = io.progress.send(Progress::Started {
        name: current.name.clone(),
        size: Some(current.size),
        batch: batch_of(index),
    });

    let mut pending = first_wire;
    let mut aborting = false;
    let mut finished = false;
    let mut last_progress = std::time::Instant::now();
    let mut silent_windows: u32 = 0;

    loop {
        if !aborting && io.abort.load(Ordering::Relaxed) {
            aborting = true;
            // Same as the download side: the cancel bytes are ours to
            // send, abort() only queues the Aborted event.
            let _ = io.wire_out.send(crate::CANCEL.to_vec());
            sender.abort();
        }
        match decode(sender.poll()) {
            Step::WriteWire(bytes) => {
                let n = bytes.len();
                let _ = io.wire_out.send(bytes);
                sender.wire_written(n);
            }
            Step::ReadFile { offset, max_len } => {
                let offset = u64::from(offset);
                if offset != current.pos {
                    current
                        .reader
                        .seek(std::io::SeekFrom::Start(offset))
                        .await
                        .map_err(|e| format!("seek: {e}"))?;
                }
                let mut buf = vec![0u8; max_len];
                let read = current
                    .reader
                    .read(&mut buf)
                    .await
                    .map_err(|e| format!("read: {e}"))?;
                buf.truncate(read);
                if read == 0 {
                    return Err("unexpected end of file during upload".into());
                }
                current.pos = offset + read as u64;
                sender
                    .submit_file(&buf)
                    .map_err(|e| format!("zmodem submit: {e:?}"))?;
                if last_progress.elapsed() >= PROGRESS_INTERVAL {
                    last_progress = std::time::Instant::now();
                    let _ = io.progress.send(Progress::Advanced {
                        transferred: current.pos,
                        total: Some(current.size),
                    });
                }
            }
            Step::WriteFile(_) => {
                return Err("sender asked to write a file (protocol error)".into());
            }
            Step::Started { .. } => {}
            Step::FileDone => {
                // Same exact-figure snap as the download side.
                let _ = io.progress.send(Progress::Advanced {
                    transferred: current.size,
                    total: Some(current.size),
                });
                let _ = io.progress.send(Progress::FileDone {
                    name: current.name.clone(),
                    path: None,
                });
                if let Some((index, path)) = queue.next() {
                    current = UploadFile::open(&path).await?;
                    // start_file queues the next ZFILE and needs the
                    // outgoing buffer empty; flush leftovers first.
                    loop {
                        match decode(sender.poll()) {
                            Step::WriteWire(bytes) => {
                                let n = bytes.len();
                                let _ = io.wire_out.send(bytes);
                                sender.wire_written(n);
                            }
                            Step::Idle => break,
                            _ => return Err("unexpected step between files".into()),
                        }
                    }
                    sender
                        .start_file(FileInfo::new(
                            current.name.as_bytes(),
                            Some(Position::new(current.size32)),
                        ))
                        .map_err(|e| format!("zmodem start_file: {e:?}"))?;
                    if index + 1 == total_files {
                        sender.finish().map_err(|e| format!("zmodem finish: {e:?}"))?;
                    }
                    last_progress = std::time::Instant::now();
                    let _ = io.progress.send(Progress::Started {
                        name: current.name.clone(),
                        size: Some(current.size),
                        batch: batch_of(index),
                    });
                }
            }
            Step::Completed => {
                // Mirror of the download side: the "OO" sign-off the
                // remote `rz` may be reading is queued behind this
                // event. Flush it before leaving.
                finished = true;
            }
            Step::Aborted => return Ok(Outcome::Aborted),
            Step::Idle => {
                if finished {
                    // Sign-off flushed; the receiver sends nothing
                    // after its ZFIN, so there is nothing to absorb.
                    return Ok(Outcome::Completed(Vec::new()));
                }
                if pending.is_empty() {
                    // Backstop, same as the download side: never park
                    // on a peer we just cancelled.
                    if aborting {
                        return Ok(Outcome::Aborted);
                    }
                    match tokio::time::timeout(RECV_TIMEOUT, io.wire_in.recv()).await {
                        Ok(Some(bytes)) => {
                            silent_windows = 0;
                            pending = bytes;
                        }
                        Ok(None) => return Err("connection closed during transfer".into()),
                        Err(_) => {
                            silent_windows += 1;
                            if silent_windows >= RECV_SILENT_WINDOWS {
                                return Err("peer stopped responding".into());
                            }
                            // Re-queue the handshake volley if one is
                            // pending; the next loop turn flushes it.
                            sender
                                .timeout()
                                .map_err(|e| format!("zmodem timeout: {e:?}"))?;
                            continue;
                        }
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

/// Move a completed part file to its final name without clobbering
/// anything already on disk: try the advertised name first, then
/// browser-style `name (N).ext` candidates. Each candidate is reserved
/// with `create_new` (an empty marker, so the existence check and the
/// claim are one atomic step) and the finished download atomically
/// replaces its own marker via rename, so a remote-controlled name can
/// never truncate an existing local file.
async fn finalize_download(
    part: &std::path::Path,
    dest_dir: &std::path::Path,
    name: &str,
) -> Result<PathBuf, String> {
    // Hard bound so a pathological directory cannot spin forever;
    // hitting it reports honestly instead of overwriting.
    for attempt in 0..10_000u32 {
        let candidate = if attempt == 0 {
            name.to_string()
        } else {
            numbered_name(name, attempt)
        };
        let path = dest_dir.join(candidate);
        match tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .await
        {
            Ok(_) => {
                tokio::fs::rename(part, &path)
                    .await
                    .map_err(|e| format!("rename {}: {e}", path.display()))?;
                return Ok(path);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(format!("create {}: {e}", path.display())),
        }
    }
    Err(format!(
        "finalize {}: too many name collisions",
        dest_dir.join(name).display()
    ))
}

/// `report.pdf` + 2 -> `report (2).pdf`; an extensionless name gets the
/// counter at the end (`README (2)`).
fn numbered_name(name: &str, n: u32) -> String {
    match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => format!("{stem} ({n}).{ext}"),
        _ => format!("{name} ({n})"),
    }
}

/// Strip a sender-advertised name down to a safe basename: no directory
/// components, no `..`, no leading dots that could hide the file, and
/// no Windows reserved device names. A hostile or careless peer must
/// not be able to write outside the chosen download folder.
fn sanitize_name(raw: &[u8]) -> String {
    let lossy = String::from_utf8_lossy(raw);
    let base = lossy
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or("")
        .trim_matches(|c: char| c == '.' || c.is_whitespace() || c.is_control());
    if base.is_empty() || base == ".." {
        return "received.bin".to_string();
    }
    // Neutralize Windows-reserved punctuation. A colon is the dangerous
    // one: `PathBuf::push`/`join` treats a `C:evil.exe` component as a
    // drive-relative path and REPLACES the whole download path, and
    // `name:stream` opens an NTFS alternate data stream. The rest
    // (`<>"|?*`) can never appear in a real Windows name; a peer sending
    // them is hostile or careless. Replace rather than reject so the
    // transfer still lands somewhere safe under the chosen folder.
    let base: String = base
        .chars()
        .map(|c| match c {
            ':' | '<' | '>' | '"' | '|' | '?' | '*' => '_',
            other => other,
        })
        .collect();
    // Windows matches device names on the part before the first dot
    // ("CON.txt" still opens the console). Defused on every platform so
    // a download folder that later syncs to a Windows machine stays
    // portable.
    let stem = base.split('.').next().unwrap_or(&base);
    if is_windows_reserved(stem) {
        format!("_{base}")
    } else {
        base
    }
}

/// Windows reserved device names: CON, PRN, AUX, NUL, COM1-9, LPT1-9
/// (case-insensitive; COM0/LPT0 and two-digit ports are NOT reserved).
fn is_windows_reserved(stem: &str) -> bool {
    let stem = stem.trim_end();
    if ["CON", "PRN", "AUX", "NUL"]
        .iter()
        .any(|r| stem.eq_ignore_ascii_case(r))
    {
        return true;
    }
    let bytes = stem.as_bytes();
    bytes.len() == 4
        && (bytes[..3].eq_ignore_ascii_case(b"COM") || bytes[..3].eq_ignore_ascii_case(b"LPT"))
        && (b'1'..=b'9').contains(&bytes[3])
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

    #[test]
    fn sanitize_neutralizes_windows_drive_and_ads() {
        // `C:evil.exe` is a drive-relative component: on Windows
        // `dest.join(it)` would drop the download dir entirely. The
        // colon must not survive.
        assert_eq!(sanitize_name(b"C:evil.exe"), "C_evil.exe");
        assert_eq!(sanitize_name(b"report.pdf:hidden"), "report.pdf_hidden");
        assert_eq!(sanitize_name(b"a<b>c|d?e*f\"g"), "a_b_c_d_e_f_g");
        // The colon collapses into the stem, so `CON:x` becomes the
        // harmless `CON_x` (no longer the bare reserved device name).
        assert_eq!(sanitize_name(b"CON:x"), "CON_x");
    }

    #[test]
    fn sanitize_defuses_windows_reserved_device_names() {
        // Reserved with or without an extension, any case.
        assert_eq!(sanitize_name(b"CON"), "_CON");
        assert_eq!(sanitize_name(b"con.txt"), "_con.txt");
        assert_eq!(sanitize_name(b"NUL.tar.gz"), "_NUL.tar.gz");
        assert_eq!(sanitize_name(b"Prn"), "_Prn");
        assert_eq!(sanitize_name(b"aux.log"), "_aux.log");
        assert_eq!(sanitize_name(b"COM1"), "_COM1");
        assert_eq!(sanitize_name(b"lpt9.bin"), "_lpt9.bin");
        // Near misses stay untouched.
        assert_eq!(sanitize_name(b"COM0"), "COM0");
        assert_eq!(sanitize_name(b"COM10"), "COM10");
        assert_eq!(sanitize_name(b"CONSOLE.txt"), "CONSOLE.txt");
        assert_eq!(sanitize_name(b"conf.ig"), "conf.ig");
    }

    #[test]
    fn numbered_name_splits_on_the_last_extension() {
        assert_eq!(numbered_name("report.pdf", 1), "report (1).pdf");
        assert_eq!(numbered_name("archive.tar.gz", 3), "archive.tar (3).gz");
        assert_eq!(numbered_name("README", 2), "README (2)");
    }

    /// A remote-controlled file name must never truncate an existing
    /// local file: finalizing a completed part against a taken name
    /// lands on a browser-style " (N)" suffix, and the original bytes
    /// survive untouched.
    #[test]
    fn download_never_clobbers_an_existing_file() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let dir = std::env::temp_dir().join(format!("oryxis-zm-uniq-{}", std::process::id()));
            let _ = tokio::fs::remove_dir_all(&dir).await;
            tokio::fs::create_dir_all(&dir).await.unwrap();
            tokio::fs::write(dir.join("report.pdf"), b"original").await.unwrap();

            let part1 = dir.join(format!("report.pdf{PART_SUFFIX}"));
            tokio::fs::write(&part1, b"fresh download").await.unwrap();
            let p1 = finalize_download(&part1, &dir, "report.pdf").await.unwrap();
            assert_eq!(p1.file_name().unwrap().to_str().unwrap(), "report (1).pdf");
            assert_eq!(tokio::fs::read(&p1).await.unwrap(), b"fresh download");
            assert!(!tokio::fs::try_exists(&part1).await.unwrap(), "part left behind");
            // A second same-name download finalizes into its own file.
            let part2 = dir.join(format!("report.pdf{PART_SUFFIX}"));
            tokio::fs::write(&part2, b"second").await.unwrap();
            let p2 = finalize_download(&part2, &dir, "report.pdf").await.unwrap();
            assert_eq!(p2.file_name().unwrap().to_str().unwrap(), "report (2).pdf");
            // The pre-existing file was not truncated.
            let orig = tokio::fs::read(dir.join("report.pdf")).await.unwrap();
            assert_eq!(orig, b"original".to_vec());

            // A fresh name takes the advertised name directly.
            let part3 = dir.join(format!("clean.txt{PART_SUFFIX}"));
            tokio::fs::write(&part3, b"clean").await.unwrap();
            let p3 = finalize_download(&part3, &dir, "clean.txt").await.unwrap();
            assert_eq!(p3.file_name().unwrap().to_str().unwrap(), "clean.txt");

            let _ = tokio::fs::remove_dir_all(&dir).await;
        });
    }

    /// A cancel must take effect while the driver is parked on
    /// `wire_in.recv()` waiting for a silent peer: the flag plus the
    /// empty wake-up chunk end the transfer with `Aborted` after the
    /// CANCEL sequence went out, instead of hanging until disconnect.
    #[test]
    fn cancel_wakes_a_driver_blocked_on_a_silent_peer() {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let dir = std::env::temp_dir().join(format!("oryxis-zm-abort-{}", std::process::id()));
            let _ = tokio::fs::create_dir_all(&dir).await;

            let (wire_tx, wire_in) = mpsc::unbounded_channel();
            let (wire_out_tx, mut wire_out_rx) = mpsc::unbounded_channel();
            let (p_tx, mut p_rx) = mpsc::unbounded_channel();
            let abort = Arc::new(AtomicBool::new(false));

            let task = tokio::spawn(run(
                Direction::Download,
                TransferSpec::Download {
                    dest_dir: dir.clone(),
                },
                Vec::new(),
                TransferIo {
                    wire_in,
                    wire_out: wire_out_tx,
                    progress: p_tx,
                    abort: abort.clone(),
                },
            ));
            // Let the receiver flush its opening wire and park on recv
            // (the peer never answers).
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;

            // Cancel: raise the flag, then wake the parked recv with an
            // empty chunk, exactly what the app's ZmodemCancel does.
            abort.store(true, Ordering::Relaxed);
            wire_tx.send(Vec::new()).unwrap();

            tokio::time::timeout(std::time::Duration::from_secs(5), task)
                .await
                .expect("cancel did not wake the blocked driver")
                .unwrap();

            // The terminal progress is Aborted (this is what releases
            // the pane's divert in the app).
            let mut aborted = false;
            while let Ok(p) = p_rx.try_recv() {
                if matches!(p, Progress::Aborted) {
                    aborted = true;
                }
            }
            assert!(aborted, "driver ended without a terminal Aborted");
            // And the peer got the canonical cancel bytes so its `sz`
            // exits instead of waiting out its timeout.
            let mut sent = Vec::new();
            while let Ok(b) = wire_out_rx.try_recv() {
                sent.extend_from_slice(&b);
            }
            assert!(
                sent.windows(crate::CANCEL.len()).any(|w| w == crate::CANCEL),
                "no CANCEL sequence on the wire after abort"
            );

            let _ = tokio::fs::remove_dir_all(&dir).await;
        });
    }

    /// The post-completion sign-off eater: consumes exactly the "OO"
    /// (plus flow-control padding before it), hands everything after
    /// it back for the terminal, and gives up quietly on a peer that
    /// never signs off.
    #[test]
    fn swallow_over_and_out_eats_the_sign_off_and_keeps_the_prompt() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            // Sign-off alone, seeded via pending: nothing trails.
            let (_tx, mut rx) = mpsc::unbounded_channel();
            assert_eq!(swallow_over_and_out(&mut rx, b"OO".to_vec()).await, b"");

            // Padding before, prompt coalesced after: prompt survives.
            let (_tx, mut rx) = mpsc::unbounded_channel();
            let trailing = swallow_over_and_out(&mut rx, b"\r\nOOuser@host$ ".to_vec()).await;
            assert_eq!(trailing, b"user@host$ ");

            // Sign-off split across two wire chunks.
            let (tx, mut rx) = mpsc::unbounded_channel();
            tx.send(b"O".to_vec()).unwrap();
            tx.send(b"O$ ".to_vec()).unwrap();
            assert_eq!(swallow_over_and_out(&mut rx, Vec::new()).await, b"$ ");

            // No sign-off at all: whatever came belongs to the terminal.
            let (_tx, mut rx) = mpsc::unbounded_channel();
            let trailing = swallow_over_and_out(&mut rx, b"logout\r\n".to_vec()).await;
            assert_eq!(trailing, b"logout\r\n");

            // Silent peer: the wait runs out empty-handed (channel
            // closed, so this returns immediately, not after 1 s).
            let (tx, mut rx) = mpsc::unbounded_channel::<Vec<u8>>();
            drop(tx);
            assert_eq!(swallow_over_and_out(&mut rx, Vec::new()).await, b"");
        });
    }

    /// A peer that never says anything (remote lrzsz killed between
    /// the initiation header and the handshake) must not park the
    /// divert forever: the driver re-sends its handshake volley each
    /// silent window and gives up with a clear error after the bounded
    /// total, releasing the pane. Runs on tokio's paused clock, so the
    /// 60 s of windows cost no real time.
    #[test]
    fn silent_peer_is_bounded_by_handshake_retries() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .start_paused(true)
            .build()
            .unwrap();
        rt.block_on(async {
            let dir = std::env::temp_dir().join(format!("oryxis-zm-silent-{}", std::process::id()));
            let _ = tokio::fs::create_dir_all(&dir).await;

            // Keep wire_tx alive: the peer is silent, not disconnected.
            let (_wire_tx, wire_in) = mpsc::unbounded_channel::<Vec<u8>>();
            let (wire_out_tx, mut wire_out_rx) = mpsc::unbounded_channel();
            let (p_tx, mut p_rx) = mpsc::unbounded_channel();

            run(
                Direction::Download,
                TransferSpec::Download { dest_dir: dir.clone() },
                Vec::new(),
                TransferIo {
                    wire_in,
                    wire_out: wire_out_tx,
                    progress: p_tx,
                    abort: Arc::new(AtomicBool::new(false)),
                },
            )
            .await;

            let mut error = None;
            while let Ok(p) = p_rx.try_recv() {
                if let Progress::Error(e) = p {
                    error = Some(e);
                }
            }
            let error = error.expect("driver ended without a terminal Error");
            assert!(
                error.contains("peer stopped responding"),
                "unexpected error: {error}"
            );
            // The handshake was actually retried on the wire, not just
            // waited out: initial ZRINIT plus one volley per silent
            // window (the last window errors out instead of poking).
            let mut volleys = 0;
            while wire_out_rx.try_recv().is_ok() {
                volleys += 1;
            }
            assert!(volleys >= 3, "expected retry volleys, got {volleys}");

            let _ = tokio::fs::remove_dir_all(&dir).await;
        });
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
            let (p_up_tx, mut p_up_rx) = mpsc::unbounded_channel();
            let (p_down_tx, mut p_down_rx) = mpsc::unbounded_channel();
            let abort = Arc::new(AtomicBool::new(false));

            let up = tokio::spawn(run(
                Direction::Upload,
                TransferSpec::Upload { sources: vec![src.clone()] },
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

            // The download side must have reported Completed, and the
            // last Advanced must be the exact total: streamed reports
            // are time-throttled, so the per-file final snap is what
            // guarantees the overlay never ends short.
            let mut completed = false;
            let mut saved: Option<PathBuf> = None;
            let mut last_advanced = None;
            while let Ok(p) = p_down_rx.try_recv() {
                match p {
                    Progress::Completed { trailing } => {
                        completed = true;
                        assert!(
                            trailing.is_empty(),
                            "loopback produced trailing bytes: {trailing:?}"
                        );
                    }
                    Progress::Advanced { transferred, .. } => last_advanced = Some(transferred),
                    Progress::FileDone { path, .. } => saved = path,
                    Progress::Error(e) => panic!("download error: {e}"),
                    _ => {}
                }
            }
            assert!(completed, "download never completed");
            assert_eq!(
                last_advanced,
                Some(payload.len() as u64),
                "final progress snap is not the exact total"
            );
            // The upload side must complete too: it only can if the
            // download flushed its queued ZFIN reply before exiting
            // (issue #77's regression; the old driver returned on the
            // completion event and left the reply stranded, so this
            // side ended in "connection closed" instead).
            let mut up_completed = false;
            while let Ok(p) = p_up_rx.try_recv() {
                match p {
                    Progress::Completed { .. } => up_completed = true,
                    Progress::Error(e) => panic!("upload error: {e}"),
                    _ => {}
                }
            }
            assert!(up_completed, "upload never completed (ZFIN reply not flushed?)");
            let saved = saved.expect("no saved path reported");
            let got = tokio::fs::read(&saved).await.unwrap();
            assert_eq!(got, payload, "round-tripped bytes differ");

            let _ = tokio::fs::remove_dir_all(&dir).await;
        });
    }
}
