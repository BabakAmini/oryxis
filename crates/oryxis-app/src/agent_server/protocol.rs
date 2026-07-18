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
//! - vault keys are read-only over the wire. ADD / REMOVE (17/25,
//!   18/19) are forwarded to the source, which refuses them unless the
//!   user opted in (`agent_server_allow_add`, Phase 4) and only ever
//!   touches the in-memory ephemeral roster; LOCK / UNLOCK (22/23) and
//!   extensions (27) always answer FAILURE.
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

/// The connection's confirm policy. `sender` is the channel to the UI
/// prompt (`None` only when no UI exists, e.g. protocol tests); `all`
/// is the global `agent_server_confirm` setting. A key added with the
/// CONFIRM constraint prompts even when `all` is off, which is why the
/// channel and the setting are separate concerns.
#[derive(Clone, Default)]
pub(crate) struct ConfirmMode {
    pub sender: Option<ConfirmSender>,
    pub all: bool,
}

/// How long a confirm prompt waits before denying by default, so a
/// forgotten prompt never wedges a `git` call forever.
const CONFIRM_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// Frame size cap, matching russh's `MAX_AGENT_FRAME_LEN`. A real
/// request is a few hundred bytes; anything past 256 KiB is broken or
/// hostile and drops the connection.
pub(crate) const MAX_AGENT_FRAME_LEN: usize = 256 * 1024;

// Message numbers (draft-miller-ssh-agent). The two read ops, the
// add/remove family (forwarded to the source, refused unless the user
// opted in); LOCK/UNLOCK and anything unrecognized fall to FAILURE.
const SSH_AGENT_FAILURE: u8 = 5;
const SSH_AGENT_SUCCESS: u8 = 6;
const SSH_AGENTC_REQUEST_IDENTITIES: u8 = 11;
const SSH_AGENT_IDENTITIES_ANSWER: u8 = 12;
const SSH_AGENTC_SIGN_REQUEST: u8 = 13;
const SSH_AGENT_SIGN_RESPONSE: u8 = 14;
const SSH_AGENTC_ADD_IDENTITY: u8 = 17;
const SSH_AGENTC_REMOVE_IDENTITY: u8 = 18;
const SSH_AGENTC_REMOVE_ALL_IDENTITIES: u8 = 19;
const SSH_AGENTC_ADD_ID_CONSTRAINED: u8 = 25;

// ADD_ID_CONSTRAINED constraint types.
const SSH_AGENT_CONSTRAIN_LIFETIME: u8 = 1;
const SSH_AGENT_CONSTRAIN_CONFIRM: u8 = 2;

// SIGN_REQUEST flag bits selecting the RSA hash.
const SSH_AGENT_RSA_SHA2_256: u32 = 2;
const SSH_AGENT_RSA_SHA2_512: u32 = 4;

/// Serve a single agent connection until the peer hangs up or sends a
/// malformed / oversized frame. Each connection is its own task, so a
/// bad frame kills only this connection.
pub(crate) async fn serve_connection<S, K>(
    mut stream: S,
    source: &K,
    confirm: &ConfirmMode,
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

        // The sync-answerable messages (identities, add/remove,
        // unknown) resolve without I/O; SIGN_REQUEST may await a
        // confirm prompt.
        let response = match respond_sync(&payload, source, confirm.sender.is_some()) {
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
/// may block on a confirm prompt). ADD (17/25) and REMOVE (18/19) are
/// forwarded to the source (which refuses them unless the user opted
/// in); LOCK 22, UNLOCK 23, extensions 27 and anything unrecognized
/// answer FAILURE.
fn respond_sync<K: AgentKeySource>(
    payload: &[u8],
    source: &K,
    confirm_available: bool,
) -> Option<Vec<u8>> {
    match payload.split_first() {
        Some((&SSH_AGENTC_REQUEST_IDENTITIES, _)) => Some(identities_answer(source)),
        Some((&SSH_AGENTC_SIGN_REQUEST, _)) => None,
        Some((&SSH_AGENTC_ADD_IDENTITY, body)) => {
            Some(add_identity(body, source, false, confirm_available))
        }
        Some((&SSH_AGENTC_ADD_ID_CONSTRAINED, body)) => {
            Some(add_identity(body, source, true, confirm_available))
        }
        Some((&SSH_AGENTC_REMOVE_IDENTITY, body)) => Some(remove_identity(body, source)),
        Some((&SSH_AGENTC_REMOVE_ALL_IDENTITIES, _)) => Some(vec![if source.remove_all() {
            SSH_AGENT_SUCCESS
        } else {
            SSH_AGENT_FAILURE
        }]),
        _ => Some(vec![SSH_AGENT_FAILURE]),
    }
}

/// `ADD_IDENTITY` / `ADD_ID_CONSTRAINED`: the body is the private key
/// in the OpenSSH private-file wire encoding (`KeypairData`), a
/// comment, and (constrained only) a constraint list. Per the draft,
/// an unrecognized constraint MUST refuse the whole add. Only key
/// types this agent can actually sign with are accepted, so the
/// roster never advertises an identity that would fail at sign time.
fn add_identity<K: AgentKeySource>(
    body: &[u8],
    source: &K,
    constrained: bool,
    confirm_available: bool,
) -> Vec<u8> {
    use ssh_key::private::KeypairData;

    let failure = || vec![SSH_AGENT_FAILURE];
    let mut reader = body;
    let Ok(keypair) = KeypairData::decode(&mut reader) else {
        return failure();
    };
    // The comment is opaque bytes to OpenSSH; a non-UTF-8 comment (a
    // legacy-encoded name from ssh-add) must not sink the whole add.
    // Decode the length-prefixed bytes and lossy-convert for display.
    let Ok(comment_bytes) = Vec::<u8>::decode(&mut reader) else {
        return failure();
    };
    let comment = String::from_utf8_lossy(&comment_bytes).into_owned();

    let mut requires_confirm = false;
    let mut lifetime_secs: Option<u32> = None;
    if constrained {
        while !reader.is_empty() {
            let Ok(kind) = u8::decode(&mut reader) else {
                return failure();
            };
            match kind {
                SSH_AGENT_CONSTRAIN_LIFETIME => {
                    let Ok(secs) = u32::decode(&mut reader) else {
                        return failure();
                    };
                    // OpenSSH semantics: a zero lifetime means no
                    // deadline, not "expires immediately".
                    lifetime_secs = (secs > 0).then_some(secs);
                }
                SSH_AGENT_CONSTRAIN_CONFIRM => requires_confirm = true,
                // Extensions (0xff) included: we recognize none, and
                // honoring the draft means refusing rather than
                // silently dropping a constraint the client relies on.
                _ => return failure(),
            }
        }
    }

    // A confirm-constrained key with no UI to ask would deadlock into
    // silent denies at sign time; refuse the add instead.
    if requires_confirm && !confirm_available {
        return failure();
    }

    // Only signable material: no sk-* (hardware-backed), no DSA
    // (deprecated), no encrypted blobs (never sent by agents anyway;
    // `PrivateKey::new` would reject them too).
    if !matches!(
        keypair,
        KeypairData::Ed25519(_) | KeypairData::Rsa(_) | KeypairData::Ecdsa(_)
    ) {
        return failure();
    }
    let Ok(private) = ssh_key::PrivateKey::new(keypair, comment.clone()) else {
        return failure();
    };

    let expires_at = lifetime_secs
        .map(|s| std::time::Instant::now() + std::time::Duration::from_secs(u64::from(s)));
    vec![if source.add(private, comment, requires_confirm, expires_at) {
        SSH_AGENT_SUCCESS
    } else {
        SSH_AGENT_FAILURE
    }]
}

/// `REMOVE_IDENTITY`: body is the public key wire blob. Only ever
/// removes client-added keys; a vault key blob answers FAILURE.
fn remove_identity<K: AgentKeySource>(body: &[u8], source: &K) -> Vec<u8> {
    let mut reader = body;
    let Ok(blob) = Vec::<u8>::decode(&mut reader) else {
        return vec![SSH_AGENT_FAILURE];
    };
    vec![if source.remove(&blob) {
        SSH_AGENT_SUCCESS
    } else {
        SSH_AGENT_FAILURE
    }]
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
/// Request body is `string key_blob, string data, uint32 flags`. The
/// user must allow the signature first when the global confirm setting
/// is on OR the key was added with the CONFIRM constraint (a deny,
/// timeout, missing UI or closed channel yields FAILURE, the safe
/// default).
async fn sign_response<K: AgentKeySource>(
    body: &[u8],
    source: &K,
    confirm: &ConfirmMode,
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

    if confirm.all || exposed.requires_confirm {
        // No UI channel = nobody to ask = deny (the add path refuses
        // confirm-constrained keys in that case, so this only guards
        // the global setting racing a runtime without a channel).
        let Some(sender) = &confirm.sender else {
            return vec![SSH_AGENT_FAILURE];
        };
        if !ask_confirm(sender, &exposed, &key_blob, peer).await {
            return vec![SSH_AGENT_FAILURE];
        }
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
            let _ =
                serve_connection(server_side, server_source.as_ref(), &ConfirmMode::default(), None)
                    .await;
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
            let _ =
                serve_connection(server_side, server_source.as_ref(), &ConfirmMode::default(), None)
                    .await;
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
        assert_eq!(respond_sync(&[99], &source, false), Some(vec![SSH_AGENT_FAILURE]));
        // On the read-only default, the whole write family answers
        // FAILURE: ADD 17/25 (empty body = parse failure, and the
        // source would refuse anyway), REMOVE 18/19, LOCK 22,
        // UNLOCK 23, extensions 27.
        for op in [17u8, 18, 19, 22, 23, 25, 27] {
            assert_eq!(respond_sync(&[op], &source, false), Some(vec![SSH_AGENT_FAILURE]), "op {op}");
        }
        // Empty payload.
        assert_eq!(respond_sync(&[], &source, false), Some(vec![SSH_AGENT_FAILURE]));
        // SIGN_REQUEST is the async path: the sync layer declines it.
        assert_eq!(respond_sync(&[13, 0, 0, 0, 0], &source, false), None);
    }

    /// Spawn a serve task over a duplex and hand back the connected
    /// russh client (the server task is detached; dropping the client
    /// ends it).
    fn client_over(
        source: std::sync::Arc<MockKeySource>,
        confirm: ConfirmMode,
    ) -> russh::keys::agent::client::AgentClient<tokio::io::DuplexStream> {
        let (client_side, server_side) = tokio::io::duplex(64 * 1024);
        tokio::spawn(async move {
            let _ = serve_connection(server_side, source.as_ref(), &confirm, None).await;
        });
        russh::keys::agent::client::AgentClient::connect(client_side)
    }

    #[tokio::test]
    async fn add_list_sign_remove_roundtrip() {
        for spec in [
            GenerateSpec::Ed25519,
            GenerateSpec::Ecdsa { curve: EcdsaCurveChoice::P256 },
            GenerateSpec::Rsa { bits: RsaBits::B2048 },
        ] {
            let source = std::sync::Arc::new(MockKeySource::writable(vec![]));
            let mut client = client_over(source.clone(), ConfirmMode::default());

            // ADD: russh encodes the draft-miller body; our decode must
            // accept it for every algorithm family we sign.
            let key = gen_key(spec);
            client.add_identity(&key, &[]).await.expect("add accepted");

            let identities = client.request_identities().await.unwrap();
            assert_eq!(identities.len(), 1, "added key is advertised");

            // SIGN with the added key, driven end-to-end by the client.
            let signed = client
                .sign_request(&identities[0], None, b"ephemeral sign".to_vec())
                .await
                .expect("added key signs");
            assert!(!signed.is_empty());

            // REMOVE by public key: the roster empties.
            client
                .remove_identity(key.public_key())
                .await
                .expect("remove accepted");
            assert!(client.request_identities().await.unwrap().is_empty());
        }
    }

    #[tokio::test]
    async fn add_refused_on_readonly_source() {
        // The default (opt-out) source: the same ADD that succeeds on a
        // writable source must answer FAILURE here.
        let source = std::sync::Arc::new(MockKeySource::new(vec![]));
        let mut client = client_over(source.clone(), ConfirmMode::default());
        let key = gen_key(GenerateSpec::Ed25519);
        assert!(client.add_identity(&key, &[]).await.is_err(), "read-only refuses ADD");
        assert!(client.request_identities().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn remove_all_spares_vault_keys() {
        let vault_key = gen_key(GenerateSpec::Ed25519);
        let source =
            std::sync::Arc::new(MockKeySource::writable(vec![(vault_key.clone(), "vault".into())]));
        let mut client = client_over(source.clone(), ConfirmMode::default());

        client.add_identity(&gen_key(GenerateSpec::Ed25519), &[]).await.unwrap();
        assert_eq!(client.request_identities().await.unwrap().len(), 2);

        // REMOVE_ALL clears only the added key; REMOVE of the vault
        // key's blob is refused outright. Driven at the message layer:
        // russh 0.61's `remove_all_identities` is broken as a client
        // (it never patches the frame length, sending `len=0`, which
        // this server rightly drops), so the framed path can't carry
        // this one. `ssh-add -D` sends the correct 1-byte frame.
        assert_eq!(
            respond_sync(&[SSH_AGENTC_REMOVE_ALL_IDENTITIES], source.as_ref(), false),
            Some(vec![SSH_AGENT_SUCCESS]),
        );
        assert_eq!(client.request_identities().await.unwrap().len(), 1, "vault key survives");

        // REMOVE of the vault key's blob answers FAILURE. Message layer
        // again: russh's `remove_identity` reads the response without
        // checking it (read_response, not read_success), so the client
        // cannot observe the refusal.
        let mut payload = vec![SSH_AGENTC_REMOVE_IDENTITY];
        let mut blob = Vec::new();
        vault_key.public_key().key_data().encode(&mut blob).unwrap();
        blob.encode(&mut payload).unwrap();
        assert_eq!(
            respond_sync(&payload, source.as_ref(), false),
            Some(vec![SSH_AGENT_FAILURE]),
            "vault keys are not removable over the wire",
        );
        assert_eq!(client.request_identities().await.unwrap().len(), 1, "roster untouched");
    }

    #[tokio::test]
    async fn confirm_constraint_prompts_even_with_global_confirm_off() {
        use russh::keys::agent::Constraint;

        let source = std::sync::Arc::new(MockKeySource::writable(vec![]));
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        // Global confirm OFF, but the channel exists: only
        // confirm-constrained keys prompt.
        let confirm = ConfirmMode { sender: Some(tx), all: false };
        let mut client = client_over(source.clone(), confirm);

        // Answer every prompt with "allow" and count them.
        let prompts = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let seen = prompts.clone();
        tokio::spawn(async move {
            while let Some(ask) = rx.recv().await {
                seen.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let _ = ask.respond.send(true);
            }
        });

        client
            .add_identity(&gen_key(GenerateSpec::Ed25519), &[Constraint::Confirm])
            .await
            .expect("constrained add accepted");
        let identities = client.request_identities().await.unwrap();
        let signed = client
            .sign_request(&identities[0], None, b"confirmed".to_vec())
            .await
            .expect("sign allowed by the prompt");
        assert!(!signed.is_empty());
        assert_eq!(prompts.load(std::sync::atomic::Ordering::SeqCst), 1, "exactly one prompt");
    }

    #[tokio::test]
    async fn confirm_constraint_refused_without_ui() {
        use russh::keys::agent::Constraint;

        // No confirm channel (headless): accepting a confirm-
        // constrained key would mean silent denies at sign time, so
        // the ADD itself must be refused.
        let source = std::sync::Arc::new(MockKeySource::writable(vec![]));
        let mut client = client_over(source.clone(), ConfirmMode::default());
        assert!(
            client
                .add_identity(&gen_key(GenerateSpec::Ed25519), &[Constraint::Confirm])
                .await
                .is_err(),
        );
        assert!(client.request_identities().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn unknown_constraint_refuses_add() {
        use russh::keys::agent::Constraint;

        let source = std::sync::Arc::new(MockKeySource::writable(vec![]));
        let mut client = client_over(source.clone(), ConfirmMode::default());
        // An extension constraint we do not recognize: the draft says
        // refuse the add, never silently drop the constraint.
        assert!(
            client
                .add_identity(
                    &gen_key(GenerateSpec::Ed25519),
                    &[Constraint::Extensions { name: b"nope@example.com".to_vec(), details: vec![] }],
                )
                .await
                .is_err(),
        );
        assert!(client.request_identities().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn lifetime_constraint_expires_the_key() {
        use russh::keys::agent::Constraint;

        let source = std::sync::Arc::new(MockKeySource::writable(vec![]));
        let mut client = client_over(source.clone(), ConfirmMode::default());
        client
            .add_identity(&gen_key(GenerateSpec::Ed25519), &[Constraint::KeyLifetime { seconds: 1 }])
            .await
            .expect("lifetime add accepted");
        assert_eq!(client.request_identities().await.unwrap().len(), 1);

        // Past the deadline the key is pruned on the next access.
        tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
        assert!(client.request_identities().await.unwrap().is_empty(), "expired key gone");
    }
}
