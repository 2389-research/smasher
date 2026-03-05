// ABOUTME: Tool infrastructure providing registration, lookup, and execution of agent tools.
// ABOUTME: Defines the AgentTool trait, ToolOutput, and ToolRegistry for managing callable tools.

pub mod apply_patch;
pub mod shared;
pub mod truncation;

use std::collections::HashMap;

use async_trait::async_trait;
use smasher_llm::types::ToolDefinition;

use truncation::truncate_output;

/// Output produced by executing a tool.
#[derive(Debug, Clone)]
pub struct ToolOutput {
    /// The tool's output content.
    pub content: String,
    /// Whether the execution resulted in an error.
    pub is_error: bool,
    /// Execution duration in milliseconds.
    pub duration_ms: u64,
}

impl ToolOutput {
    pub fn success(content: impl Into<String>, duration_ms: u64) -> Self {
        Self {
            content: content.into(),
            is_error: false,
            duration_ms,
        }
    }

    pub fn error(content: impl Into<String>, duration_ms: u64) -> Self {
        Self {
            content: content.into(),
            is_error: true,
            duration_ms,
        }
    }
}

/// A tool that can be invoked by the agent during execution.
#[async_trait]
pub trait AgentTool: Send + Sync {
    /// The unique name of this tool.
    fn name(&self) -> &str;

    /// A description of what this tool does.
    fn description(&self) -> &str;

    /// The JSON Schema for this tool's parameters.
    fn parameters_schema(&self) -> serde_json::Value;

    /// Execute the tool with the given JSON arguments.
    async fn execute(&self, arguments: &str) -> ToolOutput;
}

/// Default output character limit for tools not listed in the per-tool map.
const DEFAULT_OUTPUT_LIMIT: usize = 100_000;

/// Spec-defined per-tool output truncation limits.
fn default_tool_output_limits() -> HashMap<String, usize> {
    let mut m = HashMap::new();
    m.insert("read_file".to_string(), 50_000);
    m.insert("shell".to_string(), 30_000);
    m.insert("grep".to_string(), 20_000);
    m.insert("glob_files".to_string(), 20_000);
    m.insert("edit_file".to_string(), 10_000);
    m.insert("write_file".to_string(), 10_000);
    m.insert("apply_patch".to_string(), 10_000);
    m
}

/// Registry that holds available tools and dispatches execution requests.
pub struct ToolRegistry {
    tools: HashMap<String, Box<dyn AgentTool>>,
    /// Maximum output length in bytes before truncation is applied.
    max_output_chars: usize,
    /// Per-tool output character limits. Tools not in this map use `max_output_chars`.
    tool_output_limits: HashMap<String, usize>,
}

impl ToolRegistry {
    /// Create a new empty registry with spec-defined per-tool output limits.
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
            max_output_chars: DEFAULT_OUTPUT_LIMIT,
            tool_output_limits: default_tool_output_limits(),
        }
    }

    /// Set the default maximum output character limit (builder pattern).
    /// This only affects tools without a per-tool limit.
    pub fn with_max_output_chars(mut self, max: usize) -> Self {
        self.max_output_chars = max;
        self
    }

    /// Register a tool. Replaces any existing tool with the same name.
    pub fn register(&mut self, tool: impl AgentTool + 'static) {
        self.tools.insert(tool.name().to_string(), Box::new(tool));
    }

    /// Look up a tool by name.
    pub fn get(&self, name: &str) -> Option<&dyn AgentTool> {
        self.tools.get(name).map(|t| t.as_ref())
    }

    /// Check whether a tool with the given name is registered.
    pub fn has_tool(&self, name: &str) -> bool {
        self.tools.contains_key(name)
    }

    /// Return the names of all registered tools.
    pub fn tool_names(&self) -> Vec<&str> {
        self.tools.keys().map(|k| k.as_str()).collect()
    }

    /// Generate LLM-compatible tool definitions for all registered tools.
    pub fn tool_definitions(&self) -> Vec<ToolDefinition> {
        self.tools
            .values()
            .map(|tool| {
                ToolDefinition::new(tool.name(), tool.description(), tool.parameters_schema())
            })
            .collect()
    }

    /// Return the output character limit for a specific tool.
    /// Falls back to the default limit if no per-tool limit is configured.
    pub fn output_limit_for(&self, tool_name: &str) -> usize {
        self.tool_output_limits
            .get(tool_name)
            .copied()
            .unwrap_or(self.max_output_chars)
    }

    /// Execute a tool by name, applying per-tool output truncation if needed.
    ///
    /// Returns an error `ToolOutput` if the tool is not found.
    pub async fn execute(&self, name: &str, arguments: &str) -> ToolOutput {
        let tool = match self.tools.get(name) {
            Some(t) => t,
            None => {
                return ToolOutput::error(format!("tool '{}' not found", name), 0);
            }
        };

        let mut output = tool.execute(arguments).await;

        let limit = self.output_limit_for(name);
        if output.content.len() > limit {
            output.content = truncate_output(&output.content, limit);
        }

        output
    }

    /// Return the number of registered tools.
    pub fn len(&self) -> usize {
        self.tools.len()
    }

    /// Return true if no tools are registered.
    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Simple test tool that echoes its input.
    struct EchoTool;

    #[async_trait]
    impl AgentTool for EchoTool {
        fn name(&self) -> &str {
            "echo"
        }

        fn description(&self) -> &str {
            "Echoes input"
        }

        fn parameters_schema(&self) -> serde_json::Value {
            json!({
                "type": "object",
                "properties": { "text": { "type": "string" } }
            })
        }

        async fn execute(&self, arguments: &str) -> ToolOutput {
            let v: serde_json::Value = serde_json::from_str(arguments).unwrap_or_default();
            let text = v["text"].as_str().unwrap_or("no text");
            ToolOutput::success(text, 1)
        }
    }

    /// Test tool that produces output of a specific length.
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
            json!({ "type": "object", "properties": {} })
        }

        async fn execute(&self, _arguments: &str) -> ToolOutput {
            let content = "x".repeat(self.output_size);
            ToolOutput::success(content, 5)
        }
    }

    /// Test tool that produces output of a configurable size and has a configurable name.
    struct NamedBigOutputTool {
        tool_name: &'static str,
        output_size: usize,
    }

    #[async_trait]
    impl AgentTool for NamedBigOutputTool {
        fn name(&self) -> &str {
            self.tool_name
        }

        fn description(&self) -> &str {
            "Produces large output with a custom name"
        }

        fn parameters_schema(&self) -> serde_json::Value {
            json!({ "type": "object", "properties": {} })
        }

        async fn execute(&self, _arguments: &str) -> ToolOutput {
            let content = "x".repeat(self.output_size);
            ToolOutput::success(content, 5)
        }
    }

    #[test]
    fn tool_registry_new_has_no_tools() {
        let registry = ToolRegistry::new();
        assert!(registry.tool_names().is_empty());
    }

    #[test]
    fn register_and_look_up_tool() {
        let mut registry = ToolRegistry::new();
        registry.register(EchoTool);

        let tool = registry.get("echo");
        assert!(tool.is_some());
        assert_eq!(tool.unwrap().name(), "echo");
    }

    #[test]
    fn has_tool_returns_true_and_false() {
        let mut registry = ToolRegistry::new();
        assert!(!registry.has_tool("echo"));

        registry.register(EchoTool);
        assert!(registry.has_tool("echo"));
        assert!(!registry.has_tool("nonexistent"));
    }

    #[test]
    fn tool_names_returns_registered_names() {
        let mut registry = ToolRegistry::new();
        registry.register(EchoTool);
        registry.register(BigOutputTool { output_size: 10 });

        let mut names = registry.tool_names();
        names.sort();
        assert_eq!(names, vec!["big_output", "echo"]);
    }

    #[test]
    fn tool_definitions_generates_correct_structs() {
        let mut registry = ToolRegistry::new();
        registry.register(EchoTool);

        let defs = registry.tool_definitions();
        assert_eq!(defs.len(), 1);

        let def = &defs[0];
        assert_eq!(def.name, "echo");
        assert_eq!(def.description, "Echoes input");
        assert_eq!(
            def.parameters,
            json!({
                "type": "object",
                "properties": { "text": { "type": "string" } }
            })
        );
    }

    #[tokio::test]
    async fn execute_registered_tool_returns_output() {
        let mut registry = ToolRegistry::new();
        registry.register(EchoTool);

        let output = registry.execute("echo", r#"{"text":"hello"}"#).await;
        assert!(!output.is_error);
        assert_eq!(output.content, "hello");
        assert_eq!(output.duration_ms, 1);
    }

    #[tokio::test]
    async fn execute_unregistered_tool_returns_error() {
        let registry = ToolRegistry::new();

        let output = registry.execute("nonexistent", "{}").await;
        assert!(output.is_error);
        assert!(output.content.contains("tool 'nonexistent' not found"));
        assert_eq!(output.duration_ms, 0);
    }

    #[tokio::test]
    async fn execute_truncates_long_output() {
        let mut registry = ToolRegistry::new().with_max_output_chars(200);
        registry.register(BigOutputTool { output_size: 5000 });

        let output = registry.execute("big_output", "{}").await;
        assert!(!output.is_error);
        assert!(output.content.contains("[... truncated"));
        // The truncated output should be roughly within the max.
        assert!(
            output.content.len() <= 250,
            "output len {} should be near max 200",
            output.content.len()
        );
    }

    #[test]
    fn output_limit_for_returns_spec_defaults() {
        let registry = ToolRegistry::new();

        assert_eq!(registry.output_limit_for("read_file"), 50_000);
        assert_eq!(registry.output_limit_for("shell"), 30_000);
        assert_eq!(registry.output_limit_for("grep"), 20_000);
        assert_eq!(registry.output_limit_for("glob_files"), 20_000);
        assert_eq!(registry.output_limit_for("edit_file"), 10_000);
        assert_eq!(registry.output_limit_for("write_file"), 10_000);
        assert_eq!(registry.output_limit_for("apply_patch"), 10_000);
    }

    #[test]
    fn output_limit_for_unknown_tool_returns_default() {
        let registry = ToolRegistry::new();
        assert_eq!(registry.output_limit_for("some_unknown_tool"), 100_000);
    }

    #[tokio::test]
    async fn execute_applies_per_tool_limit_for_read_file() {
        let mut registry = ToolRegistry::new();
        // read_file limit is 50_000, so 60_000 chars should be truncated
        registry.register(NamedBigOutputTool {
            tool_name: "read_file",
            output_size: 60_000,
        });

        let output = registry.execute("read_file", "{}").await;
        assert!(!output.is_error);
        assert!(output.content.contains("[... truncated"));
        assert!(
            output.content.len() <= 55_000,
            "read_file output len {} should be near max 50000",
            output.content.len()
        );
    }

    #[tokio::test]
    async fn execute_applies_per_tool_limit_for_shell() {
        let mut registry = ToolRegistry::new();
        // shell limit is 30_000, so 40_000 chars should be truncated
        registry.register(NamedBigOutputTool {
            tool_name: "shell",
            output_size: 40_000,
        });

        let output = registry.execute("shell", "{}").await;
        assert!(!output.is_error);
        assert!(output.content.contains("[... truncated"));
        assert!(
            output.content.len() <= 35_000,
            "shell output len {} should be near max 30000",
            output.content.len()
        );
    }

    #[tokio::test]
    async fn execute_does_not_truncate_within_per_tool_limit() {
        let mut registry = ToolRegistry::new();
        // read_file limit is 50_000, output of 40_000 should NOT be truncated
        registry.register(NamedBigOutputTool {
            tool_name: "read_file",
            output_size: 40_000,
        });

        let output = registry.execute("read_file", "{}").await;
        assert!(!output.is_error);
        assert!(!output.content.contains("[... truncated"));
        assert_eq!(output.content.len(), 40_000);
    }

    #[test]
    fn tool_output_success_constructor() {
        let output = ToolOutput::success("ok", 42);
        assert_eq!(output.content, "ok");
        assert!(!output.is_error);
        assert_eq!(output.duration_ms, 42);
    }

    #[test]
    fn tool_output_error_constructor() {
        let output = ToolOutput::error("boom", 7);
        assert_eq!(output.content, "boom");
        assert!(output.is_error);
        assert_eq!(output.duration_ms, 7);
    }
}
