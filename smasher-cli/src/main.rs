// ABOUTME: Entry point for the smasher CLI binary with three subcommands.
// ABOUTME: Routes to complete (one-shot), chat (interactive agent), and run (DOT pipeline).

mod chat;
#[cfg(test)]
mod cli_spec;
mod complete;
mod error;
#[cfg(test)]
mod layout_check;
mod render;
mod run;
pub mod tui;

use std::path::PathBuf;

use clap::{Parser, Subcommand};

use error::CliError;

/// Smasher — AI workflow orchestration from the command line.
#[derive(Debug, Parser)]
#[command(name = "smasher", version, about)]
struct Cli {
    /// Enable verbose logging (writes to stderr).
    #[arg(long, short, global = true)]
    verbose: bool,

    /// Load environment variables from a specific .env file.
    #[arg(long, global = true)]
    env_file: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Send a one-shot prompt to an LLM.
    Complete(complete::CompleteArgs),

    /// Start an interactive agent chat session.
    Chat(chat::ChatArgs),

    /// Execute a DOT-based pipeline.
    Run(run::RunArgs),

    /// Render a DOT pipeline file to SVG or PNG.
    Render(render::RenderArgs),
}

fn main() {
    // Load .env from current directory before clap parsing so env vars are
    // available for any default-value logic. Silently ignored if no .env exists.
    dotenvy::dotenv().ok();

    let cli = Cli::parse();

    // If the user explicitly asked for a specific env file, load it now
    // (overrides any vars already set by the default .env).
    if let Some(ref path) = cli.env_file
        && let Err(e) = dotenvy::from_path_override(path)
    {
        eprintln!("error: failed to load env file {}: {e}", path.display());
        std::process::exit(1);
    }

    // Set up tracing — verbose goes to stderr so stdout stays clean for output.
    let filter = if cli.verbose { "debug" } else { "warn" };
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(filter)),
        )
        .with_writer(std::io::stderr)
        .init();

    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("error: failed to build tokio runtime: {e}");
            std::process::exit(1);
        }
    };

    let result: Result<(), CliError> = runtime.block_on(async {
        match cli.command {
            Command::Complete(args) => complete::run(args).await,
            Command::Chat(args) => chat::run(args).await,
            Command::Run(args) => run::run(args).await,
            Command::Render(args) => render::run(args).await,
        }
    });

    if let Err(e) = result {
        eprintln!("error: {e}");
        std::process::exit(e.exit_code());
    }
}
