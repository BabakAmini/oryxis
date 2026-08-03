//! The SFTP surface's chrome: where the split sits, and the column
//! layout with its live drags.
//!
//! Deliberately NOT part of `SftpState`: that one is parked and hoisted
//! per tab (hybrid Files mode swaps it on focus), while these describe
//! the surface itself and must not travel with a tab.

#[derive(Debug)]
pub(crate) struct SftpChrome {
    /// SFTP center-split ratio: fraction (0..1) of the content width given
    /// to the left pane. Global across SFTP tabs, persisted to the
    /// `sftp_split_ratio` setting; only changed by dragging the divider.
    pub(crate) split_ratio: f32,
    /// Some((cursor_x_at_drag_start, ratio_at_drag_start)) while the user
    /// is dragging the SFTP center divider.
    pub(crate) split_drag: Option<(f32, f32)>,
    /// Some((cursor_y_at_drag_start, height_at_drag_start)) while the user is
    /// dragging the divider above the SFTP message-log panel.
    pub(crate) log_drag: Option<(f32, f32)>,
    /// Persisted template for the per-pane column configuration. New SFTP
    /// panes/tabs are seeded from this; editing any pane's columns updates
    /// it (and the `sftp_columns` / `sftp_col_order` / `sftp_col_widths`
    /// settings) so the preferred shape carries across restarts.
    pub(crate) columns_template: crate::state::SftpColumnState,
    /// Active column-resize drag: `(side, column, cursor_x_at_start,
    /// width_at_start)`. Updated by the global mouse-move handler.
    pub(crate) col_resize: Option<( crate::state::SftpPaneSide, crate::state::SftpColumn, f32, f32, )>,
    /// Active column-reorder drag (header being dragged).
    pub(crate) col_drag: Option<crate::state::SftpColDrag>,
    /// Column header the cursor is currently over, the reorder drop target.
    pub(crate) hovered_col: Option<(crate::state::SftpPaneSide, crate::state::SftpColumn)>,
}

impl Default for SftpChrome {
    fn default() -> Self {
        Self {
            split_ratio: 0.5,
            split_drag: None,
            log_drag: None,
            columns_template: crate::state::SftpColumnState::default(),
            col_resize: None,
            col_drag: None,
            hovered_col: None,
        }
    }
}
