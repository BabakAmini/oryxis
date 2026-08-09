//! Translation between the group editor's form and `GroupDefaults`
//! (D4).
//!
//! The panel edits labels and strings, the model stores ids and typed
//! options, and the two directions live together here so they cannot
//! drift: a field added to one and forgotten in the other would look
//! like a value that silently refuses to save.

use super::*;
use oryxis_core::models::group::GroupDefaults;

impl Oryxis {
    /// Fill the editor's defaults fields from the stored group.
    ///
    /// Ids resolve back to labels against the CURRENT lists, so a
    /// reference whose target was deleted shows as "not set" rather
    /// than as a label that no longer means anything. Saving then
    /// stores that honest emptiness, which is also how a dangling
    /// reference gets cleaned up.
    pub(crate) fn hydrate_group_defaults_form(&mut self, gid: uuid::Uuid) {
        let defaults = self
            .groups
            .iter()
            .find(|g| g.id == gid)
            .and_then(|g| g.defaults.clone())
            .unwrap_or_default();

        self.group_edit.username = defaults.username.clone().unwrap_or_default();
        self.group_edit.port = defaults.port.map(|p| p.to_string()).unwrap_or_default();
        self.group_edit.identity_label = defaults.identity_id.and_then(|id| {
            self.identities
                .iter()
                .find(|i| i.id == id)
                .map(|i| i.label.clone())
        });
        self.group_edit.proxy_identity_label = defaults.proxy_identity_id.and_then(|id| {
            self.proxy_identities
                .iter()
                .find(|p| p.id == id)
                .map(|p| p.label.clone())
        });
        self.group_edit.startup_snippet_label = defaults.startup_snippet_id.and_then(|id| {
            self.snippets
                .iter()
                .find(|s| s.id == id)
                .map(|s| s.label.clone())
        });
        // The theme is stored BY NAME, not by id, so it needs no
        // lookup; an unknown name (a custom theme deleted since) still
        // shows, because the name is the value and the user may want to
        // keep it.
        self.group_edit.terminal_theme = defaults.terminal_theme.clone();
        self.group_edit.env_vars = defaults.env_vars.clone();
        // Open the section when the group already sets something, so
        // existing defaults are never hidden behind a collapsed header.
        self.group_edit.defaults_open = !defaults.is_empty();
    }

    /// Build the `GroupDefaults` to store, or `None` when the form sets
    /// nothing (which keeps the column NULL rather than `{}`).
    ///
    /// Labels resolve to ids here, at save time, against the lists as
    /// they are now: a picker still naming something the user deleted
    /// meanwhile stores nothing instead of a dangling id.
    pub(crate) fn group_edit_defaults(&self) -> Option<GroupDefaults> {
        let form = &self.group_edit;
        let defaults = GroupDefaults {
            username: Some(form.username.trim().to_string()).filter(|u| !u.is_empty()),
            identity_id: form.identity_label.as_ref().and_then(|label| {
                self.identities
                    .iter()
                    .find(|i| &i.label == label)
                    .map(|i| i.id)
            }),
            proxy_identity_id: form.proxy_identity_label.as_ref().and_then(|label| {
                self.proxy_identities
                    .iter()
                    .find(|p| &p.label == label)
                    .map(|p| p.id)
            }),
            port: form.port.trim().parse::<u16>().ok().filter(|p| *p > 0),
            // A half-typed row (no name) is not a variable yet; dropping
            // it keeps an accidental Enter from storing an empty pair
            // that would then merge over an inherited one.
            env_vars: form
                .env_vars
                .iter()
                .filter(|v| !v.key.trim().is_empty())
                .cloned()
                .collect(),
            terminal_theme: form.terminal_theme.clone().filter(|t| !t.is_empty()),
            startup_snippet_id: form.startup_snippet_label.as_ref().and_then(|label| {
                self.snippets
                    .iter()
                    .find(|s| &s.label == label)
                    .map(|s| s.id)
            }),
        };
        (!defaults.is_empty()).then_some(defaults)
    }

    /// The port a host created inside `group_id` should start with.
    ///
    /// This is the ONLY place a group's port applies. Resolving it at
    /// connect time would change where an existing host connects the
    /// moment its group gained a default, which is the one inheritance
    /// behaviour that can break something that works today.
    pub(crate) fn group_default_port(&self, group_id: Option<uuid::Uuid>) -> Option<u16> {
        let mut cursor = group_id;
        let mut seen = std::collections::HashSet::new();
        // Same cycle guard the resolver uses: a synced parent loop must
        // not spin the editor.
        while let Some(id) = cursor {
            if !seen.insert(id) {
                return None;
            }
            let group = self.groups.iter().find(|g| g.id == id)?;
            if let Some(port) = group.defaults.as_ref().and_then(|d| d.port) {
                return Some(port);
            }
            cursor = group.parent_id;
        }
        None
    }
}
