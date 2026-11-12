// ABOUTME: Lockfile audit test guarding against known-unsound dependency versions.
// ABOUTME: Fails if `rand`, `h2` or `quinn-proto` resolves to an advisory-affected version.

use std::path::{Path, PathBuf};

/// Parse `(name, version)` pairs for every package recorded in a Cargo.lock file.
fn locked_packages(lockfile: &str) -> Vec<(String, String)> {
    let mut packages = Vec::new();
    let mut name: Option<String> = None;

    for line in lockfile.lines() {
        let line = line.trim();
        if line == "[[package]]" {
            name = None;
            continue;
        }
        if let Some(rest) = line.strip_prefix("name = ") {
            name = Some(rest.trim_matches('"').to_string());
        } else if let Some(rest) = line.strip_prefix("version = ")
            && let Some(name) = name.take()
        {
            packages.push((name, rest.trim_matches('"').to_string()));
        }
    }

    packages
}

/// Split a dotted numeric version into its components.
fn version_key(version: &str) -> Vec<u64> {
    version
        .split('.')
        .map(|part| part.parse().unwrap_or(0))
        .collect()
}

fn version_gte(version: &str, min: &str) -> bool {
    let (a, b) = (version_key(version), version_key(min));
    for i in 0..a.len().max(b.len()) {
        let x = a.get(i).copied().unwrap_or(0);
        let y = b.get(i).copied().unwrap_or(0);
        if x != y {
            return x > y;
        }
    }
    true
}

fn version_lt(version: &str, max: &str) -> bool {
    !version_gte(version, max)
}

/// Vulnerable ranges from RUSTSEC-2026-0097 (rand is unsound when a custom
/// logger calls `rand::rng()` while a reseed happens).
fn is_vulnerable_rand(version: &str) -> bool {
    (version_gte(version, "0.7.0") && version_lt(version, "0.8.6"))
        || (version_gte(version, "0.9.0") && version_lt(version, "0.9.3"))
        || version == "0.10.0"
}

/// Locked `rand` versions that fall inside an advisory's vulnerable ranges.
fn vulnerable_rand_versions(packages: &[(String, String)]) -> Vec<String> {
    packages
        .iter()
        .filter(|(name, _)| name == "rand")
        .map(|(_, version)| version)
        .filter(|version| is_vulnerable_rand(version))
        .cloned()
        .collect()
}

/// Vulnerable ranges from RUSTSEC-2026-0258: `h2` accepts and queues empty DATA
/// frames without limit, so an undrained stream can grow memory unboundedly or
/// panic on length overflow.
fn is_vulnerable_h2(version: &str) -> bool {
    version_lt(version, "0.4.16")
}

/// Locked `h2` versions that fall inside an advisory's vulnerable ranges.
fn vulnerable_h2_versions(packages: &[(String, String)]) -> Vec<String> {
    packages
        .iter()
        .filter(|(name, _)| name == "h2")
        .map(|(_, version)| version)
        .filter(|version| is_vulnerable_h2(version))
        .cloned()
        .collect()
}

/// Vulnerable ranges from RUSTSEC-2026-0340: when ACK_FREQUENCY transmission is
/// enabled, a peer advertising a large `min_ack_delay` makes
/// `AckFrequencyState::candidate_max_ack_delay` call `Duration::clamp` with a
/// minimum greater than its maximum, panicking the connection.
fn is_vulnerable_quinn_proto(version: &str) -> bool {
    version_gte(version, "0.11.0") && version_lt(version, "0.11.18")
}

/// Locked `quinn-proto` versions that fall inside an advisory's vulnerable ranges.
fn vulnerable_quinn_proto_versions(packages: &[(String, String)]) -> Vec<String> {
    packages
        .iter()
        .filter(|(name, _)| name == "quinn-proto")
        .map(|(_, version)| version)
        .filter(|version| is_vulnerable_quinn_proto(version))
        .cloned()
        .collect()
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crate lives two levels below the workspace root")
        .to_path_buf()
}

#[test]
fn no_locked_rand_version_is_affected_by_rustsec_2026_0097() {
    let lockfile_path = workspace_root().join("Cargo.lock");
    let lockfile = std::fs::read_to_string(&lockfile_path)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", lockfile_path.display()));

    let packages = locked_packages(&lockfile);
    assert!(
        !packages.is_empty(),
        "no packages parsed from {}; lockfile format changed?",
        lockfile_path.display()
    );

    let vulnerable = vulnerable_rand_versions(&packages);
    assert!(
        vulnerable.is_empty(),
        "RUSTSEC-2026-0097: `rand` resolves to unsound version(s) {vulnerable:?}; \
         use >=0.8.6, <0.9.0, >=0.9.3, or >=0.10.1."
    );
}

#[test]
fn is_vulnerable_rand_covers_advisory_boundaries() {
    for version in ["0.7.0", "0.8.0", "0.8.5", "0.9.0", "0.9.2", "0.10.0"] {
        assert!(
            is_vulnerable_rand(version),
            "rand {version} is affected by RUSTSEC-2026-0097 and must be flagged"
        );
    }

    for version in [
        "0.6.4", "0.8.6", "0.8.8", "0.9.3", "0.9.10", "0.10.1", "0.11.0",
    ] {
        assert!(
            !is_vulnerable_rand(version),
            "rand {version} is not affected by RUSTSEC-2026-0097 but was flagged"
        );
    }
}

#[test]
fn version_comparison_handles_multi_digit_components() {
    assert!(version_gte("0.9.10", "0.9.3"));
    assert!(!version_gte("0.9.3", "0.9.10"));
    assert!(version_gte("0.10.0", "0.9.10"));
    assert!(version_gte("0.8.5", "0.8.5"), "version_gte is inclusive");
    assert!(version_lt("0.8.5", "0.8.6"));
    assert!(!version_lt("0.8.6", "0.8.6"), "version_lt is strict");
    assert!(!version_lt("0.8.7", "0.8.6"));
}

#[test]
fn lockfile_audit_flags_vulnerable_rand_in_synthetic_lockfile() {
    let lockfile = r#"
version = 4

[[package]]
name = "phf_generator"
version = "0.11.3"
dependencies = [
 "rand 0.8.5",
]

[[package]]
name = "rand"
version = "0.8.5"
source = "registry+https://github.com/rust-lang/crates.io-index"

[[package]]
name = "rand"
version = "0.9.3"
source = "registry+https://github.com/rust-lang/crates.io-index"

[[package]]
name = "smasher-llm"
version = "0.1.0"
"#;

    let packages = locked_packages(lockfile);
    assert!(packages.contains(&("rand".to_string(), "0.8.5".to_string())));
    assert!(packages.contains(&("rand".to_string(), "0.9.3".to_string())));
    assert_eq!(packages.len(), 4, "unexpected parse result: {packages:?}");
    assert_eq!(
        vulnerable_rand_versions(&packages),
        vec!["0.8.5".to_string()]
    );
}

#[test]
fn lockfile_audit_accepts_synthetic_lockfile_without_rand() {
    let lockfile = r#"
version = 4

[[package]]
name = "serde"
version = "1.0.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
"#;

    let packages = locked_packages(lockfile);
    assert!(
        !packages.is_empty(),
        "parser must still parse unrelated packages"
    );
    assert!(vulnerable_rand_versions(&packages).is_empty());
}

#[test]
fn no_locked_h2_version_is_affected_by_rustsec_2026_0258() {
    let lockfile_path = workspace_root().join("Cargo.lock");
    let lockfile = std::fs::read_to_string(&lockfile_path)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", lockfile_path.display()));

    let packages = locked_packages(&lockfile);
    assert!(
        !packages.is_empty(),
        "no packages parsed from {}; lockfile format changed?",
        lockfile_path.display()
    );

    let vulnerable = vulnerable_h2_versions(&packages);
    assert!(
        vulnerable.is_empty(),
        "RUSTSEC-2026-0258: `h2` resolves to vulnerable version(s) {vulnerable:?}; \
         upgrade to >=0.4.16."
    );
}

#[test]
fn is_vulnerable_h2_covers_advisory_boundaries() {
    for version in ["0.3.26", "0.4.0", "0.4.13", "0.4.15"] {
        assert!(
            is_vulnerable_h2(version),
            "h2 {version} is affected by RUSTSEC-2026-0258 and must be flagged"
        );
    }

    for version in ["0.4.16", "0.4.20", "0.5.0"] {
        assert!(
            !is_vulnerable_h2(version),
            "h2 {version} is not affected by RUSTSEC-2026-0258 but was flagged"
        );
    }
}

#[test]
fn lockfile_audit_flags_vulnerable_h2_in_synthetic_lockfile() {
    let lockfile = r#"
version = 4

[[package]]
name = "h2"
version = "0.4.13"
source = "registry+https://github.com/rust-lang/crates.io-index"

[[package]]
name = "h2"
version = "0.4.16"
source = "registry+https://github.com/rust-lang/crates.io-index"

[[package]]
name = "hyper"
version = "1.8.1"
dependencies = [
 "h2",
]
"#;

    let packages = locked_packages(lockfile);
    assert!(packages.contains(&("h2".to_string(), "0.4.13".to_string())));
    assert_eq!(
        vulnerable_h2_versions(&packages),
        vec!["0.4.13".to_string()]
    );
}

#[test]
fn lockfile_audit_accepts_synthetic_lockfile_without_h2() {
    let lockfile = r#"
version = 4

[[package]]
name = "serde"
version = "1.0.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
"#;

    let packages = locked_packages(lockfile);
    assert!(
        !packages.is_empty(),
        "parser must still parse unrelated packages"
    );
    assert!(vulnerable_h2_versions(&packages).is_empty());
}

#[test]
fn no_locked_quinn_proto_version_is_affected_by_rustsec_2026_0340() {
    let lockfile_path = workspace_root().join("Cargo.lock");
    let lockfile = std::fs::read_to_string(&lockfile_path)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", lockfile_path.display()));

    let packages = locked_packages(&lockfile);
    assert!(
        !packages.is_empty(),
        "no packages parsed from {}; lockfile format changed?",
        lockfile_path.display()
    );

    let vulnerable = vulnerable_quinn_proto_versions(&packages);
    assert!(
        vulnerable.is_empty(),
        "RUSTSEC-2026-0340: `quinn-proto` resolves to vulnerable version(s) {vulnerable:?}; \
         upgrade to >=0.11.18."
    );
}

#[test]
fn is_vulnerable_quinn_proto_covers_advisory_boundaries() {
    for version in [
        "0.11.0", "0.11.2", "0.11.9", "0.11.13", "0.11.14", "0.11.17",
    ] {
        assert!(
            is_vulnerable_quinn_proto(version),
            "quinn-proto {version} is affected by RUSTSEC-2026-0340 and must be flagged"
        );
    }

    for version in ["0.10.1", "0.11.18", "0.11.19", "0.12.0", "0.12.1"] {
        assert!(
            !is_vulnerable_quinn_proto(version),
            "quinn-proto {version} is not affected by RUSTSEC-2026-0340 but was flagged"
        );
    }
}

#[test]
fn lockfile_audit_flags_vulnerable_quinn_proto_in_synthetic_lockfile() {
    let lockfile = r#"
version = 4

[[package]]
name = "quinn-proto"
version = "0.11.14"
source = "registry+https://github.com/rust-lang/crates.io-index"

[[package]]
name = "quinn-proto"
version = "0.11.18"
source = "registry+https://github.com/rust-lang/crates.io-index"

[[package]]
name = "reqwest"
version = "0.12.28"
dependencies = [
 "quinn",
]
"#;

    let packages = locked_packages(lockfile);
    assert!(packages.contains(&("quinn-proto".to_string(), "0.11.14".to_string())));
    assert_eq!(
        vulnerable_quinn_proto_versions(&packages),
        vec!["0.11.14".to_string()]
    );
}

#[test]
fn lockfile_audit_accepts_synthetic_lockfile_without_quinn_proto() {
    let lockfile = r#"
version = 4

[[package]]
name = "serde"
version = "1.0.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
"#;

    let packages = locked_packages(lockfile);
    assert!(
        !packages.is_empty(),
        "parser must still parse unrelated packages"
    );
    assert!(vulnerable_quinn_proto_versions(&packages).is_empty());
}
