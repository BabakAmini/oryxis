//! Oryxis as an ssh-agent: expose the vault's keys to external tools
//! (git, WSL, VS Code Remote, rsync) over the standard ssh-agent
//! protocol, so they authenticate with vault-stored keys without a
//! private key ever touching disk (issue #54).
//!
//! Phase 1 (this module): the wire protocol ([`protocol`]), the key
//! source abstraction ([`source`]) and the unix listener
//! ([`listener`]), all provable against russh's public `AgentClient`.
//! Phase 2 adds the `AgentRuntime` (a dedicated unlocked vault handle,
//! mirroring `sync_runtime`), the Settings toggle, the per-signature
//! confirm modal and the lock wiring. Phase 3 adds the Windows named
//! pipe. Nothing here is mounted yet; the app gains the runtime in
//! Phase 2.
//!
//! Why not russh's `agent::server::serve`: its `Agent` trait has no
//! identity-supply hook (keys live in a private `KeyStore` filled only
//! by ADD_IDENTITY), so backing it with the vault would mean holding
//! every DECRYPTED key in russh's map for the whole unlocked window,
//! defeating the decrypt-at-sign model. We own the small frozen
//! protocol instead and use russh's `AgentClient` as the test oracle.

#![allow(dead_code)] // Phase 2 mounts the runtime that calls this.

pub(crate) mod listener;
pub(crate) mod protocol;
pub(crate) mod source;
