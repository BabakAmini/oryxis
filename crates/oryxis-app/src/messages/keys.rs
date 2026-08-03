//! Keychain: SSH keys, identities, key generation, import, certificates.

use iced::widget::text_editor;

#[derive(Debug, Clone)]
pub enum KeysMessage {
    ShowKeyPanel,
    HideKeyPanel,
    /// Keychain > ADD > Generate key: open the generation panel.
    ShowKeyGeneratePanel,
    HideKeyGeneratePanel,
    KeyGenLabelChanged(String),
    KeyGenCommentChanged(String),
    KeyGenAlgoSelected(crate::state::KeyGenAlgo),
    KeyGenBitsSelected(oryxis_vault::RsaBits),
    KeyGenCurveSelected(oryxis_vault::EcdsaCurveChoice),
    /// Kick the generation task (RSA runs seconds; spawn_blocking).
    GenerateKey,
    /// Generation finished; Ok saves to the vault and shows the
    /// result screen.
    KeyGenerated(Result<std::sync::Arc<oryxis_vault::GeneratedKey>, String>),
    CopyGeneratedPublicKey,
    SaveGeneratedPublicKeyFile,
    KeyGenExportPassphraseChanged(super::Redacted),
    KeyGenExportPassphraseConfirmChanged(super::Redacted),
    KeyGenExportPassphraseToggleVisibility,
    KeyGenExportPassphraseConfirmToggleVisibility,
    /// Export the generated private key to a file, passphrase-
    /// encrypted when the pair fields are non-empty.
    ExportGeneratedPrivateKey,
    KeyImportLabelChanged(String),
    KeyContentAction(text_editor::Action),
    BrowseKeyFile,
    KeyFileLoaded(String, String, Option<String>, Option<String>),
    KeyFileBrowseError(String),
    KeyImportPassphraseChanged(super::Redacted),
    KeyImportPassphraseToggleVisibility,
    /// Edit action on the public-key textarea (B2.1).
    KeyImportPublicAction(text_editor::Action),
    /// Edit action on the attached-certificate textarea (B2).
    KeyImportCertAction(text_editor::Action),
    /// Open the key import panel with the certificate field focused
    /// (the keychain ADD menu's "Certificate" entry, B2.1).
    ShowKeyPanelCertFocus,
    /// Open the key import panel with the public-key field focused
    /// (the keychain ADD menu's "Import public key" entry, B3).
    ShowKeyPanelPublicFocus,
    /// Pick a `.pub` certificate file for the key import form.
    BrowseCertFile,
    /// A certificate file was read; its contents fill the paste field.
    CertFileLoaded(String),
    /// Open the read-only certificate viewer for key at index.
    ViewKeyCertificate(usize),
    CloseCertViewer,
    /// Ask for confirmation before detaching a key's certificate.
    RequestRemoveKeyCertificate(usize),
    /// Detach the certificate from key at index (post-confirm).
    RemoveKeyCertificate(usize),
    ImportKey,
    /// Ask for confirmation before removing a key.
    RequestDeleteKey(usize),
    DeleteKey(usize),
    ShowKeyMenu(usize),
    #[allow(dead_code)]
    HideKeyMenu,
    EditKey(usize),
    KeySearchChanged(String),
    /// Workspace sub-nav search input wired to Snippets view.
    SnippetSearchChanged(String),
    /// Workspace sub-nav search input wired to History view.
    HistorySearchChanged(String),
    ShowIdentityPanel,
    HideIdentityPanel,
    IdentityLabelChanged(String),
    IdentityUsernameChanged(String),
    IdentityPasswordChanged(super::Redacted),
    IdentityKeyChanged(String),
    IdentityTogglePasswordVisibility,
    SaveIdentity,
    EditIdentity(usize),
    /// Ask for confirmation before removing an identity.
    RequestDeleteIdentity(usize),
    DeleteIdentity(usize),
    ShowIdentityMenu(usize),
    ToggleKeychainAddMenu,
}
