//! Server to server: the four entry points.
//!
//! All of them are one call into `start_relay`, which is where the
//! guards live (same file refused, containment refused, a move that is
//! really a rename taking the fast path). They are separate messages
//! only because the row menu offers copy and move for files and folders
//! as four distinct actions.

#![allow(clippy::result_large_err)]

use iced::Task;

use crate::app::{SftpMessage, Message, Oryxis};
use super::SftpSides;

impl Oryxis {
    pub(super) fn handle_sftp_relay(
        &mut self,
        message: SftpMessage,
        sides: SftpSides,
    ) -> Result<Task<Message>, SftpMessage> {
        let SftpSides { remote: _remote_side, local: _local_side, owner } = sides;
        match message {
            SftpMessage::SftpRelay(from, src_path) => {
                Ok(self.start_relay(owner, from, src_path, false, false))
            }
            SftpMessage::SftpRelayFolder(from, src_root) => {
                Ok(self.start_relay(owner, from, src_root, true, false))
            }
            SftpMessage::SftpRelayMove(from, src_path) => {
                Ok(self.start_relay(owner, from, src_path, false, true))
            }
            SftpMessage::SftpRelayMoveFolder(from, src_root) => {
                Ok(self.start_relay(owner, from, src_root, true, true))
            }
            // Every arm above returns, so the match IS the return
            // value here rather than a statement before one.
            m => Err(m),
        }
    }
}
