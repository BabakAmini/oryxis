//! mosh for Oryxis: the handover that starts a session, and the
//! session it starts.
//!
//! mosh is not a protocol a host is reached BY, it is one a session is
//! carried ON. Reaching the host is still SSH, and it has to be: the
//! server does not exist until something starts it there, and the port
//! and key it answers with come back over that same SSH channel. So a
//! mosh host in Oryxis is an SSH host with [`ServerCommand`] filled in,
//! not an entry under a protocol of its own, for the same reason
//! Telnet-over-TLS is a toggle on the Telnet form rather than a fourth
//! protocol.
//!
//! What that buys is everything the SSH side already knows: the
//! credentials, the jump chain, the proxy, the host-key policy. mosh's
//! own wrapper gets the same thing by shelling out to `ssh`; here it is
//! the engine that is already open.
//!
//! Two halves, and they are separable on purpose. [`bootstrap`] is pure:
//! it renders the command and reads the answer, so both are testable
//! with no server and no network, and the exec channel that carries
//! them belongs to the caller that already has one. [`session`] takes
//! the port and key and drives the UDP session, publishing bytes on the
//! channel shape every other transport uses.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod bootstrap;
pub mod screen;
pub mod session;

pub use bootstrap::{BootstrapError, Handover, ServerCommand, parse};
pub use screen::AlacrittyScreen;
pub use session::{MoshError, MoshSession};
