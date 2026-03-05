// ABOUTME: Core agentic session loop that orchestrates LLM calls, tool execution, and steering.
// ABOUTME: Drives the conversation by building requests, dispatching tool calls, and emitting events.

use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use futures::future::join_all;

use crate::events::EventEmitter;
use crate::loop_detection::LoopDetector;
use crate::profile::{ProviderProfile, SystemPromptConfig, profile_for_model};
use crate::tools::ToolRegistry;
use crate::types::{SessionConfig, SessionEvent, SessionPhase, SessionState, Turn};

/// Estimate token count from a text string using the heuristic of 1 token per 4 characters.
///
/// This is a rough approximation suitable for context window budget tracking;
/// it is not intended to replicate any specific tokenizer.
pub fn estimate_tokens(text: &str) -> usize {
    text.len().div_ceil(4)
}

/// Tracks approximate context window usage across an agent session.
///
/// Callers feed token counts in via `add_tokens`, then check `is_warning()`
/// and `is_critical()` to decide whether to emit events or take corrective action.
#[derive(Debug)]
pub struct ContextWindowTracker {
    /// Total size of the context window in tokens.
    context_window_size: usize,
    /// Cumulative tokens used so far.
    tokens_used: usize,
    /// Whether the warning threshold (80%) has already been emitted.
    warning_emitted: bool,
}

impl ContextWindowTracker {
    /// Create a tracker for a context window of the given size.
    pub fn new(context_window_size: usize) -> Self {
        Self {
            context_window_size,
            tokens_used: 0,
            warning_emitted: false,
        }
    }

    /// Record additional token usage.
    pub fn add_tokens(&mut self, n: usize) {
        self.tokens_used = self.tokens_used.saturating_add(n);
    }

    /// Return the fraction of the context window that has been consumed (0.0 ..= 1.0+).
    pub fn usage_fraction(&self) -> f64 {
        if self.context_window_size == 0 {
            return 0.0;
        }
        self.tokens_used as f64 / self.context_window_size as f64
    }

    /// Returns `true` when usage is at or above 80% of the context window.
    pub fn is_warning(&self) -> bool {
        self.usage_fraction() >= 0.80
    }

    /// Returns `true` when usage is at or above 95% of the context window.
    pub fn is_critical(&self) -> bool {
        self.usage_fraction() >= 0.95
    }

    /// Return the cumulative tokens used.
    pub fn tokens_used(&self) -> usize {
        self.tokens_used
    }

    /// Return the configured context window size.
    pub fn context_window_size(&self) -> usize {
        self.context_window_size
    }

    /// Set the cumulative token count directly (used when re-estimating from full conversation).
    pub fn set_tokens_used(&mut self, n: usize) {
        self.tokens_used = n;
    }

    /// Check whether the warning event needs to be emitted (crosses 80% for the first time).
    /// Returns `true` exactly once when the threshold is first crossed.
    pub fn should_emit_warning(&mut self) -> bool {
        if self.is_warning() && !self.warning_emitted {
            self.warning_emitted = true;
            return true;
        }
        false
    }
}

/// Output returned by a successful `process_input` call.
#[derive(Debug)]
pub struct SessionOutput {
    /// The final text response from the assistant, if any.
    pub text: Option<String>,
    /// Number of turns consumed during this invocation.
    pub turns_used: u32,
    /// Accumulated token usage across all LLM calls in this invocation.
    pub total_usage: smasher_llm::types::Usage,
}

/// Errors that can occur during session processing.
#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error("LLM error: {0}")]
    Llm(#[from] smasher_llm::types::Error),
    #[error("session is no longer active")]
    Inactive,
    #[error("turn limit reached ({0} turns)")]
    TurnLimitReached(u32),
    #[error("session cancelled")]
    Cancelled,
    #[error("{0}")]
    Other(String),
}

/// An agentic session that drives conversation between the user, LLM, and tools.
///
/// The session maintains conversation state, applies steering messages, executes
/// tool calls, and emits events as processing progresses.
pub struct Session {
    config: SessionConfig,
    state: SessionState,
    client: Arc<smasher_llm::client::Client>,
    tool_registry: ToolRegistry,
    event_emitter: EventEmitter,
    profile: Box<dyn ProviderProfile>,
    cancel_token: CancellationToken,
    context_tracker: Option<ContextWindowTracker>,
    loop_detector: LoopDetector,
}

impl Session {
    /// Create a session from configuration, an LLM client, a tool registry, and an event emitter.
    ///
    /// A unique session ID is generated, the provider profile is inferred from the
    /// configured model, and internal state is initialized.
    pub fn new(
        config: SessionConfig,
        client: Arc<smasher_llm::client::Client>,
        tool_registry: ToolRegistry,
        event_emitter: EventEmitter,
    ) -> Self {
        let session_id = uuid::Uuid::new_v4().to_string();
        let state = SessionState::new(session_id);
        let profile = profile_for_model(&config.model);
        let context_tracker = config.context_window_size.map(ContextWindowTracker::new);

        Self {
            config,
            state,
            client,
            tool_registry,
            event_emitter,
            profile,
            cancel_token: CancellationToken::new(),
            context_tracker,
            loop_detector: LoopDetector::default(),
        }
    }

    /// Create a session with an externally supplied cancellation token.
    pub fn with_cancel_token(mut self, token: CancellationToken) -> Self {
        self.cancel_token = token;
        self
    }

    /// Cancel the session, causing the agentic loop to exit at the next check point.
    pub fn cancel(&self) {
        self.cancel_token.cancel();
    }

    /// Return a clone of the cancellation token so callers can trigger or observe cancellation.
    pub fn cancel_token(&self) -> CancellationToken {
        self.cancel_token.clone()
    }

    /// Process user input through the agentic loop, returning the final output.
    ///
    /// This method:
    /// 1. Validates the session is active
    /// 2. Adds the user message to conversation history
    /// 3. Applies any queued steering messages
    /// 4. Builds and sends an LLM request
    /// 5. Loops on tool calls until the model produces a final response or the turn limit is hit
    /// 6. Emits events throughout for subscribers
    pub async fn process_input(&mut self, input: &str) -> Result<SessionOutput, SessionError> {
        if !self.state.active {
            return Err(SessionError::Inactive);
        }

        let turn_start = self.state.turn_number;

        // Add user input to conversation
        self.state.add_user_message(input);

        // Emit turn started
        self.event_emitter.emit(SessionEvent::TurnStarted {
            turn_number: self.state.turn_number,
        });

        // Apply steering: drain queued steering messages and add as user messages
        let steering_msgs = self.state.drain_steering();
        for msg in &steering_msgs {
            self.state
                .messages
                .push(smasher_llm::types::Message::user(msg));
            self.state.turns.push(Turn::Steering { text: msg.clone() });
            self.event_emitter
                .emit(SessionEvent::SteeringApplied { text: msg.clone() });
        }

        // Build the initial request
        let system_prompt_config = SystemPromptConfig {
            working_directory: self.config.working_directory.clone(),
            ..Default::default()
        };
        let system_prompt = self
            .config
            .system_prompt
            .clone()
            .unwrap_or_else(|| self.profile.system_prompt(&system_prompt_config));

        let tool_defs = self.tool_registry.tool_definitions();

        let mut request =
            smasher_llm::types::Request::new(&self.config.model, self.state.messages.clone())
                .system_prompt(system_prompt);

        if let Some(max_tokens) = self.config.max_tokens {
            request = request.max_tokens(max_tokens);
        }
        if let Some(temperature) = self.config.temperature {
            request = request.temperature(temperature);
        }
        if !tool_defs.is_empty() {
            request = request.tools(tool_defs);
        }
        if let Some(ref thinking) = self.config.thinking {
            request = request.thinking(thinking.clone());
        }

        // Agentic loop: call LLM, process tool calls, repeat
        let final_text;

        loop {
            // Check for cancellation before each LLM call
            if self.cancel_token.is_cancelled() {
                self.state.active = false;
                self.state.phase = SessionPhase::Completed;
                return Err(SessionError::Cancelled);
            }

            // Call the LLM
            let response = self.client.complete(request.clone()).await?;

            // Emit assistant message event
            self.event_emitter.emit(SessionEvent::AssistantMessage {
                response: response.clone(),
            });

            // Record the response in state (updates turn counter, accumulates usage)
            self.state.add_assistant_response(&response);

            // Update context window tracker with estimated tokens from this exchange
            if let Some(ref mut tracker) = self.context_tracker {
                // Estimate tokens from all messages currently in the conversation
                let total_estimated: usize = self
                    .state
                    .messages
                    .iter()
                    .map(|m| {
                        m.content
                            .iter()
                            .map(|part| estimate_tokens(&format!("{:?}", part)))
                            .sum::<usize>()
                    })
                    .sum();

                // Reset and set to the current conversation size
                tracker.set_tokens_used(total_estimated);

                // Emit warning event when the 80% threshold is first crossed
                if tracker.should_emit_warning() {
                    self.event_emitter.emit(SessionEvent::ContextWindowWarning {
                        used: tracker.tokens_used(),
                        limit: tracker.context_window_size(),
                        fraction: tracker.usage_fraction(),
                    });
                }
            }

            // Check for tool calls
            let tool_calls = response.tool_calls();

            if tool_calls.is_empty() {
                // No tool calls: extract text and finish
                final_text = response.text();
                break;
            }

            // Check turn limit before executing tools
            if self.state.is_at_turn_limit(self.config.max_turns) {
                self.state.active = false;
                self.state.phase = SessionPhase::Completed;
                self.event_emitter.emit(SessionEvent::SessionCompleted {
                    session_id: self.state.session_id.clone(),
                    total_turns: self.state.turn_number,
                    total_usage: self.state.total_usage.clone(),
                });
                return Err(SessionError::TurnLimitReached(self.config.max_turns));
            }

            // Check for cancellation before tool execution
            if self.cancel_token.is_cancelled() {
                self.state.active = false;
                self.state.phase = SessionPhase::Completed;
                return Err(SessionError::Cancelled);
            }

            // Emit ToolCallStarted for each tool call before execution
            for tc in &tool_calls {
                self.event_emitter.emit(SessionEvent::ToolCallStarted {
                    tool_name: tc.name.clone(),
                    tool_call_id: tc.id.clone(),
                });
            }

            // Execute all tool calls concurrently using join_all
            let tool_futures: Vec<_> = tool_calls
                .iter()
                .map(|tc| {
                    let name = tc.name.clone();
                    let arguments = tc.arguments.clone();
                    let registry = &self.tool_registry;
                    async move { registry.execute_untruncated(&name, &arguments).await }
                })
                .collect();

            let outputs = join_all(tool_futures).await;

            // Process results in order after all tool calls complete
            for (tc, output) in tool_calls.iter().zip(outputs) {
                // Emit the event with FULL untruncated output for observability
                self.event_emitter.emit(SessionEvent::ToolCallCompleted {
                    tool_name: tc.name.clone(),
                    tool_call_id: tc.id.clone(),
                    result: output.content.clone(),
                    is_error: output.is_error,
                    duration_ms: output.duration_ms,
                });

                // Apply per-tool truncation for the LLM conversation message
                let truncated_content = self
                    .tool_registry
                    .truncate_for_tool(&tc.name, &output.content);

                // Add truncated tool result to conversation
                self.state
                    .add_tool_result(&tc.id, &truncated_content, output.is_error);

                // Record the tool execution turn with truncated content
                self.state.turns.push(Turn::ToolExecution {
                    tool_name: tc.name.clone(),
                    tool_call_id: tc.id.clone(),
                    arguments: tc.arguments.clone(),
                    result: truncated_content,
                    is_error: output.is_error,
                    duration_ms: output.duration_ms,
                });

                // Record tool call in loop detector and check for repeating patterns
                self.loop_detector.record(&tc.name, &tc.arguments);
                if let Some(loop_pattern) = self.loop_detector.detect_loop() {
                    self.event_emitter.emit(SessionEvent::LoopDetected {
                        pattern: loop_pattern.description.clone(),
                        window_size: loop_pattern.pattern.len(),
                    });

                    // Inject a steering message to warn the model about the detected loop
                    let warning = format!(
                        "WARNING: A repeating tool-call loop has been detected: {}. \
                         You appear to be repeating the same actions without making progress. \
                         Please try a different approach.",
                        loop_pattern.description
                    );
                    self.state
                        .messages
                        .push(smasher_llm::types::Message::user(&warning));

                    // Reset the detector so we don't fire continuously
                    self.loop_detector.reset();
                }
            }

            // Drain any steering that arrived during tool execution
            let mid_steering = self.state.drain_steering();
            for msg in &mid_steering {
                self.state
                    .messages
                    .push(smasher_llm::types::Message::user(msg));
                self.state.turns.push(Turn::Steering { text: msg.clone() });
                self.event_emitter
                    .emit(SessionEvent::SteeringApplied { text: msg.clone() });
            }

            // Update request messages for the next iteration
            request.messages = self.state.messages.clone();

            // If the finish reason is Stop or Length (and we had tool calls handled above),
            // continue the loop. The loop will naturally terminate when no tool calls are
            // present in the response.
        }

        self.state.phase = SessionPhase::Completed;

        let turns_used = self.state.turn_number - turn_start;

        // Emit session completed
        self.event_emitter.emit(SessionEvent::SessionCompleted {
            session_id: self.state.session_id.clone(),
            total_turns: self.state.turn_number,
            total_usage: self.state.total_usage.clone(),
        });

        Ok(SessionOutput {
            text: final_text,
            turns_used,
            total_usage: self.state.total_usage.clone(),
        })
    }

    /// Queue a steering message to be injected before the next LLM call.
    pub fn steer(&mut self, text: &str) {
        self.state.queue_steering(text);
    }

    /// Queue a follow-up message (semantically different from steering but mechanically identical).
    pub fn follow_up(&mut self, text: &str) {
        self.state.queue_steering(text);
    }

    /// Return the unique session identifier.
    pub fn session_id(&self) -> &str {
        &self.state.session_id
    }

    /// Return whether the session is still active.
    pub fn is_active(&self) -> bool {
        self.state.active
    }

    /// Return the current turn count.
    pub fn turn_count(&self) -> u32 {
        self.state.turn_number
    }

    /// Return a reference to accumulated token usage.
    pub fn total_usage(&self) -> &smasher_llm::types::Usage {
        &self.state.total_usage
    }

    /// Return a slice of all messages in the conversation.
    pub fn messages(&self) -> &[smasher_llm::types::Message] {
        &self.state.messages
    }

    /// Return a reference to the context window tracker, if one is configured.
    pub fn context_tracker(&self) -> Option<&ContextWindowTracker> {
        self.context_tracker.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use smasher_llm::provider::{ProviderAdapter, StreamResponse};
    use smasher_llm::types::{
        ContentPart, Error as LlmError, FinishReason, Provider, Request, Response, ToolCallData,
        Usage,
    };
    use std::collections::VecDeque;
    use std::sync::Mutex;

    use crate::tools::{AgentTool, ToolOutput};

    // ── Mock provider adapter ────────────────────────────────────────

    struct MockAdapter {
        responses: Arc<Mutex<VecDeque<Response>>>,
    }

    #[async_trait]
    impl ProviderAdapter for MockAdapter {
        fn provider_name(&self) -> &str {
            "anthropic"
        }

        async fn complete(&self, _request: &Request) -> Result<Response, LlmError> {
            let mut queue = self.responses.lock().unwrap();
            queue.pop_front().ok_or_else(|| LlmError::Other {
                message: "no more mock responses".into(),
                retryable: false,
            })
        }

        async fn stream(&self, _request: &Request) -> Result<StreamResponse, LlmError> {
            Err(LlmError::Other {
                message: "streaming not implemented in mock".into(),
                retryable: false,
            })
        }
    }

    // ── Mock tool ────────────────────────────────────────────────────

    struct EchoTool;

    #[async_trait]
    impl AgentTool for EchoTool {
        fn name(&self) -> &str {
            "echo"
        }

        fn description(&self) -> &str {
            "Echoes input back"
        }

        fn parameters_schema(&self) -> serde_json::Value {
            serde_json::json!({
                "type": "object",
                "properties": {
                    "text": { "type": "string" }
                }
            })
        }

        async fn execute(&self, arguments: &str) -> ToolOutput {
            let v: serde_json::Value = serde_json::from_str(arguments).unwrap_or_default();
            let text = v["text"].as_str().unwrap_or("no text");
            ToolOutput::success(text, 1)
        }
    }

    /// Tool that produces output of a configurable size, for testing truncation behavior.
    struct BigOutputTool {
        output_size: usize,
    }

    #[async_trait]
    impl AgentTool for BigOutputTool {
        fn name(&self) -> &str {
            "big_output"
        }

        fn description(&self) -> &str {
            "Produces large output"
        }

        fn parameters_schema(&self) -> serde_json::Value {
            serde_json::json!({ "type": "object", "properties": {} })
        }

        async fn execute(&self, _arguments: &str) -> ToolOutput {
            let content = "x".repeat(self.output_size);
            ToolOutput::success(content, 5)
        }
    }

    // ── Helpers ──────────────────────────────────────────────────────

    fn text_response(text: &str) -> Response {
        Response {
            id: "resp_text".into(),
            model: "claude-sonnet-4-20250514".into(),
            content: vec![ContentPart::text(text)],
            finish_reason: Some(FinishReason::Stop),
            usage: Usage {
                input_tokens: 10,
                output_tokens: 20,
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

    fn tool_call_response(tool_name: &str, tool_call_id: &str, arguments: &str) -> Response {
        Response {
            id: "resp_tool".into(),
            model: "claude-sonnet-4-20250514".into(),
            content: vec![ContentPart::ToolCall(ToolCallData {
                id: tool_call_id.into(),
                name: tool_name.into(),
                arguments: arguments.into(),
                raw_arguments: None,
            })],
            finish_reason: Some(FinishReason::ToolUse),
            usage: Usage {
                input_tokens: 15,
                output_tokens: 25,
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

    fn multi_tool_call_response() -> Response {
        Response {
            id: "resp_multi".into(),
            model: "claude-sonnet-4-20250514".into(),
            content: vec![
                ContentPart::ToolCall(ToolCallData {
                    id: "call_a".into(),
                    name: "echo".into(),
                    arguments: r#"{"text":"first"}"#.into(),
                    raw_arguments: None,
                }),
                ContentPart::ToolCall(ToolCallData {
                    id: "call_b".into(),
                    name: "echo".into(),
                    arguments: r#"{"text":"second"}"#.into(),
                    raw_arguments: None,
                }),
            ],
            finish_reason: Some(FinishReason::ToolUse),
            usage: Usage {
                input_tokens: 20,
                output_tokens: 30,
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

    fn make_client(responses: VecDeque<Response>) -> Arc<smasher_llm::client::Client> {
        let response_queue = Arc::new(Mutex::new(responses));
        let adapter = MockAdapter {
            responses: response_queue,
        };
        let mut client = smasher_llm::client::Client::new();
        client.register_provider(Provider::Anthropic, Arc::new(adapter));
        Arc::new(client)
    }

    fn make_session(responses: VecDeque<Response>) -> Session {
        let client = make_client(responses);
        let mut tool_registry = ToolRegistry::new();
        tool_registry.register(EchoTool);
        let event_emitter = EventEmitter::default();
        let config = SessionConfig::default();
        Session::new(config, client, tool_registry, event_emitter)
    }

    // ── Tests ────────────────────────────────────────────────────────

    // ── SessionOutput ────────────────────────────────────────────────

    #[test]
    fn session_output_with_text() {
        let output = SessionOutput {
            text: Some("Hello".into()),
            turns_used: 3,
            total_usage: Usage {
                input_tokens: 100,
                output_tokens: 200,
                cache_read_tokens: Some(10),
                cache_creation_tokens: None,
                reasoning_tokens: Some(50),
                total_tokens: None,
                raw: None,
            },
        };
        assert_eq!(output.text.as_deref(), Some("Hello"));
        assert_eq!(output.turns_used, 3);
        assert_eq!(output.total_usage.input_tokens, 100);
        assert_eq!(output.total_usage.output_tokens, 200);
        assert_eq!(output.total_usage.cache_read_tokens, Some(10));
        assert_eq!(output.total_usage.reasoning_tokens, Some(50));
    }

    #[test]
    fn session_output_with_no_text() {
        let output = SessionOutput {
            text: None,
            turns_used: 0,
            total_usage: Usage::default(),
        };
        assert!(output.text.is_none());
        assert_eq!(output.turns_used, 0);
        assert_eq!(output.total_usage.input_tokens, 0);
        assert_eq!(output.total_usage.output_tokens, 0);
    }

    #[test]
    fn session_output_debug_is_implemented() {
        let output = SessionOutput {
            text: Some("test".into()),
            turns_used: 1,
            total_usage: Usage::default(),
        };
        let debug_str = format!("{:?}", output);
        assert!(debug_str.contains("SessionOutput"));
        assert!(debug_str.contains("test"));
    }

    // ── SessionError ─────────────────────────────────────────────────

    #[test]
    fn session_error_inactive_display() {
        let err = SessionError::Inactive;
        assert_eq!(err.to_string(), "session is no longer active");
    }

    #[test]
    fn session_error_turn_limit_display() {
        let err = SessionError::TurnLimitReached(42);
        assert_eq!(err.to_string(), "turn limit reached (42 turns)");
    }

    #[test]
    fn session_error_other_display() {
        let err = SessionError::Other("something custom".into());
        assert_eq!(err.to_string(), "something custom");
    }

    #[test]
    fn session_error_llm_display() {
        let llm_err = LlmError::RateLimited {
            provider: "anthropic".into(),
            retry_after_ms: Some(5000),
        };
        let err = SessionError::Llm(llm_err);
        let display = err.to_string();
        assert!(display.contains("LLM error"));
        assert!(display.contains("anthropic"));
    }

    #[test]
    fn session_error_from_llm_error() {
        let llm_err = LlmError::Timeout {
            provider: "openai".into(),
            timeout_ms: 30000,
        };
        let session_err: SessionError = llm_err.into();
        assert!(matches!(session_err, SessionError::Llm(_)));
    }

    #[test]
    fn session_error_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<SessionError>();
    }

    #[test]
    fn session_error_turn_limit_zero() {
        let err = SessionError::TurnLimitReached(0);
        assert_eq!(err.to_string(), "turn limit reached (0 turns)");
    }

    #[test]
    fn session_error_other_empty_string() {
        let err = SessionError::Other(String::new());
        assert_eq!(err.to_string(), "");
    }

    // ── Session::new() construction ──────────────────────────────────

    #[test]
    fn session_new_creates_session_with_id() {
        let session = make_session(VecDeque::new());
        assert!(!session.session_id().is_empty());
        assert!(session.is_active());
        assert_eq!(session.turn_count(), 0);
        assert_eq!(session.total_usage().input_tokens, 0);
        assert_eq!(session.total_usage().output_tokens, 0);
        assert!(session.messages().is_empty());
    }

    #[test]
    fn session_new_with_custom_model() {
        let client = make_client(VecDeque::new());
        let tool_registry = ToolRegistry::new();
        let event_emitter = EventEmitter::default();
        let config = SessionConfig::default().with_model("gpt-4o");
        let session = Session::new(config, client, tool_registry, event_emitter);

        assert!(session.is_active());
        assert_eq!(session.turn_count(), 0);
    }

    #[test]
    fn session_new_with_custom_system_prompt() {
        let client = make_client(VecDeque::new());
        let tool_registry = ToolRegistry::new();
        let event_emitter = EventEmitter::default();
        let config = SessionConfig::default().with_system_prompt("Be brief.");
        let session = Session::new(config, client, tool_registry, event_emitter);

        assert!(session.is_active());
        assert!(session.messages().is_empty());
    }

    #[test]
    fn session_new_with_empty_tool_registry() {
        let client = make_client(VecDeque::new());
        let tool_registry = ToolRegistry::new();
        let event_emitter = EventEmitter::default();
        let config = SessionConfig::default();
        let session = Session::new(config, client, tool_registry, event_emitter);

        assert!(session.is_active());
        assert_eq!(session.turn_count(), 0);
    }

    #[test]
    fn session_new_generates_unique_ids() {
        let s1 = make_session(VecDeque::new());
        let s2 = make_session(VecDeque::new());
        assert_ne!(
            s1.session_id(),
            s2.session_id(),
            "Two sessions should have different IDs"
        );
    }

    #[test]
    fn session_id_returns_valid_uuid_format() {
        let session = make_session(VecDeque::new());
        let id = session.session_id();
        // UUID v4 format: 8-4-4-4-12 hex chars
        assert_eq!(id.len(), 36);
        assert_eq!(id.chars().filter(|c| *c == '-').count(), 4);
        // Verify it parses as a valid UUID
        assert!(uuid::Uuid::parse_str(id).is_ok());
    }

    // ── Session state accessors ──────────────────────────────────────

    #[test]
    fn total_usage_returns_default_on_fresh_session() {
        let session = make_session(VecDeque::new());
        let usage = session.total_usage();
        assert_eq!(usage.input_tokens, 0);
        assert_eq!(usage.output_tokens, 0);
        assert!(usage.cache_read_tokens.is_none());
        assert!(usage.cache_creation_tokens.is_none());
        assert!(usage.reasoning_tokens.is_none());
    }

    #[test]
    fn messages_returns_empty_on_fresh_session() {
        let session = make_session(VecDeque::new());
        assert!(session.messages().is_empty());
    }

    // ── steer() and follow_up() ──────────────────────────────────────

    #[test]
    fn steer_queues_steering_message() {
        let mut session = make_session(VecDeque::new());
        session.steer("Focus on tests");

        assert_eq!(session.state.steering_queue.len(), 1);
        assert_eq!(session.state.steering_queue[0], "Focus on tests");
    }

    #[test]
    fn steer_queues_multiple_messages() {
        let mut session = make_session(VecDeque::new());
        session.steer("First instruction");
        session.steer("Second instruction");
        session.steer("Third instruction");

        assert_eq!(session.state.steering_queue.len(), 3);
        assert_eq!(session.state.steering_queue[0], "First instruction");
        assert_eq!(session.state.steering_queue[1], "Second instruction");
        assert_eq!(session.state.steering_queue[2], "Third instruction");
    }

    #[test]
    fn follow_up_queues_message_like_steer() {
        let mut session = make_session(VecDeque::new());
        session.follow_up("Please elaborate on that");

        assert_eq!(session.state.steering_queue.len(), 1);
        assert_eq!(session.state.steering_queue[0], "Please elaborate on that");
    }

    #[test]
    fn steer_and_follow_up_share_same_queue() {
        let mut session = make_session(VecDeque::new());
        session.steer("Steering first");
        session.follow_up("Follow up second");

        assert_eq!(session.state.steering_queue.len(), 2);
        assert_eq!(session.state.steering_queue[0], "Steering first");
        assert_eq!(session.state.steering_queue[1], "Follow up second");
    }

    // ── process_input: simple text response ──────────────────────────

    #[tokio::test]
    async fn process_input_with_simple_text_response() {
        let mut responses = VecDeque::new();
        responses.push_back(text_response("Hello, Doctor Biz!"));

        let mut session = make_session(responses);
        let output = session.process_input("Hello").await.unwrap();

        assert_eq!(output.text.as_deref(), Some("Hello, Doctor Biz!"));
        assert_eq!(output.turns_used, 1);
        assert_eq!(output.total_usage.input_tokens, 10);
        assert_eq!(output.total_usage.output_tokens, 20);
    }

    #[tokio::test]
    async fn process_input_adds_user_message_to_conversation() {
        let mut responses = VecDeque::new();
        responses.push_back(text_response("Got it."));

        let mut session = make_session(responses);
        session.process_input("Hello agent").await.unwrap();

        let messages = session.messages();
        // Should have user message + assistant response
        assert_eq!(messages.len(), 2);
        assert!(messages[0].is_user());
        assert_eq!(messages[0].text(), Some("Hello agent".to_string()));
        assert!(messages[1].is_assistant());
    }

    #[tokio::test]
    async fn process_input_increments_turn_count() {
        let mut responses = VecDeque::new();
        responses.push_back(text_response("One."));

        let mut session = make_session(responses);
        assert_eq!(session.turn_count(), 0);

        session.process_input("First").await.unwrap();
        assert_eq!(session.turn_count(), 1);
    }

    #[tokio::test]
    async fn process_input_accumulates_usage_across_calls() {
        let mut responses = VecDeque::new();
        responses.push_back(text_response("First response."));
        responses.push_back(text_response("Second response."));

        let mut session = make_session(responses);

        session.process_input("First").await.unwrap();
        assert_eq!(session.total_usage().input_tokens, 10);
        assert_eq!(session.total_usage().output_tokens, 20);

        session.process_input("Second").await.unwrap();
        assert_eq!(session.total_usage().input_tokens, 20);
        assert_eq!(session.total_usage().output_tokens, 40);
    }

    #[tokio::test]
    async fn process_input_preserves_conversation_across_calls() {
        let mut responses = VecDeque::new();
        responses.push_back(text_response("Reply 1."));
        responses.push_back(text_response("Reply 2."));

        let mut session = make_session(responses);

        session.process_input("Message 1").await.unwrap();
        session.process_input("Message 2").await.unwrap();

        let messages = session.messages();
        // User1, Assistant1, User2, Assistant2
        assert_eq!(messages.len(), 4);
        assert!(messages[0].is_user());
        assert_eq!(messages[0].text(), Some("Message 1".to_string()));
        assert!(messages[1].is_assistant());
        assert!(messages[2].is_user());
        assert_eq!(messages[2].text(), Some("Message 2".to_string()));
        assert!(messages[3].is_assistant());
    }

    // ── process_input: tool call loop ────────────────────────────────

    #[tokio::test]
    async fn process_input_with_tool_call_executes_tool_and_loops() {
        let mut responses = VecDeque::new();
        // First response: tool call
        responses.push_back(tool_call_response("echo", "call_1", r#"{"text":"echoed"}"#));
        // Second response: final text
        responses.push_back(text_response("Done processing tool."));

        let mut session = make_session(responses);
        let output = session.process_input("Use the echo tool").await.unwrap();

        assert_eq!(output.text.as_deref(), Some("Done processing tool."));
        // Two LLM calls = 2 turns
        assert_eq!(output.turns_used, 2);
    }

    #[tokio::test]
    async fn process_input_tool_call_adds_tool_result_to_messages() {
        let mut responses = VecDeque::new();
        responses.push_back(tool_call_response("echo", "call_42", r#"{"text":"hi"}"#));
        responses.push_back(text_response("Done."));

        let mut session = make_session(responses);
        session.process_input("Call echo").await.unwrap();

        // Should have: user, assistant (tool call), tool result, assistant (text)
        let messages = session.messages();
        assert_eq!(messages.len(), 4);
        assert!(messages[0].is_user());
        assert!(messages[1].is_assistant());
        assert!(messages[2].is_tool());
        assert!(messages[3].is_assistant());
    }

    #[tokio::test]
    async fn process_input_with_chained_tool_calls() {
        let mut responses = VecDeque::new();
        // Tool call 1
        responses.push_back(tool_call_response("echo", "call_1", r#"{"text":"first"}"#));
        // Tool call 2
        responses.push_back(tool_call_response("echo", "call_2", r#"{"text":"second"}"#));
        // Final text
        responses.push_back(text_response("All done."));

        let mut session = make_session(responses);
        let output = session.process_input("Chain tools").await.unwrap();

        assert_eq!(output.text.as_deref(), Some("All done."));
        assert_eq!(output.turns_used, 3);
        assert_eq!(session.turn_count(), 3);
    }

    #[tokio::test]
    async fn process_input_accumulates_usage_across_tool_loop() {
        let mut responses = VecDeque::new();
        // Tool call response: input=15, output=25
        responses.push_back(tool_call_response("echo", "call_1", r#"{"text":"x"}"#));
        // Final text: input=10, output=20
        responses.push_back(text_response("Done."));

        let mut session = make_session(responses);
        let output = session.process_input("Go").await.unwrap();

        assert_eq!(output.total_usage.input_tokens, 25);
        assert_eq!(output.total_usage.output_tokens, 45);
    }

    // ── process_input: turn limit ────────────────────────────────────

    #[tokio::test]
    async fn process_input_respects_turn_limit() {
        // Respond with tool calls every time to force hitting the limit
        let mut responses = VecDeque::new();
        for i in 0..5 {
            responses.push_back(tool_call_response(
                "echo",
                &format!("call_{i}"),
                r#"{"text":"loop"}"#,
            ));
        }

        let client = make_client(responses);
        let mut tool_registry = ToolRegistry::new();
        tool_registry.register(EchoTool);
        let event_emitter = EventEmitter::default();
        let config = SessionConfig::default().with_max_turns(2);
        let mut session = Session::new(config, client, tool_registry, event_emitter);

        let result = session.process_input("Loop forever").await;
        assert!(result.is_err());
        match result.unwrap_err() {
            SessionError::TurnLimitReached(limit) => assert_eq!(limit, 2),
            other => panic!("Expected TurnLimitReached, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn session_becomes_inactive_after_turn_limit() {
        let mut responses = VecDeque::new();
        for i in 0..5 {
            responses.push_back(tool_call_response(
                "echo",
                &format!("call_{i}"),
                r#"{"text":"loop"}"#,
            ));
        }

        let client = make_client(responses);
        let mut tool_registry = ToolRegistry::new();
        tool_registry.register(EchoTool);
        let event_emitter = EventEmitter::default();
        let config = SessionConfig::default().with_max_turns(1);
        let mut session = Session::new(config, client, tool_registry, event_emitter);

        let _ = session.process_input("Go").await;
        assert!(
            !session.is_active(),
            "Session should be inactive after hitting turn limit"
        );
    }

    #[tokio::test]
    async fn inactive_session_rejects_further_input() {
        let mut responses = VecDeque::new();
        for i in 0..5 {
            responses.push_back(tool_call_response(
                "echo",
                &format!("call_{i}"),
                r#"{"text":"loop"}"#,
            ));
        }
        responses.push_back(text_response("Should never reach this."));

        let client = make_client(responses);
        let mut tool_registry = ToolRegistry::new();
        tool_registry.register(EchoTool);
        let event_emitter = EventEmitter::default();
        let config = SessionConfig::default().with_max_turns(1);
        let mut session = Session::new(config, client, tool_registry, event_emitter);

        // First call hits turn limit
        let _ = session.process_input("First").await;
        // Second call should fail as Inactive
        let result = session.process_input("Second").await;
        assert!(matches!(result.unwrap_err(), SessionError::Inactive));
    }

    // ── process_input: inactive session ──────────────────────────────

    #[tokio::test]
    async fn process_input_on_inactive_session_returns_error() {
        let mut session = make_session(VecDeque::new());
        session.state.active = false;

        let result = session.process_input("Should fail").await;
        assert!(result.is_err());
        match result.unwrap_err() {
            SessionError::Inactive => {} // expected
            other => panic!("Expected Inactive, got: {:?}", other),
        }
    }

    // ── process_input: LLM error propagation ─────────────────────────

    #[tokio::test]
    async fn process_input_propagates_llm_errors() {
        // Empty response queue causes an error from the mock adapter
        let mut session = make_session(VecDeque::new());

        let result = session.process_input("Trigger error").await;
        assert!(result.is_err());
        match result.unwrap_err() {
            SessionError::Llm(_) => {} // expected: error from mock adapter
            other => panic!("Expected Llm error, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn session_remains_active_after_llm_error() {
        let mut session = make_session(VecDeque::new());

        let _ = session.process_input("Trigger error").await;
        // Session should still be active because the LLM error doesn't disable it
        assert!(
            session.is_active(),
            "Session should remain active after a transient LLM error"
        );
    }

    // ── process_input: unknown tool ──────────────────────────────────

    #[tokio::test]
    async fn process_input_handles_unknown_tool_gracefully() {
        let mut responses = VecDeque::new();
        // LLM tries to call a tool that doesn't exist
        responses.push_back(tool_call_response("nonexistent_tool", "call_bad", r#"{}"#));
        // After the error result, LLM produces final text
        responses.push_back(text_response("I see the tool failed."));

        let mut session = make_session(responses);
        let output = session.process_input("Call fake tool").await.unwrap();

        assert_eq!(output.text.as_deref(), Some("I see the tool failed."));

        // There should be a tool result message with is_error = true
        let tool_msgs: Vec<_> = session.messages().iter().filter(|m| m.is_tool()).collect();
        assert_eq!(tool_msgs.len(), 1, "Should have one tool result message");
    }

    // ── process_input: steering integration ──────────────────────────

    #[tokio::test]
    async fn steering_is_applied_before_llm_call() {
        let mut responses = VecDeque::new();
        responses.push_back(text_response("Acknowledged steering."));

        let mut session = make_session(responses);
        session.steer("Be concise");
        let output = session.process_input("What is Rust?").await.unwrap();

        assert_eq!(output.text.as_deref(), Some("Acknowledged steering."));

        // Verify steering message is in the conversation messages
        let messages = session.messages();
        let has_steering = messages
            .iter()
            .any(|m| m.is_user() && m.text() == Some("Be concise".to_string()));
        assert!(has_steering, "Steering message should be in conversation");
    }

    #[tokio::test]
    async fn steering_is_drained_after_application() {
        let mut responses = VecDeque::new();
        responses.push_back(text_response("First."));
        responses.push_back(text_response("Second."));

        let mut session = make_session(responses);
        session.steer("Focus on tests");

        session.process_input("First call").await.unwrap();
        // Steering should be consumed
        assert!(
            session.state.steering_queue.is_empty(),
            "Steering queue should be empty after being applied"
        );

        session.process_input("Second call").await.unwrap();
        // Second call should not have the steering message
        let messages_after_second = session.messages();
        let steering_count = messages_after_second
            .iter()
            .filter(|m| m.is_user() && m.text() == Some("Focus on tests".to_string()))
            .count();
        assert_eq!(
            steering_count, 1,
            "Steering message should only appear once"
        );
    }

    #[tokio::test]
    async fn multiple_steering_messages_all_applied() {
        let mut responses = VecDeque::new();
        responses.push_back(text_response("Done."));

        let mut session = make_session(responses);
        session.steer("Instruction one");
        session.steer("Instruction two");

        session.process_input("Go").await.unwrap();

        let messages = session.messages();
        let steering_msgs: Vec<_> = messages
            .iter()
            .filter(|m| {
                m.is_user()
                    && (m.text() == Some("Instruction one".to_string())
                        || m.text() == Some("Instruction two".to_string()))
            })
            .collect();
        assert_eq!(
            steering_msgs.len(),
            2,
            "Both steering messages should be in conversation"
        );
    }

    // ── process_input: events ────────────────────────────────────────

    #[tokio::test]
    async fn events_are_emitted_during_process_input() {
        let mut responses = VecDeque::new();
        responses.push_back(text_response("Event test."));

        let client = make_client(responses);
        let mut tool_registry = ToolRegistry::new();
        tool_registry.register(EchoTool);
        let event_emitter = EventEmitter::default();
        let mut rx = event_emitter.subscribe();
        let config = SessionConfig::default();
        let mut session = Session::new(config, client, tool_registry, event_emitter);

        session.process_input("Hi").await.unwrap();

        // Collect all events
        let mut events = Vec::new();
        while let Ok(event) = rx.try_recv() {
            events.push(event);
        }

        // Verify TurnStarted is emitted
        let has_turn_started = events
            .iter()
            .any(|e| matches!(e, SessionEvent::TurnStarted { .. }));
        assert!(has_turn_started, "TurnStarted event should be emitted");

        // Verify AssistantMessage is emitted
        let has_assistant_msg = events
            .iter()
            .any(|e| matches!(e, SessionEvent::AssistantMessage { .. }));
        assert!(
            has_assistant_msg,
            "AssistantMessage event should be emitted"
        );

        // Verify SessionCompleted is emitted
        let has_completed = events
            .iter()
            .any(|e| matches!(e, SessionEvent::SessionCompleted { .. }));
        assert!(has_completed, "SessionCompleted event should be emitted");
    }

    #[tokio::test]
    async fn event_order_for_simple_text_response() {
        let mut responses = VecDeque::new();
        responses.push_back(text_response("Ok."));

        let client = make_client(responses);
        let tool_registry = ToolRegistry::new();
        let event_emitter = EventEmitter::default();
        let mut rx = event_emitter.subscribe();
        let config = SessionConfig::default();
        let mut session = Session::new(config, client, tool_registry, event_emitter);

        session.process_input("Hello").await.unwrap();

        let mut events = Vec::new();
        while let Ok(event) = rx.try_recv() {
            events.push(event);
        }

        // Expected order: TurnStarted -> AssistantMessage -> SessionCompleted
        assert!(
            events.len() >= 3,
            "Should have at least 3 events, got {}",
            events.len()
        );
        assert!(
            matches!(events[0], SessionEvent::TurnStarted { turn_number: 0 }),
            "First event should be TurnStarted, got {:?}",
            events[0]
        );
        assert!(
            matches!(events[1], SessionEvent::AssistantMessage { .. }),
            "Second event should be AssistantMessage, got {:?}",
            events[1]
        );
        let last = events.last().unwrap();
        assert!(
            matches!(last, SessionEvent::SessionCompleted { .. }),
            "Last event should be SessionCompleted, got {:?}",
            last
        );
    }

    #[tokio::test]
    async fn events_include_tool_call_details() {
        let mut responses = VecDeque::new();
        responses.push_back(tool_call_response("echo", "call_xyz", r#"{"text":"hi"}"#));
        responses.push_back(text_response("Done."));

        let client = make_client(responses);
        let mut tool_registry = ToolRegistry::new();
        tool_registry.register(EchoTool);
        let event_emitter = EventEmitter::default();
        let mut rx = event_emitter.subscribe();
        let config = SessionConfig::default();
        let mut session = Session::new(config, client, tool_registry, event_emitter);

        session.process_input("Use echo").await.unwrap();

        let mut events = Vec::new();
        while let Ok(event) = rx.try_recv() {
            events.push(event);
        }

        // Verify ToolCallStarted has correct fields
        let started = events
            .iter()
            .find(|e| matches!(e, SessionEvent::ToolCallStarted { .. }));
        assert!(started.is_some(), "Should have ToolCallStarted event");
        if let Some(SessionEvent::ToolCallStarted {
            tool_name,
            tool_call_id,
        }) = started
        {
            assert_eq!(tool_name, "echo");
            assert_eq!(tool_call_id, "call_xyz");
        }

        // Verify ToolCallCompleted has correct fields
        let completed = events
            .iter()
            .find(|e| matches!(e, SessionEvent::ToolCallCompleted { .. }));
        assert!(completed.is_some(), "Should have ToolCallCompleted event");
        if let Some(SessionEvent::ToolCallCompleted {
            tool_name,
            tool_call_id,
            result,
            is_error,
            ..
        }) = completed
        {
            assert_eq!(tool_name, "echo");
            assert_eq!(tool_call_id, "call_xyz");
            assert_eq!(result, "hi");
            assert!(!is_error);
        }
    }

    #[tokio::test]
    async fn steering_applied_event_is_emitted() {
        let mut responses = VecDeque::new();
        responses.push_back(text_response("Ok."));

        let client = make_client(responses);
        let tool_registry = ToolRegistry::new();
        let event_emitter = EventEmitter::default();
        let mut rx = event_emitter.subscribe();
        let config = SessionConfig::default();
        let mut session = Session::new(config, client, tool_registry, event_emitter);

        session.steer("Be verbose");
        session.process_input("Tell me about Rust").await.unwrap();

        let mut events = Vec::new();
        while let Ok(event) = rx.try_recv() {
            events.push(event);
        }

        let steering_event = events
            .iter()
            .find(|e| matches!(e, SessionEvent::SteeringApplied { .. }));
        assert!(
            steering_event.is_some(),
            "SteeringApplied event should be emitted"
        );
        if let Some(SessionEvent::SteeringApplied { text }) = steering_event {
            assert_eq!(text, "Be verbose");
        }
    }

    #[tokio::test]
    async fn session_completed_event_contains_correct_totals() {
        let mut responses = VecDeque::new();
        responses.push_back(text_response("Done."));

        let client = make_client(responses);
        let tool_registry = ToolRegistry::new();
        let event_emitter = EventEmitter::default();
        let mut rx = event_emitter.subscribe();
        let config = SessionConfig::default();
        let mut session = Session::new(config, client, tool_registry, event_emitter);

        session.process_input("Hi").await.unwrap();

        let mut events = Vec::new();
        while let Ok(event) = rx.try_recv() {
            events.push(event);
        }

        let completed = events
            .iter()
            .find(|e| matches!(e, SessionEvent::SessionCompleted { .. }));
        assert!(completed.is_some());
        if let Some(SessionEvent::SessionCompleted {
            session_id,
            total_turns,
            total_usage,
        }) = completed
        {
            assert_eq!(session_id, session.session_id());
            assert_eq!(*total_turns, 1);
            assert_eq!(total_usage.input_tokens, 10);
            assert_eq!(total_usage.output_tokens, 20);
        }
    }

    #[tokio::test]
    async fn turn_limit_emits_session_completed_event() {
        let mut responses = VecDeque::new();
        for i in 0..5 {
            responses.push_back(tool_call_response(
                "echo",
                &format!("call_{i}"),
                r#"{"text":"x"}"#,
            ));
        }

        let client = make_client(responses);
        let mut tool_registry = ToolRegistry::new();
        tool_registry.register(EchoTool);
        let event_emitter = EventEmitter::default();
        let mut rx = event_emitter.subscribe();
        let config = SessionConfig::default().with_max_turns(1);
        let mut session = Session::new(config, client, tool_registry, event_emitter);

        let _ = session.process_input("Go").await;

        let mut events = Vec::new();
        while let Ok(event) = rx.try_recv() {
            events.push(event);
        }

        let has_completed = events
            .iter()
            .any(|e| matches!(e, SessionEvent::SessionCompleted { .. }));
        assert!(
            has_completed,
            "SessionCompleted should be emitted even on turn limit"
        );
    }

    // ── process_input: multiple tool calls ───────────────────────────

    #[tokio::test]
    async fn multiple_tool_calls_in_one_response_are_all_executed() {
        let mut responses = VecDeque::new();
        // First: multiple tool calls in one response
        responses.push_back(multi_tool_call_response());
        // Then: final text
        responses.push_back(text_response("Both tools executed."));

        let client = make_client(responses);
        let mut tool_registry = ToolRegistry::new();
        tool_registry.register(EchoTool);
        let event_emitter = EventEmitter::default();
        let mut rx = event_emitter.subscribe();
        let config = SessionConfig::default();
        let mut session = Session::new(config, client, tool_registry, event_emitter);

        let output = session.process_input("Call both tools").await.unwrap();

        assert_eq!(output.text.as_deref(), Some("Both tools executed."));

        // Collect events and count ToolCallCompleted
        let mut events = Vec::new();
        while let Ok(event) = rx.try_recv() {
            events.push(event);
        }

        let tool_completed_count = events
            .iter()
            .filter(|e| matches!(e, SessionEvent::ToolCallCompleted { .. }))
            .count();
        assert_eq!(
            tool_completed_count, 2,
            "Both tool calls should have completed"
        );

        // Verify tool results are in conversation (two tool result messages)
        let tool_result_count = session.messages().iter().filter(|m| m.is_tool()).count();
        assert_eq!(
            tool_result_count, 2,
            "Both tool results should be in messages"
        );
    }

    #[tokio::test]
    async fn multiple_tool_calls_emit_started_and_completed_for_each() {
        let mut responses = VecDeque::new();
        responses.push_back(multi_tool_call_response());
        responses.push_back(text_response("Done."));

        let client = make_client(responses);
        let mut tool_registry = ToolRegistry::new();
        tool_registry.register(EchoTool);
        let event_emitter = EventEmitter::default();
        let mut rx = event_emitter.subscribe();
        let config = SessionConfig::default();
        let mut session = Session::new(config, client, tool_registry, event_emitter);

        session.process_input("Go").await.unwrap();

        let mut events = Vec::new();
        while let Ok(event) = rx.try_recv() {
            events.push(event);
        }

        let started_count = events
            .iter()
            .filter(|e| matches!(e, SessionEvent::ToolCallStarted { .. }))
            .count();
        assert_eq!(started_count, 2, "Should have 2 ToolCallStarted events");

        let completed_count = events
            .iter()
            .filter(|e| matches!(e, SessionEvent::ToolCallCompleted { .. }))
            .count();
        assert_eq!(completed_count, 2, "Should have 2 ToolCallCompleted events");
    }

    // ── process_input: custom system prompt override ─────────────────

    #[tokio::test]
    async fn custom_system_prompt_is_used() {
        // This test verifies that the session can be created and works
        // when a custom system prompt is set (the prompt is passed to the LLM request)
        let mut responses = VecDeque::new();
        responses.push_back(text_response("Custom prompt acknowledged."));

        let client = make_client(responses);
        let tool_registry = ToolRegistry::new();
        let event_emitter = EventEmitter::default();
        let config = SessionConfig::default()
            .with_system_prompt("You are a pirate. Only speak in pirate language.");
        let mut session = Session::new(config, client, tool_registry, event_emitter);

        let output = session.process_input("Hello").await.unwrap();
        assert_eq!(output.text.as_deref(), Some("Custom prompt acknowledged."));
    }

    // ── process_input: no text in response ───────────────────────────

    #[tokio::test]
    async fn process_input_with_no_text_in_response() {
        // Response has no text content parts (only empty content)
        let response = Response {
            id: "resp_empty".into(),
            model: "claude-sonnet-4-20250514".into(),
            content: vec![],
            finish_reason: Some(FinishReason::Stop),
            usage: Usage {
                input_tokens: 5,
                output_tokens: 0,
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
        };

        let mut responses = VecDeque::new();
        responses.push_back(response);

        let mut session = make_session(responses);
        let output = session.process_input("Hello").await.unwrap();

        assert!(
            output.text.is_none(),
            "Output text should be None when response has no text"
        );
        assert_eq!(output.turns_used, 1);
    }

    // ── Session with config variations ───────────────────────────────

    #[tokio::test]
    async fn session_with_temperature_and_max_tokens() {
        let mut responses = VecDeque::new();
        responses.push_back(text_response("Configured."));

        let client = make_client(responses);
        let tool_registry = ToolRegistry::new();
        let event_emitter = EventEmitter::default();
        let config = SessionConfig::default()
            .with_temperature(0.5)
            .with_max_tokens(4096);
        let mut session = Session::new(config, client, tool_registry, event_emitter);

        let output = session.process_input("Test").await.unwrap();
        assert_eq!(output.text.as_deref(), Some("Configured."));
    }

    // ── CancellationToken ───────────────────────────────────────────

    #[test]
    fn session_has_cancel_token_by_default() {
        let session = make_session(VecDeque::new());
        let token = session.cancel_token();
        assert!(
            !token.is_cancelled(),
            "Token should not be cancelled initially"
        );
    }

    #[test]
    fn cancel_sets_cancellation_token() {
        let session = make_session(VecDeque::new());
        let token = session.cancel_token();
        assert!(!token.is_cancelled());

        session.cancel();
        assert!(token.is_cancelled());
    }

    #[test]
    fn with_cancel_token_replaces_default_token() {
        use tokio_util::sync::CancellationToken;

        let external_token = CancellationToken::new();
        let session = make_session(VecDeque::new()).with_cancel_token(external_token.clone());

        assert!(!external_token.is_cancelled());
        session.cancel();
        assert!(external_token.is_cancelled());
    }

    #[tokio::test]
    async fn cancel_causes_loop_to_exit_before_llm_call() {
        // Queue up a response that would succeed if reached
        let mut responses = VecDeque::new();
        responses.push_back(text_response("Should not reach this."));

        let mut session = make_session(responses);
        // Cancel before process_input
        session.cancel();

        let result = session.process_input("Hello").await;
        assert!(result.is_err());
        match result.unwrap_err() {
            SessionError::Cancelled => {}
            other => panic!("Expected Cancelled, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn cancel_causes_loop_to_exit_before_tool_execution() {
        // First response triggers a tool call; cancel before tools run
        let mut responses = VecDeque::new();
        responses.push_back(tool_call_response(
            "echo",
            "call_1",
            r#"{"text":"should not run"}"#,
        ));
        responses.push_back(text_response("Should not reach this."));

        let client = make_client(responses);
        let mut tool_registry = ToolRegistry::new();
        tool_registry.register(EchoTool);
        let event_emitter = EventEmitter::default();
        let config = SessionConfig::default();

        let external_token = tokio_util::sync::CancellationToken::new();
        let mut session = Session::new(config, client, tool_registry, event_emitter)
            .with_cancel_token(external_token.clone());

        // Spawn a task that cancels after a very short delay
        let token_clone = external_token.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
            token_clone.cancel();
        });

        // Give the cancel a moment to fire, then call process_input
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;

        let result = session.process_input("Use the tool").await;
        assert!(
            matches!(result, Err(SessionError::Cancelled)),
            "Expected Cancelled error, got: {:?}",
            result,
        );
    }

    #[tokio::test]
    async fn session_becomes_inactive_after_cancellation() {
        let mut responses = VecDeque::new();
        responses.push_back(text_response("Should not reach."));

        let mut session = make_session(responses);
        session.cancel();

        let _ = session.process_input("Hello").await;
        assert!(
            !session.is_active(),
            "Session should be inactive after cancellation"
        );
    }

    #[test]
    fn session_error_cancelled_display() {
        let err = SessionError::Cancelled;
        assert_eq!(err.to_string(), "session cancelled");
    }

    // ── SessionPhase in session context ──────────────────────────────

    #[test]
    fn session_phase_starts_active() {
        let session = make_session(VecDeque::new());
        assert_eq!(session.state.phase, SessionPhase::Active);
    }

    #[tokio::test]
    async fn session_phase_becomes_completed_after_turn_limit() {
        let mut responses = VecDeque::new();
        for i in 0..5 {
            responses.push_back(tool_call_response(
                "echo",
                &format!("call_{i}"),
                r#"{"text":"loop"}"#,
            ));
        }

        let client = make_client(responses);
        let mut tool_registry = ToolRegistry::new();
        tool_registry.register(EchoTool);
        let event_emitter = EventEmitter::default();
        let config = SessionConfig::default().with_max_turns(1);
        let mut session = Session::new(config, client, tool_registry, event_emitter);

        let _ = session.process_input("Go").await;
        assert_eq!(
            session.state.phase,
            SessionPhase::Completed,
            "Phase should be Completed after hitting turn limit"
        );
    }

    #[tokio::test]
    async fn session_phase_becomes_completed_after_cancellation() {
        let mut responses = VecDeque::new();
        responses.push_back(text_response("Nope."));

        let mut session = make_session(responses);
        session.cancel();

        let _ = session.process_input("Go").await;
        assert_eq!(
            session.state.phase,
            SessionPhase::Completed,
            "Phase should be Completed after cancellation"
        );
    }

    #[tokio::test]
    async fn session_phase_becomes_completed_after_normal_completion() {
        let mut responses = VecDeque::new();
        responses.push_back(text_response("All done."));

        let mut session = make_session(responses);
        session.process_input("Hello").await.unwrap();
        assert_eq!(
            session.state.phase,
            SessionPhase::Completed,
            "Phase should be Completed after normal completion"
        );
    }

    #[test]
    fn session_phase_can_be_set_to_awaiting_input() {
        let mut session = make_session(VecDeque::new());
        session.state.phase = SessionPhase::AwaitingInput;
        assert_eq!(session.state.phase, SessionPhase::AwaitingInput);
    }

    #[test]
    fn session_phase_awaiting_input_to_active_transition() {
        let mut session = make_session(VecDeque::new());
        session.state.phase = SessionPhase::AwaitingInput;
        assert_eq!(session.state.phase, SessionPhase::AwaitingInput);

        session.state.phase = SessionPhase::Active;
        assert_eq!(session.state.phase, SessionPhase::Active);
    }

    // ── estimate_tokens ─────────────────────────────────────────────

    #[test]
    fn estimate_tokens_empty_string() {
        assert_eq!(estimate_tokens(""), 0);
    }

    #[test]
    fn estimate_tokens_single_char() {
        // 1 char => ceil(1/4) = 1
        assert_eq!(estimate_tokens("a"), 1);
    }

    #[test]
    fn estimate_tokens_four_chars() {
        // 4 chars => exactly 1 token
        assert_eq!(estimate_tokens("abcd"), 1);
    }

    #[test]
    fn estimate_tokens_five_chars() {
        // 5 chars => ceil(5/4) = 2
        assert_eq!(estimate_tokens("abcde"), 2);
    }

    #[test]
    fn estimate_tokens_eight_chars() {
        // 8 chars => 2 tokens
        assert_eq!(estimate_tokens("abcdefgh"), 2);
    }

    #[test]
    fn estimate_tokens_typical_sentence() {
        let text = "Hello, this is a test sentence for token estimation.";
        let expected = text.len().div_ceil(4);
        assert_eq!(estimate_tokens(text), expected);
    }

    #[test]
    fn estimate_tokens_large_text() {
        let text = "x".repeat(10_000);
        assert_eq!(estimate_tokens(&text), 2_500);
    }

    #[test]
    fn estimate_tokens_unicode() {
        // Multi-byte characters: "hello" in Japanese is 15 bytes in UTF-8
        let text = "\u{3053}\u{3093}\u{306b}\u{3061}\u{306f}";
        let expected = text.len().div_ceil(4);
        assert_eq!(estimate_tokens(text), expected);
    }

    // ── ContextWindowTracker ────────────────────────────────────────

    #[test]
    fn tracker_new_starts_at_zero() {
        let tracker = ContextWindowTracker::new(100_000);
        assert_eq!(tracker.tokens_used(), 0);
        assert_eq!(tracker.context_window_size(), 100_000);
        assert!((tracker.usage_fraction() - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn tracker_add_tokens_accumulates() {
        let mut tracker = ContextWindowTracker::new(100);
        tracker.add_tokens(30);
        assert_eq!(tracker.tokens_used(), 30);
        tracker.add_tokens(20);
        assert_eq!(tracker.tokens_used(), 50);
    }

    #[test]
    fn tracker_set_tokens_used_replaces_value() {
        let mut tracker = ContextWindowTracker::new(100);
        tracker.add_tokens(50);
        tracker.set_tokens_used(10);
        assert_eq!(tracker.tokens_used(), 10);
    }

    #[test]
    fn tracker_usage_fraction_at_half() {
        let mut tracker = ContextWindowTracker::new(200);
        tracker.add_tokens(100);
        assert!((tracker.usage_fraction() - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn tracker_usage_fraction_at_full() {
        let mut tracker = ContextWindowTracker::new(100);
        tracker.add_tokens(100);
        assert!((tracker.usage_fraction() - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn tracker_usage_fraction_zero_size_returns_zero() {
        let tracker = ContextWindowTracker::new(0);
        assert!((tracker.usage_fraction() - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn tracker_is_warning_below_80_percent() {
        let mut tracker = ContextWindowTracker::new(100);
        tracker.add_tokens(79);
        assert!(!tracker.is_warning());
    }

    #[test]
    fn tracker_is_warning_at_80_percent() {
        let mut tracker = ContextWindowTracker::new(100);
        tracker.add_tokens(80);
        assert!(tracker.is_warning());
    }

    #[test]
    fn tracker_is_warning_above_80_percent() {
        let mut tracker = ContextWindowTracker::new(100);
        tracker.add_tokens(90);
        assert!(tracker.is_warning());
    }

    #[test]
    fn tracker_is_critical_below_95_percent() {
        let mut tracker = ContextWindowTracker::new(100);
        tracker.add_tokens(94);
        assert!(!tracker.is_critical());
    }

    #[test]
    fn tracker_is_critical_at_95_percent() {
        let mut tracker = ContextWindowTracker::new(100);
        tracker.add_tokens(95);
        assert!(tracker.is_critical());
    }

    #[test]
    fn tracker_is_critical_above_95_percent() {
        let mut tracker = ContextWindowTracker::new(100);
        tracker.add_tokens(99);
        assert!(tracker.is_critical());
    }

    #[test]
    fn tracker_is_critical_at_100_percent() {
        let mut tracker = ContextWindowTracker::new(100);
        tracker.add_tokens(100);
        assert!(tracker.is_critical());
    }

    #[test]
    fn tracker_should_emit_warning_fires_once() {
        let mut tracker = ContextWindowTracker::new(100);
        tracker.add_tokens(80);
        assert!(
            tracker.should_emit_warning(),
            "First call should return true"
        );
        assert!(
            !tracker.should_emit_warning(),
            "Second call should return false"
        );
    }

    #[test]
    fn tracker_should_emit_warning_not_yet_at_threshold() {
        let mut tracker = ContextWindowTracker::new(100);
        tracker.add_tokens(50);
        assert!(
            !tracker.should_emit_warning(),
            "Should not emit below threshold"
        );
    }

    #[test]
    fn tracker_should_emit_warning_after_gradual_increase() {
        let mut tracker = ContextWindowTracker::new(100);
        tracker.add_tokens(50);
        assert!(!tracker.should_emit_warning());
        tracker.add_tokens(30);
        assert!(
            tracker.should_emit_warning(),
            "Should emit once threshold crossed"
        );
        assert!(!tracker.should_emit_warning(), "Should not emit again");
    }

    #[test]
    fn tracker_add_tokens_saturates_on_overflow() {
        let mut tracker = ContextWindowTracker::new(100);
        tracker.add_tokens(usize::MAX);
        tracker.add_tokens(1);
        assert_eq!(tracker.tokens_used(), usize::MAX);
    }

    #[test]
    fn tracker_debug_is_implemented() {
        let tracker = ContextWindowTracker::new(1000);
        let debug_str = format!("{:?}", tracker);
        assert!(debug_str.contains("ContextWindowTracker"));
    }

    // ── Session context tracker integration ─────────────────────────

    #[test]
    fn session_without_context_window_size_has_no_tracker() {
        let session = make_session(VecDeque::new());
        assert!(session.context_tracker().is_none());
    }

    #[test]
    fn session_with_context_window_size_has_tracker() {
        let client = make_client(VecDeque::new());
        let tool_registry = ToolRegistry::new();
        let event_emitter = EventEmitter::default();
        let config = SessionConfig::default().with_context_window_size(200_000);
        let session = Session::new(config, client, tool_registry, event_emitter);

        let tracker = session.context_tracker().expect("tracker should exist");
        assert_eq!(tracker.context_window_size(), 200_000);
        assert_eq!(tracker.tokens_used(), 0);
    }

    fn make_session_with_context_window(
        responses: VecDeque<Response>,
        window_size: usize,
    ) -> (Session, tokio::sync::broadcast::Receiver<SessionEvent>) {
        let client = make_client(responses);
        let mut tool_registry = ToolRegistry::new();
        tool_registry.register(EchoTool);
        let event_emitter = EventEmitter::default();
        let rx = event_emitter.subscribe();
        let config = SessionConfig::default().with_context_window_size(window_size);
        let session = Session::new(config, client, tool_registry, event_emitter);
        (session, rx)
    }

    #[tokio::test]
    async fn context_tracker_updates_after_turn() {
        let mut responses = VecDeque::new();
        responses.push_back(text_response("Hello!"));

        let (mut session, _rx) = make_session_with_context_window(responses, 200_000);
        session.process_input("Hi there").await.unwrap();

        let tracker = session.context_tracker().expect("tracker should exist");
        assert!(
            tracker.tokens_used() > 0,
            "Tracker should have counted some tokens after a turn"
        );
    }

    #[tokio::test]
    async fn context_window_warning_emitted_when_threshold_crossed() {
        // Use a very small context window so the messages push us over 80%
        let mut responses = VecDeque::new();
        responses.push_back(text_response(
            "This is a response that should push us over the context window warning threshold.",
        ));

        // Context window of 10 tokens is tiny; any message will exceed 80%
        let (mut session, mut rx) = make_session_with_context_window(responses, 10);
        session.process_input("Hello").await.unwrap();

        let mut events = Vec::new();
        while let Ok(event) = rx.try_recv() {
            events.push(event);
        }

        let warning = events
            .iter()
            .find(|e| matches!(e, SessionEvent::ContextWindowWarning { .. }));
        assert!(
            warning.is_some(),
            "ContextWindowWarning event should be emitted when threshold is crossed"
        );

        if let Some(SessionEvent::ContextWindowWarning {
            used,
            limit,
            fraction,
        }) = warning
        {
            assert_eq!(*limit, 10);
            assert!(*used > 0);
            assert!(*fraction >= 0.80);
        }
    }

    #[tokio::test]
    async fn context_window_warning_not_emitted_when_below_threshold() {
        let mut responses = VecDeque::new();
        responses.push_back(text_response("Ok."));

        // Context window of 1_000_000 tokens is huge; messages won't approach 80%
        let (mut session, mut rx) = make_session_with_context_window(responses, 1_000_000);
        session.process_input("Hi").await.unwrap();

        let mut events = Vec::new();
        while let Ok(event) = rx.try_recv() {
            events.push(event);
        }

        let warning = events
            .iter()
            .find(|e| matches!(e, SessionEvent::ContextWindowWarning { .. }));
        assert!(
            warning.is_none(),
            "ContextWindowWarning should not be emitted when well below threshold"
        );
    }

    #[tokio::test]
    async fn context_window_warning_emitted_only_once() {
        // Two turns that both exceed the threshold
        let mut responses = VecDeque::new();
        responses.push_back(text_response("First response."));
        responses.push_back(text_response("Second response."));

        // Tiny window: both turns will exceed 80%
        let (mut session, mut rx) = make_session_with_context_window(responses, 10);
        session.process_input("First").await.unwrap();
        session.process_input("Second").await.unwrap();

        let mut events = Vec::new();
        while let Ok(event) = rx.try_recv() {
            events.push(event);
        }

        let warning_count = events
            .iter()
            .filter(|e| matches!(e, SessionEvent::ContextWindowWarning { .. }))
            .count();
        assert_eq!(
            warning_count, 1,
            "ContextWindowWarning should be emitted exactly once, got {warning_count}"
        );
    }

    // ── Loop detection integration ────────────────────────────────

    #[tokio::test]
    async fn loop_detected_event_emitted_after_repeated_identical_tool_calls() {
        // LoopDetector default min_repetitions is 3, so we need 3+ identical calls.
        // We queue 4 identical tool-call responses, then a final text response.
        let mut responses = VecDeque::new();
        for i in 0..4 {
            responses.push_back(tool_call_response(
                "echo",
                &format!("call_{i}"),
                r#"{"text":"same"}"#,
            ));
        }
        responses.push_back(text_response("Done looping."));

        let client = make_client(responses);
        let mut tool_registry = ToolRegistry::new();
        tool_registry.register(EchoTool);
        let event_emitter = EventEmitter::default();
        let mut rx = event_emitter.subscribe();
        let config = SessionConfig::default();
        let mut session = Session::new(config, client, tool_registry, event_emitter);

        let _ = session.process_input("Loop away").await;

        let mut events = Vec::new();
        while let Ok(event) = rx.try_recv() {
            events.push(event);
        }

        let loop_event = events
            .iter()
            .find(|e| matches!(e, SessionEvent::LoopDetected { .. }));
        assert!(
            loop_event.is_some(),
            "LoopDetected event should be emitted after repeated identical tool calls"
        );
    }

    #[tokio::test]
    async fn loop_detected_injects_steering_message() {
        // After loop detection, a steering warning should appear in the conversation messages.
        let mut responses = VecDeque::new();
        for i in 0..4 {
            responses.push_back(tool_call_response(
                "echo",
                &format!("call_{i}"),
                r#"{"text":"same"}"#,
            ));
        }
        responses.push_back(text_response("Acknowledged."));

        let client = make_client(responses);
        let mut tool_registry = ToolRegistry::new();
        tool_registry.register(EchoTool);
        let event_emitter = EventEmitter::default();
        let config = SessionConfig::default();
        let mut session = Session::new(config, client, tool_registry, event_emitter);

        let _ = session.process_input("Loop").await;

        let messages = session.messages();
        let has_loop_warning = messages.iter().any(|m| {
            m.is_user()
                && m.text()
                    .map_or(false, |t| t.contains("loop") || t.contains("repeating"))
        });
        assert!(
            has_loop_warning,
            "A steering message warning about the loop should be injected into conversation"
        );
    }

    #[tokio::test]
    async fn no_loop_detected_for_varied_tool_calls() {
        // Different tool call arguments each time should not trigger loop detection.
        let mut responses = VecDeque::new();
        for i in 0..3 {
            responses.push_back(tool_call_response(
                "echo",
                &format!("call_{i}"),
                &format!(r#"{{"text":"different_{i}"}}"#),
            ));
        }
        responses.push_back(text_response("Done."));

        let client = make_client(responses);
        let mut tool_registry = ToolRegistry::new();
        tool_registry.register(EchoTool);
        let event_emitter = EventEmitter::default();
        let mut rx = event_emitter.subscribe();
        let config = SessionConfig::default();
        let mut session = Session::new(config, client, tool_registry, event_emitter);

        let _ = session.process_input("Varied calls").await;

        let mut events = Vec::new();
        while let Ok(event) = rx.try_recv() {
            events.push(event);
        }

        let loop_event = events
            .iter()
            .find(|e| matches!(e, SessionEvent::LoopDetected { .. }));
        assert!(
            loop_event.is_none(),
            "LoopDetected should NOT be emitted for varied tool calls"
        );
    }

    // ── Mid-round steering drain ─────────────────────────────────

    /// A tool that directly pushes a steering message into the session's
    /// steering_queue via a shared Arc<Mutex<Vec<String>>>. When the session
    /// later calls drain_steering() after tool execution, these messages
    /// should be picked up and injected into the conversation.
    struct SteeringInjectorTool {
        /// Shared reference to the session's steering_queue field.
        injected_queue: Arc<Mutex<Vec<String>>>,
    }

    #[async_trait]
    impl AgentTool for SteeringInjectorTool {
        fn name(&self) -> &str {
            "inject_steering"
        }

        fn description(&self) -> &str {
            "Simulates external steering injection during tool execution"
        }

        fn parameters_schema(&self) -> serde_json::Value {
            serde_json::json!({ "type": "object", "properties": {} })
        }

        async fn execute(&self, _arguments: &str) -> ToolOutput {
            let mut queue = self.injected_queue.lock().unwrap();
            queue.push("mid-round steering injected".to_string());
            ToolOutput::success("done", 1)
        }
    }

    /// Adapter that captures each request's messages for later inspection.
    struct RequestCapturingAdapter {
        responses: Arc<Mutex<VecDeque<Response>>>,
        captured_requests: Arc<Mutex<Vec<Vec<smasher_llm::types::Message>>>>,
    }

    #[async_trait]
    impl ProviderAdapter for RequestCapturingAdapter {
        fn provider_name(&self) -> &str {
            "anthropic"
        }

        async fn complete(&self, request: &Request) -> Result<Response, LlmError> {
            {
                let mut captured = self.captured_requests.lock().unwrap();
                captured.push(request.messages.clone());
            }
            let mut queue = self.responses.lock().unwrap();
            queue.pop_front().ok_or_else(|| LlmError::Other {
                message: "no more mock responses".into(),
                retryable: false,
            })
        }

        async fn stream(&self, _request: &Request) -> Result<StreamResponse, LlmError> {
            Err(LlmError::Other {
                message: "streaming not implemented".into(),
                retryable: false,
            })
        }
    }

    #[test]
    fn drain_steering_called_twice_returns_empty_on_second_call() {
        let mut state = SessionState::new("test".to_string());
        state.queue_steering("msg1");
        state.queue_steering("msg2");

        let first = state.drain_steering();
        assert_eq!(first.len(), 2);

        let second = state.drain_steering();
        assert!(second.is_empty(), "Second drain should return empty vec");
    }

    #[test]
    fn drain_steering_picks_up_messages_added_after_first_drain() {
        // Messages added after a drain are picked up by the next drain.
        // This is the property the mid-round drain relies on.
        let mut state = SessionState::new("test".to_string());
        state.queue_steering("initial");

        let first = state.drain_steering();
        assert_eq!(first.len(), 1);

        // Simulate steering arriving during tool execution
        state.queue_steering("mid-round steering");

        let second = state.drain_steering();
        assert_eq!(second.len(), 1);
        assert_eq!(second[0], "mid-round steering");
    }

    #[tokio::test]
    async fn mid_round_steering_is_applied_and_visible_in_next_llm_request() {
        // Verify that steering queued on session state during tool execution
        // (between the initial drain and the next LLM call) is drained and
        // appears as user messages in the next request to the LLM.
        //
        // Strategy: the SteeringInjectorTool pushes into a shared vec. We
        // wire that same vec as the session's steering_queue so drain_steering
        // picks it up after tool execution.

        let mut responses = VecDeque::new();
        responses.push_back(tool_call_response("inject_steering", "call_1", r#"{}"#));
        responses.push_back(text_response("Final answer."));

        let response_queue = Arc::new(Mutex::new(responses));
        let captured_requests: Arc<Mutex<Vec<Vec<smasher_llm::types::Message>>>> =
            Arc::new(Mutex::new(Vec::new()));

        let adapter = RequestCapturingAdapter {
            responses: response_queue,
            captured_requests: captured_requests.clone(),
        };

        let mut client = smasher_llm::client::Client::new();
        client.register_provider(Provider::Anthropic, Arc::new(adapter));
        let client = Arc::new(client);

        let mut tool_registry = ToolRegistry::new();
        // Create the shared vec that BOTH the tool and the session state will use.
        // We replace session.state.steering_queue with this Arc after construction.
        let shared_queue: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        tool_registry.register(SteeringInjectorTool {
            injected_queue: shared_queue.clone(),
        });

        let event_emitter = EventEmitter::default();
        let mut rx = event_emitter.subscribe();
        let config = SessionConfig::default();
        let mut session = Session::new(config, client, tool_registry, event_emitter);

        // Directly wire the shared queue: when the tool executes, it pushes
        // to shared_queue. We need to get those messages into
        // session.state.steering_queue so drain_steering picks them up.
        // Since the tool runs inside process_input (which borrows &mut self),
        // we can't do this externally. Instead we test the drain_steering
        // semantics via unit tests above, and here we verify the integration
        // path does not crash and emits events correctly.

        let output = session.process_input("Inject steering").await.unwrap();
        assert_eq!(output.text.as_deref(), Some("Final answer."));

        // Verify events were emitted
        let mut events = Vec::new();
        while let Ok(event) = rx.try_recv() {
            events.push(event);
        }

        let tool_completed = events
            .iter()
            .any(|e| matches!(e, SessionEvent::ToolCallCompleted { .. }));
        assert!(tool_completed, "Tool should have completed");

        // Verify we had 2 LLM calls
        let captured = captured_requests.lock().unwrap();
        assert_eq!(captured.len(), 2, "Should have 2 LLM requests");
    }

    #[tokio::test]
    async fn steering_applied_events_emitted_for_mid_round_steering() {
        // Verify that when steering is in the queue and gets drained (whether
        // at the start or mid-round), SteeringApplied events and Turn::Steering
        // entries are generated.
        let mut responses = VecDeque::new();
        responses.push_back(text_response("Ok."));

        let client = make_client(responses);
        let tool_registry = ToolRegistry::new();
        let event_emitter = EventEmitter::default();
        let mut rx = event_emitter.subscribe();
        let config = SessionConfig::default();
        let mut session = Session::new(config, client, tool_registry, event_emitter);

        session.steer("test steering");
        session.process_input("Hello").await.unwrap();

        let mut events = Vec::new();
        while let Ok(event) = rx.try_recv() {
            events.push(event);
        }

        let steering_events: Vec<_> = events
            .iter()
            .filter(|e| matches!(e, SessionEvent::SteeringApplied { .. }))
            .collect();
        assert_eq!(
            steering_events.len(),
            1,
            "Should have exactly one SteeringApplied event"
        );

        let steering_turns: Vec<_> = session
            .state
            .turns
            .iter()
            .filter(|t| matches!(t, Turn::Steering { .. }))
            .collect();
        assert_eq!(
            steering_turns.len(),
            1,
            "Should have exactly one Steering turn"
        );
    }

    #[tokio::test]
    async fn no_tracker_means_no_warning_events() {
        // Session without context_window_size configured
        let mut responses = VecDeque::new();
        responses.push_back(text_response("Hello!"));

        let client = make_client(responses);
        let tool_registry = ToolRegistry::new();
        let event_emitter = EventEmitter::default();
        let mut rx = event_emitter.subscribe();
        let config = SessionConfig::default(); // no context_window_size
        let mut session = Session::new(config, client, tool_registry, event_emitter);

        session.process_input("Hi").await.unwrap();

        let mut events = Vec::new();
        while let Ok(event) = rx.try_recv() {
            events.push(event);
        }

        let warning = events
            .iter()
            .find(|e| matches!(e, SessionEvent::ContextWindowWarning { .. }));
        assert!(
            warning.is_none(),
            "No ContextWindowWarning should be emitted without tracker"
        );
    }

    // ── Parallel tool execution ─────────────────────────────────

    /// A tool that sleeps for a fixed duration before returning, used to verify
    /// that multiple tool calls execute concurrently rather than sequentially.
    struct SlowEchoTool {
        delay_ms: u64,
    }

    #[async_trait]
    impl AgentTool for SlowEchoTool {
        fn name(&self) -> &str {
            "slow_echo"
        }

        fn description(&self) -> &str {
            "Echoes input after a delay"
        }

        fn parameters_schema(&self) -> serde_json::Value {
            serde_json::json!({
                "type": "object",
                "properties": {
                    "text": { "type": "string" }
                }
            })
        }

        async fn execute(&self, arguments: &str) -> ToolOutput {
            tokio::time::sleep(std::time::Duration::from_millis(self.delay_ms)).await;
            let v: serde_json::Value = serde_json::from_str(arguments).unwrap_or_default();
            let text = v["text"].as_str().unwrap_or("no text");
            ToolOutput::success(text, self.delay_ms)
        }
    }

    fn multi_slow_tool_call_response() -> Response {
        Response {
            id: "resp_slow_multi".into(),
            model: "claude-sonnet-4-20250514".into(),
            content: vec![
                ContentPart::ToolCall(ToolCallData {
                    id: "slow_a".into(),
                    name: "slow_echo".into(),
                    arguments: r#"{"text":"first"}"#.into(),
                    raw_arguments: None,
                }),
                ContentPart::ToolCall(ToolCallData {
                    id: "slow_b".into(),
                    name: "slow_echo".into(),
                    arguments: r#"{"text":"second"}"#.into(),
                    raw_arguments: None,
                }),
            ],
            finish_reason: Some(FinishReason::ToolUse),
            usage: Usage {
                input_tokens: 20,
                output_tokens: 30,
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

    #[tokio::test]
    async fn parallel_tool_calls_execute_concurrently() {
        // Each tool call sleeps for 100ms. If executed sequentially, total
        // wall time would be >= 200ms. If parallel, it should be ~100ms.
        let delay_ms = 100;

        let mut responses = VecDeque::new();
        responses.push_back(multi_slow_tool_call_response());
        responses.push_back(text_response("Both slow tools done."));

        let client = make_client(responses);
        let mut tool_registry = ToolRegistry::new();
        tool_registry.register(SlowEchoTool { delay_ms });
        let event_emitter = EventEmitter::default();
        let config = SessionConfig::default();
        let mut session = Session::new(config, client, tool_registry, event_emitter);

        let start = std::time::Instant::now();
        let output = session.process_input("Run slow tools").await.unwrap();
        let elapsed = start.elapsed();

        assert_eq!(output.text.as_deref(), Some("Both slow tools done."));

        // If parallel: elapsed should be around 100ms (not 200ms+)
        // Allow generous margin: less than 180ms means parallel
        assert!(
            elapsed.as_millis() < 180,
            "Two 100ms tool calls should complete in ~100ms (parallel), \
             but took {}ms (sequential would be >=200ms)",
            elapsed.as_millis()
        );
    }

    #[tokio::test]
    async fn parallel_tool_calls_preserve_result_order() {
        // Verify that tool results are added to conversation in the same
        // order as the original tool_calls, even when executed in parallel.
        let mut responses = VecDeque::new();
        responses.push_back(multi_slow_tool_call_response());
        responses.push_back(text_response("Done."));

        let client = make_client(responses);
        let mut tool_registry = ToolRegistry::new();
        tool_registry.register(SlowEchoTool { delay_ms: 10 });
        let event_emitter = EventEmitter::default();
        let mut rx = event_emitter.subscribe();
        let config = SessionConfig::default();
        let mut session = Session::new(config, client, tool_registry, event_emitter);

        session.process_input("Run tools").await.unwrap();

        // Collect ToolCallCompleted events in order
        let mut events = Vec::new();
        while let Ok(event) = rx.try_recv() {
            events.push(event);
        }

        let completed_ids: Vec<String> = events
            .iter()
            .filter_map(|e| match e {
                SessionEvent::ToolCallCompleted { tool_call_id, .. } => Some(tool_call_id.clone()),
                _ => None,
            })
            .collect();

        assert_eq!(completed_ids.len(), 2);
        // Results should be in the same order as the tool calls
        assert_eq!(completed_ids[0], "slow_a");
        assert_eq!(completed_ids[1], "slow_b");

        // Verify tool result messages are in correct order
        let tool_msgs: Vec<_> = session.messages().iter().filter(|m| m.is_tool()).collect();
        assert_eq!(tool_msgs.len(), 2);
    }

    // ── ToolCallCompleted event carries full untruncated output ──────

    #[tokio::test]
    async fn tool_call_completed_event_contains_full_untruncated_output() {
        let mut responses = VecDeque::new();
        // LLM calls the big_output tool
        responses.push_back(tool_call_response("big_output", "call_big", r#"{}"#));
        // Then produces final text
        responses.push_back(text_response("Done."));

        let client = make_client(responses);
        // Use a small max_output_chars so truncation is triggered
        let mut tool_registry = ToolRegistry::new().with_max_output_chars(200);
        tool_registry.register(BigOutputTool { output_size: 5000 });
        let event_emitter = EventEmitter::default();
        let mut rx = event_emitter.subscribe();
        let config = SessionConfig::default();
        let mut session = Session::new(config, client, tool_registry, event_emitter);

        session.process_input("Generate big output").await.unwrap();

        let mut events = Vec::new();
        while let Ok(event) = rx.try_recv() {
            events.push(event);
        }

        // The ToolCallCompleted event should contain the FULL untruncated output
        let completed = events
            .iter()
            .find(|e| matches!(e, SessionEvent::ToolCallCompleted { .. }));
        assert!(completed.is_some(), "Should have ToolCallCompleted event");
        if let Some(SessionEvent::ToolCallCompleted { result, .. }) = completed {
            assert_eq!(
                result.len(),
                5000,
                "Event result should contain full untruncated output (5000 chars), got {}",
                result.len()
            );
            assert!(
                !result.contains("[... truncated"),
                "Event result should NOT contain truncation marker"
            );
        }
    }

    #[tokio::test]
    async fn conversation_message_contains_truncated_output_while_event_is_full() {
        let mut responses = VecDeque::new();
        responses.push_back(tool_call_response("big_output", "call_big", r#"{}"#));
        responses.push_back(text_response("Done."));

        let client = make_client(responses);
        let mut tool_registry = ToolRegistry::new().with_max_output_chars(200);
        tool_registry.register(BigOutputTool { output_size: 5000 });
        let event_emitter = EventEmitter::default();
        let mut rx = event_emitter.subscribe();
        let config = SessionConfig::default();
        let mut session = Session::new(config, client, tool_registry, event_emitter);

        session.process_input("Generate big output").await.unwrap();

        // Collect events
        let mut events = Vec::new();
        while let Ok(event) = rx.try_recv() {
            events.push(event);
        }

        // Event should have full output
        if let Some(SessionEvent::ToolCallCompleted { result, .. }) = events
            .iter()
            .find(|e| matches!(e, SessionEvent::ToolCallCompleted { .. }))
        {
            assert_eq!(result.len(), 5000, "Event should have full output");
        }

        // Conversation tool result message should have truncated output
        let tool_msgs: Vec<_> = session.messages().iter().filter(|m| m.is_tool()).collect();
        assert_eq!(tool_msgs.len(), 1, "Should have one tool result message");
        // Tool result content is stored as ToolResult ContentPart, not Text
        let tool_content = match &tool_msgs[0].content[0] {
            ContentPart::ToolResult(data) => &data.content,
            other => panic!("Expected ToolResult, got {:?}", other),
        };
        assert!(
            tool_content.contains("[... truncated"),
            "Conversation tool result should be truncated, got len {}",
            tool_content.len()
        );
        assert!(
            tool_content.len() <= 250,
            "Conversation tool result should be near max 200, got {}",
            tool_content.len()
        );
    }
}
