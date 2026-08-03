//! Proxy-identity entity CRUD + editor form, wrapped by [`crate::messages::Message::ProxyIdentity`]. Handled by `Oryxis::handle_proxy_identity`.

use uuid::Uuid;

#[derive(Debug, Clone)]
pub enum ProxyIdentityMessage {
    ProxySearchChanged(String),
    ShowProxyIdentityForm(Option<Uuid>),
    HideProxyIdentityForm,
    ProxyIdentityFormLabelChanged(String),
    ProxyIdentityFormKindChanged(crate::state::ProxyKind),
    ProxyIdentityFormHostChanged(String),
    ProxyIdentityFormPortChanged(String),
    ProxyIdentityFormUsernameChanged(String),
    ProxyIdentityFormPasswordChanged(super::Redacted),
    ProxyIdentityFormPasswordToggleVisibility,
    SaveProxyIdentity,
    DeleteProxyIdentity(Uuid),
}
