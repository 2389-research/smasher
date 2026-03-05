// ABOUTME: Thread-safe pipeline execution state with KV context, outcomes, and checkpointing.
// ABOUTME: Provides Context for sharing data between nodes and Checkpoint for resume support.

//! Pipeline execution state primitives.
//!
//! This module provides the foundational types that flow through every
//! pipeline execution:
//!
//! - [`Context`] -- a thread-safe key-value store (`Arc<RwLock<HashMap>>`)
//!   that handlers read from and write to during node execution.
//! - [`Outcome`] -- the result of a single node execution (success, failure,
//!   or skip).
//! - [`Checkpoint`] -- a serializable snapshot of execution state that enables
//!   resume after interruption.
//! - [`RunStore`] -- an async persistence trait for saving and loading
//!   checkpoints across runs.
//!
//! All types are `Serialize`/`Deserialize` so they can be persisted as JSON.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Errors that can occur during state operations.
#[derive(Debug, thiserror::Error)]
pub enum StateError {
    #[error("failed to acquire read lock on context")]
    ReadLockFailed,
    #[error("failed to acquire write lock on context")]
    WriteLockFailed,
    #[error("deserialization error: {message}")]
    DeserializationError { message: String },
    #[error("serialization error: {message}")]
    SerializationError { message: String },
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("run not found: {run_id}")]
    NotFound { run_id: String },
    #[error("validation error: {message}")]
    ValidationError { message: String },
}

/// Schema version tag for checkpoint serialization and migration.
///
/// Provides an explicit enum for versioning so that future schema changes
/// can be handled via sequential migrations from older versions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum CheckpointVersion {
    /// Initial checkpoint schema format.
    V1 = 1,
}

impl CheckpointVersion {
    /// Convert a raw `u32` to a `CheckpointVersion`, returning `None` for unknown values.
    pub fn from_u32(v: u32) -> Option<Self> {
        match v {
            1 => Some(Self::V1),
            _ => None,
        }
    }

    /// Convert to the underlying `u32` representation.
    pub fn as_u32(self) -> u32 {
        self as u32
    }
}

/// Handles reading raw JSON and migrating older checkpoint schemas to the current version.
///
/// When new `CheckpointVersion` variants are added, migration logic from each
/// prior version to the next is applied sequentially until the data matches
/// the current schema.
pub struct CheckpointMigrator;

impl CheckpointMigrator {
    /// Return the current (latest) checkpoint schema version.
    pub fn current_version() -> CheckpointVersion {
        CheckpointVersion::V1
    }

    /// Deserialize a checkpoint from raw JSON, applying any necessary migrations.
    ///
    /// If the `version` field is missing, it is treated as V1 (backward compat).
    /// Unknown version numbers produce a deserialization error.
    pub fn migrate(raw_json: &serde_json::Value) -> Result<Checkpoint> {
        let version_num = raw_json
            .get("version")
            .and_then(|v| v.as_u64())
            .unwrap_or(1) as u32;

        let _version = CheckpointVersion::from_u32(version_num).ok_or_else(|| {
            StateError::DeserializationError {
                message: format!("unknown checkpoint version: {version_num}"),
            }
        })?;

        // V1 is the current version; no migration needed.
        // Future versions would apply sequential transforms here.
        let checkpoint: Checkpoint = serde_json::from_value(raw_json.clone()).map_err(|e| {
            StateError::DeserializationError {
                message: e.to_string(),
            }
        })?;

        Ok(checkpoint)
    }
}

/// Describes the differences between two checkpoints.
///
/// Produced by `Checkpoint::diff` to summarize what changed between two
/// snapshots of pipeline state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckpointDiff {
    /// Context keys present in `other` but not in `self`.
    pub context_added: Vec<String>,
    /// Context keys present in `self` but not in `other`.
    pub context_removed: Vec<String>,
    /// Context keys present in both but with different values.
    pub context_changed: Vec<String>,
    /// Visited nodes present in `other` but not in `self`.
    pub nodes_added: Vec<String>,
    /// Visited nodes present in `self` but not in `other`.
    pub nodes_removed: Vec<String>,
    /// Difference in node_index: `other.node_index - self.node_index`.
    pub node_index_delta: i64,
}

impl CheckpointDiff {
    /// Returns true if no differences were detected.
    pub fn is_empty(&self) -> bool {
        self.context_added.is_empty()
            && self.context_removed.is_empty()
            && self.context_changed.is_empty()
            && self.nodes_added.is_empty()
            && self.nodes_removed.is_empty()
            && self.node_index_delta == 0
    }
}

pub type Result<T> = std::result::Result<T, StateError>;

/// Thread-safe key-value store for sharing state between pipeline nodes.
///
/// Uses `Arc<RwLock<HashMap>>` internally, making clones cheap (shared reference)
/// and allowing concurrent readers with exclusive writers.
///
/// # Examples
///
/// ```
/// use smasher_attractor::state::Context;
/// use serde_json::json;
///
/// let ctx = Context::new();
///
/// // Store a value.
/// ctx.set("model", json!("gpt-4"));
///
/// // Retrieve it back.
/// assert_eq!(ctx.get("model"), Some(json!("gpt-4")));
///
/// // Convenience accessor for string values.
/// assert_eq!(ctx.get_string("model"), Some("gpt-4".to_string()));
///
/// // Missing keys return None.
/// assert_eq!(ctx.get("missing"), None);
///
/// // Remove a key.
/// let old = ctx.remove("model");
/// assert_eq!(old, Some(json!("gpt-4")));
/// assert_eq!(ctx.get("model"), None);
/// ```
#[derive(Debug, Clone)]
pub struct Context {
    inner: Arc<RwLock<HashMap<String, serde_json::Value>>>,
}

impl Default for Context {
    fn default() -> Self {
        Self {
            inner: Arc::new(RwLock::new(HashMap::new())),
        }
    }
}

impl From<HashMap<String, serde_json::Value>> for Context {
    fn from(map: HashMap<String, serde_json::Value>) -> Self {
        Self {
            inner: Arc::new(RwLock::new(map)),
        }
    }
}

impl Context {
    /// Create an empty context.
    pub fn new() -> Self {
        Self::default()
    }

    /// Retrieve a value by key, returning `None` if the key does not exist.
    pub fn get(&self, key: &str) -> Option<serde_json::Value> {
        let guard = self.inner.read().ok()?;
        guard.get(key).cloned()
    }

    /// Insert or update a key-value pair.
    pub fn set(&self, key: impl Into<String>, value: serde_json::Value) {
        if let Ok(mut guard) = self.inner.write() {
            guard.insert(key.into(), value);
        }
    }

    /// Remove a key, returning the previous value if it existed.
    pub fn remove(&self, key: &str) -> Option<serde_json::Value> {
        let mut guard = self.inner.write().ok()?;
        guard.remove(key)
    }

    /// Convenience accessor that returns the value as a `String` if the stored
    /// value is a JSON string.
    pub fn get_string(&self, key: &str) -> Option<String> {
        let guard = self.inner.read().ok()?;
        guard.get(key).and_then(|v| v.as_str().map(String::from))
    }

    /// Typed accessor that deserializes the stored JSON value into `T`.
    ///
    /// Returns `Ok(None)` if the key does not exist, and `Err` if the value
    /// exists but cannot be deserialized into the requested type.
    pub fn get_as<T: serde::de::DeserializeOwned>(&self, key: &str) -> Result<Option<T>> {
        let guard = self.inner.read().map_err(|_| StateError::ReadLockFailed)?;
        match guard.get(key) {
            None => Ok(None),
            Some(value) => {
                let typed = serde_json::from_value(value.clone()).map_err(|e| {
                    StateError::DeserializationError {
                        message: e.to_string(),
                    }
                })?;
                Ok(Some(typed))
            }
        }
    }

    /// List all keys currently stored in the context.
    pub fn keys(&self) -> Vec<String> {
        match self.inner.read() {
            Ok(guard) => guard.keys().cloned().collect(),
            Err(_) => Vec::new(),
        }
    }

    /// Convert context to a `HashMap<String, String>` for condition evaluation.
    ///
    /// JSON strings are stored as their inner text, numbers and booleans are
    /// converted via `to_string()`, and other types (objects, arrays, null) are
    /// serialized as compact JSON.
    pub fn to_string_map(&self) -> HashMap<String, String> {
        match self.inner.read() {
            Ok(guard) => guard
                .iter()
                .map(|(k, v)| {
                    let s = match v {
                        serde_json::Value::String(s) => s.clone(),
                        serde_json::Value::Number(n) => n.to_string(),
                        serde_json::Value::Bool(b) => b.to_string(),
                        other => other.to_string(),
                    };
                    (k.clone(), s)
                })
                .collect(),
            Err(_) => HashMap::new(),
        }
    }

    /// Take a full snapshot (deep clone) of the inner data.
    pub fn snapshot(&self) -> HashMap<String, serde_json::Value> {
        match self.inner.read() {
            Ok(guard) => guard.clone(),
            Err(_) => HashMap::new(),
        }
    }
}

/// The result of executing a single pipeline node.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Outcome {
    /// Node completed successfully, optionally producing data.
    Success {
        data: Option<serde_json::Value>,
        /// Label hint for edge selection (overrides default "success").
        #[serde(skip_serializing_if = "Option::is_none")]
        preferred_label: Option<String>,
        /// Downstream node IDs the handler recommends visiting next.
        #[serde(skip_serializing_if = "Option::is_none")]
        suggested_next_ids: Option<Vec<String>>,
        /// Key-value pairs to merge into the pipeline context.
        #[serde(skip_serializing_if = "Option::is_none")]
        context_updates: Option<HashMap<String, serde_json::Value>>,
        /// Free-form notes attached by the handler.
        #[serde(skip_serializing_if = "Option::is_none")]
        notes: Option<String>,
    },
    /// Node partially succeeded — some work completed but not all.
    PartialSuccess {
        data: Option<serde_json::Value>,
        /// Label hint for edge selection.
        #[serde(skip_serializing_if = "Option::is_none")]
        preferred_label: Option<String>,
        /// Downstream node IDs the handler recommends visiting next.
        #[serde(skip_serializing_if = "Option::is_none")]
        suggested_next_ids: Option<Vec<String>>,
        /// Key-value pairs to merge into the pipeline context.
        #[serde(skip_serializing_if = "Option::is_none")]
        context_updates: Option<HashMap<String, serde_json::Value>>,
        /// Free-form notes attached by the handler.
        #[serde(skip_serializing_if = "Option::is_none")]
        notes: Option<String>,
    },
    /// Node failed, possibly retryable.
    Failure {
        error: String,
        retryable: bool,
        /// Free-form notes attached by the handler.
        #[serde(skip_serializing_if = "Option::is_none")]
        notes: Option<String>,
    },
    /// The handler explicitly requests a retry (always retryable).
    Retry {
        reason: String,
        /// Free-form notes attached by the handler.
        #[serde(skip_serializing_if = "Option::is_none")]
        notes: Option<String>,
    },
    /// Node was skipped for the given reason.
    Skip {
        reason: String,
        /// Free-form notes attached by the handler.
        #[serde(skip_serializing_if = "Option::is_none")]
        notes: Option<String>,
    },
}

impl Outcome {
    /// Create a success outcome with no data.
    pub fn success() -> Self {
        Self::Success {
            data: None,
            preferred_label: None,
            suggested_next_ids: None,
            context_updates: None,
            notes: None,
        }
    }

    /// Create a success outcome carrying output data.
    pub fn success_with(data: serde_json::Value) -> Self {
        Self::Success {
            data: Some(data),
            preferred_label: None,
            suggested_next_ids: None,
            context_updates: None,
            notes: None,
        }
    }

    /// Create a partial-success outcome with no data.
    pub fn partial_success() -> Self {
        Self::PartialSuccess {
            data: None,
            preferred_label: None,
            suggested_next_ids: None,
            context_updates: None,
            notes: None,
        }
    }

    /// Create a partial-success outcome carrying output data.
    pub fn partial_success_with(data: serde_json::Value) -> Self {
        Self::PartialSuccess {
            data: Some(data),
            preferred_label: None,
            suggested_next_ids: None,
            context_updates: None,
            notes: None,
        }
    }

    /// Create a non-retryable failure outcome.
    pub fn failure(error: impl Into<String>) -> Self {
        Self::Failure {
            error: error.into(),
            retryable: false,
            notes: None,
        }
    }

    /// Create a retryable failure outcome.
    pub fn retryable_failure(error: impl Into<String>) -> Self {
        Self::Failure {
            error: error.into(),
            retryable: true,
            notes: None,
        }
    }

    /// Create a retry outcome requesting re-execution.
    pub fn retry(reason: impl Into<String>) -> Self {
        Self::Retry {
            reason: reason.into(),
            notes: None,
        }
    }

    /// Create a skip outcome with the given reason.
    pub fn skip(reason: impl Into<String>) -> Self {
        Self::Skip {
            reason: reason.into(),
            notes: None,
        }
    }

    // ----- Builder methods -----

    /// Set the preferred label for edge selection (Success and PartialSuccess only).
    pub fn with_preferred_label(mut self, label: impl Into<String>) -> Self {
        match &mut self {
            Self::Success {
                preferred_label, ..
            }
            | Self::PartialSuccess {
                preferred_label, ..
            } => {
                *preferred_label = Some(label.into());
            }
            _ => {}
        }
        self
    }

    /// Set suggested downstream node IDs (Success and PartialSuccess only).
    pub fn with_suggested_next_ids(mut self, ids: Vec<String>) -> Self {
        match &mut self {
            Self::Success {
                suggested_next_ids, ..
            }
            | Self::PartialSuccess {
                suggested_next_ids, ..
            } => {
                *suggested_next_ids = Some(ids);
            }
            _ => {}
        }
        self
    }

    /// Set context updates to merge into the pipeline context (Success and PartialSuccess only).
    pub fn with_context_updates(mut self, updates: HashMap<String, serde_json::Value>) -> Self {
        match &mut self {
            Self::Success {
                context_updates, ..
            }
            | Self::PartialSuccess {
                context_updates, ..
            } => {
                *context_updates = Some(updates);
            }
            _ => {}
        }
        self
    }

    /// Set free-form notes on any outcome variant.
    pub fn with_notes(mut self, text: impl Into<String>) -> Self {
        let text = Some(text.into());
        match &mut self {
            Self::Success { notes, .. }
            | Self::PartialSuccess { notes, .. }
            | Self::Failure { notes, .. }
            | Self::Retry { notes, .. }
            | Self::Skip { notes, .. } => {
                *notes = text;
            }
        }
        self
    }

    // ----- Accessor methods -----

    /// Returns the preferred label, if set (Success and PartialSuccess only).
    pub fn preferred_label(&self) -> Option<&str> {
        match self {
            Self::Success {
                preferred_label, ..
            }
            | Self::PartialSuccess {
                preferred_label, ..
            } => preferred_label.as_deref(),
            _ => None,
        }
    }

    /// Returns suggested next node IDs, if set (Success and PartialSuccess only).
    pub fn suggested_next_ids(&self) -> Option<&[String]> {
        match self {
            Self::Success {
                suggested_next_ids, ..
            }
            | Self::PartialSuccess {
                suggested_next_ids, ..
            } => suggested_next_ids.as_deref(),
            _ => None,
        }
    }

    /// Returns context updates, if set (Success and PartialSuccess only).
    pub fn context_updates(&self) -> Option<&HashMap<String, serde_json::Value>> {
        match self {
            Self::Success {
                context_updates, ..
            }
            | Self::PartialSuccess {
                context_updates, ..
            } => context_updates.as_ref(),
            _ => None,
        }
    }

    /// Returns notes, if set (all variants).
    pub fn notes(&self) -> Option<&str> {
        match self {
            Self::Success { notes, .. }
            | Self::PartialSuccess { notes, .. }
            | Self::Failure { notes, .. }
            | Self::Retry { notes, .. }
            | Self::Skip { notes, .. } => notes.as_deref(),
        }
    }

    // ----- Predicates -----

    /// Returns true if this outcome is a success (full or partial).
    pub fn is_success(&self) -> bool {
        matches!(self, Self::Success { .. } | Self::PartialSuccess { .. })
    }

    /// Returns true if this outcome is a failure (retryable or not).
    pub fn is_failure(&self) -> bool {
        matches!(self, Self::Failure { .. })
    }

    /// Returns true if this outcome is retryable (Failure with retryable=true, or Retry).
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            Self::Failure {
                retryable: true,
                ..
            } | Self::Retry { .. }
        )
    }
}

/// Serializable snapshot of pipeline execution state for resume.
///
/// Captures the current position in the pipeline, visited nodes, context data,
/// and per-node outcomes so that execution can be resumed from this point.
///
/// # Examples
///
/// ```
/// use smasher_attractor::state::{Checkpoint, Context, Outcome};
/// use serde_json::json;
///
/// // Build a checkpoint from a running pipeline.
/// let ctx = Context::new();
/// ctx.set("progress", json!(42));
///
/// let mut cp = Checkpoint::new("my_pipeline", "step_b", &ctx);
/// cp.mark_visited("start");
/// cp.mark_visited("step_a");
/// cp.mark_visited("step_b");
/// cp.add_outcome("start", Outcome::success());
/// cp.add_outcome("step_a", Outcome::success());
///
/// assert!(cp.was_visited("step_a"));
/// assert!(!cp.was_visited("step_c"));
///
/// // Round-trip through JSON.
/// let json_str = cp.to_json().unwrap();
/// let restored = Checkpoint::from_json(&json_str).unwrap();
/// assert_eq!(restored.pipeline_name, "my_pipeline");
/// assert_eq!(restored.current_node, "step_b");
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Checkpoint {
    /// Schema version for forward compatibility. Defaults to 1.
    #[serde(default = "default_version")]
    pub version: u32,
    pub pipeline_name: String,
    pub current_node: String,
    pub visited_nodes: Vec<String>,
    pub context_snapshot: HashMap<String, serde_json::Value>,
    pub node_outcomes: HashMap<String, Outcome>,
    pub created_at: DateTime<Utc>,
    /// Index of the current node in the execution order (0-based).
    #[serde(default)]
    pub node_index: u64,
}

/// Default checkpoint schema version.
fn default_version() -> u32 {
    1
}

impl Checkpoint {
    /// Create a checkpoint capturing the current pipeline state.
    ///
    /// Takes a snapshot of the context at the time of creation.
    pub fn new(
        pipeline_name: impl Into<String>,
        current_node: impl Into<String>,
        context: &Context,
    ) -> Self {
        Self {
            version: 1,
            pipeline_name: pipeline_name.into(),
            current_node: current_node.into(),
            visited_nodes: Vec::new(),
            context_snapshot: context.snapshot(),
            node_outcomes: HashMap::new(),
            created_at: Utc::now(),
            node_index: 0,
        }
    }

    /// Record the outcome of a node execution.
    pub fn add_outcome(&mut self, node_id: impl Into<String>, outcome: Outcome) {
        self.node_outcomes.insert(node_id.into(), outcome);
    }

    /// Add a node to the visited list.
    pub fn mark_visited(&mut self, node_id: impl Into<String>) {
        let id = node_id.into();
        if !self.visited_nodes.contains(&id) {
            self.visited_nodes.push(id);
        }
    }

    /// Check whether a node has been visited.
    pub fn was_visited(&self, node_id: &str) -> bool {
        self.visited_nodes.iter().any(|n| n == node_id)
    }

    /// Serialize this checkpoint to a pretty-printed JSON string.
    pub fn to_json(&self) -> Result<String> {
        serde_json::to_string_pretty(self).map_err(|e| StateError::SerializationError {
            message: e.to_string(),
        })
    }

    /// Deserialize a checkpoint from a JSON string, applying version migrations
    /// if the stored schema is older than the current version.
    pub fn from_json(json: &str) -> Result<Checkpoint> {
        let raw: serde_json::Value =
            serde_json::from_str(json).map_err(|e| StateError::DeserializationError {
                message: e.to_string(),
            })?;
        CheckpointMigrator::migrate(&raw)
    }

    /// Validate checkpoint invariants.
    ///
    /// Checks:
    /// - `version` corresponds to a known `CheckpointVersion`
    /// - `current_node` is present in `visited_nodes`
    pub fn validate(&self) -> Result<()> {
        if CheckpointVersion::from_u32(self.version).is_none() {
            return Err(StateError::ValidationError {
                message: format!("unknown checkpoint version: {}", self.version),
            });
        }
        if !self.visited_nodes.contains(&self.current_node) {
            return Err(StateError::ValidationError {
                message: format!(
                    "current_node '{}' is not in visited_nodes",
                    self.current_node
                ),
            });
        }
        Ok(())
    }

    /// Compute a SHA256 checksum of this checkpoint's serialized form.
    ///
    /// Uses `serde_json::to_value` with sorted keys via `BTreeMap` conversion
    /// to produce a deterministic JSON representation before hashing.
    pub fn compute_checksum(&self) -> String {
        let canonical = canonical_json(self);
        let mut hasher = Sha256::new();
        hasher.update(canonical.as_bytes());
        format!("{:x}", hasher.finalize())
    }

    /// Verify that this checkpoint's content matches the given checksum.
    pub fn verify_checksum(&self, expected: &str) -> bool {
        self.compute_checksum() == expected
    }

    /// Compare this checkpoint to `other`, producing a `CheckpointDiff`
    /// that describes what changed.
    pub fn diff(&self, other: &Checkpoint) -> CheckpointDiff {
        use std::collections::BTreeSet;

        let self_ctx_keys: BTreeSet<&String> = self.context_snapshot.keys().collect();
        let other_ctx_keys: BTreeSet<&String> = other.context_snapshot.keys().collect();

        let mut context_added: Vec<String> = other_ctx_keys
            .difference(&self_ctx_keys)
            .map(|k| (*k).clone())
            .collect();
        context_added.sort();

        let mut context_removed: Vec<String> = self_ctx_keys
            .difference(&other_ctx_keys)
            .map(|k| (*k).clone())
            .collect();
        context_removed.sort();

        let mut context_changed: Vec<String> = self_ctx_keys
            .intersection(&other_ctx_keys)
            .filter(|k| self.context_snapshot.get(**k) != other.context_snapshot.get(**k))
            .map(|k| (*k).clone())
            .collect();
        context_changed.sort();

        let self_visited: BTreeSet<&String> = self.visited_nodes.iter().collect();
        let other_visited: BTreeSet<&String> = other.visited_nodes.iter().collect();

        let mut nodes_added: Vec<String> = other_visited
            .difference(&self_visited)
            .map(|n| (*n).clone())
            .collect();
        nodes_added.sort();

        let mut nodes_removed: Vec<String> = self_visited
            .difference(&other_visited)
            .map(|n| (*n).clone())
            .collect();
        nodes_removed.sort();

        let node_index_delta = other.node_index as i64 - self.node_index as i64;

        CheckpointDiff {
            context_added,
            context_removed,
            context_changed,
            nodes_added,
            nodes_removed,
            node_index_delta,
        }
    }
}

/// Produce a deterministic JSON string from a serializable value.
///
/// Converts the value to `serde_json::Value`, then recursively sorts all
/// object keys using `BTreeMap` ordering before serializing to a compact string.
/// This guarantees identical output regardless of `HashMap` iteration order.
fn canonical_json<T: Serialize>(value: &T) -> String {
    let v = serde_json::to_value(value).expect("serializable type");
    let sorted = sort_json_value(v);
    serde_json::to_string(&sorted).expect("sorted value serializable")
}

/// Recursively sort all JSON object keys so serialization is deterministic.
fn sort_json_value(value: serde_json::Value) -> serde_json::Value {
    use std::collections::BTreeMap;

    match value {
        serde_json::Value::Object(map) => {
            let sorted: BTreeMap<String, serde_json::Value> = map
                .into_iter()
                .map(|(k, v)| (k, sort_json_value(v)))
                .collect();
            serde_json::to_value(sorted).expect("BTreeMap serializable")
        }
        serde_json::Value::Array(arr) => {
            serde_json::Value::Array(arr.into_iter().map(sort_json_value).collect())
        }
        other => other,
    }
}

/// Wraps a `Checkpoint` with integrity metadata for tamper detection.
///
/// The envelope pairs a checkpoint with a SHA256 checksum computed over a
/// deterministic (sorted-key) JSON serialization. Use `seal()` to create
/// an envelope, `verify()` to check integrity, and `open()` to extract
/// the checkpoint after verification.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckpointEnvelope {
    /// The wrapped checkpoint payload.
    pub checkpoint: Checkpoint,
    /// SHA256 hex digest computed at seal time.
    pub checksum: String,
    /// Identifier of the component that created this envelope.
    pub created_by: String,
    /// Envelope format version for future extensibility.
    pub format_version: u32,
}

impl CheckpointEnvelope {
    /// Create a sealed envelope by computing the checkpoint's checksum.
    pub fn seal(checkpoint: Checkpoint) -> Self {
        let checksum = checkpoint.compute_checksum();
        Self {
            checkpoint,
            checksum,
            created_by: format!("smasher-engine/{}", env!("CARGO_PKG_VERSION")),
            format_version: 1,
        }
    }

    /// Verify that the stored checksum matches the checkpoint's content.
    pub fn verify(&self) -> bool {
        self.checkpoint.verify_checksum(&self.checksum)
    }

    /// Verify integrity and return the inner checkpoint.
    ///
    /// Returns `Err(StateError::ValidationError)` if the checksum does not match,
    /// indicating the checkpoint data may have been tampered with.
    pub fn open(self) -> Result<Checkpoint> {
        if !self.checkpoint.verify_checksum(&self.checksum) {
            return Err(StateError::ValidationError {
                message: "checksum mismatch: checkpoint data may have been tampered with"
                    .to_string(),
            });
        }
        Ok(self.checkpoint)
    }
}

/// Overall status of a pipeline run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    /// Pipeline is currently executing.
    Running,
    /// Pipeline finished all nodes successfully.
    Completed,
    /// Pipeline terminated due to a node failure.
    Failed,
    /// Pipeline was cancelled before completion.
    Aborted,
}

/// Metadata describing a pipeline run's identity, timing, and status.
///
/// Stored alongside the checkpoint to enable listing, filtering, and
/// querying runs without loading full checkpoint data.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunMetadata {
    pub run_id: String,
    pub graph_name: String,
    pub started_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub status: RunStatus,
    pub total_nodes_executed: usize,
    pub variables: HashMap<String, String>,
}

/// Builder for filtering runs returned by `list_runs_with_metadata`.
///
/// All filters are optional and combined with logical AND. Results are
/// sorted by `started_at` descending before the limit is applied.
#[derive(Debug, Clone, Default)]
pub struct RunQuery {
    status: Option<RunStatus>,
    graph_name: Option<String>,
    since: Option<DateTime<Utc>>,
    limit: Option<usize>,
}

impl RunQuery {
    /// Create a query with no filters applied.
    pub fn new() -> Self {
        Self::default()
    }

    /// Only return runs with this status.
    pub fn status(mut self, status: RunStatus) -> Self {
        self.status = Some(status);
        self
    }

    /// Only return runs for the given graph name.
    pub fn graph_name(mut self, name: impl Into<String>) -> Self {
        self.graph_name = Some(name.into());
        self
    }

    /// Only return runs that started at or after this timestamp.
    pub fn since(mut self, since: DateTime<Utc>) -> Self {
        self.since = Some(since);
        self
    }

    /// Limit the number of results returned.
    pub fn limit(mut self, limit: usize) -> Self {
        self.limit = Some(limit);
        self
    }

    /// Execute the query against a store, applying all configured filters.
    pub async fn execute(&self, store: &dyn RunStore) -> Result<Vec<RunMetadata>> {
        let mut runs = store.list_runs_with_metadata().await?;

        if let Some(ref status) = self.status {
            runs.retain(|r| &r.status == status);
        }
        if let Some(ref graph_name) = self.graph_name {
            runs.retain(|r| &r.graph_name == graph_name);
        }
        if let Some(since) = self.since {
            runs.retain(|r| r.started_at >= since);
        }
        if let Some(limit) = self.limit {
            runs.truncate(limit);
        }

        Ok(runs)
    }
}

/// Tracks the execution status of a single node within the pipeline.
#[derive(Debug, Clone)]
pub enum NodeStatus {
    /// Node has not yet been executed.
    Pending,
    /// Node is currently executing.
    Running,
    /// Node completed with the given outcome.
    Completed(Outcome),
    /// Node was skipped for the given reason.
    Skipped(String),
    /// Node failed after one or more attempts.
    Failed { error: String, attempts: u32 },
}

/// Async trait for persisting and loading pipeline run state.
///
/// Implementations handle the storage backend (filesystem, in-memory, database, etc.)
/// so that pipeline runs can be checkpointed, resumed, listed, and cleaned up.
#[async_trait]
pub trait RunStore: Send + Sync {
    /// Persist a checkpoint for the given run.
    async fn save_checkpoint(&self, run_id: &str, checkpoint: &Checkpoint) -> Result<()>;

    /// Load a previously saved checkpoint, returning `None` if the run does not exist.
    async fn load_checkpoint(&self, run_id: &str) -> Result<Option<Checkpoint>>;

    /// List the IDs of all persisted runs.
    async fn list_runs(&self) -> Result<Vec<String>>;

    /// Delete a persisted run. Returns `Err(StateError::NotFound)` if the run does not exist.
    async fn delete_run(&self, run_id: &str) -> Result<()>;

    /// Persist metadata for the given run.
    async fn save_metadata(&self, run_id: &str, metadata: &RunMetadata) -> Result<()>;

    /// Load metadata for a run, returning `Err(StateError::NotFound)` if not found.
    async fn load_metadata(&self, run_id: &str) -> Result<RunMetadata>;

    /// List metadata for all runs, sorted by `started_at` descending.
    async fn list_runs_with_metadata(&self) -> Result<Vec<RunMetadata>>;
}

/// Filesystem-backed run store that persists checkpoints as JSON files.
///
/// Each run is stored in its own subdirectory under the configured base path:
/// `{base_dir}/{run_id}/checkpoint.json`.
pub struct FileSystemRunStore {
    base_dir: PathBuf,
}

impl FileSystemRunStore {
    /// Create a store rooted at the given directory path.
    /// The directory and its parents are created on first write if they do not exist.
    pub fn new(base_dir: impl Into<PathBuf>) -> Self {
        Self {
            base_dir: base_dir.into(),
        }
    }

    /// Return the path to the checkpoint file for a given run ID.
    fn checkpoint_path(&self, run_id: &str) -> PathBuf {
        self.base_dir.join(run_id).join("checkpoint.json")
    }

    /// Return the path to the metadata file for a given run ID.
    fn metadata_path(&self, run_id: &str) -> PathBuf {
        self.base_dir.join(run_id).join("metadata.json")
    }
}

#[async_trait]
impl RunStore for FileSystemRunStore {
    async fn save_checkpoint(&self, run_id: &str, checkpoint: &Checkpoint) -> Result<()> {
        let path = self.checkpoint_path(run_id);
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let json = serde_json::to_string_pretty(checkpoint).map_err(|e| {
            StateError::SerializationError {
                message: e.to_string(),
            }
        })?;
        tokio::fs::write(&path, json).await?;
        Ok(())
    }

    async fn load_checkpoint(&self, run_id: &str) -> Result<Option<Checkpoint>> {
        let path = self.checkpoint_path(run_id);
        match tokio::fs::read_to_string(&path).await {
            Ok(contents) => {
                let checkpoint = serde_json::from_str(&contents).map_err(|e| {
                    StateError::DeserializationError {
                        message: e.to_string(),
                    }
                })?;
                Ok(Some(checkpoint))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(StateError::Io(e)),
        }
    }

    async fn list_runs(&self) -> Result<Vec<String>> {
        let mut runs = Vec::new();
        match tokio::fs::read_dir(&self.base_dir).await {
            Ok(mut entries) => {
                while let Some(entry) = entries.next_entry().await? {
                    let ft = entry.file_type().await?;
                    if ft.is_dir()
                        && let Some(name) = entry.file_name().to_str()
                    {
                        // Only include directories that contain a checkpoint file
                        let cp_path = entry.path().join("checkpoint.json");
                        if tokio::fs::metadata(&cp_path).await.is_ok() {
                            runs.push(name.to_string());
                        }
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // Base directory doesn't exist yet — no runs stored
            }
            Err(e) => return Err(StateError::Io(e)),
        }
        runs.sort();
        Ok(runs)
    }

    async fn delete_run(&self, run_id: &str) -> Result<()> {
        let run_dir = self.base_dir.join(run_id);
        match tokio::fs::remove_dir_all(&run_dir).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(StateError::NotFound {
                run_id: run_id.to_string(),
            }),
            Err(e) => Err(StateError::Io(e)),
        }
    }

    async fn save_metadata(&self, run_id: &str, metadata: &RunMetadata) -> Result<()> {
        let path = self.metadata_path(run_id);
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let json =
            serde_json::to_string_pretty(metadata).map_err(|e| StateError::SerializationError {
                message: e.to_string(),
            })?;
        tokio::fs::write(&path, json).await?;
        Ok(())
    }

    async fn load_metadata(&self, run_id: &str) -> Result<RunMetadata> {
        let path = self.metadata_path(run_id);
        match tokio::fs::read_to_string(&path).await {
            Ok(contents) => {
                let metadata = serde_json::from_str(&contents).map_err(|e| {
                    StateError::DeserializationError {
                        message: e.to_string(),
                    }
                })?;
                Ok(metadata)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(StateError::NotFound {
                run_id: run_id.to_string(),
            }),
            Err(e) => Err(StateError::Io(e)),
        }
    }

    async fn list_runs_with_metadata(&self) -> Result<Vec<RunMetadata>> {
        let mut results = Vec::new();
        match tokio::fs::read_dir(&self.base_dir).await {
            Ok(mut entries) => {
                while let Some(entry) = entries.next_entry().await? {
                    let ft = entry.file_type().await?;
                    if ft.is_dir()
                        && let Some(name) = entry.file_name().to_str()
                    {
                        let meta_path = entry.path().join("metadata.json");
                        if let Ok(contents) = tokio::fs::read_to_string(&meta_path).await
                            && let Ok(metadata) = serde_json::from_str::<RunMetadata>(&contents)
                        {
                            results.push(metadata);
                        }
                        // Silently skip directories without valid metadata
                        let _ = name;
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // Base directory doesn't exist yet — no runs stored
            }
            Err(e) => return Err(StateError::Io(e)),
        }
        // Sort by started_at descending
        results.sort_by(|a, b| b.started_at.cmp(&a.started_at));
        Ok(results)
    }
}

/// In-memory run store backed by a thread-safe HashMap.
///
/// Useful for testing and short-lived pipelines where persistence is not required.
pub struct InMemoryRunStore {
    checkpoints: Arc<tokio::sync::RwLock<HashMap<String, Checkpoint>>>,
    metadata: Arc<tokio::sync::RwLock<HashMap<String, RunMetadata>>>,
}

impl InMemoryRunStore {
    /// Create an empty in-memory store.
    pub fn new() -> Self {
        Self {
            checkpoints: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
            metadata: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
        }
    }
}

impl Default for InMemoryRunStore {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl RunStore for InMemoryRunStore {
    async fn save_checkpoint(&self, run_id: &str, checkpoint: &Checkpoint) -> Result<()> {
        let mut guard = self.checkpoints.write().await;
        guard.insert(run_id.to_string(), checkpoint.clone());
        Ok(())
    }

    async fn load_checkpoint(&self, run_id: &str) -> Result<Option<Checkpoint>> {
        let guard = self.checkpoints.read().await;
        Ok(guard.get(run_id).cloned())
    }

    async fn list_runs(&self) -> Result<Vec<String>> {
        let guard = self.checkpoints.read().await;
        let mut runs: Vec<String> = guard.keys().cloned().collect();
        runs.sort();
        Ok(runs)
    }

    async fn delete_run(&self, run_id: &str) -> Result<()> {
        let mut cp_guard = self.checkpoints.write().await;
        let removed = cp_guard.remove(run_id).is_some();
        drop(cp_guard);

        let mut meta_guard = self.metadata.write().await;
        let meta_removed = meta_guard.remove(run_id).is_some();
        drop(meta_guard);

        if removed || meta_removed {
            Ok(())
        } else {
            Err(StateError::NotFound {
                run_id: run_id.to_string(),
            })
        }
    }

    async fn save_metadata(&self, run_id: &str, metadata: &RunMetadata) -> Result<()> {
        let mut guard = self.metadata.write().await;
        guard.insert(run_id.to_string(), metadata.clone());
        Ok(())
    }

    async fn load_metadata(&self, run_id: &str) -> Result<RunMetadata> {
        let guard = self.metadata.read().await;
        guard.get(run_id).cloned().ok_or(StateError::NotFound {
            run_id: run_id.to_string(),
        })
    }

    async fn list_runs_with_metadata(&self) -> Result<Vec<RunMetadata>> {
        let guard = self.metadata.read().await;
        let mut results: Vec<RunMetadata> = guard.values().cloned().collect();
        // Sort by started_at descending
        results.sort_by(|a, b| b.started_at.cmp(&a.started_at));
        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ---------------------------------------------------------------
    // Context CRUD
    // ---------------------------------------------------------------

    #[test]
    fn context_set_and_get() {
        let ctx = Context::new();
        ctx.set("greeting", json!("hello"));
        let val = ctx.get("greeting");
        assert_eq!(val, Some(json!("hello")));
    }

    #[test]
    fn context_get_missing_key_returns_none() {
        let ctx = Context::new();
        assert_eq!(ctx.get("nonexistent"), None);
    }

    #[test]
    fn context_remove_returns_value_and_deletes() {
        let ctx = Context::new();
        ctx.set("temp", json!(42));
        let removed = ctx.remove("temp");
        assert_eq!(removed, Some(json!(42)));
        assert_eq!(ctx.get("temp"), None);
    }

    #[test]
    fn context_remove_missing_key_returns_none() {
        let ctx = Context::new();
        assert_eq!(ctx.remove("ghost"), None);
    }

    #[test]
    fn context_set_overwrites_existing() {
        let ctx = Context::new();
        ctx.set("key", json!("first"));
        ctx.set("key", json!("second"));
        assert_eq!(ctx.get("key"), Some(json!("second")));
    }

    // ---------------------------------------------------------------
    // Context convenience accessors
    // ---------------------------------------------------------------

    #[test]
    fn context_get_string_returns_string_value() {
        let ctx = Context::new();
        ctx.set("name", json!("Alice"));
        assert_eq!(ctx.get_string("name"), Some("Alice".to_string()));
    }

    #[test]
    fn context_get_string_returns_none_for_non_string() {
        let ctx = Context::new();
        ctx.set("count", json!(99));
        assert_eq!(ctx.get_string("count"), None);
    }

    #[test]
    fn context_get_as_typed_deserialization() {
        let ctx = Context::new();
        ctx.set("scores", json!([1, 2, 3]));
        let scores: Option<Vec<i32>> = ctx.get_as("scores").unwrap();
        assert_eq!(scores, Some(vec![1, 2, 3]));
    }

    #[test]
    fn context_get_as_missing_key_returns_ok_none() {
        let ctx = Context::new();
        let result: Result<Option<String>> = ctx.get_as("missing");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), None);
    }

    #[test]
    fn context_get_as_wrong_type_returns_error() {
        let ctx = Context::new();
        ctx.set("name", json!("not a number"));
        let result: Result<Option<i64>> = ctx.get_as("name");
        assert!(result.is_err());
    }

    // ---------------------------------------------------------------
    // Context snapshot, keys, to_string_map
    // ---------------------------------------------------------------

    #[test]
    fn context_keys_lists_all_keys() {
        let ctx = Context::new();
        ctx.set("a", json!(1));
        ctx.set("b", json!(2));
        ctx.set("c", json!(3));
        let mut keys = ctx.keys();
        keys.sort();
        assert_eq!(keys, vec!["a", "b", "c"]);
    }

    #[test]
    fn context_snapshot_returns_deep_clone() {
        let ctx = Context::new();
        ctx.set("x", json!("original"));
        let snap = ctx.snapshot();
        // Mutating the context should not affect the snapshot
        ctx.set("x", json!("modified"));
        assert_eq!(snap.get("x"), Some(&json!("original")));
        assert_eq!(ctx.get("x"), Some(json!("modified")));
    }

    #[test]
    fn context_to_string_map_converts_values() {
        let ctx = Context::new();
        ctx.set("name", json!("Alice"));
        ctx.set("score", json!(42));
        ctx.set("active", json!(true));
        ctx.set("tags", json!(["a", "b"]));
        ctx.set("empty", serde_json::Value::Null);

        let map = ctx.to_string_map();
        assert_eq!(map.get("name"), Some(&"Alice".to_string()));
        assert_eq!(map.get("score"), Some(&"42".to_string()));
        assert_eq!(map.get("active"), Some(&"true".to_string()));
        // Arrays serialize to compact JSON
        assert_eq!(map.get("tags"), Some(&r#"["a","b"]"#.to_string()));
        assert_eq!(map.get("empty"), Some(&"null".to_string()));
    }

    // ---------------------------------------------------------------
    // Context from HashMap and Default
    // ---------------------------------------------------------------

    #[test]
    fn context_from_hashmap() {
        let mut map = HashMap::new();
        map.insert("key".to_string(), json!("value"));
        let ctx = Context::from(map);
        assert_eq!(ctx.get("key"), Some(json!("value")));
    }

    #[test]
    fn context_default_is_empty() {
        let ctx = Context::default();
        assert!(ctx.keys().is_empty());
    }

    // ---------------------------------------------------------------
    // Context thread safety
    // ---------------------------------------------------------------

    #[test]
    fn context_concurrent_access() {
        let ctx = Context::new();
        let handles: Vec<_> = (0..10)
            .map(|i| {
                let ctx = ctx.clone();
                std::thread::spawn(move || {
                    ctx.set(format!("key_{i}"), json!(i));
                })
            })
            .collect();

        for h in handles {
            h.join().unwrap();
        }

        // All 10 keys should be present
        assert_eq!(ctx.keys().len(), 10);
        for i in 0..10 {
            assert_eq!(ctx.get(&format!("key_{i}")), Some(json!(i)));
        }
    }

    #[test]
    fn context_clone_shares_state() {
        let ctx1 = Context::new();
        let ctx2 = ctx1.clone();
        ctx1.set("shared", json!("hello"));
        assert_eq!(ctx2.get("shared"), Some(json!("hello")));
    }

    // ---------------------------------------------------------------
    // Outcome constructors and predicates
    // ---------------------------------------------------------------

    #[test]
    fn outcome_success_no_data() {
        let o = Outcome::success();
        assert!(o.is_success());
        assert!(!o.is_failure());
        assert!(!o.is_retryable());
        match &o {
            Outcome::Success { data, .. } => assert!(data.is_none()),
            other => panic!("expected Success, got: {other:?}"),
        }
    }

    #[test]
    fn outcome_success_with_data() {
        let data = json!({"result": "ok"});
        let o = Outcome::success_with(data.clone());
        assert!(o.is_success());
        match &o {
            Outcome::Success { data: Some(d), .. } => assert_eq!(*d, data),
            other => panic!("expected Success with data, got: {other:?}"),
        }
    }

    #[test]
    fn outcome_failure_non_retryable() {
        let o = Outcome::failure("something broke");
        assert!(o.is_failure());
        assert!(!o.is_retryable());
        assert!(!o.is_success());
        match &o {
            Outcome::Failure {
                error, retryable, ..
            } => {
                assert_eq!(error, "something broke");
                assert!(!retryable);
            }
            other => panic!("expected Failure, got: {other:?}"),
        }
    }

    #[test]
    fn outcome_retryable_failure() {
        let o = Outcome::retryable_failure("transient error");
        assert!(o.is_failure());
        assert!(o.is_retryable());
        match &o {
            Outcome::Failure {
                error, retryable, ..
            } => {
                assert_eq!(error, "transient error");
                assert!(retryable);
            }
            other => panic!("expected Failure, got: {other:?}"),
        }
    }

    #[test]
    fn outcome_skip() {
        let o = Outcome::skip("not applicable");
        assert!(!o.is_success());
        assert!(!o.is_failure());
        assert!(!o.is_retryable());
        match &o {
            Outcome::Skip { reason, .. } => assert_eq!(reason, "not applicable"),
            other => panic!("expected Skip, got: {other:?}"),
        }
    }

    #[test]
    fn outcome_partial_success_no_data() {
        let o = Outcome::partial_success();
        assert!(o.is_success());
        assert!(!o.is_failure());
        assert!(!o.is_retryable());
        match &o {
            Outcome::PartialSuccess { data, .. } => assert!(data.is_none()),
            other => panic!("expected PartialSuccess, got: {other:?}"),
        }
    }

    #[test]
    fn outcome_partial_success_with_data() {
        let data = json!({"partial": true});
        let o = Outcome::partial_success_with(data.clone());
        assert!(o.is_success());
        match &o {
            Outcome::PartialSuccess { data: Some(d), .. } => assert_eq!(*d, data),
            other => panic!("expected PartialSuccess with data, got: {other:?}"),
        }
    }

    #[test]
    fn outcome_retry() {
        let o = Outcome::retry("rate limited");
        assert!(!o.is_success());
        assert!(!o.is_failure());
        assert!(o.is_retryable());
        match &o {
            Outcome::Retry { reason, .. } => assert_eq!(reason, "rate limited"),
            other => panic!("expected Retry, got: {other:?}"),
        }
    }

    #[test]
    fn outcome_builder_preferred_label() {
        let o = Outcome::success().with_preferred_label("custom_route");
        assert_eq!(o.preferred_label(), Some("custom_route"));

        // Builder is a no-op on non-applicable variants.
        let o2 = Outcome::failure("err").with_preferred_label("ignored");
        assert_eq!(o2.preferred_label(), None);
    }

    #[test]
    fn outcome_builder_suggested_next_ids() {
        let ids = vec!["node_a".to_string(), "node_b".to_string()];
        let o = Outcome::success().with_suggested_next_ids(ids.clone());
        assert_eq!(o.suggested_next_ids(), Some(ids.as_slice()));
    }

    #[test]
    fn outcome_builder_context_updates() {
        let mut updates = HashMap::new();
        updates.insert("key".to_string(), json!("value"));
        let o = Outcome::partial_success().with_context_updates(updates.clone());
        assert_eq!(o.context_updates(), Some(&updates));
    }

    #[test]
    fn outcome_builder_notes_all_variants() {
        let variants = vec![
            Outcome::success().with_notes("s"),
            Outcome::partial_success().with_notes("ps"),
            Outcome::failure("err").with_notes("f"),
            Outcome::retry("r").with_notes("rt"),
            Outcome::skip("sk").with_notes("sk_note"),
        ];
        let expected = ["s", "ps", "f", "rt", "sk_note"];
        for (o, exp) in variants.iter().zip(expected.iter()) {
            assert_eq!(o.notes(), Some(*exp));
        }
    }

    #[test]
    fn outcome_serialization_roundtrip() {
        let mut ctx_updates = HashMap::new();
        ctx_updates.insert("k".to_string(), json!(42));

        let outcomes = vec![
            Outcome::success(),
            Outcome::success_with(json!({"x": 1})),
            Outcome::success()
                .with_preferred_label("custom")
                .with_suggested_next_ids(vec!["a".into()])
                .with_context_updates(ctx_updates)
                .with_notes("annotated"),
            Outcome::partial_success(),
            Outcome::partial_success_with(json!({"partial": true})),
            Outcome::failure("err"),
            Outcome::retryable_failure("retry_err"),
            Outcome::retry("rate limit"),
            Outcome::retry("again").with_notes("try #3"),
            Outcome::skip("skipped"),
            Outcome::skip("skipped").with_notes("reason detail"),
        ];
        for original in &outcomes {
            let json_str = serde_json::to_string(original).unwrap();
            let deserialized: Outcome = serde_json::from_str(&json_str).unwrap();
            assert_eq!(*original, deserialized);
        }
    }

    #[test]
    fn outcome_optional_fields_omitted_in_json() {
        // A plain success() should not include optional fields in the JSON output.
        let json_str = serde_json::to_string(&Outcome::success()).unwrap();
        assert!(!json_str.contains("preferred_label"));
        assert!(!json_str.contains("suggested_next_ids"));
        assert!(!json_str.contains("context_updates"));
        assert!(!json_str.contains("notes"));
    }

    // ---------------------------------------------------------------
    // Checkpoint
    // ---------------------------------------------------------------

    #[test]
    fn checkpoint_creation_captures_context() {
        let ctx = Context::new();
        ctx.set("stage", json!("init"));
        let cp = Checkpoint::new("pipeline_1", "node_a", &ctx);
        assert_eq!(cp.pipeline_name, "pipeline_1");
        assert_eq!(cp.current_node, "node_a");
        assert_eq!(cp.context_snapshot.get("stage"), Some(&json!("init")));
        assert!(cp.visited_nodes.is_empty());
        assert!(cp.node_outcomes.is_empty());
    }

    #[test]
    fn checkpoint_visited_tracking() {
        let ctx = Context::new();
        let mut cp = Checkpoint::new("p", "start", &ctx);

        assert!(!cp.was_visited("node_a"));
        cp.mark_visited("node_a");
        assert!(cp.was_visited("node_a"));

        // Marking the same node twice should not duplicate
        cp.mark_visited("node_a");
        assert_eq!(
            cp.visited_nodes.iter().filter(|n| *n == "node_a").count(),
            1
        );
    }

    #[test]
    fn checkpoint_add_outcome() {
        let ctx = Context::new();
        let mut cp = Checkpoint::new("p", "start", &ctx);
        cp.add_outcome("node_a", Outcome::success());
        cp.add_outcome("node_b", Outcome::failure("bad"));
        assert_eq!(cp.node_outcomes.get("node_a"), Some(&Outcome::success()));
        assert_eq!(
            cp.node_outcomes.get("node_b"),
            Some(&Outcome::failure("bad"))
        );
    }

    #[test]
    fn checkpoint_serialization_roundtrip() {
        let ctx = Context::new();
        ctx.set("data", json!({"nested": true}));
        let mut cp = Checkpoint::new("my_pipeline", "step_2", &ctx);
        cp.mark_visited("step_1");
        cp.mark_visited("step_2");
        cp.add_outcome("step_1", Outcome::success_with(json!("done")));
        cp.add_outcome("step_2", Outcome::retryable_failure("timeout"));

        let json_str = cp.to_json().unwrap();
        let restored = Checkpoint::from_json(&json_str).unwrap();

        assert_eq!(restored.pipeline_name, "my_pipeline");
        assert_eq!(restored.current_node, "step_2");
        assert_eq!(restored.visited_nodes, vec!["step_1", "step_2"]);
        assert_eq!(
            restored.context_snapshot.get("data"),
            Some(&json!({"nested": true}))
        );
        assert_eq!(
            restored.node_outcomes.get("step_1"),
            Some(&Outcome::success_with(json!("done")))
        );
        assert_eq!(
            restored.node_outcomes.get("step_2"),
            Some(&Outcome::retryable_failure("timeout"))
        );
    }

    #[test]
    fn checkpoint_from_invalid_json_returns_error() {
        let result = Checkpoint::from_json("not valid json at all");
        assert!(result.is_err());
    }

    // ---------------------------------------------------------------
    // NodeStatus
    // ---------------------------------------------------------------

    #[test]
    fn node_status_variants_are_constructible() {
        let _pending = NodeStatus::Pending;
        let _running = NodeStatus::Running;
        let _completed = NodeStatus::Completed(Outcome::success());
        let _skipped = NodeStatus::Skipped("reason".to_string());
        let _failed = NodeStatus::Failed {
            error: "err".to_string(),
            attempts: 3,
        };
    }

    #[test]
    fn node_status_clone_works() {
        let status = NodeStatus::Failed {
            error: "timeout".to_string(),
            attempts: 2,
        };
        let cloned = status.clone();
        // Verify the clone matches via Debug formatting
        assert_eq!(format!("{status:?}"), format!("{cloned:?}"));
    }

    // ---------------------------------------------------------------
    // Context integration with condition evaluation
    // ---------------------------------------------------------------

    #[test]
    fn context_to_string_map_works_with_condition_evaluator() {
        use crate::condition::{evaluate_condition, parse_condition};

        let ctx = Context::new();
        ctx.set("status", json!("done"));
        ctx.set("score", json!(0.8));

        let cond = parse_condition("status=done && score>0.5").unwrap();
        let map = ctx.to_string_map();
        assert!(evaluate_condition(&cond, &map));
    }

    // ---------------------------------------------------------------
    // Checkpoint version field
    // ---------------------------------------------------------------

    #[test]
    fn checkpoint_has_default_version() {
        let ctx = Context::new();
        let cp = Checkpoint::new("pipeline", "node_a", &ctx);
        assert_eq!(cp.version, 1);
    }

    #[test]
    fn checkpoint_version_survives_serialization_roundtrip() {
        let ctx = Context::new();
        let cp = Checkpoint::new("pipeline", "node_a", &ctx);
        let json_str = cp.to_json().unwrap();
        let restored = Checkpoint::from_json(&json_str).unwrap();
        assert_eq!(restored.version, 1);
    }

    #[test]
    fn checkpoint_version_defaults_when_missing_from_json() {
        // Simulate a legacy checkpoint without the version field
        let json_str = r#"{
            "pipeline_name": "legacy",
            "current_node": "start",
            "visited_nodes": [],
            "context_snapshot": {},
            "node_outcomes": {},
            "created_at": "2025-01-01T00:00:00Z"
        }"#;
        let restored = Checkpoint::from_json(json_str).unwrap();
        assert_eq!(restored.version, 1);
    }

    // ---------------------------------------------------------------
    // InMemoryRunStore
    // ---------------------------------------------------------------

    fn make_test_checkpoint(name: &str, node: &str) -> Checkpoint {
        let ctx = Context::new();
        ctx.set("key", json!("value"));
        let mut cp = Checkpoint::new(name, node, &ctx);
        cp.mark_visited(node);
        cp.add_outcome(node, Outcome::success());
        cp
    }

    #[tokio::test]
    async fn in_memory_save_and_load_roundtrip() {
        let store = InMemoryRunStore::new();
        let cp = make_test_checkpoint("pipeline_a", "step_1");

        store.save_checkpoint("run-1", &cp).await.unwrap();
        let loaded = store.load_checkpoint("run-1").await.unwrap();

        assert!(loaded.is_some());
        let loaded = loaded.unwrap();
        assert_eq!(loaded.pipeline_name, "pipeline_a");
        assert_eq!(loaded.current_node, "step_1");
        assert_eq!(loaded.version, 1);
        assert!(loaded.was_visited("step_1"));
        assert_eq!(loaded.context_snapshot.get("key"), Some(&json!("value")));
    }

    #[tokio::test]
    async fn in_memory_load_missing_returns_none() {
        let store = InMemoryRunStore::new();
        let loaded = store.load_checkpoint("nonexistent").await.unwrap();
        assert!(loaded.is_none());
    }

    #[tokio::test]
    async fn in_memory_list_runs() {
        let store = InMemoryRunStore::new();
        let cp = make_test_checkpoint("p", "n");

        store.save_checkpoint("run-b", &cp).await.unwrap();
        store.save_checkpoint("run-a", &cp).await.unwrap();

        let runs = store.list_runs().await.unwrap();
        assert_eq!(runs, vec!["run-a", "run-b"]);
    }

    #[tokio::test]
    async fn in_memory_delete_run() {
        let store = InMemoryRunStore::new();
        let cp = make_test_checkpoint("p", "n");

        store.save_checkpoint("run-1", &cp).await.unwrap();
        store.delete_run("run-1").await.unwrap();

        let loaded = store.load_checkpoint("run-1").await.unwrap();
        assert!(loaded.is_none());

        let runs = store.list_runs().await.unwrap();
        assert!(runs.is_empty());
    }

    #[tokio::test]
    async fn in_memory_delete_missing_returns_not_found() {
        let store = InMemoryRunStore::new();
        let result = store.delete_run("ghost").await;
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            StateError::NotFound { run_id } if run_id == "ghost"
        ));
    }

    #[tokio::test]
    async fn in_memory_save_overwrites_existing() {
        let store = InMemoryRunStore::new();
        let cp1 = make_test_checkpoint("pipeline_1", "node_a");
        let cp2 = make_test_checkpoint("pipeline_2", "node_b");

        store.save_checkpoint("run-1", &cp1).await.unwrap();
        store.save_checkpoint("run-1", &cp2).await.unwrap();

        let loaded = store.load_checkpoint("run-1").await.unwrap().unwrap();
        assert_eq!(loaded.pipeline_name, "pipeline_2");
    }

    // ---------------------------------------------------------------
    // FileSystemRunStore
    // ---------------------------------------------------------------

    #[tokio::test]
    async fn fs_save_and_load_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let store = FileSystemRunStore::new(tmp.path());
        let cp = make_test_checkpoint("fs_pipeline", "step_1");

        store.save_checkpoint("run-1", &cp).await.unwrap();
        let loaded = store.load_checkpoint("run-1").await.unwrap();

        assert!(loaded.is_some());
        let loaded = loaded.unwrap();
        assert_eq!(loaded.pipeline_name, "fs_pipeline");
        assert_eq!(loaded.current_node, "step_1");
        assert_eq!(loaded.version, 1);
        assert!(loaded.was_visited("step_1"));
        assert_eq!(loaded.context_snapshot.get("key"), Some(&json!("value")));
    }

    #[tokio::test]
    async fn fs_load_missing_returns_none() {
        let tmp = tempfile::tempdir().unwrap();
        let store = FileSystemRunStore::new(tmp.path());
        let loaded = store.load_checkpoint("nonexistent").await.unwrap();
        assert!(loaded.is_none());
    }

    #[tokio::test]
    async fn fs_list_runs() {
        let tmp = tempfile::tempdir().unwrap();
        let store = FileSystemRunStore::new(tmp.path());
        let cp = make_test_checkpoint("p", "n");

        store.save_checkpoint("run-b", &cp).await.unwrap();
        store.save_checkpoint("run-a", &cp).await.unwrap();

        let runs = store.list_runs().await.unwrap();
        assert_eq!(runs, vec!["run-a", "run-b"]);
    }

    #[tokio::test]
    async fn fs_list_runs_empty_when_base_dir_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let store = FileSystemRunStore::new(tmp.path().join("does_not_exist"));
        let runs = store.list_runs().await.unwrap();
        assert!(runs.is_empty());
    }

    #[tokio::test]
    async fn fs_delete_run() {
        let tmp = tempfile::tempdir().unwrap();
        let store = FileSystemRunStore::new(tmp.path());
        let cp = make_test_checkpoint("p", "n");

        store.save_checkpoint("run-1", &cp).await.unwrap();
        store.delete_run("run-1").await.unwrap();

        let loaded = store.load_checkpoint("run-1").await.unwrap();
        assert!(loaded.is_none());

        let runs = store.list_runs().await.unwrap();
        assert!(runs.is_empty());
    }

    #[tokio::test]
    async fn fs_delete_missing_returns_not_found() {
        let tmp = tempfile::tempdir().unwrap();
        let store = FileSystemRunStore::new(tmp.path());
        let result = store.delete_run("ghost").await;
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            StateError::NotFound { run_id } if run_id == "ghost"
        ));
    }

    #[tokio::test]
    async fn fs_save_overwrites_existing() {
        let tmp = tempfile::tempdir().unwrap();
        let store = FileSystemRunStore::new(tmp.path());
        let cp1 = make_test_checkpoint("pipeline_1", "node_a");
        let cp2 = make_test_checkpoint("pipeline_2", "node_b");

        store.save_checkpoint("run-1", &cp1).await.unwrap();
        store.save_checkpoint("run-1", &cp2).await.unwrap();

        let loaded = store.load_checkpoint("run-1").await.unwrap().unwrap();
        assert_eq!(loaded.pipeline_name, "pipeline_2");
    }

    #[tokio::test]
    async fn fs_creates_nested_directories() {
        let tmp = tempfile::tempdir().unwrap();
        let deep_path = tmp.path().join("level1").join("level2").join("runs");
        let store = FileSystemRunStore::new(&deep_path);
        let cp = make_test_checkpoint("p", "n");

        store.save_checkpoint("run-1", &cp).await.unwrap();
        let loaded = store.load_checkpoint("run-1").await.unwrap();
        assert!(loaded.is_some());
    }

    #[tokio::test]
    async fn fs_checkpoint_file_is_valid_json() {
        let tmp = tempfile::tempdir().unwrap();
        let store = FileSystemRunStore::new(tmp.path());
        let cp = make_test_checkpoint("p", "n");

        store.save_checkpoint("run-1", &cp).await.unwrap();

        // Read the file directly and verify it parses as valid JSON
        let path = tmp.path().join("run-1").join("checkpoint.json");
        let contents = tokio::fs::read_to_string(&path).await.unwrap();
        let value: serde_json::Value = serde_json::from_str(&contents).unwrap();
        assert_eq!(value["pipeline_name"], "p");
        assert_eq!(value["version"], 1);
    }

    // ---------------------------------------------------------------
    // RunStatus serde
    // ---------------------------------------------------------------

    #[test]
    fn run_status_serde_roundtrip() {
        let statuses = vec![
            RunStatus::Running,
            RunStatus::Completed,
            RunStatus::Failed,
            RunStatus::Aborted,
        ];
        for status in &statuses {
            let json_str = serde_json::to_string(status).unwrap();
            let deserialized: RunStatus = serde_json::from_str(&json_str).unwrap();
            assert_eq!(*status, deserialized);
        }
    }

    #[test]
    fn run_status_serializes_as_snake_case() {
        assert_eq!(
            serde_json::to_string(&RunStatus::Running).unwrap(),
            "\"running\""
        );
        assert_eq!(
            serde_json::to_string(&RunStatus::Completed).unwrap(),
            "\"completed\""
        );
        assert_eq!(
            serde_json::to_string(&RunStatus::Failed).unwrap(),
            "\"failed\""
        );
        assert_eq!(
            serde_json::to_string(&RunStatus::Aborted).unwrap(),
            "\"aborted\""
        );
    }

    // ---------------------------------------------------------------
    // RunMetadata serde
    // ---------------------------------------------------------------

    fn make_test_metadata(run_id: &str, graph_name: &str, status: RunStatus) -> RunMetadata {
        RunMetadata {
            run_id: run_id.to_string(),
            graph_name: graph_name.to_string(),
            started_at: Utc::now(),
            completed_at: None,
            status,
            total_nodes_executed: 0,
            variables: HashMap::new(),
        }
    }

    fn make_test_metadata_at(
        run_id: &str,
        graph_name: &str,
        status: RunStatus,
        started_at: DateTime<Utc>,
    ) -> RunMetadata {
        RunMetadata {
            run_id: run_id.to_string(),
            graph_name: graph_name.to_string(),
            started_at,
            completed_at: None,
            status,
            total_nodes_executed: 0,
            variables: HashMap::new(),
        }
    }

    #[test]
    fn run_metadata_serde_roundtrip() {
        let mut vars = HashMap::new();
        vars.insert("model".to_string(), "gpt-4".to_string());
        vars.insert("temp".to_string(), "0.7".to_string());

        let meta = RunMetadata {
            run_id: "run-42".to_string(),
            graph_name: "analysis_pipeline".to_string(),
            started_at: Utc::now(),
            completed_at: Some(Utc::now()),
            status: RunStatus::Completed,
            total_nodes_executed: 5,
            variables: vars,
        };

        let json_str = serde_json::to_string(&meta).unwrap();
        let restored: RunMetadata = serde_json::from_str(&json_str).unwrap();

        assert_eq!(restored.run_id, "run-42");
        assert_eq!(restored.graph_name, "analysis_pipeline");
        assert_eq!(restored.status, RunStatus::Completed);
        assert_eq!(restored.total_nodes_executed, 5);
        assert!(restored.completed_at.is_some());
        assert_eq!(restored.variables.get("model"), Some(&"gpt-4".to_string()));
        assert_eq!(restored.variables.get("temp"), Some(&"0.7".to_string()));
    }

    #[test]
    fn run_metadata_serde_with_no_completed_at() {
        let meta = make_test_metadata("run-1", "pipeline", RunStatus::Running);
        let json_str = serde_json::to_string(&meta).unwrap();
        let restored: RunMetadata = serde_json::from_str(&json_str).unwrap();
        assert!(restored.completed_at.is_none());
    }

    // ---------------------------------------------------------------
    // InMemoryRunStore metadata
    // ---------------------------------------------------------------

    #[tokio::test]
    async fn in_memory_save_and_load_metadata() {
        let store = InMemoryRunStore::new();
        let meta = make_test_metadata("run-1", "my_graph", RunStatus::Running);

        store.save_metadata("run-1", &meta).await.unwrap();
        let loaded = store.load_metadata("run-1").await.unwrap();

        assert_eq!(loaded.run_id, "run-1");
        assert_eq!(loaded.graph_name, "my_graph");
        assert_eq!(loaded.status, RunStatus::Running);
    }

    #[tokio::test]
    async fn in_memory_load_metadata_missing_returns_not_found() {
        let store = InMemoryRunStore::new();
        let result = store.load_metadata("nonexistent").await;
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            StateError::NotFound { run_id } if run_id == "nonexistent"
        ));
    }

    #[tokio::test]
    async fn in_memory_list_runs_with_metadata_sorted() {
        let store = InMemoryRunStore::new();

        let earlier = chrono::DateTime::parse_from_rfc3339("2025-01-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let later = chrono::DateTime::parse_from_rfc3339("2025-06-15T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);

        let meta_old = make_test_metadata_at("run-old", "graph_a", RunStatus::Completed, earlier);
        let meta_new = make_test_metadata_at("run-new", "graph_b", RunStatus::Running, later);

        store.save_metadata("run-old", &meta_old).await.unwrap();
        store.save_metadata("run-new", &meta_new).await.unwrap();

        let runs = store.list_runs_with_metadata().await.unwrap();
        assert_eq!(runs.len(), 2);
        // Most recent first
        assert_eq!(runs[0].run_id, "run-new");
        assert_eq!(runs[1].run_id, "run-old");
    }

    #[tokio::test]
    async fn in_memory_list_runs_with_metadata_empty() {
        let store = InMemoryRunStore::new();
        let runs = store.list_runs_with_metadata().await.unwrap();
        assert!(runs.is_empty());
    }

    #[tokio::test]
    async fn in_memory_delete_run_removes_metadata() {
        let store = InMemoryRunStore::new();
        let cp = make_test_checkpoint("p", "n");
        let meta = make_test_metadata("run-1", "graph", RunStatus::Completed);

        store.save_checkpoint("run-1", &cp).await.unwrap();
        store.save_metadata("run-1", &meta).await.unwrap();
        store.delete_run("run-1").await.unwrap();

        assert!(store.load_checkpoint("run-1").await.unwrap().is_none());
        assert!(store.load_metadata("run-1").await.is_err());
    }

    // ---------------------------------------------------------------
    // FileSystemRunStore metadata
    // ---------------------------------------------------------------

    #[tokio::test]
    async fn fs_save_and_load_metadata() {
        let tmp = tempfile::tempdir().unwrap();
        let store = FileSystemRunStore::new(tmp.path());
        let meta = make_test_metadata("run-1", "my_graph", RunStatus::Failed);

        store.save_metadata("run-1", &meta).await.unwrap();
        let loaded = store.load_metadata("run-1").await.unwrap();

        assert_eq!(loaded.run_id, "run-1");
        assert_eq!(loaded.graph_name, "my_graph");
        assert_eq!(loaded.status, RunStatus::Failed);
    }

    #[tokio::test]
    async fn fs_load_metadata_missing_returns_not_found() {
        let tmp = tempfile::tempdir().unwrap();
        let store = FileSystemRunStore::new(tmp.path());
        let result = store.load_metadata("nonexistent").await;
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            StateError::NotFound { run_id } if run_id == "nonexistent"
        ));
    }

    #[tokio::test]
    async fn fs_metadata_file_is_valid_json() {
        let tmp = tempfile::tempdir().unwrap();
        let store = FileSystemRunStore::new(tmp.path());
        let meta = make_test_metadata("run-1", "graph_x", RunStatus::Completed);

        store.save_metadata("run-1", &meta).await.unwrap();

        let path = tmp.path().join("run-1").join("metadata.json");
        let contents = tokio::fs::read_to_string(&path).await.unwrap();
        let value: serde_json::Value = serde_json::from_str(&contents).unwrap();
        assert_eq!(value["run_id"], "run-1");
        assert_eq!(value["graph_name"], "graph_x");
        assert_eq!(value["status"], "completed");
    }

    #[tokio::test]
    async fn fs_list_runs_with_metadata_sorted() {
        let tmp = tempfile::tempdir().unwrap();
        let store = FileSystemRunStore::new(tmp.path());

        let earlier = chrono::DateTime::parse_from_rfc3339("2025-01-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let later = chrono::DateTime::parse_from_rfc3339("2025-06-15T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);

        let meta_old = make_test_metadata_at("run-old", "graph_a", RunStatus::Completed, earlier);
        let meta_new = make_test_metadata_at("run-new", "graph_b", RunStatus::Running, later);

        store.save_metadata("run-old", &meta_old).await.unwrap();
        store.save_metadata("run-new", &meta_new).await.unwrap();

        let runs = store.list_runs_with_metadata().await.unwrap();
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].run_id, "run-new");
        assert_eq!(runs[1].run_id, "run-old");
    }

    #[tokio::test]
    async fn fs_list_runs_with_metadata_empty_store() {
        let tmp = tempfile::tempdir().unwrap();
        let store = FileSystemRunStore::new(tmp.path().join("does_not_exist"));
        let runs = store.list_runs_with_metadata().await.unwrap();
        assert!(runs.is_empty());
    }

    #[tokio::test]
    async fn fs_list_runs_with_metadata_skips_dirs_without_metadata() {
        let tmp = tempfile::tempdir().unwrap();
        let store = FileSystemRunStore::new(tmp.path());

        // Create a run with only a checkpoint but no metadata
        let cp = make_test_checkpoint("p", "n");
        store.save_checkpoint("run-no-meta", &cp).await.unwrap();

        // Create a run with metadata
        let meta = make_test_metadata("run-with-meta", "graph", RunStatus::Completed);
        store.save_metadata("run-with-meta", &meta).await.unwrap();

        let runs = store.list_runs_with_metadata().await.unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].run_id, "run-with-meta");
    }

    #[tokio::test]
    async fn fs_delete_run_removes_metadata() {
        let tmp = tempfile::tempdir().unwrap();
        let store = FileSystemRunStore::new(tmp.path());
        let cp = make_test_checkpoint("p", "n");
        let meta = make_test_metadata("run-1", "graph", RunStatus::Completed);

        store.save_checkpoint("run-1", &cp).await.unwrap();
        store.save_metadata("run-1", &meta).await.unwrap();
        store.delete_run("run-1").await.unwrap();

        // The whole directory is removed, so both checkpoint and metadata are gone
        assert!(store.load_checkpoint("run-1").await.unwrap().is_none());
        assert!(store.load_metadata("run-1").await.is_err());
    }

    // ---------------------------------------------------------------
    // RunQuery
    // ---------------------------------------------------------------

    #[tokio::test]
    async fn run_query_no_filters_returns_all() {
        let store = InMemoryRunStore::new();
        let m1 = make_test_metadata("run-1", "graph_a", RunStatus::Completed);
        let m2 = make_test_metadata("run-2", "graph_b", RunStatus::Running);

        store.save_metadata("run-1", &m1).await.unwrap();
        store.save_metadata("run-2", &m2).await.unwrap();

        let results = RunQuery::new().execute(&store).await.unwrap();
        assert_eq!(results.len(), 2);
    }

    #[tokio::test]
    async fn run_query_filter_by_status() {
        let store = InMemoryRunStore::new();
        let m1 = make_test_metadata("run-1", "g", RunStatus::Completed);
        let m2 = make_test_metadata("run-2", "g", RunStatus::Running);
        let m3 = make_test_metadata("run-3", "g", RunStatus::Failed);

        store.save_metadata("run-1", &m1).await.unwrap();
        store.save_metadata("run-2", &m2).await.unwrap();
        store.save_metadata("run-3", &m3).await.unwrap();

        let results = RunQuery::new()
            .status(RunStatus::Running)
            .execute(&store)
            .await
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].run_id, "run-2");
    }

    #[tokio::test]
    async fn run_query_filter_by_graph_name() {
        let store = InMemoryRunStore::new();
        let m1 = make_test_metadata("run-1", "analysis", RunStatus::Completed);
        let m2 = make_test_metadata("run-2", "deployment", RunStatus::Completed);

        store.save_metadata("run-1", &m1).await.unwrap();
        store.save_metadata("run-2", &m2).await.unwrap();

        let results = RunQuery::new()
            .graph_name("deployment")
            .execute(&store)
            .await
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].run_id, "run-2");
    }

    #[tokio::test]
    async fn run_query_filter_by_since() {
        let store = InMemoryRunStore::new();

        let old_time = chrono::DateTime::parse_from_rfc3339("2024-01-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let recent_time = chrono::DateTime::parse_from_rfc3339("2025-06-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let cutoff = chrono::DateTime::parse_from_rfc3339("2025-01-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);

        let m1 = make_test_metadata_at("run-old", "g", RunStatus::Completed, old_time);
        let m2 = make_test_metadata_at("run-recent", "g", RunStatus::Completed, recent_time);

        store.save_metadata("run-old", &m1).await.unwrap();
        store.save_metadata("run-recent", &m2).await.unwrap();

        let results = RunQuery::new().since(cutoff).execute(&store).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].run_id, "run-recent");
    }

    #[tokio::test]
    async fn run_query_limit() {
        let store = InMemoryRunStore::new();

        for i in 0..5 {
            let ts = chrono::DateTime::parse_from_rfc3339(&format!("2025-0{}-01T00:00:00Z", i + 1))
                .unwrap()
                .with_timezone(&Utc);
            let m = make_test_metadata_at(&format!("run-{i}"), "g", RunStatus::Completed, ts);
            store.save_metadata(&format!("run-{i}"), &m).await.unwrap();
        }

        let results = RunQuery::new().limit(3).execute(&store).await.unwrap();
        assert_eq!(results.len(), 3);
        // Should be the 3 most recent (sorted desc by started_at)
        assert_eq!(results[0].run_id, "run-4");
        assert_eq!(results[1].run_id, "run-3");
        assert_eq!(results[2].run_id, "run-2");
    }

    #[tokio::test]
    async fn run_query_combined_filters() {
        let store = InMemoryRunStore::new();

        let t1 = chrono::DateTime::parse_from_rfc3339("2025-01-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let t2 = chrono::DateTime::parse_from_rfc3339("2025-06-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let t3 = chrono::DateTime::parse_from_rfc3339("2025-09-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);

        let m1 = make_test_metadata_at("run-1", "deploy", RunStatus::Completed, t1);
        let m2 = make_test_metadata_at("run-2", "deploy", RunStatus::Failed, t2);
        let m3 = make_test_metadata_at("run-3", "deploy", RunStatus::Completed, t3);
        let m4 = make_test_metadata_at("run-4", "analysis", RunStatus::Completed, t3);

        store.save_metadata("run-1", &m1).await.unwrap();
        store.save_metadata("run-2", &m2).await.unwrap();
        store.save_metadata("run-3", &m3).await.unwrap();
        store.save_metadata("run-4", &m4).await.unwrap();

        let cutoff = chrono::DateTime::parse_from_rfc3339("2025-03-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);

        // Filter: completed + deploy + since March 2025
        let results = RunQuery::new()
            .status(RunStatus::Completed)
            .graph_name("deploy")
            .since(cutoff)
            .execute(&store)
            .await
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].run_id, "run-3");
    }

    #[tokio::test]
    async fn run_query_empty_store() {
        let store = InMemoryRunStore::new();
        let results = RunQuery::new()
            .status(RunStatus::Running)
            .execute(&store)
            .await
            .unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn run_query_works_with_fs_store() {
        let tmp = tempfile::tempdir().unwrap();
        let store = FileSystemRunStore::new(tmp.path());

        let t1 = chrono::DateTime::parse_from_rfc3339("2025-01-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let t2 = chrono::DateTime::parse_from_rfc3339("2025-06-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);

        let m1 = make_test_metadata_at("run-1", "g", RunStatus::Completed, t1);
        let m2 = make_test_metadata_at("run-2", "g", RunStatus::Running, t2);

        store.save_metadata("run-1", &m1).await.unwrap();
        store.save_metadata("run-2", &m2).await.unwrap();

        let results = RunQuery::new()
            .status(RunStatus::Completed)
            .execute(&store)
            .await
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].run_id, "run-1");
    }

    // ---------------------------------------------------------------
    // CheckpointVersion
    // ---------------------------------------------------------------

    #[test]
    fn checkpoint_version_ordering_v1_equals_v1() {
        assert_eq!(CheckpointVersion::V1, CheckpointVersion::V1);
        assert!(CheckpointVersion::V1 <= CheckpointVersion::V1);
        assert!(CheckpointVersion::V1 >= CheckpointVersion::V1);
    }

    #[test]
    fn checkpoint_version_from_u32() {
        assert_eq!(CheckpointVersion::from_u32(1), Some(CheckpointVersion::V1));
        assert_eq!(CheckpointVersion::from_u32(0), None);
        assert_eq!(CheckpointVersion::from_u32(99), None);
    }

    #[test]
    fn checkpoint_version_as_u32() {
        assert_eq!(CheckpointVersion::V1.as_u32(), 1);
    }

    // ---------------------------------------------------------------
    // CheckpointMigrator
    // ---------------------------------------------------------------

    #[test]
    fn checkpoint_migrator_current_version_is_v1() {
        assert_eq!(CheckpointMigrator::current_version(), CheckpointVersion::V1);
    }

    #[test]
    fn checkpoint_migrator_unknown_version_returns_error() {
        let raw = json!({
            "version": 999,
            "pipeline_name": "test",
            "current_node": "a",
            "visited_nodes": [],
            "context_snapshot": {},
            "node_outcomes": {},
            "created_at": "2025-01-01T00:00:00Z"
        });
        let result = CheckpointMigrator::migrate(&raw);
        assert!(result.is_err());
        let err_msg = format!("{}", result.unwrap_err());
        assert!(err_msg.contains("unknown checkpoint version: 999"));
    }

    // ---------------------------------------------------------------
    // Checkpoint to_json / from_json round-trip
    // ---------------------------------------------------------------

    #[test]
    fn checkpoint_to_json_from_json_roundtrip() {
        let ctx = Context::new();
        ctx.set("key", json!("value"));
        let mut cp = Checkpoint::new("pipeline_x", "node_b", &ctx);
        cp.mark_visited("node_a");
        cp.mark_visited("node_b");
        cp.node_index = 1;
        cp.add_outcome("node_a", Outcome::success());

        let json_str = cp.to_json().unwrap();
        let restored = Checkpoint::from_json(&json_str).unwrap();

        assert_eq!(restored.pipeline_name, "pipeline_x");
        assert_eq!(restored.current_node, "node_b");
        assert_eq!(restored.visited_nodes, vec!["node_a", "node_b"]);
        assert_eq!(restored.context_snapshot.get("key"), Some(&json!("value")));
        assert_eq!(restored.node_index, 1);
        assert_eq!(restored.version, 1);
    }

    #[test]
    fn checkpoint_to_json_is_pretty_printed() {
        let ctx = Context::new();
        let cp = Checkpoint::new("p", "n", &ctx);
        let json_str = cp.to_json().unwrap();
        // Pretty-printed JSON contains newlines
        assert!(json_str.contains('\n'));
    }

    #[test]
    fn checkpoint_from_json_with_missing_version_defaults_to_v1() {
        let json_str = r#"{
            "pipeline_name": "legacy",
            "current_node": "start",
            "visited_nodes": [],
            "context_snapshot": {},
            "node_outcomes": {},
            "created_at": "2025-01-01T00:00:00Z"
        }"#;
        let restored = Checkpoint::from_json(json_str).unwrap();
        assert_eq!(restored.version, 1);
        assert_eq!(restored.node_index, 0);
    }

    #[test]
    fn checkpoint_from_json_with_missing_node_index_defaults_to_zero() {
        let json_str = r#"{
            "version": 1,
            "pipeline_name": "test",
            "current_node": "a",
            "visited_nodes": ["a"],
            "context_snapshot": {},
            "node_outcomes": {},
            "created_at": "2025-01-01T00:00:00Z"
        }"#;
        let restored = Checkpoint::from_json(json_str).unwrap();
        assert_eq!(restored.node_index, 0);
    }

    // ---------------------------------------------------------------
    // Checkpoint::validate
    // ---------------------------------------------------------------

    #[test]
    fn checkpoint_validate_succeeds_with_valid_state() {
        let ctx = Context::new();
        let mut cp = Checkpoint::new("p", "node_a", &ctx);
        cp.mark_visited("node_a");
        assert!(cp.validate().is_ok());
    }

    #[test]
    fn checkpoint_validate_fails_when_current_node_not_visited() {
        let ctx = Context::new();
        let cp = Checkpoint::new("p", "node_a", &ctx);
        // current_node is "node_a" but visited_nodes is empty
        let result = cp.validate();
        assert!(result.is_err());
        let err_msg = format!("{}", result.unwrap_err());
        assert!(err_msg.contains("current_node 'node_a' is not in visited_nodes"));
    }

    #[test]
    fn checkpoint_validate_fails_with_unknown_version() {
        let ctx = Context::new();
        let mut cp = Checkpoint::new("p", "node_a", &ctx);
        cp.mark_visited("node_a");
        cp.version = 42;
        let result = cp.validate();
        assert!(result.is_err());
        let err_msg = format!("{}", result.unwrap_err());
        assert!(err_msg.contains("unknown checkpoint version: 42"));
    }

    // ---------------------------------------------------------------
    // Checkpoint::diff
    // ---------------------------------------------------------------

    #[test]
    fn checkpoint_diff_identical_is_empty() {
        let ctx = Context::new();
        ctx.set("key", json!("val"));
        let mut cp = Checkpoint::new("p", "n", &ctx);
        cp.mark_visited("n");
        cp.node_index = 3;

        let diff = cp.diff(&cp.clone());
        assert!(diff.is_empty());
        assert!(diff.context_added.is_empty());
        assert!(diff.context_removed.is_empty());
        assert!(diff.context_changed.is_empty());
        assert!(diff.nodes_added.is_empty());
        assert!(diff.nodes_removed.is_empty());
        assert_eq!(diff.node_index_delta, 0);
    }

    #[test]
    fn checkpoint_diff_context_added_and_removed() {
        let ctx1 = Context::new();
        ctx1.set("alpha", json!(1));
        ctx1.set("shared", json!("same"));
        let mut cp1 = Checkpoint::new("p", "n", &ctx1);
        cp1.mark_visited("n");

        let ctx2 = Context::new();
        ctx2.set("beta", json!(2));
        ctx2.set("shared", json!("same"));
        let mut cp2 = Checkpoint::new("p", "n", &ctx2);
        cp2.mark_visited("n");

        let diff = cp1.diff(&cp2);
        assert_eq!(diff.context_added, vec!["beta"]);
        assert_eq!(diff.context_removed, vec!["alpha"]);
        assert!(diff.context_changed.is_empty());
    }

    #[test]
    fn checkpoint_diff_context_changed() {
        let ctx1 = Context::new();
        ctx1.set("key", json!("old_value"));
        let mut cp1 = Checkpoint::new("p", "n", &ctx1);
        cp1.mark_visited("n");

        let ctx2 = Context::new();
        ctx2.set("key", json!("new_value"));
        let mut cp2 = Checkpoint::new("p", "n", &ctx2);
        cp2.mark_visited("n");

        let diff = cp1.diff(&cp2);
        assert!(diff.context_added.is_empty());
        assert!(diff.context_removed.is_empty());
        assert_eq!(diff.context_changed, vec!["key"]);
    }

    #[test]
    fn checkpoint_diff_nodes_added_and_removed() {
        let ctx = Context::new();
        let mut cp1 = Checkpoint::new("p", "a", &ctx);
        cp1.mark_visited("a");
        cp1.mark_visited("b");

        let mut cp2 = Checkpoint::new("p", "a", &ctx);
        cp2.mark_visited("a");
        cp2.mark_visited("c");

        let diff = cp1.diff(&cp2);
        assert_eq!(diff.nodes_added, vec!["c"]);
        assert_eq!(diff.nodes_removed, vec!["b"]);
    }

    #[test]
    fn checkpoint_diff_node_index_delta() {
        let ctx = Context::new();
        let mut cp1 = Checkpoint::new("p", "n", &ctx);
        cp1.mark_visited("n");
        cp1.node_index = 2;

        let mut cp2 = Checkpoint::new("p", "n", &ctx);
        cp2.mark_visited("n");
        cp2.node_index = 7;

        let diff = cp1.diff(&cp2);
        assert_eq!(diff.node_index_delta, 5);

        // Reverse direction gives negative delta
        let diff_rev = cp2.diff(&cp1);
        assert_eq!(diff_rev.node_index_delta, -5);
    }

    #[test]
    fn checkpoint_diff_is_empty_returns_false_for_non_empty() {
        let ctx = Context::new();
        let mut cp1 = Checkpoint::new("p", "n", &ctx);
        cp1.mark_visited("n");

        let mut cp2 = cp1.clone();
        cp2.node_index = 1;

        let diff = cp1.diff(&cp2);
        assert!(!diff.is_empty());
    }

    // ---------------------------------------------------------------
    // CheckpointDiff serde
    // ---------------------------------------------------------------

    #[test]
    fn checkpoint_diff_serde_roundtrip() {
        let diff = CheckpointDiff {
            context_added: vec!["new_key".to_string()],
            context_removed: vec!["old_key".to_string()],
            context_changed: vec!["modified_key".to_string()],
            nodes_added: vec!["node_c".to_string()],
            nodes_removed: vec!["node_a".to_string()],
            node_index_delta: -3,
        };

        let json_str = serde_json::to_string(&diff).unwrap();
        let restored: CheckpointDiff = serde_json::from_str(&json_str).unwrap();

        assert_eq!(restored.context_added, vec!["new_key"]);
        assert_eq!(restored.context_removed, vec!["old_key"]);
        assert_eq!(restored.context_changed, vec!["modified_key"]);
        assert_eq!(restored.nodes_added, vec!["node_c"]);
        assert_eq!(restored.nodes_removed, vec!["node_a"]);
        assert_eq!(restored.node_index_delta, -3);
    }

    #[test]
    fn checkpoint_diff_empty_serde_roundtrip() {
        let diff = CheckpointDiff {
            context_added: vec![],
            context_removed: vec![],
            context_changed: vec![],
            nodes_added: vec![],
            nodes_removed: vec![],
            node_index_delta: 0,
        };

        let json_str = serde_json::to_string(&diff).unwrap();
        let restored: CheckpointDiff = serde_json::from_str(&json_str).unwrap();

        assert!(restored.is_empty());
    }

    // ---------------------------------------------------------------
    // Checkpoint::compute_checksum / verify_checksum
    // ---------------------------------------------------------------

    #[test]
    fn compute_checksum_is_deterministic() {
        let ctx = Context::new();
        ctx.set("key", json!("value"));
        let mut cp = Checkpoint::new("pipeline", "node_a", &ctx);
        cp.mark_visited("node_a");
        cp.node_index = 1;

        let hash1 = cp.compute_checksum();
        let hash2 = cp.compute_checksum();
        assert_eq!(hash1, hash2);
    }

    #[test]
    fn compute_checksum_changes_when_data_changes() {
        let ctx = Context::new();
        ctx.set("key", json!("value"));
        let mut cp1 = Checkpoint::new("pipeline", "node_a", &ctx);
        cp1.mark_visited("node_a");

        let mut cp2 = cp1.clone();
        cp2.node_index = 42;

        let hash1 = cp1.compute_checksum();
        let hash2 = cp2.compute_checksum();
        assert_ne!(hash1, hash2);
    }

    #[test]
    fn verify_checksum_passes_with_correct_hash() {
        let ctx = Context::new();
        ctx.set("data", json!(123));
        let mut cp = Checkpoint::new("p", "n", &ctx);
        cp.mark_visited("n");

        let checksum = cp.compute_checksum();
        assert!(cp.verify_checksum(&checksum));
    }

    #[test]
    fn verify_checksum_fails_with_wrong_hash() {
        let ctx = Context::new();
        let mut cp = Checkpoint::new("p", "n", &ctx);
        cp.mark_visited("n");

        assert!(!cp.verify_checksum("deadbeef"));
    }

    // ---------------------------------------------------------------
    // CheckpointEnvelope
    // ---------------------------------------------------------------

    #[test]
    fn envelope_seal_computes_checksum() {
        let ctx = Context::new();
        ctx.set("key", json!("value"));
        let mut cp = Checkpoint::new("pipeline", "node_a", &ctx);
        cp.mark_visited("node_a");

        let envelope = CheckpointEnvelope::seal(cp.clone());
        assert!(!envelope.checksum.is_empty());
        assert_eq!(envelope.format_version, 1);
        assert!(envelope.created_by.contains("smasher-engine"));
        assert_eq!(envelope.checkpoint.pipeline_name, "pipeline");
    }

    #[test]
    fn envelope_seal_verify_roundtrip() {
        let ctx = Context::new();
        ctx.set("data", json!({"nested": true}));
        let mut cp = Checkpoint::new("my_pipeline", "step_1", &ctx);
        cp.mark_visited("step_1");

        let envelope = CheckpointEnvelope::seal(cp);
        assert!(envelope.verify());
    }

    #[test]
    fn envelope_open_succeeds_when_valid() {
        let ctx = Context::new();
        ctx.set("x", json!(42));
        let mut cp = Checkpoint::new("test_pipe", "start", &ctx);
        cp.mark_visited("start");

        let envelope = CheckpointEnvelope::seal(cp.clone());
        let opened = envelope.open().unwrap();
        assert_eq!(opened.pipeline_name, "test_pipe");
        assert_eq!(opened.current_node, "start");
        assert_eq!(opened.context_snapshot.get("x"), Some(&json!(42)));
    }

    #[test]
    fn envelope_open_fails_when_tampered() {
        let ctx = Context::new();
        let mut cp = Checkpoint::new("pipeline", "node", &ctx);
        cp.mark_visited("node");

        let mut envelope = CheckpointEnvelope::seal(cp);
        // Tamper with the checkpoint data after sealing
        envelope.checkpoint.pipeline_name = "TAMPERED".to_string();

        assert!(!envelope.verify());
        let result = envelope.open();
        assert!(result.is_err());
        let err_msg = format!("{}", result.unwrap_err());
        assert!(err_msg.contains("checksum"));
    }

    #[test]
    fn envelope_serde_roundtrip() {
        let ctx = Context::new();
        ctx.set("key", json!("val"));
        let mut cp = Checkpoint::new("pipeline", "node_a", &ctx);
        cp.mark_visited("node_a");

        let envelope = CheckpointEnvelope::seal(cp);
        let json_str = serde_json::to_string_pretty(&envelope).unwrap();
        let restored: CheckpointEnvelope = serde_json::from_str(&json_str).unwrap();

        assert_eq!(restored.checksum, envelope.checksum);
        assert_eq!(restored.created_by, envelope.created_by);
        assert_eq!(restored.format_version, envelope.format_version);
        assert_eq!(
            restored.checkpoint.pipeline_name,
            envelope.checkpoint.pipeline_name
        );
        assert!(restored.verify());
    }

    #[test]
    fn envelope_verify_fails_with_corrupted_checksum() {
        let ctx = Context::new();
        let mut cp = Checkpoint::new("p", "n", &ctx);
        cp.mark_visited("n");

        let mut envelope = CheckpointEnvelope::seal(cp);
        envelope.checksum =
            "0000000000000000000000000000000000000000000000000000000000000000".to_string();

        assert!(!envelope.verify());
    }
}
