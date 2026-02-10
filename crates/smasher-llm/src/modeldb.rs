// ABOUTME: Live model database that fetches model capabilities from OpenRouter API.
// ABOUTME: Provides cached model info with disk persistence and fallback to static catalog.

use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::types::catalog::{self, ModelInfo, Provider};

/// Per-token pricing as strings from the OpenRouter API.
const TOKENS_PER_MILLION: f64 = 1_000_000.0;

/// Default TTL for cached model data (24 hours).
pub const DEFAULT_TTL: Duration = Duration::from_secs(24 * 60 * 60);

/// Default OpenRouter API endpoint for model listing.
pub const OPENROUTER_MODELS_URL: &str = "https://openrouter.ai/api/v1/models";

/// Cache filename written inside the cache directory.
const CACHE_FILENAME: &str = "modeldb_cache.json";

// ── Error type ───────────────────────────────────────────────────────────

/// Errors that can occur during model database operations.
#[derive(Debug, thiserror::Error)]
pub enum ModelDbError {
    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("JSON parse failed: {0}")]
    Json(#[from] serde_json::Error),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("no models found in response")]
    EmptyResponse,
}

// ── OpenRouter response types ────────────────────────────────────────────

/// Top-level response from the OpenRouter `/api/v1/models` endpoint.
#[derive(Debug, Deserialize)]
pub struct OpenRouterResponse {
    pub data: Vec<OpenRouterModel>,
}

/// A single model entry from the OpenRouter API.
#[derive(Debug, Deserialize)]
pub struct OpenRouterModel {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub context_length: u32,
    #[serde(default)]
    pub pricing: Option<OpenRouterPricing>,
    #[serde(default)]
    pub top_provider: Option<OpenRouterTopProvider>,
    #[serde(default)]
    pub architecture: Option<OpenRouterArchitecture>,
}

/// Pricing information from OpenRouter (per-token costs as strings).
#[derive(Debug, Deserialize)]
pub struct OpenRouterPricing {
    #[serde(default)]
    pub prompt: Option<String>,
    #[serde(default)]
    pub completion: Option<String>,
}

/// Top provider details from OpenRouter.
#[derive(Debug, Deserialize)]
pub struct OpenRouterTopProvider {
    #[serde(default)]
    pub max_completion_tokens: Option<u32>,
}

/// Architecture/modality info from OpenRouter.
#[derive(Debug, Deserialize)]
pub struct OpenRouterArchitecture {
    #[serde(default)]
    pub modality: Option<String>,
    #[serde(default)]
    pub input_modalities: Option<Vec<String>>,
    #[serde(default)]
    pub output_modalities: Option<Vec<String>>,
}

// ── DynamicModelInfo ─────────────────────────────────────────────────────

/// Owned version of ModelInfo for dynamically-fetched model data.
///
/// Unlike the static catalog's ModelInfo which uses `&'static str`, this uses
/// owned Strings so it can hold data fetched at runtime from OpenRouter.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DynamicModelInfo {
    pub id: String,
    pub provider: Provider,
    pub display_name: String,
    pub context_window: u32,
    pub max_output_tokens: u32,
    pub supports_images: bool,
    pub supports_tool_use: bool,
    pub supports_streaming: bool,
    pub supports_thinking: bool,
    pub supports_reasoning: bool,
    pub supports_json_mode: bool,
    pub supports_system_prompt: bool,
    pub input_cost_per_million: Option<f64>,
    pub output_cost_per_million: Option<f64>,
}

impl DynamicModelInfo {
    /// Convert a static catalog ModelInfo into an owned DynamicModelInfo.
    pub fn from_static(info: &ModelInfo) -> Self {
        Self {
            id: info.id.to_owned(),
            provider: info.provider,
            display_name: info.display_name.to_owned(),
            context_window: info.context_window,
            max_output_tokens: info.max_output_tokens,
            supports_images: info.supports_images,
            supports_tool_use: info.supports_tool_use,
            supports_streaming: info.supports_streaming,
            supports_thinking: info.supports_thinking,
            supports_reasoning: info.supports_reasoning,
            supports_json_mode: info.supports_json_mode,
            supports_system_prompt: info.supports_system_prompt,
            input_cost_per_million: info.input_cost_per_million,
            output_cost_per_million: info.output_cost_per_million,
        }
    }
}

// ── Conversion helpers ───────────────────────────────────────────────────

/// Infer the Provider from an OpenRouter model ID prefix.
///
/// OpenRouter model IDs are formatted as `provider/model-name`, e.g.
/// `anthropic/claude-sonnet-4-20250514` or `openai/gpt-4o`.
pub fn provider_from_openrouter_id(id: &str) -> Option<Provider> {
    let prefix = id.split('/').next()?;
    match prefix {
        "anthropic" => Some(Provider::Anthropic),
        "openai" => Some(Provider::OpenAi),
        "google" => Some(Provider::Gemini),
        _ => None,
    }
}

/// Extract the bare model name from an OpenRouter ID by stripping the provider prefix.
///
/// For example, `anthropic/claude-sonnet-4-20250514` becomes `claude-sonnet-4-20250514`.
pub fn strip_openrouter_prefix(id: &str) -> &str {
    id.split_once('/').map_or(id, |(_, name)| name)
}

/// Convert a per-token price string to a per-million-tokens float.
///
/// OpenRouter returns pricing as decimal strings representing cost per token.
/// We multiply by 1,000,000 to get cost per million tokens.
pub fn per_token_to_per_million(per_token_str: &str) -> Option<f64> {
    let per_token: f64 = per_token_str.parse().ok()?;
    if per_token < 0.0 {
        return None;
    }
    Some(per_token * TOKENS_PER_MILLION)
}

/// Check whether an OpenRouter model supports image inputs based on its architecture.
fn supports_images_from_architecture(arch: &Option<OpenRouterArchitecture>) -> bool {
    match arch {
        Some(a) => {
            // Check input_modalities for "image"
            if let Some(ref modalities) = a.input_modalities {
                return modalities.iter().any(|m| m == "image");
            }
            // Fall back to checking modality string for "+image"
            if let Some(ref modality) = a.modality {
                return modality.contains("image");
            }
            false
        }
        None => false,
    }
}

/// Convert a single OpenRouterModel into a DynamicModelInfo.
///
/// Returns None if the provider cannot be determined from the model ID.
pub fn convert_openrouter_model(model: &OpenRouterModel) -> Option<DynamicModelInfo> {
    let provider = provider_from_openrouter_id(&model.id)?;
    let bare_id = strip_openrouter_prefix(&model.id);

    let max_output = model
        .top_provider
        .as_ref()
        .and_then(|tp| tp.max_completion_tokens)
        .unwrap_or(4096);

    let input_cost = model
        .pricing
        .as_ref()
        .and_then(|p| p.prompt.as_deref())
        .and_then(per_token_to_per_million);

    let output_cost = model
        .pricing
        .as_ref()
        .and_then(|p| p.completion.as_deref())
        .and_then(per_token_to_per_million);

    let supports_images = supports_images_from_architecture(&model.architecture);

    Some(DynamicModelInfo {
        id: bare_id.to_owned(),
        provider,
        display_name: model.name.clone(),
        context_window: model.context_length,
        max_output_tokens: max_output,
        supports_images,
        // Conservative defaults for capabilities we cannot determine from OpenRouter data
        supports_tool_use: true,
        supports_streaming: true,
        supports_thinking: false,
        supports_reasoning: false,
        supports_json_mode: true,
        supports_system_prompt: true,
        input_cost_per_million: input_cost,
        output_cost_per_million: output_cost,
    })
}

// ── Disk cache envelope ──────────────────────────────────────────────────

/// On-disk representation of the model database cache.
#[derive(Debug, Serialize, Deserialize)]
struct CacheEnvelope {
    fetched_at: DateTime<Utc>,
    models: Vec<DynamicModelInfo>,
}

// ── ModelDb ──────────────────────────────────────────────────────────────

/// Live model database backed by the OpenRouter API with disk caching.
///
/// The database fetches model information from OpenRouter, caches it to disk,
/// and provides lookup methods. When the cache is stale (beyond TTL) it will
/// re-fetch. If fetching fails, stale cache data is used as a fallback.
pub struct ModelDb {
    models: Vec<DynamicModelInfo>,
    fetched_at: Option<DateTime<Utc>>,
    cache_path: Option<PathBuf>,
    ttl: Duration,
}

impl ModelDb {
    /// Create a ModelDb instance.
    ///
    /// If `cache_path` points to a directory containing a valid, non-stale cache
    /// file, the cached data will be loaded immediately.
    pub fn new(cache_path: Option<PathBuf>, ttl: Duration) -> Self {
        let mut db = Self {
            models: Vec::new(),
            fetched_at: None,
            cache_path,
            ttl,
        };

        // Attempt to load from disk cache on construction
        if let Some(ref dir) = db.cache_path {
            let file = dir.join(CACHE_FILENAME);
            if file.exists()
                && let Ok(loaded) = Self::load_cache_file(&file)
            {
                db.models = loaded.models;
                db.fetched_at = Some(loaded.fetched_at);
            }
        }

        db
    }

    /// Fetch model data from the OpenRouter API.
    ///
    /// On success, updates the in-memory model list and writes to disk cache
    /// (if a cache path is configured).
    pub async fn fetch(&mut self) -> Result<(), ModelDbError> {
        let resp = reqwest::get(OPENROUTER_MODELS_URL)
            .await?
            .json::<OpenRouterResponse>()
            .await?;

        if resp.data.is_empty() {
            return Err(ModelDbError::EmptyResponse);
        }

        let models: Vec<DynamicModelInfo> = resp
            .data
            .iter()
            .filter_map(convert_openrouter_model)
            .collect();

        if models.is_empty() {
            return Err(ModelDbError::EmptyResponse);
        }

        let now = Utc::now();
        self.models = models;
        self.fetched_at = Some(now);

        // Persist to disk
        if let Some(ref dir) = self.cache_path {
            let _ = self.write_cache_file(&dir.join(CACHE_FILENAME));
        }

        Ok(())
    }

    /// Return all models currently in the database.
    pub fn models(&self) -> &[DynamicModelInfo] {
        &self.models
    }

    /// Look up a model by its bare ID (without OpenRouter provider prefix).
    pub fn lookup(&self, id: &str) -> Option<&DynamicModelInfo> {
        self.models.iter().find(|m| m.id == id)
    }

    /// Check whether the cached data has exceeded its TTL.
    pub fn is_stale(&self) -> bool {
        match self.fetched_at {
            None => true,
            Some(fetched) => {
                let elapsed = Utc::now()
                    .signed_duration_since(fetched)
                    .to_std()
                    .unwrap_or(Duration::MAX);
                elapsed > self.ttl
            }
        }
    }

    /// Refresh the database from OpenRouter if the cache is stale.
    ///
    /// If the fetch fails but stale cache data exists, the stale data is kept
    /// rather than leaving the database empty.
    pub async fn refresh_if_stale(&mut self) -> Result<(), ModelDbError> {
        if self.is_stale() {
            let had_data = !self.models.is_empty();
            match self.fetch().await {
                Ok(()) => Ok(()),
                Err(e) if had_data => {
                    // Keep stale data as fallback
                    tracing::warn!("Failed to refresh model database, using stale cache: {}", e);
                    Ok(())
                }
                Err(e) => Err(e),
            }
        } else {
            Ok(())
        }
    }

    /// Write the current state to a JSON snapshot file.
    ///
    /// This is useful for capturing the model database state alongside a
    /// pipeline run's artifacts.
    pub fn snapshot_to_file(&self, path: &Path) -> Result<(), ModelDbError> {
        let envelope = CacheEnvelope {
            fetched_at: self.fetched_at.unwrap_or_else(Utc::now),
            models: self.models.clone(),
        };
        let json = serde_json::to_string_pretty(&envelope)?;
        std::fs::write(path, json)?;
        Ok(())
    }

    /// Load a ModelDb from a previously-saved snapshot file.
    pub fn load_snapshot(path: &Path) -> Result<Self, ModelDbError> {
        let envelope = Self::load_cache_file(path)?;
        Ok(Self {
            models: envelope.models,
            fetched_at: Some(envelope.fetched_at),
            cache_path: None,
            ttl: DEFAULT_TTL,
        })
    }

    // ── Internal helpers ─────────────────────────────────────────────────

    fn write_cache_file(&self, path: &Path) -> Result<(), ModelDbError> {
        let envelope = CacheEnvelope {
            fetched_at: self.fetched_at.unwrap_or_else(Utc::now),
            models: self.models.clone(),
        };
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(&envelope)?;
        std::fs::write(path, json)?;
        Ok(())
    }

    fn load_cache_file(path: &Path) -> Result<CacheEnvelope, ModelDbError> {
        let data = std::fs::read_to_string(path)?;
        let envelope: CacheEnvelope = serde_json::from_str(&data)?;
        Ok(envelope)
    }
}

// ── Integration with static catalog ──────────────────────────────────────

/// Look up a model by ID, falling back to dynamic models if not in the static catalog.
///
/// The static catalog is the source of truth. If a model is found there, it is
/// returned (converted to DynamicModelInfo). Dynamic models only supplement the
/// catalog with entries that do not exist statically.
pub fn lookup_with_dynamic(id: &str, dynamic: &[DynamicModelInfo]) -> Option<DynamicModelInfo> {
    // Static catalog takes priority
    if let Some(static_info) = catalog::lookup_model(id) {
        return Some(DynamicModelInfo::from_static(static_info));
    }
    // Fall back to dynamic models
    dynamic.iter().find(|m| m.id == id).cloned()
}

/// Merge static catalog models with dynamic models.
///
/// Static entries always take precedence: if a model ID exists in both the
/// static catalog and the dynamic list, the static version is used.
/// Dynamic entries add models not present in the static catalog.
pub fn merge_with_dynamic(dynamic: &[DynamicModelInfo]) -> Vec<DynamicModelInfo> {
    let static_models: Vec<DynamicModelInfo> = catalog::models_for_provider(Provider::Anthropic)
        .into_iter()
        .chain(catalog::models_for_provider(Provider::OpenAi))
        .chain(catalog::models_for_provider(Provider::Gemini))
        .map(DynamicModelInfo::from_static)
        .collect();

    let static_ids: std::collections::HashSet<String> =
        static_models.iter().map(|m| m.id.clone()).collect();

    let mut result = static_models;
    for dyn_model in dynamic {
        if !static_ids.contains(&dyn_model.id) {
            result.push(dyn_model.clone());
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Realistic OpenRouter fixture ─────────────────────────────────────

    const OPENROUTER_FIXTURE: &str = r#"{
        "data": [
            {
                "id": "anthropic/claude-sonnet-4-20250514",
                "name": "Claude Sonnet 4",
                "context_length": 200000,
                "pricing": {
                    "prompt": "0.000003",
                    "completion": "0.000015"
                },
                "top_provider": {
                    "max_completion_tokens": 64000
                },
                "architecture": {
                    "modality": "text+image->text",
                    "input_modalities": ["text", "image"],
                    "output_modalities": ["text"]
                }
            },
            {
                "id": "openai/gpt-4o",
                "name": "GPT-4o",
                "context_length": 128000,
                "pricing": {
                    "prompt": "0.0000025",
                    "completion": "0.00001"
                },
                "top_provider": {
                    "max_completion_tokens": 16384
                },
                "architecture": {
                    "modality": "text+image->text",
                    "input_modalities": ["text", "image"],
                    "output_modalities": ["text"]
                }
            },
            {
                "id": "google/gemini-2.5-pro",
                "name": "Gemini 2.5 Pro",
                "context_length": 1048576,
                "pricing": {
                    "prompt": "0.00000125",
                    "completion": "0.00001"
                },
                "top_provider": {
                    "max_completion_tokens": 65536
                },
                "architecture": {
                    "modality": "text+image->text",
                    "input_modalities": ["text", "image"],
                    "output_modalities": ["text"]
                }
            },
            {
                "id": "meta-llama/llama-3-70b",
                "name": "Llama 3 70B",
                "context_length": 8192,
                "pricing": {
                    "prompt": "0.0000008",
                    "completion": "0.0000008"
                },
                "top_provider": {
                    "max_completion_tokens": 4096
                },
                "architecture": {
                    "modality": "text->text",
                    "input_modalities": ["text"],
                    "output_modalities": ["text"]
                }
            },
            {
                "id": "anthropic/claude-opus-4-6",
                "name": "Claude Opus 4.6",
                "context_length": 200000,
                "pricing": {
                    "prompt": "0.000005",
                    "completion": "0.000025"
                },
                "top_provider": {
                    "max_completion_tokens": 128000
                },
                "architecture": {
                    "modality": "text+image->text",
                    "input_modalities": ["text", "image"],
                    "output_modalities": ["text"]
                }
            }
        ]
    }"#;

    fn parse_fixture() -> Vec<DynamicModelInfo> {
        let resp: OpenRouterResponse = serde_json::from_str(OPENROUTER_FIXTURE).unwrap();
        resp.data
            .iter()
            .filter_map(convert_openrouter_model)
            .collect()
    }

    // ── 1. DynamicModelInfo serialization roundtrip ──────────────────────

    #[test]
    fn dynamic_model_info_serde_roundtrip() {
        let info = DynamicModelInfo {
            id: "claude-sonnet-4-20250514".to_owned(),
            provider: Provider::Anthropic,
            display_name: "Claude Sonnet 4".to_owned(),
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
        };

        let json = serde_json::to_string(&info).unwrap();
        let deserialized: DynamicModelInfo = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.id, info.id);
        assert_eq!(deserialized.provider, info.provider);
        assert_eq!(deserialized.display_name, info.display_name);
        assert_eq!(deserialized.context_window, info.context_window);
        assert_eq!(deserialized.max_output_tokens, info.max_output_tokens);
        assert_eq!(deserialized.supports_images, info.supports_images);
        assert_eq!(deserialized.supports_tool_use, info.supports_tool_use);
        assert_eq!(deserialized.supports_streaming, info.supports_streaming);
        assert_eq!(deserialized.supports_thinking, info.supports_thinking);
        assert_eq!(deserialized.supports_reasoning, info.supports_reasoning);
        assert_eq!(deserialized.supports_json_mode, info.supports_json_mode);
        assert_eq!(
            deserialized.supports_system_prompt,
            info.supports_system_prompt
        );
        assert_eq!(
            deserialized.input_cost_per_million,
            info.input_cost_per_million
        );
        assert_eq!(
            deserialized.output_cost_per_million,
            info.output_cost_per_million
        );
    }

    #[test]
    fn dynamic_model_info_with_none_costs_roundtrips() {
        let info = DynamicModelInfo {
            id: "test-model".to_owned(),
            provider: Provider::OpenAi,
            display_name: "Test".to_owned(),
            context_window: 4096,
            max_output_tokens: 1024,
            supports_images: false,
            supports_tool_use: false,
            supports_streaming: false,
            supports_thinking: false,
            supports_reasoning: false,
            supports_json_mode: false,
            supports_system_prompt: false,
            input_cost_per_million: None,
            output_cost_per_million: None,
        };

        let json = serde_json::to_string(&info).unwrap();
        let back: DynamicModelInfo = serde_json::from_str(&json).unwrap();
        assert!(back.input_cost_per_million.is_none());
        assert!(back.output_cost_per_million.is_none());
    }

    // ── 2. OpenRouter response parsing ──────────────────────────────────

    #[test]
    fn parse_openrouter_response() {
        let resp: OpenRouterResponse = serde_json::from_str(OPENROUTER_FIXTURE).unwrap();
        assert_eq!(resp.data.len(), 5);
        assert_eq!(resp.data[0].id, "anthropic/claude-sonnet-4-20250514");
        assert_eq!(resp.data[0].name, "Claude Sonnet 4");
        assert_eq!(resp.data[0].context_length, 200_000);
    }

    #[test]
    fn parse_openrouter_pricing() {
        let resp: OpenRouterResponse = serde_json::from_str(OPENROUTER_FIXTURE).unwrap();
        let pricing = resp.data[0].pricing.as_ref().unwrap();
        assert_eq!(pricing.prompt.as_deref(), Some("0.000003"));
        assert_eq!(pricing.completion.as_deref(), Some("0.000015"));
    }

    #[test]
    fn parse_openrouter_architecture() {
        let resp: OpenRouterResponse = serde_json::from_str(OPENROUTER_FIXTURE).unwrap();
        let arch = resp.data[0].architecture.as_ref().unwrap();
        assert_eq!(arch.modality.as_deref(), Some("text+image->text"));
        let input_mods = arch.input_modalities.as_ref().unwrap();
        assert!(input_mods.contains(&"image".to_string()));
    }

    #[test]
    fn convert_openrouter_model_produces_correct_fields() {
        let models = parse_fixture();
        // Find the Claude Sonnet 4 entry
        let sonnet = models
            .iter()
            .find(|m| m.id == "claude-sonnet-4-20250514")
            .unwrap();
        assert_eq!(sonnet.provider, Provider::Anthropic);
        assert_eq!(sonnet.display_name, "Claude Sonnet 4");
        assert_eq!(sonnet.context_window, 200_000);
        assert_eq!(sonnet.max_output_tokens, 64_000);
        assert!(sonnet.supports_images);
        assert_eq!(sonnet.input_cost_per_million, Some(3.0));
        assert_eq!(sonnet.output_cost_per_million, Some(15.0));
    }

    #[test]
    fn convert_filters_unknown_providers() {
        let models = parse_fixture();
        // meta-llama models should be filtered out (unknown provider)
        assert!(
            models.iter().all(|m| m.id != "llama-3-70b"),
            "Unknown providers should be filtered out"
        );
    }

    #[test]
    fn convert_only_known_providers() {
        let models = parse_fixture();
        // Should have 4 models: 2 anthropic, 1 openai, 1 google (llama filtered)
        assert_eq!(models.len(), 4);
        assert!(models.iter().any(|m| m.id == "claude-sonnet-4-20250514"));
        assert!(models.iter().any(|m| m.id == "claude-opus-4-6"));
        assert!(models.iter().any(|m| m.id == "gpt-4o"));
        assert!(models.iter().any(|m| m.id == "gemini-2.5-pro"));
    }

    // ── 3. Provider inference from OpenRouter ID ────────────────────────

    #[test]
    fn provider_from_anthropic_prefix() {
        assert_eq!(
            provider_from_openrouter_id("anthropic/claude-sonnet-4-20250514"),
            Some(Provider::Anthropic)
        );
        assert_eq!(
            provider_from_openrouter_id("anthropic/claude-opus-4-6"),
            Some(Provider::Anthropic)
        );
    }

    #[test]
    fn provider_from_openai_prefix() {
        assert_eq!(
            provider_from_openrouter_id("openai/gpt-4o"),
            Some(Provider::OpenAi)
        );
        assert_eq!(
            provider_from_openrouter_id("openai/o3"),
            Some(Provider::OpenAi)
        );
    }

    #[test]
    fn provider_from_google_prefix() {
        assert_eq!(
            provider_from_openrouter_id("google/gemini-2.5-pro"),
            Some(Provider::Gemini)
        );
    }

    #[test]
    fn provider_from_unknown_prefix() {
        assert_eq!(provider_from_openrouter_id("meta-llama/llama-3-70b"), None);
        assert_eq!(provider_from_openrouter_id("mistralai/mistral-7b"), None);
    }

    #[test]
    fn provider_from_empty_id() {
        assert_eq!(provider_from_openrouter_id(""), None);
    }

    #[test]
    fn provider_from_id_without_slash() {
        assert_eq!(provider_from_openrouter_id("no-slash-model"), None);
    }

    // ── 4. Strip OpenRouter prefix ──────────────────────────────────────

    #[test]
    fn strip_prefix_standard() {
        assert_eq!(
            strip_openrouter_prefix("anthropic/claude-sonnet-4-20250514"),
            "claude-sonnet-4-20250514"
        );
    }

    #[test]
    fn strip_prefix_no_slash() {
        assert_eq!(strip_openrouter_prefix("no-slash"), "no-slash");
    }

    #[test]
    fn strip_prefix_multiple_slashes() {
        // Only the first slash is used as separator
        assert_eq!(
            strip_openrouter_prefix("provider/model/version"),
            "model/version"
        );
    }

    // ── 5. Pricing conversion ───────────────────────────────────────────

    #[test]
    fn per_token_to_per_million_basic() {
        // $0.000003 per token = $3.0 per million
        assert_eq!(per_token_to_per_million("0.000003"), Some(3.0));
    }

    #[test]
    fn per_token_to_per_million_small_value() {
        // $0.0000008 per token = $0.8 per million
        let result = per_token_to_per_million("0.0000008").unwrap();
        assert!((result - 0.8).abs() < 1e-9);
    }

    #[test]
    fn per_token_to_per_million_zero() {
        assert_eq!(per_token_to_per_million("0"), Some(0.0));
    }

    #[test]
    fn per_token_to_per_million_invalid_string() {
        assert_eq!(per_token_to_per_million("not-a-number"), None);
    }

    #[test]
    fn per_token_to_per_million_negative() {
        assert_eq!(per_token_to_per_million("-0.000001"), None);
    }

    #[test]
    fn per_token_to_per_million_empty() {
        assert_eq!(per_token_to_per_million(""), None);
    }

    #[test]
    fn pricing_conversion_matches_fixture() {
        let models = parse_fixture();
        // Claude Sonnet 4: prompt=0.000003, completion=0.000015
        let sonnet = models
            .iter()
            .find(|m| m.id == "claude-sonnet-4-20250514")
            .unwrap();
        assert_eq!(sonnet.input_cost_per_million, Some(3.0));
        assert_eq!(sonnet.output_cost_per_million, Some(15.0));

        // GPT-4o: prompt=0.0000025, completion=0.00001
        let gpt4o = models.iter().find(|m| m.id == "gpt-4o").unwrap();
        assert_eq!(gpt4o.input_cost_per_million, Some(2.5));
        assert_eq!(gpt4o.output_cost_per_million, Some(10.0));

        // Gemini 2.5 Pro: prompt=0.00000125, completion=0.00001
        let gemini = models.iter().find(|m| m.id == "gemini-2.5-pro").unwrap();
        assert_eq!(gemini.input_cost_per_million, Some(1.25));
        assert_eq!(gemini.output_cost_per_million, Some(10.0));
    }

    // ── 6. Image support detection ──────────────────────────────────────

    #[test]
    fn supports_images_with_image_in_input_modalities() {
        let arch = Some(OpenRouterArchitecture {
            modality: Some("text+image->text".to_string()),
            input_modalities: Some(vec!["text".to_string(), "image".to_string()]),
            output_modalities: Some(vec!["text".to_string()]),
        });
        assert!(supports_images_from_architecture(&arch));
    }

    #[test]
    fn supports_images_without_image_in_modalities() {
        let arch = Some(OpenRouterArchitecture {
            modality: Some("text->text".to_string()),
            input_modalities: Some(vec!["text".to_string()]),
            output_modalities: Some(vec!["text".to_string()]),
        });
        assert!(!supports_images_from_architecture(&arch));
    }

    #[test]
    fn supports_images_falls_back_to_modality_string() {
        let arch = Some(OpenRouterArchitecture {
            modality: Some("text+image->text".to_string()),
            input_modalities: None,
            output_modalities: None,
        });
        assert!(supports_images_from_architecture(&arch));
    }

    #[test]
    fn supports_images_none_architecture() {
        assert!(!supports_images_from_architecture(&None));
    }

    // ── 7. Cache write and read ─────────────────────────────────────────

    #[test]
    fn cache_write_and_read_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let cache_dir = dir.path().to_path_buf();

        let models = parse_fixture();
        let mut db = ModelDb::new(Some(cache_dir.clone()), DEFAULT_TTL);
        db.models = models.clone();
        db.fetched_at = Some(Utc::now());

        // Write cache
        let cache_file = cache_dir.join(CACHE_FILENAME);
        db.write_cache_file(&cache_file).unwrap();

        // Create a fresh db and verify it loads the cache
        let db2 = ModelDb::new(Some(cache_dir), DEFAULT_TTL);
        assert_eq!(db2.models().len(), models.len());
        assert!(db2.fetched_at.is_some());

        // Verify model data survived roundtrip
        let sonnet = db2.lookup("claude-sonnet-4-20250514").unwrap();
        assert_eq!(sonnet.provider, Provider::Anthropic);
        assert_eq!(sonnet.context_window, 200_000);
    }

    #[test]
    fn cache_loads_on_construction() {
        let dir = tempfile::tempdir().unwrap();
        let cache_dir = dir.path().to_path_buf();

        // Manually write a cache file
        let envelope = CacheEnvelope {
            fetched_at: Utc::now(),
            models: vec![DynamicModelInfo {
                id: "test-model".to_owned(),
                provider: Provider::OpenAi,
                display_name: "Test Model".to_owned(),
                context_window: 4096,
                max_output_tokens: 1024,
                supports_images: false,
                supports_tool_use: true,
                supports_streaming: true,
                supports_thinking: false,
                supports_reasoning: false,
                supports_json_mode: true,
                supports_system_prompt: true,
                input_cost_per_million: Some(1.0),
                output_cost_per_million: Some(2.0),
            }],
        };
        let json = serde_json::to_string_pretty(&envelope).unwrap();
        std::fs::write(cache_dir.join(CACHE_FILENAME), json).unwrap();

        let db = ModelDb::new(Some(cache_dir), DEFAULT_TTL);
        assert_eq!(db.models().len(), 1);
        assert_eq!(db.lookup("test-model").unwrap().display_name, "Test Model");
    }

    #[test]
    fn new_without_cache_path_starts_empty() {
        let db = ModelDb::new(None, DEFAULT_TTL);
        assert!(db.models().is_empty());
        assert!(db.fetched_at.is_none());
    }

    #[test]
    fn new_with_missing_cache_dir_starts_empty() {
        let db = ModelDb::new(Some(PathBuf::from("/nonexistent/path/cache")), DEFAULT_TTL);
        assert!(db.models().is_empty());
    }

    // ── 8. Stale detection ──────────────────────────────────────────────

    #[test]
    fn is_stale_when_no_data() {
        let db = ModelDb::new(None, DEFAULT_TTL);
        assert!(db.is_stale());
    }

    #[test]
    fn is_stale_when_fetched_recently() {
        let mut db = ModelDb::new(None, DEFAULT_TTL);
        db.fetched_at = Some(Utc::now());
        assert!(!db.is_stale());
    }

    #[test]
    fn is_stale_when_fetched_long_ago() {
        let mut db = ModelDb::new(None, Duration::from_secs(60));
        // Set fetched_at to 2 hours ago
        db.fetched_at = Some(Utc::now() - chrono::Duration::hours(2));
        assert!(db.is_stale());
    }

    #[test]
    fn is_stale_with_zero_ttl() {
        let mut db = ModelDb::new(None, Duration::ZERO);
        // Set fetched_at to 1 second ago to guarantee staleness
        db.fetched_at = Some(Utc::now() - chrono::Duration::seconds(1));
        assert!(db.is_stale());
    }

    // ── 9. Snapshot write and load ──────────────────────────────────────

    #[test]
    fn snapshot_write_and_load() {
        let dir = tempfile::tempdir().unwrap();
        let snapshot_path = dir.path().join("run-42-models.json");

        let models = parse_fixture();
        let mut db = ModelDb::new(None, DEFAULT_TTL);
        db.models = models;
        db.fetched_at = Some(Utc::now());

        // Write snapshot
        db.snapshot_to_file(&snapshot_path).unwrap();

        // Load snapshot
        let loaded = ModelDb::load_snapshot(&snapshot_path).unwrap();
        assert_eq!(loaded.models().len(), db.models().len());
        assert!(loaded.fetched_at.is_some());

        // Verify data
        let opus = loaded.lookup("claude-opus-4-6").unwrap();
        assert_eq!(opus.provider, Provider::Anthropic);
        assert_eq!(opus.display_name, "Claude Opus 4.6");
    }

    #[test]
    fn load_snapshot_from_nonexistent_file_fails() {
        let result = ModelDb::load_snapshot(Path::new("/nonexistent/snapshot.json"));
        assert!(result.is_err());
    }

    #[test]
    fn snapshot_without_fetched_at_uses_now() {
        let dir = tempfile::tempdir().unwrap();
        let snapshot_path = dir.path().join("snap.json");

        let mut db = ModelDb::new(None, DEFAULT_TTL);
        db.models = vec![DynamicModelInfo {
            id: "test".to_owned(),
            provider: Provider::OpenAi,
            display_name: "Test".to_owned(),
            context_window: 4096,
            max_output_tokens: 1024,
            supports_images: false,
            supports_tool_use: false,
            supports_streaming: false,
            supports_thinking: false,
            supports_reasoning: false,
            supports_json_mode: false,
            supports_system_prompt: false,
            input_cost_per_million: None,
            output_cost_per_million: None,
        }];
        // fetched_at is None

        db.snapshot_to_file(&snapshot_path).unwrap();
        let loaded = ModelDb::load_snapshot(&snapshot_path).unwrap();
        assert!(loaded.fetched_at.is_some());
        assert_eq!(loaded.models().len(), 1);
    }

    // ── 10. Merge with static catalog ───────────────────────────────────

    #[test]
    fn merge_static_takes_precedence() {
        let dynamic = parse_fixture();
        let merged = merge_with_dynamic(&dynamic);

        // The static catalog has claude-sonnet-4-20250514 with supports_thinking=true
        // The dynamic version has supports_thinking=false (default from OpenRouter)
        // Static should win.
        let sonnet = merged
            .iter()
            .find(|m| m.id == "claude-sonnet-4-20250514")
            .unwrap();
        // Static catalog has thinking=true, dynamic has thinking=false
        assert!(
            sonnet.supports_thinking,
            "Static catalog entry should take precedence"
        );
    }

    #[test]
    fn merge_includes_all_static_models() {
        let dynamic = vec![];
        let merged = merge_with_dynamic(&dynamic);

        // Should contain all static catalog entries
        assert!(merged.iter().any(|m| m.id == "claude-opus-4-6"));
        assert!(merged.iter().any(|m| m.id == "gpt-4.1"));
        assert!(merged.iter().any(|m| m.id == "gemini-2.5-pro"));
    }

    #[test]
    fn merge_adds_dynamic_models_not_in_static() {
        // Create a dynamic model that doesn't exist in the static catalog
        let dynamic = vec![DynamicModelInfo {
            id: "some-brand-new-model".to_owned(),
            provider: Provider::OpenAi,
            display_name: "Brand New".to_owned(),
            context_window: 256_000,
            max_output_tokens: 32_000,
            supports_images: true,
            supports_tool_use: true,
            supports_streaming: true,
            supports_thinking: false,
            supports_reasoning: false,
            supports_json_mode: true,
            supports_system_prompt: true,
            input_cost_per_million: Some(1.0),
            output_cost_per_million: Some(4.0),
        }];
        let merged = merge_with_dynamic(&dynamic);

        assert!(
            merged.iter().any(|m| m.id == "some-brand-new-model"),
            "Dynamic-only models should be included in merge"
        );
    }

    #[test]
    fn merge_does_not_duplicate_static_models() {
        let dynamic = parse_fixture();
        let merged = merge_with_dynamic(&dynamic);

        // Count how many times claude-sonnet-4-20250514 appears (should be exactly 1)
        let count = merged
            .iter()
            .filter(|m| m.id == "claude-sonnet-4-20250514")
            .count();
        assert_eq!(
            count, 1,
            "Should not duplicate models present in both sources"
        );
    }

    // ── 11. lookup_with_dynamic ─────────────────────────────────────────

    #[test]
    fn lookup_with_dynamic_prefers_static() {
        let dynamic = parse_fixture();
        // claude-opus-4-6 is in both static and dynamic
        let result = lookup_with_dynamic("claude-opus-4-6", &dynamic).unwrap();
        // Static has supports_thinking=true, dynamic has false
        assert!(
            result.supports_thinking,
            "Static catalog should take precedence in lookup"
        );
    }

    #[test]
    fn lookup_with_dynamic_falls_back_to_dynamic() {
        let dynamic = vec![DynamicModelInfo {
            id: "dynamic-only-model".to_owned(),
            provider: Provider::OpenAi,
            display_name: "Dynamic Only".to_owned(),
            context_window: 8192,
            max_output_tokens: 2048,
            supports_images: false,
            supports_tool_use: true,
            supports_streaming: true,
            supports_thinking: false,
            supports_reasoning: false,
            supports_json_mode: true,
            supports_system_prompt: true,
            input_cost_per_million: Some(0.5),
            output_cost_per_million: Some(1.0),
        }];

        let result = lookup_with_dynamic("dynamic-only-model", &dynamic).unwrap();
        assert_eq!(result.display_name, "Dynamic Only");
    }

    #[test]
    fn lookup_with_dynamic_returns_none_if_not_found() {
        let dynamic = parse_fixture();
        assert!(lookup_with_dynamic("completely-nonexistent", &dynamic).is_none());
    }

    #[test]
    fn lookup_with_dynamic_resolves_aliases() {
        let dynamic = vec![];
        // "claude-opus" is an alias for claude-opus-4-6 in the static catalog
        let result = lookup_with_dynamic("claude-opus", &dynamic).unwrap();
        assert_eq!(result.id, "claude-opus-4-6");
    }

    // ── 12. Empty response handling ─────────────────────────────────────

    #[test]
    fn parse_empty_data_array() {
        let json = r#"{"data": []}"#;
        let resp: OpenRouterResponse = serde_json::from_str(json).unwrap();
        assert!(resp.data.is_empty());
    }

    #[test]
    fn parse_missing_optional_fields() {
        let json = r#"{
            "data": [
                {
                    "id": "anthropic/claude-test",
                    "name": "Claude Test",
                    "context_length": 4096
                }
            ]
        }"#;
        let resp: OpenRouterResponse = serde_json::from_str(json).unwrap();
        let model = &resp.data[0];
        assert!(model.pricing.is_none());
        assert!(model.top_provider.is_none());
        assert!(model.architecture.is_none());
    }

    #[test]
    fn convert_model_with_no_pricing() {
        let model = OpenRouterModel {
            id: "anthropic/claude-test".to_owned(),
            name: "Claude Test".to_owned(),
            context_length: 4096,
            pricing: None,
            top_provider: None,
            architecture: None,
        };
        let result = convert_openrouter_model(&model).unwrap();
        assert!(result.input_cost_per_million.is_none());
        assert!(result.output_cost_per_million.is_none());
        // Default max output when top_provider is missing
        assert_eq!(result.max_output_tokens, 4096);
    }

    #[test]
    fn convert_model_with_no_max_completion_tokens() {
        let model = OpenRouterModel {
            id: "openai/gpt-test".to_owned(),
            name: "GPT Test".to_owned(),
            context_length: 128000,
            pricing: Some(OpenRouterPricing {
                prompt: Some("0.000001".to_owned()),
                completion: Some("0.000002".to_owned()),
            }),
            top_provider: Some(OpenRouterTopProvider {
                max_completion_tokens: None,
            }),
            architecture: None,
        };
        let result = convert_openrouter_model(&model).unwrap();
        assert_eq!(result.max_output_tokens, 4096);
    }

    // ── 13. DynamicModelInfo::from_static ───────────────────────────────

    #[test]
    fn from_static_preserves_all_fields() {
        let static_info = catalog::lookup_model("claude-opus-4-6").unwrap();
        let dynamic = DynamicModelInfo::from_static(static_info);

        assert_eq!(dynamic.id, static_info.id);
        assert_eq!(dynamic.provider, static_info.provider);
        assert_eq!(dynamic.display_name, static_info.display_name);
        assert_eq!(dynamic.context_window, static_info.context_window);
        assert_eq!(dynamic.max_output_tokens, static_info.max_output_tokens);
        assert_eq!(dynamic.supports_images, static_info.supports_images);
        assert_eq!(dynamic.supports_tool_use, static_info.supports_tool_use);
        assert_eq!(dynamic.supports_streaming, static_info.supports_streaming);
        assert_eq!(dynamic.supports_thinking, static_info.supports_thinking);
        assert_eq!(dynamic.supports_reasoning, static_info.supports_reasoning);
        assert_eq!(dynamic.supports_json_mode, static_info.supports_json_mode);
        assert_eq!(
            dynamic.supports_system_prompt,
            static_info.supports_system_prompt
        );
        assert_eq!(
            dynamic.input_cost_per_million,
            static_info.input_cost_per_million
        );
        assert_eq!(
            dynamic.output_cost_per_million,
            static_info.output_cost_per_million
        );
    }

    // ── 14. ModelDb lookup ──────────────────────────────────────────────

    #[test]
    fn modeldb_lookup_finds_model() {
        let mut db = ModelDb::new(None, DEFAULT_TTL);
        db.models = parse_fixture();
        assert!(db.lookup("gpt-4o").is_some());
        assert!(db.lookup("claude-opus-4-6").is_some());
    }

    #[test]
    fn modeldb_lookup_returns_none_for_missing() {
        let mut db = ModelDb::new(None, DEFAULT_TTL);
        db.models = parse_fixture();
        assert!(db.lookup("nonexistent").is_none());
    }

    // ── 15. ModelDbError variants ───────────────────────────────────────

    #[test]
    fn error_display_messages() {
        let io_err = ModelDbError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "file not found",
        ));
        assert!(io_err.to_string().contains("I/O error"));

        let empty_err = ModelDbError::EmptyResponse;
        assert!(empty_err.to_string().contains("no models found"));
    }

    #[test]
    fn error_from_io() {
        let io = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "nope");
        let err: ModelDbError = io.into();
        assert!(matches!(err, ModelDbError::Io(_)));
    }

    #[test]
    fn error_from_serde() {
        let bad_json = "not json";
        let serde_err = serde_json::from_str::<OpenRouterResponse>(bad_json).unwrap_err();
        let err: ModelDbError = serde_err.into();
        assert!(matches!(err, ModelDbError::Json(_)));
    }
}
