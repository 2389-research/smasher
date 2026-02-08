// ABOUTME: Condition expression parser and evaluator for pipeline edge transitions.
// ABOUTME: Supports comparisons, boolean logic, and evaluation against key-value contexts.

use std::collections::HashMap;

/// A parsed condition expression tree.
#[derive(Debug, Clone, PartialEq)]
pub enum Condition {
    /// Always true.
    True,
    /// Always false.
    False,
    /// key == value comparison.
    Equals { key: String, value: String },
    /// key != value comparison.
    NotEquals { key: String, value: String },
    /// key > value (numeric comparison).
    GreaterThan { key: String, value: f64 },
    /// key < value (numeric comparison).
    LessThan { key: String, value: f64 },
    /// Logical AND.
    And(Box<Condition>, Box<Condition>),
    /// Logical OR.
    Or(Box<Condition>, Box<Condition>),
    /// Logical NOT.
    Not(Box<Condition>),
}

/// Errors that can occur during condition parsing.
#[derive(Debug, thiserror::Error)]
pub enum ConditionError {
    #[error("invalid condition expression: {message}")]
    ParseError { message: String },
    #[error("unexpected token: {token}")]
    UnexpectedToken { token: String },
    #[error("empty condition")]
    Empty,
}

/// Parse a condition expression string into a Condition tree.
///
/// Operator precedence (lowest to highest):
/// 1. `||` (logical OR)
/// 2. `&&` (logical AND)
/// 3. `!` (logical NOT, prefix)
/// 4. Parenthesized expressions and atoms (comparisons, literals)
pub fn parse_condition(input: &str) -> Result<Condition, ConditionError> {
    let input = input.trim();
    if input.is_empty() {
        return Err(ConditionError::Empty);
    }
    parse_or(input)
}

/// Parse an OR expression: expr || expr
fn parse_or(input: &str) -> Result<Condition, ConditionError> {
    if let Some((left, right)) = split_binary_op(input, "||") {
        let left_cond = parse_or(left)?;
        let right_cond = parse_or(right)?;
        Ok(Condition::Or(Box::new(left_cond), Box::new(right_cond)))
    } else {
        parse_and(input)
    }
}

/// Parse an AND expression: expr && expr
fn parse_and(input: &str) -> Result<Condition, ConditionError> {
    if let Some((left, right)) = split_binary_op(input, "&&") {
        let left_cond = parse_and(left)?;
        let right_cond = parse_and(right)?;
        Ok(Condition::And(Box::new(left_cond), Box::new(right_cond)))
    } else {
        parse_not(input)
    }
}

/// Parse a NOT expression: !expr
fn parse_not(input: &str) -> Result<Condition, ConditionError> {
    let input = input.trim();
    if let Some(rest) = input.strip_prefix('!') {
        let inner = parse_not(rest)?;
        Ok(Condition::Not(Box::new(inner)))
    } else {
        parse_atom(input)
    }
}

/// Parse an atom: parenthesized expression, literal, or comparison.
fn parse_atom(input: &str) -> Result<Condition, ConditionError> {
    let input = input.trim();
    if input.is_empty() {
        return Err(ConditionError::ParseError {
            message: "unexpected end of expression".to_string(),
        });
    }

    // Parenthesized expression
    if input.starts_with('(') {
        let inner = strip_outer_parens(input).ok_or_else(|| ConditionError::ParseError {
            message: "unmatched parenthesis".to_string(),
        })?;
        return parse_or(inner);
    }

    // Boolean literals
    if input == "true" {
        return Ok(Condition::True);
    }
    if input == "false" {
        return Ok(Condition::False);
    }

    // Comparisons: != must be checked before = to avoid ambiguity
    if let Some(pos) = input.find("!=") {
        let key = input[..pos].trim().to_string();
        let value = input[pos + 2..].trim().to_string();
        if key.is_empty() || value.is_empty() {
            return Err(ConditionError::ParseError {
                message: format!("invalid comparison: '{input}'"),
            });
        }
        return Ok(Condition::NotEquals { key, value });
    }

    if let Some(pos) = input.find('>') {
        let key = input[..pos].trim().to_string();
        let val_str = input[pos + 1..].trim();
        if key.is_empty() || val_str.is_empty() {
            return Err(ConditionError::ParseError {
                message: format!("invalid comparison: '{input}'"),
            });
        }
        let value = val_str.parse::<f64>().map_err(|_| ConditionError::ParseError {
            message: format!("expected numeric value in '>' comparison, got '{val_str}'"),
        })?;
        return Ok(Condition::GreaterThan { key, value });
    }

    if let Some(pos) = input.find('<') {
        let key = input[..pos].trim().to_string();
        let val_str = input[pos + 1..].trim();
        if key.is_empty() || val_str.is_empty() {
            return Err(ConditionError::ParseError {
                message: format!("invalid comparison: '{input}'"),
            });
        }
        let value = val_str.parse::<f64>().map_err(|_| ConditionError::ParseError {
            message: format!("expected numeric value in '<' comparison, got '{val_str}'"),
        })?;
        return Ok(Condition::LessThan { key, value });
    }

    if let Some(pos) = input.find('=') {
        let key = input[..pos].trim().to_string();
        let value = input[pos + 1..].trim().to_string();
        if key.is_empty() || value.is_empty() {
            return Err(ConditionError::ParseError {
                message: format!("invalid comparison: '{input}'"),
            });
        }
        return Ok(Condition::Equals { key, value });
    }

    Err(ConditionError::UnexpectedToken {
        token: input.to_string(),
    })
}

/// Split an expression on a binary operator, respecting parenthesization.
///
/// Finds the *last* occurrence of `op` at parenthesis depth 0 so that the
/// split produces left-associative grouping. Returns the left and right
/// substrings (trimmed), or None if the operator is not found at depth 0.
fn split_binary_op<'a>(input: &'a str, op: &str) -> Option<(&'a str, &'a str)> {
    let op_bytes = op.as_bytes();
    let op_len = op_bytes.len();
    if input.len() < op_len {
        return None;
    }

    let mut depth: i32 = 0;
    let mut last_pos: Option<usize> = None;
    let bytes = input.as_bytes();

    for i in 0..bytes.len() {
        match bytes[i] {
            b'(' => depth += 1,
            b')' => depth -= 1,
            _ => {}
        }
        if depth == 0 && i + op_len <= bytes.len() && &bytes[i..i + op_len] == op_bytes {
            last_pos = Some(i);
        }
    }

    if let Some(pos) = last_pos {
        let left = input[..pos].trim();
        let right = input[pos + op_len..].trim();
        if !left.is_empty() && !right.is_empty() {
            return Some((left, right));
        }
    }

    None
}

/// Strip balanced outer parentheses from a string.
///
/// Returns the inner content if the string starts with '(' and ends with a matching ')'.
fn strip_outer_parens(input: &str) -> Option<&str> {
    let input = input.trim();
    if !input.starts_with('(') {
        return None;
    }

    let mut depth = 0;
    for (i, ch) in input.char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    if i == input.len() - 1 {
                        return Some(&input[1..i]);
                    } else {
                        // Closing paren is not at end, so outer parens don't wrap the whole expr
                        return None;
                    }
                }
            }
            _ => {}
        }
    }

    None
}

/// Evaluate a condition against a key-value context.
///
/// Missing keys evaluate to false for comparisons. Numeric comparisons
/// parse context values as f64; if parsing fails, the comparison returns false.
pub fn evaluate_condition(condition: &Condition, context: &HashMap<String, String>) -> bool {
    match condition {
        Condition::True => true,
        Condition::False => false,
        Condition::Equals { key, value } => context.get(key) == Some(value),
        Condition::NotEquals { key, value } => {
            context.get(key).is_some_and(|v| v != value)
        }
        Condition::GreaterThan { key, value } => context
            .get(key)
            .and_then(|v| v.parse::<f64>().ok())
            .is_some_and(|v| v > *value),
        Condition::LessThan { key, value } => context
            .get(key)
            .and_then(|v| v.parse::<f64>().ok())
            .is_some_and(|v| v < *value),
        Condition::And(a, b) => {
            evaluate_condition(a, context) && evaluate_condition(b, context)
        }
        Condition::Or(a, b) => {
            evaluate_condition(a, context) || evaluate_condition(b, context)
        }
        Condition::Not(inner) => !evaluate_condition(inner, context),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_true_literal() {
        let cond = parse_condition("true").unwrap();
        assert_eq!(cond, Condition::True);
    }

    #[test]
    fn parse_false_literal() {
        let cond = parse_condition("false").unwrap();
        assert_eq!(cond, Condition::False);
    }

    #[test]
    fn parse_equals() {
        let cond = parse_condition("status=done").unwrap();
        assert_eq!(
            cond,
            Condition::Equals {
                key: "status".to_string(),
                value: "done".to_string(),
            }
        );
    }

    #[test]
    fn parse_not_equals() {
        let cond = parse_condition("status!=failed").unwrap();
        assert_eq!(
            cond,
            Condition::NotEquals {
                key: "status".to_string(),
                value: "failed".to_string(),
            }
        );
    }

    #[test]
    fn parse_greater_than() {
        let cond = parse_condition("score>0.5").unwrap();
        assert_eq!(
            cond,
            Condition::GreaterThan {
                key: "score".to_string(),
                value: 0.5,
            }
        );
    }

    #[test]
    fn parse_less_than() {
        let cond = parse_condition("count<10").unwrap();
        assert_eq!(
            cond,
            Condition::LessThan {
                key: "count".to_string(),
                value: 10.0,
            }
        );
    }

    #[test]
    fn parse_and() {
        let cond = parse_condition("a=b && c=d").unwrap();
        assert_eq!(
            cond,
            Condition::And(
                Box::new(Condition::Equals {
                    key: "a".to_string(),
                    value: "b".to_string(),
                }),
                Box::new(Condition::Equals {
                    key: "c".to_string(),
                    value: "d".to_string(),
                }),
            )
        );
    }

    #[test]
    fn parse_or() {
        let cond = parse_condition("a=b || c=d").unwrap();
        assert_eq!(
            cond,
            Condition::Or(
                Box::new(Condition::Equals {
                    key: "a".to_string(),
                    value: "b".to_string(),
                }),
                Box::new(Condition::Equals {
                    key: "c".to_string(),
                    value: "d".to_string(),
                }),
            )
        );
    }

    #[test]
    fn parse_not() {
        let cond = parse_condition("!status=done").unwrap();
        assert_eq!(
            cond,
            Condition::Not(Box::new(Condition::Equals {
                key: "status".to_string(),
                value: "done".to_string(),
            }))
        );
    }

    #[test]
    fn parse_parenthesized() {
        let cond = parse_condition("(a=b)").unwrap();
        assert_eq!(
            cond,
            Condition::Equals {
                key: "a".to_string(),
                value: "b".to_string(),
            }
        );
    }

    #[test]
    fn evaluate_equals_matching() {
        let cond = Condition::Equals {
            key: "status".to_string(),
            value: "done".to_string(),
        };
        let mut ctx = HashMap::new();
        ctx.insert("status".to_string(), "done".to_string());
        assert!(evaluate_condition(&cond, &ctx));
    }

    #[test]
    fn evaluate_equals_not_matching() {
        let cond = Condition::Equals {
            key: "status".to_string(),
            value: "done".to_string(),
        };
        let mut ctx = HashMap::new();
        ctx.insert("status".to_string(), "pending".to_string());
        assert!(!evaluate_condition(&cond, &ctx));
    }

    #[test]
    fn evaluate_equals_missing_key() {
        let cond = Condition::Equals {
            key: "status".to_string(),
            value: "done".to_string(),
        };
        let ctx = HashMap::new();
        assert!(!evaluate_condition(&cond, &ctx));
    }

    #[test]
    fn evaluate_and_both_true() {
        let cond = Condition::And(
            Box::new(Condition::Equals {
                key: "a".to_string(),
                value: "1".to_string(),
            }),
            Box::new(Condition::Equals {
                key: "b".to_string(),
                value: "2".to_string(),
            }),
        );
        let mut ctx = HashMap::new();
        ctx.insert("a".to_string(), "1".to_string());
        ctx.insert("b".to_string(), "2".to_string());
        assert!(evaluate_condition(&cond, &ctx));
    }

    #[test]
    fn evaluate_and_one_false() {
        let cond = Condition::And(
            Box::new(Condition::Equals {
                key: "a".to_string(),
                value: "1".to_string(),
            }),
            Box::new(Condition::Equals {
                key: "b".to_string(),
                value: "2".to_string(),
            }),
        );
        let mut ctx = HashMap::new();
        ctx.insert("a".to_string(), "1".to_string());
        ctx.insert("b".to_string(), "wrong".to_string());
        assert!(!evaluate_condition(&cond, &ctx));
    }

    #[test]
    fn evaluate_or_one_true() {
        let cond = Condition::Or(
            Box::new(Condition::False),
            Box::new(Condition::True),
        );
        let ctx = HashMap::new();
        assert!(evaluate_condition(&cond, &ctx));
    }

    #[test]
    fn evaluate_not_inverts() {
        let cond = Condition::Not(Box::new(Condition::True));
        let ctx = HashMap::new();
        assert!(!evaluate_condition(&cond, &ctx));

        let cond2 = Condition::Not(Box::new(Condition::False));
        assert!(evaluate_condition(&cond2, &ctx));
    }

    #[test]
    fn evaluate_greater_than_numeric() {
        let cond = Condition::GreaterThan {
            key: "score".to_string(),
            value: 0.5,
        };
        let mut ctx = HashMap::new();
        ctx.insert("score".to_string(), "0.8".to_string());
        assert!(evaluate_condition(&cond, &ctx));

        ctx.insert("score".to_string(), "0.3".to_string());
        assert!(!evaluate_condition(&cond, &ctx));
    }

    #[test]
    fn complex_expression() {
        // "status=ok && (score>0.5 || priority=high)"
        let cond = parse_condition("status=ok && (score>0.5 || priority=high)").unwrap();

        // Both status=ok and score>0.5 are true
        let mut ctx = HashMap::new();
        ctx.insert("status".to_string(), "ok".to_string());
        ctx.insert("score".to_string(), "0.8".to_string());
        ctx.insert("priority".to_string(), "low".to_string());
        assert!(evaluate_condition(&cond, &ctx));

        // status=ok and priority=high (score is low)
        ctx.insert("score".to_string(), "0.1".to_string());
        ctx.insert("priority".to_string(), "high".to_string());
        assert!(evaluate_condition(&cond, &ctx));

        // status is wrong, everything else true
        ctx.insert("status".to_string(), "error".to_string());
        assert!(!evaluate_condition(&cond, &ctx));
    }

    #[test]
    fn empty_input_returns_error() {
        let result = parse_condition("");
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), ConditionError::Empty));
    }

    #[test]
    fn whitespace_only_returns_error() {
        let result = parse_condition("   ");
        assert!(result.is_err());
    }
}
