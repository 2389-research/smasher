// ABOUTME: Anthropic provider adapter implementing the ProviderAdapter trait for the Messages API.
// ABOUTME: Handles both synchronous completions and streaming with SSE translation.

pub mod stream;
pub mod types;

use async_trait::async_trait;

use crate::provider::{ProviderAdapter, StreamResponse};
use crate::types::{Error, Request, Response};
use crate::util::http::build_error_from_status;
use crate::util::sse::parse_sse_stream;

/// The Anthropic API version header value.
const ANTHROPIC_API_VERSION: &str = "2023-06-01";

/// The default Anthropic API base URL.
const DEFAULT_BASE_URL: &str = "https://api.anthropic.com";

/// Provider adapter for Anthropic's Messages API.
pub struct AnthropicAdapter {
    client: reqwest::Client,
    api_key: String,
    base_url: String,
    enable_prompt_caching: bool,
}

impl AnthropicAdapter {
    /// Create a new adapter with the given API key, using the default base URL.
    /// Prompt caching is enabled by default.
    pub fn new(api_key: String) -> Self {
        Self {
            client: reqwest::Client::new(),
            api_key,
            base_url: DEFAULT_BASE_URL.to_string(),
            enable_prompt_caching: true,
        }
    }

    /// Create a new adapter with a custom base URL (for testing or proxy use).
    /// Prompt caching is enabled by default.
    pub fn with_base_url(api_key: String, base_url: String) -> Self {
        Self {
            client: reqwest::Client::new(),
            api_key,
            base_url,
            enable_prompt_caching: true,
        }
    }

    /// Enable or disable automatic prompt caching hint injection.
    /// When enabled (the default), cache_control breakpoints are placed on the
    /// system prompt and last user message to achieve up to 90% cost reduction
    /// on agentic workloads.
    pub fn prompt_caching(mut self, enabled: bool) -> Self {
        self.enable_prompt_caching = enabled;
        self
    }

    /// Build the full messages endpoint URL.
    fn messages_url(&self) -> String {
        format!("{}/v1/messages", self.base_url)
    }
}

#[async_trait]
impl ProviderAdapter for AnthropicAdapter {
    fn provider_name(&self) -> &str {
        "anthropic"
    }

    async fn complete(&self, request: &Request) -> Result<Response, Error> {
        let mut anthropic_req = types::convert_request(request);
        // Ensure stream is not set for synchronous requests.
        anthropic_req.stream = None;

        if self.enable_prompt_caching {
            types::inject_cache_control(&mut anthropic_req);
        }

        let body = serde_json::to_string(&anthropic_req).map_err(|e| Error::Serialization {
            source: e,
        })?;

        let response = self
            .client
            .post(self.messages_url())
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", ANTHROPIC_API_VERSION)
            .header("content-type", "application/json")
            .body(body)
            .send()
            .await
            .map_err(|e| Error::Http {
                provider: "anthropic".into(),
                source: e,
            })?;

        let status = response.status().as_u16();
        let headers = response.headers().clone();
        let body_text = response.text().await.map_err(|e| Error::Http {
            provider: "anthropic".into(),
            source: e,
        })?;

        if !(200..300).contains(&status) {
            return Err(build_error_from_status(
                "anthropic",
                status,
                &body_text,
                &headers,
            ));
        }

        let anthropic_response: types::AnthropicResponse =
            serde_json::from_str(&body_text).map_err(|e| Error::ResponseParse {
                provider: "anthropic".into(),
                message: format!("failed to parse response: {e}"),
            })?;

        Ok(types::convert_response(anthropic_response))
    }

    async fn stream(&self, request: &Request) -> Result<StreamResponse, Error> {
        let mut anthropic_req = types::convert_request(request);
        anthropic_req.stream = Some(true);

        if self.enable_prompt_caching {
            types::inject_cache_control(&mut anthropic_req);
        }

        let body = serde_json::to_string(&anthropic_req).map_err(|e| Error::Serialization {
            source: e,
        })?;

        let response = self
            .client
            .post(self.messages_url())
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", ANTHROPIC_API_VERSION)
            .header("content-type", "application/json")
            .body(body)
            .send()
            .await
            .map_err(|e| Error::Http {
                provider: "anthropic".into(),
                source: e,
            })?;

        let status = response.status().as_u16();

        if !(200..300).contains(&status) {
            let headers = response.headers().clone();
            let body_text = response.text().await.map_err(|e| Error::Http {
                provider: "anthropic".into(),
                source: e,
            })?;
            return Err(build_error_from_status(
                "anthropic",
                status,
                &body_text,
                &headers,
            ));
        }

        let byte_stream = response.bytes_stream();
        let sse_stream = parse_sse_stream(byte_stream);
        let event_stream = stream::translate_stream(sse_stream);

        Ok(event_stream)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{
        FinishReason, Message, StreamEvent, StreamEventType, ThinkingConfig, ToolChoice,
        ToolDefinition,
    };
    use futures::StreamExt;
    use serde_json::json;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// Helper to create a test adapter pointing at a wiremock server.
    fn test_adapter(server: &MockServer) -> AnthropicAdapter {
        AnthropicAdapter::with_base_url("test-api-key".into(), server.uri())
    }

    #[test]
    fn provider_name_is_anthropic() {
        let adapter = AnthropicAdapter::new("key".into());
        assert_eq!(adapter.provider_name(), "anthropic");
    }

    #[test]
    fn messages_url_uses_base_url() {
        let adapter = AnthropicAdapter::with_base_url(
            "key".into(),
            "https://custom.api.example.com".into(),
        );
        assert_eq!(
            adapter.messages_url(),
            "https://custom.api.example.com/v1/messages"
        );
    }

    #[tokio::test]
    async fn complete_sends_correct_headers_and_body() {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .and(header("x-api-key", "test-api-key"))
            .and(header("anthropic-version", "2023-06-01"))
            .and(header("content-type", "application/json"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "msg_test",
                "model": "claude-sonnet-4-20250514",
                "content": [{"type": "text", "text": "Hello!"}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 10, "output_tokens": 5}
            })))
            .mount(&server)
            .await;

        let adapter = test_adapter(&server);
        let request = Request::new(
            "claude-sonnet-4-20250514",
            vec![Message::user("Hi")],
        );

        let response = adapter.complete(&request).await.unwrap();
        assert_eq!(response.id, "msg_test");
        assert_eq!(response.text().as_deref(), Some("Hello!"));
        assert_eq!(response.finish_reason, Some(FinishReason::Stop));
        assert_eq!(response.usage.input_tokens, 10);
        assert_eq!(response.usage.output_tokens, 5);
    }

    #[tokio::test]
    async fn complete_with_tools_sends_correct_body() {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "msg_tools",
                "model": "claude-sonnet-4-20250514",
                "content": [
                    {
                        "type": "tool_use",
                        "id": "toolu_1",
                        "name": "get_weather",
                        "input": {"location": "NYC"}
                    }
                ],
                "stop_reason": "tool_use",
                "usage": {"input_tokens": 20, "output_tokens": 30}
            })))
            .mount(&server)
            .await;

        let adapter = test_adapter(&server);
        let tool = ToolDefinition::new(
            "get_weather",
            "Get weather",
            json!({"type": "object", "properties": {"location": {"type": "string"}}}),
        );
        let request = Request::new(
            "claude-sonnet-4-20250514",
            vec![Message::user("What's the weather?")],
        )
        .tools(vec![tool])
        .tool_choice(ToolChoice::Auto);

        let response = adapter.complete(&request).await.unwrap();
        assert_eq!(response.finish_reason, Some(FinishReason::ToolUse));
        assert!(response.has_tool_calls());
        let calls = response.tool_calls();
        assert_eq!(calls[0].name, "get_weather");
    }

    #[tokio::test]
    async fn complete_error_401() {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(
                ResponseTemplate::new(401).set_body_string("invalid api key"),
            )
            .mount(&server)
            .await;

        let adapter = test_adapter(&server);
        let request = Request::new(
            "claude-sonnet-4-20250514",
            vec![Message::user("Hi")],
        );

        let err = adapter.complete(&request).await.unwrap_err();
        match err {
            Error::Authentication { provider, message } => {
                assert_eq!(provider, "anthropic");
                assert!(message.contains("invalid api key"));
            }
            other => panic!("expected Authentication, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn complete_error_429() {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(
                ResponseTemplate::new(429)
                    .set_body_string("rate limited")
                    .insert_header("retry-after", "5"),
            )
            .mount(&server)
            .await;

        let adapter = test_adapter(&server);
        let request = Request::new(
            "claude-sonnet-4-20250514",
            vec![Message::user("Hi")],
        );

        let err = adapter.complete(&request).await.unwrap_err();
        match err {
            Error::RateLimited {
                provider,
                retry_after_ms,
            } => {
                assert_eq!(provider, "anthropic");
                assert_eq!(retry_after_ms, Some(5000));
            }
            other => panic!("expected RateLimited, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn complete_error_500() {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(
                ResponseTemplate::new(500).set_body_string("internal server error"),
            )
            .mount(&server)
            .await;

        let adapter = test_adapter(&server);
        let request = Request::new(
            "claude-sonnet-4-20250514",
            vec![Message::user("Hi")],
        );

        let err = adapter.complete(&request).await.unwrap_err();
        match err {
            Error::ServerError {
                provider,
                status_code,
                ..
            } => {
                assert_eq!(provider, "anthropic");
                assert_eq!(status_code, 500);
            }
            other => panic!("expected ServerError, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn stream_request_sets_stream_true() {
        let server = MockServer::start().await;

        // Build an SSE response body.
        let sse_body = [
            "event: message_start",
            &format!(
                "data: {}",
                json!({
                    "type": "message_start",
                    "message": {
                        "id": "msg_stream",
                        "model": "claude-sonnet-4-20250514",
                        "content": [],
                        "stop_reason": null,
                        "usage": {"input_tokens": 5, "output_tokens": 0}
                    }
                })
            ),
            "",
            "event: content_block_start",
            &format!(
                "data: {}",
                json!({
                    "type": "content_block_start",
                    "index": 0,
                    "content_block": {"type": "text", "text": ""}
                })
            ),
            "",
            "event: content_block_delta",
            &format!(
                "data: {}",
                json!({
                    "type": "content_block_delta",
                    "index": 0,
                    "delta": {"type": "text_delta", "text": "Hi!"}
                })
            ),
            "",
            "event: content_block_stop",
            &format!(
                "data: {}",
                json!({"type": "content_block_stop", "index": 0})
            ),
            "",
            "event: message_delta",
            &format!(
                "data: {}",
                json!({
                    "type": "message_delta",
                    "delta": {"stop_reason": "end_turn"},
                    "usage": {"input_tokens": 0, "output_tokens": 3}
                })
            ),
            "",
            "event: message_stop",
            &format!("data: {}", json!({"type": "message_stop"})),
            "",
        ]
        .join("\n");

        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(sse_body)
                    .insert_header("content-type", "text/event-stream"),
            )
            .mount(&server)
            .await;

        let adapter = test_adapter(&server);
        let request = Request::new(
            "claude-sonnet-4-20250514",
            vec![Message::user("Hi")],
        );

        let mut event_stream = adapter.stream(&request).await.unwrap();
        let mut events: Vec<StreamEvent> = Vec::new();
        while let Some(event_result) = event_stream.next().await {
            events.push(event_result.unwrap());
        }

        // Should have: Start, UsageDelta (from start), ContentDelta, UsageDelta (from message_delta), End
        let types: Vec<_> = events.iter().map(|e| e.event_type).collect();
        assert!(types.contains(&StreamEventType::Start));
        assert!(types.contains(&StreamEventType::ContentDelta));
        assert!(types.contains(&StreamEventType::End));

        // Find the text delta.
        let text_events: Vec<_> = events
            .iter()
            .filter(|e| e.event_type == StreamEventType::ContentDelta)
            .collect();
        assert_eq!(text_events.len(), 1);
        assert_eq!(text_events[0].text_delta.as_deref(), Some("Hi!"));
    }

    #[tokio::test]
    async fn stream_error_response() {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(
                ResponseTemplate::new(401).set_body_string("unauthorized"),
            )
            .mount(&server)
            .await;

        let adapter = test_adapter(&server);
        let request = Request::new(
            "claude-sonnet-4-20250514",
            vec![Message::user("Hi")],
        );

        let result = adapter.stream(&request).await;
        match result {
            Err(Error::Authentication { .. }) => {} // expected
            Err(other) => panic!("expected Authentication, got {:?}", other),
            Ok(_) => panic!("expected error, got Ok"),
        }
    }

    #[tokio::test]
    async fn complete_with_thinking() {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "msg_think",
                "model": "claude-sonnet-4-20250514",
                "content": [
                    {"type": "thinking", "thinking": "reasoning here", "signature": "sig_123"},
                    {"type": "text", "text": "The answer is 42."}
                ],
                "stop_reason": "end_turn",
                "usage": {
                    "input_tokens": 50,
                    "output_tokens": 100,
                    "cache_read_input_tokens": 30,
                    "cache_creation_input_tokens": 10
                }
            })))
            .mount(&server)
            .await;

        let adapter = test_adapter(&server);
        let request = Request::new(
            "claude-sonnet-4-20250514",
            vec![Message::user("What is the answer?")],
        )
        .thinking(ThinkingConfig {
            enabled: true,
            budget_tokens: Some(10000),
        });

        let response = adapter.complete(&request).await.unwrap();
        assert_eq!(response.content.len(), 2);
        assert_eq!(response.text().as_deref(), Some("The answer is 42."));
        assert_eq!(response.usage.cache_read_tokens, Some(30));
        assert_eq!(response.usage.cache_creation_tokens, Some(10));
    }

    #[tokio::test]
    async fn complete_with_system_prompt() {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "msg_sys",
                "model": "claude-sonnet-4-20250514",
                "content": [{"type": "text", "text": "I am a pirate!"}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 15, "output_tokens": 8}
            })))
            .expect(1)
            .mount(&server)
            .await;

        let adapter = test_adapter(&server);
        let request = Request::new(
            "claude-sonnet-4-20250514",
            vec![Message::user("Hello")],
        )
        .system_prompt("You are a pirate.");

        let response = adapter.complete(&request).await.unwrap();
        assert_eq!(response.text().as_deref(), Some("I am a pirate!"));
    }
}
