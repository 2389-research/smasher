// ABOUTME: Render subcommand that converts a DOT pipeline file to SVG or PNG output.
// ABOUTME: Uses the smasher-attractor rendering pipeline with optional caching.

use std::collections::HashMap;

use clap::Args;

use smasher_attractor::dot::parser;
use smasher_attractor::graph;
use smasher_attractor::rendering::{
    CachedRenderer, GraphRenderer, GraphvizRenderer, RenderFormat, RenderOutput,
};
use smasher_attractor::stylesheet::Stylesheet;
use smasher_attractor::transforms;

use crate::error::CliError;

/// Render a DOT pipeline file to DOT, SVG, or PNG.
#[derive(Debug, Args)]
pub struct RenderArgs {
    /// Path to the DOT pipeline file.
    #[arg()]
    pub pipeline: String,

    /// Output format: dot, svg, or png.
    #[arg(long, short, default_value = "svg")]
    pub format: String,

    /// Output file path. If omitted, writes to stdout.
    #[arg(long, short)]
    pub output: Option<String>,

    /// Variable assignments (key=value), repeatable. Applied via stylesheet transforms.
    #[arg(long = "var", value_name = "KEY=VALUE")]
    pub vars: Vec<String>,

    /// Path to a stylesheet file for graph transforms.
    #[arg(long)]
    pub stylesheet: Option<String>,
}

pub async fn run(args: RenderArgs) -> Result<(), CliError> {
    let format = RenderFormat::from_str_loose(&args.format).ok_or_else(|| {
        CliError::Other(format!(
            "unsupported render format '{}': expected dot, svg, or png",
            args.format
        ))
    })?;

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

    // Optionally load and apply a stylesheet.
    let stylesheet = match &args.stylesheet {
        Some(path) => {
            let css_source = std::fs::read_to_string(path)?;
            Some(Stylesheet::parse(&css_source)?)
        }
        None => None,
    };

    transforms::apply_transforms(&mut resolved, &variables, stylesheet.as_ref());

    let renderer = CachedRenderer::new(GraphvizRenderer);
    let output: RenderOutput = renderer
        .render(&resolved, format)
        .await
        .map_err(|e| CliError::Other(format!("render failed: {e}")))?;

    match &args.output {
        Some(path) => {
            std::fs::write(path, &output.content)?;
            tracing::info!(format = %output.format, path = %path, "graph rendered to file");
        }
        None => {
            // Write to stdout. For text formats, write as UTF-8. For binary, write raw bytes.
            use std::io::Write;
            let stdout = std::io::stdout();
            let mut handle = stdout.lock();
            handle.write_all(&output.content)?;
            handle.flush()?;
        }
    }

    Ok(())
}
