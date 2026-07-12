//! The ssh-agent wire protocol, server side, backed by an
//! [`AgentKeySource`] instead of an in-memory key store.
//!
//! Structure lifted from russh's `keys/agent/server.rs` (the frozen
//! draft-miller-ssh-agent framing and the IDENTITIES_ANSWER /
//! SIGN_RESPONSE encoding), with two changes:
//!
//! - the key roster and signing come from the [`AgentKeySource`], which
//!   the vault-backed impl decrypts on demand, instead of a `KeyStore`
//!   HashMap of already-decrypted keys held for the whole session;
//! - every write op (ADD / REMOVE / LOCK / UNLOCK) answers FAILURE: an
//!   Oryxis agent is a read-only signing oracle.
//!
//! Frames are `u32` big-endian length + payload, capped at
//! [`MAX_AGENT_FRAME_LEN`]. russh's public `AgentClient` is the test
//! driver ([`tests`]): if it lists our keys and verifies our
//! signatures, the encoding matched byte for byte.

use ssh_encoding::{Decode, Encode};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use super::source::{AgentKeySource, AgentSignError, SignHash};

/// A per-signature confirmation request handed to the UI. The agent
/// blocks the sign until `respond` carries the user's decision (or a
/// timeout / drop denies). Sent on the [`ConfirmSender`] the runtime
/// wires in when the confirm setting is on.
pub(crate) struct ConfirmAsk {
    pub key_comment: String,
    pub key_fingerprint: String,
    /// Best-effort requesting process, when the platform exposes it.
    pub peer: Option<String>,
    pub respond: tokio::sync::oneshot::Sender<bool>,
}

pub(crate) type ConfirmSender = tokio::sync::mpsc::UnboundedSender<ConfirmAsk>;

/// How long a confirm prompt waits before denying by default, so a
/// forgotten prompt never wedges a `git` call forever.
const CONFIRM_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// Frame size cap, matching russh's `MAX_AGENT_FRAME_LEN`. A real
/// request is a few hundred bytes; anything past 256 KiB is broken or
/// hostile and drops the connection.
pub(crate) const MAX_AGENT_FRAME_LEN: usize = 256 * 1024;

// Message numbers (draft-miller-ssh-agent). Only the two read ops are
// answered; the rest fall through to FAILURE.
const SSH_AGENT_FAILURE: u8 = 5;
const SSH_AGENTC_REQUEST_IDENTITIES: u8 = 11;
const SSH_AGENT_IDENTITIES_ANSWER: u8 = 12;
const SSH_AGENTC_SIGN_REQUEST: u8 = 13;
const SSH_AGENT_SIGN_RESPONSE: u8 = 14;

// SIGN_REQUEST flag bits selecting the RSA hash.
const SSH_AGENT_RSA_SHA2_256: u32 = 2;
const SSH_AGENT_RSA_SHA2_512: u32 = 4;

/// Serve a single agent connection until the peer hangs up or sends a
/// malformed / oversized frame. Each connection is its own task, so a
/// bad frame kills only this connection.
pub(crate) async fn serve_connection<S, K>(
    mut stream: S,
    source: &K,
    confirm: Option<&ConfirmSender>,
    peer: Option<&str>,
) -> std::io::Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin,
    K: AgentKeySource,
{
    let mut len_buf = [0u8; 4];
    loop {
        // A clean EOF on the length read is the peer closing; anything
        // else propagates.
        if let Err(e) = stream.read_exact(&mut len_buf).await {
            if e.kind() == std::io::ErrorKind::UnexpectedEof {
                return Ok(());
            }
            return Err(e);
        }
        let len = u32::from_be_bytes(len_buf) as usize;
        if len == 0 || len > MAX_AGENT_FRAME_LEN {
            // Oversized / empty frame: refuse to read it and drop the
            // connection rather than answer.
            return Ok(());
        }
        let mut payload = vec![0u8; len];
        stream.read_exact(&mut payload).await?;

        // The sync-answerable messages (identities, unknown, write ops)
        // resolve without I/O; SIGN_REQUEST may await a confirm prompt.
        let response = match respond_readonly(&payload, source) {
            Some(resp) => resp,
            None => {
                let body = &payload[1..];
                sign_response(body, source, confirm, peer).await
            }
        };
        stream
            .write_all(&(response.len() as u32).to_be_bytes())
            .await?;
        stream.write_all(&response).await?;
        stream.flush().await?;
    }
}

/// Response for every message that resolves synchronously. Returns
/// `None` only for `SIGN_REQUEST`, which the async caller handles (it
/// may block on a confirm prompt). A read-only provider answers
/// FAILURE to every write op (ADD 17/25, REMOVE 18/19, LOCK 22,
/// UNLOCK 23, extensions 27) and to anything unrecognized.
fn respond_readonly<K: AgentKeySource>(payload: &[u8], source: &K) -> Option<Vec<u8>> {
    match payload.split_first() {
        Some((&SSH_AGENTC_REQUEST_IDENTITIES, _)) => Some(identities_answer(source)),
        Some((&SSH_AGENTC_SIGN_REQUEST, _)) => None,
        _ => Some(vec![SSH_AGENT_FAILURE]),
    }
}

/// `SSH_AGENT_IDENTITIES_ANSWER`: count, then `(blob, comment)` per key.
fn identities_answer<K: AgentKeySource>(source: &K) -> Vec<u8> {
    let keys = source.list();
    let mut out = vec![SSH_AGENT_IDENTITIES_ANSWER];
    if (keys.len() as u32).encode(&mut out).is_err() {
        return vec![SSH_AGENT_FAILURE];
    }
    for key in keys {
        // `blob` is already the encoded KeyData; it rides the wire as a
        // length-prefixed string, same as the comment.
        if key.blob.encode(&mut out).is_err() || key.comment.encode(&mut out).is_err() {
            return vec![SSH_AGENT_FAILURE];
        }
    }
    out
}

/// `SSH_AGENT_SIGN_RESPONSE`: the signature wire blob as a string.
/// Request body is `string key_blob, string data, uint32 flags`. When
/// `confirm` is set, the user must allow the signature first (a deny,
/// timeout or closed channel yields FAILURE, the safe default).
async fn sign_response<K: AgentKeySource>(
    body: &[u8],
    source: &K,
    confirm: Option<&ConfirmSender>,
    peer: Option<&str>,
) -> Vec<u8> {
    let mut reader = body;
    let parsed = (|| -> Result<(Vec<u8>, Vec<u8>, u32), ssh_encoding::Error> {
        let key_blob = Vec::<u8>::decode(&mut reader)?;
        let data = Vec::<u8>::decode(&mut reader)?;
        // Flags are optional on the wire for some old clients; default 0.
        let flags = u32::decode(&mut reader).unwrap_or(0);
        Ok((key_blob, data, flags))
    })();
    let Ok((key_blob, data, flags)) = parsed else {
        return vec![SSH_AGENT_FAILURE];
    };

    // The key must be one we currently expose; look it up for the
    // confirm card and to reject unknown blobs before signing.
    let Some(exposed) = source.list().into_iter().find(|k| k.blob == key_blob) else {
        return vec![SSH_AGENT_FAILURE];
    };

    if let Some(sender) = confirm
        && !ask_confirm(sender, &exposed, &key_blob, peer).await
    {
        return vec![SSH_AGENT_FAILURE];
    }

    let hash = if flags & SSH_AGENT_RSA_SHA2_512 != 0 {
        SignHash::Sha512
    } else if flags & SSH_AGENT_RSA_SHA2_256 != 0 {
        SignHash::Sha256
    } else {
        SignHash::Default
    };

    match source.sign(&key_blob, &data, hash) {
        Ok(sig_blob) => {
            let mut out = vec![SSH_AGENT_SIGN_RESPONSE];
            if sig_blob.encode(&mut out).is_err() {
                return vec![SSH_AGENT_FAILURE];
            }
            out
        }
        Err(reason) => {
            if let AgentSignError::SignFailed(msg) = &reason {
                tracing::warn!(target = "oryxis::agent", error = %msg, "sign failed");
            }
            vec![SSH_AGENT_FAILURE]
        }
    }
}

/// Send a confirm prompt and await the decision, denying on timeout or
/// a dropped channel (the UI went away).
async fn ask_confirm(
    sender: &ConfirmSender,
    key: &super::source::AgentPublicKey,
    key_blob: &[u8],
    peer: Option<&str>,
) -> bool {
    let (tx, rx) = tokio::sync::oneshot::channel();
    let ask = ConfirmAsk {
        key_comment: key.comment.clone(),
        key_fingerprint: fingerprint_of(key_blob),
        peer: peer.map(str::to_owned),
        respond: tx,
    };
    if sender.send(ask).is_err() {
        return false;
    }
    matches!(tokio::time::timeout(CONFIRM_TIMEOUT, rx).await, Ok(Ok(true)))
}

/// SHA-256 fingerprint (`SHA256:...`) of a public-key wire blob, for
/// the confirm card. Best-effort: an unparseable blob shows a
/// placeholder rather than failing the sign path.
fn fingerprint_of(blob: &[u8]) -> String {
    let mut reader = blob;
    match ssh_key::public::KeyData::decode(&mut reader) {
        Ok(data) => data.fingerprint(ssh_key::HashAlg::Sha256).to_string(),
        Err(_) => "SHA256:?".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::super::source::mock::MockKeySource;
    use super::*;
    use oryxis_vault::{EcdsaCurveChoice, GenerateSpec, RsaBits};
    use ssh_key::PrivateKey;

    /// A fresh key via the (already tested) vault keygen, parsed to
    /// `ssh_key::PrivateKey`. Sidesteps the app-crate rand version and
    /// reuses the generation path B4 covers.
    fn gen_key(spec: GenerateSpec) -> PrivateKey {
        let g = oryxis_vault::generate_key("t", "", spec).unwrap();
        PrivateKey::from_openssh(&g.private_pem).unwrap()
    }

    /// Drive our server with russh's own `AgentClient` over an
    /// in-memory duplex. `request_identities` succeeding proves our
    /// IDENTITIES_ANSWER encoding; `sign_request` succeeding proves our
    /// SIGN_RESPONSE encoding (russh's client parses and validates the
    /// signature blob against the key algorithm internally, erroring on
    /// a malformed one). The cryptographic validity of the signature
    /// itself is checked directly in [`signature_verifies`], since
    /// russh's `sign_request` rebuilds its return value into an
    /// internal form that is not a re-decodable `ssh_key::Signature`.
    async fn round_trip(keys: Vec<(PrivateKey, String)>, sign_idx: usize) {
        use russh::keys::agent::client::AgentClient;

        let n = keys.len();
        let source = std::sync::Arc::new(MockKeySource::new(keys));

        let (client_side, server_side) = tokio::io::duplex(64 * 1024);
        let server_source = source.clone();
        let server = tokio::spawn(async move {
            let _ = serve_connection(server_side, server_source.as_ref(), None, None).await;
        });

        let mut client = AgentClient::connect(client_side);
        let identities = client.request_identities().await.unwrap();
        assert_eq!(identities.len(), n, "roster length");

        let data = b"oryxis agent protocol test".to_vec();
        let signed = client
            .sign_request(&identities[sign_idx], None, data)
            .await
            .expect("sign request accepted and signature parsed by the client");
        assert!(!signed.is_empty());

        drop(client);
        let _ = server.await;
    }

    /// Our `SIGN_RESPONSE` carries a signature that decodes as a real
    /// `ssh_key::Signature` and verifies against the public key, per
    /// algorithm. This checks the crypto directly on the source output
    /// (the wire framing is covered by [`round_trip`]).
    fn signature_verifies(spec: GenerateSpec) {
        use super::super::source::{AgentKeySource, SignHash};
        use signature::Verifier;
        use ssh_encoding::Encode;

        let key = gen_key(spec);
        let public = key.public_key().clone();
        let mut blob = Vec::new();
        public.key_data().encode(&mut blob).unwrap();

        let source = MockKeySource::new(vec![(key, "k".into())]);
        let data = b"verify me";
        let sig_blob = source.sign(&blob, data, SignHash::Default).unwrap();
        let sig = ssh_key::Signature::decode(&mut sig_blob.as_slice()).unwrap();
        public.key_data().verify(data, &sig).expect("verifies");
    }

    #[test]
    fn signatures_verify_per_algorithm() {
        signature_verifies(GenerateSpec::Ed25519);
        signature_verifies(GenerateSpec::Ecdsa { curve: EcdsaCurveChoice::P256 });
        signature_verifies(GenerateSpec::Rsa { bits: RsaBits::B2048 });
    }

    #[tokio::test]
    async fn ed25519_list_and_sign() {
        round_trip(vec![(gen_key(GenerateSpec::Ed25519), "ed".into())], 0).await;
    }

    #[tokio::test]
    async fn ecdsa_list_and_sign() {
        round_trip(
            vec![(
                gen_key(GenerateSpec::Ecdsa { curve: EcdsaCurveChoice::P256 }),
                "ec".into(),
            )],
            0,
        )
        .await;
    }

    #[tokio::test]
    async fn rsa_list_and_sign() {
        round_trip(
            vec![(gen_key(GenerateSpec::Rsa { bits: RsaBits::B2048 }), "rsa".into())],
            0,
        )
        .await;
    }

    #[tokio::test]
    async fn multiple_keys_listed() {
        round_trip(
            vec![
                (gen_key(GenerateSpec::Ed25519), "a".into()),
                (gen_key(GenerateSpec::Ed25519), "b".into()),
            ],
            1,
        )
        .await;
    }

    #[tokio::test]
    async fn locked_source_lists_empty() {
        use russh::keys::agent::client::AgentClient;

        let key = gen_key(GenerateSpec::Ed25519);
        let source = std::sync::Arc::new(MockKeySource::new(vec![(key, "k".into())]));
        source
            .locked
            .store(true, std::sync::atomic::Ordering::Relaxed);

        let (client_side, server_side) = tokio::io::duplex(64 * 1024);
        let server_source = source.clone();
        let server = tokio::spawn(async move {
            let _ = serve_connection(server_side, server_source.as_ref(), None, None).await;
        });

        let mut client = AgentClient::connect(client_side);
        // Locked: empty roster, so there is nothing to sign with.
        assert!(client.request_identities().await.unwrap().is_empty());

        drop(client);
        let _ = server.await;
    }

    #[test]
    fn unknown_message_and_write_ops_fail() {
        let source = MockKeySource::new(vec![]);
        // Unknown type.
        assert_eq!(respond_readonly(&[99], &source), Some(vec![SSH_AGENT_FAILURE]));
        // ADD_IDENTITY (17), REMOVE (18), LOCK (22): all FAILURE.
        for op in [17u8, 18, 19, 22, 23, 25, 27] {
            assert_eq!(respond_readonly(&[op], &source), Some(vec![SSH_AGENT_FAILURE]), "op {op}");
        }
        // Empty payload.
        assert_eq!(respond_readonly(&[], &source), Some(vec![SSH_AGENT_FAILURE]));
        // SIGN_REQUEST is the async path: readonly declines it.
        assert_eq!(respond_readonly(&[13, 0, 0, 0, 0], &source), None);
    }
}
