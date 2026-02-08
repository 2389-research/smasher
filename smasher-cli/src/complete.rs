// ABOUTME: One-shot LLM completion subcommand that streams text deltas to stdout.
// ABOUTME: Supports model selection, sampling params, system prompt, and JSON output mode.

use std::io::Write;

use clap::Args;
use futures::StreamExt;

use crate::error::CliError;

/// Send a single prompt to an LLM and print the response.
#[derive(Debug, Args)]
pub struct CompleteArgs {
    /// The prompt to send (positional). Omit if using --file.
    #[arg()]
    pub prompt: Option<String>,

    /// Read the prompt from a file instead.
    #[arg(long)]
    pub file: Option<String>,

    /// Model identifier (e.g. "claude-sonnet-4-20250514", "gpt-4o").
    #[arg(long, default_value = "claude-sonnet-4-20250514")]
    pub model: String,

    /// Maximum tokens to generate.
    #[arg(long)]
    pub max_tokens: Option<u32>,

    /// Sampling temperature (0.0 - 2.0).
    #[arg(long)]
    pub temperature: Option<f32>,

    /// System prompt to prepend.
    #[arg(long)]
    pub system: Option<String>,

    /// Output the full Response as pretty-printed JSON (non-streaming).
    #[arg(long)]
    pub json: bool,
}

pub async fn run(args: CompleteArgs) -> Result<(), CliError> {
    let prompt = match (&args.prompt, &args.file) {
        (Some(p), _) => p.clone(),
        (None, Some(path)) => std::fs::read_to_string(path)?,
        (None, None) => {
            return Err(CliError::Other(
                "provide a prompt argument or --file".into(),
            ));
        }
    };

    let client = smasher_llm::client::Client::from_env();
    if client.registered_providers().is_empty() {
        return Err(CliError::Other(
            "no API keys found. Set ANTHROPIC_API_KEY, OPENAI_API_KEY, or GEMINI_API_KEY.".into(),
        ));
    }

    let messages = vec![smasher_llm::types::Message::user(&prompt)];
    let mut request = smasher_llm::types::Request::new(&args.model, messages);

    if let Some(system) = &args.system {
        request = request.system_prompt(system);
    }
    if let Some(max_tokens) = args.max_tokens {
        request = request.max_tokens(max_tokens);
    }
    if let Some(temperature) = args.temperature {
        request = request.temperature(temperature);
    }

    if args.json {
        let response = client.complete(request).await?;
        let json = serde_json::to_string_pretty(&response)
            .map_err(|e| CliError::Other(format!("failed to serialize response: {e}")))?;
        println!("{json}");
    } else {
        let stream_result = smasher_llm::api::stream::stream(&client, &request).await?;
        let mut events = stream_result.events;
        let mut accumulator = stream_result.accumulator;
        let mut stdout = std::io::stdout().lock();

        while let Some(event_result) = events.next().await {
            let event = event_result?;
            accumulator.process_event(&event);

            if let Some(ref delta) = event.text_delta {
                write!(stdout, "{delta}")?;
                stdout.flush()?;
            }
        }

        // Trailing newline after streamed output.
        writeln!(stdout)?;

        let response = accumulator.into_response();
        tracing::debug!(
            input_tokens = response.usage.input_tokens,
            output_tokens = response.usage.output_tokens,
            "completion finished"
        );
    }

    Ok(())
}
