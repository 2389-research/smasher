// ABOUTME: Entry point for the smasher CLI binary with nine subcommands.
// ABOUTME: Routes to complete, chat, run, resume, render, serve, ingest, archive, and lint.

mod archive;
mod chat;
#[cfg(test)]
mod cli_spec;
mod complete;
mod error;
mod gitutil;
mod ingest;
#[cfg(test)]
mod layout_check;
mod lint;
mod llm_backends;
mod render;
mod resume;
mod run;
mod serve;
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

    /// Resume a checkpointed pipeline run.
    Resume(resume::ResumeArgs),

    /// Render a DOT pipeline file to SVG or PNG.
    Render(render::RenderArgs),

    /// Start the web dashboard server.
    Serve(serve::ServeArgs),

    /// Convert English requirements into a DOT pipeline file using an LLM.
    Ingest(ingest::IngestArgs),

    /// Create a compressed archive of a run directory.
    Archive(archive::ArchiveArgs),

    /// Validate a DOT pipeline file with lint rules.
    Lint(lint::LintArgs),
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
    // The serve command defaults to info level since the server should show
    // startup, request, and error logs. Other commands default to warn.
    let filter = if cli.verbose {
        "debug"
    } else if matches!(cli.command, Command::Serve(_)) {
        "smasher_web=info,smasher_attractor=info,smasher_agent=info,smasher_llm=info,tower_http=info"
    } else {
        "warn"
    };
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
            Command::Resume(args) => resume::run(args).await,
            Command::Render(args) => render::run(args).await,
            Command::Serve(args) => serve::run(args).await,
            Command::Ingest(args) => ingest::run(args).await,
            Command::Archive(args) => async { archive::run(args) }.await,
            Command::Lint(args) => lint::run(args).await,
        }
    });

    if let Err(e) = result {
        eprintln!("error: {e}");
        std::process::exit(e.exit_code());
    }
}
