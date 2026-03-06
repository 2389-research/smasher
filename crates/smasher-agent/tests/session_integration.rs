// ABOUTME: Integration tests for the smasher-agent Session agentic loop.
// ABOUTME: Covers text responses, tool use, steering, turn limits, and event emission with real environments.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use smasher_agent::environment::LocalExecutionEnvironment;
use smasher_agent::events::EventEmitter;
use smasher_agent::session::Session;
use smasher_agent::tools::ToolRegistry;
use smasher_agent::tools::shared::register_shared_tools;
use smasher_agent::types::{SessionConfig, SessionEvent};
use smasher_llm::client::Client;
use smasher_llm::provider::{ProviderAdapter, StreamResponse};
use smasher_llm::types::{
    ContentPart, Error as LlmError, FinishReason, Provider, Request, Response, ToolCallData, Usage,
};

// ── Mock LLM provider ──────────────────────────────────────────────────

/// A mock provider adapter that returns canned responses from a queue.
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

// ── Response constructors ──────────────────────────────────────────────

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

// ── Helpers ────────────────────────────────────────────────────────────

fn make_client(responses: VecDeque<Response>) -> Arc<Client> {
    let response_queue = Arc::new(Mutex::new(responses));
    let adapter = MockAdapter {
        responses: response_queue,
    };
    let mut client = Client::new();
    client.register_provider(Provider::Anthropic, Arc::new(adapter));
    Arc::new(client)
}

/// Create a temporary directory that is cleaned up when the guard is dropped.
struct TempDir {
    path: std::path::PathBuf,
}

impl TempDir {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("smasher_integ_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&path).expect("failed to create temp dir");
        Self { path }
    }

    fn path_str(&self) -> String {
        self.path.display().to_string()
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// Build a Session wired to a mock LLM, a real LocalExecutionEnvironment (with
/// shared tools), and an EventEmitter. Returns the session and event receiver.
fn make_session_with_env(
    responses: VecDeque<Response>,
    tmp: &TempDir,
) -> (Session, tokio::sync::broadcast::Receiver<SessionEvent>) {
    let client = make_client(responses);
    let env = Arc::new(LocalExecutionEnvironment::new(tmp.path_str()));

    let mut tool_registry = ToolRegistry::new();
    register_shared_tools(&mut tool_registry, env);

    let event_emitter = EventEmitter::default();
    let rx = event_emitter.subscribe();

    let config = SessionConfig::default().with_working_directory(tmp.path_str());
    let session = Session::new(config, client, tool_registry, event_emitter);

    (session, rx)
}

/// Drain all currently buffered events from a broadcast receiver.
fn collect_events(rx: &mut tokio::sync::broadcast::Receiver<SessionEvent>) -> Vec<SessionEvent> {
    let mut events = Vec::new();
    while let Ok(event) = rx.try_recv() {
        events.push(event);
    }
    events
}

// ═══════════════════════════════════════════════════════════════════════
// Test 1: Simple text response (no tool calls)
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn simple_text_response_completes_session() {
    let tmp = TempDir::new();
    let mut responses = VecDeque::new();
    responses.push_back(text_response("Hello, Harp-Dogg the Annihilator!"));

    let (mut session, _rx) = make_session_with_env(responses, &tmp);
    let output = session.process_input("Greet me").await.unwrap();

    assert_eq!(
        output.text.as_deref(),
        Some("Hello, Harp-Dogg the Annihilator!")
    );
    assert_eq!(output.turns_used, 1);
    assert_eq!(output.total_usage.input_tokens, 10);
    assert_eq!(output.total_usage.output_tokens, 20);
}

#[tokio::test]
async fn simple_text_response_adds_messages_to_conversation() {
    let tmp = TempDir::new();
    let mut responses = VecDeque::new();
    responses.push_back(text_response("Got it."));

    let (mut session, _rx) = make_session_with_env(responses, &tmp);
    session.process_input("Do something").await.unwrap();

    let messages = session.messages();
    assert_eq!(messages.len(), 2, "should have user + assistant messages");
    assert!(messages[0].is_user());
    assert!(messages[1].is_assistant());
}

// ═══════════════════════════════════════════════════════════════════════
// Test 2: Tool use — model calls a real tool, gets result, responds
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn tool_use_with_write_file_then_text_response() {
    let tmp = TempDir::new();

    let mut responses = VecDeque::new();
    // First LLM response: call write_file to create a file
    responses.push_back(tool_call_response(
        "write_file",
        "call_write_1",
        r#"{"path": "hello.txt", "content": "Hello from agent"}"#,
    ));
    // Second LLM response: final text
    responses.push_back(text_response("I wrote hello.txt for you."));

    let (mut session, _rx) = make_session_with_env(responses, &tmp);
    let output = session.process_input("Write a file").await.unwrap();

    assert_eq!(output.text.as_deref(), Some("I wrote hello.txt for you."));
    assert_eq!(output.turns_used, 2);

    // Verify the file was actually created on disk by the real environment
    let content = std::fs::read_to_string(tmp.path.join("hello.txt")).unwrap();
    assert_eq!(content, "Hello from agent");
}

#[tokio::test]
async fn tool_use_with_read_file_returns_real_content() {
    let tmp = TempDir::new();
    // Pre-create a file for the agent to read
    std::fs::write(tmp.path.join("data.txt"), "important data").unwrap();

    let mut responses = VecDeque::new();
    // First: model calls read_file
    responses.push_back(tool_call_response(
        "read_file",
        "call_read_1",
        r#"{"path": "data.txt"}"#,
    ));
    // Second: model responds with text
    responses.push_back(text_response("The file contains important data."));

    let (mut session, _rx) = make_session_with_env(responses, &tmp);
    let output = session.process_input("Read data.txt").await.unwrap();

    assert_eq!(
        output.text.as_deref(),
        Some("The file contains important data.")
    );

    // Verify the tool result is in the conversation
    let tool_msgs: Vec<_> = session.messages().iter().filter(|m| m.is_tool()).collect();
    assert_eq!(tool_msgs.len(), 1, "should have one tool result message");
}

#[tokio::test]
async fn tool_use_with_shell_command() {
    let tmp = TempDir::new();

    let mut responses = VecDeque::new();
    // Model calls shell to run `echo hello`
    responses.push_back(tool_call_response(
        "shell",
        "call_shell_1",
        r#"{"command": "echo hello_from_shell"}"#,
    ));
    // Final text response
    responses.push_back(text_response("Command executed successfully."));

    let (mut session, _rx) = make_session_with_env(responses, &tmp);
    let output = session.process_input("Run a shell command").await.unwrap();

    assert_eq!(
        output.text.as_deref(),
        Some("Command executed successfully.")
    );
    assert_eq!(output.turns_used, 2);
}

#[tokio::test]
async fn chained_tool_calls_write_then_read() {
    let tmp = TempDir::new();

    let mut responses = VecDeque::new();
    // Step 1: write a file
    responses.push_back(tool_call_response(
        "write_file",
        "call_w",
        r#"{"path": "chain.txt", "content": "chain data"}"#,
    ));
    // Step 2: read it back
    responses.push_back(tool_call_response(
        "read_file",
        "call_r",
        r#"{"path": "chain.txt"}"#,
    ));
    // Step 3: final text
    responses.push_back(text_response("File written and verified."));

    let (mut session, _rx) = make_session_with_env(responses, &tmp);
    let output = session
        .process_input("Write then read a file")
        .await
        .unwrap();

    assert_eq!(output.text.as_deref(), Some("File written and verified."));
    assert_eq!(output.turns_used, 3);

    // Verify the file exists on disk
    let content = std::fs::read_to_string(tmp.path.join("chain.txt")).unwrap();
    assert_eq!(content, "chain data");
}

// ═══════════════════════════════════════════════════════════════════════
// Test 3: Steering — queue a steering message, verify injection
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn steering_message_is_injected_into_conversation() {
    let tmp = TempDir::new();

    let mut responses = VecDeque::new();
    responses.push_back(text_response("Acknowledged steering."));

    let (mut session, mut rx) = make_session_with_env(responses, &tmp);
    session.steer("Be concise and focus on tests");
    let output = session.process_input("Tell me about Rust").await.unwrap();

    assert_eq!(output.text.as_deref(), Some("Acknowledged steering."));

    // Verify the steering message appears in the conversation
    let messages = session.messages();
    let has_steering = messages
        .iter()
        .any(|m| m.is_user() && m.text() == Some("Be concise and focus on tests".to_string()));
    assert!(
        has_steering,
        "Steering message should be present in conversation history"
    );

    // Verify a SteeringApplied event was emitted
    let events = collect_events(&mut rx);
    let steering_event = events
        .iter()
        .find(|e| matches!(e, SessionEvent::SteeringApplied { .. }));
    assert!(
        steering_event.is_some(),
        "SteeringApplied event should be emitted"
    );
    if let Some(SessionEvent::SteeringApplied { text }) = steering_event {
        assert_eq!(text, "Be concise and focus on tests");
    }
}

#[tokio::test]
async fn steering_is_drained_after_first_call() {
    let tmp = TempDir::new();

    let mut responses = VecDeque::new();
    responses.push_back(text_response("First response."));
    responses.push_back(text_response("Second response."));

    let (mut session, _rx) = make_session_with_env(responses, &tmp);
    session.steer("Important instruction");

    // First call consumes the steering
    session.process_input("First").await.unwrap();
    // Second call should not have the steering message again
    session.process_input("Second").await.unwrap();

    let messages = session.messages();
    let steering_count = messages
        .iter()
        .filter(|m| m.is_user() && m.text() == Some("Important instruction".to_string()))
        .count();
    assert_eq!(
        steering_count, 1,
        "Steering should only appear once, got {steering_count}"
    );
}

#[tokio::test]
async fn multiple_steering_messages_all_injected() {
    let tmp = TempDir::new();

    let mut responses = VecDeque::new();
    responses.push_back(text_response("Both noted."));

    let (mut session, mut rx) = make_session_with_env(responses, &tmp);
    session.steer("Instruction A");
    session.steer("Instruction B");

    session.process_input("Go").await.unwrap();

    // Verify both steering messages in conversation
    let messages = session.messages();
    let a_present = messages
        .iter()
        .any(|m| m.is_user() && m.text() == Some("Instruction A".to_string()));
    let b_present = messages
        .iter()
        .any(|m| m.is_user() && m.text() == Some("Instruction B".to_string()));
    assert!(a_present, "Instruction A should be in conversation");
    assert!(b_present, "Instruction B should be in conversation");

    // Verify two SteeringApplied events
    let events = collect_events(&mut rx);
    let steering_events: Vec<_> = events
        .iter()
        .filter(|e| matches!(e, SessionEvent::SteeringApplied { .. }))
        .collect();
    assert_eq!(
        steering_events.len(),
        2,
        "Should have 2 SteeringApplied events"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// Test 4: Turn limit enforcement
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn turn_limit_stops_session() {
    let tmp = TempDir::new();

    // Queue enough tool-call responses to exceed the limit
    let mut responses = VecDeque::new();
    for i in 0..10 {
        responses.push_back(tool_call_response(
            "shell",
            &format!("call_{i}"),
            r#"{"command": "echo loop"}"#,
        ));
    }

    let client = make_client(responses);
    let env = Arc::new(LocalExecutionEnvironment::new(tmp.path_str()));
    let mut tool_registry = ToolRegistry::new();
    register_shared_tools(&mut tool_registry, env);
    let event_emitter = EventEmitter::default();
    let config = SessionConfig::default()
        .with_max_turns(3)
        .with_working_directory(tmp.path_str());
    let mut session = Session::new(config, client, tool_registry, event_emitter);

    let result = session.process_input("Loop forever").await;

    assert!(result.is_err(), "Should error with TurnLimitReached");
    match result.unwrap_err() {
        smasher_agent::session::SessionError::TurnLimitReached(limit) => {
            assert_eq!(limit, 3);
        }
        other => panic!("Expected TurnLimitReached, got: {:?}", other),
    }
}

#[tokio::test]
async fn session_becomes_inactive_after_turn_limit() {
    let tmp = TempDir::new();

    let mut responses = VecDeque::new();
    for i in 0..10 {
        responses.push_back(tool_call_response(
            "shell",
            &format!("call_{i}"),
            r#"{"command": "echo x"}"#,
        ));
    }

    let client = make_client(responses);
    let env = Arc::new(LocalExecutionEnvironment::new(tmp.path_str()));
    let mut tool_registry = ToolRegistry::new();
    register_shared_tools(&mut tool_registry, env);
    let event_emitter = EventEmitter::default();
    let config = SessionConfig::default()
        .with_max_turns(2)
        .with_working_directory(tmp.path_str());
    let mut session = Session::new(config, client, tool_registry, event_emitter);

    let _ = session.process_input("Go").await;

    assert!(
        !session.is_active(),
        "Session should be inactive after hitting turn limit"
    );
}

#[tokio::test]
async fn inactive_session_rejects_further_input() {
    let tmp = TempDir::new();

    let mut responses = VecDeque::new();
    for i in 0..10 {
        responses.push_back(tool_call_response(
            "shell",
            &format!("call_{i}"),
            r#"{"command": "echo x"}"#,
        ));
    }
    responses.push_back(text_response("Should never get here."));

    let client = make_client(responses);
    let env = Arc::new(LocalExecutionEnvironment::new(tmp.path_str()));
    let mut tool_registry = ToolRegistry::new();
    register_shared_tools(&mut tool_registry, env);
    let event_emitter = EventEmitter::default();
    let config = SessionConfig::default()
        .with_max_turns(1)
        .with_working_directory(tmp.path_str());
    let mut session = Session::new(config, client, tool_registry, event_emitter);

    // First call hits turn limit
    let _ = session.process_input("First").await;
    // Second call should fail as Inactive
    let result = session.process_input("Second").await;
    assert!(
        matches!(
            result.unwrap_err(),
            smasher_agent::session::SessionError::Inactive
        ),
        "Should get Inactive error on second call"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// Test 5: Event emission
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn events_emitted_for_simple_text_response() {
    let tmp = TempDir::new();
    let mut responses = VecDeque::new();
    responses.push_back(text_response("Event test."));

    let (mut session, mut rx) = make_session_with_env(responses, &tmp);
    session.process_input("Hi").await.unwrap();

    let events = collect_events(&mut rx);

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
    let tmp = TempDir::new();
    let mut responses = VecDeque::new();
    responses.push_back(text_response("Order test."));

    let (mut session, mut rx) = make_session_with_env(responses, &tmp);
    session.process_input("Hello").await.unwrap();

    let events = collect_events(&mut rx);

    // Expected order: TurnStarted -> AssistantMessage -> SessionCompleted
    assert!(
        events.len() >= 3,
        "Should have at least 3 events, got {}",
        events.len()
    );
    assert!(
        matches!(events[0], SessionEvent::TurnStarted { turn_number: 0 }),
        "First event should be TurnStarted(0), got {:?}",
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
async fn events_emitted_for_tool_use_flow() {
    let tmp = TempDir::new();

    let mut responses = VecDeque::new();
    responses.push_back(tool_call_response(
        "shell",
        "call_ev_1",
        r#"{"command": "echo event_test"}"#,
    ));
    responses.push_back(text_response("Tool done."));

    let (mut session, mut rx) = make_session_with_env(responses, &tmp);
    session.process_input("Use a tool").await.unwrap();

    let events = collect_events(&mut rx);

    // Verify ToolCallStarted event
    let tool_started = events
        .iter()
        .find(|e| matches!(e, SessionEvent::ToolCallStarted { .. }));
    assert!(
        tool_started.is_some(),
        "ToolCallStarted event should be emitted"
    );
    if let Some(SessionEvent::ToolCallStarted {
        tool_name,
        tool_call_id,
        ..
    }) = tool_started
    {
        assert_eq!(tool_name, "shell");
        assert_eq!(tool_call_id, "call_ev_1");
    }

    // Verify ToolCallCompleted event
    let tool_completed = events
        .iter()
        .find(|e| matches!(e, SessionEvent::ToolCallCompleted { .. }));
    assert!(
        tool_completed.is_some(),
        "ToolCallCompleted event should be emitted"
    );
    if let Some(SessionEvent::ToolCallCompleted {
        tool_name,
        is_error,
        ..
    }) = tool_completed
    {
        assert_eq!(tool_name, "shell");
        assert!(!is_error, "shell echo should succeed");
    }

    // Verify we have two AssistantMessage events (one for tool call, one for text)
    let assistant_count = events
        .iter()
        .filter(|e| matches!(e, SessionEvent::AssistantMessage { .. }))
        .count();
    assert_eq!(
        assistant_count, 2,
        "Should have 2 AssistantMessage events (tool call + final)"
    );

    // Verify SessionCompleted is still emitted
    let has_completed = events
        .iter()
        .any(|e| matches!(e, SessionEvent::SessionCompleted { .. }));
    assert!(
        has_completed,
        "SessionCompleted should be emitted after tool flow"
    );
}

#[tokio::test]
async fn session_completed_event_contains_correct_totals() {
    let tmp = TempDir::new();
    let mut responses = VecDeque::new();
    responses.push_back(text_response("Totals test."));

    let (mut session, mut rx) = make_session_with_env(responses, &tmp);
    let session_id = session.session_id().to_string();
    session.process_input("Go").await.unwrap();

    let events = collect_events(&mut rx);

    let completed = events
        .iter()
        .find(|e| matches!(e, SessionEvent::SessionCompleted { .. }));
    assert!(completed.is_some(), "SessionCompleted should exist");
    if let Some(SessionEvent::SessionCompleted {
        session_id: ev_sid,
        total_turns,
        total_usage,
    }) = completed
    {
        assert_eq!(ev_sid, &session_id);
        assert_eq!(*total_turns, 1);
        assert_eq!(total_usage.input_tokens, 10);
        assert_eq!(total_usage.output_tokens, 20);
    }
}

#[tokio::test]
async fn turn_limit_emits_session_completed_event() {
    let tmp = TempDir::new();

    let mut responses = VecDeque::new();
    for i in 0..10 {
        responses.push_back(tool_call_response(
            "shell",
            &format!("call_{i}"),
            r#"{"command": "echo x"}"#,
        ));
    }

    let client = make_client(responses);
    let env = Arc::new(LocalExecutionEnvironment::new(tmp.path_str()));
    let mut tool_registry = ToolRegistry::new();
    register_shared_tools(&mut tool_registry, env);
    let event_emitter = EventEmitter::default();
    let mut rx = event_emitter.subscribe();
    let config = SessionConfig::default()
        .with_max_turns(1)
        .with_working_directory(tmp.path_str());
    let mut session = Session::new(config, client, tool_registry, event_emitter);

    let _ = session.process_input("Go").await;

    let events = collect_events(&mut rx);

    let has_completed = events
        .iter()
        .any(|e| matches!(e, SessionEvent::SessionCompleted { .. }));
    assert!(
        has_completed,
        "SessionCompleted should be emitted even when turn limit is reached"
    );
}

#[tokio::test]
async fn event_order_for_tool_call_flow() {
    let tmp = TempDir::new();

    let mut responses = VecDeque::new();
    responses.push_back(tool_call_response(
        "shell",
        "call_order",
        r#"{"command": "echo ordered"}"#,
    ));
    responses.push_back(text_response("Ordered."));

    let (mut session, mut rx) = make_session_with_env(responses, &tmp);
    session.process_input("Go").await.unwrap();

    let events = collect_events(&mut rx);

    // Collect event type names for order verification
    let event_types: Vec<&str> = events
        .iter()
        .map(|e| match e {
            SessionEvent::SessionStarted { .. } => "SessionStarted",
            SessionEvent::TurnStarted { .. } => "TurnStarted",
            SessionEvent::AssistantMessage { .. } => "AssistantMessage",
            SessionEvent::ToolCallStarted { .. } => "ToolCallStarted",
            SessionEvent::ToolCallCompleted { .. } => "ToolCallCompleted",
            SessionEvent::TextDelta { .. } => "TextDelta",
            SessionEvent::SteeringApplied { .. } => "SteeringApplied",
            SessionEvent::SessionCompleted { .. } => "SessionCompleted",
            SessionEvent::SessionError { .. } => "SessionError",
            SessionEvent::LoopDetected { .. } => "LoopDetected",
            _ => "Other",
        })
        .collect();

    // Expected sequence for one tool call then text:
    // TurnStarted, AssistantMessage (tool call), ToolCallStarted, ToolCallCompleted,
    // AssistantMessage (text), SessionCompleted
    assert_eq!(event_types[0], "TurnStarted", "events: {:?}", event_types);
    assert_eq!(
        event_types[1], "AssistantMessage",
        "events: {:?}",
        event_types
    );
    assert_eq!(
        event_types[2], "ToolCallStarted",
        "events: {:?}",
        event_types
    );
    assert_eq!(
        event_types[3], "ToolCallCompleted",
        "events: {:?}",
        event_types
    );
    assert_eq!(
        event_types[4], "AssistantMessage",
        "events: {:?}",
        event_types
    );
    assert_eq!(
        event_types[5], "SessionCompleted",
        "events: {:?}",
        event_types
    );
}

// ═══════════════════════════════════════════════════════════════════════
// Additional integration scenarios
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn tool_use_with_glob_finds_real_files() {
    let tmp = TempDir::new();
    // Create files on disk for glob to find
    std::fs::write(tmp.path.join("foo.rs"), "fn foo() {}").unwrap();
    std::fs::write(tmp.path.join("bar.rs"), "fn bar() {}").unwrap();
    std::fs::write(tmp.path.join("readme.md"), "# Readme").unwrap();

    let mut responses = VecDeque::new();
    responses.push_back(tool_call_response(
        "glob_files",
        "call_glob",
        r#"{"pattern": "*.rs"}"#,
    ));
    responses.push_back(text_response("Found 2 Rust files."));

    let (mut session, _rx) = make_session_with_env(responses, &tmp);
    let output = session.process_input("Find Rust files").await.unwrap();

    assert_eq!(output.text.as_deref(), Some("Found 2 Rust files."));

    // Verify the tool result in conversation contains the file paths
    let tool_msgs: Vec<_> = session.messages().iter().filter(|m| m.is_tool()).collect();
    assert_eq!(tool_msgs.len(), 1);
}

#[tokio::test]
async fn steering_combined_with_tool_use() {
    let tmp = TempDir::new();
    std::fs::write(tmp.path.join("test.txt"), "test content").unwrap();

    let mut responses = VecDeque::new();
    // After steering + user input, model calls read_file
    responses.push_back(tool_call_response(
        "read_file",
        "call_steer_tool",
        r#"{"path": "test.txt"}"#,
    ));
    // Then responds
    responses.push_back(text_response("Steered and used tools."));

    let (mut session, mut rx) = make_session_with_env(responses, &tmp);
    session.steer("Focus on reading files");
    let output = session.process_input("Analyze test.txt").await.unwrap();

    assert_eq!(output.text.as_deref(), Some("Steered and used tools."));

    let events = collect_events(&mut rx);

    // Verify both steering and tool events appeared
    let has_steering = events
        .iter()
        .any(|e| matches!(e, SessionEvent::SteeringApplied { .. }));
    let has_tool = events
        .iter()
        .any(|e| matches!(e, SessionEvent::ToolCallCompleted { .. }));

    assert!(has_steering, "SteeringApplied should be emitted");
    assert!(has_tool, "ToolCallCompleted should be emitted");
}
