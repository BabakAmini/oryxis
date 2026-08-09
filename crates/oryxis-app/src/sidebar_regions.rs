//! The two terminal-sidebar regions (issue #102).
//!
//! Every sidebar tab is docked to a physical side
//! (`AppPrefs::sidebar_tab_side`), so the terminal can carry a LEFT
//! and a RIGHT region at once, each with its own strip, active tab,
//! width and open flag. This module is the single authority for the
//! questions everyone else asks about a region:
//!
//! - which tabs a region offers right now (dock side + feature gates
//!   + the focused pane's transport),
//! - which tab a region shows (the remembered active tab re-resolved
//!   against those offers),
//! - whether a region is actually on screen (open AND non-empty),
//! - what happens when a tab changes sides in Settings.
//!
//! The render site, the keynav router, the tab-bar toggle buttons and
//! the SFTP width reservation all read these, so they can't drift.

use crate::app::Oryxis;
use crate::state::{SidebarSide, TerminalSidebarTab};

impl Oryxis {
    /// Whether the ACTIVE terminal tab's focused pane has a live SSH
    /// session, the transport gate shared by Files / Monitor / Tmux.
    fn active_pane_has_ssh(&self) -> bool {
        self.active_tab
            .and_then(|i| self.tabs.get(i))
            .and_then(|t| t.active().session.as_ref().and_then(|s| s.ssh()))
            .is_some()
    }

    /// Whether a sidebar tab is offered at all for the active terminal
    /// tab: its feature toggles plus the focused pane's transport.
    /// Snippets / History / Host config / Hosts never gate, which is
    /// what guarantees the RIGHT region always has content and lets a
    /// LEFT region holding only the hosts tree survive any pane.
    pub(crate) fn sidebar_tab_available(&self, tab: TerminalSidebarTab) -> bool {
        match tab {
            TerminalSidebarTab::Chat => self.ai.enabled,
            TerminalSidebarTab::Files => self.sftp_enabled && self.active_pane_has_ssh(),
            TerminalSidebarTab::Monitor => {
                self.prefs.host_monitoring && self.active_pane_has_ssh()
            }
            TerminalSidebarTab::Tmux => self.prefs.tmux_manager && self.active_pane_has_ssh(),
            TerminalSidebarTab::Snippets
            | TerminalSidebarTab::History
            | TerminalSidebarTab::HostConfig
            | TerminalSidebarTab::HostsTree => true,
        }
    }

    /// The tabs a region offers right now, in strip order.
    pub(crate) fn sidebar_region_tabs(&self, side: SidebarSide) -> Vec<TerminalSidebarTab> {
        TerminalSidebarTab::ALL
            .into_iter()
            .filter(|t| self.prefs.sidebar_tab_side(*t) == side && self.sidebar_tab_available(*t))
            .collect()
    }

    /// Whether a region has anything to show. A toggle button only
    /// renders (and an open region only mounts) while this holds.
    pub(crate) fn sidebar_region_has_tabs(&self, side: SidebarSide) -> bool {
        TerminalSidebarTab::ALL
            .into_iter()
            .any(|t| self.prefs.sidebar_tab_side(t) == side && self.sidebar_tab_available(t))
    }

    /// The tab a region shows: the remembered active tab when it still
    /// belongs to this region and passes its gates, else the region's
    /// first offer, else `None` (empty region, nothing renders).
    pub(crate) fn sidebar_region_tab(&self, side: SidebarSide) -> Option<TerminalSidebarTab> {
        let want = self.terminal_sidebar_tab[side.idx()];
        if self.prefs.sidebar_tab_side(want) == side && self.sidebar_tab_available(want) {
            return Some(want);
        }
        self.sidebar_region_tabs(side).into_iter().next()
    }

    /// Whether a region is on screen for the given terminal tab: open
    /// on that tab, not replaced by Files mode, and non-empty.
    pub(crate) fn sidebar_region_shown(
        &self,
        tab: &crate::state::TerminalTab,
        side: SidebarSide,
    ) -> bool {
        tab.sidebar_visible(side) && !tab.files_mode && self.sidebar_region_has_tabs(side)
    }

    /// `sidebar_region_shown` for the ACTIVE terminal tab.
    pub(crate) fn active_sidebar_shown(&self, side: SidebarSide) -> bool {
        self.active_tab
            .and_then(|i| self.tabs.get(i))
            .is_some_and(|t| self.sidebar_region_shown(t, side))
    }

    /// The regions that deserve a toggle button in the window chrome:
    /// every side whose region has at least one available tab, left
    /// first. One button per region, so with tabs docked to both
    /// sides the chrome shows two icons (issue #102).
    pub(crate) fn sidebar_toggle_sides(&self) -> Vec<SidebarSide> {
        SidebarSide::BOTH
            .into_iter()
            .filter(|s| self.sidebar_region_has_tabs(*s))
            .collect()
    }

    /// Whether a sidebar tab is actually on screen right now: its
    /// region is open on the active terminal tab and resolves to it.
    /// The "should I refresh what this tab shows" gate (History
    /// follows the focused pane, etc.).
    pub(crate) fn sidebar_tab_shown(&self, tab: TerminalSidebarTab) -> bool {
        let side = self.prefs.sidebar_tab_side(tab);
        self.active_sidebar_shown(side) && self.sidebar_region_tab(side) == Some(tab)
    }

    /// Make `tab` the active tab of its own region (the region is
    /// looked up, never passed, so a caller can't desync them).
    pub(crate) fn set_sidebar_region_tab(&mut self, tab: TerminalSidebarTab) {
        let side = self.prefs.sidebar_tab_side(tab);
        self.terminal_sidebar_tab[side.idx()] = tab;
    }

    /// A tab changed sides in Settings. Make it active on its new
    /// region (the user is configuring it, losing it behind another
    /// tab would read as a silent no-op) and let the old region's
    /// remembered tab re-resolve on the next read; if the moved tab
    /// was showing in an open region, carry the open state over so
    /// the tab visibly travels instead of vanishing.
    pub(crate) fn sidebar_tab_moved(&mut self, tab: TerminalSidebarTab, to: SidebarSide) {
        let from = to.other();
        let was_showing = self.terminal_sidebar_tab[from.idx()] == tab;
        self.terminal_sidebar_tab[to.idx()] = tab;
        if was_showing {
            let from_open = self
                .active_tab
                .and_then(|i| self.tabs.get(i))
                .is_some_and(|t| t.sidebar_visible(from));
            if from_open {
                // Only collapse the old region when the move emptied
                // it; otherwise it re-resolves to its next tab and
                // stays. Computed against the ALREADY-updated prefs.
                let collapse_from = !self.sidebar_region_has_tabs(from);
                if let Some(idx) = self.active_tab
                    && let Some(ttab) = self.tabs.get_mut(idx)
                {
                    ttab.sidebar_open[to.idx()] = true;
                    if collapse_from {
                        ttab.sidebar_open[from.idx()] = false;
                    }
                }
            }
        }
        // A ring engaged on the moved tab points at rows that next
        // frame will re-record in the other region's list; drop it
        // rather than let a stale (tab, idx) pair act on the wrong row.
        if self.keynav.sidebar_selected.is_some_and(|(t, _)| t == tab) {
            self.keynav.sidebar_selected = None;
        }
    }
}
