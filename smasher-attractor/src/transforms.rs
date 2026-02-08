// ABOUTME: Pipeline transforms for variable expansion and stylesheet application.
// ABOUTME: Processes graph attributes to resolve template variables and apply style rules.

use std::collections::HashMap;

use crate::graph::{Graph, NodeAttrValue};
use crate::stylesheet::Stylesheet;

/// Expand `{{variable}}` placeholders in all string attributes and labels of graph nodes.
///
/// For each node, any `NodeAttrValue::String` value in `attrs` is scanned for `{{key}}`
/// patterns. If `key` (after trimming whitespace) exists in `variables`, the placeholder
/// is replaced with the corresponding value. Unknown variables are left as-is.
/// Node labels are also expanded.
pub fn expand_variables(graph: &mut Graph, variables: &HashMap<String, String>) {
    for node in &mut graph.nodes {
        // Expand in attribute values.
        for value in node.attrs.values_mut() {
            if let NodeAttrValue::String(s) = value {
                *s = expand_string(s, variables);
            }
        }
        // Expand in label.
        if let Some(ref mut label) = node.label {
            *label = expand_string(label, variables);
        }
    }
}

/// Replace all `{{key}}` occurrences in `input` using the provided variable map.
///
/// Handles optional whitespace inside the braces: `{{ key }}` is equivalent to `{{key}}`.
/// Unknown keys are left as their original placeholder text.
fn expand_string(input: &str, variables: &HashMap<String, String>) -> String {
    let mut result = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '{' && chars.peek() == Some(&'{') {
            // Consume second '{'
            chars.next();

            // Collect everything up to '}}'
            let mut key_buf = String::new();
            let mut found_close = false;
            while let Some(inner) = chars.next() {
                if inner == '}' && chars.peek() == Some(&'}') {
                    chars.next(); // consume second '}'
                    found_close = true;
                    break;
                }
                key_buf.push(inner);
            }

            if found_close {
                let trimmed_key = key_buf.trim();
                if let Some(replacement) = variables.get(trimmed_key) {
                    result.push_str(replacement);
                } else {
                    // Leave placeholder as-is for unknown variables.
                    result.push_str("{{");
                    result.push_str(&key_buf);
                    result.push_str("}}");
                }
            } else {
                // Unterminated placeholder: emit what we consumed literally.
                result.push_str("{{");
                result.push_str(&key_buf);
            }
        } else {
            result.push(ch);
        }
    }

    result
}

/// Apply a stylesheet's computed attributes to each node in the graph.
///
/// For each node, `stylesheet.apply(node)` produces a set of computed attributes.
/// These are merged into the node's `attrs`, but the node's existing attributes
/// take precedence (they are never overwritten by the stylesheet).
pub fn apply_stylesheet(graph: &mut Graph, stylesheet: &Stylesheet) {
    for node in &mut graph.nodes {
        let computed = stylesheet.apply(node);
        for (key, value) in computed {
            // Only insert if the node does not already have this attribute.
            node.attrs.entry(key).or_insert(value);
        }
    }
}

/// Apply all transforms to a graph in the correct order.
///
/// Stylesheet is applied first (if provided), then variable expansion. This
/// ordering ensures that any string values injected by the stylesheet also
/// get their `{{variable}}` placeholders resolved.
pub fn apply_transforms(
    graph: &mut Graph,
    variables: &HashMap<String, String>,
    stylesheet: Option<&Stylesheet>,
) {
    if let Some(ss) = stylesheet {
        apply_stylesheet(graph, ss);
    }
    expand_variables(graph, variables);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{Graph, GraphNode, NodeAttrValue, NodeType};
    use crate::stylesheet::Stylesheet;
    use std::collections::HashMap;
    use std::time::Duration;

    /// Helper: build a minimal Graph with the given nodes (no edges).
    fn make_graph(nodes: Vec<GraphNode>) -> Graph {
        Graph {
            name: None,
            nodes,
            edges: Vec::new(),
            default_node_attrs: HashMap::new(),
            default_edge_attrs: HashMap::new(),
        }
    }

    /// Helper: build a GraphNode with string attributes.
    fn node_with_str_attrs(
        id: &str,
        label: Option<&str>,
        attrs: Vec<(&str, &str)>,
    ) -> GraphNode {
        let mut attr_map = HashMap::new();
        for (k, v) in attrs {
            attr_map.insert(k.to_string(), NodeAttrValue::String(v.to_string()));
        }
        GraphNode {
            id: id.to_string(),
            node_type: NodeType::Generic,
            label: label.map(|s| s.to_string()),
            attrs: attr_map,
        }
    }

    /// Helper: build a simple GraphNode with no attrs and no label.
    fn simple_node(id: &str, node_type: NodeType) -> GraphNode {
        GraphNode {
            id: id.to_string(),
            node_type,
            label: None,
            attrs: HashMap::new(),
        }
    }

    // ---- Test 1: expand_variables replaces single variable ----
    #[test]
    fn expand_variables_replaces_single_variable() {
        let mut graph = make_graph(vec![node_with_str_attrs(
            "n1",
            None,
            vec![("model", "{{model_name}}")],
        )]);
        let mut vars = HashMap::new();
        vars.insert("model_name".to_string(), "claude-sonnet".to_string());

        expand_variables(&mut graph, &vars);

        assert_eq!(
            graph.nodes[0].attrs.get("model"),
            Some(&NodeAttrValue::String("claude-sonnet".to_string()))
        );
    }

    // ---- Test 2: expand_variables replaces multiple variables in same string ----
    #[test]
    fn expand_variables_replaces_multiple_in_same_string() {
        let mut graph = make_graph(vec![node_with_str_attrs(
            "n1",
            None,
            vec![("prompt", "Hello {{name}}, your role is {{role}}")],
        )]);
        let mut vars = HashMap::new();
        vars.insert("name".to_string(), "Alice".to_string());
        vars.insert("role".to_string(), "developer".to_string());

        expand_variables(&mut graph, &vars);

        assert_eq!(
            graph.nodes[0].attrs.get("prompt"),
            Some(&NodeAttrValue::String(
                "Hello Alice, your role is developer".to_string()
            ))
        );
    }

    // ---- Test 3: expand_variables with whitespace in braces {{ key }} ----
    #[test]
    fn expand_variables_with_whitespace_in_braces() {
        let mut graph = make_graph(vec![node_with_str_attrs(
            "n1",
            None,
            vec![("value", "prefix-{{ key }}-suffix")],
        )]);
        let mut vars = HashMap::new();
        vars.insert("key".to_string(), "VALUE".to_string());

        expand_variables(&mut graph, &vars);

        assert_eq!(
            graph.nodes[0].attrs.get("value"),
            Some(&NodeAttrValue::String("prefix-VALUE-suffix".to_string()))
        );
    }

    // ---- Test 4: expand_variables leaves unknown variables as-is ----
    #[test]
    fn expand_variables_leaves_unknown_variables() {
        let mut graph = make_graph(vec![node_with_str_attrs(
            "n1",
            None,
            vec![("prompt", "Hello {{unknown_var}}")],
        )]);
        let vars = HashMap::new();

        expand_variables(&mut graph, &vars);

        assert_eq!(
            graph.nodes[0].attrs.get("prompt"),
            Some(&NodeAttrValue::String("Hello {{unknown_var}}".to_string()))
        );
    }

    // ---- Test 5: expand_variables with empty variables map (no-op) ----
    #[test]
    fn expand_variables_empty_map_is_noop() {
        let original_value = "no {{vars}} here either";
        let mut graph = make_graph(vec![node_with_str_attrs(
            "n1",
            None,
            vec![("text", original_value)],
        )]);
        let vars = HashMap::new();

        expand_variables(&mut graph, &vars);

        assert_eq!(
            graph.nodes[0].attrs.get("text"),
            Some(&NodeAttrValue::String(original_value.to_string()))
        );
    }

    // ---- Test 6: expand_variables in node labels ----
    #[test]
    fn expand_variables_in_node_labels() {
        let mut graph = make_graph(vec![node_with_str_attrs(
            "n1",
            Some("Step: {{step_name}}"),
            vec![],
        )]);
        let mut vars = HashMap::new();
        vars.insert("step_name".to_string(), "Initialize".to_string());

        expand_variables(&mut graph, &vars);

        assert_eq!(
            graph.nodes[0].label,
            Some("Step: Initialize".to_string())
        );
    }

    // ---- Test 7: expand_variables does not affect non-string attrs ----
    #[test]
    fn expand_variables_does_not_affect_non_string_attrs() {
        let mut attrs = HashMap::new();
        attrs.insert("retries".to_string(), NodeAttrValue::Number(3.0));
        attrs.insert("enabled".to_string(), NodeAttrValue::Bool(true));
        attrs.insert(
            "timeout".to_string(),
            NodeAttrValue::Duration(Duration::from_secs(30)),
        );
        let node = GraphNode {
            id: "n1".to_string(),
            node_type: NodeType::Generic,
            label: None,
            attrs,
        };
        let mut graph = make_graph(vec![node]);
        let mut vars = HashMap::new();
        vars.insert("retries".to_string(), "999".to_string());

        expand_variables(&mut graph, &vars);

        // Non-string attributes remain unchanged.
        assert_eq!(
            graph.nodes[0].attrs.get("retries"),
            Some(&NodeAttrValue::Number(3.0))
        );
        assert_eq!(
            graph.nodes[0].attrs.get("enabled"),
            Some(&NodeAttrValue::Bool(true))
        );
        assert_eq!(
            graph.nodes[0].attrs.get("timeout"),
            Some(&NodeAttrValue::Duration(Duration::from_secs(30)))
        );
    }

    // ---- Test 8: apply_stylesheet merges attrs from stylesheet ----
    #[test]
    fn apply_stylesheet_merges_attrs() {
        let ss = Stylesheet::parse(r#"codergen { model: "claude-sonnet"; max_tokens: 4096; }"#)
            .unwrap();
        let node = simple_node("gen1", NodeType::Codergen);
        let mut graph = make_graph(vec![node]);

        apply_stylesheet(&mut graph, &ss);

        assert_eq!(
            graph.nodes[0].attrs.get("model"),
            Some(&NodeAttrValue::String("claude-sonnet".to_string()))
        );
        assert_eq!(
            graph.nodes[0].attrs.get("max_tokens"),
            Some(&NodeAttrValue::Number(4096.0))
        );
    }

    // ---- Test 9: apply_stylesheet node attrs take precedence ----
    #[test]
    fn apply_stylesheet_node_attrs_take_precedence() {
        let ss = Stylesheet::parse(r#"codergen { model: "default-model"; temperature: 0.5; }"#)
            .unwrap();
        let mut attrs = HashMap::new();
        attrs.insert(
            "model".to_string(),
            NodeAttrValue::String("my-custom-model".to_string()),
        );
        let node = GraphNode {
            id: "gen1".to_string(),
            node_type: NodeType::Codergen,
            label: None,
            attrs,
        };
        let mut graph = make_graph(vec![node]);

        apply_stylesheet(&mut graph, &ss);

        // Node's existing "model" is preserved, not overwritten by stylesheet.
        assert_eq!(
            graph.nodes[0].attrs.get("model"),
            Some(&NodeAttrValue::String("my-custom-model".to_string()))
        );
        // "temperature" is new, so it gets added from stylesheet.
        assert_eq!(
            graph.nodes[0].attrs.get("temperature"),
            Some(&NodeAttrValue::Number(0.5))
        );
    }

    // ---- Test 10: apply_stylesheet no matching rules leaves node unchanged ----
    #[test]
    fn apply_stylesheet_no_matching_rules() {
        let ss = Stylesheet::parse(r#"codergen { model: "claude"; }"#).unwrap();
        let mut attrs = HashMap::new();
        attrs.insert("timeout".to_string(), NodeAttrValue::Number(60.0));
        let node = GraphNode {
            id: "my_tool".to_string(),
            node_type: NodeType::Tool,
            label: None,
            attrs,
        };
        let mut graph = make_graph(vec![node]);

        apply_stylesheet(&mut graph, &ss);

        // Tool node doesn't match the codergen selector, so no changes.
        assert_eq!(graph.nodes[0].attrs.len(), 1);
        assert_eq!(
            graph.nodes[0].attrs.get("timeout"),
            Some(&NodeAttrValue::Number(60.0))
        );
    }

    // ---- Test 11: apply_transforms combines both transforms ----
    #[test]
    fn apply_transforms_combines_both() {
        let ss = Stylesheet::parse(r#"codergen { temperature: 0.7; }"#).unwrap();
        let mut graph = make_graph(vec![node_with_str_attrs(
            "gen1",
            Some("Generate {{task}}"),
            vec![("prompt", "Do {{task}} now")],
        )]);
        // Override the node type to Codergen so stylesheet matches.
        graph.nodes[0].node_type = NodeType::Codergen;

        let mut vars = HashMap::new();
        vars.insert("task".to_string(), "code review".to_string());

        apply_transforms(&mut graph, &vars, Some(&ss));

        // Variable expansion happened.
        assert_eq!(
            graph.nodes[0].attrs.get("prompt"),
            Some(&NodeAttrValue::String("Do code review now".to_string()))
        );
        assert_eq!(
            graph.nodes[0].label,
            Some("Generate code review".to_string())
        );
        // Stylesheet was applied.
        assert_eq!(
            graph.nodes[0].attrs.get("temperature"),
            Some(&NodeAttrValue::Number(0.7))
        );
    }

    // ---- Test 12: apply_transforms stylesheet-applied values get variable-expanded ----
    #[test]
    fn apply_transforms_stylesheet_values_get_expanded() {
        let ss =
            Stylesheet::parse(r#"codergen { system_prompt: "You are {{role}}"; }"#).unwrap();
        let node = simple_node("gen1", NodeType::Codergen);
        let mut graph = make_graph(vec![node]);

        let mut vars = HashMap::new();
        vars.insert("role".to_string(), "a senior engineer".to_string());

        apply_transforms(&mut graph, &vars, Some(&ss));

        // The stylesheet injected a string with a variable placeholder, and
        // variable expansion (which runs after stylesheet application) resolved it.
        assert_eq!(
            graph.nodes[0].attrs.get("system_prompt"),
            Some(&NodeAttrValue::String(
                "You are a senior engineer".to_string()
            ))
        );
    }

    // ---- Test 13: apply_transforms with no stylesheet ----
    #[test]
    fn apply_transforms_without_stylesheet() {
        let mut graph = make_graph(vec![node_with_str_attrs(
            "n1",
            None,
            vec![("msg", "Hello {{who}}")],
        )]);
        let mut vars = HashMap::new();
        vars.insert("who".to_string(), "world".to_string());

        apply_transforms(&mut graph, &vars, None);

        assert_eq!(
            graph.nodes[0].attrs.get("msg"),
            Some(&NodeAttrValue::String("Hello world".to_string()))
        );
    }

    // ---- Test 14: expand_string handles adjacent variables ----
    #[test]
    fn expand_string_adjacent_variables() {
        let mut vars = HashMap::new();
        vars.insert("a".to_string(), "X".to_string());
        vars.insert("b".to_string(), "Y".to_string());

        let result = expand_string("{{a}}{{b}}", &vars);
        assert_eq!(result, "XY");
    }

    // ---- Test 15: expand_string leaves single braces alone ----
    #[test]
    fn expand_string_single_braces_untouched() {
        let vars = HashMap::new();
        let result = expand_string("{not a variable}", &vars);
        assert_eq!(result, "{not a variable}");
    }

    // ---- Test 16: expand_variables across multiple nodes ----
    #[test]
    fn expand_variables_across_multiple_nodes() {
        let mut graph = make_graph(vec![
            node_with_str_attrs("n1", None, vec![("val", "{{x}}")]),
            node_with_str_attrs("n2", None, vec![("val", "{{y}}")]),
        ]);
        let mut vars = HashMap::new();
        vars.insert("x".to_string(), "alpha".to_string());
        vars.insert("y".to_string(), "beta".to_string());

        expand_variables(&mut graph, &vars);

        assert_eq!(
            graph.nodes[0].attrs.get("val"),
            Some(&NodeAttrValue::String("alpha".to_string()))
        );
        assert_eq!(
            graph.nodes[1].attrs.get("val"),
            Some(&NodeAttrValue::String("beta".to_string()))
        );
    }
}
