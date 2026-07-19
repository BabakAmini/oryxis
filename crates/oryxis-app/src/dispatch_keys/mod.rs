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

use crate::app::{Message, KeysMessage, Oryxis};

impl Oryxis {
    /// Route a keys/identities message straight to the submodule that
    /// owns its variant. Exhaustive on purpose: a new `KeysMessage`
    /// variant fails to compile until it is listed in its owner's
    /// group, so it can never be silently dropped.
    pub(crate) fn handle_keys(&mut self, message: KeysMessage) -> Task<Message> {
        match message {
            m @ (KeysMessage::ShowKeyPanel
            | KeysMessage::HideKeyPanel
            | KeysMessage::KeyImportLabelChanged(..)
            | KeysMessage::KeyContentAction(..)
            | KeysMessage::BrowseKeyFile
            | KeysMessage::KeyFileLoaded(..)
            | KeysMessage::KeyFileBrowseError(..)
            | KeysMessage::KeyImportPassphraseChanged(..)
            | KeysMessage::KeyImportPassphraseToggleVisibility
            | KeysMessage::KeyImportPublicAction(..)
            | KeysMessage::ShowKeyPanelCertFocus
            | KeysMessage::ShowKeyPanelPublicFocus
            | KeysMessage::ImportKey
            | KeysMessage::RequestDeleteKey(..)
            | KeysMessage::DeleteKey(..)
            | KeysMessage::ShowKeyMenu(..)
            | KeysMessage::HideKeyMenu
            | KeysMessage::EditKey(..)
            | KeysMessage::KeySearchChanged(..)
            | KeysMessage::SnippetSearchChanged(..)
            | KeysMessage::HistorySearchChanged(..)) => self
                .handle_keys_import(m)
                .unwrap_or_else(crate::dispatch::unrouted),
            m @ (KeysMessage::ShowKeyGeneratePanel
            | KeysMessage::HideKeyGeneratePanel
            | KeysMessage::KeyGenLabelChanged(..)
            | KeysMessage::KeyGenCommentChanged(..)
            | KeysMessage::KeyGenAlgoSelected(..)
            | KeysMessage::KeyGenBitsSelected(..)
            | KeysMessage::KeyGenCurveSelected(..)
            | KeysMessage::GenerateKey
            | KeysMessage::KeyGenerated(..)
            | KeysMessage::CopyGeneratedPublicKey
            | KeysMessage::SaveGeneratedPublicKeyFile
            | KeysMessage::KeyGenExportPassphraseChanged(..)
            | KeysMessage::KeyGenExportPassphraseConfirmChanged(..)
            | KeysMessage::KeyGenExportPassphraseToggleVisibility
            | KeysMessage::KeyGenExportPassphraseConfirmToggleVisibility
            | KeysMessage::ExportGeneratedPrivateKey) => self
                .handle_keys_generate(m)
                .unwrap_or_else(crate::dispatch::unrouted),
            m @ (KeysMessage::KeyImportCertAction(..)
            | KeysMessage::BrowseCertFile
            | KeysMessage::CertFileLoaded(..)
            | KeysMessage::ViewKeyCertificate(..)
            | KeysMessage::CloseCertViewer
            | KeysMessage::RequestRemoveKeyCertificate(..)
            | KeysMessage::RemoveKeyCertificate(..)) => self
                .handle_keys_certs(m)
                .unwrap_or_else(crate::dispatch::unrouted),
            m @ (KeysMessage::ShowIdentityPanel
            | KeysMessage::HideIdentityPanel
            | KeysMessage::IdentityLabelChanged(..)
            | KeysMessage::IdentityUsernameChanged(..)
            | KeysMessage::IdentityPasswordChanged(..)
            | KeysMessage::IdentityKeyChanged(..)
            | KeysMessage::IdentityTogglePasswordVisibility
            | KeysMessage::SaveIdentity
            | KeysMessage::EditIdentity(..)
            | KeysMessage::RequestDeleteIdentity(..)
            | KeysMessage::DeleteIdentity(..)
            | KeysMessage::ShowIdentityMenu(..)
            | KeysMessage::ToggleKeychainAddMenu) => self
                .handle_keys_identities(m)
                .unwrap_or_else(crate::dispatch::unrouted),
        }
    }
}
