// ABOUTME: Lockfile audit test guarding against known-unsound dependency versions.
// ABOUTME: Fails if `rand`, `h2` or `tokio` resolves to an advisory-affected version.

use std::path::{Path, PathBuf};

use semver::{Version, VersionReq};

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

/// Advisory edges for RUSTSEC-2026-0386: the first version covered, the end of
/// the affected 1.51 line, the start of the never-patched 1.52 line, and the
/// first patched 1.53 release.
const TOKIO_UNAFFECTED_BOUND: &str = "0.3.2";
const TOKIO_PATCHED_151: &str = "1.51.6";
const TOKIO_AFFECTED_152: &str = "1.52.0";
const TOKIO_PATCHED: &str = "1.53.3";

/// Advisory edges as representative `semver` versions, used both for range
/// checks and as candidate witnesses when auditing version requirements.
fn advisory_edge(version: &str) -> Version {
    Version::parse(version).expect("advisory boundaries are valid semver")
}

/// Vulnerable ranges from RUSTSEC-2026-0386: `AsyncFd::new` and friends accept
/// any `T: AsRawFd` without proof of ownership or validity, which violates I/O
/// safety and can lead to a use-after-free in the I/O driver.
///
/// Patched: >=1.51.6, <1.52.0 and >=1.53.3. Unaffected: <0.3.2.
fn is_vulnerable_tokio_version(version: &Version) -> bool {
    if version < &advisory_edge(TOKIO_UNAFFECTED_BOUND) {
        return false;
    }
    if version >= &advisory_edge(TOKIO_AFFECTED_152) {
        return version < &advisory_edge(TOKIO_PATCHED);
    }
    version < &advisory_edge(TOKIO_PATCHED_151)
}

/// Whether a locked `tokio` version string is advisory-affected.
///
/// Versions that do not parse cannot be shown to be patched, so they count as
/// affected.
fn is_vulnerable_tokio(version: &str) -> bool {
    let Ok(version) = Version::parse(version) else {
        return true;
    };
    is_vulnerable_tokio_version(&version)
}

/// Locked `tokio` versions that fall inside an advisory's vulnerable ranges.
fn vulnerable_tokio_versions(packages: &[(String, String)]) -> Vec<String> {
    packages
        .iter()
        .filter(|(name, _)| name == "tokio")
        .map(|(_, version)| version)
        .filter(|version| is_vulnerable_tokio(version))
        .cloned()
        .collect()
}

/// Strip a trailing `#` comment, ignoring `#` characters inside quoted strings.
fn strip_trailing_comment(line: &str) -> &str {
    let mut in_quotes = false;
    let mut escaped = false;
    for (index, ch) in line.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match ch {
            '\\' if in_quotes => escaped = true,
            '"' => in_quotes = !in_quotes,
            '#' if !in_quotes => return &line[..index],
            _ => {}
        }
    }
    line
}

/// Split an inline table body on commas that sit outside strings and nested
/// brackets, so `features = ["a", "b"], version = "1.53.3"` yields two parts.
fn split_inline_table(body: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut depth = 0usize;
    let mut in_quotes = false;
    let mut escaped = false;
    let mut start = 0usize;

    for (index, ch) in body.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match ch {
            '\\' if in_quotes => escaped = true,
            '"' => in_quotes = !in_quotes,
            '[' | '{' if !in_quotes => depth += 1,
            ']' | '}' if !in_quotes => depth = depth.saturating_sub(1),
            ',' if !in_quotes && depth == 0 => {
                parts.push(&body[start..index]);
                start = index + ch.len_utf8();
            }
            _ => {}
        }
    }

    parts.push(&body[start..]);
    parts
}

/// True when every `{` outside a quoted string has a matching `}`.
fn braces_balanced(text: &str) -> bool {
    let mut depth = 0i64;
    let mut in_quotes = false;
    let mut escaped = false;
    for ch in text.chars() {
        if escaped {
            escaped = false;
            continue;
        }
        match ch {
            '\\' if in_quotes => escaped = true,
            '"' => in_quotes = !in_quotes,
            '{' if !in_quotes => depth += 1,
            '}' if !in_quotes => depth -= 1,
            _ => {}
        }
    }
    depth <= 0
}

/// Extract the declared version requirement for `tokio` from a manifest's
/// `[workspace.dependencies]` table, e.g. `1.53.3` from
/// `tokio = { version = "1.53.3", features = ["full"] }`.
///
/// Understands `version` anywhere in an inline table (including tables wrapped
/// over several lines), bare string requirements, trailing `#` comments, and
/// the `[workspace.dependencies.tokio]` sub-table form.
fn manifest_tokio_requirement(manifest: &str) -> Option<String> {
    let mut section: Option<String> = None;
    let mut entry: Option<String> = None;
    let mut subtable_version: Option<String> = None;
    let mut collecting: Option<String> = None;

    for raw in manifest.lines() {
        let line = strip_trailing_comment(raw).trim();
        if line.is_empty() {
            continue;
        }
        if let Some(buffer) = collecting.as_mut() {
            buffer.push(' ');
            buffer.push_str(line);
            if braces_balanced(buffer) {
                entry = collecting.take();
            }
            continue;
        }
        if line.starts_with('[') {
            section = Some(line.to_string());
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match section.as_deref() {
            // Skip keys such as `tokio-util` or `tokio-test`.
            Some("[workspace.dependencies]") if key.trim() == "tokio" => {
                let value = value.trim();
                if value.starts_with('{') && !braces_balanced(value) {
                    collecting = Some(value.to_string());
                } else {
                    entry = Some(value.to_string());
                }
            }
            Some("[workspace.dependencies.tokio]") if key.trim() == "version" => {
                subtable_version = Some(value.trim().trim_matches('"').to_string());
            }
            _ => {}
        }
    }

    if let Some(buffer) = collecting {
        // Unterminated inline table; audit whatever version information we have.
        entry = Some(buffer);
    }

    let value = entry.or(subtable_version)?;
    let value = value.trim();

    if let Some(body) = value.strip_prefix('{') {
        let body = body.strip_suffix('}').unwrap_or(body);
        let version = split_inline_table(body).into_iter().find_map(|part| {
            let (key, val) = part.split_once('=')?;
            (key.trim() == "version").then(|| val.trim())
        })?;
        return Some(version.trim_matches('"').to_string());
    }

    Some(value.trim_matches('"').to_string())
}

/// Parse a Cargo version requirement such as `1.53.3`, `^1.51.6` or
/// `>=1.51.6, <1.52.0`.
fn parse_requirement(requirement: &str) -> Result<VersionReq, semver::Error> {
    VersionReq::parse(requirement.trim())
}

/// Versions that pin down which slice of the `tokio` release line a requirement
/// admits: every comparator's own version plus its immediate neighbours, and
/// the advisory's vulnerable-range edges. Any of these that both satisfies the
/// requirement and is advisory-affected proves the requirement is unsafe.
fn candidate_versions(requirement: &VersionReq) -> Vec<Version> {
    let mut candidates: Vec<Version> = Vec::new();
    let mut push = |version: Version| {
        if !candidates.contains(&version) {
            candidates.push(version);
        }
    };

    for edge in [
        "0.0.0",
        TOKIO_UNAFFECTED_BOUND,
        TOKIO_PATCHED_151,
        TOKIO_AFFECTED_152,
        TOKIO_PATCHED,
    ] {
        push(advisory_edge(edge));
    }

    for comparator in &requirement.comparators {
        let minor = comparator.minor.unwrap_or(0);
        let patch = comparator.patch.unwrap_or(0);
        let bounds = [
            (comparator.major, minor, patch),
            (comparator.major.saturating_add(1), 0, 0),
            (comparator.major, minor.saturating_add(1), 0),
            (comparator.major, minor, patch.saturating_add(1)),
        ];
        for (major, minor, patch) in bounds {
            push(Version::new(major, minor, patch));
            if !comparator.pre.is_empty() {
                let mut with_pre = Version::new(major, minor, patch);
                with_pre.pre = comparator.pre.clone();
                push(with_pre);
            }
        }
    }

    candidates
}

/// A RUSTSEC-2026-0386-affected `tokio` version that `requirement` admits, if
/// any.
///
/// Auditing the whole requirement rather than only its floor matters because a
/// caret or tilde requirement like `^1.51.6` admits the vulnerable 1.52.0-1.53.2
/// window even though its floor is patched.
///
/// Returns `Err` for syntax that is not a valid Cargo/semver requirement, so
/// the audit fails closed instead of assuming an ancient (hence "unaffected")
/// floor.
fn admitted_vulnerable_tokio(requirement: &str) -> Result<Option<Version>, String> {
    let requirement = requirement.trim();
    let parsed =
        parse_requirement(requirement).map_err(|error| format!("{requirement:?}: {error}"))?;
    Ok(candidate_versions(&parsed)
        .into_iter()
        .find(|version| parsed.matches(version) && is_vulnerable_tokio_version(version)))
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
fn no_locked_tokio_version_is_affected_by_rustsec_2026_0386() {
    let lockfile_path = workspace_root().join("Cargo.lock");
    let lockfile = std::fs::read_to_string(&lockfile_path)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", lockfile_path.display()));

    let packages = locked_packages(&lockfile);
    assert!(
        !packages.is_empty(),
        "no packages parsed from {}; lockfile format changed?",
        lockfile_path.display()
    );

    let vulnerable = vulnerable_tokio_versions(&packages);
    assert!(
        vulnerable.is_empty(),
        "RUSTSEC-2026-0386: `tokio` resolves to unsound version(s) {vulnerable:?}; \
         use >=1.51.6, <1.52.0 or >=1.53.3."
    );
}

#[test]
fn is_vulnerable_tokio_covers_advisory_boundaries() {
    for version in [
        "0.3.2", "1.0.0", "1.49.0", "1.50.0", "1.51.0", "1.51.5", "1.52.0", "1.52.4", "1.53.0",
        "1.53.2",
    ] {
        assert!(
            is_vulnerable_tokio(version),
            "tokio {version} is affected by RUSTSEC-2026-0386 and must be flagged"
        );
    }

    for version in [
        "0.3.1", "0.2.25", "1.51.6", "1.51.7", "1.53.3", "1.53.10", "1.54.0", "2.0.0",
    ] {
        assert!(
            !is_vulnerable_tokio(version),
            "tokio {version} is not affected by RUSTSEC-2026-0386 but was flagged"
        );
    }
}

#[test]
fn lockfile_audit_flags_vulnerable_tokio_in_synthetic_lockfile() {
    let lockfile = r#"
version = 4

[[package]]
name = "tokio"
version = "1.50.0"
source = "registry+https://github.com/rust-lang/crates.io-index"

[[package]]
name = "tokio"
version = "1.53.3"
source = "registry+https://github.com/rust-lang/crates.io-index"

[[package]]
name = "smasher-web"
version = "0.1.0"
dependencies = [
 "tokio",
]
"#;

    let packages = locked_packages(lockfile);
    assert!(packages.contains(&("tokio".to_string(), "1.50.0".to_string())));
    assert_eq!(
        vulnerable_tokio_versions(&packages),
        vec!["1.50.0".to_string()]
    );
}

#[test]
fn lockfile_audit_accepts_synthetic_lockfile_without_tokio() {
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
    assert!(vulnerable_tokio_versions(&packages).is_empty());
}

#[test]
fn manifest_tokio_requirement_floor_is_patched() {
    let manifest_path = workspace_root().join("Cargo.toml");
    let manifest = std::fs::read_to_string(&manifest_path)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", manifest_path.display()));

    let requirement = manifest_tokio_requirement(&manifest).unwrap_or_else(|| {
        panic!(
            "no `tokio` requirement found in {}'s [workspace.dependencies]",
            manifest_path.display()
        )
    });
    let admitted = admitted_vulnerable_tokio(&requirement).unwrap_or_else(|error| {
        panic!("`tokio` requirement {requirement:?} cannot be audited: {error}")
    });
    assert!(
        admitted.is_none(),
        "RUSTSEC-2026-0386: `tokio` requirement {requirement:?} admits unsound version \
         {admitted:?}; require >=1.53.3, or >=1.51.6, <1.52.0."
    );
}

#[test]
fn manifest_tokio_requirement_parser_reads_workspace_table() {
    let manifest = r#"
[workspace.dependencies]
tokio-util = "0.7"
tokio = { version = "1.53.3", features = ["full"] }
tokio-test = "0.4"
"#;
    let requirement = manifest_tokio_requirement(manifest).expect("tokio requirement parsed");
    assert_eq!(requirement, "1.53.3");
    assert_eq!(
        parse_requirement(&requirement).expect("requirement parses"),
        VersionReq::parse("^1.53.3").expect("caret requirement parses")
    );

    let bare = r#"
[workspace.dependencies]
tokio = "1.53.3"
"#;
    assert_eq!(manifest_tokio_requirement(bare), Some("1.53.3".to_string()));

    let none = r#"
[dependencies]
serde = "1"
"#;
    assert_eq!(manifest_tokio_requirement(none), None);
}

#[test]
fn manifest_tokio_requirement_parser_handles_key_order_and_inline_comments() {
    let reordered = r#"
[workspace.dependencies]
tokio = { features = ["full", "net"], version = "1.53.3" }
"#;
    assert_eq!(
        manifest_tokio_requirement(reordered),
        Some("1.53.3".to_string()),
        "`version` must be recognised wherever it appears in an inline table"
    );

    let commented = r#"
[workspace.dependencies]
tokio = "1.53.3" # pinned for RUSTSEC-2026-0386
"#;
    assert_eq!(
        manifest_tokio_requirement(commented),
        Some("1.53.3".to_string()),
        "a trailing inline comment must not be read as part of the version"
    );

    let inline_table_commented = r#"
[workspace.dependencies]
tokio = { version = "1.53.3", features = ["full"] } # pinned
"#;
    assert_eq!(
        manifest_tokio_requirement(inline_table_commented),
        Some("1.53.3".to_string())
    );
}

#[test]
fn manifest_audit_flags_vulnerable_floor_in_reordered_inline_table() {
    let manifest = r#"
[workspace.dependencies]
tokio = { features = ["full"], version = "1.51.5" }
"#;
    let requirement = manifest_tokio_requirement(manifest).expect("tokio requirement parsed");
    assert!(
        admitted_vulnerable_tokio(&requirement)
            .expect("requirement parses")
            .is_some(),
        "reordered inline table {requirement:?} hides an unsound floor"
    );
}

#[test]
fn manifest_audit_flags_requirements_that_admit_vulnerable_tokio() {
    for requirement in [
        "1",
        "1.52",
        "*",
        "0.*",
        "<1.53.3",
        "^1.51.5",
        "1.51.6",
        "^1.51.6",
        "~1.51",
        "1.51.*",
        ">=1.51.6",
        ">=1.51.6, <1.53.0",
        ">=1.52.0, <2",
        ">1.52.4",
    ] {
        let admitted = admitted_vulnerable_tokio(requirement)
            .unwrap_or_else(|error| panic!("{requirement:?} should parse: {error}"));
        assert!(
            admitted.is_some(),
            "requirement {requirement:?} admits a RUSTSEC-2026-0386-affected tokio version"
        );
    }
}

#[test]
fn manifest_audit_accepts_requirements_that_exclude_vulnerable_tokio() {
    for requirement in [
        "1.53.3",
        "^1.53.3",
        "=1.53.3",
        ">=1.53.3",
        ">1.53.3",
        "~1.54",
        "1.53.3, <2",
        ">=1.51.6, <1.52.0",
        "~1.51.6",
    ] {
        let admitted = admitted_vulnerable_tokio(requirement)
            .unwrap_or_else(|error| panic!("{requirement:?} should parse: {error}"));
        assert!(
            admitted.is_none(),
            "requirement {requirement:?} admits patched-only tokio versions but was flagged \
             as admitting {admitted:?}"
        );
    }
}

#[test]
fn manifest_audit_fails_closed_on_unparseable_tokio_requirement() {
    for requirement in ["latest", "^1.53.3.4", "1.53.3 || 1.54", ""] {
        assert!(
            admitted_vulnerable_tokio(requirement).is_err(),
            "requirement {requirement:?} is not valid semver and must not be treated as patched"
        );
    }
}

#[test]
fn manifest_tokio_requirement_parser_handles_wrapped_and_subtable_forms() {
    let wrapped = r#"
[workspace.dependencies]
tokio = {
    version = "1.53.3",
    features = ["full"],
}
tokio-util = "0.7"
"#;
    assert_eq!(
        manifest_tokio_requirement(wrapped),
        Some("1.53.3".to_string()),
        "a wrapped inline table must still yield the version requirement"
    );

    let subtable = r#"
[workspace.dependencies.tokio]
version = "1.53.3"
features = ["full"]

[workspace.dependencies.tokio-util]
version = "0.7"
"#;
    assert_eq!(
        manifest_tokio_requirement(subtable),
        Some("1.53.3".to_string()),
        "the `[workspace.dependencies.tokio]` sub-table must be understood"
    );
}
