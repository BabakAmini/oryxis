//! mRemoteNG importer: parses `confCons.xml` into [`DirectHost`]s.
//!
//! The file is XML with one `<Node>` per connection (and nested
//! `Container` nodes as folders; the container path is recorded in
//! the imported host's notes so the hierarchy isn't silently lost).
//! Passwords are always encrypted, even in "plaintext" files: AES-GCM
//! (BouncyCastle defaults: 16-byte salt, 16-byte nonce) with a
//! PBKDF2-HMAC-SHA1 key over the file password, which is the literal
//! `mR3m` unless the user set one. `FullFileEncryption` wraps the
//! whole document in the same scheme.
//!
//! Both are supported: the caller first tries the default password;
//! when that fails (a real file password), the Import hub asks for it
//! and retries. A password that fails only for SOME attributes (file
//! saved across versions) degrades per-host: those import without a
//! password plus a note.
//!
//! Protocol mapping: SSH1/SSH2 -> SSH, Telnet -> Telnet, RDP -> the
//! remote-desktop launcher, VNC -> likewise. HTTP/HTTPS/Rlogin/RAW
//! nodes are named in `skipped`.

use aes_gcm::aead::Aead;
use aes_gcm::{aes::Aes256, AesGcm, KeyInit};
use base64::Engine as _;
use oryxis_core::models::connection::{Connection, ConnectionProtocol};
use oryxis_core::models::remote_desktop::RemoteDesktopKind;

use super::{DirectHost, DirectImport};

/// GCM with mRemoteNG's 16-byte nonce (BouncyCastle default), not the
/// RFC-standard 12 the `Aes256Gcm` alias fixes.
type MrngCipher = AesGcm<Aes256, aes_gcm::aead::consts::U16>;

#[derive(Debug, Clone)]
pub(crate) enum MrngParse {
    Ready(DirectImport),
    /// The file (or its passwords) need a real file password: the hub
    /// should ask and retry with it.
    NeedsPassword,
    /// Not parseable as mRemoteNG XML at all.
    Invalid,
}

/// Parse `confCons.xml` with the given file password (`None` = the
/// mRemoteNG default `mR3m`).
pub(crate) fn parse(bytes: &[u8], password: Option<&str>) -> MrngParse {
    let password = password.unwrap_or("mR3m");
    let text = String::from_utf8_lossy(bytes);
    let Some(doc) = Document::parse(&text) else {
        return MrngParse::Invalid;
    };

    // Full-file encryption: the root carries a single base64 blob
    // instead of Node children. Decrypt, then re-parse the inner XML.
    if doc.nodes.is_empty()
        && let Some(blob) = doc.inner_blob.as_deref()
    {
        let Some(inner) = decrypt_blob(blob, password, doc.kdf_iterations) else {
            return MrngParse::NeedsPassword;
        };
        let Ok(inner) = String::from_utf8(inner) else {
            return MrngParse::Invalid;
        };
        let Some(inner_doc) = Document::parse(&inner) else {
            return MrngParse::Invalid;
        };
        return MrngParse::Ready(map_nodes(&inner_doc, password));
    }

    if doc.nodes.is_empty() {
        return MrngParse::Invalid;
    }

    // Wrong file password shows up as EVERY password attribute
    // failing its GCM tag while at least one is present: that is a
    // protected file, not a broken one, so ask instead of importing
    // a batch of password-less hosts nobody wanted.
    let with_password = doc
        .nodes
        .iter()
        .filter(|n| !n.password_blob.is_empty())
        .count();
    if with_password > 0 {
        let decodable = doc
            .nodes
            .iter()
            .filter(|n| !n.password_blob.is_empty())
            .filter(|n| {
                decrypt_blob(&n.password_blob, password, doc.kdf_iterations).is_some()
            })
            .count();
        if decodable == 0 {
            return MrngParse::NeedsPassword;
        }
    }

    MrngParse::Ready(map_nodes(&doc, password))
}

fn map_nodes(doc: &Document, password: &str) -> DirectImport {
    let mut out = DirectImport {
        source_key: "import_mremoteng_btn",
        hosts: Vec::new(),
        skipped: Vec::new(),
    };
    for node in &doc.nodes {
        let hostname = node.hostname.trim();
        if hostname.is_empty() {
            out.skipped.push(node.name.clone());
            continue;
        }
        let (protocol, rd_kind, default_port) = match node.protocol.as_str() {
            "SSH1" | "SSH2" => (ConnectionProtocol::Ssh, None, 22),
            "Telnet" => (ConnectionProtocol::Telnet, None, 23),
            "RDP" => (
                ConnectionProtocol::RemoteDesktop,
                Some(RemoteDesktopKind::Rdp),
                3389,
            ),
            "VNC" => (
                ConnectionProtocol::RemoteDesktop,
                Some(RemoteDesktopKind::Vnc),
                5900,
            ),
            _ => {
                out.skipped.push(node.name.clone());
                continue;
            }
        };
        let mut conn = Connection::new(node.name.clone(), hostname.to_string());
        conn.protocol = protocol;
        if let Some(kind) = rd_kind {
            conn.rd_kind = kind;
        }
        conn.port = node.port.unwrap_or(default_port);
        if !node.username.is_empty() {
            conn.username = Some(node.username.clone());
        }
        let mut notes = format!("Imported from mRemoteNG (node `{}`)", node.name);
        if !node.container_path.is_empty() {
            notes.push_str(&format!("\nmRemoteNG folder: {}", node.container_path));
        }
        let password = if node.password_blob.is_empty() {
            None
        } else {
            match decrypt_blob(&node.password_blob, password, doc.kdf_iterations)
                .and_then(|p| String::from_utf8(p).ok())
                .filter(|p| !p.is_empty())
            {
                Some(p) => Some(p),
                None => {
                    notes.push_str(
                        "\nStored password could not be decoded, set it manually",
                    );
                    None
                }
            }
        };
        conn.notes = Some(notes);
        out.hosts.push(DirectHost { conn, password });
    }
    out
}

/// Decrypt one mRemoteNG blob: base64( salt[16] || nonce[16] ||
/// ciphertext||tag ), key = PBKDF2-HMAC-SHA1(password, salt,
/// iterations, 32).
fn decrypt_blob(blob: &str, password: &str, iterations: u32) -> Option<Vec<u8>> {
    let raw = base64::engine::general_purpose::STANDARD
        .decode(blob.trim())
        .ok()?;
    if raw.len() < 33 {
        return None;
    }
    let (salt, rest) = raw.split_at(16);
    let (nonce, ciphertext) = rest.split_at(16);
    let mut key = [0u8; 32];
    pbkdf2::pbkdf2_hmac::<sha1::Sha1>(password.as_bytes(), salt, iterations, &mut key);
    let cipher = MrngCipher::new((&key).into());
    let nonce = aes_gcm::Nonce::try_from(nonce).ok()?;
    cipher.decrypt(&nonce, ciphertext).ok()
}

/// The slice of confCons.xml we care about, pulled with quick-xml:
/// every `Type="Connection"` node (with its container path) plus the
/// root's KDF/encryption attributes.
struct Document {
    kdf_iterations: u32,
    /// Base64 payload of a fully-encrypted file (the root's text).
    inner_blob: Option<String>,
    nodes: Vec<RawNode>,
}

#[derive(Default, Clone)]
struct RawNode {
    name: String,
    hostname: String,
    port: Option<u16>,
    username: String,
    password_blob: String,
    protocol: String,
    container_path: String,
}

impl Document {
    fn parse(text: &str) -> Option<Document> {
        use quick_xml::events::Event;
        let mut reader = quick_xml::Reader::from_str(text);
        reader.config_mut().trim_text(true);
        let mut kdf_iterations = 1000;
        let mut inner_blob: Option<String> = None;
        let mut nodes: Vec<RawNode> = Vec::new();
        let mut containers: Vec<String> = Vec::new();
        let mut saw_root = false;
        let mut in_root_text = false;
        let mut buf = Vec::new();
        loop {
            match reader.read_event_into(&mut buf) {
                Ok(Event::Start(e)) | Ok(Event::Empty(e)) => {
                    let name = e.name();
                    let tag = std::str::from_utf8(name.as_ref()).ok()?;
                    if tag == "Connections" || tag.ends_with(":Connections") {
                        saw_root = true;
                        in_root_text = true;
                        for attr in e.attributes().flatten() {
                            let key =
                                std::str::from_utf8(attr.key.as_ref()).ok()?.to_string();
                            let value = attr
                                .normalized_value(quick_xml::XmlVersion::Implicit1_0)
                                .ok()?
                                .into_owned();
                            if key == "KdfIterations" {
                                // File-controlled and driving PBKDF2 once
                                // per password blob: unbounded, a hostile
                                // file sets 4 billion and turns the parse
                                // into hours of key stretching. mRemoteNG
                                // itself defaults to 1000 and its UI caps
                                // at 50k; 1M is far above any real file
                                // and still finishes in well under a
                                // second per blob.
                                kdf_iterations =
                                    value.parse().map(|n: u32| n.min(1_000_000)).unwrap_or(1000);
                            }
                        }
                        continue;
                    }
                    if tag != "Node" {
                        continue;
                    }
                    in_root_text = false;
                    let mut node = RawNode {
                        container_path: containers.join(" / "),
                        ..Default::default()
                    };
                    let mut node_type = String::new();
                    for attr in e.attributes().flatten() {
                        let key = std::str::from_utf8(attr.key.as_ref()).ok()?;
                        // Same replacement as above: the deprecated
                        // `unescape_value` was exactly this call.
                        let value = attr
                            .normalized_value(quick_xml::XmlVersion::Implicit1_0)
                            .ok()?
                            .into_owned();
                        match key {
                            "Name" => node.name = value,
                            "Type" => node_type = value,
                            "Hostname" => node.hostname = value,
                            "Port" => node.port = value.parse().ok(),
                            "Username" => node.username = value,
                            "Password" => node.password_blob = value,
                            "Protocol" => node.protocol = value,
                            _ => {}
                        }
                    }
                    if node_type == "Container" {
                        // Only Start events push depth; an Empty
                        // container has no children to scope.
                        containers.push(node.name.clone());
                    } else if node_type == "Connection" || node_type.is_empty() {
                        nodes.push(node);
                    }
                }
                Ok(Event::End(e)) => {
                    let name = e.name();
                    let tag = std::str::from_utf8(name.as_ref()).ok()?;
                    if tag == "Node" {
                        // End events fire for Start containers only
                        // (connections are Empty tags in practice, but
                        // a Start/End connection pair is harmless: it
                        // pushed nothing).
                        containers.pop();
                    }
                }
                Ok(Event::Text(t)) => {
                    if saw_root && in_root_text {
                        // quick-xml 0.41 split what `BytesText::unescape`
                        // used to do into its two halves: `decode` handles
                        // the encoding, `escape::unescape` resolves the
                        // predefined entities. Both are needed here, since
                        // the payload is base64 that an `&amp;` would
                        // corrupt.
                        let decoded = t.decode().ok()?;
                        let text = quick_xml::escape::unescape(&decoded)
                            .ok()?
                            .trim()
                            .to_string();
                        if !text.is_empty() {
                            inner_blob = Some(text);
                        }
                    }
                }
                Ok(Event::Eof) => break,
                Ok(_) => {}
                Err(_) => return None,
            }
            buf.clear();
        }
        saw_root.then_some(Document {
            kdf_iterations,
            inner_blob,
            nodes,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Encrypt like mRemoteNG does, to build test vectors.
    fn encrypt_blob(plain: &[u8], password: &str, iterations: u32) -> String {
        let salt = [7u8; 16];
        // aead 0.6 dropped its OsRng re-export; the nonce is just
        // random bytes of the cipher's nonce size.
        let mut nonce_bytes = [0u8; 16];
        getrandom::fill(&mut nonce_bytes).expect("OS RNG unavailable");
        let nonce = aes_gcm::Nonce::<aes_gcm::aead::consts::U16>::from(nonce_bytes);
        let mut key = [0u8; 32];
        pbkdf2::pbkdf2_hmac::<sha1::Sha1>(
            password.as_bytes(),
            &salt,
            iterations,
            &mut key,
        );
        let cipher = MrngCipher::new((&key).into());
        let ciphertext = cipher.encrypt(&nonce, plain).unwrap();
        let mut raw = salt.to_vec();
        raw.extend_from_slice(&nonce);
        raw.extend_from_slice(&ciphertext);
        base64::engine::general_purpose::STANDARD.encode(raw)
    }

    fn sample_xml(password_blob: &str) -> String {
        format!(
            r#"<?xml version="1.0" encoding="utf-8"?>
<mrng:Connections xmlns:mrng="http://mremoteng.org" Name="Connections" EncryptionEngine="AES" BlockCipherMode="GCM" KdfIterations="1000" FullFileEncryption="false" Protected="x" ConfVersion="2.6">
    <Node Name="Prod" Type="Container" Expanded="true">
        <Node Name="web-1" Type="Connection" Hostname="web1.example.com" Port="2222" Username="deploy" Password="{password_blob}" Protocol="SSH2" />
        <Node Name="win-box" Type="Connection" Hostname="10.0.0.9" Port="3389" Username="admin" Password="" Protocol="RDP" />
    </Node>
    <Node Name="intranet" Type="Connection" Hostname="wiki.corp" Port="443" Username="" Password="" Protocol="HTTPS" />
</mrng:Connections>"#
        )
    }

    #[test]
    fn parses_nodes_with_default_password() {
        let blob = encrypt_blob(b"hunter2", "mR3m", 1000);
        let MrngParse::Ready(import) = parse(sample_xml(&blob).as_bytes(), None) else {
            panic!("expected a ready batch");
        };
        assert_eq!(import.hosts.len(), 2);
        assert_eq!(import.skipped, vec!["intranet".to_string()]);

        let ssh = &import.hosts[0];
        assert_eq!(ssh.conn.label, "web-1");
        assert_eq!(ssh.conn.port, 2222);
        assert_eq!(ssh.conn.protocol, ConnectionProtocol::Ssh);
        assert_eq!(ssh.password.as_deref(), Some("hunter2"));
        // The container hierarchy survives in the notes.
        assert!(ssh.conn.notes.as_deref().unwrap().contains("Prod"));

        let rdp = &import.hosts[1];
        assert_eq!(rdp.conn.protocol, ConnectionProtocol::RemoteDesktop);
        assert_eq!(rdp.conn.rd_kind, RemoteDesktopKind::Rdp);
        assert_eq!(rdp.password, None);
    }

    #[test]
    fn a_real_file_password_asks_instead_of_stripping() {
        let blob = encrypt_blob(b"hunter2", "s3cret-file-pw", 1000);
        let xml = sample_xml(&blob);
        // Default password fails every present blob: ask.
        assert!(matches!(parse(xml.as_bytes(), None), MrngParse::NeedsPassword));
        // The right one goes through.
        let MrngParse::Ready(import) = parse(xml.as_bytes(), Some("s3cret-file-pw"))
        else {
            panic!("expected a ready batch with the file password");
        };
        assert_eq!(import.hosts[0].password.as_deref(), Some("hunter2"));
    }

    #[test]
    fn full_file_encryption_roundtrip() {
        let inner = sample_xml("");
        let blob = encrypt_blob(inner.as_bytes(), "filepw", 1000);
        let outer = format!(
            r#"<?xml version="1.0" encoding="utf-8"?>
<mrng:Connections xmlns:mrng="http://mremoteng.org" Name="Connections" EncryptionEngine="AES" BlockCipherMode="GCM" KdfIterations="1000" FullFileEncryption="true" Protected="x" ConfVersion="2.6">{blob}</mrng:Connections>"#
        );
        assert!(matches!(parse(outer.as_bytes(), None), MrngParse::NeedsPassword));
        let MrngParse::Ready(import) = parse(outer.as_bytes(), Some("filepw")) else {
            panic!("expected the decrypted inner document");
        };
        assert_eq!(import.hosts.len(), 2);
    }

    #[test]
    fn garbage_is_invalid_not_a_prompt() {
        assert!(matches!(parse(b"not xml", None), MrngParse::Invalid));
        assert!(matches!(
            parse(b"<other><thing/></other>", None),
            MrngParse::Invalid
        ));
    }
}
