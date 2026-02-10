// ABOUTME: Convenience function for structured LLM output that deserializes JSON responses into typed Rust structs.
// ABOUTME: Wraps client.complete with JSON schema response format and automatic serde deserialization.

use serde::de::DeserializeOwned;

use crate::client::Client;
use crate::types::{Error, Request, Response, ResponseFormat};

/// The result of a successful `generate_object` call, containing both the
/// deserialized object and the raw LLM response.
#[derive(Debug)]
pub struct ObjectResult<T> {
    /// The deserialized object.
    pub object: T,
    /// The raw response from the LLM.
    pub response: Response,
}

/// Send a request with a JSON schema response format and deserialize the
/// response text into a typed Rust struct.
///
/// This sets `request.response_format` to a strict JSON schema, calls
/// `client.complete`, extracts the text from the response, and parses it
/// into the target type `T`.
pub async fn generate_object<T: DeserializeOwned>(
    client: &Client,
    mut request: Request,
    schema_name: &str,
    schema: serde_json::Value,
) -> Result<ObjectResult<T>, Error> {
    request.response_format = Some(ResponseFormat::JsonSchema {
        name: schema_name.to_string(),
        schema,
        strict: true,
    });

    let response = client.complete(request).await?;

    let text = response.text().ok_or_else(|| Error::ResponseParse {
        provider: "generate_object".into(),
        message: "no text in response".into(),
    })?;

    let object: T = serde_json::from_str(&text).map_err(|e| Error::ResponseParse {
        provider: "generate_object".into(),
        message: format!("failed to deserialize response JSON: {e}"),
    })?;

    Ok(ObjectResult { object, response })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::{ProviderAdapter, StreamResponse};
    use crate::types::{ContentPart, FinishReason, Message, Provider, Usage};
    use async_trait::async_trait;
    use serde::Deserialize;
    use std::sync::Arc;

    struct MockAdapter {
        response_text: String,
    }

    #[async_trait]
    impl ProviderAdapter for MockAdapter {
        fn provider_name(&self) -> &str {
            "anthropic"
        }

        async fn complete(&self, _: &Request) -> Result<Response, Error> {
            Ok(Response {
                id: "test".into(),
                model: "test".into(),
                content: vec![ContentPart::text(&self.response_text)],
                finish_reason: Some(FinishReason::Stop),
                usage: Usage::default(),
                warnings: vec![],
                rate_limit: None,
                provider: None,
                raw: None,
            })
        }

        async fn stream(&self, _: &Request) -> Result<StreamResponse, Error> {
            unimplemented!()
        }
    }

    /// Mock adapter that returns a response with no text content.
    struct EmptyMockAdapter;

    #[async_trait]
    impl ProviderAdapter for EmptyMockAdapter {
        fn provider_name(&self) -> &str {
            "anthropic"
        }

        async fn complete(&self, _: &Request) -> Result<Response, Error> {
            Ok(Response {
                id: "test".into(),
                model: "test".into(),
                content: vec![],
                finish_reason: Some(FinishReason::Stop),
                usage: Usage::default(),
                warnings: vec![],
                rate_limit: None,
                provider: None,
                raw: None,
            })
        }

        async fn stream(&self, _: &Request) -> Result<StreamResponse, Error> {
            unimplemented!()
        }
    }

    /// Mock adapter that captures the request to verify response_format was set.
    struct CapturingMockAdapter {
        response_text: String,
        captured_request: std::sync::Mutex<Option<Request>>,
    }

    #[async_trait]
    impl ProviderAdapter for CapturingMockAdapter {
        fn provider_name(&self) -> &str {
            "anthropic"
        }

        async fn complete(&self, request: &Request) -> Result<Response, Error> {
            *self.captured_request.lock().unwrap() = Some(request.clone());
            Ok(Response {
                id: "test".into(),
                model: "test".into(),
                content: vec![ContentPart::text(&self.response_text)],
                finish_reason: Some(FinishReason::Stop),
                usage: Usage::default(),
                warnings: vec![],
                rate_limit: None,
                provider: None,
                raw: None,
            })
        }

        async fn stream(&self, _: &Request) -> Result<StreamResponse, Error> {
            unimplemented!()
        }
    }

    fn test_client(adapter: impl ProviderAdapter + 'static) -> Client {
        let mut client = Client::new();
        client.register_provider(Provider::Anthropic, Arc::new(adapter));
        client
    }

    fn test_request() -> Request {
        Request::new(
            "claude-sonnet-4-20250514",
            vec![Message::user("test prompt")],
        )
    }

    fn person_schema() -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "name": {"type": "string"},
                "age": {"type": "integer"}
            },
            "required": ["name", "age"]
        })
    }

    #[derive(Debug, Deserialize, PartialEq)]
    struct Person {
        name: String,
        age: u32,
    }

    // Test 1: Successfully deserializes a simple struct from JSON response.
    #[tokio::test]
    async fn deserializes_simple_struct() {
        let client = test_client(MockAdapter {
            response_text: r#"{"name": "Alice", "age": 30}"#.into(),
        });

        let result: ObjectResult<Person> =
            generate_object(&client, test_request(), "person", person_schema())
                .await
                .unwrap();

        assert_eq!(
            result.object,
            Person {
                name: "Alice".into(),
                age: 30,
            }
        );
        assert_eq!(result.response.id, "test");
    }

    // Test 2: Returns error when response has no text.
    #[tokio::test]
    async fn error_when_no_text_in_response() {
        let client = test_client(EmptyMockAdapter);

        let result: Result<ObjectResult<Person>, Error> =
            generate_object(&client, test_request(), "person", person_schema()).await;

        let err = result.unwrap_err();
        assert!(matches!(err, Error::ResponseParse { .. }));
        assert!(err.to_string().contains("no text in response"));
    }

    // Test 3: Returns error when JSON doesn't match expected type.
    #[tokio::test]
    async fn error_when_json_does_not_match_type() {
        let client = test_client(MockAdapter {
            response_text: r#"{"color": "blue", "count": 5}"#.into(),
        });

        let result: Result<ObjectResult<Person>, Error> =
            generate_object(&client, test_request(), "person", person_schema()).await;

        let err = result.unwrap_err();
        assert!(matches!(err, Error::ResponseParse { .. }));
        assert!(err.to_string().contains("failed to deserialize"));
    }

    // Test 4: The request has response_format set correctly.
    #[tokio::test]
    async fn sets_response_format_on_request() {
        let adapter = Arc::new(CapturingMockAdapter {
            response_text: r#"{"name": "Bob", "age": 25}"#.into(),
            captured_request: std::sync::Mutex::new(None),
        });
        let mut client = Client::new();
        client.register_provider(Provider::Anthropic, adapter.clone());

        let schema = person_schema();
        let _result: ObjectResult<Person> =
            generate_object(&client, test_request(), "person", schema.clone())
                .await
                .unwrap();

        let captured = adapter.captured_request.lock().unwrap();
        let req = captured
            .as_ref()
            .expect("request should have been captured");

        match &req.response_format {
            Some(ResponseFormat::JsonSchema {
                name,
                schema: req_schema,
                strict,
            }) => {
                assert_eq!(name, "person");
                assert_eq!(req_schema, &schema);
                assert!(strict);
            }
            other => panic!("expected JsonSchema response_format, got {other:?}"),
        }
    }

    // Test 5: Works with nested structs.
    #[tokio::test]
    async fn works_with_nested_structs() {
        #[derive(Debug, Deserialize, PartialEq)]
        struct Address {
            street: String,
            city: String,
        }

        #[derive(Debug, Deserialize, PartialEq)]
        struct PersonWithAddress {
            name: String,
            address: Address,
        }

        let client = test_client(MockAdapter {
            response_text:
                r#"{"name": "Charlie", "address": {"street": "123 Main St", "city": "Springfield"}}"#
                    .into(),
        });

        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "name": {"type": "string"},
                "address": {
                    "type": "object",
                    "properties": {
                        "street": {"type": "string"},
                        "city": {"type": "string"}
                    },
                    "required": ["street", "city"]
                }
            },
            "required": ["name", "address"]
        });

        let result: ObjectResult<PersonWithAddress> =
            generate_object(&client, test_request(), "person_with_address", schema)
                .await
                .unwrap();

        assert_eq!(
            result.object,
            PersonWithAddress {
                name: "Charlie".into(),
                address: Address {
                    street: "123 Main St".into(),
                    city: "Springfield".into(),
                },
            }
        );
    }

    // Test 6: Works with Vec fields.
    #[tokio::test]
    async fn works_with_vec_fields() {
        #[derive(Debug, Deserialize, PartialEq)]
        struct Team {
            name: String,
            members: Vec<String>,
        }

        let client = test_client(MockAdapter {
            response_text: r#"{"name": "Alpha", "members": ["Alice", "Bob", "Charlie"]}"#.into(),
        });

        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "name": {"type": "string"},
                "members": {
                    "type": "array",
                    "items": {"type": "string"}
                }
            },
            "required": ["name", "members"]
        });

        let result: ObjectResult<Team> = generate_object(&client, test_request(), "team", schema)
            .await
            .unwrap();

        assert_eq!(result.object.name, "Alpha");
        assert_eq!(result.object.members, vec!["Alice", "Bob", "Charlie"]);
    }
}
