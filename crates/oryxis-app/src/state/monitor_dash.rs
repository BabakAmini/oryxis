//! Multi-host monitor dashboard state (issue #95).
//!
//! One link per opted-in host. Metrics do NOT live here: samples land
//! in the same `monitor.series` rings the per-session sidebar reads,
//! so both surfaces always show identical numbers for a host.

use std::collections::HashMap;

use uuid::Uuid;

/// How the dashboard reaches a host.
#[derive(Clone)]
pub(crate) enum DashTransport {
    /// Borrowed from a live terminal tab. Never closed by the
    /// dashboard: the tab owns the session's lifecycle.
    Tab(std::sync::Arc<oryxis_ssh::SshSession>),
    /// Dialed by the dashboard itself (headless, probe-only). Closed
    /// by the idle-TTL sweep, the feature toggle-off and the lock
    /// sweep.
    Pool(std::sync::Arc<oryxis_ssh::MonitorConn>),
}

impl DashTransport {
    pub(crate) fn is_alive(&self) -> bool {
        match self {
            Self::Tab(s) => s.is_alive(),
            Self::Pool(c) => c.is_alive(),
        }
    }

    /// Close a pooled transport; a borrowed tab session is left alone.
    pub(crate) fn close_pooled(&self) {
        if let Self::Pool(c) = self {
            c.close();
        }
    }
}

/// A host's slot on the dashboard.
#[derive(Clone)]
pub(crate) enum DashLink {
    /// Dial in flight.
    Connecting,
    Live(DashTransport),
    /// Dial (or a probe on a dead link's redial) failed. Sticky: the
    /// dashboard never retries on its own; the card's retry action and
    /// re-entering the view do.
    Failed(String),
}

/// Dashboard state hanging off the app.
#[derive(Default)]
pub(crate) struct MonitorDash {
    pub links: HashMap<Uuid, DashLink>,
    /// One-second counter driving the per-host stagger, so N hosts
    /// don't open N exec channels on the same tick.
    pub tick: u64,
    /// Bumped by every sweep (lock, toggle-off, TTL) so dials and
    /// probes still in flight land on a generation that no longer
    /// exists and are discarded (and their connection closed).
    pub stamp: u64,
    /// Card filter typed in the view's search field. Display-only: the
    /// whole fleet keeps polling, a filter is a lens.
    pub search: String,
    /// The host whose detail panel is open on the trailing edge.
    /// Clicking a card selects it; connecting to the host is an
    /// explicit action inside the panel, never the card click (owner
    /// call on the first live build).
    pub selected: Option<Uuid>,
}

impl MonitorDash {
    /// Drop every link, closing the pooled ones, and invalidate
    /// whatever is in flight. The detail panel dies with them: a
    /// panel about a swept host would render stale vitals as live.
    pub(crate) fn sweep(&mut self) {
        for link in self.links.values() {
            if let DashLink::Live(t) = link {
                t.close_pooled();
            }
        }
        self.links.clear();
        self.selected = None;
        self.stamp = self.stamp.wrapping_add(1);
    }
}
