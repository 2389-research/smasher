// ABOUTME: Sandboxed execution environment for agent tool operations (file I/O, search, shell).
// ABOUTME: Defines the ExecutionEnvironment trait and a LocalExecutionEnvironment implementation.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use async_trait::async_trait;
use glob::glob as glob_match;
use regex::Regex;
use tokio::fs;
use tokio::process::Command;

/// Errors that can occur during environment operations.
#[derive(Debug, thiserror::Error)]
pub enum EnvironmentError {
    #[error("file not found: {path}")]
    FileNotFound { path: String },

    #[error("permission denied: {path}")]
    PermissionDenied { path: String },

    #[error("command failed with exit code {exit_code}: {stderr}")]
    CommandFailed { exit_code: i32, stderr: String },

    #[error("command timed out after {timeout_ms}ms")]
    CommandTimeout { timeout_ms: u64 },

    #[error("I/O error: {message}")]
    Io { message: String },

    #[error("invalid pattern: {message}")]
    InvalidPattern { message: String },

    #[error("path traversal denied: {path} escapes the working directory")]
    PathTraversal { path: String },
}

/// Options controlling grep behavior.
#[derive(Debug, Clone, Default)]
pub struct GrepOptions {
    /// Whether to search recursively.
    pub recursive: bool,
    /// Maximum number of matches to return.
    pub max_matches: Option<usize>,
    /// File glob pattern to filter files.
    pub file_pattern: Option<String>,
    /// Whether the regex is case-insensitive.
    pub case_insensitive: bool,
}

/// A single grep match with file location and content.
#[derive(Debug, Clone)]
pub struct GrepMatch {
    pub file: String,
    pub line_number: u32,
    pub line: String,
}

/// Options controlling shell command execution.
#[derive(Debug, Clone, Default)]
pub struct ExecOptions {
    /// Working directory override.
    pub cwd: Option<String>,
    /// Timeout in milliseconds.
    pub timeout_ms: Option<u64>,
    /// Environment variables to set.
    pub env: Option<HashMap<String, String>>,
}

/// The result of executing a shell command.
#[derive(Debug, Clone)]
pub struct ExecResult {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
    pub duration_ms: u64,
}

/// Sandboxed access to the filesystem and process execution for agent tools.
///
/// Implementations provide file I/O, search, and shell access within a
/// controlled working directory.
#[async_trait]
pub trait ExecutionEnvironment: Send + Sync {
    /// Read a file's contents.
    async fn read_file(&self, path: &str) -> Result<String, EnvironmentError>;

    /// Write content to a file (creating directories as needed).
    async fn write_file(&self, path: &str, content: &str) -> Result<(), EnvironmentError>;

    /// List files matching a glob pattern.
    async fn glob_files(&self, pattern: &str) -> Result<Vec<String>, EnvironmentError>;

    /// Search file contents with a regex pattern.
    async fn grep(
        &self,
        pattern: &str,
        path: &str,
        options: GrepOptions,
    ) -> Result<Vec<GrepMatch>, EnvironmentError>;

    /// Execute a shell command.
    async fn exec_command(
        &self,
        command: &str,
        options: ExecOptions,
    ) -> Result<ExecResult, EnvironmentError>;

    /// Get the working directory.
    fn working_directory(&self) -> &str;

    /// Delete a file from the filesystem.
    async fn delete_file(&self, path: &str) -> Result<(), EnvironmentError>;

    /// Check if a path exists.
    async fn path_exists(&self, path: &str) -> bool;

    /// Check if a path is a directory.
    async fn is_directory(&self, path: &str) -> bool;
}

/// An execution environment backed by the local filesystem and OS shell.
pub struct LocalExecutionEnvironment {
    working_directory: String,
    /// Environment variable key substrings to filter out (security-sensitive).
    env_filter: Vec<String>,
}

impl LocalExecutionEnvironment {
    /// Create a local execution environment rooted at the given working directory.
    pub fn new(working_directory: String) -> Self {
        Self {
            working_directory,
            env_filter: vec![
                "AWS_SECRET".to_string(),
                "API_KEY".to_string(),
                "TOKEN".to_string(),
                "PASSWORD".to_string(),
                "SECRET".to_string(),
            ],
        }
    }

    /// Override the default environment variable filter list.
    pub fn with_env_filter(mut self, filter: Vec<String>) -> Self {
        self.env_filter = filter;
        self
    }

    /// Resolve a path relative to the working directory, or return it as-is if absolute.
    fn resolve_path(&self, path: &str) -> PathBuf {
        let p = Path::new(path);
        if p.is_absolute() {
            p.to_path_buf()
        } else {
            Path::new(&self.working_directory).join(p)
        }
    }

    /// Resolve and validate that a path stays within the working directory.
    ///
    /// For existing paths, canonicalizes the full path. For paths that don't
    /// exist yet (e.g. write targets), canonicalizes the nearest existing
    /// ancestor and appends the remaining components.
    fn validate_path(&self, path: &str) -> Result<PathBuf, EnvironmentError> {
        let resolved = self.resolve_path(path);

        let canonical_root = Path::new(&self.working_directory)
            .canonicalize()
            .map_err(|e| EnvironmentError::Io {
                message: format!("cannot canonicalize working directory: {e}"),
            })?;

        // Try to canonicalize the full resolved path first (works for existing paths).
        let canonical = if resolved.exists() {
            resolved.canonicalize().map_err(|e| EnvironmentError::Io {
                message: format!("{}: {e}", resolved.display()),
            })?
        } else {
            // For paths that don't exist yet, walk up to the nearest existing
            // ancestor, canonicalize that, then re-append the remaining tail.
            let mut ancestor = resolved.as_path();
            let mut tail_components = Vec::new();
            loop {
                if ancestor.exists() {
                    break;
                }
                if let Some(file_name) = ancestor.file_name() {
                    tail_components.push(file_name.to_os_string());
                    ancestor = ancestor.parent().unwrap_or(Path::new("/"));
                } else {
                    // No more components to strip — fall back to resolved.
                    break;
                }
            }
            let mut base = ancestor.canonicalize().map_err(|e| EnvironmentError::Io {
                message: format!("{}: {e}", ancestor.display()),
            })?;
            for component in tail_components.into_iter().rev() {
                base.push(component);
            }
            base
        };

        if !canonical.starts_with(&canonical_root) {
            return Err(EnvironmentError::PathTraversal {
                path: path.to_string(),
            });
        }

        Ok(canonical)
    }

    /// Determine whether an env var key should be filtered out.
    fn should_filter_env(&self, key: &str) -> bool {
        let key_upper = key.to_uppercase();
        self.env_filter
            .iter()
            .any(|f| key_upper.contains(&f.to_uppercase()))
    }

    /// Collect files to search for grep, honoring recursive and file_pattern options.
    fn collect_files_for_grep(
        &self,
        search_path: &Path,
        options: &GrepOptions,
    ) -> Result<Vec<PathBuf>, EnvironmentError> {
        if search_path.is_file() {
            return Ok(vec![search_path.to_path_buf()]);
        }

        if let Some(ref file_pattern) = options.file_pattern {
            let full_pattern = if options.recursive {
                format!("{}/**/{}", search_path.display(), file_pattern)
            } else {
                format!("{}/{}", search_path.display(), file_pattern)
            };
            let entries =
                glob_match(&full_pattern).map_err(|e| EnvironmentError::InvalidPattern {
                    message: e.to_string(),
                })?;
            let mut files = Vec::new();
            for entry in entries {
                match entry {
                    Ok(p) if p.is_file() => files.push(p),
                    Ok(_) => {}
                    Err(e) => {
                        return Err(EnvironmentError::Io {
                            message: e.to_string(),
                        });
                    }
                }
            }
            Ok(files)
        } else if options.recursive {
            Self::walk_dir_recursive(search_path)
        } else {
            Self::walk_dir_shallow(search_path)
        }
    }

    /// Recursively collect all files under a directory.
    fn walk_dir_recursive(dir: &Path) -> Result<Vec<PathBuf>, EnvironmentError> {
        let mut files = Vec::new();
        let entries = std::fs::read_dir(dir).map_err(|e| EnvironmentError::Io {
            message: e.to_string(),
        })?;
        for entry in entries {
            let entry = entry.map_err(|e| EnvironmentError::Io {
                message: e.to_string(),
            })?;
            let path = entry.path();
            if path.is_dir() {
                files.extend(Self::walk_dir_recursive(&path)?);
            } else if path.is_file() {
                files.push(path);
            }
        }
        Ok(files)
    }

    /// Collect files in a single directory (non-recursive).
    fn walk_dir_shallow(dir: &Path) -> Result<Vec<PathBuf>, EnvironmentError> {
        let mut files = Vec::new();
        let entries = std::fs::read_dir(dir).map_err(|e| EnvironmentError::Io {
            message: e.to_string(),
        })?;
        for entry in entries {
            let entry = entry.map_err(|e| EnvironmentError::Io {
                message: e.to_string(),
            })?;
            let path = entry.path();
            if path.is_file() {
                files.push(path);
            }
        }
        Ok(files)
    }

    /// Kill a process group given the leader's PID.
    #[cfg(unix)]
    fn kill_process_group(pid: u32) {
        use std::process::Command as StdCommand;
        // Use kill to send SIGKILL to the process group (negative PID).
        let _ = StdCommand::new("kill")
            .args(["-9", &format!("-{pid}")])
            .output();
    }
}

/// Map std::io::Error to EnvironmentError, distinguishing not-found and permission errors.
fn map_io_error(e: std::io::Error, path: &str) -> EnvironmentError {
    match e.kind() {
        std::io::ErrorKind::NotFound => EnvironmentError::FileNotFound {
            path: path.to_string(),
        },
        std::io::ErrorKind::PermissionDenied => EnvironmentError::PermissionDenied {
            path: path.to_string(),
        },
        _ => EnvironmentError::Io {
            message: format!("{path}: {e}"),
        },
    }
}

#[async_trait]
impl ExecutionEnvironment for LocalExecutionEnvironment {
    async fn read_file(&self, path: &str) -> Result<String, EnvironmentError> {
        let resolved = self.validate_path(path)?;
        let resolved_str = resolved.display().to_string();
        fs::read_to_string(&resolved)
            .await
            .map_err(|e| map_io_error(e, &resolved_str))
    }

    async fn write_file(&self, path: &str, content: &str) -> Result<(), EnvironmentError> {
        let resolved = self.validate_path(path)?;
        let resolved_str = resolved.display().to_string();
        if let Some(parent) = resolved.parent() {
            fs::create_dir_all(parent)
                .await
                .map_err(|e| map_io_error(e, &parent.display().to_string()))?;
        }
        fs::write(&resolved, content)
            .await
            .map_err(|e| map_io_error(e, &resolved_str))
    }

    async fn glob_files(&self, pattern: &str) -> Result<Vec<String>, EnvironmentError> {
        // Reject patterns that contain path traversal components.
        if pattern.contains("..") {
            return Err(EnvironmentError::PathTraversal {
                path: pattern.to_string(),
            });
        }

        // Reject absolute patterns that don't fall under the working directory.
        let resolved_pattern = if Path::new(pattern).is_absolute() {
            let canonical_root =
                Path::new(&self.working_directory)
                    .canonicalize()
                    .map_err(|e| EnvironmentError::Io {
                        message: format!("cannot canonicalize working directory: {e}"),
                    })?;
            let canonical_root_str = canonical_root.display().to_string();
            if !pattern.starts_with(&canonical_root_str) {
                return Err(EnvironmentError::PathTraversal {
                    path: pattern.to_string(),
                });
            }
            pattern.to_string()
        } else {
            format!("{}/{}", self.working_directory, pattern)
        };

        let entries =
            glob_match(&resolved_pattern).map_err(|e| EnvironmentError::InvalidPattern {
                message: e.to_string(),
            })?;

        let mut results = Vec::new();
        for entry in entries {
            match entry {
                Ok(path) => results.push(path.display().to_string()),
                Err(e) => {
                    return Err(EnvironmentError::Io {
                        message: e.to_string(),
                    });
                }
            }
        }
        Ok(results)
    }

    async fn grep(
        &self,
        pattern: &str,
        path: &str,
        options: GrepOptions,
    ) -> Result<Vec<GrepMatch>, EnvironmentError> {
        let regex_pattern = if options.case_insensitive {
            format!("(?i){pattern}")
        } else {
            pattern.to_string()
        };
        let re = Regex::new(&regex_pattern).map_err(|e| EnvironmentError::InvalidPattern {
            message: e.to_string(),
        })?;

        let search_path = self.validate_path(path)?;
        let files = self.collect_files_for_grep(&search_path, &options)?;

        let mut matches = Vec::new();
        let max = options.max_matches;

        'outer: for file_path in &files {
            let content = match std::fs::read_to_string(file_path) {
                Ok(c) => c,
                // Skip binary / unreadable files.
                Err(_) => continue,
            };
            for (idx, line) in content.lines().enumerate() {
                if re.is_match(line) {
                    matches.push(GrepMatch {
                        file: file_path.display().to_string(),
                        line_number: (idx + 1) as u32,
                        line: line.to_string(),
                    });
                    if let Some(m) = max
                        && matches.len() >= m
                    {
                        break 'outer;
                    }
                }
            }
        }

        Ok(matches)
    }

    async fn exec_command(
        &self,
        command: &str,
        options: ExecOptions,
    ) -> Result<ExecResult, EnvironmentError> {
        // Validate custom cwd stays within the working directory.
        if let Some(ref custom_cwd) = options.cwd {
            self.validate_path(custom_cwd)?;
        }
        let cwd = options.cwd.as_deref().unwrap_or(&self.working_directory);

        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg(command);
        cmd.current_dir(cwd);

        // Enable process group for clean timeout cleanup.
        #[cfg(unix)]
        cmd.process_group(0);

        // Filter sensitive environment variables from the inherited env.
        cmd.env_clear();
        for (key, value) in std::env::vars() {
            if !self.should_filter_env(&key) {
                cmd.env(&key, &value);
            }
        }

        // Apply caller-specified env vars (these bypass the filter).
        if let Some(ref extra_env) = options.env {
            for (key, value) in extra_env {
                cmd.env(key, value);
            }
        }

        let start = Instant::now();

        let child = cmd
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| EnvironmentError::Io {
                message: format!("failed to spawn command: {e}"),
            })?;

        // Capture the PID before consuming the child via wait_with_output.
        let child_pid = child.id();

        if let Some(timeout_ms) = options.timeout_ms {
            let timeout_duration = std::time::Duration::from_millis(timeout_ms);
            match tokio::time::timeout(timeout_duration, child.wait_with_output()).await {
                Ok(Ok(output)) => {
                    let duration_ms = start.elapsed().as_millis() as u64;
                    Ok(ExecResult {
                        exit_code: output.status.code().unwrap_or(-1),
                        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
                        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
                        duration_ms,
                    })
                }
                Ok(Err(e)) => Err(EnvironmentError::Io {
                    message: format!("failed to wait for command: {e}"),
                }),
                Err(_) => {
                    // Timeout: kill the process group using the saved PID.
                    if let Some(pid) = child_pid {
                        #[cfg(unix)]
                        Self::kill_process_group(pid);
                    }
                    Err(EnvironmentError::CommandTimeout { timeout_ms })
                }
            }
        } else {
            let output = child
                .wait_with_output()
                .await
                .map_err(|e| EnvironmentError::Io {
                    message: format!("failed to wait for command: {e}"),
                })?;
            let duration_ms = start.elapsed().as_millis() as u64;
            Ok(ExecResult {
                exit_code: output.status.code().unwrap_or(-1),
                stdout: String::from_utf8_lossy(&output.stdout).to_string(),
                stderr: String::from_utf8_lossy(&output.stderr).to_string(),
                duration_ms,
            })
        }
    }

    fn working_directory(&self) -> &str {
        &self.working_directory
    }

    async fn delete_file(&self, path: &str) -> Result<(), EnvironmentError> {
        let resolved = self.validate_path(path)?;
        let resolved_str = resolved.display().to_string();
        fs::remove_file(&resolved)
            .await
            .map_err(|e| map_io_error(e, &resolved_str))
    }

    async fn path_exists(&self, path: &str) -> bool {
        let resolved = self.resolve_path(path);
        fs::metadata(&resolved).await.is_ok()
    }

    async fn is_directory(&self, path: &str) -> bool {
        let resolved = self.resolve_path(path);
        match fs::metadata(&resolved).await {
            Ok(meta) => meta.is_dir(),
            Err(_) => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Create a unique temporary directory for a test.
    fn make_temp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("smasher_env_test_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("failed to create temp dir");
        dir
    }

    /// Remove a temporary directory after a test.
    fn cleanup(dir: &Path) {
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn read_file_returns_content() {
        let dir = make_temp_dir();
        let file = dir.join("hello.txt");
        std::fs::write(&file, "hello world").unwrap();

        let env = LocalExecutionEnvironment::new(dir.display().to_string());
        let content = env.read_file("hello.txt").await.unwrap();
        assert_eq!(content, "hello world");

        cleanup(&dir);
    }

    #[tokio::test]
    async fn read_file_returns_error_for_missing() {
        let dir = make_temp_dir();
        let env = LocalExecutionEnvironment::new(dir.display().to_string());

        let result = env.read_file("nonexistent.txt").await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            matches!(err, EnvironmentError::FileNotFound { .. }),
            "expected FileNotFound, got: {err:?}"
        );

        cleanup(&dir);
    }

    #[tokio::test]
    async fn write_file_creates_file_and_parent_dirs() {
        let dir = make_temp_dir();
        let env = LocalExecutionEnvironment::new(dir.display().to_string());

        env.write_file("sub/dir/file.txt", "nested content")
            .await
            .unwrap();

        let content = std::fs::read_to_string(dir.join("sub/dir/file.txt")).unwrap();
        assert_eq!(content, "nested content");

        cleanup(&dir);
    }

    #[tokio::test]
    async fn write_file_overwrites_existing() {
        let dir = make_temp_dir();
        let file = dir.join("overwrite.txt");
        std::fs::write(&file, "original").unwrap();

        let env = LocalExecutionEnvironment::new(dir.display().to_string());
        env.write_file("overwrite.txt", "replaced").await.unwrap();

        let content = std::fs::read_to_string(&file).unwrap();
        assert_eq!(content, "replaced");

        cleanup(&dir);
    }

    #[tokio::test]
    async fn glob_files_finds_matching() {
        let dir = make_temp_dir();
        std::fs::write(dir.join("a.rs"), "").unwrap();
        std::fs::write(dir.join("b.rs"), "").unwrap();
        std::fs::write(dir.join("c.txt"), "").unwrap();

        let env = LocalExecutionEnvironment::new(dir.display().to_string());
        let mut results = env.glob_files("*.rs").await.unwrap();
        results.sort();

        assert_eq!(results.len(), 2);
        assert!(results[0].ends_with("a.rs"));
        assert!(results[1].ends_with("b.rs"));

        cleanup(&dir);
    }

    #[tokio::test]
    async fn glob_files_returns_empty_for_no_matches() {
        let dir = make_temp_dir();
        let env = LocalExecutionEnvironment::new(dir.display().to_string());

        let results = env.glob_files("*.nonexistent").await.unwrap();
        assert!(results.is_empty());

        cleanup(&dir);
    }

    #[tokio::test]
    async fn grep_finds_matching_lines() {
        let dir = make_temp_dir();
        std::fs::write(
            dir.join("search.txt"),
            "line one\nline two\nfind me here\nline four\nfind me again\n",
        )
        .unwrap();

        let env = LocalExecutionEnvironment::new(dir.display().to_string());
        let matches = env
            .grep("find me", "search.txt", GrepOptions::default())
            .await
            .unwrap();

        assert_eq!(matches.len(), 2);
        assert_eq!(matches[0].line_number, 3);
        assert_eq!(matches[0].line, "find me here");
        assert_eq!(matches[1].line_number, 5);
        assert_eq!(matches[1].line, "find me again");

        cleanup(&dir);
    }

    #[tokio::test]
    async fn grep_respects_max_matches() {
        let dir = make_temp_dir();
        std::fs::write(dir.join("many.txt"), "match\nmatch\nmatch\nmatch\nmatch\n").unwrap();

        let env = LocalExecutionEnvironment::new(dir.display().to_string());
        let matches = env
            .grep(
                "match",
                "many.txt",
                GrepOptions {
                    max_matches: Some(2),
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        assert_eq!(matches.len(), 2);

        cleanup(&dir);
    }

    #[tokio::test]
    async fn grep_case_insensitive() {
        let dir = make_temp_dir();
        std::fs::write(
            dir.join("case.txt"),
            "Hello World\nhello world\nHELLO WORLD\n",
        )
        .unwrap();

        let env = LocalExecutionEnvironment::new(dir.display().to_string());
        let matches = env
            .grep(
                "hello",
                "case.txt",
                GrepOptions {
                    case_insensitive: true,
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        assert_eq!(matches.len(), 3);

        cleanup(&dir);
    }

    #[tokio::test]
    async fn exec_command_runs_simple_command() {
        let dir = make_temp_dir();
        let env = LocalExecutionEnvironment::new(dir.display().to_string());

        let result = env
            .exec_command("echo hello", ExecOptions::default())
            .await
            .unwrap();

        assert_eq!(result.exit_code, 0);
        assert_eq!(result.stdout.trim(), "hello");

        cleanup(&dir);
    }

    #[tokio::test]
    async fn exec_command_captures_exit_code_on_failure() {
        let dir = make_temp_dir();
        let env = LocalExecutionEnvironment::new(dir.display().to_string());

        let result = env
            .exec_command("exit 42", ExecOptions::default())
            .await
            .unwrap();

        assert_eq!(result.exit_code, 42);

        cleanup(&dir);
    }

    #[tokio::test]
    async fn exec_command_respects_cwd() {
        let dir = make_temp_dir();
        let subdir = dir.join("workdir");
        std::fs::create_dir_all(&subdir).unwrap();

        let env = LocalExecutionEnvironment::new(dir.display().to_string());
        let result = env
            .exec_command(
                "pwd",
                ExecOptions {
                    cwd: Some(subdir.display().to_string()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        assert_eq!(result.exit_code, 0);
        // The resolved path might have /private prefix on macOS.
        let stdout = result.stdout.trim();
        assert!(
            stdout.ends_with("workdir"),
            "expected cwd ending in 'workdir', got: {stdout}"
        );

        cleanup(&dir);
    }

    #[tokio::test]
    async fn path_exists_returns_true_for_existing() {
        let dir = make_temp_dir();
        std::fs::write(dir.join("exists.txt"), "").unwrap();

        let env = LocalExecutionEnvironment::new(dir.display().to_string());
        assert!(env.path_exists("exists.txt").await);

        cleanup(&dir);
    }

    #[tokio::test]
    async fn path_exists_returns_false_for_missing() {
        let dir = make_temp_dir();
        let env = LocalExecutionEnvironment::new(dir.display().to_string());

        assert!(!env.path_exists("ghost.txt").await);

        cleanup(&dir);
    }

    #[tokio::test]
    async fn is_directory_returns_true_for_dir() {
        let dir = make_temp_dir();
        std::fs::create_dir_all(dir.join("mydir")).unwrap();

        let env = LocalExecutionEnvironment::new(dir.display().to_string());
        assert!(env.is_directory("mydir").await);

        cleanup(&dir);
    }

    #[tokio::test]
    async fn is_directory_returns_false_for_file() {
        let dir = make_temp_dir();
        std::fs::write(dir.join("afile.txt"), "").unwrap();

        let env = LocalExecutionEnvironment::new(dir.display().to_string());
        assert!(!env.is_directory("afile.txt").await);

        cleanup(&dir);
    }

    #[tokio::test]
    async fn working_directory_returns_correct_value() {
        let dir = make_temp_dir();
        let expected = dir.display().to_string();
        let env = LocalExecutionEnvironment::new(expected.clone());

        assert_eq!(env.working_directory(), expected);

        cleanup(&dir);
    }

    #[tokio::test]
    async fn exec_command_timeout() {
        let dir = make_temp_dir();
        let env = LocalExecutionEnvironment::new(dir.display().to_string());

        let result = env
            .exec_command(
                "sleep 10",
                ExecOptions {
                    timeout_ms: Some(100),
                    ..Default::default()
                },
            )
            .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            matches!(err, EnvironmentError::CommandTimeout { timeout_ms: 100 }),
            "expected CommandTimeout, got: {err:?}"
        );

        cleanup(&dir);
    }

    // --- delete_file tests ---

    #[tokio::test]
    async fn delete_file_removes_existing_file() {
        let dir = make_temp_dir();
        let file = dir.join("doomed.txt");
        std::fs::write(&file, "goodbye").unwrap();
        assert!(file.exists());

        let env = LocalExecutionEnvironment::new(dir.display().to_string());
        env.delete_file("doomed.txt").await.unwrap();

        assert!(!file.exists(), "file should be removed from the filesystem");

        cleanup(&dir);
    }

    #[tokio::test]
    async fn delete_file_returns_error_for_missing() {
        let dir = make_temp_dir();
        let env = LocalExecutionEnvironment::new(dir.display().to_string());

        let result = env.delete_file("nonexistent.txt").await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            matches!(err, EnvironmentError::FileNotFound { .. }),
            "expected FileNotFound, got: {err:?}"
        );

        cleanup(&dir);
    }

    // --- Path traversal prevention tests ---

    #[tokio::test]
    async fn read_file_rejects_relative_traversal() {
        let dir = make_temp_dir();
        let env = LocalExecutionEnvironment::new(dir.display().to_string());

        let result = env.read_file("../../etc/passwd").await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            matches!(err, EnvironmentError::PathTraversal { .. }),
            "expected PathTraversal, got: {err:?}"
        );

        cleanup(&dir);
    }

    #[tokio::test]
    async fn read_file_rejects_absolute_path_outside() {
        let dir = make_temp_dir();
        let env = LocalExecutionEnvironment::new(dir.display().to_string());

        let result = env.read_file("/etc/passwd").await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            matches!(err, EnvironmentError::PathTraversal { .. }),
            "expected PathTraversal, got: {err:?}"
        );

        cleanup(&dir);
    }

    #[tokio::test]
    async fn write_file_rejects_traversal() {
        let dir = make_temp_dir();
        let env = LocalExecutionEnvironment::new(dir.display().to_string());

        let result = env.write_file("../escape.txt", "bad").await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            matches!(err, EnvironmentError::PathTraversal { .. }),
            "expected PathTraversal, got: {err:?}"
        );

        cleanup(&dir);
    }

    #[tokio::test]
    async fn exec_command_rejects_cwd_outside_working_dir() {
        let dir = make_temp_dir();
        let env = LocalExecutionEnvironment::new(dir.display().to_string());

        let result = env
            .exec_command(
                "ls",
                ExecOptions {
                    cwd: Some("/tmp".to_string()),
                    ..Default::default()
                },
            )
            .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            matches!(err, EnvironmentError::PathTraversal { .. }),
            "expected PathTraversal, got: {err:?}"
        );

        cleanup(&dir);
    }

    #[tokio::test]
    async fn relative_path_within_working_dir_succeeds() {
        let dir = make_temp_dir();
        let subdir = dir.join("inner");
        std::fs::create_dir_all(&subdir).unwrap();
        std::fs::write(subdir.join("data.txt"), "safe content").unwrap();

        let env = LocalExecutionEnvironment::new(dir.display().to_string());
        let content = env.read_file("inner/data.txt").await.unwrap();
        assert_eq!(content, "safe content");

        cleanup(&dir);
    }

    #[tokio::test]
    async fn write_file_to_subdir_succeeds() {
        let dir = make_temp_dir();
        let env = LocalExecutionEnvironment::new(dir.display().to_string());

        env.write_file("newdir/output.txt", "hello").await.unwrap();
        let content = std::fs::read_to_string(dir.join("newdir/output.txt")).unwrap();
        assert_eq!(content, "hello");

        cleanup(&dir);
    }

    #[tokio::test]
    async fn glob_files_rejects_dotdot_pattern() {
        let dir = make_temp_dir();
        let env = LocalExecutionEnvironment::new(dir.display().to_string());

        let result = env.glob_files("../../*").await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            matches!(err, EnvironmentError::PathTraversal { .. }),
            "expected PathTraversal, got: {err:?}"
        );

        cleanup(&dir);
    }

    #[tokio::test]
    async fn glob_files_rejects_absolute_pattern_outside() {
        let dir = make_temp_dir();
        let env = LocalExecutionEnvironment::new(dir.display().to_string());

        let result = env.glob_files("/tmp/*").await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            matches!(err, EnvironmentError::PathTraversal { .. }),
            "expected PathTraversal, got: {err:?}"
        );

        cleanup(&dir);
    }

    #[tokio::test]
    async fn grep_rejects_path_traversal() {
        let dir = make_temp_dir();
        let env = LocalExecutionEnvironment::new(dir.display().to_string());

        let result = env
            .grep("pattern", "../../etc", GrepOptions::default())
            .await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            matches!(err, EnvironmentError::PathTraversal { .. }),
            "expected PathTraversal, got: {err:?}"
        );

        cleanup(&dir);
    }

    #[tokio::test]
    async fn exec_command_allows_cwd_within_working_dir() {
        let dir = make_temp_dir();
        let subdir = dir.join("allowed");
        std::fs::create_dir_all(&subdir).unwrap();

        let env = LocalExecutionEnvironment::new(dir.display().to_string());
        let result = env
            .exec_command(
                "pwd",
                ExecOptions {
                    cwd: Some(subdir.display().to_string()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        assert_eq!(result.exit_code, 0);
        let stdout = result.stdout.trim();
        assert!(
            stdout.ends_with("allowed"),
            "expected cwd ending in 'allowed', got: {stdout}"
        );

        cleanup(&dir);
    }
}
