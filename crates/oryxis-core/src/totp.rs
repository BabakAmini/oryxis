//! RFC 6238 TOTP generation for keyboard-interactive 2FA autofill.
//!
//! The vault stores the user's raw input verbatim (either a bare Base32
//! secret or a full `otpauth://totp/...` URI); parsing happens here at
//! code-generation time so a stored URI keeps its digits / period /
//! algorithm parameters and a bare secret gets the universal defaults
//! (SHA-1, 6 digits, 30 s), which is what every major provider issues.

use hmac::{Hmac, Mac};
use sha1::Sha1;
use sha2::{Sha256, Sha512};

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum TotpError {
    #[error("empty TOTP secret")]
    Empty,
    #[error("invalid Base32 secret")]
    BadBase32,
    #[error("otpauth URI is not a totp:// type")]
    NotTotp,
    #[error("otpauth URI has no secret parameter")]
    MissingSecret,
    #[error("unsupported TOTP algorithm: {0}")]
    BadAlgorithm(String),
    #[error("invalid digits parameter (must be 6..=10)")]
    BadDigits,
    #[error("invalid period parameter")]
    BadPeriod,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TotpAlgorithm {
    Sha1,
    Sha256,
    Sha512,
}

/// A parsed, ready-to-generate TOTP configuration.
#[derive(Clone, PartialEq)]
pub struct Totp {
    key: Vec<u8>,
    pub digits: u32,
    pub period: u64,
    pub algorithm: TotpAlgorithm,
}

/// Manual impl: the derived `Debug` would print the raw decoded seed,
/// which is credential material one `{:?}` in a log line away from
/// leaking. The key is redacted; everything else prints normally.
impl std::fmt::Debug for Totp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Totp")
            .field("key", &"[redacted]")
            .field("digits", &self.digits)
            .field("period", &self.period)
            .field("algorithm", &self.algorithm)
            .finish()
    }
}

/// Best-effort key hygiene: wipe the decoded seed when the value is
/// dropped, so it doesn't linger on freed heap pages between the
/// autofill that used it and whenever the allocator reuses the block.
/// Volatile writes so the optimizer can't elide the "dead" stores;
/// hand-rolled because `oryxis-core` doesn't otherwise depend on the
/// `zeroize` crate.
impl Drop for Totp {
    fn drop(&mut self) {
        for b in self.key.iter_mut() {
            // SAFETY: `b` is a valid, aligned `&mut u8` from iter_mut.
            unsafe { std::ptr::write_volatile(b, 0) };
        }
        std::sync::atomic::compiler_fence(std::sync::atomic::Ordering::SeqCst);
    }
}

impl Totp {
    /// Parse the user's stored input: a full `otpauth://` URI or a bare
    /// Base32 secret (spaces and dashes tolerated, case-insensitive,
    /// padding optional, the formats authenticator apps display).
    pub fn parse(input: &str) -> Result<Self, TotpError> {
        let input = input.trim();
        if input.is_empty() {
            return Err(TotpError::Empty);
        }
        if input.len() >= 10 && input[..10].eq_ignore_ascii_case("otpauth://") {
            return Self::parse_otpauth(input);
        }
        Ok(Self {
            key: decode_base32(input)?,
            digits: 6,
            period: 30,
            algorithm: TotpAlgorithm::Sha1,
        })
    }

    fn parse_otpauth(uri: &str) -> Result<Self, TotpError> {
        // otpauth://TYPE/LABEL?secret=...&digits=...&period=...&algorithm=...
        let rest = &uri[10..];
        let (ty, rest) = rest.split_once('/').ok_or(TotpError::NotTotp)?;
        if !ty.eq_ignore_ascii_case("totp") {
            return Err(TotpError::NotTotp);
        }
        let query = rest.split_once('?').map(|(_, q)| q).unwrap_or("");

        let mut secret: Option<&str> = None;
        let mut digits: u32 = 6;
        let mut period: u64 = 30;
        let mut algorithm = TotpAlgorithm::Sha1;
        for pair in query.split('&') {
            let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
            if k.eq_ignore_ascii_case("secret") {
                secret = Some(v);
            } else if k.eq_ignore_ascii_case("digits") {
                digits = v.parse().map_err(|_| TotpError::BadDigits)?;
            } else if k.eq_ignore_ascii_case("period") {
                period = v.parse().map_err(|_| TotpError::BadPeriod)?;
            } else if k.eq_ignore_ascii_case("algorithm") {
                algorithm = match v.to_ascii_uppercase().as_str() {
                    "SHA1" => TotpAlgorithm::Sha1,
                    "SHA256" => TotpAlgorithm::Sha256,
                    "SHA512" => TotpAlgorithm::Sha512,
                    other => return Err(TotpError::BadAlgorithm(other.to_string())),
                };
            }
            // Unknown parameters (issuer, image, ...) are ignored.
        }
        if !(6..=10).contains(&digits) {
            return Err(TotpError::BadDigits);
        }
        if period == 0 {
            return Err(TotpError::BadPeriod);
        }
        let secret = secret.ok_or(TotpError::MissingSecret)?;
        Ok(Self {
            key: decode_base32(secret)?,
            digits,
            period,
            algorithm,
        })
    }

    /// The code for a given Unix timestamp, zero-padded to `digits`.
    pub fn code_at(&self, unix_secs: u64) -> String {
        let counter = (unix_secs / self.period).to_be_bytes();
        let hash = match self.algorithm {
            TotpAlgorithm::Sha1 => hmac_digest::<Hmac<Sha1>>(&self.key, &counter),
            TotpAlgorithm::Sha256 => hmac_digest::<Hmac<Sha256>>(&self.key, &counter),
            TotpAlgorithm::Sha512 => hmac_digest::<Hmac<Sha512>>(&self.key, &counter),
        };
        // RFC 4226 dynamic truncation, generalized to the validated
        // 6..=10 digit range. The modulo runs in u64: 10^10 overflows
        // u32, and the previous `min(9)` clamp silently collapsed a
        // 10-digit configuration to a zero-padded 9-digit code, i.e.
        // codes a server expecting 10 digits would reject (the 31-bit
        // truncated value never exceeds 10 decimal digits, so the u64
        // modulo is exact).
        let offset = (hash[hash.len() - 1] & 0x0f) as usize;
        let binary = (u32::from(hash[offset] & 0x7f) << 24)
            | (u32::from(hash[offset + 1]) << 16)
            | (u32::from(hash[offset + 2]) << 8)
            | u32::from(hash[offset + 3]);
        let code = u64::from(binary) % 10u64.pow(self.digits.min(10));
        format!("{code:0width$}", width = self.digits as usize)
    }

    /// The code for the current system time.
    pub fn code_now(&self) -> String {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        self.code_at(now)
    }
}

fn hmac_digest<M: Mac + hmac::digest::KeyInit>(key: &[u8], data: &[u8]) -> Vec<u8> {
    // HMAC accepts any key length, so new_from_slice can't fail.
    let mut mac = <M as hmac::digest::KeyInit>::new_from_slice(key)
        .expect("HMAC accepts any key size");
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}

/// Decode a Base32 (RFC 4648, no padding required) secret, tolerating
/// the cosmetic spacing / dashes / lowercase that provider UIs display.
fn decode_base32(s: &str) -> Result<Vec<u8>, TotpError> {
    let cleaned: String = s
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .map(|c| c.to_ascii_uppercase())
        .collect();
    let cleaned = cleaned.trim_end_matches('=');
    if cleaned.is_empty() {
        return Err(TotpError::Empty);
    }
    data_encoding::BASE32_NOPAD
        .decode(cleaned.as_bytes())
        .map_err(|_| TotpError::BadBase32)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RFC 6238 Appendix B test vectors. The RFC uses 8 digits and
    /// per-algorithm keys built by repeating the ASCII seed
    /// "12345678901234567890" to the hash's key length.
    fn rfc_totp(algorithm: TotpAlgorithm) -> Totp {
        let seed = b"12345678901234567890";
        let len = match algorithm {
            TotpAlgorithm::Sha1 => 20,
            TotpAlgorithm::Sha256 => 32,
            TotpAlgorithm::Sha512 => 64,
        };
        let key: Vec<u8> = seed.iter().cycle().take(len).copied().collect();
        Totp { key, digits: 8, period: 30, algorithm }
    }

    #[test]
    fn rfc6238_sha1_vectors() {
        let t = rfc_totp(TotpAlgorithm::Sha1);
        assert_eq!(t.code_at(59), "94287082");
        assert_eq!(t.code_at(1111111109), "07081804");
        assert_eq!(t.code_at(1234567890), "89005924");
        assert_eq!(t.code_at(20000000000), "65353130");
    }

    #[test]
    fn ten_digit_codes_keep_the_full_truncation() {
        // The full 31-bit truncated value for the SHA-1 vector at t=59
        // is 1094287082 (its low 8 digits are the RFC's "94287082").
        // The old u32 arithmetic clamped 10-digit configs to 9 digits
        // and would have produced "0094287082".
        let mut t = rfc_totp(TotpAlgorithm::Sha1);
        t.digits = 10;
        assert_eq!(t.code_at(59), "1094287082");
    }

    #[test]
    fn rfc6238_sha256_vectors() {
        let t = rfc_totp(TotpAlgorithm::Sha256);
        assert_eq!(t.code_at(59), "46119246");
        assert_eq!(t.code_at(1111111109), "68084774");
        assert_eq!(t.code_at(20000000000), "77737706");
    }

    #[test]
    fn rfc6238_sha512_vectors() {
        let t = rfc_totp(TotpAlgorithm::Sha512);
        assert_eq!(t.code_at(59), "90693936");
        assert_eq!(t.code_at(1111111109), "25091201");
        assert_eq!(t.code_at(20000000000), "47863826");
    }

    #[test]
    fn bare_base32_gets_defaults() {
        let t = Totp::parse("JBSWY3DPEHPK3PXP").unwrap();
        assert_eq!(t.digits, 6);
        assert_eq!(t.period, 30);
        assert_eq!(t.algorithm, TotpAlgorithm::Sha1);
        // Cosmetic formatting must not change the key.
        let spaced = Totp::parse("jbsw y3dp ehpk 3pxp").unwrap();
        let dashed = Totp::parse("JBSW-Y3DP-EHPK-3PXP").unwrap();
        assert_eq!(t, spaced);
        assert_eq!(t, dashed);
        // 6 digits, zero-padded.
        assert_eq!(t.code_at(0).len(), 6);
    }

    #[test]
    fn otpauth_uri_parses_parameters() {
        let t = Totp::parse(
            "otpauth://totp/Example:alice@host?secret=JBSWY3DPEHPK3PXP&issuer=Example&algorithm=SHA256&digits=8&period=60",
        )
        .unwrap();
        assert_eq!(t.digits, 8);
        assert_eq!(t.period, 60);
        assert_eq!(t.algorithm, TotpAlgorithm::Sha256);
    }

    #[test]
    fn otpauth_rejects_hotp_and_missing_secret() {
        assert_eq!(
            Totp::parse("otpauth://hotp/x?secret=JBSWY3DPEHPK3PXP"),
            Err(TotpError::NotTotp)
        );
        assert_eq!(
            Totp::parse("otpauth://totp/x?issuer=Example"),
            Err(TotpError::MissingSecret)
        );
    }

    #[test]
    fn invalid_inputs_error_cleanly() {
        assert_eq!(Totp::parse("   "), Err(TotpError::Empty));
        assert_eq!(Totp::parse("not!base32@@"), Err(TotpError::BadBase32));
        assert!(matches!(
            Totp::parse("otpauth://totp/x?secret=JBSWY3DPEHPK3PXP&algorithm=MD5"),
            Err(TotpError::BadAlgorithm(_))
        ));
        assert_eq!(
            Totp::parse("otpauth://totp/x?secret=JBSWY3DPEHPK3PXP&digits=4"),
            Err(TotpError::BadDigits)
        );
        assert_eq!(
            Totp::parse("otpauth://totp/x?secret=JBSWY3DPEHPK3PXP&period=0"),
            Err(TotpError::BadPeriod)
        );
    }

    #[test]
    fn debug_never_prints_the_key() {
        let t = Totp::parse("JBSWY3DPEHPK3PXP").unwrap();
        let dbg = format!("{t:?}");
        assert!(dbg.contains("[redacted]"), "unexpected Debug output: {dbg}");
        // "JBSWY3DPEHPK3PXP" decodes to b"Hello!\xde\xad\xbe\xef"; neither
        // the raw bytes nor a byte listing may appear.
        assert!(!dbg.contains("Hello"), "raw key bytes leaked: {dbg}");
        assert!(!dbg.contains("72, 101"), "byte listing leaked: {dbg}");
    }

    #[test]
    fn padded_base32_accepted() {
        // Authenticator exports sometimes keep RFC 4648 padding.
        let t = Totp::parse("MFRGGZDFMZTWQ2LK====").unwrap();
        assert_eq!(t.code_at(59).len(), 6);
    }
}
