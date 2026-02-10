// ABOUTME: Streaming event types for real-time LLM response delivery.
// ABOUTME: Defines the start/delta/end event pattern used across all provider adapters.

use serde::{Deserialize, Serialize};

/// The type of a streaming event, following the start/delta/end lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StreamEventType {
    /// Stream has started; metadata (model, id) is available.
    Start,
    /// A content delta has arrived.
    ContentDelta,
    /// A tool call delta has arrived (name or argument chunk).
    ToolCallDelta,
    /// A thinking/reasoning delta has arrived.
    ThinkingDelta,
    /// Token usage statistics update.
    UsageDelta,
    /// Stream has ended normally.
    End,
    /// An error occurred during streaming.
    Error,
    /// Text content segment has started.
    TextStart,
    /// Text content segment has ended.
    TextEnd,
    /// Reasoning/thinking segment has started.
    ReasoningStart,
    /// Reasoning/thinking segment has ended.
    ReasoningEnd,
    /// Tool call has started (name available).
    ToolCallStart,
    /// Tool call has ended.
    ToolCallEnd,
    /// Provider-specific event that doesn't map to standard types.
    ProviderEvent,
}

/// A single event in an LLM response stream.
///
/// Events follow a lifecycle: Start → (ContentDelta | ToolCallDelta | ThinkingDelta | UsageDelta)* → End
/// An Error event can occur at any point and terminates the stream.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamEvent {
    pub event_type: StreamEventType,

    /// The response ID, available from the Start event onward.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_id: Option<String>,

    /// The model that generated this response.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,

    /// Text content delta.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text_delta: Option<String>,

    /// Tool call ID for tool call deltas.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,

    /// Tool name for tool call deltas (sent in the first delta for a tool call).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,

    /// Tool call argument fragment.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arguments_delta: Option<String>,

    /// Thinking/reasoning text delta.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking_delta: Option<String>,

    /// Index of the content block this delta belongs to (for multi-block responses).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_index: Option<u32>,

    /// Incremental token usage update.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<super::response::Usage>,

    /// Finish reason, present in End events.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finish_reason: Option<super::response::FinishReason>,

    /// Error message, present in Error events.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,

    /// Raw provider event for debugging/passthrough.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw: Option<serde_json::Value>,

    /// Type of provider-specific event (for ProviderEvent events).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_event_type: Option<String>,
}

impl StreamEvent {
    /// Create a helper that returns a StreamEvent with all fields set to None.
    fn empty(event_type: StreamEventType) -> Self {
        Self {
            event_type,
            response_id: None,
            model: None,
            text_delta: None,
            tool_call_id: None,
            tool_name: None,
            arguments_delta: None,
            thinking_delta: None,
            content_index: None,
            usage: None,
            finish_reason: None,
            error_message: None,
            raw: None,
            provider_event_type: None,
        }
    }

    /// Create a Start event with response metadata.
    pub fn start(response_id: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            response_id: Some(response_id.into()),
            model: Some(model.into()),
            ..Self::empty(StreamEventType::Start)
        }
    }

    /// Create a text content delta event.
    pub fn text_delta(text: impl Into<String>) -> Self {
        Self {
            text_delta: Some(text.into()),
            ..Self::empty(StreamEventType::ContentDelta)
        }
    }

    /// Create a tool call delta event.
    pub fn tool_call_delta(
        tool_call_id: impl Into<String>,
        tool_name: Option<String>,
        arguments_delta: impl Into<String>,
    ) -> Self {
        Self {
            tool_call_id: Some(tool_call_id.into()),
            tool_name,
            arguments_delta: Some(arguments_delta.into()),
            ..Self::empty(StreamEventType::ToolCallDelta)
        }
    }

    /// Create a thinking delta event.
    pub fn thinking_delta(text: impl Into<String>) -> Self {
        Self {
            thinking_delta: Some(text.into()),
            ..Self::empty(StreamEventType::ThinkingDelta)
        }
    }

    /// Create a usage delta event.
    pub fn usage_delta(usage: super::response::Usage) -> Self {
        Self {
            usage: Some(usage),
            ..Self::empty(StreamEventType::UsageDelta)
        }
    }

    /// Create a usage event (alias for usage_delta).
    pub fn usage(usage: super::response::Usage) -> Self {
        Self::usage_delta(usage)
    }

    /// Create an End event with optional finish reason and final usage.
    pub fn end(
        finish_reason: Option<super::response::FinishReason>,
        usage: Option<super::response::Usage>,
    ) -> Self {
        Self {
            finish_reason,
            usage,
            ..Self::empty(StreamEventType::End)
        }
    }

    /// Create an error event.
    pub fn error(message: impl Into<String>) -> Self {
        Self {
            error_message: Some(message.into()),
            ..Self::empty(StreamEventType::Error)
        }
    }

    /// Create a TextStart event indicating a text content segment has started.
    pub fn text_start(content_index: u32) -> Self {
        Self {
            content_index: Some(content_index),
            ..Self::empty(StreamEventType::TextStart)
        }
    }

    /// Create a TextEnd event indicating a text content segment has ended.
    pub fn text_end(content_index: u32) -> Self {
        Self {
            content_index: Some(content_index),
            ..Self::empty(StreamEventType::TextEnd)
        }
    }

    /// Create a ReasoningStart event indicating a thinking segment has started.
    pub fn thinking_start(content_index: u32) -> Self {
        Self {
            content_index: Some(content_index),
            ..Self::empty(StreamEventType::ReasoningStart)
        }
    }

    /// Create a ReasoningEnd event indicating a thinking segment has ended.
    pub fn thinking_end(content_index: u32) -> Self {
        Self {
            content_index: Some(content_index),
            ..Self::empty(StreamEventType::ReasoningEnd)
        }
    }

    /// Create a ToolCallStart event indicating a tool call has started.
    pub fn tool_call_start(
        id: impl Into<String>,
        name: impl Into<String>,
        content_index: u32,
    ) -> Self {
        Self {
            tool_call_id: Some(id.into()),
            tool_name: Some(name.into()),
            content_index: Some(content_index),
            ..Self::empty(StreamEventType::ToolCallStart)
        }
    }

    /// Create a ToolCallEnd event indicating a tool call has ended.
    pub fn tool_call_end(id: impl Into<String>) -> Self {
        Self {
            tool_call_id: Some(id.into()),
            ..Self::empty(StreamEventType::ToolCallEnd)
        }
    }

    /// Create a ProviderEvent for provider-specific events.
    pub fn provider_event(event_type: impl Into<String>, raw: serde_json::Value) -> Self {
        Self {
            provider_event_type: Some(event_type.into()),
            raw: Some(raw),
            ..Self::empty(StreamEventType::ProviderEvent)
        }
    }

    /// Set the content index on this event (builder-style).
    pub fn with_content_index(mut self, index: u32) -> Self {
        self.content_index = Some(index);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::response::{FinishReason, Usage};

    #[test]
    fn start_event_carries_metadata() {
        let event = StreamEvent::start("resp_123", "claude-sonnet-4");
        assert_eq!(event.event_type, StreamEventType::Start);
        assert_eq!(event.response_id.as_deref(), Some("resp_123"));
        assert_eq!(event.model.as_deref(), Some("claude-sonnet-4"));
        assert!(event.text_delta.is_none());
    }

    #[test]
    fn text_delta_event() {
        let event = StreamEvent::text_delta("Hello");
        assert_eq!(event.event_type, StreamEventType::ContentDelta);
        assert_eq!(event.text_delta.as_deref(), Some("Hello"));
        assert!(event.response_id.is_none());
    }

    #[test]
    fn tool_call_delta_event() {
        let event =
            StreamEvent::tool_call_delta("call_abc", Some("read_file".into()), r#"{"path":"#);
        assert_eq!(event.event_type, StreamEventType::ToolCallDelta);
        assert_eq!(event.tool_call_id.as_deref(), Some("call_abc"));
        assert_eq!(event.tool_name.as_deref(), Some("read_file"));
        assert_eq!(event.arguments_delta.as_deref(), Some(r#"{"path":"#));
    }

    #[test]
    fn thinking_delta_event() {
        let event = StreamEvent::thinking_delta("Let me think about this...");
        assert_eq!(event.event_type, StreamEventType::ThinkingDelta);
        assert_eq!(
            event.thinking_delta.as_deref(),
            Some("Let me think about this...")
        );
    }

    #[test]
    fn usage_delta_event() {
        let usage = Usage {
            input_tokens: 100,
            output_tokens: 50,
            ..Default::default()
        };
        let event = StreamEvent::usage_delta(usage);
        assert_eq!(event.event_type, StreamEventType::UsageDelta);
        let u = event.usage.unwrap();
        assert_eq!(u.input_tokens, 100);
        assert_eq!(u.output_tokens, 50);
    }

    #[test]
    fn end_event_with_finish_reason() {
        let event = StreamEvent::end(Some(FinishReason::Stop), None);
        assert_eq!(event.event_type, StreamEventType::End);
        assert_eq!(event.finish_reason, Some(FinishReason::Stop));
    }

    #[test]
    fn error_event() {
        let event = StreamEvent::error("connection reset");
        assert_eq!(event.event_type, StreamEventType::Error);
        assert_eq!(event.error_message.as_deref(), Some("connection reset"));
    }

    #[test]
    fn with_content_index() {
        let event = StreamEvent::text_delta("chunk").with_content_index(2);
        assert_eq!(event.content_index, Some(2));
    }

    #[test]
    fn stream_event_type_serde_roundtrip() {
        for event_type in [
            StreamEventType::Start,
            StreamEventType::ContentDelta,
            StreamEventType::ToolCallDelta,
            StreamEventType::ThinkingDelta,
            StreamEventType::UsageDelta,
            StreamEventType::End,
            StreamEventType::Error,
            StreamEventType::TextStart,
            StreamEventType::TextEnd,
            StreamEventType::ReasoningStart,
            StreamEventType::ReasoningEnd,
            StreamEventType::ToolCallStart,
            StreamEventType::ToolCallEnd,
            StreamEventType::ProviderEvent,
        ] {
            let json = serde_json::to_string(&event_type).unwrap();
            let back: StreamEventType = serde_json::from_str(&json).unwrap();
            assert_eq!(event_type, back);
        }
    }

    #[test]
    fn stream_event_serde_roundtrip() {
        let event = StreamEvent::start("resp_1", "gpt-4o");
        let json = serde_json::to_string(&event).unwrap();
        let back: StreamEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(back.event_type, StreamEventType::Start);
        assert_eq!(back.response_id.as_deref(), Some("resp_1"));
    }

    #[test]
    fn none_fields_omitted_in_json() {
        let event = StreamEvent::text_delta("hi");
        let json = serde_json::to_string(&event).unwrap();
        assert!(!json.contains("response_id"));
        assert!(!json.contains("tool_call_id"));
        assert!(!json.contains("error_message"));
        assert!(!json.contains("raw"));
        assert!(!json.contains("provider_event_type"));
        assert!(json.contains("text_delta"));
    }

    // --- New event type constructor tests ---

    #[test]
    fn text_start_event() {
        let event = StreamEvent::text_start(0);
        assert_eq!(event.event_type, StreamEventType::TextStart);
        assert_eq!(event.content_index, Some(0));
        assert!(event.text_delta.is_none());
    }

    #[test]
    fn text_end_event() {
        let event = StreamEvent::text_end(3);
        assert_eq!(event.event_type, StreamEventType::TextEnd);
        assert_eq!(event.content_index, Some(3));
        assert!(event.text_delta.is_none());
    }

    #[test]
    fn thinking_start_event() {
        let event = StreamEvent::thinking_start(1);
        assert_eq!(event.event_type, StreamEventType::ReasoningStart);
        assert_eq!(event.content_index, Some(1));
        assert!(event.thinking_delta.is_none());
    }

    #[test]
    fn thinking_end_event() {
        let event = StreamEvent::thinking_end(1);
        assert_eq!(event.event_type, StreamEventType::ReasoningEnd);
        assert_eq!(event.content_index, Some(1));
        assert!(event.thinking_delta.is_none());
    }

    #[test]
    fn tool_call_start_event() {
        let event = StreamEvent::tool_call_start("call_123", "read_file", 2);
        assert_eq!(event.event_type, StreamEventType::ToolCallStart);
        assert_eq!(event.tool_call_id.as_deref(), Some("call_123"));
        assert_eq!(event.tool_name.as_deref(), Some("read_file"));
        assert_eq!(event.content_index, Some(2));
        assert!(event.arguments_delta.is_none());
    }

    #[test]
    fn tool_call_end_event() {
        let event = StreamEvent::tool_call_end("call_123");
        assert_eq!(event.event_type, StreamEventType::ToolCallEnd);
        assert_eq!(event.tool_call_id.as_deref(), Some("call_123"));
        assert!(event.tool_name.is_none());
    }

    #[test]
    fn provider_event_creation() {
        let raw = serde_json::json!({"type": "custom", "data": 42});
        let event = StreamEvent::provider_event("custom_event", raw.clone());
        assert_eq!(event.event_type, StreamEventType::ProviderEvent);
        assert_eq!(event.provider_event_type.as_deref(), Some("custom_event"));
        assert_eq!(event.raw, Some(raw));
        assert!(event.text_delta.is_none());
    }

    #[test]
    fn usage_alias_works() {
        let usage = Usage {
            input_tokens: 42,
            output_tokens: 17,
            ..Default::default()
        };
        let event = StreamEvent::usage(usage);
        assert_eq!(event.event_type, StreamEventType::UsageDelta);
        let u = event.usage.unwrap();
        assert_eq!(u.input_tokens, 42);
        assert_eq!(u.output_tokens, 17);
    }

    // --- Serde roundtrip for new event types ---

    #[test]
    fn text_start_serde_roundtrip() {
        let event = StreamEvent::text_start(5);
        let json = serde_json::to_string(&event).unwrap();
        let back: StreamEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(back.event_type, StreamEventType::TextStart);
        assert_eq!(back.content_index, Some(5));
    }

    #[test]
    fn text_end_serde_roundtrip() {
        let event = StreamEvent::text_end(5);
        let json = serde_json::to_string(&event).unwrap();
        let back: StreamEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(back.event_type, StreamEventType::TextEnd);
        assert_eq!(back.content_index, Some(5));
    }

    #[test]
    fn thinking_start_serde_roundtrip() {
        let event = StreamEvent::thinking_start(0);
        let json = serde_json::to_string(&event).unwrap();
        let back: StreamEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(back.event_type, StreamEventType::ReasoningStart);
        assert_eq!(back.content_index, Some(0));
    }

    #[test]
    fn thinking_end_serde_roundtrip() {
        let event = StreamEvent::thinking_end(0);
        let json = serde_json::to_string(&event).unwrap();
        let back: StreamEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(back.event_type, StreamEventType::ReasoningEnd);
        assert_eq!(back.content_index, Some(0));
    }

    #[test]
    fn tool_call_start_serde_roundtrip() {
        let event = StreamEvent::tool_call_start("call_1", "search", 0);
        let json = serde_json::to_string(&event).unwrap();
        let back: StreamEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(back.event_type, StreamEventType::ToolCallStart);
        assert_eq!(back.tool_call_id.as_deref(), Some("call_1"));
        assert_eq!(back.tool_name.as_deref(), Some("search"));
        assert_eq!(back.content_index, Some(0));
    }

    #[test]
    fn tool_call_end_serde_roundtrip() {
        let event = StreamEvent::tool_call_end("call_1");
        let json = serde_json::to_string(&event).unwrap();
        let back: StreamEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(back.event_type, StreamEventType::ToolCallEnd);
        assert_eq!(back.tool_call_id.as_deref(), Some("call_1"));
    }

    #[test]
    fn provider_event_serde_roundtrip() {
        let raw = serde_json::json!({"key": "value"});
        let event = StreamEvent::provider_event("my_type", raw.clone());
        let json = serde_json::to_string(&event).unwrap();
        let back: StreamEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(back.event_type, StreamEventType::ProviderEvent);
        assert_eq!(back.provider_event_type.as_deref(), Some("my_type"));
        assert_eq!(back.raw, Some(raw));
    }

    #[test]
    fn content_index_correlation_across_lifecycle() {
        let start = StreamEvent::text_start(2);
        let delta = StreamEvent::text_delta("hello").with_content_index(2);
        let end = StreamEvent::text_end(2);

        assert_eq!(start.content_index, Some(2));
        assert_eq!(delta.content_index, Some(2));
        assert_eq!(end.content_index, Some(2));

        assert_eq!(start.event_type, StreamEventType::TextStart);
        assert_eq!(delta.event_type, StreamEventType::ContentDelta);
        assert_eq!(end.event_type, StreamEventType::TextEnd);
    }

    #[test]
    fn tool_call_lifecycle_content_index() {
        let start = StreamEvent::tool_call_start("call_1", "search", 0);
        let delta =
            StreamEvent::tool_call_delta("call_1", None, r#"{"q":"rust"}"#).with_content_index(0);
        let end = StreamEvent::tool_call_end("call_1");

        assert_eq!(start.content_index, Some(0));
        assert_eq!(delta.content_index, Some(0));
        assert_eq!(start.tool_call_id.as_deref(), Some("call_1"));
        assert_eq!(delta.tool_call_id.as_deref(), Some("call_1"));
        assert_eq!(end.tool_call_id.as_deref(), Some("call_1"));
    }

    #[test]
    fn reasoning_lifecycle_content_index() {
        let start = StreamEvent::thinking_start(0);
        let delta = StreamEvent::thinking_delta("Let me reason...").with_content_index(0);
        let end = StreamEvent::thinking_end(0);

        assert_eq!(start.content_index, Some(0));
        assert_eq!(delta.content_index, Some(0));
        assert_eq!(end.content_index, Some(0));

        assert_eq!(start.event_type, StreamEventType::ReasoningStart);
        assert_eq!(delta.event_type, StreamEventType::ThinkingDelta);
        assert_eq!(end.event_type, StreamEventType::ReasoningEnd);
    }

    #[test]
    fn new_event_types_serialize_to_snake_case() {
        assert_eq!(
            serde_json::to_string(&StreamEventType::TextStart).unwrap(),
            "\"text_start\""
        );
        assert_eq!(
            serde_json::to_string(&StreamEventType::TextEnd).unwrap(),
            "\"text_end\""
        );
        assert_eq!(
            serde_json::to_string(&StreamEventType::ReasoningStart).unwrap(),
            "\"reasoning_start\""
        );
        assert_eq!(
            serde_json::to_string(&StreamEventType::ReasoningEnd).unwrap(),
            "\"reasoning_end\""
        );
        assert_eq!(
            serde_json::to_string(&StreamEventType::ToolCallStart).unwrap(),
            "\"tool_call_start\""
        );
        assert_eq!(
            serde_json::to_string(&StreamEventType::ToolCallEnd).unwrap(),
            "\"tool_call_end\""
        );
        assert_eq!(
            serde_json::to_string(&StreamEventType::ProviderEvent).unwrap(),
            "\"provider_event\""
        );
    }

    #[test]
    fn provider_event_raw_field_present_in_json() {
        let raw = serde_json::json!({"debug": true});
        let event = StreamEvent::provider_event("debug_event", raw);
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("raw"));
        assert!(json.contains("provider_event_type"));
        assert!(json.contains("debug_event"));
    }

    #[test]
    fn raw_and_provider_event_type_omitted_when_none() {
        let event = StreamEvent::text_delta("test");
        let json = serde_json::to_string(&event).unwrap();
        assert!(!json.contains("raw"));
        assert!(!json.contains("provider_event_type"));
    }
}
