// ABOUTME: Native Gemini API serde types for request and response serialization.
// ABOUTME: Handles conversion between unified LLM types and Gemini's camelCase JSON format.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::types::{
    ContentPart, FinishReason, ImageSourceType, Request, Response, ResponseFormat, Role,
    ToolCallData, ToolChoice, Usage,
};

// ── Request Types ────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GeminiRequest {
    pub contents: Vec<GeminiContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_instruction: Option<GeminiContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub generation_config: Option<GeminiGenerationConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<GeminiToolDeclaration>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_config: Option<GeminiToolConfig>,
    /// Extra provider-specific fields from provider_options, serialized inline.
    #[serde(flatten, skip_serializing_if = "Option::is_none")]
    pub extra: Option<serde_json::Map<String, serde_json::Value>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GeminiContent {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    pub parts: Vec<GeminiPart>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum GeminiPart {
    Text {
        text: String,
    },
    InlineData {
        #[serde(rename = "inlineData")]
        inline_data: GeminiBlob,
    },
    FunctionCall {
        #[serde(rename = "functionCall")]
        function_call: GeminiFunctionCall,
    },
    FunctionResponse {
        #[serde(rename = "functionResponse")]
        function_response: GeminiFunctionResponse,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GeminiBlob {
    pub mime_type: String,
    pub data: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GeminiFunctionCall {
    pub name: String,
    pub args: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GeminiFunctionResponse {
    pub name: String,
    pub response: Value,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GeminiGenerationConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop_sequences: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_mime_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_schema: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking_config: Option<GeminiThinkingConfig>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GeminiThinkingConfig {
    pub thinking_budget: u32,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GeminiToolDeclaration {
    pub function_declarations: Vec<GeminiFunctionDeclaration>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GeminiFunctionDeclaration {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GeminiToolConfig {
    pub function_calling_config: GeminiFunctionCallingConfig,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GeminiFunctionCallingConfig {
    pub mode: String,
}

// ── Response Types ───────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GeminiResponse {
    pub candidates: Option<Vec<GeminiCandidate>>,
    pub usage_metadata: Option<GeminiUsageMetadata>,
    pub model_version: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GeminiCandidate {
    pub content: Option<GeminiContent>,
    pub finish_reason: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GeminiUsageMetadata {
    pub prompt_token_count: Option<u32>,
    pub candidates_token_count: Option<u32>,
    pub total_token_count: Option<u32>,
}

// ── Conversion Functions ─────────────────────────────────────────────────

/// Convert a unified Request into the native Gemini API request format.
///
/// Maps system messages to system_instruction, user messages to "user" role,
/// and assistant messages to "model" role. Tool results are mapped to
/// function responses and tool calls to function calls.
pub fn convert_request(request: &Request) -> GeminiRequest {
    let mut contents: Vec<GeminiContent> = Vec::new();
    let mut system_instruction: Option<GeminiContent> = None;

    // Build a lookup table mapping tool_call_id to tool name from previous
    // assistant messages, so we can populate function response names.
    let mut tool_call_id_to_name: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    for msg in &request.messages {
        if msg.role == Role::Assistant {
            for part in &msg.content {
                if let ContentPart::ToolCall(tc) = part {
                    tool_call_id_to_name.insert(tc.id.clone(), tc.name.clone());
                }
            }
        }
    }

    // If a system_prompt is set on the request, use that as system_instruction.
    if let Some(ref sys_prompt) = request.system_prompt {
        system_instruction = Some(GeminiContent {
            role: None,
            parts: vec![GeminiPart::Text {
                text: sys_prompt.clone(),
            }],
        });
    }

    for msg in &request.messages {
        match msg.role {
            Role::System => {
                // System messages become system_instruction; last one wins.
                let parts = convert_content_parts_to_gemini(&msg.content, &tool_call_id_to_name);
                if !parts.is_empty() {
                    system_instruction = Some(GeminiContent { role: None, parts });
                }
            }
            Role::User | Role::Developer => {
                let parts = convert_content_parts_to_gemini(&msg.content, &tool_call_id_to_name);
                if !parts.is_empty() {
                    contents.push(GeminiContent {
                        role: Some("user".to_string()),
                        parts,
                    });
                }
            }
            Role::Assistant => {
                let parts = convert_content_parts_to_gemini(&msg.content, &tool_call_id_to_name);
                if !parts.is_empty() {
                    contents.push(GeminiContent {
                        role: Some("model".to_string()),
                        parts,
                    });
                }
            }
            Role::Tool => {
                let parts = convert_content_parts_to_gemini(&msg.content, &tool_call_id_to_name);
                if !parts.is_empty() {
                    contents.push(GeminiContent {
                        role: Some("user".to_string()),
                        parts,
                    });
                }
            }
        }
    }

    // Build generation config.
    let generation_config = build_generation_config(request);

    // Build tools.
    let tools = request.tools.as_ref().map(|tool_defs| {
        vec![GeminiToolDeclaration {
            function_declarations: tool_defs
                .iter()
                .map(|td| GeminiFunctionDeclaration {
                    name: td.name.clone(),
                    description: td.description.clone(),
                    parameters: td.parameters.clone(),
                })
                .collect(),
        }]
    });

    // Build tool config from tool_choice.
    let tool_config = request.tool_choice.as_ref().map(|choice| {
        let mode = match choice {
            ToolChoice::Auto => "AUTO",
            ToolChoice::Required => "ANY",
            ToolChoice::None => "NONE",
            // Gemini doesn't have a direct "specific" mode; use ANY as closest match.
            ToolChoice::Specific { .. } => "ANY",
        };
        GeminiToolConfig {
            function_calling_config: GeminiFunctionCallingConfig {
                mode: mode.to_string(),
            },
        }
    });

    // Extract provider-specific options for Gemini.
    let extra = request
        .provider_options
        .as_ref()
        .and_then(|opts| opts.get("gemini"))
        .and_then(|v| v.as_object())
        .cloned()
        .filter(|m| !m.is_empty());

    GeminiRequest {
        contents,
        system_instruction,
        generation_config,
        tools,
        tool_config,
        extra,
    }
}

/// Convert unified content parts to Gemini's part format.
fn convert_content_parts_to_gemini(
    parts: &[ContentPart],
    tool_call_id_to_name: &std::collections::HashMap<String, String>,
) -> Vec<GeminiPart> {
    let mut gemini_parts = Vec::new();

    for part in parts {
        match part {
            ContentPart::Text { text } => {
                gemini_parts.push(GeminiPart::Text { text: text.clone() });
            }
            ContentPart::Image(img) => {
                if img.source_type == ImageSourceType::Base64 {
                    gemini_parts.push(GeminiPart::InlineData {
                        inline_data: GeminiBlob {
                            mime_type: img
                                .media_type
                                .clone()
                                .unwrap_or_else(|| "image/png".to_string()),
                            data: img.data.clone(),
                        },
                    });
                }
                // URL-referenced images are not directly supported via inlineData;
                // they would need to be fetched first. Skip for now.
            }
            ContentPart::ToolCall(tc) => {
                let args: Value = serde_json::from_str(&tc.arguments)
                    .unwrap_or(Value::Object(Default::default()));
                gemini_parts.push(GeminiPart::FunctionCall {
                    function_call: GeminiFunctionCall {
                        name: tc.name.clone(),
                        args,
                    },
                });
            }
            ContentPart::ToolResult(tr) => {
                let tool_name = tool_call_id_to_name
                    .get(&tr.tool_call_id)
                    .cloned()
                    .unwrap_or_else(|| "unknown".to_string());
                gemini_parts.push(GeminiPart::FunctionResponse {
                    function_response: GeminiFunctionResponse {
                        name: tool_name,
                        response: serde_json::json!({ "content": tr.content }),
                    },
                });
            }
            // Thinking, RedactedThinking, Audio, and Document parts have no direct Gemini equivalent; skip.
            ContentPart::Thinking(_)
            | ContentPart::RedactedThinking(_)
            | ContentPart::Audio(_)
            | ContentPart::Document(_) => {}
        }
    }

    gemini_parts
}

/// Build a GeminiGenerationConfig from the unified request parameters.
fn build_generation_config(request: &Request) -> Option<GeminiGenerationConfig> {
    let has_any = request.max_tokens.is_some()
        || request.temperature.is_some()
        || request.top_p.is_some()
        || request.stop_sequences.is_some()
        || request.response_format.is_some()
        || request.thinking.is_some();

    if !has_any {
        return None;
    }

    let (response_mime_type, response_schema) = match &request.response_format {
        Some(ResponseFormat::JsonObject) => (Some("application/json".to_string()), None),
        Some(ResponseFormat::JsonSchema { schema, .. }) => {
            (Some("application/json".to_string()), Some(schema.clone()))
        }
        _ => (None, None),
    };

    let thinking_config = request.thinking.as_ref().and_then(|tc| {
        if tc.enabled {
            tc.budget_tokens.map(|budget| GeminiThinkingConfig {
                thinking_budget: budget,
            })
        } else {
            None
        }
    });

    Some(GeminiGenerationConfig {
        max_output_tokens: request.max_tokens,
        temperature: request.temperature,
        top_p: request.top_p,
        stop_sequences: request.stop_sequences.clone(),
        response_mime_type,
        response_schema,
        thinking_config,
    })
}

/// Convert a native Gemini API response into the unified Response format.
///
/// Takes the first candidate, maps parts to ContentPart variants, translates
/// finish reasons, and extracts usage metadata.
pub fn convert_response(response: GeminiResponse, model: &str) -> Response {
    let mut content: Vec<ContentPart> = Vec::new();
    let mut finish_reason: Option<FinishReason> = None;

    if let Some(candidates) = &response.candidates
        && let Some(candidate) = candidates.first()
    {
        // Map parts to unified content parts.
        if let Some(ref gemini_content) = candidate.content {
            for part in &gemini_content.parts {
                match part {
                    GeminiPart::Text { text } => {
                        content.push(ContentPart::Text { text: text.clone() });
                    }
                    GeminiPart::FunctionCall { function_call } => {
                        let id = Uuid::new_v4().to_string();
                        let arguments = serde_json::to_string(&function_call.args)
                            .unwrap_or_else(|_| "{}".to_string());
                        content.push(ContentPart::ToolCall(ToolCallData {
                            id,
                            name: function_call.name.clone(),
                            arguments,
                            raw_arguments: None,
                        }));
                    }
                    // InlineData and FunctionResponse in a response are unusual;
                    // skip them for content mapping.
                    GeminiPart::InlineData { .. } | GeminiPart::FunctionResponse { .. } => {}
                }
            }
        }

        // Map finish reason.
        finish_reason = candidate.finish_reason.as_deref().map(map_finish_reason);
    }

    // Map usage.
    let usage = response
        .usage_metadata
        .as_ref()
        .map(|u| Usage {
            input_tokens: u.prompt_token_count.unwrap_or(0),
            output_tokens: u.candidates_token_count.unwrap_or(0),
            cache_read_tokens: None,
            cache_creation_tokens: None,
            reasoning_tokens: None,
            total_tokens: u.total_token_count,
            raw: None,
        })
        .unwrap_or_default();

    let id = response
        .model_version
        .clone()
        .unwrap_or_else(|| Uuid::new_v4().to_string());

    Response {
        id,
        model: model.to_string(),
        content,
        finish_reason,
        usage,
        warnings: vec![],
        rate_limit: None,
        provider: Some("gemini".to_string()),
        raw: None,
    }
}

/// Map a Gemini finish reason string to the unified FinishReason enum.
pub fn map_finish_reason(reason: &str) -> FinishReason {
    match reason {
        "STOP" => FinishReason::Stop,
        "MAX_TOKENS" => FinishReason::Length,
        "SAFETY" => FinishReason::ContentFilter,
        "RECITATION" => FinishReason::ContentFilter,
        "TOOL_USE" | "FUNCTION_CALL" => FinishReason::ToolUse,
        _ => FinishReason::Stop,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::tool::ToolDefinition;
    use crate::types::{
        ImageData, ImageSourceType, Message, ThinkingConfig, ThinkingData, ToolCallData,
    };
    use serde_json::json;

    // ── GeminiPart serde roundtrip tests ────────────────────────────────

    #[test]
    fn gemini_part_text_serializes_correctly() {
        let part = GeminiPart::Text {
            text: "hello".to_string(),
        };
        let json = serde_json::to_value(&part).unwrap();
        assert_eq!(json, json!({"text": "hello"}));
    }

    #[test]
    fn gemini_part_text_deserializes_correctly() {
        let json = json!({"text": "world"});
        let part: GeminiPart = serde_json::from_value(json).unwrap();
        match part {
            GeminiPart::Text { text } => assert_eq!(text, "world"),
            other => panic!("expected Text, got {other:?}"),
        }
    }

    #[test]
    fn gemini_part_inline_data_serializes_correctly() {
        let part = GeminiPart::InlineData {
            inline_data: GeminiBlob {
                mime_type: "image/png".to_string(),
                data: "aWNvbg==".to_string(),
            },
        };
        let json = serde_json::to_value(&part).unwrap();
        assert_eq!(json["inlineData"]["mimeType"], "image/png");
        assert_eq!(json["inlineData"]["data"], "aWNvbg==");
    }

    #[test]
    fn gemini_part_inline_data_deserializes_correctly() {
        let json = json!({"inlineData": {"mimeType": "image/jpeg", "data": "abc="}});
        let part: GeminiPart = serde_json::from_value(json).unwrap();
        match part {
            GeminiPart::InlineData { inline_data } => {
                assert_eq!(inline_data.mime_type, "image/jpeg");
                assert_eq!(inline_data.data, "abc=");
            }
            other => panic!("expected InlineData, got {other:?}"),
        }
    }

    #[test]
    fn gemini_part_function_call_serializes_correctly() {
        let part = GeminiPart::FunctionCall {
            function_call: GeminiFunctionCall {
                name: "get_weather".to_string(),
                args: json!({"location": "NYC"}),
            },
        };
        let json = serde_json::to_value(&part).unwrap();
        assert_eq!(json["functionCall"]["name"], "get_weather");
        assert_eq!(json["functionCall"]["args"]["location"], "NYC");
    }

    #[test]
    fn gemini_part_function_call_deserializes_correctly() {
        let json = json!({"functionCall": {"name": "search", "args": {"q": "rust"}}});
        let part: GeminiPart = serde_json::from_value(json).unwrap();
        match part {
            GeminiPart::FunctionCall { function_call } => {
                assert_eq!(function_call.name, "search");
                assert_eq!(function_call.args["q"], "rust");
            }
            other => panic!("expected FunctionCall, got {other:?}"),
        }
    }

    #[test]
    fn gemini_part_function_response_serializes_correctly() {
        let part = GeminiPart::FunctionResponse {
            function_response: GeminiFunctionResponse {
                name: "get_weather".to_string(),
                response: json!({"content": "72F and sunny"}),
            },
        };
        let json = serde_json::to_value(&part).unwrap();
        assert_eq!(json["functionResponse"]["name"], "get_weather");
        assert_eq!(
            json["functionResponse"]["response"]["content"],
            "72F and sunny"
        );
    }

    #[test]
    fn gemini_part_function_response_deserializes_correctly() {
        let json = json!({"functionResponse": {"name": "calc", "response": {"content": "42"}}});
        let part: GeminiPart = serde_json::from_value(json).unwrap();
        match part {
            GeminiPart::FunctionResponse { function_response } => {
                assert_eq!(function_response.name, "calc");
                assert_eq!(function_response.response["content"], "42");
            }
            other => panic!("expected FunctionResponse, got {other:?}"),
        }
    }

    // ── GeminiContent serde tests ───────────────────────────────────────

    #[test]
    fn gemini_content_serializes_with_camel_case() {
        let content = GeminiContent {
            role: Some("user".to_string()),
            parts: vec![GeminiPart::Text {
                text: "hello".to_string(),
            }],
        };
        let json = serde_json::to_value(&content).unwrap();
        assert_eq!(json["role"], "user");
        assert_eq!(json["parts"][0]["text"], "hello");
    }

    #[test]
    fn gemini_content_skips_none_role() {
        let content = GeminiContent {
            role: None,
            parts: vec![GeminiPart::Text {
                text: "system msg".to_string(),
            }],
        };
        let json = serde_json::to_value(&content).unwrap();
        assert!(json.get("role").is_none());
    }

    // ── GeminiGenerationConfig serde tests ──────────────────────────────

    #[test]
    fn generation_config_serializes_camel_case() {
        let config = GeminiGenerationConfig {
            max_output_tokens: Some(1024),
            temperature: Some(0.7),
            top_p: Some(0.9),
            stop_sequences: Some(vec!["STOP".to_string()]),
            response_mime_type: None,
            response_schema: None,
            thinking_config: None,
        };
        let json = serde_json::to_value(&config).unwrap();
        assert_eq!(json["maxOutputTokens"], 1024);
        let temp = json["temperature"].as_f64().unwrap();
        assert!((temp - 0.7).abs() < 0.001, "temperature was {temp}");
        let top_p = json["topP"].as_f64().unwrap();
        assert!((top_p - 0.9).abs() < 0.001, "topP was {top_p}");
        assert_eq!(json["stopSequences"][0], "STOP");
        assert!(json.get("responseMimeType").is_none());
    }

    #[test]
    fn generation_config_with_thinking_serializes() {
        let config = GeminiGenerationConfig {
            max_output_tokens: None,
            temperature: None,
            top_p: None,
            stop_sequences: None,
            response_mime_type: None,
            response_schema: None,
            thinking_config: Some(GeminiThinkingConfig {
                thinking_budget: 5000,
            }),
        };
        let json = serde_json::to_value(&config).unwrap();
        assert_eq!(json["thinkingConfig"]["thinkingBudget"], 5000);
    }

    // ── GeminiResponse deserialization tests ─────────────────────────────

    #[test]
    fn gemini_response_deserializes_text_response() {
        let json = json!({
            "candidates": [{
                "content": {
                    "role": "model",
                    "parts": [{"text": "Hello! How can I help?"}]
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
        let response: GeminiResponse = serde_json::from_value(json).unwrap();
        assert!(response.candidates.is_some());
        let candidates = response.candidates.unwrap();
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].finish_reason.as_deref(), Some("STOP"));

        let usage = response.usage_metadata.unwrap();
        assert_eq!(usage.prompt_token_count, Some(10));
        assert_eq!(usage.candidates_token_count, Some(20));
        assert_eq!(usage.total_token_count, Some(30));
        assert_eq!(response.model_version.as_deref(), Some("gemini-2.0-flash"));
    }

    #[test]
    fn gemini_response_deserializes_function_call() {
        let json = json!({
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
                "promptTokenCount": 15,
                "candidatesTokenCount": 10,
                "totalTokenCount": 25
            }
        });
        let response: GeminiResponse = serde_json::from_value(json).unwrap();
        let candidates = response.candidates.unwrap();
        let content = candidates[0].content.as_ref().unwrap();
        match &content.parts[0] {
            GeminiPart::FunctionCall { function_call } => {
                assert_eq!(function_call.name, "get_weather");
                assert_eq!(function_call.args["location"], "NYC");
            }
            other => panic!("expected FunctionCall, got {other:?}"),
        }
    }

    #[test]
    fn gemini_response_handles_missing_optional_fields() {
        let json = json!({
            "candidates": [{
                "content": {
                    "role": "model",
                    "parts": [{"text": "hi"}]
                }
            }]
        });
        let response: GeminiResponse = serde_json::from_value(json).unwrap();
        assert!(response.usage_metadata.is_none());
        assert!(response.model_version.is_none());
        let candidates = response.candidates.unwrap();
        assert!(candidates[0].finish_reason.is_none());
    }

    // ── convert_request tests ───────────────────────────────────────────

    #[test]
    fn convert_request_simple_text_message() {
        let request = Request::new("gemini-2.0-flash", vec![Message::user("Hello, Gemini!")]);

        let gemini_req = convert_request(&request);

        assert_eq!(gemini_req.contents.len(), 1);
        assert_eq!(gemini_req.contents[0].role.as_deref(), Some("user"));
        match &gemini_req.contents[0].parts[0] {
            GeminiPart::Text { text } => assert_eq!(text, "Hello, Gemini!"),
            other => panic!("expected Text, got {other:?}"),
        }
        assert!(gemini_req.system_instruction.is_none());
        assert!(gemini_req.generation_config.is_none());
    }

    #[test]
    fn convert_request_system_prompt_field() {
        let request = Request::new("gemini-2.0-flash", vec![Message::user("Hi")])
            .system_prompt("You are a helpful assistant.");

        let gemini_req = convert_request(&request);

        let sys = gemini_req.system_instruction.unwrap();
        assert!(sys.role.is_none());
        match &sys.parts[0] {
            GeminiPart::Text { text } => assert_eq!(text, "You are a helpful assistant."),
            other => panic!("expected Text, got {other:?}"),
        }
    }

    #[test]
    fn convert_request_system_message_overrides_system_prompt() {
        let request = Request::new(
            "gemini-2.0-flash",
            vec![
                Message::system("System message in messages array"),
                Message::user("Hi"),
            ],
        )
        .system_prompt("System prompt field");

        let gemini_req = convert_request(&request);

        // The system message in the messages array should override the system_prompt field.
        let sys = gemini_req.system_instruction.unwrap();
        match &sys.parts[0] {
            GeminiPart::Text { text } => {
                assert_eq!(text, "System message in messages array");
            }
            other => panic!("expected Text, got {other:?}"),
        }
    }

    #[test]
    fn convert_request_assistant_maps_to_model_role() {
        let request = Request::new(
            "gemini-2.0-flash",
            vec![
                Message::user("What is 2+2?"),
                Message::assistant("4"),
                Message::user("Thanks!"),
            ],
        );

        let gemini_req = convert_request(&request);

        assert_eq!(gemini_req.contents.len(), 3);
        assert_eq!(gemini_req.contents[0].role.as_deref(), Some("user"));
        assert_eq!(gemini_req.contents[1].role.as_deref(), Some("model"));
        assert_eq!(gemini_req.contents[2].role.as_deref(), Some("user"));
    }

    #[test]
    fn convert_request_with_generation_config() {
        let request = Request::new("gemini-2.0-flash", vec![Message::user("Hi")])
            .max_tokens(512)
            .temperature(0.8)
            .top_p(0.95)
            .stop_sequences(vec!["END".to_string()]);

        let gemini_req = convert_request(&request);

        let config = gemini_req.generation_config.unwrap();
        assert_eq!(config.max_output_tokens, Some(512));
        assert_eq!(config.temperature, Some(0.8));
        assert_eq!(config.top_p, Some(0.95));
        assert_eq!(config.stop_sequences, Some(vec!["END".to_string()]));
    }

    #[test]
    fn convert_request_with_json_response_format() {
        let request = Request::new("gemini-2.0-flash", vec![Message::user("Hi")])
            .response_format(ResponseFormat::JsonObject);

        let gemini_req = convert_request(&request);

        let config = gemini_req.generation_config.unwrap();
        assert_eq!(
            config.response_mime_type.as_deref(),
            Some("application/json")
        );
        assert!(config.response_schema.is_none());
    }

    #[test]
    fn convert_request_with_json_schema_response_format() {
        let schema = json!({"type": "object", "properties": {"name": {"type": "string"}}});
        let request = Request::new("gemini-2.0-flash", vec![Message::user("Hi")]).response_format(
            ResponseFormat::JsonSchema {
                name: "person".to_string(),
                schema: schema.clone(),
                strict: true,
            },
        );

        let gemini_req = convert_request(&request);

        let config = gemini_req.generation_config.unwrap();
        assert_eq!(
            config.response_mime_type.as_deref(),
            Some("application/json")
        );
        assert_eq!(config.response_schema, Some(schema));
    }

    #[test]
    fn convert_request_with_thinking_config() {
        let request = Request::new(
            "gemini-2.0-flash-thinking",
            vec![Message::user("Think about this")],
        )
        .thinking(ThinkingConfig {
            enabled: true,
            budget_tokens: Some(8000),
        });

        let gemini_req = convert_request(&request);

        let config = gemini_req.generation_config.unwrap();
        let thinking = config.thinking_config.unwrap();
        assert_eq!(thinking.thinking_budget, 8000);
    }

    #[test]
    fn convert_request_disabled_thinking_has_no_thinking_config() {
        let request =
            Request::new("gemini-2.0-flash", vec![Message::user("Hi")]).thinking(ThinkingConfig {
                enabled: false,
                budget_tokens: Some(8000),
            });

        let gemini_req = convert_request(&request);

        let config = gemini_req.generation_config.unwrap();
        assert!(config.thinking_config.is_none());
    }

    #[test]
    fn convert_request_with_tools() {
        let tool = ToolDefinition::new(
            "get_weather",
            "Get the weather",
            json!({"type": "object", "properties": {"location": {"type": "string"}}}),
        );

        let request = Request::new(
            "gemini-2.0-flash",
            vec![Message::user("What's the weather?")],
        )
        .tools(vec![tool]);

        let gemini_req = convert_request(&request);

        let tools = gemini_req.tools.unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].function_declarations.len(), 1);
        assert_eq!(tools[0].function_declarations[0].name, "get_weather");
    }

    #[test]
    fn convert_request_tool_choice_auto() {
        let request = Request::new("gemini-2.0-flash", vec![Message::user("Hi")])
            .tool_choice(ToolChoice::Auto);

        let gemini_req = convert_request(&request);

        let tc = gemini_req.tool_config.unwrap();
        assert_eq!(tc.function_calling_config.mode, "AUTO");
    }

    #[test]
    fn convert_request_tool_choice_required() {
        let request = Request::new("gemini-2.0-flash", vec![Message::user("Hi")])
            .tool_choice(ToolChoice::Required);

        let gemini_req = convert_request(&request);

        let tc = gemini_req.tool_config.unwrap();
        assert_eq!(tc.function_calling_config.mode, "ANY");
    }

    #[test]
    fn convert_request_tool_choice_none() {
        let request = Request::new("gemini-2.0-flash", vec![Message::user("Hi")])
            .tool_choice(ToolChoice::None);

        let gemini_req = convert_request(&request);

        let tc = gemini_req.tool_config.unwrap();
        assert_eq!(tc.function_calling_config.mode, "NONE");
    }

    #[test]
    fn convert_request_with_tool_call_and_result() {
        let request = Request::new(
            "gemini-2.0-flash",
            vec![
                Message::user("What's the weather?"),
                Message::assistant_with_tool_calls(vec![ToolCallData {
                    id: "call_123".to_string(),
                    name: "get_weather".to_string(),
                    arguments: r#"{"location":"NYC"}"#.to_string(),
                    raw_arguments: None,
                }]),
                Message::tool_result("call_123", "72F and sunny", false),
            ],
        );

        let gemini_req = convert_request(&request);

        // Should have 3 content entries: user, model (tool call), user (tool result).
        assert_eq!(gemini_req.contents.len(), 3);

        // Check the model's function call.
        let model_parts = &gemini_req.contents[1];
        assert_eq!(model_parts.role.as_deref(), Some("model"));
        match &model_parts.parts[0] {
            GeminiPart::FunctionCall { function_call } => {
                assert_eq!(function_call.name, "get_weather");
                assert_eq!(function_call.args["location"], "NYC");
            }
            other => panic!("expected FunctionCall, got {other:?}"),
        }

        // Check the tool result is a function response.
        let tool_parts = &gemini_req.contents[2];
        assert_eq!(tool_parts.role.as_deref(), Some("user"));
        match &tool_parts.parts[0] {
            GeminiPart::FunctionResponse { function_response } => {
                assert_eq!(function_response.name, "get_weather");
                assert_eq!(function_response.response["content"], "72F and sunny");
            }
            other => panic!("expected FunctionResponse, got {other:?}"),
        }
    }

    #[test]
    fn convert_request_tool_result_without_matching_call_uses_unknown() {
        let request = Request::new(
            "gemini-2.0-flash",
            vec![Message::tool_result("orphan_id", "some result", false)],
        );

        let gemini_req = convert_request(&request);

        match &gemini_req.contents[0].parts[0] {
            GeminiPart::FunctionResponse { function_response } => {
                assert_eq!(function_response.name, "unknown");
            }
            other => panic!("expected FunctionResponse, got {other:?}"),
        }
    }

    #[test]
    fn convert_request_with_image_base64() {
        let request = Request::new(
            "gemini-2.0-flash",
            vec![Message {
                role: Role::User,
                content: vec![
                    ContentPart::Text {
                        text: "What is this?".to_string(),
                    },
                    ContentPart::Image(ImageData {
                        source_type: ImageSourceType::Base64,
                        media_type: Some("image/png".to_string()),
                        data: "iVBOR...".to_string(),
                    }),
                ],
                name: None,
                tool_call_id: None,
            }],
        );

        let gemini_req = convert_request(&request);

        assert_eq!(gemini_req.contents[0].parts.len(), 2);
        match &gemini_req.contents[0].parts[1] {
            GeminiPart::InlineData { inline_data } => {
                assert_eq!(inline_data.mime_type, "image/png");
                assert_eq!(inline_data.data, "iVBOR...");
            }
            other => panic!("expected InlineData, got {other:?}"),
        }
    }

    #[test]
    fn convert_request_skips_thinking_parts() {
        let request = Request::new(
            "gemini-2.0-flash",
            vec![Message {
                role: Role::Assistant,
                content: vec![
                    ContentPart::Thinking(ThinkingData {
                        thinking: "Let me think...".to_string(),
                        signature: None,
                        redacted: false,
                    }),
                    ContentPart::Text {
                        text: "Here's the answer".to_string(),
                    },
                ],
                name: None,
                tool_call_id: None,
            }],
        );

        let gemini_req = convert_request(&request);

        // Thinking part should be skipped, only text remains.
        assert_eq!(gemini_req.contents[0].parts.len(), 1);
        match &gemini_req.contents[0].parts[0] {
            GeminiPart::Text { text } => assert_eq!(text, "Here's the answer"),
            other => panic!("expected Text, got {other:?}"),
        }
    }

    #[test]
    fn convert_request_developer_message_maps_to_user() {
        let request = Request::new(
            "gemini-2.0-flash",
            vec![Message::developer("Internal instructions")],
        );

        let gemini_req = convert_request(&request);

        assert_eq!(gemini_req.contents[0].role.as_deref(), Some("user"));
    }

    // ── convert_response tests ──────────────────────────────────────────

    #[test]
    fn convert_response_simple_text() {
        let gemini_resp = GeminiResponse {
            candidates: Some(vec![GeminiCandidate {
                content: Some(GeminiContent {
                    role: Some("model".to_string()),
                    parts: vec![GeminiPart::Text {
                        text: "Hello!".to_string(),
                    }],
                }),
                finish_reason: Some("STOP".to_string()),
            }]),
            usage_metadata: Some(GeminiUsageMetadata {
                prompt_token_count: Some(10),
                candidates_token_count: Some(5),
                total_token_count: Some(15),
            }),
            model_version: Some("gemini-2.0-flash".to_string()),
        };

        let response = convert_response(gemini_resp, "gemini-2.0-flash");

        assert_eq!(response.text().as_deref(), Some("Hello!"));
        assert_eq!(response.finish_reason, Some(FinishReason::Stop));
        assert_eq!(response.usage.input_tokens, 10);
        assert_eq!(response.usage.output_tokens, 5);
        assert_eq!(response.usage.total_tokens, Some(15));
        assert_eq!(response.model, "gemini-2.0-flash");
        assert_eq!(response.id, "gemini-2.0-flash");
    }

    #[test]
    fn convert_response_function_call() {
        let gemini_resp = GeminiResponse {
            candidates: Some(vec![GeminiCandidate {
                content: Some(GeminiContent {
                    role: Some("model".to_string()),
                    parts: vec![GeminiPart::FunctionCall {
                        function_call: GeminiFunctionCall {
                            name: "get_weather".to_string(),
                            args: json!({"location": "NYC"}),
                        },
                    }],
                }),
                finish_reason: Some("STOP".to_string()),
            }]),
            usage_metadata: Some(GeminiUsageMetadata {
                prompt_token_count: Some(10),
                candidates_token_count: Some(15),
                total_token_count: Some(25),
            }),
            model_version: None,
        };

        let response = convert_response(gemini_resp, "gemini-2.0-flash");

        assert!(response.has_tool_calls());
        let calls = response.tool_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "get_weather");
        // The ID should be a valid UUID.
        assert!(!calls[0].id.is_empty());
        let parsed_args: serde_json::Value = serde_json::from_str(&calls[0].arguments).unwrap();
        assert_eq!(parsed_args["location"], "NYC");
    }

    #[test]
    fn convert_response_max_tokens_finish_reason() {
        let gemini_resp = GeminiResponse {
            candidates: Some(vec![GeminiCandidate {
                content: Some(GeminiContent {
                    role: Some("model".to_string()),
                    parts: vec![GeminiPart::Text {
                        text: "truncated...".to_string(),
                    }],
                }),
                finish_reason: Some("MAX_TOKENS".to_string()),
            }]),
            usage_metadata: None,
            model_version: None,
        };

        let response = convert_response(gemini_resp, "gemini-2.0-flash");
        assert_eq!(response.finish_reason, Some(FinishReason::Length));
    }

    #[test]
    fn convert_response_safety_finish_reason() {
        let gemini_resp = GeminiResponse {
            candidates: Some(vec![GeminiCandidate {
                content: Some(GeminiContent {
                    role: Some("model".to_string()),
                    parts: vec![GeminiPart::Text {
                        text: "".to_string(),
                    }],
                }),
                finish_reason: Some("SAFETY".to_string()),
            }]),
            usage_metadata: None,
            model_version: None,
        };

        let response = convert_response(gemini_resp, "gemini-2.0-flash");
        assert_eq!(response.finish_reason, Some(FinishReason::ContentFilter));
    }

    #[test]
    fn convert_response_no_candidates() {
        let gemini_resp = GeminiResponse {
            candidates: None,
            usage_metadata: Some(GeminiUsageMetadata {
                prompt_token_count: Some(10),
                candidates_token_count: Some(0),
                total_token_count: Some(10),
            }),
            model_version: None,
        };

        let response = convert_response(gemini_resp, "gemini-2.0-flash");
        assert!(response.content.is_empty());
        assert!(response.finish_reason.is_none());
    }

    #[test]
    fn convert_response_empty_candidates_list() {
        let gemini_resp = GeminiResponse {
            candidates: Some(vec![]),
            usage_metadata: None,
            model_version: None,
        };

        let response = convert_response(gemini_resp, "gemini-2.0-flash");
        assert!(response.content.is_empty());
        assert!(response.finish_reason.is_none());
    }

    #[test]
    fn convert_response_missing_usage() {
        let gemini_resp = GeminiResponse {
            candidates: Some(vec![GeminiCandidate {
                content: Some(GeminiContent {
                    role: Some("model".to_string()),
                    parts: vec![GeminiPart::Text {
                        text: "hi".to_string(),
                    }],
                }),
                finish_reason: Some("STOP".to_string()),
            }]),
            usage_metadata: None,
            model_version: None,
        };

        let response = convert_response(gemini_resp, "gemini-2.0-flash");
        assert_eq!(response.usage.input_tokens, 0);
        assert_eq!(response.usage.output_tokens, 0);
    }

    #[test]
    fn convert_response_multiple_parts() {
        let gemini_resp = GeminiResponse {
            candidates: Some(vec![GeminiCandidate {
                content: Some(GeminiContent {
                    role: Some("model".to_string()),
                    parts: vec![
                        GeminiPart::Text {
                            text: "Let me check the weather.".to_string(),
                        },
                        GeminiPart::FunctionCall {
                            function_call: GeminiFunctionCall {
                                name: "get_weather".to_string(),
                                args: json!({"city": "SF"}),
                            },
                        },
                    ],
                }),
                finish_reason: Some("STOP".to_string()),
            }]),
            usage_metadata: None,
            model_version: None,
        };

        let response = convert_response(gemini_resp, "gemini-2.0-flash");
        assert_eq!(response.content.len(), 2);
        assert_eq!(
            response.text().as_deref(),
            Some("Let me check the weather.")
        );
        assert!(response.has_tool_calls());
    }

    // ── map_finish_reason tests ─────────────────────────────────────────

    #[test]
    fn map_finish_reason_all_variants() {
        assert_eq!(map_finish_reason("STOP"), FinishReason::Stop);
        assert_eq!(map_finish_reason("MAX_TOKENS"), FinishReason::Length);
        assert_eq!(map_finish_reason("SAFETY"), FinishReason::ContentFilter);
        assert_eq!(map_finish_reason("RECITATION"), FinishReason::ContentFilter);
        assert_eq!(map_finish_reason("TOOL_USE"), FinishReason::ToolUse);
        assert_eq!(map_finish_reason("FUNCTION_CALL"), FinishReason::ToolUse);
        assert_eq!(map_finish_reason("UNKNOWN_REASON"), FinishReason::Stop);
    }

    // ── Full request serialization test ─────────────────────────────────

    #[test]
    fn convert_request_serializes_to_valid_json() {
        let tool = ToolDefinition::new(
            "search",
            "Search the web",
            json!({"type": "object", "properties": {"q": {"type": "string"}}}),
        );

        let request = Request::new("gemini-2.0-flash", vec![Message::user("Search for Rust")])
            .system_prompt("You are a search assistant.")
            .max_tokens(1024)
            .temperature(0.5)
            .tools(vec![tool])
            .tool_choice(ToolChoice::Auto);

        let gemini_req = convert_request(&request);
        let json = serde_json::to_value(&gemini_req).unwrap();

        // Verify structure.
        assert!(json["contents"].is_array());
        assert!(json["systemInstruction"].is_object());
        assert!(json["generationConfig"].is_object());
        assert!(json["tools"].is_array());
        assert!(json["toolConfig"].is_object());
        assert_eq!(json["toolConfig"]["functionCallingConfig"]["mode"], "AUTO");
        assert_eq!(json["generationConfig"]["maxOutputTokens"], 1024);
    }

    #[test]
    fn convert_request_no_generation_config_when_nothing_set() {
        let request = Request::new("gemini-2.0-flash", vec![Message::user("Hi")]);

        let gemini_req = convert_request(&request);

        assert!(gemini_req.generation_config.is_none());
        let json = serde_json::to_value(&gemini_req).unwrap();
        assert!(json.get("generationConfig").is_none());
    }

    #[test]
    fn convert_request_tool_result_is_error() {
        let request = Request::new(
            "gemini-2.0-flash",
            vec![
                Message::user("Do something"),
                Message::assistant_with_tool_calls(vec![ToolCallData {
                    id: "call_err".to_string(),
                    name: "risky_fn".to_string(),
                    arguments: "{}".to_string(),
                    raw_arguments: None,
                }]),
                Message::tool_result("call_err", "API key expired", true),
            ],
        );

        let gemini_req = convert_request(&request);

        // The tool result should still be converted as a function response.
        match &gemini_req.contents[2].parts[0] {
            GeminiPart::FunctionResponse { function_response } => {
                assert_eq!(function_response.name, "risky_fn");
                assert_eq!(function_response.response["content"], "API key expired");
            }
            other => panic!("expected FunctionResponse, got {other:?}"),
        }
    }

    #[test]
    fn convert_request_invalid_tool_arguments_defaults_to_empty_object() {
        let request = Request::new(
            "gemini-2.0-flash",
            vec![Message::assistant_with_tool_calls(vec![ToolCallData {
                id: "call_bad".to_string(),
                name: "broken_fn".to_string(),
                arguments: "not valid json".to_string(),
                raw_arguments: None,
            }])],
        );

        let gemini_req = convert_request(&request);

        match &gemini_req.contents[0].parts[0] {
            GeminiPart::FunctionCall { function_call } => {
                assert_eq!(function_call.name, "broken_fn");
                assert_eq!(function_call.args, json!({}));
            }
            other => panic!("expected FunctionCall, got {other:?}"),
        }
    }

    #[test]
    fn convert_response_candidate_without_content() {
        let gemini_resp = GeminiResponse {
            candidates: Some(vec![GeminiCandidate {
                content: None,
                finish_reason: Some("SAFETY".to_string()),
            }]),
            usage_metadata: None,
            model_version: None,
        };

        let response = convert_response(gemini_resp, "gemini-2.0-flash");
        assert!(response.content.is_empty());
        assert_eq!(response.finish_reason, Some(FinishReason::ContentFilter));
    }

    #[test]
    fn convert_response_usage_with_missing_counts() {
        let gemini_resp = GeminiResponse {
            candidates: Some(vec![GeminiCandidate {
                content: Some(GeminiContent {
                    role: Some("model".to_string()),
                    parts: vec![GeminiPart::Text {
                        text: "ok".to_string(),
                    }],
                }),
                finish_reason: Some("STOP".to_string()),
            }]),
            usage_metadata: Some(GeminiUsageMetadata {
                prompt_token_count: None,
                candidates_token_count: None,
                total_token_count: Some(100),
            }),
            model_version: None,
        };

        let response = convert_response(gemini_resp, "gemini-2.0-flash");
        assert_eq!(response.usage.input_tokens, 0);
        assert_eq!(response.usage.output_tokens, 0);
        assert_eq!(response.usage.total_tokens, Some(100));
    }

    // -- provider_options tests --

    #[test]
    fn convert_request_provider_options_merged_into_extra() {
        use std::collections::HashMap;
        let mut opts = HashMap::new();
        opts.insert(
            "gemini".into(),
            json!({"cachedContent": "projects/123/cachedContents/abc"}),
        );
        let req =
            Request::new("gemini-2.0-flash", vec![Message::user("Hi")]).provider_options(opts);
        let gemini = convert_request(&req);

        let serialized = serde_json::to_value(&gemini).unwrap();
        assert_eq!(
            serialized["cachedContent"],
            "projects/123/cachedContents/abc"
        );
    }

    #[test]
    fn convert_request_provider_options_ignores_other_providers() {
        use std::collections::HashMap;
        let mut opts = HashMap::new();
        opts.insert("openai".into(), json!({"store": true}));
        let req =
            Request::new("gemini-2.0-flash", vec![Message::user("Hi")]).provider_options(opts);
        let gemini = convert_request(&req);

        let serialized = serde_json::to_value(&gemini).unwrap();
        assert!(serialized.get("store").is_none());
    }

    #[test]
    fn convert_request_provider_options_none_leaves_no_extra() {
        let req = Request::new("gemini-2.0-flash", vec![Message::user("Hi")]);
        let gemini = convert_request(&req);

        let serialized = serde_json::to_value(&gemini).unwrap();
        assert!(serialized.get("cachedContent").is_none());
    }
}
