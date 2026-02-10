// ABOUTME: Defines tool-related types for LLM function calling across providers.
// ABOUTME: Covers tool definitions, tool calls, tool results, and tool choice configuration.

use serde::{Deserialize, Serialize, de::DeserializeOwned};

/// Definition of a tool that can be called by the LLM.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    /// JSON Schema object describing the tool's parameters.
    pub parameters: serde_json::Value,
}

impl ToolDefinition {
    /// Create a new tool definition with the given name, description, and JSON Schema parameters.
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        parameters: serde_json::Value,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            parameters,
        }
    }
}

/// A tool call made by the LLM.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    /// JSON string containing the tool call arguments.
    pub arguments: String,
    /// Raw argument string before any parsing or normalization.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw_arguments: Option<String>,
}

impl ToolCall {
    /// Parse the JSON arguments string into a typed value.
    pub fn parse_arguments<T: DeserializeOwned>(&self) -> Result<T, serde_json::Error> {
        serde_json::from_str(&self.arguments)
    }
}

/// Result of executing a tool call.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    pub tool_call_id: String,
    pub content: String,
    pub is_error: bool,
}

impl ToolResult {
    /// Create a successful tool result.
    pub fn success(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            tool_call_id: tool_call_id.into(),
            content: content.into(),
            is_error: false,
        }
    }

    /// Create an error tool result.
    pub fn error(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            tool_call_id: tool_call_id.into(),
            content: content.into(),
            is_error: true,
        }
    }
}

/// Controls how the LLM should use tools.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ToolChoice {
    Auto,
    None,
    Required,
    Specific { name: String },
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // --- ToolDefinition tests ---

    #[test]
    fn tool_definition_new_constructor() {
        let params = json!({
            "type": "object",
            "properties": {
                "query": { "type": "string" }
            },
            "required": ["query"]
        });
        let def = ToolDefinition::new("search", "Search the web", params.clone());

        assert_eq!(def.name, "search");
        assert_eq!(def.description, "Search the web");
        assert_eq!(def.parameters, params);
    }

    #[test]
    fn tool_definition_serde_roundtrip() {
        let def = ToolDefinition::new(
            "get_weather",
            "Get current weather",
            json!({
                "type": "object",
                "properties": {
                    "location": { "type": "string" }
                }
            }),
        );
        let json_str = serde_json::to_string(&def).unwrap();
        let back: ToolDefinition = serde_json::from_str(&json_str).unwrap();

        assert_eq!(back.name, def.name);
        assert_eq!(back.description, def.description);
        assert_eq!(back.parameters, def.parameters);
    }

    // --- ToolCall tests ---

    #[test]
    fn tool_call_serde_roundtrip() {
        let call = ToolCall {
            id: "call_abc123".into(),
            name: "search".into(),
            arguments: r#"{"query":"rust programming"}"#.into(),
            raw_arguments: None,
        };
        let json_str = serde_json::to_string(&call).unwrap();
        let back: ToolCall = serde_json::from_str(&json_str).unwrap();

        assert_eq!(back.id, call.id);
        assert_eq!(back.name, call.name);
        assert_eq!(back.arguments, call.arguments);
    }

    #[test]
    fn tool_call_parse_arguments_valid() {
        let call = ToolCall {
            id: "call_001".into(),
            name: "get_weather".into(),
            arguments: r#"{"location":"San Francisco","units":"celsius"}"#.into(),
            raw_arguments: None,
        };

        #[derive(Deserialize, PartialEq, Debug)]
        struct WeatherArgs {
            location: String,
            units: String,
        }

        let args: WeatherArgs = call.parse_arguments().unwrap();
        assert_eq!(args.location, "San Francisco");
        assert_eq!(args.units, "celsius");
    }

    #[test]
    fn tool_call_parse_arguments_invalid_json() {
        let call = ToolCall {
            id: "call_002".into(),
            name: "search".into(),
            arguments: "not valid json".into(),
            raw_arguments: None,
        };

        let result = call.parse_arguments::<serde_json::Value>();
        assert!(result.is_err());
    }

    #[test]
    fn tool_call_parse_arguments_type_mismatch() {
        let call = ToolCall {
            id: "call_003".into(),
            name: "search".into(),
            arguments: r#"{"query":"hello"}"#.into(),
            raw_arguments: None,
        };

        #[derive(Deserialize)]
        struct ExpectedArgs {
            #[allow(dead_code)]
            count: u64,
        }

        let result = call.parse_arguments::<ExpectedArgs>();
        assert!(result.is_err());
    }

    // --- ToolResult tests ---

    #[test]
    fn tool_result_success_constructor() {
        let result = ToolResult::success("call_001", "The weather is sunny");
        assert_eq!(result.tool_call_id, "call_001");
        assert_eq!(result.content, "The weather is sunny");
        assert!(!result.is_error);
    }

    #[test]
    fn tool_result_error_constructor() {
        let result = ToolResult::error("call_002", "API key expired");
        assert_eq!(result.tool_call_id, "call_002");
        assert_eq!(result.content, "API key expired");
        assert!(result.is_error);
    }

    #[test]
    fn tool_result_serde_roundtrip() {
        let success = ToolResult::success("call_s", "ok");
        let json_str = serde_json::to_string(&success).unwrap();
        let back: ToolResult = serde_json::from_str(&json_str).unwrap();

        assert_eq!(back.tool_call_id, success.tool_call_id);
        assert_eq!(back.content, success.content);
        assert_eq!(back.is_error, success.is_error);

        let err = ToolResult::error("call_e", "boom");
        let json_str = serde_json::to_string(&err).unwrap();
        let back: ToolResult = serde_json::from_str(&json_str).unwrap();

        assert_eq!(back.tool_call_id, err.tool_call_id);
        assert_eq!(back.content, err.content);
        assert_eq!(back.is_error, err.is_error);
    }

    // --- ToolChoice tests ---

    #[test]
    fn tool_choice_auto_serialization() {
        let json_str = serde_json::to_string(&ToolChoice::Auto).unwrap();
        assert_eq!(json_str, r#""auto""#);
    }

    #[test]
    fn tool_choice_none_serialization() {
        let json_str = serde_json::to_string(&ToolChoice::None).unwrap();
        assert_eq!(json_str, r#""none""#);
    }

    #[test]
    fn tool_choice_required_serialization() {
        let json_str = serde_json::to_string(&ToolChoice::Required).unwrap();
        assert_eq!(json_str, r#""required""#);
    }

    #[test]
    fn tool_choice_specific_serialization() {
        let choice = ToolChoice::Specific {
            name: "get_weather".into(),
        };
        let json_str = serde_json::to_string(&choice).unwrap();
        let value: serde_json::Value = serde_json::from_str(&json_str).unwrap();

        assert_eq!(value, json!({"specific": {"name": "get_weather"}}));
    }

    #[test]
    fn tool_choice_serde_roundtrip() {
        let variants = [
            ToolChoice::Auto,
            ToolChoice::None,
            ToolChoice::Required,
            ToolChoice::Specific {
                name: "search".into(),
            },
        ];

        for choice in &variants {
            let json_str = serde_json::to_string(choice).unwrap();
            let back: ToolChoice = serde_json::from_str(&json_str).unwrap();
            assert_eq!(&back, choice);
        }
    }

    // --- raw_arguments field tests ---

    #[test]
    fn tool_call_raw_arguments_defaults_to_none() {
        let call = ToolCall {
            id: "call_1".into(),
            name: "fn".into(),
            arguments: "{}".into(),
            raw_arguments: None,
        };
        assert!(call.raw_arguments.is_none());
    }

    #[test]
    fn tool_call_raw_arguments_can_hold_value() {
        let call = ToolCall {
            id: "call_1".into(),
            name: "fn".into(),
            arguments: r#"{"key":"value"}"#.into(),
            raw_arguments: Some(r#"{ "key" : "value" }"#.into()),
        };
        assert_eq!(
            call.raw_arguments.as_deref(),
            Some(r#"{ "key" : "value" }"#)
        );
    }

    #[test]
    fn tool_call_serde_raw_arguments_omitted_when_none() {
        let call = ToolCall {
            id: "call_1".into(),
            name: "fn".into(),
            arguments: "{}".into(),
            raw_arguments: None,
        };
        let value: serde_json::Value = serde_json::to_value(&call).unwrap();
        let obj = value.as_object().unwrap();
        assert!(!obj.contains_key("raw_arguments"));
    }

    #[test]
    fn tool_call_serde_raw_arguments_present_when_set() {
        let call = ToolCall {
            id: "call_1".into(),
            name: "fn".into(),
            arguments: "{}".into(),
            raw_arguments: Some("{ }".into()),
        };
        let value: serde_json::Value = serde_json::to_value(&call).unwrap();
        assert_eq!(value["raw_arguments"], "{ }");
    }

    #[test]
    fn tool_call_serde_roundtrip_with_raw_arguments() {
        let call = ToolCall {
            id: "call_rt".into(),
            name: "search".into(),
            arguments: r#"{"q":"rust"}"#.into(),
            raw_arguments: Some(r#"{ "q" : "rust" }"#.into()),
        };
        let json_str = serde_json::to_string(&call).unwrap();
        let back: ToolCall = serde_json::from_str(&json_str).unwrap();

        assert_eq!(back.id, "call_rt");
        assert_eq!(back.name, "search");
        assert_eq!(back.arguments, r#"{"q":"rust"}"#);
        assert_eq!(back.raw_arguments.as_deref(), Some(r#"{ "q" : "rust" }"#));
    }

    #[test]
    fn tool_call_parse_arguments_ignores_raw_arguments() {
        let call = ToolCall {
            id: "call_1".into(),
            name: "fn".into(),
            arguments: r#"{"key":"value"}"#.into(),
            raw_arguments: Some("unparseable raw stuff".into()),
        };
        // parse_arguments should use `arguments`, not `raw_arguments`.
        let result: serde_json::Value = call.parse_arguments().unwrap();
        assert_eq!(result["key"], "value");
    }
}
