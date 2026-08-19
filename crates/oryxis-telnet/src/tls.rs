//! TLS for `telnets` (conventionally port 992).
//!
//! There is no in-band upgrade to negotiate: the handshake runs on the
//! bare socket and the whole Telnet NVT, greeting included, rides
//! inside the tunnel. So this module has exactly one job, wrapping a
//! connected `TcpStream`, and the session code above it never learns
//! which of the two stream types it is reading.
//!
//! Verification is on by default against the webpki root store, the
//! same trust anchors the rest of the app uses for HTTPS. The per-host
//! escape (`insecure`) exists because network appliances ship
//! self-signed certificates and their owners cannot always replace
//! them; it is per host and never global, so turning it on for the one
//! switch that needs it cannot quietly disarm verification anywhere
//! else.

use std::sync::Arc;

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, SignatureScheme};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tokio_rustls::client::TlsStream;

use crate::session::TelnetError;

/// Per-host TLS settings, mirroring `oryxis_core::models::telnet`
/// without depending on its shape (the engine takes a transport
/// config, not a vault model).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TelnetTls {
    /// Accept a certificate the trust store rejects. Off by default.
    pub insecure: bool,
}

/// Run the TLS handshake over an already-connected socket.
///
/// `server_name` is the host as the user typed it, which is what the
/// certificate must match. An IP literal is a valid `ServerName` too
/// (rustls checks it against the certificate's IP SANs), and when it
/// isn't a usable name at all the handshake fails honestly instead of
/// falling back to no verification.
pub async fn wrap(
    stream: TcpStream,
    server_name: &str,
    tls: TelnetTls,
) -> Result<TlsStream<TcpStream>, TelnetError> {
    // rustls 0.23 wants an explicit provider. Idempotent: this only
    // errors when one is already installed, which is the normal case
    // inside the app (`util::install_crypto_provider` runs at boot) and
    // the reason it is also done here, so the engine works in a test
    // binary or a plugin process that never ran that.
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

    let config = if tls.insecure {
        rustls::ClientConfig::builder()
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(NoVerification))
            .with_no_client_auth()
    } else {
        let roots = rustls::RootCertStore {
            roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
        };
        rustls::ClientConfig::builder().with_root_certificates(roots).with_no_client_auth()
    };

    let name = ServerName::try_from(server_name.to_string())
        .map_err(|_| TelnetError::Tls(format!("{server_name} is not a valid TLS server name")))?;
    TlsConnector::from(Arc::new(config))
        .connect(name, stream)
        .await
        .map_err(|e| TelnetError::Tls(e.to_string()))
}

/// The `insecure` escape: accepts any certificate and any signature.
///
/// Signature SCHEMES still come from the crypto provider rather than a
/// hand-written list, so this only ever disables the trust decision
/// (chain, expiry, name), never the cryptography itself.
#[derive(Debug)]
struct NoVerification;

impl ServerCertVerifier for NoVerification {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        rustls::crypto::aws_lc_rs::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}
