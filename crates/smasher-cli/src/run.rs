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

use crate::error::CliError;
use crate::gitutil;

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
        cmd.arg("--dangerously-skip-permissions")
            .arg("--print")
            .arg("-p")
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

    /// Backend for codergen nodes: "claude-cli" (default) or "agent".
    #[arg(long, default_value = "claude-cli")]
    pub backend: String,

    /// Wall-clock timeout in seconds for the claude-cli backend (default: 600).
    #[arg(long, default_value = "600")]
    pub agent_timeout: u64,

    /// Interviewer mode for human-in-the-loop nodes: auto, console, or timeout:N
    /// (default: auto if no tty, console if tty).
    #[arg(long, default_value = "auto")]
    pub interviewer: String,
}

pub async fn run(args: RunArgs) -> Result<(), CliError> {
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

    let client = smasher_llm::client::Client::from_env();
    if client.registered_providers().is_empty() {
        return Err(CliError::Other(
            "no API keys found. Set ANTHROPIC_API_KEY, OPENAI_API_KEY, or GEMINI_API_KEY.".into(),
        ));
    }
    let client = Arc::new(client);

    // Run preflight health-checks unless explicitly skipped.
    if !args.skip_preflight {
        match smasher_attractor::preflight::preflight_check(&resolved, &client).await {
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

    // Create per-run artifact directory for isolation.
    let run_id = generate_run_id();
    let artifacts_base = std::path::Path::new(&working_dir).join("artifacts");
    let graph_name =
        smasher_attractor::run_dir::sanitize_graph_name(&resolved.name.clone().unwrap_or_default());
    let run_directory = smasher_attractor::run_dir::RunDirectory::create(
        &artifacts_base,
        &run_id,
        &graph_name,
        &dot_source,
    )?;
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

    let mut registry = default_registry();
    match args.backend.as_str() {
        "claude-cli" => {
            let claude_backend: Arc<dyn CodergenBackend> = Arc::new(ClaudeCliBackend {
                working_dir: effective_working_dir.clone(),
                timeout: Duration::from_secs(args.agent_timeout),
                claude_path: None,
            });
            let agent_backend: Arc<dyn CodergenBackend> = Arc::new(AgentCodergenBackend::new(
                Arc::clone(&client),
                args.model.clone(),
                effective_working_dir.clone(),
            ));
            registry.register(Arc::new(CodergenHandler::with_backends(
                claude_backend,
                agent_backend,
            )));
        }
        "agent" => {
            let agent_backend: Arc<dyn CodergenBackend> = Arc::new(AgentCodergenBackend::new(
                Arc::clone(&client),
                args.model.clone(),
                effective_working_dir.clone(),
            ));
            registry.register(Arc::new(CodergenHandler::new(agent_backend)));
        }
        other => {
            return Err(CliError::Other(format!(
                "unknown --backend value '{}': expected 'claude-cli' or 'agent'",
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

    // Register manager and tool handlers with LLM backends.
    let manager_backend = Arc::new(crate::llm_backends::LlmManagerBackend::new(
        Arc::clone(&client),
        args.model.clone(),
        effective_working_dir.clone(),
    ));
    registry.register(Arc::new(ManagerHandler::new(manager_backend)));

    let tool_backend = Arc::new(crate::llm_backends::LlmToolBackend::new(
        Arc::clone(&client),
        args.model.clone(),
        effective_working_dir.clone(),
    ));
    registry.register(Arc::new(ToolHandler::new(tool_backend)));

    // Register parallel handler. The registry parameter is reserved for future
    // engine-level parallel dispatch; the handler itself uses node attributes only.
    registry.register(Arc::new(ParallelHandler::new(Arc::new(
        smasher_attractor::handler::HandlerRegistry::new(),
    ))));

    let mut engine = Engine::with_config(resolved, registry, config);

    // Apply sub-pipeline composition if any SubPipeline nodes exist.
    let pipeline_dir = std::path::Path::new(&args.pipeline)
        .parent()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| ".".to_string());
    engine.apply_sub_pipeline_transform(&pipeline_dir)?;
    let context = Context::default();

    // Seed variables into the context.
    for (key, value) in &variables {
        context.set(key, serde_json::Value::String(value.clone()));
    }

    let result = engine.run(context).await;

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
}
