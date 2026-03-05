// ABOUTME: Pipeline execution statistics tracking per-node timing and outcome counts.
// ABOUTME: Provides NodeStats, OutcomeKind, and PipelineStats for post-run reporting.

//! Pipeline statistics collected during execution.
//!
//! After every pipeline run, an [`ExecutionResult`] carries a [`PipelineStats`]
//! that summarises how long each node took, how many nodes succeeded or failed,
//! and the total wall-clock duration.

use std::collections::HashMap;

/// Coarse categorisation of a node's execution result.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum OutcomeKind {
    /// Node completed successfully (Success or PartialSuccess).
    Success,
    /// Node failed.
    Failure,
    /// Node explicitly requested a retry.
    Retry,
    /// Node was skipped.
    Skip,
}

/// Timing and outcome information for a single node execution.
#[derive(Debug, Clone)]
pub struct NodeStats {
    /// The node ID as it appears in the graph.
    pub node_id: String,
    /// Wall-clock duration of the node execution in milliseconds.
    pub duration_ms: u64,
    /// Coarse outcome category for this node.
    pub outcome_kind: OutcomeKind,
}

/// Aggregate statistics for a complete pipeline run.
#[derive(Debug, Clone)]
pub struct PipelineStats {
    /// Total number of node executions performed (may exceed unique node count
    /// if the same node is visited multiple times due to retries or loops).
    pub total_nodes_visited: usize,
    /// Count of nodes grouped by their coarse outcome category.
    pub nodes_by_outcome: HashMap<OutcomeKind, usize>,
    /// Total wall-clock duration for the entire pipeline in milliseconds.
    pub total_duration_ms: u64,
    /// Per-node timing records in the order the nodes were executed.
    pub node_timings: Vec<NodeStats>,
}

impl PipelineStats {
    /// Build a [`PipelineStats`] from a list of per-node records and a total duration.
    pub fn from_node_timings(node_timings: Vec<NodeStats>, total_duration_ms: u64) -> Self {
        let total_nodes_visited = node_timings.len();
        let mut nodes_by_outcome: HashMap<OutcomeKind, usize> = HashMap::new();
        for ns in &node_timings {
            *nodes_by_outcome.entry(ns.outcome_kind.clone()).or_insert(0) += 1;
        }
        Self {
            total_nodes_visited,
            nodes_by_outcome,
            total_duration_ms,
            node_timings,
        }
    }

    /// Return the top-`n` slowest nodes sorted by descending duration.
    ///
    /// If `n` is greater than the number of nodes, all nodes are returned.
    pub fn top_slowest(&self, n: usize) -> Vec<&NodeStats> {
        let mut sorted: Vec<&NodeStats> = self.node_timings.iter().collect();
        sorted.sort_by(|a, b| b.duration_ms.cmp(&a.duration_ms));
        sorted.into_iter().take(n).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_ns(id: &str, duration_ms: u64, kind: OutcomeKind) -> NodeStats {
        NodeStats {
            node_id: id.to_string(),
            duration_ms,
            outcome_kind: kind,
        }
    }

    #[test]
    fn top_slowest_returns_correct_order() {
        let timings = vec![
            make_ns("fast", 10, OutcomeKind::Success),
            make_ns("medium", 50, OutcomeKind::Success),
            make_ns("slow", 200, OutcomeKind::Failure),
            make_ns("very_slow", 500, OutcomeKind::Retry),
        ];
        let stats = PipelineStats::from_node_timings(timings, 760);

        let top2 = stats.top_slowest(2);
        assert_eq!(top2.len(), 2);
        assert_eq!(top2[0].node_id, "very_slow");
        assert_eq!(top2[0].duration_ms, 500);
        assert_eq!(top2[1].node_id, "slow");
        assert_eq!(top2[1].duration_ms, 200);
    }

    #[test]
    fn top_slowest_with_n_larger_than_node_count_returns_all() {
        let timings = vec![
            make_ns("a", 100, OutcomeKind::Success),
            make_ns("b", 200, OutcomeKind::Success),
        ];
        let stats = PipelineStats::from_node_timings(timings, 300);

        let all = stats.top_slowest(10);
        assert_eq!(all.len(), 2);
        // Still sorted descending.
        assert_eq!(all[0].node_id, "b");
        assert_eq!(all[1].node_id, "a");
    }

    #[test]
    fn nodes_by_outcome_counts_correctly() {
        let timings = vec![
            make_ns("n1", 10, OutcomeKind::Success),
            make_ns("n2", 20, OutcomeKind::Success),
            make_ns("n3", 30, OutcomeKind::Failure),
            make_ns("n4", 40, OutcomeKind::Retry),
            make_ns("n5", 50, OutcomeKind::Skip),
            make_ns("n6", 60, OutcomeKind::Success),
        ];
        let stats = PipelineStats::from_node_timings(timings, 210);

        assert_eq!(stats.nodes_by_outcome[&OutcomeKind::Success], 3);
        assert_eq!(stats.nodes_by_outcome[&OutcomeKind::Failure], 1);
        assert_eq!(stats.nodes_by_outcome[&OutcomeKind::Retry], 1);
        assert_eq!(stats.nodes_by_outcome[&OutcomeKind::Skip], 1);
        assert_eq!(stats.total_nodes_visited, 6);
    }

    #[test]
    fn total_duration_ms_is_stored() {
        let stats = PipelineStats::from_node_timings(vec![], 12345);
        assert_eq!(stats.total_duration_ms, 12345);
    }

    #[test]
    fn top_slowest_with_zero_n_returns_empty() {
        let timings = vec![make_ns("a", 100, OutcomeKind::Success)];
        let stats = PipelineStats::from_node_timings(timings, 100);
        assert!(stats.top_slowest(0).is_empty());
    }
}
