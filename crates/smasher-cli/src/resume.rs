// ABOUTME: Resume subcommand that restores a previously checkpointed pipeline run.
// ABOUTME: Loads checkpoint from a run directory or explicit file path, re-parses the graph, and continues.

use std::path::PathBuf;
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
use smasher_attractor::state::{Checkpoint, Context, Outcome};

use crate::error::CliError;

/// CodergenBackend that runs a full agent session with file/shell tools.
///
/// Identical to the one in run.rs; kept local to avoid coupling the modules.
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

/// Resume a checkpointed pipeline run.
#[derive(Debug, Args)]
pub struct ResumeArgs {
    /// Path to a run directory (looks for checkpoints/checkpoint.json inside).
    #[arg(conflicts_with = "checkpoint")]
    pub run_dir: Option<String>,

    /// Path to a checkpoint JSON file directly.
    #[arg(long)]
    pub checkpoint: Option<String>,

    /// Model identifier for codergen nodes.
    #[arg(long, default_value = "claude-sonnet-4-20250514")]
    pub model: String,

    /// Maximum pipeline steps before forced stop.
    #[arg(long, default_value = "1000")]
    pub max_steps: usize,

    /// Skip preflight health-checks of LLM providers before execution.
    #[arg(long)]
    pub skip_preflight: bool,
}

pub async fn run(args: ResumeArgs) -> Result<(), CliError> {
    // Resolve the checkpoint file path from either run_dir or --checkpoint.
    let checkpoint_path = resolve_checkpoint_path(&args)?;
    eprintln!("Loading checkpoint: {}", checkpoint_path.display());

    let checkpoint_json = std::fs::read_to_string(&checkpoint_path)?;
    let checkpoint = Checkpoint::from_json(&checkpoint_json)?;

    // Determine the working directory: parent of the checkpoints directory.
    let working_dir = checkpoint_path
        .parent()
        .and_then(|p| p.parent())
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| {
            std::env::current_dir()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|_| ".".to_string())
        });

    eprintln!(
        "Resuming pipeline '{}' from node '{}'",
        checkpoint.pipeline_name, checkpoint.current_node
    );
    eprintln!(
        "Previously visited: {} node(s)",
        checkpoint.visited_nodes.len()
    );

    // Re-read the DOT source from the run directory's manifest.
    let dot_source = find_dot_source(&checkpoint_path)?;
    let dot_graph = parser::parse(&dot_source)?;
    let resolved = graph::resolve(&dot_graph)?;

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

    let backend = Arc::new(AgentCodergenBackend::new(
        Arc::clone(&client),
        args.model.clone(),
        working_dir,
    ));
    let mut registry = default_registry();
    registry.register(Arc::new(CodergenHandler::new(backend)));

    let config = EngineConfig {
        max_steps: args.max_steps,
        enable_checkpointing: false,
        ..EngineConfig::default()
    };

    let engine = Engine::with_config(resolved, registry, config);
    let context = Context::default();

    let result = engine.run_from_checkpoint(checkpoint, context).await?;

    let json = serde_json::to_string_pretty(&result.final_context)
        .map_err(|e| CliError::Other(format!("failed to serialize context: {e}")))?;
    println!("{json}");

    tracing::info!(
        steps = result.steps_taken,
        nodes_visited = result.visited_nodes.len(),
        "resumed pipeline completed"
    );

    Ok(())
}

/// Resolve the checkpoint file path from the arguments.
///
/// Either `--checkpoint <path>` pointing directly at a JSON file, or a
/// positional `<run_dir>` where we look for `checkpoints/checkpoint.json`.
fn resolve_checkpoint_path(args: &ResumeArgs) -> Result<PathBuf, CliError> {
    if let Some(ref cp_path) = args.checkpoint {
        let path = PathBuf::from(cp_path);
        if !path.exists() {
            return Err(CliError::Other(format!(
                "checkpoint file not found: {}",
                path.display()
            )));
        }
        return Ok(path);
    }

    if let Some(ref run_dir) = args.run_dir {
        let dir = PathBuf::from(run_dir);
        let cp_path = dir.join("checkpoints").join("checkpoint.json");
        if !cp_path.exists() {
            return Err(CliError::Other(format!(
                "no checkpoint found at: {}",
                cp_path.display()
            )));
        }
        return Ok(cp_path);
    }

    Err(CliError::Other(
        "provide either <run_dir> or --checkpoint <path>".into(),
    ))
}

/// Find the DOT source for a checkpoint's pipeline.
///
/// Looks for `graph.dot` in the run directory root (written by RunDirectory::create).
fn find_dot_source(checkpoint_path: &std::path::Path) -> Result<String, CliError> {
    // checkpoint_path is typically {run_dir}/checkpoints/checkpoint.json
    // So the run root is two parents up.
    let run_root = checkpoint_path
        .parent()
        .and_then(|p| p.parent())
        .ok_or_else(|| {
            CliError::Other(format!(
                "cannot determine run directory from checkpoint path: {}",
                checkpoint_path.display()
            ))
        })?;

    let dot_path = run_root.join("graph.dot");
    if dot_path.exists() {
        return std::fs::read_to_string(&dot_path).map_err(CliError::from);
    }

    // Fall back to manifest.json for the DOT source.
    let manifest_path = run_root.join("manifest.json");
    if manifest_path.exists() {
        // The manifest doesn't store DOT source directly, but the graph.dot
        // file should always be present. If neither exists, we can't resume.
        return Err(CliError::Other(format!(
            "graph.dot not found in run directory: {}",
            run_root.display()
        )));
    }

    Err(CliError::Other(format!(
        "cannot find DOT source in run directory: {}. Expected graph.dot at {}",
        run_root.display(),
        dot_path.display()
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    /// Wrapper struct for testing ResumeArgs parsing via clap.
    #[derive(Debug, Parser)]
    struct TestCli {
        #[command(flatten)]
        resume: ResumeArgs,
    }

    #[test]
    fn resume_with_run_dir() {
        let cli = TestCli::parse_from(["test", "/tmp/my-run"]);
        assert_eq!(cli.resume.run_dir, Some("/tmp/my-run".into()));
        assert!(cli.resume.checkpoint.is_none());
    }

    #[test]
    fn resume_with_checkpoint_flag() {
        let cli = TestCli::parse_from(["test", "--checkpoint", "/tmp/cp.json"]);
        assert!(cli.resume.run_dir.is_none());
        assert_eq!(cli.resume.checkpoint, Some("/tmp/cp.json".into()));
    }

    #[test]
    fn resume_default_model() {
        let cli = TestCli::parse_from(["test", "/tmp/run"]);
        assert_eq!(cli.resume.model, "claude-sonnet-4-20250514");
    }

    #[test]
    fn resume_custom_model() {
        let cli = TestCli::parse_from(["test", "--model", "gpt-4o", "/tmp/run"]);
        assert_eq!(cli.resume.model, "gpt-4o");
    }

    #[test]
    fn resume_custom_max_steps() {
        let cli = TestCli::parse_from(["test", "--max-steps", "500", "/tmp/run"]);
        assert_eq!(cli.resume.max_steps, 500);
    }

    #[test]
    fn resolve_checkpoint_path_from_run_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let cp_dir = tmp.path().join("checkpoints");
        std::fs::create_dir_all(&cp_dir).unwrap();
        let cp_file = cp_dir.join("checkpoint.json");
        std::fs::write(&cp_file, "{}").unwrap();

        let args = ResumeArgs {
            run_dir: Some(tmp.path().display().to_string()),
            checkpoint: None,
            model: "test".into(),
            max_steps: 100,
            skip_preflight: false,
        };

        let resolved = resolve_checkpoint_path(&args).unwrap();
        assert_eq!(resolved, cp_file);
    }

    #[test]
    fn resolve_checkpoint_path_from_explicit_path() {
        let tmp = tempfile::tempdir().unwrap();
        let cp_file = tmp.path().join("my_checkpoint.json");
        std::fs::write(&cp_file, "{}").unwrap();

        let args = ResumeArgs {
            run_dir: None,
            checkpoint: Some(cp_file.display().to_string()),
            model: "test".into(),
            max_steps: 100,
            skip_preflight: false,
        };

        let resolved = resolve_checkpoint_path(&args).unwrap();
        assert_eq!(resolved, cp_file);
    }

    #[test]
    fn resolve_checkpoint_path_missing_file_errors() {
        let args = ResumeArgs {
            run_dir: None,
            checkpoint: Some("/nonexistent/checkpoint.json".into()),
            model: "test".into(),
            max_steps: 100,
            skip_preflight: false,
        };

        let result = resolve_checkpoint_path(&args);
        assert!(result.is_err());
    }

    #[test]
    fn resolve_checkpoint_path_no_args_errors() {
        let args = ResumeArgs {
            run_dir: None,
            checkpoint: None,
            model: "test".into(),
            max_steps: 100,
            skip_preflight: false,
        };

        let result = resolve_checkpoint_path(&args);
        assert!(result.is_err());
    }
}
