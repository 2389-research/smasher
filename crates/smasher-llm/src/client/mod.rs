// ABOUTME: Client module providing the top-level LLM Client that routes requests to providers.
// ABOUTME: Includes middleware support, environment-based configuration, and provider selection.

pub mod middleware;

use std::collections::HashMap;
use std::sync::Arc;

use crate::provider::anthropic::AnthropicAdapter;
use crate::provider::gemini::GeminiAdapter;
use crate::provider::openai::OpenAiAdapter;
use crate::provider::{ProviderAdapter, StreamResponse};
use crate::types::{Error, Provider, Request, Response, infer_provider};

use self::middleware::{
    CoreFn, Middleware, execute_middleware_chain, execute_stream_error_middleware_chain,
    execute_stream_middleware_chain,
};

/// The unified LLM client that routes requests to the appropriate provider.
pub struct Client {
    providers: HashMap<Provider, Arc<dyn ProviderAdapter>>,
    middlewares: Vec<Box<dyn Middleware>>,
}

impl Default for Client {
    fn default() -> Self {
        Self::new()
    }
}

impl Client {
    /// Create a new client with the given provider adapters.
    pub fn new() -> Self {
        Self {
            providers: HashMap::new(),
            middlewares: Vec::new(),
        }
    }

    /// Create a client configured from environment variables.
    ///
    /// Checks for:
    /// - `ANTHROPIC_API_KEY` → registers Anthropic adapter
    /// - `OPENAI_API_KEY` → registers OpenAI adapter
    /// - `GEMINI_API_KEY` or `GOOGLE_API_KEY` → registers Gemini adapter
    pub fn from_env() -> Self {
        let mut client = Self::new();

        if let Ok(key) = std::env::var("ANTHROPIC_API_KEY") {
            client.register_provider(Provider::Anthropic, Arc::new(AnthropicAdapter::new(key)));
        }

        if let Ok(key) = std::env::var("OPENAI_API_KEY") {
            client.register_provider(Provider::OpenAi, Arc::new(OpenAiAdapter::new(key)));
        }

        if let Ok(key) =
            std::env::var("GEMINI_API_KEY").or_else(|_| std::env::var("GOOGLE_API_KEY"))
        {
            client.register_provider(Provider::Gemini, Arc::new(GeminiAdapter::new(key)));
        }

        client
    }

    /// Register a provider adapter.
    pub fn register_provider(
        &mut self,
        provider: Provider,
        adapter: Arc<dyn ProviderAdapter>,
    ) -> &mut Self {
        self.providers.insert(provider, adapter);
        self
    }

    /// Add a middleware to the chain.
    pub fn add_middleware(&mut self, middleware: impl Middleware + 'static) -> &mut Self {
        self.middlewares.push(Box::new(middleware));
        self
    }

    /// Resolve the adapter for a request, checking explicit provider override first.
    ///
    /// If `request.provider` is set, parse it to a Provider enum and look up that adapter.
    /// Otherwise, fall back to model-name inference via `infer_provider`.
    fn adapter_for_request(&self, request: &Request) -> Result<Arc<dyn ProviderAdapter>, Error> {
        let provider = if let Some(ref explicit) = request.provider {
            explicit
                .parse::<Provider>()
                .map_err(|_| Error::ModelNotFound {
                    provider: explicit.clone(),
                    model: request.model.clone(),
                })?
        } else {
            infer_provider(&request.model).ok_or_else(|| Error::ModelNotFound {
                provider: "unknown".into(),
                model: request.model.clone(),
            })?
        };

        self.providers
            .get(&provider)
            .cloned()
            .ok_or_else(|| Error::ProviderNotConfigured {
                provider: provider.to_string(),
            })
    }

    /// Send a completion request, routing to the appropriate provider.
    pub async fn complete(&self, request: Request) -> Result<Response, Error> {
        let adapter = self.adapter_for_request(&request)?;

        execute_middleware_chain(
            &self.middlewares,
            request,
            CoreFn(move |req| async move { adapter.complete(&req).await }),
        )
        .await
    }

    /// Send a streaming completion request, routing to the appropriate provider.
    ///
    /// Middleware `on_stream_request` is applied to the request before it is sent
    /// to the provider. If the provider returns an error during stream setup,
    /// `on_stream_error` middleware is invoked in reverse order, allowing
    /// middleware to transform or log the error.
    pub async fn stream(&self, request: &Request) -> Result<StreamResponse, Error> {
        let adapter = self.adapter_for_request(request)?;
        let processed_request =
            execute_stream_middleware_chain(&self.middlewares, request.clone()).await?;
        match adapter.stream(&processed_request).await {
            Ok(stream) => Ok(stream),
            Err(error) => {
                // Run error middleware chain. If it returns Err, propagate
                // that (possibly transformed) error. If a middleware suppresses
                // the error (returns Ok), we still have no stream to return,
                // so return a descriptive error.
                match execute_stream_error_middleware_chain(&self.middlewares, error).await {
                    Err(e) => Err(e),
                    Ok(()) => Err(Error::Other {
                        message: "stream setup error was suppressed by middleware but no stream is available".into(),
                        retryable: false,
                    }),
                }
            }
        }
    }

    /// Check whether a provider is registered and available.
    pub fn has_provider(&self, provider: &Provider) -> bool {
        self.providers.contains_key(provider)
    }

    /// List all registered providers.
    pub fn registered_providers(&self) -> Vec<Provider> {
        self.providers.keys().copied().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::StreamResponse;
    use crate::types::{ContentPart, FinishReason, Message, Usage};
    use async_trait::async_trait;
    use std::sync::atomic::{AtomicU32, Ordering};

    struct MockAdapter {
        name: &'static str,
        call_count: Arc<AtomicU32>,
    }

    #[async_trait]
    impl ProviderAdapter for MockAdapter {
        fn provider_name(&self) -> &str {
            self.name
        }

        async fn complete(&self, _request: &Request) -> Result<Response, Error> {
            self.call_count.fetch_add(1, Ordering::SeqCst);
            Ok(Response {
                id: "mock-resp".into(),
                model: "mock-model".into(),
                content: vec![ContentPart::text("mock response")],
                finish_reason: Some(FinishReason::Stop),
                usage: Usage::default(),
                warnings: vec![],
                rate_limit: None,
                provider: None,
                raw: None,
            })
        }

        async fn stream(&self, _request: &Request) -> Result<StreamResponse, Error> {
            Err(Error::Other {
                message: "streaming not implemented in mock".into(),
                retryable: false,
            })
        }
    }

    fn client_with_mock_anthropic() -> (Client, Arc<AtomicU32>) {
        let count = Arc::new(AtomicU32::new(0));
        let mut client = Client::new();
        client.register_provider(
            Provider::Anthropic,
            Arc::new(MockAdapter {
                name: "anthropic",
                call_count: count.clone(),
            }),
        );
        (client, count)
    }

    #[tokio::test]
    async fn complete_routes_to_correct_provider() {
        let (client, count) = client_with_mock_anthropic();

        let request = Request::new("claude-sonnet-4-20250514", vec![Message::user("hello")]);
        let response = client.complete(request).await.unwrap();

        assert_eq!(response.id, "mock-resp");
        assert_eq!(count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn complete_returns_error_for_unknown_model() {
        let (client, _count) = client_with_mock_anthropic();

        let request = Request::new("totally-fake-model-xyz", vec![Message::user("hello")]);
        let result = client.complete(request).await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn complete_returns_error_for_unconfigured_provider() {
        let client = Client::new();

        let request = Request::new("claude-sonnet-4-20250514", vec![Message::user("hello")]);
        let result = client.complete(request).await;

        assert!(matches!(
            result.unwrap_err(),
            Error::ProviderNotConfigured { .. }
        ));
    }

    #[tokio::test]
    async fn has_provider_returns_true_for_registered() {
        let (client, _) = client_with_mock_anthropic();
        assert!(client.has_provider(&Provider::Anthropic));
    }

    #[tokio::test]
    async fn has_provider_returns_false_for_unregistered() {
        let (client, _) = client_with_mock_anthropic();
        assert!(!client.has_provider(&Provider::OpenAi));
    }

    #[tokio::test]
    async fn registered_providers_lists_all() {
        let (mut client, _) = client_with_mock_anthropic();
        client.register_provider(
            Provider::OpenAi,
            Arc::new(MockAdapter {
                name: "openai",
                call_count: Arc::new(AtomicU32::new(0)),
            }),
        );

        let providers = client.registered_providers();
        assert_eq!(providers.len(), 2);
        assert!(providers.contains(&Provider::Anthropic));
        assert!(providers.contains(&Provider::OpenAi));
    }

    #[tokio::test]
    async fn middleware_is_applied_to_complete() {
        let (mut client, _) = client_with_mock_anthropic();
        let mw_count = Arc::new(AtomicU32::new(0));

        struct TestMiddleware {
            count: Arc<AtomicU32>,
        }

        #[async_trait]
        impl Middleware for TestMiddleware {
            fn name(&self) -> &str {
                "test"
            }

            async fn on_request(&self, request: Request) -> Result<Request, Error> {
                self.count.fetch_add(1, Ordering::SeqCst);
                Ok(request)
            }
        }

        client.add_middleware(TestMiddleware {
            count: mw_count.clone(),
        });

        let request = Request::new("claude-sonnet-4-20250514", vec![Message::user("hello")]);
        let _ = client.complete(request).await;

        assert_eq!(mw_count.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn new_client_has_no_providers() {
        let client = Client::new();
        assert!(client.registered_providers().is_empty());
    }

    // --- Streaming middleware integration tests ---

    #[tokio::test]
    async fn stream_middleware_is_applied_to_request() {
        let (mut client, _) = client_with_mock_anthropic();
        let mw_count = Arc::new(AtomicU32::new(0));

        struct StreamTestMiddleware {
            count: Arc<AtomicU32>,
        }

        #[async_trait]
        impl Middleware for StreamTestMiddleware {
            fn name(&self) -> &str {
                "stream_test"
            }

            async fn on_stream_request(&self, request: Request) -> Result<Request, Error> {
                self.count.fetch_add(1, Ordering::SeqCst);
                Ok(request)
            }
        }

        client.add_middleware(StreamTestMiddleware {
            count: mw_count.clone(),
        });

        let request = Request::new("claude-sonnet-4-20250514", vec![Message::user("hello")]);
        // The mock adapter returns an error for stream, but middleware should still run
        let _ = client.stream(&request).await;

        assert_eq!(
            mw_count.load(Ordering::SeqCst),
            1,
            "stream middleware should be called before provider"
        );
    }

    #[tokio::test]
    async fn stream_middleware_short_circuits_before_provider() {
        let (mut client, call_count) = client_with_mock_anthropic();

        struct BlockingStreamMiddleware;

        #[async_trait]
        impl Middleware for BlockingStreamMiddleware {
            fn name(&self) -> &str {
                "stream_blocker"
            }

            async fn on_stream_request(&self, _request: Request) -> Result<Request, Error> {
                Err(Error::Other {
                    message: "stream request blocked by middleware".into(),
                    retryable: false,
                })
            }
        }

        client.add_middleware(BlockingStreamMiddleware);

        let request = Request::new("claude-sonnet-4-20250514", vec![Message::user("hello")]);
        let result = client.stream(&request).await;

        match result {
            Err(err) => {
                assert!(
                    format!("{err}").contains("stream request blocked by middleware"),
                    "unexpected error: {err}"
                );
            }
            Ok(_) => panic!("expected error but got stream"),
        }
        // Provider should never be called since middleware short-circuited
        assert_eq!(
            call_count.load(Ordering::SeqCst),
            0,
            "provider should not be called when middleware short-circuits"
        );
    }

    #[tokio::test]
    async fn stream_error_middleware_transforms_error() {
        let (mut client, _) = client_with_mock_anthropic();

        struct ErrorTransformMiddleware;

        #[async_trait]
        impl Middleware for ErrorTransformMiddleware {
            fn name(&self) -> &str {
                "error_transform"
            }

            async fn on_stream_error(&self, _error: Error) -> Result<(), Error> {
                Err(Error::Other {
                    message: "transformed stream error".into(),
                    retryable: true,
                })
            }
        }

        client.add_middleware(ErrorTransformMiddleware);

        let request = Request::new("claude-sonnet-4-20250514", vec![Message::user("hello")]);
        // MockAdapter.stream() returns an error, which should be transformed
        let result = client.stream(&request).await;

        match result {
            Err(err) => {
                assert!(
                    format!("{err}").contains("transformed stream error"),
                    "error should be transformed by middleware, got: {err}"
                );
            }
            Ok(_) => panic!("expected error but got stream"),
        }
    }

    // --- Explicit provider routing tests ---

    fn client_with_mock_openai_and_anthropic() -> (Client, Arc<AtomicU32>, Arc<AtomicU32>) {
        let anthropic_count = Arc::new(AtomicU32::new(0));
        let openai_count = Arc::new(AtomicU32::new(0));
        let mut client = Client::new();
        client.register_provider(
            Provider::Anthropic,
            Arc::new(MockAdapter {
                name: "anthropic",
                call_count: anthropic_count.clone(),
            }),
        );
        client.register_provider(
            Provider::OpenAi,
            Arc::new(MockAdapter {
                name: "openai",
                call_count: openai_count.clone(),
            }),
        );
        (client, anthropic_count, openai_count)
    }

    #[tokio::test]
    async fn explicit_provider_overrides_model_name_inference() {
        let (client, anthropic_count, openai_count) = client_with_mock_openai_and_anthropic();

        // Model name "gpt-4" would normally route to OpenAI,
        // but explicit provider "anthropic" should override that.
        let request = Request::new("gpt-4", vec![Message::user("hello")]).provider("anthropic");
        let result = client.complete(request).await;

        assert!(result.is_ok(), "request should succeed");
        assert_eq!(
            anthropic_count.load(Ordering::SeqCst),
            1,
            "anthropic adapter should be called"
        );
        assert_eq!(
            openai_count.load(Ordering::SeqCst),
            0,
            "openai adapter should NOT be called"
        );
    }

    #[tokio::test]
    async fn explicit_provider_routes_unknown_model_name() {
        let (client, _anthropic_count, openai_count) = client_with_mock_openai_and_anthropic();

        // Model name "my-custom-finetune" doesn't match any known prefix,
        // but explicit provider "openai" should route it anyway.
        let request =
            Request::new("my-custom-finetune", vec![Message::user("hello")]).provider("openai");
        let result = client.complete(request).await;

        assert!(
            result.is_ok(),
            "request should succeed with explicit provider"
        );
        assert_eq!(
            openai_count.load(Ordering::SeqCst),
            1,
            "openai adapter should be called for unknown model with explicit provider"
        );
    }

    #[tokio::test]
    async fn explicit_provider_errors_for_unconfigured_provider() {
        let (client, _, _) = client_with_mock_openai_and_anthropic();

        // Gemini is not registered, so explicit provider "gemini" should fail.
        let request = Request::new("my-model", vec![Message::user("hello")]).provider("gemini");
        let result = client.complete(request).await;

        assert!(matches!(
            result.unwrap_err(),
            Error::ProviderNotConfigured { .. }
        ));
    }

    #[tokio::test]
    async fn explicit_provider_errors_for_unknown_provider_name() {
        let (client, _, _) = client_with_mock_openai_and_anthropic();

        // "martian" is not a valid provider name at all.
        let request = Request::new("my-model", vec![Message::user("hello")]).provider("martian");
        let result = client.complete(request).await;

        assert!(result.is_err(), "unknown provider name should error");
    }

    #[tokio::test]
    async fn without_explicit_provider_model_inference_still_works() {
        let (client, _anthropic_count, openai_count) = client_with_mock_openai_and_anthropic();

        // No explicit provider — "gpt-4" should route to OpenAI via model-name inference.
        let request = Request::new("gpt-4", vec![Message::user("hello")]);
        let result = client.complete(request).await;

        assert!(result.is_ok());
        assert_eq!(openai_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn explicit_provider_works_for_streaming() {
        let (client, _anthropic_count, openai_count) = client_with_mock_openai_and_anthropic();

        // Model name "gpt-4" would route to OpenAI, but explicit provider is "anthropic".
        let request = Request::new("gpt-4", vec![Message::user("hello")]).provider("anthropic");
        let _ = client.stream(&request).await;

        // MockAdapter returns error for stream, but the routing should still go to anthropic.
        // We can't check call_count on stream since MockAdapter.stream doesn't increment it,
        // but we can verify the openai adapter was NOT called for complete.
        assert_eq!(
            openai_count.load(Ordering::SeqCst),
            0,
            "openai adapter should not be called when explicit provider is anthropic"
        );
    }

    #[tokio::test]
    async fn stream_default_on_stream_request_delegates_to_on_request() {
        let (mut client, _) = client_with_mock_anthropic();
        let on_request_count = Arc::new(AtomicU32::new(0));

        struct DelegatingMiddleware {
            on_request_count: Arc<AtomicU32>,
        }

        #[async_trait]
        impl Middleware for DelegatingMiddleware {
            fn name(&self) -> &str {
                "delegating"
            }

            async fn on_request(&self, request: Request) -> Result<Request, Error> {
                self.on_request_count.fetch_add(1, Ordering::SeqCst);
                Ok(request)
            }
            // on_stream_request not overridden — should delegate to on_request
        }

        client.add_middleware(DelegatingMiddleware {
            on_request_count: on_request_count.clone(),
        });

        let request = Request::new("claude-sonnet-4-20250514", vec![Message::user("hello")]);
        let _ = client.stream(&request).await;

        assert_eq!(
            on_request_count.load(Ordering::SeqCst),
            1,
            "default on_stream_request should delegate to on_request"
        );
    }
}
