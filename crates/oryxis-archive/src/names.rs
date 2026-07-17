//! Pure naming helpers shared by the local and remote archive paths.

/// Recognized archive formats. `Zip` and `TarGz` are the two we also
/// CREATE; the rest are extract-only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveKind {
    Zip,
    TarGz,
    Tar,
    TarBz2,
    TarXz,
    TarZst,
}

impl ArchiveKind {
    /// Classify a file name by extension (case-insensitive). `None`
    /// means "not an archive we handle".
    pub fn from_name(name: &str) -> Option<Self> {
        let lower = name.to_ascii_lowercase();
        // Longest suffixes first so `.tar.gz` never classifies as `.gz`.
        const MAP: &[(&str, ArchiveKind)] = &[
            (".tar.gz", ArchiveKind::TarGz),
            (".tgz", ArchiveKind::TarGz),
            (".tar.bz2", ArchiveKind::TarBz2),
            (".tbz2", ArchiveKind::TarBz2),
            (".tar.xz", ArchiveKind::TarXz),
            (".txz", ArchiveKind::TarXz),
            (".tar.zst", ArchiveKind::TarZst),
            (".tzst", ArchiveKind::TarZst),
            (".tar", ArchiveKind::Tar),
            (".zip", ArchiveKind::Zip),
        ];
        MAP.iter()
            .find(|(suffix, _)| lower.ends_with(suffix) && lower.len() > suffix.len())
            .map(|(_, kind)| *kind)
    }

    /// The canonical extension used when CREATING an archive of this
    /// kind.
    pub fn extension(self) -> &'static str {
        match self {
            ArchiveKind::Zip => ".zip",
            ArchiveKind::TarGz => ".tar.gz",
            ArchiveKind::Tar => ".tar",
            ArchiveKind::TarBz2 => ".tar.bz2",
            ArchiveKind::TarXz => ".tar.xz",
            ArchiveKind::TarZst => ".tar.zst",
        }
    }
}

/// Strip the archive extension from a file name, yielding the stem used
/// as the default extraction directory (`backup.tar.gz` -> `backup`).
/// Names that are ONLY an extension (`.zip`) are returned unchanged.
pub fn archive_stem(name: &str) -> &str {
    let lower = name.to_ascii_lowercase();
    for suffix in [
        ".tar.gz", ".tgz", ".tar.bz2", ".tbz2", ".tar.xz", ".txz", ".tar.zst", ".tzst", ".tar",
        ".zip",
    ] {
        if lower.ends_with(suffix) && lower.len() > suffix.len() {
            return &name[..name.len() - suffix.len()];
        }
    }
    name
}

/// Pick a name not present in `existing` (case-sensitive, matching how
/// both SFTP servers and local file systems list back what we create on
/// the paths we care about). `base` is tried first, then `base-1`,
/// `base-2`, ... For names with an extension pass the pieces separately:
/// `unique_name("photos", ".zip", ...)` yields `photos-1.zip`.
pub fn unique_name<'a, I>(base: &str, suffix: &str, existing: I) -> String
where
    I: IntoIterator<Item = &'a str>,
{
    let taken: std::collections::HashSet<&str> = existing.into_iter().collect();
    let first = format!("{base}{suffix}");
    if !taken.contains(first.as_str()) {
        return first;
    }
    for n in 1u32.. {
        let candidate = format!("{base}-{n}{suffix}");
        if !taken.contains(candidate.as_str()) {
            return candidate;
        }
    }
    unreachable!("u32 candidate space exhausted");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_from_name() {
        assert_eq!(ArchiveKind::from_name("a.zip"), Some(ArchiveKind::Zip));
        assert_eq!(ArchiveKind::from_name("a.ZIP"), Some(ArchiveKind::Zip));
        assert_eq!(ArchiveKind::from_name("a.tar.gz"), Some(ArchiveKind::TarGz));
        assert_eq!(ArchiveKind::from_name("a.tgz"), Some(ArchiveKind::TarGz));
        assert_eq!(ArchiveKind::from_name("a.tar"), Some(ArchiveKind::Tar));
        assert_eq!(ArchiveKind::from_name("a.tar.bz2"), Some(ArchiveKind::TarBz2));
        assert_eq!(ArchiveKind::from_name("a.tar.xz"), Some(ArchiveKind::TarXz));
        assert_eq!(ArchiveKind::from_name("a.tar.zst"), Some(ArchiveKind::TarZst));
        assert_eq!(ArchiveKind::from_name("a.txt"), None);
        assert_eq!(ArchiveKind::from_name("a.gz"), None);
        // An extension with no stem is not an archive name.
        assert_eq!(ArchiveKind::from_name(".zip"), None);
    }

    #[test]
    fn stem_strips_compound_extensions() {
        assert_eq!(archive_stem("backup.tar.gz"), "backup");
        assert_eq!(archive_stem("backup.tgz"), "backup");
        assert_eq!(archive_stem("photos.zip"), "photos");
        assert_eq!(archive_stem("Weird.Name.v2.zip"), "Weird.Name.v2");
        assert_eq!(archive_stem(".zip"), ".zip");
        assert_eq!(archive_stem("noext"), "noext");
    }

    #[test]
    fn unique_name_dedups() {
        let existing = ["photos.zip", "photos-1.zip"];
        assert_eq!(unique_name("photos", ".zip", existing), "photos-2.zip");
        assert_eq!(unique_name("fresh", ".zip", existing), "fresh.zip");
        let dirs = ["backup"];
        assert_eq!(unique_name("backup", "", dirs), "backup-1");
    }
}
