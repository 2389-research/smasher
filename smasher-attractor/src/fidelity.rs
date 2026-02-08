// ABOUTME: Fidelity modes controlling context carryover between pipeline nodes.
// ABOUTME: Determines how much context is preserved, summarized, or reset at node boundaries.

use std::collections::HashMap;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::state::Context;

/// Controls how much context is carried from one pipeline node to the next.
///
/// Different modes trade off between continuity (the downstream node can see
/// everything) and isolation (the downstream node starts with a clean slate).
/// The six attractor-spec modes are: full, truncate, compact, summary:low,
/// summary:medium, summary:high. Reset and ResultOnly are additional modes
/// for complete isolation and system-key-only carryover.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FidelityMode {
    /// Full context is carried forward. Nothing is dropped.
    #[default]
    Full,
    /// Context is truncated to max_context_tokens.
    Truncate,
    /// Context is compacted — redundant/verbose entries are removed.
    Compact,
    /// Context is summarized at low detail level.
    SummaryLow,
    /// Context is summarized at medium detail level.
    SummaryMedium,
    /// Context is summarized at high detail level.
    SummaryHigh,
    /// Context is reset — next node starts fresh.
    Reset,
    /// Only the final output/result is carried forward.
    ResultOnly,
}

impl FidelityMode {
    /// Returns true for modes that preserve context in some form.
    /// Full, Truncate, Compact, and all Summary levels are preserving.
    pub fn is_preserving(&self) -> bool {
        matches!(
            self,
            FidelityMode::Full
                | FidelityMode::Truncate
                | FidelityMode::Compact
                | FidelityMode::SummaryLow
                | FidelityMode::SummaryMedium
                | FidelityMode::SummaryHigh
        )
    }

    /// Returns true for modes that discard all context (Reset).
    pub fn is_discarding(&self) -> bool {
        matches!(self, FidelityMode::Reset)
    }
}

impl fmt::Display for FidelityMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FidelityMode::Full => write!(f, "full"),
            FidelityMode::Truncate => write!(f, "truncate"),
            FidelityMode::Compact => write!(f, "compact"),
            FidelityMode::SummaryLow => write!(f, "summary:low"),
            FidelityMode::SummaryMedium => write!(f, "summary:medium"),
            FidelityMode::SummaryHigh => write!(f, "summary:high"),
            FidelityMode::Reset => write!(f, "reset"),
            FidelityMode::ResultOnly => write!(f, "result_only"),
        }
    }
}

/// Configuration for fidelity behavior across a pipeline.
///
/// Specifies a default mode for all edges and allows per-edge overrides
/// using a `"from_node->to_node"` key format.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FidelityConfig {
    /// Default fidelity mode for all edges.
    pub default_mode: FidelityMode,
    /// Per-edge overrides: key is "from_node->to_node".
    pub edge_overrides: HashMap<String, FidelityMode>,
    /// Maximum context tokens to carry (used by Truncate and Summary modes).
    pub max_context_tokens: Option<usize>,
}

impl Default for FidelityConfig {
    fn default() -> Self {
        Self {
            default_mode: FidelityMode::Full,
            edge_overrides: HashMap::new(),
            max_context_tokens: None,
        }
    }
}

impl FidelityConfig {
    /// Create a new configuration with the given default mode.
    pub fn new(default_mode: FidelityMode) -> Self {
        Self {
            default_mode,
            edge_overrides: HashMap::new(),
            max_context_tokens: None,
        }
    }

    /// Add an override for a specific edge. Builder pattern.
    pub fn with_edge_override(mut self, from: &str, to: &str, mode: FidelityMode) -> Self {
        let key = Self::edge_key(from, to);
        self.edge_overrides.insert(key, mode);
        self
    }

    /// Set the maximum context tokens. Builder pattern.
    pub fn with_max_tokens(mut self, max_tokens: usize) -> Self {
        self.max_context_tokens = Some(max_tokens);
        self
    }

    /// Resolve the fidelity mode for a given edge.
    ///
    /// Returns the per-edge override if one exists, otherwise the default mode.
    pub fn mode_for_edge(&self, from: &str, to: &str) -> FidelityMode {
        let key = Self::edge_key(from, to);
        self.edge_overrides
            .get(&key)
            .copied()
            .unwrap_or(self.default_mode)
    }

    /// Build the canonical edge key string: `"{from}->{to}"`.
    pub fn edge_key(from: &str, to: &str) -> String {
        format!("{from}->{to}")
    }
}

/// Applies fidelity transformations to a Context when transitioning between nodes.
///
/// Given a source node, destination node, and the current context, produces a new
/// context according to the configured fidelity mode for that edge.
pub struct FidelityProcessor {
    config: FidelityConfig,
}

impl FidelityProcessor {
    /// Create a new processor with the given configuration.
    pub fn new(config: FidelityConfig) -> Self {
        Self { config }
    }

    /// Create a new Context based on the fidelity mode for the edge from `from` to `to`.
    ///
    /// - **Full**: Deep-copy all keys from the source context.
    /// - **Truncate**: Deep-copy all keys and add a `_fidelity_mode: "truncate"` marker.
    /// - **Compact**: Deep-copy all keys and add a `_fidelity_mode: "compact"` marker.
    /// - **SummaryLow**: Deep-copy all keys and add a `_fidelity_mode: "summary:low"` marker.
    /// - **SummaryMedium**: Deep-copy all keys and add a `_fidelity_mode: "summary:medium"` marker.
    /// - **SummaryHigh**: Deep-copy all keys and add a `_fidelity_mode: "summary:high"` marker.
    /// - **Reset**: Return an empty context.
    /// - **ResultOnly**: Copy only keys starting with `_` (system keys).
    pub fn process(&self, from: &str, to: &str, context: &Context) -> Context {
        let mode = self.config.mode_for_edge(from, to);
        match mode {
            FidelityMode::Full => {
                let snapshot = context.snapshot();
                Context::from(snapshot)
            }
            FidelityMode::Truncate => {
                self.clone_with_marker(context, "truncate")
            }
            FidelityMode::Compact => {
                self.clone_with_marker(context, "compact")
            }
            FidelityMode::SummaryLow => {
                self.clone_with_marker(context, "summary:low")
            }
            FidelityMode::SummaryMedium => {
                self.clone_with_marker(context, "summary:medium")
            }
            FidelityMode::SummaryHigh => {
                self.clone_with_marker(context, "summary:high")
            }
            FidelityMode::Reset => Context::new(),
            FidelityMode::ResultOnly => {
                let snapshot = context.snapshot();
                let filtered: HashMap<String, serde_json::Value> = snapshot
                    .into_iter()
                    .filter(|(k, _)| k.starts_with('_'))
                    .collect();
                Context::from(filtered)
            }
        }
    }

    /// Deep-copy the context and add a `_fidelity_mode` marker with the given value.
    fn clone_with_marker(&self, context: &Context, marker: &str) -> Context {
        let snapshot = context.snapshot();
        let new_ctx = Context::from(snapshot);
        new_ctx.set(
            "_fidelity_mode",
            serde_json::Value::String(marker.to_string()),
        );
        new_ctx
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ---------------------------------------------------------------
    // FidelityMode basics
    // ---------------------------------------------------------------

    #[test]
    fn fidelity_mode_default_is_full() {
        let mode = FidelityMode::default();
        assert_eq!(mode, FidelityMode::Full);
    }

    #[test]
    fn fidelity_mode_is_preserving_for_context_carrying_modes() {
        assert!(FidelityMode::Full.is_preserving());
        assert!(FidelityMode::Truncate.is_preserving());
        assert!(FidelityMode::Compact.is_preserving());
        assert!(FidelityMode::SummaryLow.is_preserving());
        assert!(FidelityMode::SummaryMedium.is_preserving());
        assert!(FidelityMode::SummaryHigh.is_preserving());
        assert!(!FidelityMode::Reset.is_preserving());
        assert!(!FidelityMode::ResultOnly.is_preserving());
    }

    #[test]
    fn fidelity_mode_is_discarding_for_reset() {
        assert!(FidelityMode::Reset.is_discarding());
        assert!(!FidelityMode::Full.is_discarding());
        assert!(!FidelityMode::Truncate.is_discarding());
        assert!(!FidelityMode::Compact.is_discarding());
        assert!(!FidelityMode::SummaryLow.is_discarding());
        assert!(!FidelityMode::SummaryMedium.is_discarding());
        assert!(!FidelityMode::SummaryHigh.is_discarding());
        assert!(!FidelityMode::ResultOnly.is_discarding());
    }

    #[test]
    fn fidelity_mode_display_formatting() {
        assert_eq!(FidelityMode::Full.to_string(), "full");
        assert_eq!(FidelityMode::Truncate.to_string(), "truncate");
        assert_eq!(FidelityMode::Compact.to_string(), "compact");
        assert_eq!(FidelityMode::SummaryLow.to_string(), "summary:low");
        assert_eq!(FidelityMode::SummaryMedium.to_string(), "summary:medium");
        assert_eq!(FidelityMode::SummaryHigh.to_string(), "summary:high");
        assert_eq!(FidelityMode::Reset.to_string(), "reset");
        assert_eq!(FidelityMode::ResultOnly.to_string(), "result_only");
    }

    #[test]
    fn fidelity_mode_serialization_roundtrip() {
        let modes = [
            FidelityMode::Full,
            FidelityMode::Truncate,
            FidelityMode::Compact,
            FidelityMode::SummaryLow,
            FidelityMode::SummaryMedium,
            FidelityMode::SummaryHigh,
            FidelityMode::Reset,
            FidelityMode::ResultOnly,
        ];
        for mode in &modes {
            let json_str = serde_json::to_string(mode).unwrap();
            let deserialized: FidelityMode = serde_json::from_str(&json_str).unwrap();
            assert_eq!(*mode, deserialized, "roundtrip failed for {mode}");
        }
    }

    // ---------------------------------------------------------------
    // FidelityConfig
    // ---------------------------------------------------------------

    #[test]
    fn fidelity_config_default_uses_full_mode() {
        let config = FidelityConfig::default();
        assert_eq!(config.default_mode, FidelityMode::Full);
        assert!(config.edge_overrides.is_empty());
        assert_eq!(config.max_context_tokens, None);
    }

    #[test]
    fn fidelity_config_with_edge_override_builder() {
        let config = FidelityConfig::new(FidelityMode::Full)
            .with_edge_override("a", "b", FidelityMode::Reset)
            .with_edge_override("b", "c", FidelityMode::SummaryMedium);

        assert_eq!(config.edge_overrides.len(), 2);
        assert_eq!(
            config.edge_overrides.get("a->b"),
            Some(&FidelityMode::Reset)
        );
        assert_eq!(
            config.edge_overrides.get("b->c"),
            Some(&FidelityMode::SummaryMedium)
        );
    }

    #[test]
    fn fidelity_config_with_max_tokens_builder() {
        let config = FidelityConfig::new(FidelityMode::Truncate).with_max_tokens(4096);
        assert_eq!(config.max_context_tokens, Some(4096));
    }

    #[test]
    fn fidelity_config_mode_for_edge_returns_override_when_present() {
        let config = FidelityConfig::new(FidelityMode::Full)
            .with_edge_override("parse", "validate", FidelityMode::Reset);

        assert_eq!(
            config.mode_for_edge("parse", "validate"),
            FidelityMode::Reset
        );
    }

    #[test]
    fn fidelity_config_mode_for_edge_returns_default_when_no_override() {
        let config = FidelityConfig::new(FidelityMode::SummaryMedium);
        assert_eq!(
            config.mode_for_edge("any_node", "other_node"),
            FidelityMode::SummaryMedium
        );
    }

    #[test]
    fn fidelity_config_edge_key_format() {
        assert_eq!(FidelityConfig::edge_key("start", "end"), "start->end");
        assert_eq!(
            FidelityConfig::edge_key("node_a", "node_b"),
            "node_a->node_b"
        );
    }

    // ---------------------------------------------------------------
    // FidelityProcessor
    // ---------------------------------------------------------------

    #[test]
    fn processor_full_mode_deep_copies_context() {
        let config = FidelityConfig::new(FidelityMode::Full);
        let processor = FidelityProcessor::new(config);

        let ctx = Context::new();
        ctx.set("greeting", json!("hello"));
        ctx.set("count", json!(42));

        let result = processor.process("a", "b", &ctx);

        // Values should be copied over
        assert_eq!(result.get("greeting"), Some(json!("hello")));
        assert_eq!(result.get("count"), Some(json!(42)));

        // Modifying the result should NOT affect the original (it's a deep copy, not shared Arc)
        result.set("greeting", json!("goodbye"));
        assert_eq!(ctx.get("greeting"), Some(json!("hello")));
    }

    #[test]
    fn processor_truncate_mode_adds_marker() {
        let config = FidelityConfig::new(FidelityMode::Truncate);
        let processor = FidelityProcessor::new(config);

        let ctx = Context::new();
        ctx.set("data", json!("important stuff"));

        let result = processor.process("a", "b", &ctx);

        assert_eq!(result.get("data"), Some(json!("important stuff")));
        assert_eq!(result.get("_fidelity_mode"), Some(json!("truncate")));
    }

    #[test]
    fn processor_compact_mode_adds_marker() {
        let config = FidelityConfig::new(FidelityMode::Compact);
        let processor = FidelityProcessor::new(config);

        let ctx = Context::new();
        ctx.set("data", json!("important stuff"));

        let result = processor.process("a", "b", &ctx);

        assert_eq!(result.get("data"), Some(json!("important stuff")));
        assert_eq!(result.get("_fidelity_mode"), Some(json!("compact")));
    }

    #[test]
    fn processor_summary_low_mode_adds_marker() {
        let config = FidelityConfig::new(FidelityMode::SummaryLow);
        let processor = FidelityProcessor::new(config);

        let ctx = Context::new();
        ctx.set("data", json!("important stuff"));

        let result = processor.process("a", "b", &ctx);

        assert_eq!(result.get("data"), Some(json!("important stuff")));
        assert_eq!(result.get("_fidelity_mode"), Some(json!("summary:low")));
    }

    #[test]
    fn processor_summary_medium_mode_adds_marker() {
        let config = FidelityConfig::new(FidelityMode::SummaryMedium);
        let processor = FidelityProcessor::new(config);

        let ctx = Context::new();
        ctx.set("data", json!("important stuff"));

        let result = processor.process("a", "b", &ctx);

        assert_eq!(result.get("data"), Some(json!("important stuff")));
        assert_eq!(result.get("_fidelity_mode"), Some(json!("summary:medium")));
    }

    #[test]
    fn processor_summary_high_mode_adds_marker() {
        let config = FidelityConfig::new(FidelityMode::SummaryHigh);
        let processor = FidelityProcessor::new(config);

        let ctx = Context::new();
        ctx.set("data", json!("important stuff"));

        let result = processor.process("a", "b", &ctx);

        assert_eq!(result.get("data"), Some(json!("important stuff")));
        assert_eq!(result.get("_fidelity_mode"), Some(json!("summary:high")));
    }

    #[test]
    fn processor_reset_mode_returns_empty_context() {
        let config = FidelityConfig::new(FidelityMode::Reset);
        let processor = FidelityProcessor::new(config);

        let ctx = Context::new();
        ctx.set("secret", json!("should not carry"));
        ctx.set("_system_key", json!("also gone"));

        let result = processor.process("a", "b", &ctx);

        assert!(result.keys().is_empty());
        assert_eq!(result.get("secret"), None);
        assert_eq!(result.get("_system_key"), None);
    }

    #[test]
    fn processor_result_only_mode_keeps_result_output_and_system_keys() {
        let config = FidelityConfig::new(FidelityMode::ResultOnly);
        let processor = FidelityProcessor::new(config);

        let ctx = Context::new();
        ctx.set("_result_final", json!("the answer"));
        ctx.set("_output_log", json!("log data"));
        ctx.set("_system_flag", json!(true));
        ctx.set("user_data", json!("should be dropped"));
        ctx.set("conversation", json!("also dropped"));

        let result = processor.process("a", "b", &ctx);

        // Keys starting with _ should be kept
        assert_eq!(result.get("_result_final"), Some(json!("the answer")));
        assert_eq!(result.get("_output_log"), Some(json!("log data")));
        assert_eq!(result.get("_system_flag"), Some(json!(true)));

        // Regular keys should be dropped
        assert_eq!(result.get("user_data"), None);
        assert_eq!(result.get("conversation"), None);

        // Verify the exact count
        let mut keys = result.keys();
        keys.sort();
        assert_eq!(keys, vec!["_output_log", "_result_final", "_system_flag"]);
    }

    // ---------------------------------------------------------------
    // FidelityConfig serialization
    // ---------------------------------------------------------------

    #[test]
    fn fidelity_config_serialization_roundtrip() {
        let config = FidelityConfig::new(FidelityMode::SummaryMedium)
            .with_edge_override("a", "b", FidelityMode::Reset)
            .with_edge_override("b", "c", FidelityMode::Truncate)
            .with_max_tokens(2048);

        let json_str = serde_json::to_string(&config).unwrap();
        let deserialized: FidelityConfig = serde_json::from_str(&json_str).unwrap();

        assert_eq!(deserialized.default_mode, FidelityMode::SummaryMedium);
        assert_eq!(deserialized.max_context_tokens, Some(2048));
        assert_eq!(deserialized.edge_overrides.len(), 2);
        assert_eq!(
            deserialized.edge_overrides.get("a->b"),
            Some(&FidelityMode::Reset)
        );
        assert_eq!(
            deserialized.edge_overrides.get("b->c"),
            Some(&FidelityMode::Truncate)
        );
    }

    // ---------------------------------------------------------------
    // Edge override specificity
    // ---------------------------------------------------------------

    #[test]
    fn processor_uses_edge_override_over_default() {
        let config = FidelityConfig::new(FidelityMode::Full)
            .with_edge_override("node_1", "node_2", FidelityMode::Reset);
        let processor = FidelityProcessor::new(config);

        let ctx = Context::new();
        ctx.set("data", json!("important"));

        // The overridden edge should produce an empty context
        let result = processor.process("node_1", "node_2", &ctx);
        assert!(result.keys().is_empty());

        // A non-overridden edge should use the default (Full)
        let result2 = processor.process("node_1", "node_3", &ctx);
        assert_eq!(result2.get("data"), Some(json!("important")));
    }

    // ---------------------------------------------------------------
    // Marker-based modes preserve all original data
    // ---------------------------------------------------------------

    #[test]
    fn processor_marker_modes_preserve_all_original_keys() {
        let marker_modes = [
            FidelityMode::Truncate,
            FidelityMode::Compact,
            FidelityMode::SummaryLow,
            FidelityMode::SummaryMedium,
            FidelityMode::SummaryHigh,
        ];

        for mode in &marker_modes {
            let config = FidelityConfig::new(*mode);
            let processor = FidelityProcessor::new(config);

            let ctx = Context::new();
            ctx.set("alpha", json!(1));
            ctx.set("beta", json!("two"));
            ctx.set("_system", json!(true));

            let result = processor.process("a", "b", &ctx);

            assert_eq!(result.get("alpha"), Some(json!(1)), "failed for {mode}");
            assert_eq!(result.get("beta"), Some(json!("two")), "failed for {mode}");
            assert_eq!(result.get("_system"), Some(json!(true)), "failed for {mode}");
            // The marker key should also be present
            assert!(result.get("_fidelity_mode").is_some(), "missing marker for {mode}");
        }
    }

    #[test]
    fn processor_full_mode_does_not_add_marker() {
        let config = FidelityConfig::new(FidelityMode::Full);
        let processor = FidelityProcessor::new(config);

        let ctx = Context::new();
        ctx.set("data", json!("hello"));

        let result = processor.process("a", "b", &ctx);
        assert_eq!(result.get("_fidelity_mode"), None);
    }

    // ---------------------------------------------------------------
    // Serde: specific serialized forms
    // ---------------------------------------------------------------

    #[test]
    fn fidelity_mode_serializes_to_expected_json_strings() {
        assert_eq!(serde_json::to_string(&FidelityMode::Full).unwrap(), "\"full\"");
        assert_eq!(serde_json::to_string(&FidelityMode::Truncate).unwrap(), "\"truncate\"");
        assert_eq!(serde_json::to_string(&FidelityMode::Compact).unwrap(), "\"compact\"");
        assert_eq!(serde_json::to_string(&FidelityMode::SummaryLow).unwrap(), "\"summary_low\"");
        assert_eq!(serde_json::to_string(&FidelityMode::SummaryMedium).unwrap(), "\"summary_medium\"");
        assert_eq!(serde_json::to_string(&FidelityMode::SummaryHigh).unwrap(), "\"summary_high\"");
        assert_eq!(serde_json::to_string(&FidelityMode::Reset).unwrap(), "\"reset\"");
        assert_eq!(serde_json::to_string(&FidelityMode::ResultOnly).unwrap(), "\"result_only\"");
    }

    #[test]
    fn fidelity_mode_deserializes_from_json_strings() {
        assert_eq!(serde_json::from_str::<FidelityMode>("\"full\"").unwrap(), FidelityMode::Full);
        assert_eq!(serde_json::from_str::<FidelityMode>("\"truncate\"").unwrap(), FidelityMode::Truncate);
        assert_eq!(serde_json::from_str::<FidelityMode>("\"compact\"").unwrap(), FidelityMode::Compact);
        assert_eq!(serde_json::from_str::<FidelityMode>("\"summary_low\"").unwrap(), FidelityMode::SummaryLow);
        assert_eq!(serde_json::from_str::<FidelityMode>("\"summary_medium\"").unwrap(), FidelityMode::SummaryMedium);
        assert_eq!(serde_json::from_str::<FidelityMode>("\"summary_high\"").unwrap(), FidelityMode::SummaryHigh);
        assert_eq!(serde_json::from_str::<FidelityMode>("\"reset\"").unwrap(), FidelityMode::Reset);
        assert_eq!(serde_json::from_str::<FidelityMode>("\"result_only\"").unwrap(), FidelityMode::ResultOnly);
    }

    // ---------------------------------------------------------------
    // Edge override with new modes
    // ---------------------------------------------------------------

    #[test]
    fn processor_edge_override_with_compact_mode() {
        let config = FidelityConfig::new(FidelityMode::Full)
            .with_edge_override("parse", "transform", FidelityMode::Compact);
        let processor = FidelityProcessor::new(config);

        let ctx = Context::new();
        ctx.set("data", json!("raw"));

        // Overridden edge should use Compact
        let result = processor.process("parse", "transform", &ctx);
        assert_eq!(result.get("_fidelity_mode"), Some(json!("compact")));
        assert_eq!(result.get("data"), Some(json!("raw")));

        // Non-overridden edge should use Full (no marker)
        let result2 = processor.process("parse", "validate", &ctx);
        assert_eq!(result2.get("_fidelity_mode"), None);
        assert_eq!(result2.get("data"), Some(json!("raw")));
    }
}
