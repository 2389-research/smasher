// ABOUTME: CLI specification documentation and integration tests that validate help output and exit codes.
// ABOUTME: Uses std::process::Command to invoke the compiled binary and assert expected behavior.

/// # CLI Specification
///
/// ## Binary
///
/// The `smasher` binary is the main entry point for the smasher CLI. It provides
/// three subcommands for AI workflow orchestration from the command line.
///
/// ## Global Flags
///
/// | Flag                    | Description                                      |
/// |-------------------------|--------------------------------------------------|
/// | `-v`, `--verbose`       | Enable verbose (debug-level) logging to stderr.   |
/// | `--env-file <PATH>`     | Load environment variables from a specific file.  |
/// | `-h`, `--help`          | Print help information.                           |
/// | `-V`, `--version`       | Print version information.                        |
///
/// ## Subcommands
///
/// ### `smasher complete`
///
/// Send a one-shot prompt to an LLM. Streams text deltas to stdout by default.
///
/// | Argument / Flag            | Description                                      |
/// |----------------------------|--------------------------------------------------|
/// | `<PROMPT>`                 | Positional prompt text. Omit if using `--file`.  |
/// | `--file <PATH>`            | Read the prompt from a file instead.             |
/// | `--model <MODEL>`          | Model identifier (default: `claude-sonnet-4-20250514`). |
/// | `--max-tokens <N>`         | Maximum tokens to generate.                      |
/// | `--temperature <FLOAT>`    | Sampling temperature (0.0 - 2.0).                |
/// | `--system <TEXT>`           | System prompt to prepend.                        |
/// | `--json`                   | Output the full Response as pretty-printed JSON. |
///
/// ### `smasher chat`
///
/// Start an interactive agent chat session with tool access.
///
/// | Argument / Flag            | Description                                      |
/// |----------------------------|--------------------------------------------------|
/// | `--model <MODEL>`          | Model identifier (default: `claude-sonnet-4-20250514`). |
/// | `--max-turns <N>`          | Maximum agentic turns (default: 100).            |
/// | `--system <TEXT>`           | System prompt override.                          |
/// | `--working-dir <PATH>`     | Working directory for tool operations.           |
///
/// ### `smasher run`
///
/// Execute a DOT-based pipeline.
///
/// | Argument / Flag            | Description                                      |
/// |----------------------------|--------------------------------------------------|
/// | `<PIPELINE>`               | Path to the DOT pipeline file (positional).      |
/// | `--var <KEY=VALUE>`        | Variable assignment, repeatable.                 |
/// | `--model <MODEL>`          | Model identifier (default: `claude-sonnet-4-20250514`). |
/// | `--max-steps <N>`          | Maximum pipeline steps (default: 1000).          |
/// | `--stylesheet <PATH>`      | Path to a stylesheet file for graph transforms.  |
/// | `--render <FILE>`          | Render the graph to a file before execution.     |
///
/// ### `smasher render`
///
/// Render a DOT pipeline file to SVG or PNG.
///
/// | Argument / Flag            | Description                                      |
/// |----------------------------|--------------------------------------------------|
/// | `<PIPELINE>`               | Path to the DOT pipeline file (positional).      |
/// | `-f`, `--format <FORMAT>`  | Output format: dot, svg, or png (default: svg).  |
/// | `-o`, `--output <FILE>`    | Output file path. Writes to stdout if omitted.   |
/// | `--var <KEY=VALUE>`        | Variable assignment, repeatable.                 |
/// | `--stylesheet <PATH>`      | Path to a stylesheet file for graph transforms.  |
///
/// ## Exit Codes
///
/// | Code | Meaning                                                  |
/// |------|----------------------------------------------------------|
/// | 0    | Success.                                                 |
/// | 1    | General / uncategorized error (`CliError::Other`).       |
/// | 2    | LLM provider error (`CliError::Llm`).                    |
/// | 3    | Agent session error (`CliError::Session`).               |
/// | 4    | Pipeline engine error (`CliError::Engine`).               |
/// | 5    | Parse / resolution / stylesheet error (`CliError::Resolution`, `CliError::DotParse`, `CliError::Stylesheet`). |
/// | 6    | I/O error (`CliError::Io`).                              |
///
/// ## Output Conventions
///
/// - **stdout**: primary output (streamed text, JSON responses, pipeline results).
/// - **stderr**: logging, diagnostics, tool call progress, and error messages.
/// - Verbose mode (`-v`) enables `debug`-level tracing to stderr.
/// - Without `-v`, only `warn`-level messages are shown.
/// - The `RUST_LOG` environment variable overrides the default filter.
#[cfg(test)]
mod tests {
    use std::process::Command;

    /// Locate the `smasher` binary next to the test executable.
    ///
    /// When `cargo test` compiles the binary crate, both the test runner and the
    /// actual binary end up in the same target directory (e.g.
    /// `target/debug/deps/`). We walk up from the test runner's path to the
    /// parent dir and look for `smasher` there or one level up in `target/debug/`.
    fn smasher_cmd() -> Command {
        let test_exe = std::env::current_exe().expect("cannot determine test executable path");
        // test_exe is something like target/debug/deps/smasher-<hash>
        // The binary is at target/debug/smasher
        let target_dir = test_exe
            .parent() // deps/
            .and_then(|p| p.parent()) // debug/
            .expect("cannot determine target directory");
        let bin_path = target_dir.join("smasher");
        assert!(
            bin_path.exists(),
            "smasher binary not found at {}",
            bin_path.display()
        );
        Command::new(bin_path)
    }

    // -----------------------------------------------------------------------
    // Top-level help and version
    // -----------------------------------------------------------------------

    #[test]
    fn help_exits_zero() {
        let output = smasher_cmd()
            .arg("--help")
            .output()
            .expect("failed to run smasher --help");

        assert!(output.status.success(), "smasher --help should exit 0");
    }

    #[test]
    fn help_contains_expected_sections() {
        let output = smasher_cmd()
            .arg("--help")
            .output()
            .expect("failed to run smasher --help");

        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains("complete") || stdout.contains("Complete"),
            "help should mention the complete subcommand: {stdout}"
        );
        assert!(
            stdout.contains("chat") || stdout.contains("Chat"),
            "help should mention the chat subcommand: {stdout}"
        );
        assert!(
            stdout.contains("run") || stdout.contains("Run"),
            "help should mention the run subcommand: {stdout}"
        );
        assert!(
            stdout.contains("render") || stdout.contains("Render"),
            "help should mention the render subcommand: {stdout}"
        );
        assert!(
            stdout.contains("--verbose") || stdout.contains("-v"),
            "help should mention --verbose flag: {stdout}"
        );
        assert!(
            stdout.contains("--env-file"),
            "help should mention --env-file flag: {stdout}"
        );
    }

    #[test]
    fn version_exits_zero() {
        let output = smasher_cmd()
            .arg("--version")
            .output()
            .expect("failed to run smasher --version");

        assert!(output.status.success(), "smasher --version should exit 0");
    }

    #[test]
    fn version_contains_version_string() {
        let output = smasher_cmd()
            .arg("--version")
            .output()
            .expect("failed to run smasher --version");

        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains(env!("CARGO_PKG_VERSION")),
            "version output should contain the crate version {}: got {stdout}",
            env!("CARGO_PKG_VERSION")
        );
    }

    // -----------------------------------------------------------------------
    // `smasher complete --help`
    // -----------------------------------------------------------------------

    #[test]
    fn complete_help_exits_zero() {
        let output = smasher_cmd()
            .args(["complete", "--help"])
            .output()
            .expect("failed to run smasher complete --help");

        assert!(
            output.status.success(),
            "smasher complete --help should exit 0"
        );
    }

    #[test]
    fn complete_help_contains_expected_flags() {
        let output = smasher_cmd()
            .args(["complete", "--help"])
            .output()
            .expect("failed to run smasher complete --help");

        let stdout = String::from_utf8_lossy(&output.stdout);

        let expected_flags = [
            "--model",
            "--json",
            "--system",
            "--temperature",
            "--max-tokens",
            "--file",
        ];
        for flag in &expected_flags {
            assert!(
                stdout.contains(flag),
                "complete --help should mention {flag}: {stdout}"
            );
        }
    }

    // -----------------------------------------------------------------------
    // `smasher chat --help`
    // -----------------------------------------------------------------------

    #[test]
    fn chat_help_exits_zero() {
        let output = smasher_cmd()
            .args(["chat", "--help"])
            .output()
            .expect("failed to run smasher chat --help");

        assert!(output.status.success(), "smasher chat --help should exit 0");
    }

    #[test]
    fn chat_help_contains_expected_flags() {
        let output = smasher_cmd()
            .args(["chat", "--help"])
            .output()
            .expect("failed to run smasher chat --help");

        let stdout = String::from_utf8_lossy(&output.stdout);

        let expected_flags = ["--model", "--max-turns", "--system"];
        for flag in &expected_flags {
            assert!(
                stdout.contains(flag),
                "chat --help should mention {flag}: {stdout}"
            );
        }
    }

    // -----------------------------------------------------------------------
    // `smasher render --help`
    // -----------------------------------------------------------------------

    #[test]
    fn render_help_exits_zero() {
        let output = smasher_cmd()
            .args(["render", "--help"])
            .output()
            .expect("failed to run smasher render --help");

        assert!(
            output.status.success(),
            "smasher render --help should exit 0"
        );
    }

    #[test]
    fn render_help_contains_expected_flags() {
        let output = smasher_cmd()
            .args(["render", "--help"])
            .output()
            .expect("failed to run smasher render --help");

        let stdout = String::from_utf8_lossy(&output.stdout);

        let expected_flags = ["--format", "--output", "--var", "--stylesheet"];
        for flag in &expected_flags {
            assert!(
                stdout.contains(flag),
                "render --help should mention {flag}: {stdout}"
            );
        }
    }

    // -----------------------------------------------------------------------
    // `smasher run --help`
    // -----------------------------------------------------------------------

    #[test]
    fn run_help_exits_zero() {
        let output = smasher_cmd()
            .args(["run", "--help"])
            .output()
            .expect("failed to run smasher run --help");

        assert!(output.status.success(), "smasher run --help should exit 0");
    }

    #[test]
    fn run_help_contains_expected_flags() {
        let output = smasher_cmd()
            .args(["run", "--help"])
            .output()
            .expect("failed to run smasher run --help");

        let stdout = String::from_utf8_lossy(&output.stdout);

        let expected_flags = [
            "--model",
            "--var",
            "--stylesheet",
            "--max-steps",
            "--render",
        ];
        for flag in &expected_flags {
            assert!(
                stdout.contains(flag),
                "run --help should mention {flag}: {stdout}"
            );
        }
    }

    // -----------------------------------------------------------------------
    // Exit code validation
    // -----------------------------------------------------------------------

    #[test]
    fn complete_no_prompt_exits_nonzero() {
        let output = smasher_cmd()
            .arg("complete")
            .output()
            .expect("failed to run smasher complete");

        assert!(
            !output.status.success(),
            "smasher complete with no prompt should exit non-zero"
        );
    }

    #[test]
    fn render_nonexistent_file_exits_six() {
        let output = smasher_cmd()
            .args([
                "render",
                "/nonexistent_pipeline_file_that_does_not_exist.dot",
            ])
            .output()
            .expect("failed to run smasher render /nonexistent.dot");

        assert!(
            !output.status.success(),
            "smasher render with nonexistent file should exit non-zero"
        );

        let code = output.status.code().expect("process should have exit code");
        assert_eq!(code, 6, "I/O error should map to exit code 6, got {code}");
    }

    #[test]
    fn run_nonexistent_file_exits_six() {
        let output = smasher_cmd()
            .args(["run", "/nonexistent_pipeline_file_that_does_not_exist.dot"])
            .output()
            .expect("failed to run smasher run /nonexistent.dot");

        assert!(
            !output.status.success(),
            "smasher run with nonexistent file should exit non-zero"
        );

        let code = output.status.code().expect("process should have exit code");
        assert_eq!(code, 6, "I/O error should map to exit code 6, got {code}");
    }

    // -----------------------------------------------------------------------
    // Exit code mapping from CliError (unit tests)
    // -----------------------------------------------------------------------

    #[test]
    fn exit_code_mapping_other() {
        use crate::error::CliError;
        let err = CliError::Other("something went wrong".into());
        assert_eq!(err.exit_code(), 1);
    }

    #[test]
    fn exit_code_mapping_io() {
        use crate::error::CliError;
        let err = CliError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "not found",
        ));
        assert_eq!(err.exit_code(), 6);
    }
}
