// ABOUTME: Goal gate enforcement for critical pipeline nodes.
// ABOUTME: Ensures designated goal nodes are visited before pipeline completion.

use std::fmt;

use crate::graph::{Graph, NodeAttrValue};
use crate::state::{Checkpoint, Outcome};

/// Errors related to goal enforcement.
#[derive(Debug, thiserror::Error)]
pub enum GoalError {
    #[error("pipeline completion blocked: {unmet_count} goal(s) not yet visited: {unmet_goals}")]
    GoalsNotMet {
        unmet_count: usize,
        unmet_goals: String,
    },
}

/// Result of checking goal completion against visited nodes.
#[derive(Debug, Clone, PartialEq)]
pub struct GoalStatus {
    pub total: usize,
    pub met: Vec<String>,
    pub unmet: Vec<String>,
}

impl GoalStatus {
    /// Returns true when every goal has been met (no unmet goals remain).
    pub fn is_complete(&self) -> bool {
        self.unmet.is_empty()
    }

    /// Returns the fraction of goals met as a value between 0.0 and 1.0.
    /// Returns 0.0 if there are no goals defined.
    pub fn progress_fraction(&self) -> f64 {
        if self.total == 0 {
            return 0.0;
        }
        self.met.len() as f64 / self.total as f64
    }
}

impl fmt::Display for GoalStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{} goals met", self.met.len(), self.total)
    }
}

/// Describes which goal gate node is unsatisfied and why.
#[derive(Debug, Clone)]
pub struct UnsatisfiedGoal {
    pub node_id: String,
    pub reason: String,
}

impl fmt::Display for UnsatisfiedGoal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "goal '{}' unsatisfied: {}", self.node_id, self.reason)
    }
}

/// Manages the set of goal nodes and checks whether they have been visited.
///
/// A goal node is any graph node whose `attrs` map contains `goal_gate=true`
/// (canonical) or `goal=true` (legacy fallback). When both are present,
/// `goal_gate` takes precedence.
#[derive(Debug, Clone)]
pub struct GoalGate {
    goals: Vec<String>,
}

impl GoalGate {
    /// Scan all nodes in the graph for `goal_gate=true` (or legacy `goal=true`)
    /// attributes and collect their IDs. When both are present, `goal_gate` wins.
    pub fn from_graph(graph: &Graph) -> GoalGate {
        let goals = graph
            .nodes
            .iter()
            .filter(|node| {
                // Prefer `goal_gate` attribute; fall back to `goal` for backward compat
                let attr = node
                    .attrs
                    .get("goal_gate")
                    .or_else(|| node.attrs.get("goal"));
                matches!(attr, Some(NodeAttrValue::Bool(true)))
            })
            .map(|node| node.id.clone())
            .collect();
        GoalGate { goals }
    }

    /// Returns the list of goal node IDs.
    pub fn goals(&self) -> &[String] {
        &self.goals
    }

    /// Returns true if no goals are defined.
    pub fn is_empty(&self) -> bool {
        self.goals.is_empty()
    }

    /// Check which goals have been met vs not, given a list of visited node IDs.
    pub fn check(&self, visited: &[String]) -> GoalStatus {
        let mut met = Vec::new();
        let mut unmet = Vec::new();
        for goal in &self.goals {
            if visited.iter().any(|v| v == goal) {
                met.push(goal.clone());
            } else {
                unmet.push(goal.clone());
            }
        }
        GoalStatus {
            total: self.goals.len(),
            met,
            unmet,
        }
    }

    /// Check goal completion against a Checkpoint's visited nodes.
    pub fn check_checkpoint(&self, checkpoint: &Checkpoint) -> GoalStatus {
        let mut met = Vec::new();
        let mut unmet = Vec::new();
        for goal in &self.goals {
            if checkpoint.was_visited(goal) {
                met.push(goal.clone());
            } else {
                unmet.push(goal.clone());
            }
        }
        GoalStatus {
            total: self.goals.len(),
            met,
            unmet,
        }
    }

    /// Returns true if all goals have been visited.
    pub fn all_met(&self, visited: &[String]) -> bool {
        self.goals
            .iter()
            .all(|goal| visited.iter().any(|v| v == goal))
    }

    /// Returns the goal IDs that have NOT been visited yet.
    pub fn unmet_goals<'a>(&'a self, visited: &[String]) -> Vec<&'a str> {
        self.goals
            .iter()
            .filter(|goal| !visited.iter().any(|v| v == goal.as_str()))
            .map(|s| s.as_str())
            .collect()
    }

    /// Returns the goal IDs that HAVE been visited.
    pub fn met_goals<'a>(&'a self, visited: &[String]) -> Vec<&'a str> {
        self.goals
            .iter()
            .filter(|goal| visited.iter().any(|v| v == goal.as_str()))
            .map(|s| s.as_str())
            .collect()
    }

    /// Check goal gates per spec section 3.4.
    ///
    /// All nodes with `goal_gate=true` must have an outcome of SUCCESS or
    /// PARTIAL_SUCCESS. Unvisited goals (missing from outcomes map) and goals
    /// with any other outcome status are considered unsatisfied.
    ///
    /// Returns the first unsatisfied goal found, or Ok(()) if all gates pass.
    pub fn check_outcomes(
        &self,
        node_outcomes: &std::collections::HashMap<String, Outcome>,
    ) -> Result<(), UnsatisfiedGoal> {
        for goal_id in &self.goals {
            match node_outcomes.get(goal_id) {
                Some(outcome) if outcome.is_success() => continue,
                Some(_) => {
                    return Err(UnsatisfiedGoal {
                        node_id: goal_id.clone(),
                        reason: "non-success outcome".to_string(),
                    });
                }
                None => {
                    return Err(UnsatisfiedGoal {
                        node_id: goal_id.clone(),
                        reason: "not visited".to_string(),
                    });
                }
            }
        }
        Ok(())
    }

    /// Enforce that all goals have been met. Returns `Ok(())` if all goals
    /// are visited, or a `GoalError::GoalsNotMet` describing the unmet goals.
    pub fn enforce(&self, visited: &[String]) -> Result<(), GoalError> {
        let unmet: Vec<&str> = self.unmet_goals(visited);
        if unmet.is_empty() {
            Ok(())
        } else {
            Err(GoalError::GoalsNotMet {
                unmet_count: unmet.len(),
                unmet_goals: unmet.join(", "),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{GraphNode, NodeType};
    use crate::state::{Context, Outcome};
    use std::collections::HashMap;

    /// Helper: build a GraphNode with optional goal attribute.
    fn make_node(id: &str, goal: bool) -> GraphNode {
        let mut attrs = HashMap::new();
        if goal {
            attrs.insert("goal".to_string(), NodeAttrValue::Bool(true));
        }
        GraphNode {
            id: id.to_string(),
            node_type: NodeType::Generic,
            label: None,
            attrs,
        }
    }

    /// Helper: build a GraphNode with a non-bool goal attribute.
    fn make_node_with_string_goal(id: &str, goal_value: &str) -> GraphNode {
        let mut attrs = HashMap::new();
        attrs.insert(
            "goal".to_string(),
            NodeAttrValue::String(goal_value.to_string()),
        );
        GraphNode {
            id: id.to_string(),
            node_type: NodeType::Generic,
            label: None,
            attrs,
        }
    }

    /// Helper: build a GraphNode with goal=false attribute.
    fn make_node_with_false_goal(id: &str) -> GraphNode {
        let mut attrs = HashMap::new();
        attrs.insert("goal".to_string(), NodeAttrValue::Bool(false));
        GraphNode {
            id: id.to_string(),
            node_type: NodeType::Generic,
            label: None,
            attrs,
        }
    }

    /// Helper: build a minimal Graph from a list of nodes.
    fn make_graph(nodes: Vec<GraphNode>) -> Graph {
        Graph {
            name: None,
            nodes,
            edges: vec![],
            default_node_attrs: HashMap::new(),
            default_edge_attrs: HashMap::new(),
            graph_attrs: HashMap::new(),
        }
    }

    /// Helper: convert a slice of &str into Vec<String>.
    fn visited(ids: &[&str]) -> Vec<String> {
        ids.iter().map(|s| s.to_string()).collect()
    }

    // ---- Test 1: from_graph with no goal nodes ----
    #[test]
    fn from_graph_no_goal_nodes() {
        let graph = make_graph(vec![make_node("a", false), make_node("b", false)]);
        let gate = GoalGate::from_graph(&graph);
        assert!(gate.goals().is_empty());
        assert!(gate.is_empty());
    }

    // ---- Test 2: from_graph with one goal node ----
    #[test]
    fn from_graph_one_goal_node() {
        let graph = make_graph(vec![
            make_node("a", false),
            make_node("b", true),
            make_node("c", false),
        ]);
        let gate = GoalGate::from_graph(&graph);
        assert_eq!(gate.goals(), &["b".to_string()]);
        assert!(!gate.is_empty());
    }

    // ---- Test 3: from_graph with multiple goal nodes ----
    #[test]
    fn from_graph_multiple_goal_nodes() {
        let graph = make_graph(vec![
            make_node("a", true),
            make_node("b", false),
            make_node("c", true),
            make_node("d", true),
        ]);
        let gate = GoalGate::from_graph(&graph);
        assert_eq!(gate.goals().len(), 3);
        let goal_ids: Vec<&str> = gate.goals().iter().map(|s| s.as_str()).collect();
        assert!(goal_ids.contains(&"a"));
        assert!(goal_ids.contains(&"c"));
        assert!(goal_ids.contains(&"d"));
    }

    // ---- Test 4: Non-bool goal attribute is ignored ----
    #[test]
    fn non_bool_goal_attribute_is_ignored() {
        let graph = make_graph(vec![
            make_node_with_string_goal("a", "true"),
            make_node_with_string_goal("b", "yes"),
            make_node("c", true),
        ]);
        let gate = GoalGate::from_graph(&graph);
        // Only "c" has a real Bool(true) goal attribute
        assert_eq!(gate.goals().len(), 1);
        assert_eq!(gate.goals()[0], "c");
    }

    // ---- Test 5: goal=false is not treated as a goal ----
    #[test]
    fn goal_false_is_not_a_goal() {
        let graph = make_graph(vec![make_node_with_false_goal("a"), make_node("b", true)]);
        let gate = GoalGate::from_graph(&graph);
        assert_eq!(gate.goals().len(), 1);
        assert_eq!(gate.goals()[0], "b");
    }

    // ---- Test 6: check with no goals met ----
    #[test]
    fn check_no_goals_met() {
        let graph = make_graph(vec![make_node("a", true), make_node("b", true)]);
        let gate = GoalGate::from_graph(&graph);
        let status = gate.check(&visited(&["x", "y"]));
        assert_eq!(status.total, 2);
        assert!(status.met.is_empty());
        assert_eq!(status.unmet.len(), 2);
        assert!(!status.is_complete());
    }

    // ---- Test 7: check with some goals met ----
    #[test]
    fn check_some_goals_met() {
        let graph = make_graph(vec![
            make_node("a", true),
            make_node("b", true),
            make_node("c", true),
        ]);
        let gate = GoalGate::from_graph(&graph);
        let status = gate.check(&visited(&["a", "x"]));
        assert_eq!(status.total, 3);
        assert_eq!(status.met, vec!["a".to_string()]);
        assert_eq!(status.unmet.len(), 2);
        assert!(!status.is_complete());
    }

    // ---- Test 8: check with all goals met ----
    #[test]
    fn check_all_goals_met() {
        let graph = make_graph(vec![make_node("a", true), make_node("b", true)]);
        let gate = GoalGate::from_graph(&graph);
        let status = gate.check(&visited(&["a", "b", "extra"]));
        assert_eq!(status.total, 2);
        assert_eq!(status.met.len(), 2);
        assert!(status.unmet.is_empty());
        assert!(status.is_complete());
    }

    // ---- Test 9: all_met returns true when all visited ----
    #[test]
    fn all_met_returns_true_when_all_visited() {
        let graph = make_graph(vec![make_node("g1", true), make_node("g2", true)]);
        let gate = GoalGate::from_graph(&graph);
        assert!(gate.all_met(&visited(&["g1", "g2"])));
        assert!(gate.all_met(&visited(&["g1", "g2", "extra"])));
    }

    // ---- Test 10: all_met returns false when some missing ----
    #[test]
    fn all_met_returns_false_when_missing() {
        let graph = make_graph(vec![make_node("g1", true), make_node("g2", true)]);
        let gate = GoalGate::from_graph(&graph);
        assert!(!gate.all_met(&visited(&["g1"])));
        assert!(!gate.all_met(&visited(&[])));
    }

    // ---- Test 11: all_met with no goals always returns true ----
    #[test]
    fn all_met_no_goals_returns_true() {
        let graph = make_graph(vec![make_node("a", false)]);
        let gate = GoalGate::from_graph(&graph);
        assert!(gate.all_met(&visited(&[])));
    }

    // ---- Test 12: unmet_goals and met_goals accessors ----
    #[test]
    fn unmet_goals_and_met_goals() {
        let graph = make_graph(vec![
            make_node("g1", true),
            make_node("g2", true),
            make_node("g3", true),
        ]);
        let gate = GoalGate::from_graph(&graph);
        let v = visited(&["g1", "g3"]);

        let met = gate.met_goals(&v);
        assert!(met.contains(&"g1"));
        assert!(met.contains(&"g3"));
        assert_eq!(met.len(), 2);

        let unmet = gate.unmet_goals(&v);
        assert_eq!(unmet, vec!["g2"]);
    }

    // ---- Test 13: enforce returns Ok when all met ----
    #[test]
    fn enforce_returns_ok_when_all_met() {
        let graph = make_graph(vec![make_node("g1", true), make_node("g2", true)]);
        let gate = GoalGate::from_graph(&graph);
        let result = gate.enforce(&visited(&["g1", "g2"]));
        assert!(result.is_ok());
    }

    // ---- Test 14: enforce returns GoalError when unmet ----
    #[test]
    fn enforce_returns_error_when_unmet() {
        let graph = make_graph(vec![
            make_node("g1", true),
            make_node("g2", true),
            make_node("g3", true),
        ]);
        let gate = GoalGate::from_graph(&graph);
        let result = gate.enforce(&visited(&["g1"]));
        assert!(result.is_err());
        let err = result.unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("2 goal(s) not yet visited"));
        assert!(msg.contains("g2"));
        assert!(msg.contains("g3"));
    }

    // ---- Test 15: enforce with no goals returns Ok ----
    #[test]
    fn enforce_no_goals_returns_ok() {
        let graph = make_graph(vec![make_node("a", false)]);
        let gate = GoalGate::from_graph(&graph);
        assert!(gate.enforce(&visited(&[])).is_ok());
    }

    // ---- Test 16: GoalStatus progress_fraction calculation ----
    #[test]
    fn progress_fraction_calculation() {
        let status = GoalStatus {
            total: 4,
            met: vec!["a".into(), "b".into()],
            unmet: vec!["c".into(), "d".into()],
        };
        assert!((status.progress_fraction() - 0.5).abs() < f64::EPSILON);

        let all_met = GoalStatus {
            total: 3,
            met: vec!["a".into(), "b".into(), "c".into()],
            unmet: vec![],
        };
        assert!((all_met.progress_fraction() - 1.0).abs() < f64::EPSILON);

        let none_met = GoalStatus {
            total: 2,
            met: vec![],
            unmet: vec!["a".into(), "b".into()],
        };
        assert!((none_met.progress_fraction() - 0.0).abs() < f64::EPSILON);
    }

    // ---- Test 17: progress_fraction with zero goals ----
    #[test]
    fn progress_fraction_zero_goals() {
        let status = GoalStatus {
            total: 0,
            met: vec![],
            unmet: vec![],
        };
        assert!((status.progress_fraction() - 0.0).abs() < f64::EPSILON);
    }

    // ---- Test 18: GoalStatus Display output ----
    #[test]
    fn goal_status_display() {
        let status = GoalStatus {
            total: 5,
            met: vec!["a".into(), "b".into(), "c".into()],
            unmet: vec!["d".into(), "e".into()],
        };
        assert_eq!(format!("{status}"), "3/5 goals met");

        let complete = GoalStatus {
            total: 2,
            met: vec!["a".into(), "b".into()],
            unmet: vec![],
        };
        assert_eq!(format!("{complete}"), "2/2 goals met");

        let empty = GoalStatus {
            total: 0,
            met: vec![],
            unmet: vec![],
        };
        assert_eq!(format!("{empty}"), "0/0 goals met");
    }

    // ---- Test 19: check_checkpoint integration ----
    #[test]
    fn check_checkpoint_integration() {
        let graph = make_graph(vec![
            make_node("g1", true),
            make_node("g2", true),
            make_node("g3", true),
        ]);
        let gate = GoalGate::from_graph(&graph);

        let ctx = Context::new();
        let mut checkpoint = Checkpoint::new("test_pipeline", "g2", &ctx);
        checkpoint.mark_visited("g1");
        checkpoint.mark_visited("g2");

        let status = gate.check_checkpoint(&checkpoint);
        assert_eq!(status.total, 3);
        assert_eq!(status.met.len(), 2);
        assert!(status.met.contains(&"g1".to_string()));
        assert!(status.met.contains(&"g2".to_string()));
        assert_eq!(status.unmet, vec!["g3".to_string()]);
        assert!(!status.is_complete());
    }

    // ---- Test 20: check_checkpoint all visited ----
    #[test]
    fn check_checkpoint_all_visited() {
        let graph = make_graph(vec![make_node("g1", true), make_node("g2", true)]);
        let gate = GoalGate::from_graph(&graph);

        let ctx = Context::new();
        let mut checkpoint = Checkpoint::new("test_pipeline", "g2", &ctx);
        checkpoint.mark_visited("g1");
        checkpoint.mark_visited("g2");

        let status = gate.check_checkpoint(&checkpoint);
        assert!(status.is_complete());
        assert!((status.progress_fraction() - 1.0).abs() < f64::EPSILON);
    }

    // ---- Test 21: is_empty for empty vs non-empty ----
    #[test]
    fn is_empty_for_empty_vs_non_empty() {
        let empty_graph = make_graph(vec![make_node("a", false), make_node("b", false)]);
        let empty_gate = GoalGate::from_graph(&empty_graph);
        assert!(empty_gate.is_empty());

        let nonempty_graph = make_graph(vec![make_node("a", true)]);
        let nonempty_gate = GoalGate::from_graph(&nonempty_graph);
        assert!(!nonempty_gate.is_empty());
    }

    // ---- Test 22: GoalStatus is_complete ----
    #[test]
    fn goal_status_is_complete() {
        let complete = GoalStatus {
            total: 2,
            met: vec!["a".into(), "b".into()],
            unmet: vec![],
        };
        assert!(complete.is_complete());

        let incomplete = GoalStatus {
            total: 2,
            met: vec!["a".into()],
            unmet: vec!["b".into()],
        };
        assert!(!incomplete.is_complete());

        // Zero goals is considered complete (vacuously true)
        let empty = GoalStatus {
            total: 0,
            met: vec![],
            unmet: vec![],
        };
        assert!(empty.is_complete());
    }

    // ---- Test 23: from_graph with empty graph ----
    #[test]
    fn from_graph_empty_graph() {
        let graph = make_graph(vec![]);
        let gate = GoalGate::from_graph(&graph);
        assert!(gate.is_empty());
        assert!(gate.goals().is_empty());
    }

    // ---- Test 24: check with empty graph produces complete status ----
    #[test]
    fn check_empty_gate_is_complete() {
        let graph = make_graph(vec![]);
        let gate = GoalGate::from_graph(&graph);
        let status = gate.check(&visited(&[]));
        assert!(status.is_complete());
        assert_eq!(status.total, 0);
    }

    // ---- Test 25: GoalError display message format ----
    #[test]
    fn goal_error_display_message() {
        let err = GoalError::GoalsNotMet {
            unmet_count: 2,
            unmet_goals: "deploy, validate".to_string(),
        };
        let msg = err.to_string();
        assert_eq!(
            msg,
            "pipeline completion blocked: 2 goal(s) not yet visited: deploy, validate"
        );
    }

    // ---- Test 26: goal_gate attribute is the canonical attribute name ----
    #[test]
    fn goal_gate_attribute_is_recognized() {
        let mut attrs = HashMap::new();
        attrs.insert("goal_gate".to_string(), NodeAttrValue::Bool(true));
        let graph = make_graph(vec![GraphNode {
            id: "n1".to_string(),
            node_type: NodeType::Generic,
            label: None,
            attrs,
        }]);
        let gate = GoalGate::from_graph(&graph);
        assert_eq!(gate.goals().len(), 1);
        assert_eq!(gate.goals()[0], "n1");
    }

    // ---- Test 27: goal_gate takes precedence over goal attribute ----
    #[test]
    fn goal_gate_takes_precedence_over_goal() {
        let mut attrs = HashMap::new();
        // goal_gate=false should win over goal=true
        attrs.insert("goal_gate".to_string(), NodeAttrValue::Bool(false));
        attrs.insert("goal".to_string(), NodeAttrValue::Bool(true));
        let graph = make_graph(vec![GraphNode {
            id: "n1".to_string(),
            node_type: NodeType::Generic,
            label: None,
            attrs,
        }]);
        let gate = GoalGate::from_graph(&graph);
        // goal_gate=false takes precedence, so no goals
        assert!(gate.is_empty());
    }

    // ---- Test 28: legacy goal attribute still works as fallback ----
    #[test]
    fn legacy_goal_attribute_still_works() {
        // When only "goal" is set (no "goal_gate"), it should still be recognized
        let graph = make_graph(vec![make_node("g1", true)]);
        let gate = GoalGate::from_graph(&graph);
        assert_eq!(gate.goals().len(), 1);
        assert_eq!(gate.goals()[0], "g1");
    }

    // ---- Test 29: number goal attribute is ignored ----
    #[test]
    fn number_goal_attribute_is_ignored() {
        let mut attrs = HashMap::new();
        attrs.insert("goal".to_string(), NodeAttrValue::Number(1.0));
        let graph = make_graph(vec![GraphNode {
            id: "n1".to_string(),
            node_type: NodeType::Generic,
            label: None,
            attrs,
        }]);
        let gate = GoalGate::from_graph(&graph);
        assert!(gate.is_empty());
    }

    // ---- Test 30: check_outcomes all succeeded ----
    #[test]
    fn check_outcomes_all_succeeded() {
        let graph = make_graph(vec![make_node("g1", true), make_node("g2", true)]);
        let gate = GoalGate::from_graph(&graph);
        let mut outcomes = HashMap::new();
        outcomes.insert("g1".to_string(), Outcome::success());
        outcomes.insert("g2".to_string(), Outcome::success());
        assert!(gate.check_outcomes(&outcomes).is_ok());
    }

    // ---- Test 31: check_outcomes partial success counts as met ----
    #[test]
    fn check_outcomes_partial_success_counts_as_met() {
        let graph = make_graph(vec![make_node("g1", true)]);
        let gate = GoalGate::from_graph(&graph);
        let mut outcomes = HashMap::new();
        outcomes.insert("g1".to_string(), Outcome::partial_success());
        assert!(gate.check_outcomes(&outcomes).is_ok());
    }

    // ---- Test 32: check_outcomes failed goal returns unsatisfied ----
    #[test]
    fn check_outcomes_failed_goal_returns_unsatisfied() {
        let graph = make_graph(vec![make_node("g1", true), make_node("g2", true)]);
        let gate = GoalGate::from_graph(&graph);
        let mut outcomes = HashMap::new();
        outcomes.insert("g1".to_string(), Outcome::success());
        outcomes.insert("g2".to_string(), Outcome::failure("timed out"));
        let err = gate.check_outcomes(&outcomes).unwrap_err();
        assert_eq!(err.node_id, "g2");
    }

    // ---- Test 33: check_outcomes unvisited goal returns unsatisfied ----
    #[test]
    fn check_outcomes_unvisited_goal_returns_unsatisfied() {
        let graph = make_graph(vec![make_node("g1", true), make_node("g2", true)]);
        let gate = GoalGate::from_graph(&graph);
        let mut outcomes = HashMap::new();
        outcomes.insert("g1".to_string(), Outcome::success());
        // g2 not in outcomes — unvisited
        let err = gate.check_outcomes(&outcomes).unwrap_err();
        assert_eq!(err.node_id, "g2");
        assert!(err.reason.contains("not visited"));
    }

    // ---- Test 34: check_outcomes skipped goal returns unsatisfied ----
    #[test]
    fn check_outcomes_skipped_goal_returns_unsatisfied() {
        let graph = make_graph(vec![make_node("g1", true)]);
        let gate = GoalGate::from_graph(&graph);
        let mut outcomes = HashMap::new();
        outcomes.insert("g1".to_string(), Outcome::skip("branch not taken"));
        let err = gate.check_outcomes(&outcomes).unwrap_err();
        assert_eq!(err.node_id, "g1");
    }

    // ---- Test 35: check_outcomes no goals always ok ----
    #[test]
    fn check_outcomes_no_goals_always_ok() {
        let graph = make_graph(vec![make_node("a", false)]);
        let gate = GoalGate::from_graph(&graph);
        let outcomes = HashMap::new();
        assert!(gate.check_outcomes(&outcomes).is_ok());
    }

    // ---- Test 36: UnsatisfiedGoal Display output ----
    #[test]
    fn unsatisfied_goal_display() {
        let ug = UnsatisfiedGoal {
            node_id: "deploy".to_string(),
            reason: "not visited".to_string(),
        };
        assert_eq!(format!("{ug}"), "goal 'deploy' unsatisfied: not visited");
    }
}
