//! `.Xauthority` reader.
//!
//! The file is a flat sequence of records, every field length-prefixed
//! with a BIG-ENDIAN `u16` regardless of host byte order:
//!
//! ```text
//! family:u16 addr_len:u16 addr  number_len:u16 number
//! name_len:u16 name  data_len:u16 data
//! ```
//!
//! `number` is the display number in ASCII (`"0"`), `name` the auth
//! protocol (`"MIT-MAGIC-COOKIE-1"`) and `data` the raw cookie bytes.
//!
//! A missing file is NOT an error: a Windows VcXsrv started with
//! "Disable access control" (`-ac`) serves an unauthenticated display,
//! and that is a legitimate configuration we must forward to.

use std::path::PathBuf;

/// Address family tags used in `.Xauthority` records.
const FAMILY_LOCAL: u16 = 256;
const FAMILY_WILD: u16 = 65535;

/// One `.Xauthority` record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XAuthEntry {
    pub family: u16,
    pub address: Vec<u8>,
    pub number: String,
    pub name: String,
    pub data: Vec<u8>,
}

/// Locate the authority file: `$XAUTHORITY` wins, else `~/.Xauthority`.
pub fn authority_path() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("XAUTHORITY") {
        let p = PathBuf::from(p);
        if !p.as_os_str().is_empty() {
            return Some(p);
        }
    }
    dirs::home_dir().map(|h| h.join(".Xauthority"))
}

/// Parse every record in an `.Xauthority` blob.
///
/// A truncated trailing record ends the scan and keeps the records read
/// so far: partially-written authority files are a real occurrence (the
/// file is rewritten in place by `xauth`) and losing every cookie over a
/// torn tail would be worse than using the intact prefix.
pub fn parse(buf: &[u8]) -> Vec<XAuthEntry> {
    let mut out = Vec::new();
    let mut pos = 0usize;

    // Read a big-endian u16 length followed by that many bytes.
    let take = |pos: &mut usize, buf: &[u8]| -> Option<Vec<u8>> {
        let len = read_u16(buf, *pos)? as usize;
        *pos += 2;
        let end = pos.checked_add(len)?;
        if end > buf.len() {
            return None;
        }
        let slice = buf[*pos..end].to_vec();
        *pos = end;
        Some(slice)
    };

    while let Some(family) = read_u16(buf, pos) {
        pos += 2;

        let Some(address) = take(&mut pos, buf) else { break };
        let Some(number) = take(&mut pos, buf) else { break };
        let Some(name) = take(&mut pos, buf) else { break };
        let Some(data) = take(&mut pos, buf) else { break };

        out.push(XAuthEntry {
            family,
            address,
            number: String::from_utf8_lossy(&number).into_owned(),
            name: String::from_utf8_lossy(&name).into_owned(),
            data,
        });
    }

    out
}

fn read_u16(buf: &[u8], pos: usize) -> Option<u16> {
    let hi = *buf.get(pos)? as u16;
    let lo = *buf.get(pos + 1)? as u16;
    Some((hi << 8) | lo)
}

/// Pick the cookie for `display_number`, preferring an entry bound to
/// this machine.
///
/// Ranking, best first:
///   1. `FamilyLocal` (or wild) entry whose display number matches,
///   2. any other entry whose display number matches.
///
/// Entries for a *different* display are never returned: handing the X
/// server a cookie minted for another display just yields a silent auth
/// rejection with no diagnostic.
pub fn select<'a>(
    entries: &'a [XAuthEntry],
    display_number: u32,
    hostname: Option<&str>,
) -> Option<&'a XAuthEntry> {
    let want = display_number.to_string();
    let matching = || entries.iter().filter(|e| e.number == want);

    // Exact host match on a local-family entry is the strongest signal.
    if let Some(host) = hostname
        && let Some(e) = matching()
            .find(|e| e.family == FAMILY_LOCAL && e.address.as_slice() == host.as_bytes())
    {
        return Some(e);
    }

    matching()
        .find(|e| e.family == FAMILY_LOCAL || e.family == FAMILY_WILD)
        .or_else(|| matching().next())
}

/// Best-effort local hostname, used only to disambiguate between several
/// `FamilyLocal` records. `None` simply relaxes the match.
pub fn hostname() -> Option<String> {
    #[cfg(windows)]
    {
        std::env::var("COMPUTERNAME").ok().filter(|s| !s.is_empty())
    }
    #[cfg(not(windows))]
    {
        // `/proc` first (always current), then the env as a fallback for
        // non-Linux unices where the file does not exist.
        std::fs::read_to_string("/proc/sys/kernel/hostname")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .or_else(|| std::env::var("HOSTNAME").ok().filter(|s| !s.is_empty()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build one wire-format record.
    fn record(family: u16, addr: &[u8], number: &str, name: &str, data: &[u8]) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(&family.to_be_bytes());
        for field in [addr, number.as_bytes(), name.as_bytes(), data] {
            v.extend_from_slice(&(field.len() as u16).to_be_bytes());
            v.extend_from_slice(field);
        }
        v
    }

    #[test]
    fn parses_a_single_record() {
        let raw = record(FAMILY_LOCAL, b"myhost", "0", "MIT-MAGIC-COOKIE-1", &[0xAB; 16]);
        let got = parse(&raw);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].family, FAMILY_LOCAL);
        assert_eq!(got[0].address, b"myhost");
        assert_eq!(got[0].number, "0");
        assert_eq!(got[0].name, "MIT-MAGIC-COOKIE-1");
        assert_eq!(got[0].data, vec![0xAB; 16]);
    }

    #[test]
    fn parses_multiple_records() {
        let mut raw = record(FAMILY_LOCAL, b"a", "0", "MIT-MAGIC-COOKIE-1", &[1; 16]);
        raw.extend(record(FAMILY_LOCAL, b"b", "1", "MIT-MAGIC-COOKIE-1", &[2; 16]));
        let got = parse(&raw);
        assert_eq!(got.len(), 2);
        assert_eq!(got[1].number, "1");
        assert_eq!(got[1].data, vec![2; 16]);
    }

    /// Lengths are big-endian even on little-endian hosts; a native-endian
    /// read would produce a wildly wrong length here.
    #[test]
    fn lengths_are_big_endian() {
        // A 256-byte address encodes as 0x01 0x00, not 0x00 0x01.
        let raw = record(FAMILY_LOCAL, &[7u8; 256], "0", "X", &[9]);
        let got = parse(&raw);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].address.len(), 256);
    }

    #[test]
    fn truncated_tail_keeps_intact_prefix() {
        let mut raw = record(FAMILY_LOCAL, b"a", "0", "MIT-MAGIC-COOKIE-1", &[1; 16]);
        let mut torn = record(FAMILY_LOCAL, b"b", "1", "MIT-MAGIC-COOKIE-1", &[2; 16]);
        torn.truncate(torn.len() - 5);
        raw.extend(torn);
        let got = parse(&raw);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].number, "0");
    }

    #[test]
    fn empty_file_yields_no_entries() {
        assert!(parse(&[]).is_empty());
    }

    #[test]
    fn select_prefers_matching_hostname() {
        let entries = parse(&{
            let mut v = record(FAMILY_LOCAL, b"other", "0", "MIT-MAGIC-COOKIE-1", &[1; 16]);
            v.extend(record(FAMILY_LOCAL, b"myhost", "0", "MIT-MAGIC-COOKIE-1", &[2; 16]));
            v
        });
        let picked = select(&entries, 0, Some("myhost")).unwrap();
        assert_eq!(picked.data, vec![2; 16]);
    }

    #[test]
    fn select_matches_display_number() {
        let entries = parse(&{
            let mut v = record(FAMILY_LOCAL, b"h", "0", "MIT-MAGIC-COOKIE-1", &[1; 16]);
            v.extend(record(FAMILY_LOCAL, b"h", "10", "MIT-MAGIC-COOKIE-1", &[2; 16]));
            v
        });
        assert_eq!(select(&entries, 10, None).unwrap().data, vec![2; 16]);
    }

    /// A cookie for another display must never be substituted in: it
    /// would fail authentication with no useful error.
    #[test]
    fn select_never_returns_a_foreign_display() {
        let entries = parse(&record(FAMILY_LOCAL, b"h", "0", "MIT-MAGIC-COOKIE-1", &[1; 16]));
        assert!(select(&entries, 7, None).is_none());
    }
}
