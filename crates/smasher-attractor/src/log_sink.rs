// ABOUTME: Structured log storage and query capabilities for pipeline execution events.
// ABOUTME: Provides LogSink trait with InMemoryLogSink and FileLogSink implementations, plus LogFilter for querying.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::events::PipelineEvent;

/// A sequenced log entry wrapping a pipeline event with a monotonic counter.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogEntry {
    pub sequence: u64,
    pub event: PipelineEvent,
}

/// Errors that can occur during log sink operations.
#[derive(Debug, thiserror::Error)]
pub enum LogSinkError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("serialization error: {0}")]
    Serialization(String),
}

/// Policy controlling how many entries a log sink retains.
///
/// All fields are optional. When set, entries exceeding the limits are pruned
/// oldest-first by `apply_retention`.
#[derive(Debug, Clone)]
pub struct RetentionPolicy {
    /// Maximum number of entries to keep.
    pub max_entries: Option<usize>,
    /// Remove entries older than this duration relative to now.
    pub max_age: Option<chrono::Duration>,
    /// Cap the serialized file size (only meaningful for file-backed sinks).
    pub max_file_size_bytes: Option<u64>,
}

impl Default for RetentionPolicy {
    /// Create a policy with no limits.
    fn default() -> Self {
        Self {
            max_entries: None,
            max_age: None,
            max_file_size_bytes: None,
        }
    }
}

impl RetentionPolicy {
    /// Create a policy that caps the total number of entries.
    pub fn with_max_entries(n: usize) -> Self {
        Self {
            max_entries: Some(n),
            ..Self::default()
        }
    }

    /// Create a policy that removes entries older than the given duration.
    pub fn with_max_age(duration: chrono::Duration) -> Self {
        Self {
            max_age: Some(duration),
            ..Self::default()
        }
    }
}

/// Index for fast lookups of log entries by node ID or event kind.
///
/// Maps node IDs and event kind strings to vectors of sequence numbers,
/// enabling O(1) lookups instead of scanning all entries.
#[derive(Debug, Clone)]
pub struct LogIndex {
    by_node: HashMap<String, Vec<u64>>,
    by_kind: HashMap<String, Vec<u64>>,
}

impl LogIndex {
    /// Build an index from a slice of log entries.
    pub fn build(entries: &[LogEntry]) -> Self {
        let mut by_node: HashMap<String, Vec<u64>> = HashMap::new();
        let mut by_kind: HashMap<String, Vec<u64>> = HashMap::new();

        for entry in entries {
            if let Some(node_id) = entry.event.node_id() {
                by_node
                    .entry(node_id.to_string())
                    .or_default()
                    .push(entry.sequence);
            }

            let kind = event_kind_str(&entry.event).to_string();
            by_kind.entry(kind).or_default().push(entry.sequence);
        }

        Self { by_node, by_kind }
    }

    /// Return the sequence numbers for entries belonging to the given node.
    pub fn sequences_for_node(&self, node_id: &str) -> &[u64] {
        self.by_node
            .get(node_id)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    /// Return the sequence numbers for entries of the given event kind.
    pub fn sequences_for_kind(&self, kind: &str) -> &[u64] {
        self.by_kind.get(kind).map(|v| v.as_slice()).unwrap_or(&[])
    }
}

/// Async trait for appending and querying pipeline events.
#[async_trait]
pub trait LogSink: Send + Sync {
    /// Append an event to the log and return its sequence number.
    async fn append(&self, event: PipelineEvent) -> Result<u64, LogSinkError>;

    /// Query log entries matching the given filter.
    async fn query(&self, filter: &LogFilter) -> Result<Vec<LogEntry>, LogSinkError>;

    /// Return the total number of entries in the log.
    async fn count(&self) -> Result<u64, LogSinkError>;
}

/// Builder-pattern filter for querying log entries.
///
/// All fields are optional and combined with logical AND. The `limit` is applied
/// after all other predicates have been evaluated.
#[derive(Debug, Clone, Default)]
pub struct LogFilter {
    node_id: Option<String>,
    event_kinds: Option<Vec<String>>,
    since: Option<DateTime<Utc>>,
    until: Option<DateTime<Utc>>,
    limit: Option<usize>,
}

impl LogFilter {
    /// Create a filter with no constraints.
    pub fn new() -> Self {
        Self::default()
    }

    /// Only match entries whose event has this node_id.
    pub fn node_id(mut self, node_id: impl Into<String>) -> Self {
        self.node_id = Some(node_id.into());
        self
    }

    /// Add an event kind to match against (e.g. "node_started", "pipeline_completed").
    ///
    /// Multiple calls accumulate kinds; an entry matches if its kind is in the set.
    pub fn event_kind(mut self, kind: impl Into<String>) -> Self {
        self.event_kinds
            .get_or_insert_with(Vec::new)
            .push(kind.into());
        self
    }

    /// Only match entries whose event timestamp is at or after this time.
    pub fn since(mut self, since: DateTime<Utc>) -> Self {
        self.since = Some(since);
        self
    }

    /// Only match entries whose event timestamp is at or before this time.
    pub fn until(mut self, until: DateTime<Utc>) -> Self {
        self.until = Some(until);
        self
    }

    /// Limit the number of results returned.
    pub fn limit(mut self, limit: usize) -> Self {
        self.limit = Some(limit);
        self
    }

    /// Returns true if the given entry matches all active filter predicates.
    pub fn matches(&self, entry: &LogEntry) -> bool {
        if let Some(ref node_id) = self.node_id
            && entry.event.node_id() != Some(node_id.as_str())
        {
            return false;
        }

        if let Some(ref kinds) = self.event_kinds {
            let entry_kind = event_kind_str(&entry.event);
            if !kinds.iter().any(|k| k == entry_kind) {
                return false;
            }
        }

        if let Some(since) = self.since
            && entry.event.timestamp() < since
        {
            return false;
        }

        if let Some(until) = self.until
            && entry.event.timestamp() > until
        {
            return false;
        }

        true
    }
}

/// Extract the serde tag "kind" string from a PipelineEvent variant.
///
/// This mirrors the `#[serde(tag = "kind", rename_all = "snake_case")]` attribute
/// on `PipelineEvent` so that filtering by kind string works without serializing.
fn event_kind_str(event: &PipelineEvent) -> &'static str {
    match event {
        PipelineEvent::NodeStarted { .. } => "node_started",
        PipelineEvent::NodeCompleted { .. } => "node_completed",
        PipelineEvent::NodeFailed { .. } => "node_failed",
        PipelineEvent::EdgeTraversed { .. } => "edge_traversed",
        PipelineEvent::HumanPromptIssued { .. } => "human_prompt_issued",
        PipelineEvent::HumanResponseReceived { .. } => "human_response_received",
        PipelineEvent::ContextUpdated { .. } => "context_updated",
        PipelineEvent::CheckpointCreated { .. } => "checkpoint_created",
        PipelineEvent::PipelineStarted { .. } => "pipeline_started",
        PipelineEvent::PipelineCompleted { .. } => "pipeline_completed",
        PipelineEvent::PipelineAborted { .. } => "pipeline_aborted",
        PipelineEvent::LoopRestarted { .. } => "loop_restarted",
        PipelineEvent::AgentToolCallStarted { .. } => "agent_tool_call_started",
        PipelineEvent::AgentToolCallCompleted { .. } => "agent_tool_call_completed",
        PipelineEvent::AgentMessage { .. } => "agent_message",
        PipelineEvent::AgentTurnStarted { .. } => "agent_turn_started",
    }
}

/// In-memory log sink backed by a mutex-guarded vector and atomic counter.
pub struct InMemoryLogSink {
    entries: Arc<Mutex<Vec<LogEntry>>>,
    counter: AtomicU64,
}

impl InMemoryLogSink {
    /// Create an empty in-memory log sink.
    pub fn new() -> Self {
        Self {
            entries: Arc::new(Mutex::new(Vec::new())),
            counter: AtomicU64::new(0),
        }
    }

    /// Return a clone of all entries currently stored.
    pub fn entries(&self) -> Vec<LogEntry> {
        match self.entries.lock() {
            Ok(guard) => guard.clone(),
            Err(_) => Vec::new(),
        }
    }

    /// Apply a retention policy, removing entries that exceed the limits.
    ///
    /// Returns the number of entries removed. Oldest entries (by position, which
    /// corresponds to insertion order) are removed first when `max_entries` is
    /// exceeded. Entries whose event timestamp is older than `now - max_age` are
    /// removed when `max_age` is set.
    pub async fn apply_retention(&self, policy: &RetentionPolicy) -> Result<usize, LogSinkError> {
        let mut guard = self
            .entries
            .lock()
            .map_err(|e| LogSinkError::Serialization(format!("failed to acquire lock: {e}")))?;

        let before = guard.len();

        // Remove entries older than max_age
        if let Some(max_age) = policy.max_age {
            let cutoff = Utc::now() - max_age;
            guard.retain(|entry| entry.event.timestamp() >= cutoff);
        }

        // Trim to max_entries (remove oldest, which are at the front)
        if let Some(max_entries) = policy.max_entries
            && guard.len() > max_entries
        {
            let excess = guard.len() - max_entries;
            guard.drain(..excess);
        }

        let after = guard.len();
        Ok(before - after)
    }

    /// Build a `LogIndex` from the entries currently stored.
    pub fn build_index(&self) -> LogIndex {
        let entries = self.entries();
        LogIndex::build(&entries)
    }
}

impl Default for InMemoryLogSink {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl LogSink for InMemoryLogSink {
    async fn append(&self, event: PipelineEvent) -> Result<u64, LogSinkError> {
        let seq = self.counter.fetch_add(1, Ordering::SeqCst);
        let entry = LogEntry {
            sequence: seq,
            event,
        };
        match self.entries.lock() {
            Ok(mut guard) => guard.push(entry),
            Err(e) => {
                return Err(LogSinkError::Serialization(format!(
                    "failed to acquire lock: {e}"
                )));
            }
        }
        Ok(seq)
    }

    async fn query(&self, filter: &LogFilter) -> Result<Vec<LogEntry>, LogSinkError> {
        let all = match self.entries.lock() {
            Ok(guard) => guard.clone(),
            Err(e) => {
                return Err(LogSinkError::Serialization(format!(
                    "failed to acquire lock: {e}"
                )));
            }
        };
        let mut results: Vec<LogEntry> = all.into_iter().filter(|e| filter.matches(e)).collect();
        if let Some(limit) = filter.limit {
            results.truncate(limit);
        }
        Ok(results)
    }

    async fn count(&self) -> Result<u64, LogSinkError> {
        match self.entries.lock() {
            Ok(guard) => Ok(guard.len() as u64),
            Err(e) => Err(LogSinkError::Serialization(format!(
                "failed to acquire lock: {e}"
            ))),
        }
    }
}

/// File-backed log sink that stores entries as JSONL (one JSON object per line).
pub struct FileLogSink {
    path: PathBuf,
    counter: AtomicU64,
}

impl FileLogSink {
    /// Create a file log sink writing to the given path.
    ///
    /// If the file already exists, new entries are appended. The sequence counter
    /// starts from the number of existing lines so that sequence numbers remain
    /// monotonically increasing across restarts.
    pub fn new(path: PathBuf) -> Self {
        // Count existing lines to initialize the counter correctly
        let initial_count = std::fs::read_to_string(&path)
            .map(|contents| contents.lines().filter(|l| !l.is_empty()).count() as u64)
            .unwrap_or(0);
        Self {
            path,
            counter: AtomicU64::new(initial_count),
        }
    }
}

#[async_trait]
impl LogSink for FileLogSink {
    async fn append(&self, event: PipelineEvent) -> Result<u64, LogSinkError> {
        let seq = self.counter.fetch_add(1, Ordering::SeqCst);
        let entry = LogEntry {
            sequence: seq,
            event,
        };
        let mut line = serde_json::to_string(&entry)
            .map_err(|e| LogSinkError::Serialization(e.to_string()))?;
        line.push('\n');

        use tokio::io::AsyncWriteExt;
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .await?;
        file.write_all(line.as_bytes()).await?;
        file.flush().await?;

        Ok(seq)
    }

    async fn query(&self, filter: &LogFilter) -> Result<Vec<LogEntry>, LogSinkError> {
        let contents = match tokio::fs::read_to_string(&self.path).await {
            Ok(c) => c,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(LogSinkError::Io(e)),
        };

        let mut results = Vec::new();
        for line in contents.lines() {
            if line.is_empty() {
                continue;
            }
            let entry: LogEntry = serde_json::from_str(line)
                .map_err(|e| LogSinkError::Serialization(e.to_string()))?;
            if filter.matches(&entry) {
                results.push(entry);
                if let Some(limit) = filter.limit
                    && results.len() >= limit
                {
                    break;
                }
            }
        }

        Ok(results)
    }

    async fn count(&self) -> Result<u64, LogSinkError> {
        let contents = match tokio::fs::read_to_string(&self.path).await {
            Ok(c) => c,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
            Err(e) => return Err(LogSinkError::Io(e)),
        };
        let count = contents.lines().filter(|l| !l.is_empty()).count() as u64;
        Ok(count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::Outcome;
    use chrono::{Duration, Utc};
    use serde_json::json;

    fn now() -> DateTime<Utc> {
        Utc::now()
    }

    fn make_node_started(node_id: &str, ts: DateTime<Utc>) -> PipelineEvent {
        PipelineEvent::NodeStarted {
            node_id: node_id.into(),
            node_type: "llm".into(),
            timestamp: ts,
        }
    }

    fn make_node_completed(node_id: &str, ts: DateTime<Utc>) -> PipelineEvent {
        PipelineEvent::NodeCompleted {
            node_id: node_id.into(),
            outcome: Outcome::success(),
            duration_ms: 100,
            timestamp: ts,
        }
    }

    fn make_pipeline_started(name: &str, ts: DateTime<Utc>) -> PipelineEvent {
        PipelineEvent::PipelineStarted {
            graph_name: name.into(),
            timestamp: ts,
        }
    }

    fn make_pipeline_completed(ts: DateTime<Utc>) -> PipelineEvent {
        PipelineEvent::PipelineCompleted {
            outcome: Outcome::success(),
            total_nodes: 3,
            duration_ms: 500,
            timestamp: ts,
        }
    }

    // ---------------------------------------------------------------
    // LogEntry serde round-trip
    // ---------------------------------------------------------------

    #[test]
    fn log_entry_serde_roundtrip() {
        let entry = LogEntry {
            sequence: 42,
            event: make_node_started("test_node", now()),
        };
        let json = serde_json::to_string(&entry).unwrap();
        let restored: LogEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.sequence, 42);
        assert!(matches!(
            restored.event,
            PipelineEvent::NodeStarted { ref node_id, .. }
            if node_id == "test_node"
        ));
    }

    #[test]
    fn log_entry_serde_roundtrip_with_complex_event() {
        let entry = LogEntry {
            sequence: 99,
            event: PipelineEvent::NodeCompleted {
                node_id: "transform".into(),
                outcome: Outcome::success_with(json!({"tokens": 42})),
                duration_ms: 150,
                timestamp: now(),
            },
        };
        let json = serde_json::to_string(&entry).unwrap();
        let restored: LogEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.sequence, 99);
        assert!(matches!(
            restored.event,
            PipelineEvent::NodeCompleted {
                duration_ms: 150,
                ..
            }
        ));
    }

    // ---------------------------------------------------------------
    // InMemoryLogSink: append increments sequence
    // ---------------------------------------------------------------

    #[tokio::test]
    async fn in_memory_append_increments_sequence() {
        let sink = InMemoryLogSink::new();

        let seq0 = sink.append(make_node_started("a", now())).await.unwrap();
        let seq1 = sink.append(make_node_started("b", now())).await.unwrap();
        let seq2 = sink.append(make_node_started("c", now())).await.unwrap();

        assert_eq!(seq0, 0);
        assert_eq!(seq1, 1);
        assert_eq!(seq2, 2);
    }

    // ---------------------------------------------------------------
    // InMemoryLogSink: count
    // ---------------------------------------------------------------

    #[tokio::test]
    async fn in_memory_count() {
        let sink = InMemoryLogSink::new();
        assert_eq!(sink.count().await.unwrap(), 0);

        sink.append(make_node_started("a", now())).await.unwrap();
        assert_eq!(sink.count().await.unwrap(), 1);

        sink.append(make_node_started("b", now())).await.unwrap();
        assert_eq!(sink.count().await.unwrap(), 2);
    }

    // ---------------------------------------------------------------
    // InMemoryLogSink: entries() accessor
    // ---------------------------------------------------------------

    #[tokio::test]
    async fn in_memory_entries_returns_all() {
        let sink = InMemoryLogSink::new();
        sink.append(make_node_started("x", now())).await.unwrap();
        sink.append(make_node_completed("x", now())).await.unwrap();

        let entries = sink.entries();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].sequence, 0);
        assert_eq!(entries[1].sequence, 1);
    }

    // ---------------------------------------------------------------
    // InMemoryLogSink: query with node_id filter
    // ---------------------------------------------------------------

    #[tokio::test]
    async fn in_memory_query_node_id_filter() {
        let sink = InMemoryLogSink::new();
        let ts = now();

        sink.append(make_node_started("alpha", ts)).await.unwrap();
        sink.append(make_node_started("beta", ts)).await.unwrap();
        sink.append(make_node_completed("alpha", ts)).await.unwrap();
        sink.append(make_pipeline_started("pipeline", ts))
            .await
            .unwrap();

        let filter = LogFilter::new().node_id("alpha");
        let results = sink.query(&filter).await.unwrap();

        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|e| e.event.node_id() == Some("alpha")));
    }

    #[tokio::test]
    async fn in_memory_query_node_id_filter_no_match() {
        let sink = InMemoryLogSink::new();
        sink.append(make_node_started("alpha", now()))
            .await
            .unwrap();

        let filter = LogFilter::new().node_id("nonexistent");
        let results = sink.query(&filter).await.unwrap();

        assert!(results.is_empty());
    }

    // ---------------------------------------------------------------
    // InMemoryLogSink: query with event_kind filter
    // ---------------------------------------------------------------

    #[tokio::test]
    async fn in_memory_query_event_kind_filter() {
        let sink = InMemoryLogSink::new();
        let ts = now();

        sink.append(make_node_started("a", ts)).await.unwrap();
        sink.append(make_node_completed("a", ts)).await.unwrap();
        sink.append(make_pipeline_started("p", ts)).await.unwrap();
        sink.append(make_pipeline_completed(ts)).await.unwrap();

        let filter = LogFilter::new().event_kind("node_started");
        let results = sink.query(&filter).await.unwrap();

        assert_eq!(results.len(), 1);
        assert!(matches!(
            results[0].event,
            PipelineEvent::NodeStarted { .. }
        ));
    }

    #[tokio::test]
    async fn in_memory_query_multiple_event_kinds() {
        let sink = InMemoryLogSink::new();
        let ts = now();

        sink.append(make_node_started("a", ts)).await.unwrap();
        sink.append(make_node_completed("a", ts)).await.unwrap();
        sink.append(make_pipeline_started("p", ts)).await.unwrap();
        sink.append(make_pipeline_completed(ts)).await.unwrap();

        let filter = LogFilter::new()
            .event_kind("node_started")
            .event_kind("pipeline_completed");
        let results = sink.query(&filter).await.unwrap();

        assert_eq!(results.len(), 2);
        assert!(matches!(
            results[0].event,
            PipelineEvent::NodeStarted { .. }
        ));
        assert!(matches!(
            results[1].event,
            PipelineEvent::PipelineCompleted { .. }
        ));
    }

    // ---------------------------------------------------------------
    // InMemoryLogSink: query with time range
    // ---------------------------------------------------------------

    #[tokio::test]
    async fn in_memory_query_time_range() {
        let sink = InMemoryLogSink::new();

        let t1 = Utc::now() - Duration::hours(3);
        let t2 = Utc::now() - Duration::hours(2);
        let t3 = Utc::now() - Duration::hours(1);

        sink.append(make_node_started("a", t1)).await.unwrap();
        sink.append(make_node_started("b", t2)).await.unwrap();
        sink.append(make_node_started("c", t3)).await.unwrap();

        // Query for entries between t1+30min and t3-30min (should get only t2)
        let since = t1 + Duration::minutes(30);
        let until = t3 - Duration::minutes(30);
        let filter = LogFilter::new().since(since).until(until);
        let results = sink.query(&filter).await.unwrap();

        assert_eq!(results.len(), 1);
        assert!(matches!(
            results[0].event,
            PipelineEvent::NodeStarted { ref node_id, .. }
            if node_id == "b"
        ));
    }

    #[tokio::test]
    async fn in_memory_query_since_only() {
        let sink = InMemoryLogSink::new();

        let t1 = Utc::now() - Duration::hours(2);
        let t2 = Utc::now();

        sink.append(make_node_started("old", t1)).await.unwrap();
        sink.append(make_node_started("recent", t2)).await.unwrap();

        let cutoff = Utc::now() - Duration::hours(1);
        let filter = LogFilter::new().since(cutoff);
        let results = sink.query(&filter).await.unwrap();

        assert_eq!(results.len(), 1);
        assert!(matches!(
            results[0].event,
            PipelineEvent::NodeStarted { ref node_id, .. }
            if node_id == "recent"
        ));
    }

    #[tokio::test]
    async fn in_memory_query_until_only() {
        let sink = InMemoryLogSink::new();

        let t1 = Utc::now() - Duration::hours(2);
        let t2 = Utc::now();

        sink.append(make_node_started("old", t1)).await.unwrap();
        sink.append(make_node_started("recent", t2)).await.unwrap();

        let cutoff = Utc::now() - Duration::hours(1);
        let filter = LogFilter::new().until(cutoff);
        let results = sink.query(&filter).await.unwrap();

        assert_eq!(results.len(), 1);
        assert!(matches!(
            results[0].event,
            PipelineEvent::NodeStarted { ref node_id, .. }
            if node_id == "old"
        ));
    }

    // ---------------------------------------------------------------
    // InMemoryLogSink: query with limit
    // ---------------------------------------------------------------

    #[tokio::test]
    async fn in_memory_query_with_limit() {
        let sink = InMemoryLogSink::new();
        let ts = now();

        for i in 0..10 {
            sink.append(make_node_started(&format!("node_{i}"), ts))
                .await
                .unwrap();
        }

        let filter = LogFilter::new().limit(3);
        let results = sink.query(&filter).await.unwrap();

        assert_eq!(results.len(), 3);
        assert_eq!(results[0].sequence, 0);
        assert_eq!(results[1].sequence, 1);
        assert_eq!(results[2].sequence, 2);
    }

    #[tokio::test]
    async fn in_memory_query_limit_greater_than_entries() {
        let sink = InMemoryLogSink::new();
        sink.append(make_node_started("a", now())).await.unwrap();

        let filter = LogFilter::new().limit(100);
        let results = sink.query(&filter).await.unwrap();

        assert_eq!(results.len(), 1);
    }

    // ---------------------------------------------------------------
    // LogFilter::matches tests
    // ---------------------------------------------------------------

    #[test]
    fn filter_matches_empty_filter_matches_everything() {
        let filter = LogFilter::new();
        let entry = LogEntry {
            sequence: 0,
            event: make_node_started("any", now()),
        };
        assert!(filter.matches(&entry));
    }

    #[test]
    fn filter_matches_node_id_positive() {
        let filter = LogFilter::new().node_id("target");
        let entry = LogEntry {
            sequence: 0,
            event: make_node_started("target", now()),
        };
        assert!(filter.matches(&entry));
    }

    #[test]
    fn filter_matches_node_id_negative() {
        let filter = LogFilter::new().node_id("target");
        let entry = LogEntry {
            sequence: 0,
            event: make_node_started("other", now()),
        };
        assert!(!filter.matches(&entry));
    }

    #[test]
    fn filter_matches_node_id_pipeline_event_has_no_node_id() {
        let filter = LogFilter::new().node_id("target");
        let entry = LogEntry {
            sequence: 0,
            event: make_pipeline_started("p", now()),
        };
        assert!(!filter.matches(&entry));
    }

    #[test]
    fn filter_matches_event_kind_positive() {
        let filter = LogFilter::new().event_kind("node_started");
        let entry = LogEntry {
            sequence: 0,
            event: make_node_started("a", now()),
        };
        assert!(filter.matches(&entry));
    }

    #[test]
    fn filter_matches_event_kind_negative() {
        let filter = LogFilter::new().event_kind("node_completed");
        let entry = LogEntry {
            sequence: 0,
            event: make_node_started("a", now()),
        };
        assert!(!filter.matches(&entry));
    }

    #[test]
    fn filter_matches_since_positive() {
        let ts = Utc::now();
        let filter = LogFilter::new().since(ts - Duration::hours(1));
        let entry = LogEntry {
            sequence: 0,
            event: make_node_started("a", ts),
        };
        assert!(filter.matches(&entry));
    }

    #[test]
    fn filter_matches_since_negative() {
        let ts = Utc::now() - Duration::hours(2);
        let filter = LogFilter::new().since(Utc::now() - Duration::hours(1));
        let entry = LogEntry {
            sequence: 0,
            event: make_node_started("a", ts),
        };
        assert!(!filter.matches(&entry));
    }

    #[test]
    fn filter_matches_until_positive() {
        let ts = Utc::now() - Duration::hours(2);
        let filter = LogFilter::new().until(Utc::now());
        let entry = LogEntry {
            sequence: 0,
            event: make_node_started("a", ts),
        };
        assert!(filter.matches(&entry));
    }

    #[test]
    fn filter_matches_until_negative() {
        let ts = Utc::now();
        let filter = LogFilter::new().until(Utc::now() - Duration::hours(1));
        let entry = LogEntry {
            sequence: 0,
            event: make_node_started("a", ts),
        };
        assert!(!filter.matches(&entry));
    }

    // ---------------------------------------------------------------
    // Combined filters
    // ---------------------------------------------------------------

    #[tokio::test]
    async fn in_memory_query_combined_node_id_and_event_kind() {
        let sink = InMemoryLogSink::new();
        let ts = now();

        sink.append(make_node_started("alpha", ts)).await.unwrap();
        sink.append(make_node_completed("alpha", ts)).await.unwrap();
        sink.append(make_node_started("beta", ts)).await.unwrap();
        sink.append(make_node_completed("beta", ts)).await.unwrap();

        let filter = LogFilter::new()
            .node_id("alpha")
            .event_kind("node_completed");
        let results = sink.query(&filter).await.unwrap();

        assert_eq!(results.len(), 1);
        assert!(matches!(
            results[0].event,
            PipelineEvent::NodeCompleted { ref node_id, .. }
            if node_id == "alpha"
        ));
    }

    #[tokio::test]
    async fn in_memory_query_combined_time_and_limit() {
        let sink = InMemoryLogSink::new();

        let t_old = Utc::now() - Duration::hours(5);
        let t_mid = Utc::now() - Duration::hours(2);
        let t_recent = Utc::now();

        sink.append(make_node_started("a", t_old)).await.unwrap();
        sink.append(make_node_started("b", t_mid)).await.unwrap();
        sink.append(make_node_started("c", t_mid)).await.unwrap();
        sink.append(make_node_started("d", t_recent)).await.unwrap();

        let cutoff = Utc::now() - Duration::hours(3);
        let filter = LogFilter::new().since(cutoff).limit(2);
        let results = sink.query(&filter).await.unwrap();

        assert_eq!(results.len(), 2);
        // Should be the first 2 entries that pass the since filter: b and c
        assert!(matches!(
            results[0].event,
            PipelineEvent::NodeStarted { ref node_id, .. }
            if node_id == "b"
        ));
        assert!(matches!(
            results[1].event,
            PipelineEvent::NodeStarted { ref node_id, .. }
            if node_id == "c"
        ));
    }

    #[tokio::test]
    async fn in_memory_query_all_filters_combined() {
        let sink = InMemoryLogSink::new();

        let t1 = Utc::now() - Duration::hours(3);
        let t2 = Utc::now() - Duration::hours(1);
        let t3 = Utc::now();

        // Node alpha at t1 - too old
        sink.append(make_node_started("alpha", t1)).await.unwrap();
        // Node alpha completed at t2 - matches
        sink.append(make_node_completed("alpha", t2)).await.unwrap();
        // Node beta completed at t2 - wrong node
        sink.append(make_node_completed("beta", t2)).await.unwrap();
        // Node alpha started at t2 - wrong kind
        sink.append(make_node_started("alpha", t2)).await.unwrap();
        // Node alpha completed at t3 - matches
        sink.append(make_node_completed("alpha", t3)).await.unwrap();

        let cutoff = Utc::now() - Duration::hours(2);
        let filter = LogFilter::new()
            .node_id("alpha")
            .event_kind("node_completed")
            .since(cutoff)
            .limit(1);
        let results = sink.query(&filter).await.unwrap();

        assert_eq!(results.len(), 1);
        assert!(matches!(
            results[0].event,
            PipelineEvent::NodeCompleted { ref node_id, .. }
            if node_id == "alpha"
        ));
    }

    // ---------------------------------------------------------------
    // InMemoryLogSink: empty query returns all
    // ---------------------------------------------------------------

    #[tokio::test]
    async fn in_memory_query_no_filter_returns_all() {
        let sink = InMemoryLogSink::new();
        let ts = now();

        sink.append(make_node_started("a", ts)).await.unwrap();
        sink.append(make_node_completed("a", ts)).await.unwrap();

        let filter = LogFilter::new();
        let results = sink.query(&filter).await.unwrap();
        assert_eq!(results.len(), 2);
    }

    // ---------------------------------------------------------------
    // FileLogSink: append creates JSONL
    // ---------------------------------------------------------------

    #[tokio::test]
    async fn file_sink_append_creates_jsonl() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("events.jsonl");
        let sink = FileLogSink::new(path.clone());

        let ts = now();
        let seq0 = sink.append(make_node_started("a", ts)).await.unwrap();
        let seq1 = sink.append(make_node_completed("a", ts)).await.unwrap();

        assert_eq!(seq0, 0);
        assert_eq!(seq1, 1);

        // Verify file content is valid JSONL
        let contents = tokio::fs::read_to_string(&path).await.unwrap();
        let lines: Vec<&str> = contents.lines().collect();
        assert_eq!(lines.len(), 2);

        // Each line should be valid JSON deserializable as LogEntry
        let entry0: LogEntry = serde_json::from_str(lines[0]).unwrap();
        let entry1: LogEntry = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(entry0.sequence, 0);
        assert_eq!(entry1.sequence, 1);
    }

    // ---------------------------------------------------------------
    // FileLogSink: query reads back entries
    // ---------------------------------------------------------------

    #[tokio::test]
    async fn file_sink_query_reads_back_entries() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("events.jsonl");
        let sink = FileLogSink::new(path.clone());

        let ts = now();
        sink.append(make_node_started("alpha", ts)).await.unwrap();
        sink.append(make_node_started("beta", ts)).await.unwrap();
        sink.append(make_node_completed("alpha", ts)).await.unwrap();

        let filter = LogFilter::new().node_id("alpha");
        let results = sink.query(&filter).await.unwrap();

        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|e| e.event.node_id() == Some("alpha")));
    }

    #[tokio::test]
    async fn file_sink_query_no_file_returns_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("nonexistent.jsonl");
        let sink = FileLogSink::new(path);

        let filter = LogFilter::new();
        let results = sink.query(&filter).await.unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn file_sink_count() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("events.jsonl");
        let sink = FileLogSink::new(path);

        assert_eq!(sink.count().await.unwrap(), 0);

        sink.append(make_node_started("a", now())).await.unwrap();
        assert_eq!(sink.count().await.unwrap(), 1);

        sink.append(make_node_started("b", now())).await.unwrap();
        assert_eq!(sink.count().await.unwrap(), 2);
    }

    #[tokio::test]
    async fn file_sink_count_no_file_returns_zero() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("nonexistent.jsonl");
        let sink = FileLogSink::new(path);

        assert_eq!(sink.count().await.unwrap(), 0);
    }

    // ---------------------------------------------------------------
    // FileLogSink: query with event_kind filter
    // ---------------------------------------------------------------

    #[tokio::test]
    async fn file_sink_query_event_kind() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("events.jsonl");
        let sink = FileLogSink::new(path);

        let ts = now();
        sink.append(make_node_started("a", ts)).await.unwrap();
        sink.append(make_node_completed("a", ts)).await.unwrap();
        sink.append(make_pipeline_completed(ts)).await.unwrap();

        let filter = LogFilter::new().event_kind("pipeline_completed");
        let results = sink.query(&filter).await.unwrap();
        assert_eq!(results.len(), 1);
        assert!(matches!(
            results[0].event,
            PipelineEvent::PipelineCompleted { .. }
        ));
    }

    // ---------------------------------------------------------------
    // FileLogSink: query with limit
    // ---------------------------------------------------------------

    #[tokio::test]
    async fn file_sink_query_with_limit() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("events.jsonl");
        let sink = FileLogSink::new(path);

        let ts = now();
        for i in 0..5 {
            sink.append(make_node_started(&format!("node_{i}"), ts))
                .await
                .unwrap();
        }

        let filter = LogFilter::new().limit(2);
        let results = sink.query(&filter).await.unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].sequence, 0);
        assert_eq!(results[1].sequence, 1);
    }

    // ---------------------------------------------------------------
    // event_kind_str covers all variants
    // ---------------------------------------------------------------

    #[test]
    fn event_kind_str_covers_all_variants() {
        let ts = now();
        let cases = vec![
            (make_node_started("a", ts), "node_started"),
            (make_node_completed("a", ts), "node_completed"),
            (
                PipelineEvent::NodeFailed {
                    node_id: "a".into(),
                    error: "err".into(),
                    duration_ms: 0,
                    timestamp: ts,
                },
                "node_failed",
            ),
            (
                PipelineEvent::EdgeTraversed {
                    from: "a".into(),
                    to: "b".into(),
                    label: None,
                    timestamp: ts,
                },
                "edge_traversed",
            ),
            (
                PipelineEvent::HumanPromptIssued {
                    node_id: "a".into(),
                    question: "?".into(),
                    timestamp: ts,
                },
                "human_prompt_issued",
            ),
            (
                PipelineEvent::HumanResponseReceived {
                    node_id: "a".into(),
                    response: "yes".into(),
                    timestamp: ts,
                },
                "human_response_received",
            ),
            (
                PipelineEvent::ContextUpdated {
                    key: "k".into(),
                    timestamp: ts,
                },
                "context_updated",
            ),
            (
                PipelineEvent::CheckpointCreated {
                    node_id: "a".into(),
                    timestamp: ts,
                },
                "checkpoint_created",
            ),
            (make_pipeline_started("p", ts), "pipeline_started"),
            (make_pipeline_completed(ts), "pipeline_completed"),
            (
                PipelineEvent::PipelineAborted {
                    reason: "cancelled".into(),
                    timestamp: ts,
                },
                "pipeline_aborted",
            ),
            (
                PipelineEvent::LoopRestarted {
                    from: "a".into(),
                    to: "b".into(),
                    restart_count: 1,
                    timestamp: ts,
                },
                "loop_restarted",
            ),
        ];

        for (event, expected_kind) in cases {
            assert_eq!(
                event_kind_str(&event),
                expected_kind,
                "kind mismatch for {:?}",
                event
            );
        }
    }

    // ---------------------------------------------------------------
    // Verify event_kind_str matches serde tag
    // ---------------------------------------------------------------

    #[test]
    fn event_kind_str_matches_serde_kind_tag() {
        let event = make_node_started("test", now());
        let json = serde_json::to_string(&event).unwrap();
        let kind = event_kind_str(&event);
        assert!(
            json.contains(&format!("\"kind\":\"{kind}\"")),
            "serde tag does not match event_kind_str: json={json}, kind={kind}"
        );
    }

    // ---------------------------------------------------------------
    // RetentionPolicy defaults
    // ---------------------------------------------------------------

    #[test]
    fn retention_policy_default_has_no_limits() {
        let policy = RetentionPolicy::default();
        assert!(policy.max_entries.is_none());
        assert!(policy.max_age.is_none());
        assert!(policy.max_file_size_bytes.is_none());
    }

    #[test]
    fn retention_policy_with_max_entries() {
        let policy = RetentionPolicy::with_max_entries(100);
        assert_eq!(policy.max_entries, Some(100));
        assert!(policy.max_age.is_none());
        assert!(policy.max_file_size_bytes.is_none());
    }

    #[test]
    fn retention_policy_with_max_age() {
        let dur = Duration::hours(24);
        let policy = RetentionPolicy::with_max_age(dur);
        assert!(policy.max_entries.is_none());
        assert_eq!(policy.max_age, Some(dur));
        assert!(policy.max_file_size_bytes.is_none());
    }

    // ---------------------------------------------------------------
    // apply_retention with max_entries
    // ---------------------------------------------------------------

    #[tokio::test]
    async fn apply_retention_max_entries_removes_oldest() {
        let sink = InMemoryLogSink::new();
        let ts = now();

        for i in 0..10 {
            sink.append(make_node_started(&format!("node_{i}"), ts))
                .await
                .unwrap();
        }
        assert_eq!(sink.count().await.unwrap(), 10);

        let policy = RetentionPolicy::with_max_entries(5);
        let removed = sink.apply_retention(&policy).await.unwrap();
        assert_eq!(removed, 5);
        assert_eq!(sink.count().await.unwrap(), 5);

        // The remaining entries should be the last 5 (sequences 5..9)
        let entries = sink.entries();
        assert_eq!(entries[0].sequence, 5);
        assert_eq!(entries[4].sequence, 9);
    }

    // ---------------------------------------------------------------
    // apply_retention with max_age
    // ---------------------------------------------------------------

    #[tokio::test]
    async fn apply_retention_max_age_removes_old_entries() {
        let sink = InMemoryLogSink::new();

        let old_ts = Utc::now() - Duration::hours(5);
        let recent_ts = Utc::now();

        sink.append(make_node_started("old_1", old_ts))
            .await
            .unwrap();
        sink.append(make_node_started("old_2", old_ts))
            .await
            .unwrap();
        sink.append(make_node_started("recent_1", recent_ts))
            .await
            .unwrap();

        let policy = RetentionPolicy::with_max_age(Duration::hours(1));
        let removed = sink.apply_retention(&policy).await.unwrap();
        assert_eq!(removed, 2);
        assert_eq!(sink.count().await.unwrap(), 1);

        let entries = sink.entries();
        assert!(matches!(
            entries[0].event,
            PipelineEvent::NodeStarted { ref node_id, .. }
            if node_id == "recent_1"
        ));
    }

    // ---------------------------------------------------------------
    // apply_retention removes nothing when within limits
    // ---------------------------------------------------------------

    #[tokio::test]
    async fn apply_retention_removes_nothing_when_within_limits() {
        let sink = InMemoryLogSink::new();
        let ts = now();

        sink.append(make_node_started("a", ts)).await.unwrap();
        sink.append(make_node_started("b", ts)).await.unwrap();

        let policy = RetentionPolicy::with_max_entries(10);
        let removed = sink.apply_retention(&policy).await.unwrap();
        assert_eq!(removed, 0);
        assert_eq!(sink.count().await.unwrap(), 2);
    }

    #[tokio::test]
    async fn apply_retention_default_policy_removes_nothing() {
        let sink = InMemoryLogSink::new();
        let ts = now();

        sink.append(make_node_started("a", ts)).await.unwrap();
        sink.append(make_node_started("b", ts)).await.unwrap();

        let policy = RetentionPolicy::default();
        let removed = sink.apply_retention(&policy).await.unwrap();
        assert_eq!(removed, 0);
        assert_eq!(sink.count().await.unwrap(), 2);
    }

    // ---------------------------------------------------------------
    // LogIndex: build and query by node
    // ---------------------------------------------------------------

    #[tokio::test]
    async fn log_index_build_and_query_by_node() {
        let sink = InMemoryLogSink::new();
        let ts = now();

        sink.append(make_node_started("alpha", ts)).await.unwrap();
        sink.append(make_node_started("beta", ts)).await.unwrap();
        sink.append(make_node_completed("alpha", ts)).await.unwrap();
        sink.append(make_pipeline_started("pipeline", ts))
            .await
            .unwrap();

        let index = sink.build_index();

        let alpha_seqs = index.sequences_for_node("alpha");
        assert_eq!(alpha_seqs, &[0, 2]);

        let beta_seqs = index.sequences_for_node("beta");
        assert_eq!(beta_seqs, &[1]);

        // Pipeline-level events have no node_id
        let pipeline_seqs = index.sequences_for_node("pipeline");
        assert_eq!(pipeline_seqs, &[] as &[u64]);
    }

    // ---------------------------------------------------------------
    // LogIndex: build and query by kind
    // ---------------------------------------------------------------

    #[tokio::test]
    async fn log_index_build_and_query_by_kind() {
        let sink = InMemoryLogSink::new();
        let ts = now();

        sink.append(make_node_started("a", ts)).await.unwrap();
        sink.append(make_node_completed("a", ts)).await.unwrap();
        sink.append(make_node_started("b", ts)).await.unwrap();
        sink.append(make_pipeline_completed(ts)).await.unwrap();

        let index = sink.build_index();

        let started_seqs = index.sequences_for_kind("node_started");
        assert_eq!(started_seqs, &[0, 2]);

        let completed_seqs = index.sequences_for_kind("node_completed");
        assert_eq!(completed_seqs, &[1]);

        let pipeline_completed_seqs = index.sequences_for_kind("pipeline_completed");
        assert_eq!(pipeline_completed_seqs, &[3]);
    }

    // ---------------------------------------------------------------
    // LogIndex: empty entries
    // ---------------------------------------------------------------

    #[test]
    fn log_index_empty_entries() {
        let index = LogIndex::build(&[]);
        assert_eq!(index.sequences_for_node("anything"), &[] as &[u64]);
        assert_eq!(index.sequences_for_kind("anything"), &[] as &[u64]);
    }
}
