// ABOUTME: Backend implementations for web-based pipeline execution handlers.
// ABOUTME: Provides AgentCodergenBackend, LlmManagerBackend, and LlmToolBackend for the web server.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use smasher_agent::environment::LocalExecutionEnvironment;
use smasher_agent::events::EventEmitter;
use smasher_agent::session::Session;
use smasher_agent::tools::ToolRegistry;
use smasher_agent::tools::shared::register_shared_tools;
use smasher_agent::types::{SessionConfig, SessionEvent};

use smasher_attractor::events::{PipelineEvent, PipelineEventEmitter};
use smasher_attractor::handler::{CodergenBackend, HandlerError};
use smasher_attractor::manager_handler::ManagerBackend;
use smasher_attractor::state::{Context, Outcome};
use smasher_attractor::tool_handler::ToolBackend;

/// CodergenBackend that runs a full agent session with file/shell tools.
///
/// Each codergen node invocation creates a fresh agent Session with all six
/// shared tools, sends the node prompt as user input, and lets the LLM drive
/// tool use until the task is complete.
pub struct AgentCodergenBackend {
    client: Arc<smasher_llm::client::Client>,
    default_model: String,
    working_dir: String,
    input_tokens: Arc<AtomicU64>,
    output_tokens: Arc<AtomicU64>,
    pipeline_emitter: Arc<PipelineEventEmitter>,
}

impl AgentCodergenBackend {
    pub fn new(
        client: Arc<smasher_llm::client::Client>,
        default_model: String,
        working_dir: String,
        input_tokens: Arc<AtomicU64>,
        output_tokens: Arc<AtomicU64>,
        pipeline_emitter: Arc<PipelineEventEmitter>,
    ) -> Self {
        Self {
            client,
            default_model,
            working_dir,
            input_tokens,
            output_tokens,
            pipeline_emitter,
        }
    }
}

/// Truncate a string to at most `max_len` chars, appending "..." if truncated.
fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        let mut truncated: String = s.chars().take(max_len).collect();
        truncated.push_str("...");
        truncated
    }
}

#[async_trait::async_trait]
impl CodergenBackend for AgentCodergenBackend {
    async fn generate(
        &self,
        prompt: &str,
        model: Option<&str>,
        context: &Context,
    ) -> Result<Outcome, HandlerError> {
        let model_id = model.unwrap_or(&self.default_model);

        // Build context summary from pipeline state for the system prompt.
        let context_summary = context.to_string_map();
        let system_parts: Vec<String> = context_summary
            .iter()
            .filter(|(k, _)| !k.starts_with('_'))
            .map(|(k, v)| format!("{k}: {v}"))
            .collect();

        let system_prompt = if system_parts.is_empty() {
            "You are an AI coding assistant executing a pipeline step. You have tools for reading files, writing files, editing files, running shell commands, grep, and glob. Use them to complete the task. Write all files in the current working directory.".to_string()
        } else {
            format!(
                "You are an AI coding assistant executing a pipeline step. Pipeline context:\n{}\n\nYou have tools for reading files, writing files, editing files, running shell commands, grep, and glob. Use them to complete the task. Write all files in the current working directory.",
                system_parts.join("\n")
            )
        };

        // Create a fresh agent session with all shared tools.
        let env = Arc::new(LocalExecutionEnvironment::new(self.working_dir.clone()));
        let mut tool_registry = ToolRegistry::new();
        register_shared_tools(&mut tool_registry, env);

        let session_emitter = EventEmitter::default();

        // Read the current node id from context so we can tag forwarded events.
        let node_id = context
            .get_string("_current_node_id")
            .unwrap_or_else(|| "unknown".to_string());

        // Subscribe to session events and forward them as pipeline events.
        // Clone token counters into the bridge so we can update them incrementally
        // as each LLM response arrives, rather than waiting for the entire session.
        let mut session_rx = session_emitter.subscribe();
        let pipeline_emitter = Arc::clone(&self.pipeline_emitter);
        let bridge_node_id = node_id.clone();
        let bridge_input_tokens = Arc::clone(&self.input_tokens);
        let bridge_output_tokens = Arc::clone(&self.output_tokens);
        tokio::spawn(async move {
            loop {
                match session_rx.recv().await {
                    Ok(session_event) => {
                        let pipeline_event = match session_event {
                            SessionEvent::ToolCallStarted {
                                tool_name,
                                tool_call_id,
                                input_preview,
                            } => Some(PipelineEvent::AgentToolCallStarted {
                                node_id: bridge_node_id.clone(),
                                tool_name,
                                tool_call_id,
                                input_preview,
                                timestamp: chrono::Utc::now(),
                            }),
                            SessionEvent::ToolCallCompleted {
                                tool_name,
                                tool_call_id,
                                result,
                                is_error,
                                duration_ms,
                            } => Some(PipelineEvent::AgentToolCallCompleted {
                                node_id: bridge_node_id.clone(),
                                tool_name,
                                tool_call_id,
                                duration_ms,
                                is_error,
                                result_preview: truncate(&result, 120),
                                timestamp: chrono::Utc::now(),
                            }),
                            SessionEvent::TurnStarted { turn_number } => {
                                Some(PipelineEvent::AgentTurnStarted {
                                    node_id: bridge_node_id.clone(),
                                    turn_number,
                                    timestamp: chrono::Utc::now(),
                                })
                            }
                            SessionEvent::TextDelta { text } => {
                                // Only forward substantial text, not single-char deltas.
                                if text.len() > 10 {
                                    Some(PipelineEvent::AgentMessage {
                                        node_id: bridge_node_id.clone(),
                                        text: truncate(&text, 200),
                                        timestamp: chrono::Utc::now(),
                                    })
                                } else {
                                    None
                                }
                            }
                            SessionEvent::AssistantMessage { response } => {
                                bridge_input_tokens.fetch_add(
                                    response.usage.input_tokens as u64,
                                    Ordering::Relaxed,
                                );
                                bridge_output_tokens.fetch_add(
                                    response.usage.output_tokens as u64,
                                    Ordering::Relaxed,
                                );
                                None
                            }
                            // Skip events that don't need forwarding.
                            _ => None,
                        };
                        if let Some(pe) = pipeline_event {
                            pipeline_emitter.emit(pe);
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!(missed = n, "session event bridge lagged");
                    }
                }
            }
        });

        let config = SessionConfig::default()
            .with_model(model_id)
            .with_max_turns(50)
            .with_system_prompt(&system_prompt)
            .with_working_directory(&self.working_dir);

        let mut session = Session::new(
            config,
            Arc::clone(&self.client),
            tool_registry,
            session_emitter,
        );

        match session.process_input(prompt).await {
            Ok(output) => {
                // Token counters are updated incrementally by the event bridge
                // as each AssistantMessage arrives, so no bulk update needed here.
                let text = output.text.unwrap_or_default();

                tracing::info!(
                    model = model_id,
                    turns = output.turns_used,
                    input_tokens = output.total_usage.input_tokens,
                    output_tokens = output.total_usage.output_tokens,
                    "codergen node completed"
                );

                Ok(Outcome::success_with(serde_json::json!({"response": text})))
            }
            Err(e) => Err(HandlerError::Other(format!("Agent session error: {e}"))),
        }
    }
}

// ---------------------------------------------------------------------------
// LlmManagerBackend
// ---------------------------------------------------------------------------

/// ManagerBackend that delegates coordination tasks to an LLM agent session.
///
/// Each coordination invocation creates a fresh agent Session, sends the task
/// description as a user prompt, and returns the LLM's response as the outcome.
pub struct LlmManagerBackend {
    client: Arc<smasher_llm::client::Client>,
    default_model: String,
    working_dir: String,
}

impl LlmManagerBackend {
    /// Create a new LlmManagerBackend with the given LLM client and model.
    pub fn new(
        client: Arc<smasher_llm::client::Client>,
        default_model: String,
        working_dir: String,
    ) -> Self {
        Self {
            client,
            default_model,
            working_dir,
        }
    }
}

#[async_trait::async_trait]
impl ManagerBackend for LlmManagerBackend {
    async fn coordinate(
        &self,
        task: &str,
        config: &serde_json::Value,
        context: &Context,
    ) -> Result<Outcome, HandlerError> {
        // Build a prompt from the task, config, and any relevant context.
        let context_summary = context.to_string_map();
        let context_lines: Vec<String> = context_summary
            .iter()
            .filter(|(k, _)| !k.starts_with('_'))
            .map(|(k, v)| format!("{k}: {v}"))
            .collect();

        let prompt = if context_lines.is_empty() {
            format!("Task: {task}\nConfig: {config}")
        } else {
            format!(
                "Task: {task}\nConfig: {config}\nContext:\n{}",
                context_lines.join("\n")
            )
        };

        let system_prompt = "You are an AI coordination agent executing a manager task in a pipeline. Analyze the task, consider the configuration, and produce a clear result.";

        let env = Arc::new(LocalExecutionEnvironment::new(self.working_dir.clone()));
        let mut tool_registry = ToolRegistry::new();
        register_shared_tools(&mut tool_registry, env);

        let session_config = SessionConfig::default()
            .with_model(&self.default_model)
            .with_max_turns(20)
            .with_system_prompt(system_prompt)
            .with_working_directory(&self.working_dir);

        let session_emitter = EventEmitter::default();
        let mut session = Session::new(
            session_config,
            Arc::clone(&self.client),
            tool_registry,
            session_emitter,
        );

        match session.process_input(&prompt).await {
            Ok(output) => {
                let text = output.text.unwrap_or_default();
                tracing::info!(
                    model = %self.default_model,
                    turns = output.turns_used,
                    "manager coordination completed"
                );
                Ok(Outcome::success_with(serde_json::json!({"response": text})))
            }
            Err(e) => Err(HandlerError::Other(format!(
                "Manager agent session error: {e}"
            ))),
        }
    }
}

// ---------------------------------------------------------------------------
// LlmToolBackend
// ---------------------------------------------------------------------------

/// ToolBackend that delegates tool execution to an LLM agent session.
///
/// Each tool invocation creates a fresh agent Session, sends the tool name
/// and arguments as a user prompt, and returns the LLM's response as the outcome.
pub struct LlmToolBackend {
    client: Arc<smasher_llm::client::Client>,
    default_model: String,
    working_dir: String,
}

impl LlmToolBackend {
    /// Create a new LlmToolBackend with the given LLM client and model.
    pub fn new(
        client: Arc<smasher_llm::client::Client>,
        default_model: String,
        working_dir: String,
    ) -> Self {
        Self {
            client,
            default_model,
            working_dir,
        }
    }
}

#[async_trait::async_trait]
impl ToolBackend for LlmToolBackend {
    async fn execute_tool(
        &self,
        tool_name: &str,
        args: &serde_json::Value,
        context: &Context,
    ) -> Result<Outcome, HandlerError> {
        // Build a prompt from the tool name, args, and any relevant context.
        let context_summary = context.to_string_map();
        let context_lines: Vec<String> = context_summary
            .iter()
            .filter(|(k, _)| !k.starts_with('_'))
            .map(|(k, v)| format!("{k}: {v}"))
            .collect();

        let prompt = if context_lines.is_empty() {
            format!("Execute tool '{tool_name}' with arguments: {args}")
        } else {
            format!(
                "Execute tool '{tool_name}' with arguments: {args}\nContext:\n{}",
                context_lines.join("\n")
            )
        };

        let system_prompt = "You are an AI tool executor in a pipeline. Execute the specified tool operation and return the results.";

        let env = Arc::new(LocalExecutionEnvironment::new(self.working_dir.clone()));
        let mut tool_registry = ToolRegistry::new();
        register_shared_tools(&mut tool_registry, env);

        let session_config = SessionConfig::default()
            .with_model(&self.default_model)
            .with_max_turns(20)
            .with_system_prompt(system_prompt)
            .with_working_directory(&self.working_dir);

        let session_emitter = EventEmitter::default();
        let mut session = Session::new(
            session_config,
            Arc::clone(&self.client),
            tool_registry,
            session_emitter,
        );

        match session.process_input(&prompt).await {
            Ok(output) => {
                let text = output.text.unwrap_or_default();
                tracing::info!(
                    model = %self.default_model,
                    turns = output.turns_used,
                    "tool execution completed"
                );
                Ok(Outcome::success_with(serde_json::json!({"response": text})))
            }
            Err(e) => Err(HandlerError::Other(format!(
                "Tool agent session error: {e}"
            ))),
        }
    }

    fn available_tools(&self) -> Vec<String> {
        // LLM-backed tool executor can handle any tool via prompt.
        vec![]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_creation() {
        let client = Arc::new(smasher_llm::client::Client::from_env());
        let emitter = Arc::new(PipelineEventEmitter::default());
        let backend = AgentCodergenBackend::new(
            client,
            "claude-sonnet-4-20250514".into(),
            "/tmp".into(),
            Arc::new(AtomicU64::new(0)),
            Arc::new(AtomicU64::new(0)),
            emitter,
        );
        assert_eq!(backend.default_model, "claude-sonnet-4-20250514");
        assert_eq!(backend.working_dir, "/tmp");
    }

    #[test]
    fn truncate_short_string_unchanged() {
        assert_eq!(truncate("hello", 10), "hello");
    }

    #[test]
    fn truncate_long_string_adds_ellipsis() {
        let long = "a".repeat(150);
        let result = truncate(&long, 120);
        assert!(result.ends_with("..."));
        // 120 chars + "..." = 123 chars
        assert_eq!(result.len(), 123);
    }

    #[test]
    fn truncate_exact_length_unchanged() {
        let exact = "a".repeat(120);
        assert_eq!(truncate(&exact, 120), exact);
    }

    #[test]
    fn manager_backend_creation() {
        let client = Arc::new(smasher_llm::client::Client::from_env());
        let backend =
            LlmManagerBackend::new(client, "claude-sonnet-4-20250514".into(), "/tmp".into());
        assert_eq!(backend.default_model, "claude-sonnet-4-20250514");
        assert_eq!(backend.working_dir, "/tmp");
    }

    #[test]
    fn tool_backend_creation() {
        let client = Arc::new(smasher_llm::client::Client::from_env());
        let backend = LlmToolBackend::new(client, "claude-sonnet-4-20250514".into(), "/tmp".into());
        assert_eq!(backend.default_model, "claude-sonnet-4-20250514");
        assert_eq!(backend.working_dir, "/tmp");
    }

    #[test]
    fn tool_backend_available_tools_empty() {
        let client = Arc::new(smasher_llm::client::Client::from_env());
        let backend = LlmToolBackend::new(client, "test".into(), "/tmp".into());
        assert!(backend.available_tools().is_empty());
    }
}
