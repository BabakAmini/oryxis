//! Multi-host monitor dashboard state (issue #95).
//!
//! One link per monitored MACHINE, not per card (issue #156): several
//! vault rows routinely point at one server, and giving each its own
//! link had them reporting that server three times, at three instants,
//! for three logins. Metrics do NOT live here either: samples land in
//! the same `monitor.series` rings the per-session sidebar reads, keyed
//! by the same machine, so every surface always shows identical numbers.

use std::collections::HashMap;

use uuid::Uuid;

use crate::monitor::endpoint::MonitorKey;

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

/// A machine's slot on the dashboard, shared by every card that
/// reaches it. `via` is the row the link authenticates as: the cards
/// of a machine differ in credentials, and only one of them is used.
#[derive(Clone)]
pub(crate) enum DashLink {
    /// Dial in flight.
    Connecting { via: Uuid, tried: Vec<Uuid> },
    Live {
        via: Uuid,
        transport: DashTransport,
    },
    /// The dial (or a probe on a dead link's redial) failed. Every row
    /// that reaches this machine is tried ONCE before the slot settles
    /// here: a machine whose `root` row refuses password auth is still
    /// monitored through its `deploy` row. Then it is sticky, per the
    /// dashboard's rule of never retrying a down host on its own; the
    /// card's retry action and re-entering the view do.
    Failed {
        via: Uuid,
        error: String,
        tried: Vec<Uuid>,
    },
}

impl DashLink {
    /// The row this slot is (or was last) reached through, for the
    /// card that has to say whose credentials answered.
    pub(crate) fn via(&self) -> Uuid {
        match self {
            Self::Connecting { via, .. } | Self::Live { via, .. } | Self::Failed { via, .. } => {
                *via
            }
        }
    }

}

/// Dashboard state hanging off the app.
pub(crate) struct MonitorDash {
    pub links: HashMap<MonitorKey, DashLink>,
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
    /// Table-mode sort column and direction. Session-only: a sort is
    /// a working posture, not a preference worth a vault row.
    pub sort_key: DashSortKey,
    pub sort_asc: bool,
    /// "Stop reading my servers for now" (issue #156 follow-up).
    ///
    /// Session-only on purpose: the persistent answers already exist
    /// one level up (the master monitoring toggle and the per-host
    /// opt-in), and this one is the posture you take for the next few
    /// minutes, not a setting you maintain.
    ///
    /// Scope is the FLEET, not monitoring as a whole: this view is the
    /// only surface that opens connections by itself, while the
    /// sidebar tab reads over the session its own tab already holds.
    /// While paused the heartbeat does no work and the dialed
    /// connections are closed as soon as nothing is reading them, so a
    /// pause really is nothing on the wire; the cards keep showing the
    /// last sample, greyed, because a blank card would read as "this
    /// host went away".
    pub paused: bool,
}

impl Default for MonitorDash {
    fn default() -> Self {
        Self {
            links: HashMap::new(),
            tick: 0,
            stamp: 0,
            search: String::new(),
            selected: None,
            sort_key: DashSortKey::Label,
            // A-z out of the box; `derive(Default)` would boot the
            // table sorted Z-a.
            sort_asc: true,
            paused: false,
        }
    }
}

/// Sortable columns of the dashboard's table (list) mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum DashSortKey {
    #[default]
    Label,
    Cpu,
    Mem,
    Net,
    Disk,
    Uptime,
}

impl MonitorDash {
    /// Drop every link, closing the pooled ones, and invalidate
    /// whatever is in flight. The detail panel dies with them: a
    /// panel about a swept host would render stale vitals as live.
    pub(crate) fn sweep(&mut self) {
        for link in self.links.values() {
            if let DashLink::Live { transport, .. } = link {
                transport.close_pooled();
            }
        }
        self.links.clear();
        self.selected = None;
        self.stamp = self.stamp.wrapping_add(1);
    }

    /// Close and drop the links of machines nothing is reading right
    /// now, keeping the windows and the open detail panel.
    ///
    /// This is what makes a pause (and a one-shot refresh taken during
    /// one) hold no connection: the probe in flight is left alone, and
    /// what it was reading is closed on the tick after it lands. Unlike
    /// [`Self::sweep`] it does NOT bump the stamp, because nothing here
    /// invalidates a result that is still on its way.
    pub(crate) fn park_idle_links(&mut self, busy: impl Fn(&MonitorKey) -> bool) {
        let idle: Vec<MonitorKey> = self
            .links
            .iter()
            .filter(|(key, link)| matches!(link, DashLink::Live { .. }) && !busy(key))
            .map(|(key, _)| key.clone())
            .collect();
        for key in idle {
            if let Some(DashLink::Live { transport, .. }) = self.links.remove(&key) {
                transport.close_pooled();
            }
        }
    }
}
