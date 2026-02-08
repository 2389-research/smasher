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
fn workspace_has_four_crates() {
    let root = workspace_root();
    let expected_crates = [
        "smasher-llm",
        "smasher-agent",
        "smasher-attractor",
        "smasher-cli",
    ];
    for name in &expected_crates {
        let crate_cargo = root.join(name).join("Cargo.toml");
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
    ];

    let mut failures: Vec<String> = Vec::new();
    for crate_name in &crate_dirs {
        let src_dir = root.join(crate_name).join("src");
        let test_dir = root.join(crate_name).join("tests");
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
        let toml_path = root.join(crate_name).join("Cargo.toml");
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

    // CLI: smasher-cli must not be depended upon by any other crate.
    let cli_deps = read_deps("smasher-cli");
    // cli can depend on anything, but no crate should depend on cli.
    for crate_name in &["smasher-llm", "smasher-agent", "smasher-attractor"] {
        let deps = read_deps(crate_name);
        assert!(
            !deps.contains(&"smasher-cli".to_string()),
            "{crate_name} must not depend on smasher-cli, but does"
        );
    }

    // Sanity: cli does depend on all three layers.
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
}
