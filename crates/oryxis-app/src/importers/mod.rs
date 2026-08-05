//! Third-party config importers (roadmap D2): each format parses into
//! plain [`Connection`](oryxis_core::models::connection::Connection)s
//! and rides the shared import-preview dialog (pick per host, dedup by
//! existing label, batch save). The ssh_config importer predates this
//! module and keeps its own file (it needs a second alias-resolution
//! pass none of these formats have).

pub(crate) mod csv;
pub(crate) mod detect;
pub(crate) mod finalshell;
pub(crate) mod ini;
pub(crate) mod mobaxterm;
pub(crate) mod mremoteng;
pub(crate) mod putty;
pub(crate) mod regfile;
pub(crate) mod securecrt;
pub(crate) mod winscp;
pub(crate) mod xshell;

/// One host produced by a foreign-format parse: the mapped connection
/// plus the credential the source carried, when it carried one
/// (WinSCP's and mRemoteNG's decodable schemes). The password travels
/// only from parse to the batch save, where it lands in the encrypted
/// column.
#[derive(Clone)]
pub(crate) struct DirectHost {
    pub conn: oryxis_core::models::connection::Connection,
    pub password: Option<String>,
}

// Hand-written so a debug print can never put an imported password in
// a log line: the field is reported as present/absent, never by value.
// (Message enums derive Debug, which is what forces the impl.)
impl std::fmt::Debug for DirectHost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DirectHost")
            .field("conn", &self.conn.label)
            .field("password", &self.password.is_some())
            .finish()
    }
}

/// A parsed batch waiting in the shared import-preview dialog, for
/// the formats that map straight to [`Connection`]s (no second
/// alias-resolution pass). When `Oryxis::ssh_import_direct` holds
/// one, the dialog renders it instead of the ssh_config host list and
/// the confirm saves it directly.
#[derive(Debug, Clone)]
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

