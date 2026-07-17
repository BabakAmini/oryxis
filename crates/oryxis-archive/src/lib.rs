//! Archive operations for the SFTP surfaces.
//!
//! Three independent capabilities, all transport-agnostic:
//!
//! - [`remote`]: synthesis of the shell commands that extract / create
//!   archives on the REMOTE host over an SSH exec channel (POSIX and
//!   Windows OpenSSH shells), plus the per-session probe that discovers
//!   which tools (`tar` / `unzip` / `zip` / bsdtar) the host actually
//!   has. Pure string logic, aggressively tested: every path that ends
//!   up inside a shell command goes through [`quote`].
//! - [`browse`]: virtual navigation INSIDE a zip archive without
//!   extracting it. Zip keeps its index (the central directory) at the
//!   end of the file and SFTP supports positioned reads, so listing a
//!   remote zip costs a few KiB of traffic regardless of archive size.
//!   Works over any [`ranged::RangedSource`] (a local file or a bridge
//!   to SFTP ranged reads).
//! - [`local`]: extract / compress on the local pane using pure Rust
//!   (`zip` + `tar` + `flate2`), no shell involved.

pub mod browse;
pub mod local;
pub mod names;
pub mod quote;
pub mod ranged;
pub mod remote;

/// Errors surfaced by archive operations. Stringly inner payloads on
/// purpose: everything here ends up in a user-facing toast / inline
/// error, never matched on programmatically.
#[derive(Debug, thiserror::Error)]
pub enum ArchiveError {
    #[error("{0}")]
    Io(#[from] std::io::Error),
    /// Malformed or unreadable archive.
    #[error("{0}")]
    Malformed(String),
    /// Readable archive but an entry we cannot process (encrypted,
    /// exotic compression method, unsafe path).
    #[error("{0}")]
    Unsupported(String),
    /// A name that cannot be safely placed inside a shell command for
    /// the target platform (see [`quote`]).
    #[error("{0}")]
    UnsafeName(String),
}
