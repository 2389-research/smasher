// ABOUTME: Hardcoded catalog of known LLM models with their capabilities and provider metadata.
// ABOUTME: Provides lookup, inference, and enumeration functions for the unified LLM client.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;
use std::sync::LazyLock;

/// Identifies which LLM provider serves a model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    Anthropic,
    OpenAi,
    Gemini,
}

impl fmt::Display for Provider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Provider::Anthropic => write!(f, "anthropic"),
            Provider::OpenAi => write!(f, "openai"),
            Provider::Gemini => write!(f, "gemini"),
        }
    }
}

impl FromStr for Provider {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "anthropic" => Ok(Provider::Anthropic),
            "openai" => Ok(Provider::OpenAi),
            "gemini" | "google" => Ok(Provider::Gemini),
            other => Err(format!("unknown provider: {other}")),
        }
    }
}

/// Information about a known model.
#[derive(Debug, Clone)]
pub struct ModelInfo {
    /// Model ID string (e.g. "claude-sonnet-4-5-20250929").
    pub id: &'static str,
    /// Which provider serves this model.
    pub provider: Provider,
    /// Human-readable display name.
    pub display_name: &'static str,
    /// Alternative names that resolve to this model (e.g. "claude-sonnet" for a dated model ID).
    pub aliases: &'static [&'static str],
    /// Maximum context window in tokens.
    pub context_window: u32,
    /// Maximum output tokens the model can generate.
    pub max_output_tokens: u32,
    /// Whether the model supports image inputs.
    pub supports_images: bool,
    /// Whether the model supports tool/function calling.
    pub supports_tool_use: bool,
    /// Whether the model supports streaming responses.
    pub supports_streaming: bool,
    /// Whether the model supports extended thinking (Anthropic) or reasoning (OpenAI o-series).
    pub supports_thinking: bool,
    /// Whether the model supports reasoning/chain-of-thought natively.
    pub supports_reasoning: bool,
    /// Whether the model supports structured JSON output mode.
    pub supports_json_mode: bool,
    /// Whether the model supports a system prompt.
    pub supports_system_prompt: bool,
    /// Cost per million input tokens in USD, if known.
    pub input_cost_per_million: Option<f64>,
    /// Cost per million output tokens in USD, if known.
    pub output_cost_per_million: Option<f64>,
}

static CATALOG: LazyLock<Vec<ModelInfo>> = LazyLock::new(|| {
    vec![
        // ── Anthropic models ──────────────────────────────────────────
        ModelInfo {
            id: "claude-opus-4-6",
            provider: Provider::Anthropic,
            display_name: "Claude Opus 4.6",
            aliases: &["claude-opus"],
            context_window: 200_000,
            max_output_tokens: 128_000,
            supports_images: true,
            supports_tool_use: true,
            supports_streaming: true,
            supports_thinking: true,
            supports_reasoning: true,
            supports_json_mode: true,
            supports_system_prompt: true,
            input_cost_per_million: Some(5.0),
            output_cost_per_million: Some(25.0),
        },
        ModelInfo {
            id: "claude-sonnet-4-5-20250929",
            provider: Provider::Anthropic,
            display_name: "Claude Sonnet 4.5",
            aliases: &["claude-sonnet-4-5", "claude-sonnet"],
            context_window: 200_000,
            max_output_tokens: 64_000,
            supports_images: true,
            supports_tool_use: true,
            supports_streaming: true,
            supports_thinking: true,
            supports_reasoning: true,
            supports_json_mode: true,
            supports_system_prompt: true,
            input_cost_per_million: Some(3.0),
            output_cost_per_million: Some(15.0),
        },
        ModelInfo {
            id: "claude-haiku-4-5-20251001",
            provider: Provider::Anthropic,
            display_name: "Claude Haiku 4.5",
            aliases: &["claude-haiku-4-5", "claude-haiku"],
            context_window: 200_000,
            max_output_tokens: 64_000,
            supports_images: true,
            supports_tool_use: true,
            supports_streaming: true,
            supports_thinking: true,
            supports_reasoning: true,
            supports_json_mode: true,
            supports_system_prompt: true,
            input_cost_per_million: Some(1.0),
            output_cost_per_million: Some(5.0),
        },
        // Legacy Anthropic models
        ModelInfo {
            id: "claude-sonnet-4-20250514",
            provider: Provider::Anthropic,
            display_name: "Claude Sonnet 4",
            aliases: &["claude-sonnet-4-0"],
            context_window: 200_000,
            max_output_tokens: 64_000,
            supports_images: true,
            supports_tool_use: true,
            supports_streaming: true,
            supports_thinking: true,
            supports_reasoning: true,
            supports_json_mode: true,
            supports_system_prompt: true,
            input_cost_per_million: Some(3.0),
            output_cost_per_million: Some(15.0),
        },
        ModelInfo {
            id: "claude-opus-4-20250514",
            provider: Provider::Anthropic,
            display_name: "Claude Opus 4",
            aliases: &["claude-opus-4-0"],
            context_window: 200_000,
            max_output_tokens: 32_000,
            supports_images: true,
            supports_tool_use: true,
            supports_streaming: true,
            supports_thinking: true,
            supports_reasoning: true,
            supports_json_mode: true,
            supports_system_prompt: true,
            input_cost_per_million: Some(15.0),
            output_cost_per_million: Some(75.0),
        },
        // ── OpenAI models ─────────────────────────────────────────────
        ModelInfo {
            id: "gpt-4.1",
            provider: Provider::OpenAi,
            display_name: "GPT-4.1",
            aliases: &[],
            context_window: 1_047_576,
            max_output_tokens: 32_768,
            supports_images: true,
            supports_tool_use: true,
            supports_streaming: true,
            supports_thinking: false,
            supports_reasoning: false,
            supports_json_mode: true,
            supports_system_prompt: true,
            input_cost_per_million: Some(2.0),
            output_cost_per_million: Some(8.0),
        },
        ModelInfo {
            id: "gpt-4.1-mini",
            provider: Provider::OpenAi,
            display_name: "GPT-4.1 Mini",
            aliases: &[],
            context_window: 1_047_576,
            max_output_tokens: 32_768,
            supports_images: true,
            supports_tool_use: true,
            supports_streaming: true,
            supports_thinking: false,
            supports_reasoning: false,
            supports_json_mode: true,
            supports_system_prompt: true,
            input_cost_per_million: Some(0.4),
            output_cost_per_million: Some(1.6),
        },
        ModelInfo {
            id: "gpt-4.1-nano",
            provider: Provider::OpenAi,
            display_name: "GPT-4.1 Nano",
            aliases: &[],
            context_window: 1_047_576,
            max_output_tokens: 32_768,
            supports_images: true,
            supports_tool_use: true,
            supports_streaming: true,
            supports_thinking: false,
            supports_reasoning: false,
            supports_json_mode: true,
            supports_system_prompt: true,
            input_cost_per_million: Some(0.1),
            output_cost_per_million: Some(0.4),
        },
        ModelInfo {
            id: "o3",
            provider: Provider::OpenAi,
            display_name: "o3",
            aliases: &[],
            context_window: 200_000,
            max_output_tokens: 100_000,
            supports_images: true,
            supports_tool_use: true,
            supports_streaming: true,
            supports_thinking: true,
            supports_reasoning: true,
            supports_json_mode: true,
            supports_system_prompt: true,
            input_cost_per_million: Some(2.0),
            output_cost_per_million: Some(8.0),
        },
        ModelInfo {
            id: "o3-mini",
            provider: Provider::OpenAi,
            display_name: "o3-mini",
            aliases: &[],
            context_window: 200_000,
            max_output_tokens: 100_000,
            supports_images: false,
            supports_tool_use: true,
            supports_streaming: true,
            supports_thinking: true,
            supports_reasoning: true,
            supports_json_mode: true,
            supports_system_prompt: true,
            input_cost_per_million: Some(1.1),
            output_cost_per_million: Some(4.4),
        },
        ModelInfo {
            id: "o4-mini",
            provider: Provider::OpenAi,
            display_name: "o4-mini",
            aliases: &[],
            context_window: 200_000,
            max_output_tokens: 100_000,
            supports_images: true,
            supports_tool_use: true,
            supports_streaming: true,
            supports_thinking: true,
            supports_reasoning: true,
            supports_json_mode: true,
            supports_system_prompt: true,
            input_cost_per_million: Some(1.1),
            output_cost_per_million: Some(4.4),
        },
        // Legacy OpenAI models
        ModelInfo {
            id: "gpt-4o",
            provider: Provider::OpenAi,
            display_name: "GPT-4o",
            aliases: &[],
            context_window: 128_000,
            max_output_tokens: 16_384,
            supports_images: true,
            supports_tool_use: true,
            supports_streaming: true,
            supports_thinking: false,
            supports_reasoning: false,
            supports_json_mode: true,
            supports_system_prompt: true,
            input_cost_per_million: Some(2.5),
            output_cost_per_million: Some(10.0),
        },
        ModelInfo {
            id: "gpt-4o-mini",
            provider: Provider::OpenAi,
            display_name: "GPT-4o Mini",
            aliases: &[],
            context_window: 128_000,
            max_output_tokens: 16_384,
            supports_images: true,
            supports_tool_use: true,
            supports_streaming: true,
            supports_thinking: false,
            supports_reasoning: false,
            supports_json_mode: true,
            supports_system_prompt: true,
            input_cost_per_million: Some(0.15),
            output_cost_per_million: Some(0.6),
        },
        // ── Gemini models ─────────────────────────────────────────────
        ModelInfo {
            id: "gemini-2.5-pro",
            provider: Provider::Gemini,
            display_name: "Gemini 2.5 Pro",
            aliases: &[],
            context_window: 1_048_576,
            max_output_tokens: 65_536,
            supports_images: true,
            supports_tool_use: true,
            supports_streaming: true,
            supports_thinking: true,
            supports_reasoning: true,
            supports_json_mode: true,
            supports_system_prompt: true,
            input_cost_per_million: Some(1.25),
            output_cost_per_million: Some(10.0),
        },
        ModelInfo {
            id: "gemini-2.5-flash",
            provider: Provider::Gemini,
            display_name: "Gemini 2.5 Flash",
            aliases: &[],
            context_window: 1_048_576,
            max_output_tokens: 65_536,
            supports_images: true,
            supports_tool_use: true,
            supports_streaming: true,
            supports_thinking: true,
            supports_reasoning: true,
            supports_json_mode: true,
            supports_system_prompt: true,
            input_cost_per_million: Some(0.3),
            output_cost_per_million: Some(2.5),
        },
        // Legacy Gemini models
        ModelInfo {
            id: "gemini-2.0-flash",
            provider: Provider::Gemini,
            display_name: "Gemini 2.0 Flash (Deprecated)",
            aliases: &[],
            context_window: 1_048_576,
            max_output_tokens: 8_192,
            supports_images: true,
            supports_tool_use: true,
            supports_streaming: true,
            supports_thinking: true,
            supports_reasoning: false,
            supports_json_mode: true,
            supports_system_prompt: true,
            input_cost_per_million: Some(0.1),
            output_cost_per_million: Some(0.4),
        },
    ]
});

/// Look up a model by its exact ID string or by alias. Returns None if not in the catalog.
pub fn lookup_model(model_id: &str) -> Option<&'static ModelInfo> {
    CATALOG
        .iter()
        .find(|m| m.id == model_id || m.aliases.contains(&model_id))
}

/// Infer the provider from a model ID string based on naming patterns.
///
/// - "claude-*" or "anthropic*" -> Anthropic
/// - "gpt-*" or "o1-*" or "o1" or "o3-*" or "o3" or "o4-*" or "o4" -> OpenAi
/// - "gemini-*" -> Gemini
pub fn infer_provider(model_id: &str) -> Option<Provider> {
    if model_id.starts_with("claude-") || model_id.starts_with("anthropic") {
        Some(Provider::Anthropic)
    } else if model_id.starts_with("gpt-")
        || model_id.starts_with("o1-")
        || model_id == "o1"
        || model_id.starts_with("o3-")
        || model_id == "o3"
        || model_id.starts_with("o4-")
        || model_id == "o4"
    {
        Some(Provider::OpenAi)
    } else if model_id.starts_with("gemini-") {
        Some(Provider::Gemini)
    } else {
        None
    }
}

/// Return the most capable model for a given provider.
///
/// "Most capable" is defined as the flagship/largest model currently offered by each provider:
/// - Anthropic: Claude Opus 4.6
/// - OpenAI: o3
/// - Gemini: Gemini 2.5 Pro
pub fn get_latest_model(provider: Provider) -> Option<&'static ModelInfo> {
    let target_id = match provider {
        Provider::Anthropic => "claude-opus-4-6",
        Provider::OpenAi => "o3",
        Provider::Gemini => "gemini-2.5-pro",
    };
    CATALOG.iter().find(|m| m.id == target_id)
}

/// Look up a model, returning a default ModelInfo for unknown models based on provider inference.
///
/// If the model ID is found in the catalog, returns a clone of that entry.
/// Otherwise, infers the provider from the model ID and returns a conservative default.
/// If the provider cannot be inferred, falls back to OpenAI defaults.
pub fn lookup_model_or_default(model_id: &str) -> ModelInfo {
    if let Some(info) = lookup_model(model_id) {
        return info.clone();
    }

    let provider = infer_provider(model_id).unwrap_or(Provider::OpenAi);

    match provider {
        Provider::Anthropic => ModelInfo {
            id: "unknown",
            provider: Provider::Anthropic,
            display_name: "Unknown Anthropic Model",
            aliases: &[],
            context_window: 200_000,
            max_output_tokens: 8_000,
            supports_images: true,
            supports_tool_use: true,
            supports_streaming: true,
            supports_thinking: false,
            supports_reasoning: false,
            supports_json_mode: true,
            supports_system_prompt: true,
            input_cost_per_million: None,
            output_cost_per_million: None,
        },
        Provider::OpenAi => ModelInfo {
            id: "unknown",
            provider: Provider::OpenAi,
            display_name: "Unknown OpenAI Model",
            aliases: &[],
            context_window: 128_000,
            max_output_tokens: 16_000,
            supports_images: true,
            supports_tool_use: true,
            supports_streaming: true,
            supports_thinking: false,
            supports_reasoning: false,
            supports_json_mode: true,
            supports_system_prompt: true,
            input_cost_per_million: None,
            output_cost_per_million: None,
        },
        Provider::Gemini => ModelInfo {
            id: "unknown",
            provider: Provider::Gemini,
            display_name: "Unknown Gemini Model",
            aliases: &[],
            context_window: 1_000_000,
            max_output_tokens: 8_000,
            supports_images: true,
            supports_tool_use: true,
            supports_streaming: true,
            supports_thinking: false,
            supports_reasoning: false,
            supports_json_mode: true,
            supports_system_prompt: true,
            input_cost_per_million: None,
            output_cost_per_million: None,
        },
    }
}

/// List all models for a given provider.
pub fn models_for_provider(provider: Provider) -> Vec<&'static ModelInfo> {
    CATALOG.iter().filter(|m| m.provider == provider).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Anthropic model lookups ───────────────────────────────────────

    #[test]
    fn lookup_claude_opus_4_6() {
        let info = lookup_model("claude-opus-4-6").unwrap();
        assert_eq!(info.id, "claude-opus-4-6");
        assert_eq!(info.provider, Provider::Anthropic);
        assert_eq!(info.display_name, "Claude Opus 4.6");
        assert_eq!(info.context_window, 200_000);
        assert_eq!(info.max_output_tokens, 128_000);
        assert!(info.supports_images);
        assert!(info.supports_tool_use);
        assert!(info.supports_streaming);
        assert!(info.supports_thinking);
        assert!(info.supports_reasoning);
        assert!(info.supports_json_mode);
        assert!(info.supports_system_prompt);
        assert_eq!(info.input_cost_per_million, Some(5.0));
        assert_eq!(info.output_cost_per_million, Some(25.0));
    }

    #[test]
    fn lookup_claude_sonnet_4_5() {
        let info = lookup_model("claude-sonnet-4-5-20250929").unwrap();
        assert_eq!(info.id, "claude-sonnet-4-5-20250929");
        assert_eq!(info.provider, Provider::Anthropic);
        assert_eq!(info.display_name, "Claude Sonnet 4.5");
        assert_eq!(info.context_window, 200_000);
        assert_eq!(info.max_output_tokens, 64_000);
        assert!(info.supports_thinking);
        assert!(info.supports_reasoning);
        assert_eq!(info.input_cost_per_million, Some(3.0));
        assert_eq!(info.output_cost_per_million, Some(15.0));
    }

    #[test]
    fn lookup_claude_haiku_4_5() {
        let info = lookup_model("claude-haiku-4-5-20251001").unwrap();
        assert_eq!(info.id, "claude-haiku-4-5-20251001");
        assert_eq!(info.provider, Provider::Anthropic);
        assert_eq!(info.display_name, "Claude Haiku 4.5");
        assert_eq!(info.context_window, 200_000);
        assert_eq!(info.max_output_tokens, 64_000);
        assert!(info.supports_thinking);
        assert!(info.supports_reasoning);
        assert_eq!(info.input_cost_per_million, Some(1.0));
        assert_eq!(info.output_cost_per_million, Some(5.0));
    }

    #[test]
    fn lookup_legacy_claude_sonnet_4() {
        let info = lookup_model("claude-sonnet-4-20250514").unwrap();
        assert_eq!(info.id, "claude-sonnet-4-20250514");
        assert_eq!(info.provider, Provider::Anthropic);
        assert_eq!(info.display_name, "Claude Sonnet 4");
        assert_eq!(info.context_window, 200_000);
        assert_eq!(info.max_output_tokens, 64_000);
        assert!(info.supports_thinking);
    }

    #[test]
    fn lookup_legacy_claude_opus_4() {
        let info = lookup_model("claude-opus-4-20250514").unwrap();
        assert_eq!(info.id, "claude-opus-4-20250514");
        assert_eq!(info.provider, Provider::Anthropic);
        assert_eq!(info.max_output_tokens, 32_000);
        assert!(info.supports_thinking);
    }

    // ── OpenAI model lookups ──────────────────────────────────────────

    #[test]
    fn lookup_gpt_4_1() {
        let info = lookup_model("gpt-4.1").unwrap();
        assert_eq!(info.id, "gpt-4.1");
        assert_eq!(info.provider, Provider::OpenAi);
        assert_eq!(info.display_name, "GPT-4.1");
        assert_eq!(info.context_window, 1_047_576);
        assert_eq!(info.max_output_tokens, 32_768);
        assert!(info.supports_images);
        assert!(info.supports_tool_use);
        assert!(!info.supports_thinking);
        assert!(!info.supports_reasoning);
        assert_eq!(info.input_cost_per_million, Some(2.0));
        assert_eq!(info.output_cost_per_million, Some(8.0));
    }

    #[test]
    fn lookup_gpt_4_1_mini() {
        let info = lookup_model("gpt-4.1-mini").unwrap();
        assert_eq!(info.id, "gpt-4.1-mini");
        assert_eq!(info.provider, Provider::OpenAi);
        assert_eq!(info.context_window, 1_047_576);
        assert_eq!(info.max_output_tokens, 32_768);
        assert_eq!(info.input_cost_per_million, Some(0.4));
        assert_eq!(info.output_cost_per_million, Some(1.6));
    }

    #[test]
    fn lookup_gpt_4_1_nano() {
        let info = lookup_model("gpt-4.1-nano").unwrap();
        assert_eq!(info.id, "gpt-4.1-nano");
        assert_eq!(info.provider, Provider::OpenAi);
        assert_eq!(info.context_window, 1_047_576);
        assert_eq!(info.max_output_tokens, 32_768);
        assert!(info.supports_images);
        assert!(!info.supports_reasoning);
        assert_eq!(info.input_cost_per_million, Some(0.1));
        assert_eq!(info.output_cost_per_million, Some(0.4));
    }

    #[test]
    fn lookup_o3() {
        let info = lookup_model("o3").unwrap();
        assert_eq!(info.id, "o3");
        assert_eq!(info.provider, Provider::OpenAi);
        assert_eq!(info.context_window, 200_000);
        assert_eq!(info.max_output_tokens, 100_000);
        assert!(info.supports_images);
        assert!(info.supports_thinking);
        assert!(info.supports_reasoning);
        assert_eq!(info.input_cost_per_million, Some(2.0));
        assert_eq!(info.output_cost_per_million, Some(8.0));
    }

    #[test]
    fn lookup_o3_mini() {
        let info = lookup_model("o3-mini").unwrap();
        assert_eq!(info.provider, Provider::OpenAi);
        assert_eq!(info.context_window, 200_000);
        assert_eq!(info.max_output_tokens, 100_000);
        assert!(!info.supports_images);
        assert!(info.supports_thinking);
        assert!(info.supports_reasoning);
        assert_eq!(info.input_cost_per_million, Some(1.1));
        assert_eq!(info.output_cost_per_million, Some(4.4));
    }

    #[test]
    fn lookup_o4_mini() {
        let info = lookup_model("o4-mini").unwrap();
        assert_eq!(info.id, "o4-mini");
        assert_eq!(info.provider, Provider::OpenAi);
        assert_eq!(info.context_window, 200_000);
        assert_eq!(info.max_output_tokens, 100_000);
        assert!(info.supports_images);
        assert!(info.supports_thinking);
        assert!(info.supports_reasoning);
        assert_eq!(info.input_cost_per_million, Some(1.1));
        assert_eq!(info.output_cost_per_million, Some(4.4));
    }

    #[test]
    fn lookup_legacy_gpt_4o() {
        let info = lookup_model("gpt-4o").unwrap();
        assert_eq!(info.id, "gpt-4o");
        assert_eq!(info.provider, Provider::OpenAi);
        assert_eq!(info.context_window, 128_000);
        assert!(!info.supports_thinking);
        assert!(!info.supports_reasoning);
    }

    // ── Gemini model lookups ──────────────────────────────────────────

    #[test]
    fn lookup_gemini_2_5_pro() {
        let info = lookup_model("gemini-2.5-pro").unwrap();
        assert_eq!(info.id, "gemini-2.5-pro");
        assert_eq!(info.provider, Provider::Gemini);
        assert_eq!(info.display_name, "Gemini 2.5 Pro");
        assert_eq!(info.context_window, 1_048_576);
        assert_eq!(info.max_output_tokens, 65_536);
        assert!(info.supports_images);
        assert!(info.supports_thinking);
        assert!(info.supports_reasoning);
        assert_eq!(info.input_cost_per_million, Some(1.25));
        assert_eq!(info.output_cost_per_million, Some(10.0));
    }

    #[test]
    fn lookup_gemini_2_5_flash() {
        let info = lookup_model("gemini-2.5-flash").unwrap();
        assert_eq!(info.id, "gemini-2.5-flash");
        assert_eq!(info.provider, Provider::Gemini);
        assert_eq!(info.display_name, "Gemini 2.5 Flash");
        assert_eq!(info.context_window, 1_048_576);
        assert_eq!(info.max_output_tokens, 65_536);
        assert!(info.supports_images);
        assert!(info.supports_thinking);
        assert!(info.supports_reasoning);
        assert_eq!(info.input_cost_per_million, Some(0.3));
        assert_eq!(info.output_cost_per_million, Some(2.5));
    }

    #[test]
    fn lookup_legacy_gemini_2_0_flash() {
        let info = lookup_model("gemini-2.0-flash").unwrap();
        assert_eq!(info.id, "gemini-2.0-flash");
        assert_eq!(info.provider, Provider::Gemini);
        assert_eq!(info.context_window, 1_048_576);
        assert!(!info.supports_reasoning);
    }

    // ── Lookup unknown ────────────────────────────────────────────────

    #[test]
    fn lookup_model_returns_none_for_unknown() {
        assert!(lookup_model("gpt-99-ultra").is_none());
        assert!(lookup_model("").is_none());
        assert!(lookup_model("nonexistent-model").is_none());
    }

    // ── Alias lookups ─────────────────────────────────────────────────

    #[test]
    fn lookup_by_alias_claude_opus() {
        let info = lookup_model("claude-opus").unwrap();
        assert_eq!(info.id, "claude-opus-4-6");
        assert_eq!(info.provider, Provider::Anthropic);
    }

    #[test]
    fn lookup_by_alias_claude_sonnet() {
        let info = lookup_model("claude-sonnet").unwrap();
        assert_eq!(info.id, "claude-sonnet-4-5-20250929");
        assert_eq!(info.provider, Provider::Anthropic);
    }

    #[test]
    fn lookup_by_alias_claude_haiku() {
        let info = lookup_model("claude-haiku").unwrap();
        assert_eq!(info.id, "claude-haiku-4-5-20251001");
        assert_eq!(info.provider, Provider::Anthropic);
    }

    #[test]
    fn lookup_by_alias_claude_sonnet_4_5() {
        let info = lookup_model("claude-sonnet-4-5").unwrap();
        assert_eq!(info.id, "claude-sonnet-4-5-20250929");
    }

    #[test]
    fn lookup_by_alias_claude_haiku_4_5() {
        let info = lookup_model("claude-haiku-4-5").unwrap();
        assert_eq!(info.id, "claude-haiku-4-5-20251001");
    }

    #[test]
    fn lookup_by_alias_legacy_claude_sonnet_4_0() {
        let info = lookup_model("claude-sonnet-4-0").unwrap();
        assert_eq!(info.id, "claude-sonnet-4-20250514");
    }

    #[test]
    fn lookup_by_alias_legacy_claude_opus_4_0() {
        let info = lookup_model("claude-opus-4-0").unwrap();
        assert_eq!(info.id, "claude-opus-4-20250514");
    }

    // ── get_latest_model ──────────────────────────────────────────────

    #[test]
    fn get_latest_model_anthropic() {
        let info = get_latest_model(Provider::Anthropic).unwrap();
        assert_eq!(info.id, "claude-opus-4-6");
        assert_eq!(info.display_name, "Claude Opus 4.6");
    }

    #[test]
    fn get_latest_model_openai() {
        let info = get_latest_model(Provider::OpenAi).unwrap();
        assert_eq!(info.id, "o3");
        assert_eq!(info.display_name, "o3");
    }

    #[test]
    fn get_latest_model_gemini() {
        let info = get_latest_model(Provider::Gemini).unwrap();
        assert_eq!(info.id, "gemini-2.5-pro");
        assert_eq!(info.display_name, "Gemini 2.5 Pro");
    }

    // ── lookup_model_or_default ───────────────────────────────────────

    #[test]
    fn lookup_model_or_default_returns_catalog_entry_for_known_model() {
        let info = lookup_model_or_default("claude-opus-4-6");
        assert_eq!(info.id, "claude-opus-4-6");
        assert_eq!(info.provider, Provider::Anthropic);
        assert_eq!(info.max_output_tokens, 128_000);
    }

    #[test]
    fn lookup_model_or_default_returns_catalog_entry_via_alias() {
        let info = lookup_model_or_default("claude-sonnet");
        assert_eq!(info.id, "claude-sonnet-4-5-20250929");
        assert_eq!(info.provider, Provider::Anthropic);
    }

    #[test]
    fn lookup_model_or_default_returns_anthropic_default_for_unknown_claude() {
        let info = lookup_model_or_default("claude-next-99");
        assert_eq!(info.id, "unknown");
        assert_eq!(info.provider, Provider::Anthropic);
        assert_eq!(info.context_window, 200_000);
        assert!(info.input_cost_per_million.is_none());
        assert!(info.output_cost_per_million.is_none());
    }

    #[test]
    fn lookup_model_or_default_returns_openai_default_for_unknown_gpt() {
        let info = lookup_model_or_default("gpt-5-turbo");
        assert_eq!(info.id, "unknown");
        assert_eq!(info.provider, Provider::OpenAi);
        assert_eq!(info.context_window, 128_000);
    }

    #[test]
    fn lookup_model_or_default_returns_gemini_default_for_unknown_gemini() {
        let info = lookup_model_or_default("gemini-99-ultra");
        assert_eq!(info.id, "unknown");
        assert_eq!(info.provider, Provider::Gemini);
        assert_eq!(info.context_window, 1_000_000);
    }

    #[test]
    fn lookup_model_or_default_falls_back_to_openai_for_unrecognized() {
        let info = lookup_model_or_default("totally-unknown-model");
        assert_eq!(info.id, "unknown");
        assert_eq!(info.provider, Provider::OpenAi);
    }

    // ── infer_provider ────────────────────────────────────────────────

    #[test]
    fn infer_provider_claude_prefix() {
        assert_eq!(
            infer_provider("claude-sonnet-4-5-20250929"),
            Some(Provider::Anthropic)
        );
        assert_eq!(infer_provider("claude-anything"), Some(Provider::Anthropic));
    }

    #[test]
    fn infer_provider_anthropic_prefix() {
        assert_eq!(
            infer_provider("anthropic-model-v1"),
            Some(Provider::Anthropic)
        );
    }

    #[test]
    fn infer_provider_gpt_prefix() {
        assert_eq!(infer_provider("gpt-4.1"), Some(Provider::OpenAi));
        assert_eq!(infer_provider("gpt-4.1-mini"), Some(Provider::OpenAi));
        assert_eq!(infer_provider("gpt-4o"), Some(Provider::OpenAi));
        assert_eq!(infer_provider("gpt-4o-mini"), Some(Provider::OpenAi));
    }

    #[test]
    fn infer_provider_o_series_prefixes() {
        assert_eq!(infer_provider("o1-preview"), Some(Provider::OpenAi));
        assert_eq!(infer_provider("o3-mini"), Some(Provider::OpenAi));
        assert_eq!(infer_provider("o3"), Some(Provider::OpenAi));
        assert_eq!(infer_provider("o4-mini"), Some(Provider::OpenAi));
        assert_eq!(infer_provider("o4-mega"), Some(Provider::OpenAi));
        assert_eq!(infer_provider("o1"), Some(Provider::OpenAi));
    }

    #[test]
    fn infer_provider_gemini_prefix() {
        assert_eq!(infer_provider("gemini-2.5-pro"), Some(Provider::Gemini));
        assert_eq!(infer_provider("gemini-2.5-flash"), Some(Provider::Gemini));
        assert_eq!(infer_provider("gemini-pro"), Some(Provider::Gemini));
    }

    #[test]
    fn infer_provider_returns_none_for_unrecognized() {
        assert_eq!(infer_provider("llama-3"), None);
        assert_eq!(infer_provider("mistral-7b"), None);
        assert_eq!(infer_provider(""), None);
    }

    // ── models_for_provider ───────────────────────────────────────────

    #[test]
    fn models_for_provider_anthropic() {
        let models = models_for_provider(Provider::Anthropic);
        assert_eq!(models.len(), 5);
        assert!(models.iter().all(|m| m.provider == Provider::Anthropic));
    }

    #[test]
    fn models_for_provider_openai() {
        let models = models_for_provider(Provider::OpenAi);
        assert_eq!(models.len(), 8);
        assert!(models.iter().all(|m| m.provider == Provider::OpenAi));
    }

    #[test]
    fn models_for_provider_gemini() {
        let models = models_for_provider(Provider::Gemini);
        assert_eq!(models.len(), 3);
        assert!(models.iter().all(|m| m.provider == Provider::Gemini));
    }

    // ── Provider display / serde ──────────────────────────────────────

    #[test]
    fn provider_display() {
        assert_eq!(Provider::Anthropic.to_string(), "anthropic");
        assert_eq!(Provider::OpenAi.to_string(), "openai");
        assert_eq!(Provider::Gemini.to_string(), "gemini");
    }

    #[test]
    fn provider_serde_roundtrip() {
        for provider in [Provider::Anthropic, Provider::OpenAi, Provider::Gemini] {
            let json = serde_json::to_string(&provider).unwrap();
            let back: Provider = serde_json::from_str(&json).unwrap();
            assert_eq!(provider, back);
        }
    }

    #[test]
    fn provider_serializes_to_snake_case() {
        assert_eq!(
            serde_json::to_string(&Provider::Anthropic).unwrap(),
            "\"anthropic\""
        );
        assert_eq!(
            serde_json::to_string(&Provider::OpenAi).unwrap(),
            "\"open_ai\""
        );
        assert_eq!(
            serde_json::to_string(&Provider::Gemini).unwrap(),
            "\"gemini\""
        );
    }

    // ── Catalog-wide validation ───────────────────────────────────────

    #[test]
    fn all_catalog_models_have_valid_fields() {
        for model in CATALOG.iter() {
            assert!(!model.id.is_empty(), "Model ID should not be empty");
            assert!(
                !model.display_name.is_empty(),
                "Display name should not be empty"
            );
            assert!(
                model.context_window > 0,
                "Context window should be positive for {}",
                model.id
            );
            assert!(
                model.max_output_tokens > 0,
                "Max output tokens should be positive for {}",
                model.id
            );
        }
    }

    #[test]
    fn all_catalog_models_have_pricing() {
        for model in CATALOG.iter() {
            assert!(
                model.input_cost_per_million.is_some(),
                "Input cost should be set for {}",
                model.id
            );
            assert!(
                model.output_cost_per_million.is_some(),
                "Output cost should be set for {}",
                model.id
            );
        }
    }

    #[test]
    fn all_aliases_are_unique_across_catalog() {
        let mut seen_aliases = std::collections::HashSet::new();
        for model in CATALOG.iter() {
            for alias in model.aliases {
                assert!(
                    seen_aliases.insert(*alias),
                    "Duplicate alias '{}' found in catalog",
                    alias
                );
            }
        }
    }

    #[test]
    fn no_alias_conflicts_with_model_ids() {
        let model_ids: std::collections::HashSet<&str> = CATALOG.iter().map(|m| m.id).collect();
        for model in CATALOG.iter() {
            for alias in model.aliases {
                assert!(
                    !model_ids.contains(alias),
                    "Alias '{}' conflicts with a model ID",
                    alias
                );
            }
        }
    }

    // ── supports_reasoning field ──────────────────────────────────────

    #[test]
    fn reasoning_models_are_marked_correctly() {
        // Reasoning models
        for id in &[
            "claude-opus-4-6",
            "o3",
            "o3-mini",
            "o4-mini",
            "gemini-2.5-pro",
        ] {
            let info = lookup_model(id).unwrap();
            assert!(info.supports_reasoning, "{} should support reasoning", id);
        }
        // Non-reasoning models
        for id in &[
            "gpt-4.1",
            "gpt-4.1-mini",
            "gpt-4.1-nano",
            "gpt-4o",
            "gpt-4o-mini",
        ] {
            let info = lookup_model(id).unwrap();
            assert!(
                !info.supports_reasoning,
                "{} should not support reasoning",
                id
            );
        }
    }

    #[test]
    fn claude_opus_4_6_is_most_capable_anthropic() {
        let opus = lookup_model("claude-opus-4-6").unwrap();
        let sonnet = lookup_model("claude-sonnet-4-5-20250929").unwrap();
        let haiku = lookup_model("claude-haiku-4-5-20251001").unwrap();
        assert!(opus.max_output_tokens >= sonnet.max_output_tokens);
        assert!(opus.max_output_tokens >= haiku.max_output_tokens);
    }

    // ── Provider FromStr ─────────────────────────────────────────────

    #[test]
    fn provider_from_str_known_names() {
        assert_eq!(
            "anthropic".parse::<Provider>().unwrap(),
            Provider::Anthropic
        );
        assert_eq!("openai".parse::<Provider>().unwrap(), Provider::OpenAi);
        assert_eq!("gemini".parse::<Provider>().unwrap(), Provider::Gemini);
        assert_eq!("google".parse::<Provider>().unwrap(), Provider::Gemini);
    }

    #[test]
    fn provider_from_str_case_insensitive() {
        assert_eq!(
            "Anthropic".parse::<Provider>().unwrap(),
            Provider::Anthropic
        );
        assert_eq!("OPENAI".parse::<Provider>().unwrap(), Provider::OpenAi);
        assert_eq!("Gemini".parse::<Provider>().unwrap(), Provider::Gemini);
        assert_eq!("GOOGLE".parse::<Provider>().unwrap(), Provider::Gemini);
    }

    #[test]
    fn provider_from_str_unknown_errors() {
        assert!("martian".parse::<Provider>().is_err());
        assert!("".parse::<Provider>().is_err());
        assert!("gpt".parse::<Provider>().is_err());
    }

    #[test]
    fn provider_from_str_roundtrips_with_display() {
        for provider in [Provider::Anthropic, Provider::OpenAi, Provider::Gemini] {
            let s = provider.to_string();
            let back: Provider = s.parse().unwrap();
            assert_eq!(provider, back);
        }
    }
}
