// ABOUTME: SSE-to-StreamEvent translation for Anthropic's streaming Messages API.
// ABOUTME: Tracks content block state across deltas to produce unified stream events.

use std::collections::HashMap;
use std::pin::Pin;

use futures::Stream;

use crate::types::{Error, FinishReason, StreamEvent, Usage};
use crate::util::sse::SseEvent;

use super::types::{AnthropicContentBlock, AnthropicDelta, AnthropicStreamEvent};

/// State tracked for each in-progress content block during streaming.
#[derive(Debug, Clone)]
struct BlockState {
    /// For tool_use blocks, the tool call ID assigned at content_block_start.
    tool_call_id: Option<String>,
}

/// Translate a stream of SSE events from the Anthropic API into unified `StreamEvent`s.
///
/// Maintains state across content blocks to correctly pair deltas with their
/// originating block (text, tool_use, or thinking).
pub fn translate_stream(
    sse_stream: Pin<Box<dyn Stream<Item = Result<SseEvent, Error>> + Send>>,
) -> Pin<Box<dyn Stream<Item = Result<StreamEvent, Error>> + Send>> {
    use async_stream::try_stream;
    use futures::StreamExt;

    Box::pin(try_stream! {
        let mut sse_stream = std::pin::pin!(sse_stream);
        let mut block_states: HashMap<u32, BlockState> = HashMap::new();
        let mut stop_reason: Option<FinishReason> = None;
        let mut final_usage: Option<Usage> = None;

        while let Some(sse_result) = sse_stream.next().await {
            let sse_event = sse_result?;

            // Skip the [DONE] sentinel if present.
            if sse_event.data.trim() == "[DONE]" {
                continue;
            }

            let anthropic_event: AnthropicStreamEvent =
                serde_json::from_str(&sse_event.data).map_err(|e| Error::ResponseParse {
                    provider: "anthropic".into(),
                    message: format!("failed to parse stream event: {e}"),
                })?;

            match anthropic_event {
                AnthropicStreamEvent::MessageStart { message } => {
                    yield StreamEvent::start(&message.id, &message.model);

                    // Emit initial usage if present.
                    let usage = super::types::convert_usage(&message.usage);
                    if usage.input_tokens > 0 || usage.output_tokens > 0 {
                        yield StreamEvent::usage_delta(usage);
                    }
                }

                AnthropicStreamEvent::ContentBlockStart {
                    index,
                    content_block,
                } => {
                    match &content_block {
                        AnthropicContentBlock::Text { .. } => {
                            block_states.insert(
                                index,
                                BlockState {
                                    tool_call_id: None,
                                },
                            );
                        }
                        AnthropicContentBlock::ToolUse { id, name, .. } => {
                            block_states.insert(
                                index,
                                BlockState {
                                    tool_call_id: Some(id.clone()),
                                },
                            );
                            // Emit the initial tool call delta with the name.
                            yield StreamEvent::tool_call_delta(
                                id.clone(),
                                Some(name.clone()),
                                "",
                            )
                            .with_content_index(index);
                        }
                        AnthropicContentBlock::Thinking { .. } => {
                            block_states.insert(
                                index,
                                BlockState {
                                    tool_call_id: None,
                                },
                            );
                        }
                        _ => {
                            // Image, ToolResult blocks are not expected in stream starts.
                        }
                    }
                }

                AnthropicStreamEvent::ContentBlockDelta { index, delta } => {
                    match delta {
                        AnthropicDelta::TextDelta { text } => {
                            yield StreamEvent::text_delta(text).with_content_index(index);
                        }

                        AnthropicDelta::InputJsonDelta { partial_json } => {
                            // Look up the tool_call_id from block state.
                            let tool_call_id = block_states
                                .get(&index)
                                .and_then(|s| s.tool_call_id.clone())
                                .unwrap_or_default();
                            yield StreamEvent::tool_call_delta(
                                tool_call_id,
                                None,
                                partial_json,
                            )
                            .with_content_index(index);
                        }

                        AnthropicDelta::ThinkingDelta { thinking } => {
                            yield StreamEvent::thinking_delta(thinking).with_content_index(index);
                        }

                        AnthropicDelta::SignatureDelta { .. } => {
                            // Signature deltas are metadata; no unified event needed.
                        }
                    }
                }

                AnthropicStreamEvent::ContentBlockStop { index } => {
                    block_states.remove(&index);
                }

                AnthropicStreamEvent::MessageDelta { delta, usage } => {
                    if let Some(reason) = delta.stop_reason.as_deref() {
                        stop_reason = Some(super::types::map_stop_reason(reason));
                    }
                    if let Some(u) = usage {
                        let converted = super::types::convert_usage(&u);
                        final_usage = Some(converted.clone());
                        yield StreamEvent::usage_delta(converted);
                    }
                }

                AnthropicStreamEvent::MessageStop => {
                    yield StreamEvent::end(stop_reason.take(), final_usage.take());
                }

                AnthropicStreamEvent::Error { error } => {
                    yield StreamEvent::error(format!("{}: {}", error.error_type, error.message));
                }

                AnthropicStreamEvent::Ping => {
                    // Heartbeat; skip.
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::StreamEventType;
    use crate::util::sse::SseEvent;
    use futures::StreamExt;
    use futures::stream;
    use serde_json::json;

    /// Helper to create an SSE event with given JSON data.
    fn sse(data: &str) -> Result<SseEvent, Error> {
        Ok(SseEvent {
            event_type: "message".into(),
            data: data.to_string(),
            id: None,
        })
    }

    /// Collect all stream events from the translated stream.
    async fn collect_events(events: Vec<Result<SseEvent, Error>>) -> Vec<StreamEvent> {
        let sse_stream: Pin<Box<dyn Stream<Item = Result<SseEvent, Error>> + Send>> =
            Box::pin(stream::iter(events));
        let translated = translate_stream(sse_stream);
        translated
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .map(|r| r.expect("unexpected error"))
            .collect()
    }

    #[tokio::test]
    async fn translate_message_start() {
        let events = vec![sse(&json!({
            "type": "message_start",
            "message": {
                "id": "msg_01",
                "model": "claude-sonnet-4-20250514",
                "content": [],
                "stop_reason": null,
                "usage": {"input_tokens": 10, "output_tokens": 0}
            }
        })
        .to_string())];
        let result = collect_events(events).await;

        assert_eq!(result.len(), 2); // start + usage_delta
        assert_eq!(result[0].event_type, StreamEventType::Start);
        assert_eq!(result[0].response_id.as_deref(), Some("msg_01"));
        assert_eq!(result[0].model.as_deref(), Some("claude-sonnet-4-20250514"));
        assert_eq!(result[1].event_type, StreamEventType::UsageDelta);
    }

    #[tokio::test]
    async fn translate_text_content_delta() {
        let events = vec![
            sse(&json!({
                "type": "content_block_start",
                "index": 0,
                "content_block": {"type": "text", "text": ""}
            })
            .to_string()),
            sse(&json!({
                "type": "content_block_delta",
                "index": 0,
                "delta": {"type": "text_delta", "text": "Hello "}
            })
            .to_string()),
            sse(&json!({
                "type": "content_block_delta",
                "index": 0,
                "delta": {"type": "text_delta", "text": "world!"}
            })
            .to_string()),
        ];
        let result = collect_events(events).await;

        assert_eq!(result.len(), 2);
        assert_eq!(result[0].event_type, StreamEventType::ContentDelta);
        assert_eq!(result[0].text_delta.as_deref(), Some("Hello "));
        assert_eq!(result[1].text_delta.as_deref(), Some("world!"));
    }

    #[tokio::test]
    async fn translate_tool_use_stream() {
        let events = vec![
            sse(&json!({
                "type": "content_block_start",
                "index": 0,
                "content_block": {
                    "type": "tool_use",
                    "id": "toolu_123",
                    "name": "get_weather",
                    "input": {}
                }
            })
            .to_string()),
            sse(&json!({
                "type": "content_block_delta",
                "index": 0,
                "delta": {"type": "input_json_delta", "partial_json": "{\"loc"}
            })
            .to_string()),
            sse(&json!({
                "type": "content_block_delta",
                "index": 0,
                "delta": {"type": "input_json_delta", "partial_json": "ation\":\"NYC\"}"}
            })
            .to_string()),
            sse(&json!({
                "type": "content_block_stop",
                "index": 0
            })
            .to_string()),
        ];
        let result = collect_events(events).await;

        // Should have: initial tool_call_delta (with name), 2 argument deltas.
        assert_eq!(result.len(), 3);
        assert_eq!(result[0].event_type, StreamEventType::ToolCallDelta);
        assert_eq!(result[0].tool_name.as_deref(), Some("get_weather"));
        assert_eq!(result[0].tool_call_id.as_deref(), Some("toolu_123"));

        assert_eq!(result[1].event_type, StreamEventType::ToolCallDelta);
        assert_eq!(result[1].tool_call_id.as_deref(), Some("toolu_123"));
        assert!(result[1].tool_name.is_none());
        assert_eq!(result[1].arguments_delta.as_deref(), Some("{\"loc"));

        assert_eq!(
            result[2].arguments_delta.as_deref(),
            Some("ation\":\"NYC\"}")
        );
    }

    #[tokio::test]
    async fn translate_thinking_delta() {
        let events = vec![
            sse(&json!({
                "type": "content_block_start",
                "index": 0,
                "content_block": {"type": "thinking", "thinking": "", "signature": ""}
            })
            .to_string()),
            sse(&json!({
                "type": "content_block_delta",
                "index": 0,
                "delta": {"type": "thinking_delta", "thinking": "Let me think..."}
            })
            .to_string()),
        ];
        let result = collect_events(events).await;

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].event_type, StreamEventType::ThinkingDelta);
        assert_eq!(result[0].thinking_delta.as_deref(), Some("Let me think..."));
    }

    #[tokio::test]
    async fn translate_message_delta_and_stop() {
        let events = vec![
            sse(&json!({
                "type": "message_delta",
                "delta": {"stop_reason": "end_turn"},
                "usage": {"input_tokens": 0, "output_tokens": 42}
            })
            .to_string()),
            sse(&json!({"type": "message_stop"}).to_string()),
        ];
        let result = collect_events(events).await;

        assert_eq!(result.len(), 2); // usage_delta + end
        assert_eq!(result[0].event_type, StreamEventType::UsageDelta);
        let u = result[0].usage.as_ref().unwrap();
        assert_eq!(u.output_tokens, 42);

        assert_eq!(result[1].event_type, StreamEventType::End);
        assert_eq!(result[1].finish_reason, Some(FinishReason::Stop));
    }

    #[tokio::test]
    async fn translate_error_event() {
        let events = vec![sse(&json!({
            "type": "error",
            "error": {
                "type": "overloaded_error",
                "message": "Server is overloaded"
            }
        })
        .to_string())];
        let result = collect_events(events).await;

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].event_type, StreamEventType::Error);
        assert!(
            result[0]
                .error_message
                .as_ref()
                .unwrap()
                .contains("overloaded")
        );
    }

    #[tokio::test]
    async fn translate_ping_is_skipped() {
        let events = vec![sse(&json!({"type": "ping"}).to_string())];
        let result = collect_events(events).await;
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn translate_done_sentinel_is_skipped() {
        let events = vec![Ok(SseEvent {
            event_type: "message".into(),
            data: "[DONE]".into(),
            id: None,
        })];
        let result = collect_events(events).await;
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn translate_full_conversation_flow() {
        let events = vec![
            sse(&json!({
                "type": "message_start",
                "message": {
                    "id": "msg_full",
                    "model": "claude-sonnet-4-20250514",
                    "content": [],
                    "stop_reason": null,
                    "usage": {"input_tokens": 25, "output_tokens": 0}
                }
            })
            .to_string()),
            sse(&json!({"type": "ping"}).to_string()),
            sse(&json!({
                "type": "content_block_start",
                "index": 0,
                "content_block": {"type": "text", "text": ""}
            })
            .to_string()),
            sse(&json!({
                "type": "content_block_delta",
                "index": 0,
                "delta": {"type": "text_delta", "text": "Hi there!"}
            })
            .to_string()),
            sse(&json!({
                "type": "content_block_stop",
                "index": 0
            })
            .to_string()),
            sse(&json!({
                "type": "message_delta",
                "delta": {"stop_reason": "end_turn"},
                "usage": {"input_tokens": 0, "output_tokens": 10}
            })
            .to_string()),
            sse(&json!({"type": "message_stop"}).to_string()),
        ];
        let result = collect_events(events).await;

        // Expected sequence: start, usage_delta (from start), text_delta, usage_delta (from message_delta), end
        let types: Vec<_> = result.iter().map(|e| e.event_type).collect();
        assert_eq!(
            types,
            vec![
                StreamEventType::Start,
                StreamEventType::UsageDelta,
                StreamEventType::ContentDelta,
                StreamEventType::UsageDelta,
                StreamEventType::End,
            ]
        );
    }

    #[tokio::test]
    async fn translate_parse_error_propagated() {
        let events = vec![Ok(SseEvent {
            event_type: "message".into(),
            data: "not valid json".into(),
            id: None,
        })];
        let sse_stream: Pin<Box<dyn Stream<Item = Result<SseEvent, Error>> + Send>> =
            Box::pin(stream::iter(events));
        let translated = translate_stream(sse_stream);
        let results: Vec<_> = translated.collect::<Vec<_>>().await;

        assert_eq!(results.len(), 1);
        assert!(results[0].is_err());
        match results[0].as_ref().unwrap_err() {
            Error::ResponseParse { provider, .. } => assert_eq!(provider, "anthropic"),
            other => panic!("expected ResponseParse, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn translate_signature_delta_is_silent() {
        let events = vec![
            sse(&json!({
                "type": "content_block_start",
                "index": 0,
                "content_block": {"type": "thinking", "thinking": "", "signature": ""}
            })
            .to_string()),
            sse(&json!({
                "type": "content_block_delta",
                "index": 0,
                "delta": {"type": "signature_delta", "signature": "sig_partial"}
            })
            .to_string()),
        ];
        let result = collect_events(events).await;
        // Signature deltas produce no output events.
        assert!(result.is_empty());
    }
}
