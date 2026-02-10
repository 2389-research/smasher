// ABOUTME: Defines the Role enum representing message sender roles in LLM conversations.
// ABOUTME: Supports System, User, Assistant, Tool, and Developer roles across all providers.

use serde::{Deserialize, Serialize};
use std::fmt;

/// Represents the role of a message sender in an LLM conversation.
///
/// Different providers support different subsets of roles:
/// - Anthropic: User, Assistant (system is separate), Tool
/// - OpenAI: System, User, Assistant, Tool, Developer
/// - Gemini: User, Model (mapped from Assistant), Tool
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
    Developer,
}

impl fmt::Display for Role {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Role::System => write!(f, "system"),
            Role::User => write!(f, "user"),
            Role::Assistant => write!(f, "assistant"),
            Role::Tool => write!(f, "tool"),
            Role::Developer => write!(f, "developer"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn role_serializes_to_snake_case() {
        assert_eq!(serde_json::to_string(&Role::System).unwrap(), "\"system\"");
        assert_eq!(serde_json::to_string(&Role::User).unwrap(), "\"user\"");
        assert_eq!(
            serde_json::to_string(&Role::Assistant).unwrap(),
            "\"assistant\""
        );
        assert_eq!(serde_json::to_string(&Role::Tool).unwrap(), "\"tool\"");
        assert_eq!(
            serde_json::to_string(&Role::Developer).unwrap(),
            "\"developer\""
        );
    }

    #[test]
    fn role_deserializes_from_snake_case() {
        assert_eq!(
            serde_json::from_str::<Role>("\"system\"").unwrap(),
            Role::System
        );
        assert_eq!(
            serde_json::from_str::<Role>("\"user\"").unwrap(),
            Role::User
        );
        assert_eq!(
            serde_json::from_str::<Role>("\"assistant\"").unwrap(),
            Role::Assistant
        );
        assert_eq!(
            serde_json::from_str::<Role>("\"tool\"").unwrap(),
            Role::Tool
        );
        assert_eq!(
            serde_json::from_str::<Role>("\"developer\"").unwrap(),
            Role::Developer
        );
    }

    #[test]
    fn role_roundtrips_through_serde() {
        for role in [
            Role::System,
            Role::User,
            Role::Assistant,
            Role::Tool,
            Role::Developer,
        ] {
            let json = serde_json::to_string(&role).unwrap();
            let back: Role = serde_json::from_str(&json).unwrap();
            assert_eq!(role, back);
        }
    }

    #[test]
    fn role_display() {
        assert_eq!(Role::System.to_string(), "system");
        assert_eq!(Role::User.to_string(), "user");
        assert_eq!(Role::Assistant.to_string(), "assistant");
        assert_eq!(Role::Tool.to_string(), "tool");
        assert_eq!(Role::Developer.to_string(), "developer");
    }
}
