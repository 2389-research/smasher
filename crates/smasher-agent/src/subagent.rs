// ABOUTME: Subagent support for spawning child sessions with independent conversation histories.
// ABOUTME: Provides config, result types, and a manager that enforces depth and concurrency limits.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use crate::events::EventEmitter;
use crate::session::{Session, SessionError};
use crate::tools::ToolRegistry;
use crate::types::SessionConfig;

/// Configuration for spawning a subagent session.
#[derive(Debug, Clone)]
pub struct SubagentConfig {
    /// The task/prompt for the subagent.
    pub task: String,
    /// Model override (if different from parent).
    pub model: Option<String>,
    /// System prompt override.
    pub system_prompt: Option<String>,
    /// Maximum turns for the subagent.
    pub max_turns: u32,
    /// Current depth (incremented from parent).
    pub depth: u32,
    /// Maximum allowed depth.
    pub max_depth: u32,
}

impl Default for SubagentConfig {
    fn default() -> Self {
        Self {
            task: String::new(),
            model: None,
            system_prompt: None,
            max_turns: 20,
            depth: 0,
            max_depth: 3,
        }
    }
}

impl SubagentConfig {
    /// Set the task prompt for this subagent.
    pub fn with_task(mut self, task: impl Into<String>) -> Self {
        self.task = task.into();
        self
    }

    /// Set an optional model override for this subagent.
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    /// Set an optional system prompt override for this subagent.
    pub fn with_system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.system_prompt = Some(prompt.into());
        self
    }

    /// Set the maximum number of turns for this subagent.
    pub fn with_max_turns(mut self, max_turns: u32) -> Self {
        self.max_turns = max_turns;
        self
    }
}

/// The result produced by a completed subagent session.
#[derive(Debug)]
pub struct SubagentResult {
    /// The subagent's session ID.
    pub session_id: String,
    /// The final text output from the subagent.
    pub output: Option<String>,
    /// Number of turns the subagent used.
    pub turns_used: u32,
    /// Token usage from the subagent's session.
    pub usage: smasher_llm::types::Usage,
    /// Whether the subagent completed successfully.
    pub success: bool,
    /// Error message if the subagent failed.
    pub error: Option<String>,
}

/// Errors that can occur when spawning or running a subagent.
#[derive(Debug, thiserror::Error)]
pub enum SubagentError {
    #[error("maximum subagent depth ({max_depth}) exceeded")]
    DepthLimitExceeded { max_depth: u32 },
    #[error("maximum concurrent subagents ({max_concurrent}) reached")]
    ConcurrencyLimitReached { max_concurrent: usize },
    #[error("subagent session error: {0}")]
    SessionError(#[from] SessionError),
}

/// Manages subagent lifecycle, enforcing depth and concurrency limits.
pub struct SubagentManager {
    /// Maximum concurrent subagents.
    max_concurrent: usize,
    /// Currently active subagent count.
    active_count: Arc<AtomicU32>,
}

impl SubagentManager {
    /// Create a manager with the given concurrency limit.
    pub fn new(max_concurrent: usize) -> Self {
        Self {
            max_concurrent,
            active_count: Arc::new(AtomicU32::new(0)),
        }
    }

    /// Check if we are below the concurrent subagent limit.
    pub fn can_spawn(&self) -> bool {
        (self.active_count.load(Ordering::SeqCst) as usize) < self.max_concurrent
    }

    /// Spawn a subagent session, run its task, and return the result.
    ///
    /// Validates depth and concurrency limits before creating the session.
    /// The active count is incremented while the subagent runs and decremented
    /// when it finishes (regardless of success or failure).
    pub async fn spawn(
        &self,
        config: SubagentConfig,
        client: Arc<smasher_llm::client::Client>,
        tool_registry: ToolRegistry,
        event_emitter: EventEmitter,
    ) -> Result<SubagentResult, SubagentError> {
        // Check depth limit
        if config.depth >= config.max_depth {
            return Err(SubagentError::DepthLimitExceeded {
                max_depth: config.max_depth,
            });
        }

        // Check concurrency limit
        if !self.can_spawn() {
            return Err(SubagentError::ConcurrencyLimitReached {
                max_concurrent: self.max_concurrent,
            });
        }

        // Increment active count
        self.active_count.fetch_add(1, Ordering::SeqCst);

        // Build a SessionConfig from the SubagentConfig
        let mut session_config = SessionConfig::default().with_max_turns(config.max_turns);

        if let Some(ref model) = config.model {
            session_config = session_config.with_model(model);
        }

        if let Some(ref prompt) = config.system_prompt {
            session_config = session_config.with_system_prompt(prompt);
        }

        // Create and run the session
        let mut session = Session::new(session_config, client, tool_registry, event_emitter);
        let session_id = session.session_id().to_string();

        let result = session.process_input(&config.task).await;

        // Decrement active count (always, even on failure)
        self.active_count.fetch_sub(1, Ordering::SeqCst);

        match result {
            Ok(output) => Ok(SubagentResult {
                session_id,
                output: output.text,
                turns_used: output.turns_used,
                usage: output.total_usage,
                success: true,
                error: None,
            }),
            Err(e) => Ok(SubagentResult {
                session_id,
                output: None,
                turns_used: session.turn_count(),
                usage: session.total_usage().clone(),
                success: false,
                error: Some(e.to_string()),
            }),
        }
    }

    /// Return the current number of active subagents (visible for testing).
    #[cfg(test)]
    fn active_count(&self) -> u32 {
        self.active_count.load(Ordering::SeqCst)
    }
}

impl Default for SubagentManager {
    fn default() -> Self {
        Self::new(4)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use smasher_llm::provider::{ProviderAdapter, StreamResponse};
    use smasher_llm::types::{
        ContentPart, Error as LlmError, FinishReason, Provider, Request, Response, Usage,
    };
    use std::collections::VecDeque;
    use std::sync::Mutex;

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

    fn make_client(responses: VecDeque<Response>) -> Arc<smasher_llm::client::Client> {
        let response_queue = Arc::new(Mutex::new(responses));
        let adapter = MockAdapter {
            responses: response_queue,
        };
        let mut client = smasher_llm::client::Client::new();
        client.register_provider(Provider::Anthropic, Arc::new(adapter));
        Arc::new(client)
    }

    // ── SubagentConfig tests ─────────────────────────────────────────

    #[test]
    fn subagent_config_default_values() {
        let config = SubagentConfig::default();
        assert_eq!(config.task, "");
        assert!(config.model.is_none());
        assert!(config.system_prompt.is_none());
        assert_eq!(config.max_turns, 20);
        assert_eq!(config.depth, 0);
        assert_eq!(config.max_depth, 3);
    }

    #[test]
    fn subagent_config_builder_with_task() {
        let config = SubagentConfig::default().with_task("Refactor the parser");
        assert_eq!(config.task, "Refactor the parser");
    }

    #[test]
    fn subagent_config_builder_with_model() {
        let config = SubagentConfig::default().with_model("gpt-4o");
        assert_eq!(config.model.as_deref(), Some("gpt-4o"));
    }

    #[test]
    fn subagent_config_builder_with_system_prompt() {
        let config = SubagentConfig::default().with_system_prompt("Be concise.");
        assert_eq!(config.system_prompt.as_deref(), Some("Be concise."));
    }

    #[test]
    fn subagent_config_builder_with_max_turns() {
        let config = SubagentConfig::default().with_max_turns(10);
        assert_eq!(config.max_turns, 10);
    }

    #[test]
    fn subagent_config_builder_chaining() {
        let config = SubagentConfig::default()
            .with_task("Do stuff")
            .with_model("claude-sonnet-4-20250514")
            .with_system_prompt("You are helpful.")
            .with_max_turns(5);

        assert_eq!(config.task, "Do stuff");
        assert_eq!(config.model.as_deref(), Some("claude-sonnet-4-20250514"));
        assert_eq!(config.system_prompt.as_deref(), Some("You are helpful."));
        assert_eq!(config.max_turns, 5);
    }

    // ── SubagentManager tests ────────────────────────────────────────

    #[test]
    fn subagent_manager_new_with_custom_max() {
        let manager = SubagentManager::new(8);
        assert_eq!(manager.max_concurrent, 8);
        assert_eq!(manager.active_count(), 0);
    }

    #[test]
    fn subagent_manager_default() {
        let manager = SubagentManager::default();
        assert_eq!(manager.max_concurrent, 4);
        assert_eq!(manager.active_count(), 0);
    }

    #[test]
    fn can_spawn_true_when_below_limit() {
        let manager = SubagentManager::new(2);
        assert!(manager.can_spawn());
    }

    #[test]
    fn can_spawn_false_when_at_limit() {
        let manager = SubagentManager::new(2);
        manager.active_count.store(2, Ordering::SeqCst);
        assert!(!manager.can_spawn());
    }

    #[tokio::test]
    async fn spawn_returns_depth_error_when_exceeded() {
        let manager = SubagentManager::new(4);
        let config = SubagentConfig {
            task: "deep task".into(),
            depth: 3,
            max_depth: 3,
            ..Default::default()
        };

        let client = make_client(VecDeque::new());
        let tool_registry = ToolRegistry::new();
        let event_emitter = EventEmitter::default();

        let result = manager
            .spawn(config, client, tool_registry, event_emitter)
            .await;
        assert!(result.is_err());
        match result.unwrap_err() {
            SubagentError::DepthLimitExceeded { max_depth } => {
                assert_eq!(max_depth, 3);
            }
            other => panic!("Expected DepthLimitExceeded, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn spawn_returns_concurrency_error_when_limit_reached() {
        let manager = SubagentManager::new(2);
        // Manually set active count to the limit
        manager.active_count.store(2, Ordering::SeqCst);

        let config = SubagentConfig::default().with_task("blocked task");
        let client = make_client(VecDeque::new());
        let tool_registry = ToolRegistry::new();
        let event_emitter = EventEmitter::default();

        let result = manager
            .spawn(config, client, tool_registry, event_emitter)
            .await;
        assert!(result.is_err());
        match result.unwrap_err() {
            SubagentError::ConcurrencyLimitReached { max_concurrent } => {
                assert_eq!(max_concurrent, 2);
            }
            other => panic!("Expected ConcurrencyLimitReached, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn spawn_creates_and_runs_subagent() {
        let manager = SubagentManager::new(4);

        let mut responses = VecDeque::new();
        responses.push_back(text_response("subagent output"));

        let client = make_client(responses);
        let tool_registry = ToolRegistry::new();
        let event_emitter = EventEmitter::default();

        let config = SubagentConfig::default()
            .with_task("Summarize the code")
            .with_max_turns(5);

        let result = manager
            .spawn(config, client, tool_registry, event_emitter)
            .await
            .expect("spawn should succeed");

        assert!(result.success);
        assert_eq!(result.output.as_deref(), Some("subagent output"));
        assert_eq!(result.turns_used, 1);
        assert!(result.error.is_none());
        assert!(!result.session_id.is_empty());
        // Active count should be back to 0 after completion
        assert_eq!(manager.active_count(), 0);
    }

    #[tokio::test]
    async fn subagent_result_fields_correct_after_successful_spawn() {
        let manager = SubagentManager::new(4);

        let mut responses = VecDeque::new();
        responses.push_back(text_response("result check"));

        let client = make_client(responses);
        let tool_registry = ToolRegistry::new();
        let event_emitter = EventEmitter::default();

        let config = SubagentConfig {
            task: "Verify fields".into(),
            model: Some("claude-sonnet-4-20250514".into()),
            system_prompt: Some("Test prompt".into()),
            max_turns: 10,
            depth: 1,
            max_depth: 3,
        };

        let result = manager
            .spawn(config, client, tool_registry, event_emitter)
            .await
            .expect("spawn should succeed");

        assert!(result.success);
        assert_eq!(result.output.as_deref(), Some("result check"));
        assert_eq!(result.turns_used, 1);
        assert_eq!(result.usage.input_tokens, 10);
        assert_eq!(result.usage.output_tokens, 20);
        assert!(result.error.is_none());
        // Session ID should be a valid UUID (36 chars with 4 dashes)
        assert_eq!(result.session_id.len(), 36);
        assert_eq!(result.session_id.chars().filter(|c| *c == '-').count(), 4);
    }
}
