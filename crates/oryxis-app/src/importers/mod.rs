//! Third-party config importers (roadmap D2): each format parses into
//! plain [`Connection`](oryxis_core::models::connection::Connection)s
//! and rides the shared import-preview dialog (pick per host, dedup by
//! existing label, batch save). The ssh_config importer predates this
//! module and keeps its own file (it needs a second alias-resolution
//! pass none of these formats have).

pub(crate) mod putty;
pub(crate) mod regfile;
pub(crate) mod winscp;

/// One host produced by a foreign-format parse: the mapped connection
/// plus the credential the source carried, when it carried one
/// (WinSCP's reversible obfuscation). The password travels only from
/// parse to the batch save, where it lands in the encrypted column.
pub(crate) struct DirectHost {
    pub conn: oryxis_core::models::connection::Connection,
    pub password: Option<String>,
}

/// A parsed batch waiting in the shared import-preview dialog, for
/// the formats that map straight to [`Connection`]s (no second
/// alias-resolution pass). When `Oryxis::ssh_import_direct` holds
/// one, the dialog renders it instead of the ssh_config host list and
/// the confirm saves it directly.
pub(crate) struct DirectImport {
    /// i18n key of the source's menu label, reused as the dialog
    /// title so the two always agree.
    pub source_key: &'static str,
    pub hosts: Vec<DirectHost>,
    /// Sessions the parser could not map (unsupported protocol, no
    /// host), by name: shown as a muted line so nothing disappears
    /// silently.
    pub skipped: Vec<String>,
}

/// The foreign formats behind the ONE import driver: every format is
/// a file-picker filter set plus a pure `parse`, and the shared
/// dispatch (`ShareMessage::ImportForeign` -> picker -> parse ->
/// preview -> batch save) never grows another copy per format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ForeignFormat {
    Putty,
    WinScp,
}

impl ForeignFormat {
    /// i18n key of the menu label; doubles as the dialog title.
    pub(crate) fn source_key(self) -> &'static str {
        match self {
            Self::Putty => "import_putty_btn",
            Self::WinScp => "import_winscp_btn",
        }
    }

    /// File-picker configuration: (dialog title, filter name,
    /// extensions).
    pub(crate) fn picker(self) -> (&'static str, &'static str, &'static [&'static str]) {
        match self {
            Self::Putty => ("Import PuTTY sessions", "Registry export", &["reg"]),
            Self::WinScp => (
                "Import WinSCP sites",
                "WinSCP.ini / registry export",
                &["ini", "reg"],
            ),
        }
    }

    /// Parse the picked file into the shared preview batch.
    pub(crate) fn parse(self, bytes: &[u8]) -> DirectImport {
        match self {
            Self::Putty => {
                let import = putty::parse_reg(&regfile::decode_reg_bytes(bytes));
                DirectImport {
                    source_key: self.source_key(),
                    hosts: import
                        .connections
                        .into_iter()
                        .map(|conn| DirectHost { conn, password: None })
                        .collect(),
                    skipped: import.skipped,
                }
            }
            Self::WinScp => {
                let import = winscp::parse(&regfile::decode_reg_bytes(bytes));
                DirectImport {
                    source_key: self.source_key(),
                    hosts: import.hosts,
                    skipped: import.skipped,
                }
            }
        }
    }
}
