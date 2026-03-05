// ABOUTME: Concrete implementations of the six shared agent tools (read, write, edit, shell, grep, glob).
// ABOUTME: Each tool wraps an ExecutionEnvironment and implements the AgentTool trait.

use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use serde_json::{Value, json};

use crate::environment::{ExecOptions, ExecutionEnvironment, GrepOptions};
use crate::tools::{AgentTool, ToolOutput, ToolRegistry};

/// Parse a JSON string into a serde_json::Value, returning a ToolOutput error on failure.
fn parse_arguments(arguments: &str) -> Result<Value, ToolOutput> {
    serde_json::from_str(arguments)
        .map_err(|e| ToolOutput::error(format!("Invalid JSON arguments: {e}"), 0))
}

/// Extract a required string field from parsed JSON arguments.
fn required_str<'a>(args: &'a Value, field: &str) -> Result<&'a str, ToolOutput> {
    args.get(field)
        .and_then(|v| v.as_str())
        .ok_or_else(|| ToolOutput::error(format!("Missing required parameter: {field}"), 0))
}

// ---------------------------------------------------------------------------
// ReadFileTool
// ---------------------------------------------------------------------------

/// Reads the contents of a file at a given path.
pub struct ReadFileTool {
    env: Arc<dyn ExecutionEnvironment>,
}

impl ReadFileTool {
    pub fn new(env: Arc<dyn ExecutionEnvironment>) -> Self {
        Self { env }
    }
}

#[async_trait]
impl AgentTool for ReadFileTool {
    fn name(&self) -> &str {
        "read_file"
    }

    fn description(&self) -> &str {
        "Read the contents of a file at the given path"
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the file to read"
                },
                "offset": {
                    "type": "integer",
                    "description": "1-based line number to start reading from (default 1)"
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of lines to return (default 2000)"
                }
            },
            "required": ["path"]
        })
    }

    async fn execute(&self, arguments: &str) -> ToolOutput {
        let start = Instant::now();
        let args = match parse_arguments(arguments) {
            Ok(v) => v,
            Err(e) => return e,
        };
        let path = match required_str(&args, "path") {
            Ok(p) => p,
            Err(e) => return e,
        };

        let offset = args
            .get("offset")
            .and_then(|v| v.as_u64())
            .map(|v| v.max(1) as usize)
            .unwrap_or(1);
        let limit = args
            .get("limit")
            .and_then(|v| v.as_u64())
            .map(|v| v as usize)
            .unwrap_or(2000);

        let elapsed = || start.elapsed().as_millis() as u64;

        match self.env.read_file(path).await {
            Ok(content) => {
                let all_lines: Vec<&str> = content.lines().collect();
                let total = all_lines.len();
                // offset is 1-based, convert to 0-based index
                let start_idx = (offset - 1).min(total);
                let end_idx = start_idx.saturating_add(limit).min(total);
                let selected = &all_lines[start_idx..end_idx];

                // Determine width for right-aligned line numbers
                let max_line_num = if selected.is_empty() {
                    1
                } else {
                    start_idx + selected.len()
                };
                let width = max_line_num.to_string().len();

                let formatted: Vec<String> = selected
                    .iter()
                    .enumerate()
                    .map(|(i, line)| {
                        let line_num = start_idx + i + 1;
                        format!("{line_num:>width$} | {line}")
                    })
                    .collect();

                ToolOutput::success(formatted.join("\n"), elapsed())
            }
            Err(e) => ToolOutput::error(e.to_string(), elapsed()),
        }
    }
}

// ---------------------------------------------------------------------------
// WriteFileTool
// ---------------------------------------------------------------------------

/// Writes content to a file, creating parent directories as needed.
pub struct WriteFileTool {
    env: Arc<dyn ExecutionEnvironment>,
}

impl WriteFileTool {
    pub fn new(env: Arc<dyn ExecutionEnvironment>) -> Self {
        Self { env }
    }
}

#[async_trait]
impl AgentTool for WriteFileTool {
    fn name(&self) -> &str {
        "write_file"
    }

    fn description(&self) -> &str {
        "Write content to a file, creating directories as needed"
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string"
                },
                "content": {
                    "type": "string"
                }
            },
            "required": ["path", "content"]
        })
    }

    async fn execute(&self, arguments: &str) -> ToolOutput {
        let start = Instant::now();
        let args = match parse_arguments(arguments) {
            Ok(v) => v,
            Err(e) => return e,
        };
        let path = match required_str(&args, "path") {
            Ok(p) => p,
            Err(e) => return e,
        };
        let content = match required_str(&args, "content") {
            Ok(c) => c,
            Err(e) => return e,
        };

        let elapsed = || start.elapsed().as_millis() as u64;

        match self.env.write_file(path, content).await {
            Ok(()) => ToolOutput::success(format!("Successfully wrote to {path}"), elapsed()),
            Err(e) => ToolOutput::error(e.to_string(), elapsed()),
        }
    }
}

// ---------------------------------------------------------------------------
// EditFileTool
// ---------------------------------------------------------------------------

/// Edits a file by replacing an exact string match with new content.
pub struct EditFileTool {
    env: Arc<dyn ExecutionEnvironment>,
}

impl EditFileTool {
    pub fn new(env: Arc<dyn ExecutionEnvironment>) -> Self {
        Self { env }
    }
}

#[async_trait]
impl AgentTool for EditFileTool {
    fn name(&self) -> &str {
        "edit_file"
    }

    fn description(&self) -> &str {
        "Edit a file by replacing an exact string match with new content"
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string"
                },
                "old_string": {
                    "type": "string"
                },
                "new_string": {
                    "type": "string"
                }
            },
            "required": ["path", "old_string", "new_string"]
        })
    }

    async fn execute(&self, arguments: &str) -> ToolOutput {
        let start = Instant::now();
        let args = match parse_arguments(arguments) {
            Ok(v) => v,
            Err(e) => return e,
        };
        let path = match required_str(&args, "path") {
            Ok(p) => p,
            Err(e) => return e,
        };
        let old_string = match required_str(&args, "old_string") {
            Ok(s) => s,
            Err(e) => return e,
        };
        let new_string = match required_str(&args, "new_string") {
            Ok(s) => s,
            Err(e) => return e,
        };

        let elapsed = || start.elapsed().as_millis() as u64;

        // Read existing content
        let content = match self.env.read_file(path).await {
            Ok(c) => c,
            Err(e) => {
                return ToolOutput::error(format!("Failed to read file: {e}"), elapsed());
            }
        };

        // Count occurrences to detect ambiguity
        let match_count = content.matches(old_string).count();

        if match_count == 0 {
            return ToolOutput::error(format!("old_string not found in {path}"), elapsed());
        }

        if match_count > 1 {
            return ToolOutput::error(
                format!("old_string is ambiguous: found {match_count} occurrences in {path}"),
                elapsed(),
            );
        }

        // Exactly one match -- replace it
        let updated = content.replacen(old_string, new_string, 1);

        match self.env.write_file(path, &updated).await {
            Ok(()) => ToolOutput::success(format!("Successfully edited {path}"), elapsed()),
            Err(e) => ToolOutput::error(format!("Failed to write file: {e}"), elapsed()),
        }
    }
}

// ---------------------------------------------------------------------------
// ShellTool
// ---------------------------------------------------------------------------

/// Executes a shell command and returns its output.
pub struct ShellTool {
    env: Arc<dyn ExecutionEnvironment>,
}

impl ShellTool {
    pub fn new(env: Arc<dyn ExecutionEnvironment>) -> Self {
        Self { env }
    }
}

#[async_trait]
impl AgentTool for ShellTool {
    fn name(&self) -> &str {
        "shell"
    }

    fn description(&self) -> &str {
        "Execute a shell command and return its output"
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string"
                },
                "timeout_ms": {
                    "type": "integer",
                    "description": "Timeout in milliseconds (default 120000)"
                }
            },
            "required": ["command"]
        })
    }

    async fn execute(&self, arguments: &str) -> ToolOutput {
        let start = Instant::now();
        let args = match parse_arguments(arguments) {
            Ok(v) => v,
            Err(e) => return e,
        };
        let command = match required_str(&args, "command") {
            Ok(c) => c,
            Err(e) => return e,
        };
        let timeout_ms = args.get("timeout_ms").and_then(|v| v.as_u64());

        let options = ExecOptions {
            timeout_ms,
            ..Default::default()
        };

        let elapsed = || start.elapsed().as_millis() as u64;

        match self.env.exec_command(command, options).await {
            Ok(result) => {
                let mut output = String::new();

                if !result.stdout.is_empty() {
                    output.push_str(&result.stdout);
                }

                if !result.stderr.is_empty() {
                    if !output.is_empty() {
                        output.push('\n');
                    }
                    output.push_str("STDERR:\n");
                    output.push_str(&result.stderr);
                }

                if result.exit_code != 0 {
                    if !output.is_empty() {
                        output.push('\n');
                    }
                    output.push_str(&format!("Exit code: {}", result.exit_code));
                }

                if output.is_empty() {
                    output.push_str("(no output)");
                }

                if result.exit_code != 0 {
                    ToolOutput::error(output, elapsed())
                } else {
                    ToolOutput::success(output, elapsed())
                }
            }
            Err(e) => ToolOutput::error(e.to_string(), elapsed()),
        }
    }
}

// ---------------------------------------------------------------------------
// GrepTool
// ---------------------------------------------------------------------------

/// Searches file contents using a regex pattern.
pub struct GrepTool {
    env: Arc<dyn ExecutionEnvironment>,
}

impl GrepTool {
    pub fn new(env: Arc<dyn ExecutionEnvironment>) -> Self {
        Self { env }
    }
}

#[async_trait]
impl AgentTool for GrepTool {
    fn name(&self) -> &str {
        "grep"
    }

    fn description(&self) -> &str {
        "Search file contents using a regex pattern"
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string"
                },
                "path": {
                    "type": "string",
                    "description": "File or directory to search"
                },
                "file_pattern": {
                    "type": "string",
                    "description": "Glob pattern to filter files"
                },
                "case_insensitive": {
                    "type": "boolean"
                }
            },
            "required": ["pattern"]
        })
    }

    async fn execute(&self, arguments: &str) -> ToolOutput {
        let start = Instant::now();
        let args = match parse_arguments(arguments) {
            Ok(v) => v,
            Err(e) => return e,
        };
        let pattern = match required_str(&args, "pattern") {
            Ok(p) => p,
            Err(e) => return e,
        };

        let search_path = args.get("path").and_then(|v| v.as_str()).unwrap_or(".");

        let options = GrepOptions {
            file_pattern: args
                .get("file_pattern")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
            case_insensitive: args
                .get("case_insensitive")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
            recursive: true,
            ..Default::default()
        };

        let elapsed = || start.elapsed().as_millis() as u64;

        match self.env.grep(pattern, search_path, options).await {
            Ok(matches) => {
                if matches.is_empty() {
                    return ToolOutput::success("No matches found", elapsed());
                }

                let formatted: Vec<String> = matches
                    .iter()
                    .map(|m| format!("{}:{}:{}", m.file, m.line_number, m.line))
                    .collect();

                ToolOutput::success(formatted.join("\n"), elapsed())
            }
            Err(e) => ToolOutput::error(e.to_string(), elapsed()),
        }
    }
}

// ---------------------------------------------------------------------------
// GlobTool
// ---------------------------------------------------------------------------

/// Finds files matching a glob pattern.
pub struct GlobTool {
    env: Arc<dyn ExecutionEnvironment>,
}

impl GlobTool {
    pub fn new(env: Arc<dyn ExecutionEnvironment>) -> Self {
        Self { env }
    }
}

#[async_trait]
impl AgentTool for GlobTool {
    fn name(&self) -> &str {
        "glob_files"
    }

    fn description(&self) -> &str {
        "Find files matching a glob pattern"
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "Glob pattern to match files"
                }
            },
            "required": ["pattern"]
        })
    }

    async fn execute(&self, arguments: &str) -> ToolOutput {
        let start = Instant::now();
        let args = match parse_arguments(arguments) {
            Ok(v) => v,
            Err(e) => return e,
        };
        let pattern = match required_str(&args, "pattern") {
            Ok(p) => p,
            Err(e) => return e,
        };

        let elapsed = || start.elapsed().as_millis() as u64;

        match self.env.glob_files(pattern).await {
            Ok(paths) => {
                if paths.is_empty() {
                    return ToolOutput::success("No files matched", elapsed());
                }

                ToolOutput::success(paths.join("\n"), elapsed())
            }
            Err(e) => ToolOutput::error(e.to_string(), elapsed()),
        }
    }
}

// ---------------------------------------------------------------------------
// Registration helper
// ---------------------------------------------------------------------------

/// Register all six shared tools with the given registry.
pub fn register_shared_tools(registry: &mut ToolRegistry, env: Arc<dyn ExecutionEnvironment>) {
    registry.register(ReadFileTool::new(Arc::clone(&env)));
    registry.register(WriteFileTool::new(Arc::clone(&env)));
    registry.register(EditFileTool::new(Arc::clone(&env)));
    registry.register(ShellTool::new(Arc::clone(&env)));
    registry.register(GrepTool::new(Arc::clone(&env)));
    registry.register(GlobTool::new(env));
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::environment::{EnvironmentError, ExecResult, GrepMatch};
    use std::collections::HashMap;
    use std::sync::Mutex;

    /// In-memory mock of ExecutionEnvironment for testing tools without real I/O.
    struct MockEnvironment {
        files: Arc<Mutex<HashMap<String, String>>>,
        working_dir: String,
    }

    impl MockEnvironment {
        fn new() -> Self {
            Self {
                files: Arc::new(Mutex::new(HashMap::new())),
                working_dir: "/mock".to_string(),
            }
        }

        fn with_file(self, path: &str, content: &str) -> Self {
            self.files
                .lock()
                .unwrap()
                .insert(path.to_string(), content.to_string());
            self
        }
    }

    #[async_trait]
    impl ExecutionEnvironment for MockEnvironment {
        async fn read_file(&self, path: &str) -> Result<String, EnvironmentError> {
            let files = self.files.lock().unwrap();
            files
                .get(path)
                .cloned()
                .ok_or_else(|| EnvironmentError::FileNotFound {
                    path: path.to_string(),
                })
        }

        async fn write_file(&self, path: &str, content: &str) -> Result<(), EnvironmentError> {
            let mut files = self.files.lock().unwrap();
            files.insert(path.to_string(), content.to_string());
            Ok(())
        }

        async fn glob_files(&self, pattern: &str) -> Result<Vec<String>, EnvironmentError> {
            let files = self.files.lock().unwrap();
            let mut matched: Vec<String> = files
                .keys()
                .filter(|k| {
                    // Simple suffix matching for tests (e.g., "*.rs" matches "foo.rs").
                    if let Some(suffix) = pattern.strip_prefix('*') {
                        k.ends_with(suffix)
                    } else {
                        k.contains(pattern)
                    }
                })
                .cloned()
                .collect();
            matched.sort();
            Ok(matched)
        }

        async fn grep(
            &self,
            pattern: &str,
            _path: &str,
            _options: GrepOptions,
        ) -> Result<Vec<GrepMatch>, EnvironmentError> {
            let files = self.files.lock().unwrap();
            let mut matches = Vec::new();

            let mut sorted_files: Vec<_> = files.iter().collect();
            sorted_files.sort_by_key(|(k, _)| (*k).clone());

            for (file_path, content) in sorted_files {
                for (line_num, line) in content.lines().enumerate() {
                    if line.contains(pattern) {
                        matches.push(GrepMatch {
                            file: file_path.clone(),
                            line_number: (line_num + 1) as u32,
                            line: line.to_string(),
                        });
                    }
                }
            }

            Ok(matches)
        }

        async fn exec_command(
            &self,
            command: &str,
            _options: ExecOptions,
        ) -> Result<ExecResult, EnvironmentError> {
            Ok(ExecResult {
                stdout: format!("executed: {command}"),
                stderr: String::new(),
                exit_code: 0,
                duration_ms: 1,
            })
        }

        fn working_directory(&self) -> &str {
            &self.working_dir
        }

        async fn delete_file(&self, path: &str) -> Result<(), EnvironmentError> {
            let mut files = self.files.lock().unwrap();
            if files.remove(path).is_some() {
                Ok(())
            } else {
                Err(EnvironmentError::FileNotFound {
                    path: path.to_string(),
                })
            }
        }

        async fn path_exists(&self, path: &str) -> bool {
            let files = self.files.lock().unwrap();
            files.contains_key(path)
        }

        async fn is_directory(&self, _path: &str) -> bool {
            false
        }
    }

    // -- ReadFileTool tests --

    #[tokio::test]
    async fn read_file_reads_existing_file_with_line_numbers() {
        let env: Arc<dyn ExecutionEnvironment> =
            Arc::new(MockEnvironment::new().with_file("/test/hello.txt", "hello world"));
        let tool = ReadFileTool::new(env);

        let result = tool.execute(r#"{"path": "/test/hello.txt"}"#).await;

        assert!(!result.is_error);
        assert_eq!(result.content, "1 | hello world");
    }

    #[tokio::test]
    async fn read_file_multiline_line_numbers() {
        let env: Arc<dyn ExecutionEnvironment> = Arc::new(
            MockEnvironment::new()
                .with_file("/test/code.rs", "fn main() {\n    println!(\"hi\");\n}"),
        );
        let tool = ReadFileTool::new(env);

        let result = tool.execute(r#"{"path": "/test/code.rs"}"#).await;

        assert!(!result.is_error);
        assert!(result.content.contains("1 | fn main() {"));
        assert!(result.content.contains("2 |     println!(\"hi\");"));
        assert!(result.content.contains("3 | }"));
    }

    #[tokio::test]
    async fn read_file_with_offset_and_limit() {
        let env: Arc<dyn ExecutionEnvironment> = Arc::new(
            MockEnvironment::new()
                .with_file("/test/lines.txt", "line1\nline2\nline3\nline4\nline5"),
        );
        let tool = ReadFileTool::new(env);

        let result = tool
            .execute(r#"{"path": "/test/lines.txt", "offset": 2, "limit": 2}"#)
            .await;

        assert!(!result.is_error);
        // Should show lines 2 and 3 with original line numbers
        assert!(
            result.content.contains("2 | line2"),
            "got: {}",
            result.content
        );
        assert!(
            result.content.contains("3 | line3"),
            "got: {}",
            result.content
        );
        assert!(!result.content.contains("line1"));
        assert!(!result.content.contains("line4"));
    }

    #[tokio::test]
    async fn read_file_offset_only_defaults_limit_2000() {
        // Build a file with 10 lines; offset=3 should give lines 3..10 (well under 2000)
        let lines: Vec<String> = (1..=10).map(|i| format!("row_{i}")).collect();
        let content = lines.join("\n");
        let env: Arc<dyn ExecutionEnvironment> =
            Arc::new(MockEnvironment::new().with_file("/test/big.txt", &content));
        let tool = ReadFileTool::new(env);

        let result = tool
            .execute(r#"{"path": "/test/big.txt", "offset": 3}"#)
            .await;

        assert!(!result.is_error);
        assert!(!result.content.contains("row_1\n"));
        assert!(!result.content.contains("row_2\n"));
        assert!(result.content.contains(" 3 | row_3"));
        assert!(result.content.contains("10 | row_10"));
    }

    #[tokio::test]
    async fn read_file_limit_only_defaults_offset_1() {
        let env: Arc<dyn ExecutionEnvironment> = Arc::new(
            MockEnvironment::new()
                .with_file("/test/lines.txt", "line1\nline2\nline3\nline4\nline5"),
        );
        let tool = ReadFileTool::new(env);

        let result = tool
            .execute(r#"{"path": "/test/lines.txt", "limit": 3}"#)
            .await;

        assert!(!result.is_error);
        assert!(result.content.contains("1 | line1"));
        assert!(result.content.contains("3 | line3"));
        assert!(!result.content.contains("line4"));
    }

    #[tokio::test]
    async fn read_file_right_aligns_line_numbers() {
        // A file with 100+ lines should have right-aligned numbers
        let lines: Vec<String> = (1..=105).map(|i| format!("content{i}")).collect();
        let content = lines.join("\n");
        let env: Arc<dyn ExecutionEnvironment> =
            Arc::new(MockEnvironment::new().with_file("/test/big.txt", &content));
        let tool = ReadFileTool::new(env);

        let result = tool.execute(r#"{"path": "/test/big.txt"}"#).await;

        assert!(!result.is_error);
        // Line 1 should be right-aligned to 3 digits width: "  1 | content1"
        assert!(
            result.content.contains("  1 | content1"),
            "got: {}",
            result.content
        );
        assert!(
            result.content.contains("105 | content105"),
            "got: {}",
            result.content
        );
    }

    #[tokio::test]
    async fn read_file_returns_error_for_missing_file() {
        let env: Arc<dyn ExecutionEnvironment> = Arc::new(MockEnvironment::new());
        let tool = ReadFileTool::new(env);

        let result = tool.execute(r#"{"path": "/nonexistent.txt"}"#).await;

        assert!(result.is_error);
        assert!(result.content.contains("not found"));
    }

    #[tokio::test]
    async fn read_file_saturating_add_does_not_overflow() {
        // offset + limit would overflow usize if added naively; saturating_add must clamp.
        let env: Arc<dyn ExecutionEnvironment> =
            Arc::new(MockEnvironment::new().with_file("/test/small.txt", "line1\nline2\nline3"));
        let tool = ReadFileTool::new(env);

        // offset=1, limit=usize::MAX — the slice must be clamped to the file length, not panic.
        let args = format!(
            r#"{{"path": "/test/small.txt", "offset": 1, "limit": {}}}"#,
            usize::MAX
        );
        let result = tool.execute(&args).await;

        assert!(!result.is_error, "unexpected error: {}", result.content);
        assert!(result.content.contains("1 | line1"));
        assert!(result.content.contains("3 | line3"));
    }

    // -- WriteFileTool tests --

    #[tokio::test]
    async fn write_file_creates_file() {
        let env: Arc<dyn ExecutionEnvironment> = Arc::new(MockEnvironment::new());
        let tool = WriteFileTool::new(Arc::clone(&env));

        let result = tool
            .execute(r#"{"path": "/test/new.txt", "content": "new content"}"#)
            .await;

        assert!(!result.is_error);

        // Verify the file was written
        let content = env.read_file("/test/new.txt").await.unwrap();
        assert_eq!(content, "new content");
    }

    #[tokio::test]
    async fn write_file_overwrites_existing_file() {
        let env: Arc<dyn ExecutionEnvironment> =
            Arc::new(MockEnvironment::new().with_file("/test/existing.txt", "old content"));
        let tool = WriteFileTool::new(Arc::clone(&env));

        let result = tool
            .execute(r#"{"path": "/test/existing.txt", "content": "replaced"}"#)
            .await;

        assert!(!result.is_error);

        let content = env.read_file("/test/existing.txt").await.unwrap();
        assert_eq!(content, "replaced");
    }

    // -- EditFileTool tests --

    #[tokio::test]
    async fn edit_file_replaces_string() {
        let env: Arc<dyn ExecutionEnvironment> = Arc::new(
            MockEnvironment::new()
                .with_file("/test/code.rs", "fn main() {\n    println!(\"hello\");\n}"),
        );
        let tool = EditFileTool::new(Arc::clone(&env));

        let result = tool
            .execute(
                r#"{"path": "/test/code.rs", "old_string": "println!(\"hello\")", "new_string": "println!(\"world\")"}"#,
            )
            .await;

        assert!(!result.is_error, "unexpected error: {}", result.content);

        let content = env.read_file("/test/code.rs").await.unwrap();
        assert!(content.contains("println!(\"world\")"));
        assert!(!content.contains("println!(\"hello\")"));
    }

    #[tokio::test]
    async fn edit_file_returns_error_when_old_string_not_found() {
        let env: Arc<dyn ExecutionEnvironment> =
            Arc::new(MockEnvironment::new().with_file("/test/code.rs", "fn main() {}"));
        let tool = EditFileTool::new(env);

        let result = tool
            .execute(
                r#"{"path": "/test/code.rs", "old_string": "nonexistent text", "new_string": "replacement"}"#,
            )
            .await;

        assert!(result.is_error);
        assert!(result.content.contains("not found"));
    }

    #[tokio::test]
    async fn edit_file_returns_error_when_old_string_is_ambiguous() {
        let env: Arc<dyn ExecutionEnvironment> = Arc::new(
            MockEnvironment::new().with_file("/test/code.rs", "let x = 1;\nlet y = 1;\nlet z = 1;"),
        );
        let tool = EditFileTool::new(env);

        let result = tool
            .execute(r#"{"path": "/test/code.rs", "old_string": " = 1;", "new_string": " = 2;"}"#)
            .await;

        assert!(result.is_error);
        assert!(result.content.contains("ambiguous"));
    }

    // -- ShellTool tests --

    #[tokio::test]
    async fn shell_executes_command() {
        let env: Arc<dyn ExecutionEnvironment> = Arc::new(MockEnvironment::new());
        let tool = ShellTool::new(env);

        let result = tool.execute(r#"{"command": "echo hello"}"#).await;

        assert!(!result.is_error);
        assert!(result.content.contains("echo hello"));
    }

    // -- GrepTool tests --

    #[tokio::test]
    async fn grep_finds_matching_lines() {
        let env: Arc<dyn ExecutionEnvironment> = Arc::new(
            MockEnvironment::new()
                .with_file(
                    "/src/lib.rs",
                    "pub fn add(a: i32, b: i32) -> i32 {\n    a + b\n}",
                )
                .with_file("/src/main.rs", "fn main() {\n    let x = 42;\n}"),
        );
        let tool = GrepTool::new(env);

        let result = tool.execute(r#"{"pattern": "fn"}"#).await;

        assert!(!result.is_error);
        assert!(result.content.contains("fn main()"));
        assert!(result.content.contains("pub fn add"));
    }

    // -- GlobTool tests --

    #[tokio::test]
    async fn glob_finds_matching_files() {
        let env: Arc<dyn ExecutionEnvironment> = Arc::new(
            MockEnvironment::new()
                .with_file("/src/main.rs", "fn main() {}")
                .with_file("/src/lib.rs", "pub mod foo;")
                .with_file("/README.md", "# Hello"),
        );
        let tool = GlobTool::new(env);

        let result = tool.execute(r#"{"pattern": "*.rs"}"#).await;

        assert!(!result.is_error);
        assert!(result.content.contains("main.rs"));
        assert!(result.content.contains("lib.rs"));
        assert!(!result.content.contains("README.md"));
    }

    // -- register_shared_tools tests --

    #[tokio::test]
    async fn register_shared_tools_registers_all_six_tools() {
        let env: Arc<dyn ExecutionEnvironment> = Arc::new(MockEnvironment::new());
        let mut registry = ToolRegistry::new();

        register_shared_tools(&mut registry, env);

        assert_eq!(registry.len(), 6);
        assert!(registry.has_tool("read_file"));
        assert!(registry.has_tool("write_file"));
        assert!(registry.has_tool("edit_file"));
        assert!(registry.has_tool("shell"));
        assert!(registry.has_tool("grep"));
        assert!(registry.has_tool("glob_files"));
    }

    // -- Invalid JSON arguments tests --

    #[tokio::test]
    async fn invalid_json_arguments_return_error() {
        let env: Arc<dyn ExecutionEnvironment> = Arc::new(MockEnvironment::new());

        // ReadFileTool with missing path
        let tool = ReadFileTool::new(Arc::clone(&env));
        let result = tool.execute("{}").await;
        assert!(result.is_error);
        assert!(result.content.contains("path"));

        // WriteFileTool with missing content
        let tool = WriteFileTool::new(Arc::clone(&env));
        let result = tool.execute(r#"{"path": "/test.txt"}"#).await;
        assert!(result.is_error);
        assert!(result.content.contains("content"));

        // EditFileTool with missing old_string
        let tool = EditFileTool::new(Arc::clone(&env));
        let result = tool
            .execute(r#"{"path": "/test.txt", "new_string": "foo"}"#)
            .await;
        assert!(result.is_error);
        assert!(result.content.contains("old_string"));

        // ShellTool with missing command
        let tool = ShellTool::new(Arc::clone(&env));
        let result = tool.execute("{}").await;
        assert!(result.is_error);
        assert!(result.content.contains("command"));

        // GrepTool with missing pattern
        let tool = GrepTool::new(Arc::clone(&env));
        let result = tool.execute("{}").await;
        assert!(result.is_error);
        assert!(result.content.contains("pattern"));

        // GlobTool with missing pattern
        let tool = GlobTool::new(Arc::clone(&env));
        let result = tool.execute("{}").await;
        assert!(result.is_error);
        assert!(result.content.contains("pattern"));

        // Completely invalid JSON
        let tool = ReadFileTool::new(env);
        let result = tool.execute("not valid json").await;
        assert!(result.is_error);
        assert!(result.content.contains("Invalid JSON"));
    }
}
