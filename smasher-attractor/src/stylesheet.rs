// ABOUTME: CSS-like stylesheet parser for pipeline model configuration.
// ABOUTME: Supports selector/declaration syntax for applying properties to graph nodes.

use std::collections::HashMap;
use std::time::Duration;

use crate::graph::{GraphNode, NodeAttrValue, NodeType};

/// Error types for stylesheet parsing and validation.
#[derive(Debug, thiserror::Error)]
pub enum StylesheetError {
    #[error("parse error at position {position}: {message}")]
    ParseError { position: usize, message: String },
    #[error("invalid selector: {selector}")]
    InvalidSelector { selector: String },
    #[error("invalid value: {value}")]
    InvalidValue { value: String },
}

/// A selector determines which graph nodes a rule applies to.
#[derive(Debug, Clone, PartialEq)]
pub enum Selector {
    /// Matches all nodes: `*`
    All,
    /// Matches by node type name: `codergen`, `tool`, etc.
    NodeType(String),
    /// Matches by node ID: `#node_id`
    Id(String),
    /// Matches by class (nodes with a `class` attribute containing this value): `.classname`
    Class(String),
}

/// A typed value in a stylesheet declaration.
#[derive(Debug, Clone, PartialEq)]
pub enum StyleValue {
    String(String),
    Number(f64),
    Duration(Duration),
    Bool(bool),
}

/// A single property assignment within a rule block.
#[derive(Debug, Clone, PartialEq)]
pub struct Declaration {
    pub property: String,
    pub value: StyleValue,
}

/// A rule pairs a selector with a list of declarations.
#[derive(Debug, Clone, PartialEq)]
pub struct Rule {
    pub selector: Selector,
    pub declarations: Vec<Declaration>,
}

/// A collection of rules parsed from stylesheet text.
#[derive(Debug, Clone)]
pub struct Stylesheet {
    pub rules: Vec<Rule>,
}

/// Maps a NodeType variant to its lowercase string name.
pub fn node_type_name(node_type: &NodeType) -> &str {
    match node_type {
        NodeType::Start => "start",
        NodeType::Exit => "exit",
        NodeType::Codergen => "codergen",
        NodeType::Conditional => "conditional",
        NodeType::Tool => "tool",
        NodeType::Interviewer => "interviewer",
        NodeType::Parallel => "parallel",
        NodeType::Manager => "manager",
        NodeType::SubPipeline => "subpipeline",
        NodeType::Generic => "generic",
    }
}

impl From<StyleValue> for NodeAttrValue {
    fn from(sv: StyleValue) -> Self {
        match sv {
            StyleValue::String(s) => NodeAttrValue::String(s),
            StyleValue::Number(n) => NodeAttrValue::Number(n),
            StyleValue::Duration(d) => NodeAttrValue::Duration(d),
            StyleValue::Bool(b) => NodeAttrValue::Bool(b),
        }
    }
}

/// Returns the specificity rank for a selector. Higher values are more specific.
fn specificity(selector: &Selector) -> u8 {
    match selector {
        Selector::All => 0,
        Selector::NodeType(_) => 1,
        Selector::Class(_) => 2,
        Selector::Id(_) => 3,
    }
}

/// Checks whether a selector matches a given graph node.
fn selector_matches(selector: &Selector, node: &GraphNode) -> bool {
    match selector {
        Selector::All => true,
        Selector::NodeType(name) => node_type_name(&node.node_type) == name,
        Selector::Id(id) => node.id == *id,
        Selector::Class(class) => {
            if let Some(NodeAttrValue::String(classes)) = node.attrs.get("class") {
                classes.split_whitespace().any(|c| c == class)
            } else {
                false
            }
        }
    }
}

impl Stylesheet {
    /// Parse stylesheet text into a Stylesheet.
    pub fn parse(input: &str) -> Result<Stylesheet, StylesheetError> {
        let mut rules = Vec::new();
        let chars: Vec<char> = input.chars().collect();
        let mut pos = 0;

        loop {
            skip_whitespace_and_comments(&chars, &mut pos);
            if pos >= chars.len() {
                break;
            }

            let selector = parse_selector(&chars, &mut pos)?;
            skip_whitespace_and_comments(&chars, &mut pos);

            if pos >= chars.len() || chars[pos] != '{' {
                return Err(StylesheetError::ParseError {
                    position: pos,
                    message: "expected '{'".to_string(),
                });
            }
            pos += 1; // consume '{'

            let declarations = parse_declarations(&chars, &mut pos)?;

            skip_whitespace_and_comments(&chars, &mut pos);
            if pos >= chars.len() || chars[pos] != '}' {
                return Err(StylesheetError::ParseError {
                    position: pos,
                    message: "expected '}'".to_string(),
                });
            }
            pos += 1; // consume '}'

            rules.push(Rule {
                selector,
                declarations,
            });
        }

        Ok(Stylesheet { rules })
    }

    /// Compute effective attributes for a node by matching rules in order.
    ///
    /// Rules are sorted by specificity (All < NodeType < Class < Id), and within the
    /// same specificity level, later rules override earlier ones. The result is a merged
    /// set of property values from all matching rules.
    pub fn apply(&self, node: &GraphNode) -> HashMap<String, NodeAttrValue> {
        let mut result = HashMap::new();

        // Collect matching rules with their original index for stable sort.
        let mut matches: Vec<(usize, &Rule)> = self
            .rules
            .iter()
            .enumerate()
            .filter(|(_, rule)| selector_matches(&rule.selector, node))
            .collect();

        // Sort by specificity first, then by original order (index) for tie-breaking.
        matches.sort_by_key(|(idx, rule)| (specificity(&rule.selector), *idx));

        for (_, rule) in matches {
            for decl in &rule.declarations {
                result.insert(decl.property.clone(), decl.value.clone().into());
            }
        }

        result
    }

    /// Return all rules whose selector matches the given node.
    pub fn matching_rules(&self, node: &GraphNode) -> Vec<&Rule> {
        self.rules
            .iter()
            .filter(|rule| selector_matches(&rule.selector, node))
            .collect()
    }
}

/// Skip whitespace characters and block comments (/* ... */).
fn skip_whitespace_and_comments(chars: &[char], pos: &mut usize) {
    loop {
        // Skip whitespace.
        while *pos < chars.len() && chars[*pos].is_whitespace() {
            *pos += 1;
        }

        // Skip block comment /* ... */.
        if *pos + 1 < chars.len() && chars[*pos] == '/' && chars[*pos + 1] == '*' {
            *pos += 2;
            while *pos + 1 < chars.len() && !(chars[*pos] == '*' && chars[*pos + 1] == '/') {
                *pos += 1;
            }
            if *pos + 1 < chars.len() {
                *pos += 2; // consume '*/'
            }
            continue;
        }

        break;
    }
}

/// Parse a selector from the character stream.
fn parse_selector(chars: &[char], pos: &mut usize) -> Result<Selector, StylesheetError> {
    skip_whitespace_and_comments(chars, pos);

    if *pos >= chars.len() {
        return Err(StylesheetError::ParseError {
            position: *pos,
            message: "unexpected end of input, expected selector".to_string(),
        });
    }

    let ch = chars[*pos];

    if ch == '*' {
        *pos += 1;
        return Ok(Selector::All);
    }

    if ch == '#' {
        *pos += 1;
        let name = parse_identifier(chars, pos)?;
        if name.is_empty() {
            return Err(StylesheetError::InvalidSelector {
                selector: "#".to_string(),
            });
        }
        return Ok(Selector::Id(name));
    }

    if ch == '.' {
        *pos += 1;
        let name = parse_identifier(chars, pos)?;
        if name.is_empty() {
            return Err(StylesheetError::InvalidSelector {
                selector: ".".to_string(),
            });
        }
        return Ok(Selector::Class(name));
    }

    // Must be a node type name.
    let name = parse_identifier(chars, pos)?;
    if name.is_empty() {
        return Err(StylesheetError::InvalidSelector {
            selector: format!("unexpected character '{ch}'"),
        });
    }

    Ok(Selector::NodeType(name))
}

/// Parse an identifier (alphanumeric + underscore + hyphen).
fn parse_identifier(chars: &[char], pos: &mut usize) -> Result<String, StylesheetError> {
    let start = *pos;
    while *pos < chars.len()
        && (chars[*pos].is_alphanumeric() || chars[*pos] == '_' || chars[*pos] == '-')
    {
        *pos += 1;
    }
    Ok(chars[start..*pos].iter().collect())
}

/// Parse declarations inside a rule block (between { and }).
fn parse_declarations(
    chars: &[char],
    pos: &mut usize,
) -> Result<Vec<Declaration>, StylesheetError> {
    let mut declarations = Vec::new();

    loop {
        skip_whitespace_and_comments(chars, pos);

        if *pos >= chars.len() || chars[*pos] == '}' {
            break;
        }

        let property = parse_identifier(chars, pos)?;
        if property.is_empty() {
            return Err(StylesheetError::ParseError {
                position: *pos,
                message: "expected property name".to_string(),
            });
        }

        skip_whitespace_and_comments(chars, pos);

        if *pos >= chars.len() || chars[*pos] != ':' {
            return Err(StylesheetError::ParseError {
                position: *pos,
                message: format!("expected ':' after property '{property}'"),
            });
        }
        *pos += 1; // consume ':'

        skip_whitespace_and_comments(chars, pos);

        let value = parse_value(chars, pos)?;

        skip_whitespace_and_comments(chars, pos);

        if *pos < chars.len() && chars[*pos] == ';' {
            *pos += 1; // consume ';'
        }

        declarations.push(Declaration { property, value });
    }

    Ok(declarations)
}

/// Parse a value from the character stream.
fn parse_value(chars: &[char], pos: &mut usize) -> Result<StyleValue, StylesheetError> {
    skip_whitespace_and_comments(chars, pos);

    if *pos >= chars.len() {
        return Err(StylesheetError::ParseError {
            position: *pos,
            message: "unexpected end of input, expected value".to_string(),
        });
    }

    // Quoted string: "..."
    if chars[*pos] == '"' {
        *pos += 1;
        let start = *pos;
        while *pos < chars.len() && chars[*pos] != '"' {
            *pos += 1;
        }
        if *pos >= chars.len() {
            return Err(StylesheetError::ParseError {
                position: start,
                message: "unterminated string literal".to_string(),
            });
        }
        let s: String = chars[start..*pos].iter().collect();
        *pos += 1; // consume closing '"'
        return Ok(StyleValue::String(s));
    }

    // Collect the raw token up to ';' or '}' or whitespace.
    let start = *pos;
    while *pos < chars.len()
        && chars[*pos] != ';'
        && chars[*pos] != '}'
        && !chars[*pos].is_whitespace()
    {
        *pos += 1;
    }

    let raw: String = chars[start..*pos].iter().collect();
    if raw.is_empty() {
        return Err(StylesheetError::ParseError {
            position: start,
            message: "expected value".to_string(),
        });
    }

    // Boolean.
    if raw == "true" {
        return Ok(StyleValue::Bool(true));
    }
    if raw == "false" {
        return Ok(StyleValue::Bool(false));
    }

    // Duration: ends with 's', 'm', or 'h'.
    if raw.len() > 1 {
        let last = raw.chars().last().unwrap();
        if last == 's' || last == 'm' || last == 'h' {
            let numeric_part = &raw[..raw.len() - 1];
            if let Ok(n) = numeric_part.parse::<f64>() {
                let duration = match last {
                    's' => Duration::from_secs_f64(n),
                    'm' => Duration::from_secs_f64(n * 60.0),
                    'h' => Duration::from_secs_f64(n * 3600.0),
                    _ => unreachable!(),
                };
                return Ok(StyleValue::Duration(duration));
            }
        }
    }

    // Number.
    if let Ok(n) = raw.parse::<f64>() {
        return Ok(StyleValue::Number(n));
    }

    Err(StylesheetError::InvalidValue { value: raw })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{GraphNode, NodeAttrValue, NodeType};
    use std::collections::HashMap;
    use std::time::Duration;

    /// Helper: build a GraphNode with the given fields.
    fn make_node(
        id: &str,
        node_type: NodeType,
        attrs: HashMap<String, NodeAttrValue>,
    ) -> GraphNode {
        GraphNode {
            id: id.to_string(),
            node_type,
            label: None,
            attrs,
        }
    }

    /// Helper: build a GraphNode with no extra attributes.
    fn simple_node(id: &str, node_type: NodeType) -> GraphNode {
        make_node(id, node_type, HashMap::new())
    }

    /// Helper: build a GraphNode with a class attribute.
    fn node_with_class(id: &str, node_type: NodeType, class: &str) -> GraphNode {
        let mut attrs = HashMap::new();
        attrs.insert(
            "class".to_string(),
            NodeAttrValue::String(class.to_string()),
        );
        make_node(id, node_type, attrs)
    }

    // ---- Test 1: Parse empty stylesheet ----
    #[test]
    fn parse_empty_stylesheet() {
        let ss = Stylesheet::parse("").unwrap();
        assert!(ss.rules.is_empty());
    }

    // ---- Test 2: Parse empty stylesheet with only whitespace ----
    #[test]
    fn parse_whitespace_only_stylesheet() {
        let ss = Stylesheet::parse("   \n\t  \n  ").unwrap();
        assert!(ss.rules.is_empty());
    }

    // ---- Test 3: Parse single rule with one declaration ----
    #[test]
    fn parse_single_rule_one_declaration() {
        let input = r#"codergen { model: "claude-sonnet-4-20250514"; }"#;
        let ss = Stylesheet::parse(input).unwrap();
        assert_eq!(ss.rules.len(), 1);
        assert_eq!(
            ss.rules[0].selector,
            Selector::NodeType("codergen".to_string())
        );
        assert_eq!(ss.rules[0].declarations.len(), 1);
        assert_eq!(ss.rules[0].declarations[0].property, "model");
        assert_eq!(
            ss.rules[0].declarations[0].value,
            StyleValue::String("claude-sonnet-4-20250514".to_string())
        );
    }

    // ---- Test 4: Parse multiple rules ----
    #[test]
    fn parse_multiple_rules() {
        let input = r#"
            codergen {
                model: "claude-sonnet-4-20250514";
                max_tokens: 4096;
            }
            tool {
                timeout: 30s;
            }
        "#;
        let ss = Stylesheet::parse(input).unwrap();
        assert_eq!(ss.rules.len(), 2);
        assert_eq!(
            ss.rules[0].selector,
            Selector::NodeType("codergen".to_string())
        );
        assert_eq!(ss.rules[0].declarations.len(), 2);
        assert_eq!(ss.rules[1].selector, Selector::NodeType("tool".to_string()));
        assert_eq!(ss.rules[1].declarations.len(), 1);
    }

    // ---- Test 5: Parse ID selector (#name) ----
    #[test]
    fn parse_id_selector() {
        let input = r#"#code_gen_1 { model: "claude-opus-4-20250514"; }"#;
        let ss = Stylesheet::parse(input).unwrap();
        assert_eq!(ss.rules.len(), 1);
        assert_eq!(ss.rules[0].selector, Selector::Id("code_gen_1".to_string()));
    }

    // ---- Test 6: Parse class selector (.name) ----
    #[test]
    fn parse_class_selector() {
        let input = ".critical { retries: 3; }";
        let ss = Stylesheet::parse(input).unwrap();
        assert_eq!(ss.rules.len(), 1);
        assert_eq!(
            ss.rules[0].selector,
            Selector::Class("critical".to_string())
        );
    }

    // ---- Test 7: Parse wildcard selector (*) ----
    #[test]
    fn parse_wildcard_selector() {
        let input = "* { temperature: 0.5; }";
        let ss = Stylesheet::parse(input).unwrap();
        assert_eq!(ss.rules.len(), 1);
        assert_eq!(ss.rules[0].selector, Selector::All);
    }

    // ---- Test 8: Parse node type selector ----
    #[test]
    fn parse_node_type_selector() {
        let input = "interviewer { greeting: \"hello\"; }";
        let ss = Stylesheet::parse(input).unwrap();
        assert_eq!(
            ss.rules[0].selector,
            Selector::NodeType("interviewer".to_string())
        );
    }

    // ---- Test 9: Parse string value (quoted) ----
    #[test]
    fn parse_string_value() {
        let input = r#"codergen { model: "gpt-4o"; }"#;
        let ss = Stylesheet::parse(input).unwrap();
        assert_eq!(
            ss.rules[0].declarations[0].value,
            StyleValue::String("gpt-4o".to_string())
        );
    }

    // ---- Test 10: Parse number value ----
    #[test]
    fn parse_number_value() {
        let input = "codergen { max_tokens: 4096; temperature: 0.7; }";
        let ss = Stylesheet::parse(input).unwrap();
        assert_eq!(
            ss.rules[0].declarations[0].value,
            StyleValue::Number(4096.0)
        );
        assert_eq!(ss.rules[0].declarations[1].value, StyleValue::Number(0.7));
    }

    // ---- Test 11: Parse duration values (seconds, minutes, hours) ----
    #[test]
    fn parse_duration_values() {
        let input = "tool { short: 30s; medium: 5m; long: 2h; }";
        let ss = Stylesheet::parse(input).unwrap();
        assert_eq!(
            ss.rules[0].declarations[0].value,
            StyleValue::Duration(Duration::from_secs(30))
        );
        assert_eq!(
            ss.rules[0].declarations[1].value,
            StyleValue::Duration(Duration::from_secs(300))
        );
        assert_eq!(
            ss.rules[0].declarations[2].value,
            StyleValue::Duration(Duration::from_secs(7200))
        );
    }

    // ---- Test 12: Parse boolean values ----
    #[test]
    fn parse_boolean_values() {
        let input = "codergen { streaming: true; cache: false; }";
        let ss = Stylesheet::parse(input).unwrap();
        assert_eq!(ss.rules[0].declarations[0].value, StyleValue::Bool(true));
        assert_eq!(ss.rules[0].declarations[1].value, StyleValue::Bool(false));
    }

    // ---- Test 13: Parse with comments ----
    #[test]
    fn parse_with_comments() {
        let input = r#"
            /* Global settings for all codergen nodes */
            codergen {
                model: "claude-sonnet-4-20250514"; /* default model */
                max_tokens: 4096;
            }
            /* Override for specific node */
            #special { temperature: 0.9; }
        "#;
        let ss = Stylesheet::parse(input).unwrap();
        assert_eq!(ss.rules.len(), 2);
        assert_eq!(
            ss.rules[0].selector,
            Selector::NodeType("codergen".to_string())
        );
        assert_eq!(ss.rules[0].declarations.len(), 2);
        assert_eq!(ss.rules[1].selector, Selector::Id("special".to_string()));
    }

    // ---- Test 14: Apply rule to matching node ----
    #[test]
    fn apply_rule_to_matching_node() {
        let input = r#"codergen { model: "claude-sonnet-4-20250514"; max_tokens: 4096; }"#;
        let ss = Stylesheet::parse(input).unwrap();

        let node = simple_node("gen1", NodeType::Codergen);
        let attrs = ss.apply(&node);

        assert_eq!(
            attrs.get("model"),
            Some(&NodeAttrValue::String(
                "claude-sonnet-4-20250514".to_string()
            ))
        );
        assert_eq!(
            attrs.get("max_tokens"),
            Some(&NodeAttrValue::Number(4096.0))
        );
    }

    // ---- Test 15: Apply rule to non-matching node ----
    #[test]
    fn apply_rule_to_non_matching_node() {
        let input = r#"codergen { model: "claude-sonnet-4-20250514"; }"#;
        let ss = Stylesheet::parse(input).unwrap();

        let node = simple_node("my_tool", NodeType::Tool);
        let attrs = ss.apply(&node);

        assert!(attrs.is_empty());
    }

    // ---- Test 16: Specificity ordering (Id overrides NodeType) ----
    #[test]
    fn specificity_id_overrides_node_type() {
        let input = r#"
            codergen {
                model: "claude-sonnet-4-20250514";
                temperature: 0.5;
            }
            #special_gen {
                model: "claude-opus-4-20250514";
            }
        "#;
        let ss = Stylesheet::parse(input).unwrap();

        let node = simple_node("special_gen", NodeType::Codergen);
        let attrs = ss.apply(&node);

        // Id selector should override NodeType selector for 'model'.
        assert_eq!(
            attrs.get("model"),
            Some(&NodeAttrValue::String("claude-opus-4-20250514".to_string()))
        );
        // temperature from NodeType rule still applies (not overridden).
        assert_eq!(attrs.get("temperature"), Some(&NodeAttrValue::Number(0.5)));
    }

    // ---- Test 17: Later rules override earlier (same specificity) ----
    #[test]
    fn later_rules_override_earlier_same_specificity() {
        let input = r#"
            codergen {
                model: "first-model";
            }
            codergen {
                model: "second-model";
            }
        "#;
        let ss = Stylesheet::parse(input).unwrap();

        let node = simple_node("gen1", NodeType::Codergen);
        let attrs = ss.apply(&node);

        assert_eq!(
            attrs.get("model"),
            Some(&NodeAttrValue::String("second-model".to_string()))
        );
    }

    // ---- Test 18: Class matching with space-separated classes ----
    #[test]
    fn class_matching_space_separated() {
        let input = ".critical { retries: 3; }";
        let ss = Stylesheet::parse(input).unwrap();

        // Node has multiple space-separated classes.
        let node = node_with_class("gen1", NodeType::Codergen, "fast critical production");
        let attrs = ss.apply(&node);
        assert_eq!(attrs.get("retries"), Some(&NodeAttrValue::Number(3.0)));

        // Node without the class should not match.
        let node2 = node_with_class("gen2", NodeType::Codergen, "fast production");
        let attrs2 = ss.apply(&node2);
        assert!(!attrs2.contains_key("retries"));
    }

    // ---- Test 19: Invalid input returns error ----
    #[test]
    fn invalid_input_returns_error() {
        // Missing opening brace.
        let result = Stylesheet::parse("codergen model: 42; }");
        assert!(result.is_err());

        // Unterminated string.
        let result2 = Stylesheet::parse(r#"codergen { model: "unterminated; }"#);
        assert!(result2.is_err());

        // Invalid value (not a string, number, duration, or bool).
        let result3 = Stylesheet::parse("codergen { model: @invalid; }");
        assert!(result3.is_err());
    }

    // ---- Test 20: Matching rules returns correct subset ----
    #[test]
    fn matching_rules_returns_correct_subset() {
        let input = r#"
            * { temperature: 0.5; }
            codergen { model: "claude-sonnet-4-20250514"; }
            tool { timeout: 60s; }
            #gen1 { max_tokens: 8192; }
        "#;
        let ss = Stylesheet::parse(input).unwrap();

        let node = simple_node("gen1", NodeType::Codergen);
        let matches = ss.matching_rules(&node);

        // Should match: * (All), codergen (NodeType), #gen1 (Id). NOT tool.
        assert_eq!(matches.len(), 3);
        assert_eq!(matches[0].selector, Selector::All);
        assert_eq!(
            matches[1].selector,
            Selector::NodeType("codergen".to_string())
        );
        assert_eq!(matches[2].selector, Selector::Id("gen1".to_string()));
    }

    // ---- Test 21: node_type_name maps all variants ----
    #[test]
    fn node_type_name_maps_all_variants() {
        assert_eq!(node_type_name(&NodeType::Start), "start");
        assert_eq!(node_type_name(&NodeType::Exit), "exit");
        assert_eq!(node_type_name(&NodeType::Codergen), "codergen");
        assert_eq!(node_type_name(&NodeType::Conditional), "conditional");
        assert_eq!(node_type_name(&NodeType::Tool), "tool");
        assert_eq!(node_type_name(&NodeType::Interviewer), "interviewer");
        assert_eq!(node_type_name(&NodeType::Parallel), "parallel");
        assert_eq!(node_type_name(&NodeType::Manager), "manager");
        assert_eq!(node_type_name(&NodeType::Generic), "generic");
    }

    // ---- Test 22: StyleValue to NodeAttrValue conversion ----
    #[test]
    fn style_value_to_node_attr_value_conversion() {
        let sv_string = StyleValue::String("hello".to_string());
        assert_eq!(
            NodeAttrValue::from(sv_string),
            NodeAttrValue::String("hello".to_string())
        );

        let sv_number = StyleValue::Number(42.0);
        assert_eq!(NodeAttrValue::from(sv_number), NodeAttrValue::Number(42.0));

        let sv_dur = StyleValue::Duration(Duration::from_secs(60));
        assert_eq!(
            NodeAttrValue::from(sv_dur),
            NodeAttrValue::Duration(Duration::from_secs(60))
        );

        let sv_bool = StyleValue::Bool(true);
        assert_eq!(NodeAttrValue::from(sv_bool), NodeAttrValue::Bool(true));
    }

    // ---- Test 23: Wildcard matches all node types ----
    #[test]
    fn wildcard_matches_all_node_types() {
        let input = "* { temperature: 0.5; }";
        let ss = Stylesheet::parse(input).unwrap();

        let types = [
            NodeType::Start,
            NodeType::Exit,
            NodeType::Codergen,
            NodeType::Conditional,
            NodeType::Tool,
            NodeType::Interviewer,
            NodeType::Parallel,
            NodeType::Manager,
            NodeType::Generic,
        ];

        for nt in &types {
            let node = simple_node("any", nt.clone());
            let attrs = ss.apply(&node);
            assert_eq!(
                attrs.get("temperature"),
                Some(&NodeAttrValue::Number(0.5)),
                "wildcard should match {:?}",
                nt
            );
        }
    }

    // ---- Test 24: Full specificity cascade ----
    #[test]
    fn full_specificity_cascade() {
        let input = r#"
            * {
                model: "base";
                temperature: 0.1;
                retries: 1;
                timeout: 10s;
            }
            codergen {
                model: "type-level";
                temperature: 0.5;
                retries: 2;
            }
            .critical {
                model: "class-level";
                temperature: 0.8;
            }
            #node_42 {
                model: "id-level";
            }
        "#;
        let ss = Stylesheet::parse(input).unwrap();

        let mut attrs = HashMap::new();
        attrs.insert(
            "class".to_string(),
            NodeAttrValue::String("critical".to_string()),
        );
        let node = make_node("node_42", NodeType::Codergen, attrs);
        let result = ss.apply(&node);

        // model: Id wins.
        assert_eq!(
            result.get("model"),
            Some(&NodeAttrValue::String("id-level".to_string()))
        );
        // temperature: Class wins over NodeType and All.
        assert_eq!(result.get("temperature"), Some(&NodeAttrValue::Number(0.8)));
        // retries: NodeType wins over All.
        assert_eq!(result.get("retries"), Some(&NodeAttrValue::Number(2.0)));
        // timeout: Only set by All.
        assert_eq!(
            result.get("timeout"),
            Some(&NodeAttrValue::Duration(Duration::from_secs(10)))
        );
    }

    // ---- Test 25: Class selector does not match node without class attr ----
    #[test]
    fn class_selector_no_match_without_class_attr() {
        let input = ".fast { speed: 100; }";
        let ss = Stylesheet::parse(input).unwrap();

        let node = simple_node("n1", NodeType::Generic);
        let attrs = ss.apply(&node);
        assert!(attrs.is_empty());
    }

    // ---- Test 26: Declaration without trailing semicolon ----
    #[test]
    fn declaration_without_trailing_semicolon() {
        let input = "codergen { temperature: 0.7 }";
        let ss = Stylesheet::parse(input).unwrap();
        assert_eq!(ss.rules[0].declarations.len(), 1);
        assert_eq!(ss.rules[0].declarations[0].value, StyleValue::Number(0.7));
    }

    // ---- Test 27: Missing closing brace returns error ----
    #[test]
    fn missing_closing_brace_returns_error() {
        let result = Stylesheet::parse("codergen { model: 42;");
        assert!(result.is_err());
    }

    // ---- Test 28: Id selector with hyphenated name ----
    #[test]
    fn id_selector_with_hyphenated_name() {
        let input = "#my-node-1 { retries: 5; }";
        let ss = Stylesheet::parse(input).unwrap();
        assert_eq!(ss.rules[0].selector, Selector::Id("my-node-1".to_string()));
    }
}
