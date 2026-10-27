// ABOUTME: Workspace layout validation tests ensuring structural invariants hold.
// ABOUTME: Verifies crate count, ABOUTME comments, dependency layering, and file conventions.

use std::fs;
use std::path::{Path, PathBuf};

/// Walk a directory tree and collect all `.rs` files, skipping `target/`.
fn collect_rs_files(dir: &Path) -> Vec<PathBuf> {
    let mut results = Vec::new();
    collect_rs_files_recursive(dir, &mut results);
    results
}

fn collect_rs_files_recursive(dir: &Path, results: &mut Vec<PathBuf>) {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            // Skip build artifacts and hidden directories.
            if name == "target" || name.starts_with('.') {
                continue;
            }
            collect_rs_files_recursive(&path, results);
        } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
            results.push(path);
        }
    }
}

/// Locate the workspace root by searching upward from CARGO_MANIFEST_DIR for a
/// Cargo.toml containing `[workspace]`.
fn workspace_root() -> PathBuf {
    let manifest_dir =
        PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set"));
    let mut candidate = manifest_dir.as_path();
    loop {
        let cargo_toml = candidate.join("Cargo.toml");
        if cargo_toml.exists() {
            let contents = fs::read_to_string(&cargo_toml).unwrap_or_default();
            if contents.contains("[workspace]") {
                return candidate.to_path_buf();
            }
        }
        candidate = match candidate.parent() {
            Some(p) => p,
            None => panic!(
                "could not locate workspace root from {}",
                manifest_dir.display()
            ),
        };
    }
}

#[test]
fn workspace_has_five_crates() {
    let root = workspace_root();
    let expected_crates = [
        "smasher-llm",
        "smasher-agent",
        "smasher-attractor",
        "smasher-cli",
        "smasher-web",
    ];
    for name in &expected_crates {
        let crate_cargo = root.join("crates").join(name).join("Cargo.toml");
        assert!(
            crate_cargo.exists(),
            "Expected crate Cargo.toml not found: {}",
            crate_cargo.display()
        );
    }

    // Also verify the workspace Cargo.toml lists exactly these members.
    let ws_toml = fs::read_to_string(root.join("Cargo.toml")).expect("read workspace Cargo.toml");
    for name in &expected_crates {
        assert!(
            ws_toml.contains(name),
            "Workspace Cargo.toml does not mention member crate '{name}'"
        );
    }
}

#[test]
fn all_source_files_have_aboutme() {
    let root = workspace_root();
    let crate_dirs = [
        "smasher-llm",
        "smasher-agent",
        "smasher-attractor",
        "smasher-cli",
        "smasher-web",
    ];

    let mut failures: Vec<String> = Vec::new();
    for crate_name in &crate_dirs {
        let src_dir = root.join("crates").join(crate_name).join("src");
        let test_dir = root.join("crates").join(crate_name).join("tests");
        let mut rs_files = collect_rs_files(&src_dir);
        rs_files.extend(collect_rs_files(&test_dir));

        for path in &rs_files {
            let contents = match fs::read_to_string(path) {
                Ok(c) => c,
                Err(e) => {
                    failures.push(format!("{}: could not read: {e}", path.display()));
                    continue;
                }
            };
            let lines: Vec<&str> = contents.lines().collect();
            if lines.len() < 2 {
                failures.push(format!("{}: file has fewer than 2 lines", path.display()));
                continue;
            }
            if !lines[0].starts_with("// ABOUTME:") {
                failures.push(format!(
                    "{}: line 1 does not start with '// ABOUTME:'",
                    path.display()
                ));
            }
            if !lines[1].starts_with("// ABOUTME:") {
                failures.push(format!(
                    "{}: line 2 does not start with '// ABOUTME:'",
                    path.display()
                ));
            }
        }
    }

    if !failures.is_empty() {
        panic!(
            "The following files are missing ABOUTME comments:\n  {}",
            failures.join("\n  ")
        );
    }
}

#[test]
fn no_circular_dependencies() {
    let root = workspace_root();

    // Read each crate's Cargo.toml and extract [dependencies] section workspace
    // crate references. Then verify the expected layering.
    let read_deps = |crate_name: &str| -> Vec<String> {
        let toml_path = root.join("crates").join(crate_name).join("Cargo.toml");
        let contents = fs::read_to_string(&toml_path)
            .unwrap_or_else(|_| panic!("read {}", toml_path.display()));
        let mut in_deps = false;
        let mut deps = Vec::new();
        for line in contents.lines() {
            let trimmed = line.trim();
            if trimmed == "[dependencies]" {
                in_deps = true;
                continue;
            }
            if trimmed.starts_with('[') {
                in_deps = false;
                continue;
            }
            if in_deps {
                // Lines like `smasher-llm.workspace = true`
                for ws_crate in &[
                    "smasher-llm",
                    "smasher-agent",
                    "smasher-attractor",
                    "smasher-cli",
                    "smasher-web",
                ] {
                    if trimmed.starts_with(ws_crate) {
                        deps.push(ws_crate.to_string());
                    }
                }
            }
        }
        deps
    };

    // Layer 1: smasher-llm must not depend on any sibling crate.
    let llm_deps = read_deps("smasher-llm");
    assert!(
        llm_deps.is_empty(),
        "smasher-llm (Layer 1) must not depend on sibling crates, but depends on: {llm_deps:?}"
    );

    // Layer 2: smasher-agent may depend on smasher-llm only.
    let agent_deps = read_deps("smasher-agent");
    assert!(
        agent_deps.iter().all(|d| d == "smasher-llm"),
        "smasher-agent (Layer 2) may only depend on smasher-llm, but depends on: {agent_deps:?}"
    );

    // Layer 3: smasher-attractor may depend on smasher-llm and smasher-agent.
    let attractor_deps = read_deps("smasher-attractor");
    assert!(
        attractor_deps
            .iter()
            .all(|d| d == "smasher-llm" || d == "smasher-agent"),
        "smasher-attractor (Layer 3) may only depend on smasher-llm and smasher-agent, but depends on: {attractor_deps:?}"
    );

    // Web: smasher-web depends on the three layers but not cli.
    let web_deps = read_deps("smasher-web");
    assert!(
        !web_deps.contains(&"smasher-cli".to_string()),
        "smasher-web must not depend on smasher-cli"
    );

    // CLI: smasher-cli must not be depended upon by any other crate.
    let cli_deps = read_deps("smasher-cli");
    for crate_name in &[
        "smasher-llm",
        "smasher-agent",
        "smasher-attractor",
        "smasher-web",
    ] {
        let deps = read_deps(crate_name);
        assert!(
            !deps.contains(&"smasher-cli".to_string()),
            "{crate_name} must not depend on smasher-cli, but does"
        );
    }

    // Sanity: cli depends on all layers including web.
    assert!(
        cli_deps.contains(&"smasher-llm".to_string()),
        "smasher-cli should depend on smasher-llm"
    );
    assert!(
        cli_deps.contains(&"smasher-agent".to_string()),
        "smasher-cli should depend on smasher-agent"
    );
    assert!(
        cli_deps.contains(&"smasher-attractor".to_string()),
        "smasher-cli should depend on smasher-attractor"
    );
    assert!(
        cli_deps.contains(&"smasher-web".to_string()),
        "smasher-cli should depend on smasher-web"
    );
}

/// Read the locked versions of `package_name` from the workspace `Cargo.lock`.
fn locked_versions(package_name: &str) -> Vec<String> {
    let root = workspace_root();
    let lock_path = root.join("Cargo.lock");
    let contents = fs::read_to_string(&lock_path)
        .unwrap_or_else(|e| panic!("read {}: {e}", lock_path.display()));

    let mut versions = Vec::new();
    let mut current_package: Option<String> = None;
    for line in contents.lines() {
        let trimmed = line.trim();
        if trimmed == "[[package]]" {
            current_package = None;
            continue;
        }
        if let Some(name) = trimmed.strip_prefix("name = ") {
            current_package = Some(name.trim_matches('"').to_string());
            continue;
        }
        if let Some(version) = trimmed.strip_prefix("version = ")
            && current_package.as_deref() == Some(package_name)
        {
            versions.push(version.trim_matches('"').to_string());
            current_package = None;
        }
    }

    assert!(
        !versions.is_empty(),
        "package '{package_name}' not found in {}",
        lock_path.display()
    );
    versions
}

/// Returns true when `version` is affected by RUSTSEC-2026-0098.
///
/// Patched ranges are `>=0.103.12, <0.104.0-alpha.1` and
/// `>=0.104.0-alpha.6`, so every `0.104.0-alpha.N` below `alpha.6` is still
/// vulnerable even though it sorts after `0.103.12`. Conversely, pre-releases
/// of `0.103.12` (e.g. `0.103.12-rc.1`) sort *below* `0.103.12` and are still
/// vulnerable. Comparing parsed `semver` versions keeps both edges, and any
/// pre-release/build metadata, correct.
fn rustls_webpki_is_vulnerable(version: &str) -> bool {
    let bound = |s: &str| semver::Version::parse(s).expect("advisory bound should be valid semver");

    let Ok(version) = semver::Version::parse(version) else {
        // An unparseable version cannot be proven patched, so report it.
        return true;
    };

    let first_patch = bound("0.103.12");
    let patched_alpha = bound("0.104.0-alpha.1");
    let patched_alpha_6 = bound("0.104.0-alpha.6");

    version < first_patch || (version >= patched_alpha && version < patched_alpha_6)
}

#[test]
fn rustls_webpki_is_patched_for_rustsec_2026_0098() {
    // RUSTSEC-2026-0098 / GHSA-965h-392x-2mh5: name constraints for URI names
    // were ignored and therefore accepted. URI name constraints are now
    // rejected unconditionally. Patched in >=0.103.12 (and >=0.104.0-alpha.6).
    let versions = locked_versions("rustls-webpki");
    for version in &versions {
        assert!(
            !rustls_webpki_is_vulnerable(version),
            "Cargo.lock pins rustls-webpki {version}, which is affected by \
             RUSTSEC-2026-0098; upgrade to >=0.103.12 (or >=0.104.0-alpha.6)"
        );
    }
}

#[test]
fn rustls_webpki_vulnerable_version_predicate_is_accurate() {
    for vulnerable in [
        "0.103.9",
        "0.103.0",
        "0.103.11",
        "0.102.8",
        // Pre-releases sort below their release, so these are still affected.
        "0.103.12-alpha.1",
        "0.103.12-rc.1",
        "0.104.0-alpha.1",
        "0.104.0-alpha.5",
        // Unparseable input is treated as potentially affected.
        "not-a-version",
    ] {
        assert!(
            rustls_webpki_is_vulnerable(vulnerable),
            "{vulnerable} should be reported as vulnerable"
        );
    }
    for patched in [
        "0.103.12",
        "0.103.13",
        "0.103.12+build.5",
        "0.104.0",
        "0.104.0-alpha.6",
        "0.104.0-alpha.6.1",
        "0.104.0-alpha.10",
        "0.104.0-beta.1",
        "0.104.1",
        "0.105.0",
    ] {
        assert!(
            !rustls_webpki_is_vulnerable(patched),
            "{patched} should be reported as patched"
        );
    }
}
