// ABOUTME: Native serde types for the OpenAI Responses API (not the older Chat Completions API).
// ABOUTME: Provides request/response structs and conversion functions to/from unified types.

use serde::{Deserialize, Serialize};

use crate::types::{
    ContentPart, Error, FinishReason, ImageData, ImageSourceType, Request, Response,
    ResponseFormat, Role, ToolCallData, ToolChoice, Usage,
};

// ── Request Types ───────────────────────────────────────────────────────

/// Top-level request body for the OpenAI Responses API.
#[derive(Debug, Serialize)]
pub struct OpenAiRequest {
    pub model: String,
    pub input: Vec<OpenAiInputItem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<OpenAiTool>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<OpenAiReasoning>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<OpenAiTextConfig>,
}

/// An input item in the Responses API request. Uses internally-tagged JSON.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum OpenAiInputItem {
    /// A message input item (user, assistant, or developer).
    #[serde(rename = "message")]
    Message {
        role: String,
        content: OpenAiContent,
    },
    /// A function call output fed back to the model.
    #[serde(rename = "function_call_output")]
    FunctionCallOutput { call_id: String, output: String },
}

/// Content in a message can be either a plain string or an array of content parts.
#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum OpenAiContent {
    Text(String),
    Parts(Vec<OpenAiContentPart>),
}

/// A single content part within a message's content array.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum OpenAiContentPart {
    /// Text input content.
    #[serde(rename = "input_text")]
    InputText { text: String },
    /// Image input content via URL.
    #[serde(rename = "input_image")]
    InputImage {
        #[serde(skip_serializing_if = "Option::is_none")]
        image_url: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
    },
}

/// A tool definition for the Responses API.
#[derive(Debug, Serialize, Deserialize)]
pub struct OpenAiTool {
    #[serde(rename = "type")]
    pub tool_type: String,
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub strict: Option<bool>,
}

/// Reasoning / thinking configuration.
#[derive(Debug, Serialize, Deserialize)]
pub struct OpenAiReasoning {
    pub effort: String,
}

/// Text output configuration, used for structured output (JSON schema).
#[derive(Debug, Serialize, Deserialize)]
pub struct OpenAiTextConfig {
    pub format: OpenAiTextFormat,
}

/// Format specification inside OpenAiTextConfig.
#[derive(Debug, Serialize, Deserialize)]
pub struct OpenAiTextFormat {
    #[serde(rename = "type")]
    pub format_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schema: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub strict: Option<bool>,
}

// ── Response Types ──────────────────────────────────────────────────────

/// Top-level response from the OpenAI Responses API.
#[derive(Debug, Deserialize)]
pub struct OpenAiResponse {
    pub id: String,
    pub model: String,
    pub output: Vec<OpenAiOutputItem>,
    pub usage: Option<OpenAiUsage>,
    pub status: Option<String>,
    #[serde(default)]
    pub incomplete_details: Option<OpenAiIncompleteDetails>,
}

/// An output item in the Responses API response. Uses internally-tagged JSON.
#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
pub enum OpenAiOutputItem {
    /// An assistant message output.
    #[serde(rename = "message")]
    Message {
        #[allow(dead_code)]
        role: String,
        content: Vec<OpenAiOutputContent>,
    },
    /// A function call issued by the model.
    #[serde(rename = "function_call")]
    FunctionCall {
        #[allow(dead_code)]
        id: String,
        call_id: String,
        name: String,
        arguments: String,
    },
    /// A reasoning/thinking trace from the model.
    #[serde(rename = "reasoning")]
    Reasoning {
        #[serde(default)]
        summary: Vec<OpenAiReasoningSummary>,
    },
}

/// Content within a message output item.
#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
pub enum OpenAiOutputContent {
    /// Text output content.
    #[serde(rename = "output_text")]
    OutputText { text: String },
}

/// A reasoning summary entry returned by the model.
#[derive(Debug, Deserialize)]
pub struct OpenAiReasoningSummary {
    #[serde(rename = "type")]
    #[allow(dead_code)]
    pub summary_type: String,
    pub text: String,
}

/// Token usage statistics from the Responses API.
#[derive(Debug, Deserialize)]
pub struct OpenAiUsage {
    pub input_tokens: u32,
    pub output_tokens: u32,
    #[serde(default)]
    pub input_tokens_details: Option<OpenAiInputTokensDetails>,
    #[serde(default)]
    pub output_tokens_details: Option<OpenAiOutputTokensDetails>,
}

/// Detailed breakdown of input token usage.
#[derive(Debug, Deserialize)]
pub struct OpenAiInputTokensDetails {
    #[serde(default)]
    pub cached_tokens: u32,
}

/// Detailed breakdown of output token usage.
#[derive(Debug, Deserialize)]
pub struct OpenAiOutputTokensDetails {
    #[serde(default)]
    pub reasoning_tokens: u32,
}

/// Details about why a response was incomplete.
#[derive(Debug, Deserialize)]
pub struct OpenAiIncompleteDetails {
    pub reason: String,
}

// ── Conversion Functions ────────────────────────────────────────────────

/// Convert a unified Request into an OpenAI Responses API request body.
pub fn convert_request(request: &Request) -> OpenAiRequest {
    let mut instructions = request.system_prompt.clone();
    let mut input_items: Vec<OpenAiInputItem> = Vec::new();

    for message in &request.messages {
        match message.role {
            Role::System => {
                // System messages become the instructions field; last one wins.
                if let Some(text) = message.text() {
                    instructions = Some(text.to_string());
                }
            }
            Role::Developer => {
                if let Some(text) = message.text() {
                    input_items.push(OpenAiInputItem::Message {
                        role: "developer".to_string(),
                        content: OpenAiContent::Text(text.to_string()),
                    });
                }
            }
            Role::User => {
                let content = convert_content_parts(&message.content);
                input_items.push(OpenAiInputItem::Message {
                    role: "user".to_string(),
                    content,
                });
            }
            Role::Assistant => {
                // Assistant messages may contain text and/or tool calls.
                // Text parts become a message; tool calls are tracked but
                // the Responses API handles them as part of the conversation context.
                let has_tool_calls = message.has_tool_calls();
                if has_tool_calls {
                    // For assistant messages with tool calls, include text if present,
                    // then the tool calls become part of the conversation history.
                    let text_parts: Vec<&str> =
                        message.content.iter().filter_map(|p| p.as_text()).collect();
                    if !text_parts.is_empty() {
                        input_items.push(OpenAiInputItem::Message {
                            role: "assistant".to_string(),
                            content: OpenAiContent::Text(text_parts.join("")),
                        });
                    }
                } else {
                    let content = convert_content_parts(&message.content);
                    input_items.push(OpenAiInputItem::Message {
                        role: "assistant".to_string(),
                        content,
                    });
                }
            }
            Role::Tool => {
                // Tool result messages become function_call_output items.
                for part in &message.content {
                    if let ContentPart::ToolResult(data) = part {
                        input_items.push(OpenAiInputItem::FunctionCallOutput {
                            call_id: data.tool_call_id.clone(),
                            output: data.content.clone(),
                        });
                    }
                }
            }
        }
    }

    let tools = request.tools.as_ref().map(|tools| {
        tools
            .iter()
            .map(|t| OpenAiTool {
                tool_type: "function".to_string(),
                name: t.name.clone(),
                description: t.description.clone(),
                parameters: t.parameters.clone(),
                strict: Some(true),
            })
            .collect()
    });

    let tool_choice = request.tool_choice.as_ref().map(|tc| match tc {
        ToolChoice::Auto => serde_json::Value::String("auto".to_string()),
        ToolChoice::None => serde_json::Value::String("none".to_string()),
        ToolChoice::Required => serde_json::Value::String("required".to_string()),
        ToolChoice::Specific { name } => serde_json::json!({
            "type": "function",
            "name": name,
        }),
    });

    let reasoning = request.thinking.as_ref().and_then(|tc| {
        if !tc.enabled {
            return None;
        }
        let effort = match tc.budget_tokens {
            Some(budget) if budget < 5000 => "low",
            Some(budget) if budget < 20000 => "medium",
            _ => "high",
        };
        Some(OpenAiReasoning {
            effort: effort.to_string(),
        })
    });

    let text = request.response_format.as_ref().and_then(|rf| match rf {
        ResponseFormat::JsonSchema {
            name,
            schema,
            strict,
        } => Some(OpenAiTextConfig {
            format: OpenAiTextFormat {
                format_type: "json_schema".to_string(),
                name: Some(name.clone()),
                schema: Some(schema.clone()),
                strict: Some(*strict),
            },
        }),
        ResponseFormat::JsonObject => Some(OpenAiTextConfig {
            format: OpenAiTextFormat {
                format_type: "json_object".to_string(),
                name: None,
                schema: None,
                strict: None,
            },
        }),
        ResponseFormat::Text => None,
    });

    OpenAiRequest {
        model: request.model.clone(),
        input: input_items,
        instructions,
        max_output_tokens: request.max_tokens,
        temperature: request.temperature,
        top_p: request.top_p,
        tools,
        tool_choice,
        stream: request.stream,
        reasoning,
        text,
    }
}

/// Convert content parts from the unified format to OpenAI content.
fn convert_content_parts(parts: &[ContentPart]) -> OpenAiContent {
    // If there is a single text part, use the simple string form.
    if parts.len() == 1
        && let Some(text) = parts[0].as_text()
    {
        return OpenAiContent::Text(text.to_string());
    }

    let openai_parts: Vec<OpenAiContentPart> = parts
        .iter()
        .filter_map(|part| match part {
            ContentPart::Text { text } => Some(OpenAiContentPart::InputText { text: text.clone() }),
            ContentPart::Image(ImageData {
                source_type: ImageSourceType::Url,
                data,
                ..
            }) => Some(OpenAiContentPart::InputImage {
                image_url: Some(data.clone()),
                detail: None,
            }),
            ContentPart::Image(ImageData {
                source_type: ImageSourceType::Base64,
                media_type,
                data,
            }) => {
                // Convert base64 image to a data URL.
                let mime = media_type.as_deref().unwrap_or("image/png");
                let data_url = format!("data:{};base64,{}", mime, data);
                Some(OpenAiContentPart::InputImage {
                    image_url: Some(data_url),
                    detail: None,
                })
            }
            _ => None,
        })
        .collect();

    if openai_parts.is_empty() {
        OpenAiContent::Text(String::new())
    } else {
        OpenAiContent::Parts(openai_parts)
    }
}

/// Convert an OpenAI Responses API response into a unified Response.
pub fn convert_response(response: OpenAiResponse) -> Result<Response, Error> {
    let mut content: Vec<ContentPart> = Vec::new();

    for item in &response.output {
        match item {
            OpenAiOutputItem::Message {
                content: msg_content,
                ..
            } => {
                for c in msg_content {
                    match c {
                        OpenAiOutputContent::OutputText { text } => {
                            content.push(ContentPart::text(text));
                        }
                    }
                }
            }
            OpenAiOutputItem::FunctionCall {
                call_id,
                name,
                arguments,
                ..
            } => {
                content.push(ContentPart::ToolCall(ToolCallData {
                    id: call_id.clone(),
                    name: name.clone(),
                    arguments: arguments.clone(),
                    raw_arguments: None,
                }));
            }
            OpenAiOutputItem::Reasoning { summary } => {
                for s in summary {
                    content.push(ContentPart::Thinking(crate::types::content::ThinkingData {
                        thinking: s.text.clone(),
                        signature: None,
                        redacted: false,
                    }));
                }
            }
        }
    }

    let finish_reason = convert_finish_reason(
        response.status.as_deref(),
        response.incomplete_details.as_ref(),
        &content,
    );

    let usage = convert_usage(response.usage.as_ref());

    Ok(Response {
        id: response.id,
        model: response.model,
        content,
        finish_reason: Some(finish_reason),
        usage,
        warnings: vec![],
        rate_limit: None,
        provider: Some("openai".to_string()),
        raw: None,
    })
}

/// Map the OpenAI response status to a unified FinishReason.
fn convert_finish_reason(
    status: Option<&str>,
    incomplete_details: Option<&OpenAiIncompleteDetails>,
    content: &[ContentPart],
) -> FinishReason {
    match status {
        Some("completed") => {
            if content.iter().any(|p| p.is_tool_call()) {
                FinishReason::ToolUse
            } else {
                FinishReason::Stop
            }
        }
        Some("incomplete") => {
            if let Some(details) = incomplete_details
                && details.reason == "max_output_tokens"
            {
                return FinishReason::Length;
            }
            FinishReason::Length
        }
        Some("failed") => FinishReason::Error,
        _ => FinishReason::Stop,
    }
}

/// Map OpenAI usage to unified Usage.
fn convert_usage(usage: Option<&OpenAiUsage>) -> Usage {
    match usage {
        Some(u) => {
            let cache_read_tokens = u
                .input_tokens_details
                .as_ref()
                .map(|d| d.cached_tokens)
                .filter(|&t| t > 0);
            let reasoning_tokens = u
                .output_tokens_details
                .as_ref()
                .map(|d| d.reasoning_tokens)
                .filter(|&t| t > 0);

            Usage {
                input_tokens: u.input_tokens,
                output_tokens: u.output_tokens,
                cache_read_tokens,
                cache_creation_tokens: None,
                reasoning_tokens,
                total_tokens: None,
                raw: None,
            }
        }
        None => Usage::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{
        Message, ThinkingConfig, ToolDefinition,
        content::{ThinkingData, ToolCallData, ToolResultData},
    };
    use serde_json::json;

    // ── Request conversion tests ────────────────────────────────────────

    #[test]
    fn convert_simple_user_message() {
        let req = Request::new("gpt-4o", vec![Message::user("Hello")]);
        let oai = convert_request(&req);

        assert_eq!(oai.model, "gpt-4o");
        assert_eq!(oai.input.len(), 1);
        assert!(oai.instructions.is_none());

        let serialized = serde_json::to_value(&oai.input[0]).unwrap();
        assert_eq!(serialized["type"], "message");
        assert_eq!(serialized["role"], "user");
        assert_eq!(serialized["content"], "Hello");
    }

    #[test]
    fn convert_system_prompt_becomes_instructions() {
        let req =
            Request::new("gpt-4o", vec![Message::user("Hi")]).system_prompt("You are helpful.");

        let oai = convert_request(&req);
        assert_eq!(oai.instructions.as_deref(), Some("You are helpful."));
    }

    #[test]
    fn convert_system_message_becomes_instructions() {
        let req = Request::new(
            "gpt-4o",
            vec![Message::system("System instructions"), Message::user("Hi")],
        );

        let oai = convert_request(&req);
        assert_eq!(oai.instructions.as_deref(), Some("System instructions"));
        // System message should not appear as an input item.
        assert_eq!(oai.input.len(), 1);
    }

    #[test]
    fn convert_developer_message() {
        let req = Request::new(
            "gpt-4o",
            vec![Message::developer("Dev msg"), Message::user("Hi")],
        );

        let oai = convert_request(&req);
        assert_eq!(oai.input.len(), 2);

        let serialized = serde_json::to_value(&oai.input[0]).unwrap();
        assert_eq!(serialized["role"], "developer");
    }

    #[test]
    fn convert_tool_results() {
        let req = Request::new(
            "gpt-4o",
            vec![
                Message::user("What is the weather?"),
                Message::tool_result("call_abc", "72F sunny", false),
            ],
        );

        let oai = convert_request(&req);
        assert_eq!(oai.input.len(), 2);

        let serialized = serde_json::to_value(&oai.input[1]).unwrap();
        assert_eq!(serialized["type"], "function_call_output");
        assert_eq!(serialized["call_id"], "call_abc");
        assert_eq!(serialized["output"], "72F sunny");
    }

    #[test]
    fn convert_tools() {
        let tool = ToolDefinition::new(
            "get_weather",
            "Get the weather",
            json!({"type": "object", "properties": {"location": {"type": "string"}}}),
        );
        let req = Request::new("gpt-4o", vec![Message::user("Hi")]).tools(vec![tool]);

        let oai = convert_request(&req);
        let tools = oai.tools.unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].tool_type, "function");
        assert_eq!(tools[0].name, "get_weather");
        assert_eq!(tools[0].strict, Some(true));
    }

    #[test]
    fn convert_tool_choice_auto() {
        let req = Request::new("gpt-4o", vec![Message::user("Hi")]).tool_choice(ToolChoice::Auto);

        let oai = convert_request(&req);
        assert_eq!(oai.tool_choice, Some(json!("auto")));
    }

    #[test]
    fn convert_tool_choice_none() {
        let req = Request::new("gpt-4o", vec![Message::user("Hi")]).tool_choice(ToolChoice::None);

        let oai = convert_request(&req);
        assert_eq!(oai.tool_choice, Some(json!("none")));
    }

    #[test]
    fn convert_tool_choice_required() {
        let req =
            Request::new("gpt-4o", vec![Message::user("Hi")]).tool_choice(ToolChoice::Required);

        let oai = convert_request(&req);
        assert_eq!(oai.tool_choice, Some(json!("required")));
    }

    #[test]
    fn convert_tool_choice_specific() {
        let req =
            Request::new("gpt-4o", vec![Message::user("Hi")]).tool_choice(ToolChoice::Specific {
                name: "get_weather".into(),
            });

        let oai = convert_request(&req);
        let tc = oai.tool_choice.unwrap();
        assert_eq!(tc["type"], "function");
        assert_eq!(tc["name"], "get_weather");
    }

    #[test]
    fn convert_thinking_low_budget() {
        let req = Request::new("o3", vec![Message::user("Think")]).thinking(ThinkingConfig {
            enabled: true,
            budget_tokens: Some(3000),
        });

        let oai = convert_request(&req);
        assert_eq!(oai.reasoning.as_ref().unwrap().effort, "low");
    }

    #[test]
    fn convert_thinking_medium_budget() {
        let req = Request::new("o3", vec![Message::user("Think")]).thinking(ThinkingConfig {
            enabled: true,
            budget_tokens: Some(10000),
        });

        let oai = convert_request(&req);
        assert_eq!(oai.reasoning.as_ref().unwrap().effort, "medium");
    }

    #[test]
    fn convert_thinking_high_budget() {
        let req = Request::new("o3", vec![Message::user("Think")]).thinking(ThinkingConfig {
            enabled: true,
            budget_tokens: Some(50000),
        });

        let oai = convert_request(&req);
        assert_eq!(oai.reasoning.as_ref().unwrap().effort, "high");
    }

    #[test]
    fn convert_thinking_disabled() {
        let req = Request::new("o3", vec![Message::user("Think")]).thinking(ThinkingConfig {
            enabled: false,
            budget_tokens: Some(10000),
        });

        let oai = convert_request(&req);
        assert!(oai.reasoning.is_none());
    }

    #[test]
    fn convert_json_schema_response_format() {
        let req = Request::new("gpt-4o", vec![Message::user("Hi")]).response_format(
            ResponseFormat::JsonSchema {
                name: "person".into(),
                schema: json!({"type": "object"}),
                strict: true,
            },
        );

        let oai = convert_request(&req);
        let text_cfg = oai.text.unwrap();
        assert_eq!(text_cfg.format.format_type, "json_schema");
        assert_eq!(text_cfg.format.name.as_deref(), Some("person"));
        assert_eq!(text_cfg.format.strict, Some(true));
    }

    #[test]
    fn convert_json_object_response_format() {
        let req = Request::new("gpt-4o", vec![Message::user("Hi")])
            .response_format(ResponseFormat::JsonObject);

        let oai = convert_request(&req);
        let text_cfg = oai.text.unwrap();
        assert_eq!(text_cfg.format.format_type, "json_object");
    }

    #[test]
    fn convert_text_response_format_is_none() {
        let req =
            Request::new("gpt-4o", vec![Message::user("Hi")]).response_format(ResponseFormat::Text);

        let oai = convert_request(&req);
        assert!(oai.text.is_none());
    }

    #[test]
    fn convert_max_tokens_and_sampling() {
        let req = Request::new("gpt-4o", vec![Message::user("Hi")])
            .max_tokens(1024)
            .temperature(0.7)
            .top_p(0.9);

        let oai = convert_request(&req);
        assert_eq!(oai.max_output_tokens, Some(1024));
        assert_eq!(oai.temperature, Some(0.7));
        assert_eq!(oai.top_p, Some(0.9));
    }

    #[test]
    fn convert_assistant_message_with_tool_calls() {
        let req = Request::new(
            "gpt-4o",
            vec![
                Message::user("Weather?"),
                Message::assistant_with_tool_calls(vec![ToolCallData {
                    id: "call_1".into(),
                    name: "get_weather".into(),
                    arguments: r#"{"location":"NYC"}"#.into(),
                    raw_arguments: None,
                }]),
                Message::tool_result("call_1", "72F", false),
            ],
        );

        let oai = convert_request(&req);
        // User message + tool result (assistant tool calls are tracked but may
        // not produce an explicit input item if there's no text).
        assert_eq!(oai.input.len(), 2);
    }

    #[test]
    fn convert_multiple_tool_results() {
        let msg = Message {
            role: Role::Tool,
            content: vec![
                ContentPart::ToolResult(ToolResultData {
                    tool_call_id: "call_1".into(),
                    content: "result 1".into(),
                    is_error: false,
                }),
                ContentPart::ToolResult(ToolResultData {
                    tool_call_id: "call_2".into(),
                    content: "result 2".into(),
                    is_error: false,
                }),
            ],
            name: None,
            tool_call_id: None,
        };
        let req = Request::new("gpt-4o", vec![Message::user("Do stuff"), msg]);

        let oai = convert_request(&req);
        // 1 user message + 2 function_call_output items
        assert_eq!(oai.input.len(), 3);
    }

    // ── Response conversion tests ───────────────────────────────────────

    #[test]
    fn convert_simple_text_response() {
        let oai_response = OpenAiResponse {
            id: "resp_abc".into(),
            model: "gpt-4o".into(),
            output: vec![OpenAiOutputItem::Message {
                role: "assistant".into(),
                content: vec![OpenAiOutputContent::OutputText {
                    text: "Hello there!".into(),
                }],
            }],
            usage: Some(OpenAiUsage {
                input_tokens: 10,
                output_tokens: 5,
                input_tokens_details: None,
                output_tokens_details: None,
            }),
            status: Some("completed".into()),
            incomplete_details: None,
        };

        let resp = convert_response(oai_response).unwrap();
        assert_eq!(resp.id, "resp_abc");
        assert_eq!(resp.model, "gpt-4o");
        assert_eq!(resp.text().as_deref(), Some("Hello there!"));
        assert_eq!(resp.finish_reason, Some(FinishReason::Stop));
        assert_eq!(resp.usage.input_tokens, 10);
        assert_eq!(resp.usage.output_tokens, 5);
    }

    #[test]
    fn convert_function_call_response() {
        let oai_response = OpenAiResponse {
            id: "resp_def".into(),
            model: "gpt-4o".into(),
            output: vec![OpenAiOutputItem::FunctionCall {
                id: "fc_1".into(),
                call_id: "call_xyz".into(),
                name: "get_weather".into(),
                arguments: r#"{"location":"NYC"}"#.into(),
            }],
            usage: Some(OpenAiUsage {
                input_tokens: 20,
                output_tokens: 15,
                input_tokens_details: None,
                output_tokens_details: None,
            }),
            status: Some("completed".into()),
            incomplete_details: None,
        };

        let resp = convert_response(oai_response).unwrap();
        assert!(resp.has_tool_calls());
        assert_eq!(resp.finish_reason, Some(FinishReason::ToolUse));

        let calls = resp.tool_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].id, "call_xyz");
        assert_eq!(calls[0].name, "get_weather");
    }

    #[test]
    fn convert_incomplete_response() {
        let oai_response = OpenAiResponse {
            id: "resp_inc".into(),
            model: "gpt-4o".into(),
            output: vec![OpenAiOutputItem::Message {
                role: "assistant".into(),
                content: vec![OpenAiOutputContent::OutputText {
                    text: "Partial output...".into(),
                }],
            }],
            usage: None,
            status: Some("incomplete".into()),
            incomplete_details: Some(OpenAiIncompleteDetails {
                reason: "max_output_tokens".into(),
            }),
        };

        let resp = convert_response(oai_response).unwrap();
        assert_eq!(resp.finish_reason, Some(FinishReason::Length));
    }

    #[test]
    fn convert_failed_response() {
        let oai_response = OpenAiResponse {
            id: "resp_fail".into(),
            model: "gpt-4o".into(),
            output: vec![],
            usage: None,
            status: Some("failed".into()),
            incomplete_details: None,
        };

        let resp = convert_response(oai_response).unwrap();
        assert_eq!(resp.finish_reason, Some(FinishReason::Error));
    }

    #[test]
    fn convert_usage_with_cached_and_reasoning_tokens() {
        let oai_response = OpenAiResponse {
            id: "resp_usage".into(),
            model: "gpt-4o".into(),
            output: vec![OpenAiOutputItem::Message {
                role: "assistant".into(),
                content: vec![OpenAiOutputContent::OutputText { text: "Hi".into() }],
            }],
            usage: Some(OpenAiUsage {
                input_tokens: 100,
                output_tokens: 50,
                input_tokens_details: Some(OpenAiInputTokensDetails { cached_tokens: 30 }),
                output_tokens_details: Some(OpenAiOutputTokensDetails {
                    reasoning_tokens: 20,
                }),
            }),
            status: Some("completed".into()),
            incomplete_details: None,
        };

        let resp = convert_response(oai_response).unwrap();
        assert_eq!(resp.usage.input_tokens, 100);
        assert_eq!(resp.usage.output_tokens, 50);
        assert_eq!(resp.usage.cache_read_tokens, Some(30));
        assert_eq!(resp.usage.reasoning_tokens, Some(20));
        assert_eq!(resp.usage.cache_creation_tokens, None);
    }

    #[test]
    fn convert_usage_zero_cached_tokens_becomes_none() {
        let oai_response = OpenAiResponse {
            id: "resp_z".into(),
            model: "gpt-4o".into(),
            output: vec![],
            usage: Some(OpenAiUsage {
                input_tokens: 50,
                output_tokens: 25,
                input_tokens_details: Some(OpenAiInputTokensDetails { cached_tokens: 0 }),
                output_tokens_details: Some(OpenAiOutputTokensDetails {
                    reasoning_tokens: 0,
                }),
            }),
            status: Some("completed".into()),
            incomplete_details: None,
        };

        let resp = convert_response(oai_response).unwrap();
        assert_eq!(resp.usage.cache_read_tokens, None);
        assert_eq!(resp.usage.reasoning_tokens, None);
    }

    #[test]
    fn convert_reasoning_output() {
        let oai_response = OpenAiResponse {
            id: "resp_reason".into(),
            model: "o3".into(),
            output: vec![
                OpenAiOutputItem::Reasoning {
                    summary: vec![OpenAiReasoningSummary {
                        summary_type: "summary_text".into(),
                        text: "I need to think carefully...".into(),
                    }],
                },
                OpenAiOutputItem::Message {
                    role: "assistant".into(),
                    content: vec![OpenAiOutputContent::OutputText {
                        text: "The answer is 42.".into(),
                    }],
                },
            ],
            usage: None,
            status: Some("completed".into()),
            incomplete_details: None,
        };

        let resp = convert_response(oai_response).unwrap();
        assert_eq!(resp.content.len(), 2);

        match &resp.content[0] {
            ContentPart::Thinking(ThinkingData { thinking, .. }) => {
                assert_eq!(thinking, "I need to think carefully...");
            }
            other => panic!("Expected Thinking, got {:?}", other),
        }

        assert_eq!(resp.text().as_deref(), Some("The answer is 42."));
    }

    // ── Serde tests ─────────────────────────────────────────────────────

    #[test]
    fn openai_input_item_message_serialization() {
        let item = OpenAiInputItem::Message {
            role: "user".into(),
            content: OpenAiContent::Text("Hello".into()),
        };

        let json = serde_json::to_value(&item).unwrap();
        assert_eq!(json["type"], "message");
        assert_eq!(json["role"], "user");
        assert_eq!(json["content"], "Hello");
    }

    #[test]
    fn openai_input_item_function_call_output_serialization() {
        let item = OpenAiInputItem::FunctionCallOutput {
            call_id: "call_123".into(),
            output: "result data".into(),
        };

        let json = serde_json::to_value(&item).unwrap();
        assert_eq!(json["type"], "function_call_output");
        assert_eq!(json["call_id"], "call_123");
        assert_eq!(json["output"], "result data");
    }

    #[test]
    fn openai_content_text_serialization() {
        let content = OpenAiContent::Text("simple text".into());
        let json = serde_json::to_value(&content).unwrap();
        assert_eq!(json, "simple text");
    }

    #[test]
    fn openai_content_parts_serialization() {
        let content = OpenAiContent::Parts(vec![OpenAiContentPart::InputText {
            text: "part one".into(),
        }]);
        let json = serde_json::to_value(&content).unwrap();
        assert!(json.is_array());
        assert_eq!(json[0]["type"], "input_text");
        assert_eq!(json[0]["text"], "part one");
    }

    #[test]
    fn openai_tool_serialization() {
        let tool = OpenAiTool {
            tool_type: "function".into(),
            name: "search".into(),
            description: "Search the web".into(),
            parameters: json!({"type": "object"}),
            strict: Some(true),
        };

        let json = serde_json::to_value(&tool).unwrap();
        assert_eq!(json["type"], "function");
        assert_eq!(json["name"], "search");
        assert_eq!(json["strict"], true);
    }

    #[test]
    fn openai_response_deserialization() {
        let json_str = r#"{
            "id": "resp_123",
            "model": "gpt-4o",
            "output": [
                {
                    "type": "message",
                    "role": "assistant",
                    "content": [
                        {"type": "output_text", "text": "Hello world"}
                    ]
                }
            ],
            "usage": {
                "input_tokens": 10,
                "output_tokens": 5
            },
            "status": "completed"
        }"#;

        let resp: OpenAiResponse = serde_json::from_str(json_str).unwrap();
        assert_eq!(resp.id, "resp_123");
        assert_eq!(resp.model, "gpt-4o");
        assert_eq!(resp.output.len(), 1);
        assert_eq!(resp.status.as_deref(), Some("completed"));
    }

    #[test]
    fn openai_response_with_function_call() {
        let json_str = r#"{
            "id": "resp_456",
            "model": "gpt-4o",
            "output": [
                {
                    "type": "function_call",
                    "id": "fc_1",
                    "call_id": "call_abc",
                    "name": "get_weather",
                    "arguments": "{\"location\":\"NYC\"}"
                }
            ],
            "usage": {
                "input_tokens": 20,
                "output_tokens": 10
            },
            "status": "completed"
        }"#;

        let resp: OpenAiResponse = serde_json::from_str(json_str).unwrap();
        assert_eq!(resp.output.len(), 1);
        match &resp.output[0] {
            OpenAiOutputItem::FunctionCall { call_id, name, .. } => {
                assert_eq!(call_id, "call_abc");
                assert_eq!(name, "get_weather");
            }
            other => panic!("Expected FunctionCall, got {:?}", other),
        }
    }

    #[test]
    fn openai_response_with_reasoning() {
        let json_str = r#"{
            "id": "resp_789",
            "model": "o3",
            "output": [
                {
                    "type": "reasoning",
                    "summary": [
                        {"type": "summary_text", "text": "Thinking step..."}
                    ]
                }
            ],
            "usage": {
                "input_tokens": 30,
                "output_tokens": 40,
                "output_tokens_details": {
                    "reasoning_tokens": 25
                }
            },
            "status": "completed"
        }"#;

        let resp: OpenAiResponse = serde_json::from_str(json_str).unwrap();
        match &resp.output[0] {
            OpenAiOutputItem::Reasoning { summary } => {
                assert_eq!(summary.len(), 1);
                assert_eq!(summary[0].text, "Thinking step...");
            }
            other => panic!("Expected Reasoning, got {:?}", other),
        }
    }

    #[test]
    fn openai_response_with_usage_details() {
        let json_str = r#"{
            "id": "resp_ud",
            "model": "gpt-4o",
            "output": [],
            "usage": {
                "input_tokens": 100,
                "output_tokens": 50,
                "input_tokens_details": {
                    "cached_tokens": 30
                },
                "output_tokens_details": {
                    "reasoning_tokens": 20
                }
            },
            "status": "completed"
        }"#;

        let resp: OpenAiResponse = serde_json::from_str(json_str).unwrap();
        let usage = resp.usage.unwrap();
        assert_eq!(usage.input_tokens, 100);
        assert_eq!(usage.output_tokens, 50);
        assert_eq!(
            usage.input_tokens_details.as_ref().unwrap().cached_tokens,
            30
        );
        assert_eq!(
            usage
                .output_tokens_details
                .as_ref()
                .unwrap()
                .reasoning_tokens,
            20
        );
    }

    #[test]
    fn openai_request_skip_serializing_none_fields() {
        let oai = OpenAiRequest {
            model: "gpt-4o".into(),
            input: vec![],
            instructions: None,
            max_output_tokens: None,
            temperature: None,
            top_p: None,
            tools: None,
            tool_choice: None,
            stream: None,
            reasoning: None,
            text: None,
        };

        let json = serde_json::to_value(&oai).unwrap();
        let obj = json.as_object().unwrap();
        assert!(obj.contains_key("model"));
        assert!(obj.contains_key("input"));
        assert!(!obj.contains_key("instructions"));
        assert!(!obj.contains_key("max_output_tokens"));
        assert!(!obj.contains_key("temperature"));
        assert!(!obj.contains_key("tools"));
        assert!(!obj.contains_key("stream"));
        assert!(!obj.contains_key("reasoning"));
        assert!(!obj.contains_key("text"));
    }

    #[test]
    fn convert_image_url_content() {
        let msg = Message {
            role: Role::User,
            content: vec![
                ContentPart::text("What is this?"),
                ContentPart::Image(ImageData {
                    source_type: ImageSourceType::Url,
                    media_type: None,
                    data: "https://example.com/img.png".into(),
                }),
            ],
            name: None,
            tool_call_id: None,
        };
        let req = Request::new("gpt-4o", vec![msg]);
        let oai = convert_request(&req);

        let serialized = serde_json::to_value(&oai.input[0]).unwrap();
        let parts = serialized["content"].as_array().unwrap();
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0]["type"], "input_text");
        assert_eq!(parts[1]["type"], "input_image");
        assert_eq!(parts[1]["image_url"], "https://example.com/img.png");
    }

    #[test]
    fn convert_image_base64_content() {
        let msg = Message {
            role: Role::User,
            content: vec![ContentPart::Image(ImageData {
                source_type: ImageSourceType::Base64,
                media_type: Some("image/jpeg".into()),
                data: "aWNvbg==".into(),
            })],
            name: None,
            tool_call_id: None,
        };
        let req = Request::new("gpt-4o", vec![msg]);
        let oai = convert_request(&req);

        let serialized = serde_json::to_value(&oai.input[0]).unwrap();
        let parts = serialized["content"].as_array().unwrap();
        assert_eq!(parts[0]["type"], "input_image");
        assert_eq!(parts[0]["image_url"], "data:image/jpeg;base64,aWNvbg==");
    }
}
