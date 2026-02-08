// ABOUTME: Defines the unified Request struct sent to any LLM provider via the builder pattern.
// ABOUTME: Covers model selection, messages, sampling params, tools, thinking, and provider extras.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use super::message::Message;
use super::response_format::ResponseFormat;
use super::tool::{ToolChoice, ToolDefinition};

/// A unified request to be sent to any LLM provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Request {
    pub model: String,
    pub messages: Vec<Message>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop_sequences: Option<Vec<String>>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<ToolDefinition>>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<ToolChoice>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_format: Option<ResponseFormat>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<ThinkingConfig>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream: Option<bool>,

    /// Provider-specific extra parameters.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extra: Option<serde_json::Value>,

    /// Explicit provider override (instead of model-based inference).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,

    /// Reasoning effort hint: "low", "medium", or "high".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,

    /// Arbitrary metadata dict.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,

    /// Provider-specific options keyed by provider name (e.g. `{"anthropic": {"prompt_caching": true}}`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_options: Option<HashMap<String, serde_json::Value>>,
}

/// Configuration for extended thinking / reasoning.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThinkingConfig {
    pub enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub budget_tokens: Option<u32>,
}

impl Request {
    /// Create a minimal request with model and messages.
    pub fn new(model: impl Into<String>, messages: Vec<Message>) -> Self {
        Self {
            model: model.into(),
            messages,
            system_prompt: None,
            max_tokens: None,
            temperature: None,
            top_p: None,
            stop_sequences: None,
            tools: None,
            tool_choice: None,
            response_format: None,
            thinking: None,
            stream: None,
            extra: None,
            provider: None,
            reasoning_effort: None,
            metadata: None,
            provider_options: None,
        }
    }

    /// Set the system prompt.
    pub fn system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.system_prompt = Some(prompt.into());
        self
    }

    /// Set the maximum number of tokens to generate.
    pub fn max_tokens(mut self, tokens: u32) -> Self {
        self.max_tokens = Some(tokens);
        self
    }

    /// Set the sampling temperature.
    pub fn temperature(mut self, temp: f32) -> Self {
        self.temperature = Some(temp);
        self
    }

    /// Set the top-p (nucleus) sampling parameter.
    pub fn top_p(mut self, p: f32) -> Self {
        self.top_p = Some(p);
        self
    }

    /// Set stop sequences that halt generation.
    pub fn stop_sequences(mut self, seqs: Vec<String>) -> Self {
        self.stop_sequences = Some(seqs);
        self
    }

    /// Set the tool definitions available to the model.
    pub fn tools(mut self, tools: Vec<ToolDefinition>) -> Self {
        self.tools = Some(tools);
        self
    }

    /// Set how the model should choose among available tools.
    pub fn tool_choice(mut self, choice: ToolChoice) -> Self {
        self.tool_choice = Some(choice);
        self
    }

    /// Set the desired response format.
    pub fn response_format(mut self, format: ResponseFormat) -> Self {
        self.response_format = Some(format);
        self
    }

    /// Set the extended thinking / reasoning configuration.
    pub fn thinking(mut self, config: ThinkingConfig) -> Self {
        self.thinking = Some(config);
        self
    }

    /// Enable or disable streaming.
    pub fn stream(mut self, enabled: bool) -> Self {
        self.stream = Some(enabled);
        self
    }

    /// Set provider-specific extra parameters.
    pub fn extra(mut self, extra: serde_json::Value) -> Self {
        self.extra = Some(extra);
        self
    }

    /// Set an explicit provider override.
    pub fn provider(mut self, provider: impl Into<String>) -> Self {
        self.provider = Some(provider.into());
        self
    }

    /// Set the reasoning effort hint ("low", "medium", or "high").
    pub fn reasoning_effort(mut self, effort: impl Into<String>) -> Self {
        self.reasoning_effort = Some(effort.into());
        self
    }

    /// Set arbitrary metadata.
    pub fn metadata(mut self, metadata: serde_json::Value) -> Self {
        self.metadata = Some(metadata);
        self
    }

    /// Set provider-specific options keyed by provider name.
    pub fn provider_options(mut self, options: HashMap<String, serde_json::Value>) -> Self {
        self.provider_options = Some(options);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::HashMap;

    fn sample_messages() -> Vec<Message> {
        vec![Message::user("Hello, world!")]
    }

    // --- Request::new tests ---

    #[test]
    fn new_creates_minimal_request() {
        let req = Request::new("gpt-4", sample_messages());

        assert_eq!(req.model, "gpt-4");
        assert_eq!(req.messages.len(), 1);
        assert!(req.system_prompt.is_none());
        assert!(req.max_tokens.is_none());
        assert!(req.temperature.is_none());
        assert!(req.top_p.is_none());
        assert!(req.stop_sequences.is_none());
        assert!(req.tools.is_none());
        assert!(req.tool_choice.is_none());
        assert!(req.response_format.is_none());
        assert!(req.thinking.is_none());
        assert!(req.stream.is_none());
        assert!(req.extra.is_none());
        assert!(req.provider.is_none());
        assert!(req.reasoning_effort.is_none());
        assert!(req.metadata.is_none());
        assert!(req.provider_options.is_none());
    }

    #[test]
    fn new_accepts_string_owned() {
        let model = String::from("claude-sonnet-4-20250514");
        let req = Request::new(model, sample_messages());
        assert_eq!(req.model, "claude-sonnet-4-20250514");
    }

    // --- Builder method tests ---

    #[test]
    fn builder_system_prompt() {
        let req = Request::new("gpt-4", sample_messages())
            .system_prompt("You are a helpful assistant.");

        assert_eq!(
            req.system_prompt.as_deref(),
            Some("You are a helpful assistant.")
        );
    }

    #[test]
    fn builder_max_tokens() {
        let req = Request::new("gpt-4", sample_messages()).max_tokens(1024);
        assert_eq!(req.max_tokens, Some(1024));
    }

    #[test]
    fn builder_temperature() {
        let req = Request::new("gpt-4", sample_messages()).temperature(0.7);
        assert_eq!(req.temperature, Some(0.7));
    }

    #[test]
    fn builder_top_p() {
        let req = Request::new("gpt-4", sample_messages()).top_p(0.9);
        assert_eq!(req.top_p, Some(0.9));
    }

    #[test]
    fn builder_stop_sequences() {
        let req = Request::new("gpt-4", sample_messages())
            .stop_sequences(vec!["STOP".into(), "END".into()]);

        let seqs = req.stop_sequences.unwrap();
        assert_eq!(seqs.len(), 2);
        assert_eq!(seqs[0], "STOP");
        assert_eq!(seqs[1], "END");
    }

    #[test]
    fn builder_tools() {
        let tool = ToolDefinition::new(
            "search",
            "Search the web",
            json!({"type": "object", "properties": {"query": {"type": "string"}}}),
        );
        let req = Request::new("gpt-4", sample_messages()).tools(vec![tool]);

        let tools = req.tools.unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "search");
    }

    #[test]
    fn builder_tool_choice() {
        let req = Request::new("gpt-4", sample_messages()).tool_choice(ToolChoice::Required);
        assert!(matches!(req.tool_choice, Some(ToolChoice::Required)));
    }

    #[test]
    fn builder_response_format() {
        let req = Request::new("gpt-4", sample_messages())
            .response_format(ResponseFormat::JsonObject);

        assert!(matches!(
            req.response_format,
            Some(ResponseFormat::JsonObject)
        ));
    }

    #[test]
    fn builder_thinking() {
        let config = ThinkingConfig {
            enabled: true,
            budget_tokens: Some(10000),
        };
        let req = Request::new("claude-sonnet-4-20250514", sample_messages()).thinking(config);

        let thinking = req.thinking.unwrap();
        assert!(thinking.enabled);
        assert_eq!(thinking.budget_tokens, Some(10000));
    }

    #[test]
    fn builder_stream() {
        let req = Request::new("gpt-4", sample_messages()).stream(true);
        assert_eq!(req.stream, Some(true));
    }

    #[test]
    fn builder_extra() {
        let req =
            Request::new("gpt-4", sample_messages()).extra(json!({"seed": 42, "logprobs": true}));

        let extra = req.extra.unwrap();
        assert_eq!(extra["seed"], 42);
        assert_eq!(extra["logprobs"], true);
    }

    // --- New field builder tests ---

    #[test]
    fn builder_provider() {
        let req = Request::new("gpt-4", sample_messages()).provider("openai");
        assert_eq!(req.provider.as_deref(), Some("openai"));
    }

    #[test]
    fn builder_reasoning_effort() {
        let req = Request::new("gpt-4", sample_messages()).reasoning_effort("high");
        assert_eq!(req.reasoning_effort.as_deref(), Some("high"));
    }

    #[test]
    fn builder_metadata() {
        let req = Request::new("gpt-4", sample_messages())
            .metadata(json!({"user_id": "abc123", "session": 42}));
        let meta = req.metadata.unwrap();
        assert_eq!(meta["user_id"], "abc123");
        assert_eq!(meta["session"], 42);
    }

    #[test]
    fn builder_provider_options() {
        let mut opts = HashMap::new();
        opts.insert(
            "anthropic".into(),
            json!({"prompt_caching": true}),
        );
        opts.insert(
            "openai".into(),
            json!({"store": true}),
        );
        let req = Request::new("gpt-4", sample_messages()).provider_options(opts);
        let po = req.provider_options.unwrap();
        assert_eq!(po["anthropic"]["prompt_caching"], true);
        assert_eq!(po["openai"]["store"], true);
    }

    // --- Chaining tests ---

    #[test]
    fn chaining_multiple_builder_methods() {
        let req = Request::new("gpt-4", sample_messages())
            .system_prompt("Be concise.")
            .max_tokens(512)
            .temperature(0.5)
            .top_p(0.95)
            .stream(true);

        assert_eq!(req.system_prompt.as_deref(), Some("Be concise."));
        assert_eq!(req.max_tokens, Some(512));
        assert_eq!(req.temperature, Some(0.5));
        assert_eq!(req.top_p, Some(0.95));
        assert_eq!(req.stream, Some(true));
    }

    #[test]
    fn chaining_all_builder_methods() {
        let tool = ToolDefinition::new("calc", "Calculator", json!({"type": "object"}));
        let thinking = ThinkingConfig {
            enabled: true,
            budget_tokens: Some(5000),
        };
        let mut opts = HashMap::new();
        opts.insert("anthropic".into(), json!({"cache": true}));

        let req = Request::new("claude-sonnet-4-20250514", sample_messages())
            .system_prompt("You are a calculator.")
            .max_tokens(2048)
            .temperature(0.3)
            .top_p(0.8)
            .stop_sequences(vec!["DONE".into()])
            .tools(vec![tool])
            .tool_choice(ToolChoice::Auto)
            .response_format(ResponseFormat::Text)
            .thinking(thinking)
            .stream(false)
            .extra(json!({"custom": "value"}))
            .provider("anthropic")
            .reasoning_effort("medium")
            .metadata(json!({"trace_id": "xyz"}))
            .provider_options(opts);

        assert_eq!(req.model, "claude-sonnet-4-20250514");
        assert_eq!(
            req.system_prompt.as_deref(),
            Some("You are a calculator.")
        );
        assert_eq!(req.max_tokens, Some(2048));
        assert_eq!(req.temperature, Some(0.3));
        assert_eq!(req.top_p, Some(0.8));
        assert_eq!(req.stop_sequences.as_ref().unwrap().len(), 1);
        assert_eq!(req.tools.as_ref().unwrap().len(), 1);
        assert!(matches!(req.tool_choice, Some(ToolChoice::Auto)));
        assert!(matches!(req.response_format, Some(ResponseFormat::Text)));
        assert!(req.thinking.as_ref().unwrap().enabled);
        assert_eq!(req.stream, Some(false));
        assert_eq!(req.extra.as_ref().unwrap()["custom"], "value");
        assert_eq!(req.provider.as_deref(), Some("anthropic"));
        assert_eq!(req.reasoning_effort.as_deref(), Some("medium"));
        assert_eq!(req.metadata.as_ref().unwrap()["trace_id"], "xyz");
        assert_eq!(
            req.provider_options.as_ref().unwrap()["anthropic"]["cache"],
            true
        );
    }

    // --- Serde roundtrip tests ---

    #[test]
    fn request_serde_roundtrip_minimal() {
        let req = Request::new("gpt-4", sample_messages());
        let json_str = serde_json::to_string(&req).unwrap();
        let back: Request = serde_json::from_str(&json_str).unwrap();

        assert_eq!(back.model, "gpt-4");
        assert_eq!(back.messages.len(), 1);
        assert!(back.system_prompt.is_none());
        assert!(back.max_tokens.is_none());
        assert!(back.provider.is_none());
        assert!(back.reasoning_effort.is_none());
        assert!(back.metadata.is_none());
        assert!(back.provider_options.is_none());
    }

    #[test]
    fn request_serde_roundtrip_full() {
        let tool = ToolDefinition::new("search", "Search", json!({"type": "object"}));
        let thinking = ThinkingConfig {
            enabled: true,
            budget_tokens: Some(8000),
        };

        let req = Request::new("claude-sonnet-4-20250514", sample_messages())
            .system_prompt("You help.")
            .max_tokens(4096)
            .temperature(0.7)
            .top_p(0.9)
            .stop_sequences(vec!["<END>".into()])
            .tools(vec![tool])
            .tool_choice(ToolChoice::Auto)
            .response_format(ResponseFormat::JsonObject)
            .thinking(thinking)
            .stream(true)
            .extra(json!({"seed": 123}));

        let json_str = serde_json::to_string(&req).unwrap();
        let back: Request = serde_json::from_str(&json_str).unwrap();

        assert_eq!(back.model, "claude-sonnet-4-20250514");
        assert_eq!(back.system_prompt.as_deref(), Some("You help."));
        assert_eq!(back.max_tokens, Some(4096));
        assert_eq!(back.temperature, Some(0.7));
        assert_eq!(back.top_p, Some(0.9));
        assert_eq!(back.stop_sequences.as_ref().unwrap()[0], "<END>");
        assert_eq!(back.tools.as_ref().unwrap()[0].name, "search");
        assert!(matches!(back.tool_choice, Some(ToolChoice::Auto)));
        assert!(matches!(
            back.response_format,
            Some(ResponseFormat::JsonObject)
        ));
        assert!(back.thinking.as_ref().unwrap().enabled);
        assert_eq!(back.thinking.as_ref().unwrap().budget_tokens, Some(8000));
        assert_eq!(back.stream, Some(true));
        assert_eq!(back.extra.as_ref().unwrap()["seed"], 123);
    }

    // --- JSON serialization skip_serializing_if tests ---

    #[test]
    fn none_fields_skipped_in_json_serialization() {
        let req = Request::new("gpt-4", sample_messages());
        let value: serde_json::Value = serde_json::to_value(&req).unwrap();
        let obj = value.as_object().unwrap();

        // Required fields are present.
        assert!(obj.contains_key("model"));
        assert!(obj.contains_key("messages"));

        // Optional None fields are absent.
        assert!(!obj.contains_key("system_prompt"));
        assert!(!obj.contains_key("max_tokens"));
        assert!(!obj.contains_key("temperature"));
        assert!(!obj.contains_key("top_p"));
        assert!(!obj.contains_key("stop_sequences"));
        assert!(!obj.contains_key("tools"));
        assert!(!obj.contains_key("tool_choice"));
        assert!(!obj.contains_key("response_format"));
        assert!(!obj.contains_key("thinking"));
        assert!(!obj.contains_key("stream"));
        assert!(!obj.contains_key("extra"));
        assert!(!obj.contains_key("provider"));
        assert!(!obj.contains_key("reasoning_effort"));
        assert!(!obj.contains_key("metadata"));
        assert!(!obj.contains_key("provider_options"));
    }

    #[test]
    fn set_fields_present_in_json_serialization() {
        let req = Request::new("gpt-4", sample_messages())
            .max_tokens(100)
            .temperature(0.5)
            .stream(true)
            .provider("openai")
            .reasoning_effort("high");

        let value: serde_json::Value = serde_json::to_value(&req).unwrap();
        let obj = value.as_object().unwrap();

        assert!(obj.contains_key("max_tokens"));
        assert!(obj.contains_key("temperature"));
        assert!(obj.contains_key("stream"));
        assert!(obj.contains_key("provider"));
        assert!(obj.contains_key("reasoning_effort"));

        // Fields not set remain absent.
        assert!(!obj.contains_key("system_prompt"));
        assert!(!obj.contains_key("tools"));
        assert!(!obj.contains_key("extra"));
        assert!(!obj.contains_key("metadata"));
        assert!(!obj.contains_key("provider_options"));
    }

    // --- ThinkingConfig tests ---

    #[test]
    fn thinking_config_serde_roundtrip_with_budget() {
        let config = ThinkingConfig {
            enabled: true,
            budget_tokens: Some(10000),
        };
        let json_str = serde_json::to_string(&config).unwrap();
        let back: ThinkingConfig = serde_json::from_str(&json_str).unwrap();

        assert!(back.enabled);
        assert_eq!(back.budget_tokens, Some(10000));
    }

    #[test]
    fn thinking_config_serde_roundtrip_without_budget() {
        let config = ThinkingConfig {
            enabled: false,
            budget_tokens: None,
        };
        let json_str = serde_json::to_string(&config).unwrap();
        let back: ThinkingConfig = serde_json::from_str(&json_str).unwrap();

        assert!(!back.enabled);
        assert!(back.budget_tokens.is_none());
    }

    #[test]
    fn thinking_config_budget_tokens_skipped_when_none() {
        let config = ThinkingConfig {
            enabled: true,
            budget_tokens: None,
        };
        let value: serde_json::Value = serde_json::to_value(&config).unwrap();
        let obj = value.as_object().unwrap();

        assert!(obj.contains_key("enabled"));
        assert!(!obj.contains_key("budget_tokens"));
    }

    #[test]
    fn thinking_config_budget_tokens_present_when_some() {
        let config = ThinkingConfig {
            enabled: true,
            budget_tokens: Some(5000),
        };
        let value: serde_json::Value = serde_json::to_value(&config).unwrap();
        let obj = value.as_object().unwrap();

        assert!(obj.contains_key("enabled"));
        assert!(obj.contains_key("budget_tokens"));
        assert_eq!(obj["budget_tokens"], 5000);
    }
}
