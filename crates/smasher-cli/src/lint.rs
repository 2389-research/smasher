// ABOUTME: Lint subcommand that validates DOT pipeline graphs for structural issues.
// ABOUTME: Runs built-in lint rules and reports diagnostics with severity-coded exit.

use std::collections::HashMap;

use clap::Args;
use smasher_attractor::dot::parser;
use smasher_attractor::graph;
use smasher_attractor::lint::{LintRunner, Severity};
use smasher_attractor::stylesheet::Stylesheet;
use smasher_attractor::transforms;

use crate::error::CliError;

/// Validate a DOT pipeline file with built-in lint rules.
#[derive(Debug, Args)]
pub struct LintArgs {
    /// Path to the DOT pipeline file.
    #[arg()]
    pub pipeline: String,

    /// Variable assignments (key=value), repeatable.
    #[arg(long = "var", value_name = "KEY=VALUE")]
    pub vars: Vec<String>,

    /// Path to a stylesheet file for graph transforms.
    #[arg(long)]
    pub stylesheet: Option<String>,
}

pub async fn run(args: LintArgs) -> Result<(), CliError> {
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

    // Apply stylesheet if provided.
    let stylesheet = match &args.stylesheet {
        Some(path) => {
            let css_source = std::fs::read_to_string(path)?;
            Some(Stylesheet::parse(&css_source)?)
        }
        None => None,
    };
    transforms::apply_transforms(&mut resolved, &variables, stylesheet.as_ref());

    let runner = LintRunner::with_builtins();
    let report = runner.run(&resolved);

    // Print diagnostics to stderr.
    for diag in &report.diagnostics {
        let severity_tag = match diag.severity {
            Severity::Error => "E",
            Severity::Warning => "W",
            Severity::Info => "I",
        };
        let node_info = diag
            .node_id
            .as_deref()
            .map(|id| format!(" (node: {id})"))
            .unwrap_or_default();
        eprintln!(
            "[{severity_tag}:{code}] {msg}{node_info}",
            code = diag.code,
            msg = diag.message
        );
        if let Some(ref suggestion) = diag.suggestion {
            eprintln!("  suggestion: {suggestion}");
        }
    }

    // Summary line.
    let error_count = report.errors().len();
    let warning_count = report.warnings().len();

    if report.is_clean() {
        eprintln!("lint: clean");
    } else {
        eprintln!("lint: {error_count} error(s), {warning_count} warning(s)");
    }

    // Exit with code 1 (CliError::Other) if any errors found.
    if report.has_errors() {
        return Err(CliError::Other(format!(
            "lint found {error_count} error(s)"
        )));
    }

    Ok(())
}

/// Run lint checks on a resolved graph, printing diagnostics to stderr.
/// Returns Ok(()) if no errors (warnings are printed but do not fail).
/// Returns Err with exit code 1 if errors are found.
pub fn lint_graph(graph: &smasher_attractor::graph::Graph) -> Result<(), CliError> {
    let runner = LintRunner::with_builtins();
    let report = runner.run(graph);

    for diag in &report.diagnostics {
        let severity_tag = match diag.severity {
            Severity::Error => "E",
            Severity::Warning => "W",
            Severity::Info => "I",
        };
        let node_info = diag
            .node_id
            .as_deref()
            .map(|id| format!(" (node: {id})"))
            .unwrap_or_default();
        eprintln!(
            "[{severity_tag}:{code}] {msg}{node_info}",
            code = diag.code,
            msg = diag.message
        );
        if let Some(ref suggestion) = diag.suggestion {
            eprintln!("  suggestion: {suggestion}");
        }
    }

    if report.has_errors() {
        let error_count = report.errors().len();
        let warning_count = report.warnings().len();
        eprintln!("lint: {error_count} error(s), {warning_count} warning(s)");
        return Err(CliError::Other(format!(
            "lint found {error_count} error(s)"
        )));
    }

    if !report.is_clean() {
        let warning_count = report.warnings().len();
        eprintln!("lint: {warning_count} warning(s)");
    }

    Ok(())
}
