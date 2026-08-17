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

/// Move a user name and a port OUT of an imported host string. Foreign
/// formats carry the value the user typed into THEIR host box, and
/// several accept a whole `user@host:port` there (PuTTY splits one at
/// connect time, and the clients that inherited its config layout do
/// too). Our dial builds `{hostname}:{port}` verbatim, so an unsplit
/// value reaches the resolver intact and fails as a DNS error that
/// names nothing (issue #171).
///
/// The source's own dedicated fields win: a format that already mapped
/// a user or a non-default port stated it more specifically than the
/// host string did. A value that is not a target in any reading is
/// left exactly as found, to be fixed by hand rather than guessed at.
pub(crate) fn split_host_field(conn: &mut oryxis_core::models::connection::Connection) {
    let Some(target) = oryxis_core::ssh_target::SshTarget::from_host_field(&conn.hostname) else {
        return;
    };
    if target.host == conn.hostname && target.username.is_none() && target.port.is_none() {
        return;
    }
    conn.hostname = target.host;
    if let Some(user) = target.username
        && conn.username.as_ref().is_none_or(|u| u.trim().is_empty())
    {
        conn.username = Some(user);
    }
    if let Some(port) = target.port
        && Some(conn.port) == conn.protocol.default_port()
    {
        conn.port = port;
    }
}

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

#[cfg(test)]
mod tests {
    use oryxis_core::models::connection::{Connection, ConnectionProtocol};

    #[test]
    fn split_moves_user_and_port_out_of_the_host_string() {
        let mut conn = Connection::new("web", "root@10.0.0.7:2222");
        super::split_host_field(&mut conn);
        assert_eq!(conn.hostname, "10.0.0.7");
        assert_eq!(conn.username.as_deref(), Some("root"));
        assert_eq!(conn.port, 2222);
    }

    #[test]
    fn split_yields_to_the_fields_the_source_already_mapped() {
        // A format that mapped its own User / Port stated them more
        // specifically than the host string did, so only the hostname
        // gets cleaned.
        let mut conn = Connection::new("web", "root@10.0.0.7:2222");
        conn.username = Some("deploy".to_string());
        conn.port = 2022;
        super::split_host_field(&mut conn);
        assert_eq!(conn.hostname, "10.0.0.7");
        assert_eq!(conn.username.as_deref(), Some("deploy"));
        assert_eq!(conn.port, 2022);
    }

    #[test]
    fn split_leaves_a_value_it_cannot_read_exactly_as_found() {
        // Not a target in any reading: rewriting it would corrupt a row
        // the user can still fix by hand, and the connect-time hint
        // names the problem either way.
        let mut conn = Connection::new("web", "root@10.0.0.7/srv");
        super::split_host_field(&mut conn);
        assert_eq!(conn.hostname, "root@10.0.0.7/srv");
        assert_eq!(conn.username, None);
    }

    #[test]
    fn split_is_a_no_op_on_a_plain_host() {
        let mut conn = Connection::new("web", "web01.example.com");
        conn.port = 22;
        super::split_host_field(&mut conn);
        assert_eq!(conn.hostname, "web01.example.com");
        assert_eq!(conn.username, None);
        assert_eq!(conn.port, 22);
    }

    #[test]
    fn split_trims_a_padded_host() {
        let mut conn = Connection::new("web", "  web01  ");
        super::split_host_field(&mut conn);
        assert_eq!(conn.hostname, "web01");
    }

    #[test]
    fn split_keeps_a_telnet_port_off_the_ssh_default() {
        // The port only moves when the row still carries its protocol
        // default; anything else was a deliberate choice.
        let mut conn = Connection::new("sw", "admin@10.0.0.9:2323");
        conn.protocol = ConnectionProtocol::Telnet;
        conn.port = 23;
        super::split_host_field(&mut conn);
        assert_eq!(conn.hostname, "10.0.0.9");
        assert_eq!(conn.port, 2323);
    }
}

