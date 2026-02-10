// ABOUTME: System prompt assembly for coding agent sessions.
// ABOUTME: Discovers project docs, gathers environment info, and delegates final prompt generation to a provider profile.

use std::path::Path;

use crate::profile::{ProviderProfile, SystemPromptConfig};
use crate::types::SessionConfig;

/// Describes a project documentation file and which providers should load it.
#[derive(Debug, Clone)]
pub struct ProjectDocSpec {
    /// Filename (or relative path) of the doc file.
    pub filename: &'static str,
    /// Which provider this doc is specific to, or `None` if it applies to all providers.
    pub provider: Option<&'static str>,
}

/// Well-known project documentation files with their provider affinity.
///
/// Files with `provider: None` are loaded for all providers. Files with a
/// specific provider name are only loaded when the active profile matches.
pub const PROJECT_DOC_SPECS: &[ProjectDocSpec] = &[
    ProjectDocSpec {
        filename: "CLAUDE.md",
        provider: Some("anthropic"),
    },
    ProjectDocSpec {
        filename: ".claude",
        provider: Some("anthropic"),
    },
    ProjectDocSpec {
        filename: "GEMINI.md",
        provider: Some("gemini"),
    },
    ProjectDocSpec {
        filename: ".cursorrules",
        provider: Some("openai"),
    },
    ProjectDocSpec {
        filename: "AGENTS.md",
        provider: None,
    },
    ProjectDocSpec {
        filename: ".github/copilot-instructions.md",
        provider: None,
    },
    ProjectDocSpec {
        filename: "CONVENTIONS.md",
        provider: None,
    },
    ProjectDocSpec {
        filename: "AI_INSTRUCTIONS.md",
        provider: None,
    },
];

/// Well-known project documentation files that may contain agent instructions.
/// Kept for backward compatibility; use [`PROJECT_DOC_SPECS`] for provider filtering.
pub const PROJECT_DOC_FILES: &[&str] = &[
    "CLAUDE.md",
    ".claude",
    "GEMINI.md",
    "AGENTS.md",
    ".cursorrules",
    ".github/copilot-instructions.md",
    "CONVENTIONS.md",
    "AI_INSTRUCTIONS.md",
];

/// Collected operating system and environment information.
#[derive(Debug, Clone)]
pub struct SystemInfo {
    /// Operating system and architecture (e.g. "macos aarch64").
    pub os: String,
    /// Current local date in YYYY-MM-DD format.
    pub date: String,
}

/// Gather basic system information: OS/arch and current date.
pub fn gather_system_info() -> SystemInfo {
    let os = format!("{} {}", std::env::consts::OS, std::env::consts::ARCH);
    let date = chrono::Local::now().format("%Y-%m-%d").to_string();
    SystemInfo { os, date }
}

/// Discover and read project documentation files from a working directory.
///
/// Checks each file in [`PROJECT_DOC_FILES`] under the given directory. If any
/// are found, their contents are concatenated with header separators and
/// returned. Returns `None` if no documentation files exist.
pub async fn discover_project_docs(working_directory: &str) -> Option<String> {
    let base = Path::new(working_directory);
    let mut parts: Vec<String> = Vec::new();

    for filename in PROJECT_DOC_FILES {
        let path = base.join(filename);
        if tokio::fs::metadata(&path).await.is_ok()
            && let Ok(content) = tokio::fs::read_to_string(&path).await
        {
            parts.push(format!("\n\n# From {filename}\n\n{content}"));
        }
    }

    if parts.is_empty() {
        None
    } else {
        Some(parts.concat())
    }
}

/// Read content from a single file entry within a directory.
///
/// For a regular file, reads its content. For a directory (e.g. `.claude`),
/// reads files directly within it (immediate children only) and concatenates their contents.
async fn read_doc_entry(base: &Path, filename: &str) -> Option<String> {
    let path = base.join(filename);
    let meta = tokio::fs::metadata(&path).await.ok()?;

    if meta.is_dir() {
        let mut parts = Vec::new();
        let mut read_dir = tokio::fs::read_dir(&path).await.ok()?;
        while let Ok(Some(entry)) = read_dir.next_entry().await {
            let entry_path = entry.path();
            if entry_path.is_file()
                && let Ok(content) = tokio::fs::read_to_string(&entry_path).await
            {
                let rel = entry_path
                    .file_name()
                    .map(|n| format!("{filename}/{}", n.to_string_lossy()))
                    .unwrap_or_else(|| filename.to_string());
                parts.push(format!("\n\n# From {rel}\n\n{content}"));
            }
        }
        if parts.is_empty() {
            None
        } else {
            Some(parts.concat())
        }
    } else {
        let content = tokio::fs::read_to_string(&path).await.ok()?;
        Some(format!("\n\n# From {filename}\n\n{content}"))
    }
}

/// Discover and read project documentation files, filtered by provider profile.
///
/// Only includes files whose provider affinity matches the given `provider_name`,
/// plus files with no provider affinity (loaded for all providers).
pub async fn discover_project_docs_for_provider(
    working_directory: &str,
    provider_name: &str,
) -> Option<String> {
    let base = Path::new(working_directory);
    let mut parts: Vec<String> = Vec::new();

    for spec in PROJECT_DOC_SPECS {
        // Skip files that belong to a different provider.
        if let Some(required_provider) = spec.provider
            && required_provider != provider_name
        {
            continue;
        }

        if let Some(content) = read_doc_entry(base, spec.filename).await {
            parts.push(content);
        }
    }

    if parts.is_empty() {
        None
    } else {
        Some(parts.concat())
    }
}

/// Build the complete system prompt for a session.
///
/// Gathers system info, discovers project docs from the working directory (if
/// provided) filtered by the provider profile, assembles a
/// [`SystemPromptConfig`], and delegates to the given [`ProviderProfile`] for
/// final prompt generation.
pub async fn build_system_prompt(
    config: &SessionConfig,
    profile: &dyn ProviderProfile,
    working_directory: Option<&str>,
) -> String {
    let info = gather_system_info();

    let project_instructions = match working_directory {
        Some(wd) => discover_project_docs_for_provider(wd, profile.name()).await,
        None => None,
    };

    let prompt_config = SystemPromptConfig {
        working_directory: working_directory.map(|s| s.to_string()),
        project_instructions,
        custom_instructions: config.system_prompt.clone(),
        current_date: Some(info.date),
        os_info: Some(info.os),
    };

    profile.system_prompt(&prompt_config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::AnthropicProfile;
    use std::fs;
    use tempfile::TempDir;

    // ── discover_project_docs ────────────────────────────────────────

    #[tokio::test]
    async fn discover_project_docs_finds_claude_md() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("CLAUDE.md"), "# Project rules").unwrap();

        let result = discover_project_docs(tmp.path().to_str().unwrap()).await;
        assert!(result.is_some());
        let docs = result.unwrap();
        assert!(docs.contains("# From CLAUDE.md"));
        assert!(docs.contains("# Project rules"));
    }

    #[tokio::test]
    async fn discover_project_docs_returns_none_when_no_docs_exist() {
        let tmp = TempDir::new().unwrap();

        let result = discover_project_docs(tmp.path().to_str().unwrap()).await;
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn discover_project_docs_concatenates_multiple_docs() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("CLAUDE.md"), "Claude instructions").unwrap();
        fs::write(tmp.path().join("AGENTS.md"), "Agent instructions").unwrap();
        fs::write(tmp.path().join("CONVENTIONS.md"), "Code conventions").unwrap();

        let result = discover_project_docs(tmp.path().to_str().unwrap()).await;
        assert!(result.is_some());
        let docs = result.unwrap();
        assert!(docs.contains("# From CLAUDE.md"));
        assert!(docs.contains("Claude instructions"));
        assert!(docs.contains("# From AGENTS.md"));
        assert!(docs.contains("Agent instructions"));
        assert!(docs.contains("# From CONVENTIONS.md"));
        assert!(docs.contains("Code conventions"));
    }

    // ── gather_system_info ───────────────────────────────────────────

    #[test]
    fn gather_system_info_returns_valid_os_and_date() {
        let info = gather_system_info();

        // OS string should contain the platform constant
        assert!(info.os.contains(std::env::consts::OS));
        assert!(info.os.contains(std::env::consts::ARCH));

        // Date should match YYYY-MM-DD pattern
        assert_eq!(info.date.len(), 10);
        assert_eq!(&info.date[4..5], "-");
        assert_eq!(&info.date[7..8], "-");
    }

    // ── build_system_prompt ──────────────────────────────────────────

    #[tokio::test]
    async fn build_system_prompt_includes_working_directory() {
        let tmp = TempDir::new().unwrap();
        let wd = tmp.path().to_str().unwrap();

        let config = SessionConfig::default();
        let profile = AnthropicProfile;

        let prompt = build_system_prompt(&config, &profile, Some(wd)).await;
        assert!(prompt.contains(&format!("Working directory: {wd}")));
    }

    #[tokio::test]
    async fn build_system_prompt_includes_custom_instructions_from_config() {
        let config = SessionConfig::default().with_system_prompt("Always write tests first");
        let profile = AnthropicProfile;

        let prompt = build_system_prompt(&config, &profile, None).await;
        assert!(prompt.contains("Always write tests first"));
    }

    #[tokio::test]
    async fn build_system_prompt_works_with_no_working_directory() {
        let config = SessionConfig::default();
        let profile = AnthropicProfile;

        let prompt = build_system_prompt(&config, &profile, None).await;
        // Should still produce a valid prompt with base instructions
        assert!(prompt.contains("You are an AI coding assistant"));
        // Should not contain a working directory line
        assert!(!prompt.contains("Working directory:"));
    }

    #[tokio::test]
    async fn build_system_prompt_includes_project_docs_when_found() {
        let tmp = TempDir::new().unwrap();
        let wd = tmp.path().to_str().unwrap();
        fs::write(tmp.path().join("CLAUDE.md"), "Use Rust for everything").unwrap();

        let config = SessionConfig::default();
        let profile = AnthropicProfile;

        let prompt = build_system_prompt(&config, &profile, Some(wd)).await;
        assert!(prompt.contains("Use Rust for everything"));
        assert!(prompt.contains("# From CLAUDE.md"));
    }

    // ── PROJECT_DOC_FILES ────────────────────────────────────────────

    #[test]
    fn project_doc_files_contains_expected_filenames() {
        assert!(PROJECT_DOC_FILES.contains(&"CLAUDE.md"));
        assert!(PROJECT_DOC_FILES.contains(&".claude"));
        assert!(PROJECT_DOC_FILES.contains(&"GEMINI.md"));
        assert!(PROJECT_DOC_FILES.contains(&"AGENTS.md"));
        assert!(PROJECT_DOC_FILES.contains(&".cursorrules"));
        assert!(PROJECT_DOC_FILES.contains(&".github/copilot-instructions.md"));
        assert!(PROJECT_DOC_FILES.contains(&"CONVENTIONS.md"));
        assert!(PROJECT_DOC_FILES.contains(&"AI_INSTRUCTIONS.md"));
    }

    #[test]
    fn project_doc_files_has_expected_count() {
        assert_eq!(PROJECT_DOC_FILES.len(), 8);
    }

    #[test]
    fn project_doc_specs_has_expected_count() {
        assert_eq!(PROJECT_DOC_SPECS.len(), 8);
    }

    #[test]
    fn project_doc_specs_anthropic_entries() {
        let anthropic: Vec<_> = PROJECT_DOC_SPECS
            .iter()
            .filter(|s| s.provider == Some("anthropic"))
            .collect();
        assert_eq!(anthropic.len(), 2);
        let filenames: Vec<_> = anthropic.iter().map(|s| s.filename).collect();
        assert!(filenames.contains(&"CLAUDE.md"));
        assert!(filenames.contains(&".claude"));
    }

    #[test]
    fn project_doc_specs_gemini_entries() {
        let gemini: Vec<_> = PROJECT_DOC_SPECS
            .iter()
            .filter(|s| s.provider == Some("gemini"))
            .collect();
        assert_eq!(gemini.len(), 1);
        assert_eq!(gemini[0].filename, "GEMINI.md");
    }

    #[test]
    fn project_doc_specs_openai_entries() {
        let openai: Vec<_> = PROJECT_DOC_SPECS
            .iter()
            .filter(|s| s.provider == Some("openai"))
            .collect();
        assert_eq!(openai.len(), 1);
        assert_eq!(openai[0].filename, ".cursorrules");
    }

    #[test]
    fn project_doc_specs_universal_entries() {
        let universal: Vec<_> = PROJECT_DOC_SPECS
            .iter()
            .filter(|s| s.provider.is_none())
            .collect();
        assert_eq!(universal.len(), 4);
        let filenames: Vec<_> = universal.iter().map(|s| s.filename).collect();
        assert!(filenames.contains(&"AGENTS.md"));
        assert!(filenames.contains(&".github/copilot-instructions.md"));
        assert!(filenames.contains(&"CONVENTIONS.md"));
        assert!(filenames.contains(&"AI_INSTRUCTIONS.md"));
    }

    // ── discover_project_docs (edge cases) ───────────────────────────

    #[tokio::test]
    async fn discover_project_docs_finds_cursorrules() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join(".cursorrules"), "use tabs").unwrap();

        let result = discover_project_docs(tmp.path().to_str().unwrap()).await;
        assert!(result.is_some());
        let docs = result.unwrap();
        assert!(docs.contains("# From .cursorrules"));
        assert!(docs.contains("use tabs"));
    }

    #[tokio::test]
    async fn discover_project_docs_finds_agents_md() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("AGENTS.md"), "agent config here").unwrap();

        let result = discover_project_docs(tmp.path().to_str().unwrap()).await;
        assert!(result.is_some());
        let docs = result.unwrap();
        assert!(docs.contains("# From AGENTS.md"));
        assert!(docs.contains("agent config here"));
    }

    #[tokio::test]
    async fn discover_project_docs_finds_conventions_md() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("CONVENTIONS.md"), "naming rules").unwrap();

        let result = discover_project_docs(tmp.path().to_str().unwrap()).await;
        assert!(result.is_some());
        let docs = result.unwrap();
        assert!(docs.contains("# From CONVENTIONS.md"));
        assert!(docs.contains("naming rules"));
    }

    #[tokio::test]
    async fn discover_project_docs_finds_ai_instructions_md() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("AI_INSTRUCTIONS.md"), "be helpful").unwrap();

        let result = discover_project_docs(tmp.path().to_str().unwrap()).await;
        assert!(result.is_some());
        let docs = result.unwrap();
        assert!(docs.contains("# From AI_INSTRUCTIONS.md"));
        assert!(docs.contains("be helpful"));
    }

    #[tokio::test]
    async fn discover_project_docs_finds_nested_copilot_instructions() {
        let tmp = TempDir::new().unwrap();
        fs::create_dir_all(tmp.path().join(".github")).unwrap();
        fs::write(
            tmp.path().join(".github/copilot-instructions.md"),
            "copilot rules",
        )
        .unwrap();

        let result = discover_project_docs(tmp.path().to_str().unwrap()).await;
        assert!(result.is_some());
        let docs = result.unwrap();
        assert!(docs.contains("# From .github/copilot-instructions.md"));
        assert!(docs.contains("copilot rules"));
    }

    #[tokio::test]
    async fn discover_project_docs_preserves_file_order_from_constant() {
        let tmp = TempDir::new().unwrap();
        // Create docs in reverse order of PROJECT_DOC_FILES
        fs::write(tmp.path().join("AI_INSTRUCTIONS.md"), "last_in_list").unwrap();
        fs::write(tmp.path().join("CLAUDE.md"), "first_in_list").unwrap();

        let result = discover_project_docs(tmp.path().to_str().unwrap()).await;
        let docs = result.unwrap();

        // CLAUDE.md appears before AI_INSTRUCTIONS.md in PROJECT_DOC_FILES
        let claude_pos = docs.find("# From CLAUDE.md").unwrap();
        let ai_pos = docs.find("# From AI_INSTRUCTIONS.md").unwrap();
        assert!(
            claude_pos < ai_pos,
            "CLAUDE.md should appear before AI_INSTRUCTIONS.md"
        );
    }

    #[tokio::test]
    async fn discover_project_docs_handles_empty_file() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("CLAUDE.md"), "").unwrap();

        let result = discover_project_docs(tmp.path().to_str().unwrap()).await;
        // Even an empty file gets discovered since it exists
        assert!(result.is_some());
        let docs = result.unwrap();
        assert!(docs.contains("# From CLAUDE.md"));
    }

    #[tokio::test]
    async fn discover_project_docs_ignores_non_listed_files() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("README.md"), "readme content").unwrap();
        fs::write(tmp.path().join("CLAUDE.md"), "claude content").unwrap();

        let result = discover_project_docs(tmp.path().to_str().unwrap()).await;
        let docs = result.unwrap();
        // README.md is not in PROJECT_DOC_FILES, should not appear
        assert!(!docs.contains("README.md"));
        assert!(docs.contains("# From CLAUDE.md"));
    }

    #[tokio::test]
    async fn discover_project_docs_nonexistent_directory() {
        let result = discover_project_docs("/nonexistent/path/that/does/not/exist").await;
        assert!(result.is_none());
    }

    // ── SystemInfo field access ──────────────────────────────────────

    #[test]
    fn system_info_os_contains_both_os_and_arch() {
        let info = gather_system_info();
        let parts: Vec<&str> = info.os.split(' ').collect();
        assert_eq!(parts.len(), 2, "os field should be 'OS ARCH' format");
    }

    #[test]
    fn system_info_date_parses_as_valid_date() {
        let info = gather_system_info();
        let parsed = chrono::NaiveDate::parse_from_str(&info.date, "%Y-%m-%d");
        assert!(parsed.is_ok(), "date should parse as YYYY-MM-DD");
    }

    #[test]
    fn system_info_clone_works() {
        let info = gather_system_info();
        let cloned = info.clone();
        assert_eq!(info.os, cloned.os);
        assert_eq!(info.date, cloned.date);
    }

    #[test]
    fn system_info_debug_format() {
        let info = gather_system_info();
        let debug_str = format!("{:?}", info);
        assert!(debug_str.contains("SystemInfo"));
        assert!(debug_str.contains("os:"));
        assert!(debug_str.contains("date:"));
    }

    // ── build_system_prompt (additional coverage) ────────────────────

    #[tokio::test]
    async fn build_system_prompt_includes_os_info() {
        let config = SessionConfig::default();
        let profile = AnthropicProfile;

        let prompt = build_system_prompt(&config, &profile, None).await;
        // OS info is included via gather_system_info
        let expected_os = format!("{} {}", std::env::consts::OS, std::env::consts::ARCH);
        assert!(
            prompt.contains(&expected_os),
            "prompt should contain OS info"
        );
    }

    #[tokio::test]
    async fn build_system_prompt_includes_date() {
        let config = SessionConfig::default();
        let profile = AnthropicProfile;

        let prompt = build_system_prompt(&config, &profile, None).await;
        let today = chrono::Local::now().format("%Y-%m-%d").to_string();
        assert!(
            prompt.contains(&today),
            "prompt should contain current date"
        );
    }

    #[tokio::test]
    async fn build_system_prompt_with_all_options() {
        let tmp = TempDir::new().unwrap();
        let wd = tmp.path().to_str().unwrap();
        fs::write(tmp.path().join("CLAUDE.md"), "Project-specific rules").unwrap();

        let config = SessionConfig::default().with_system_prompt("Custom instructions from config");
        let profile = AnthropicProfile;

        let prompt = build_system_prompt(&config, &profile, Some(wd)).await;

        // Should contain base assistant identity
        assert!(prompt.contains("You are an AI coding assistant"));
        // Should contain working directory
        assert!(prompt.contains(&format!("Working directory: {wd}")));
        // Should contain project docs
        assert!(prompt.contains("Project-specific rules"));
        // Should contain custom instructions
        assert!(prompt.contains("Custom instructions from config"));
        // Should contain OS info
        assert!(prompt.contains(std::env::consts::OS));
        // Should contain date
        let today = chrono::Local::now().format("%Y-%m-%d").to_string();
        assert!(prompt.contains(&today));
    }

    #[tokio::test]
    async fn build_system_prompt_with_openai_profile() {
        let config = SessionConfig::default();
        let profile = crate::profile::OpenAiProfile;

        let prompt = build_system_prompt(&config, &profile, None).await;
        assert!(prompt.contains("OpenAI"));
    }

    #[tokio::test]
    async fn build_system_prompt_with_gemini_profile() {
        let config = SessionConfig::default();
        let profile = crate::profile::GeminiProfile;

        let prompt = build_system_prompt(&config, &profile, None).await;
        assert!(prompt.contains("Gemini"));
    }

    #[tokio::test]
    async fn build_system_prompt_working_directory_with_no_docs() {
        let tmp = TempDir::new().unwrap();
        let wd = tmp.path().to_str().unwrap();
        // No doc files created — empty directory

        let config = SessionConfig::default();
        let profile = AnthropicProfile;

        let prompt = build_system_prompt(&config, &profile, Some(wd)).await;
        // Working directory should still be present even without docs
        assert!(prompt.contains(&format!("Working directory: {wd}")));
        // But no project doc headers should appear
        assert!(!prompt.contains("# From CLAUDE.md"));
    }

    // ── discover_project_docs_for_provider ──────────────────────────

    #[tokio::test]
    async fn provider_filter_anthropic_includes_claude_md() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("CLAUDE.md"), "Claude rules").unwrap();
        fs::write(tmp.path().join("AGENTS.md"), "Universal rules").unwrap();

        let result =
            discover_project_docs_for_provider(tmp.path().to_str().unwrap(), "anthropic").await;
        let docs = result.unwrap();
        assert!(docs.contains("# From CLAUDE.md"));
        assert!(docs.contains("Claude rules"));
        assert!(docs.contains("# From AGENTS.md"));
        assert!(docs.contains("Universal rules"));
    }

    #[tokio::test]
    async fn provider_filter_anthropic_excludes_cursorrules() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join(".cursorrules"), "OpenAI only").unwrap();
        fs::write(tmp.path().join("AGENTS.md"), "Universal").unwrap();

        let result =
            discover_project_docs_for_provider(tmp.path().to_str().unwrap(), "anthropic").await;
        let docs = result.unwrap();
        assert!(!docs.contains(".cursorrules"));
        assert!(!docs.contains("OpenAI only"));
        assert!(docs.contains("# From AGENTS.md"));
    }

    #[tokio::test]
    async fn provider_filter_anthropic_excludes_gemini_md() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("GEMINI.md"), "Gemini only").unwrap();
        fs::write(tmp.path().join("CLAUDE.md"), "Claude stuff").unwrap();

        let result =
            discover_project_docs_for_provider(tmp.path().to_str().unwrap(), "anthropic").await;
        let docs = result.unwrap();
        assert!(!docs.contains("GEMINI.md"));
        assert!(docs.contains("# From CLAUDE.md"));
    }

    #[tokio::test]
    async fn provider_filter_openai_includes_cursorrules() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join(".cursorrules"), "OpenAI rules").unwrap();
        fs::write(tmp.path().join("AGENTS.md"), "Universal").unwrap();

        let result =
            discover_project_docs_for_provider(tmp.path().to_str().unwrap(), "openai").await;
        let docs = result.unwrap();
        assert!(docs.contains("# From .cursorrules"));
        assert!(docs.contains("OpenAI rules"));
        assert!(docs.contains("# From AGENTS.md"));
    }

    #[tokio::test]
    async fn provider_filter_openai_excludes_claude_md() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("CLAUDE.md"), "Anthropic only").unwrap();
        fs::write(tmp.path().join(".cursorrules"), "OpenAI rules").unwrap();

        let result =
            discover_project_docs_for_provider(tmp.path().to_str().unwrap(), "openai").await;
        let docs = result.unwrap();
        assert!(!docs.contains("CLAUDE.md"));
        assert!(docs.contains("# From .cursorrules"));
    }

    #[tokio::test]
    async fn provider_filter_gemini_includes_gemini_md() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("GEMINI.md"), "Gemini rules").unwrap();
        fs::write(tmp.path().join("CONVENTIONS.md"), "Universal").unwrap();

        let result =
            discover_project_docs_for_provider(tmp.path().to_str().unwrap(), "gemini").await;
        let docs = result.unwrap();
        assert!(docs.contains("# From GEMINI.md"));
        assert!(docs.contains("Gemini rules"));
        assert!(docs.contains("# From CONVENTIONS.md"));
    }

    #[tokio::test]
    async fn provider_filter_gemini_excludes_claude_and_cursorrules() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("CLAUDE.md"), "Anthropic").unwrap();
        fs::write(tmp.path().join(".cursorrules"), "OpenAI").unwrap();
        fs::write(tmp.path().join("GEMINI.md"), "Gemini").unwrap();

        let result =
            discover_project_docs_for_provider(tmp.path().to_str().unwrap(), "gemini").await;
        let docs = result.unwrap();
        assert!(!docs.contains("CLAUDE.md"));
        assert!(!docs.contains(".cursorrules"));
        assert!(docs.contains("# From GEMINI.md"));
    }

    #[tokio::test]
    async fn provider_filter_universal_docs_loaded_for_all_providers() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("AGENTS.md"), "Agents").unwrap();
        fs::write(tmp.path().join("CONVENTIONS.md"), "Conventions").unwrap();
        fs::write(tmp.path().join("AI_INSTRUCTIONS.md"), "Instructions").unwrap();

        for provider in &["anthropic", "openai", "gemini"] {
            let result =
                discover_project_docs_for_provider(tmp.path().to_str().unwrap(), provider).await;
            let docs = result.unwrap();
            assert!(
                docs.contains("# From AGENTS.md"),
                "AGENTS.md should be loaded for {provider}"
            );
            assert!(
                docs.contains("# From CONVENTIONS.md"),
                "CONVENTIONS.md should be loaded for {provider}"
            );
            assert!(
                docs.contains("# From AI_INSTRUCTIONS.md"),
                "AI_INSTRUCTIONS.md should be loaded for {provider}"
            );
        }
    }

    #[tokio::test]
    async fn provider_filter_returns_none_when_no_matching_docs() {
        let tmp = TempDir::new().unwrap();
        // Only create Anthropic-specific doc
        fs::write(tmp.path().join("CLAUDE.md"), "Anthropic only").unwrap();

        let result =
            discover_project_docs_for_provider(tmp.path().to_str().unwrap(), "gemini").await;
        assert!(result.is_none(), "Gemini should not see CLAUDE.md");
    }

    #[tokio::test]
    async fn provider_filter_anthropic_reads_claude_directory() {
        let tmp = TempDir::new().unwrap();
        fs::create_dir_all(tmp.path().join(".claude")).unwrap();
        fs::write(
            tmp.path().join(".claude/settings.md"),
            "Claude settings content",
        )
        .unwrap();

        let result =
            discover_project_docs_for_provider(tmp.path().to_str().unwrap(), "anthropic").await;
        let docs = result.unwrap();
        assert!(docs.contains("Claude settings content"));
        assert!(docs.contains(".claude/"));
    }

    #[tokio::test]
    async fn provider_filter_openai_does_not_read_claude_directory() {
        let tmp = TempDir::new().unwrap();
        fs::create_dir_all(tmp.path().join(".claude")).unwrap();
        fs::write(
            tmp.path().join(".claude/settings.md"),
            "Claude settings content",
        )
        .unwrap();

        let result =
            discover_project_docs_for_provider(tmp.path().to_str().unwrap(), "openai").await;
        assert!(
            result.is_none(),
            "OpenAI should not see .claude directory contents"
        );
    }

    #[tokio::test]
    async fn build_system_prompt_filters_docs_by_provider() {
        let tmp = TempDir::new().unwrap();
        let wd = tmp.path().to_str().unwrap();
        fs::write(tmp.path().join("CLAUDE.md"), "Anthropic-specific rules").unwrap();
        fs::write(tmp.path().join(".cursorrules"), "OpenAI-specific rules").unwrap();
        fs::write(tmp.path().join("AGENTS.md"), "Universal rules").unwrap();

        // Anthropic profile should see CLAUDE.md but not .cursorrules
        let config = SessionConfig::default();
        let profile = AnthropicProfile;
        let prompt = build_system_prompt(&config, &profile, Some(wd)).await;
        assert!(prompt.contains("Anthropic-specific rules"));
        assert!(prompt.contains("Universal rules"));
        assert!(!prompt.contains("OpenAI-specific rules"));

        // OpenAI profile should see .cursorrules but not CLAUDE.md
        let profile = crate::profile::OpenAiProfile;
        let prompt = build_system_prompt(&config, &profile, Some(wd)).await;
        assert!(prompt.contains("OpenAI-specific rules"));
        assert!(prompt.contains("Universal rules"));
        assert!(!prompt.contains("Anthropic-specific rules"));
    }

    #[tokio::test]
    async fn build_system_prompt_gemini_sees_gemini_md() {
        let tmp = TempDir::new().unwrap();
        let wd = tmp.path().to_str().unwrap();
        fs::write(tmp.path().join("GEMINI.md"), "Gemini-specific instructions").unwrap();
        fs::write(tmp.path().join("CLAUDE.md"), "Anthropic only").unwrap();

        let config = SessionConfig::default();
        let profile = crate::profile::GeminiProfile;
        let prompt = build_system_prompt(&config, &profile, Some(wd)).await;
        assert!(prompt.contains("Gemini-specific instructions"));
        assert!(!prompt.contains("Anthropic only"));
    }

    #[tokio::test]
    async fn provider_filter_nonexistent_directory() {
        let result = discover_project_docs_for_provider(
            "/nonexistent/path/that/does/not/exist",
            "anthropic",
        )
        .await;
        assert!(result.is_none());
    }
}
