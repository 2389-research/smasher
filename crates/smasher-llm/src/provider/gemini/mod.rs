// ABOUTME: GeminiAdapter implementing the ProviderAdapter trait for Google's Gemini API.
// ABOUTME: Routes requests through the generateContent and streamGenerateContent endpoints with API key auth.

pub mod stream;
pub mod types;

use async_trait::async_trait;
use reqwest::Client;

use crate::provider::{ProviderAdapter, StreamResponse};
use crate::types::{Error, Request, Response};
use crate::util::http::build_error_from_status;
use crate::util::sse::parse_sse_stream;

use self::stream::translate_stream;
use self::types::{convert_request, convert_response};

const DEFAULT_BASE_URL: &str = "https://generativelanguage.googleapis.com";

/// Adapter for Google's Gemini API (native generateContent endpoint).
///
/// Uses API key authentication via query parameter for direct Google API
/// access. When a custom base URL is configured (e.g. Cloudflare AI Gateway),
/// the key is sent as a Bearer token header instead.
pub struct GeminiAdapter {
    client: Client,
    api_key: String,
    base_url: String,
    /// When true, send the API key as an Authorization header instead of a
    /// query parameter. Enabled automatically for non-default base URLs
    /// (proxies and gateways).
    use_header_auth: bool,
}

impl GeminiAdapter {
    /// Create a GeminiAdapter with the default base URL.
    pub fn new(api_key: String) -> Self {
        Self {
            client: Client::new(),
            api_key,
            base_url: DEFAULT_BASE_URL.to_string(),
            use_header_auth: false,
        }
    }

    /// Create a GeminiAdapter with a custom base URL (for testing or proxies).
    pub fn with_base_url(api_key: String, base_url: String) -> Self {
        let use_header_auth = base_url != DEFAULT_BASE_URL;
        Self {
            client: Client::new(),
            api_key,
            base_url,
            use_header_auth,
        }
    }

    /// Build the URL for the generateContent (non-streaming) endpoint.
    fn complete_url(&self, model: &str) -> String {
        if self.use_header_auth {
            format!("{}/v1beta/models/{}:generateContent", self.base_url, model)
        } else {
            format!(
                "{}/v1beta/models/{}:generateContent?key={}",
                self.base_url, model, self.api_key
            )
        }
    }

    /// Build the URL for the streamGenerateContent (streaming) endpoint.
    fn stream_url(&self, model: &str) -> String {
        if self.use_header_auth {
            format!(
                "{}/v1beta/models/{}:streamGenerateContent?alt=sse",
                self.base_url, model
            )
        } else {
            format!(
                "{}/v1beta/models/{}:streamGenerateContent?alt=sse&key={}",
                self.base_url, model, self.api_key
            )
        }
    }
}

#[async_trait]
impl ProviderAdapter for GeminiAdapter {
    fn provider_name(&self) -> &str {
        "gemini"
    }

    async fn complete(&self, request: &Request) -> Result<Response, Error> {
        let gemini_req = convert_request(request);
        let url = self.complete_url(&request.model);

        let body =
            serde_json::to_string(&gemini_req).map_err(|e| Error::Serialization { source: e })?;

        let mut req_builder = self
            .client
            .post(&url)
            .header("Content-Type", "application/json");
        if self.use_header_auth {
            req_builder = req_builder.header("Authorization", format!("Bearer {}", self.api_key));
        }
        let http_response = req_builder
            .body(body)
            .send()
            .await
            .map_err(|e| Error::Http {
                provider: "gemini".to_string(),
                source: e,
            })?;

        let status = http_response.status().as_u16();
        let headers = http_response.headers().clone();

        let response_body = http_response.text().await.map_err(|e| Error::Http {
            provider: "gemini".to_string(),
            source: e,
        })?;

        if !(200..300).contains(&status) {
            return Err(build_error_from_status(
                "gemini",
                status,
                &response_body,
                &headers,
            ));
        }

        let gemini_response: types::GeminiResponse =
            serde_json::from_str(&response_body).map_err(|e| Error::ResponseParse {
                provider: "gemini".to_string(),
                message: format!("failed to parse response: {e}"),
            })?;

        Ok(convert_response(gemini_response, &request.model))
    }

    async fn stream(&self, request: &Request) -> Result<StreamResponse, Error> {
        let gemini_req = convert_request(request);
        let url = self.stream_url(&request.model);

        let body =
            serde_json::to_string(&gemini_req).map_err(|e| Error::Serialization { source: e })?;

        let mut req_builder = self
            .client
            .post(&url)
            .header("Content-Type", "application/json");
        if self.use_header_auth {
            req_builder = req_builder.header("Authorization", format!("Bearer {}", self.api_key));
        }
        let http_response = req_builder
            .body(body)
            .send()
            .await
            .map_err(|e| Error::Http {
                provider: "gemini".to_string(),
                source: e,
            })?;

        let status = http_response.status().as_u16();

        if !(200..300).contains(&status) {
            let headers = http_response.headers().clone();
            let response_body = http_response.text().await.map_err(|e| Error::Http {
                provider: "gemini".to_string(),
                source: e,
            })?;
            return Err(build_error_from_status(
                "gemini",
                status,
                &response_body,
                &headers,
            ));
        }

        let byte_stream = http_response.bytes_stream();
        let sse_stream = parse_sse_stream(byte_stream);
        let event_stream = translate_stream(sse_stream, request.model.clone());

        Ok(event_stream)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Message;
    use futures::StreamExt;
    use serde_json::json;
    use wiremock::matchers::{method, path_regex};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[test]
    fn provider_name_returns_gemini() {
        let adapter = GeminiAdapter::new("test-key".to_string());
        assert_eq!(adapter.provider_name(), "gemini");
    }

    #[test]
    fn complete_url_format() {
        let adapter = GeminiAdapter::new("my-api-key".to_string());
        let url = adapter.complete_url("gemini-2.0-flash");
        assert_eq!(
            url,
            "https://generativelanguage.googleapis.com/v1beta/models/gemini-2.0-flash:generateContent?key=my-api-key"
        );
    }

    #[test]
    fn stream_url_format() {
        let adapter = GeminiAdapter::new("my-api-key".to_string());
        let url = adapter.stream_url("gemini-2.0-flash");
        assert_eq!(
            url,
            "https://generativelanguage.googleapis.com/v1beta/models/gemini-2.0-flash:streamGenerateContent?alt=sse&key=my-api-key"
        );
    }

    #[test]
    fn custom_base_url_uses_header_auth() {
        let adapter =
            GeminiAdapter::with_base_url("key".to_string(), "http://localhost:9999".to_string());
        assert!(adapter.use_header_auth);
        let url = adapter.complete_url("gemini-pro");
        assert!(url.starts_with("http://localhost:9999"));
        // Custom base URL should NOT include the API key in the query string.
        assert!(!url.contains("key="));
    }

    #[tokio::test]
    async fn complete_text_response() {
        let mock_server = MockServer::start().await;

        let response_body = json!({
            "candidates": [{
                "content": {
                    "role": "model",
                    "parts": [{"text": "Hello from Gemini!"}]
                },
                "finishReason": "STOP"
            }],
            "usageMetadata": {
                "promptTokenCount": 5,
                "candidatesTokenCount": 4,
                "totalTokenCount": 9
            },
            "modelVersion": "gemini-2.0-flash"
        });

        Mock::given(method("POST"))
            .and(path_regex(r"/v1beta/models/.*:generateContent"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(&response_body)
                    .insert_header("Content-Type", "application/json"),
            )
            .mount(&mock_server)
            .await;

        let adapter = GeminiAdapter::with_base_url("test-key".to_string(), mock_server.uri());

        let request = Request::new("gemini-2.0-flash", vec![Message::user("Hello!")]);

        let response = adapter.complete(&request).await.unwrap();

        assert_eq!(response.text().as_deref(), Some("Hello from Gemini!"));
        assert_eq!(
            response.finish_reason,
            Some(crate::types::FinishReason::Stop)
        );
        assert_eq!(response.usage.input_tokens, 5);
        assert_eq!(response.usage.output_tokens, 4);
        assert_eq!(response.model, "gemini-2.0-flash");
    }

    #[tokio::test]
    async fn complete_function_call_response() {
        let mock_server = MockServer::start().await;

        let response_body = json!({
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
                "promptTokenCount": 10,
                "candidatesTokenCount": 8,
                "totalTokenCount": 18
            }
        });

        Mock::given(method("POST"))
            .and(path_regex(r"/v1beta/models/.*:generateContent"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(&response_body)
                    .insert_header("Content-Type", "application/json"),
            )
            .mount(&mock_server)
            .await;

        let adapter = GeminiAdapter::with_base_url("test-key".to_string(), mock_server.uri());

        let request = Request::new(
            "gemini-2.0-flash",
            vec![Message::user("What's the weather?")],
        );

        let response = adapter.complete(&request).await.unwrap();

        assert!(response.has_tool_calls());
        let calls = response.tool_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "get_weather");
    }

    #[tokio::test]
    async fn complete_auth_error() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path_regex(r"/v1beta/models/.*:generateContent"))
            .respond_with(ResponseTemplate::new(401).set_body_string("API key not valid"))
            .mount(&mock_server)
            .await;

        let adapter = GeminiAdapter::with_base_url("bad-key".to_string(), mock_server.uri());

        let request = Request::new("gemini-2.0-flash", vec![Message::user("Hello")]);

        let err = adapter.complete(&request).await.unwrap_err();
        assert!(matches!(err, Error::Authentication { .. }));
    }

    #[tokio::test]
    async fn complete_rate_limited_error() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path_regex(r"/v1beta/models/.*:generateContent"))
            .respond_with(ResponseTemplate::new(429).set_body_string("Too many requests"))
            .mount(&mock_server)
            .await;

        let adapter = GeminiAdapter::with_base_url("test-key".to_string(), mock_server.uri());

        let request = Request::new("gemini-2.0-flash", vec![Message::user("Hello")]);

        let err = adapter.complete(&request).await.unwrap_err();
        assert!(matches!(err, Error::RateLimited { .. }));
    }

    #[tokio::test]
    async fn complete_server_error() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path_regex(r"/v1beta/models/.*:generateContent"))
            .respond_with(ResponseTemplate::new(500).set_body_string("Internal Server Error"))
            .mount(&mock_server)
            .await;

        let adapter = GeminiAdapter::with_base_url("test-key".to_string(), mock_server.uri());

        let request = Request::new("gemini-2.0-flash", vec![Message::user("Hello")]);

        let err = adapter.complete(&request).await.unwrap_err();
        assert!(matches!(err, Error::ServerError { .. }));
    }

    #[tokio::test]
    async fn complete_invalid_json_response() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path_regex(r"/v1beta/models/.*:generateContent"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string("not json at all")
                    .insert_header("Content-Type", "application/json"),
            )
            .mount(&mock_server)
            .await;

        let adapter = GeminiAdapter::with_base_url("test-key".to_string(), mock_server.uri());

        let request = Request::new("gemini-2.0-flash", vec![Message::user("Hello")]);

        let err = adapter.complete(&request).await.unwrap_err();
        assert!(matches!(err, Error::ResponseParse { .. }));
    }

    #[tokio::test]
    async fn stream_text_response() {
        let mock_server = MockServer::start().await;

        let chunk1 = json!({
            "candidates": [{
                "content": {
                    "role": "model",
                    "parts": [{"text": "Streaming "}]
                }
            }],
            "modelVersion": "gemini-2.0-flash"
        });

        let chunk2 = json!({
            "candidates": [{
                "content": {
                    "role": "model",
                    "parts": [{"text": "response!"}]
                },
                "finishReason": "STOP"
            }],
            "usageMetadata": {
                "promptTokenCount": 5,
                "candidatesTokenCount": 3,
                "totalTokenCount": 8
            }
        });

        let sse_body = format!("data: {}\n\ndata: {}\n\n", chunk1, chunk2);

        Mock::given(method("POST"))
            .and(path_regex(r"/v1beta/models/.*:streamGenerateContent"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(sse_body)
                    .insert_header("Content-Type", "text/event-stream"),
            )
            .mount(&mock_server)
            .await;

        let adapter = GeminiAdapter::with_base_url("test-key".to_string(), mock_server.uri());

        let request = Request::new("gemini-2.0-flash", vec![Message::user("Stream please")]);

        let stream = adapter.stream(&request).await.unwrap();
        let events: Vec<_> = stream
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .map(|r| r.unwrap())
            .collect();

        // Should have at least Start, ContentDelta, ContentDelta, End.
        assert!(events.len() >= 3);

        // First is Start.
        assert_eq!(events[0].event_type, crate::types::StreamEventType::Start);

        // Check we got content deltas.
        let text_deltas: Vec<_> = events
            .iter()
            .filter_map(|e| e.text_delta.as_deref())
            .collect();
        assert!(text_deltas.contains(&"Streaming "));
        assert!(text_deltas.contains(&"response!"));

        // Check End event.
        let end_event = events
            .iter()
            .find(|e| e.event_type == crate::types::StreamEventType::End);
        assert!(end_event.is_some());
    }

    #[tokio::test]
    async fn stream_error_response() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path_regex(r"/v1beta/models/.*:streamGenerateContent"))
            .respond_with(ResponseTemplate::new(403).set_body_string("Forbidden"))
            .mount(&mock_server)
            .await;

        let adapter = GeminiAdapter::with_base_url("test-key".to_string(), mock_server.uri());

        let request = Request::new("gemini-2.0-flash", vec![Message::user("Stream this")]);

        let result = adapter.stream(&request).await;
        match result {
            Err(Error::AccessDenied { .. }) => {} // 403 maps to AccessDenied
            Err(other) => panic!("Expected AccessDenied error, got: {:?}", other),
            Ok(_) => panic!("Expected error, got Ok"),
        }
    }

    #[tokio::test]
    async fn complete_sends_correct_request_body() {
        let mock_server = MockServer::start().await;

        let response_body = json!({
            "candidates": [{
                "content": {
                    "role": "model",
                    "parts": [{"text": "ok"}]
                },
                "finishReason": "STOP"
            }]
        });

        Mock::given(method("POST"))
            .and(path_regex(r"/v1beta/models/.*:generateContent"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(&response_body)
                    .insert_header("Content-Type", "application/json"),
            )
            .expect(1)
            .mount(&mock_server)
            .await;

        let adapter = GeminiAdapter::with_base_url("test-key".to_string(), mock_server.uri());

        let request = Request::new("gemini-2.0-flash", vec![Message::user("Test request")])
            .system_prompt("You are helpful.")
            .max_tokens(100);

        let response = adapter.complete(&request).await.unwrap();
        assert_eq!(response.text().as_deref(), Some("ok"));
    }

    #[tokio::test]
    async fn complete_model_not_found() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path_regex(r"/v1beta/models/.*:generateContent"))
            .respond_with(ResponseTemplate::new(404).set_body_string("Model not found"))
            .mount(&mock_server)
            .await;

        let adapter = GeminiAdapter::with_base_url("test-key".to_string(), mock_server.uri());

        let request = Request::new("nonexistent-model", vec![Message::user("Hello")]);

        let err = adapter.complete(&request).await.unwrap_err();
        assert!(matches!(err, Error::ModelNotFound { .. }));
    }

    #[tokio::test]
    async fn complete_bad_request() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path_regex(r"/v1beta/models/.*:generateContent"))
            .respond_with(ResponseTemplate::new(400).set_body_string("Invalid request body"))
            .mount(&mock_server)
            .await;

        let adapter = GeminiAdapter::with_base_url("test-key".to_string(), mock_server.uri());

        let request = Request::new("gemini-2.0-flash", vec![Message::user("Hello")]);

        let err = adapter.complete(&request).await.unwrap_err();
        assert!(matches!(err, Error::InvalidRequest { .. }));
    }
}
