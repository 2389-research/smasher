// ABOUTME: OpenAI provider adapter using the Responses API (not Chat Completions).
// ABOUTME: Implements ProviderAdapter for non-streaming and streaming completions.

mod stream;
mod types;

use async_trait::async_trait;

use crate::provider::{ProviderAdapter, StreamResponse};
use crate::types::{Error, Request, Response};
use crate::util::http::{build_error_from_status, parse_rate_limit_headers};
use crate::util::sse::parse_sse_stream;

use self::stream::translate_stream;
use self::types::{convert_request, convert_response};

const DEFAULT_BASE_URL: &str = "https://api.openai.com";

/// Adapter for the OpenAI Responses API.
pub struct OpenAiAdapter {
    client: reqwest::Client,
    api_key: String,
    base_url: String,
}

impl OpenAiAdapter {
    /// Create a new adapter with the given API key and default base URL.
    pub fn new(api_key: String) -> Self {
        Self {
            client: reqwest::Client::new(),
            api_key,
            base_url: DEFAULT_BASE_URL.to_string(),
        }
    }

    /// Create a new adapter with a custom base URL (for proxies, testing, etc).
    pub fn with_base_url(api_key: String, base_url: String) -> Self {
        Self {
            client: reqwest::Client::new(),
            api_key,
            base_url,
        }
    }
}

#[async_trait]
impl ProviderAdapter for OpenAiAdapter {
    fn provider_name(&self) -> &str {
        "openai"
    }

    async fn complete(&self, request: &Request) -> Result<Response, Error> {
        let oai_request = convert_request(request);
        let url = format!("{}/v1/responses", self.base_url);

        let body = serde_json::to_string(&oai_request).map_err(|e| Error::Serialization {
            source: e,
        })?;

        let http_response = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .body(body)
            .send()
            .await
            .map_err(|e| Error::Http {
                provider: "openai".to_string(),
                source: e,
            })?;

        let status = http_response.status().as_u16();
        let headers = http_response.headers().clone();

        if !(200..300).contains(&status) {
            let body_text = http_response.text().await.unwrap_or_default();
            return Err(build_error_from_status("openai", status, &body_text, &headers));
        }

        let rate_limit = parse_rate_limit_headers(&headers);
        let body_text = http_response.text().await.map_err(|e| Error::Http {
            provider: "openai".to_string(),
            source: e,
        })?;

        let oai_response: types::OpenAiResponse =
            serde_json::from_str(&body_text).map_err(|e| Error::ResponseParse {
                provider: "openai".to_string(),
                message: format!("failed to parse OpenAI response: {}", e),
            })?;

        let mut response = convert_response(oai_response)?;
        response.rate_limit = rate_limit;

        Ok(response)
    }

    async fn stream(&self, request: &Request) -> Result<StreamResponse, Error> {
        let mut oai_request = convert_request(request);
        oai_request.stream = Some(true);

        let url = format!("{}/v1/responses", self.base_url);

        let body = serde_json::to_string(&oai_request).map_err(|e| Error::Serialization {
            source: e,
        })?;

        let http_response = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .body(body)
            .send()
            .await
            .map_err(|e| Error::Http {
                provider: "openai".to_string(),
                source: e,
            })?;

        let status = http_response.status().as_u16();
        let headers = http_response.headers().clone();

        if !(200..300).contains(&status) {
            let body_text = http_response.text().await.unwrap_or_default();
            return Err(build_error_from_status("openai", status, &body_text, &headers));
        }

        let byte_stream = http_response.bytes_stream();
        let sse_stream = parse_sse_stream(byte_stream);
        let event_stream = translate_stream(sse_stream);

        Ok(event_stream)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Message, StreamEventType};
    use futures::StreamExt;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[test]
    fn provider_name_is_openai() {
        let adapter = OpenAiAdapter::new("test-key".into());
        assert_eq!(adapter.provider_name(), "openai");
    }

    #[test]
    fn with_base_url_sets_url() {
        let adapter = OpenAiAdapter::with_base_url(
            "test-key".into(),
            "https://custom.api.com".into(),
        );
        assert_eq!(adapter.base_url, "https://custom.api.com");
    }

    #[tokio::test]
    async fn complete_sends_correct_request_and_parses_response() {
        let server = MockServer::start().await;

        let response_body = serde_json::json!({
            "id": "resp_test_001",
            "model": "gpt-4o",
            "output": [
                {
                    "type": "message",
                    "role": "assistant",
                    "content": [
                        {"type": "output_text", "text": "Hello from GPT!"}
                    ]
                }
            ],
            "usage": {
                "input_tokens": 15,
                "output_tokens": 8
            },
            "status": "completed"
        });

        Mock::given(method("POST"))
            .and(path("/v1/responses"))
            .and(header("Authorization", "Bearer test-api-key"))
            .and(header("Content-Type", "application/json"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(&response_body),
            )
            .expect(1)
            .mount(&server)
            .await;

        let adapter = OpenAiAdapter::with_base_url(
            "test-api-key".into(),
            server.uri(),
        );

        let request = Request::new("gpt-4o", vec![Message::user("Hello!")]);
        let response = adapter.complete(&request).await.unwrap();

        assert_eq!(response.id, "resp_test_001");
        assert_eq!(response.model, "gpt-4o");
        assert_eq!(response.text().as_deref(), Some("Hello from GPT!"));
        assert_eq!(response.usage.input_tokens, 15);
        assert_eq!(response.usage.output_tokens, 8);
    }

    #[tokio::test]
    async fn complete_handles_401_error() {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/v1/responses"))
            .respond_with(
                ResponseTemplate::new(401)
                    .set_body_string("invalid api key"),
            )
            .mount(&server)
            .await;

        let adapter = OpenAiAdapter::with_base_url(
            "bad-key".into(),
            server.uri(),
        );

        let request = Request::new("gpt-4o", vec![Message::user("Hello!")]);
        let err = adapter.complete(&request).await.unwrap_err();

        match err {
            Error::Authentication { provider, .. } => {
                assert_eq!(provider, "openai");
            }
            other => panic!("Expected Authentication error, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn complete_handles_429_rate_limit() {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/v1/responses"))
            .respond_with(
                ResponseTemplate::new(429)
                    .set_body_string("rate limited")
                    .insert_header("retry-after", "10"),
            )
            .mount(&server)
            .await;

        let adapter = OpenAiAdapter::with_base_url(
            "key".into(),
            server.uri(),
        );

        let request = Request::new("gpt-4o", vec![Message::user("Hello!")]);
        let err = adapter.complete(&request).await.unwrap_err();

        match err {
            Error::RateLimited {
                provider,
                retry_after_ms,
            } => {
                assert_eq!(provider, "openai");
                assert_eq!(retry_after_ms, Some(10_000));
            }
            other => panic!("Expected RateLimited error, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn complete_handles_500_server_error() {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/v1/responses"))
            .respond_with(
                ResponseTemplate::new(500)
                    .set_body_string("internal server error"),
            )
            .mount(&server)
            .await;

        let adapter = OpenAiAdapter::with_base_url(
            "key".into(),
            server.uri(),
        );

        let request = Request::new("gpt-4o", vec![Message::user("Hello!")]);
        let err = adapter.complete(&request).await.unwrap_err();

        assert!(matches!(
            err,
            Error::ServerError {
                status_code: 500,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn complete_with_tool_call_response() {
        let server = MockServer::start().await;

        let response_body = serde_json::json!({
            "id": "resp_tool",
            "model": "gpt-4o",
            "output": [
                {
                    "type": "function_call",
                    "id": "fc_1",
                    "call_id": "call_xyz",
                    "name": "get_weather",
                    "arguments": "{\"location\":\"NYC\"}"
                }
            ],
            "usage": {
                "input_tokens": 25,
                "output_tokens": 12
            },
            "status": "completed"
        });

        Mock::given(method("POST"))
            .and(path("/v1/responses"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(&response_body),
            )
            .mount(&server)
            .await;

        let adapter = OpenAiAdapter::with_base_url(
            "key".into(),
            server.uri(),
        );

        let request = Request::new("gpt-4o", vec![Message::user("Weather?")]);
        let response = adapter.complete(&request).await.unwrap();

        assert!(response.has_tool_calls());
        let calls = response.tool_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "get_weather");
        assert_eq!(calls[0].id, "call_xyz");
    }

    #[tokio::test]
    async fn stream_sends_correct_request_and_yields_events() {
        let server = MockServer::start().await;

        let sse_body = [
            "event: response.created\n",
            "data: {\"id\":\"resp_stream\",\"model\":\"gpt-4o\"}\n\n",
            "event: response.output_item.added\n",
            "data: {\"item\":{\"type\":\"message\"},\"output_index\":0}\n\n",
            "event: response.output_text.delta\n",
            "data: {\"delta\":\"Hello\"}\n\n",
            "event: response.output_text.delta\n",
            "data: {\"delta\":\" world\"}\n\n",
            "event: response.output_item.done\n",
            "data: {}\n\n",
            "event: response.completed\n",
            "data: {\"response\":{\"status\":\"completed\",\"usage\":{\"input_tokens\":10,\"output_tokens\":5}}}\n\n",
        ]
        .join("");

        Mock::given(method("POST"))
            .and(path("/v1/responses"))
            .and(header("Authorization", "Bearer stream-key"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(&sse_body)
                    .insert_header("content-type", "text/event-stream"),
            )
            .mount(&server)
            .await;

        let adapter = OpenAiAdapter::with_base_url(
            "stream-key".into(),
            server.uri(),
        );

        let request = Request::new("gpt-4o", vec![Message::user("Hello!")]);
        let stream = adapter.stream(&request).await.unwrap();

        let events: Vec<_> = stream
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .map(|r| r.expect("stream event error"))
            .collect();

        // Expect: Start, ContentDelta("Hello"), ContentDelta(" world"), End
        assert_eq!(events.len(), 4);

        assert_eq!(events[0].event_type, StreamEventType::Start);
        assert_eq!(events[0].response_id.as_deref(), Some("resp_stream"));

        assert_eq!(events[1].event_type, StreamEventType::ContentDelta);
        assert_eq!(events[1].text_delta.as_deref(), Some("Hello"));

        assert_eq!(events[2].event_type, StreamEventType::ContentDelta);
        assert_eq!(events[2].text_delta.as_deref(), Some(" world"));

        assert_eq!(events[3].event_type, StreamEventType::End);
    }

    #[tokio::test]
    async fn stream_handles_error_status() {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/v1/responses"))
            .respond_with(
                ResponseTemplate::new(401)
                    .set_body_string("unauthorized"),
            )
            .mount(&server)
            .await;

        let adapter = OpenAiAdapter::with_base_url(
            "bad-key".into(),
            server.uri(),
        );

        let request = Request::new("gpt-4o", vec![Message::user("Hello!")]);
        let result = adapter.stream(&request).await;
        match result {
            Err(Error::Authentication { .. }) => {} // expected
            Err(other) => panic!("Expected Authentication error, got: {:?}", other),
            Ok(_) => panic!("Expected error, got Ok"),
        }
    }

    #[tokio::test]
    async fn complete_handles_rate_limit_headers() {
        let server = MockServer::start().await;

        let response_body = serde_json::json!({
            "id": "resp_rl",
            "model": "gpt-4o",
            "output": [
                {
                    "type": "message",
                    "role": "assistant",
                    "content": [
                        {"type": "output_text", "text": "Hi"}
                    ]
                }
            ],
            "usage": {"input_tokens": 5, "output_tokens": 2},
            "status": "completed"
        });

        Mock::given(method("POST"))
            .and(path("/v1/responses"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(&response_body)
                    .insert_header("x-ratelimit-remaining-requests", "42")
                    .insert_header("x-ratelimit-limit-requests", "100"),
            )
            .mount(&server)
            .await;

        let adapter = OpenAiAdapter::with_base_url(
            "key".into(),
            server.uri(),
        );

        let request = Request::new("gpt-4o", vec![Message::user("Hi")]);
        let response = adapter.complete(&request).await.unwrap();

        let rl = response.rate_limit.unwrap();
        assert_eq!(rl.requests_remaining, Some(42));
        assert_eq!(rl.requests_limit, Some(100));
    }
}
