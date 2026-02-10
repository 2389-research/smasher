// ABOUTME: Git worktree management utilities for isolating pipeline runs in separate branches.
// ABOUTME: Provides create, commit, and cleanup operations via git CLI subprocess calls.

use std::path::Path;
use std::process::Command;

/// Error type for git operations.
#[derive(Debug, thiserror::Error)]
pub enum GitError {
    #[error("git command failed: {message}")]
    CommandFailed { message: String },

    #[error("not a git repository: {path}")]
    NotGitRepo { path: String },

    #[error("working tree is dirty")]
    DirtyWorkingTree,

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

/// Check if a path is inside a git repository.
pub fn is_git_repo(path: &Path) -> Result<bool, GitError> {
    let output = Command::new("git")
        .args(["-C", &path.display().to_string(), "rev-parse", "--git-dir"])
        .output()?;
    Ok(output.status.success())
}

/// Check if the working tree is clean (no uncommitted changes).
pub fn is_clean(repo_path: &Path) -> Result<bool, GitError> {
    if !is_git_repo(repo_path)? {
        return Err(GitError::NotGitRepo {
            path: repo_path.display().to_string(),
        });
    }
    let output = Command::new("git")
        .args([
            "-C",
            &repo_path.display().to_string(),
            "status",
            "--porcelain",
        ])
        .output()?;
    if !output.status.success() {
        return Err(GitError::CommandFailed {
            message: String::from_utf8_lossy(&output.stderr).to_string(),
        });
    }
    Ok(output.stdout.is_empty())
}

/// Get the current HEAD SHA.
pub fn current_sha(repo_path: &Path) -> Result<String, GitError> {
    if !is_git_repo(repo_path)? {
        return Err(GitError::NotGitRepo {
            path: repo_path.display().to_string(),
        });
    }
    let output = Command::new("git")
        .args(["-C", &repo_path.display().to_string(), "rev-parse", "HEAD"])
        .output()?;
    if !output.status.success() {
        return Err(GitError::CommandFailed {
            message: String::from_utf8_lossy(&output.stderr).to_string(),
        });
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Create a new git worktree at the given directory on a new branch.
///
/// Equivalent to: `git -C {repo_path} worktree add -b {branch_name} {worktree_dir} {base_sha}`
pub fn create_worktree(
    repo_path: &Path,
    worktree_dir: &Path,
    branch_name: &str,
    base_sha: &str,
) -> Result<(), GitError> {
    let output = Command::new("git")
        .args([
            "-C",
            &repo_path.display().to_string(),
            "worktree",
            "add",
            "-b",
            branch_name,
            &worktree_dir.display().to_string(),
            base_sha,
        ])
        .output()?;
    if !output.status.success() {
        return Err(GitError::CommandFailed {
            message: String::from_utf8_lossy(&output.stderr).to_string(),
        });
    }
    Ok(())
}

/// Commit all changes in a worktree directory.
///
/// Returns the commit SHA if changes were committed, or None if the worktree was clean.
pub fn commit_all_changes(worktree_dir: &Path, message: &str) -> Result<Option<String>, GitError> {
    let dir_str = worktree_dir.display().to_string();

    // Stage everything
    let add_output = Command::new("git")
        .args(["-C", &dir_str, "add", "-A"])
        .output()?;
    if !add_output.status.success() {
        return Err(GitError::CommandFailed {
            message: String::from_utf8_lossy(&add_output.stderr).to_string(),
        });
    }

    // Check if there's anything to commit
    let diff_output = Command::new("git")
        .args(["-C", &dir_str, "diff", "--cached", "--quiet"])
        .output()?;
    if diff_output.status.success() {
        // Exit 0 means no differences — nothing to commit
        return Ok(None);
    }

    // Commit
    let commit_output = Command::new("git")
        .args(["-C", &dir_str, "commit", "-m", message])
        .output()?;
    if !commit_output.status.success() {
        return Err(GitError::CommandFailed {
            message: String::from_utf8_lossy(&commit_output.stderr).to_string(),
        });
    }

    // Get the commit SHA
    let sha_output = Command::new("git")
        .args(["-C", &dir_str, "rev-parse", "HEAD"])
        .output()?;
    if !sha_output.status.success() {
        return Err(GitError::CommandFailed {
            message: String::from_utf8_lossy(&sha_output.stderr).to_string(),
        });
    }

    Ok(Some(
        String::from_utf8_lossy(&sha_output.stdout)
            .trim()
            .to_string(),
    ))
}

/// Remove a git worktree and delete its branch.
///
/// Finds the main repo via `git rev-parse --git-common-dir`, removes the worktree,
/// then deletes the branch that was created for it.
pub fn remove_worktree(worktree_dir: &Path) -> Result<(), GitError> {
    let dir_str = worktree_dir.display().to_string();

    // Find the main repo's .git dir
    let common_dir_output = Command::new("git")
        .args(["-C", &dir_str, "rev-parse", "--git-common-dir"])
        .output()?;
    if !common_dir_output.status.success() {
        return Err(GitError::CommandFailed {
            message: String::from_utf8_lossy(&common_dir_output.stderr).to_string(),
        });
    }
    let common_git_dir = String::from_utf8_lossy(&common_dir_output.stdout)
        .trim()
        .to_string();

    // The main repo is the parent of the .git dir (unless it's bare).
    // Use the common git dir to find the main worktree.
    let main_repo = if common_git_dir.ends_with("/.git") || common_git_dir.ends_with("\\.git") {
        Path::new(&common_git_dir)
            .parent()
            .unwrap_or(Path::new(&common_git_dir))
    } else {
        // Could be a bare repo or the path itself is the .git dir
        Path::new(&common_git_dir)
    };
    let main_repo_str = main_repo.display().to_string();

    // Get the branch name of the worktree before removing it
    let branch_output = Command::new("git")
        .args(["-C", &dir_str, "rev-parse", "--abbrev-ref", "HEAD"])
        .output()?;
    let branch_name = if branch_output.status.success() {
        let name = String::from_utf8_lossy(&branch_output.stdout)
            .trim()
            .to_string();
        if name == "HEAD" { None } else { Some(name) }
    } else {
        None
    };

    // Remove the worktree
    let remove_output = Command::new("git")
        .args([
            "-C",
            &main_repo_str,
            "worktree",
            "remove",
            &dir_str,
            "--force",
        ])
        .output()?;
    if !remove_output.status.success() {
        return Err(GitError::CommandFailed {
            message: String::from_utf8_lossy(&remove_output.stderr).to_string(),
        });
    }

    // Delete the branch if we found one
    if let Some(branch) = branch_name {
        let branch_output = Command::new("git")
            .args(["-C", &main_repo_str, "branch", "-D", &branch])
            .output()?;
        if !branch_output.status.success() {
            // Branch deletion failure is not fatal — the worktree is already removed
            tracing::warn!(
                branch = %branch,
                stderr = %String::from_utf8_lossy(&branch_output.stderr),
                "failed to delete worktree branch"
            );
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// Create a temporary git repo with an initial commit for testing.
    fn setup_test_repo() -> (tempfile::TempDir, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().to_path_buf();

        Command::new("git")
            .args(["init"])
            .current_dir(&repo)
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "user.email", "test@test.com"])
            .current_dir(&repo)
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "user.name", "Test"])
            .current_dir(&repo)
            .output()
            .unwrap();

        std::fs::write(repo.join("README.md"), "test").unwrap();

        Command::new("git")
            .args(["add", "."])
            .current_dir(&repo)
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "initial"])
            .current_dir(&repo)
            .output()
            .unwrap();

        (tmp, repo)
    }

    // ---------------------------------------------------------------
    // is_git_repo
    // ---------------------------------------------------------------

    #[test]
    fn is_git_repo_true_for_actual_repo() {
        let (_tmp, repo) = setup_test_repo();
        assert!(is_git_repo(&repo).unwrap());
    }

    #[test]
    fn is_git_repo_false_for_plain_directory() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(!is_git_repo(tmp.path()).unwrap());
    }

    // ---------------------------------------------------------------
    // is_clean
    // ---------------------------------------------------------------

    #[test]
    fn is_clean_true_for_clean_repo() {
        let (_tmp, repo) = setup_test_repo();
        assert!(is_clean(&repo).unwrap());
    }

    #[test]
    fn is_clean_false_after_modification() {
        let (_tmp, repo) = setup_test_repo();
        std::fs::write(repo.join("dirty.txt"), "uncommitted").unwrap();
        assert!(!is_clean(&repo).unwrap());
    }

    // ---------------------------------------------------------------
    // current_sha
    // ---------------------------------------------------------------

    #[test]
    fn current_sha_returns_valid_sha() {
        let (_tmp, repo) = setup_test_repo();
        let sha = current_sha(&repo).unwrap();
        assert_eq!(sha.len(), 40, "SHA should be 40 hex characters");
        assert!(
            sha.chars().all(|c| c.is_ascii_hexdigit()),
            "SHA should be all hex"
        );
    }

    // ---------------------------------------------------------------
    // create_worktree
    // ---------------------------------------------------------------

    #[test]
    fn create_worktree_creates_directory_and_branch() {
        let (_tmp, repo) = setup_test_repo();
        let sha = current_sha(&repo).unwrap();

        let worktree_dir = repo.join("wt-test");
        create_worktree(&repo, &worktree_dir, "test/branch", &sha).unwrap();

        assert!(worktree_dir.exists(), "worktree directory should exist");
        assert!(
            worktree_dir.join("README.md").exists(),
            "worktree should contain repo files"
        );

        // Verify branch was created
        let branch_output = Command::new("git")
            .args([
                "-C",
                &repo.display().to_string(),
                "branch",
                "--list",
                "test/branch",
            ])
            .output()
            .unwrap();
        let branch_list = String::from_utf8_lossy(&branch_output.stdout);
        assert!(
            branch_list.contains("test/branch"),
            "branch should exist in repo"
        );
    }

    // ---------------------------------------------------------------
    // commit_all_changes
    // ---------------------------------------------------------------

    #[test]
    fn commit_all_changes_with_changes_returns_sha() {
        let (_tmp, repo) = setup_test_repo();
        let sha = current_sha(&repo).unwrap();

        let worktree_dir = repo.join("wt-commit");
        create_worktree(&repo, &worktree_dir, "commit/test", &sha).unwrap();

        // Add a new file in the worktree
        std::fs::write(worktree_dir.join("new_file.txt"), "hello").unwrap();

        let result = commit_all_changes(&worktree_dir, "test commit").unwrap();
        assert!(result.is_some(), "should return a commit SHA");

        let commit_sha = result.unwrap();
        assert_eq!(commit_sha.len(), 40, "commit SHA should be 40 hex chars");
        assert!(commit_sha.chars().all(|c| c.is_ascii_hexdigit()));

        // The commit SHA should differ from the base SHA
        assert_ne!(commit_sha, sha);
    }

    #[test]
    fn commit_all_changes_no_changes_returns_none() {
        let (_tmp, repo) = setup_test_repo();
        let sha = current_sha(&repo).unwrap();

        let worktree_dir = repo.join("wt-clean");
        create_worktree(&repo, &worktree_dir, "clean/test", &sha).unwrap();

        let result = commit_all_changes(&worktree_dir, "empty commit").unwrap();
        assert!(
            result.is_none(),
            "should return None when nothing to commit"
        );
    }

    // ---------------------------------------------------------------
    // remove_worktree
    // ---------------------------------------------------------------

    #[test]
    fn remove_worktree_cleans_up_directory_and_branch() {
        let (_tmp, repo) = setup_test_repo();
        let sha = current_sha(&repo).unwrap();

        let worktree_dir = repo.join("wt-remove");
        create_worktree(&repo, &worktree_dir, "remove/test", &sha).unwrap();

        // Verify worktree exists before removal
        assert!(worktree_dir.exists());

        remove_worktree(&worktree_dir).unwrap();

        // Worktree directory should be gone
        assert!(
            !worktree_dir.exists(),
            "worktree directory should be removed"
        );

        // Branch should be deleted
        let branch_output = Command::new("git")
            .args([
                "-C",
                &repo.display().to_string(),
                "branch",
                "--list",
                "remove/test",
            ])
            .output()
            .unwrap();
        let branch_list = String::from_utf8_lossy(&branch_output.stdout);
        assert!(
            !branch_list.contains("remove/test"),
            "branch should be deleted"
        );
    }

    // ---------------------------------------------------------------
    // Error cases
    // ---------------------------------------------------------------

    #[test]
    fn is_clean_errors_for_non_git_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let result = is_clean(tmp.path());
        assert!(result.is_err());
        assert!(
            matches!(result.unwrap_err(), GitError::NotGitRepo { .. }),
            "should return NotGitRepo error"
        );
    }

    #[test]
    fn current_sha_errors_for_non_git_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let result = current_sha(tmp.path());
        assert!(result.is_err());
        assert!(
            matches!(result.unwrap_err(), GitError::NotGitRepo { .. }),
            "should return NotGitRepo error"
        );
    }

    // ---------------------------------------------------------------
    // Worktree cleanup with dirty state (simulates engine failure mid-run)
    // ---------------------------------------------------------------

    #[test]
    fn remove_worktree_succeeds_with_dirty_state() {
        let (_tmp, repo) = setup_test_repo();
        let sha = current_sha(&repo).unwrap();

        let worktree_dir = repo.join("wt-dirty");
        create_worktree(&repo, &worktree_dir, "dirty/test", &sha).unwrap();

        // Simulate a failed engine run leaving uncommitted changes.
        std::fs::write(worktree_dir.join("partial_output.txt"), "incomplete work").unwrap();
        std::fs::write(worktree_dir.join("README.md"), "modified content").unwrap();

        // Cleanup must succeed even with dirty working tree.
        remove_worktree(&worktree_dir).unwrap();

        assert!(
            !worktree_dir.exists(),
            "dirty worktree directory should still be removed"
        );
    }
}
