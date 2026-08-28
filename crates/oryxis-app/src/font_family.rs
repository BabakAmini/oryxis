//! Renaming a downloaded CJK font's family, in memory, to the name the
//! text stack's per-script fallback actually asks for (issue #189).
//!
//! cosmic-text picks a font for a codepoint the primary family can't
//! draw by walking a hard-coded list of family NAMES per script. For
//! Han that list holds exactly one entry per locale, and on Unix it is
//! `Noto Sans CJK SC` (macOS asks for `PingFang SC`, Windows for its
//! own). The files we download from Google Fonts are named
//! `Noto Sans SC` / `TC` / `JP` / `KR`, a different family, so they are
//! never on that list: they are only reachable through the sweep over
//! every remaining face, ordered by distance from the requested weight,
//! where they lose to anything the system has. Worse, the variable
//! fonts declare `usWeightClass = 100` (their default instance is Thin),
//! which puts them 300 to 600 units away from every weight the UI asks
//! for.
//!
//! The visible result on a machine with no `Noto Sans CJK SC` installed
//! is that one Chinese word gets its characters from whatever fonts
//! happen to cover them: measured here, `恢复默认` came back as three
//! glyphs from a JAPANESE face plus one from Unifont, and asking for
//! Bold reshuffled the list into a fourth font. A face that has a
//! codepoint in its `cmap` but no outline behind it then renders as a
//! blank at full width, which is what issue #189 reported as "truncated"
//! text.
//!
//! So the bytes are rewritten before they reach `iced::font::load`:
//! family becomes the name the fallback asks for, and `usWeightClass`
//! becomes 400 so the face sorts as an ordinary Regular. The claim is
//! honest, not a squat: Noto Sans SC IS the Simplified Chinese subset of
//! Noto Sans CJK, so a machine reading `Noto Sans CJK SC` off our file
//! gets the typeface it asked for. Where the system really has the CJK
//! family installed the two faces share it and the system's exact weight
//! match wins, which is fine because that machine already rendered
//! correctly. We only win where the named family is missing, which is
//! exactly the case that was broken.
//!
//! Two things this deliberately does NOT do. It never touches the cached
//! FILE: `fonts::is_language_cached` validates the cache by byte length
//! against the pinned `len`, so a renamed file on disk would fail that
//! test forever and re-download on every boot. And it is wired only into
//! the CJK path, never into the terminal font pack, whose picker
//! resolves families by the name the user sees.
//!
//! Known limitation, recorded rather than worked around: the fallback
//! list is keyed by the SYSTEM locale, not by the app's language. A
//! Japanese UI on an `en-US` machine asks for the Simplified Chinese
//! family for its kanji, and the `...JP` file we registered is not
//! consulted. Registering one file under several names would double an
//! 18 MB face in the font database for a case where the glyphs are
//! shared anyway.

/// The `name` table entries that carry the family, or a string derived
/// from it. Everything else in the table is copied through untouched.
const FAMILY: u16 = 1;
const SUBFAMILY: u16 = 2;
const FULL_NAME: u16 = 4;
const POSTSCRIPT_NAME: u16 = 6;
const TYPOGRAPHIC_FAMILY: u16 = 16;
const TYPOGRAPHIC_SUBFAMILY: u16 = 17;

/// The weight the rewritten face declares. The files carry a `wght`
/// axis and cosmic-text applies the requested weight along it at
/// rasterization time, so the declared value only decides how the face
/// sorts when the fallback compares it against others: Thin sorts last
/// everywhere, Regular sorts like the text it is standing in for.
const REGULAR_WEIGHT: u16 = 400;

/// Rewrite `data` so the font reports `family` and a Regular weight.
///
/// Returns `None` when the bytes are not a plain TrueType sfnt we can
/// safely rewrite (a collection, a table directory that doesn't parse,
/// a `name` table in format 1, which carries language-tag records that
/// a format-0 rebuild would strand). Every caller falls back to the
/// original bytes, so a font we can't rename still loads and still
/// renders; it just keeps today's behaviour.
pub fn with_family(data: &[u8], family: &str) -> Option<Vec<u8>> {
    if !family.is_ascii() || family.is_empty() {
        return None;
    }
    // Only a bare TrueType outline font. `ttcf` collections carry
    // several directories and `OTTO` has no `glyf` we would want to
    // touch; neither is what we pin.
    if read_u32(data, 0)? != 0x0001_0000 {
        return None;
    }
    let num_tables = read_u16(data, 4)? as usize;
    let mut name_rec = None;
    let mut os2_rec = None;
    let mut head_rec = None;
    for i in 0..num_tables {
        let rec = 12 + i * 16;
        let tag = data.get(rec..rec + 4)?;
        match tag {
            b"name" => name_rec = Some(rec),
            b"OS/2" => os2_rec = Some(rec),
            b"head" => head_rec = Some(rec),
            _ => {}
        }
    }
    let name_rec = name_rec?;
    let name_off = read_u32(data, name_rec + 8)? as usize;
    let name_len = read_u32(data, name_rec + 12)? as usize;
    let table = data.get(name_off..name_off.checked_add(name_len)?)?;
    let name = build_name_table(table, family)?;

    // The rewritten table is appended rather than patched in place: the
    // new strings are a different length, and moving every table after
    // it would mean rewriting every offset in the directory.
    let mut out = data.to_vec();
    while !out.len().is_multiple_of(4) {
        out.push(0);
    }
    let new_off = out.len();
    out.extend_from_slice(&name);
    while !out.len().is_multiple_of(4) {
        out.push(0);
    }
    write_u32(&mut out, name_rec + 8, new_off as u32);
    write_u32(&mut out, name_rec + 12, name.len() as u32);
    let name_sum = table_checksum(&out, new_off, name.len());
    write_u32(&mut out, name_rec + 4, name_sum);

    if let Some(os2_rec) = os2_rec {
        let os2_off = read_u32(&out, os2_rec + 8)? as usize;
        let os2_len = read_u32(&out, os2_rec + 12)? as usize;
        // usWeightClass is the third field of OS/2: version, then the
        // signed xAvgCharWidth, then the weight.
        if os2_len >= 6 {
            write_u16(&mut out, os2_off + 4, REGULAR_WEIGHT);
            let os2_sum = table_checksum(&out, os2_off, os2_len);
            write_u32(&mut out, os2_rec + 4, os2_sum);
        }
    }

    // Nothing we ship verifies these, but a font that lies about its own
    // checksums is a font that fails on the next tool that does.
    if let Some(head_rec) = head_rec {
        let head_off = read_u32(&out, head_rec + 8)? as usize;
        let head_len = read_u32(&out, head_rec + 12)? as usize;
        if head_len >= 12 {
            write_u32(&mut out, head_off + 8, 0);
            let head_sum = table_checksum(&out, head_off, head_len);
            write_u32(&mut out, head_rec + 4, head_sum);
            let total = sum_u32(&out, 0, out.len());
            write_u32(&mut out, head_off + 8, 0xB1B0_AFBA_u32.wrapping_sub(total));
        }
    }
    Some(out)
}

/// Rebuild a format-0 `name` table with the family strings replaced.
///
/// Records are preserved one for one, including the variable font's
/// instance names (IDs 256 and up), so the only thing that changes about
/// the font's identity is the family it answers to.
fn build_name_table(table: &[u8], family: &str) -> Option<Vec<u8>> {
    if read_u16(table, 0)? != 0 {
        return None;
    }
    let count = read_u16(table, 2)? as usize;
    let storage = read_u16(table, 4)? as usize;

    // PostScript names are ASCII with no spaces and no `[](){}<>/%`.
    let postscript: String = family.chars().filter(|c| !c.is_whitespace()).collect();

    let mut records = Vec::with_capacity(count);
    let mut strings: Vec<u8> = Vec::new();
    for i in 0..count {
        let rec = 6 + i * 12;
        let platform = read_u16(table, rec)?;
        let encoding = read_u16(table, rec + 2)?;
        let language = read_u16(table, rec + 4)?;
        let name_id = read_u16(table, rec + 6)?;
        let len = read_u16(table, rec + 8)? as usize;
        let off = read_u16(table, rec + 10)? as usize;

        let replacement = match name_id {
            FAMILY | TYPOGRAPHIC_FAMILY | FULL_NAME => Some(family),
            POSTSCRIPT_NAME => Some(postscript.as_str()),
            // The declared weight becomes Regular, so the style name has
            // to agree: leaving "Thin" behind would describe a face that
            // no longer exists.
            SUBFAMILY | TYPOGRAPHIC_SUBFAMILY => Some("Regular"),
            _ => None,
        };
        let bytes = match replacement {
            Some(text) => encode(text, platform, encoding)?,
            None => table.get(storage + off..storage + off + len)?.to_vec(),
        };
        records.push((platform, encoding, language, name_id, strings.len(), bytes.len()));
        strings.extend_from_slice(&bytes);
    }

    let storage_start = 6 + records.len() * 12;
    let mut out = Vec::with_capacity(storage_start + strings.len());
    out.extend_from_slice(&0u16.to_be_bytes());
    out.extend_from_slice(&(records.len() as u16).to_be_bytes());
    out.extend_from_slice(&(storage_start as u16).to_be_bytes());
    for (platform, encoding, language, name_id, off, len) in records {
        // A string offset is a u16 from the start of the storage area,
        // so a table whose strings outgrow 64 KB cannot be expressed.
        // Ours are a couple of KB; refuse rather than emit a wrapped
        // offset that would read back as a different string.
        out.extend_from_slice(&platform.to_be_bytes());
        out.extend_from_slice(&encoding.to_be_bytes());
        out.extend_from_slice(&language.to_be_bytes());
        out.extend_from_slice(&name_id.to_be_bytes());
        out.extend_from_slice(&u16::try_from(len).ok()?.to_be_bytes());
        out.extend_from_slice(&u16::try_from(off).ok()?.to_be_bytes());
    }
    out.extend_from_slice(&strings);
    Some(out)
}

/// Encode an ASCII string the way the record's platform expects:
/// UTF-16BE for the Windows platform (3) and for Unicode (0), one byte
/// per character for Macintosh Roman (1). Any other platform is left
/// alone by refusing the whole rewrite, since guessing an encoding is
/// how a family name turns into mojibake in a font picker.
fn encode(text: &str, platform: u16, _encoding: u16) -> Option<Vec<u8>> {
    match platform {
        0 | 3 => Some(text.encode_utf16().flat_map(u16::to_be_bytes).collect()),
        1 => Some(text.as_bytes().to_vec()),
        _ => None,
    }
}

/// The sfnt checksum of one table: the sum of its big-endian u32 words,
/// with the tail padded with zeros, wrapping.
fn table_checksum(data: &[u8], offset: usize, len: usize) -> u32 {
    sum_u32(data, offset, len)
}

fn sum_u32(data: &[u8], offset: usize, len: usize) -> u32 {
    let mut sum = 0u32;
    let mut i = 0;
    while i < len {
        let mut word = [0u8; 4];
        for (j, b) in word.iter_mut().enumerate() {
            if let Some(v) = data.get(offset + i + j) {
                *b = *v;
            }
        }
        sum = sum.wrapping_add(u32::from_be_bytes(word));
        i += 4;
    }
    sum
}

fn read_u16(data: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes(data.get(at..at + 2)?.try_into().ok()?))
}

fn read_u32(data: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(data.get(at..at + 4)?.try_into().ok()?))
}

fn write_u16(data: &mut [u8], at: usize, value: u16) {
    data[at..at + 2].copy_from_slice(&value.to_be_bytes());
}

fn write_u32(data: &mut [u8], at: usize, value: u32) {
    data[at..at + 4].copy_from_slice(&value.to_be_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The bundled MenuCJK subset stands in for a downloaded CJK file:
    /// it is a real TrueType sfnt with a format-0 `name` table, small
    /// enough to keep in the binary, and it is the only CJK font the
    /// test suite can rely on being present (the big ones are
    /// downloaded at runtime).
    fn sample() -> &'static [u8] {
        include_bytes!("../../../resources/fonts/MenuCJK.ttf")
    }

    fn families(data: &[u8]) -> Vec<String> {
        let face = ttf_parser::Face::parse(data, 0).expect("rewritten font parses");
        face.names()
            .into_iter()
            .filter(|n| n.name_id == FAMILY || n.name_id == TYPOGRAPHIC_FAMILY)
            .filter_map(|n| n.to_string())
            .collect()
    }

    /// The whole point: after the rewrite the font answers to the name
    /// the per-script fallback asks for. If this breaks, CJK text goes
    /// back to being drawn by whatever the system happens to have, which
    /// is issue #189.
    #[test]
    fn rewrite_replaces_every_family_record() {
        let out = with_family(sample(), "Noto Sans CJK SC").expect("rewrite succeeds");
        let names = families(&out);
        assert!(!names.is_empty(), "the sample must carry family records");
        for name in names {
            assert_eq!(name, "Noto Sans CJK SC");
        }
    }

    /// A Thin-declaring face sorts last in the fallback's weight-ordered
    /// sweep, which is the second half of why the download never won.
    #[test]
    fn rewrite_declares_a_regular_weight() {
        let out = with_family(sample(), "Noto Sans CJK SC").expect("rewrite succeeds");
        let face = ttf_parser::Face::parse(&out, 0).expect("rewritten font parses");
        assert_eq!(face.weight().to_number(), REGULAR_WEIGHT);
    }

    /// The rewrite must not disturb the outlines: it only appends a
    /// table and edits the directory, so every glyph the original drew
    /// must still be there. A font that renamed itself and lost its
    /// glyphs would reproduce the reported bug rather than fix it.
    #[test]
    fn rewrite_preserves_glyph_coverage() {
        let before = ttf_parser::Face::parse(sample(), 0).expect("sample parses");
        let out = with_family(sample(), "Noto Sans CJK SC").expect("rewrite succeeds");
        let after = ttf_parser::Face::parse(&out, 0).expect("rewritten font parses");
        assert_eq!(before.number_of_glyphs(), after.number_of_glyphs());
        for ch in "简体中文繁體日本語한국어".chars() {
            assert_eq!(
                before.glyph_index(ch).map(|g| g.0),
                after.glyph_index(ch).map(|g| g.0),
                "glyph for {ch:?} moved"
            );
        }
    }

    /// The PostScript name is the one string that cannot carry spaces.
    #[test]
    fn postscript_name_drops_spaces() {
        let out = with_family(sample(), "Noto Sans CJK SC").expect("rewrite succeeds");
        let face = ttf_parser::Face::parse(&out, 0).expect("rewritten font parses");
        let ps: Vec<String> = face
            .names()
            .into_iter()
            .filter(|n| n.name_id == POSTSCRIPT_NAME)
            .filter_map(|n| n.to_string())
            .collect();
        assert!(!ps.is_empty(), "the sample must carry a PostScript name");
        for name in ps {
            assert_eq!(name, "NotoSansCJKSC");
        }
    }

    /// Bytes we don't understand come back as `None` so the caller keeps
    /// the original file rather than loading something half-rewritten.
    #[test]
    fn refuses_what_it_cannot_rewrite() {
        assert!(with_family(b"not a font at all", "Noto Sans CJK SC").is_none());
        assert!(with_family(sample(), "").is_none());
        assert!(with_family(sample(), "Noto Sans CJK 简").is_none());
    }
}
