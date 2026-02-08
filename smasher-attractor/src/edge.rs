// ABOUTME: Edge selection algorithm for choosing the next node transition.
// ABOUTME: Implements a 5-step priority algorithm considering conditions, outcomes, and priorities.

use crate::condition::{evaluate_condition, parse_condition};
use crate::graph::{Graph, GraphEdge};
use crate::state::{Context, Outcome};

/// Errors that can occur during edge selection.
#[derive(Debug, thiserror::Error)]
pub enum EdgeSelectionError {
    #[error("condition parse error on edge {from} -> {to}: {message}")]
    ConditionParseError {
        from: String,
        to: String,
        message: String,
    },
}

/// Check if an edge label matches a given outcome.
///
/// Matching rules (case-insensitive):
/// - Success: "success", "yes", "true"
/// - Failure: "failure", "error", "no", "false"
/// - Skip: "skip"
fn edge_matches_outcome(edge: &GraphEdge, outcome: &Outcome) -> bool {
    let label = match &edge.label {
        Some(l) => l.to_lowercase(),
        None => return false,
    };

    match outcome {
        Outcome::Success { .. } => matches!(label.as_str(), "success" | "yes" | "true"),
        Outcome::Failure { .. } => matches!(label.as_str(), "failure" | "error" | "no" | "false"),
        Outcome::Skip { .. } => label.as_str() == "skip",
    }
}

/// Determine whether an edge has an explicit condition (not just a label echo).
///
/// The graph resolution module falls back to the label when no explicit condition
/// attribute is present. When `condition == label`, the condition was derived from
/// the label and should not be parsed as a condition expression.
fn has_explicit_condition(edge: &GraphEdge) -> bool {
    match (&edge.condition, &edge.label) {
        (Some(cond), Some(label)) => cond != label,
        (Some(_), None) => true,
        _ => false,
    }
}

/// Select the best outgoing edge from a node using the 5-step priority algorithm.
///
/// Steps:
/// 1. Gather all outgoing edges from `node_id`
/// 2. Evaluate conditions: filter out edges whose conditions are false
/// 3. Apply outcome-based filtering if a matching edge exists
/// 4. Sort remaining edges by priority (descending, default 0)
/// 5. Return the highest-priority edge, or None if no candidates remain
pub fn select_edge<'a>(
    graph: &'a Graph,
    node_id: &str,
    context: &Context,
    last_outcome: Option<&Outcome>,
) -> Result<Option<&'a GraphEdge>, EdgeSelectionError> {
    // Step 1: Gather candidates
    let candidates = graph.edges_from(node_id);
    if candidates.is_empty() {
        return Ok(None);
    }

    // Step 2: Evaluate conditions
    let ctx_map = context.to_string_map();
    let mut passing: Vec<&GraphEdge> = Vec::new();

    for edge in &candidates {
        if has_explicit_condition(edge) {
            let cond_str = edge.condition.as_ref().unwrap();
            let parsed = parse_condition(cond_str).map_err(|e| {
                EdgeSelectionError::ConditionParseError {
                    from: edge.from.clone(),
                    to: edge.to.clone(),
                    message: e.to_string(),
                }
            })?;
            if evaluate_condition(&parsed, &ctx_map) {
                passing.push(edge);
            }
        } else {
            // No explicit condition -- edge passes through
            passing.push(edge);
        }
    }

    if passing.is_empty() {
        return Ok(None);
    }

    // Step 3: Check outcome-based edges
    if let Some(outcome) = last_outcome {
        let outcome_matched: Vec<&GraphEdge> = passing
            .iter()
            .copied()
            .filter(|e| edge_matches_outcome(e, outcome))
            .collect();

        // Only filter to outcome-matched edges if at least one exists
        if !outcome_matched.is_empty() {
            passing = outcome_matched;
        }
    }

    // Step 4: Sort by priority (higher number = higher priority, descending)
    passing.sort_by(|a, b| {
        let pa = a.priority.unwrap_or(0);
        let pb = b.priority.unwrap_or(0);
        pb.cmp(&pa)
    });

    // Step 5: Return highest priority
    Ok(passing.into_iter().next())
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::graph::{GraphEdge, GraphNode, NodeType};
    use crate::state::{Context, Outcome};
    use serde_json::json;

    /// Helper: build a minimal Graph with the given edges and auto-generated nodes.
    fn make_graph(edges: Vec<GraphEdge>) -> Graph {
        let mut node_ids = std::collections::HashSet::new();
        for edge in &edges {
            node_ids.insert(edge.from.clone());
            node_ids.insert(edge.to.clone());
        }
        let nodes: Vec<GraphNode> = node_ids
            .into_iter()
            .map(|id| GraphNode {
                id,
                node_type: NodeType::Generic,
                label: None,
                attrs: HashMap::new(),
            })
            .collect();

        Graph {
            name: None,
            nodes,
            edges,
            default_node_attrs: HashMap::new(),
            default_edge_attrs: HashMap::new(),
        }
    }

    /// Helper: build a simple unconditional edge with no label or priority.
    fn plain_edge(from: &str, to: &str) -> GraphEdge {
        GraphEdge {
            from: from.to_string(),
            to: to.to_string(),
            label: None,
            condition: None,
            priority: None,
            attrs: HashMap::new(),
        }
    }

    /// Helper: build an edge with an explicit condition (different from label).
    fn conditional_edge(from: &str, to: &str, condition: &str) -> GraphEdge {
        GraphEdge {
            from: from.to_string(),
            to: to.to_string(),
            label: None,
            condition: Some(condition.to_string()),
            priority: None,
            attrs: HashMap::new(),
        }
    }

    /// Helper: build an edge with a label (used for outcome matching).
    fn labeled_edge(from: &str, to: &str, label: &str) -> GraphEdge {
        // Mimic graph resolution behavior: when only label is set,
        // condition is also set to the label value.
        GraphEdge {
            from: from.to_string(),
            to: to.to_string(),
            label: Some(label.to_string()),
            condition: Some(label.to_string()),
            priority: None,
            attrs: HashMap::new(),
        }
    }

    /// Helper: build an edge with a label and explicit priority.
    fn labeled_priority_edge(from: &str, to: &str, label: &str, priority: i32) -> GraphEdge {
        GraphEdge {
            from: from.to_string(),
            to: to.to_string(),
            label: Some(label.to_string()),
            condition: Some(label.to_string()),
            priority: Some(priority),
            attrs: HashMap::new(),
        }
    }

    /// Helper: build an edge with priority but no label.
    fn priority_edge(from: &str, to: &str, priority: i32) -> GraphEdge {
        GraphEdge {
            from: from.to_string(),
            to: to.to_string(),
            label: None,
            condition: None,
            priority: Some(priority),
            attrs: HashMap::new(),
        }
    }

    // ---- Test 1: No outgoing edges returns None ----
    #[test]
    fn no_outgoing_edges_returns_none() {
        let graph = make_graph(vec![plain_edge("a", "b")]);
        let ctx = Context::new();
        let result = select_edge(&graph, "b", &ctx, None).unwrap();
        assert!(result.is_none());
    }

    // ---- Test 2: Single unconditional edge is selected ----
    #[test]
    fn single_unconditional_edge_selected() {
        let graph = make_graph(vec![plain_edge("a", "b")]);
        let ctx = Context::new();
        let result = select_edge(&graph, "a", &ctx, None).unwrap();
        assert!(result.is_some());
        let edge = result.unwrap();
        assert_eq!(edge.from, "a");
        assert_eq!(edge.to, "b");
    }

    // ---- Test 3: Condition evaluates to true -> edge selected ----
    #[test]
    fn condition_true_selects_edge() {
        let graph = make_graph(vec![conditional_edge("a", "b", "status=done")]);
        let ctx = Context::new();
        ctx.set("status", json!("done"));
        let result = select_edge(&graph, "a", &ctx, None).unwrap();
        assert!(result.is_some());
        assert_eq!(result.unwrap().to, "b");
    }

    // ---- Test 4: Condition evaluates to false -> edge filtered out ----
    #[test]
    fn condition_false_filters_edge() {
        let graph = make_graph(vec![conditional_edge("a", "b", "status=done")]);
        let ctx = Context::new();
        ctx.set("status", json!("pending"));
        let result = select_edge(&graph, "a", &ctx, None).unwrap();
        assert!(result.is_none());
    }

    // ---- Test 5: Multiple edges, highest priority wins ----
    #[test]
    fn highest_priority_wins() {
        let graph = make_graph(vec![
            priority_edge("a", "low", 1),
            priority_edge("a", "high", 10),
            priority_edge("a", "mid", 5),
        ]);
        let ctx = Context::new();
        let result = select_edge(&graph, "a", &ctx, None).unwrap();
        assert!(result.is_some());
        assert_eq!(result.unwrap().to, "high");
    }

    // ---- Test 6: Edges without priority default to 0 ----
    #[test]
    fn edges_without_priority_default_to_zero() {
        let graph = make_graph(vec![
            plain_edge("a", "default_prio"),
            priority_edge("a", "explicit_one", 1),
        ]);
        let ctx = Context::new();
        let result = select_edge(&graph, "a", &ctx, None).unwrap();
        assert!(result.is_some());
        // Priority 1 beats the default 0
        assert_eq!(result.unwrap().to, "explicit_one");
    }

    // ---- Test 7: Outcome-based filtering for success label ----
    #[test]
    fn outcome_success_prefers_success_label() {
        let graph = make_graph(vec![
            labeled_edge("a", "ok_path", "success"),
            labeled_edge("a", "err_path", "failure"),
        ]);
        let ctx = Context::new();
        let outcome = Outcome::success();
        let result = select_edge(&graph, "a", &ctx, Some(&outcome)).unwrap();
        assert!(result.is_some());
        assert_eq!(result.unwrap().to, "ok_path");
    }

    // ---- Test 8: Outcome-based filtering for failure label ----
    #[test]
    fn outcome_failure_prefers_failure_label() {
        let graph = make_graph(vec![
            labeled_edge("a", "ok_path", "success"),
            labeled_edge("a", "err_path", "error"),
        ]);
        let ctx = Context::new();
        let outcome = Outcome::failure("something broke");
        let result = select_edge(&graph, "a", &ctx, Some(&outcome)).unwrap();
        assert!(result.is_some());
        assert_eq!(result.unwrap().to, "err_path");
    }

    // ---- Test 9: Outcome-based filtering for skip label ----
    #[test]
    fn outcome_skip_prefers_skip_label() {
        let graph = make_graph(vec![
            labeled_edge("a", "ok_path", "success"),
            labeled_edge("a", "skip_path", "skip"),
        ]);
        let ctx = Context::new();
        let outcome = Outcome::skip("not needed");
        let result = select_edge(&graph, "a", &ctx, Some(&outcome)).unwrap();
        assert!(result.is_some());
        assert_eq!(result.unwrap().to, "skip_path");
    }

    // ---- Test 10: Outcome-based filtering falls back when no matching label ----
    #[test]
    fn outcome_fallback_when_no_matching_label() {
        let graph = make_graph(vec![
            labeled_edge("a", "other_path", "proceed"),
            plain_edge("a", "default_path"),
        ]);
        let ctx = Context::new();
        let outcome = Outcome::success();
        // No edge labeled "success"/"yes"/"true", so all candidates remain
        let result = select_edge(&graph, "a", &ctx, Some(&outcome)).unwrap();
        assert!(result.is_some());
    }

    // ---- Test 11: Invalid condition returns error ----
    #[test]
    fn invalid_condition_returns_error() {
        // An explicit condition that differs from label and is unparseable
        let edge = GraphEdge {
            from: "a".to_string(),
            to: "b".to_string(),
            label: Some("display".to_string()),
            condition: Some("@#$%^&*".to_string()),
            priority: None,
            attrs: HashMap::new(),
        };
        let graph = make_graph(vec![edge]);
        let ctx = Context::new();
        let result = select_edge(&graph, "a", &ctx, None);
        assert!(result.is_err());
        let err = result.unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("condition parse error on edge a -> b"));
    }

    // ---- Test 12: Mixed conditions and priorities ----
    #[test]
    fn mixed_conditions_and_priorities() {
        // Two conditional edges from "a": one passes with high priority, one passes with low
        let mut edge_high = conditional_edge("a", "winner", "mode=fast");
        edge_high.priority = Some(10);

        let mut edge_low = conditional_edge("a", "loser", "mode=fast");
        edge_low.priority = Some(1);

        let graph = make_graph(vec![edge_high, edge_low]);
        let ctx = Context::new();
        ctx.set("mode", json!("fast"));

        let result = select_edge(&graph, "a", &ctx, None).unwrap();
        assert!(result.is_some());
        assert_eq!(result.unwrap().to, "winner");
    }

    // ---- Test 13: Case-insensitive outcome matching ----
    #[test]
    fn case_insensitive_outcome_matching() {
        let graph = make_graph(vec![
            labeled_edge("a", "upper", "SUCCESS"),
            labeled_edge("a", "other", "failure"),
        ]);
        let ctx = Context::new();
        let outcome = Outcome::success();
        let result = select_edge(&graph, "a", &ctx, Some(&outcome)).unwrap();
        assert!(result.is_some());
        assert_eq!(result.unwrap().to, "upper");
    }

    // ---- Test 14: All conditions false returns None ----
    #[test]
    fn all_conditions_false_returns_none() {
        let graph = make_graph(vec![
            conditional_edge("a", "b", "x=1"),
            conditional_edge("a", "c", "x=2"),
        ]);
        let ctx = Context::new();
        ctx.set("x", json!("999"));
        let result = select_edge(&graph, "a", &ctx, None).unwrap();
        assert!(result.is_none());
    }

    // ---- Test 15: edge_matches_outcome with success variants ----
    #[test]
    fn edge_matches_outcome_success_variants() {
        let outcome = Outcome::success();
        for label in &["success", "yes", "true"] {
            let edge = labeled_edge("a", "b", label);
            assert!(
                edge_matches_outcome(&edge, &outcome),
                "label '{label}' should match Success"
            );
        }
        // Non-matching labels
        for label in &["failure", "error", "skip", "no", "other"] {
            let edge = labeled_edge("a", "b", label);
            assert!(
                !edge_matches_outcome(&edge, &outcome),
                "label '{label}' should NOT match Success"
            );
        }
    }

    // ---- Test 16: edge_matches_outcome with failure variants ----
    #[test]
    fn edge_matches_outcome_failure_variants() {
        let outcome = Outcome::failure("oops");
        for label in &["failure", "error", "no", "false"] {
            let edge = labeled_edge("a", "b", label);
            assert!(
                edge_matches_outcome(&edge, &outcome),
                "label '{label}' should match Failure"
            );
        }
        for label in &["success", "yes", "skip", "other"] {
            let edge = labeled_edge("a", "b", label);
            assert!(
                !edge_matches_outcome(&edge, &outcome),
                "label '{label}' should NOT match Failure"
            );
        }
    }

    // ---- Test 17: edge_matches_outcome with skip ----
    #[test]
    fn edge_matches_outcome_skip() {
        let outcome = Outcome::skip("reason");
        let edge = labeled_edge("a", "b", "skip");
        assert!(edge_matches_outcome(&edge, &outcome));

        let non_skip = labeled_edge("a", "b", "success");
        assert!(!edge_matches_outcome(&non_skip, &outcome));
    }

    // ---- Test 18: edge_matches_outcome with no label returns false ----
    #[test]
    fn edge_matches_outcome_no_label() {
        let outcome = Outcome::success();
        let edge = plain_edge("a", "b");
        assert!(!edge_matches_outcome(&edge, &outcome));
    }

    // ---- Test 19: Node with no edges in graph returns None ----
    #[test]
    fn node_not_in_graph_returns_none() {
        let graph = make_graph(vec![plain_edge("a", "b")]);
        let ctx = Context::new();
        let result = select_edge(&graph, "nonexistent", &ctx, None).unwrap();
        assert!(result.is_none());
    }

    // ---- Test 20: Outcome with priority - outcome filtering then priority sort ----
    #[test]
    fn outcome_filtering_then_priority_sort() {
        let graph = make_graph(vec![
            labeled_priority_edge("a", "low_success", "yes", 1),
            labeled_priority_edge("a", "high_success", "YES", 10),
            labeled_priority_edge("a", "failure_path", "failure", 100),
        ]);
        let ctx = Context::new();
        let outcome = Outcome::success();
        let result = select_edge(&graph, "a", &ctx, Some(&outcome)).unwrap();
        assert!(result.is_some());
        // "failure" is filtered out by outcome matching, then "high_success" wins on priority
        assert_eq!(result.unwrap().to, "high_success");
    }

    // ---- Test 21: Label-only edge (condition == label) is not parsed as condition ----
    #[test]
    fn label_only_edge_not_parsed_as_condition() {
        // "success" is not a valid condition expression, but since condition == label,
        // it should be treated as label-only and pass through step 2 without error.
        let graph = make_graph(vec![labeled_edge("a", "b", "success")]);
        let ctx = Context::new();
        let result = select_edge(&graph, "a", &ctx, None);
        // Should not error, should select the edge
        assert!(result.is_ok());
        assert!(result.unwrap().is_some());
    }

    // ---- Test 22: Condition with explicit condition AND label ----
    #[test]
    fn explicit_condition_with_label() {
        // Edge has an explicit condition different from label
        let edge = GraphEdge {
            from: "a".to_string(),
            to: "b".to_string(),
            label: Some("success".to_string()),
            condition: Some("status=ok".to_string()),
            priority: None,
            attrs: HashMap::new(),
        };
        let graph = make_graph(vec![edge]);

        // Condition passes
        let ctx = Context::new();
        ctx.set("status", json!("ok"));
        let result = select_edge(&graph, "a", &ctx, None).unwrap();
        assert!(result.is_some());

        // Condition fails
        let ctx2 = Context::new();
        ctx2.set("status", json!("bad"));
        let result2 = select_edge(&graph, "a", &ctx2, None).unwrap();
        assert!(result2.is_none());
    }
}
