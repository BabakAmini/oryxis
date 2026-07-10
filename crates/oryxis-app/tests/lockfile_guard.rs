//! Guards Cargo.lock resolution invariants that cargo cannot express.
//!
//! `gpu-allocator 0.28` accepts `windows >=0.58,<=0.62`, a range that
//! spans semver-incompatible majors, so a full re-resolution is free
//! to unify it onto the `windows 0.61` that tauri-winrt-notification
//! (via notify-rust) pins. wgpu-hal 29 compiles its DX12 suballocator
//! against `windows 0.62` types, so that unification breaks the
//! Windows build (ID3D12Device / D3D12_RESOURCE_DESC mismatches),
//! and only on Windows, which local Linux gates never see. It has
//! happened twice (fixed in 155a572, regressed by a lock refresh on
//! 2026-07-10); this test makes the third time a red test run
//! instead of a broken nightly.

use std::path::Path;

/// Returns the dependency lines of `package`'s block in Cargo.lock.
fn lock_dependencies(lock: &str, package: &str) -> Vec<String> {
    let header = format!("name = \"{package}\"");
    let mut in_block = false;
    let mut deps = Vec::new();
    for line in lock.lines() {
        if line.starts_with("name = ") {
            in_block = line.trim() == header;
            continue;
        }
        if in_block {
            if line.starts_with("[[package]]") {
                break;
            }
            let line = line.trim();
            if let Some(dep) = line.strip_prefix('"').and_then(|l| l.strip_suffix("\",")) {
                deps.push(dep.to_owned());
            }
        }
    }
    deps
}

#[test]
fn gpu_allocator_binds_windows_062() {
    let lock_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../Cargo.lock");
    let lock = std::fs::read_to_string(&lock_path).expect("read workspace Cargo.lock");

    let deps = lock_dependencies(&lock, "gpu-allocator");
    assert!(
        !deps.is_empty(),
        "gpu-allocator not found in Cargo.lock; if it left the graph, delete this test"
    );

    let windows_dep = deps
        .iter()
        .find(|d| d.starts_with("windows "))
        .expect("gpu-allocator should depend on the `windows` crate");
    assert!(
        windows_dep.starts_with("windows 0.62"),
        "gpu-allocator resolved onto `{windows_dep}` instead of windows 0.62; \
         this breaks the Windows DX12 build against wgpu-hal 29. Fix with \
         `cargo update -p tauri-winrt-notification` (or re-pin the edge in \
         Cargo.lock) before pushing."
    );
}
