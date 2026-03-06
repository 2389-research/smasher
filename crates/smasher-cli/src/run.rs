// ABOUTME: DOT pipeline execution subcommand that parses, resolves, and runs graph workflows.
// ABOUTME: Supports variables, stylesheets, step limits, and outputs final context as JSON.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use clap::Args;
use smasher_agent::environment::LocalExecutionEnvironment;
use smasher_agent::events::EventEmitter;
use smasher_agent::session::Session;
use smasher_agent::tools::ToolRegistry;
use smasher_agent::tools::shared::register_shared_tools;
use smasher_agent::types::SessionConfig;

use smasher_attractor::artifact::ArtifactStore;
use smasher_attractor::dot::parser;
use smasher_attractor::engine::{Engine, EngineConfig};
use smasher_attractor::events::{PipelineEvent, PipelineEventEmitter};
use smasher_attractor::graph;
use smasher_attractor::handler::{
    CodergenBackend, CodergenHandler, HandlerError, default_registry,
};
use smasher_attractor::interviewer::{
    AutoApproveInterviewer, ConsoleInterviewer, HumanGateHandler, Interviewer, InterviewerHandler,
    TimeoutInterviewer,
};
use smasher_attractor::manager_handler::ManagerHandler;
use smasher_attractor::parallel::ParallelHandler;
use smasher_attractor::rendering::{CachedRenderer, GraphRenderer, GraphvizRenderer, RenderFormat};
use smasher_attractor::state::Context;
use smasher_attractor::state::Outcome;
use smasher_attractor::stylesheet::Stylesheet;
use smasher_attractor::tool_handler::ToolHandler;
use smasher_attractor::transforms;

use std::io::IsTerminal;

use crate::error::CliError;
use crate::gitutil;
use crate::tui::{Msg as TuiMsg, TuiFlags};

/// CodergenBackend that runs a full agent session with file/shell tools.
///
/// Each codergen node invocation creates a fresh agent Session with all six
/// shared tools (read_file, write_file, edit_file, shell, grep, glob_files),
/// sends the node prompt as user input, and lets the LLM drive tool use until
/// the task is complete.
struct AgentCodergenBackend {
    client: Arc<smasher_llm::client::Client>,
    default_model: String,
    working_dir: String,
}

impl AgentCodergenBackend {
    fn new(
        client: Arc<smasher_llm::client::Client>,
        default_model: String,
        working_dir: String,
    ) -> Self {
        Self {
            client,
            default_model,
            working_dir,
        }
    }
}

#[async_trait::async_trait]
impl CodergenBackend for AgentCodergenBackend {
    async fn generate(
        &self,
        prompt: &str,
        model: Option<&str>,
        context: &Context,
    ) -> Result<Outcome, HandlerError> {
        let model_id = model.unwrap_or(&self.default_model);

        // Build context summary from pipeline state for the system prompt.
        let context_summary = context.to_string_map();
        let system_parts: Vec<String> = context_summary
            .iter()
            .filter(|(k, _)| !k.starts_with('_'))
            .map(|(k, v)| format!("{k}: {v}"))
            .collect();

        let system_prompt = if system_parts.is_empty() {
            "You are an AI coding assistant executing a pipeline step. You have tools for reading files, writing files, editing files, running shell commands, grep, and glob. Use them to complete the task. Write all files in the current working directory.".to_string()
        } else {
            format!(
                "You are an AI coding assistant executing a pipeline step. Pipeline context:\n{}\n\nYou have tools for reading files, writing files, editing files, running shell commands, grep, and glob. Use them to complete the task. Write all files in the current working directory.",
                system_parts.join("\n")
            )
        };

        // Create a fresh agent session with all shared tools.
        let env = Arc::new(LocalExecutionEnvironment::new(self.working_dir.clone()));
        let mut tool_registry = ToolRegistry::new();
        register_shared_tools(&mut tool_registry, env);

        let emitter = EventEmitter::default();
        let mut rx = emitter.subscribe();

        let config = SessionConfig::default()
            .with_model(model_id)
            .with_max_turns(50)
            .with_system_prompt(&system_prompt)
            .with_working_directory(&self.working_dir);

        // Spawn event listener for tool call logging to stderr.
        tokio::spawn(async move {
            use smasher_agent::types::SessionEvent;
            loop {
                match rx.recv().await {
                    Ok(SessionEvent::ToolCallStarted { tool_name, .. }) => {
                        eprintln!("  [tool] {tool_name}...");
                    }
                    Ok(SessionEvent::ToolCallCompleted {
                        tool_name,
                        is_error,
                        duration_ms,
                        ..
                    }) => {
                        let status = if is_error { "ERR" } else { "ok" };
                        eprintln!("  [tool] {tool_name} {status} ({duration_ms}ms)");
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        eprintln!("  [warn] missed {n} events");
                    }
                    _ => {}
                }
            }
        });

        let mut session = Session::new(config, Arc::clone(&self.client), tool_registry, emitter);

        match session.process_input(prompt).await {
            Ok(output) => {
                let text = output.text.unwrap_or_default();

                tracing::info!(
                    model = model_id,
                    turns = output.turns_used,
                    input_tokens = output.total_usage.input_tokens,
                    output_tokens = output.total_usage.output_tokens,
                    "codergen node completed"
                );

                Ok(Outcome::success_with(serde_json::json!({"response": text})))
            }
            Err(e) => Err(HandlerError::Other(format!("Agent session error: {e}"))),
        }
    }
}

/// CodergenBackend that spawns `claude` CLI as a subprocess.
///
/// Builds a combined prompt from pipeline context and the node prompt, then
/// runs `claude --dangerously-skip-permissions --print -p <prompt>` with a
/// wall-clock timeout. Captures stdout as the outcome text.
struct ClaudeCliBackend {
    working_dir: String,
    timeout: Duration,
    /// Override the path to the `claude` binary (for testing).
    claude_path: Option<String>,
    /// When true, use `--output-format stream-json` and parse NDJSON events line by line.
    streaming: bool,
    /// Optional emitter for forwarding parsed NDJSON events to the TUI pipeline bridge.
    emitter: Option<Arc<PipelineEventEmitter>>,
}

#[async_trait::async_trait]
impl CodergenBackend for ClaudeCliBackend {
    async fn generate(
        &self,
        prompt: &str,
        // The model parameter is intentionally not forwarded to the claude CLI —
        // the CLI uses its own model selection. Per-node model overrides only
        // apply to the agent backend.
        _model: Option<&str>,
        context: &Context,
    ) -> Result<Outcome, HandlerError> {
        // Build context summary from pipeline state, same logic as AgentCodergenBackend.
        let context_summary = context.to_string_map();
        let system_parts: Vec<String> = context_summary
            .iter()
            .filter(|(k, _)| !k.starts_with('_'))
            .map(|(k, v)| format!("{k}: {v}"))
            .collect();

        let combined_prompt = if system_parts.is_empty() {
            prompt.to_string()
        } else {
            format!(
                "Pipeline context:\n{}\n\n{}",
                system_parts.join("\n"),
                prompt
            )
        };

        let claude_bin = self.claude_path.as_deref().unwrap_or("claude");

        let mut cmd = tokio::process::Command::new(claude_bin);
        cmd.arg("--dangerously-skip-permissions");

        if self.streaming {
            // Stream JSON mode: read NDJSON events line-by-line and emit PipelineEvents.
            // The claude CLI requires --verbose when using --output-format=stream-json.
            cmd.arg("--verbose")
                .arg("--output-format")
                .arg("stream-json");
        } else {
            // Print mode: collect all output at the end.
            cmd.arg("--print");
        }

        cmd.arg("-p")
            .arg(&combined_prompt)
            .current_dir(&self.working_dir)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        // Strip env vars that cause the inner claude process to detect a nested
        // session and refuse to launch. The outer Claude Code session sets these.
        cmd.env_remove("CLAUDE_CODE_ENTRYPOINT");
        cmd.env_remove("CLAUDECODE");
        cmd.env_remove("CLAUDE_CODE_DISABLE_FEEDBACK_SURVEY");
        cmd.env_remove("CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS");
        cmd.env_remove("CLAUDE_CODE_SESSION");

        let mut child = cmd
            .spawn()
            .map_err(|e| HandlerError::Other(format!("failed to spawn claude CLI: {e}")))?;

        if self.streaming {
            // Streaming path: read stdout line-by-line as NDJSON and emit PipelineEvents.
            // stderr is consumed in the background to prevent pipe buffer deadlock.
            let emitter = self.emitter.clone();
            let node_id = context.get_string("_current_node_id").unwrap_or_default();

            let stdout = child
                .stdout
                .take()
                .ok_or_else(|| HandlerError::Other("failed to get stdout handle".to_string()))?;
            let stderr_handle = child.stderr.take();

            let stderr_task = tokio::spawn(async move {
                let mut buf = Vec::new();
                if let Some(mut err) = stderr_handle {
                    tokio::io::AsyncReadExt::read_to_end(&mut err, &mut buf).await?;
                }
                Ok::<Vec<u8>, std::io::Error>(buf)
            });

            // Spawn NDJSON parsing as a task so it can run concurrently with child.wait().
            let stdout_task = tokio::spawn(async move {
                use tokio::io::AsyncBufReadExt;
                let reader = tokio::io::BufReader::new(stdout);
                let mut lines = reader.lines();
                let mut result_text: Option<String> = None;

                while let Some(line) = lines.next_line().await? {
                    let line = line.trim().to_string();
                    if line.is_empty() {
                        continue;
                    }
                    let Ok(obj) = serde_json::from_str::<serde_json::Value>(&line) else {
                        continue;
                    };
                    match obj.get("type").and_then(|v| v.as_str()) {
                        Some("assistant") => {
                            let Some(content) = obj["message"]["content"].as_array() else {
                                continue;
                            };
                            for block in content {
                                match block.get("type").and_then(|v| v.as_str()) {
                                    Some("tool_use") => {
                                        let tool_name =
                                            block["name"].as_str().unwrap_or("").to_string();
                                        let tool_call_id =
                                            block["id"].as_str().unwrap_or("").to_string();
                                        let input_preview =
                                            smasher_agent::session::tool_input_preview_from_value(
                                                &tool_name,
                                                &block["input"],
                                            );
                                        if let Some(ref emitter) = emitter {
                                            emitter.emit(PipelineEvent::AgentToolCallStarted {
                                                node_id: node_id.clone(),
                                                tool_name,
                                                tool_call_id,
                                                input_preview,
                                                timestamp: chrono::Utc::now(),
                                            });
                                        }
                                    }
                                    Some("text") => {
                                        let text = block["text"].as_str().unwrap_or("").to_string();
                                        if let Some(ref emitter) = emitter {
                                            emitter.emit(PipelineEvent::AgentMessage {
                                                node_id: node_id.clone(),
                                                text,
                                                timestamp: chrono::Utc::now(),
                                            });
                                        }
                                    }
                                    _ => {}
                                }
                            }
                        }
                        Some("result") => {
                            result_text = Some(obj["result"].as_str().unwrap_or("").to_string());
                            // Emit token usage if available
                            if let Some(ref emitter) = emitter {
                                let input_tokens =
                                    obj["usage"]["input_tokens"].as_u64().unwrap_or(0);
                                let output_tokens =
                                    obj["usage"]["output_tokens"].as_u64().unwrap_or(0);
                                if input_tokens > 0 || output_tokens > 0 {
                                    emitter.emit(PipelineEvent::AgentTokenUsage {
                                        node_id: node_id.clone(),
                                        input_tokens,
                                        output_tokens,
                                        timestamp: chrono::Utc::now(),
                                    });
                                }
                            }
                        }
                        _ => {}
                    }
                }

                Ok::<Option<String>, std::io::Error>(result_text)
            });

            let (status, result_text, stderr) = match tokio::time::timeout(self.timeout, async {
                let status = child.wait();
                let stdout_join = async { stdout_task.await.map_err(std::io::Error::other)? };
                let stderr_join = async { stderr_task.await.map_err(std::io::Error::other)? };
                let (status, result_text, stderr) =
                    tokio::try_join!(status, stdout_join, stderr_join)?;
                Ok::<(std::process::ExitStatus, Option<String>, Vec<u8>), std::io::Error>((
                    status,
                    result_text,
                    stderr,
                ))
            })
            .await
            {
                Ok(Ok(result)) => result,
                Ok(Err(e)) => {
                    return Err(HandlerError::Other(format!(
                        "failed to run claude CLI: {e}"
                    )));
                }
                Err(_) => {
                    let _ = child.kill().await;
                    return Err(HandlerError::Other(format!(
                        "claude CLI timed out after {}s",
                        self.timeout.as_secs()
                    )));
                }
            };

            if !status.success() {
                let stderr = String::from_utf8_lossy(&stderr);
                let code = status
                    .code()
                    .map(|c| c.to_string())
                    .unwrap_or_else(|| "unknown".to_string());
                return Err(HandlerError::Other(format!(
                    "claude CLI exit code {code}: {stderr}"
                )));
            }

            let text = result_text.unwrap_or_default();
            return Ok(Outcome::success_with(serde_json::json!({"response": text})));
        }

        // Non-streaming path: collect all stdout at the end.

        // Take stdout/stderr handles before waiting so we can read them
        // concurrently with the child process. Reading must happen in parallel
        // with wait(): if the child produces more output than the OS pipe
        // buffer (64KB on Linux, 16KB on macOS), the child blocks on write
        // and wait() never returns, causing a deadlock.
        let stdout_handle = child.stdout.take();
        let stderr_handle = child.stderr.take();

        let stdout_task = tokio::spawn(async move {
            let mut buf = Vec::new();
            if let Some(mut out) = stdout_handle {
                tokio::io::AsyncReadExt::read_to_end(&mut out, &mut buf).await?;
            }
            Ok::<Vec<u8>, std::io::Error>(buf)
        });

        let stderr_task = tokio::spawn(async move {
            let mut buf = Vec::new();
            if let Some(mut err) = stderr_handle {
                tokio::io::AsyncReadExt::read_to_end(&mut err, &mut buf).await?;
            }
            Ok::<Vec<u8>, std::io::Error>(buf)
        });

        let output = match tokio::time::timeout(self.timeout, async {
            let status = child.wait();
            let stdout_join = async { stdout_task.await.map_err(std::io::Error::other)? };
            let stderr_join = async { stderr_task.await.map_err(std::io::Error::other)? };
            let (status, stdout, stderr) = tokio::try_join!(status, stdout_join, stderr_join)?;
            Ok::<std::process::Output, std::io::Error>(std::process::Output {
                status,
                stdout,
                stderr,
            })
        })
        .await
        {
            Ok(Ok(output)) => output,
            Ok(Err(e)) => {
                return Err(HandlerError::Other(format!(
                    "failed to run claude CLI: {e}"
                )));
            }
            Err(_) => {
                let _ = child.kill().await;
                return Err(HandlerError::Other(format!(
                    "claude CLI timed out after {}s",
                    self.timeout.as_secs()
                )));
            }
        };

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let code = output
                .status
                .code()
                .map(|c| c.to_string())
                .unwrap_or_else(|| "unknown".to_string());
            return Err(HandlerError::Other(format!(
                "claude CLI exit code {code}: {stderr}"
            )));
        }

        let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
        Ok(Outcome::success_with(serde_json::json!({"response": text})))
    }
}

/// CodergenBackend that runs the node prompt as a shell command via `sh -c`.
///
/// Designed for deterministic pipeline testing without an LLM. The `prompt`
/// attribute is executed directly as a shell script. Exit code 0 means success
/// (stdout captured), non-zero means failure (stderr captured).
///
/// Pipeline context entries (excluding underscore-prefixed internals) are
/// exposed as `SMASHER_CTX_<key>` environment variables.
struct ShellCodergenBackend {
    working_dir: String,
    timeout: Duration,
}

#[async_trait::async_trait]
impl CodergenBackend for ShellCodergenBackend {
    async fn generate(
        &self,
        prompt: &str,
        _model: Option<&str>,
        context: &Context,
    ) -> Result<Outcome, HandlerError> {
        let mut cmd = tokio::process::Command::new("sh");
        cmd.arg("-c")
            .arg(prompt)
            .current_dir(&self.working_dir)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        // Run in its own process group so timeout can kill the entire subtree.
        #[cfg(unix)]
        unsafe {
            cmd.pre_exec(|| {
                libc::setpgid(0, 0);
                Ok(())
            });
        }

        // Clear inherited env and whitelist only essentials for determinism.
        // Pipeline context is exposed as SMASHER_CTX_* variables.
        cmd.env_clear();
        for var in ["PATH", "HOME", "USER", "LANG", "TERM", "TMPDIR", "SHELL"] {
            if let Ok(val) = std::env::var(var) {
                cmd.env(var, val);
            }
        }
        let context_snapshot = context.to_string_map();
        for (key, value) in &context_snapshot {
            if !key.starts_with('_') {
                cmd.env(format!("SMASHER_CTX_{key}"), value);
            }
        }

        let mut child = cmd
            .spawn()
            .map_err(|e| HandlerError::Other(format!("failed to spawn shell: {e}")))?;

        // Read stdout and stderr concurrently to avoid pipe buffer deadlock.
        let stdout_handle = child.stdout.take();
        let stderr_handle = child.stderr.take();

        let stdout_task = tokio::spawn(async move {
            let mut buf = Vec::new();
            if let Some(mut out) = stdout_handle {
                tokio::io::AsyncReadExt::read_to_end(&mut out, &mut buf).await?;
            }
            Ok::<Vec<u8>, std::io::Error>(buf)
        });

        let stderr_task = tokio::spawn(async move {
            let mut buf = Vec::new();
            if let Some(mut err) = stderr_handle {
                tokio::io::AsyncReadExt::read_to_end(&mut err, &mut buf).await?;
            }
            Ok::<Vec<u8>, std::io::Error>(buf)
        });

        let (status, stdout_bytes, stderr_bytes) = match tokio::time::timeout(self.timeout, async {
            let status = child.wait().await?;
            let stdout = stdout_task.await.map_err(std::io::Error::other)??;
            let stderr = stderr_task.await.map_err(std::io::Error::other)??;
            Ok::<_, std::io::Error>((status, stdout, stderr))
        })
        .await
        {
            Ok(Ok(result)) => result,
            Ok(Err(e)) => {
                return Err(HandlerError::Other(format!("shell command failed: {e}")));
            }
            Err(_) => {
                // Kill the entire process group so child/grandchild processes are reaped.
                #[cfg(unix)]
                if let Some(id) = child.id() {
                    unsafe {
                        libc::killpg(id as libc::pid_t, libc::SIGKILL);
                    }
                }
                let _ = child.kill().await;
                return Err(HandlerError::Other(format!(
                    "shell command timed out after {}s",
                    self.timeout.as_secs()
                )));
            }
        };

        let stdout = String::from_utf8_lossy(&stdout_bytes).to_string();
        let stderr = String::from_utf8_lossy(&stderr_bytes).to_string();
        let exit_code = status.code().unwrap_or(-1);

        if status.success() {
            Ok(Outcome::success_with(serde_json::json!({
                "stdout": stdout,
                "stderr": stderr,
                "exit_code": exit_code,
            })))
        } else {
            let error_msg = if stderr.trim().is_empty() {
                format!("shell command exited with code {exit_code}")
            } else {
                format!("exit code {exit_code}: {}", stderr.trim())
            };
            Ok(Outcome::failure(error_msg))
        }
    }
}

/// Execute a DOT-based pipeline.
#[derive(Debug, Args)]
pub struct RunArgs {
    /// Path to the DOT pipeline file.
    #[arg()]
    pub pipeline: String,

    /// Variable assignments (key=value), repeatable.
    #[arg(long = "var", value_name = "KEY=VALUE")]
    pub vars: Vec<String>,

    /// Model identifier for codergen nodes.
    #[arg(long, default_value = "claude-sonnet-4-20250514")]
    pub model: String,

    /// Maximum pipeline steps before forced stop.
    #[arg(long, default_value = "1000")]
    pub max_steps: usize,

    /// Path to a stylesheet file for graph transforms.
    #[arg(long)]
    pub stylesheet: Option<String>,

    /// Render the resolved graph to a file before execution. Format is inferred
    /// from the file extension (.dot, .svg, .png), or defaults to SVG.
    #[arg(long, value_name = "FILE")]
    pub render: Option<String>,

    /// Run in an isolated git worktree branch. Creates a fresh branch and
    /// working directory for the pipeline, commits results when done.
    #[arg(long)]
    pub worktree: bool,

    /// Allow running with a dirty working tree (only meaningful with --worktree).
    #[arg(long)]
    pub allow_dirty: bool,

    /// Skip preflight health-checks of LLM providers before execution.
    #[arg(long)]
    pub skip_preflight: bool,

    /// Skip lint pre-check before pipeline execution.
    #[arg(long)]
    pub skip_lint: bool,

    /// Backend for codergen nodes: "claude-cli" (default), "agent", or "shell".
    #[arg(long, default_value = "claude-cli")]
    pub backend: String,

    /// Wall-clock timeout in seconds for codergen backends (claude-cli, shell). Default: 600.
    #[arg(long, default_value = "600")]
    pub agent_timeout: u64,

    /// Interviewer mode for human-in-the-loop nodes: auto, console, or timeout:N
    /// (default: auto if no tty, console if tty).
    #[arg(long, default_value = "auto")]
    pub interviewer: String,

    /// Enable the TUI dashboard (default when stderr is a terminal).
    #[arg(long)]
    pub tui: bool,

    /// Disable the TUI dashboard.
    #[arg(long)]
    pub no_tui: bool,

    /// Resume the latest incomplete run of this pipeline instead of starting fresh.
    #[arg(long)]
    pub resume: bool,
}

fn should_enable_tui(args: &RunArgs) -> bool {
    if args.no_tui {
        return false;
    }
    if args.tui {
        return true;
    }
    // Default: enable when stderr is an interactive terminal.
    std::io::stderr().is_terminal()
}

pub async fn run(args: RunArgs) -> Result<(), CliError> {
    let tui_enabled = should_enable_tui(&args);
    let dot_source = std::fs::read_to_string(&args.pipeline)?;
    let dot_graph = parser::parse(&dot_source)?;
    let mut resolved = graph::resolve(&dot_graph)?;

    // Parse variables from --var key=value flags.
    let mut variables: HashMap<String, String> = HashMap::new();
    for var_str in &args.vars {
        let (key, value) = var_str.split_once('=').ok_or_else(|| {
            CliError::Other(format!(
                "invalid --var format '{}': expected KEY=VALUE",
                var_str
            ))
        })?;
        variables.insert(key.to_string(), value.to_string());
    }
    // Inject the model as a variable so codergen nodes can use it.
    variables.insert("model".to_string(), args.model.clone());

    // Optionally load and apply a stylesheet.
    let stylesheet = match &args.stylesheet {
        Some(path) => {
            let css_source = std::fs::read_to_string(path)?;
            Some(Stylesheet::parse(&css_source)?)
        }
        None => None,
    };

    transforms::apply_transforms(&mut resolved, &variables, stylesheet.as_ref());

    // Lint pre-check: catch structural issues before execution.
    if !args.skip_lint {
        crate::lint::lint_graph(&resolved)?;
    }

    // Optionally render the graph before execution.
    if let Some(ref render_path) = args.render {
        let format = infer_render_format(render_path);
        let renderer = CachedRenderer::new(GraphvizRenderer);
        let output = renderer
            .render(&resolved, format)
            .await
            .map_err(|e| CliError::Other(format!("graph render failed: {e}")))?;
        std::fs::write(render_path, &output.content)?;
        tracing::info!(format = %format, path = %render_path, "graph rendered to file");
    }

    // Shell backend does not need an LLM client or preflight checks.
    let needs_llm = args.backend != "shell";

    let client: Option<Arc<smasher_llm::client::Client>> = if needs_llm {
        let c = smasher_llm::client::Client::from_env();
        if c.registered_providers().is_empty() {
            return Err(CliError::Other(
                "no API keys found. Set ANTHROPIC_API_KEY, OPENAI_API_KEY, or GEMINI_API_KEY."
                    .into(),
            ));
        }
        Some(Arc::new(c))
    } else {
        None
    };

    // Run preflight health-checks unless explicitly skipped or backend is shell.
    if needs_llm && !args.skip_preflight {
        let client_ref = client.as_ref().unwrap();
        match smasher_attractor::preflight::preflight_check(&resolved, client_ref).await {
            Ok(report) => {
                for probe in &report.probes {
                    eprintln!(
                        "  preflight {}/{}: ok ({}ms)",
                        probe.provider, probe.model, probe.latency_ms
                    );
                }
            }
            Err(smasher_attractor::preflight::PreflightError::ProviderUnreachable {
                report,
                ..
            }) => {
                for probe in &report.probes {
                    if probe.passed {
                        eprintln!(
                            "  preflight {}/{}: ok ({}ms)",
                            probe.provider, probe.model, probe.latency_ms
                        );
                    } else {
                        eprintln!(
                            "  preflight {}/{}: FAILED - {}",
                            probe.provider,
                            probe.model,
                            probe.error.as_deref().unwrap_or("unknown error")
                        );
                    }
                }
                return Err(CliError::Other(format!(
                    "preflight check failed: {} provider(s) unreachable. Use --skip-preflight to bypass.",
                    report.failure_count()
                )));
            }
        }
    }

    let working_dir = std::env::current_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| ".".to_string());

    let artifacts_base = std::path::Path::new(&working_dir).join("artifacts");
    let hash = graph_hash(&dot_source);

    // Check for a previous incomplete run that can be resumed.
    if let Some(incomplete) = find_latest_incomplete_run(&artifacts_base, &hash) {
        let should_resume = if args.resume {
            true
        } else if std::io::stderr().is_terminal() {
            eprintln!(
                "Found incomplete run {} (started {})",
                incomplete.run_id, incomplete.created_at
            );
            eprint!("Resume from checkpoint? [Y/n] ");
            let mut answer = String::new();
            std::io::stdin().read_line(&mut answer).unwrap_or(0);
            let answer = answer.trim().to_lowercase();
            answer.is_empty() || answer == "y" || answer == "yes"
        } else {
            false
        };

        if should_resume {
            eprintln!("Resuming run {}", incomplete.run_id);
            // Ensure graph.dot exists in the run directory (older runs may lack it).
            let graph_dot = incomplete.run_dir.join("graph.dot");
            if !graph_dot.exists() {
                std::fs::write(&graph_dot, &dot_source)?;
            }
            let resume_args = crate::resume::ResumeArgs {
                run_dir: Some(incomplete.run_dir.display().to_string()),
                checkpoint: None,
                model: args.model.clone(),
                max_steps: args.max_steps,
                skip_preflight: args.skip_preflight,
            };
            return crate::resume::run(resume_args).await;
        }
    } else if args.resume {
        return Err(CliError::Other(
            "no incomplete run found for this pipeline. Starting fresh.".into(),
        ));
    }

    // Create per-run artifact directory for isolation.
    let run_id = generate_run_id();
    let graph_name =
        smasher_attractor::run_dir::sanitize_graph_name(&resolved.name.clone().unwrap_or_default());
    let run_directory = smasher_attractor::run_dir::RunDirectory::create(
        &artifacts_base,
        &run_id,
        &graph_name,
        &dot_source,
    )?;

    // Write graph.dot to the run directory so `smasher resume` can find it.
    let graph_dot_path = run_directory.manifest().directories.root.join("graph.dot");
    std::fs::write(&graph_dot_path, &dot_source)?;
    let run_working_dir = run_directory
        .manifest()
        .directories
        .root
        .display()
        .to_string();
    eprintln!("Run directory: {run_working_dir}");

    // Optionally set up git worktree isolation for the pipeline run.
    let effective_working_dir = if args.worktree {
        let repo_path = std::env::current_dir()?;

        if !gitutil::is_git_repo(&repo_path)? {
            return Err(CliError::Other(
                "--worktree requires a git repository".into(),
            ));
        }

        if !args.allow_dirty && !gitutil::is_clean(&repo_path)? {
            return Err(gitutil::GitError::DirtyWorkingTree.into());
        }

        let base_sha = gitutil::current_sha(&repo_path)?;
        let branch_name = format!("attractor/run/{run_id}");
        let worktree_dir = run_directory.manifest().directories.root.join("worktree");

        gitutil::create_worktree(&repo_path, &worktree_dir, &branch_name, &base_sha)?;

        eprintln!(
            "Worktree created: {} (branch: {branch_name})",
            worktree_dir.display()
        );

        worktree_dir.display().to_string()
    } else {
        run_working_dir
    };

    let artifact_store = ArtifactStore::new();
    let config = EngineConfig {
        max_steps: args.max_steps,
        enable_checkpointing: true,
        checkpoint_dir: Some(run_directory.manifest().directories.checkpoints.clone()),
        artifact_store: Some(artifact_store),
        ..EngineConfig::default()
    };

    // Create a pipeline event emitter when the TUI is active so the engine and
    // the ClaudeCliBackend streaming path can both forward events to the TUI.
    let pipeline_emitter: Option<Arc<PipelineEventEmitter>> = if tui_enabled {
        Some(Arc::new(PipelineEventEmitter::new(256)))
    } else {
        None
    };

    let mut registry = default_registry();
    match args.backend.as_str() {
        "claude-cli" => {
            let client = client.as_ref().unwrap();
            let claude_backend: Arc<dyn CodergenBackend> = Arc::new(ClaudeCliBackend {
                working_dir: effective_working_dir.clone(),
                timeout: Duration::from_secs(args.agent_timeout),
                claude_path: None,
                streaming: tui_enabled,
                emitter: pipeline_emitter.clone(),
            });
            let agent_backend: Arc<dyn CodergenBackend> = Arc::new(AgentCodergenBackend::new(
                Arc::clone(client),
                args.model.clone(),
                effective_working_dir.clone(),
            ));
            registry.register(Arc::new(CodergenHandler::with_backends(
                claude_backend,
                agent_backend,
            )));
        }
        "agent" => {
            let client = client.as_ref().unwrap();
            let agent_backend: Arc<dyn CodergenBackend> = Arc::new(AgentCodergenBackend::new(
                Arc::clone(client),
                args.model.clone(),
                effective_working_dir.clone(),
            ));
            registry.register(Arc::new(CodergenHandler::new(agent_backend)));
        }
        "shell" => {
            let shell_backend: Arc<dyn CodergenBackend> = Arc::new(ShellCodergenBackend {
                working_dir: effective_working_dir.clone(),
                timeout: Duration::from_secs(args.agent_timeout),
            });
            registry.register(Arc::new(CodergenHandler::new(shell_backend)));
        }
        other => {
            return Err(CliError::Other(format!(
                "unknown --backend value '{}': expected 'claude-cli', 'agent', or 'shell'",
                other
            )));
        }
    }

    // Build interviewer based on --interviewer flag.
    let interviewer: Arc<dyn Interviewer> = if args.interviewer.starts_with("timeout:") {
        let secs: u64 = args.interviewer["timeout:".len()..]
            .parse()
            .map_err(|e| CliError::Other(format!("invalid timeout value: {e}")))?;
        Arc::new(TimeoutInterviewer::new(
            Arc::new(ConsoleInterviewer::from_stdio()),
            std::time::Duration::from_secs(secs),
        ))
    } else if args.interviewer == "console" {
        Arc::new(ConsoleInterviewer::from_stdio())
    } else {
        // "auto" mode: auto-approve (non-interactive)
        Arc::new(AutoApproveInterviewer::new())
    };

    // Register interviewer-dependent handlers.
    registry.register(Arc::new(InterviewerHandler::new(interviewer.clone())));
    registry.register(Arc::new(HumanGateHandler::new(interviewer)));

    // Register manager and tool handlers with LLM backends (skipped for shell backend).
    if let Some(ref client) = client {
        let manager_backend = Arc::new(crate::llm_backends::LlmManagerBackend::new(
            Arc::clone(client),
            args.model.clone(),
            effective_working_dir.clone(),
        ));
        registry.register(Arc::new(ManagerHandler::new(manager_backend)));

        let tool_backend = Arc::new(crate::llm_backends::LlmToolBackend::new(
            Arc::clone(client),
            args.model.clone(),
            effective_working_dir.clone(),
        ));
        registry.register(Arc::new(ToolHandler::new(tool_backend)));
    }

    // Register parallel handler. The registry parameter is reserved for future
    // engine-level parallel dispatch; the handler itself uses node attributes only.
    registry.register(Arc::new(ParallelHandler::new(Arc::new(
        smasher_attractor::handler::HandlerRegistry::new(),
    ))));

    // Capture graph and pipeline name before `resolved` is moved into the engine,
    // so the TUI can display node state without an extra clone on the no-TUI path.
    let tui_graph = if tui_enabled {
        Some(resolved.clone())
    } else {
        None
    };
    let tui_pipeline_name = resolved
        .name
        .clone()
        .unwrap_or_else(|| args.pipeline.clone());

    let mut engine = Engine::with_config(resolved, registry, config);

    // Apply sub-pipeline composition if any SubPipeline nodes exist.
    let pipeline_dir = std::path::Path::new(&args.pipeline)
        .parent()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| ".".to_string());
    engine.apply_sub_pipeline_transform(&pipeline_dir)?;

    // Attach the emitter so the engine broadcasts PipelineEvents during execution.
    if let Some(ref emitter) = pipeline_emitter {
        engine = engine.with_emitter(Arc::clone(emitter));
    }

    let context = Context::default();

    // Seed variables into the context.
    for (key, value) in &variables {
        context.set(key, serde_json::Value::String(value.clone()));
    }

    // Run the engine, with or without the TUI dashboard.
    let result = if tui_enabled {
        let emitter = pipeline_emitter.as_ref().unwrap();
        let event_rx = emitter.subscribe();

        let flags = TuiFlags {
            graph: tui_graph.unwrap(),
            run_id: run_id.clone(),
            pipeline_name: tui_pipeline_name,
        };

        let program = crate::tui::build_program(flags)
            .map_err(|e| CliError::Other(format!("TUI init failed: {e}")))?;

        let handle = program.handle();

        // Bridge: forward PipelineEvents from the broadcast channel to the TUI.
        // When the channel closes (engine dropped its emitter), send PipelineDone
        // so the TUI quits cleanly.
        tokio::spawn(async move {
            let mut rx = event_rx;
            loop {
                match rx.recv().await {
                    Ok(event) => {
                        handle.send(TuiMsg::PipelineEvent(event));
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!("TUI bridge skipped {n} events (slow consumer)");
                        continue;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        handle.send(TuiMsg::PipelineDone);
                        break;
                    }
                }
            }
        });

        // Run the engine as a background task so the TUI can render concurrently.
        let engine_task = tokio::spawn(async move { engine.run(context).await });

        // Block the current task running the TUI until the user quits or the
        // pipeline finishes (PipelineDone sends Command::quit()).
        program
            .run()
            .await
            .map_err(|e| CliError::Other(format!("TUI error: {e}")))?;

        // Retrieve the engine result after the TUI exits.
        engine_task
            .await
            .map_err(|e| CliError::Other(format!("engine task panicked: {e}")))?
    } else {
        engine.run(context).await
    };

    // Always clean up the worktree, even if the engine failed. Capture the
    // engine result and propagate any error *after* cleanup so that the
    // worktree branch and directory are not leaked on failure.
    if args.worktree {
        let worktree_dir = run_directory.manifest().directories.root.join("worktree");

        let msg = if result.is_ok() {
            format!("attractor({run_id}): final")
        } else {
            format!("attractor({run_id}): failed (partial)")
        };
        if let Ok(Some(sha)) = gitutil::commit_all_changes(&worktree_dir, &msg) {
            eprintln!("Final commit: {sha}");
        }

        if let Err(e) = gitutil::remove_worktree(&worktree_dir) {
            tracing::warn!("failed to clean up worktree: {e}");
        } else {
            eprintln!("Worktree cleaned up");
        }
    }

    let result = result?;

    let json = serde_json::to_string_pretty(&result.final_context)
        .map_err(|e| CliError::Other(format!("failed to serialize context: {e}")))?;
    println!("{json}");

    tracing::info!(
        steps = result.steps_taken,
        nodes_visited = result.visited_nodes.len(),
        "pipeline completed"
    );

    // Create a compressed archive of the run artifacts.
    let archive_path = run_directory.manifest().directories.root.join("run.tgz");
    match crate::archive::create_archive(&run_directory.manifest().directories.root, &archive_path)
    {
        Ok(path) => eprintln!("Archive: {}", path.display()),
        Err(e) => tracing::warn!("failed to create run archive: {e}"),
    }

    Ok(())
}

/// Generate a new run ID using ULID (Universally Unique Lexicographically Sortable Identifier).
///
/// ULIDs are time-ordered, so sequential IDs sort chronologically.
/// Output is lowercase Crockford base32, 26 characters.
fn generate_run_id() -> String {
    ulid::Ulid::new().to_string().to_lowercase()
}

/// Infer the render format from a file path extension.
///
/// Returns SVG as the default if the extension is not recognized.
fn infer_render_format(path: &str) -> RenderFormat {
    match path.rsplit('.').next() {
        Some(ext) => RenderFormat::from_str_loose(ext).unwrap_or(RenderFormat::Svg),
        None => RenderFormat::Svg,
    }
}

/// Information about a previous incomplete run that can be resumed.
struct IncompleteRun {
    run_dir: std::path::PathBuf,
    run_id: String,
    created_at: String,
}

/// Scan the artifacts directory for incomplete runs of the same pipeline.
///
/// A run is "incomplete" if it has a checkpoint.json but no run.tgz (the archive
/// is only written after the engine finishes successfully).
/// Matches by `graph_hash` (SHA256 of DOT source) for exact pipeline identity.
fn find_latest_incomplete_run(
    artifacts_base: &std::path::Path,
    graph_hash: &str,
) -> Option<IncompleteRun> {
    let entries = std::fs::read_dir(artifacts_base).ok()?;

    let mut candidates: Vec<IncompleteRun> = Vec::new();

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        let manifest_path = path.join("manifest.json");
        let checkpoint_path = path.join("checkpoints").join("checkpoint.json");
        let archive_path = path.join("run.tgz");

        // Must have checkpoint (was interrupted) and no archive (didn't finish).
        if !checkpoint_path.exists() || archive_path.exists() {
            continue;
        }

        // Read manifest and match graph_hash.
        let manifest_json = match std::fs::read_to_string(&manifest_path) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let manifest: serde_json::Value = match serde_json::from_str(&manifest_json) {
            Ok(v) => v,
            Err(_) => continue,
        };

        if manifest["graph_hash"].as_str() != Some(graph_hash) {
            continue;
        }

        let run_id = manifest["run_id"].as_str().unwrap_or("").to_string();
        let created_at = manifest["created_at"]
            .as_str()
            .unwrap_or("unknown")
            .to_string();

        candidates.push(IncompleteRun {
            run_dir: path,
            run_id,
            created_at,
        });
    }

    // ULIDs sort lexicographically by creation time — take the latest.
    candidates.sort_by(|a, b| b.run_id.cmp(&a.run_id));
    candidates.into_iter().next()
}

/// Compute the SHA256 hex digest of DOT source (matches run_dir.rs hashing).
fn graph_hash(dot_source: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(dot_source.as_bytes());
    format!("{:x}", hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    /// Wrapper struct for testing RunArgs parsing via clap.
    #[derive(Debug, Parser)]
    struct TestCli {
        #[command(flatten)]
        run: RunArgs,
    }

    #[test]
    fn worktree_flag_defaults_to_false() {
        let cli = TestCli::parse_from(["test", "pipeline.dot"]);
        assert!(!cli.run.worktree);
        assert!(!cli.run.allow_dirty);
    }

    #[test]
    fn worktree_flag_parsed_when_present() {
        let cli = TestCli::parse_from(["test", "--worktree", "pipeline.dot"]);
        assert!(cli.run.worktree);
        assert!(!cli.run.allow_dirty);
    }

    #[test]
    fn allow_dirty_flag_parsed_when_present() {
        let cli = TestCli::parse_from(["test", "--worktree", "--allow-dirty", "pipeline.dot"]);
        assert!(cli.run.worktree);
        assert!(cli.run.allow_dirty);
    }

    #[test]
    fn allow_dirty_without_worktree_parses_successfully() {
        let cli = TestCli::parse_from(["test", "--allow-dirty", "pipeline.dot"]);
        assert!(!cli.run.worktree);
        assert!(cli.run.allow_dirty);
    }

    #[test]
    fn infer_svg_from_extension() {
        assert_eq!(infer_render_format("graph.svg"), RenderFormat::Svg);
    }

    #[test]
    fn infer_png_from_extension() {
        assert_eq!(infer_render_format("output.png"), RenderFormat::Png);
    }

    #[test]
    fn infer_dot_from_extension() {
        assert_eq!(infer_render_format("pipeline.dot"), RenderFormat::Dot);
    }

    #[test]
    fn infer_defaults_to_svg_for_unknown() {
        assert_eq!(infer_render_format("pipeline.pdf"), RenderFormat::Svg);
    }

    #[test]
    fn infer_defaults_to_svg_for_no_extension() {
        assert_eq!(infer_render_format("pipeline"), RenderFormat::Svg);
    }

    #[test]
    fn infer_handles_path_with_dirs() {
        assert_eq!(
            infer_render_format("/tmp/output/graph.png"),
            RenderFormat::Png
        );
    }

    // ---- ULID run ID tests ----

    #[test]
    fn generate_run_id_is_valid_crockford_base32() {
        let id = generate_run_id();
        // ULIDs are 26 characters of Crockford base32
        assert_eq!(
            id.len(),
            26,
            "ULID should be 26 characters, got {}",
            id.len()
        );
        // Crockford base32 lowercase: 0-9 and a-z excluding i, l, o, u
        let valid_chars = "0123456789abcdefghjkmnpqrstvwxyz";
        for c in id.chars() {
            assert!(
                valid_chars.contains(c),
                "character '{}' is not valid Crockford base32 in '{}'",
                c,
                id,
            );
        }
    }

    #[test]
    fn generate_run_id_sequential_ids_sort_chronologically() {
        let id1 = generate_run_id();
        // Small delay to ensure different timestamp component
        std::thread::sleep(std::time::Duration::from_millis(2));
        let id2 = generate_run_id();
        assert!(
            id2 > id1,
            "sequential ULIDs should sort in order: {} vs {}",
            id1,
            id2,
        );
    }

    #[test]
    fn generate_run_id_is_url_safe() {
        let id = generate_run_id();
        for c in id.chars() {
            assert!(
                c.is_ascii_alphanumeric(),
                "character '{}' is not URL-safe in '{}'",
                c,
                id,
            );
        }
    }

    #[test]
    fn generate_run_id_is_lowercase() {
        let id = generate_run_id();
        assert_eq!(id, id.to_lowercase(), "ULID should be lowercase");
    }

    // ---- Interviewer flag tests ----

    #[test]
    fn interviewer_flag_defaults_to_auto() {
        let cli = TestCli::parse_from(["test", "pipeline.dot"]);
        assert_eq!(cli.run.interviewer, "auto");
    }

    #[test]
    fn interviewer_flag_parsed_when_present() {
        let cli = TestCli::parse_from(["test", "--interviewer", "console", "pipeline.dot"]);
        assert_eq!(cli.run.interviewer, "console");
    }

    #[test]
    fn interviewer_timeout_flag_parsed() {
        let cli = TestCli::parse_from(["test", "--interviewer", "timeout:30", "pipeline.dot"]);
        assert_eq!(cli.run.interviewer, "timeout:30");
    }

    #[test]
    fn skip_lint_flag_defaults_to_false() {
        let cli = TestCli::parse_from(["test", "pipeline.dot"]);
        assert!(!cli.run.skip_lint);
    }

    #[test]
    fn skip_lint_flag_parsed_when_present() {
        let cli = TestCli::parse_from(["test", "--skip-lint", "pipeline.dot"]);
        assert!(cli.run.skip_lint);
    }

    // ---- ClaudeCliBackend streaming mode tests ----

    #[tokio::test]
    async fn claude_cli_backend_streaming_uses_output_format_stream_json() {
        let tmp = tempfile::tempdir().unwrap();
        let args_file = tmp.path().join("captured_args.txt");
        let ndjson_file = tmp.path().join("output.ndjson");
        std::fs::write(
            &ndjson_file,
            "{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false,\"duration_ms\":1,\"result\":\"done\"}\n",
        )
        .unwrap();
        let script_path = tmp.path().join("claude");
        let script_content = format!(
            "#!/bin/sh\necho \"$@\" > {}\ncat {}\n",
            args_file.display(),
            ndjson_file.display()
        );
        std::fs::write(&script_path, script_content).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        let backend = ClaudeCliBackend {
            working_dir: tmp.path().display().to_string(),
            timeout: std::time::Duration::from_secs(10),
            claude_path: Some(script_path.display().to_string()),
            streaming: true,
            emitter: None,
        };

        let ctx = smasher_attractor::state::Context::new();
        backend.generate("test prompt", None, &ctx).await.unwrap();

        let captured = std::fs::read_to_string(&args_file).unwrap();
        assert!(
            !captured.contains("--print"),
            "streaming mode should not pass --print, got: {captured}"
        );
        assert!(
            captured.contains("--output-format"),
            "streaming mode should pass --output-format, got: {captured}"
        );
        assert!(
            captured.contains("stream-json"),
            "streaming mode should use stream-json format, got: {captured}"
        );
    }

    #[tokio::test]
    async fn claude_cli_backend_streaming_returns_result_from_result_line() {
        let tmp = tempfile::tempdir().unwrap();
        let ndjson_file = tmp.path().join("output.ndjson");
        std::fs::write(
            &ndjson_file,
            concat!(
                "{\"type\":\"assistant\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"intermediate\"}]}}\n",
                "{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false,\"duration_ms\":100,\"result\":\"the final answer\"}\n"
            ),
        )
        .unwrap();
        let script_path = tmp.path().join("claude");
        let script_content = format!("#!/bin/sh\ncat {}\n", ndjson_file.display());
        std::fs::write(&script_path, script_content).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        let backend = ClaudeCliBackend {
            working_dir: tmp.path().display().to_string(),
            timeout: std::time::Duration::from_secs(10),
            claude_path: Some(script_path.display().to_string()),
            streaming: true,
            emitter: None,
        };

        let ctx = smasher_attractor::state::Context::new();
        let result = backend.generate("test", None, &ctx).await.unwrap();

        match result {
            Outcome::Success {
                data: Some(data), ..
            } => {
                let response = data["response"].as_str().unwrap();
                assert_eq!(response, "the final answer");
            }
            other => panic!("expected success, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn claude_cli_backend_streaming_emits_agent_message_for_text_blocks() {
        let tmp = tempfile::tempdir().unwrap();
        let ndjson_file = tmp.path().join("output.ndjson");
        std::fs::write(
            &ndjson_file,
            concat!(
                "{\"type\":\"assistant\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"hello world\"}]}}\n",
                "{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false,\"duration_ms\":1,\"result\":\"hello world\"}\n"
            ),
        )
        .unwrap();
        let script_path = tmp.path().join("claude");
        let script_content = format!("#!/bin/sh\ncat {}\n", ndjson_file.display());
        std::fs::write(&script_path, script_content).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        let emitter = Arc::new(smasher_attractor::events::PipelineEventEmitter::default());
        let mut rx = emitter.subscribe();

        let backend = ClaudeCliBackend {
            working_dir: tmp.path().display().to_string(),
            timeout: std::time::Duration::from_secs(10),
            claude_path: Some(script_path.display().to_string()),
            streaming: true,
            emitter: Some(Arc::clone(&emitter)),
        };

        let ctx = smasher_attractor::state::Context::new();
        ctx.set("_current_node_id", serde_json::json!("node1"));
        backend.generate("test", None, &ctx).await.unwrap();
        drop(emitter);

        let mut agent_messages = Vec::new();
        while let Ok(event) = rx.try_recv() {
            if let smasher_attractor::events::PipelineEvent::AgentMessage {
                text, node_id, ..
            } = event
            {
                agent_messages.push((text, node_id));
            }
        }

        assert_eq!(agent_messages.len(), 1, "expected one AgentMessage event");
        assert_eq!(agent_messages[0].0, "hello world");
        assert_eq!(agent_messages[0].1, "node1");
    }

    #[tokio::test]
    async fn claude_cli_backend_streaming_emits_tool_call_started_for_tool_use() {
        let tmp = tempfile::tempdir().unwrap();
        let ndjson_file = tmp.path().join("output.ndjson");
        std::fs::write(
            &ndjson_file,
            concat!(
                "{\"type\":\"assistant\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"tool_use\",\"id\":\"tc1\",\"name\":\"bash\",\"input\":{\"command\":\"ls\"}}]}}\n",
                "{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false,\"duration_ms\":1,\"result\":\"done\"}\n"
            ),
        )
        .unwrap();
        let script_path = tmp.path().join("claude");
        let script_content = format!("#!/bin/sh\ncat {}\n", ndjson_file.display());
        std::fs::write(&script_path, script_content).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        let emitter = Arc::new(smasher_attractor::events::PipelineEventEmitter::default());
        let mut rx = emitter.subscribe();

        let backend = ClaudeCliBackend {
            working_dir: tmp.path().display().to_string(),
            timeout: std::time::Duration::from_secs(10),
            claude_path: Some(script_path.display().to_string()),
            streaming: true,
            emitter: Some(Arc::clone(&emitter)),
        };

        let ctx = smasher_attractor::state::Context::new();
        ctx.set("_current_node_id", serde_json::json!("coder_node"));
        backend.generate("test", None, &ctx).await.unwrap();
        drop(emitter);

        let mut tool_starts = Vec::new();
        while let Ok(event) = rx.try_recv() {
            if let smasher_attractor::events::PipelineEvent::AgentToolCallStarted {
                tool_name,
                tool_call_id,
                node_id,
                ..
            } = event
            {
                tool_starts.push((tool_name, tool_call_id, node_id));
            }
        }

        assert_eq!(
            tool_starts.len(),
            1,
            "expected one AgentToolCallStarted event"
        );
        assert_eq!(tool_starts[0].0, "bash");
        assert_eq!(tool_starts[0].1, "tc1");
        assert_eq!(tool_starts[0].2, "coder_node");
    }

    // ---- Backend flag tests ----

    #[test]
    fn backend_flag_defaults_to_claude_cli() {
        let cli = TestCli::parse_from(["test", "pipeline.dot"]);
        assert_eq!(cli.run.backend, "claude-cli");
    }

    #[test]
    fn backend_flag_parsed_as_agent() {
        let cli = TestCli::parse_from(["test", "--backend", "agent", "pipeline.dot"]);
        assert_eq!(cli.run.backend, "agent");
    }

    #[test]
    fn backend_flag_parsed_as_claude_cli() {
        let cli = TestCli::parse_from(["test", "--backend", "claude-cli", "pipeline.dot"]);
        assert_eq!(cli.run.backend, "claude-cli");
    }

    // ---- Agent timeout flag tests ----

    #[test]
    fn agent_timeout_flag_defaults_to_600() {
        let cli = TestCli::parse_from(["test", "pipeline.dot"]);
        assert_eq!(cli.run.agent_timeout, 600);
    }

    #[test]
    fn agent_timeout_flag_parsed_when_present() {
        let cli = TestCli::parse_from(["test", "--agent-timeout", "120", "pipeline.dot"]);
        assert_eq!(cli.run.agent_timeout, 120);
    }

    // ---- ClaudeCliBackend tests ----

    #[tokio::test]
    async fn claude_cli_backend_captures_stdout_as_response() {
        let tmp = tempfile::tempdir().unwrap();
        let script_path = tmp.path().join("claude");
        std::fs::write(&script_path, "#!/bin/sh\necho \"hello from fake claude\"\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        let backend = ClaudeCliBackend {
            working_dir: tmp.path().display().to_string(),
            timeout: std::time::Duration::from_secs(10),
            claude_path: Some(script_path.display().to_string()),
            streaming: false,
            emitter: None,
        };

        let ctx = smasher_attractor::state::Context::new();
        let result = backend.generate("test prompt", None, &ctx).await.unwrap();

        match result {
            Outcome::Success {
                data: Some(data), ..
            } => {
                let response = data["response"].as_str().unwrap();
                assert_eq!(response, "hello from fake claude");
            }
            other => panic!("expected success, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn claude_cli_backend_passes_prompt_to_process() {
        let tmp = tempfile::tempdir().unwrap();
        // Script that writes all args to a file so we can inspect them.
        let args_file = tmp.path().join("captured_args.txt");
        let script_path = tmp.path().join("claude");
        let script_content = format!(
            "#!/bin/sh\necho \"$@\" > {}\necho \"done\"\n",
            args_file.display()
        );
        std::fs::write(&script_path, script_content).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        let backend = ClaudeCliBackend {
            working_dir: tmp.path().display().to_string(),
            timeout: std::time::Duration::from_secs(10),
            claude_path: Some(script_path.display().to_string()),
            streaming: false,
            emitter: None,
        };

        let ctx = smasher_attractor::state::Context::new();
        backend
            .generate("my special prompt", None, &ctx)
            .await
            .unwrap();

        let captured = std::fs::read_to_string(&args_file).unwrap();
        assert!(
            captured.contains("--dangerously-skip-permissions"),
            "should pass --dangerously-skip-permissions, got: {captured}"
        );
        assert!(
            captured.contains("--print"),
            "should pass --print, got: {captured}"
        );
        assert!(
            captured.contains("-p"),
            "should pass -p flag, got: {captured}"
        );
    }

    #[tokio::test]
    async fn claude_cli_backend_returns_error_on_nonzero_exit() {
        let tmp = tempfile::tempdir().unwrap();
        let script_path = tmp.path().join("claude");
        std::fs::write(&script_path, "#!/bin/sh\nexit 1\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        let backend = ClaudeCliBackend {
            working_dir: tmp.path().display().to_string(),
            timeout: std::time::Duration::from_secs(10),
            claude_path: Some(script_path.display().to_string()),
            streaming: false,
            emitter: None,
        };

        let ctx = smasher_attractor::state::Context::new();
        let result = backend.generate("test prompt", None, &ctx).await;

        assert!(result.is_err(), "non-zero exit should produce an error");
        let err = result.unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("exit"),
            "error should mention exit code, got: {msg}"
        );
    }

    #[tokio::test]
    async fn claude_cli_backend_times_out() {
        let tmp = tempfile::tempdir().unwrap();
        let script_path = tmp.path().join("claude");
        // Script that sleeps longer than our timeout.
        std::fs::write(&script_path, "#!/bin/sh\nsleep 30\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        let backend = ClaudeCliBackend {
            working_dir: tmp.path().display().to_string(),
            timeout: std::time::Duration::from_millis(100),
            claude_path: Some(script_path.display().to_string()),
            streaming: false,
            emitter: None,
        };

        let ctx = smasher_attractor::state::Context::new();
        let result = backend.generate("test prompt", None, &ctx).await;

        assert!(result.is_err(), "timeout should produce an error");
        let err = result.unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.to_lowercase().contains("timeout") || msg.to_lowercase().contains("timed out"),
            "error should mention timeout, got: {msg}"
        );
    }

    #[tokio::test]
    async fn claude_cli_backend_includes_context_in_prompt() {
        let tmp = tempfile::tempdir().unwrap();
        // Script that writes the prompt arg (everything after -p) to a file.
        let prompt_file = tmp.path().join("captured_prompt.txt");
        let script_path = tmp.path().join("claude");
        // Capture the argument after -p.
        let script_content = format!(
            "#!/bin/sh\nwhile [ $# -gt 0 ]; do\n  if [ \"$1\" = \"-p\" ]; then\n    shift\n    echo \"$1\" > {}\n    break\n  fi\n  shift\ndone\necho \"done\"\n",
            prompt_file.display()
        );
        std::fs::write(&script_path, script_content).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        let backend = ClaudeCliBackend {
            working_dir: tmp.path().display().to_string(),
            timeout: std::time::Duration::from_secs(10),
            claude_path: Some(script_path.display().to_string()),
            streaming: false,
            emitter: None,
        };

        let ctx = smasher_attractor::state::Context::new();
        ctx.set("project_name", serde_json::json!("my_project"));
        ctx.set("_internal", serde_json::json!("hidden"));

        backend.generate("do the thing", None, &ctx).await.unwrap();

        let captured = std::fs::read_to_string(&prompt_file).unwrap();
        assert!(
            captured.contains("project_name"),
            "prompt should include context key, got: {captured}"
        );
        assert!(
            captured.contains("my_project"),
            "prompt should include context value, got: {captured}"
        );
        assert!(
            !captured.contains("_internal"),
            "prompt should NOT include underscore-prefixed keys, got: {captured}"
        );
        assert!(
            captured.contains("do the thing"),
            "prompt should include original prompt, got: {captured}"
        );
    }

    #[tokio::test]
    async fn claude_cli_backend_sets_working_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let pwd_file = tmp.path().join("captured_pwd.txt");
        let script_path = tmp.path().join("claude");
        let script_content = format!("#!/bin/sh\npwd > {}\necho \"done\"\n", pwd_file.display());
        std::fs::write(&script_path, script_content).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        let work_dir = tmp.path().display().to_string();
        let backend = ClaudeCliBackend {
            working_dir: work_dir.clone(),
            timeout: std::time::Duration::from_secs(10),
            claude_path: Some(script_path.display().to_string()),
            streaming: false,
            emitter: None,
        };

        let ctx = smasher_attractor::state::Context::new();
        backend.generate("test", None, &ctx).await.unwrap();

        let captured_pwd = std::fs::read_to_string(&pwd_file).unwrap();
        let captured_pwd = captured_pwd.trim();
        // Resolve symlinks for comparison (macOS /tmp -> /private/tmp).
        let expected = std::fs::canonicalize(tmp.path()).unwrap();
        let actual = std::fs::canonicalize(captured_pwd).unwrap();
        assert_eq!(actual, expected, "working dir should match");
    }

    // ---- TUI flag tests ----

    #[test]
    fn tui_flag_defaults_to_false() {
        let cli = TestCli::parse_from(["test", "pipeline.dot"]);
        assert!(!cli.run.tui);
        assert!(!cli.run.no_tui);
    }

    #[test]
    fn tui_flag_parsed_when_present() {
        let cli = TestCli::parse_from(["test", "--tui", "pipeline.dot"]);
        assert!(cli.run.tui);
        assert!(!cli.run.no_tui);
    }

    #[test]
    fn no_tui_flag_parsed_when_present() {
        let cli = TestCli::parse_from(["test", "--no-tui", "pipeline.dot"]);
        assert!(!cli.run.tui);
        assert!(cli.run.no_tui);
    }

    #[test]
    fn should_enable_tui_returns_true_for_explicit_tui_flag() {
        let cli = TestCli::parse_from(["test", "--tui", "pipeline.dot"]);
        assert!(should_enable_tui(&cli.run));
    }

    #[test]
    fn should_enable_tui_returns_false_for_no_tui_flag() {
        let cli = TestCli::parse_from(["test", "--no-tui", "pipeline.dot"]);
        assert!(!should_enable_tui(&cli.run));
    }

    #[test]
    fn should_enable_tui_no_tui_overrides_tui() {
        let cli = TestCli::parse_from(["test", "--tui", "--no-tui", "pipeline.dot"]);
        assert!(!should_enable_tui(&cli.run));
    }

    // ---- Shell backend flag test ----

    #[test]
    fn backend_flag_parsed_as_shell() {
        let cli = TestCli::parse_from(["test", "--backend", "shell", "pipeline.dot"]);
        assert_eq!(cli.run.backend, "shell");
    }

    // ---- ShellCodergenBackend tests ----

    #[tokio::test]
    async fn shell_backend_runs_prompt_as_shell_command() {
        let tmp = tempfile::tempdir().unwrap();
        let backend = ShellCodergenBackend {
            working_dir: tmp.path().display().to_string(),
            timeout: Duration::from_secs(10),
        };

        let ctx = smasher_attractor::state::Context::new();
        let result = backend
            .generate("echo 'hello from shell'", None, &ctx)
            .await
            .unwrap();

        match result {
            Outcome::Success {
                data: Some(data), ..
            } => {
                let response = data["stdout"].as_str().unwrap();
                assert_eq!(response.trim(), "hello from shell");
            }
            other => panic!("expected success, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn shell_backend_returns_failure_on_nonzero_exit() {
        let tmp = tempfile::tempdir().unwrap();
        let backend = ShellCodergenBackend {
            working_dir: tmp.path().display().to_string(),
            timeout: Duration::from_secs(10),
        };

        let ctx = smasher_attractor::state::Context::new();
        let result = backend
            .generate("echo 'oops' >&2; exit 1", None, &ctx)
            .await
            .unwrap();

        match result {
            Outcome::Failure { error, .. } => {
                assert!(
                    error.contains("oops"),
                    "failure should include stderr, got: {error}"
                );
            }
            other => panic!("expected failure, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn shell_backend_sets_working_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let backend = ShellCodergenBackend {
            working_dir: tmp.path().display().to_string(),
            timeout: Duration::from_secs(10),
        };

        let ctx = smasher_attractor::state::Context::new();
        let result = backend.generate("pwd", None, &ctx).await.unwrap();

        match result {
            Outcome::Success {
                data: Some(data), ..
            } => {
                let stdout = data["stdout"].as_str().unwrap().trim();
                let expected = std::fs::canonicalize(tmp.path()).unwrap();
                let actual = std::fs::canonicalize(stdout).unwrap();
                assert_eq!(actual, expected, "working dir should match");
            }
            other => panic!("expected success, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn shell_backend_times_out() {
        let tmp = tempfile::tempdir().unwrap();
        let backend = ShellCodergenBackend {
            working_dir: tmp.path().display().to_string(),
            timeout: Duration::from_millis(100),
        };

        let ctx = smasher_attractor::state::Context::new();
        let result = backend.generate("sleep 30", None, &ctx).await;

        assert!(result.is_err(), "timeout should produce an error");
        let err = result.unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.to_lowercase().contains("timeout") || msg.to_lowercase().contains("timed out"),
            "error should mention timeout, got: {msg}"
        );
    }

    #[tokio::test]
    async fn shell_backend_includes_context_as_env_vars() {
        let tmp = tempfile::tempdir().unwrap();
        let backend = ShellCodergenBackend {
            working_dir: tmp.path().display().to_string(),
            timeout: Duration::from_secs(10),
        };

        let ctx = smasher_attractor::state::Context::new();
        ctx.set("project_name", serde_json::json!("my_project"));
        ctx.set("_internal", serde_json::json!("hidden"));

        // Verify public context is exported and underscore-prefixed internals are excluded.
        let result = backend
            .generate(
                "echo $SMASHER_CTX_project_name; echo \"internal=${SMASHER_CTX__internal:-unset}\"",
                None,
                &ctx,
            )
            .await
            .unwrap();

        match result {
            Outcome::Success {
                data: Some(data), ..
            } => {
                let stdout = data["stdout"].as_str().unwrap();
                let lines: Vec<&str> = stdout.trim().lines().collect();
                assert_eq!(lines[0], "my_project");
                assert_eq!(
                    lines[1], "internal=unset",
                    "_internal should not be exported"
                );
            }
            other => panic!("expected success, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn shell_backend_captures_both_stdout_and_exit_code() {
        let tmp = tempfile::tempdir().unwrap();
        let backend = ShellCodergenBackend {
            working_dir: tmp.path().display().to_string(),
            timeout: Duration::from_secs(10),
        };

        let ctx = smasher_attractor::state::Context::new();
        let result = backend
            .generate("echo 'line1'; echo 'line2'", None, &ctx)
            .await
            .unwrap();

        match result {
            Outcome::Success {
                data: Some(data), ..
            } => {
                let stdout = data["stdout"].as_str().unwrap();
                assert!(stdout.contains("line1"));
                assert!(stdout.contains("line2"));
                assert_eq!(data["exit_code"].as_i64().unwrap(), 0);
            }
            other => panic!("expected success, got {other:?}"),
        }
    }

    #[test]
    fn resume_flag_defaults_to_false() {
        let cli = TestCli::parse_from(["test", "pipeline.dot"]);
        assert!(!cli.run.resume);
    }

    #[test]
    fn resume_flag_parsed_when_present() {
        let cli = TestCli::parse_from(["test", "--resume", "pipeline.dot"]);
        assert!(cli.run.resume);
    }

    #[test]
    fn find_incomplete_run_returns_none_when_no_artifacts() {
        let tmp = tempfile::tempdir().unwrap();
        let result = find_latest_incomplete_run(tmp.path(), "abc123");
        assert!(result.is_none());
    }

    #[test]
    fn find_incomplete_run_returns_none_when_completed() {
        let tmp = tempfile::tempdir().unwrap();
        let run_dir = tmp.path().join("01abc");
        std::fs::create_dir_all(run_dir.join("checkpoints")).unwrap();
        std::fs::write(run_dir.join("checkpoints").join("checkpoint.json"), "{}").unwrap();
        // run.tgz exists → completed
        std::fs::write(run_dir.join("run.tgz"), "archive").unwrap();
        std::fs::write(
            run_dir.join("manifest.json"),
            serde_json::json!({
                "run_id": "01abc",
                "graph_hash": "hash1",
                "created_at": "2026-01-01T00:00:00Z",
            })
            .to_string(),
        )
        .unwrap();

        let result = find_latest_incomplete_run(tmp.path(), "hash1");
        assert!(result.is_none());
    }

    #[test]
    fn find_incomplete_run_returns_latest_match() {
        let tmp = tempfile::tempdir().unwrap();

        // Older incomplete run
        let run1 = tmp.path().join("01aaa");
        std::fs::create_dir_all(run1.join("checkpoints")).unwrap();
        std::fs::write(run1.join("checkpoints/checkpoint.json"), "{}").unwrap();
        std::fs::write(
            run1.join("manifest.json"),
            serde_json::json!({
                "run_id": "01aaa",
                "graph_hash": "myhash",
                "created_at": "2026-01-01T00:00:00Z",
            })
            .to_string(),
        )
        .unwrap();

        // Newer incomplete run
        let run2 = tmp.path().join("01bbb");
        std::fs::create_dir_all(run2.join("checkpoints")).unwrap();
        std::fs::write(run2.join("checkpoints/checkpoint.json"), "{}").unwrap();
        std::fs::write(
            run2.join("manifest.json"),
            serde_json::json!({
                "run_id": "01bbb",
                "graph_hash": "myhash",
                "created_at": "2026-01-02T00:00:00Z",
            })
            .to_string(),
        )
        .unwrap();

        // Different pipeline (should be ignored)
        let run3 = tmp.path().join("01ccc");
        std::fs::create_dir_all(run3.join("checkpoints")).unwrap();
        std::fs::write(run3.join("checkpoints/checkpoint.json"), "{}").unwrap();
        std::fs::write(
            run3.join("manifest.json"),
            serde_json::json!({
                "run_id": "01ccc",
                "graph_hash": "otherhash",
                "created_at": "2026-01-03T00:00:00Z",
            })
            .to_string(),
        )
        .unwrap();

        let result = find_latest_incomplete_run(tmp.path(), "myhash").unwrap();
        assert_eq!(result.run_id, "01bbb");
    }

    #[test]
    fn graph_hash_is_deterministic() {
        let source = "digraph { a -> b }";
        assert_eq!(graph_hash(source), graph_hash(source));
    }

    #[test]
    fn graph_hash_differs_for_different_sources() {
        assert_ne!(
            graph_hash("digraph { a -> b }"),
            graph_hash("digraph { x -> y }")
        );
    }
}
