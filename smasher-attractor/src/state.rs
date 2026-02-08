// ABOUTME: Thread-safe pipeline execution state with KV context, outcomes, and checkpointing.
// ABOUTME: Provides Context for sharing data between nodes and Checkpoint for resume support.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

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
}

pub type Result<T> = std::result::Result<T, StateError>;

/// Thread-safe key-value store for sharing state between pipeline nodes.
///
/// Uses `Arc<RwLock<HashMap>>` internally, making clones cheap (shared reference)
/// and allowing concurrent readers with exclusive writers.
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
    Success { data: Option<serde_json::Value> },
    /// Node failed, possibly retryable.
    Failure { error: String, retryable: bool },
    /// Node was skipped for the given reason.
    Skip { reason: String },
}

impl Outcome {
    /// Create a success outcome with no data.
    pub fn success() -> Self {
        Self::Success { data: None }
    }

    /// Create a success outcome carrying output data.
    pub fn success_with(data: serde_json::Value) -> Self {
        Self::Success { data: Some(data) }
    }

    /// Create a non-retryable failure outcome.
    pub fn failure(error: impl Into<String>) -> Self {
        Self::Failure {
            error: error.into(),
            retryable: false,
        }
    }

    /// Create a retryable failure outcome.
    pub fn retryable_failure(error: impl Into<String>) -> Self {
        Self::Failure {
            error: error.into(),
            retryable: true,
        }
    }

    /// Create a skip outcome with the given reason.
    pub fn skip(reason: impl Into<String>) -> Self {
        Self::Skip {
            reason: reason.into(),
        }
    }

    /// Returns true if this outcome is a success.
    pub fn is_success(&self) -> bool {
        matches!(self, Self::Success { .. })
    }

    /// Returns true if this outcome is a failure (retryable or not).
    pub fn is_failure(&self) -> bool {
        matches!(self, Self::Failure { .. })
    }

    /// Returns true if this outcome is a retryable failure.
    pub fn is_retryable(&self) -> bool {
        matches!(self, Self::Failure { retryable: true, .. })
    }
}

/// Serializable snapshot of pipeline execution state for resume.
///
/// Captures the current position in the pipeline, visited nodes, context data,
/// and per-node outcomes so that execution can be resumed from this point.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Checkpoint {
    pub pipeline_name: String,
    pub current_node: String,
    pub visited_nodes: Vec<String>,
    pub context_snapshot: HashMap<String, serde_json::Value>,
    pub node_outcomes: HashMap<String, Outcome>,
    pub created_at: DateTime<Utc>,
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
            pipeline_name: pipeline_name.into(),
            current_node: current_node.into(),
            visited_nodes: Vec::new(),
            context_snapshot: context.snapshot(),
            node_outcomes: HashMap::new(),
            created_at: Utc::now(),
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

    /// Serialize this checkpoint to a JSON string.
    pub fn to_json(&self) -> Result<String> {
        serde_json::to_string(self).map_err(|e| StateError::SerializationError {
            message: e.to_string(),
        })
    }

    /// Deserialize a checkpoint from a JSON string.
    pub fn from_json(json: &str) -> Result<Checkpoint> {
        serde_json::from_str(json).map_err(|e| StateError::DeserializationError {
            message: e.to_string(),
        })
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
        assert_eq!(o, Outcome::Success { data: None });
    }

    #[test]
    fn outcome_success_with_data() {
        let data = json!({"result": "ok"});
        let o = Outcome::success_with(data.clone());
        assert!(o.is_success());
        assert_eq!(o, Outcome::Success { data: Some(data) });
    }

    #[test]
    fn outcome_failure_non_retryable() {
        let o = Outcome::failure("something broke");
        assert!(o.is_failure());
        assert!(!o.is_retryable());
        assert!(!o.is_success());
        assert_eq!(
            o,
            Outcome::Failure {
                error: "something broke".to_string(),
                retryable: false,
            }
        );
    }

    #[test]
    fn outcome_retryable_failure() {
        let o = Outcome::retryable_failure("transient error");
        assert!(o.is_failure());
        assert!(o.is_retryable());
        assert_eq!(
            o,
            Outcome::Failure {
                error: "transient error".to_string(),
                retryable: true,
            }
        );
    }

    #[test]
    fn outcome_skip() {
        let o = Outcome::skip("not applicable");
        assert!(!o.is_success());
        assert!(!o.is_failure());
        assert!(!o.is_retryable());
        assert_eq!(
            o,
            Outcome::Skip {
                reason: "not applicable".to_string(),
            }
        );
    }

    #[test]
    fn outcome_serialization_roundtrip() {
        let outcomes = vec![
            Outcome::success(),
            Outcome::success_with(json!({"x": 1})),
            Outcome::failure("err"),
            Outcome::retryable_failure("retry_err"),
            Outcome::skip("skipped"),
        ];
        for original in &outcomes {
            let json_str = serde_json::to_string(original).unwrap();
            let deserialized: Outcome = serde_json::from_str(&json_str).unwrap();
            assert_eq!(*original, deserialized);
        }
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
        assert_eq!(
            cp.context_snapshot.get("stage"),
            Some(&json!("init"))
        );
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
        assert_eq!(
            cp.node_outcomes.get("node_a"),
            Some(&Outcome::success())
        );
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
}
