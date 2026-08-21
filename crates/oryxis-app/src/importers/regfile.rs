//! Shared `.reg` (regedit export) parsing bits used by the PuTTY and
//! WinSCP importers: both programs keep their sessions in the
//! registry with %XX-escaped names, and regedit exports both hives in
//! the same UTF-16LE file format.

/// Decode the bytes of a `.reg` file. regedit exports UTF-16LE with a
/// BOM (and old exports can be ANSI); both land here as text.
pub fn decode_reg_bytes(bytes: &[u8]) -> String {
    if bytes.len() >= 2 && bytes[0] == 0xFF && bytes[1] == 0xFE {
        let units: Vec<u16> = bytes[2..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u16::from_le_bytes(*c))
            .collect();
        return String::from_utf16_lossy(&units);
    }
    String::from_utf8_lossy(bytes).into_owned()
}

/// Registry session names are %XX-escaped in the key path
/// (space -> %20 and every character outside the safe set).
pub(crate) fn decode_session_name(escaped: &str) -> String {
    let bytes = escaped.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let (Some(hi), Some(lo)) = (
                (bytes[i + 1] as char).to_digit(16),
                (bytes[i + 2] as char).to_digit(16),
            )
        {
            out.push((hi * 16 + lo) as u8);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// A `.reg` value line: `"Key"="string"` or `"Key"=dword:00000016`.
/// Other value types (hex:, multi-line) exist in the format but none
/// of the session keys the importers read use them.
pub(crate) enum RegValue {
    Str(String),
    Dword(u32),
    Other,
}

impl RegValue {
    pub(crate) fn as_str(&self) -> Option<&str> {
        match self {
            RegValue::Str(s) => Some(s.as_str()),
            _ => None,
        }
    }
    pub(crate) fn as_dword(&self) -> Option<u32> {
        match self {
            RegValue::Dword(d) => Some(*d),
            _ => None,
        }
    }
}

pub(crate) fn split_reg_line(line: &str) -> Option<(&str, RegValue)> {
    let rest = line.strip_prefix('"')?;
    let quote = rest.find('"')?;
    let key = &rest[..quote];
    let rest = rest[quote + 1..].strip_prefix('=')?;
    if let Some(hex) = rest.strip_prefix("dword:") {
        return Some((key, RegValue::Dword(u32::from_str_radix(hex.trim(), 16).ok()?)));
    }
    if let Some(s) = rest.strip_prefix('"').and_then(|r| r.strip_suffix('"')) {
        // .reg escapes backslashes and quotes.
        let unescaped = s.replace("\\\\", "\\").replace("\\\"", "\"");
        return Some((key, RegValue::Str(unescaped)));
    }
    Some((key, RegValue::Other))
}
