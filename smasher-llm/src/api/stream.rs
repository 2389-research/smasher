// ABOUTME: High-level streaming API that wraps raw StreamEvent streams with an accumulator.
// ABOUTME: Provides StreamAccumulator to build a complete Response from streamed events.

use std::pin::Pin;

use futures::Stream;

use crate::client::Client;
use crate::types::{
    ContentPart, Error, FinishReason, Request, Response, StreamEvent, StreamEventType,
    ThinkingData, ToolCallData, Usage, infer_provider,
};

/// The result of initiating a streaming request: a raw event stream plus an accumulator.
pub struct StreamResult {
    /// The raw event stream.
    pub events: Pin<Box<dyn Stream<Item = Result<StreamEvent, Error>> + Send>>,
    /// An accumulator that can be used to build the final response.
    pub accumulator: StreamAccumulator,
}

/// Accumulates streaming events into a complete Response.
///
/// The caller iterates the event stream and feeds each event to `process_event`.
/// Once the stream is exhausted, `into_response` produces the final `Response`.
#[derive(Debug, Clone)]
pub struct StreamAccumulator {
    response_id: Option<String>,
    model: Option<String>,
    provider: Option<String>,
    text_buffer: String,
    tool_calls: Vec<ToolCallAccumulator>,
    thinking_buffer: String,
    usage: Usage,
    finish_reason: Option<FinishReason>,
}

/// Internal accumulator for a single tool call being streamed incrementally.
#[derive(Debug, Clone)]
struct ToolCallAccumulator {
    id: String,
    name: String,
    arguments: String,
}

impl StreamAccumulator {
    /// Create an empty accumulator with no state.
    pub fn new() -> Self {
        Self {
            response_id: None,
            model: None,
            provider: None,
            text_buffer: String::new(),
            tool_calls: Vec::new(),
            thinking_buffer: String::new(),
            usage: Usage::default(),
            finish_reason: None,
        }
    }

    /// Process a single stream event, updating the accumulator state.
    pub fn process_event(&mut self, event: &StreamEvent) {
        match event.event_type {
            StreamEventType::Start => {
                if let Some(ref id) = event.response_id {
                    self.response_id = Some(id.clone());
                }
                if let Some(ref model) = event.model {
                    self.provider = infer_provider(model).map(|p| p.to_string());
                    self.model = Some(model.clone());
                }
            }
            StreamEventType::ContentDelta => {
                if let Some(ref delta) = event.text_delta {
                    self.text_buffer.push_str(delta);
                }
            }
            StreamEventType::ToolCallDelta => {
                if let Some(ref tool_call_id) = event.tool_call_id {
                    // Find an existing accumulator for this tool call, or create one.
                    let acc = self
                        .tool_calls
                        .iter_mut()
                        .find(|tc| tc.id == *tool_call_id);

                    match acc {
                        Some(existing) => {
                            if let Some(ref name) = event.tool_name {
                                existing.name = name.clone();
                            }
                            if let Some(ref args) = event.arguments_delta {
                                existing.arguments.push_str(args);
                            }
                        }
                        None => {
                            self.tool_calls.push(ToolCallAccumulator {
                                id: tool_call_id.clone(),
                                name: event.tool_name.clone().unwrap_or_default(),
                                arguments: event.arguments_delta.clone().unwrap_or_default(),
                            });
                        }
                    }
                }
            }
            StreamEventType::ThinkingDelta => {
                if let Some(ref delta) = event.thinking_delta {
                    self.thinking_buffer.push_str(delta);
                }
            }
            StreamEventType::UsageDelta => {
                if let Some(ref usage) = event.usage {
                    self.usage += usage.clone();
                }
            }
            StreamEventType::End => {
                self.finish_reason = event.finish_reason.clone();
                if let Some(ref usage) = event.usage {
                    self.usage += usage.clone();
                }
            }
            StreamEventType::Error => {
                // Errors come through the stream Result, so we ignore error events here.
            }
            StreamEventType::TextStart
            | StreamEventType::TextEnd
            | StreamEventType::ReasoningStart
            | StreamEventType::ReasoningEnd
            | StreamEventType::ToolCallStart
            | StreamEventType::ToolCallEnd
            | StreamEventType::ProviderEvent => {
                // Lifecycle boundary events and provider-specific events are informational.
                // The accumulator only needs to process the actual data-carrying delta events.
            }
        }
    }

    /// Build the final Response from accumulated state.
    pub fn into_response(self) -> Response {
        let mut content = Vec::new();

        if !self.thinking_buffer.is_empty() {
            content.push(ContentPart::Thinking(ThinkingData {
                thinking: self.thinking_buffer,
                signature: None,
                redacted: false,
            }));
        }

        if !self.text_buffer.is_empty() {
            content.push(ContentPart::text(self.text_buffer));
        }

        for tc in self.tool_calls {
            content.push(ContentPart::ToolCall(ToolCallData {
                id: tc.id,
                name: tc.name,
                arguments: tc.arguments,
                raw_arguments: None,
            }));
        }

        Response {
            id: self.response_id.unwrap_or_default(),
            model: self.model.unwrap_or_default(),
            content,
            finish_reason: self.finish_reason,
            usage: self.usage,
            warnings: vec![],
            rate_limit: None,
            provider: self.provider,
            raw: None,
        }
    }

    /// Return the current accumulated text.
    pub fn text(&self) -> &str {
        &self.text_buffer
    }

    /// Check whether any tool calls have been accumulated.
    pub fn has_tool_calls(&self) -> bool {
        !self.tool_calls.is_empty()
    }

    /// Return the current accumulated usage statistics.
    pub fn usage(&self) -> &Usage {
        &self.usage
    }
}

impl Default for StreamAccumulator {
    fn default() -> Self {
        Self::new()
    }
}

/// Initiate a streaming request and return an event stream with an accumulator.
///
/// The caller is responsible for iterating the events and calling
/// `accumulator.process_event()` on each one. Once the stream is exhausted,
/// call `accumulator.into_response()` to get the final `Response`.
pub async fn stream(client: &Client, request: &Request) -> Result<StreamResult, Error> {
    let events = client.stream(request).await?;
    Ok(StreamResult {
        events,
        accumulator: StreamAccumulator::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{FinishReason, Usage};

    // --- StreamAccumulator tests ---

    #[test]
    fn process_start_event_stores_metadata() {
        let mut acc = StreamAccumulator::new();
        let event = StreamEvent::start("resp_123", "claude-sonnet-4");
        acc.process_event(&event);

        assert_eq!(acc.response_id.as_deref(), Some("resp_123"));
        assert_eq!(acc.model.as_deref(), Some("claude-sonnet-4"));
    }

    #[test]
    fn process_text_delta_accumulates_text() {
        let mut acc = StreamAccumulator::new();
        let event = StreamEvent::text_delta("Hello");
        acc.process_event(&event);

        assert_eq!(acc.text(), "Hello");
    }

    #[test]
    fn process_multiple_text_deltas_concatenates() {
        let mut acc = StreamAccumulator::new();
        acc.process_event(&StreamEvent::text_delta("Hello"));
        acc.process_event(&StreamEvent::text_delta(", "));
        acc.process_event(&StreamEvent::text_delta("world!"));

        assert_eq!(acc.text(), "Hello, world!");
    }

    #[test]
    fn process_tool_call_deltas_builds_tool_calls() {
        let mut acc = StreamAccumulator::new();
        acc.process_event(&StreamEvent::tool_call_delta(
            "call_1",
            Some("search".into()),
            r#"{"q":"#,
        ));
        acc.process_event(&StreamEvent::tool_call_delta(
            "call_1",
            None,
            r#""rust"}"#,
        ));

        assert!(acc.has_tool_calls());
        assert_eq!(acc.tool_calls.len(), 1);
        assert_eq!(acc.tool_calls[0].id, "call_1");
        assert_eq!(acc.tool_calls[0].name, "search");
        assert_eq!(acc.tool_calls[0].arguments, r#"{"q":"rust"}"#);
    }

    #[test]
    fn process_tool_call_with_name_on_first_delta() {
        let mut acc = StreamAccumulator::new();
        acc.process_event(&StreamEvent::tool_call_delta(
            "call_abc",
            Some("read_file".into()),
            r#"{"path":""#,
        ));
        acc.process_event(&StreamEvent::tool_call_delta(
            "call_abc",
            None,
            r#"/tmp"}"#,
        ));

        assert_eq!(acc.tool_calls[0].name, "read_file");
        assert_eq!(acc.tool_calls[0].arguments, r#"{"path":"/tmp"}"#);
    }

    #[test]
    fn process_thinking_deltas_accumulates_thinking_text() {
        let mut acc = StreamAccumulator::new();
        acc.process_event(&StreamEvent::thinking_delta("Let me "));
        acc.process_event(&StreamEvent::thinking_delta("think about this."));

        assert_eq!(acc.thinking_buffer, "Let me think about this.");
    }

    #[test]
    fn process_usage_delta_stores_usage() {
        let mut acc = StreamAccumulator::new();
        let usage = Usage {
            input_tokens: 100,
            output_tokens: 50,
            ..Default::default()
        };
        acc.process_event(&StreamEvent::usage_delta(usage));

        assert_eq!(acc.usage().input_tokens, 100);
        assert_eq!(acc.usage().output_tokens, 50);
    }

    #[test]
    fn process_end_event_stores_finish_reason() {
        let mut acc = StreamAccumulator::new();
        acc.process_event(&StreamEvent::end(Some(FinishReason::Stop), None));

        assert_eq!(acc.finish_reason, Some(FinishReason::Stop));
    }

    #[test]
    fn into_response_builds_complete_response_text_only() {
        let mut acc = StreamAccumulator::new();
        acc.process_event(&StreamEvent::start("resp_1", "gpt-4o"));
        acc.process_event(&StreamEvent::text_delta("Hello, world!"));
        acc.process_event(&StreamEvent::end(Some(FinishReason::Stop), None));

        let response = acc.into_response();
        assert_eq!(response.id, "resp_1");
        assert_eq!(response.model, "gpt-4o");
        assert_eq!(response.text(), Some("Hello, world!".to_string()));
        assert_eq!(response.finish_reason, Some(FinishReason::Stop));
        assert_eq!(response.content.len(), 1);
        assert!(!response.has_tool_calls());
    }

    #[test]
    fn into_response_builds_response_with_tool_calls() {
        let mut acc = StreamAccumulator::new();
        acc.process_event(&StreamEvent::start("resp_2", "claude-sonnet-4"));
        acc.process_event(&StreamEvent::tool_call_delta(
            "call_1",
            Some("search".into()),
            r#"{"query":"rust"}"#,
        ));
        acc.process_event(&StreamEvent::end(Some(FinishReason::ToolUse), None));

        let response = acc.into_response();
        assert!(response.has_tool_calls());
        let calls = response.tool_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "search");
        assert_eq!(calls[0].arguments, r#"{"query":"rust"}"#);
        assert_eq!(response.finish_reason, Some(FinishReason::ToolUse));
    }

    #[test]
    fn into_response_builds_response_with_thinking_and_text() {
        let mut acc = StreamAccumulator::new();
        acc.process_event(&StreamEvent::start("resp_3", "claude-sonnet-4"));
        acc.process_event(&StreamEvent::thinking_delta("Let me reason..."));
        acc.process_event(&StreamEvent::text_delta("The answer is 42."));
        acc.process_event(&StreamEvent::end(Some(FinishReason::Stop), None));

        let response = acc.into_response();
        assert_eq!(response.content.len(), 2);

        // Thinking comes first.
        match &response.content[0] {
            ContentPart::Thinking(data) => {
                assert_eq!(data.thinking, "Let me reason...");
            }
            other => panic!("expected Thinking, got {:?}", other),
        }

        // Then text.
        assert_eq!(response.text(), Some("The answer is 42.".to_string()));
    }

    #[test]
    fn into_response_with_no_content_produces_empty_content_vec() {
        let mut acc = StreamAccumulator::new();
        acc.process_event(&StreamEvent::start("resp_4", "gpt-4o"));
        acc.process_event(&StreamEvent::end(Some(FinishReason::Stop), None));

        let response = acc.into_response();
        assert!(response.content.is_empty());
    }

    #[test]
    fn text_returns_current_buffer() {
        let mut acc = StreamAccumulator::new();
        assert_eq!(acc.text(), "");

        acc.process_event(&StreamEvent::text_delta("partial"));
        assert_eq!(acc.text(), "partial");

        acc.process_event(&StreamEvent::text_delta(" response"));
        assert_eq!(acc.text(), "partial response");
    }

    #[test]
    fn has_tool_calls_false_initially_true_after_delta() {
        let mut acc = StreamAccumulator::new();
        assert!(!acc.has_tool_calls());

        acc.process_event(&StreamEvent::tool_call_delta(
            "call_1",
            Some("fn".into()),
            "{}",
        ));
        assert!(acc.has_tool_calls());
    }

    #[test]
    fn full_lifecycle_start_deltas_end_into_response() {
        let mut acc = StreamAccumulator::new();

        // Start
        acc.process_event(&StreamEvent::start("resp_full", "claude-sonnet-4"));

        // Thinking
        acc.process_event(&StreamEvent::thinking_delta("Analyzing the problem..."));

        // Text deltas
        acc.process_event(&StreamEvent::text_delta("Here is "));
        acc.process_event(&StreamEvent::text_delta("the answer."));

        // Tool call
        acc.process_event(&StreamEvent::tool_call_delta(
            "call_x",
            Some("calculator".into()),
            r#"{"expr":"#,
        ));
        acc.process_event(&StreamEvent::tool_call_delta(
            "call_x",
            None,
            r#""2+2"}"#,
        ));

        // Usage
        let usage = Usage {
            input_tokens: 50,
            output_tokens: 30,
            ..Default::default()
        };
        acc.process_event(&StreamEvent::usage_delta(usage));

        // End with additional usage
        let final_usage = Usage {
            input_tokens: 0,
            output_tokens: 5,
            ..Default::default()
        };
        acc.process_event(&StreamEvent::end(
            Some(FinishReason::ToolUse),
            Some(final_usage),
        ));

        // Verify accumulated state before into_response.
        assert_eq!(acc.text(), "Here is the answer.");
        assert!(acc.has_tool_calls());
        assert_eq!(acc.usage().input_tokens, 50);
        assert_eq!(acc.usage().output_tokens, 35);

        // Build the final response.
        let response = acc.into_response();
        assert_eq!(response.id, "resp_full");
        assert_eq!(response.model, "claude-sonnet-4");
        assert_eq!(response.finish_reason, Some(FinishReason::ToolUse));

        // Content order: thinking, text, tool calls.
        assert_eq!(response.content.len(), 3);
        match &response.content[0] {
            ContentPart::Thinking(data) => {
                assert_eq!(data.thinking, "Analyzing the problem...");
            }
            other => panic!("expected Thinking, got {:?}", other),
        }
        assert_eq!(response.text(), Some("Here is the answer.".to_string()));
        let calls = response.tool_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "calculator");
        assert_eq!(calls[0].arguments, r#"{"expr":"2+2"}"#);

        assert_eq!(response.usage.input_tokens, 50);
        assert_eq!(response.usage.output_tokens, 35);
    }

    #[test]
    fn default_accumulator_is_empty() {
        let acc = StreamAccumulator::default();
        assert_eq!(acc.text(), "");
        assert!(!acc.has_tool_calls());
        assert_eq!(acc.usage().input_tokens, 0);
        assert_eq!(acc.usage().output_tokens, 0);
        assert!(acc.response_id.is_none());
        assert!(acc.model.is_none());
        assert!(acc.provider.is_none());
        assert!(acc.finish_reason.is_none());
    }

    #[test]
    fn process_error_event_is_ignored() {
        let mut acc = StreamAccumulator::new();
        acc.process_event(&StreamEvent::text_delta("before"));
        acc.process_event(&StreamEvent::error("something went wrong"));
        // State should be unchanged by the error event.
        assert_eq!(acc.text(), "before");
        assert!(acc.finish_reason.is_none());
    }

    #[test]
    fn multiple_tool_calls_accumulated_separately() {
        let mut acc = StreamAccumulator::new();
        acc.process_event(&StreamEvent::tool_call_delta(
            "call_a",
            Some("search".into()),
            r#"{"q":"foo"}"#,
        ));
        acc.process_event(&StreamEvent::tool_call_delta(
            "call_b",
            Some("read_file".into()),
            r#"{"path":"/tmp"}"#,
        ));

        assert_eq!(acc.tool_calls.len(), 2);
        assert_eq!(acc.tool_calls[0].id, "call_a");
        assert_eq!(acc.tool_calls[0].name, "search");
        assert_eq!(acc.tool_calls[1].id, "call_b");
        assert_eq!(acc.tool_calls[1].name, "read_file");

        let response = acc.into_response();
        let calls = response.tool_calls();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].id, "call_a");
        assert_eq!(calls[1].id, "call_b");
    }

    #[test]
    fn end_event_accumulates_final_usage() {
        let mut acc = StreamAccumulator::new();
        let incremental = Usage {
            input_tokens: 10,
            output_tokens: 5,
            ..Default::default()
        };
        acc.process_event(&StreamEvent::usage_delta(incremental));

        let final_usage = Usage {
            input_tokens: 0,
            output_tokens: 20,
            cache_read_tokens: Some(3),
            ..Default::default()
        };
        acc.process_event(&StreamEvent::end(Some(FinishReason::Stop), Some(final_usage)));

        assert_eq!(acc.usage().input_tokens, 10);
        assert_eq!(acc.usage().output_tokens, 25);
        assert_eq!(acc.usage().cache_read_tokens, Some(3));
    }

    #[test]
    fn start_event_infers_provider_from_model() {
        // Anthropic model
        let mut acc = StreamAccumulator::new();
        acc.process_event(&StreamEvent::start("resp_1", "claude-sonnet-4"));
        assert_eq!(acc.provider.as_deref(), Some("anthropic"));
        let response = acc.into_response();
        assert_eq!(response.provider.as_deref(), Some("anthropic"));

        // OpenAI model
        let mut acc = StreamAccumulator::new();
        acc.process_event(&StreamEvent::start("resp_2", "gpt-4o"));
        assert_eq!(acc.provider.as_deref(), Some("openai"));
        let response = acc.into_response();
        assert_eq!(response.provider.as_deref(), Some("openai"));

        // Gemini model
        let mut acc = StreamAccumulator::new();
        acc.process_event(&StreamEvent::start("resp_3", "gemini-2.5-pro"));
        assert_eq!(acc.provider.as_deref(), Some("gemini"));
        let response = acc.into_response();
        assert_eq!(response.provider.as_deref(), Some("gemini"));

        // Unknown model — provider stays None
        let mut acc = StreamAccumulator::new();
        acc.process_event(&StreamEvent::start("resp_4", "llama-3"));
        assert!(acc.provider.is_none());
        let response = acc.into_response();
        assert!(response.provider.is_none());
    }
}
