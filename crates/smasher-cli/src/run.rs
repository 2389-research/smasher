// ABOUTME: DOT pipeline execution subcommand that parses, resolves, and runs graph workflows.
// ABOUTME: Supports variables, stylesheets, step limits, and outputs final context as JSON.

use std::collections::HashMap;
use std::sync::Arc;

use clap::Args;
use smasher_agent::environment::LocalExecutionEnvironment;
use smasher_agent::events::EventEmitter;
use smasher_agent::session::Session;
use smasher_agent::tools::ToolRegistry;
use smasher_agent::tools::shared::register_shared_tools;
use smasher_agent::types::SessionConfig;

use smasher_attractor::dot::parser;
use smasher_attractor::engine::{Engine, EngineConfig};
use smasher_attractor::graph;
use smasher_attractor::handler::{
    CodergenBackend, CodergenHandler, HandlerError, default_registry,
};
use smasher_attractor::rendering::{CachedRenderer, GraphRenderer, GraphvizRenderer, RenderFormat};
use smasher_attractor::state::Context;
use smasher_attractor::state::Outcome;
use smasher_attractor::stylesheet::Stylesheet;
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

    let config = EngineConfig {
        max_steps: args.max_steps,
        enable_checkpointing: false,
        ..EngineConfig::default()
    };

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

    let backend = Arc::new(AgentCodergenBackend::new(
        Arc::clone(&client),
        args.model.clone(),
        effective_working_dir,
    ));
    let mut registry = default_registry();
    registry.register(Arc::new(CodergenHandler::new(backend)));

    let engine = Engine::with_config(resolved, registry, config);
    let context = Context::default();

    // Seed variables into the context.
    for (key, value) in &variables {
        context.set(key, serde_json::Value::String(value.clone()));
    }

    let result = engine.run(context).await?;

    // Commit and clean up the worktree if one was created.
    if args.worktree {
        let worktree_dir = run_directory.manifest().directories.root.join("worktree");

        if let Some(sha) =
            gitutil::commit_all_changes(&worktree_dir, &format!("attractor({run_id}): final"))?
        {
            eprintln!("Final commit: {sha}");
        }

        gitutil::remove_worktree(&worktree_dir)?;
        eprintln!("Worktree cleaned up");
    }

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
}
