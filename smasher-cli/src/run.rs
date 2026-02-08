// ABOUTME: DOT pipeline execution subcommand that parses, resolves, and runs graph workflows.
// ABOUTME: Supports variables, stylesheets, step limits, and outputs final context as JSON.

use std::collections::HashMap;

use clap::Args;

use smasher_attractor::dot::parser;
use smasher_attractor::engine::{Engine, EngineConfig};
use smasher_attractor::graph;
use smasher_attractor::handler::default_registry;
use smasher_attractor::state::Context;
use smasher_attractor::stylesheet::Stylesheet;
use smasher_attractor::transforms;

use crate::error::CliError;

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

    let config = EngineConfig {
        max_steps: args.max_steps,
        enable_checkpointing: false,
    };

    let registry = default_registry();
    let engine = Engine::with_config(resolved, registry, config);
    let context = Context::default();

    // Seed variables into the context.
    for (key, value) in &variables {
        context.set(key, serde_json::Value::String(value.clone()));
    }

    let result = engine.run(context).await?;

    let json = serde_json::to_string_pretty(&result.final_context)
        .map_err(|e| CliError::Other(format!("failed to serialize context: {e}")))?;
    println!("{json}");

    tracing::info!(
        steps = result.steps_taken,
        nodes_visited = result.visited_nodes.len(),
        "pipeline completed"
    );

    Ok(())
}
