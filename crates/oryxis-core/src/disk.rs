//! Free space on the volume that holds a given path.
//!
//! Three callers, one question. Every one of them is about to let a
//! REMOTE peer decide how many bytes land on the user's disk:
//!
//! - SFTP downloads know the size up front (the listing carries it), so
//!   they answer "does this fit" before the first byte moves, rather
//!   than failing at 90% and leaving a part file behind.
//! - ZMODEM downloads start from six bytes the server printed. The
//!   announced ZFILE size is checkable; an unannounced one is not, so a
//!   floor is the only bound there.
//! - Session recording writes for as long as the peer keeps printing,
//!   with no size in hand at all, so the floor is the whole mechanism.
//!
//! Best-effort by design: a platform that will not answer returns
//! `None` and the caller proceeds, exactly like the engine's other
//! fixed-path probes (`~/.ssh/pageant.conf`, `~/.Xauthority`). A check
//! that cannot run must never be the reason a transfer refuses.
//!
//! The number is a snapshot, not a reservation. Another process can eat
//! the slack a millisecond later, which is why the callers pair it with
//! headroom rather than treating it as an exact budget.

use std::path::Path;

/// Bytes available to a non-privileged process on the volume holding
/// `path`, or `None` when the platform cannot answer (unsupported
/// target, a path that does not resolve, a failing syscall).
///
/// The path does not have to exist: the nearest existing ancestor is
/// probed instead, which is what makes it usable before
/// `create_dir_all` has run for a download destination.
pub fn available_space(path: &Path) -> Option<u64> {
    let probe = nearest_existing(path)?;
    available_space_of_existing(&probe)
}

/// Walk up until something exists. A download destination is routinely
/// a directory nobody has created yet, and every platform API here
/// wants a live path.
fn nearest_existing(path: &Path) -> Option<std::path::PathBuf> {
    let mut cur = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().ok()?.join(path)
    };
    loop {
        if cur.exists() {
            return Some(cur);
        }
        if !cur.pop() {
            return None;
        }
    }
}

#[cfg(unix)]
fn available_space_of_existing(path: &Path) -> Option<u64> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let c_path = CString::new(path.as_os_str().as_bytes()).ok()?;
    // SAFETY: `statvfs` writes into a caller-owned struct and reads a
    // NUL-terminated path we just built. Zeroed is a valid starting
    // state for it, and nothing here escapes the call.
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c_path.as_ptr(), &mut stat) } != 0 {
        return None;
    }
    // `f_bavail` is the count available to an UNPRIVILEGED process,
    // which is the honest number here: `f_bfree` includes the reserved
    // blocks only root may touch, and counting those is what makes a
    // "there is room" answer wrong on a nearly full filesystem. Same
    // distinction the monitor dashboard's `df` reading already draws.
    let block = if stat.f_frsize > 0 { stat.f_frsize } else { stat.f_bsize };
    (block > 0).then(|| (stat.f_bavail as u64).saturating_mul(block as u64))
}

#[cfg(windows)]
fn available_space_of_existing(path: &Path) -> Option<u64> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;

    let mut wide: Vec<u16> = path.as_os_str().encode_wide().collect();
    wide.push(0);
    let mut free_to_caller: u64 = 0;
    // SAFETY: the path is NUL-terminated and outlives the call; the two
    // trailing out-params are optional and passed as null.
    let ok = unsafe {
        GetDiskFreeSpaceExW(
            wide.as_ptr(),
            &mut free_to_caller,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    // The FIRST out-param is the quota-aware figure for the calling
    // user, not the raw volume free space, which is the number that
    // matters on a machine with disk quotas.
    (ok != 0).then_some(free_to_caller)
}

#[cfg(not(any(unix, windows)))]
fn available_space_of_existing(_path: &Path) -> Option<u64> {
    None
}

/// Whether `need` bytes fit on the volume holding `path`, keeping
/// `headroom` free underneath.
///
/// Returns `true` when the platform will not answer: a probe that
/// cannot run is not evidence of a full disk, and refusing on it would
/// break transfers on every target this does not cover.
pub fn fits(path: &Path, need: u64, headroom: u64) -> bool {
    match available_space(path) {
        Some(free) => free >= need.saturating_add(headroom),
        None => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_something_for_the_temp_dir() {
        // The number is the platform's, so the assertion is only that a
        // real volume answers at all and answers plausibly.
        let free = available_space(&std::env::temp_dir());
        if let Some(free) = free {
            assert!(free > 0, "a writable temp volume should report free space");
        }
    }

    #[test]
    fn resolves_through_a_path_that_does_not_exist_yet() {
        let missing = std::env::temp_dir()
            .join("oryxis-disk-probe-does-not-exist")
            .join("nor")
            .join("this");
        assert!(!missing.exists());
        assert_eq!(
            available_space(&missing).is_some(),
            available_space(&std::env::temp_dir()).is_some(),
            "a destination that is not created yet must probe its nearest existing ancestor"
        );
    }

    #[test]
    fn an_absurd_request_does_not_fit() {
        let tmp = std::env::temp_dir();
        if available_space(&tmp).is_some() {
            assert!(!fits(&tmp, u64::MAX / 2, 0));
            assert!(fits(&tmp, 0, 0));
        }
    }

    /// The ancestor walk is what makes the probe usable on a
    /// destination that has not been created yet, and it means even a
    /// nonsense relative path lands on a real volume rather than
    /// failing: it terminates at the filesystem root, which exists.
    /// So on a supported target the answer is effectively always
    /// available, and the `None` arm of `fits` covers unsupported
    /// targets rather than bad input.
    #[test]
    fn the_ancestor_walk_always_terminates_on_a_real_volume() {
        let nonsense = Path::new("this/does/not/exist/anywhere");
        assert_eq!(
            available_space(nonsense).is_some(),
            available_space(&std::env::current_dir().unwrap()).is_some(),
        );
    }

    #[test]
    fn headroom_is_added_to_the_requirement() {
        let tmp = std::env::temp_dir();
        let Some(free) = available_space(&tmp) else {
            return;
        };
        assert!(fits(&tmp, 0, free / 2), "half the free space is available");
        assert!(
            !fits(&tmp, free, free),
            "asking for everything twice over must not fit"
        );
    }
}
