// ABOUTME: Run archive creation that bundles pipeline artifacts into a compressed tarball.
// ABOUTME: Provides post-run archival and a standalone archive subcommand.

use std::path::{Path, PathBuf};

use clap::Args;
use flate2::Compression;
use flate2::write::GzEncoder;
use tar::Builder;

use crate::error::CliError;

/// Errors that can occur during archive creation.
#[derive(Debug, thiserror::Error)]
pub enum ArchiveError {
    #[error("I/O error during archival: {0}")]
    Io(#[from] std::io::Error),

    #[error("run directory does not exist: {path}")]
    RunDirNotFound { path: String },
}

/// Create a .tgz archive of a run directory.
///
/// Includes: manifest.json, checkpoints/, nodes/, artifacts/, events/
/// Excludes: worktree/ directory (if present)
pub fn create_archive(run_dir: &Path, output_path: &Path) -> Result<PathBuf, ArchiveError> {
    if !run_dir.exists() {
        return Err(ArchiveError::RunDirNotFound {
            path: run_dir.display().to_string(),
        });
    }

    let file = std::fs::File::create(output_path)?;
    let enc = GzEncoder::new(file, Compression::default());
    let mut tar = Builder::new(enc);

    // Walk run_dir, skip "worktree" subdirectory
    for entry in std::fs::read_dir(run_dir)? {
        let entry = entry?;
        let name = entry.file_name();

        // Skip the worktree directory (used for git worktree isolation)
        if name == "worktree" {
            continue;
        }

        // Skip the archive file itself if it's inside the run directory
        if let Ok(entry_canonical) = entry.path().canonicalize()
            && let Ok(output_canonical) = output_path.canonicalize()
            && entry_canonical == output_canonical
        {
            continue;
        }

        let name_str = name.to_string_lossy();
        if entry.file_type()?.is_dir() {
            tar.append_dir_all(&*name_str, entry.path())?;
        } else {
            tar.append_path_with_name(entry.path(), &*name_str)?;
        }
    }

    tar.finish()?;
    Ok(output_path.to_path_buf())
}

/// Archive a run directory from the command line.
#[derive(Debug, Args)]
pub struct ArchiveArgs {
    /// Path to the run directory to archive.
    pub run_dir: String,

    /// Output path for the archive (default: {run_dir}/run.tgz).
    #[arg(short, long)]
    pub output: Option<String>,
}

/// Execute the archive subcommand.
pub fn run(args: ArchiveArgs) -> Result<(), CliError> {
    let run_dir = PathBuf::from(&args.run_dir);
    let output_path = match &args.output {
        Some(p) => PathBuf::from(p),
        None => run_dir.join("run.tgz"),
    };

    let archive = create_archive(&run_dir, &output_path).map_err(|e| match e {
        ArchiveError::Io(io_err) => CliError::Io(io_err),
        other => CliError::Other(other.to_string()),
    })?;

    eprintln!("Archive: {}", archive.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Create a fake run directory structure in a tempdir for testing.
    fn create_test_run_dir(tmp: &Path) -> PathBuf {
        let run_dir = tmp.join("test-run");
        std::fs::create_dir_all(run_dir.join("checkpoints")).unwrap();
        std::fs::create_dir_all(run_dir.join("nodes")).unwrap();
        std::fs::create_dir_all(run_dir.join("artifacts")).unwrap();
        std::fs::create_dir_all(run_dir.join("events")).unwrap();

        // Write a manifest.json
        std::fs::write(
            run_dir.join("manifest.json"),
            r#"{"run_id":"test-run","graph_name":"test","layout_version":1}"#,
        )
        .unwrap();

        // Write some artifact files
        std::fs::write(run_dir.join("artifacts").join("output.txt"), "hello world").unwrap();
        std::fs::write(
            run_dir.join("events").join("events.jsonl"),
            r#"{"event":"started"}"#,
        )
        .unwrap();

        run_dir
    }

    #[test]
    fn create_archive_produces_valid_tgz() {
        let tmp = tempfile::tempdir().unwrap();
        let run_dir = create_test_run_dir(tmp.path());
        let output = tmp.path().join("output.tgz");

        let result = create_archive(&run_dir, &output).unwrap();
        assert_eq!(result, output);
        assert!(output.exists(), "archive file should exist");

        // Verify it's a valid gzip file by reading it back
        let file = std::fs::File::open(&output).unwrap();
        let decoder = flate2::read::GzDecoder::new(file);
        let mut archive = tar::Archive::new(decoder);

        let entries: Vec<String> = archive
            .entries()
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.path().unwrap().display().to_string())
            .collect();

        assert!(
            entries.iter().any(|e| e == "manifest.json"),
            "archive should contain manifest.json, found: {:?}",
            entries
        );
        assert!(
            entries.iter().any(|e| e.contains("artifacts")),
            "archive should contain artifacts/, found: {:?}",
            entries
        );
        assert!(
            entries.iter().any(|e| e.contains("events")),
            "archive should contain events/, found: {:?}",
            entries
        );
    }

    #[test]
    fn create_archive_excludes_worktree_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let run_dir = create_test_run_dir(tmp.path());

        // Add a worktree directory that should be excluded
        let worktree_dir = run_dir.join("worktree");
        std::fs::create_dir_all(&worktree_dir).unwrap();
        std::fs::write(worktree_dir.join("some_file.rs"), "fn main() {}").unwrap();

        let output = tmp.path().join("output.tgz");
        create_archive(&run_dir, &output).unwrap();

        // Verify worktree is not in the archive
        let file = std::fs::File::open(&output).unwrap();
        let decoder = flate2::read::GzDecoder::new(file);
        let mut archive = tar::Archive::new(decoder);

        let entries: Vec<String> = archive
            .entries()
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.path().unwrap().display().to_string())
            .collect();

        assert!(
            !entries.iter().any(|e| e.contains("worktree")),
            "archive should NOT contain worktree/, found: {:?}",
            entries
        );
    }

    #[test]
    fn create_archive_includes_manifest_json() {
        let tmp = tempfile::tempdir().unwrap();
        let run_dir = create_test_run_dir(tmp.path());
        let output = tmp.path().join("output.tgz");

        create_archive(&run_dir, &output).unwrap();

        let file = std::fs::File::open(&output).unwrap();
        let decoder = flate2::read::GzDecoder::new(file);
        let mut archive = tar::Archive::new(decoder);

        let has_manifest = archive
            .entries()
            .unwrap()
            .filter_map(|e| e.ok())
            .any(|e| e.path().unwrap().display().to_string() == "manifest.json");

        assert!(has_manifest, "archive must include manifest.json");
    }

    #[test]
    fn create_archive_of_empty_run_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let run_dir = tmp.path().join("empty-run");
        std::fs::create_dir_all(&run_dir).unwrap();

        let output = tmp.path().join("output.tgz");
        let result = create_archive(&run_dir, &output);

        // Should succeed even with empty directory
        assert!(result.is_ok());
        assert!(output.exists());
    }

    #[test]
    fn create_archive_nonexistent_run_dir_returns_error() {
        let tmp = tempfile::tempdir().unwrap();
        let run_dir = tmp.path().join("does-not-exist");
        let output = tmp.path().join("output.tgz");

        let result = create_archive(&run_dir, &output);
        assert!(result.is_err());

        match result.unwrap_err() {
            ArchiveError::RunDirNotFound { path } => {
                assert!(path.contains("does-not-exist"));
            }
            other => panic!("expected RunDirNotFound, got: {other}"),
        }
    }

    #[test]
    fn archive_args_parse_with_defaults() {
        use clap::Parser;

        #[derive(Debug, Parser)]
        struct TestCli {
            #[command(flatten)]
            archive: ArchiveArgs,
        }

        let cli = TestCli::parse_from(["test", "/tmp/run-123"]);
        assert_eq!(cli.archive.run_dir, "/tmp/run-123");
        assert!(cli.archive.output.is_none());
    }

    #[test]
    fn archive_args_parse_with_output() {
        use clap::Parser;

        #[derive(Debug, Parser)]
        struct TestCli {
            #[command(flatten)]
            archive: ArchiveArgs,
        }

        let cli = TestCli::parse_from(["test", "/tmp/run-123", "-o", "/tmp/archive.tgz"]);
        assert_eq!(cli.archive.run_dir, "/tmp/run-123");
        assert_eq!(cli.archive.output, Some("/tmp/archive.tgz".into()));
    }
}
