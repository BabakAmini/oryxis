//! Tiny INI reader shared by the session-file importers (Xshell,
//! SecureCRT). Section-scoped `key=value` pairs, `;` / `#` comments,
//! values kept verbatim (SecureCRT's carry their own type prefix).

/// `[(section, [(key, value)])]` in file order. Entries before the
/// first section land under an empty section name.
pub(crate) fn sections(text: &str) -> Vec<(String, Vec<(String, String)>)> {
    let mut out: Vec<(String, Vec<(String, String)>)> = vec![(String::new(), Vec::new())];
    for raw in text.lines() {
        let line = raw.trim_end_matches('\r').trim();
        if line.is_empty() || line.starts_with(';') || line.starts_with('#') {
            continue;
        }
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            out.push((name.to_string(), Vec::new()));
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            out.last_mut()
                .expect("seeded with one section")
                .1
                .push((key.trim().to_string(), value.to_string()));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn splits_sections_and_keeps_values_verbatim() {
        let text = "; comment\nloose=1\n[A]\nx = 2 \n[B]\ny=\"quoted, thing\"\n";
        let s = super::sections(text);
        assert_eq!(s[0].1, vec![("loose".to_string(), "1".to_string())]);
        assert_eq!(s[1].0, "A");
        assert_eq!(s[1].1[0].1.trim(), "2");
        assert_eq!(s[2].1[0].1, "\"quoted, thing\"");
    }
}
