//! Tab-strip ordering helpers split out of `dispatch_tabs`:
//! reconcile / replace-id / live-slide reorder, and the connect-
//! progress re-anchor after a bulk tab filter.

use crate::app::Oryxis;

impl Oryxis {
    /// Give the Settings surface a strip entry (issue #120), so leaving it
    /// and coming back is one click instead of a hunt through the toolbar.
    /// Idempotent, and called from `ChangeView(Settings)`, which is the
    /// single door every entry point (gear, burger menu, hotkey, command
    /// palette, the strip entry itself) already goes through. That is why
    /// there is at most one: nothing else can mint a second.
    ///
    /// Modelled on `ensure_sftp_tab`, which does the same for `View::Sftp`.
    pub(crate) fn ensure_settings_tab(&mut self) {
        if self.settings_tab_open {
            return;
        }
        self.settings_tab_open = true;
        if !self.tab_order.contains(&crate::state::TabRef::Settings) {
            self.tab_order.push(crate::state::TabRef::Settings);
        }
    }

    /// Close the Settings tab. When Settings is the surface on screen the
    /// close has to take you somewhere, so it lands on the previously
    /// focused tab if there still is one, and on the Home dashboard
    /// otherwise. Closing it from another surface only removes the chip.
    pub(crate) fn close_settings_tab(&mut self) -> iced::Task<crate::app::Message> {
        self.settings_tab_open = false;
        self.tab_order.retain(|r| !matches!(r, crate::state::TabRef::Settings));
        self.settings_scroll.clear();
        if !(self.active_tab.is_none() && self.active_view == crate::state::View::Settings) {
            return iced::Task::none();
        }
        // Most-recently-used first, skipping the entry we just dropped.
        let fallback = self
            .tab_mru
            .iter()
            .find(|r| !matches!(r, crate::state::TabRef::Settings))
            .copied()
            .and_then(|r| self.tab_ref_select_msg(&r));
        iced::Task::done(fallback.unwrap_or(crate::app::Message::Navigation(
            crate::app::NavigationMessage::ChangeView(crate::state::View::Dashboard),
        )))
    }

    /// Sync `tab_order` (the authoritative strip display order across terminal
    /// and SFTP tabs) with the live tabs: append refs for newly-created tabs,
    /// drop refs for closed ones, preserve the existing (drag-reordered) order.
    /// Cheap; called at the end of every `update`.
    pub(crate) fn reconcile_tab_order(&mut self) {
        use crate::state::TabRef;
        self.tab_order.retain(|r| match r {
            TabRef::Terminal(id) => self.tabs.iter().any(|t| t._id == *id),
            TabRef::Sftp(id) => self.sftp_tabs.iter().any(|t| t.id == *id),
            // Not backed by a storage vec: `settings_tab_open` is the
            // whole existence test.
            TabRef::Settings => self.settings_tab_open,
        });
        for id in self.tabs.iter().map(|t| t._id).collect::<Vec<_>>() {
            if !self.tab_order.iter().any(|r| matches!(r, TabRef::Terminal(x) if *x == id)) {
                self.tab_order.push(TabRef::Terminal(id));
            }
        }
        for id in self.sftp_tabs.iter().map(|t| t.id).collect::<Vec<_>>() {
            if !self.tab_order.iter().any(|r| matches!(r, TabRef::Sftp(x) if *x == id)) {
                self.tab_order.push(TabRef::Sftp(id));
            }
        }
    }

    /// Replace a terminal tab's id in `tab_order` in place (same position).
    /// Used when a dormant placeholder is swapped for its freshly-connected
    /// live tab (new id) so the reopened tab keeps its strip position instead
    /// of being appended at the end by `reconcile_tab_order`.
    pub(crate) fn replace_tab_order_id(&mut self, old: uuid::Uuid, new: uuid::Uuid) {
        for r in self.tab_order.iter_mut() {
            if let crate::state::TabRef::Terminal(id) = r
                && *id == old
            {
                *id = new;
                return;
            }
        }
    }

    /// Move the tab identified by `from_id` to just before `target_id` in
    /// `tab_order`, but only within the same pin partition (can't drag an
    /// unpinned tab above a pinned one, matching the terminal behaviour). Used
    /// by the unified live-slide drag. Re-anchors nothing (the storage vecs and
    /// `active_tab` / `active_sftp` indices are untouched; only display order
    /// changes).
    pub(crate) fn slide_tab_in_order(&mut self, from_id: uuid::Uuid, target_id: uuid::Uuid) {
        let pinned_of = |r: &crate::state::TabRef| -> bool {
            match r {
                crate::state::TabRef::Terminal(id) => {
                    self.tabs.iter().find(|t| t._id == *id).map(|t| t.pinned).unwrap_or(false)
                }
                crate::state::TabRef::Sftp(id) => {
                    self.sftp_tabs.iter().find(|t| t.id == *id).map(|t| t.pinned).unwrap_or(false)
                }
                // Transient by design, so pinning it would promise a
                // persistence it does not have.
                crate::state::TabRef::Settings => false,
            }
        };
        let id_of = |r: &crate::state::TabRef| -> uuid::Uuid { r.strip_id() };
        let Some(from_pos) = self.tab_order.iter().position(|r| id_of(r) == from_id) else { return };
        let Some(to_pos) = self.tab_order.iter().position(|r| id_of(r) == target_id) else { return };
        if from_pos == to_pos {
            return;
        }
        // Same partition only.
        if pinned_of(&self.tab_order[from_pos]) != pinned_of(&self.tab_order[to_pos]) {
            return;
        }
        let moved = self.tab_order.remove(from_pos);
        let dest = if from_pos < to_pos { to_pos - 1 } else { to_pos };
        self.tab_order.insert(dest, moved);
    }

    /// Move the tab identified by `from_id` to the very end of its own pin
    /// partition in `tab_order` (last among normal tabs, or last among pinned).
    /// Powers the trailing drop zone so a tab can reach the rightmost slot,
    /// which the before-the-target live-slide can never express. Idempotent:
    /// a no-op when the tab already sits at its partition's end, so repeated
    /// `CursorMoved`-driven calls don't thrash.
    pub(crate) fn slide_tab_to_partition_end(&mut self, from_id: uuid::Uuid) {
        let pinned_of = |r: &crate::state::TabRef| -> bool {
            match r {
                crate::state::TabRef::Terminal(id) => {
                    self.tabs.iter().find(|t| t._id == *id).map(|t| t.pinned).unwrap_or(false)
                }
                crate::state::TabRef::Sftp(id) => {
                    self.sftp_tabs.iter().find(|t| t.id == *id).map(|t| t.pinned).unwrap_or(false)
                }
                // Transient by design, so pinning it would promise a
                // persistence it does not have.
                crate::state::TabRef::Settings => false,
            }
        };
        let id_of = |r: &crate::state::TabRef| -> uuid::Uuid { r.strip_id() };
        let Some(from_pos) = self.tab_order.iter().position(|r| id_of(r) == from_id) else {
            return;
        };
        let from_pinned = pinned_of(&self.tab_order[from_pos]);
        // Last slot that belongs to the dragged tab's partition.
        let Some(last_same) = self.tab_order.iter().rposition(|r| pinned_of(r) == from_pinned)
        else {
            return;
        };
        if from_pos >= last_same {
            return;
        }
        // Removing `from_pos` shifts everything after it down one, so the old
        // `last_same` now sits at `last_same - 1`; inserting at `last_same`
        // drops the tab immediately after it (the new partition end).
        let moved = self.tab_order.remove(from_pos);
        self.tab_order.insert(last_same, moved);
    }

    /// Re-anchor (or clear) the in-flight connect progress after the tab
    /// list was filtered by close-others / close-all (both keep pinned
    /// tabs). `connecting_id` is the connecting tab's id captured *before*
    /// the filter: if that tab survived, point `tab_idx` at its new slot;
    /// if it was closed, drop the progress so a later SshRetry /
    /// SshCloseProgress can't `remove()` the wrong (surviving / pinned) tab.
    pub(super) fn reanchor_connecting_after_filter(&mut self, connecting_id: Option<uuid::Uuid>) {
        if self.connecting.is_none() {
            return;
        }
        match connecting_id.and_then(|cid| self.tabs.iter().position(|t| t._id == cid)) {
            Some(i) => {
                if let Some(p) = self.connecting.as_mut() {
                    p.tab_idx = i;
                }
            }
            None => self.connecting = None,
        }
    }

}
