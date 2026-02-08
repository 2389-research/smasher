// ABOUTME: Translates OpenAI Responses API SSE events into unified StreamEvent values.
// ABOUTME: Tracks per-output-item state (type, call_id) to correctly map deltas.

use std::pin::Pin;

use futures::Stream;

use crate::types::{Error, FinishReason, StreamEvent, Usage};
use crate::util::sse::SseEvent;

/// Translate a stream of raw SSE events from the OpenAI Responses API into
/// unified StreamEvent values.
///
/// The Responses API emits typed SSE events with an `event:` field like
/// `response.created`, `response.output_text.delta`, etc. This function
/// maps those to the unified start/delta/end lifecycle.
pub fn translate_stream(
    sse_stream: Pin<Box<dyn Stream<Item = Result<SseEvent, Error>> + Send>>,
) -> Pin<Box<dyn Stream<Item = Result<StreamEvent, Error>> + Send>> {
    use async_stream::try_stream;
    use futures::StreamExt;

    Box::pin(try_stream! {
        let mut sse_stream = std::pin::pin!(sse_stream);

        // Track state for the current output items so we can map deltas.
        let mut current_call_id: Option<String> = None;
        let mut current_tool_name: Option<String> = None;
        let mut content_index: u32 = 0;

        while let Some(event_result) = sse_stream.next().await {
            let sse = event_result?;

            // The Responses API uses the SSE event type (not the data) to indicate
            // what happened. The data payload is a JSON object with details.
            let event_type = sse.event_type.as_str();

            match event_type {
                "response.created" => {
                    // Parse the response object to get id and model.
                    if let Ok(data) = serde_json::from_str::<serde_json::Value>(&sse.data) {
                        let id = data["id"].as_str().unwrap_or("").to_string();
                        let model = data["model"].as_str().unwrap_or("").to_string();
                        yield StreamEvent::start(id, model);
                    }
                }

                "response.output_item.added" => {
                    // Track the type and details of the new output item.
                    if let Ok(data) = serde_json::from_str::<serde_json::Value>(&sse.data) {
                        let item = &data["item"];
                        let item_type = item["type"].as_str();

                        if item_type == Some("function_call") {
                            current_call_id = item["call_id"].as_str().map(|s| s.to_string());
                            current_tool_name = item["name"].as_str().map(|s| s.to_string());
                        }

                        if let Some(idx) = data["output_index"].as_u64() {
                            content_index = idx as u32;
                        }
                    }
                }

                "response.content_part.added" => {
                    // Note the content part; no event to yield yet.
                }

                "response.output_text.delta" => {
                    if let Ok(data) = serde_json::from_str::<serde_json::Value>(&sse.data)
                        && let Some(delta) = data["delta"].as_str()
                    {
                        yield StreamEvent::text_delta(delta)
                            .with_content_index(content_index);
                    }
                }

                "response.function_call_arguments.delta" => {
                    if let Ok(data) = serde_json::from_str::<serde_json::Value>(&sse.data)
                        && let Some(delta) = data["delta"].as_str()
                    {
                        let call_id = current_call_id.clone().unwrap_or_default();
                        // Send tool name on the first delta, then None.
                        let name = current_tool_name.take();
                        yield StreamEvent::tool_call_delta(call_id, name, delta)
                            .with_content_index(content_index);
                    }
                }

                "response.reasoning_summary_text.delta" => {
                    if let Ok(data) = serde_json::from_str::<serde_json::Value>(&sse.data)
                        && let Some(delta) = data["delta"].as_str()
                    {
                        yield StreamEvent::thinking_delta(delta)
                            .with_content_index(content_index);
                    }
                }

                "response.output_item.done" => {
                    // Reset per-item state for the next item.
                    current_call_id = None;
                    current_tool_name = None;
                }

                "response.completed" => {
                    // Parse final usage if available.
                    let mut usage = None;
                    let mut finish_reason = Some(FinishReason::Stop);

                    if let Ok(data) = serde_json::from_str::<serde_json::Value>(&sse.data) {
                        let response = &data["response"];
                        if let Some(u) = response.get("usage") {
                            let input_tokens = u["input_tokens"].as_u64().unwrap_or(0) as u32;
                            let output_tokens = u["output_tokens"].as_u64().unwrap_or(0) as u32;

                            let cache_read_tokens = u.get("input_tokens_details")
                                .and_then(|d| d["cached_tokens"].as_u64())
                                .map(|t| t as u32)
                                .filter(|&t| t > 0);

                            let reasoning_tokens = u.get("output_tokens_details")
                                .and_then(|d| d["reasoning_tokens"].as_u64())
                                .map(|t| t as u32)
                                .filter(|&t| t > 0);

                            usage = Some(Usage {
                                input_tokens,
                                output_tokens,
                                cache_read_tokens,
                                cache_creation_tokens: None,
                                reasoning_tokens,
                                total_tokens: None,
                                raw: None,
                            });
                        }

                        // Check status for finish reason.
                        if let Some(status) = response["status"].as_str() {
                            finish_reason = Some(match status {
                                "completed" => {
                                    // Check if there are tool calls in the output.
                                    let has_tool_calls = response.get("output")
                                        .and_then(|o| o.as_array())
                                        .map(|items| items.iter().any(|item| {
                                            item["type"].as_str() == Some("function_call")
                                        }))
                                        .unwrap_or(false);
                                    if has_tool_calls {
                                        FinishReason::ToolUse
                                    } else {
                                        FinishReason::Stop
                                    }
                                }
                                "incomplete" => FinishReason::Length,
                                "failed" => FinishReason::Error,
                                _ => FinishReason::Stop,
                            });
                        }
                    }

                    yield StreamEvent::end(finish_reason, usage);
                }

                "response.failed" => {
                    // Extract error message if available.
                    let message = if let Ok(data) = serde_json::from_str::<serde_json::Value>(&sse.data) {
                        data["error"]["message"]
                            .as_str()
                            .unwrap_or("response failed")
                            .to_string()
                    } else {
                        "response failed".to_string()
                    };

                    yield StreamEvent::error(message);
                }

                _ => {
                    // Ignore unrecognized event types (e.g., response.in_progress,
                    // response.content_part.done, etc.).
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::stream;
    use futures::StreamExt;

    /// Helper to create an SSE event with a given event type and JSON data.
    fn sse(event_type: &str, data: &str) -> Result<SseEvent, Error> {
        Ok(SseEvent {
            event_type: event_type.to_string(),
            data: data.to_string(),
            id: None,
        })
    }

    /// Collect all stream events from a translated stream.
    async fn collect_events(
        events: Vec<Result<SseEvent, Error>>,
    ) -> Vec<StreamEvent> {
        let sse_stream: Pin<Box<dyn Stream<Item = Result<SseEvent, Error>> + Send>> =
            Box::pin(stream::iter(events));

        let translated = translate_stream(sse_stream);
        translated
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .map(|r| r.expect("unexpected error in stream"))
            .collect()
    }

    #[tokio::test]
    async fn translate_simple_text_stream() {
        let events = vec![
            sse("response.created", r#"{"id":"resp_1","model":"gpt-4o"}"#),
            sse(
                "response.output_item.added",
                r#"{"item":{"type":"message"},"output_index":0}"#,
            ),
            sse(
                "response.content_part.added",
                r#"{"part":{"type":"output_text"}}"#,
            ),
            sse("response.output_text.delta", r#"{"delta":"Hello"}"#),
            sse("response.output_text.delta", r#"{"delta":" world"}"#),
            sse("response.output_item.done", r#"{}"#),
            sse(
                "response.completed",
                r#"{"response":{"status":"completed","usage":{"input_tokens":10,"output_tokens":5}}}"#,
            ),
        ];

        let result = collect_events(events).await;
        // Start + 2 ContentDelta + End = 4 events
        assert_eq!(result.len(), 4);
        assert_eq!(result[0].event_type, crate::types::StreamEventType::Start);
        assert_eq!(result[0].response_id.as_deref(), Some("resp_1"));
        assert_eq!(result[0].model.as_deref(), Some("gpt-4o"));

        assert_eq!(result[1].event_type, crate::types::StreamEventType::ContentDelta);
        assert_eq!(result[1].text_delta.as_deref(), Some("Hello"));

        assert_eq!(result[2].event_type, crate::types::StreamEventType::ContentDelta);
        assert_eq!(result[2].text_delta.as_deref(), Some(" world"));

        assert_eq!(result[3].event_type, crate::types::StreamEventType::End);
        assert_eq!(result[3].finish_reason, Some(FinishReason::Stop));
        let usage = result[3].usage.as_ref().unwrap();
        assert_eq!(usage.input_tokens, 10);
        assert_eq!(usage.output_tokens, 5);
    }

    #[tokio::test]
    async fn translate_function_call_stream() {
        let events = vec![
            sse("response.created", r#"{"id":"resp_2","model":"gpt-4o"}"#),
            sse(
                "response.output_item.added",
                r#"{"item":{"type":"function_call","call_id":"call_abc","name":"get_weather"},"output_index":0}"#,
            ),
            sse(
                "response.function_call_arguments.delta",
                r#"{"delta":"{\"loc"}"#,
            ),
            sse(
                "response.function_call_arguments.delta",
                r#"{"delta":"ation\":\"NYC\"}"}"#,
            ),
            sse("response.output_item.done", r#"{}"#),
            sse(
                "response.completed",
                r#"{"response":{"status":"completed","output":[{"type":"function_call"}],"usage":{"input_tokens":20,"output_tokens":15}}}"#,
            ),
        ];

        let result = collect_events(events).await;
        // Start + 2 ToolCallDelta + End = 4
        assert_eq!(result.len(), 4);

        assert_eq!(result[0].event_type, crate::types::StreamEventType::Start);

        // First tool call delta should include the name.
        assert_eq!(result[1].event_type, crate::types::StreamEventType::ToolCallDelta);
        assert_eq!(result[1].tool_call_id.as_deref(), Some("call_abc"));
        assert_eq!(result[1].tool_name.as_deref(), Some("get_weather"));

        // Second delta should not repeat the name.
        assert_eq!(result[2].event_type, crate::types::StreamEventType::ToolCallDelta);
        assert_eq!(result[2].tool_call_id.as_deref(), Some("call_abc"));
        assert!(result[2].tool_name.is_none());

        assert_eq!(result[3].event_type, crate::types::StreamEventType::End);
        assert_eq!(result[3].finish_reason, Some(FinishReason::ToolUse));
    }

    #[tokio::test]
    async fn translate_reasoning_stream() {
        let events = vec![
            sse("response.created", r#"{"id":"resp_3","model":"o3"}"#),
            sse(
                "response.output_item.added",
                r#"{"item":{"type":"reasoning"},"output_index":0}"#,
            ),
            sse(
                "response.reasoning_summary_text.delta",
                r#"{"delta":"Thinking..."}"#,
            ),
            sse("response.output_item.done", r#"{}"#),
            sse(
                "response.completed",
                r#"{"response":{"status":"completed","usage":{"input_tokens":30,"output_tokens":40,"output_tokens_details":{"reasoning_tokens":25}}}}"#,
            ),
        ];

        let result = collect_events(events).await;
        // Start + ThinkingDelta + End = 3
        assert_eq!(result.len(), 3);

        assert_eq!(result[1].event_type, crate::types::StreamEventType::ThinkingDelta);
        assert_eq!(result[1].thinking_delta.as_deref(), Some("Thinking..."));

        let usage = result[2].usage.as_ref().unwrap();
        assert_eq!(usage.reasoning_tokens, Some(25));
    }

    #[tokio::test]
    async fn translate_failed_response() {
        let events = vec![
            sse("response.created", r#"{"id":"resp_4","model":"gpt-4o"}"#),
            sse(
                "response.failed",
                r#"{"error":{"message":"content policy violation"}}"#,
            ),
        ];

        let result = collect_events(events).await;
        assert_eq!(result.len(), 2);

        assert_eq!(result[1].event_type, crate::types::StreamEventType::Error);
        assert_eq!(
            result[1].error_message.as_deref(),
            Some("content policy violation")
        );
    }

    #[tokio::test]
    async fn translate_incomplete_response() {
        let events = vec![
            sse("response.created", r#"{"id":"resp_5","model":"gpt-4o"}"#),
            sse(
                "response.completed",
                r#"{"response":{"status":"incomplete","usage":{"input_tokens":100,"output_tokens":4096}}}"#,
            ),
        ];

        let result = collect_events(events).await;
        assert_eq!(result.len(), 2);
        assert_eq!(result[1].finish_reason, Some(FinishReason::Length));
    }

    #[tokio::test]
    async fn translate_empty_stream() {
        let events: Vec<Result<SseEvent, Error>> = vec![];
        let result = collect_events(events).await;
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn translate_unknown_event_types_ignored() {
        let events = vec![
            sse("response.created", r#"{"id":"resp_6","model":"gpt-4o"}"#),
            sse("response.in_progress", r#"{}"#),
            sse("response.content_part.done", r#"{}"#),
            sse(
                "response.completed",
                r#"{"response":{"status":"completed","usage":{"input_tokens":5,"output_tokens":3}}}"#,
            ),
        ];

        let result = collect_events(events).await;
        // Only Start + End (unknown types are ignored).
        assert_eq!(result.len(), 2);
    }

    #[tokio::test]
    async fn translate_usage_with_cached_tokens() {
        let events = vec![
            sse("response.created", r#"{"id":"resp_7","model":"gpt-4o"}"#),
            sse(
                "response.completed",
                r#"{"response":{"status":"completed","usage":{"input_tokens":100,"output_tokens":50,"input_tokens_details":{"cached_tokens":30}}}}"#,
            ),
        ];

        let result = collect_events(events).await;
        let usage = result[1].usage.as_ref().unwrap();
        assert_eq!(usage.cache_read_tokens, Some(30));
    }

    #[tokio::test]
    async fn translate_content_index_tracked() {
        let events = vec![
            sse("response.created", r#"{"id":"resp_8","model":"gpt-4o"}"#),
            sse(
                "response.output_item.added",
                r#"{"item":{"type":"message"},"output_index":2}"#,
            ),
            sse("response.output_text.delta", r#"{"delta":"Hi"}"#),
            sse("response.output_item.done", r#"{}"#),
            sse(
                "response.completed",
                r#"{"response":{"status":"completed","usage":{"input_tokens":1,"output_tokens":1}}}"#,
            ),
        ];

        let result = collect_events(events).await;
        // The text delta should carry content_index = 2.
        assert_eq!(result[1].content_index, Some(2));
    }
}
