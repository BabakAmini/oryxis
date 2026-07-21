//! `Oryxis::handle_editor`, match arms for the connection editor:
//! field changes, save/cancel/duplicate/delete, port-forwarding edits,
//! identity selection, MCP-enabled toggle, OS detection.

#![allow(clippy::result_large_err)]

use iced::Task;

use oryxis_core::models::connection::{AuthMethod, Connection, ProxyType};
use oryxis_core::models::group::Group;

use crate::app::{EditorMessage, SshMessage, Message, Oryxis};
use crate::state::{ConnectionForm, EnvVarForm, PortForwardForm, ProxyKind};

impl Oryxis {
    /// A blank connection form pre-filled with the user's new-connection
    /// defaults (agent forwarding, port, keepalive, TERM), so they don't
    /// re-set the same fields on every new host.
    /// Open the host editor for a brand-new host of the given protocol.
    /// Shared by "New host" (SSH) and "Add remote desktop" (RemoteDesktop),
    /// which differ only in the seeded protocol + default port.
    pub(crate) fn open_new_host_editor(
        &mut self,
        protocol: oryxis_core::models::connection::ConnectionProtocol,
    ) -> iced::Task<crate::app::Message> {
        // Dismiss the `…` overflow menu if it launched this.
        self.overlay = None;
        // Mutually exclusive right-panel slot, close any other panel
        // before opening the host editor.
        self.cloud_form.visible = false;
        self.cloud_dynamic_form.visible = false;
        self.cloud_discover_visible = false;
        self.show_session_group_panel = false;
        self.group_edit.visible = false;
        self.show_host_panel = true;
        self.panel_nav_clear();
        self.editor_form = self.new_connection_form();
        self.editor_form.protocol = protocol;
        if let Some(p) = protocol.default_port() {
            self.editor_form.port = p.to_string();
        }
        self.editor_initial_command = iced::widget::text_editor::Content::new();
        self.editor_startup_choice = crate::state::StartupChoice::None;
        if let Some(gid) = self.active_group
            && let Some(g) = self.groups.iter().find(|g| g.id == gid)
        {
            self.editor_form.group_name = g.label.clone();
        }
        self.host_panel_error = None;
        self.rebuild_editor_combos();
        // Land the cursor in the first field so the very first Tab keypress
        // walks the form (focus_next with nothing focused would otherwise
        // grab the grid search input).
        iced::widget::operation::focus(iced::widget::Id::new("editor-hostname"))
    }

    pub(crate) fn new_connection_form(&self) -> crate::state::ConnectionForm {
        let term = &self.setting_default_terminal_type;
        // Resolve the entity-reference defaults (identity / key / group /
        // proxy) to the label the form uses, dropping any that point at a
        // deleted entity so a stale default never blocks a new host.
        let default_identity = self.setting_default_identity_id.and_then(|id| {
            self.identities.iter().find(|i| i.id == id).map(|i| i.label.clone())
        });
        let default_key = self
            .setting_default_key_id
            .and_then(|id| self.keys.iter().find(|k| k.id == id))
            // A Certificate default only accepts cert-carrying keys (the
            // combo filters them out, so a bare default would be stuck).
            .filter(|k| {
                self.setting_default_auth_method
                    != oryxis_core::models::connection::AuthMethod::Certificate
                    || k.certificate.is_some()
            })
            .map(|k| k.label.clone());
        let default_group = self
            .setting_default_group_id
            .and_then(|id| self.groups.iter().find(|g| g.id == id).map(|g| g.label.clone()))
            .unwrap_or_default();
        // A default proxy is a saved Proxy Identity reference; inline
        // proxies are per-host by nature and aren't defaulted. Drop a
        // dangling reference (identity deleted) back to no proxy.
        let proxy_kind = self
            .setting_default_proxy_identity_id
            .filter(|id| self.proxy_identities.iter().any(|p| p.id == *id))
            .map(crate::state::ProxyKind::Identity)
            .unwrap_or(crate::state::ProxyKind::None);
        crate::state::ConnectionForm {
            agent_forwarding: self.setting_default_agent_forwarding,
            port: if self.setting_default_port.is_empty() || self.setting_default_port == "0" {
                "22".to_string()
            } else {
                self.setting_default_port.clone()
            },
            keepalive_interval: self.setting_default_keepalive.clone(),
            terminal_type: if term.is_empty() || term == "xterm-256color" {
                None
            } else {
                Some(term.clone())
            },
            username: self.setting_default_username.clone(),
            auth_method: self.setting_default_auth_method.clone(),
            selected_identity: default_identity,
            selected_key: default_key,
            group_name: default_group,
            proxy_kind,
            mcp_enabled: self.setting_default_mcp_enabled,
            monitor_enabled: false,
            encoding: self.setting_default_encoding.clone(),
            env_vars: self.setting_default_env_vars.clone(),
            ..crate::state::ConnectionForm::default()
        }
    }

    /// Rebuild the native combo_box states backing the host editor's
    /// Parent Group and Initial Command / Snippet fields. Called on
    /// editor-open.
    ///
    /// Parent Group: options are the visible (non-phantom) groups and
    /// the current `group_name` seeds the selection so an existing host
    /// pre-fills its folder. Typing / picking drives
    /// `editor_form.group_name`, so the save path (find-or-create by
    /// label) is untouched.
    ///
    /// Initial Command / Snippet: a forced-selection searchable combo.
    /// Options are the `None` / `Custom` sentinels first, then every
    /// snippet label. Picking commits via `EditorStartupChoiceChanged`;
    /// there is no free-text path (no `on_input`), so typing only
    /// filters. The current choice seeds the selection for prefill.
    pub(crate) fn rebuild_editor_combos(&mut self) {
        let visible = self.visible_group_ids();
        let mut labels: Vec<String> = self
            .groups
            .iter()
            .filter(|g| visible.contains(&g.id))
            .map(|g| g.label.clone())
            .collect();
        labels.sort_by_key(|s| s.to_lowercase());
        labels.dedup();
        let selection = self.editor_form.group_name.clone();
        let selection = (!selection.is_empty()).then_some(selection);
        self.editor_parent_combo =
            iced::widget::combo_box::State::with_selection(labels, selection.as_ref());

        self.reset_editor_startup_combo();
        self.reset_editor_key_combo();
    }

    /// Option list for the Initial Command / Snippet combo: the
    /// `None` / `Custom` sentinels first, then every snippet label.
    fn editor_startup_options(&self) -> Vec<String> {
        let mut opts: Vec<String> = vec![
            crate::i18n::t("startup_none").to_string(),
            crate::i18n::t("startup_custom").to_string(),
        ];
        for s in &self.snippets {
            opts.push(s.label.clone());
        }
        opts
    }

    /// (Re)build the startup combo with an *empty* typed value. The
    /// committed choice is shown via the widget's `selection` prop, not
    /// the internal value, so the field still displays the current pick
    /// while focusing clears the input for a fresh search over the full
    /// list. Called on editor-open and again on every focus (`on_open`)
    /// so a previous abandoned search doesn't pre-filter the list.
    pub(crate) fn reset_editor_startup_combo(&mut self) {
        self.editor_startup_combo =
            iced::widget::combo_box::State::new(self.editor_startup_options());
    }

    /// Option list for the SSH Key combo: the `(none)` sentinel first,
    /// then every saved key's label. Under `AuthMethod::Certificate`
    /// (B2.1) only keys carrying a certificate are listed, the method
    /// offers the cert and nothing else, so a bare key is never a valid
    /// pick there. Under `Agent` (B3) every key qualifies (the pick is
    /// the preferred agent identity) with security keys sorted first,
    /// they are the reason the pin exists.
    fn editor_key_options(&self) -> Vec<String> {
        use oryxis_core::models::connection::AuthMethod;
        let filter = match self.editor_form.auth_method {
            AuthMethod::Certificate => KeyComboFilter::CertificateOnly,
            AuthMethod::Agent => KeyComboFilter::SecurityKeysFirst,
            _ => KeyComboFilter::All,
        };
        key_combo_options(&self.keys, filter)
    }

    /// (Re)build the SSH Key combo with an empty typed value. Same
    /// forced-selection pattern as `reset_editor_startup_combo`: the
    /// committed key (`editor_form.selected_key`) drives the display via
    /// the widget's `selection` prop, so focusing clears the input for a
    /// fresh search while the current pick is preserved.
    pub(crate) fn reset_editor_key_combo(&mut self) {
        self.editor_key_combo =
            iced::widget::combo_box::State::new(self.editor_key_options());
    }

    /// Display label for the current startup-command choice (the
    /// `None` / `Custom` sentinels or the referenced snippet's label).
    /// Shared by the combo's selection prop and its rebuild seed; a
    /// dangling snippet id falls back to `Custom`.
    pub(crate) fn editor_startup_label(&self) -> String {
        match &self.editor_startup_choice {
            crate::state::StartupChoice::None => crate::i18n::t("startup_none").to_string(),
            crate::state::StartupChoice::Custom => crate::i18n::t("startup_custom").to_string(),
            crate::state::StartupChoice::Snippet(id) => self
                .snippets
                .iter()
                .find(|s| s.id == *id)
                .map(|s| s.label.clone())
                .unwrap_or_else(|| crate::i18n::t("startup_custom").to_string()),
        }
    }


    /// Build a `Connection` from the host-editor form: everything
    /// `EditorSave` persists except the tri-state secrets (main password,
    /// proxy password, TOTP secret), which each flow handles itself.
    /// `persist_group` gates the find-or-create group side effect; the
    /// connect-without-saving flow passes `false` so nothing is written.
    /// Errors are user-facing strings for `host_panel_error`.
    fn connection_from_editor_form(
        &mut self,
        persist_group: bool,
    ) -> Result<Connection, String> {
        let port: u16 = self.editor_form.port.parse().unwrap_or(22);

        // Find or create group. Skipped entirely for the
        // connect-without-saving flow: an ad-hoc host must not write
        // anything, not even a newly typed group.
        let group_id = if persist_group && !self.editor_form.group_name.is_empty() {
            let existing = self
                .groups
                .iter()
                .find(|g| g.label == self.editor_form.group_name);
            match existing {
                Some(g) => Some(g.id),
                None => {
                    let g = Group::new(&self.editor_form.group_name);
                    let gid = g.id;
                    if let Some(vault) = &self.vault {
                        let _ = vault.save_group(&g);
                    }
                    self.groups.push(g);
                    Some(gid)
                }
            }
        } else {
            None
        };

        // Snapshot the pre-edit Connection (when editing an
        // existing row) so we can diff the user's changes after
        // all the per-field assignments below. The diff feeds
        // `customized_fields`, which the cloud reimport flow
        // honours to leave user-edited values alone on refresh.
        let original: Option<Connection> = self
            .editor_form
            .editing_id
            .and_then(|id| self.connections.iter().find(|c| c.id == id).cloned());

        let mut conn = original
            .clone()
            .unwrap_or_else(|| Connection::new("", ""));

        conn.label = self.editor_form.label.clone();
        conn.protocol = self.editor_form.protocol;
        // Serial params are only meaningful on a Serial host; clear them
        // otherwise so a host switched away from Serial doesn't carry a
        // stale config.
        conn.serial = if self.editor_form.protocol
            == oryxis_core::models::connection::ConnectionProtocol::Serial
        {
            Some(self.editor_form.serial.unwrap_or_default())
        } else {
            None
        };
        // Remote-desktop fields: kind rides on every host (harmless
        // scalar); the SSH gateway is meaningful only for a RemoteDesktop
        // host, cleared on any other protocol.
        conn.rd_kind = self.editor_form.rd_kind;
        conn.rd_gateway_id = if self.editor_form.protocol
            == oryxis_core::models::connection::ConnectionProtocol::RemoteDesktop
        {
            self.editor_form.rd_gateway_id
        } else {
            None
        };
        // Address-family preference rides on every host (harmless scalar;
        // only the SSH dial paths read it today).
        conn.address_family = self.editor_form.address_family;
        conn.hostname = self.editor_form.hostname.clone();
        conn.port = port;
        conn.username = if self.editor_form.username.is_empty() {
            None
        } else {
            Some(self.editor_form.username.clone())
        };
        conn.auth_method = self.editor_form.auth_method.clone();
        conn.group_id = group_id;
        conn.key_id = self.editor_form.selected_key.as_ref().and_then(|label| {
            self.keys.iter().find(|k| k.label == *label).map(|k| k.id)
        });
        conn.identity_id = self.editor_form.selected_identity.as_ref().and_then(|label| {
            self.identities.iter().find(|i| i.label == *label).map(|i| i.id)
        });
        // Persist the full ordered chain. Drop any hop pointing
        // at a host that no longer exists or at this host itself
        // (a self-reference would be a connect-time loop), so a
        // stale form never writes a broken chain.
        let self_id = self.editor_form.editing_id;
        conn.jump_chain = self
            .editor_form
            .jump_chain
            .iter()
            .filter(|id| Some(**id) != self_id)
            .filter(|id| self.connections.iter().any(|c| c.id == **id))
            .copied()
            .collect();
        conn.port_forwards = self.editor_form.port_forwards.iter().filter_map(|pf| {
            let local_port = pf.local_port.parse::<u16>().ok()?;
            let remote_port = pf.remote_port.parse::<u16>().ok()?;
            if pf.remote_host.is_empty() { return None; }
            Some(oryxis_core::models::connection::PortForward {
                local_port,
                remote_host: pf.remote_host.clone(),
                remote_port,
            })
        }).collect();
        // Env vars: keep rows with a non-empty key (value may be
        // empty); trim the key so accidental whitespace doesn't
        // create a bogus variable name.
        conn.env_vars = self.editor_form.env_vars.iter().filter_map(|e| {
            let key = e.key.trim();
            if key.is_empty() { return None; }
            Some(oryxis_core::models::connection::EnvVar {
                key: key.to_string(),
                value: e.value.clone(),
            })
        }).collect();
        // MCP exposure is SSH-only (the handler resolves through the SSH
        // engine). The reduced Telnet/Serial editor hides the toggle, so
        // clamp here too: a host switched away from SSH must not stay
        // MCP-advertised. `list_mcp_connections` filters by protocol as
        // the source-of-truth guard for synced / imported hosts.
        conn.mcp_enabled = self.editor_form.protocol
            == oryxis_core::models::connection::ConnectionProtocol::Ssh
            && self.editor_form.mcp_enabled;
        // Same SSH clamp as `mcp_enabled`: monitoring reads /proc over an
        // SSH exec channel, so a host switched to Telnet / serial /
        // remote-desktop can't stay monitored.
        conn.monitor_enabled = self.editor_form.protocol
            == oryxis_core::models::connection::ConnectionProtocol::Ssh
            && self.editor_form.monitor_enabled;
        // Opting the host OUT drops its series right away: the status
        // bar / sidebar must not keep painting the last sample as if it
        // were live, and an in-flight probe must land dead (stamp bump).
        if original.as_ref().is_some_and(|o| o.monitor_enabled) && !conn.monitor_enabled {
            self.monitor_reset_host(&conn.id);
        }
        conn.agent_forwarding = self.editor_form.agent_forwarding;
        conn.session_logging = self.editor_form.session_logging;
        conn.terminal_theme = self.editor_form.terminal_theme.clone();
        conn.icon_style = self.editor_form.icon_style.clone();
        conn.encoding = self.editor_form.encoding.clone();
        conn.terminal_type = self.editor_form.terminal_type.clone();
        conn.ciphers = self.editor_form.ciphers.clone();
        conn.kex = self.editor_form.kex.clone();
        conn.macs = self.editor_form.macs.clone();
        conn.host_key_algorithms = self.editor_form.host_key_algorithms.clone();
        // Startup command source. Snippet -> store the live id and
        // clear the literal; Custom -> store the trimmed text (empty
        // == None); None -> clear both. `.text()` appends a trailing
        // newline, so trim before checking.
        match &self.editor_startup_choice {
            crate::state::StartupChoice::Snippet(id) => {
                conn.startup_snippet_id = Some(*id);
                conn.initial_command = None;
            }
            crate::state::StartupChoice::Custom => {
                conn.startup_snippet_id = None;
                let initial_command = self.editor_initial_command.text();
                conn.initial_command = if initial_command.trim().is_empty() {
                    None
                } else {
                    Some(initial_command.trim_end().to_string())
                };
            }
            crate::state::StartupChoice::None => {
                conn.startup_snippet_id = None;
                conn.initial_command = None;
            }
        }
        // If the host is cloud-imported (carries a cloud_ref)
        // and the user picked a transport in the editor,
        // persist it onto the existing CloudRef. Don't touch
        // anything else (resource_id, region, profile_id).
        if let Some(picked) = self.editor_form.cloud_transport
            && let Some(cref) = conn.cloud_ref.as_mut()
        {
            cref.transport_pref = picked;
        }
        // Empty string == inherit global; "0" == explicitly disabled
        // on this host; positive integer == per-host override.
        conn.keepalive_interval = if self.editor_form.keepalive_interval.is_empty() {
            None
        } else {
            self.editor_form.keepalive_interval.parse::<u32>().ok()
        };
        conn.auto_title = self.editor_form.auto_title;
        conn.tags = crate::util::parse_tags(&self.editor_form.tags_text);
        conn.privacy_mode = self.editor_form.privacy_mode;
        conn.sidebar_auto_open = self.editor_form.sidebar_auto_open;
        // C5: store quirks only when they differ from the xterm default,
        // so an untouched host keeps `quirks = None` (old-payload parity).
        conn.quirks = (self.editor_form.quirks
            != oryxis_core::models::terminal_quirks::TerminalQuirks::default())
        .then_some(self.editor_form.quirks);
        conn.rekey_limit_mb = self
            .editor_form
            .rekey_limit_mb
            .trim()
            .parse::<u32>()
            .ok()
            .filter(|&n| n > 0);
        // Map the editor form into either an inline ProxyConfig
        // or a `proxy_identity_id` reference. Validates host /
        // port / command up-front so the user gets an error
        // instead of a silently-broken proxy entry.
        let proxy_resolution = build_proxy_resolution(&self.editor_form)?;
        conn.proxy = proxy_resolution.proxy;
        conn.proxy_identity_id = proxy_resolution.proxy_identity_id;
        conn.updated_at = chrono::Utc::now();

        // Track user edits on cloud-imported hosts so the next
        // refresh from AWS doesn't clobber them. Only the
        // fields that discovery actually pushes are tracked,
        // anything else (port, color, group_id, ...) is fully
        // user-controlled on imported hosts already and doesn't
        // need a flag.
        if conn.cloud_ref.is_some()
            && let Some(orig) = &original
        {
            let mut customized = conn.customized_fields.clone();
            let mark = |list: &mut Vec<String>, name: &str| {
                if !list.iter().any(|s| s == name) {
                    list.push(name.to_string());
                }
            };
            if conn.label != orig.label {
                mark(&mut customized, "label");
            }
            if conn.hostname != orig.hostname {
                mark(&mut customized, "hostname");
            }
            if conn.username != orig.username {
                mark(&mut customized, "username");
            }
            conn.customized_fields = customized;
        }
        // Validate a newly typed TOTP secret before anything is
        // written, so a typo'd secret can't be stored and then
        // silently fail at connect time. Cleared/untouched skip.
        if let Some(secret) = self.editor_form.totp_secret.resolve()
            && !secret.trim().is_empty()
            && let Err(e) = oryxis_core::totp::Totp::parse(secret)
        {
            return Err(format!("{}: {e}", crate::i18n::t("totp_invalid")));
        }

        Ok(conn)
    }


    /// Build a fully populated `ConnectionForm` from an existing
    /// `Connection` (labels resolved against the current groups / keys /
    /// identities lists). Secrets are never prefilled: the `has_*` flags
    /// drive the masked placeholders and the `SecretInput` tri-state
    /// decides what a later save writes. Shared by `EditConnection`
    /// (vault hosts)
    /// and `SaveQuickHost` (ad-hoc hosts being persisted).
    fn form_from_connection(
        &self,
        conn: &Connection,
        has_pw: bool,
        has_proxy_pw: bool,
        has_totp: bool,
    ) -> ConnectionForm {
        ConnectionForm {
            label: conn.label.clone(),
            protocol: conn.protocol,
            serial: conn.serial,
            rd_kind: conn.rd_kind,
            rd_gateway_id: conn.rd_gateway_id,
            address_family: conn.address_family,
            quick_flow: false,
            hostname: conn.hostname.clone(),
            port: conn.port.to_string(),
            username: conn.username.clone().unwrap_or_default(),
            // Never pre-fill the connection password: an untouched
            // SecretInput resolves to None (preserve on save).
            password: Default::default(),
            auth_method: conn.auth_method.clone(),
            group_name: conn
                .group_id
                .and_then(|gid| {
                    self.groups.iter().find(|g| g.id == gid).map(|g| g.label.clone())
                })
                .unwrap_or_default(),
            selected_key: conn.key_id.and_then(|kid| {
                self.keys.iter().find(|k| k.id == kid).map(|k| k.label.clone())
            }),
            jump_chain: conn.jump_chain.clone(),
            selected_identity: conn.identity_id.and_then(|iid| {
                self.identities.iter().find(|i| i.id == iid).map(|i| i.label.clone())
            }),
            editing_id: Some(conn.id),
            has_existing_password: has_pw,
            password_visible: false,
            username_focused: false,
            port_forwards: conn.port_forwards.iter().map(|pf| PortForwardForm {
                local_port: pf.local_port.to_string(),
                remote_host: pf.remote_host.clone(),
                remote_port: pf.remote_port.to_string(),
            }).collect(),
            env_vars: conn.env_vars.iter().map(|e| crate::state::EnvVarForm {
                key: e.key.clone(),
                value: e.value.clone(),
            }).collect(),
            mcp_enabled: conn.mcp_enabled,
            monitor_enabled: conn.monitor_enabled,
            agent_forwarding: conn.agent_forwarding,
            session_logging: conn.session_logging,
            // Saved-identity reference takes precedence over
            // an inline proxy when both are populated, mirroring
            // the runtime resolver in `Vault::resolve_proxy`.
            proxy_kind: if let Some(pid) = conn.proxy_identity_id {
                ProxyKind::Identity(pid)
            } else {
                conn.proxy.as_ref().map(|p| match &p.proxy_type {
                    ProxyType::Socks5 => ProxyKind::Socks5,
                    ProxyType::Socks4 => ProxyKind::Socks4,
                    ProxyType::Http => ProxyKind::Http,
                    ProxyType::Command(_) => ProxyKind::Command,
                }).unwrap_or(ProxyKind::None)
            },
            proxy_host: conn.proxy.as_ref().map(|p| p.host.clone()).unwrap_or_default(),
            proxy_port: conn.proxy.as_ref().map(|p| p.port.to_string()).unwrap_or_default(),
            proxy_username: conn.proxy.as_ref().and_then(|p| p.username.clone()).unwrap_or_default(),
            // Never pre-fill proxy_password from the encrypted vault, keep it
            // empty and untouched so save preserves the stored value,
            // mirroring the main connection-password flow.
            proxy_password: Default::default(),
            proxy_command: conn.proxy.as_ref().and_then(|p| match &p.proxy_type {
                ProxyType::Command(cmd) => Some(cmd.clone()),
                _ => None,
            }).unwrap_or_default(),
            has_existing_proxy_password: has_proxy_pw,
            // Never pre-fill the TOTP secret either; the
            // masked placeholder signals one is stored.
            totp_secret: Default::default(),
            has_existing_totp: has_totp,
            totp_visible: false,
            terminal_theme: conn.terminal_theme.clone(),
            keepalive_interval: conn
                .keepalive_interval
                .map(|n| n.to_string())
                .unwrap_or_default(),
            auto_title: conn.auto_title,
            tags_text: conn.tags.join(", "),
            cloud_transport: conn
                .cloud_ref
                .as_ref()
                .map(|r| r.transport_pref),
            icon_style: conn.icon_style.clone(),
            encoding: conn.encoding.clone(),
            terminal_type: conn.terminal_type.clone(),
            ciphers: conn.ciphers.clone(),
            kex: conn.kex.clone(),
            macs: conn.macs.clone(),
            host_key_algorithms: conn.host_key_algorithms.clone(),
            privacy_mode: conn.privacy_mode,
            sidebar_auto_open: conn.sidebar_auto_open,
            quirks: conn.quirks.unwrap_or_default(),
            rekey_limit_mb: conn
                .rekey_limit_mb
                .map(|n| n.to_string())
                .unwrap_or_default(),
        }
    }

    pub(crate) fn handle_editor(
        &mut self,
        message: EditorMessage,
    ) -> Task<Message> {
        match message {
            EditorMessage::EditorToggleMcpEnabled => {
                self.editor_form.mcp_enabled = !self.editor_form.mcp_enabled;
            }
            EditorMessage::EditorToggleMonitorEnabled => {
                self.editor_form.monitor_enabled = !self.editor_form.monitor_enabled;
            }
            EditorMessage::EditorToggleAgentForwarding => {
                self.editor_form.agent_forwarding = !self.editor_form.agent_forwarding;
            }
            // Cycle the per-host recording override: Default (inherit the
            // global setting) -> On -> Off -> Default.
            EditorMessage::EditorCycleSessionLogging => {
                self.editor_form.session_logging = match self.editor_form.session_logging {
                    None => Some(true),
                    Some(true) => Some(false),
                    Some(false) => None,
                };
            }
            EditorMessage::EditorAddPortForward => {
                self.editor_form.port_forwards.push(PortForwardForm::default());
            }
            EditorMessage::EditorRemovePortForward(i) => {
                if i < self.editor_form.port_forwards.len() {
                    self.editor_form.port_forwards.remove(i);
                }
            }
            EditorMessage::EditorPortFwdLocalPortChanged(i, v) => {
                if let Some(pf) = self.editor_form.port_forwards.get_mut(i) {
                    pf.local_port = v;
                }
            }
            EditorMessage::EditorPortFwdRemoteHostChanged(i, v) => {
                if let Some(pf) = self.editor_form.port_forwards.get_mut(i) {
                    pf.remote_host = v;
                }
            }
            EditorMessage::EditorPortFwdRemotePortChanged(i, v) => {
                if let Some(pf) = self.editor_form.port_forwards.get_mut(i) {
                    pf.remote_port = v;
                }
            }
            EditorMessage::EditorAddEnvVar => {
                self.editor_form.env_vars.push(EnvVarForm::default());
            }
            EditorMessage::EditorRemoveEnvVar(i) => {
                if i < self.editor_form.env_vars.len() {
                    self.editor_form.env_vars.remove(i);
                }
            }
            EditorMessage::EditorEnvVarKeyChanged(i, v) => {
                if let Some(e) = self.editor_form.env_vars.get_mut(i) {
                    e.key = v;
                }
            }
            EditorMessage::EditorEnvVarValueChanged(i, v) => {
                if let Some(e) = self.editor_form.env_vars.get_mut(i) {
                    e.value = v;
                }
            }
            // -- Connection editor --
            EditorMessage::ShowNewConnection => {
                return self.open_new_host_editor(
                    oryxis_core::models::connection::ConnectionProtocol::Ssh,
                );
            }
            EditorMessage::ShowNewRemoteDesktop => {
                return self.open_new_host_editor(
                    oryxis_core::models::connection::ConnectionProtocol::RemoteDesktop,
                );
            }
            EditorMessage::EditConnection(idx) => {
                self.card_context_menu = None;
                self.overlay = None;
                if let Some(conn) = self.connections.get(idx) {
                    // Mutually exclusive right-panel slot.
                    self.cloud_form.visible = false;
                    self.cloud_dynamic_form.visible = false;
                    self.cloud_discover_visible = false;
                    self.show_session_group_panel = false;
                    self.group_edit.visible = false;
                    self.show_host_panel = true;
                    // When invoked from a focused terminal tab (the
                    // OpenPortForwards / edit-host hotkey), leave the
                    // terminal surface so the right-panel editor actually
                    // renders: it only shows when no tab is focused.
                    // Without this the flag sticks true and silently
                    // disables Ctrl+Tab MRU, IME routing and sidebar
                    // keynav. The tab keeps running in the background.
                    if self.active_tab.is_some() {
                        self.active_tab = None;
                        self.active_view = crate::state::View::Dashboard;
                    }
                    // Inline panel_nav_clear: a method call would
                    // borrow all of self while `conn` holds it.
                    self.keynav.panel_selected = None;
                    self.keynav.panel_last_row.set(None);
                    self.host_panel_error = None;
                    let has_pw = self.vault.as_ref()
                        .and_then(|v| v.get_connection_password(&conn.id).ok())
                        .flatten()
                        .is_some();
                    let has_proxy_pw = self.vault.as_ref()
                        .and_then(|v| v.get_proxy_password(&conn.id).ok())
                        .flatten()
                        .is_some();
                    let has_totp = self.vault.as_ref()
                        .and_then(|v| v.get_connection_totp_secret(&conn.id).ok())
                        .flatten()
                        .is_some();
                    self.editor_form =
                        self.form_from_connection(conn, has_pw, has_proxy_pw, has_totp);
                    let cmd = conn.initial_command.as_deref().unwrap_or_default();
                    self.editor_initial_command =
                        iced::widget::text_editor::Content::with_text(cmd);
                    // Recover the startup source: a live snippet reference
                    // (whose snippet still exists) wins; else a non-empty
                    // literal command is Custom; else None. A dangling
                    // snippet id falls back to None.
                    self.editor_startup_choice = match conn.startup_snippet_id {
                        Some(id) if self.snippets.iter().any(|s| s.id == id) => {
                            crate::state::StartupChoice::Snippet(id)
                        }
                        _ if !cmd.trim().is_empty() => crate::state::StartupChoice::Custom,
                        _ => crate::state::StartupChoice::None,
                    };
                    self.rebuild_editor_combos();
                    return iced::widget::operation::focus(iced::widget::Id::new(
                        "editor-hostname",
                    ));
                }
            }
            EditorMessage::SaveQuickHost(id) => {
                self.overlay = None;
                self.card_context_menu = None;
                let Some(entry) = self.quick_connects.get(&id).cloned() else {
                    return Task::none();
                };
                // Mutually exclusive right-panel slot, and the panel lives
                // on the dashboard (the menu was clicked from a terminal).
                self.cloud_form.visible = false;
                self.cloud_dynamic_form.visible = false;
                self.cloud_discover_visible = false;
                self.show_session_group_panel = false;
                self.group_edit.visible = false;
                self.show_host_panel = true;
                self.panel_nav_clear();
                self.host_panel_error = None;
                self.active_view = crate::state::View::Dashboard;
                let mut form = self.form_from_connection(&entry.conn, false, false, false);
                // Prefill as a NEW host: saving must insert a fresh row,
                // never overwrite; the open tab stays ephemeral until its
                // next reconnect.
                form.editing_id = None;
                // Re-seed the credentials typed in the editor flow so the
                // save persists them (set marks touched => tri-state writes).
                if let Some(pw) = entry.password.clone() {
                    form.password.set(pw);
                }
                if let Some(secret) = entry.totp_secret.clone() {
                    form.totp_secret.set(secret);
                }
                if let Some(pw) = entry.proxy_password.clone() {
                    form.proxy_password.set(pw);
                }
                self.editor_form = form;
                let cmd = entry.conn.initial_command.as_deref().unwrap_or_default();
                self.editor_initial_command =
                    iced::widget::text_editor::Content::with_text(cmd);
                self.editor_startup_choice = match entry.conn.startup_snippet_id {
                    Some(sid) if self.snippets.iter().any(|s| s.id == sid) => {
                        crate::state::StartupChoice::Snippet(sid)
                    }
                    _ if !cmd.trim().is_empty() => crate::state::StartupChoice::Custom,
                    _ => crate::state::StartupChoice::None,
                };
                self.rebuild_editor_combos();
                return iced::widget::operation::focus(iced::widget::Id::new(
                    "editor-hostname",
                ));
            }
            EditorMessage::EditQuickHost(id) => {
                // Same prefill as SaveQuickHost; the flag only swaps the
                // footer emphasis (Connect primary / Save secondary) so
                // the flow reads as "edit the temporary host and dial",
                // never an implicit vault write.
                let task = self.update(Message::Editor(EditorMessage::SaveQuickHost(id)));
                if self.show_host_panel {
                    self.editor_form.quick_flow = true;
                }
                return task;
            }
            EditorMessage::EditorLabelChanged(v) => { self.editor_form.label = v; self.editor_form.username_focused = false; }
            EditorMessage::EditorTagsChanged(v) => { self.editor_form.tags_text = v; }
            EditorMessage::EditorHostnameChanged(v) => { self.editor_form.hostname = v; self.editor_form.username_focused = false; }
            EditorMessage::EditorProtocolChanged(protocol) => {
                let prev = self.editor_form.protocol;
                if prev != protocol {
                    // Retarget the numeric port only when both protocols
                    // use one AND the field still holds the old default,
                    // so a user-typed port survives the switch untouched.
                    // Serial has no numeric port (`None`), so switching
                    // to/from it leaves the field alone (it's hidden).
                    if let (Some(prev_port), Some(new_port)) =
                        (prev.default_port(), protocol.default_port())
                        && self.editor_form.port.trim() == prev_port.to_string()
                    {
                        self.editor_form.port = new_port.to_string();
                    }
                    // Materialize serial defaults the first time a host
                    // becomes Serial so the reduced form has values to
                    // show (9600 8N1).
                    if protocol == oryxis_core::models::connection::ConnectionProtocol::Serial
                        && self.editor_form.serial.is_none()
                    {
                        self.editor_form.serial =
                            Some(oryxis_core::models::serial::SerialParams::default());
                    }
                    self.editor_form.protocol = protocol;
                }
                self.editor_form.username_focused = false;
            }
            EditorMessage::EditorSerialBaudChanged(v) => {
                self.editor_form.serial.get_or_insert_with(Default::default).baud = v;
            }
            EditorMessage::EditorSerialDataBitsChanged(v) => {
                self.editor_form.serial.get_or_insert_with(Default::default).data_bits = v;
            }
            EditorMessage::EditorSerialParityChanged(v) => {
                self.editor_form.serial.get_or_insert_with(Default::default).parity = v;
            }
            EditorMessage::EditorSerialStopBitsChanged(v) => {
                self.editor_form.serial.get_or_insert_with(Default::default).stop_bits = v;
            }
            EditorMessage::EditorSerialFlowChanged(v) => {
                self.editor_form.serial.get_or_insert_with(Default::default).flow_control = v;
            }
            EditorMessage::EditorSerialLineEndingChanged(v) => {
                self.editor_form.serial.get_or_insert_with(Default::default).line_ending = v;
            }
            EditorMessage::EditorSerialLocalEchoToggled => {
                let s = self.editor_form.serial.get_or_insert_with(Default::default);
                s.local_echo = !s.local_echo;
            }
            EditorMessage::EditorRdKindChanged(kind) => {
                // Retarget the port field when it still holds the other
                // kind's default, so a typed port survives the RDP<->VNC
                // switch (the endpoint port reuses the normal port field).
                let old_default = self.editor_form.rd_kind.default_port().to_string();
                if self.editor_form.port.trim() == old_default {
                    self.editor_form.port = kind.default_port().to_string();
                }
                self.editor_form.rd_kind = kind;
            }
            EditorMessage::EditorRdGatewayChanged(id) => {
                self.editor_form.rd_gateway_id = id;
            }
            EditorMessage::EditorAddressFamilyChanged(family) => {
                self.editor_form.address_family = family;
            }
            EditorMessage::EditorPortChanged(v) => { self.editor_form.port = v; self.editor_form.username_focused = false; }
            EditorMessage::EditorUsernameChanged(v) => {
                self.editor_form.username = v;
                self.editor_form.username_focused = true;
            }
            EditorMessage::EditorPasswordChanged(v) => {
                self.editor_form.username_focused = false;
                self.editor_form.password.set(v);
            }
            EditorMessage::EditorTogglePasswordVisibility => {
                self.editor_form.password_visible = !self.editor_form.password_visible;
            }
            EditorMessage::EditorTotpChanged(v) => {
                self.editor_form.username_focused = false;
                self.editor_form.totp_secret.set(v);
            }
            EditorMessage::EditorToggleTotpVisibility => {
                self.editor_form.totp_visible = !self.editor_form.totp_visible;
            }
            EditorMessage::EditorAuthMethodChanged(v) => {
                // Localized (or English) label -> enum, shared with the
                // Settings default-auth picker.
                self.editor_form.auth_method = crate::util::auth_method_from_label(&v);
                // Certificate lists only keys that carry a cert: drop a
                // selection that is no longer offerable and rebuild the
                // combo with the filtered (or restored) option list.
                if self.editor_form.auth_method == AuthMethod::Certificate
                    && let Some(sel) = self.editor_form.selected_key.as_deref()
                    && !self
                        .keys
                        .iter()
                        .any(|k| k.label == sel && k.certificate.is_some())
                {
                    self.editor_form.selected_key = None;
                }
                self.reset_editor_key_combo();
            }
            EditorMessage::EditorGroupChanged(v) => self.editor_form.group_name = v,
            EditorMessage::EditorKeyChanged(v) => {
                self.editor_form.selected_key = if v == "(none)" { None } else { Some(v) };
            }
            EditorMessage::EditorKeyComboOpened => {
                // Focus clears the typed value so the dropdown opens on
                // the full key list, not pre-filtered by the current pick.
                self.reset_editor_key_combo();
            }
            EditorMessage::OpenChainEditor => {
                self.show_chain_editor = true;
                self.chain_editor_adding = false;
                self.chain_editor_search.clear();
            }
            EditorMessage::CloseChainEditor => {
                self.show_chain_editor = false;
                self.chain_editor_adding = false;
                self.chain_editor_search.clear();
            }
            EditorMessage::ChainEditorStartAdd => {
                self.chain_editor_adding = true;
                self.chain_editor_search.clear();
            }
            EditorMessage::ChainEditorCancelAdd => {
                self.chain_editor_adding = false;
                self.chain_editor_search.clear();
            }
            EditorMessage::ChainEditorSearchChanged(v) => {
                self.chain_editor_search = v;
            }
            EditorMessage::ChainEditorAddHop(id) => {
                // Append the hop, ignoring duplicates so the same host
                // can't appear twice in one chain.
                if !self.editor_form.jump_chain.contains(&id) {
                    self.editor_form.jump_chain.push(id);
                }
                self.chain_editor_adding = false;
                self.chain_editor_search.clear();
            }
            EditorMessage::ChainEditorRemoveHop(idx) => {
                if idx < self.editor_form.jump_chain.len() {
                    self.editor_form.jump_chain.remove(idx);
                }
            }
            EditorMessage::ChainEditorMoveHopUp(idx) => {
                if idx > 0 && idx < self.editor_form.jump_chain.len() {
                    self.editor_form.jump_chain.swap(idx, idx - 1);
                }
            }
            EditorMessage::ChainEditorMoveHopDown(idx) => {
                if idx + 1 < self.editor_form.jump_chain.len() {
                    self.editor_form.jump_chain.swap(idx, idx + 1);
                }
            }
            EditorMessage::EditorProxyKindChanged(kind) => {
                let prev = self.editor_form.proxy_kind;
                self.editor_form.proxy_kind = kind;
                match kind {
                    ProxyKind::Identity(_) => {
                        // Switching to a saved identity, wipe inline state
                        // so a later switch back to Custom starts clean.
                        // The identity carries its own host/port/username/
                        // password, all hydrated by `resolve_proxy` at
                        // connect time.
                        self.editor_form.proxy_host.clear();
                        self.editor_form.proxy_port.clear();
                        self.editor_form.proxy_username.clear();
                        // SecretInput::clear also drops the touched flag,
                        // back to "preserve the stored value".
                        self.editor_form.proxy_password.clear();
                        self.editor_form.proxy_command.clear();
                    }
                    _ => {
                        // Coming back from an Identity selection: empty
                        // form, fall through to default-port pre-fill.
                        if matches!(prev, ProxyKind::Identity(_)) {
                            self.editor_form.proxy_host.clear();
                            self.editor_form.proxy_port.clear();
                            self.editor_form.proxy_username.clear();
                            self.editor_form.proxy_password.clear();
                            self.editor_form.proxy_command.clear();
                        }
                        // Pre-fill the canonical port for the chosen type
                        // when the field is still blank, saves the user a
                        // hop and is easy to override by typing.
                        if self.editor_form.proxy_port.is_empty()
                            && let Some(default_port) = kind.default_port()
                        {
                            self.editor_form.proxy_port = default_port.to_string();
                        }
                    }
                }
            }
            EditorMessage::EditorProxyHostChanged(v) => { self.editor_form.proxy_host = v; }
            EditorMessage::EditorProxyPortChanged(v) => { self.editor_form.proxy_port = v; }
            EditorMessage::EditorProxyUsernameChanged(v) => { self.editor_form.proxy_username = v; }
            EditorMessage::EditorProxyPasswordChanged(v) => {
                self.editor_form.proxy_password.set(v);
            }
            EditorMessage::EditorProxyCommandChanged(v) => { self.editor_form.proxy_command = v; }
            EditorMessage::EditorOpenThemePicker => {
                self.show_theme_picker = true;
            }
            EditorMessage::EditorCloseThemePicker => {
                self.show_theme_picker = false;
            }
            EditorMessage::EditorTerminalThemeChanged(name) => {
                // Empty string == "inherit the global pick".
                self.editor_form.terminal_theme =
                    if name.is_empty() { None } else { Some(name) };
                self.show_theme_picker = false;
            }
            EditorMessage::EditorCloudTransportChanged(t) => {
                self.editor_form.cloud_transport = Some(t);
            }
            EditorMessage::EditorInitialCommandChanged(action) => {
                self.editor_initial_command.perform(action);
            }
            EditorMessage::EditorStartupComboOpened => {
                // Focus clears the typed value so the dropdown opens on
                // the full snippet list, not pre-filtered by the current
                // selection (the committed choice is preserved untouched).
                self.reset_editor_startup_combo();
            }
            EditorMessage::EditorStartupChoiceChanged(label) => {
                use crate::state::StartupChoice;
                // Map the picker label back to a source. The None / Custom
                // sentinels come from i18n; anything else is a snippet
                // label. A snippet is stored as a live reference (its id),
                // resolved to the snippet body at connect time, so we
                // don't copy the body into the custom text editor here.
                if label == crate::i18n::t("startup_none") {
                    self.editor_startup_choice = StartupChoice::None;
                    self.editor_initial_command =
                        iced::widget::text_editor::Content::new();
                } else if label == crate::i18n::t("startup_custom") {
                    self.editor_startup_choice = StartupChoice::Custom;
                } else if let Some(s) =
                    self.snippets.iter().find(|s| s.label == label)
                {
                    self.editor_startup_choice = StartupChoice::Snippet(s.id);
                }
            }
            EditorMessage::EditorIconStyleChanged(v) => {
                // "" clears the override; anything else is normalized to
                // the known set so a stale UI value can't smuggle in a
                // string the renderer doesn't understand.
                self.editor_form.icon_style = match v.as_str() {
                    "circular" | "square" | "rounded" | "outline" | "initials" => Some(v),
                    _ => None,
                };
            }
            EditorMessage::EditorEncodingChanged(v) => {
                // "UTF-8" is the implicit default, stored as None so the
                // SSH engine skips transcoding entirely.
                self.editor_form.encoding = if v == "UTF-8" { None } else { Some(v) };
            }
            EditorMessage::EditorTerminalTypeChanged(v) => {
                // "xterm-256color" is the implicit default, stored as None.
                self.editor_form.terminal_type =
                    if v == "xterm-256color" { None } else { Some(v) };
            }
            EditorMessage::EditorKeepaliveChanged(v) => {
                // Digits only; preserve empty (= inherit global). Cap at
                // 86_400s (1 day) like the global setting field, so users
                // can't accidentally type a runaway value.
                let digits: String = v.chars().filter(|c| c.is_ascii_digit()).collect();
                self.editor_form.keepalive_interval = if digits.is_empty() {
                    String::new()
                } else {
                    let n: u64 = digits.parse().unwrap_or(86_400);
                    n.min(86_400).to_string()
                };
            }
            EditorMessage::EditorAutoTitleChanged(v) => {
                use crate::i18n::t;
                // Map the localized pick label back to the tri-state override.
                self.editor_form.auto_title = if v == t("host_auto_title_show") {
                    Some(true)
                } else if v == t("host_auto_title_hide") {
                    Some(false)
                } else {
                    None
                };
            }
            EditorMessage::EditorPrivacyModeChanged(v) => {
                use crate::i18n::t;
                // Map the localized pick label back to the tri-state override.
                self.editor_form.privacy_mode = if v == t("host_privacy_mode_on") {
                    Some(true)
                } else if v == t("host_privacy_mode_off") {
                    Some(false)
                } else {
                    None
                };
            }
            EditorMessage::EditorSidebarAutoOpenChanged(v) => {
                use crate::i18n::t;
                // Same localized-label mapping as the privacy row above.
                self.editor_form.sidebar_auto_open = if v == t("host_privacy_mode_on") {
                    Some(true)
                } else if v == t("host_privacy_mode_off") {
                    Some(false)
                } else {
                    None
                };
            }
            EditorMessage::EditorQuirkBackspaceChanged(v) => {
                self.editor_form.quirks.backspace = crate::util::quirk_backspace_from_label(&v);
            }
            EditorMessage::EditorQuirkHomeEndChanged(v) => {
                self.editor_form.quirks.home_end = crate::util::quirk_home_end_from_label(&v);
            }
            EditorMessage::EditorQuirkFnKeysChanged(v) => {
                self.editor_form.quirks.function_keys = crate::util::quirk_fn_keys_from_label(&v);
            }
            EditorMessage::EditorQuirkMouseReportingChanged(on) => {
                // Toggle shows the positive "report mouse"; off disables it.
                self.editor_form.quirks.disable_mouse_reporting = !on;
            }
            EditorMessage::EditorQuirkTitleChangeChanged(on) => {
                self.editor_form.quirks.disable_title_change = !on;
            }
            EditorMessage::EditorQuirkOsc52Changed(v) => {
                self.editor_form.quirks.osc52 = crate::util::quirk_osc52_from_label(&v);
            }
            EditorMessage::EditorQuirkOptionAsMetaChanged(v) => {
                self.editor_form.quirks.option_as_meta =
                    crate::util::quirk_option_as_meta_from_label(&v);
            }
            EditorMessage::EditorQuirkRekeyChanged(v) => {
                // Digits only; empty allowed (= default). Clamp to russh's
                // 1 GiB cap (1024 MB) so the field can't exceed it.
                self.editor_form.rekey_limit_mb = if v.trim().is_empty() {
                    String::new()
                } else {
                    crate::util::sanitize_uint(&v, 1024)
                };
            }
            EditorMessage::EditorAlgoSetAuto(cat, auto) => {
                // Auto = None (russh defaults). Switching to custom seeds the
                // list with the safe defaults so the user adds legacy entries
                // (or trims) from a working set rather than from nothing.
                *self.editor_form.algo_list_mut(cat) = if auto {
                    None
                } else {
                    Some(cat.defaults())
                };
            }
            EditorMessage::EditorAlgoToggle(cat, name) => {
                let list = self.editor_form.algo_list_mut(cat).get_or_insert_with(Vec::new);
                if let Some(pos) = list.iter().position(|n| n == &name) {
                    list.remove(pos);
                } else {
                    list.push(name);
                }
            }
            EditorMessage::EditorSave => {
                if self.editor_form.label.is_empty() || self.editor_form.hostname.is_empty() {
                    self.host_panel_error =
                        Some(crate::i18n::t("editor_label_host_required").to_string());
                    return Task::none();
                }
                let conn = match self.connection_from_editor_form(true) {
                    Ok(conn) => conn,
                    Err(msg) => {
                        self.host_panel_error = Some(msg);
                        return Task::none();
                    }
                };
                // Tri-state: untouched preserves the stored password,
                // cleared removes it, typed stores (SecretInput::resolve).
                let password = self.editor_form.password.resolve();

                if let Some(vault) = &self.vault {
                    match vault.save_connection(&conn, password) {
                        Ok(()) => {
                            // Persist the encrypted proxy password in its own
                            // column. We only touch it when the user edited
                            // the field (resolve returns Some), mirroring the
                            // main connection password; an edited-empty field
                            // maps to None = remove for this setter.
                            if let Some(pw) = self.editor_form.proxy_password.resolve() {
                                let _ = vault.set_proxy_password(
                                    &conn.id,
                                    (!pw.is_empty()).then_some(pw),
                                );
                            }
                            // If the proxy was disabled in this save, drop any
                            // previously stored proxy password, keeping a
                            // dangling encrypted credential would be surprising.
                            if conn.proxy.is_none() {
                                let _ = vault.set_proxy_password(&conn.id, None);
                            }
                            // TOTP secret, same touched tri-state as the
                            // proxy password (empty input clears). TOTP is
                            // SSH-only (keyboard-interactive 2FA); if the
                            // protocol was switched to Telnet/Serial/RDP the
                            // field is hidden, so clear any secret rather than
                            // persisting dead credential material, mirroring
                            // the `mcp_enabled` SSH clamp above.
                            let is_ssh = self.editor_form.protocol
                                == oryxis_core::models::connection::ConnectionProtocol::Ssh;
                            if !is_ssh {
                                let _ = vault.set_connection_totp_secret(&conn.id, None);
                            } else if let Some(secret) =
                                self.editor_form.totp_secret.resolve()
                            {
                                let s = secret.trim();
                                let s = (!s.is_empty()).then_some(s);
                                let _ = vault.set_connection_totp_secret(&conn.id, s);
                            }
                            self.show_host_panel = false;
                            self.panel_nav_clear();
                            self.host_panel_error = None;
                            // Re-paint any open tabs of this host so a
                            // newly chosen palette takes effect without
                            // a reconnect.
                            let host_label = conn.label.clone();
                            self.load_data_from_vault();
                            self.repaint_terminal_palettes_for_label(&host_label);
                        }
                        Err(e) => {
                            self.host_panel_error = Some(e.to_string());
                        }
                    }
                }
            }
            EditorMessage::EditorConnectWithoutSaving => {
                // Ad-hoc connect from the "+ Host" flow: build the full
                // Connection from the form but persist nothing. Only the
                // hostname is required; an empty label defaults to the
                // canonical user@host[:port].
                if self.editor_form.hostname.is_empty() {
                    self.host_panel_error =
                        Some(crate::i18n::t("quick_connect_hostname_required").into());
                    return Task::none();
                }
                let mut conn = match self.connection_from_editor_form(false) {
                    Ok(conn) => conn,
                    Err(msg) => {
                        self.host_panel_error = Some(msg);
                        return Task::none();
                    }
                };
                if conn.label.is_empty() {
                    conn.label = oryxis_core::ssh_target::SshTarget {
                        username: conn.username.clone(),
                        host: conn.hostname.clone(),
                        port: (conn.port != 22).then_some(conn.port),
                    }
                    .canonical();
                }
                // Typed credentials ride the ephemeral entry (there is no
                // vault row to hydrate from at connect time). Untouched or
                // cleared fields stay None.
                let form = &self.editor_form;
                let password = form
                    .password
                    .resolve()
                    .filter(|pw| !pw.is_empty())
                    .map(str::to_string);
                let totp_secret = form
                    .totp_secret
                    .resolve()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string);
                let proxy_password = if conn.proxy.is_some() {
                    form.proxy_password
                        .resolve()
                        .filter(|pw| !pw.is_empty())
                        .map(str::to_string)
                } else {
                    None
                };
                let entry = crate::state::QuickConnectEntry {
                    conn,
                    password,
                    totp_secret,
                    proxy_password,
                };
                self.show_host_panel = false;
                self.panel_nav_clear();
                self.host_panel_error = None;
                return self.update(Message::Ssh(SshMessage::QuickConnect(Box::new(entry))));
            }
            EditorMessage::EditorCancel => {
                self.show_host_panel = false;
                self.panel_nav_clear();
                self.host_panel_error = None;
            }
            EditorMessage::RequestDeleteConnection(idx) => {
                if let Some(conn) = self.connections.get(idx) {
                    let name = conn.label.clone();
                    self.confirm_remove(name, Message::Editor(EditorMessage::DeleteConnection(idx)));
                }
            }
            EditorMessage::DeleteConnection(idx) => {
                self.card_context_menu = None;
                self.overlay = None;
                if let Some(conn) = self.connections.get(idx) {
                    let id = conn.id;
                    if let Some(vault) = &self.vault {
                        let _ = vault.delete_connection(&id);
                        self.show_host_panel = false;
                        self.panel_nav_clear();
                        self.load_data_from_vault();
                    }
                }
            }
            EditorMessage::DuplicateConnection(idx) => {
                self.card_context_menu = None;
                self.overlay = None;
                if let Some(conn) = self.connections.get(idx).cloned() {
                    let mut dup = Connection::new(
                        format!("{} (copy)", conn.label),
                        &conn.hostname,
                    );
                    // Protocol + its params must carry, or a Telnet/Serial
                    // host silently duplicates as SSH. Encoding / terminal
                    // type are host config that applies to all protocols.
                    dup.protocol = conn.protocol;
                    dup.serial = conn.serial;
                    dup.rd_kind = conn.rd_kind;
                    dup.rd_gateway_id = conn.rd_gateway_id;
                    dup.address_family = conn.address_family;
                    dup.encoding = conn.encoding.clone();
                    dup.terminal_type = conn.terminal_type.clone();
                    dup.port = conn.port;
                    dup.username = conn.username.clone();
                    dup.auth_method = conn.auth_method.clone();
                    dup.key_id = conn.key_id;
                    dup.group_id = conn.group_id;
                    dup.jump_chain = conn.jump_chain.clone();
                    dup.port_forwards = conn.port_forwards.clone();
                    dup.proxy = conn.proxy.clone();
                    dup.tags = conn.tags.clone();
                    dup.notes = conn.notes.clone();
                    dup.color = conn.color.clone();
                    dup.agent_forwarding = conn.agent_forwarding;
                    if let Some(vault) = &self.vault {
                        // Copy password and proxy password to the duplicate.
                        let pw = vault.get_connection_password(&conn.id).ok().flatten();
                        let proxy_pw = vault.get_proxy_password(&conn.id).ok().flatten();
                        let _ = vault.save_connection(&dup, pw.as_deref());
                        if proxy_pw.is_some() {
                            let _ = vault.set_proxy_password(&dup.id, proxy_pw.as_deref());
                        }
                        self.load_data_from_vault();
                    }
                }
            }
            // ── Connection identity ──
            EditorMessage::EditorIdentityChanged(v) => {
                self.editor_form.username_focused = false;
                if v == "(none)" {
                    self.editor_form.selected_identity = None;
                } else {
                    self.editor_form.selected_identity = Some(v);
                }
            }

            // ── Live host-config edits from the terminal sidebar tab ──
            EditorMessage::HostConfigThemeChanged(name) => {
                // Empty sentinel = follow the global terminal theme (None).
                let value = if name.is_empty() { None } else { Some(name) };
                self.host_config_apply(|c| c.terminal_theme = value, true);
            }
            EditorMessage::HostConfigEncodingChanged(v) => {
                let value = if v == "UTF-8" { None } else { Some(v) };
                self.host_config_apply(|c| c.encoding = value, false);
            }
            EditorMessage::HostConfigTerminalTypeChanged(v) => {
                let value = if v == "xterm-256color" { None } else { Some(v) };
                self.host_config_apply(|c| c.terminal_type = value, false);
            }
            EditorMessage::HostConfigAutoTitleChanged(v) => {
                use crate::i18n::t;
                let value = if v == t("host_auto_title_show") {
                    Some(true)
                } else if v == t("host_auto_title_hide") {
                    Some(false)
                } else {
                    None
                };
                self.host_config_apply(|c| c.auto_title = value, false);
            }
        }
        Task::none()
    }

    /// Resolve the focused pane's connection index, apply `mutate`, persist
    /// it (preserving the password), and refresh in-memory state. When
    /// `repaint` is set (theme changes) the running terminal is repainted
    /// for instant preview. A no-op when the focused pane isn't a saved host.
    pub(crate) fn host_config_apply<F: FnOnce(&mut oryxis_core::models::connection::Connection)>(
        &mut self,
        mutate: F,
        repaint: bool,
    ) {
        let Some(id) = self
            .active_tab
            .and_then(|i| self.tabs.get(i))
            .and_then(|tab| match &tab.active().origin {
                crate::state::PaneOrigin::Host(id) => Some(*id),
                _ => None,
            })
        else {
            return;
        };
        let Some(idx) = self.connections.iter().position(|c| c.id == id) else {
            return;
        };
        mutate(&mut self.connections[idx]);
        let label = self.connections[idx].label.clone();
        if let Some(vault) = &self.vault {
            // `None` preserves the encrypted password column untouched.
            let _ = vault.save_connection(&self.connections[idx], None);
        }
        if repaint {
            self.repaint_terminal_palettes_for_label(&label);
        }
    }
}

/// How the host editor's SSH Key combo narrows / orders the key list,
/// per auth method.
#[derive(Clone, Copy, PartialEq, Eq)]
enum KeyComboFilter {
    /// Every key, vault order (the `Key` method).
    All,
    /// Only certificate-carrying keys (the `Certificate` method, B2.1).
    CertificateOnly,
    /// Every key, security keys first (the `Agent` method's preferred-
    /// identity pick, B3).
    SecurityKeysFirst,
}

/// Option list for the host editor's SSH Key combo, pure so it
/// unit-tests: the `(none)` sentinel first, then the key labels per
/// the filter. `Key` and `Certificate` both decode the private key
/// locally to sign, so they only list rows that HOLD a private
/// (`has_private`); a security-key / public-only row belongs under
/// `Agent`, where the hardware token signs.
fn key_combo_options(
    keys: &[oryxis_core::models::key::SshKey],
    filter: KeyComboFilter,
) -> Vec<String> {
    let mut opts = vec!["(none)".to_string()];
    match filter {
        KeyComboFilter::All => opts.extend(
            keys.iter()
                .filter(|k| k.has_private)
                .map(|k| k.label.clone()),
        ),
        KeyComboFilter::CertificateOnly => opts.extend(
            keys.iter()
                .filter(|k| k.certificate.is_some() && k.has_private)
                .map(|k| k.label.clone()),
        ),
        KeyComboFilter::SecurityKeysFirst => {
            opts.extend(
                keys.iter()
                    .filter(|k| k.algorithm.is_security_key())
                    .map(|k| k.label.clone()),
            );
            opts.extend(
                keys.iter()
                    .filter(|k| !k.algorithm.is_security_key())
                    .map(|k| k.label.clone()),
            );
        }
    }
    opts
}

/// Result of resolving the editor form's proxy section into model
/// fields. `Identity(_)` selections route to `proxy_identity_id`, the
/// other static kinds populate the inline `ProxyConfig`. Note that
/// `password` is left as `None` here, it's persisted in the encrypted
/// `proxy_password` column via `set_proxy_password`, never inside the
/// serialized inline JSON.
pub(crate) struct ProxyResolution {
    pub proxy: Option<oryxis_core::models::connection::ProxyConfig>,
    pub proxy_identity_id: Option<uuid::Uuid>,
}

fn build_proxy_resolution(form: &ConnectionForm) -> Result<ProxyResolution, String> {
    use oryxis_core::models::connection::ProxyConfig;

    match form.proxy_kind {
        ProxyKind::None => Ok(ProxyResolution {
            proxy: None,
            proxy_identity_id: None,
        }),
        ProxyKind::Identity(id) => Ok(ProxyResolution {
            proxy: None,
            proxy_identity_id: Some(id),
        }),
        ProxyKind::Command => {
            if form.proxy_command.trim().is_empty() {
                return Err(crate::i18n::t("proxy_err_command_required").into());
            }
            Ok(ProxyResolution {
                proxy: Some(ProxyConfig {
                    proxy_type: ProxyType::Command(form.proxy_command.clone()),
                    host: String::new(),
                    port: 0,
                    username: None,
                    password: None,
                }),
                proxy_identity_id: None,
            })
        }
        kind @ (ProxyKind::Socks5 | ProxyKind::Socks4 | ProxyKind::Http) => {
            if form.proxy_host.trim().is_empty() {
                return Err(crate::i18n::t("proxy_err_host_required").into());
            }
            let port = form
                .proxy_port
                .parse::<u16>()
                .ok()
                .filter(|p| *p > 0)
                .ok_or_else(|| crate::i18n::t("proxy_err_port_invalid").to_string())?;

            let proxy_type = match kind {
                ProxyKind::Socks5 => ProxyType::Socks5,
                ProxyKind::Socks4 => ProxyType::Socks4,
                ProxyKind::Http => ProxyType::Http,
                _ => unreachable!(),
            };

            Ok(ProxyResolution {
                proxy: Some(ProxyConfig {
                    proxy_type,
                    host: form.proxy_host.clone(),
                    port,
                    username: if form.proxy_username.is_empty() {
                        None
                    } else {
                        Some(form.proxy_username.clone())
                    },
                    password: None,
                }),
                proxy_identity_id: None,
            })
        }
    }
}

#[cfg(test)]
mod key_combo_tests {
    use super::{key_combo_options, KeyComboFilter};
    use oryxis_core::models::key::{KeyAlgorithm, SshKey};

    // A normal key holds a private (has_private = true).
    fn key(label: &str, with_cert: bool) -> SshKey {
        let mut k = SshKey::new(label, KeyAlgorithm::Ed25519);
        k.has_private = true;
        if with_cert {
            k.certificate = Some("ssh-ed25519-cert-v01@openssh.com AAAA... u@h".into());
        }
        k
    }

    // A security key is public-only (has_private = false).
    fn sk(label: &str) -> SshKey {
        SshKey::new(label, KeyAlgorithm::SkEd25519)
    }

    #[test]
    fn unfiltered_lists_every_key_after_the_sentinel() {
        let keys = vec![key("bare", false), key("certified", true)];
        assert_eq!(
            key_combo_options(&keys, KeyComboFilter::All),
            vec!["(none)", "bare", "certified"]
        );
    }

    #[test]
    fn key_mode_excludes_public_only_rows() {
        // A security key (no private) can never authenticate under `Key`;
        // it must not appear in the combo.
        let keys = vec![key("bare", false), sk("yubi")];
        assert_eq!(
            key_combo_options(&keys, KeyComboFilter::All),
            vec!["(none)", "bare"]
        );
    }

    #[test]
    fn certificate_mode_lists_only_cert_carrying_keys() {
        let keys = vec![key("bare", false), key("certified", true), key("plain2", false)];
        assert_eq!(
            key_combo_options(&keys, KeyComboFilter::CertificateOnly),
            vec!["(none)", "certified"]
        );
    }

    #[test]
    fn certificate_mode_excludes_public_only_even_with_a_cert() {
        // A public-only row can carry a cert (delegation), but `Certificate`
        // auth signs with the local private, which it lacks.
        let mut yubi_cert = sk("yubi-cert");
        yubi_cert.certificate = Some("sk-ssh-ed25519-cert-v01@openssh.com AAAA... u@h".into());
        let keys = vec![key("certified", true), yubi_cert];
        assert_eq!(
            key_combo_options(&keys, KeyComboFilter::CertificateOnly),
            vec!["(none)", "certified"]
        );
    }

    #[test]
    fn certificate_mode_with_no_certs_keeps_the_sentinel_only() {
        let keys = vec![key("bare", false)];
        assert_eq!(
            key_combo_options(&keys, KeyComboFilter::CertificateOnly),
            vec!["(none)"]
        );
    }

    #[test]
    fn agent_mode_lists_security_keys_first_including_public_only() {
        // Agent delegates signing, so public-only rows DO belong here.
        let keys = vec![key("bare", false), sk("yubi"), key("other", true), sk("solo")];
        assert_eq!(
            key_combo_options(&keys, KeyComboFilter::SecurityKeysFirst),
            vec!["(none)", "yubi", "solo", "bare", "other"]
        );
    }
}
