//! Agentless host monitoring (issue #83, plan J2).
//!
//! One read-only probe engine feeds the terminal sidebar's Monitor tab:
//! every metric comes from reading `/proc` and `df` over an exec channel
//! multiplexed on the tab's live SSH session, so nothing is installed on
//! the server and no extra connection is opened.
//!
//! Scope note: the global fleet view and the bottom status bar are
//! separate surfaces on the same engine, deliberately not built here (the
//! global view needs the pooled ad-hoc transports from the connection-
//! reuse work; see `plans/1.0/j2-host-monitoring.md`).
//!
//! Samples live in an in-memory ring per MACHINE (`endpoint::MonitorKey`,
//! issue #156: several vault rows routinely point at one server) and are
//! never persisted, synced or exported.

pub(crate) mod alert;
pub(crate) mod disks;
pub(crate) mod endpoint;
pub(crate) mod kill;
pub(crate) mod model;
pub(crate) mod probe;
pub(crate) mod probe_bsd;
pub(crate) mod ring;

pub(crate) use ring::MonitorState;
