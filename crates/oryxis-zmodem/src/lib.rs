//! In-terminal ZMODEM file transfer for Oryxis.
//!
//! Two pieces of terminal-side glue around the `zmodem2` protocol state
//! machine (which this crate re-exports):
//!
//! - [`ZmodemDetector`]: watches a terminal output stream for a `sz` /
//!   `rz` initiation header and reports the split point where the wire
//!   stream begins (the `OscSniffer` pattern).
//! - the async driver (added alongside) maps `zmodem2`'s poll/submit
//!   [`Action`]s onto the pane's transport (`WriteWire`) and the local
//!   filesystem (`ReadFile` / `WriteFile`), surfacing progress.
//!
//! The engine lives in the core binary, not a plugin: it must sit in
//! the live byte path of the terminal, which a subprocess cannot reach,
//! and `zmodem2` is small enough (no_std / heapless) to bundle freely.

pub mod detector;

pub use detector::{Direction, Scan, ZmodemDetector};

// Re-export the protocol primitives so the app drives one dependency.
pub use zmodem2::{Action, Event, FileInfo, Position, Receiver, Sender};
