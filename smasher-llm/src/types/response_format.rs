// ABOUTME: Defines the ResponseFormat enum for requesting structured output from LLM providers.
// ABOUTME: Supports plain text, JSON object, and JSON schema (with strict mode) response formats.

use serde::{Deserialize, Serialize};

/// Specifies the desired response format from the LLM.
///
/// Providers map these variants to their native structured-output mechanisms:
/// - `Text`: default free-form text output
/// - `JsonObject`: request that the model return valid JSON
/// - `JsonSchema`: request output conforming to a specific JSON Schema
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ResponseFormat {
    /// Plain text output (provider default).
    Text,
    /// Request a valid JSON object as the response.
    JsonObject,
    /// Request output conforming to a specific JSON Schema.
    JsonSchema {
        /// A human-readable name for the schema (used by some providers for caching).
        name: String,
        /// The JSON Schema definition.
        schema: serde_json::Value,
        /// Whether the provider should enforce strict schema adherence.
        strict: bool,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn text_variant_serde_roundtrip() {
        let fmt = ResponseFormat::Text;
        let json = serde_json::to_string(&fmt).unwrap();
        let back: ResponseFormat = serde_json::from_str(&json).unwrap();
        assert!(matches!(back, ResponseFormat::Text));
    }

    #[test]
    fn json_object_variant_serde_roundtrip() {
        let fmt = ResponseFormat::JsonObject;
        let json = serde_json::to_string(&fmt).unwrap();
        let back: ResponseFormat = serde_json::from_str(&json).unwrap();
        assert!(matches!(back, ResponseFormat::JsonObject));
    }

    #[test]
    fn json_schema_variant_serde_roundtrip() {
        let fmt = ResponseFormat::JsonSchema {
            name: "person".into(),
            schema: json!({
                "type": "object",
                "properties": {
                    "name": {"type": "string"},
                    "age": {"type": "integer"}
                },
                "required": ["name", "age"]
            }),
            strict: true,
        };

        let serialized = serde_json::to_string(&fmt).unwrap();
        let back: ResponseFormat = serde_json::from_str(&serialized).unwrap();

        match back {
            ResponseFormat::JsonSchema {
                name,
                schema,
                strict,
            } => {
                assert_eq!(name, "person");
                assert!(strict);
                assert_eq!(schema["properties"]["name"]["type"], "string");
                assert_eq!(schema["properties"]["age"]["type"], "integer");
            }
            other => panic!("expected JsonSchema, got {other:?}"),
        }
    }

    #[test]
    fn text_variant_json_has_correct_type_tag() {
        let fmt = ResponseFormat::Text;
        let value: serde_json::Value = serde_json::to_value(&fmt).unwrap();
        assert_eq!(value["type"], "text");
    }

    #[test]
    fn json_object_variant_json_has_correct_type_tag() {
        let fmt = ResponseFormat::JsonObject;
        let value: serde_json::Value = serde_json::to_value(&fmt).unwrap();
        assert_eq!(value["type"], "json_object");
    }

    #[test]
    fn json_schema_variant_json_has_correct_type_tag() {
        let fmt = ResponseFormat::JsonSchema {
            name: "test".into(),
            schema: json!({}),
            strict: false,
        };
        let value: serde_json::Value = serde_json::to_value(&fmt).unwrap();
        assert_eq!(value["type"], "json_schema");
    }

    #[test]
    fn json_schema_strict_false_roundtrips() {
        let fmt = ResponseFormat::JsonSchema {
            name: "loose".into(),
            schema: json!({"type": "object"}),
            strict: false,
        };

        let json = serde_json::to_string(&fmt).unwrap();
        let back: ResponseFormat = serde_json::from_str(&json).unwrap();

        match back {
            ResponseFormat::JsonSchema { strict, .. } => assert!(!strict),
            other => panic!("expected JsonSchema, got {other:?}"),
        }
    }
}
