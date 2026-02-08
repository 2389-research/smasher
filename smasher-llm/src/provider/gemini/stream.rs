// ABOUTME: Translates Gemini SSE streaming responses into unified StreamEvent items.
// ABOUTME: Handles parsing of GeminiResponse JSON from SSE data fields and maps to the start/delta/end lifecycle.

use std::pin::Pin;

use futures::Stream;
use futures::StreamExt;
use uuid::Uuid;

use crate::types::{Error, FinishReason, StreamEvent, Usage};
use crate::util::sse::SseEvent;

use super::types::{GeminiPart, GeminiResponse, map_finish_reason};

/// Translate a stream of SSE events (from Gemini's streamGenerateContent?alt=sse endpoint)
/// into a stream of unified StreamEvent items.
///
/// Each SSE event data field contains a complete GeminiResponse JSON object representing
/// a partial response. The function emits Start, ContentDelta, ToolCallDelta, UsageDelta,
/// and End events following the unified streaming lifecycle.
pub fn translate_stream(
    sse_stream: Pin<Box<dyn Stream<Item = Result<SseEvent, Error>> + Send>>,
    model: String,
) -> Pin<Box<dyn Stream<Item = Result<StreamEvent, Error>> + Send>> {
    Box::pin(async_stream::try_stream! {
        let mut sse_stream = sse_stream;
        let mut started = false;

        while let Some(sse_result) = sse_stream.next().await {
            let sse_event = sse_result?;

            // Skip non-data events and empty data.
            if sse_event.data.is_empty() || sse_event.data == "[DONE]" {
                continue;
            }

            // Parse the SSE data as a GeminiResponse.
            let gemini_resp: GeminiResponse = serde_json::from_str(&sse_event.data)
                .map_err(|e| Error::ResponseParse {
                    provider: "gemini".to_string(),
                    message: format!("failed to parse streaming chunk: {e}"),
                })?;

            // Emit Start event on first chunk.
            if !started {
                let response_id = gemini_resp
                    .model_version
                    .clone()
                    .unwrap_or_else(|| Uuid::new_v4().to_string());
                yield StreamEvent::start(response_id, &model);
                started = true;
            }

            // Track finish reason from candidate.
            let mut chunk_finish_reason: Option<FinishReason> = None;

            if let Some(ref candidates) = gemini_resp.candidates
                && let Some(candidate) = candidates.first()
            {
                // Map finish reason if present.
                if let Some(ref reason) = candidate.finish_reason {
                    chunk_finish_reason = Some(map_finish_reason(reason));
                }

                // Process content parts.
                if let Some(ref content) = candidate.content {
                    for (idx, part) in content.parts.iter().enumerate() {
                        match part {
                            GeminiPart::Text { text } => {
                                if !text.is_empty() {
                                    yield StreamEvent::text_delta(text)
                                        .with_content_index(idx as u32);
                                }
                            }
                            GeminiPart::FunctionCall { function_call } => {
                                let tool_call_id = Uuid::new_v4().to_string();
                                let arguments = serde_json::to_string(&function_call.args)
                                    .unwrap_or_else(|_| "{}".to_string());
                                yield StreamEvent::tool_call_delta(
                                    tool_call_id,
                                    Some(function_call.name.clone()),
                                    arguments,
                                ).with_content_index(idx as u32);
                            }
                            // InlineData and FunctionResponse are not expected in streaming
                            // responses; skip them.
                            GeminiPart::InlineData { .. }
                            | GeminiPart::FunctionResponse { .. } => {}
                        }
                    }
                }
            }

            // Emit usage delta if present.
            if let Some(ref usage_meta) = gemini_resp.usage_metadata {
                let usage = Usage {
                    input_tokens: usage_meta.prompt_token_count.unwrap_or(0),
                    output_tokens: usage_meta.candidates_token_count.unwrap_or(0),
                    cache_read_tokens: None,
                    cache_creation_tokens: None,
                    reasoning_tokens: None,
                    total_tokens: usage_meta.total_token_count,
                    raw: None,
                };
                yield StreamEvent::usage_delta(usage);
            }

            // Emit End event if finish reason is present.
            // Usage data is already carried by the UsageDelta event (when present
            // in this chunk), so we always set final_usage to None to avoid
            // the StreamAccumulator double-counting usage.
            if let Some(reason) = chunk_finish_reason {
                yield StreamEvent::end(Some(reason), None);
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::stream;
    use serde_json::json;

    /// Helper: create an SseEvent with the given JSON data.
    fn sse_event(data: &str) -> Result<SseEvent, Error> {
        Ok(SseEvent {
            event_type: "message".to_string(),
            data: data.to_string(),
            id: None,
        })
    }

    /// Helper: collect all stream events from a translated stream.
    async fn collect_stream_events(
        events: Vec<Result<SseEvent, Error>>,
        model: &str,
    ) -> Vec<StreamEvent> {
        let sse_stream: Pin<Box<dyn Stream<Item = Result<SseEvent, Error>> + Send>> =
            Box::pin(stream::iter(events));

        let translated = translate_stream(sse_stream, model.to_string());
        translated
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .map(|r| r.expect("unexpected error in stream"))
            .collect()
    }

    #[tokio::test]
    async fn stream_simple_text_response() {
        let chunk1 = json!({
            "candidates": [{
                "content": {
                    "role": "model",
                    "parts": [{"text": "Hello"}]
                }
            }],
            "modelVersion": "gemini-2.0-flash"
        });

        let chunk2 = json!({
            "candidates": [{
                "content": {
                    "role": "model",
                    "parts": [{"text": " world!"}]
                },
                "finishReason": "STOP"
            }],
            "usageMetadata": {
                "promptTokenCount": 5,
                "candidatesTokenCount": 3,
                "totalTokenCount": 8
            }
        });

        let events = collect_stream_events(
            vec![
                sse_event(&chunk1.to_string()),
                sse_event(&chunk2.to_string()),
            ],
            "gemini-2.0-flash",
        )
        .await;

        // Expected: Start, ContentDelta("Hello"), ContentDelta(" world!"), UsageDelta, End
        assert!(events.len() >= 4);

        // First should be Start.
        assert_eq!(events[0].event_type, crate::types::StreamEventType::Start);
        assert_eq!(events[0].model.as_deref(), Some("gemini-2.0-flash"));

        // Second should be content delta.
        assert_eq!(
            events[1].event_type,
            crate::types::StreamEventType::ContentDelta
        );
        assert_eq!(events[1].text_delta.as_deref(), Some("Hello"));

        // Find the " world!" delta.
        let world_delta = events
            .iter()
            .find(|e| e.text_delta.as_deref() == Some(" world!"));
        assert!(world_delta.is_some());

        // Should have an End event.
        let end = events
            .iter()
            .find(|e| e.event_type == crate::types::StreamEventType::End);
        assert!(end.is_some());
        assert_eq!(end.unwrap().finish_reason, Some(FinishReason::Stop));
    }

    #[tokio::test]
    async fn stream_function_call() {
        let chunk = json!({
            "candidates": [{
                "content": {
                    "role": "model",
                    "parts": [{
                        "functionCall": {
                            "name": "get_weather",
                            "args": {"location": "NYC"}
                        }
                    }]
                },
                "finishReason": "STOP"
            }],
            "usageMetadata": {
                "promptTokenCount": 10,
                "candidatesTokenCount": 5,
                "totalTokenCount": 15
            },
            "modelVersion": "gemini-2.0-flash"
        });

        let events = collect_stream_events(
            vec![sse_event(&chunk.to_string())],
            "gemini-2.0-flash",
        )
        .await;

        // Should have Start, ToolCallDelta, UsageDelta, End.
        let tool_delta = events
            .iter()
            .find(|e| e.event_type == crate::types::StreamEventType::ToolCallDelta);
        assert!(tool_delta.is_some());
        let td = tool_delta.unwrap();
        assert_eq!(td.tool_name.as_deref(), Some("get_weather"));
        assert!(td.tool_call_id.is_some());
        assert!(td.arguments_delta.is_some());
    }

    #[tokio::test]
    async fn stream_skips_done_sentinel() {
        let chunk = json!({
            "candidates": [{
                "content": {
                    "role": "model",
                    "parts": [{"text": "hi"}]
                },
                "finishReason": "STOP"
            }],
            "modelVersion": "gemini-2.0-flash"
        });

        let events = collect_stream_events(
            vec![
                sse_event(&chunk.to_string()),
                sse_event("[DONE]"),
            ],
            "gemini-2.0-flash",
        )
        .await;

        // [DONE] should be skipped, should still have Start + ContentDelta + End.
        assert!(events.len() >= 3);
    }

    #[tokio::test]
    async fn stream_skips_empty_data() {
        let chunk = json!({
            "candidates": [{
                "content": {
                    "role": "model",
                    "parts": [{"text": "ok"}]
                },
                "finishReason": "STOP"
            }],
            "modelVersion": "gemini-2.0-flash"
        });

        let events = collect_stream_events(
            vec![
                sse_event(""),
                sse_event(&chunk.to_string()),
            ],
            "gemini-2.0-flash",
        )
        .await;

        // Empty data should be skipped.
        assert!(events.len() >= 3);
        assert_eq!(events[0].event_type, crate::types::StreamEventType::Start);
    }

    #[tokio::test]
    async fn stream_parse_error_propagated() {
        let events_input = vec![sse_event("not valid json")];

        let sse_stream: Pin<Box<dyn Stream<Item = Result<SseEvent, Error>> + Send>> =
            Box::pin(stream::iter(events_input));

        let translated = translate_stream(sse_stream, "gemini-2.0-flash".to_string());
        let results: Vec<_> = translated.collect::<Vec<_>>().await;

        // Should have exactly one error.
        assert_eq!(results.len(), 1);
        assert!(results[0].is_err());
        match results[0].as_ref().unwrap_err() {
            Error::ResponseParse { provider, .. } => {
                assert_eq!(provider, "gemini");
            }
            other => panic!("expected ResponseParse, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn stream_sse_error_propagated() {
        let events_input: Vec<Result<SseEvent, Error>> = vec![Err(Error::StreamError {
            provider: "gemini".to_string(),
            message: "connection reset".to_string(),
        })];

        let sse_stream: Pin<Box<dyn Stream<Item = Result<SseEvent, Error>> + Send>> =
            Box::pin(stream::iter(events_input));

        let translated = translate_stream(sse_stream, "gemini-2.0-flash".to_string());
        let results: Vec<_> = translated.collect::<Vec<_>>().await;

        assert_eq!(results.len(), 1);
        assert!(results[0].is_err());
    }

    #[tokio::test]
    async fn stream_usage_delta_emitted() {
        let chunk = json!({
            "candidates": [{
                "content": {
                    "role": "model",
                    "parts": [{"text": "hi"}]
                },
                "finishReason": "STOP"
            }],
            "usageMetadata": {
                "promptTokenCount": 100,
                "candidatesTokenCount": 50,
                "totalTokenCount": 150
            },
            "modelVersion": "gemini-2.0-flash"
        });

        let events = collect_stream_events(
            vec![sse_event(&chunk.to_string())],
            "gemini-2.0-flash",
        )
        .await;

        let usage_event = events
            .iter()
            .find(|e| e.event_type == crate::types::StreamEventType::UsageDelta);
        assert!(usage_event.is_some());
        let u = usage_event.unwrap().usage.as_ref().unwrap();
        assert_eq!(u.input_tokens, 100);
        assert_eq!(u.output_tokens, 50);
    }

    #[tokio::test]
    async fn stream_content_index_set() {
        let chunk = json!({
            "candidates": [{
                "content": {
                    "role": "model",
                    "parts": [
                        {"text": "first part"},
                        {"text": "second part"}
                    ]
                }
            }],
            "modelVersion": "gemini-2.0-flash"
        });

        let events = collect_stream_events(
            vec![sse_event(&chunk.to_string())],
            "gemini-2.0-flash",
        )
        .await;

        let content_deltas: Vec<_> = events
            .iter()
            .filter(|e| e.event_type == crate::types::StreamEventType::ContentDelta)
            .collect();

        assert_eq!(content_deltas.len(), 2);
        assert_eq!(content_deltas[0].content_index, Some(0));
        assert_eq!(content_deltas[1].content_index, Some(1));
    }

    #[tokio::test]
    async fn stream_empty_text_parts_skipped() {
        let chunk = json!({
            "candidates": [{
                "content": {
                    "role": "model",
                    "parts": [{"text": ""}]
                }
            }],
            "modelVersion": "gemini-2.0-flash"
        });

        let events = collect_stream_events(
            vec![sse_event(&chunk.to_string())],
            "gemini-2.0-flash",
        )
        .await;

        // Should only have Start (no content delta for empty text).
        let content_deltas: Vec<_> = events
            .iter()
            .filter(|e| e.event_type == crate::types::StreamEventType::ContentDelta)
            .collect();
        assert!(content_deltas.is_empty());
    }

    #[tokio::test]
    async fn stream_end_event_no_duplicate_usage() {
        // When a chunk has both usageMetadata and finishReason, usage should
        // only appear on the UsageDelta event, NOT on the End event. This
        // prevents the StreamAccumulator from double-counting usage.
        let chunk = json!({
            "candidates": [{
                "content": {
                    "role": "model",
                    "parts": [{"text": "done"}]
                },
                "finishReason": "STOP"
            }],
            "usageMetadata": {
                "promptTokenCount": 10,
                "candidatesTokenCount": 20,
                "totalTokenCount": 30
            },
            "modelVersion": "gemini-2.0-flash"
        });

        let events = collect_stream_events(
            vec![sse_event(&chunk.to_string())],
            "gemini-2.0-flash",
        )
        .await;

        // UsageDelta should carry the usage data.
        let usage_event = events
            .iter()
            .find(|e| e.event_type == crate::types::StreamEventType::UsageDelta)
            .unwrap();
        let usage = usage_event.usage.as_ref().unwrap();
        assert_eq!(usage.input_tokens, 10);
        assert_eq!(usage.output_tokens, 20);
        assert_eq!(usage.total_tokens, Some(30));

        // End event should NOT carry usage (avoids double-counting).
        let end_event = events
            .iter()
            .find(|e| e.event_type == crate::types::StreamEventType::End)
            .unwrap();
        assert_eq!(end_event.finish_reason, Some(FinishReason::Stop));
        assert!(end_event.usage.is_none(), "End event should not duplicate usage from UsageDelta");
    }

    #[tokio::test]
    async fn stream_no_candidates_chunk() {
        let chunk = json!({
            "usageMetadata": {
                "promptTokenCount": 10,
                "candidatesTokenCount": 0,
                "totalTokenCount": 10
            },
            "modelVersion": "gemini-2.0-flash"
        });

        let events = collect_stream_events(
            vec![sse_event(&chunk.to_string())],
            "gemini-2.0-flash",
        )
        .await;

        // Should have Start + UsageDelta (no content delta, no end).
        assert!(events.len() >= 2);
        assert_eq!(events[0].event_type, crate::types::StreamEventType::Start);
    }
}
