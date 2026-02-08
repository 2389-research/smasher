// ABOUTME: Defines the ContentPart enum, a tagged union representing different content types in LLM messages.
// ABOUTME: Supports text, images, audio, documents, tool calls, tool results, and thinking blocks.

use serde::{Deserialize, Serialize};

/// A single piece of content within an LLM message.
///
/// Uses a `kind` tag in the JSON representation to distinguish between variants,
/// enabling clean serialization across OpenAI, Anthropic, and Gemini providers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum ContentPart {
    /// Plain text content.
    #[serde(rename = "text")]
    Text { text: String },

    /// Image content, either base64-encoded or referenced by URL.
    #[serde(rename = "image")]
    Image(ImageData),

    /// Audio content, base64-encoded with a format specifier.
    #[serde(rename = "audio")]
    Audio(AudioData),

    /// Document content (e.g. PDF), either base64-encoded or referenced by URL.
    #[serde(rename = "document")]
    Document(DocumentData),

    /// A tool/function call issued by the assistant.
    #[serde(rename = "tool_call")]
    ToolCall(ToolCallData),

    /// The result of executing a tool call.
    #[serde(rename = "tool_result")]
    ToolResult(ToolResultData),

    /// An extended thinking/reasoning block (Anthropic).
    #[serde(rename = "thinking")]
    Thinking(ThinkingData),

    /// Redacted thinking block for opaque round-tripping (Anthropic).
    #[serde(rename = "redacted_thinking")]
    RedactedThinking(RedactedThinkingData),
}

impl ContentPart {
    /// Create a Text content part from anything that converts to String.
    pub fn text(s: impl Into<String>) -> Self {
        ContentPart::Text { text: s.into() }
    }

    /// Returns true if this is a Text variant.
    pub fn is_text(&self) -> bool {
        matches!(self, ContentPart::Text { .. })
    }

    /// Returns the text string if this is a Text variant, None otherwise.
    pub fn as_text(&self) -> Option<&str> {
        match self {
            ContentPart::Text { text } => Some(text),
            _ => None,
        }
    }

    /// Returns true if this is a ToolCall variant.
    pub fn is_tool_call(&self) -> bool {
        matches!(self, ContentPart::ToolCall(_))
    }

    /// Returns true if this is a ToolResult variant.
    pub fn is_tool_result(&self) -> bool {
        matches!(self, ContentPart::ToolResult(_))
    }

    /// Returns true if this is a Thinking variant.
    pub fn is_thinking(&self) -> bool {
        matches!(self, ContentPart::Thinking(_))
    }

    /// Returns true if this is a RedactedThinking variant.
    pub fn is_redacted_thinking(&self) -> bool {
        matches!(self, ContentPart::RedactedThinking(_))
    }
}

/// How an image is sourced: inline base64 data or an external URL.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImageSourceType {
    Base64,
    Url,
}

/// Image content data.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImageData {
    /// Whether the image data is base64-encoded or a URL reference.
    pub source_type: ImageSourceType,
    /// MIME type of the image, e.g. "image/png".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub media_type: Option<String>,
    /// The base64-encoded image data or the URL, depending on `source_type`.
    pub data: String,
}

/// How a document is sourced: inline base64 data or an external URL.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DocumentSourceType {
    Base64,
    Url,
}

/// Audio content data.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AudioData {
    /// Base64-encoded audio data.
    pub data: String,
    /// Audio format, e.g. "wav", "mp3".
    pub format: String,
}

/// Document content data (e.g. PDF attachments).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DocumentData {
    /// Whether the document data is base64-encoded or a URL reference.
    pub source_type: DocumentSourceType,
    /// MIME type of the document, e.g. "application/pdf".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub media_type: Option<String>,
    /// The base64-encoded document data or the URL, depending on `source_type`.
    pub data: String,
    /// Optional filename for the document.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filename: Option<String>,
}

/// Data for a tool/function call issued by the assistant.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCallData {
    /// Provider-assigned identifier for this tool call.
    pub id: String,
    /// Name of the tool/function to invoke.
    pub name: String,
    /// JSON-encoded string of the tool call arguments.
    pub arguments: String,
    /// Raw argument string before any parsing or normalization.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw_arguments: Option<String>,
}

/// Data for the result of executing a tool call.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolResultData {
    /// The id of the tool call this result corresponds to.
    pub tool_call_id: String,
    /// The content/output returned by the tool.
    pub content: String,
    /// Whether the tool execution resulted in an error.
    pub is_error: bool,
}

/// Data for an extended thinking/reasoning block.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ThinkingData {
    /// The thinking/reasoning text.
    pub thinking: String,
    /// Optional cryptographic signature for Anthropic thinking blocks.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
    /// Whether this thinking block was redacted by the provider.
    #[serde(default)]
    pub redacted: bool,
}

/// Data for a redacted thinking block that must be round-tripped opaquely.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RedactedThinkingData {
    /// Opaque data that must be preserved and sent back to the provider.
    pub data: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- Serde roundtrip tests --

    #[test]
    fn text_roundtrip() {
        let part = ContentPart::text("hello world");
        let json = serde_json::to_string(&part).unwrap();
        let back: ContentPart = serde_json::from_str(&json).unwrap();
        assert_eq!(part, back);
    }

    #[test]
    fn image_roundtrip() {
        let part = ContentPart::Image(ImageData {
            source_type: ImageSourceType::Base64,
            media_type: Some("image/png".into()),
            data: "aWNvbg==".into(),
        });
        let json = serde_json::to_string(&part).unwrap();
        let back: ContentPart = serde_json::from_str(&json).unwrap();
        assert_eq!(part, back);
    }

    #[test]
    fn image_url_roundtrip() {
        let part = ContentPart::Image(ImageData {
            source_type: ImageSourceType::Url,
            media_type: None,
            data: "https://example.com/image.png".into(),
        });
        let json = serde_json::to_string(&part).unwrap();
        let back: ContentPart = serde_json::from_str(&json).unwrap();
        assert_eq!(part, back);
    }

    #[test]
    fn audio_roundtrip() {
        let part = ContentPart::Audio(AudioData {
            data: "YXVkaW8=".into(),
            format: "wav".into(),
        });
        let json = serde_json::to_string(&part).unwrap();
        let back: ContentPart = serde_json::from_str(&json).unwrap();
        assert_eq!(part, back);
    }

    #[test]
    fn document_roundtrip() {
        let part = ContentPart::Document(DocumentData {
            source_type: DocumentSourceType::Base64,
            media_type: Some("application/pdf".into()),
            data: "cGRm".into(),
            filename: Some("report.pdf".into()),
        });
        let json = serde_json::to_string(&part).unwrap();
        let back: ContentPart = serde_json::from_str(&json).unwrap();
        assert_eq!(part, back);
    }

    #[test]
    fn document_url_roundtrip() {
        let part = ContentPart::Document(DocumentData {
            source_type: DocumentSourceType::Url,
            media_type: None,
            data: "https://example.com/report.pdf".into(),
            filename: None,
        });
        let json = serde_json::to_string(&part).unwrap();
        let back: ContentPart = serde_json::from_str(&json).unwrap();
        assert_eq!(part, back);
    }

    #[test]
    fn tool_call_roundtrip() {
        let part = ContentPart::ToolCall(ToolCallData {
            id: "call_abc123".into(),
            name: "get_weather".into(),
            arguments: r#"{"location":"NYC"}"#.into(),
            raw_arguments: None,
        });
        let json = serde_json::to_string(&part).unwrap();
        let back: ContentPart = serde_json::from_str(&json).unwrap();
        assert_eq!(part, back);
    }

    #[test]
    fn tool_result_roundtrip() {
        let part = ContentPart::ToolResult(ToolResultData {
            tool_call_id: "call_abc123".into(),
            content: "72°F and sunny".into(),
            is_error: false,
        });
        let json = serde_json::to_string(&part).unwrap();
        let back: ContentPart = serde_json::from_str(&json).unwrap();
        assert_eq!(part, back);
    }

    #[test]
    fn tool_result_error_roundtrip() {
        let part = ContentPart::ToolResult(ToolResultData {
            tool_call_id: "call_fail".into(),
            content: "API key expired".into(),
            is_error: true,
        });
        let json = serde_json::to_string(&part).unwrap();
        let back: ContentPart = serde_json::from_str(&json).unwrap();
        assert_eq!(part, back);
    }

    #[test]
    fn thinking_roundtrip() {
        let part = ContentPart::Thinking(ThinkingData {
            thinking: "Let me reason through this...".into(),
            signature: Some("sig_xyz".into()),
            redacted: false,
        });
        let json = serde_json::to_string(&part).unwrap();
        let back: ContentPart = serde_json::from_str(&json).unwrap();
        assert_eq!(part, back);
    }

    #[test]
    fn thinking_without_signature_roundtrip() {
        let part = ContentPart::Thinking(ThinkingData {
            thinking: "Thinking without signature".into(),
            signature: None,
            redacted: false,
        });
        let json = serde_json::to_string(&part).unwrap();
        let back: ContentPart = serde_json::from_str(&json).unwrap();
        assert_eq!(part, back);
    }

    // -- Tag correctness tests --

    #[test]
    fn text_json_has_kind_tag() {
        let part = ContentPart::text("hi");
        let value: serde_json::Value = serde_json::to_value(&part).unwrap();
        assert_eq!(value["kind"], "text");
        assert_eq!(value["text"], "hi");
    }

    #[test]
    fn image_json_has_kind_tag() {
        let part = ContentPart::Image(ImageData {
            source_type: ImageSourceType::Base64,
            media_type: Some("image/jpeg".into()),
            data: "abc=".into(),
        });
        let value: serde_json::Value = serde_json::to_value(&part).unwrap();
        assert_eq!(value["kind"], "image");
        assert_eq!(value["source_type"], "base64");
    }

    #[test]
    fn audio_json_has_kind_tag() {
        let part = ContentPart::Audio(AudioData {
            data: "YXVkaW8=".into(),
            format: "mp3".into(),
        });
        let value: serde_json::Value = serde_json::to_value(&part).unwrap();
        assert_eq!(value["kind"], "audio");
        assert_eq!(value["format"], "mp3");
    }

    #[test]
    fn document_json_has_kind_tag() {
        let part = ContentPart::Document(DocumentData {
            source_type: DocumentSourceType::Url,
            media_type: None,
            data: "https://example.com/doc.pdf".into(),
            filename: None,
        });
        let value: serde_json::Value = serde_json::to_value(&part).unwrap();
        assert_eq!(value["kind"], "document");
        assert_eq!(value["source_type"], "url");
    }

    #[test]
    fn tool_call_json_has_kind_tag() {
        let part = ContentPart::ToolCall(ToolCallData {
            id: "tc_1".into(),
            name: "search".into(),
            arguments: "{}".into(),
            raw_arguments: None,
        });
        let value: serde_json::Value = serde_json::to_value(&part).unwrap();
        assert_eq!(value["kind"], "tool_call");
        assert_eq!(value["name"], "search");
    }

    #[test]
    fn tool_result_json_has_kind_tag() {
        let part = ContentPart::ToolResult(ToolResultData {
            tool_call_id: "tc_1".into(),
            content: "found it".into(),
            is_error: false,
        });
        let value: serde_json::Value = serde_json::to_value(&part).unwrap();
        assert_eq!(value["kind"], "tool_result");
        assert_eq!(value["is_error"], false);
    }

    #[test]
    fn thinking_json_has_kind_tag() {
        let part = ContentPart::Thinking(ThinkingData {
            thinking: "hmm".into(),
            signature: None,
            redacted: false,
        });
        let value: serde_json::Value = serde_json::to_value(&part).unwrap();
        assert_eq!(value["kind"], "thinking");
        assert_eq!(value["thinking"], "hmm");
        assert_eq!(value["signature"], serde_json::Value::Null);
    }

    // -- Deserialization from raw JSON with kind tag --

    #[test]
    fn deserialize_text_from_json() {
        let json = r#"{"kind":"text","text":"hello from json"}"#;
        let part: ContentPart = serde_json::from_str(json).unwrap();
        assert_eq!(part, ContentPart::text("hello from json"));
    }

    #[test]
    fn deserialize_image_from_json() {
        let json = r#"{"kind":"image","source_type":"url","media_type":null,"data":"https://img.example.com/cat.jpg"}"#;
        let part: ContentPart = serde_json::from_str(json).unwrap();
        assert_eq!(
            part,
            ContentPart::Image(ImageData {
                source_type: ImageSourceType::Url,
                media_type: None,
                data: "https://img.example.com/cat.jpg".into(),
            })
        );
    }

    #[test]
    fn deserialize_audio_from_json() {
        let json = r#"{"kind":"audio","data":"c291bmQ=","format":"wav"}"#;
        let part: ContentPart = serde_json::from_str(json).unwrap();
        assert_eq!(
            part,
            ContentPart::Audio(AudioData {
                data: "c291bmQ=".into(),
                format: "wav".into(),
            })
        );
    }

    #[test]
    fn deserialize_document_from_json() {
        let json = r#"{"kind":"document","source_type":"base64","media_type":"application/pdf","data":"cGRm","filename":"test.pdf"}"#;
        let part: ContentPart = serde_json::from_str(json).unwrap();
        assert_eq!(
            part,
            ContentPart::Document(DocumentData {
                source_type: DocumentSourceType::Base64,
                media_type: Some("application/pdf".into()),
                data: "cGRm".into(),
                filename: Some("test.pdf".into()),
            })
        );
    }

    #[test]
    fn deserialize_tool_call_from_json() {
        let json = r#"{"kind":"tool_call","id":"call_1","name":"calculator","arguments":"{\"expr\":\"2+2\"}"}"#;
        let part: ContentPart = serde_json::from_str(json).unwrap();
        assert_eq!(
            part,
            ContentPart::ToolCall(ToolCallData {
                id: "call_1".into(),
                name: "calculator".into(),
                arguments: r#"{"expr":"2+2"}"#.into(),
                raw_arguments: None,
            })
        );
    }

    #[test]
    fn deserialize_tool_result_from_json() {
        let json = r#"{"kind":"tool_result","tool_call_id":"call_1","content":"4","is_error":false}"#;
        let part: ContentPart = serde_json::from_str(json).unwrap();
        assert_eq!(
            part,
            ContentPart::ToolResult(ToolResultData {
                tool_call_id: "call_1".into(),
                content: "4".into(),
                is_error: false,
            })
        );
    }

    #[test]
    fn deserialize_thinking_from_json() {
        let json = r#"{"kind":"thinking","thinking":"step by step","signature":"sig_abc"}"#;
        let part: ContentPart = serde_json::from_str(json).unwrap();
        assert_eq!(
            part,
            ContentPart::Thinking(ThinkingData {
                thinking: "step by step".into(),
                signature: Some("sig_abc".into()),
                redacted: false,
            })
        );
    }

    #[test]
    fn unknown_kind_fails_to_deserialize() {
        let json = r#"{"kind":"video","data":"something"}"#;
        let result = serde_json::from_str::<ContentPart>(json);
        assert!(result.is_err());
    }

    // -- Convenience method tests --

    #[test]
    fn text_convenience_constructor() {
        let part = ContentPart::text("test");
        assert_eq!(part, ContentPart::Text { text: "test".into() });
    }

    #[test]
    fn text_from_string_owned() {
        let owned = String::from("owned string");
        let part = ContentPart::text(owned);
        assert_eq!(part.as_text(), Some("owned string"));
    }

    #[test]
    fn is_text_returns_true_for_text() {
        assert!(ContentPart::text("hello").is_text());
    }

    #[test]
    fn is_text_returns_false_for_non_text() {
        let part = ContentPart::Audio(AudioData {
            data: "YQ==".into(),
            format: "wav".into(),
        });
        assert!(!part.is_text());
    }

    #[test]
    fn as_text_returns_some_for_text() {
        let part = ContentPart::text("extract me");
        assert_eq!(part.as_text(), Some("extract me"));
    }

    #[test]
    fn as_text_returns_none_for_non_text() {
        let part = ContentPart::ToolCall(ToolCallData {
            id: "x".into(),
            name: "y".into(),
            arguments: "{}".into(),
            raw_arguments: None,
        });
        assert_eq!(part.as_text(), None);
    }

    #[test]
    fn is_tool_call_returns_true_for_tool_call() {
        let part = ContentPart::ToolCall(ToolCallData {
            id: "1".into(),
            name: "fn".into(),
            arguments: "{}".into(),
            raw_arguments: None,
        });
        assert!(part.is_tool_call());
    }

    #[test]
    fn is_tool_call_returns_false_for_non_tool_call() {
        assert!(!ContentPart::text("not a tool call").is_tool_call());
    }

    #[test]
    fn is_tool_result_returns_true_for_tool_result() {
        let part = ContentPart::ToolResult(ToolResultData {
            tool_call_id: "1".into(),
            content: "ok".into(),
            is_error: false,
        });
        assert!(part.is_tool_result());
    }

    #[test]
    fn is_tool_result_returns_false_for_non_tool_result() {
        assert!(!ContentPart::text("not a tool result").is_tool_result());
    }

    #[test]
    fn is_tool_result_returns_false_for_tool_call() {
        let part = ContentPart::ToolCall(ToolCallData {
            id: "1".into(),
            name: "fn".into(),
            arguments: "{}".into(),
            raw_arguments: None,
        });
        assert!(!part.is_tool_result());
    }

    // -- RedactedThinking tests --

    #[test]
    fn redacted_thinking_roundtrip() {
        let part = ContentPart::RedactedThinking(RedactedThinkingData {
            data: "opaque-base64-data-here".into(),
        });
        let json = serde_json::to_string(&part).unwrap();
        let back: ContentPart = serde_json::from_str(&json).unwrap();
        assert_eq!(part, back);
    }

    #[test]
    fn redacted_thinking_json_has_kind_tag() {
        let part = ContentPart::RedactedThinking(RedactedThinkingData {
            data: "secret-data".into(),
        });
        let value: serde_json::Value = serde_json::to_value(&part).unwrap();
        assert_eq!(value["kind"], "redacted_thinking");
        assert_eq!(value["data"], "secret-data");
    }

    #[test]
    fn deserialize_redacted_thinking_from_json() {
        let json = r#"{"kind":"redacted_thinking","data":"opaque-round-trip"}"#;
        let part: ContentPart = serde_json::from_str(json).unwrap();
        assert_eq!(
            part,
            ContentPart::RedactedThinking(RedactedThinkingData {
                data: "opaque-round-trip".into(),
            })
        );
    }

    #[test]
    fn redacted_thinking_data_preserves_opaque_data() {
        let original_data = "aBcDeFgHiJkLmNoPqRsTuVwXyZ0123456789+/=".to_string();
        let part = ContentPart::RedactedThinking(RedactedThinkingData {
            data: original_data.clone(),
        });
        let json = serde_json::to_string(&part).unwrap();
        let back: ContentPart = serde_json::from_str(&json).unwrap();
        match back {
            ContentPart::RedactedThinking(data) => {
                assert_eq!(data.data, original_data);
            }
            other => panic!("expected RedactedThinking, got {:?}", other),
        }
    }

    #[test]
    fn is_thinking_returns_true_for_thinking() {
        let part = ContentPart::Thinking(ThinkingData {
            thinking: "Let me think...".into(),
            signature: None,
            redacted: false,
        });
        assert!(part.is_thinking());
    }

    #[test]
    fn is_thinking_returns_false_for_non_thinking() {
        assert!(!ContentPart::text("not thinking").is_thinking());
    }

    #[test]
    fn is_thinking_returns_false_for_redacted_thinking() {
        let part = ContentPart::RedactedThinking(RedactedThinkingData {
            data: "opaque".into(),
        });
        assert!(!part.is_thinking());
    }

    #[test]
    fn is_redacted_thinking_returns_true_for_redacted_thinking() {
        let part = ContentPart::RedactedThinking(RedactedThinkingData {
            data: "opaque".into(),
        });
        assert!(part.is_redacted_thinking());
    }

    #[test]
    fn is_redacted_thinking_returns_false_for_thinking() {
        let part = ContentPart::Thinking(ThinkingData {
            thinking: "visible thinking".into(),
            signature: None,
            redacted: false,
        });
        assert!(!part.is_redacted_thinking());
    }

    #[test]
    fn is_redacted_thinking_returns_false_for_text() {
        assert!(!ContentPart::text("hello").is_redacted_thinking());
    }

    #[test]
    fn thinking_data_with_redacted_true() {
        let part = ContentPart::Thinking(ThinkingData {
            thinking: "partially visible".into(),
            signature: Some("sig_abc".into()),
            redacted: true,
        });
        let json = serde_json::to_string(&part).unwrap();
        let back: ContentPart = serde_json::from_str(&json).unwrap();
        match back {
            ContentPart::Thinking(data) => {
                assert!(data.redacted);
                assert_eq!(data.thinking, "partially visible");
                assert_eq!(data.signature, Some("sig_abc".into()));
            }
            other => panic!("expected Thinking, got {:?}", other),
        }
    }

    #[test]
    fn thinking_data_redacted_defaults_to_false() {
        let json = r#"{"kind":"thinking","thinking":"test","signature":null}"#;
        let part: ContentPart = serde_json::from_str(json).unwrap();
        match part {
            ContentPart::Thinking(data) => {
                assert!(!data.redacted);
            }
            other => panic!("expected Thinking, got {:?}", other),
        }
    }

    // -- ToolCallData raw_arguments tests --

    #[test]
    fn tool_call_data_raw_arguments_defaults_to_none() {
        let data = ToolCallData {
            id: "call_1".into(),
            name: "fn".into(),
            arguments: "{}".into(),
            raw_arguments: None,
        };
        assert!(data.raw_arguments.is_none());
    }

    #[test]
    fn tool_call_data_raw_arguments_can_hold_value() {
        let data = ToolCallData {
            id: "call_1".into(),
            name: "fn".into(),
            arguments: r#"{"key":"value"}"#.into(),
            raw_arguments: Some(r#"{ "key" : "value" }"#.into()),
        };
        assert_eq!(
            data.raw_arguments.as_deref(),
            Some(r#"{ "key" : "value" }"#)
        );
    }

    #[test]
    fn tool_call_data_serde_raw_arguments_omitted_when_none() {
        let data = ToolCallData {
            id: "call_1".into(),
            name: "fn".into(),
            arguments: "{}".into(),
            raw_arguments: None,
        };
        let value: serde_json::Value = serde_json::to_value(
            ContentPart::ToolCall(data)
        ).unwrap();
        let obj = value.as_object().unwrap();
        assert!(!obj.contains_key("raw_arguments"));
    }

    #[test]
    fn tool_call_data_serde_raw_arguments_present_when_set() {
        let data = ToolCallData {
            id: "call_1".into(),
            name: "fn".into(),
            arguments: "{}".into(),
            raw_arguments: Some("{ }".into()),
        };
        let value: serde_json::Value = serde_json::to_value(
            ContentPart::ToolCall(data)
        ).unwrap();
        assert_eq!(value["raw_arguments"], "{ }");
    }

    #[test]
    fn tool_call_data_serde_roundtrip_with_raw_arguments() {
        let part = ContentPart::ToolCall(ToolCallData {
            id: "call_rt".into(),
            name: "search".into(),
            arguments: r#"{"q":"rust"}"#.into(),
            raw_arguments: Some(r#"{ "q" : "rust" }"#.into()),
        });
        let json = serde_json::to_string(&part).unwrap();
        let back: ContentPart = serde_json::from_str(&json).unwrap();
        assert_eq!(part, back);
    }

    #[test]
    fn tool_call_data_raw_arguments_deserialized_when_absent() {
        let json = r#"{"kind":"tool_call","id":"call_1","name":"fn","arguments":"{}"}"#;
        let part: ContentPart = serde_json::from_str(json).unwrap();
        match part {
            ContentPart::ToolCall(data) => {
                assert!(data.raw_arguments.is_none());
            }
            other => panic!("expected ToolCall, got {:?}", other),
        }
    }
}
