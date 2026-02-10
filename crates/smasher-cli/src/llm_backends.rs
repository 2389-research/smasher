// ABOUTME: LLM-backed implementations of ManagerBackend and ToolBackend traits.
// ABOUTME: Delegates manager coordination and tool execution to the agent's LLM session.

use std::sync::Arc;

use serde_json::{Value, json};

use smasher_agent::environment::LocalExecutionEnvironment;
use smasher_agent::events::EventEmitter;
use smasher_agent::session::Session;
use smasher_agent::tools::ToolRegistry;
use smasher_agent::tools::shared::register_shared_tools;
use smasher_agent::types::SessionConfig;

use smasher_attractor::handler::HandlerError;
use smasher_attractor::manager_handler::ManagerBackend;
use smasher_attractor::state::{Context, Outcome};
use smasher_attractor::tool_handler::ToolBackend;

/// ManagerBackend that sends coordination tasks to an LLM as prompts.
///
/// Each coordination request creates a fresh agent session, sends the task
/// description as user input, and returns the LLM's response as the outcome.
pub struct LlmManagerBackend {
    client: Arc<smasher_llm::client::Client>,
    model: String,
    working_dir: String,
}

impl LlmManagerBackend {
    pub fn new(
        client: Arc<smasher_llm::client::Client>,
        model: String,
        working_dir: String,
    ) -> Self {
        Self {
            client,
            model,
            working_dir,
        }
    }
}

#[async_trait::async_trait]
impl ManagerBackend for LlmManagerBackend {
    async fn coordinate(
        &self,
        task: &str,
        config: &Value,
        context: &Context,
    ) -> Result<Outcome, HandlerError> {
        let context_summary = context.to_string_map();
        let context_parts: Vec<String> = context_summary
            .iter()
            .filter(|(k, _)| !k.starts_with('_'))
            .map(|(k, v)| format!("{k}: {v}"))
            .collect();

        let system_prompt = format!(
            "You are an AI coordinator managing a pipeline step.\n\
             Task: {task}\n\
             Config: {config}\n\
             \n\
             Pipeline context:\n{}\n\
             \n\
             Analyze the task and provide your coordination response. \
             You have tools for reading files, writing files, editing files, \
             running shell commands, grep, and glob.",
            if context_parts.is_empty() {
                "(empty)".to_string()
            } else {
                context_parts.join("\n")
            }
        );

        let env = Arc::new(LocalExecutionEnvironment::new(self.working_dir.clone()));
        let mut tool_registry = ToolRegistry::new();
        register_shared_tools(&mut tool_registry, env);

        let session_config = SessionConfig::default()
            .with_model(&self.model)
            .with_max_turns(30)
            .with_system_prompt(&system_prompt)
            .with_working_directory(&self.working_dir);

        let emitter = EventEmitter::default();
        let mut session = Session::new(
            session_config,
            Arc::clone(&self.client),
            tool_registry,
            emitter,
        );

        match session.process_input(task).await {
            Ok(output) => {
                let text = output.text.unwrap_or_default();
                Ok(Outcome::success_with(json!({"response": text})))
            }
            Err(e) => Err(HandlerError::Other(format!("Manager session error: {e}"))),
        }
    }
}

/// ToolBackend that delegates tool execution through an LLM agent session.
///
/// Each tool invocation creates a fresh agent session, presents the tool
/// name and arguments as a prompt, and returns the LLM's response.
pub struct LlmToolBackend {
    client: Arc<smasher_llm::client::Client>,
    model: String,
    working_dir: String,
}

impl LlmToolBackend {
    pub fn new(
        client: Arc<smasher_llm::client::Client>,
        model: String,
        working_dir: String,
    ) -> Self {
        Self {
            client,
            model,
            working_dir,
        }
    }
}

#[async_trait::async_trait]
impl ToolBackend for LlmToolBackend {
    async fn execute_tool(
        &self,
        tool_name: &str,
        args: &Value,
        context: &Context,
    ) -> Result<Outcome, HandlerError> {
        let context_summary = context.to_string_map();
        let context_parts: Vec<String> = context_summary
            .iter()
            .filter(|(k, _)| !k.starts_with('_'))
            .map(|(k, v)| format!("{k}: {v}"))
            .collect();

        let system_prompt = format!(
            "You are an AI assistant executing a tool in a pipeline.\n\
             Tool: {tool_name}\n\
             Arguments: {args}\n\
             \n\
             Pipeline context:\n{}\n\
             \n\
             Execute the requested tool operation. You have tools for reading files, \
             writing files, editing files, running shell commands, grep, and glob.",
            if context_parts.is_empty() {
                "(empty)".to_string()
            } else {
                context_parts.join("\n")
            }
        );

        let env = Arc::new(LocalExecutionEnvironment::new(self.working_dir.clone()));
        let mut tool_registry = ToolRegistry::new();
        register_shared_tools(&mut tool_registry, env);

        let session_config = SessionConfig::default()
            .with_model(&self.model)
            .with_max_turns(30)
            .with_system_prompt(&system_prompt)
            .with_working_directory(&self.working_dir);

        let emitter = EventEmitter::default();
        let mut session = Session::new(
            session_config,
            Arc::clone(&self.client),
            tool_registry,
            emitter,
        );

        let prompt = format!("Execute tool '{tool_name}' with arguments: {args}");
        match session.process_input(&prompt).await {
            Ok(output) => {
                let text = output.text.unwrap_or_default();
                Ok(Outcome::success_with(json!({"response": text})))
            }
            Err(e) => Err(HandlerError::Other(format!("Tool session error: {e}"))),
        }
    }

    fn available_tools(&self) -> Vec<String> {
        vec![
            "read_file".into(),
            "write_file".into(),
            "edit_file".into(),
            "shell".into(),
            "grep".into(),
            "glob_files".into(),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn llm_manager_backend_creation() {
        let client = Arc::new(smasher_llm::client::Client::from_env());
        let backend = LlmManagerBackend::new(client, "test-model".into(), "/tmp".into());
        assert_eq!(backend.model, "test-model");
        assert_eq!(backend.working_dir, "/tmp");
    }

    #[test]
    fn llm_tool_backend_creation() {
        let client = Arc::new(smasher_llm::client::Client::from_env());
        let backend = LlmToolBackend::new(client, "test-model".into(), "/tmp".into());
        assert_eq!(backend.model, "test-model");
        assert_eq!(backend.working_dir, "/tmp");
    }

    #[test]
    fn llm_tool_backend_available_tools() {
        let client = Arc::new(smasher_llm::client::Client::from_env());
        let backend = LlmToolBackend::new(client, "test-model".into(), "/tmp".into());
        let tools = backend.available_tools();
        assert_eq!(tools.len(), 6);
        assert!(tools.contains(&"read_file".to_string()));
        assert!(tools.contains(&"shell".to_string()));
    }
}
