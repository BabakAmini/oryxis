//! Native Telnet client engine for Oryxis.
//!
//! Mirrors `oryxis-ssh`'s session shape: [`TelnetSession::connect`]
//! returns a session handle plus an unbounded output receiver, and the
//! handle exposes `write` / `resize` / `is_alive` / `close`, so the
//! terminal pane consumes both transports through one surface.
//!
//! Protocol coverage: RFC 854/855 IAC option negotiation with the full
//! RFC 1143 Q method (loop-proof), RFC 1073 NAWS window size, RFC 1091
//! TERMINAL-TYPE, RFC 1572 NEW-ENVIRON (`USER`), plus prompt-driven
//! credential autofill with a once-per-session, time-boxed guard.

pub mod autologin;
pub mod negotiation;
pub mod session;

pub use session::{TelnetConfig, TelnetError, TelnetSession};
