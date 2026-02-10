// ABOUTME: Applies unified diff patches to files, parsing hunks and patching line-by-line.
// ABOUTME: Used by coding agents that produce OpenAI-style unified diff output.

use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use serde_json::{Value, json};

use crate::environment::ExecutionEnvironment;
use crate::tools::{AgentTool, ToolOutput};

/// A tool that applies unified diff patches to files via an ExecutionEnvironment.
pub struct ApplyPatchTool {
    env: Arc<dyn ExecutionEnvironment>,
}

impl ApplyPatchTool {
    pub fn new(env: Arc<dyn ExecutionEnvironment>) -> Self {
        Self { env }
    }
}

#[async_trait]
impl AgentTool for ApplyPatchTool {
    fn name(&self) -> &str {
        "apply_patch"
    }

    fn description(&self) -> &str {
        "Apply a unified diff patch to one or more files"
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "patch": {
                    "type": "string",
                    "description": "Unified diff format patch"
                }
            },
            "required": ["patch"]
        })
    }

    async fn execute(&self, arguments: &str) -> ToolOutput {
        let start = Instant::now();
        let elapsed = || start.elapsed().as_millis() as u64;

        let args: Value = match serde_json::from_str(arguments) {
            Ok(v) => v,
            Err(e) => return ToolOutput::error(format!("Invalid JSON arguments: {e}"), 0),
        };

        let patch_str = match args.get("patch").and_then(|v| v.as_str()) {
            Some(p) => p,
            None => return ToolOutput::error("Missing required parameter: patch", 0),
        };

        let file_patches = match parse_patch(patch_str) {
            Ok(fp) => fp,
            Err(e) => return ToolOutput::error(format!("Failed to parse patch: {e}"), elapsed()),
        };

        if file_patches.is_empty() {
            return ToolOutput::error("No file patches found in patch input", elapsed());
        }

        let mut results = Vec::new();

        for fp in &file_patches {
            if fp.is_deleted {
                let path = &fp.original_path;
                match self.env.delete_file(path).await {
                    Ok(()) => results.push(format!("Deleted {path}")),
                    Err(e) => {
                        return ToolOutput::error(
                            format!("Failed to delete {path}: {e}"),
                            elapsed(),
                        );
                    }
                }
                continue;
            }

            let path = &fp.modified_path;

            let original = if fp.is_new_file {
                String::new()
            } else {
                match self.env.read_file(path).await {
                    Ok(c) => c,
                    Err(e) => {
                        return ToolOutput::error(format!("Failed to read {path}: {e}"), elapsed());
                    }
                }
            };

            let patched = match apply_file_patch(&original, fp) {
                Ok(c) => c,
                Err(e) => {
                    return ToolOutput::error(
                        format!("Failed to apply patch to {path}: {e}"),
                        elapsed(),
                    );
                }
            };

            match self.env.write_file(path, &patched).await {
                Ok(()) => {
                    if fp.is_new_file {
                        results.push(format!("Created {path}"));
                    } else {
                        results.push(format!("Patched {path}"));
                    }
                }
                Err(e) => {
                    return ToolOutput::error(format!("Failed to write {path}: {e}"), elapsed());
                }
            }
        }

        ToolOutput::success(results.join("\n"), elapsed())
    }
}

// ---------------------------------------------------------------------------
// Patch data types
// ---------------------------------------------------------------------------

/// A parsed patch for a single file, containing one or more hunks.
#[derive(Debug, Clone)]
pub struct FilePatch {
    pub original_path: String,
    pub modified_path: String,
    pub hunks: Vec<Hunk>,
    pub is_new_file: bool,
    pub is_deleted: bool,
}

/// A single hunk within a file patch.
#[derive(Debug, Clone)]
pub struct Hunk {
    pub original_start: usize,
    pub original_count: usize,
    pub modified_start: usize,
    pub modified_count: usize,
    pub lines: Vec<PatchLine>,
}

/// A single line within a hunk.
#[derive(Debug, Clone)]
pub enum PatchLine {
    Context(String),
    Added(String),
    Removed(String),
}

// ---------------------------------------------------------------------------
// Patch parser
// ---------------------------------------------------------------------------

/// Strip the `a/` or `b/` prefix that `git diff` adds to paths.
fn strip_ab_prefix(path: &str) -> String {
    if let Some(rest) = path.strip_prefix("a/").or_else(|| path.strip_prefix("b/")) {
        rest.to_string()
    } else {
        path.to_string()
    }
}

/// Parse a unified diff string into a list of per-file patches.
pub fn parse_patch(patch: &str) -> Result<Vec<FilePatch>, String> {
    let lines: Vec<&str> = patch.lines().collect();
    let mut file_patches = Vec::new();
    let mut i = 0;

    while i < lines.len() {
        // Look for the start of a file diff: a `---` line followed by a `+++` line.
        if lines[i].starts_with("--- ") && i + 1 < lines.len() && lines[i + 1].starts_with("+++ ") {
            let orig_raw = lines[i].trim_start_matches("--- ").trim();
            let mod_raw = lines[i + 1].trim_start_matches("+++ ").trim();

            let is_new_file = orig_raw == "/dev/null";
            let is_deleted = mod_raw == "/dev/null";

            let original_path = if is_new_file {
                String::new()
            } else {
                strip_ab_prefix(orig_raw)
            };

            let modified_path = if is_deleted {
                String::new()
            } else {
                strip_ab_prefix(mod_raw)
            };

            i += 2;

            // Collect hunks for this file.
            let mut hunks = Vec::new();

            while i < lines.len() {
                if lines[i].starts_with("@@ ") {
                    let hunk = parse_hunk_header(lines[i])?;
                    let mut hunk_lines = Vec::new();
                    i += 1;

                    while i < lines.len() {
                        let line = lines[i];
                        if line.starts_with("@@ ")
                            || line.starts_with("--- ")
                            || line.starts_with("diff ")
                        {
                            break;
                        }
                        if let Some(rest) = line.strip_prefix('+') {
                            hunk_lines.push(PatchLine::Added(rest.to_string()));
                        } else if let Some(rest) = line.strip_prefix('-') {
                            hunk_lines.push(PatchLine::Removed(rest.to_string()));
                        } else if let Some(rest) = line.strip_prefix(' ') {
                            hunk_lines.push(PatchLine::Context(rest.to_string()));
                        } else if line.starts_with('\\') {
                            // "\ No newline at end of file" -- skip.
                        } else {
                            // Treat bare lines as context (handles lines with no leading space).
                            hunk_lines.push(PatchLine::Context(line.to_string()));
                        }
                        i += 1;
                    }

                    hunks.push(Hunk {
                        original_start: hunk.0,
                        original_count: hunk.1,
                        modified_start: hunk.2,
                        modified_count: hunk.3,
                        lines: hunk_lines,
                    });
                } else if lines[i].starts_with("--- ") || lines[i].starts_with("diff ") {
                    // Start of a new file diff -- stop collecting hunks.
                    break;
                } else {
                    i += 1;
                }
            }

            if hunks.is_empty() && !is_new_file && !is_deleted {
                return Err("No hunks found for file patch".to_string());
            }

            file_patches.push(FilePatch {
                original_path,
                modified_path,
                hunks,
                is_new_file,
                is_deleted,
            });
        } else {
            i += 1;
        }
    }

    Ok(file_patches)
}

/// Parse a hunk header like `@@ -1,5 +1,6 @@` into (orig_start, orig_count, mod_start, mod_count).
fn parse_hunk_header(line: &str) -> Result<(usize, usize, usize, usize), String> {
    // Strip the leading "@@ " and trailing " @@..." parts.
    let at_parts: Vec<&str> = line.splitn(3, "@@").collect();
    if at_parts.len() < 3 {
        return Err(format!("Invalid hunk header: {line}"));
    }
    let range_part = at_parts[1].trim();
    let mut parts = range_part.split_whitespace();

    let orig_range = parts
        .next()
        .ok_or_else(|| format!("Missing original range in hunk header: {line}"))?;
    let mod_range = parts
        .next()
        .ok_or_else(|| format!("Missing modified range in hunk header: {line}"))?;

    let (orig_start, orig_count) = parse_range(orig_range.trim_start_matches('-'))?;
    let (mod_start, mod_count) = parse_range(mod_range.trim_start_matches('+'))?;

    Ok((orig_start, orig_count, mod_start, mod_count))
}

/// Parse a range like `1,5` or just `1` (which means count=1).
fn parse_range(s: &str) -> Result<(usize, usize), String> {
    if let Some((start_str, count_str)) = s.split_once(',') {
        let start = start_str
            .parse::<usize>()
            .map_err(|e| format!("Invalid range start '{start_str}': {e}"))?;
        let count = count_str
            .parse::<usize>()
            .map_err(|e| format!("Invalid range count '{count_str}': {e}"))?;
        Ok((start, count))
    } else {
        let start = s
            .parse::<usize>()
            .map_err(|e| format!("Invalid range '{s}': {e}"))?;
        Ok((start, 1))
    }
}

// ---------------------------------------------------------------------------
// Patch applier
// ---------------------------------------------------------------------------

/// Apply a parsed file patch to the original file content, producing the patched result.
pub fn apply_file_patch(original: &str, patch: &FilePatch) -> Result<String, String> {
    if patch.is_deleted {
        return Ok(String::new());
    }

    if patch.is_new_file {
        // Build content entirely from the Added lines in all hunks.
        let mut result = Vec::new();
        for hunk in &patch.hunks {
            for line in &hunk.lines {
                match line {
                    PatchLine::Added(s) | PatchLine::Context(s) => {
                        result.push(s.as_str());
                    }
                    PatchLine::Removed(_) => {}
                }
            }
        }
        return Ok(finish_lines(&result, original));
    }

    let orig_lines: Vec<&str> = original.lines().collect();
    let mut result: Vec<&str> = Vec::new();

    // `orig_idx` tracks our position in the original file (0-indexed).
    let mut orig_idx: usize = 0;

    for hunk in &patch.hunks {
        // Hunk positions are 1-indexed; convert to 0-indexed.
        let hunk_start = if hunk.original_start == 0 {
            0
        } else {
            hunk.original_start - 1
        };

        // Try exact position first, then fuzzy-search nearby.
        let actual_start = find_hunk_position(&orig_lines, hunk, hunk_start)?;

        // Copy any lines between the current position and the hunk start verbatim.
        if actual_start > orig_idx {
            result.extend_from_slice(&orig_lines[orig_idx..actual_start]);
        } else if actual_start < orig_idx {
            return Err(format!(
                "Overlapping hunks: hunk targets line {} but we are already at line {}",
                actual_start + 1,
                orig_idx + 1
            ));
        }

        let mut pos = actual_start;
        for line in &hunk.lines {
            match line {
                PatchLine::Context(s) => {
                    if pos >= orig_lines.len() {
                        return Err(format!(
                            "Context line '{}' goes past end of file at line {}",
                            s,
                            pos + 1
                        ));
                    }
                    if orig_lines[pos] != s.as_str() {
                        return Err(format!(
                            "Context mismatch at line {}: expected '{}', found '{}'",
                            pos + 1,
                            s,
                            orig_lines[pos]
                        ));
                    }
                    result.push(orig_lines[pos]);
                    pos += 1;
                }
                PatchLine::Removed(s) => {
                    if pos >= orig_lines.len() {
                        return Err(format!(
                            "Remove line '{}' goes past end of file at line {}",
                            s,
                            pos + 1
                        ));
                    }
                    if orig_lines[pos] != s.as_str() {
                        return Err(format!(
                            "Remove mismatch at line {}: expected '{}', found '{}'",
                            pos + 1,
                            s,
                            orig_lines[pos]
                        ));
                    }
                    // Skip this line (don't add to result).
                    pos += 1;
                }
                PatchLine::Added(s) => {
                    result.push(s.as_str());
                }
            }
        }

        orig_idx = pos;
    }

    // Append any remaining original lines after the last hunk.
    if orig_idx < orig_lines.len() {
        result.extend_from_slice(&orig_lines[orig_idx..]);
    }

    Ok(finish_lines(&result, original))
}

/// Rejoin lines, preserving the original file's trailing-newline behavior.
fn finish_lines(lines: &[&str], original: &str) -> String {
    if lines.is_empty() {
        return String::new();
    }
    let mut out = lines.join("\n");
    // If the original ended with a newline, preserve that.
    if original.ends_with('\n') || original.is_empty() {
        out.push('\n');
    }
    out
}

/// Try to find where a hunk should be applied in the original file.
/// First checks the exact expected position, then searches nearby.
fn find_hunk_position(
    orig_lines: &[&str],
    hunk: &Hunk,
    expected_start: usize,
) -> Result<usize, String> {
    // Collect the context/removed lines that must match in the original.
    let match_lines: Vec<&str> = hunk
        .lines
        .iter()
        .filter_map(|l| match l {
            PatchLine::Context(s) | PatchLine::Removed(s) => Some(s.as_str()),
            PatchLine::Added(_) => None,
        })
        .collect();

    if match_lines.is_empty() {
        // Pure addition hunk -- the expected start is fine.
        return Ok(expected_start);
    }

    // Try exact position first.
    if matches_at(orig_lines, expected_start, &match_lines) {
        return Ok(expected_start);
    }

    // Fuzzy search: scan a window of +/- 50 lines around the expected position.
    let max_drift = 50;
    for delta in 1..=max_drift {
        // Check below.
        if expected_start + delta + match_lines.len() <= orig_lines.len()
            && matches_at(orig_lines, expected_start + delta, &match_lines)
        {
            return Ok(expected_start + delta);
        }
        // Check above.
        if delta <= expected_start && matches_at(orig_lines, expected_start - delta, &match_lines) {
            return Ok(expected_start - delta);
        }
    }

    Err(format!(
        "Could not find hunk target near line {} (expected first line: '{}')",
        expected_start + 1,
        match_lines.first().unwrap_or(&"")
    ))
}

/// Check whether the context/removed lines of a hunk match `orig_lines` starting at `pos`.
fn matches_at(orig_lines: &[&str], pos: usize, match_lines: &[&str]) -> bool {
    if pos + match_lines.len() > orig_lines.len() {
        return false;
    }
    match_lines
        .iter()
        .enumerate()
        .all(|(i, expected)| orig_lines[pos + i] == *expected)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::environment::{EnvironmentError, ExecOptions, ExecResult, GrepMatch, GrepOptions};
    use std::collections::HashMap;
    use std::sync::Mutex;

    /// In-memory mock of ExecutionEnvironment for testing the tool without real I/O.
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

        fn read(&self, path: &str) -> Option<String> {
            self.files.lock().unwrap().get(path).cloned()
        }
    }

    #[async_trait]
    impl ExecutionEnvironment for MockEnvironment {
        async fn read_file(&self, path: &str) -> Result<String, EnvironmentError> {
            self.files
                .lock()
                .unwrap()
                .get(path)
                .cloned()
                .ok_or_else(|| EnvironmentError::FileNotFound {
                    path: path.to_string(),
                })
        }

        async fn write_file(&self, path: &str, content: &str) -> Result<(), EnvironmentError> {
            self.files
                .lock()
                .unwrap()
                .insert(path.to_string(), content.to_string());
            Ok(())
        }

        async fn glob_files(&self, _pattern: &str) -> Result<Vec<String>, EnvironmentError> {
            Ok(vec![])
        }

        async fn grep(
            &self,
            _pattern: &str,
            _path: &str,
            _options: GrepOptions,
        ) -> Result<Vec<GrepMatch>, EnvironmentError> {
            Ok(vec![])
        }

        async fn exec_command(
            &self,
            _command: &str,
            _options: ExecOptions,
        ) -> Result<ExecResult, EnvironmentError> {
            Ok(ExecResult {
                stdout: String::new(),
                stderr: String::new(),
                exit_code: 0,
                duration_ms: 0,
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
            self.files.lock().unwrap().contains_key(path)
        }

        async fn is_directory(&self, _path: &str) -> bool {
            false
        }
    }

    // -----------------------------------------------------------------------
    // Parser tests
    // -----------------------------------------------------------------------

    #[test]
    fn parse_simple_one_file_one_hunk() {
        let patch = "\
--- a/src/main.rs
+++ b/src/main.rs
@@ -1,3 +1,4 @@
 fn main() {
+    println!(\"hello\");
     let x = 1;
 }
";
        let result = parse_patch(patch).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].original_path, "src/main.rs");
        assert_eq!(result[0].modified_path, "src/main.rs");
        assert!(!result[0].is_new_file);
        assert!(!result[0].is_deleted);
        assert_eq!(result[0].hunks.len(), 1);
        assert_eq!(result[0].hunks[0].original_start, 1);
        assert_eq!(result[0].hunks[0].original_count, 3);
        assert_eq!(result[0].hunks[0].modified_start, 1);
        assert_eq!(result[0].hunks[0].modified_count, 4);
        assert_eq!(result[0].hunks[0].lines.len(), 4);
    }

    #[test]
    fn parse_patch_with_multiple_hunks() {
        let patch = "\
--- a/lib.rs
+++ b/lib.rs
@@ -1,3 +1,4 @@
 use std::io;
+use std::fs;

 fn foo() {}
@@ -10,3 +11,4 @@
 fn bar() {}
+fn baz() {}

 // end
";
        let result = parse_patch(patch).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].hunks.len(), 2);
        assert_eq!(result[0].hunks[0].original_start, 1);
        assert_eq!(result[0].hunks[1].original_start, 10);
    }

    #[test]
    fn parse_new_file_patch() {
        let patch = "\
--- /dev/null
+++ b/new_file.txt
@@ -0,0 +1,3 @@
+line one
+line two
+line three
";
        let result = parse_patch(patch).unwrap();
        assert_eq!(result.len(), 1);
        assert!(result[0].is_new_file);
        assert!(!result[0].is_deleted);
        assert_eq!(result[0].modified_path, "new_file.txt");
        assert_eq!(result[0].original_path, "");
    }

    #[test]
    fn parse_deleted_file_patch() {
        let patch = "\
--- a/old_file.txt
+++ /dev/null
@@ -1,3 +0,0 @@
-line one
-line two
-line three
";
        let result = parse_patch(patch).unwrap();
        assert_eq!(result.len(), 1);
        assert!(!result[0].is_new_file);
        assert!(result[0].is_deleted);
        assert_eq!(result[0].original_path, "old_file.txt");
        assert_eq!(result[0].modified_path, "");
    }

    #[test]
    fn parse_patch_with_multiple_files() {
        let patch = "\
--- a/file_a.rs
+++ b/file_a.rs
@@ -1,2 +1,3 @@
 fn a() {
+    // added
 }
--- a/file_b.rs
+++ b/file_b.rs
@@ -1,2 +1,3 @@
 fn b() {
+    // also added
 }
";
        let result = parse_patch(patch).unwrap();
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].modified_path, "file_a.rs");
        assert_eq!(result[1].modified_path, "file_b.rs");
    }

    // -----------------------------------------------------------------------
    // Applier tests
    // -----------------------------------------------------------------------

    #[test]
    fn apply_add_a_line() {
        let original = "fn main() {\n    let x = 1;\n}\n";
        let patch = parse_patch(
            "\
--- a/main.rs
+++ b/main.rs
@@ -1,3 +1,4 @@
 fn main() {
+    println!(\"hello\");
     let x = 1;
 }
",
        )
        .unwrap();

        let result = apply_file_patch(original, &patch[0]).unwrap();
        assert_eq!(
            result,
            "fn main() {\n    println!(\"hello\");\n    let x = 1;\n}\n"
        );
    }

    #[test]
    fn apply_remove_a_line() {
        let original = "fn main() {\n    println!(\"hello\");\n    let x = 1;\n}\n";
        let patch = parse_patch(
            "\
--- a/main.rs
+++ b/main.rs
@@ -1,4 +1,3 @@
 fn main() {
-    println!(\"hello\");
     let x = 1;
 }
",
        )
        .unwrap();

        let result = apply_file_patch(original, &patch[0]).unwrap();
        assert_eq!(result, "fn main() {\n    let x = 1;\n}\n");
    }

    #[test]
    fn apply_modify_a_line() {
        let original = "fn main() {\n    let x = 1;\n}\n";
        let patch = parse_patch(
            "\
--- a/main.rs
+++ b/main.rs
@@ -1,3 +1,3 @@
 fn main() {
-    let x = 1;
+    let x = 42;
 }
",
        )
        .unwrap();

        let result = apply_file_patch(original, &patch[0]).unwrap();
        assert_eq!(result, "fn main() {\n    let x = 42;\n}\n");
    }

    #[test]
    fn apply_with_context_lines() {
        let original = "a\nb\nc\nd\ne\n";
        let patch = parse_patch(
            "\
--- a/file.txt
+++ b/file.txt
@@ -1,5 +1,5 @@
 a
 b
-c
+C
 d
 e
",
        )
        .unwrap();

        let result = apply_file_patch(original, &patch[0]).unwrap();
        assert_eq!(result, "a\nb\nC\nd\ne\n");
    }

    #[test]
    fn apply_multi_hunk_patch() {
        let original = "line1\nline2\nline3\nline4\nline5\nline6\nline7\nline8\nline9\nline10\n";
        let patch = parse_patch(
            "\
--- a/file.txt
+++ b/file.txt
@@ -1,3 +1,4 @@
 line1
+inserted_after_1
 line2
 line3
@@ -8,3 +9,4 @@
 line8
+inserted_after_8
 line9
 line10
",
        )
        .unwrap();

        let result = apply_file_patch(original, &patch[0]).unwrap();
        let lines: Vec<&str> = result.lines().collect();
        assert_eq!(lines.len(), 12);
        assert_eq!(lines[0], "line1");
        assert_eq!(lines[1], "inserted_after_1");
        assert_eq!(lines[2], "line2");
        assert_eq!(lines[9], "inserted_after_8");
    }

    #[test]
    fn apply_new_file_creates_content() {
        let patch = parse_patch(
            "\
--- /dev/null
+++ b/brand_new.txt
@@ -0,0 +1,3 @@
+first line
+second line
+third line
",
        )
        .unwrap();

        let result = apply_file_patch("", &patch[0]).unwrap();
        assert_eq!(result, "first line\nsecond line\nthird line\n");
    }

    #[test]
    fn apply_deleted_file_produces_empty() {
        let patch = parse_patch(
            "\
--- a/gone.txt
+++ /dev/null
@@ -1,2 +0,0 @@
-bye
-bye
",
        )
        .unwrap();

        let result = apply_file_patch("bye\nbye\n", &patch[0]).unwrap();
        assert!(result.is_empty());
    }

    // -----------------------------------------------------------------------
    // AgentTool interface tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn agent_tool_execute_applies_patch() {
        let env: Arc<dyn ExecutionEnvironment> = Arc::new(
            MockEnvironment::new().with_file("src/main.rs", "fn main() {\n    let x = 1;\n}\n"),
        );
        let mock_ref = Arc::clone(&env);
        let tool = ApplyPatchTool::new(env);

        let patch_json = serde_json::json!({
            "patch": "--- a/src/main.rs\n+++ b/src/main.rs\n@@ -1,3 +1,4 @@\n fn main() {\n+    println!(\"hello\");\n     let x = 1;\n }\n"
        });

        let result = tool.execute(&patch_json.to_string()).await;
        assert!(!result.is_error, "unexpected error: {}", result.content);
        assert!(result.content.contains("Patched src/main.rs"));

        // Verify the file was written correctly.
        let content = mock_ref.read_file("src/main.rs").await.unwrap();
        assert!(content.contains("println!(\"hello\")"));
    }

    #[tokio::test]
    async fn agent_tool_invalid_patch_returns_error() {
        let env: Arc<dyn ExecutionEnvironment> = Arc::new(MockEnvironment::new());
        let tool = ApplyPatchTool::new(env);

        let patch_json = serde_json::json!({
            "patch": "this is not a valid patch"
        });

        let result = tool.execute(&patch_json.to_string()).await;
        assert!(result.is_error);
        assert!(
            result.content.contains("No file patches found"),
            "got: {}",
            result.content
        );
    }

    #[tokio::test]
    async fn agent_tool_missing_param_returns_error() {
        let env: Arc<dyn ExecutionEnvironment> = Arc::new(MockEnvironment::new());
        let tool = ApplyPatchTool::new(env);

        let result = tool.execute("{}").await;
        assert!(result.is_error);
        assert!(result.content.contains("patch"));
    }

    #[tokio::test]
    async fn agent_tool_new_file_creates_it() {
        let mock = Arc::new(MockEnvironment::new());
        let env: Arc<dyn ExecutionEnvironment> = Arc::clone(&mock) as Arc<dyn ExecutionEnvironment>;
        let tool = ApplyPatchTool::new(env);

        let patch_json = serde_json::json!({
            "patch": "--- /dev/null\n+++ b/hello.txt\n@@ -0,0 +1,2 @@\n+hello\n+world\n"
        });

        let result = tool.execute(&patch_json.to_string()).await;
        assert!(!result.is_error, "unexpected error: {}", result.content);
        assert!(result.content.contains("Created hello.txt"));

        let content = mock.read("hello.txt").unwrap();
        assert_eq!(content, "hello\nworld\n");
    }

    #[tokio::test]
    async fn agent_tool_delete_removes_file() {
        let mock =
            Arc::new(MockEnvironment::new().with_file("old_file.txt", "this will be deleted\n"));
        let env: Arc<dyn ExecutionEnvironment> = Arc::clone(&mock) as Arc<dyn ExecutionEnvironment>;
        let tool = ApplyPatchTool::new(env);

        // Verify the file exists before deletion.
        assert!(mock.read("old_file.txt").is_some());

        let patch_json = serde_json::json!({
            "patch": "--- a/old_file.txt\n+++ /dev/null\n@@ -1,1 +0,0 @@\n-this will be deleted\n"
        });

        let result = tool.execute(&patch_json.to_string()).await;
        assert!(!result.is_error, "unexpected error: {}", result.content);
        assert!(result.content.contains("Deleted old_file.txt"));

        // Verify the file has been removed, not just emptied.
        assert!(
            mock.read("old_file.txt").is_none(),
            "file should be removed, not just emptied"
        );
    }

    #[tokio::test]
    async fn agent_tool_name_and_schema() {
        let env: Arc<dyn ExecutionEnvironment> = Arc::new(MockEnvironment::new());
        let tool = ApplyPatchTool::new(env);

        assert_eq!(tool.name(), "apply_patch");
        assert_eq!(
            tool.description(),
            "Apply a unified diff patch to one or more files"
        );

        let schema = tool.parameters_schema();
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["patch"].is_object());
        assert_eq!(schema["required"], json!(["patch"]));
    }
}
