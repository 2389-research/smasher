// ABOUTME: In-memory artifact storage for pipeline outputs.
// ABOUTME: Stores, retrieves, and manages artifacts produced during pipeline execution.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, RwLock};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Errors that can occur during artifact operations.
#[derive(Debug, thiserror::Error)]
pub enum ArtifactError {
    #[error("artifact not found: {id}")]
    NotFound { id: String },
    #[error("artifact storage error: {message}")]
    StorageError { message: String },
}

/// Metadata associated with a stored artifact.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactMetadata {
    pub node_id: String,
    pub name: String,
    pub content_type: String,
    pub created_at: DateTime<Utc>,
    pub tags: Vec<String>,
}

/// A pipeline artifact containing metadata and arbitrary JSON data.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Artifact {
    pub id: String,
    pub metadata: ArtifactMetadata,
    pub data: serde_json::Value,
}

/// Thread-safe in-memory store for pipeline artifacts.
///
/// Uses `Arc<RwLock<HashMap>>` internally, making clones cheap (shared reference)
/// and allowing concurrent readers with exclusive writers. An atomic counter
/// ensures unique IDs across threads.
pub struct ArtifactStore {
    artifacts: Arc<RwLock<HashMap<String, Artifact>>>,
    counter: Arc<AtomicUsize>,
}

impl Default for ArtifactStore {
    fn default() -> Self {
        Self {
            artifacts: Arc::new(RwLock::new(HashMap::new())),
            counter: Arc::new(AtomicUsize::new(0)),
        }
    }
}

impl Clone for ArtifactStore {
    fn clone(&self) -> Self {
        Self {
            artifacts: Arc::clone(&self.artifacts),
            counter: Arc::clone(&self.counter),
        }
    }
}

impl ArtifactStore {
    /// Create an empty artifact store.
    pub fn new() -> Self {
        Self::default()
    }

    /// Store an artifact and return its unique ID.
    ///
    /// The ID is generated as `{node_id}_{name}_{counter}` for deterministic
    /// uniqueness within a store instance.
    pub fn store(
        &self,
        node_id: &str,
        name: &str,
        content_type: &str,
        data: serde_json::Value,
    ) -> String {
        self.store_with_tags(node_id, name, content_type, data, Vec::new())
    }

    /// Store an artifact with tags and return its unique ID.
    pub fn store_with_tags(
        &self,
        node_id: &str,
        name: &str,
        content_type: &str,
        data: serde_json::Value,
        tags: Vec<String>,
    ) -> String {
        let seq = self.counter.fetch_add(1, Ordering::SeqCst);
        let id = format!("{node_id}_{name}_{seq}");

        let artifact = Artifact {
            id: id.clone(),
            metadata: ArtifactMetadata {
                node_id: node_id.to_string(),
                name: name.to_string(),
                content_type: content_type.to_string(),
                created_at: Utc::now(),
                tags,
            },
            data,
        };

        if let Ok(mut guard) = self.artifacts.write() {
            guard.insert(id.clone(), artifact);
        }

        id
    }

    /// Retrieve an artifact by its ID, returning `None` if not found.
    pub fn get(&self, id: &str) -> Option<Artifact> {
        let guard = self.artifacts.read().ok()?;
        guard.get(id).cloned()
    }

    /// Retrieve all artifacts produced by a given node.
    pub fn get_by_node(&self, node_id: &str) -> Vec<Artifact> {
        match self.artifacts.read() {
            Ok(guard) => guard
                .values()
                .filter(|a| a.metadata.node_id == node_id)
                .cloned()
                .collect(),
            Err(_) => Vec::new(),
        }
    }

    /// Retrieve all artifacts that carry a given tag.
    pub fn get_by_tag(&self, tag: &str) -> Vec<Artifact> {
        match self.artifacts.read() {
            Ok(guard) => guard
                .values()
                .filter(|a| a.metadata.tags.iter().any(|t| t == tag))
                .cloned()
                .collect(),
            Err(_) => Vec::new(),
        }
    }

    /// List metadata for every stored artifact (without the heavy data payloads).
    pub fn list(&self) -> Vec<ArtifactMetadata> {
        match self.artifacts.read() {
            Ok(guard) => guard.values().map(|a| a.metadata.clone()).collect(),
            Err(_) => Vec::new(),
        }
    }

    /// Remove an artifact by ID, returning it if it existed.
    pub fn remove(&self, id: &str) -> Option<Artifact> {
        let mut guard = self.artifacts.write().ok()?;
        guard.remove(id)
    }

    /// Return the number of stored artifacts.
    pub fn count(&self) -> usize {
        match self.artifacts.read() {
            Ok(guard) => guard.len(),
            Err(_) => 0,
        }
    }

    /// Remove all artifacts from the store.
    pub fn clear(&self) {
        if let Ok(mut guard) = self.artifacts.write() {
            guard.clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ---------------------------------------------------------------
    // Store and retrieve
    // ---------------------------------------------------------------

    #[test]
    fn store_and_retrieve_artifact_by_id() {
        let store = ArtifactStore::new();
        let id = store.store(
            "node_1",
            "output",
            "application/json",
            json!({"key": "value"}),
        );
        let artifact = store.get(&id).expect("artifact should exist");
        assert_eq!(artifact.id, id);
        assert_eq!(artifact.metadata.node_id, "node_1");
        assert_eq!(artifact.metadata.name, "output");
        assert_eq!(artifact.metadata.content_type, "application/json");
        assert_eq!(artifact.data, json!({"key": "value"}));
        assert!(artifact.metadata.tags.is_empty());
    }

    #[test]
    fn store_with_tags_and_retrieve_by_tag() {
        let store = ArtifactStore::new();
        let id = store.store_with_tags(
            "node_1",
            "report",
            "text/plain",
            json!("some report"),
            vec!["important".to_string(), "final".to_string()],
        );
        let artifact = store.get(&id).unwrap();
        assert_eq!(artifact.metadata.tags, vec!["important", "final"]);
    }

    // ---------------------------------------------------------------
    // get_by_node
    // ---------------------------------------------------------------

    #[test]
    fn get_by_node_returns_all_artifacts_from_node() {
        let store = ArtifactStore::new();
        store.store("node_a", "out1", "text/plain", json!("first"));
        store.store("node_a", "out2", "text/plain", json!("second"));
        store.store("node_b", "out1", "text/plain", json!("third"));

        let node_a_artifacts = store.get_by_node("node_a");
        assert_eq!(node_a_artifacts.len(), 2);
        for a in &node_a_artifacts {
            assert_eq!(a.metadata.node_id, "node_a");
        }
    }

    #[test]
    fn get_by_node_with_no_matching_artifacts_returns_empty() {
        let store = ArtifactStore::new();
        store.store("node_a", "out", "text/plain", json!("data"));
        let results = store.get_by_node("nonexistent_node");
        assert!(results.is_empty());
    }

    // ---------------------------------------------------------------
    // get_by_tag
    // ---------------------------------------------------------------

    #[test]
    fn get_by_tag_returns_matching_artifacts() {
        let store = ArtifactStore::new();
        store.store_with_tags(
            "n1",
            "a",
            "text/plain",
            json!("tagged"),
            vec!["alpha".to_string(), "beta".to_string()],
        );
        store.store_with_tags(
            "n2",
            "b",
            "text/plain",
            json!("also tagged"),
            vec!["beta".to_string()],
        );
        store.store("n3", "c", "text/plain", json!("untagged"));

        let beta_artifacts = store.get_by_tag("beta");
        assert_eq!(beta_artifacts.len(), 2);

        let alpha_artifacts = store.get_by_tag("alpha");
        assert_eq!(alpha_artifacts.len(), 1);

        let gamma_artifacts = store.get_by_tag("gamma");
        assert!(gamma_artifacts.is_empty());
    }

    // ---------------------------------------------------------------
    // list
    // ---------------------------------------------------------------

    #[test]
    fn list_returns_metadata_for_all_artifacts() {
        let store = ArtifactStore::new();
        store.store(
            "n1",
            "output",
            "application/json",
            json!({"big": "payload"}),
        );
        store.store("n2", "log", "text/plain", json!("log data"));

        let metadata_list = store.list();
        assert_eq!(metadata_list.len(), 2);

        let names: Vec<&str> = metadata_list.iter().map(|m| m.name.as_str()).collect();
        assert!(names.contains(&"output"));
        assert!(names.contains(&"log"));
    }

    // ---------------------------------------------------------------
    // remove
    // ---------------------------------------------------------------

    #[test]
    fn remove_returns_artifact_and_removes_from_store() {
        let store = ArtifactStore::new();
        let id = store.store("node", "out", "text/plain", json!("data"));
        assert_eq!(store.count(), 1);

        let removed = store.remove(&id);
        assert!(removed.is_some());
        assert_eq!(removed.unwrap().id, id);
        assert_eq!(store.count(), 0);
        assert!(store.get(&id).is_none());
    }

    #[test]
    fn remove_nonexistent_returns_none() {
        let store = ArtifactStore::new();
        assert!(store.remove("does_not_exist").is_none());
    }

    // ---------------------------------------------------------------
    // count
    // ---------------------------------------------------------------

    #[test]
    fn count_returns_correct_count() {
        let store = ArtifactStore::new();
        assert_eq!(store.count(), 0);

        store.store("n1", "a", "text/plain", json!(1));
        assert_eq!(store.count(), 1);

        store.store("n2", "b", "text/plain", json!(2));
        assert_eq!(store.count(), 2);

        store.store("n3", "c", "text/plain", json!(3));
        assert_eq!(store.count(), 3);
    }

    // ---------------------------------------------------------------
    // clear
    // ---------------------------------------------------------------

    #[test]
    fn clear_removes_all_artifacts() {
        let store = ArtifactStore::new();
        store.store("n1", "a", "text/plain", json!(1));
        store.store("n2", "b", "text/plain", json!(2));
        assert_eq!(store.count(), 2);

        store.clear();
        assert_eq!(store.count(), 0);
        assert!(store.list().is_empty());
    }

    // ---------------------------------------------------------------
    // Unique ID generation
    // ---------------------------------------------------------------

    #[test]
    fn store_generates_unique_ids() {
        let store = ArtifactStore::new();
        let id1 = store.store("node", "out", "text/plain", json!(1));
        let id2 = store.store("node", "out", "text/plain", json!(2));
        let id3 = store.store("node", "out", "text/plain", json!(3));

        assert_ne!(id1, id2);
        assert_ne!(id2, id3);
        assert_ne!(id1, id3);
    }

    // ---------------------------------------------------------------
    // Thread safety
    // ---------------------------------------------------------------

    #[test]
    fn concurrent_store_and_retrieve() {
        let store = ArtifactStore::new();
        let handles: Vec<_> = (0..20)
            .map(|i| {
                let store = store.clone();
                std::thread::spawn(move || {
                    let id = store.store(&format!("node_{i}"), "output", "text/plain", json!(i));
                    // Immediately retrieve the artifact we just stored
                    let artifact = store.get(&id);
                    assert!(artifact.is_some(), "artifact {id} should be retrievable");
                    id
                })
            })
            .collect();

        let ids: Vec<String> = handles.into_iter().map(|h| h.join().unwrap()).collect();

        // All 20 artifacts should be present
        assert_eq!(store.count(), 20);

        // All IDs should be unique
        let unique: std::collections::HashSet<&String> = ids.iter().collect();
        assert_eq!(unique.len(), 20);
    }

    // ---------------------------------------------------------------
    // ArtifactError display formatting
    // ---------------------------------------------------------------

    #[test]
    fn artifact_error_display_formatting() {
        let not_found = ArtifactError::NotFound {
            id: "abc_123".to_string(),
        };
        assert_eq!(not_found.to_string(), "artifact not found: abc_123");

        let storage_err = ArtifactError::StorageError {
            message: "disk full".to_string(),
        };
        assert_eq!(storage_err.to_string(), "artifact storage error: disk full");
    }

    // ---------------------------------------------------------------
    // Default trait
    // ---------------------------------------------------------------

    #[test]
    fn default_trait_for_artifact_store() {
        let store = ArtifactStore::default();
        assert_eq!(store.count(), 0);
        assert!(store.list().is_empty());
    }

    // ---------------------------------------------------------------
    // get nonexistent
    // ---------------------------------------------------------------

    #[test]
    fn get_nonexistent_returns_none() {
        let store = ArtifactStore::new();
        assert!(store.get("no_such_id").is_none());
    }

    // ---------------------------------------------------------------
    // Serialization roundtrip
    // ---------------------------------------------------------------

    #[test]
    fn artifact_serialization_roundtrip() {
        let artifact = Artifact {
            id: "node_1_output_0".to_string(),
            metadata: ArtifactMetadata {
                node_id: "node_1".to_string(),
                name: "output".to_string(),
                content_type: "application/json".to_string(),
                created_at: Utc::now(),
                tags: vec!["tag_a".to_string(), "tag_b".to_string()],
            },
            data: json!({"result": [1, 2, 3]}),
        };

        let json_str = serde_json::to_string(&artifact).unwrap();
        let deserialized: Artifact = serde_json::from_str(&json_str).unwrap();

        assert_eq!(deserialized.id, artifact.id);
        assert_eq!(deserialized.metadata.node_id, artifact.metadata.node_id);
        assert_eq!(deserialized.metadata.name, artifact.metadata.name);
        assert_eq!(
            deserialized.metadata.content_type,
            artifact.metadata.content_type
        );
        assert_eq!(deserialized.metadata.tags, artifact.metadata.tags);
        assert_eq!(deserialized.data, artifact.data);
        // chrono DateTime roundtrips through serde
        assert_eq!(
            deserialized.metadata.created_at,
            artifact.metadata.created_at
        );
    }

    #[test]
    fn artifact_metadata_serialization_roundtrip() {
        let metadata = ArtifactMetadata {
            node_id: "test_node".to_string(),
            name: "test_artifact".to_string(),
            content_type: "text/plain".to_string(),
            created_at: Utc::now(),
            tags: vec!["x".to_string()],
        };

        let json_str = serde_json::to_string(&metadata).unwrap();
        let deserialized: ArtifactMetadata = serde_json::from_str(&json_str).unwrap();

        assert_eq!(deserialized.node_id, metadata.node_id);
        assert_eq!(deserialized.name, metadata.name);
        assert_eq!(deserialized.content_type, metadata.content_type);
        assert_eq!(deserialized.created_at, metadata.created_at);
        assert_eq!(deserialized.tags, metadata.tags);
    }
}
