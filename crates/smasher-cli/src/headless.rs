// ABOUTME: Clean headless progress output for pipeline execution without the TUI.
// ABOUTME: Subscribes to PipelineEvents and prints a linear walk through the graph to stderr.

use smasher_attractor::events::PipelineEvent;
use tokio::sync::broadcast;

/// Log pipeline events to stderr in a clean, linear format.
///
/// Shows the pipeline walk as a sequence of node executions, edge traversals,
/// and human gate interactions — one line per significant event.
pub async fn log_pipeline_events(mut rx: broadcast::Receiver<PipelineEvent>, pipeline_name: &str) {
    eprintln!("\x1b[1;36m--- {pipeline_name} ---\x1b[0m");
    eprintln!();

    loop {
        match rx.recv().await {
            Ok(event) => print_event(&event),
            Err(broadcast::error::RecvError::Closed) => break,
            Err(broadcast::error::RecvError::Lagged(n)) => {
                eprintln!("  \x1b[33m... skipped {n} events\x1b[0m");
            }
        }
    }
}

fn print_event(event: &PipelineEvent) {
    match event {
        PipelineEvent::PipelineStarted { .. } => {}

        PipelineEvent::NodeStarted {
            node_id, node_type, ..
        } => {
            let icon = match node_type.as_str() {
                "interviewer" => "\x1b[35m?\x1b[0m",
                "start" => "\x1b[32m>\x1b[0m",
                "exit" => "\x1b[32m<\x1b[0m",
                _ => "\x1b[34m*\x1b[0m",
            };
            eprintln!("  {icon} \x1b[1m{node_id}\x1b[0m");
        }

        PipelineEvent::NodeCompleted { duration_ms, .. } => {
            let secs = *duration_ms as f64 / 1000.0;
            if secs >= 1.0 {
                eprintln!("    \x1b[32mdone\x1b[0m ({secs:.1}s)");
            }
        }

        PipelineEvent::NodeFailed { error, .. } => {
            eprintln!("    \x1b[31mFAILED: {error}\x1b[0m");
        }

        PipelineEvent::EdgeTraversed { to, label, .. } => {
            if let Some(label) = label {
                eprintln!("    \x1b[2m-> {to} [{label}]\x1b[0m");
            } else {
                eprintln!("    \x1b[2m-> {to}\x1b[0m");
            }
        }

        PipelineEvent::HumanPromptIssued { question, .. } => {
            eprintln!("    \x1b[35m{question}\x1b[0m");
        }

        PipelineEvent::HumanResponseReceived { response, .. } => {
            eprintln!("    \x1b[36m> {response}\x1b[0m");
        }

        PipelineEvent::AgentToolCallStarted {
            tool_name,
            input_preview,
            ..
        } => {
            if input_preview.is_empty() {
                eprintln!("    \x1b[2m[tool] {tool_name}\x1b[0m");
            } else {
                eprintln!("    \x1b[2m[tool] {tool_name} {input_preview}\x1b[0m");
            }
        }

        PipelineEvent::AgentToolCallCompleted {
            tool_name,
            is_error,
            duration_ms,
            ..
        } => {
            let status = if *is_error {
                "\x1b[31mERR\x1b[0m"
            } else {
                "\x1b[32mok\x1b[0m"
            };
            if *duration_ms >= 1000 {
                let secs = *duration_ms as f64 / 1000.0;
                eprintln!("    \x1b[2m[tool] {tool_name} {status} ({secs:.1}s)\x1b[0m");
            }
        }

        PipelineEvent::AgentMessage { text, .. } => {
            // Show first line of agent message, truncated.
            let first_line = text.lines().next().unwrap_or("");
            let truncated = if first_line.len() > 120 {
                format!("{}...", &first_line[..117])
            } else {
                first_line.to_string()
            };
            if !truncated.is_empty() {
                eprintln!("    \x1b[2m{truncated}\x1b[0m");
            }
        }

        PipelineEvent::AgentTurnStarted { turn_number, .. } => {
            if *turn_number > 1 {
                eprintln!("    \x1b[2mturn {turn_number}\x1b[0m");
            }
        }

        PipelineEvent::PipelineCompleted {
            total_nodes,
            duration_ms,
            ..
        } => {
            let secs = *duration_ms as f64 / 1000.0;
            eprintln!();
            eprintln!("\x1b[1;32m--- done ({total_nodes} nodes, {secs:.1}s) ---\x1b[0m");
        }

        PipelineEvent::PipelineAborted { reason, .. } => {
            eprintln!();
            eprintln!("\x1b[1;31m--- aborted: {reason} ---\x1b[0m");
        }

        _ => {}
    }
}
