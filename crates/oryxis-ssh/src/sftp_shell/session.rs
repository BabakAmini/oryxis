//! The console as a transport: one task, one loop, the pane's surface.
//!
//! [`SftpShellSession`] exposes what every other terminal transport does
//! (write / resize / senders / is_alive / close), so a pane driven by it
//! is an ordinary pane and every generic path in the app works unchanged.
//! Behind that surface is a single task running the read-eval-print loop
//! over [`super::exec`].

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tokio::sync::mpsc;

use crate::engine::{SshError, SshSession};
use crate::sftp::SftpClient;

use super::PROMPT;
use super::editor::{LineEditor, LineEvent};
use super::exec::{self, Outcome, ShellState};
use super::parser;
use super::render::CRLF;

/// A live SFTP console.
///
/// Cheap to clone-by-Arc like the other sessions; the app holds it in
/// `TerminalTransport`.
#[derive(Debug)]
pub struct SftpShellSession {
    /// Keystrokes in. Also what the emulator's in-band query replies
    /// travel down, which is why the line editor's escape decoder has to
    /// swallow whole CSI sequences.
    input_tx: mpsc::UnboundedSender<Vec<u8>>,
    resize_tx: mpsc::UnboundedSender<(u16, u16)>,
    task: tokio::task::JoinHandle<()>,
    /// Latched by [`SftpShellSession::close`] so teardown runs exactly
    /// once even when an explicit close and the `Drop` backstop both
    /// fire.
    closed: AtomicBool,
    /// Set by the REPL task on its way out, BEFORE it drops the output
    /// sender. See [`SftpShellSession::is_alive`].
    repl_done: Arc<AtomicBool>,
    /// The SSH session the console's channel rides. Held for two
    /// reasons: it keeps the link alive for as long as the console
    /// needs it, and it is what `is_alive` consults, since the SFTP
    /// client cannot report the health of the channel underneath it.
    ssh: Arc<SshSession>,
}

impl SftpShellSession {
    /// Start a console over `client`, which must already be open on
    /// `ssh`.
    ///
    /// Returns the session and the byte stream the pane renders. The
    /// caller maps that receiver into its own output message, exactly as
    /// the SSH and local paths do.
    pub fn spawn(
        ssh: Arc<SshSession>,
        client: SftpClient,
        remote_home: String,
        local_cwd: PathBuf,
        cols: u16,
        label: String,
    ) -> (Self, mpsc::UnboundedReceiver<Vec<u8>>) {
        let (input_tx, input_rx) = mpsc::unbounded_channel();
        let (resize_tx, resize_rx) = mpsc::unbounded_channel();
        let (output_tx, output_rx) = mpsc::unbounded_channel();
        let repl_done = Arc::new(AtomicBool::new(false));

        let task = tokio::spawn(
            Repl {
                ssh: Arc::clone(&ssh),
                client,
                state: ShellState::new(remote_home, local_cwd, cols),
                input_rx,
                resize_rx,
                output_tx,
                repl_done: Arc::clone(&repl_done),
                label,
            }
            .run(),
        );

        (
            Self {
                input_tx,
                resize_tx,
                task,
                closed: AtomicBool::new(false),
                repl_done,
                ssh,
            },
            output_rx,
        )
    }

    pub fn write(&self, data: &[u8]) -> Result<(), SshError> {
        self.input_tx
            .send(data.to_vec())
            .map_err(|_| SshError::Channel("sftp console is closed".into()))
    }

    pub fn resize(&self, cols: u16, rows: u16) {
        let _ = self.resize_tx.send((cols, rows));
    }

    pub fn resize_sender(&self) -> mpsc::UnboundedSender<(u16, u16)> {
        self.resize_tx.clone()
    }

    /// The input sender, which the emulator uses for in-band query
    /// replies (cursor position, DECRQM). Those bytes land in the same
    /// place a keystroke does, and the line editor is what tells them
    /// apart.
    pub fn write_sender(&self) -> mpsc::UnboundedSender<Vec<u8>> {
        self.input_tx.clone()
    }

    /// Whether the console is still usable.
    ///
    /// Four signals, in the shape [`SshSession::is_alive`] establishes,
    /// and for the reason spelled out there at length: the app reads the
    /// end of the output stream as the pane's death notice and asks this
    /// before acting on it, so a session that really died while still
    /// answering "alive" would have its own notice discarded.
    ///
    /// `repl_done` is the one with the guaranteed ORDER. The REPL task
    /// sets it before dropping the output sender, in the same task with
    /// no await in between, which makes "dead before silent" true by
    /// construction rather than by scheduling luck.
    ///
    /// The fourth signal is the SSH session underneath. A console is not
    /// independently alive: its channel rides that link, and the SFTP
    /// client cannot report on it (every failure it can raise is
    /// `SshError::Channel`, a missing file and a dead link alike). So
    /// the question is passed through to the thing that can answer.
    pub fn is_alive(&self) -> bool {
        !self.closed.load(Ordering::SeqCst)
            && !self.repl_done.load(Ordering::SeqCst)
            && !self.task.is_finished()
            && !self.input_tx.is_closed()
            && self.ssh.is_alive()
    }

    /// Tear the console down. Idempotent.
    pub fn close(&self) {
        if self.closed.swap(true, Ordering::SeqCst) {
            return;
        }
        self.task.abort();
        // The SSH session is deliberately NOT closed here. The console
        // may be riding a link a terminal tab also holds (the reuse
        // pool hands out the same transport), and closing it would take
        // that tab's shell down with a console the user merely finished
        // with. Dropping our `Arc`, which happens when this session is
        // dropped, is how the console says it is done with the link.
    }
}

impl Drop for SftpShellSession {
    fn drop(&mut self) {
        self.close();
    }
}

/// The read-eval-print loop.
///
/// Two states, and the difference between them is what makes Ctrl+C work
/// during a four-gigabyte transfer:
///
/// - IDLE: input feeds the line editor, a submitted line becomes a
///   command.
/// - RUNNING: a command's future is in flight and input is NOT fed to
///   the editor. It is scanned for `0x03` and otherwise discarded.
///
/// Feeding the editor while a command runs would collect the keystrokes
/// into a phantom line that appeared, fully typed, the moment the prompt
/// came back. And running the command without also polling input would
/// leave Ctrl+C unread until the transfer finished, which is precisely
/// when nobody needs it any more.
struct Repl {
    ssh: Arc<SshSession>,
    client: SftpClient,
    state: ShellState,
    input_rx: mpsc::UnboundedReceiver<Vec<u8>>,
    resize_rx: mpsc::UnboundedReceiver<(u16, u16)>,
    output_tx: mpsc::UnboundedSender<Vec<u8>>,
    repl_done: Arc<AtomicBool>,
    label: String,
}

impl Repl {
    async fn run(self) {
        let Repl {
            ssh,
            client,
            mut state,
            mut input_rx,
            mut resize_rx,
            output_tx,
            repl_done,
            label,
        } = self;
        let mut editor = LineEditor::new(PROMPT, state.cols);
        let mut out = output_tx.clone();

        // The banner names the host the way `sftp(1)` does, so a screenshot
        // of the console says which machine it is.
        emit(
            &out,
            &format!("Connected to {label}.{CRLF}Type \"help\" for a list of commands.{CRLF}"),
        );
        emit_bytes(&out, &editor.redraw_fresh());

        loop {
            // ---- IDLE ---------------------------------------------------
            let line = tokio::select! {
                chunk = input_rx.recv() => {
                    let Some(chunk) = chunk else { break };
                    let (echo, events) = editor.feed(&chunk);
                    emit_bytes(&out, &echo);
                    let mut submitted = None;
                    for event in events {
                        match event {
                            LineEvent::Submitted(line) => submitted = Some(line),
                            LineEvent::Eof => {
                                emit_bytes(&out, b"\r\n");
                                break;
                            }
                            // Ctrl+C at the prompt: the editor already
                            // painted the `^C` and the fresh prompt, so
                            // there is nothing left to do.
                            LineEvent::Interrupted => {}
                            LineEvent::CompleteRequested(word) => {
                                let bytes = complete(&word, &client, &state, &mut editor).await;
                                emit_bytes(&out, &bytes);
                            }
                        }
                    }
                    match submitted {
                        Some(line) => line,
                        None => continue,
                    }
                }
                Some((cols, rows)) = resize_rx.recv() => {
                    let _ = rows;
                    state.cols = cols.max(1);
                    editor.set_cols(state.cols);
                    // Repaint at the new geometry: the line the user is
                    // typing was drawn against the old width and would
                    // otherwise be wrong until they touched a key.
                    emit_bytes(&out, &editor.redraw());
                    continue;
                }
            };

            let cmd = match parser::parse(&line) {
                Ok(cmd) => cmd,
                Err(parser::ParseError::Empty) => {
                    emit_bytes(&out, &editor.redraw_fresh());
                    continue;
                }
                Err(e) => {
                    emit(&out, &format!("{e}{CRLF}"));
                    emit_bytes(&out, &editor.redraw_fresh());
                    continue;
                }
            };

            // ---- RUNNING ------------------------------------------------
            // A resize arriving mid-command is REMEMBERED, not applied: the
            // running future holds `&mut state`, so there is nothing to
            // assign to until it returns. That restriction happens to be the
            // right behaviour anyway. The command already captured its width
            // and repainting under it would tear a progress meter in half,
            // which is why `sftp(1)` also lets a resize land at the next
            // prompt.
            let mut pending_cols: Option<u16> = None;
            let outcome = {
                let future = exec::run(cmd, &client, &mut state, &mut out);
                tokio::pin!(future);
                loop {
                    tokio::select! {
                        // Biased so a command that has finished is seen as
                        // finished rather than losing a race to a keystroke.
                        biased;
                        done = &mut future => break Some(done),
                        chunk = input_rx.recv() => {
                            let Some(chunk) = chunk else { break None };
                            if chunk.contains(&0x03) {
                                // Dropping the future cancels the transfer at
                                // its next await. The partial file it leaves
                                // is the caller's to sweep; `SftpClient` has
                                // `discard_download_scratch` for exactly this
                                // and the resume machinery is what makes
                                // keeping it worthwhile.
                                break None;
                            }
                            // Everything else typed during a command is
                            // discarded, not buffered. See the doc above.
                        }
                        Some((cols, _rows)) = resize_rx.recv() => {
                            pending_cols = Some(cols.max(1));
                        }
                    }
                }
            };
            if let Some(cols) = pending_cols {
                state.cols = cols;
                editor.set_cols(cols);
            }

            match outcome {
                Some(Outcome::Quit) => break,
                // Cancelled, or the input channel closed mid-command.
                None => {
                    emit(&out, &format!("{CRLF}Interrupted.{CRLF}"));
                }
                Some(Outcome::Continue) => {}
            }

            // The health question is asked here, once per command, rather
            // than inferred from the error: every failure `SftpClient` can
            // raise is `SshError::Channel`, so a missing file and a dead
            // link are the same value. A REPL that kept prompting over a
            // dead channel would be a tab reading "connected" that answers
            // nothing.
            if !ssh.is_alive() {
                emit(&out, &format!("Connection closed.{CRLF}"));
                break;
            }

            emit_bytes(&out, &editor.redraw_fresh());
        }

        // The ordering contract, and the only place it can be honoured:
        // mark dead, THEN drop the sender, in this task, with no await in
        // between. Anything that settles later (a JoinHandle, a channel some
        // other task closes) is a race rather than a guarantee.
        repl_done.store(true, Ordering::SeqCst);
        drop(out);
        drop(output_tx);
    }
}

/// Resolve a Tab completion against the remote directory.
///
/// Returns the bytes to paint. A completion that matches nothing paints
/// nothing, which is what every shell does: a bell or an error for a Tab
/// that found no file would be noise on a key people press speculatively.
async fn complete(
    word: &str,
    client: &SftpClient,
    state: &ShellState,
    editor: &mut LineEditor,
) -> Vec<u8> {
    let (dir, prefix) = match word.rsplit_once('/') {
        Some((dir, prefix)) => (dir.to_string(), prefix.to_string()),
        None => (String::new(), word.to_string()),
    };
    let listing_dir = if dir.is_empty() {
        state.remote_cwd.clone()
    } else if dir.starts_with('/') {
        if dir.is_empty() {
            "/".to_string()
        } else {
            dir.clone()
        }
    } else {
        format!("{}/{}", state.remote_cwd.trim_end_matches('/'), dir)
    };
    let Ok(entries) = client.list_dir(&listing_dir).await else {
        return Vec::new();
    };
    let matches: Vec<&crate::SftpEntry> = entries
        .iter()
        .filter(|e| e.name.starts_with(&prefix))
        // A completion that has to be asked for by name is not a
        // completion: dotfiles only appear once the user typed the dot.
        .filter(|e| prefix.starts_with('.') || !e.name.starts_with('.'))
        .collect();

    let Some(common) = common_prefix(matches.iter().map(|e| e.name.as_str())) else {
        return Vec::new();
    };
    // One candidate completes fully, with the marker that says what it
    // is: a `/` invites the next component, a space ends the word.
    let completed = if matches.len() == 1 {
        let only = matches[0];
        let sep = if only.is_dir { "/" } else { " " };
        format!("{}{}{}", rebuild(&dir, &only.name), "", sep)
    } else {
        rebuild(&dir, &common)
    };
    editor.apply_completion(&completed)
}

/// Put a completed name back under the directory the user had typed, so
/// `get /var/lo<Tab>` becomes `/var/log/` rather than `log/`.
fn rebuild(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        name.to_string()
    } else {
        format!("{dir}/{name}")
    }
}

/// The longest prefix every candidate shares, or `None` when there are
/// no candidates. Compares by CHARACTER, so a shared multi-byte prefix
/// is not cut mid-character.
fn common_prefix<'a>(mut names: impl Iterator<Item = &'a str>) -> Option<String> {
    let first = names.next()?;
    let mut common: Vec<char> = first.chars().collect();
    for name in names {
        let mut shared = 0;
        for (a, b) in common.iter().zip(name.chars()) {
            if *a != b {
                break;
            }
            shared += 1;
        }
        common.truncate(shared);
        if common.is_empty() {
            break;
        }
    }
    Some(common.into_iter().collect())
}

fn emit(out: &mpsc::UnboundedSender<Vec<u8>>, text: &str) {
    let _ = out.send(text.as_bytes().to_vec());
}

fn emit_bytes(out: &mpsc::UnboundedSender<Vec<u8>>, bytes: &[u8]) {
    if bytes.is_empty() {
        return;
    }
    let _ = out.send(bytes.to_vec());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn common_prefix_of_one_is_itself() {
        assert_eq!(
            common_prefix(["access.log"].into_iter()),
            Some("access.log".to_string())
        );
    }

    #[test]
    fn common_prefix_stops_at_the_first_difference() {
        assert_eq!(
            common_prefix(["access.log", "access.log.1", "access.old"].into_iter()),
            Some("access.".to_string())
        );
    }

    #[test]
    fn common_prefix_of_nothing_is_none() {
        assert_eq!(common_prefix(std::iter::empty()), None);
    }

    #[test]
    fn common_prefix_can_be_empty_when_nothing_is_shared() {
        assert_eq!(
            common_prefix(["alpha", "beta"].into_iter()),
            Some(String::new())
        );
    }

    /// Comparing by character rather than by byte is what keeps a shared
    /// multi-byte prefix from being cut in half, which would produce a
    /// completion that is not valid UTF-8 at all.
    #[test]
    fn common_prefix_does_not_cut_a_character_in_half() {
        assert_eq!(
            common_prefix(["文档a", "文档b"].into_iter()),
            Some("文档".to_string())
        );
    }

    #[test]
    fn rebuild_puts_the_name_back_under_its_directory() {
        assert_eq!(rebuild("/var/lo", "log"), "/var/lo/log");
        assert_eq!(rebuild("", "access.log"), "access.log");
    }
}
