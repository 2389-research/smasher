// ABOUTME: Defines unified LLM response types including finish reasons, token usage, and rate limit info.
// ABOUTME: Provides a provider-agnostic Response struct with helpers for extracting text and tool calls.

use serde::{Deserialize, Serialize};
use std::ops::{Add, AddAssign};

use super::content::{ContentPart, ToolCallData};

/// Normalized finish reason across all providers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FinishReason {
    /// Natural completion — the model stopped on its own.
    Stop,
    /// Hit the max token limit.
    Length,
    /// Model wants to call one or more tools.
    ToolUse,
    /// Content was filtered by the provider's safety system.
    ContentFilter,
    /// An error occurred during generation.
    Error,
    /// Unmapped provider finish reason, carries the raw string.
    Other(String),
}

/// Token usage statistics returned by the provider.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Usage {
    pub input_tokens: u32,
    pub output_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_read_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_creation_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_tokens: Option<u32>,
    /// Computed or provider-supplied total token count.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_tokens: Option<u32>,
    /// Raw provider usage data for debugging.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw: Option<serde_json::Value>,
}

/// Helper to add two `Option<u32>` values, treating `None` as 0 when the other is `Some`.
fn add_optional(a: Option<u32>, b: Option<u32>) -> Option<u32> {
    match (a, b) {
        (Some(x), Some(y)) => Some(x + y),
        (Some(x), None) => Some(x),
        (None, Some(y)) => Some(y),
        (None, None) => None,
    }
}

impl Add for Usage {
    type Output = Self;

    fn add(self, rhs: Self) -> Self::Output {
        Self {
            input_tokens: self.input_tokens + rhs.input_tokens,
            output_tokens: self.output_tokens + rhs.output_tokens,
            cache_read_tokens: add_optional(self.cache_read_tokens, rhs.cache_read_tokens),
            cache_creation_tokens: add_optional(
                self.cache_creation_tokens,
                rhs.cache_creation_tokens,
            ),
            reasoning_tokens: add_optional(self.reasoning_tokens, rhs.reasoning_tokens),
            total_tokens: add_optional(self.total_tokens, rhs.total_tokens),
            // Raw provider data is not merged; drop both sides on addition.
            raw: None,
        }
    }
}

impl AddAssign for Usage {
    fn add_assign(&mut self, rhs: Self) {
        *self = self.clone() + rhs;
    }
}

/// A warning returned by the provider alongside the response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Warning {
    pub code: String,
    pub message: String,
}

/// Rate limit information parsed from provider response headers.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct RateLimitInfo {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requests_remaining: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requests_limit: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tokens_remaining: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tokens_limit: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reset_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// The unified response from any LLM provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    pub id: String,
    pub model: String,
    pub content: Vec<ContentPart>,
    pub finish_reason: Option<FinishReason>,
    pub usage: Usage,
    pub warnings: Vec<Warning>,
    pub rate_limit: Option<RateLimitInfo>,
    /// Which provider fulfilled the request.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// Raw provider response for debugging.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw: Option<serde_json::Value>,
}

impl Response {
    /// Returns concatenated text from all Text content parts, or None if there are no text parts.
    pub fn text(&self) -> Option<String> {
        let texts: Vec<&str> = self
            .content
            .iter()
            .filter_map(|part| part.as_text())
            .collect();
        if texts.is_empty() {
            None
        } else {
            Some(texts.join(""))
        }
    }

    /// Returns concatenated thinking text from all Thinking content parts, or None if there are none.
    pub fn reasoning(&self) -> Option<String> {
        let parts: Vec<&str> = self
            .content
            .iter()
            .filter_map(|part| match part {
                ContentPart::Thinking(data) => Some(data.thinking.as_str()),
                _ => None,
            })
            .collect();
        if parts.is_empty() {
            None
        } else {
            Some(parts.join(""))
        }
    }

    /// Extracts references to all tool call data from the content vector.
    pub fn tool_calls(&self) -> Vec<&ToolCallData> {
        self.content
            .iter()
            .filter_map(|part| match part {
                ContentPart::ToolCall(data) => Some(data),
                _ => None,
            })
            .collect()
    }

    /// Returns `true` if the response contains at least one tool call.
    pub fn has_tool_calls(&self) -> bool {
        self.content.iter().any(|part| part.is_tool_call())
    }
}

#[cfg(test)]
mod tests {
    use super::super::content::ToolCallData;
    use super::*;

    // ── Usage arithmetic ──────────────────────────────────────────────

    #[test]
    fn usage_add_sums_all_fields() {
        let a = Usage {
            input_tokens: 10,
            output_tokens: 20,
            cache_read_tokens: Some(5),
            cache_creation_tokens: Some(3),
            reasoning_tokens: Some(8),
            total_tokens: Some(30),
            raw: None,
        };
        let b = Usage {
            input_tokens: 100,
            output_tokens: 200,
            cache_read_tokens: Some(50),
            cache_creation_tokens: Some(30),
            reasoning_tokens: Some(80),
            total_tokens: Some(300),
            raw: None,
        };

        let sum = a + b;

        assert_eq!(sum.input_tokens, 110);
        assert_eq!(sum.output_tokens, 220);
        assert_eq!(sum.cache_read_tokens, Some(55));
        assert_eq!(sum.cache_creation_tokens, Some(33));
        assert_eq!(sum.reasoning_tokens, Some(88));
        assert_eq!(sum.total_tokens, Some(330));
    }

    #[test]
    fn usage_add_assign_works() {
        let mut usage = Usage {
            input_tokens: 10,
            output_tokens: 20,
            cache_read_tokens: None,
            cache_creation_tokens: None,
            reasoning_tokens: None,
            total_tokens: None,
            raw: None,
        };

        usage += Usage {
            input_tokens: 5,
            output_tokens: 15,
            cache_read_tokens: Some(3),
            cache_creation_tokens: None,
            reasoning_tokens: Some(7),
            total_tokens: Some(20),
            raw: None,
        };

        assert_eq!(usage.input_tokens, 15);
        assert_eq!(usage.output_tokens, 35);
        assert_eq!(usage.cache_read_tokens, Some(3));
        assert_eq!(usage.cache_creation_tokens, None);
        assert_eq!(usage.reasoning_tokens, Some(7));
        assert_eq!(usage.total_tokens, Some(20));
    }

    #[test]
    fn usage_add_none_plus_some_yields_some() {
        let a = Usage {
            input_tokens: 0,
            output_tokens: 0,
            cache_read_tokens: None,
            cache_creation_tokens: Some(10),
            reasoning_tokens: None,
            total_tokens: None,
            raw: None,
        };
        let b = Usage {
            input_tokens: 0,
            output_tokens: 0,
            cache_read_tokens: Some(5),
            cache_creation_tokens: None,
            reasoning_tokens: None,
            total_tokens: None,
            raw: None,
        };

        let sum = a + b;

        assert_eq!(sum.cache_read_tokens, Some(5));
        assert_eq!(sum.cache_creation_tokens, Some(10));
        assert_eq!(sum.reasoning_tokens, None);
        assert_eq!(sum.total_tokens, None);
    }

    #[test]
    fn usage_default_is_zero() {
        let usage = Usage::default();
        assert_eq!(usage.input_tokens, 0);
        assert_eq!(usage.output_tokens, 0);
        assert_eq!(usage.cache_read_tokens, None);
        assert_eq!(usage.cache_creation_tokens, None);
        assert_eq!(usage.reasoning_tokens, None);
        assert_eq!(usage.total_tokens, None);
        assert!(usage.raw.is_none());
    }

    #[test]
    fn usage_total_tokens_add_both_some() {
        let a = Usage {
            total_tokens: Some(10),
            ..Default::default()
        };
        let b = Usage {
            total_tokens: Some(20),
            ..Default::default()
        };
        assert_eq!((a + b).total_tokens, Some(30));
    }

    #[test]
    fn usage_total_tokens_add_one_none() {
        let a = Usage {
            total_tokens: Some(10),
            ..Default::default()
        };
        let b = Usage {
            total_tokens: None,
            ..Default::default()
        };
        assert_eq!((a + b).total_tokens, Some(10));
    }

    #[test]
    fn usage_raw_dropped_on_add() {
        let a = Usage {
            raw: Some(serde_json::json!({"a": 1})),
            ..Default::default()
        };
        let b = Usage {
            raw: Some(serde_json::json!({"b": 2})),
            ..Default::default()
        };
        assert!((a + b).raw.is_none());
    }

    // ── FinishReason serde ────────────────────────────────────────────

    #[test]
    fn finish_reason_serde_roundtrip() {
        let variants = vec![
            FinishReason::Stop,
            FinishReason::Length,
            FinishReason::ToolUse,
            FinishReason::ContentFilter,
            FinishReason::Error,
            FinishReason::Other("custom_reason".into()),
        ];

        for reason in variants {
            let json = serde_json::to_string(&reason).unwrap();
            let back: FinishReason = serde_json::from_str(&json).unwrap();
            assert_eq!(reason, back);
        }
    }

    #[test]
    fn finish_reason_serializes_to_snake_case() {
        assert_eq!(
            serde_json::to_string(&FinishReason::Stop).unwrap(),
            "\"stop\""
        );
        assert_eq!(
            serde_json::to_string(&FinishReason::ToolUse).unwrap(),
            "\"tool_use\""
        );
        assert_eq!(
            serde_json::to_string(&FinishReason::ContentFilter).unwrap(),
            "\"content_filter\""
        );
    }

    #[test]
    fn finish_reason_other_roundtrip() {
        let reason = FinishReason::Other("recitation".into());
        let json = serde_json::to_string(&reason).unwrap();
        let back: FinishReason = serde_json::from_str(&json).unwrap();
        assert_eq!(reason, back);
    }

    #[test]
    fn finish_reason_other_equality() {
        assert_eq!(
            FinishReason::Other("foo".into()),
            FinishReason::Other("foo".into())
        );
        assert_ne!(
            FinishReason::Other("foo".into()),
            FinishReason::Other("bar".into())
        );
        assert_ne!(FinishReason::Other("stop".into()), FinishReason::Stop);
    }

    // ── Warning serde ─────────────────────────────────────────────────

    #[test]
    fn warning_serde_roundtrip() {
        let warning = Warning {
            code: "deprecated_model".into(),
            message: "This model will be removed in 30 days.".into(),
        };

        let json = serde_json::to_string(&warning).unwrap();
        let back: Warning = serde_json::from_str(&json).unwrap();

        assert_eq!(back.code, "deprecated_model");
        assert_eq!(back.message, "This model will be removed in 30 days.");
    }

    // ── RateLimitInfo serde ───────────────────────────────────────────

    #[test]
    fn rate_limit_info_serde_roundtrip() {
        let info = RateLimitInfo {
            requests_remaining: Some(99),
            requests_limit: Some(100),
            tokens_remaining: Some(90_000),
            tokens_limit: Some(100_000),
            reset_at: Some(
                chrono::DateTime::parse_from_rfc3339("2026-01-15T12:00:00Z")
                    .unwrap()
                    .with_timezone(&chrono::Utc),
            ),
        };

        let json = serde_json::to_string(&info).unwrap();
        let back: RateLimitInfo = serde_json::from_str(&json).unwrap();

        assert_eq!(back.requests_remaining, Some(99));
        assert_eq!(back.requests_limit, Some(100));
        assert_eq!(back.tokens_remaining, Some(90_000));
        assert_eq!(back.tokens_limit, Some(100_000));
        assert!(back.reset_at.is_some());
    }

    #[test]
    fn rate_limit_info_with_all_none_fields() {
        let info = RateLimitInfo {
            requests_remaining: None,
            requests_limit: None,
            tokens_remaining: None,
            tokens_limit: None,
            reset_at: None,
        };

        let json = serde_json::to_string(&info).unwrap();
        let back: RateLimitInfo = serde_json::from_str(&json).unwrap();

        assert_eq!(back.requests_remaining, None);
        assert_eq!(back.reset_at, None);
    }

    // ── Response helpers ──────────────────────────────────────────────

    fn sample_response() -> Response {
        Response {
            id: "resp_001".into(),
            model: "claude-opus-4-20250514".into(),
            content: vec![
                ContentPart::text("Hello, world!"),
                ContentPart::ToolCall(ToolCallData {
                    id: "call_1".into(),
                    name: "search".into(),
                    arguments: r#"{"q":"rust"}"#.into(),
                    raw_arguments: None,
                }),
                ContentPart::ToolCall(ToolCallData {
                    id: "call_2".into(),
                    name: "read_file".into(),
                    arguments: r#"{"path":"/tmp"}"#.into(),
                    raw_arguments: None,
                }),
            ],
            finish_reason: Some(FinishReason::ToolUse),
            usage: Usage {
                input_tokens: 50,
                output_tokens: 120,
                cache_read_tokens: None,
                cache_creation_tokens: None,
                reasoning_tokens: None,
                total_tokens: None,
                raw: None,
            },
            warnings: vec![],
            rate_limit: None,
            provider: None,
            raw: None,
        }
    }

    #[test]
    fn response_text_returns_single_text_part() {
        let resp = sample_response();
        assert_eq!(resp.text().as_deref(), Some("Hello, world!"));
    }

    #[test]
    fn response_text_concatenates_multiple_text_parts() {
        let resp = Response {
            id: "resp_multi_text".into(),
            model: "test".into(),
            content: vec![ContentPart::text("Hello, "), ContentPart::text("world!")],
            finish_reason: Some(FinishReason::Stop),
            usage: Usage::default(),
            warnings: vec![],
            rate_limit: None,
            provider: None,
            raw: None,
        };
        assert_eq!(resp.text().as_deref(), Some("Hello, world!"));
    }

    #[test]
    fn response_text_concatenates_with_interleaved_parts() {
        use super::super::content::ThinkingData;
        let resp = Response {
            id: "resp_interleave".into(),
            model: "test".into(),
            content: vec![
                ContentPart::text("part1"),
                ContentPart::Thinking(ThinkingData {
                    thinking: "reasoning".into(),
                    signature: None,
                    redacted: false,
                }),
                ContentPart::text("part2"),
            ],
            finish_reason: Some(FinishReason::Stop),
            usage: Usage::default(),
            warnings: vec![],
            rate_limit: None,
            provider: None,
            raw: None,
        };
        assert_eq!(resp.text().as_deref(), Some("part1part2"));
    }

    #[test]
    fn response_text_returns_none_when_no_text() {
        let resp = Response {
            id: "resp_002".into(),
            model: "claude-opus-4-20250514".into(),
            content: vec![ContentPart::ToolCall(ToolCallData {
                id: "call_1".into(),
                name: "search".into(),
                arguments: "{}".into(),
                raw_arguments: None,
            })],
            finish_reason: Some(FinishReason::ToolUse),
            usage: Usage::default(),
            warnings: vec![],
            rate_limit: None,
            provider: None,
            raw: None,
        };
        assert_eq!(resp.text(), None);
    }

    #[test]
    fn response_reasoning_returns_thinking_text() {
        use super::super::content::ThinkingData;
        let resp = Response {
            id: "resp_think".into(),
            model: "test".into(),
            content: vec![
                ContentPart::Thinking(ThinkingData {
                    thinking: "Step 1. ".into(),
                    signature: None,
                    redacted: false,
                }),
                ContentPart::text("Answer."),
                ContentPart::Thinking(ThinkingData {
                    thinking: "Step 2.".into(),
                    signature: Some("sig".into()),
                    redacted: false,
                }),
            ],
            finish_reason: Some(FinishReason::Stop),
            usage: Usage::default(),
            warnings: vec![],
            rate_limit: None,
            provider: None,
            raw: None,
        };
        assert_eq!(resp.reasoning().as_deref(), Some("Step 1. Step 2."));
    }

    #[test]
    fn response_reasoning_returns_none_when_no_thinking() {
        let resp = sample_response();
        assert_eq!(resp.reasoning(), None);
    }

    #[test]
    fn response_tool_calls_extracts_all_tool_call_parts() {
        let resp = sample_response();
        let calls = resp.tool_calls();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].name, "search");
        assert_eq!(calls[1].name, "read_file");
    }

    #[test]
    fn response_has_tool_calls_true_when_present() {
        let resp = sample_response();
        assert!(resp.has_tool_calls());
    }

    #[test]
    fn response_has_tool_calls_false_when_absent() {
        let resp = Response {
            id: "resp_003".into(),
            model: "claude-opus-4-20250514".into(),
            content: vec![ContentPart::text("Just text.")],
            finish_reason: Some(FinishReason::Stop),
            usage: Usage::default(),
            warnings: vec![],
            rate_limit: None,
            provider: None,
            raw: None,
        };
        assert!(!resp.has_tool_calls());
    }

    #[test]
    fn response_provider_and_raw_fields() {
        let resp = Response {
            id: "resp_004".into(),
            model: "test".into(),
            content: vec![ContentPart::text("hi")],
            finish_reason: Some(FinishReason::Stop),
            usage: Usage::default(),
            warnings: vec![],
            rate_limit: None,
            provider: Some("anthropic".into()),
            raw: Some(serde_json::json!({"id": "msg_123"})),
        };
        assert_eq!(resp.provider.as_deref(), Some("anthropic"));
        assert_eq!(resp.raw.as_ref().unwrap()["id"], "msg_123");
    }

    #[test]
    fn response_provider_and_raw_skipped_when_none() {
        let resp = sample_response();
        let value: serde_json::Value = serde_json::to_value(&resp).unwrap();
        let obj = value.as_object().unwrap();
        assert!(!obj.contains_key("provider"));
        assert!(!obj.contains_key("raw"));
    }

    #[test]
    fn response_serde_roundtrip() {
        let resp = sample_response();
        let json = serde_json::to_string(&resp).unwrap();
        let back: Response = serde_json::from_str(&json).unwrap();

        assert_eq!(back.id, "resp_001");
        assert_eq!(back.model, "claude-opus-4-20250514");
        assert_eq!(back.finish_reason, Some(FinishReason::ToolUse));
        assert_eq!(back.usage.input_tokens, 50);
        assert_eq!(back.content.len(), 3);
        assert!(back.provider.is_none());
        assert!(back.raw.is_none());
    }

    #[test]
    fn usage_deserializes_with_missing_optional_fields() {
        let json = r#"{"input_tokens": 100, "output_tokens": 200}"#;
        let usage: Usage = serde_json::from_str(json).unwrap();
        assert_eq!(usage.input_tokens, 100);
        assert_eq!(usage.output_tokens, 200);
        assert_eq!(usage.cache_read_tokens, None);
        assert_eq!(usage.cache_creation_tokens, None);
        assert_eq!(usage.reasoning_tokens, None);
        assert_eq!(usage.total_tokens, None);
        assert!(usage.raw.is_none());
    }

    #[test]
    fn usage_deserializes_from_empty_json() {
        let json = r#"{}"#;
        let usage: Usage = serde_json::from_str(json).unwrap();
        assert_eq!(usage.input_tokens, 0);
        assert_eq!(usage.output_tokens, 0);
        assert_eq!(usage.cache_read_tokens, None);
    }

    #[test]
    fn usage_skips_none_optional_fields_on_serialize() {
        let usage = Usage {
            input_tokens: 10,
            output_tokens: 20,
            ..Default::default()
        };
        let value: serde_json::Value = serde_json::to_value(&usage).unwrap();
        let obj = value.as_object().unwrap();
        assert!(!obj.contains_key("cache_read_tokens"));
        assert!(!obj.contains_key("cache_creation_tokens"));
        assert!(!obj.contains_key("reasoning_tokens"));
        assert!(!obj.contains_key("total_tokens"));
        assert!(!obj.contains_key("raw"));
    }

    #[test]
    fn rate_limit_info_deserializes_from_empty_json() {
        let json = r#"{}"#;
        let info: RateLimitInfo = serde_json::from_str(json).unwrap();
        assert_eq!(info.requests_remaining, None);
        assert_eq!(info.requests_limit, None);
        assert_eq!(info.tokens_remaining, None);
        assert_eq!(info.tokens_limit, None);
        assert_eq!(info.reset_at, None);
    }

    #[test]
    fn rate_limit_info_deserializes_with_partial_fields() {
        let json = r#"{"requests_remaining": 42}"#;
        let info: RateLimitInfo = serde_json::from_str(json).unwrap();
        assert_eq!(info.requests_remaining, Some(42));
        assert_eq!(info.requests_limit, None);
        assert_eq!(info.tokens_remaining, None);
    }

    #[test]
    fn rate_limit_info_skips_none_fields_on_serialize() {
        let info = RateLimitInfo::default();
        let value: serde_json::Value = serde_json::to_value(&info).unwrap();
        let obj = value.as_object().unwrap();
        assert!(!obj.contains_key("requests_remaining"));
        assert!(!obj.contains_key("requests_limit"));
        assert!(!obj.contains_key("tokens_remaining"));
        assert!(!obj.contains_key("tokens_limit"));
        assert!(!obj.contains_key("reset_at"));
    }

    #[test]
    fn response_serde_roundtrip_with_provider_and_raw() {
        let resp = Response {
            id: "resp_full".into(),
            model: "test".into(),
            content: vec![ContentPart::text("ok")],
            finish_reason: Some(FinishReason::Other("custom".into())),
            usage: Usage {
                total_tokens: Some(42),
                raw: Some(serde_json::json!({"total": 42})),
                ..Default::default()
            },
            warnings: vec![],
            rate_limit: None,
            provider: Some("openai".into()),
            raw: Some(serde_json::json!({"raw_field": true})),
        };
        let json = serde_json::to_string(&resp).unwrap();
        let back: Response = serde_json::from_str(&json).unwrap();

        assert_eq!(back.provider.as_deref(), Some("openai"));
        assert_eq!(back.raw.as_ref().unwrap()["raw_field"], true);
        assert_eq!(
            back.finish_reason,
            Some(FinishReason::Other("custom".into()))
        );
        assert_eq!(back.usage.total_tokens, Some(42));
        assert_eq!(back.usage.raw.as_ref().unwrap()["total"], 42);
    }
}
