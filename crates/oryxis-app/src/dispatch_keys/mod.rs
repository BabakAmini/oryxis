//! `Oryxis::handle_keys`, match arms for the Keys + Identities
//! panels: import/edit/delete keys, manage identities, keychain menu.
//! The router fans `Message` variants out to per-area submodules:
//!
//! - `import`    : key CRUD + the import panel (file dialogs, the
//!   `ImportKey` save path, the searches).
//! - `generate`  : the keygen panel flow (keychain > ADD > Generate).
//! - `certs`     : certificate attach / validate / viewer.
//! - `identities`: identity CRUD + the keychain ADD menu.

#![allow(clippy::result_large_err)]

mod certs;
mod generate;
mod identities;
mod import;

use iced::Task;

use crate::app::{Message, Oryxis};

impl Oryxis {
    /// Dispatch a keys/identities `Message` to the matching submodule
    /// handler. Each submodule returns `Err(message)` for variants it
    /// doesn't handle so the chain falls through to the next; the
    /// final `Err` propagates back to `dispatch::update` so the other
    /// handlers (or the inline match) get their turn.
    pub(crate) fn handle_keys(
        &mut self,
        message: Message,
    ) -> Result<Task<Message>, Message> {
        let message = match self.handle_keys_import(message) {
            Ok(task) => return Ok(task),
            Err(m) => m,
        };
        let message = match self.handle_keys_generate(message) {
            Ok(task) => return Ok(task),
            Err(m) => m,
        };
        let message = match self.handle_keys_certs(message) {
            Ok(task) => return Ok(task),
            Err(m) => m,
        };
        let message = match self.handle_keys_identities(message) {
            Ok(task) => return Ok(task),
            Err(m) => m,
        };
        Err(message)
    }
}
