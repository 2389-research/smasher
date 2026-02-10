// ABOUTME: Run directory layout with manifest file, per-node log dirs, and artifact storage.
// ABOUTME: Creates and manages the on-disk structure for a single pipeline run.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::state::StateError;

/// Describes the subdirectory layout within a run directory.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RunDirectories {
    pub root: PathBuf,
    pub checkpoints: PathBuf,
    pub node_logs: PathBuf,
    pub artifacts: PathBuf,
    pub events: PathBuf,
}

/// Serializable manifest describing a pipeline run's identity and layout.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RunManifest {
    pub run_id: String,
    pub graph_name: String,
    pub graph_hash: String,
    pub created_at: DateTime<Utc>,
    pub layout_version: u32,
    pub directories: RunDirectories,
}

/// Builder and manager for run directory layout on disk.
///
/// Handles creating the directory tree, writing the manifest, and
/// providing convenience accessors for per-node subdirectories.
#[derive(Debug, Clone)]
pub struct RunDirectory {
    manifest: RunManifest,
}

/// Maximum length for a sanitized graph name.
const MAX_GRAPH_NAME_LEN: usize = 128;

/// Sanitize a graph name from user input for safe storage.
///
/// Strips path separators (`/`, `\`) and parent-directory traversals (`..`),
/// replacing them with underscores. Truncates to 128 characters and defaults
/// to `"unnamed"` if the result is empty.
pub fn sanitize_graph_name(raw: &str) -> String {
    let sanitized = raw.replace(['/', '\\'], "_").replace("..", "_");
    let trimmed = sanitized.trim();
    if trimmed.is_empty() {
        return "unnamed".to_string();
    }
    if trimmed.len() > MAX_GRAPH_NAME_LEN {
        trimmed[..MAX_GRAPH_NAME_LEN].to_string()
    } else {
        trimmed.to_string()
    }
}

/// Compute a hex-encoded SHA256 hash of the given input.
fn sha256_hex(input: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    let result = hasher.finalize();
    result
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>()
}

impl RunDirectory {
    /// Create a run directory structure on disk.
    ///
    /// Creates subdirectories for checkpoints, node logs, artifacts, and events
    /// under `{base_dir}/{run_id}/`, then writes a `manifest.json` file at the root.
    pub fn create(
        base_dir: &Path,
        run_id: &str,
        graph_name: &str,
        graph_source: &str,
    ) -> Result<Self, StateError> {
        let root = base_dir.join(run_id);
        let directories = RunDirectories {
            root: root.clone(),
            checkpoints: root.join("checkpoints"),
            node_logs: root.join("nodes"),
            artifacts: root.join("artifacts"),
            events: root.join("events"),
        };

        // Create all subdirectories
        std::fs::create_dir_all(&directories.checkpoints)?;
        std::fs::create_dir_all(&directories.node_logs)?;
        std::fs::create_dir_all(&directories.artifacts)?;
        std::fs::create_dir_all(&directories.events)?;

        let manifest = RunManifest {
            run_id: run_id.to_string(),
            graph_name: graph_name.to_string(),
            graph_hash: sha256_hex(graph_source),
            created_at: Utc::now(),
            layout_version: 1,
            directories,
        };

        // Write manifest.json
        let manifest_json = serde_json::to_string_pretty(&manifest).map_err(|e| {
            StateError::SerializationError {
                message: e.to_string(),
            }
        })?;
        std::fs::write(root.join("manifest.json"), manifest_json)?;

        Ok(Self { manifest })
    }

    /// Open an existing run directory by reading its manifest.
    pub fn open(path: &Path) -> Result<Self, StateError> {
        let manifest_path = path.join("manifest.json");
        let contents = std::fs::read_to_string(&manifest_path)?;
        let manifest: RunManifest =
            serde_json::from_str(&contents).map_err(|e| StateError::DeserializationError {
                message: e.to_string(),
            })?;
        Ok(Self { manifest })
    }

    /// Return a reference to the run manifest.
    pub fn manifest(&self) -> &RunManifest {
        &self.manifest
    }

    /// Return the log directory path for a specific node.
    ///
    /// The directory is `{root}/nodes/{node_id}/`.
    pub fn node_log_dir(&self, node_id: &str) -> PathBuf {
        self.manifest.directories.node_logs.join(node_id)
    }

    /// Return the path to the checkpoint file.
    ///
    /// The path is `{root}/checkpoints/checkpoint.json`.
    pub fn checkpoint_path(&self) -> PathBuf {
        self.manifest
            .directories
            .checkpoints
            .join("checkpoint.json")
    }

    /// Return the artifact directory for a specific node.
    ///
    /// The directory is `{root}/artifacts/{node_id}/`.
    pub fn artifact_dir(&self, node_id: &str) -> PathBuf {
        self.manifest.directories.artifacts.join(node_id)
    }

    /// Return the path to the event log file.
    ///
    /// The path is `{root}/events/events.jsonl`.
    pub fn event_log_path(&self) -> PathBuf {
        self.manifest.directories.events.join("events.jsonl")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---------------------------------------------------------------
    // RunManifest serde round-trip
    // ---------------------------------------------------------------

    #[test]
    fn run_manifest_serde_roundtrip() {
        let manifest = RunManifest {
            run_id: "run-42".to_string(),
            graph_name: "my_pipeline".to_string(),
            graph_hash: "abc123def456".to_string(),
            created_at: Utc::now(),
            layout_version: 1,
            directories: RunDirectories {
                root: PathBuf::from("/tmp/runs/run-42"),
                checkpoints: PathBuf::from("/tmp/runs/run-42/checkpoints"),
                node_logs: PathBuf::from("/tmp/runs/run-42/nodes"),
                artifacts: PathBuf::from("/tmp/runs/run-42/artifacts"),
                events: PathBuf::from("/tmp/runs/run-42/events"),
            },
        };

        let json_str = serde_json::to_string_pretty(&manifest).unwrap();
        let restored: RunManifest = serde_json::from_str(&json_str).unwrap();

        assert_eq!(manifest, restored);
    }

    // ---------------------------------------------------------------
    // RunDirectory::create creates all subdirectories
    // ---------------------------------------------------------------

    #[test]
    fn create_creates_all_subdirectories() {
        let tmp = tempfile::tempdir().unwrap();
        let run_dir =
            RunDirectory::create(tmp.path(), "run-1", "test_graph", "digraph { a -> b }").unwrap();

        let root = tmp.path().join("run-1");
        assert!(root.exists(), "root directory should exist");
        assert!(
            root.join("checkpoints").is_dir(),
            "checkpoints dir should exist"
        );
        assert!(root.join("nodes").is_dir(), "nodes dir should exist");
        assert!(
            root.join("artifacts").is_dir(),
            "artifacts dir should exist"
        );
        assert!(root.join("events").is_dir(), "events dir should exist");

        // Manifest fields should reflect the directory structure
        let dirs = &run_dir.manifest().directories;
        assert_eq!(dirs.root, root);
        assert_eq!(dirs.checkpoints, root.join("checkpoints"));
        assert_eq!(dirs.node_logs, root.join("nodes"));
        assert_eq!(dirs.artifacts, root.join("artifacts"));
        assert_eq!(dirs.events, root.join("events"));
    }

    // ---------------------------------------------------------------
    // RunDirectory::create writes valid manifest.json
    // ---------------------------------------------------------------

    #[test]
    fn create_writes_valid_manifest_json() {
        let tmp = tempfile::tempdir().unwrap();
        let _run_dir = RunDirectory::create(
            tmp.path(),
            "run-abc",
            "pipeline_x",
            "digraph { start -> end }",
        )
        .unwrap();

        let manifest_path = tmp.path().join("run-abc").join("manifest.json");
        assert!(manifest_path.exists(), "manifest.json should be written");

        let contents = std::fs::read_to_string(&manifest_path).unwrap();
        let value: serde_json::Value = serde_json::from_str(&contents).unwrap();

        assert_eq!(value["run_id"], "run-abc");
        assert_eq!(value["graph_name"], "pipeline_x");
        assert_eq!(value["layout_version"], 1);
        // graph_hash should be present and non-empty
        assert!(!value["graph_hash"].as_str().unwrap().is_empty());
    }

    // ---------------------------------------------------------------
    // RunDirectory::open reads manifest back
    // ---------------------------------------------------------------

    #[test]
    fn open_reads_manifest_back() {
        let tmp = tempfile::tempdir().unwrap();
        let created =
            RunDirectory::create(tmp.path(), "run-reopen", "my_graph", "digraph { x -> y }")
                .unwrap();

        let opened = RunDirectory::open(&tmp.path().join("run-reopen")).unwrap();

        assert_eq!(created.manifest().run_id, opened.manifest().run_id);
        assert_eq!(created.manifest().graph_name, opened.manifest().graph_name);
        assert_eq!(created.manifest().graph_hash, opened.manifest().graph_hash);
        assert_eq!(
            created.manifest().layout_version,
            opened.manifest().layout_version
        );
        assert_eq!(
            created.manifest().directories,
            opened.manifest().directories
        );
    }

    // ---------------------------------------------------------------
    // graph_hash is deterministic
    // ---------------------------------------------------------------

    #[test]
    fn graph_hash_is_deterministic() {
        let source = "digraph { a -> b; b -> c; }";

        let tmp1 = tempfile::tempdir().unwrap();
        let run1 = RunDirectory::create(tmp1.path(), "run-1", "g", source).unwrap();

        let tmp2 = tempfile::tempdir().unwrap();
        let run2 = RunDirectory::create(tmp2.path(), "run-2", "g", source).unwrap();

        assert_eq!(
            run1.manifest().graph_hash,
            run2.manifest().graph_hash,
            "same source should produce the same hash"
        );

        // Different source should produce a different hash
        let tmp3 = tempfile::tempdir().unwrap();
        let run3 = RunDirectory::create(tmp3.path(), "run-3", "g", "digraph { x -> y }").unwrap();

        assert_ne!(
            run1.manifest().graph_hash,
            run3.manifest().graph_hash,
            "different sources should produce different hashes"
        );
    }

    // ---------------------------------------------------------------
    // graph_hash is valid SHA256 (64 hex characters)
    // ---------------------------------------------------------------

    #[test]
    fn graph_hash_is_valid_sha256_hex() {
        let tmp = tempfile::tempdir().unwrap();
        let run_dir =
            RunDirectory::create(tmp.path(), "run-sha", "g", "digraph { a -> b }").unwrap();

        let hash = &run_dir.manifest().graph_hash;
        assert_eq!(hash.len(), 64, "SHA256 hex string should be 64 characters");
        assert!(
            hash.chars().all(|c| c.is_ascii_hexdigit()),
            "hash should contain only hex characters"
        );
    }

    // ---------------------------------------------------------------
    // node_log_dir returns correct path
    // ---------------------------------------------------------------

    #[test]
    fn node_log_dir_returns_correct_path() {
        let tmp = tempfile::tempdir().unwrap();
        let run_dir =
            RunDirectory::create(tmp.path(), "run-logs", "g", "digraph { a -> b }").unwrap();

        let log_dir = run_dir.node_log_dir("node_alpha");
        assert_eq!(
            log_dir,
            tmp.path().join("run-logs").join("nodes").join("node_alpha")
        );
    }

    // ---------------------------------------------------------------
    // artifact_dir returns correct path
    // ---------------------------------------------------------------

    #[test]
    fn artifact_dir_returns_correct_path() {
        let tmp = tempfile::tempdir().unwrap();
        let run_dir =
            RunDirectory::create(tmp.path(), "run-arts", "g", "digraph { a -> b }").unwrap();

        let art_dir = run_dir.artifact_dir("node_beta");
        assert_eq!(
            art_dir,
            tmp.path()
                .join("run-arts")
                .join("artifacts")
                .join("node_beta")
        );
    }

    // ---------------------------------------------------------------
    // checkpoint_path returns correct path
    // ---------------------------------------------------------------

    #[test]
    fn checkpoint_path_returns_correct_path() {
        let tmp = tempfile::tempdir().unwrap();
        let run_dir =
            RunDirectory::create(tmp.path(), "run-cp", "g", "digraph { a -> b }").unwrap();

        let cp_path = run_dir.checkpoint_path();
        assert_eq!(
            cp_path,
            tmp.path()
                .join("run-cp")
                .join("checkpoints")
                .join("checkpoint.json")
        );
    }

    // ---------------------------------------------------------------
    // event_log_path returns correct path
    // ---------------------------------------------------------------

    #[test]
    fn event_log_path_returns_correct_path() {
        let tmp = tempfile::tempdir().unwrap();
        let run_dir =
            RunDirectory::create(tmp.path(), "run-ev", "g", "digraph { a -> b }").unwrap();

        let ev_path = run_dir.event_log_path();
        assert_eq!(
            ev_path,
            tmp.path()
                .join("run-ev")
                .join("events")
                .join("events.jsonl")
        );
    }

    // ---------------------------------------------------------------
    // open fails for non-existent directory
    // ---------------------------------------------------------------

    #[test]
    fn open_fails_for_nonexistent_directory() {
        let result = RunDirectory::open(Path::new("/tmp/this_does_not_exist_at_all"));
        assert!(result.is_err());
    }

    // ---------------------------------------------------------------
    // sha256_hex unit test
    // ---------------------------------------------------------------

    #[test]
    fn sha256_hex_known_value() {
        // SHA256 of empty string is well-known
        let hash = sha256_hex("");
        assert_eq!(
            hash,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    // ---------------------------------------------------------------
    // sanitize_graph_name tests
    // ---------------------------------------------------------------

    #[test]
    fn sanitize_replaces_forward_slashes() {
        assert_eq!(sanitize_graph_name("foo/bar/baz"), "foo_bar_baz");
    }

    #[test]
    fn sanitize_replaces_backslashes() {
        assert_eq!(sanitize_graph_name("foo\\bar\\baz"), "foo_bar_baz");
    }

    #[test]
    fn sanitize_replaces_dot_dot_traversal() {
        assert_eq!(
            sanitize_graph_name("../../../etc/passwd"),
            "______etc_passwd"
        );
    }

    #[test]
    fn sanitize_empty_string_defaults_to_unnamed() {
        assert_eq!(sanitize_graph_name(""), "unnamed");
    }

    #[test]
    fn sanitize_whitespace_only_defaults_to_unnamed() {
        assert_eq!(sanitize_graph_name("   "), "unnamed");
    }

    #[test]
    fn sanitize_truncates_long_names() {
        let long_name = "a".repeat(200);
        let result = sanitize_graph_name(&long_name);
        assert_eq!(result.len(), MAX_GRAPH_NAME_LEN);
    }

    #[test]
    fn sanitize_preserves_normal_names() {
        assert_eq!(sanitize_graph_name("my_pipeline"), "my_pipeline");
    }

    #[test]
    fn sanitize_handles_mixed_dangerous_input() {
        assert_eq!(sanitize_graph_name("../../foo/bar\\baz"), "____foo_bar_baz");
    }

    #[test]
    fn sanitize_slashes_only_defaults_to_unnamed() {
        assert_eq!(sanitize_graph_name("///"), "___");
    }
}
