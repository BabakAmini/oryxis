//! ZMODEM in-terminal transfer progress and download-dir settings, wrapped by [`crate::messages::Message::Zmodem`]. Handled by `Oryxis::handle_zmodem`.

use uuid::Uuid;

#[derive(Debug, Clone)]
pub enum ZmodemMessage {
    /// A ZMODEM transfer streamed a progress / outcome event for a pane.
    /// Terminal states (Completed / Aborted / Error) clear the pane's
    /// transfer and resume the terminal.
    ZmodemProgress(Uuid, oryxis_zmodem::Progress),  // (pane_id, progress)
    /// User asked to cancel the pane's in-flight ZMODEM transfer.
    ZmodemCancel(Uuid),  // (pane_id)
    /// Pick the folder ZMODEM downloads are saved into.
    PickZmodemDownloadDir,
    /// ZMODEM download folder chosen (or dialog dismissed with `None`).
    ZmodemDownloadDirPicked(Option<String>),
    /// Reset the ZMODEM download folder to the OS default.
    ClearZmodemDownloadDir,
}
