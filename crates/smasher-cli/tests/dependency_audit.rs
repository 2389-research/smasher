// ABOUTME: Regression tests asserting vulnerable dependency versions are not resolved.
// ABOUTME: Covers RUSTSEC-2026-0067 (CVE-2026-33056) in the `tar` crate.

use std::path::{Path, PathBuf};

/// Locate the workspace root by walking upward from this crate's manifest dir.
fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .find(|dir| dir.join("Cargo.lock").is_file() && dir.join("Makefile").is_file())
        .expect("workspace root containing Cargo.lock and Makefile")
        .to_path_buf()
}

/// Parse the version field of the `[[package]]` block with the given name.
fn locked_version(lock: &str, name: &str) -> Option<String> {
    let needle = format!("name = \"{name}\"");
    let mut in_block = false;

    for line in lock.lines() {
        let line = line.trim();
        if line == "[[package]]" {
            in_block = false;
        } else if line == needle {
            in_block = true;
        } else if in_block && line.starts_with("version = ") {
            return Some(
                line.trim_start_matches("version = ")
                    .trim_matches('"')
                    .to_string(),
            );
        }
    }
    None
}

/// Parse a semver `major.minor.patch` string into a comparable tuple.
fn parse_semver(version: &str) -> (u64, u64, u64) {
    let mut parts = version.split('.');
    let major = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
    let minor = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
    let patch = parts
        .next()
        .map(|p| p.split('-').next().unwrap_or(p))
        .and_then(|p| p.parse().ok())
        .unwrap_or(0);
    (major, minor, patch)
}

#[test]
fn tar_is_not_the_symlink_chmod_vulnerable_version() {
    // RUSTSEC-2026-0067 / CVE-2026-33056: `unpack_in` can chmod arbitrary
    // directories by following symlinks. Fixed in 0.4.45.
    const PATCHED: (u64, u64, u64) = (0, 4, 45);

    let lock_path = workspace_root().join("Cargo.lock");
    let lock = std::fs::read_to_string(&lock_path)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", lock_path.display()));

    let version = locked_version(&lock, "tar").expect("tar should be a resolved dependency");

    assert!(
        parse_semver(&version) >= PATCHED,
        "tar {version} is vulnerable to RUSTSEC-2026-0067; require >= 0.4.45"
    );
}
