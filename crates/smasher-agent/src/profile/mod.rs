// ABOUTME: Provider profile configuration for LLM-specific system prompts and capabilities.
// ABOUTME: Each profile tailors the agent's presentation to a specific provider (Anthropic, OpenAI, Gemini).

use smasher_llm::types::{Provider, infer_provider};

/// Configuration values that feed into system prompt generation.
#[derive(Debug, Clone, Default)]
pub struct SystemPromptConfig {
    /// The project's working directory.
    pub working_directory: Option<String>,
    /// Custom instructions from project docs (e.g., CLAUDE.md, AGENTS.md).
    pub project_instructions: Option<String>,
    /// Additional custom instructions.
    pub custom_instructions: Option<String>,
    /// Current date/time string.
    pub current_date: Option<String>,
    /// Operating system info.
    pub os_info: Option<String>,
}

/// Defines how the agent presents itself to a specific LLM provider.
pub trait ProviderProfile: Send + Sync {
    /// The name of this profile (e.g., "anthropic", "openai", "gemini").
    fn name(&self) -> &str;

    /// Build the system prompt for this provider.
    fn system_prompt(&self, config: &SystemPromptConfig) -> String;

    /// Get the tool names that this profile makes available.
    fn tool_names(&self) -> Vec<&str>;

    /// Whether this profile supports thinking/reasoning.
    fn supports_thinking(&self) -> bool;

    /// Maximum context window for the model.
    fn context_window(&self) -> u32;

    /// The default model for this profile.
    fn default_model(&self) -> &str;
}

/// Claude Code-aligned provider profile for Anthropic models.
pub struct AnthropicProfile;

impl ProviderProfile for AnthropicProfile {
    fn name(&self) -> &str {
        "anthropic"
    }

    fn system_prompt(&self, config: &SystemPromptConfig) -> String {
        let mut sections = Vec::new();

        sections.push(
            "You are an AI coding assistant. You help users with software engineering tasks."
                .to_string(),
        );

        // Environment section
        let mut env_lines = Vec::new();
        if let Some(ref wd) = config.working_directory {
            env_lines.push(format!("Working directory: {wd}"));
        }
        if let Some(ref os) = config.os_info {
            env_lines.push(format!("OS: {os}"));
        }
        if let Some(ref date) = config.current_date {
            env_lines.push(format!("Date: {date}"));
        }
        if !env_lines.is_empty() {
            sections.push(format!("# Environment\n{}", env_lines.join("\n")));
        }

        // Tools section
        sections.push(
            "# Available Tools\nYou have access to tools for reading files, writing files, editing files, running commands, searching code, and finding files.".to_string()
        );

        // Instructions section
        let mut instr_lines = vec![
            "- Read files before modifying them to understand existing code".to_string(),
            "- Make minimal, focused changes".to_string(),
            "- Prefer editing existing files over creating new ones".to_string(),
            "- Run tests after making changes".to_string(),
        ];
        if let Some(ref proj) = config.project_instructions {
            instr_lines.push(proj.clone());
        }
        if let Some(ref custom) = config.custom_instructions {
            instr_lines.push(custom.clone());
        }
        sections.push(format!("# Instructions\n{}", instr_lines.join("\n")));

        sections.join("\n\n")
    }

    fn tool_names(&self) -> Vec<&str> {
        vec![
            "read_file",
            "write_file",
            "edit_file",
            "shell",
            "grep",
            "glob_files",
        ]
    }

    fn supports_thinking(&self) -> bool {
        true
    }

    fn context_window(&self) -> u32 {
        200_000
    }

    fn default_model(&self) -> &str {
        "claude-sonnet-4-20250514"
    }
}

/// OpenAI/Codex-aligned provider profile.
pub struct OpenAiProfile;

impl ProviderProfile for OpenAiProfile {
    fn name(&self) -> &str {
        "openai"
    }

    fn system_prompt(&self, config: &SystemPromptConfig) -> String {
        let mut sections = Vec::new();

        sections.push("You are an AI coding assistant powered by OpenAI. You help users write, debug, and improve code.".to_string());

        // Environment section
        let mut env_lines = Vec::new();
        if let Some(ref wd) = config.working_directory {
            env_lines.push(format!("Working directory: {wd}"));
        }
        if let Some(ref os) = config.os_info {
            env_lines.push(format!("OS: {os}"));
        }
        if let Some(ref date) = config.current_date {
            env_lines.push(format!("Date: {date}"));
        }
        if !env_lines.is_empty() {
            sections.push(format!("# Environment\n{}", env_lines.join("\n")));
        }

        // Tools section — OpenAI uses apply_patch for file modifications
        sections.push(
            "# Available Tools\nYou have access to tools for reading files, applying patches via apply_patch, running commands, searching code, and finding files. Use apply_patch to create, modify, or delete files.".to_string()
        );

        // Instructions section
        let mut instr_lines = vec![
            "- Read files before modifying them to understand existing code".to_string(),
            "- Use apply_patch for all file modifications".to_string(),
            "- Make minimal, focused changes".to_string(),
            "- Run tests after making changes".to_string(),
        ];
        if let Some(ref proj) = config.project_instructions {
            instr_lines.push(proj.clone());
        }
        if let Some(ref custom) = config.custom_instructions {
            instr_lines.push(custom.clone());
        }
        sections.push(format!("# Instructions\n{}", instr_lines.join("\n")));

        sections.join("\n\n")
    }

    fn tool_names(&self) -> Vec<&str> {
        vec!["read_file", "apply_patch", "shell", "grep", "glob_files"]
    }

    fn supports_thinking(&self) -> bool {
        true
    }

    fn context_window(&self) -> u32 {
        128_000
    }

    fn default_model(&self) -> &str {
        "gpt-4o"
    }
}

/// Gemini-aligned provider profile for Google models.
pub struct GeminiProfile;

impl ProviderProfile for GeminiProfile {
    fn name(&self) -> &str {
        "gemini"
    }

    fn system_prompt(&self, config: &SystemPromptConfig) -> String {
        let mut sections = Vec::new();

        sections.push("You are an AI coding assistant powered by Google Gemini. You help users with software engineering tasks including writing, reviewing, and debugging code.".to_string());

        // Environment section
        let mut env_lines = Vec::new();
        if let Some(ref wd) = config.working_directory {
            env_lines.push(format!("Working directory: {wd}"));
        }
        if let Some(ref os) = config.os_info {
            env_lines.push(format!("OS: {os}"));
        }
        if let Some(ref date) = config.current_date {
            env_lines.push(format!("Date: {date}"));
        }
        if !env_lines.is_empty() {
            sections.push(format!("# Environment\n{}", env_lines.join("\n")));
        }

        // Tools section
        sections.push(
            "# Available Tools\nYou have access to tools for reading files, writing files, editing files, running commands, searching code, and finding files.".to_string()
        );

        // Instructions section
        let mut instr_lines = vec![
            "- Read files before modifying them to understand existing code".to_string(),
            "- Make minimal, focused changes".to_string(),
            "- Prefer editing existing files over creating new ones".to_string(),
            "- Run tests after making changes".to_string(),
        ];
        if let Some(ref proj) = config.project_instructions {
            instr_lines.push(proj.clone());
        }
        if let Some(ref custom) = config.custom_instructions {
            instr_lines.push(custom.clone());
        }
        sections.push(format!("# Instructions\n{}", instr_lines.join("\n")));

        sections.join("\n\n")
    }

    fn tool_names(&self) -> Vec<&str> {
        vec![
            "read_file",
            "write_file",
            "edit_file",
            "shell",
            "grep",
            "glob_files",
        ]
    }

    fn supports_thinking(&self) -> bool {
        true
    }

    fn context_window(&self) -> u32 {
        1_000_000
    }

    fn default_model(&self) -> &str {
        "gemini-2.5-pro"
    }
}

/// Returns the appropriate provider profile for a given model ID.
///
/// Uses `smasher_llm::types::infer_provider` to determine the provider from
/// model naming patterns. Defaults to `AnthropicProfile` if the provider
/// cannot be determined.
pub fn profile_for_model(model: &str) -> Box<dyn ProviderProfile> {
    match infer_provider(model) {
        Some(Provider::Anthropic) => Box::new(AnthropicProfile),
        Some(Provider::OpenAi) => Box::new(OpenAiProfile),
        Some(Provider::Gemini) => Box::new(GeminiProfile),
        None => Box::new(AnthropicProfile),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- AnthropicProfile basic properties ---

    #[test]
    fn anthropic_profile_name() {
        let profile = AnthropicProfile;
        assert_eq!(profile.name(), "anthropic");
    }

    #[test]
    fn anthropic_profile_default_model() {
        let profile = AnthropicProfile;
        assert_eq!(profile.default_model(), "claude-sonnet-4-20250514");
    }

    #[test]
    fn anthropic_profile_context_window() {
        let profile = AnthropicProfile;
        assert_eq!(profile.context_window(), 200_000);
    }

    #[test]
    fn anthropic_profile_supports_thinking() {
        let profile = AnthropicProfile;
        assert!(profile.supports_thinking());
    }

    #[test]
    fn anthropic_profile_tool_names() {
        let profile = AnthropicProfile;
        let tools = profile.tool_names();
        assert_eq!(
            tools,
            vec![
                "read_file",
                "write_file",
                "edit_file",
                "shell",
                "grep",
                "glob_files"
            ]
        );
    }

    // --- AnthropicProfile system prompt ---

    #[test]
    fn anthropic_system_prompt_includes_working_directory() {
        let profile = AnthropicProfile;
        let config = SystemPromptConfig {
            working_directory: Some("/home/user/project".to_string()),
            ..Default::default()
        };
        let prompt = profile.system_prompt(&config);
        assert!(prompt.contains("Working directory: /home/user/project"));
    }

    #[test]
    fn anthropic_system_prompt_includes_project_instructions() {
        let profile = AnthropicProfile;
        let config = SystemPromptConfig {
            project_instructions: Some("- Always use TDD".to_string()),
            ..Default::default()
        };
        let prompt = profile.system_prompt(&config);
        assert!(prompt.contains("- Always use TDD"));
    }

    #[test]
    fn anthropic_system_prompt_includes_custom_instructions() {
        let profile = AnthropicProfile;
        let config = SystemPromptConfig {
            custom_instructions: Some("- Prefer Rust over Python".to_string()),
            ..Default::default()
        };
        let prompt = profile.system_prompt(&config);
        assert!(prompt.contains("- Prefer Rust over Python"));
    }

    #[test]
    fn anthropic_system_prompt_without_optional_fields() {
        let profile = AnthropicProfile;
        let config = SystemPromptConfig::default();
        let prompt = profile.system_prompt(&config);
        assert!(prompt.contains("You are an AI coding assistant"));
        assert!(prompt.contains("# Available Tools"));
        assert!(prompt.contains("# Instructions"));
        // Should not contain environment section when no env fields are set
        assert!(!prompt.contains("# Environment"));
    }

    // --- OpenAiProfile basic properties ---

    #[test]
    fn openai_profile_name() {
        let profile = OpenAiProfile;
        assert_eq!(profile.name(), "openai");
    }

    #[test]
    fn openai_profile_default_model() {
        let profile = OpenAiProfile;
        assert_eq!(profile.default_model(), "gpt-4o");
    }

    #[test]
    fn openai_profile_context_window() {
        let profile = OpenAiProfile;
        assert_eq!(profile.context_window(), 128_000);
    }

    #[test]
    fn openai_profile_supports_thinking() {
        let profile = OpenAiProfile;
        assert!(profile.supports_thinking());
    }

    #[test]
    fn openai_profile_tool_names_uses_apply_patch() {
        let profile = OpenAiProfile;
        let tools = profile.tool_names();
        assert_eq!(
            tools,
            vec!["read_file", "apply_patch", "shell", "grep", "glob_files"]
        );
    }

    #[test]
    fn openai_profile_excludes_edit_file_and_write_file() {
        let profile = OpenAiProfile;
        let tools = profile.tool_names();
        assert!(
            !tools.contains(&"edit_file"),
            "OpenAI profile should not include edit_file"
        );
        assert!(
            !tools.contains(&"write_file"),
            "OpenAI profile should not include write_file"
        );
    }

    // --- GeminiProfile basic properties ---

    #[test]
    fn gemini_profile_name() {
        let profile = GeminiProfile;
        assert_eq!(profile.name(), "gemini");
    }

    #[test]
    fn gemini_profile_default_model() {
        let profile = GeminiProfile;
        assert_eq!(profile.default_model(), "gemini-2.5-pro");
    }

    #[test]
    fn gemini_profile_context_window() {
        let profile = GeminiProfile;
        assert_eq!(profile.context_window(), 1_000_000);
    }

    #[test]
    fn gemini_profile_supports_thinking() {
        let profile = GeminiProfile;
        assert!(profile.supports_thinking());
    }

    #[test]
    fn gemini_profile_tool_names() {
        let profile = GeminiProfile;
        let tools = profile.tool_names();
        assert_eq!(
            tools,
            vec![
                "read_file",
                "write_file",
                "edit_file",
                "shell",
                "grep",
                "glob_files"
            ]
        );
    }

    #[test]
    fn anthropic_profile_excludes_apply_patch() {
        let profile = AnthropicProfile;
        let tools = profile.tool_names();
        assert!(
            !tools.contains(&"apply_patch"),
            "Anthropic profile should not include apply_patch"
        );
    }

    #[test]
    fn gemini_profile_excludes_apply_patch() {
        let profile = GeminiProfile;
        let tools = profile.tool_names();
        assert!(
            !tools.contains(&"apply_patch"),
            "Gemini profile should not include apply_patch"
        );
    }

    #[test]
    fn each_profile_has_distinct_system_prompt() {
        let config = SystemPromptConfig::default();
        let anthropic_prompt = AnthropicProfile.system_prompt(&config);
        let openai_prompt = OpenAiProfile.system_prompt(&config);
        let gemini_prompt = GeminiProfile.system_prompt(&config);

        // All three must be different from each other
        assert_ne!(
            anthropic_prompt, openai_prompt,
            "Anthropic and OpenAI prompts should differ"
        );
        assert_ne!(
            anthropic_prompt, gemini_prompt,
            "Anthropic and Gemini prompts should differ"
        );
        assert_ne!(
            openai_prompt, gemini_prompt,
            "OpenAI and Gemini prompts should differ"
        );
    }

    #[test]
    fn openai_system_prompt_mentions_apply_patch() {
        let config = SystemPromptConfig::default();
        let prompt = OpenAiProfile.system_prompt(&config);
        assert!(
            prompt.contains("apply_patch"),
            "OpenAI system prompt should mention apply_patch tool"
        );
    }

    // --- profile_for_model ---

    #[test]
    fn profile_for_model_returns_anthropic_for_claude() {
        let profile = profile_for_model("claude-sonnet-4-20250514");
        assert_eq!(profile.name(), "anthropic");
    }

    #[test]
    fn profile_for_model_returns_openai_for_gpt() {
        let profile = profile_for_model("gpt-4o");
        assert_eq!(profile.name(), "openai");
    }

    #[test]
    fn profile_for_model_returns_gemini_for_gemini() {
        let profile = profile_for_model("gemini-2.0-flash");
        assert_eq!(profile.name(), "gemini");
    }

    #[test]
    fn profile_for_model_defaults_to_anthropic_for_unknown() {
        let profile = profile_for_model("llama-3-70b");
        assert_eq!(profile.name(), "anthropic");
    }

    // --- SystemPromptConfig ---

    #[test]
    fn system_prompt_config_default_is_all_none() {
        let config = SystemPromptConfig::default();
        assert!(config.working_directory.is_none());
        assert!(config.project_instructions.is_none());
        assert!(config.custom_instructions.is_none());
        assert!(config.current_date.is_none());
        assert!(config.os_info.is_none());
    }
}
