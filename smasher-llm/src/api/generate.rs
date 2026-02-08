// ABOUTME: Implements the tool execution loop for LLM generation with automatic tool calling.
// ABOUTME: Sends requests to the LLM and iteratively executes tool calls until a final response.

use async_trait::async_trait;

use crate::client::Client;
use crate::types::{ContentPart, Error, Message, Request, Response, ToolCallData, Usage};

/// Trait for executing tool calls returned by the LLM.
#[async_trait]
pub trait ToolExecutor: Send + Sync {
    /// Execute a tool call and return the result as a string.
    /// The is_error flag in the returned tuple indicates whether the execution failed.
    async fn execute(&self, name: &str, arguments: &str) -> Result<(String, bool), Error>;
}

/// Options controlling the tool execution loop in [`generate`].
pub struct GenerateOptions {
    /// Maximum number of tool-use roundtrips before stopping.
    pub max_iterations: u32,
    /// Whether to execute tool calls in parallel (all at once) or sequentially.
    pub parallel_tool_calls: bool,
}

impl Default for GenerateOptions {
    fn default() -> Self {
        Self {
            max_iterations: 10,
            parallel_tool_calls: true,
        }
    }
}

/// The result of a [`generate`] call, including all intermediate responses.
pub struct GenerateResult {
    /// The final response from the LLM (after all tool iterations).
    pub response: Response,
    /// All intermediate responses (including tool-call responses).
    pub history: Vec<Response>,
    /// Total accumulated usage across all iterations.
    pub total_usage: Usage,
    /// Number of tool-call iterations performed.
    pub iterations: u32,
}

/// Run the LLM generation loop with optional tool execution.
///
/// Sends the request to the LLM, and if the response contains tool calls,
/// executes them via the provided [`ToolExecutor`] and feeds results back.
/// This repeats until the LLM produces a final response without tool calls,
/// no executor is available, or the max iteration limit is reached.
pub async fn generate(
    client: &Client,
    mut request: Request,
    tool_executor: Option<&dyn ToolExecutor>,
    options: Option<GenerateOptions>,
) -> Result<GenerateResult, Error> {
    let options = options.unwrap_or_default();
    let mut iterations: u32 = 0;
    let mut total_usage = Usage::default();
    let mut history: Vec<Response> = vec![];

    loop {
        tracing::debug!(iteration = iterations, "generate: sending request to LLM");

        let response = client.complete(request.clone()).await?;
        total_usage += response.usage.clone();

        if !response.has_tool_calls()
            || tool_executor.is_none()
            || iterations >= options.max_iterations
        {
            return Ok(GenerateResult {
                response,
                history,
                total_usage,
                iterations,
            });
        }

        let executor = tool_executor.unwrap();

        // Extract tool call data from the response content.
        let tool_calls: Vec<ToolCallData> = response
            .content
            .iter()
            .filter_map(|part| match part {
                ContentPart::ToolCall(data) => Some(data.clone()),
                _ => None,
            })
            .collect();

        // Push the response into history before mutating the request.
        history.push(response.clone());

        // Add the assistant's response (with tool calls) back into the conversation.
        request
            .messages
            .push(Message::assistant_with_tool_calls(tool_calls.clone()));

        // Execute each tool call and collect results.
        let results: Vec<(String, String, bool)> = if options.parallel_tool_calls {
            let futures: Vec<_> = tool_calls
                .iter()
                .map(|tc| {
                    let name = tc.name.clone();
                    let arguments = tc.arguments.clone();
                    let id = tc.id.clone();
                    async move {
                        let (content, is_error) = executor.execute(&name, &arguments).await?;
                        Ok::<(String, String, bool), Error>((id, content, is_error))
                    }
                })
                .collect();
            let joined = futures::future::join_all(futures).await;
            let mut collected = Vec::with_capacity(joined.len());
            for result in joined {
                collected.push(result?);
            }
            collected
        } else {
            let mut collected = Vec::with_capacity(tool_calls.len());
            for tc in &tool_calls {
                let (content, is_error) = executor.execute(&tc.name, &tc.arguments).await?;
                collected.push((tc.id.clone(), content, is_error));
            }
            collected
        };

        // Add each tool result as a message.
        for (tool_call_id, content, is_error) in results {
            request
                .messages
                .push(Message::tool_result(tool_call_id, content, is_error));
        }

        iterations += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::{ProviderAdapter, StreamResponse};
    use crate::types::{ContentPart, FinishReason, Provider, ToolCallData, Usage};
    use std::collections::{HashMap, VecDeque};
    use std::sync::{Arc, Mutex};

    // -- Mock Provider Adapter --

    struct MockAdapter {
        responses: Arc<Mutex<VecDeque<Response>>>,
    }

    #[async_trait]
    impl ProviderAdapter for MockAdapter {
        fn provider_name(&self) -> &str {
            "anthropic"
        }

        async fn complete(&self, _request: &Request) -> Result<Response, Error> {
            let response = self
                .responses
                .lock()
                .unwrap()
                .pop_front()
                .expect("MockAdapter ran out of canned responses");
            Ok(response)
        }

        async fn stream(&self, _request: &Request) -> Result<StreamResponse, Error> {
            unimplemented!("streaming not used in generate tests")
        }
    }

    // -- Mock Tool Executor --

    struct MockToolExecutor {
        results: HashMap<String, (String, bool)>,
    }

    #[async_trait]
    impl ToolExecutor for MockToolExecutor {
        async fn execute(&self, name: &str, _arguments: &str) -> Result<(String, bool), Error> {
            self.results.get(name).cloned().ok_or_else(|| Error::Other {
                message: format!("unknown tool: {}", name),
                retryable: false,
            })
        }
    }

    // -- Test Helpers --

    fn make_client(responses: Vec<Response>) -> Client {
        let mut client = Client::new();
        client.register_provider(
            Provider::Anthropic,
            Arc::new(MockAdapter {
                responses: Arc::new(Mutex::new(VecDeque::from(responses))),
            }),
        );
        client
    }

    fn text_response(text: &str, input_tokens: u32, output_tokens: u32) -> Response {
        Response {
            id: "resp_text".into(),
            model: "claude-sonnet-4-20250514".into(),
            content: vec![ContentPart::text(text)],
            finish_reason: Some(FinishReason::Stop),
            usage: Usage {
                input_tokens,
                output_tokens,
                cache_read_tokens: None,
                cache_creation_tokens: None,
                reasoning_tokens: None,
                total_tokens: None,
                raw: None,
            },
            warnings: vec![],
            rate_limit: None,
            provider: None,
            raw: None,
        }
    }

    fn tool_call_response(
        tool_calls: Vec<ToolCallData>,
        input_tokens: u32,
        output_tokens: u32,
    ) -> Response {
        Response {
            id: "resp_tool".into(),
            model: "claude-sonnet-4-20250514".into(),
            content: tool_calls.into_iter().map(ContentPart::ToolCall).collect(),
            finish_reason: Some(FinishReason::ToolUse),
            usage: Usage {
                input_tokens,
                output_tokens,
                cache_read_tokens: None,
                cache_creation_tokens: None,
                reasoning_tokens: None,
                total_tokens: None,
                raw: None,
            },
            warnings: vec![],
            rate_limit: None,
            provider: None,
            raw: None,
        }
    }

    fn sample_request() -> Request {
        Request::new(
            "claude-sonnet-4-20250514",
            vec![Message::user("What is the weather?")],
        )
    }

    fn search_tool_call() -> ToolCallData {
        ToolCallData {
            id: "call_1".into(),
            name: "search".into(),
            arguments: r#"{"q":"weather"}"#.into(),
            raw_arguments: None,
        }
    }

    fn calc_tool_call() -> ToolCallData {
        ToolCallData {
            id: "call_2".into(),
            name: "calc".into(),
            arguments: r#"{"expr":"2+2"}"#.into(),
            raw_arguments: None,
        }
    }

    // -- Tests --

    #[tokio::test]
    async fn generate_with_no_tools_returns_immediately() {
        let client = make_client(vec![text_response("Hello!", 10, 20)]);
        let request = sample_request();

        let result = generate(&client, request, None, None).await.unwrap();

        assert_eq!(result.response.text(), Some("Hello!".to_string()));
        assert_eq!(result.iterations, 0);
        assert!(result.history.is_empty());
        assert_eq!(result.total_usage.input_tokens, 10);
        assert_eq!(result.total_usage.output_tokens, 20);
    }

    #[tokio::test]
    async fn generate_with_tool_calls_executes_tools_and_continues() {
        let client = make_client(vec![
            tool_call_response(vec![search_tool_call()], 10, 15),
            text_response("The weather is sunny.", 25, 30),
        ]);

        let executor = MockToolExecutor {
            results: HashMap::from([("search".into(), ("sunny, 72F".into(), false))]),
        };

        let result = generate(&client, sample_request(), Some(&executor), None)
            .await
            .unwrap();

        assert_eq!(
            result.response.text(),
            Some("The weather is sunny.".to_string())
        );
        assert_eq!(result.iterations, 1);
        assert_eq!(result.history.len(), 1);
        assert!(result.history[0].has_tool_calls());
        assert_eq!(result.total_usage.input_tokens, 35);
        assert_eq!(result.total_usage.output_tokens, 45);
    }

    #[tokio::test]
    async fn generate_respects_max_iterations_limit() {
        // The LLM keeps returning tool calls, but we cap at 1 iteration.
        let client = make_client(vec![
            tool_call_response(vec![search_tool_call()], 10, 15),
            // After 1 iteration, the LLM returns tool calls again.
            tool_call_response(vec![search_tool_call()], 10, 15),
        ]);

        let executor = MockToolExecutor {
            results: HashMap::from([("search".into(), ("result".into(), false))]),
        };

        let options = GenerateOptions {
            max_iterations: 1,
            parallel_tool_calls: true,
        };

        let result = generate(&client, sample_request(), Some(&executor), Some(options))
            .await
            .unwrap();

        // The second response has tool calls, but we've hit max_iterations so it's returned as-is.
        assert!(result.response.has_tool_calls());
        assert_eq!(result.iterations, 1);
        assert_eq!(result.history.len(), 1);
    }

    #[tokio::test]
    async fn generate_with_no_tool_executor_returns_even_with_tool_calls() {
        let client = make_client(vec![tool_call_response(vec![search_tool_call()], 10, 15)]);

        let result = generate(&client, sample_request(), None, None)
            .await
            .unwrap();

        // No executor means tool calls are not executed; response returned directly.
        assert!(result.response.has_tool_calls());
        assert_eq!(result.iterations, 0);
        assert!(result.history.is_empty());
    }

    #[tokio::test]
    async fn generate_accumulates_usage_across_iterations() {
        let client = make_client(vec![
            tool_call_response(vec![search_tool_call()], 100, 50),
            tool_call_response(vec![calc_tool_call()], 200, 75),
            text_response("done", 300, 100),
        ]);

        let executor = MockToolExecutor {
            results: HashMap::from([
                ("search".into(), ("found".into(), false)),
                ("calc".into(), ("4".into(), false)),
            ]),
        };

        let result = generate(&client, sample_request(), Some(&executor), None)
            .await
            .unwrap();

        assert_eq!(result.iterations, 2);
        assert_eq!(result.total_usage.input_tokens, 600);
        assert_eq!(result.total_usage.output_tokens, 225);
    }

    #[tokio::test]
    async fn generate_parallel_tool_execution_works() {
        let client = make_client(vec![
            tool_call_response(vec![search_tool_call(), calc_tool_call()], 10, 20),
            text_response("all done", 30, 40),
        ]);

        let executor = MockToolExecutor {
            results: HashMap::from([
                ("search".into(), ("found it".into(), false)),
                ("calc".into(), ("4".into(), false)),
            ]),
        };

        let options = GenerateOptions {
            max_iterations: 10,
            parallel_tool_calls: true,
        };

        let result = generate(&client, sample_request(), Some(&executor), Some(options))
            .await
            .unwrap();

        assert_eq!(result.response.text(), Some("all done".to_string()));
        assert_eq!(result.iterations, 1);
        assert_eq!(result.history.len(), 1);
    }

    #[tokio::test]
    async fn generate_sequential_tool_execution_works() {
        let client = make_client(vec![
            tool_call_response(vec![search_tool_call(), calc_tool_call()], 10, 20),
            text_response("all done sequentially", 30, 40),
        ]);

        let executor = MockToolExecutor {
            results: HashMap::from([
                ("search".into(), ("found it".into(), false)),
                ("calc".into(), ("4".into(), false)),
            ]),
        };

        let options = GenerateOptions {
            max_iterations: 10,
            parallel_tool_calls: false,
        };

        let result = generate(&client, sample_request(), Some(&executor), Some(options))
            .await
            .unwrap();

        assert_eq!(
            result.response.text(),
            Some("all done sequentially".to_string())
        );
        assert_eq!(result.iterations, 1);
        assert_eq!(result.history.len(), 1);
    }

    #[tokio::test]
    async fn generate_handles_tool_execution_errors_gracefully() {
        let client = make_client(vec![
            tool_call_response(vec![search_tool_call()], 10, 20),
            text_response("I see the tool errored", 30, 40),
        ]);

        // The tool executor returns an error result (is_error=true), not a Rust error.
        let executor = MockToolExecutor {
            results: HashMap::from([("search".into(), ("API key expired".into(), true))]),
        };

        let result = generate(&client, sample_request(), Some(&executor), None)
            .await
            .unwrap();

        assert_eq!(
            result.response.text(),
            Some("I see the tool errored".to_string())
        );
        assert_eq!(result.iterations, 1);
        // The history should contain the tool-call response.
        assert_eq!(result.history.len(), 1);
    }
}
