// ABOUTME: Middleware trait for intercepting and transforming LLM requests and responses.
// ABOUTME: Enables logging, caching, rate limiting, and other cross-cutting concerns.

use async_trait::async_trait;

use crate::types::{Error, Request, Response};

/// Middleware that can inspect and transform requests and responses.
///
/// Middleware is executed in order: each middleware's `on_request` is called
/// before the provider, and `on_response` is called after (in reverse order).
#[async_trait]
pub trait Middleware: Send + Sync {
    /// The name of this middleware (for logging/debugging).
    fn name(&self) -> &str;

    /// Called before the request is sent to the provider.
    /// Can modify the request or short-circuit by returning an error.
    async fn on_request(&self, request: Request) -> Result<Request, Error> {
        Ok(request)
    }

    /// Called after the response is received from the provider.
    /// Can modify the response or convert it to an error.
    async fn on_response(&self, response: Response) -> Result<Response, Error> {
        Ok(response)
    }

    /// Called when an error occurs during the request.
    /// Can transform the error or recover by returning a response.
    async fn on_error(&self, error: Error) -> Result<Response, Error> {
        Err(error)
    }

    /// Called before a streaming request is sent to the provider.
    /// Can modify the request or short-circuit by returning an error.
    async fn on_stream_request(&self, request: Request) -> Result<Request, Error> {
        // Default: delegate to on_request for consistency
        self.on_request(request).await
    }

    /// Called when an error occurs during stream setup.
    /// Can transform the error or suppress it by returning Ok(()).
    async fn on_stream_error(&self, error: Error) -> Result<(), Error> {
        Err(error)
    }
}

/// Execute a chain of middleware around a core operation.
pub async fn execute_middleware_chain(
    middlewares: &[Box<dyn Middleware>],
    mut request: Request,
    core: impl AsyncCoreFn,
) -> Result<Response, Error> {
    // Forward pass: on_request
    for mw in middlewares {
        request = mw.on_request(request).await?;
    }

    // Core operation
    let result = core.execute(request).await;

    // Reverse pass: on_response or on_error
    match result {
        Ok(mut response) => {
            for mw in middlewares.iter().rev() {
                response = mw.on_response(response).await?;
            }
            Ok(response)
        }
        Err(mut error) => {
            for mw in middlewares.iter().rev() {
                match mw.on_error(error).await {
                    Ok(response) => return Ok(response),
                    Err(e) => error = e,
                }
            }
            Err(error)
        }
    }
}

/// Execute a chain of middleware for a streaming request.
///
/// Applies each middleware's `on_stream_request` in order. If any middleware
/// returns an error, the chain short-circuits and the error is returned.
/// Unlike `execute_middleware_chain`, there is no reverse pass since we cannot
/// easily intercept individual stream events.
pub async fn execute_stream_middleware_chain(
    middlewares: &[Box<dyn Middleware>],
    mut request: Request,
) -> Result<Request, Error> {
    for mw in middlewares {
        request = mw.on_stream_request(request).await?;
    }
    Ok(request)
}

/// Execute a chain of middleware for a stream setup error.
///
/// Applies each middleware's `on_stream_error` in reverse order. If any
/// middleware returns Ok(()), the error is considered handled/suppressed.
pub async fn execute_stream_error_middleware_chain(
    middlewares: &[Box<dyn Middleware>],
    mut error: Error,
) -> Result<(), Error> {
    for mw in middlewares.iter().rev() {
        match mw.on_stream_error(error).await {
            Ok(()) => return Ok(()),
            Err(e) => error = e,
        }
    }
    Err(error)
}

/// Trait for the core operation that middleware wraps around.
#[async_trait]
pub trait AsyncCoreFn: Send {
    async fn execute(self, request: Request) -> Result<Response, Error>;
}

/// Blanket implementation for async closures (via wrapper).
pub struct CoreFn<F>(pub F);

#[async_trait]
impl<F, Fut> AsyncCoreFn for CoreFn<F>
where
    F: FnOnce(Request) -> Fut + Send,
    Fut: std::future::Future<Output = Result<Response, Error>> + Send,
{
    async fn execute(self, request: Request) -> Result<Response, Error> {
        (self.0)(request).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{ContentPart, FinishReason, Usage};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};

    struct CountingMiddleware {
        request_count: Arc<AtomicU32>,
        response_count: Arc<AtomicU32>,
    }

    #[async_trait]
    impl Middleware for CountingMiddleware {
        fn name(&self) -> &str {
            "counting"
        }

        async fn on_request(&self, request: Request) -> Result<Request, Error> {
            self.request_count.fetch_add(1, Ordering::SeqCst);
            Ok(request)
        }

        async fn on_response(&self, response: Response) -> Result<Response, Error> {
            self.response_count.fetch_add(1, Ordering::SeqCst);
            Ok(response)
        }
    }

    fn dummy_response() -> Response {
        Response {
            id: "test".into(),
            model: "test-model".into(),
            content: vec![ContentPart::text("hello")],
            finish_reason: Some(FinishReason::Stop),
            usage: Usage::default(),
            warnings: vec![],
            rate_limit: None,
            provider: None,
            raw: None,
        }
    }

    fn dummy_request() -> Request {
        Request::new("test-model", vec![])
    }

    #[tokio::test]
    async fn middleware_on_request_is_called() {
        let count = Arc::new(AtomicU32::new(0));
        let mw = CountingMiddleware {
            request_count: count.clone(),
            response_count: Arc::new(AtomicU32::new(0)),
        };

        let middlewares: Vec<Box<dyn Middleware>> = vec![Box::new(mw)];
        let result = execute_middleware_chain(
            &middlewares,
            dummy_request(),
            CoreFn(|_req| async { Ok(dummy_response()) }),
        )
        .await;

        assert!(result.is_ok());
        assert_eq!(count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn middleware_on_response_is_called() {
        let resp_count = Arc::new(AtomicU32::new(0));
        let mw = CountingMiddleware {
            request_count: Arc::new(AtomicU32::new(0)),
            response_count: resp_count.clone(),
        };

        let middlewares: Vec<Box<dyn Middleware>> = vec![Box::new(mw)];
        let result = execute_middleware_chain(
            &middlewares,
            dummy_request(),
            CoreFn(|_req| async { Ok(dummy_response()) }),
        )
        .await;

        assert!(result.is_ok());
        assert_eq!(resp_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn middleware_chain_executes_in_order() {
        let order = Arc::new(std::sync::Mutex::new(Vec::new()));

        struct OrderMiddleware {
            id: &'static str,
            order: Arc<std::sync::Mutex<Vec<String>>>,
        }

        #[async_trait]
        impl Middleware for OrderMiddleware {
            fn name(&self) -> &str {
                self.id
            }

            async fn on_request(&self, request: Request) -> Result<Request, Error> {
                self.order.lock().unwrap().push(format!("req_{}", self.id));
                Ok(request)
            }

            async fn on_response(&self, response: Response) -> Result<Response, Error> {
                self.order.lock().unwrap().push(format!("resp_{}", self.id));
                Ok(response)
            }
        }

        let middlewares: Vec<Box<dyn Middleware>> = vec![
            Box::new(OrderMiddleware {
                id: "a",
                order: order.clone(),
            }),
            Box::new(OrderMiddleware {
                id: "b",
                order: order.clone(),
            }),
        ];

        let _ = execute_middleware_chain(
            &middlewares,
            dummy_request(),
            CoreFn(|_req| async { Ok(dummy_response()) }),
        )
        .await;

        let recorded = order.lock().unwrap().clone();
        assert_eq!(recorded, vec!["req_a", "req_b", "resp_b", "resp_a"]);
    }

    #[tokio::test]
    async fn middleware_on_error_is_called_on_failure() {
        let error_count = Arc::new(AtomicU32::new(0));

        struct ErrorMiddleware {
            count: Arc<AtomicU32>,
        }

        #[async_trait]
        impl Middleware for ErrorMiddleware {
            fn name(&self) -> &str {
                "error_counter"
            }

            async fn on_error(&self, error: Error) -> Result<Response, Error> {
                self.count.fetch_add(1, Ordering::SeqCst);
                Err(error)
            }
        }

        let middlewares: Vec<Box<dyn Middleware>> = vec![Box::new(ErrorMiddleware {
            count: error_count.clone(),
        })];

        let result = execute_middleware_chain(
            &middlewares,
            dummy_request(),
            CoreFn(|_req| async {
                Err(Error::Other {
                    message: "boom".into(),
                    retryable: false,
                })
            }),
        )
        .await;

        assert!(result.is_err());
        assert_eq!(error_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn middleware_on_error_can_recover() {
        struct RecoveryMiddleware;

        #[async_trait]
        impl Middleware for RecoveryMiddleware {
            fn name(&self) -> &str {
                "recovery"
            }

            async fn on_error(&self, _error: Error) -> Result<Response, Error> {
                Ok(dummy_response())
            }
        }

        let middlewares: Vec<Box<dyn Middleware>> = vec![Box::new(RecoveryMiddleware)];

        let result = execute_middleware_chain(
            &middlewares,
            dummy_request(),
            CoreFn(|_req| async {
                Err(Error::Other {
                    message: "boom".into(),
                    retryable: false,
                })
            }),
        )
        .await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn empty_middleware_chain_passes_through() {
        let middlewares: Vec<Box<dyn Middleware>> = vec![];

        let result = execute_middleware_chain(
            &middlewares,
            dummy_request(),
            CoreFn(|_req| async { Ok(dummy_response()) }),
        )
        .await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap().id, "test");
    }

    #[tokio::test]
    async fn middleware_can_short_circuit_request() {
        struct BlockingMiddleware;

        #[async_trait]
        impl Middleware for BlockingMiddleware {
            fn name(&self) -> &str {
                "blocker"
            }

            async fn on_request(&self, _request: Request) -> Result<Request, Error> {
                Err(Error::Other {
                    message: "blocked".into(),
                    retryable: false,
                })
            }
        }

        let middlewares: Vec<Box<dyn Middleware>> = vec![Box::new(BlockingMiddleware)];

        let result = execute_middleware_chain(
            &middlewares,
            dummy_request(),
            CoreFn(|_req| async { Ok(dummy_response()) }),
        )
        .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn default_middleware_trait_methods_pass_through() {
        struct NoopMiddleware;

        #[async_trait]
        impl Middleware for NoopMiddleware {
            fn name(&self) -> &str {
                "noop"
            }
        }

        let middlewares: Vec<Box<dyn Middleware>> = vec![Box::new(NoopMiddleware)];

        let result = execute_middleware_chain(
            &middlewares,
            dummy_request(),
            CoreFn(|_req| async { Ok(dummy_response()) }),
        )
        .await;

        assert!(result.is_ok());
    }

    // --- Streaming middleware tests ---

    #[tokio::test]
    async fn on_stream_request_is_called_for_streaming() {
        let count = Arc::new(AtomicU32::new(0));

        struct StreamCountMiddleware {
            count: Arc<AtomicU32>,
        }

        #[async_trait]
        impl Middleware for StreamCountMiddleware {
            fn name(&self) -> &str {
                "stream_counter"
            }

            async fn on_stream_request(&self, request: Request) -> Result<Request, Error> {
                self.count.fetch_add(1, Ordering::SeqCst);
                Ok(request)
            }
        }

        let middlewares: Vec<Box<dyn Middleware>> = vec![Box::new(StreamCountMiddleware {
            count: count.clone(),
        })];

        let result = execute_stream_middleware_chain(&middlewares, dummy_request()).await;

        assert!(result.is_ok());
        assert_eq!(count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn default_on_stream_request_delegates_to_on_request() {
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
            // on_stream_request is NOT overridden, so it should delegate to on_request
        }

        let middlewares: Vec<Box<dyn Middleware>> = vec![Box::new(DelegatingMiddleware {
            on_request_count: on_request_count.clone(),
        })];

        let result = execute_stream_middleware_chain(&middlewares, dummy_request()).await;

        assert!(result.is_ok());
        assert_eq!(
            on_request_count.load(Ordering::SeqCst),
            1,
            "default on_stream_request should delegate to on_request"
        );
    }

    #[tokio::test]
    async fn multiple_stream_middlewares_chain_correctly() {
        let order = Arc::new(std::sync::Mutex::new(Vec::new()));

        struct OrderStreamMiddleware {
            id: &'static str,
            order: Arc<std::sync::Mutex<Vec<String>>>,
        }

        #[async_trait]
        impl Middleware for OrderStreamMiddleware {
            fn name(&self) -> &str {
                self.id
            }

            async fn on_stream_request(&self, request: Request) -> Result<Request, Error> {
                self.order
                    .lock()
                    .unwrap()
                    .push(format!("stream_{}", self.id));
                Ok(request)
            }
        }

        let middlewares: Vec<Box<dyn Middleware>> = vec![
            Box::new(OrderStreamMiddleware {
                id: "first",
                order: order.clone(),
            }),
            Box::new(OrderStreamMiddleware {
                id: "second",
                order: order.clone(),
            }),
            Box::new(OrderStreamMiddleware {
                id: "third",
                order: order.clone(),
            }),
        ];

        let result = execute_stream_middleware_chain(&middlewares, dummy_request()).await;

        assert!(result.is_ok());
        let recorded = order.lock().unwrap().clone();
        assert_eq!(
            recorded,
            vec!["stream_first", "stream_second", "stream_third"]
        );
    }

    #[tokio::test]
    async fn stream_middleware_can_short_circuit_request() {
        struct BlockingStreamMiddleware;

        #[async_trait]
        impl Middleware for BlockingStreamMiddleware {
            fn name(&self) -> &str {
                "stream_blocker"
            }

            async fn on_stream_request(&self, _request: Request) -> Result<Request, Error> {
                Err(Error::Other {
                    message: "stream blocked".into(),
                    retryable: false,
                })
            }
        }

        let second_called = Arc::new(AtomicU32::new(0));

        struct SecondMiddleware {
            count: Arc<AtomicU32>,
        }

        #[async_trait]
        impl Middleware for SecondMiddleware {
            fn name(&self) -> &str {
                "second"
            }

            async fn on_stream_request(&self, request: Request) -> Result<Request, Error> {
                self.count.fetch_add(1, Ordering::SeqCst);
                Ok(request)
            }
        }

        let middlewares: Vec<Box<dyn Middleware>> = vec![
            Box::new(BlockingStreamMiddleware),
            Box::new(SecondMiddleware {
                count: second_called.clone(),
            }),
        ];

        let result = execute_stream_middleware_chain(&middlewares, dummy_request()).await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            format!("{err}").contains("stream blocked"),
            "error should contain the short-circuit message"
        );
        assert_eq!(
            second_called.load(Ordering::SeqCst),
            0,
            "second middleware should not be called after short-circuit"
        );
    }

    #[tokio::test]
    async fn on_stream_error_is_called_on_setup_failure() {
        let error_count = Arc::new(AtomicU32::new(0));

        struct StreamErrorMiddleware {
            count: Arc<AtomicU32>,
        }

        #[async_trait]
        impl Middleware for StreamErrorMiddleware {
            fn name(&self) -> &str {
                "stream_error_handler"
            }

            async fn on_stream_error(&self, error: Error) -> Result<(), Error> {
                self.count.fetch_add(1, Ordering::SeqCst);
                Err(error)
            }
        }

        let middlewares: Vec<Box<dyn Middleware>> = vec![Box::new(StreamErrorMiddleware {
            count: error_count.clone(),
        })];

        let error = Error::Other {
            message: "stream setup failed".into(),
            retryable: false,
        };
        let result = execute_stream_error_middleware_chain(&middlewares, error).await;

        assert!(result.is_err());
        assert_eq!(error_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn on_stream_error_can_suppress_error() {
        struct SuppressingMiddleware;

        #[async_trait]
        impl Middleware for SuppressingMiddleware {
            fn name(&self) -> &str {
                "suppressor"
            }

            async fn on_stream_error(&self, _error: Error) -> Result<(), Error> {
                Ok(()) // suppress the error
            }
        }

        let middlewares: Vec<Box<dyn Middleware>> = vec![Box::new(SuppressingMiddleware)];

        let error = Error::Other {
            message: "should be suppressed".into(),
            retryable: false,
        };
        let result = execute_stream_error_middleware_chain(&middlewares, error).await;

        assert!(result.is_ok(), "error should be suppressed by middleware");
    }

    #[tokio::test]
    async fn empty_stream_middleware_chain_passes_through() {
        let middlewares: Vec<Box<dyn Middleware>> = vec![];

        let request = dummy_request();
        let result = execute_stream_middleware_chain(&middlewares, request).await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap().model, "test-model");
    }

    #[tokio::test]
    async fn default_on_stream_error_propagates_error() {
        struct NoopMiddleware;

        #[async_trait]
        impl Middleware for NoopMiddleware {
            fn name(&self) -> &str {
                "noop"
            }
            // on_stream_error not overridden — default should propagate
        }

        let middlewares: Vec<Box<dyn Middleware>> = vec![Box::new(NoopMiddleware)];

        let error = Error::Other {
            message: "original error".into(),
            retryable: false,
        };
        let result = execute_stream_error_middleware_chain(&middlewares, error).await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(format!("{err}").contains("original error"));
    }
}
