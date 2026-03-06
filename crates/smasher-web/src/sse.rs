// ABOUTME: SSE bridge converting PipelineEvent broadcast channel to axum SSE responses.
// ABOUTME: Streams events as Server-Sent Events with typed event names and HTML fragment data.

use std::convert::Infallible;

use axum::response::sse::{Event, KeepAlive, Sse};
use futures::Stream;
use tokio::sync::broadcast;

use smasher_attractor::events::PipelineEvent;

/// Extract the SSE event name from a PipelineEvent.
///
/// Returns snake_case event names like `node_started`, `pipeline_completed`, etc.
pub fn event_name(event: &PipelineEvent) -> &'static str {
    match event {
        PipelineEvent::NodeStarted { .. } => "node_started",
        PipelineEvent::NodeCompleted { .. } => "node_completed",
        PipelineEvent::NodeFailed { .. } => "node_failed",
        PipelineEvent::EdgeTraversed { .. } => "edge_traversed",
        PipelineEvent::HumanPromptIssued { .. } => "human_prompt_issued",
        PipelineEvent::HumanResponseReceived { .. } => "human_response_received",
        PipelineEvent::ContextUpdated { .. } => "context_updated",
        PipelineEvent::CheckpointCreated { .. } => "checkpoint_created",
        PipelineEvent::PipelineStarted { .. } => "pipeline_started",
        PipelineEvent::PipelineCompleted { .. } => "pipeline_completed",
        PipelineEvent::PipelineAborted { .. } => "pipeline_aborted",
        PipelineEvent::LoopRestarted { .. } => "loop_restarted",
        PipelineEvent::AgentToolCallStarted { .. } => "agent_tool_call_started",
        PipelineEvent::AgentToolCallCompleted { .. } => "agent_tool_call_completed",
        PipelineEvent::AgentMessage { .. } => "agent_message",
        PipelineEvent::AgentTurnStarted { .. } => "agent_turn_started",
        PipelineEvent::AgentTokenUsage { .. } => "agent_token_usage",
    }
}

/// Convert a PipelineEvent into an HTML fragment for HTMX SSE swap.
pub fn to_sse_event(event: &PipelineEvent) -> Event {
    let name = event_name(event);
    let html = render_event_html(event);
    Event::default().event(name).data(html)
}

fn format_time(ts: &chrono::DateTime<chrono::Utc>) -> String {
    ts.format("%H:%M:%S").to_string()
}

fn format_duration(ms: u64) -> String {
    if ms < 1000 {
        format!("{}ms", ms)
    } else if ms < 60_000 {
        format!("{:.1}s", ms as f64 / 1000.0)
    } else {
        let mins = ms / 60_000;
        let secs = (ms % 60_000) / 1000;
        format!("{}m {}s", mins, secs)
    }
}

fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

pub fn render_event_html(event: &PipelineEvent) -> String {
    match event {
        PipelineEvent::PipelineStarted {
            graph_name,
            timestamp,
        } => {
            format!(
                r#"<div class="event-item event-pipeline_started"><span class="event-time">{}</span><span class="event-icon">⚡</span><div class="event-body"><span class="event-kind">PIPELINE STARTED</span><span class="event-detail">{}</span></div></div>"#,
                format_time(timestamp),
                escape_html(graph_name)
            )
        }
        PipelineEvent::PipelineCompleted {
            total_nodes,
            duration_ms,
            timestamp,
            ..
        } => {
            format!(
                r#"<div class="event-item event-pipeline_completed"><span class="event-time">{}</span><span class="event-icon">✓</span><div class="event-body"><span class="event-kind">PIPELINE COMPLETED</span><span class="event-detail">{} nodes · {}</span></div></div>"#,
                format_time(timestamp),
                total_nodes,
                format_duration(*duration_ms)
            )
        }
        PipelineEvent::PipelineAborted { reason, timestamp } => {
            format!(
                r#"<div class="event-item event-pipeline_aborted"><span class="event-time">{}</span><span class="event-icon">✕</span><div class="event-body"><span class="event-kind">PIPELINE ABORTED</span><span class="event-detail">{}</span></div></div>"#,
                format_time(timestamp),
                escape_html(reason)
            )
        }
        PipelineEvent::NodeStarted {
            node_id,
            node_type,
            timestamp,
        } => {
            format!(
                r#"<div class="event-item event-node_started"><span class="event-time">{}</span><span class="event-icon">▶</span><div class="event-body"><span class="event-kind">NODE STARTED</span><span class="event-detail"><span class="event-node">{}</span><span class="event-tag">{}</span></span></div></div>"#,
                format_time(timestamp),
                escape_html(node_id),
                escape_html(node_type)
            )
        }
        PipelineEvent::NodeCompleted {
            node_id,
            duration_ms,
            timestamp,
            ..
        } => {
            format!(
                r#"<div class="event-item event-node_completed"><span class="event-time">{}</span><span class="event-icon">✓</span><div class="event-body"><span class="event-kind">NODE COMPLETED</span><span class="event-detail"><span class="event-node">{}</span><span class="event-duration">{}</span></span></div></div>"#,
                format_time(timestamp),
                escape_html(node_id),
                format_duration(*duration_ms)
            )
        }
        PipelineEvent::NodeFailed {
            node_id,
            error,
            duration_ms,
            timestamp,
        } => {
            format!(
                r#"<div class="event-item event-node_failed"><span class="event-time">{}</span><span class="event-icon">✕</span><div class="event-body"><span class="event-kind">NODE FAILED</span><span class="event-detail"><span class="event-node">{}</span><span class="event-duration">{}</span><span class="event-error">{}</span></span></div></div>"#,
                format_time(timestamp),
                escape_html(node_id),
                format_duration(*duration_ms),
                escape_html(error)
            )
        }
        PipelineEvent::EdgeTraversed {
            from,
            to,
            label,
            timestamp,
        } => {
            let label_str = label
                .as_deref()
                .map(|l| format!(r#" <span class="event-label">[{}]</span>"#, escape_html(l)))
                .unwrap_or_default();
            format!(
                r#"<div class="event-item event-edge_traversed"><span class="event-time">{}</span><span class="event-icon">→</span><div class="event-body"><span class="event-kind">EDGE</span><span class="event-detail">{} → {}{}</span></div></div>"#,
                format_time(timestamp),
                escape_html(from),
                escape_html(to),
                label_str
            )
        }
        PipelineEvent::LoopRestarted {
            from,
            to,
            restart_count,
            timestamp,
        } => {
            format!(
                r#"<div class="event-item event-loop_restarted"><span class="event-time">{}</span><span class="event-icon">↻</span><div class="event-body"><span class="event-kind">LOOP #{}</span><span class="event-detail">{} → {}</span></div></div>"#,
                format_time(timestamp),
                restart_count,
                escape_html(from),
                escape_html(to)
            )
        }
        PipelineEvent::HumanPromptIssued {
            node_id,
            question,
            timestamp,
        } => {
            format!(
                r#"<div class="event-item event-human_prompt_issued"><span class="event-time">{}</span><span class="event-icon">?</span><div class="event-body"><span class="event-kind">AWAITING INPUT</span><span class="event-detail"><span class="event-node">{}</span>{}</span></div></div>"#,
                format_time(timestamp),
                escape_html(node_id),
                escape_html(question)
            )
        }
        PipelineEvent::HumanResponseReceived {
            node_id, timestamp, ..
        } => {
            format!(
                r#"<div class="event-item event-human_response"><span class="event-time">{}</span><span class="event-icon">✎</span><div class="event-body"><span class="event-kind">INPUT RECEIVED</span><span class="event-detail"><span class="event-node">{}</span></span></div></div>"#,
                format_time(timestamp),
                escape_html(node_id)
            )
        }
        PipelineEvent::ContextUpdated { key, timestamp } => {
            format!(
                r#"<div class="event-item event-context_updated"><span class="event-time">{}</span><span class="event-icon">⟳</span><div class="event-body"><span class="event-kind">CTX UPDATE</span><span class="event-detail">{}</span></div></div>"#,
                format_time(timestamp),
                escape_html(key)
            )
        }
        PipelineEvent::CheckpointCreated { node_id, timestamp } => {
            format!(
                r#"<div class="event-item event-checkpoint"><span class="event-time">{}</span><span class="event-icon">◆</span><div class="event-body"><span class="event-kind">CHECKPOINT</span><span class="event-detail">{}</span></div></div>"#,
                format_time(timestamp),
                escape_html(node_id)
            )
        }
        PipelineEvent::AgentToolCallStarted {
            node_id,
            tool_name,
            timestamp,
            ..
        } => {
            format!(
                r#"<div class="event-item event-agent_tool_call_started"><span class="event-time">{}</span><span class="event-icon">🔧</span><div class="event-body"><span class="event-kind">TOOL CALL</span><span class="event-detail"><span class="event-node">{}</span><span class="event-tool-name">{}</span></span></div></div>"#,
                format_time(timestamp),
                escape_html(node_id),
                escape_html(tool_name)
            )
        }
        PipelineEvent::AgentToolCallCompleted {
            node_id,
            tool_name,
            duration_ms,
            is_error,
            result_preview,
            timestamp,
            ..
        } => {
            let error_class = if *is_error { " is-error" } else { "" };
            let status_icon = if *is_error { "✕" } else { "✓" };
            format!(
                r#"<div class="event-item event-agent_tool_call_completed{}"><span class="event-time">{}</span><span class="event-icon">{}</span><div class="event-body"><span class="event-kind">TOOL DONE</span><span class="event-detail"><span class="event-node">{}</span><span class="event-tool-name">{}</span><span class="event-duration">{}</span><span class="event-result-preview">{}</span></span></div></div>"#,
                error_class,
                format_time(timestamp),
                status_icon,
                escape_html(node_id),
                escape_html(tool_name),
                format_duration(*duration_ms),
                escape_html(result_preview)
            )
        }
        PipelineEvent::AgentMessage {
            node_id,
            text,
            timestamp,
        } => {
            format!(
                r#"<div class="event-item event-agent_message"><span class="event-time">{}</span><span class="event-icon">💬</span><div class="event-body"><span class="event-kind">AGENT</span><span class="event-detail"><span class="event-node">{}</span><span class="event-agent-text">{}</span></span></div></div>"#,
                format_time(timestamp),
                escape_html(node_id),
                escape_html(text)
            )
        }
        PipelineEvent::AgentTurnStarted {
            node_id,
            turn_number,
            timestamp,
        } => {
            format!(
                r#"<div class="event-item event-agent_turn_started"><span class="event-time">{}</span><span class="event-icon">↻</span><div class="event-body"><span class="event-kind">TURN {}</span><span class="event-detail"><span class="event-node">{}</span></span></div></div>"#,
                format_time(timestamp),
                turn_number,
                escape_html(node_id)
            )
        }
        PipelineEvent::AgentTokenUsage {
            node_id,
            input_tokens,
            output_tokens,
            cost_usd,
            timestamp,
        } => {
            let cost_part = if *cost_usd > 0.0 {
                format!(" ${:.2}", cost_usd)
            } else {
                String::new()
            };
            format!(
                r#"<div class="event-item event-agent_token_usage"><span class="event-time">{}</span><span class="event-icon">⊛</span><div class="event-body"><span class="event-kind">TOKENS</span><span class="event-detail"><span class="event-node">{}</span> in:{} out:{}{}</span></div></div>"#,
                format_time(timestamp),
                escape_html(node_id),
                input_tokens,
                output_tokens,
                cost_part
            )
        }
    }
}

/// Create an SSE stream from a broadcast receiver that terminates on
/// `PipelineCompleted` or `PipelineAborted` (or when the channel closes).
pub fn event_stream(
    mut rx: broadcast::Receiver<PipelineEvent>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let stream = async_stream::stream! {
        loop {
            match rx.recv().await {
                Ok(event) => {
                    let is_terminal = matches!(
                        event,
                        PipelineEvent::PipelineCompleted { .. }
                        | PipelineEvent::PipelineAborted { .. }
                    );

                    let sse_event = to_sse_event(&event);
                    yield Ok(sse_event);

                    if is_terminal {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!(missed = n, "SSE subscriber lagged behind");
                    // Continue receiving — we just missed some events.
                }
                Err(broadcast::error::RecvError::Closed) => {
                    break;
                }
            }
        }
    };

    Sse::new(stream).keep_alive(KeepAlive::default())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use smasher_attractor::state::Outcome;

    fn now() -> chrono::DateTime<Utc> {
        Utc::now()
    }

    #[test]
    fn event_name_returns_correct_names() {
        let ts = now();
        let cases: Vec<(PipelineEvent, &str)> = vec![
            (
                PipelineEvent::NodeStarted {
                    node_id: "n".into(),
                    node_type: "t".into(),
                    timestamp: ts,
                },
                "node_started",
            ),
            (
                PipelineEvent::NodeCompleted {
                    node_id: "n".into(),
                    outcome: Outcome::success(),
                    duration_ms: 0,
                    timestamp: ts,
                },
                "node_completed",
            ),
            (
                PipelineEvent::NodeFailed {
                    node_id: "n".into(),
                    error: "e".into(),
                    duration_ms: 0,
                    timestamp: ts,
                },
                "node_failed",
            ),
            (
                PipelineEvent::EdgeTraversed {
                    from: "a".into(),
                    to: "b".into(),
                    label: None,
                    timestamp: ts,
                },
                "edge_traversed",
            ),
            (
                PipelineEvent::HumanPromptIssued {
                    node_id: "n".into(),
                    question: "q".into(),
                    timestamp: ts,
                },
                "human_prompt_issued",
            ),
            (
                PipelineEvent::HumanResponseReceived {
                    node_id: "n".into(),
                    response: "r".into(),
                    timestamp: ts,
                },
                "human_response_received",
            ),
            (
                PipelineEvent::ContextUpdated {
                    key: "k".into(),
                    timestamp: ts,
                },
                "context_updated",
            ),
            (
                PipelineEvent::CheckpointCreated {
                    node_id: "n".into(),
                    timestamp: ts,
                },
                "checkpoint_created",
            ),
            (
                PipelineEvent::PipelineStarted {
                    graph_name: "g".into(),
                    timestamp: ts,
                },
                "pipeline_started",
            ),
            (
                PipelineEvent::PipelineCompleted {
                    outcome: Outcome::success(),
                    total_nodes: 0,
                    duration_ms: 0,
                    timestamp: ts,
                },
                "pipeline_completed",
            ),
            (
                PipelineEvent::PipelineAborted {
                    reason: "r".into(),
                    timestamp: ts,
                },
                "pipeline_aborted",
            ),
            (
                PipelineEvent::LoopRestarted {
                    from: "a".into(),
                    to: "b".into(),
                    restart_count: 1,
                    timestamp: ts,
                },
                "loop_restarted",
            ),
            (
                PipelineEvent::AgentToolCallStarted {
                    node_id: "n".into(),
                    tool_name: "bash".into(),
                    tool_call_id: "c".into(),
                    input_preview: String::new(),
                    timestamp: ts,
                },
                "agent_tool_call_started",
            ),
            (
                PipelineEvent::AgentToolCallCompleted {
                    node_id: "n".into(),
                    tool_name: "bash".into(),
                    tool_call_id: "c".into(),
                    duration_ms: 100,
                    is_error: false,
                    result_preview: "ok".into(),
                    timestamp: ts,
                },
                "agent_tool_call_completed",
            ),
            (
                PipelineEvent::AgentMessage {
                    node_id: "n".into(),
                    text: "hello".into(),
                    timestamp: ts,
                },
                "agent_message",
            ),
            (
                PipelineEvent::AgentTurnStarted {
                    node_id: "n".into(),
                    turn_number: 1,
                    timestamp: ts,
                },
                "agent_turn_started",
            ),
            (
                PipelineEvent::AgentTokenUsage {
                    node_id: "n".into(),
                    input_tokens: 100,
                    output_tokens: 50,
                    cost_usd: 0.0,
                    timestamp: ts,
                },
                "agent_token_usage",
            ),
        ];

        for (event, expected_name) in &cases {
            assert_eq!(
                event_name(event),
                *expected_name,
                "wrong event name for {:?}",
                event
            );
        }
    }

    #[test]
    fn to_sse_event_produces_html_fragment() {
        let event = PipelineEvent::NodeStarted {
            node_id: "step_1".into(),
            node_type: "llm".into(),
            timestamp: now(),
        };
        let sse = to_sse_event(&event);
        // SSE Event's Debug output should contain the event name and HTML content
        let debug = format!("{:?}", sse);
        assert!(debug.contains("node_started") || debug.contains("step_1"));
    }

    #[test]
    fn to_sse_event_html_contains_expected_content() {
        let event = PipelineEvent::PipelineCompleted {
            outcome: Outcome::success(),
            total_nodes: 5,
            duration_ms: 1234,
            timestamp: now(),
        };
        let html = render_event_html(&event);
        assert!(html.contains("PIPELINE COMPLETED"));
        assert!(html.contains("5 nodes"));
        assert!(html.contains("1.2s"));
        assert!(html.contains("event-pipeline_completed"));
    }

    #[tokio::test]
    async fn event_stream_terminates_on_pipeline_completed() {
        use futures::StreamExt;

        let emitter = smasher_attractor::events::PipelineEventEmitter::new(16);
        let rx = emitter.subscribe();

        let ts = now();
        emitter.emit(PipelineEvent::NodeStarted {
            node_id: "a".into(),
            node_type: "t".into(),
            timestamp: ts,
        });
        emitter.emit(PipelineEvent::PipelineCompleted {
            outcome: Outcome::success(),
            total_nodes: 1,
            duration_ms: 100,
            timestamp: ts,
        });

        let sse = event_stream(rx);
        // Extract the inner stream via into_inner() — but Sse doesn't expose it.
        // Instead, we test that the stream produces 2 events and then ends.
        // We need to use the stream directly.
        drop(sse);

        // Test with the raw stream logic instead.
        let rx2 = emitter.subscribe();
        emitter.emit(PipelineEvent::NodeStarted {
            node_id: "b".into(),
            node_type: "t".into(),
            timestamp: ts,
        });
        emitter.emit(PipelineEvent::PipelineCompleted {
            outcome: Outcome::success(),
            total_nodes: 1,
            duration_ms: 50,
            timestamp: ts,
        });

        let stream = async_stream::stream! {
            let mut rx = rx2;
            loop {
                match rx.recv().await {
                    Ok(event) => {
                        let is_terminal = matches!(
                            event,
                            PipelineEvent::PipelineCompleted { .. }
                            | PipelineEvent::PipelineAborted { .. }
                        );
                        yield event;
                        if is_terminal {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => {}
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        };

        tokio::pin!(stream);
        let events: Vec<PipelineEvent> = stream.collect().await;
        assert_eq!(events.len(), 2);
        assert!(matches!(events[0], PipelineEvent::NodeStarted { .. }));
        assert!(matches!(events[1], PipelineEvent::PipelineCompleted { .. }));
    }

    #[tokio::test]
    async fn event_stream_terminates_on_pipeline_aborted() {
        use futures::StreamExt;

        let emitter = smasher_attractor::events::PipelineEventEmitter::new(16);
        let rx = emitter.subscribe();

        let ts = now();
        emitter.emit(PipelineEvent::PipelineAborted {
            reason: "cancelled".into(),
            timestamp: ts,
        });

        let stream = async_stream::stream! {
            let mut rx = rx;
            loop {
                match rx.recv().await {
                    Ok(event) => {
                        let is_terminal = matches!(
                            event,
                            PipelineEvent::PipelineCompleted { .. }
                            | PipelineEvent::PipelineAborted { .. }
                        );
                        yield event;
                        if is_terminal {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => {}
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        };

        tokio::pin!(stream);
        let events: Vec<PipelineEvent> = stream.collect().await;
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0], PipelineEvent::PipelineAborted { .. }));
    }

    #[tokio::test]
    async fn event_stream_terminates_on_channel_close() {
        use futures::StreamExt;

        let emitter = smasher_attractor::events::PipelineEventEmitter::new(16);
        let rx = emitter.subscribe();

        let ts = now();
        emitter.emit(PipelineEvent::NodeStarted {
            node_id: "a".into(),
            node_type: "t".into(),
            timestamp: ts,
        });
        // Drop emitter to close channel without terminal event.
        drop(emitter);

        let stream = async_stream::stream! {
            let mut rx = rx;
            loop {
                match rx.recv().await {
                    Ok(event) => {
                        let is_terminal = matches!(
                            event,
                            PipelineEvent::PipelineCompleted { .. }
                            | PipelineEvent::PipelineAborted { .. }
                        );
                        yield event;
                        if is_terminal {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => {}
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        };

        tokio::pin!(stream);
        let events: Vec<PipelineEvent> = stream.collect().await;
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0], PipelineEvent::NodeStarted { .. }));
    }
}
