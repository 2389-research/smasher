// ABOUTME: Core type definitions for the coding agent session loop.
// ABOUTME: Provides Turn, SessionEvent, SessionConfig, and SessionState used to drive agentic conversations.

use std::collections::HashMap;

use smasher_llm::types::{Message, Response, ThinkingConfig, Usage};

/// Represents a single turn in the agent conversation loop.
#[derive(Debug, Clone)]
pub enum Turn {
    /// User provided input text.
    UserInput { text: String },
    /// Assistant produced a response (may include text and/or tool calls).
    AssistantResponse { response: Response },
    /// A tool was called and produced a result.
    ToolExecution {
        tool_name: String,
        tool_call_id: String,
        arguments: String,
        result: String,
        is_error: bool,
        duration_ms: u64,
    },
    /// A steering message was injected by the system.
    Steering { text: String },
    /// An error occurred during the turn.
    Error { message: String },
}

/// Events emitted by the session for subscribers (via broadcast channel).
#[derive(Debug, Clone)]
pub enum SessionEvent {
    /// Session has started.
    SessionStarted { session_id: String },
    /// A new turn began.
    TurnStarted { turn_number: u32 },
    /// The LLM produced a response.
    AssistantMessage { response: Response },
    /// A tool is being executed.
    ToolCallStarted {
        tool_name: String,
        tool_call_id: String,
        input_preview: String,
    },
    /// A tool finished execution.
    ToolCallCompleted {
        tool_name: String,
        tool_call_id: String,
        result: String,
        is_error: bool,
        duration_ms: u64,
    },
    /// A streaming text delta arrived.
    TextDelta { text: String },
    /// Steering was applied.
    SteeringApplied { text: String },
    /// Session completed normally.
    SessionCompleted {
        session_id: String,
        total_turns: u32,
        total_usage: Usage,
    },
    /// Session ended with an error.
    SessionError { session_id: String, error: String },
    /// Loop detection triggered.
    LoopDetected { pattern: String, window_size: usize },
    /// Context window usage crossed a warning threshold.
    ContextWindowWarning {
        used: usize,
        limit: usize,
        fraction: f64,
    },
}

/// Configuration for a coding agent session.
#[derive(Debug, Clone)]
pub struct SessionConfig {
    /// The model to use for this session.
    pub model: String,
    /// Optional system prompt override.
    pub system_prompt: Option<String>,
    /// Maximum turns before the session is forcibly ended.
    pub max_turns: u32,
    /// Maximum tokens per response.
    pub max_tokens: Option<u32>,
    /// Temperature for sampling.
    pub temperature: Option<f32>,
    /// Whether to enable streaming.
    pub stream: bool,
    /// Whether to enable thinking/reasoning.
    pub thinking: Option<ThinkingConfig>,
    /// Working directory for the session.
    pub working_directory: Option<String>,
    /// Environment variables to pass to tool executions.
    pub env_vars: Option<HashMap<String, String>>,
    /// Default timeout in milliseconds for shell tool commands.
    pub default_command_timeout_ms: Option<u64>,
    /// Maximum allowed timeout in milliseconds for shell tool commands.
    pub max_command_timeout_ms: Option<u64>,
    /// Size of the model's context window in tokens (for usage tracking).
    pub context_window_size: Option<usize>,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            model: "claude-sonnet-4-20250514".into(),
            max_turns: 100,
            max_tokens: Some(8192),
            temperature: None,
            stream: true,
            thinking: None,
            system_prompt: None,
            working_directory: None,
            env_vars: None,
            default_command_timeout_ms: None,
            max_command_timeout_ms: None,
            context_window_size: None,
        }
    }
}

impl SessionConfig {
    /// Set the model for this session.
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }

    /// Set the system prompt for this session.
    pub fn with_system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.system_prompt = Some(prompt.into());
        self
    }

    /// Set the maximum number of turns.
    pub fn with_max_turns(mut self, max_turns: u32) -> Self {
        self.max_turns = max_turns;
        self
    }

    /// Set the maximum tokens per response.
    pub fn with_max_tokens(mut self, max_tokens: u32) -> Self {
        self.max_tokens = Some(max_tokens);
        self
    }

    /// Set the sampling temperature.
    pub fn with_temperature(mut self, temperature: f32) -> Self {
        self.temperature = Some(temperature);
        self
    }

    /// Enable or disable streaming.
    pub fn with_stream(mut self, stream: bool) -> Self {
        self.stream = stream;
        self
    }

    /// Set the thinking/reasoning configuration.
    pub fn with_thinking(mut self, thinking: ThinkingConfig) -> Self {
        self.thinking = Some(thinking);
        self
    }

    /// Set the working directory for the session.
    pub fn with_working_directory(mut self, dir: impl Into<String>) -> Self {
        self.working_directory = Some(dir.into());
        self
    }

    /// Set the default timeout for shell tool commands (in milliseconds).
    pub fn with_default_command_timeout_ms(mut self, timeout_ms: u64) -> Self {
        self.default_command_timeout_ms = Some(timeout_ms);
        self
    }

    /// Set the maximum allowed timeout for shell tool commands (in milliseconds).
    pub fn with_max_command_timeout_ms(mut self, timeout_ms: u64) -> Self {
        self.max_command_timeout_ms = Some(timeout_ms);
        self
    }

    /// Set the context window size in tokens (for usage tracking).
    pub fn with_context_window_size(mut self, size: usize) -> Self {
        self.context_window_size = Some(size);
        self
    }
}

/// Tracks the lifecycle phase of a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionPhase {
    /// The session is running and processing turns.
    Active,
    /// Waiting for user input (model asked a question).
    AwaitingInput,
    /// The session has completed (either normally or via turn limit).
    Completed,
}

/// Mutable state tracking a running agent session.
#[derive(Debug)]
pub struct SessionState {
    /// Unique session identifier.
    pub session_id: String,
    /// All messages in the conversation so far.
    pub messages: Vec<Message>,
    /// History of all turns.
    pub turns: Vec<Turn>,
    /// Current turn number.
    pub turn_number: u32,
    /// Accumulated token usage.
    pub total_usage: Usage,
    /// Whether the session is still active.
    pub active: bool,
    /// Current lifecycle phase of the session.
    pub phase: SessionPhase,
    /// Pending steering messages to inject.
    pub steering_queue: Vec<String>,
}

impl SessionState {
    /// Create a fresh session state with the given identifier.
    pub fn new(session_id: String) -> Self {
        Self {
            session_id,
            messages: Vec::new(),
            turns: Vec::new(),
            turn_number: 0,
            total_usage: Usage::default(),
            active: true,
            phase: SessionPhase::Active,
            steering_queue: Vec::new(),
        }
    }

    /// Push a user message onto the conversation history.
    pub fn add_user_message(&mut self, text: &str) {
        self.messages.push(Message::user(text));
        self.turns.push(Turn::UserInput {
            text: text.to_string(),
        });
    }

    /// Push an assistant response onto the conversation and record the turn.
    pub fn add_assistant_response(&mut self, response: &Response) {
        self.messages.push(Message {
            role: smasher_llm::types::Role::Assistant,
            content: response.content.clone(),
            name: None,
            tool_call_id: None,
        });
        self.turns.push(Turn::AssistantResponse {
            response: response.clone(),
        });
        self.total_usage += response.usage.clone();
        self.turn_number += 1;
    }

    /// Push a tool result message onto the conversation history.
    pub fn add_tool_result(&mut self, tool_call_id: &str, content: &str, is_error: bool) {
        self.messages
            .push(Message::tool_result(tool_call_id, content, is_error));
    }

    /// Add a steering message to the pending queue.
    pub fn queue_steering(&mut self, text: &str) {
        self.steering_queue.push(text.to_string());
    }

    /// Take all pending steering messages, leaving the queue empty.
    pub fn drain_steering(&mut self) -> Vec<String> {
        std::mem::take(&mut self.steering_queue)
    }

    /// Check whether the current turn number has reached the given limit.
    pub fn is_at_turn_limit(&self, max_turns: u32) -> bool {
        self.turn_number >= max_turns
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use smasher_llm::types::{ContentPart, FinishReason};

    // ── Helper ────────────────────────────────────────────────────────

    fn sample_response() -> Response {
        Response {
            id: "resp_001".into(),
            model: "claude-sonnet-4-20250514".into(),
            content: vec![ContentPart::text("Hello from the assistant")],
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

    // ── SessionConfig defaults ────────────────────────────────────────

    #[test]
    fn session_config_default_values() {
        let config = SessionConfig::default();
        assert_eq!(config.model, "claude-sonnet-4-20250514");
        assert_eq!(config.max_turns, 100);
        assert_eq!(config.max_tokens, Some(8192));
        assert!(config.temperature.is_none());
        assert!(config.stream);
        assert!(config.thinking.is_none());
        assert!(config.system_prompt.is_none());
        assert!(config.working_directory.is_none());
        assert!(config.env_vars.is_none());
        assert!(config.default_command_timeout_ms.is_none());
        assert!(config.max_command_timeout_ms.is_none());
        assert!(config.context_window_size.is_none());
    }

    // ── SessionConfig builder methods ─────────────────────────────────

    #[test]
    fn session_config_with_model() {
        let config = SessionConfig::default().with_model("gpt-4o");
        assert_eq!(config.model, "gpt-4o");
    }

    #[test]
    fn session_config_with_system_prompt() {
        let config = SessionConfig::default().with_system_prompt("Be helpful.");
        assert_eq!(config.system_prompt.as_deref(), Some("Be helpful."));
    }

    #[test]
    fn session_config_with_max_turns() {
        let config = SessionConfig::default().with_max_turns(50);
        assert_eq!(config.max_turns, 50);
    }

    #[test]
    fn session_config_with_max_tokens() {
        let config = SessionConfig::default().with_max_tokens(4096);
        assert_eq!(config.max_tokens, Some(4096));
    }

    #[test]
    fn session_config_with_temperature() {
        let config = SessionConfig::default().with_temperature(0.7);
        assert_eq!(config.temperature, Some(0.7));
    }

    #[test]
    fn session_config_with_stream() {
        let config = SessionConfig::default().with_stream(false);
        assert!(!config.stream);
    }

    #[test]
    fn session_config_with_thinking() {
        let thinking = ThinkingConfig {
            enabled: true,
            budget_tokens: Some(10_000),
        };
        let config = SessionConfig::default().with_thinking(thinking);
        let t = config.thinking.unwrap();
        assert!(t.enabled);
        assert_eq!(t.budget_tokens, Some(10_000));
    }

    #[test]
    fn session_config_with_working_directory() {
        let config = SessionConfig::default().with_working_directory("/tmp/work");
        assert_eq!(config.working_directory.as_deref(), Some("/tmp/work"));
    }

    #[test]
    fn session_config_builder_chaining() {
        let config = SessionConfig::default()
            .with_model("gemini-pro")
            .with_max_turns(25)
            .with_max_tokens(2048)
            .with_temperature(0.5)
            .with_stream(false)
            .with_system_prompt("You are concise.")
            .with_working_directory("/home/user");

        assert_eq!(config.model, "gemini-pro");
        assert_eq!(config.max_turns, 25);
        assert_eq!(config.max_tokens, Some(2048));
        assert_eq!(config.temperature, Some(0.5));
        assert!(!config.stream);
        assert_eq!(config.system_prompt.as_deref(), Some("You are concise."));
        assert_eq!(config.working_directory.as_deref(), Some("/home/user"));
    }

    // ── SessionState::new ─────────────────────────────────────────────

    #[test]
    fn session_state_new_creates_fresh_state() {
        let state = SessionState::new("sess_abc".into());
        assert_eq!(state.session_id, "sess_abc");
        assert!(state.messages.is_empty());
        assert!(state.turns.is_empty());
        assert_eq!(state.turn_number, 0);
        assert_eq!(state.total_usage.input_tokens, 0);
        assert_eq!(state.total_usage.output_tokens, 0);
        assert!(state.steering_queue.is_empty());
    }

    #[test]
    fn session_state_active_flag_starts_true() {
        let state = SessionState::new("sess_active".into());
        assert!(state.active);
    }

    // ── SessionState::add_user_message ────────────────────────────────

    #[test]
    fn session_state_add_user_message() {
        let mut state = SessionState::new("sess_001".into());
        state.add_user_message("Hello agent");

        assert_eq!(state.messages.len(), 1);
        assert_eq!(state.messages[0].text(), Some("Hello agent".to_string()));
        assert_eq!(state.turns.len(), 1);
        assert!(matches!(&state.turns[0], Turn::UserInput { text } if text == "Hello agent"));
    }

    // ── SessionState::add_assistant_response ──────────────────────────

    #[test]
    fn session_state_add_assistant_response_increments_turn() {
        let mut state = SessionState::new("sess_002".into());
        let resp = sample_response();

        state.add_assistant_response(&resp);

        assert_eq!(state.turn_number, 1);
        assert_eq!(state.messages.len(), 1);
        assert_eq!(state.turns.len(), 1);
        assert!(matches!(&state.turns[0], Turn::AssistantResponse { .. }));
        assert_eq!(state.total_usage.input_tokens, 10);
        assert_eq!(state.total_usage.output_tokens, 20);
    }

    #[test]
    fn session_state_add_assistant_response_accumulates_usage() {
        let mut state = SessionState::new("sess_003".into());
        let resp = sample_response();

        state.add_assistant_response(&resp);
        state.add_assistant_response(&resp);

        assert_eq!(state.turn_number, 2);
        assert_eq!(state.total_usage.input_tokens, 20);
        assert_eq!(state.total_usage.output_tokens, 40);
    }

    // ── SessionState::add_tool_result ─────────────────────────────────

    #[test]
    fn session_state_add_tool_result() {
        let mut state = SessionState::new("sess_004".into());
        state.add_tool_result("call_123", "tool output here", false);

        assert_eq!(state.messages.len(), 1);
        assert!(state.messages[0].is_tool());
    }

    #[test]
    fn session_state_add_tool_result_with_error() {
        let mut state = SessionState::new("sess_005".into());
        state.add_tool_result("call_fail", "something broke", true);

        assert_eq!(state.messages.len(), 1);
        match &state.messages[0].content[0] {
            ContentPart::ToolResult(data) => {
                assert_eq!(data.tool_call_id, "call_fail");
                assert_eq!(data.content, "something broke");
                assert!(data.is_error);
            }
            other => panic!("Expected ToolResult, got {:?}", other),
        }
    }

    // ── SessionState steering queue/drain ─────────────────────────────

    #[test]
    fn session_state_queue_steering() {
        let mut state = SessionState::new("sess_006".into());
        state.queue_steering("focus on tests");
        state.queue_steering("be concise");

        assert_eq!(state.steering_queue.len(), 2);
        assert_eq!(state.steering_queue[0], "focus on tests");
        assert_eq!(state.steering_queue[1], "be concise");
    }

    #[test]
    fn session_state_drain_steering_returns_all_and_empties() {
        let mut state = SessionState::new("sess_007".into());
        state.queue_steering("msg1");
        state.queue_steering("msg2");

        let drained = state.drain_steering();
        assert_eq!(drained.len(), 2);
        assert_eq!(drained[0], "msg1");
        assert_eq!(drained[1], "msg2");
        assert!(state.steering_queue.is_empty());
    }

    #[test]
    fn session_state_drain_steering_on_empty_returns_empty() {
        let mut state = SessionState::new("sess_008".into());
        let drained = state.drain_steering();
        assert!(drained.is_empty());
        assert!(state.steering_queue.is_empty());
    }

    // ── SessionState::is_at_turn_limit ────────────────────────────────

    #[test]
    fn session_state_is_at_turn_limit_false_when_below() {
        let state = SessionState::new("sess_009".into());
        assert!(!state.is_at_turn_limit(10));
    }

    #[test]
    fn session_state_is_at_turn_limit_true_when_at_limit() {
        let mut state = SessionState::new("sess_010".into());
        let resp = sample_response();
        for _ in 0..5 {
            state.add_assistant_response(&resp);
        }
        assert!(state.is_at_turn_limit(5));
    }

    #[test]
    fn session_state_is_at_turn_limit_true_when_over_limit() {
        let mut state = SessionState::new("sess_011".into());
        let resp = sample_response();
        for _ in 0..10 {
            state.add_assistant_response(&resp);
        }
        assert!(state.is_at_turn_limit(5));
    }

    // ── Turn variants can be constructed ──────────────────────────────

    #[test]
    fn turn_user_input_construction() {
        let turn = Turn::UserInput { text: "hi".into() };
        assert!(matches!(turn, Turn::UserInput { text } if text == "hi"));
    }

    #[test]
    fn turn_assistant_response_construction() {
        let turn = Turn::AssistantResponse {
            response: sample_response(),
        };
        assert!(matches!(turn, Turn::AssistantResponse { .. }));
    }

    #[test]
    fn turn_tool_execution_construction() {
        let turn = Turn::ToolExecution {
            tool_name: "bash".into(),
            tool_call_id: "call_1".into(),
            arguments: r#"{"cmd":"ls"}"#.into(),
            result: "file1.rs\nfile2.rs".into(),
            is_error: false,
            duration_ms: 42,
        };
        assert!(matches!(
            turn,
            Turn::ToolExecution {
                duration_ms: 42,
                ..
            }
        ));
    }

    #[test]
    fn turn_steering_construction() {
        let turn = Turn::Steering {
            text: "stay focused".into(),
        };
        assert!(matches!(turn, Turn::Steering { text } if text == "stay focused"));
    }

    #[test]
    fn turn_error_construction() {
        let turn = Turn::Error {
            message: "something went wrong".into(),
        };
        assert!(matches!(turn, Turn::Error { message } if message == "something went wrong"));
    }

    // ── SessionEvent variants compile ─────────────────────────────────

    #[test]
    fn session_event_session_started() {
        let _event = SessionEvent::SessionStarted {
            session_id: "s1".into(),
        };
    }

    #[test]
    fn session_event_turn_started() {
        let _event = SessionEvent::TurnStarted { turn_number: 1 };
    }

    #[test]
    fn session_event_assistant_message() {
        let _event = SessionEvent::AssistantMessage {
            response: sample_response(),
        };
    }

    #[test]
    fn session_event_tool_call_started() {
        let _event = SessionEvent::ToolCallStarted {
            tool_name: "bash".into(),
            tool_call_id: "call_1".into(),
            input_preview: "echo hello".into(),
        };
    }

    #[test]
    fn session_event_tool_call_completed() {
        let _event = SessionEvent::ToolCallCompleted {
            tool_name: "bash".into(),
            tool_call_id: "call_1".into(),
            result: "ok".into(),
            is_error: false,
            duration_ms: 100,
        };
    }

    #[test]
    fn session_event_text_delta() {
        let _event = SessionEvent::TextDelta {
            text: "partial".into(),
        };
    }

    #[test]
    fn session_event_steering_applied() {
        let _event = SessionEvent::SteeringApplied {
            text: "steer".into(),
        };
    }

    #[test]
    fn session_event_session_completed() {
        let _event = SessionEvent::SessionCompleted {
            session_id: "s1".into(),
            total_turns: 5,
            total_usage: Usage::default(),
        };
    }

    #[test]
    fn session_event_session_error() {
        let _event = SessionEvent::SessionError {
            session_id: "s1".into(),
            error: "boom".into(),
        };
    }

    #[test]
    fn session_event_loop_detected() {
        let _event = SessionEvent::LoopDetected {
            pattern: "bash->bash->bash".into(),
            window_size: 3,
        };
    }

    // ── SessionPhase ────────────────────────────────────────────────────

    #[test]
    fn session_phase_active_is_default_for_new_state() {
        let state = SessionState::new("sess_phase".into());
        assert_eq!(state.phase, SessionPhase::Active);
    }

    #[test]
    fn session_phase_awaiting_input_can_be_set() {
        let mut state = SessionState::new("sess_await".into());
        state.phase = SessionPhase::AwaitingInput;
        assert_eq!(state.phase, SessionPhase::AwaitingInput);
    }

    #[test]
    fn session_phase_completed_can_be_set() {
        let mut state = SessionState::new("sess_done".into());
        state.phase = SessionPhase::Completed;
        assert_eq!(state.phase, SessionPhase::Completed);
    }

    #[test]
    fn session_phase_transitions_active_to_awaiting_to_active() {
        let mut state = SessionState::new("sess_trans".into());
        assert_eq!(state.phase, SessionPhase::Active);

        state.phase = SessionPhase::AwaitingInput;
        assert_eq!(state.phase, SessionPhase::AwaitingInput);

        state.phase = SessionPhase::Active;
        assert_eq!(state.phase, SessionPhase::Active);
    }

    #[test]
    fn session_phase_transitions_active_to_completed() {
        let mut state = SessionState::new("sess_end".into());
        state.phase = SessionPhase::Completed;
        assert_eq!(state.phase, SessionPhase::Completed);
        assert!(state.phase != SessionPhase::Active);
        assert!(state.phase != SessionPhase::AwaitingInput);
    }

    #[test]
    fn session_phase_clone_and_copy() {
        let phase = SessionPhase::AwaitingInput;
        let cloned = phase;
        let copied = phase;
        assert_eq!(phase, cloned);
        assert_eq!(phase, copied);
    }

    #[test]
    fn session_phase_debug_format() {
        let phase = SessionPhase::AwaitingInput;
        let debug_str = format!("{:?}", phase);
        assert!(debug_str.contains("AwaitingInput"));
    }

    // ── SessionConfig command timeout fields ────────────────────────────

    #[test]
    fn session_config_with_default_command_timeout_ms() {
        let config = SessionConfig::default().with_default_command_timeout_ms(30_000);
        assert_eq!(config.default_command_timeout_ms, Some(30_000));
    }

    #[test]
    fn session_config_with_max_command_timeout_ms() {
        let config = SessionConfig::default().with_max_command_timeout_ms(120_000);
        assert_eq!(config.max_command_timeout_ms, Some(120_000));
    }

    #[test]
    fn session_config_timeout_fields_independent() {
        let config = SessionConfig::default()
            .with_default_command_timeout_ms(10_000)
            .with_max_command_timeout_ms(60_000);
        assert_eq!(config.default_command_timeout_ms, Some(10_000));
        assert_eq!(config.max_command_timeout_ms, Some(60_000));
    }

    #[test]
    fn session_config_builder_chaining_with_timeouts() {
        let config = SessionConfig::default()
            .with_model("gemini-pro")
            .with_max_turns(25)
            .with_default_command_timeout_ms(5_000)
            .with_max_command_timeout_ms(300_000);

        assert_eq!(config.model, "gemini-pro");
        assert_eq!(config.max_turns, 25);
        assert_eq!(config.default_command_timeout_ms, Some(5_000));
        assert_eq!(config.max_command_timeout_ms, Some(300_000));
    }

    // ── SessionConfig context_window_size ────────────────────────────────

    #[test]
    fn session_config_with_context_window_size() {
        let config = SessionConfig::default().with_context_window_size(200_000);
        assert_eq!(config.context_window_size, Some(200_000));
    }

    #[test]
    fn session_config_context_window_size_chains_with_other_builders() {
        let config = SessionConfig::default()
            .with_model("gpt-4o")
            .with_context_window_size(128_000)
            .with_max_turns(50);
        assert_eq!(config.model, "gpt-4o");
        assert_eq!(config.context_window_size, Some(128_000));
        assert_eq!(config.max_turns, 50);
    }

    // ── SessionEvent::ContextWindowWarning ──────────────────────────────

    #[test]
    fn session_event_context_window_warning() {
        let event = SessionEvent::ContextWindowWarning {
            used: 80_000,
            limit: 100_000,
            fraction: 0.8,
        };
        match event {
            SessionEvent::ContextWindowWarning {
                used,
                limit,
                fraction,
            } => {
                assert_eq!(used, 80_000);
                assert_eq!(limit, 100_000);
                assert!((fraction - 0.8).abs() < f64::EPSILON);
            }
            other => panic!("unexpected event: {:?}", other),
        }
    }

    #[test]
    fn session_event_context_window_warning_clone() {
        let event = SessionEvent::ContextWindowWarning {
            used: 95_000,
            limit: 100_000,
            fraction: 0.95,
        };
        let cloned = event.clone();
        match cloned {
            SessionEvent::ContextWindowWarning {
                used,
                limit,
                fraction,
            } => {
                assert_eq!(used, 95_000);
                assert_eq!(limit, 100_000);
                assert!((fraction - 0.95).abs() < f64::EPSILON);
            }
            other => panic!("unexpected cloned event: {:?}", other),
        }
    }
}
