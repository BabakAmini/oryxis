pub mod keygen;
pub mod portable;
pub mod store;

pub use keygen::{
    encrypt_private_pem, generate_ed25519, generate_key, import_key, import_public_key,
    is_key_encrypted, EcdsaCurveChoice, GenerateSpec, GeneratedKey, RsaBits,
};
pub use portable::{export_vault, import_vault, inspect_export, is_valid_export, export_includes_keys, ExportCategory, ExportFilter, ExportOptions, ExportSelection, ExportSummary, ImportResult};
pub use store::{
    derive_sync_secret, CommandHistoryEntry, SessionLogEntry, SessionLogEvent, SyncPeerRow,
    Tombstone,
    VaultError, VaultStore,
};
