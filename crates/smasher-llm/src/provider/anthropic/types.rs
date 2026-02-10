// ABOUTME: Native Anthropic Messages API serde types for request/response serialization.
// ABOUTME: Handles Anthropic-specific constructs like thinking blocks, cache_control, and content block tagging.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::types::{
    ContentPart, FinishReason, ImageSourceType, Message, Request, Response, Role, ToolChoice, Usage,
};

// ---------------------------------------------------------------------------
// Request types
// ---------------------------------------------------------------------------

/// Top-level request body for the Anthropic Messages API.
#[derive(Debug, Serialize)]
pub struct AnthropicRequest {
    pub model: String,
    pub messages: Vec<AnthropicMessage>,
    pub max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system: Option<Vec<AnthropicSystemBlock>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop_sequences: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<AnthropicTool>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<AnthropicToolChoice>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<AnthropicThinking>,
}

/// A system block in the top-level `system` array.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnthropicSystemBlock {
    #[serde(rename = "type")]
    pub block_type: String,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_control: Option<CacheControl>,
}

/// Cache control directive for prompt caching.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheControl {
    #[serde(rename = "type")]
    pub cache_type: String,
}

/// A single message in the Anthropic messages array.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnthropicMessage {
    pub role: String,
    pub content: Vec<AnthropicContentBlock>,
}

/// Tagged union of content block types sent to/from the Anthropic API.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum AnthropicContentBlock {
    #[serde(rename = "text")]
    Text {
        text: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_control: Option<CacheControl>,
    },

    #[serde(rename = "image")]
    Image {
        source: AnthropicImageSource,
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_control: Option<CacheControl>,
    },

    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        input: Value,
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_control: Option<CacheControl>,
    },

    #[serde(rename = "tool_result")]
    ToolResult {
        tool_use_id: String,
        content: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        is_error: Option<bool>,
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_control: Option<CacheControl>,
    },

    #[serde(rename = "thinking")]
    Thinking { thinking: String, signature: String },

    #[serde(rename = "redacted_thinking")]
    RedactedThinking { data: String },
}

/// Image source payload for the Anthropic image content block.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnthropicImageSource {
    #[serde(rename = "type")]
    pub source_type: String,
    pub media_type: String,
    pub data: String,
}

/// A tool definition in the Anthropic format.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnthropicTool {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

/// Tool choice specification for the Anthropic API.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum AnthropicToolChoice {
    #[serde(rename = "auto")]
    Auto,
    #[serde(rename = "any")]
    Any,
    #[serde(rename = "tool")]
    Tool { name: String },
}

/// Thinking configuration for extended reasoning.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnthropicThinking {
    #[serde(rename = "type")]
    pub thinking_type: String,
    pub budget_tokens: u32,
}

// ---------------------------------------------------------------------------
// Response types
// ---------------------------------------------------------------------------

/// Top-level response body from the Anthropic Messages API.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AnthropicResponse {
    pub id: String,
    pub model: String,
    pub content: Vec<AnthropicContentBlock>,
    pub stop_reason: Option<String>,
    pub usage: AnthropicUsage,
}

/// Token usage counters returned by Anthropic, including cache metrics.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct AnthropicUsage {
    #[serde(default)]
    pub input_tokens: u32,
    #[serde(default)]
    pub output_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_read_input_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_creation_input_tokens: Option<u32>,
}

// ---------------------------------------------------------------------------
// Stream event types
// ---------------------------------------------------------------------------

/// A single server-sent event from the Anthropic streaming API.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type")]
pub enum AnthropicStreamEvent {
    #[serde(rename = "message_start")]
    MessageStart { message: AnthropicResponse },

    #[serde(rename = "content_block_start")]
    ContentBlockStart {
        index: u32,
        content_block: AnthropicContentBlock,
    },

    #[serde(rename = "content_block_delta")]
    ContentBlockDelta { index: u32, delta: AnthropicDelta },

    #[serde(rename = "content_block_stop")]
    ContentBlockStop { index: u32 },

    #[serde(rename = "message_delta")]
    MessageDelta {
        delta: AnthropicMessageDelta,
        usage: Option<AnthropicUsage>,
    },

    #[serde(rename = "message_stop")]
    MessageStop,

    #[serde(rename = "ping")]
    Ping,

    #[serde(rename = "error")]
    Error { error: AnthropicErrorDetail },
}

/// A content delta within a streaming content block.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type")]
pub enum AnthropicDelta {
    #[serde(rename = "text_delta")]
    TextDelta { text: String },

    #[serde(rename = "input_json_delta")]
    InputJsonDelta { partial_json: String },

    #[serde(rename = "thinking_delta")]
    ThinkingDelta { thinking: String },

    #[serde(rename = "signature_delta")]
    SignatureDelta { signature: String },
}

/// Message-level delta carrying the stop reason.
#[derive(Debug, Clone, Deserialize)]
pub struct AnthropicMessageDelta {
    pub stop_reason: Option<String>,
}

/// Error detail payload in an error stream event.
#[derive(Debug, Clone, Deserialize)]
pub struct AnthropicErrorDetail {
    #[serde(rename = "type")]
    pub error_type: String,
    pub message: String,
}

// ---------------------------------------------------------------------------
// Conversion: unified Request -> AnthropicRequest
// ---------------------------------------------------------------------------

/// Default max_tokens when the caller does not specify one.
const DEFAULT_MAX_TOKENS: u32 = 4096;

/// Convert a unified `Request` into an `AnthropicRequest`.
///
/// System messages are extracted from the messages vec and placed in the
/// top-level `system` field as Anthropic requires.
pub fn convert_request(request: &Request) -> AnthropicRequest {
    // Collect system-level content from both the explicit system_prompt field
    // and any system-role messages in the messages vec.
    let mut system_blocks: Vec<AnthropicSystemBlock> = Vec::new();

    if let Some(ref prompt) = request.system_prompt {
        system_blocks.push(AnthropicSystemBlock {
            block_type: "text".into(),
            text: prompt.clone(),
            cache_control: None,
        });
    }

    // Convert non-system messages and extract system messages.
    let mut messages: Vec<AnthropicMessage> = Vec::new();

    for msg in &request.messages {
        if msg.role == Role::System {
            // Extract text content parts as system blocks.
            for part in &msg.content {
                if let Some(text) = part.as_text() {
                    system_blocks.push(AnthropicSystemBlock {
                        block_type: "text".into(),
                        text: text.to_string(),
                        cache_control: None,
                    });
                }
            }
            continue;
        }

        messages.push(convert_message(msg));
    }

    // Check if tool_choice is None (meaning "do not use tools").
    // Anthropic has no direct "none" tool_choice, so we omit both tool_choice
    // and tools entirely to prevent the model from using tools.
    let is_tool_choice_none = matches!(request.tool_choice, Some(ToolChoice::None));

    // Build tools list (omitted when ToolChoice::None to prevent tool use).
    let tools = if is_tool_choice_none {
        None
    } else {
        request.tools.as_ref().map(|tools| {
            tools
                .iter()
                .map(|t| AnthropicTool {
                    name: t.name.clone(),
                    description: t.description.clone(),
                    input_schema: t.parameters.clone(),
                })
                .collect()
        })
    };

    // Map tool choice (None variant omits tool_choice entirely).
    let tool_choice = request
        .tool_choice
        .as_ref()
        .and_then(|choice| match choice {
            ToolChoice::Auto => Some(AnthropicToolChoice::Auto),
            ToolChoice::None => None,
            ToolChoice::Required => Some(AnthropicToolChoice::Any),
            ToolChoice::Specific { name } => Some(AnthropicToolChoice::Tool { name: name.clone() }),
        });

    // Map thinking config.
    let thinking = request.thinking.as_ref().and_then(|config| {
        if config.enabled {
            Some(AnthropicThinking {
                thinking_type: "enabled".into(),
                budget_tokens: config.budget_tokens.unwrap_or(DEFAULT_MAX_TOKENS),
            })
        } else {
            None
        }
    });

    AnthropicRequest {
        model: request.model.clone(),
        messages,
        max_tokens: request.max_tokens.unwrap_or(DEFAULT_MAX_TOKENS),
        system: if system_blocks.is_empty() {
            None
        } else {
            Some(system_blocks)
        },
        temperature: request.temperature,
        top_p: request.top_p,
        stop_sequences: request.stop_sequences.clone(),
        tools,
        tool_choice,
        stream: request.stream,
        thinking,
    }
}

/// Inject prompt caching hints for Anthropic's caching system.
/// Places cache_control breakpoints on:
/// 1. The last system prompt block (if present)
/// 2. The last content block of the last user message
///
/// Anthropic's prompt caching uses a prefix-based approach: everything up to and
/// including a cache_control breakpoint is eligible for caching. By placing
/// breakpoints on the system prompt and the trailing edge of the conversation,
/// agentic workloads achieve up to 90% cost reduction on cached prefixes.
pub fn inject_cache_control(request: &mut AnthropicRequest) {
    let ephemeral = CacheControl {
        cache_type: "ephemeral".into(),
    };

    // Mark the last system block for caching.
    if let Some(ref mut blocks) = request.system
        && let Some(last) = blocks.last_mut()
    {
        last.cache_control = Some(ephemeral.clone());
    }

    // Mark the last content block of the last user-role message for caching.
    if let Some(last_user_msg) = request.messages.iter_mut().rev().find(|m| m.role == "user")
        && let Some(last_block) = last_user_msg.content.last_mut()
    {
        set_cache_control(last_block, ephemeral);
    }
}

/// Set cache_control on a content block. Thinking blocks do not support caching.
fn set_cache_control(block: &mut AnthropicContentBlock, cc: CacheControl) {
    match block {
        AnthropicContentBlock::Text { cache_control, .. } => *cache_control = Some(cc),
        AnthropicContentBlock::Image { cache_control, .. } => *cache_control = Some(cc),
        AnthropicContentBlock::ToolUse { cache_control, .. } => *cache_control = Some(cc),
        AnthropicContentBlock::ToolResult { cache_control, .. } => *cache_control = Some(cc),
        AnthropicContentBlock::Thinking { .. } => {
            // Thinking blocks do not support cache_control.
        }
        AnthropicContentBlock::RedactedThinking { .. } => {
            // Redacted thinking blocks do not support cache_control.
        }
    }
}

/// Convert a single unified `Message` to an `AnthropicMessage`.
fn convert_message(msg: &Message) -> AnthropicMessage {
    let role = match msg.role {
        Role::User | Role::Tool => "user".to_string(),
        Role::Assistant => "assistant".to_string(),
        // Developer role maps to user in Anthropic.
        Role::Developer => "user".to_string(),
        // System should already be filtered out, but handle gracefully.
        Role::System => "user".to_string(),
    };

    let content = msg.content.iter().map(convert_content_part).collect();

    AnthropicMessage { role, content }
}

/// Convert a unified `ContentPart` to an `AnthropicContentBlock`.
fn convert_content_part(part: &ContentPart) -> AnthropicContentBlock {
    match part {
        ContentPart::Text { text } => AnthropicContentBlock::Text {
            text: text.clone(),
            cache_control: None,
        },

        ContentPart::Image(img) => match img.source_type {
            ImageSourceType::Base64 => AnthropicContentBlock::Image {
                source: AnthropicImageSource {
                    source_type: "base64".into(),
                    media_type: img.media_type.clone().unwrap_or_else(|| "image/png".into()),
                    data: img.data.clone(),
                },
                cache_control: None,
            },
            ImageSourceType::Url => {
                // Anthropic does not natively support URL images via the same block;
                // we embed it as text with the URL for compatibility.
                AnthropicContentBlock::Text {
                    text: img.data.clone(),
                    cache_control: None,
                }
            }
        },

        ContentPart::ToolCall(tc) => {
            // Parse the arguments string to a JSON Value for Anthropic's `input` field.
            let input = serde_json::from_str(&tc.arguments)
                .unwrap_or(Value::Object(serde_json::Map::new()));
            AnthropicContentBlock::ToolUse {
                id: tc.id.clone(),
                name: tc.name.clone(),
                input,
                cache_control: None,
            }
        }

        ContentPart::ToolResult(tr) => AnthropicContentBlock::ToolResult {
            tool_use_id: tr.tool_call_id.clone(),
            content: tr.content.clone(),
            is_error: if tr.is_error { Some(true) } else { None },
            cache_control: None,
        },

        ContentPart::Thinking(th) => AnthropicContentBlock::Thinking {
            thinking: th.thinking.clone(),
            signature: th.signature.clone().unwrap_or_default(),
        },

        ContentPart::Audio(audio) => {
            // Anthropic doesn't support audio blocks natively; send description.
            AnthropicContentBlock::Text {
                text: format!(
                    "[Audio: {} format, {} bytes]",
                    audio.format,
                    audio.data.len()
                ),
                cache_control: None,
            }
        }

        ContentPart::Document(doc) => {
            // Anthropic doesn't support document blocks natively; send description.
            AnthropicContentBlock::Text {
                text: format!(
                    "[Document: {}]",
                    doc.filename.as_deref().unwrap_or("unnamed")
                ),
                cache_control: None,
            }
        }

        ContentPart::RedactedThinking(rt) => AnthropicContentBlock::RedactedThinking {
            data: rt.data.clone(),
        },
    }
}

// ---------------------------------------------------------------------------
// Conversion: AnthropicResponse -> unified Response
// ---------------------------------------------------------------------------

/// Map an Anthropic stop reason string to a unified `FinishReason`.
pub fn map_stop_reason(reason: &str) -> FinishReason {
    match reason {
        "end_turn" => FinishReason::Stop,
        "max_tokens" => FinishReason::Length,
        "tool_use" => FinishReason::ToolUse,
        "content_filter" => FinishReason::ContentFilter,
        _ => FinishReason::Stop,
    }
}

/// Convert an `AnthropicResponse` into a unified `Response`.
pub fn convert_response(response: AnthropicResponse) -> Response {
    let content = response
        .content
        .into_iter()
        .map(convert_content_block_to_part)
        .collect();

    let finish_reason = response.stop_reason.as_deref().map(map_stop_reason);

    let usage = convert_usage(&response.usage);

    Response {
        id: response.id,
        model: response.model,
        content,
        finish_reason,
        usage,
        warnings: vec![],
        rate_limit: None,
        provider: Some("anthropic".to_string()),
        raw: None,
    }
}

/// Convert an `AnthropicContentBlock` back to a unified `ContentPart`.
fn convert_content_block_to_part(block: AnthropicContentBlock) -> ContentPart {
    match block {
        AnthropicContentBlock::Text { text, .. } => ContentPart::text(text),

        AnthropicContentBlock::Image { source, .. } => {
            ContentPart::Image(crate::types::ImageData {
                source_type: ImageSourceType::Base64,
                media_type: Some(source.media_type),
                data: source.data,
            })
        }

        AnthropicContentBlock::ToolUse {
            id, name, input, ..
        } => ContentPart::ToolCall(crate::types::ToolCallData {
            id,
            name,
            arguments: serde_json::to_string(&input).unwrap_or_default(),
            raw_arguments: None,
        }),

        AnthropicContentBlock::ToolResult {
            tool_use_id,
            content,
            is_error,
            ..
        } => ContentPart::ToolResult(crate::types::ToolResultData {
            tool_call_id: tool_use_id,
            content,
            is_error: is_error.unwrap_or(false),
        }),

        AnthropicContentBlock::Thinking {
            thinking,
            signature,
        } => ContentPart::Thinking(crate::types::ThinkingData {
            thinking,
            signature: if signature.is_empty() {
                None
            } else {
                Some(signature)
            },
            redacted: false,
        }),

        AnthropicContentBlock::RedactedThinking { data } => {
            ContentPart::RedactedThinking(crate::types::RedactedThinkingData { data })
        }
    }
}

/// Convert Anthropic usage counters to the unified `Usage` type.
pub fn convert_usage(usage: &AnthropicUsage) -> Usage {
    Usage {
        input_tokens: usage.input_tokens,
        output_tokens: usage.output_tokens,
        cache_read_tokens: usage.cache_read_input_tokens,
        cache_creation_tokens: usage.cache_creation_input_tokens,
        reasoning_tokens: None,
        total_tokens: None,
        raw: None,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{
        ContentPart, ImageData, ImageSourceType, ThinkingConfig, ToolCallData, ToolChoice,
        ToolDefinition, ToolResultData,
    };
    use serde_json::json;

    // -- Request conversion tests --

    fn simple_request() -> Request {
        Request::new("claude-sonnet-4-20250514", vec![Message::user("Hello")])
    }

    #[test]
    fn convert_request_basic_text_message() {
        let req = simple_request();
        let anthropic = convert_request(&req);

        assert_eq!(anthropic.model, "claude-sonnet-4-20250514");
        assert_eq!(anthropic.messages.len(), 1);
        assert_eq!(anthropic.messages[0].role, "user");
        assert_eq!(anthropic.max_tokens, DEFAULT_MAX_TOKENS);
        assert!(anthropic.system.is_none());
    }

    #[test]
    fn convert_request_explicit_max_tokens() {
        let req = simple_request().max_tokens(1024);
        let anthropic = convert_request(&req);
        assert_eq!(anthropic.max_tokens, 1024);
    }

    #[test]
    fn convert_request_system_prompt_extraction() {
        let req = simple_request().system_prompt("You are helpful.");
        let anthropic = convert_request(&req);

        let system = anthropic.system.expect("should have system blocks");
        assert_eq!(system.len(), 1);
        assert_eq!(system[0].text, "You are helpful.");
        assert_eq!(system[0].block_type, "text");
    }

    #[test]
    fn convert_request_system_message_extraction() {
        let req = Request::new(
            "claude-sonnet-4-20250514",
            vec![
                Message::system("System instruction"),
                Message::user("Hello"),
            ],
        );
        let anthropic = convert_request(&req);

        let system = anthropic.system.expect("should have system blocks");
        assert_eq!(system.len(), 1);
        assert_eq!(system[0].text, "System instruction");
        // System message should be removed from messages.
        assert_eq!(anthropic.messages.len(), 1);
        assert_eq!(anthropic.messages[0].role, "user");
    }

    #[test]
    fn convert_request_both_system_prompt_and_system_message() {
        let req = Request::new(
            "claude-sonnet-4-20250514",
            vec![Message::system("From messages"), Message::user("Hello")],
        )
        .system_prompt("From field");
        let anthropic = convert_request(&req);

        let system = anthropic.system.expect("should have system blocks");
        assert_eq!(system.len(), 2);
        assert_eq!(system[0].text, "From field");
        assert_eq!(system[1].text, "From messages");
    }

    #[test]
    fn convert_request_tool_definitions() {
        let tool = ToolDefinition::new(
            "get_weather",
            "Get weather for a location",
            json!({"type": "object", "properties": {"location": {"type": "string"}}}),
        );
        let req = simple_request().tools(vec![tool]);
        let anthropic = convert_request(&req);

        let tools = anthropic.tools.expect("should have tools");
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "get_weather");
        assert_eq!(tools[0].description, "Get weather for a location");
    }

    #[test]
    fn convert_request_tool_choice_auto() {
        let req = simple_request().tool_choice(ToolChoice::Auto);
        let anthropic = convert_request(&req);
        let choice = anthropic.tool_choice.expect("should have tool_choice");
        assert!(matches!(choice, AnthropicToolChoice::Auto));
    }

    #[test]
    fn convert_request_tool_choice_required_maps_to_any() {
        let req = simple_request().tool_choice(ToolChoice::Required);
        let anthropic = convert_request(&req);
        let choice = anthropic.tool_choice.expect("should have tool_choice");
        assert!(matches!(choice, AnthropicToolChoice::Any));
    }

    #[test]
    fn convert_request_tool_choice_none_omits_tools_and_tool_choice() {
        // ToolChoice::None should omit both tool_choice and tools from the
        // request, preventing the model from using any tools. Anthropic has
        // no native "none" tool_choice, so omitting both fields is the
        // correct way to disable tool use.
        let tool = ToolDefinition::new(
            "get_weather",
            "Get weather for a location",
            json!({"type": "object", "properties": {"location": {"type": "string"}}}),
        );
        let req = simple_request()
            .tools(vec![tool])
            .tool_choice(ToolChoice::None);
        let anthropic = convert_request(&req);

        assert!(
            anthropic.tool_choice.is_none(),
            "tool_choice should be None when ToolChoice::None"
        );
        assert!(
            anthropic.tools.is_none(),
            "tools should be None when ToolChoice::None to prevent tool use"
        );
    }

    #[test]
    fn convert_request_tool_choice_specific() {
        let req = simple_request().tool_choice(ToolChoice::Specific {
            name: "get_weather".into(),
        });
        let anthropic = convert_request(&req);
        let choice = anthropic.tool_choice.expect("should have tool_choice");
        match choice {
            AnthropicToolChoice::Tool { name } => assert_eq!(name, "get_weather"),
            other => panic!("expected Tool, got {:?}", other),
        }
    }

    #[test]
    fn convert_request_thinking_enabled() {
        let req = simple_request().thinking(ThinkingConfig {
            enabled: true,
            budget_tokens: Some(10000),
        });
        let anthropic = convert_request(&req);
        let thinking = anthropic.thinking.expect("should have thinking");
        assert_eq!(thinking.thinking_type, "enabled");
        assert_eq!(thinking.budget_tokens, 10000);
    }

    #[test]
    fn convert_request_thinking_disabled() {
        let req = simple_request().thinking(ThinkingConfig {
            enabled: false,
            budget_tokens: None,
        });
        let anthropic = convert_request(&req);
        assert!(anthropic.thinking.is_none());
    }

    #[test]
    fn convert_request_tool_call_content() {
        let msg = Message {
            role: Role::Assistant,
            content: vec![ContentPart::ToolCall(ToolCallData {
                id: "toolu_123".into(),
                name: "search".into(),
                arguments: r#"{"query":"rust"}"#.into(),
                raw_arguments: None,
            })],
            name: None,
            tool_call_id: None,
        };
        let req = Request::new("claude-sonnet-4-20250514", vec![msg]);
        let anthropic = convert_request(&req);

        match &anthropic.messages[0].content[0] {
            AnthropicContentBlock::ToolUse {
                id, name, input, ..
            } => {
                assert_eq!(id, "toolu_123");
                assert_eq!(name, "search");
                assert_eq!(input["query"], "rust");
            }
            other => panic!("expected ToolUse, got {:?}", other),
        }
    }

    #[test]
    fn convert_request_tool_result_content() {
        let msg = Message {
            role: Role::User,
            content: vec![ContentPart::ToolResult(ToolResultData {
                tool_call_id: "toolu_123".into(),
                content: "Found 42 results".into(),
                is_error: false,
            })],
            name: None,
            tool_call_id: None,
        };
        let req = Request::new("claude-sonnet-4-20250514", vec![msg]);
        let anthropic = convert_request(&req);

        match &anthropic.messages[0].content[0] {
            AnthropicContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
                ..
            } => {
                assert_eq!(tool_use_id, "toolu_123");
                assert_eq!(content, "Found 42 results");
                assert!(is_error.is_none());
            }
            other => panic!("expected ToolResult, got {:?}", other),
        }
    }

    #[test]
    fn convert_request_tool_result_with_error() {
        let msg = Message {
            role: Role::User,
            content: vec![ContentPart::ToolResult(ToolResultData {
                tool_call_id: "toolu_456".into(),
                content: "API error".into(),
                is_error: true,
            })],
            name: None,
            tool_call_id: None,
        };
        let req = Request::new("claude-sonnet-4-20250514", vec![msg]);
        let anthropic = convert_request(&req);

        match &anthropic.messages[0].content[0] {
            AnthropicContentBlock::ToolResult { is_error, .. } => {
                assert_eq!(*is_error, Some(true));
            }
            other => panic!("expected ToolResult, got {:?}", other),
        }
    }

    #[test]
    fn convert_request_image_base64() {
        let msg = Message {
            role: Role::User,
            content: vec![ContentPart::Image(ImageData {
                source_type: ImageSourceType::Base64,
                media_type: Some("image/png".into()),
                data: "iVBOR...".into(),
            })],
            name: None,
            tool_call_id: None,
        };
        let req = Request::new("claude-sonnet-4-20250514", vec![msg]);
        let anthropic = convert_request(&req);

        match &anthropic.messages[0].content[0] {
            AnthropicContentBlock::Image { source, .. } => {
                assert_eq!(source.source_type, "base64");
                assert_eq!(source.media_type, "image/png");
                assert_eq!(source.data, "iVBOR...");
            }
            other => panic!("expected Image, got {:?}", other),
        }
    }

    #[test]
    fn convert_request_sampling_params() {
        let req = simple_request()
            .temperature(0.7)
            .top_p(0.9)
            .stop_sequences(vec!["STOP".into()]);
        let anthropic = convert_request(&req);

        assert_eq!(anthropic.temperature, Some(0.7));
        assert_eq!(anthropic.top_p, Some(0.9));
        assert_eq!(anthropic.stop_sequences, Some(vec!["STOP".to_string()]));
    }

    // -- Response conversion tests --

    #[test]
    fn convert_response_text() {
        let resp = AnthropicResponse {
            id: "msg_abc".into(),
            model: "claude-sonnet-4-20250514".into(),
            content: vec![AnthropicContentBlock::Text {
                text: "Hello!".into(),
                cache_control: None,
            }],
            stop_reason: Some("end_turn".into()),
            usage: AnthropicUsage {
                input_tokens: 10,
                output_tokens: 5,
                cache_read_input_tokens: None,
                cache_creation_input_tokens: None,
            },
        };
        let unified = convert_response(resp);

        assert_eq!(unified.id, "msg_abc");
        assert_eq!(unified.model, "claude-sonnet-4-20250514");
        assert_eq!(unified.text().as_deref(), Some("Hello!"));
        assert_eq!(unified.finish_reason, Some(FinishReason::Stop));
        assert_eq!(unified.usage.input_tokens, 10);
        assert_eq!(unified.usage.output_tokens, 5);
    }

    #[test]
    fn convert_response_tool_use() {
        let resp = AnthropicResponse {
            id: "msg_def".into(),
            model: "claude-sonnet-4-20250514".into(),
            content: vec![AnthropicContentBlock::ToolUse {
                id: "toolu_789".into(),
                name: "get_weather".into(),
                input: json!({"location": "NYC"}),
                cache_control: None,
            }],
            stop_reason: Some("tool_use".into()),
            usage: AnthropicUsage {
                input_tokens: 20,
                output_tokens: 15,
                cache_read_input_tokens: None,
                cache_creation_input_tokens: None,
            },
        };
        let unified = convert_response(resp);

        assert_eq!(unified.finish_reason, Some(FinishReason::ToolUse));
        assert!(unified.has_tool_calls());
        let calls = unified.tool_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].id, "toolu_789");
        assert_eq!(calls[0].name, "get_weather");
        assert!(calls[0].arguments.contains("NYC"));
    }

    #[test]
    fn convert_response_stop_reason_mapping() {
        assert_eq!(map_stop_reason("end_turn"), FinishReason::Stop);
        assert_eq!(map_stop_reason("max_tokens"), FinishReason::Length);
        assert_eq!(map_stop_reason("tool_use"), FinishReason::ToolUse);
        assert_eq!(
            map_stop_reason("content_filter"),
            FinishReason::ContentFilter
        );
        assert_eq!(map_stop_reason("unknown_reason"), FinishReason::Stop);
    }

    #[test]
    fn convert_response_usage_with_cache() {
        let resp = AnthropicResponse {
            id: "msg_cache".into(),
            model: "claude-sonnet-4-20250514".into(),
            content: vec![AnthropicContentBlock::Text {
                text: "cached".into(),
                cache_control: None,
            }],
            stop_reason: Some("end_turn".into()),
            usage: AnthropicUsage {
                input_tokens: 100,
                output_tokens: 50,
                cache_read_input_tokens: Some(80),
                cache_creation_input_tokens: Some(20),
            },
        };
        let unified = convert_response(resp);

        assert_eq!(unified.usage.input_tokens, 100);
        assert_eq!(unified.usage.output_tokens, 50);
        assert_eq!(unified.usage.cache_read_tokens, Some(80));
        assert_eq!(unified.usage.cache_creation_tokens, Some(20));
        assert_eq!(unified.usage.reasoning_tokens, None);
    }

    #[test]
    fn convert_response_thinking_block() {
        let resp = AnthropicResponse {
            id: "msg_think".into(),
            model: "claude-sonnet-4-20250514".into(),
            content: vec![
                AnthropicContentBlock::Thinking {
                    thinking: "Let me reason...".into(),
                    signature: "sig_abc".into(),
                },
                AnthropicContentBlock::Text {
                    text: "The answer is 42.".into(),
                    cache_control: None,
                },
            ],
            stop_reason: Some("end_turn".into()),
            usage: AnthropicUsage::default(),
        };
        let unified = convert_response(resp);

        assert_eq!(unified.content.len(), 2);
        match &unified.content[0] {
            ContentPart::Thinking(data) => {
                assert_eq!(data.thinking, "Let me reason...");
                assert_eq!(data.signature, Some("sig_abc".into()));
            }
            other => panic!("expected Thinking, got {:?}", other),
        }
        assert_eq!(unified.text().as_deref(), Some("The answer is 42."));
    }

    #[test]
    fn convert_response_no_stop_reason() {
        let resp = AnthropicResponse {
            id: "msg_none".into(),
            model: "claude-sonnet-4-20250514".into(),
            content: vec![],
            stop_reason: None,
            usage: AnthropicUsage::default(),
        };
        let unified = convert_response(resp);
        assert!(unified.finish_reason.is_none());
    }

    // -- Serde roundtrip tests on native types --

    #[test]
    fn content_block_text_serialization() {
        let block = AnthropicContentBlock::Text {
            text: "hello".into(),
            cache_control: None,
        };
        let json = serde_json::to_value(&block).unwrap();
        assert_eq!(json["type"], "text");
        assert_eq!(json["text"], "hello");
    }

    #[test]
    fn content_block_tool_use_serialization() {
        let block = AnthropicContentBlock::ToolUse {
            id: "toolu_1".into(),
            name: "search".into(),
            input: json!({"q": "rust"}),
            cache_control: None,
        };
        let json = serde_json::to_value(&block).unwrap();
        assert_eq!(json["type"], "tool_use");
        assert_eq!(json["id"], "toolu_1");
        assert_eq!(json["name"], "search");
        assert_eq!(json["input"]["q"], "rust");
    }

    #[test]
    fn content_block_tool_result_serialization() {
        let block = AnthropicContentBlock::ToolResult {
            tool_use_id: "toolu_1".into(),
            content: "42 results".into(),
            is_error: None,
            cache_control: None,
        };
        let json = serde_json::to_value(&block).unwrap();
        assert_eq!(json["type"], "tool_result");
        assert_eq!(json["tool_use_id"], "toolu_1");
        assert_eq!(json["content"], "42 results");
        // is_error should be absent when None.
        assert!(json.get("is_error").is_none());
    }

    #[test]
    fn content_block_thinking_serialization() {
        let block = AnthropicContentBlock::Thinking {
            thinking: "hmm".into(),
            signature: "sig_x".into(),
        };
        let json = serde_json::to_value(&block).unwrap();
        assert_eq!(json["type"], "thinking");
        assert_eq!(json["thinking"], "hmm");
        assert_eq!(json["signature"], "sig_x");
    }

    #[test]
    fn content_block_image_serialization() {
        let block = AnthropicContentBlock::Image {
            source: AnthropicImageSource {
                source_type: "base64".into(),
                media_type: "image/png".into(),
                data: "abc123".into(),
            },
            cache_control: None,
        };
        let json = serde_json::to_value(&block).unwrap();
        assert_eq!(json["type"], "image");
        assert_eq!(json["source"]["type"], "base64");
        assert_eq!(json["source"]["media_type"], "image/png");
    }

    #[test]
    fn tool_choice_auto_serialization() {
        let choice = AnthropicToolChoice::Auto;
        let json = serde_json::to_value(&choice).unwrap();
        assert_eq!(json["type"], "auto");
    }

    #[test]
    fn tool_choice_any_serialization() {
        let choice = AnthropicToolChoice::Any;
        let json = serde_json::to_value(&choice).unwrap();
        assert_eq!(json["type"], "any");
    }

    #[test]
    fn tool_choice_tool_serialization() {
        let choice = AnthropicToolChoice::Tool {
            name: "get_weather".into(),
        };
        let json = serde_json::to_value(&choice).unwrap();
        assert_eq!(json["type"], "tool");
        assert_eq!(json["name"], "get_weather");
    }

    #[test]
    fn thinking_config_serialization() {
        let thinking = AnthropicThinking {
            thinking_type: "enabled".into(),
            budget_tokens: 10000,
        };
        let json = serde_json::to_value(&thinking).unwrap();
        assert_eq!(json["type"], "enabled");
        assert_eq!(json["budget_tokens"], 10000);
    }

    #[test]
    fn system_block_serialization() {
        let block = AnthropicSystemBlock {
            block_type: "text".into(),
            text: "Be helpful".into(),
            cache_control: Some(CacheControl {
                cache_type: "ephemeral".into(),
            }),
        };
        let json = serde_json::to_value(&block).unwrap();
        assert_eq!(json["type"], "text");
        assert_eq!(json["text"], "Be helpful");
        assert_eq!(json["cache_control"]["type"], "ephemeral");
    }

    #[test]
    fn system_block_without_cache_control() {
        let block = AnthropicSystemBlock {
            block_type: "text".into(),
            text: "Be helpful".into(),
            cache_control: None,
        };
        let json = serde_json::to_value(&block).unwrap();
        assert_eq!(json["type"], "text");
        assert!(json.get("cache_control").is_none());
    }

    #[test]
    fn content_block_text_roundtrip() {
        let block = AnthropicContentBlock::Text {
            text: "hello".into(),
            cache_control: None,
        };
        let json = serde_json::to_string(&block).unwrap();
        let back: AnthropicContentBlock = serde_json::from_str(&json).unwrap();
        match back {
            AnthropicContentBlock::Text { text, .. } => assert_eq!(text, "hello"),
            other => panic!("expected Text, got {:?}", other),
        }
    }

    #[test]
    fn content_block_tool_use_roundtrip() {
        let block = AnthropicContentBlock::ToolUse {
            id: "toolu_1".into(),
            name: "search".into(),
            input: json!({"q": "test"}),
            cache_control: None,
        };
        let json = serde_json::to_string(&block).unwrap();
        let back: AnthropicContentBlock = serde_json::from_str(&json).unwrap();
        match back {
            AnthropicContentBlock::ToolUse {
                id, name, input, ..
            } => {
                assert_eq!(id, "toolu_1");
                assert_eq!(name, "search");
                assert_eq!(input["q"], "test");
            }
            other => panic!("expected ToolUse, got {:?}", other),
        }
    }

    #[test]
    fn anthropic_response_deserialization() {
        let json = json!({
            "id": "msg_01",
            "model": "claude-sonnet-4-20250514",
            "content": [
                {"type": "text", "text": "Hello"}
            ],
            "stop_reason": "end_turn",
            "usage": {
                "input_tokens": 10,
                "output_tokens": 5
            }
        });
        let resp: AnthropicResponse = serde_json::from_value(json).unwrap();
        assert_eq!(resp.id, "msg_01");
        assert_eq!(resp.content.len(), 1);
        assert_eq!(resp.stop_reason, Some("end_turn".into()));
        assert_eq!(resp.usage.input_tokens, 10);
    }

    #[test]
    fn anthropic_usage_deserialization_with_cache() {
        let json = json!({
            "input_tokens": 100,
            "output_tokens": 50,
            "cache_read_input_tokens": 80,
            "cache_creation_input_tokens": 20
        });
        let usage: AnthropicUsage = serde_json::from_value(json).unwrap();
        assert_eq!(usage.input_tokens, 100);
        assert_eq!(usage.output_tokens, 50);
        assert_eq!(usage.cache_read_input_tokens, Some(80));
        assert_eq!(usage.cache_creation_input_tokens, Some(20));
    }

    #[test]
    fn anthropic_usage_deserialization_without_cache() {
        let json = json!({
            "input_tokens": 100,
            "output_tokens": 50
        });
        let usage: AnthropicUsage = serde_json::from_value(json).unwrap();
        assert_eq!(usage.input_tokens, 100);
        assert_eq!(usage.output_tokens, 50);
        assert!(usage.cache_read_input_tokens.is_none());
        assert!(usage.cache_creation_input_tokens.is_none());
    }

    // -- Stream event deserialization tests --

    #[test]
    fn stream_event_message_start_deserialization() {
        let json = json!({
            "type": "message_start",
            "message": {
                "id": "msg_01",
                "model": "claude-sonnet-4-20250514",
                "content": [],
                "stop_reason": null,
                "usage": {"input_tokens": 10, "output_tokens": 0}
            }
        });
        let event: AnthropicStreamEvent = serde_json::from_value(json).unwrap();
        match event {
            AnthropicStreamEvent::MessageStart { message } => {
                assert_eq!(message.id, "msg_01");
            }
            other => panic!("expected MessageStart, got {:?}", other),
        }
    }

    #[test]
    fn stream_event_content_block_start_deserialization() {
        let json = json!({
            "type": "content_block_start",
            "index": 0,
            "content_block": {"type": "text", "text": ""}
        });
        let event: AnthropicStreamEvent = serde_json::from_value(json).unwrap();
        match event {
            AnthropicStreamEvent::ContentBlockStart {
                index,
                content_block,
            } => {
                assert_eq!(index, 0);
                assert!(matches!(content_block, AnthropicContentBlock::Text { .. }));
            }
            other => panic!("expected ContentBlockStart, got {:?}", other),
        }
    }

    #[test]
    fn stream_event_content_block_delta_text() {
        let json = json!({
            "type": "content_block_delta",
            "index": 0,
            "delta": {"type": "text_delta", "text": "Hello"}
        });
        let event: AnthropicStreamEvent = serde_json::from_value(json).unwrap();
        match event {
            AnthropicStreamEvent::ContentBlockDelta { index, delta } => {
                assert_eq!(index, 0);
                match delta {
                    AnthropicDelta::TextDelta { text } => assert_eq!(text, "Hello"),
                    other => panic!("expected TextDelta, got {:?}", other),
                }
            }
            other => panic!("expected ContentBlockDelta, got {:?}", other),
        }
    }

    #[test]
    fn stream_event_content_block_delta_input_json() {
        let json = json!({
            "type": "content_block_delta",
            "index": 1,
            "delta": {"type": "input_json_delta", "partial_json": "{\"q\":"}
        });
        let event: AnthropicStreamEvent = serde_json::from_value(json).unwrap();
        match event {
            AnthropicStreamEvent::ContentBlockDelta { delta, .. } => match delta {
                AnthropicDelta::InputJsonDelta { partial_json } => {
                    assert_eq!(partial_json, "{\"q\":");
                }
                other => panic!("expected InputJsonDelta, got {:?}", other),
            },
            other => panic!("expected ContentBlockDelta, got {:?}", other),
        }
    }

    #[test]
    fn stream_event_content_block_delta_thinking() {
        let json = json!({
            "type": "content_block_delta",
            "index": 0,
            "delta": {"type": "thinking_delta", "thinking": "Let me think..."}
        });
        let event: AnthropicStreamEvent = serde_json::from_value(json).unwrap();
        match event {
            AnthropicStreamEvent::ContentBlockDelta { delta, .. } => match delta {
                AnthropicDelta::ThinkingDelta { thinking } => {
                    assert_eq!(thinking, "Let me think...");
                }
                other => panic!("expected ThinkingDelta, got {:?}", other),
            },
            other => panic!("expected ContentBlockDelta, got {:?}", other),
        }
    }

    #[test]
    fn stream_event_message_delta_deserialization() {
        let json = json!({
            "type": "message_delta",
            "delta": {"stop_reason": "end_turn"},
            "usage": {"input_tokens": 0, "output_tokens": 20}
        });
        let event: AnthropicStreamEvent = serde_json::from_value(json).unwrap();
        match event {
            AnthropicStreamEvent::MessageDelta { delta, usage } => {
                assert_eq!(delta.stop_reason, Some("end_turn".into()));
                let u = usage.expect("should have usage");
                assert_eq!(u.output_tokens, 20);
            }
            other => panic!("expected MessageDelta, got {:?}", other),
        }
    }

    #[test]
    fn stream_event_message_stop_deserialization() {
        let json = json!({"type": "message_stop"});
        let event: AnthropicStreamEvent = serde_json::from_value(json).unwrap();
        assert!(matches!(event, AnthropicStreamEvent::MessageStop));
    }

    #[test]
    fn stream_event_ping_deserialization() {
        let json = json!({"type": "ping"});
        let event: AnthropicStreamEvent = serde_json::from_value(json).unwrap();
        assert!(matches!(event, AnthropicStreamEvent::Ping));
    }

    #[test]
    fn stream_event_error_deserialization() {
        let json = json!({
            "type": "error",
            "error": {
                "type": "overloaded_error",
                "message": "Overloaded"
            }
        });
        let event: AnthropicStreamEvent = serde_json::from_value(json).unwrap();
        match event {
            AnthropicStreamEvent::Error { error } => {
                assert_eq!(error.error_type, "overloaded_error");
                assert_eq!(error.message, "Overloaded");
            }
            other => panic!("expected Error, got {:?}", other),
        }
    }

    #[test]
    fn anthropic_request_serialization_skips_none_fields() {
        let req = convert_request(&simple_request());
        let json = serde_json::to_value(&req).unwrap();
        let obj = json.as_object().unwrap();

        assert!(obj.contains_key("model"));
        assert!(obj.contains_key("messages"));
        assert!(obj.contains_key("max_tokens"));
        assert!(!obj.contains_key("system"));
        assert!(!obj.contains_key("temperature"));
        assert!(!obj.contains_key("top_p"));
        assert!(!obj.contains_key("stop_sequences"));
        assert!(!obj.contains_key("tools"));
        assert!(!obj.contains_key("tool_choice"));
        assert!(!obj.contains_key("stream"));
        assert!(!obj.contains_key("thinking"));
    }

    // -- Cache control injection tests --

    #[test]
    fn cache_control_injected_on_system_prompt() {
        let req = simple_request().system_prompt("You are a helpful assistant.");
        let mut anthropic_req = convert_request(&req);
        inject_cache_control(&mut anthropic_req);

        let system = anthropic_req.system.expect("should have system blocks");
        assert_eq!(system.len(), 1);
        let cc = system[0]
            .cache_control
            .as_ref()
            .expect("system block should have cache_control");
        assert_eq!(cc.cache_type, "ephemeral");
    }

    #[test]
    fn cache_control_injected_on_last_user_message() {
        let req = Request::new(
            "claude-sonnet-4-20250514",
            vec![
                Message::user("First message"),
                Message {
                    role: Role::Assistant,
                    content: vec![ContentPart::text("Response")],
                    name: None,
                    tool_call_id: None,
                },
                Message::user("Second message"),
            ],
        );
        let mut anthropic_req = convert_request(&req);
        inject_cache_control(&mut anthropic_req);

        // The last user message is at index 2 (messages[2]).
        let last_user = &anthropic_req.messages[2];
        assert_eq!(last_user.role, "user");
        match &last_user.content[0] {
            AnthropicContentBlock::Text {
                cache_control,
                text,
            } => {
                assert_eq!(text, "Second message");
                let cc = cache_control
                    .as_ref()
                    .expect("last user text block should have cache_control");
                assert_eq!(cc.cache_type, "ephemeral");
            }
            other => panic!("expected Text, got {:?}", other),
        }

        // The first user message should NOT have cache_control.
        match &anthropic_req.messages[0].content[0] {
            AnthropicContentBlock::Text { cache_control, .. } => {
                assert!(
                    cache_control.is_none(),
                    "first user message should not have cache_control"
                );
            }
            other => panic!("expected Text, got {:?}", other),
        }
    }

    #[test]
    fn cache_control_not_injected_when_disabled() {
        let req = simple_request().system_prompt("System prompt");
        let anthropic_req = convert_request(&req);
        // Simulate disabled: simply do not call inject_cache_control.
        // The adapter checks enable_prompt_caching before calling.

        let system = anthropic_req.system.expect("should have system blocks");
        assert!(
            system[0].cache_control.is_none(),
            "cache_control should be None when injection is not called"
        );

        match &anthropic_req.messages[0].content[0] {
            AnthropicContentBlock::Text { cache_control, .. } => {
                assert!(
                    cache_control.is_none(),
                    "cache_control should be None when injection is not called"
                );
            }
            other => panic!("expected Text, got {:?}", other),
        }
    }

    #[test]
    fn cache_control_serialization() {
        let cc = CacheControl {
            cache_type: "ephemeral".into(),
        };
        let json = serde_json::to_value(&cc).unwrap();
        assert_eq!(json["type"], "ephemeral");

        // Verify it roundtrips.
        let back: CacheControl = serde_json::from_value(json).unwrap();
        assert_eq!(back.cache_type, "ephemeral");
    }

    #[test]
    fn cache_control_on_content_block_serialization() {
        let block = AnthropicContentBlock::Text {
            text: "hello".into(),
            cache_control: Some(CacheControl {
                cache_type: "ephemeral".into(),
            }),
        };
        let json = serde_json::to_value(&block).unwrap();
        assert_eq!(json["type"], "text");
        assert_eq!(json["text"], "hello");
        assert_eq!(json["cache_control"]["type"], "ephemeral");
    }

    #[test]
    fn cache_control_absent_when_none_on_content_block() {
        let block = AnthropicContentBlock::Text {
            text: "hello".into(),
            cache_control: None,
        };
        let json = serde_json::to_value(&block).unwrap();
        assert!(
            json.get("cache_control").is_none(),
            "cache_control should be absent in JSON when None"
        );
    }

    #[test]
    fn request_without_system_prompt_still_works() {
        let req = Request::new("claude-sonnet-4-20250514", vec![Message::user("Hello")]);
        let mut anthropic_req = convert_request(&req);
        inject_cache_control(&mut anthropic_req);

        // System should still be None.
        assert!(anthropic_req.system.is_none());

        // The user message should have cache_control on its last content block.
        match &anthropic_req.messages[0].content[0] {
            AnthropicContentBlock::Text {
                cache_control,
                text,
            } => {
                assert_eq!(text, "Hello");
                let cc = cache_control
                    .as_ref()
                    .expect("user text block should have cache_control");
                assert_eq!(cc.cache_type, "ephemeral");
            }
            other => panic!("expected Text, got {:?}", other),
        }
    }

    #[test]
    fn cache_control_injected_on_multiple_system_blocks_only_last() {
        let req = Request::new(
            "claude-sonnet-4-20250514",
            vec![
                Message::system("System instruction"),
                Message::user("Hello"),
            ],
        )
        .system_prompt("Main system prompt");
        let mut anthropic_req = convert_request(&req);
        inject_cache_control(&mut anthropic_req);

        let system = anthropic_req.system.expect("should have system blocks");
        assert_eq!(system.len(), 2);
        // Only the last system block should have cache_control.
        assert!(
            system[0].cache_control.is_none(),
            "first system block should not have cache_control"
        );
        let cc = system[1]
            .cache_control
            .as_ref()
            .expect("last system block should have cache_control");
        assert_eq!(cc.cache_type, "ephemeral");
    }

    #[test]
    fn cache_control_on_tool_result_content_block() {
        let req = Request::new(
            "claude-sonnet-4-20250514",
            vec![
                Message::user("Use the tool"),
                Message {
                    role: Role::Assistant,
                    content: vec![ContentPart::ToolCall(ToolCallData {
                        id: "toolu_1".into(),
                        name: "search".into(),
                        arguments: "{}".into(),
                        raw_arguments: None,
                    })],
                    name: None,
                    tool_call_id: None,
                },
                Message {
                    role: Role::User,
                    content: vec![ContentPart::ToolResult(ToolResultData {
                        tool_call_id: "toolu_1".into(),
                        content: "result data".into(),
                        is_error: false,
                    })],
                    name: None,
                    tool_call_id: None,
                },
            ],
        );
        let mut anthropic_req = convert_request(&req);
        inject_cache_control(&mut anthropic_req);

        // The last user message is the tool result at index 2.
        let last_user = &anthropic_req.messages[2];
        assert_eq!(last_user.role, "user");
        match &last_user.content[0] {
            AnthropicContentBlock::ToolResult { cache_control, .. } => {
                let cc = cache_control
                    .as_ref()
                    .expect("tool result block should have cache_control");
                assert_eq!(cc.cache_type, "ephemeral");
            }
            other => panic!("expected ToolResult, got {:?}", other),
        }
    }

    #[test]
    fn cache_control_with_empty_messages() {
        let req = Request::new("claude-sonnet-4-20250514", vec![]);
        let mut anthropic_req = convert_request(&req);
        // Should not panic on empty messages.
        inject_cache_control(&mut anthropic_req);
        assert!(anthropic_req.system.is_none());
        assert!(anthropic_req.messages.is_empty());
    }
}
