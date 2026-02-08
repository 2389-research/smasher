// ABOUTME: Defines the Message struct representing a single message in an LLM conversation.
// ABOUTME: Provides convenience constructors for common message types and query methods for content inspection.

use serde::{Deserialize, Serialize};

use super::content::{ContentPart, ToolCallData, ToolResultData};
use super::role::Role;

/// A single message in an LLM conversation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: Vec<ContentPart>,
    /// Sender name for attribution (e.g. in multi-agent scenarios).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Links a tool result message to its originating tool call.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

impl Message {
    /// Create a system message with text content.
    pub fn system(text: impl Into<String>) -> Self {
        Self {
            role: Role::System,
            content: vec![ContentPart::text(text)],
            name: None,
            tool_call_id: None,
        }
    }

    /// Create a user message with text content.
    pub fn user(text: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: vec![ContentPart::text(text)],
            name: None,
            tool_call_id: None,
        }
    }

    /// Create an assistant message with text content.
    pub fn assistant(text: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            content: vec![ContentPart::text(text)],
            name: None,
            tool_call_id: None,
        }
    }

    /// Create a tool result message.
    pub fn tool_result(
        tool_call_id: impl Into<String>,
        content: impl Into<String>,
        is_error: bool,
    ) -> Self {
        let tool_call_id = tool_call_id.into();
        Self {
            role: Role::Tool,
            content: vec![ContentPart::ToolResult(ToolResultData {
                tool_call_id: tool_call_id.clone(),
                content: content.into(),
                is_error,
            })],
            name: None,
            tool_call_id: Some(tool_call_id),
        }
    }

    /// Create a developer message with text content.
    pub fn developer(text: impl Into<String>) -> Self {
        Self {
            role: Role::Developer,
            content: vec![ContentPart::text(text)],
            name: None,
            tool_call_id: None,
        }
    }

    /// Create an assistant message containing tool calls.
    pub fn assistant_with_tool_calls(tool_calls: Vec<ToolCallData>) -> Self {
        Self {
            role: Role::Assistant,
            content: tool_calls.into_iter().map(ContentPart::ToolCall).collect(),
            name: None,
            tool_call_id: None,
        }
    }

    /// Get the text of the first text content part, if any.
    pub fn first_text(&self) -> Option<&str> {
        self.content.iter().find_map(|part| part.as_text())
    }

    /// Concatenate all text content parts into a single String.
    /// Returns None if there are no text parts.
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

    /// Set the sender name for attribution.
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// Set the tool call ID linking this message to its originating call.
    pub fn with_tool_call_id(mut self, id: impl Into<String>) -> Self {
        self.tool_call_id = Some(id.into());
        self
    }

    /// Get all tool calls in this message.
    pub fn tool_calls(&self) -> Vec<&ToolCallData> {
        self.content
            .iter()
            .filter_map(|part| match part {
                ContentPart::ToolCall(data) => Some(data),
                _ => None,
            })
            .collect()
    }

    /// Whether this message contains any tool calls.
    pub fn has_tool_calls(&self) -> bool {
        self.content.iter().any(|part| part.is_tool_call())
    }

    /// Whether this is a system message.
    pub fn is_system(&self) -> bool {
        self.role == Role::System
    }

    /// Whether this is a user message.
    pub fn is_user(&self) -> bool {
        self.role == Role::User
    }

    /// Whether this is an assistant message.
    pub fn is_assistant(&self) -> bool {
        self.role == Role::Assistant
    }

    /// Whether this is a tool result message.
    pub fn is_tool(&self) -> bool {
        self.role == Role::Tool
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- Constructor tests --

    #[test]
    fn system_creates_system_message_with_text() {
        let msg = Message::system("You are a helpful assistant.");
        assert_eq!(msg.role, Role::System);
        assert_eq!(msg.content.len(), 1);
        assert_eq!(msg.text().as_deref(), Some("You are a helpful assistant."));
    }

    #[test]
    fn user_creates_user_message_with_text() {
        let msg = Message::user("Hello!");
        assert_eq!(msg.role, Role::User);
        assert_eq!(msg.content.len(), 1);
        assert_eq!(msg.text().as_deref(), Some("Hello!"));
    }

    #[test]
    fn assistant_creates_assistant_message_with_text() {
        let msg = Message::assistant("Hi there!");
        assert_eq!(msg.role, Role::Assistant);
        assert_eq!(msg.content.len(), 1);
        assert_eq!(msg.text().as_deref(), Some("Hi there!"));
    }

    #[test]
    fn tool_result_creates_tool_message() {
        let msg = Message::tool_result("call_123", "72F and sunny", false);
        assert_eq!(msg.role, Role::Tool);
        assert_eq!(msg.content.len(), 1);
        match &msg.content[0] {
            ContentPart::ToolResult(data) => {
                assert_eq!(data.tool_call_id, "call_123");
                assert_eq!(data.content, "72F and sunny");
                assert!(!data.is_error);
            }
            other => panic!("Expected ToolResult, got {:?}", other),
        }
    }

    #[test]
    fn tool_result_with_error_flag() {
        let msg = Message::tool_result("call_fail", "API key expired", true);
        assert_eq!(msg.role, Role::Tool);
        match &msg.content[0] {
            ContentPart::ToolResult(data) => {
                assert_eq!(data.tool_call_id, "call_fail");
                assert_eq!(data.content, "API key expired");
                assert!(data.is_error);
            }
            other => panic!("Expected ToolResult, got {:?}", other),
        }
    }

    #[test]
    fn developer_creates_developer_message_with_text() {
        let msg = Message::developer("Internal instructions here.");
        assert_eq!(msg.role, Role::Developer);
        assert_eq!(msg.content.len(), 1);
        assert_eq!(msg.text().as_deref(), Some("Internal instructions here."));
    }

    // -- assistant_with_tool_calls --

    #[test]
    fn assistant_with_tool_calls_creates_correct_structure() {
        let calls = vec![
            ToolCallData {
                id: "call_1".into(),
                name: "get_weather".into(),
                arguments: r#"{"city":"NYC"}"#.into(),
                raw_arguments: None,
            },
            ToolCallData {
                id: "call_2".into(),
                name: "get_time".into(),
                arguments: r#"{"tz":"EST"}"#.into(),
                raw_arguments: None,
            },
        ];
        let msg = Message::assistant_with_tool_calls(calls);
        assert_eq!(msg.role, Role::Assistant);
        assert_eq!(msg.content.len(), 2);
        assert!(msg.content[0].is_tool_call());
        assert!(msg.content[1].is_tool_call());
    }

    // -- text() --

    #[test]
    fn text_returns_first_text_from_content() {
        let msg = Message {
            role: Role::Assistant,
            content: vec![ContentPart::text("first"), ContentPart::text("second")],
            name: None,
            tool_call_id: None,
        };
        assert_eq!(msg.text().as_deref(), Some("firstsecond"));
    }

    #[test]
    fn text_returns_none_when_no_text_content() {
        let msg = Message::assistant_with_tool_calls(vec![ToolCallData {
            id: "call_1".into(),
            name: "search".into(),
            arguments: "{}".into(),
            raw_arguments: None,
        }]);
        assert_eq!(msg.text(), None);
    }

    #[test]
    fn text_skips_non_text_to_find_first_text() {
        let msg = Message {
            role: Role::Assistant,
            content: vec![
                ContentPart::ToolCall(ToolCallData {
                    id: "call_1".into(),
                    name: "search".into(),
                    arguments: "{}".into(),
                    raw_arguments: None,
                }),
                ContentPart::text("found it"),
            ],
            name: None,
            tool_call_id: None,
        };
        assert_eq!(msg.text().as_deref(), Some("found it"));
    }

    // -- tool_calls() --

    #[test]
    fn tool_calls_extracts_tool_call_data() {
        let tc1 = ToolCallData {
            id: "call_1".into(),
            name: "get_weather".into(),
            arguments: r#"{"city":"NYC"}"#.into(),
            raw_arguments: None,
        };
        let tc2 = ToolCallData {
            id: "call_2".into(),
            name: "get_time".into(),
            arguments: r#"{"tz":"EST"}"#.into(),
            raw_arguments: None,
        };
        let msg = Message::assistant_with_tool_calls(vec![tc1, tc2]);
        let calls = msg.tool_calls();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].id, "call_1");
        assert_eq!(calls[0].name, "get_weather");
        assert_eq!(calls[1].id, "call_2");
        assert_eq!(calls[1].name, "get_time");
    }

    #[test]
    fn tool_calls_returns_empty_for_text_message() {
        let msg = Message::user("hello");
        assert!(msg.tool_calls().is_empty());
    }

    // -- has_tool_calls() --

    #[test]
    fn has_tool_calls_true_when_tool_calls_present() {
        let msg = Message::assistant_with_tool_calls(vec![ToolCallData {
            id: "call_1".into(),
            name: "fn".into(),
            arguments: "{}".into(),
            raw_arguments: None,
        }]);
        assert!(msg.has_tool_calls());
    }

    #[test]
    fn has_tool_calls_false_when_no_tool_calls() {
        let msg = Message::user("no tools here");
        assert!(!msg.has_tool_calls());
    }

    // -- Role checking methods --

    #[test]
    fn is_system_returns_true_for_system_message() {
        assert!(Message::system("sys").is_system());
    }

    #[test]
    fn is_system_returns_false_for_non_system_message() {
        assert!(!Message::user("usr").is_system());
    }

    #[test]
    fn is_user_returns_true_for_user_message() {
        assert!(Message::user("usr").is_user());
    }

    #[test]
    fn is_user_returns_false_for_non_user_message() {
        assert!(!Message::assistant("asst").is_user());
    }

    #[test]
    fn is_assistant_returns_true_for_assistant_message() {
        assert!(Message::assistant("asst").is_assistant());
    }

    #[test]
    fn is_assistant_returns_false_for_non_assistant_message() {
        assert!(!Message::user("usr").is_assistant());
    }

    #[test]
    fn is_tool_returns_true_for_tool_message() {
        assert!(Message::tool_result("id", "result", false).is_tool());
    }

    #[test]
    fn is_tool_returns_false_for_non_tool_message() {
        assert!(!Message::assistant("asst").is_tool());
    }

    #[test]
    fn is_assistant_true_for_assistant_with_tool_calls() {
        let msg = Message::assistant_with_tool_calls(vec![ToolCallData {
            id: "call_1".into(),
            name: "fn".into(),
            arguments: "{}".into(),
            raw_arguments: None,
        }]);
        assert!(msg.is_assistant());
    }

    // -- Serde roundtrip tests --

    #[test]
    fn serde_roundtrip_text_message() {
        let msg = Message::user("Hello, world!");
        let json = serde_json::to_string(&msg).unwrap();
        let back: Message = serde_json::from_str(&json).unwrap();
        assert_eq!(back.role, Role::User);
        assert_eq!(back.text().as_deref(), Some("Hello, world!"));
    }

    #[test]
    fn serde_roundtrip_tool_result_message() {
        let msg = Message::tool_result("call_abc", "result data", false);
        let json = serde_json::to_string(&msg).unwrap();
        let back: Message = serde_json::from_str(&json).unwrap();
        assert_eq!(back.role, Role::Tool);
        match &back.content[0] {
            ContentPart::ToolResult(data) => {
                assert_eq!(data.tool_call_id, "call_abc");
                assert_eq!(data.content, "result data");
                assert!(!data.is_error);
            }
            other => panic!("Expected ToolResult, got {:?}", other),
        }
    }

    #[test]
    fn serde_roundtrip_assistant_with_tool_calls() {
        let msg = Message::assistant_with_tool_calls(vec![
            ToolCallData {
                id: "call_1".into(),
                name: "search".into(),
                arguments: r#"{"q":"rust"}"#.into(),
                raw_arguments: None,
            },
            ToolCallData {
                id: "call_2".into(),
                name: "calc".into(),
                arguments: r#"{"expr":"1+1"}"#.into(),
                raw_arguments: None,
            },
        ]);
        let json = serde_json::to_string(&msg).unwrap();
        let back: Message = serde_json::from_str(&json).unwrap();
        assert_eq!(back.role, Role::Assistant);
        assert_eq!(back.tool_calls().len(), 2);
        assert_eq!(back.tool_calls()[0].name, "search");
        assert_eq!(back.tool_calls()[1].name, "calc");
    }

    #[test]
    fn serde_roundtrip_developer_message() {
        let msg = Message::developer("secret instructions");
        let json = serde_json::to_string(&msg).unwrap();
        let back: Message = serde_json::from_str(&json).unwrap();
        assert_eq!(back.role, Role::Developer);
        assert_eq!(back.text().as_deref(), Some("secret instructions"));
    }

    #[test]
    fn serde_roundtrip_system_message() {
        let msg = Message::system("You are helpful.");
        let json = serde_json::to_string(&msg).unwrap();
        let back: Message = serde_json::from_str(&json).unwrap();
        assert_eq!(back.role, Role::System);
        assert_eq!(back.text().as_deref(), Some("You are helpful."));
    }

    // -- Multiple content parts --

    #[test]
    fn message_with_multiple_content_parts() {
        let msg = Message {
            role: Role::Assistant,
            content: vec![
                ContentPart::text("Here are the results:"),
                ContentPart::ToolCall(ToolCallData {
                    id: "call_1".into(),
                    name: "search".into(),
                    arguments: r#"{"q":"rust"}"#.into(),
                    raw_arguments: None,
                }),
                ContentPart::text("And some more text"),
            ],
            name: None,
            tool_call_id: None,
        };
        assert_eq!(msg.text().as_deref(), Some("Here are the results:And some more text"));
        assert_eq!(msg.tool_calls().len(), 1);
        assert!(msg.has_tool_calls());
        assert!(msg.is_assistant());
        assert_eq!(msg.content.len(), 3);
    }

    #[test]
    fn serde_roundtrip_mixed_content_message() {
        let msg = Message {
            role: Role::Assistant,
            content: vec![
                ContentPart::text("Calling tools now"),
                ContentPart::ToolCall(ToolCallData {
                    id: "call_x".into(),
                    name: "lookup".into(),
                    arguments: r#"{"key":"val"}"#.into(),
                    raw_arguments: None,
                }),
            ],
            name: None,
            tool_call_id: None,
        };
        let json = serde_json::to_string(&msg).unwrap();
        let back: Message = serde_json::from_str(&json).unwrap();
        assert_eq!(back.content.len(), 2);
        assert_eq!(back.text().as_deref(), Some("Calling tools now"));
        assert_eq!(back.tool_calls().len(), 1);
        assert_eq!(back.tool_calls()[0].id, "call_x");
    }

    // -- Constructors accept String and &str --

    #[test]
    fn constructors_accept_owned_string() {
        let owned = String::from("owned text");
        let msg = Message::user(owned);
        assert_eq!(msg.text().as_deref(), Some("owned text"));
    }

    #[test]
    fn constructors_accept_str_ref() {
        let msg = Message::user("str ref");
        assert_eq!(msg.text().as_deref(), Some("str ref"));
    }

    #[test]
    fn empty_content_message() {
        let msg = Message {
            role: Role::User,
            content: vec![],
            name: None,
            tool_call_id: None,
        };
        assert_eq!(msg.text(), None);
        assert!(msg.tool_calls().is_empty());
        assert!(!msg.has_tool_calls());
    }

    // -- name field tests --

    #[test]
    fn constructors_set_name_to_none() {
        let msg = Message::user("hi");
        assert!(msg.name.is_none());
        let msg = Message::assistant("hello");
        assert!(msg.name.is_none());
        let msg = Message::system("sys");
        assert!(msg.name.is_none());
        let msg = Message::developer("dev");
        assert!(msg.name.is_none());
        let msg = Message::tool_result("id", "result", false);
        assert!(msg.name.is_none());
    }

    #[test]
    fn with_name_sets_name() {
        let msg = Message::user("hi").with_name("agent_alpha");
        assert_eq!(msg.name.as_deref(), Some("agent_alpha"));
    }

    #[test]
    fn with_name_accepts_owned_string() {
        let name = String::from("agent_beta");
        let msg = Message::user("hi").with_name(name);
        assert_eq!(msg.name.as_deref(), Some("agent_beta"));
    }

    #[test]
    fn with_name_overwrites_previous_name() {
        let msg = Message::user("hi")
            .with_name("first")
            .with_name("second");
        assert_eq!(msg.name.as_deref(), Some("second"));
    }

    // -- tool_call_id field tests --

    #[test]
    fn tool_result_constructor_sets_tool_call_id() {
        let msg = Message::tool_result("call_xyz", "result data", false);
        assert_eq!(msg.tool_call_id.as_deref(), Some("call_xyz"));
    }

    #[test]
    fn non_tool_constructors_set_tool_call_id_to_none() {
        let msg = Message::user("hi");
        assert!(msg.tool_call_id.is_none());
        let msg = Message::assistant("hello");
        assert!(msg.tool_call_id.is_none());
    }

    #[test]
    fn with_tool_call_id_sets_tool_call_id() {
        let msg = Message::user("result for call")
            .with_tool_call_id("call_abc");
        assert_eq!(msg.tool_call_id.as_deref(), Some("call_abc"));
    }

    #[test]
    fn with_tool_call_id_accepts_owned_string() {
        let id = String::from("call_owned");
        let msg = Message::user("result").with_tool_call_id(id);
        assert_eq!(msg.tool_call_id.as_deref(), Some("call_owned"));
    }

    // -- first_text() tests --

    #[test]
    fn first_text_returns_first_text_part() {
        let msg = Message {
            role: Role::Assistant,
            content: vec![ContentPart::text("first"), ContentPart::text("second")],
            name: None,
            tool_call_id: None,
        };
        assert_eq!(msg.first_text(), Some("first"));
    }

    #[test]
    fn first_text_skips_non_text_parts() {
        let msg = Message {
            role: Role::Assistant,
            content: vec![
                ContentPart::ToolCall(ToolCallData {
                    id: "call_1".into(),
                    name: "fn".into(),
                    arguments: "{}".into(),
                    raw_arguments: None,
                }),
                ContentPart::text("after tool"),
            ],
            name: None,
            tool_call_id: None,
        };
        assert_eq!(msg.first_text(), Some("after tool"));
    }

    #[test]
    fn first_text_returns_none_when_no_text() {
        let msg = Message::assistant_with_tool_calls(vec![ToolCallData {
            id: "call_1".into(),
            name: "fn".into(),
            arguments: "{}".into(),
            raw_arguments: None,
        }]);
        assert_eq!(msg.first_text(), None);
    }

    // -- text() concatenation tests --

    #[test]
    fn text_concatenates_all_text_parts() {
        let msg = Message {
            role: Role::Assistant,
            content: vec![
                ContentPart::text("Hello, "),
                ContentPart::text("world!"),
            ],
            name: None,
            tool_call_id: None,
        };
        assert_eq!(msg.text().as_deref(), Some("Hello, world!"));
    }

    #[test]
    fn text_concatenates_skipping_non_text_parts() {
        let msg = Message {
            role: Role::Assistant,
            content: vec![
                ContentPart::text("before"),
                ContentPart::ToolCall(ToolCallData {
                    id: "call_1".into(),
                    name: "fn".into(),
                    arguments: "{}".into(),
                    raw_arguments: None,
                }),
                ContentPart::text("after"),
            ],
            name: None,
            tool_call_id: None,
        };
        assert_eq!(msg.text().as_deref(), Some("beforeafter"));
    }

    // -- Serde roundtrip with new fields --

    #[test]
    fn serde_roundtrip_message_with_name() {
        let msg = Message::user("hello").with_name("agent_x");
        let json = serde_json::to_string(&msg).unwrap();
        let back: Message = serde_json::from_str(&json).unwrap();
        assert_eq!(back.name.as_deref(), Some("agent_x"));
        assert_eq!(back.text().as_deref(), Some("hello"));
    }

    #[test]
    fn serde_roundtrip_message_with_tool_call_id() {
        let msg = Message::tool_result("call_abc", "ok", false);
        let json = serde_json::to_string(&msg).unwrap();
        let back: Message = serde_json::from_str(&json).unwrap();
        assert_eq!(back.tool_call_id.as_deref(), Some("call_abc"));
    }

    #[test]
    fn serde_name_omitted_when_none() {
        let msg = Message::user("hello");
        let value: serde_json::Value = serde_json::to_value(&msg).unwrap();
        let obj = value.as_object().unwrap();
        assert!(!obj.contains_key("name"));
    }

    #[test]
    fn serde_tool_call_id_omitted_when_none() {
        let msg = Message::user("hello");
        let value: serde_json::Value = serde_json::to_value(&msg).unwrap();
        let obj = value.as_object().unwrap();
        assert!(!obj.contains_key("tool_call_id"));
    }

    #[test]
    fn serde_name_present_when_set() {
        let msg = Message::user("hello").with_name("agent_x");
        let value: serde_json::Value = serde_json::to_value(&msg).unwrap();
        assert_eq!(value["name"], "agent_x");
    }

    #[test]
    fn serde_tool_call_id_present_when_set() {
        let msg = Message::tool_result("call_1", "ok", false);
        let value: serde_json::Value = serde_json::to_value(&msg).unwrap();
        assert_eq!(value["tool_call_id"], "call_1");
    }

    #[test]
    fn builder_methods_chainable() {
        let msg = Message::user("hi")
            .with_name("agent_x")
            .with_tool_call_id("call_1");
        assert_eq!(msg.name.as_deref(), Some("agent_x"));
        assert_eq!(msg.tool_call_id.as_deref(), Some("call_1"));
        assert_eq!(msg.text().as_deref(), Some("hi"));
    }
}
