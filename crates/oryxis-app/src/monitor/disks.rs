//! Per-host disk selection (issue #135).
//!
//! `Connection.monitor_disks` is `None` (Auto) on nearly every host: the
//! probe's own mount rules already keep one row per storage device. A
//! host whose mount table no rule can guess sets Custom instead, a list
//! of mount patterns, and only those are reported.
//!
//! The selection is applied ONCE, where the sample is parsed
//! (`dispatch_monitor`), never per surface: the sidebar, the dashboard,
//! the status bar and the threshold ALERTS all read the same window, and
//! a disk the user chose not to monitor must not be able to raise a toast
//! from a surface that forgot to filter.

use super::model::DiskStat;

/// Narrow a sample's disks to what the host asked for. `None` is Auto
/// and returns them untouched.
///
/// Custom output follows the order the USER wrote the patterns in, not
/// `df`'s: the list is a priority the user expressed, and the first
/// entry is what the status bar shows next to the busiest one. Within a
/// single pattern (`/mnt/*`), `df`'s order survives, and a mount matched
/// by two patterns is reported once, at the first one.
pub(crate) fn select_disks(patterns: Option<&[String]>, disks: Vec<DiskStat>) -> Vec<DiskStat> {
    let Some(patterns) = patterns else { return disks };
    let mut out: Vec<DiskStat> = Vec::new();
    for pattern in patterns {
        for disk in &disks {
            if mount_matches(pattern, &disk.mount)
                && !out.iter().any(|d| d.mount == disk.mount)
            {
                out.push(disk.clone());
            }
        }
    }
    out
}

/// Does one pattern name this mount?
///
/// A pattern without `*` is an exact mount path. `*` matches any run of
/// characters, `/` included, so `/mnt/*` covers a nested mount too.
/// Trailing slashes are normalised on both sides, since a user typing
/// `/data/` means the mount `df` calls `/data` and a silent miss would
/// look like the feature is broken.
fn mount_matches(pattern: &str, mount: &str) -> bool {
    let pattern = trim_slash(pattern.trim());
    let mount = trim_slash(mount);
    // An empty line is an unfinished row in the editor, not a pattern
    // that matches everything.
    if pattern.is_empty() {
        return false;
    }
    if !pattern.contains('*') {
        return pattern == mount;
    }
    glob_matches(pattern, mount)
}

/// `/data/` and `/data` are the same mount; `/` stays itself.
fn trim_slash(path: &str) -> &str {
    match path.strip_suffix('/') {
        Some("") | None => path,
        Some(stripped) => stripped,
    }
}

/// Wildcard match with backtracking: linear in the common case, and it
/// never recurses, so a pathological pattern can't blow the stack of the
/// UI thread that runs it once per sample.
fn glob_matches(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    let (mut pi, mut ti) = (0usize, 0usize);
    // Where to resume if the current `*` turns out to have swallowed too
    // little: `star` is the wildcard's position, `resume` the next text
    // character to hand it.
    let (mut star, mut resume) = (None, 0usize);
    while ti < t.len() {
        if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            resume = ti;
            pi += 1;
        } else if pi < p.len() && p[pi] == t[ti] {
            pi += 1;
            ti += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            resume += 1;
            ti = resume;
        } else {
            return false;
        }
    }
    // Trailing wildcards may still match nothing.
    p[pi..].iter().all(|c| *c == '*')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn disk(mount: &str) -> DiskStat {
        DiskStat { mount: mount.to_string(), used: 1, total: 2 }
    }

    fn mounts(disks: &[DiskStat]) -> Vec<&str> {
        disks.iter().map(|d| d.mount.as_str()).collect()
    }

    #[test]
    fn auto_reports_every_disk_the_probe_kept() {
        let disks = vec![disk("/"), disk("/data")];
        assert_eq!(mounts(&select_disks(None, disks.clone())), vec!["/", "/data"]);
    }

    #[test]
    fn custom_keeps_only_the_listed_mounts_in_the_users_order() {
        let disks = vec![disk("/"), disk("/data"), disk("/efs"), disk("/sbin")];
        let patterns = ["/data".to_string(), "/".to_string()];
        assert_eq!(mounts(&select_disks(Some(&patterns), disks)), vec!["/data", "/"]);
    }

    #[test]
    fn an_empty_custom_list_reports_nothing() {
        // The explicit "no disks on this host" answer, not a missing
        // value: Auto is `None`, and it is a different row.
        let disks = vec![disk("/"), disk("/data")];
        assert!(select_disks(Some(&[]), disks).is_empty());
    }

    #[test]
    fn a_wildcard_covers_a_family_of_mounts_once() {
        let disks =
            vec![disk("/"), disk("/mnt/disk1"), disk("/mnt/disk2"), disk("/mnt/pool/a")];
        let patterns = ["/mnt/*".to_string(), "/mnt/disk1".to_string()];
        // `*` crosses `/`, and the mount both patterns name is reported
        // once, at the first pattern that claimed it.
        assert_eq!(
            mounts(&select_disks(Some(&patterns), disks)),
            vec!["/mnt/disk1", "/mnt/disk2", "/mnt/pool/a"]
        );
    }

    #[test]
    fn patterns_are_exact_paths_otherwise() {
        assert!(mount_matches("/data", "/data"));
        // A prefix is NOT a match: `/data` must not drag in `/data/media`,
        // which is a different filesystem on the host that reported #135.
        assert!(!mount_matches("/data", "/data/media"));
        assert!(!mount_matches("data", "/data"));
        // A trailing slash on either side is the same mount.
        assert!(mount_matches("/data/", "/data"));
        assert!(mount_matches("/", "/"));
        // A blank row in the editor matches nothing at all.
        assert!(!mount_matches("   ", "/data"));
    }

    #[test]
    fn wildcards_match_where_they_should_and_no_further() {
        assert!(mount_matches("*", "/anything"));
        assert!(mount_matches("/mnt/*", "/mnt/backup"));
        // `/mnt/*` names what is UNDER /mnt, so the mount point itself
        // is not one of them, spelled with or without its slash.
        assert!(!mount_matches("/mnt/*", "/mnt"));
        assert!(!mount_matches("/mnt/*", "/mnt/"));
        assert!(!mount_matches("/mnt/*", "/media/backup"));
        assert!(mount_matches("/data*media", "/data/x/media"));
        assert!(mount_matches("*/media", "/data/media"));
        assert!(!mount_matches("*/media", "/data/mediax"));
    }
}
