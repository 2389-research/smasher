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

/// Errors that can occur during condition parsing or evaluation.
#[derive(Debug, thiserror::Error)]
pub enum ConditionError {
    #[error("parse error at position {position}: {message}")]
    ParseError { message: String, position: usize },
    #[error("unexpected token at position {position}: '{token}'")]
    UnexpectedToken { token: String, position: usize },
    #[error("empty condition expression")]
    Empty,
    #[error(
        "undefined variable '{name}' referenced in condition; provide a value for '{name}' in the context"
    )]
    UndefinedVariable { name: String },
}

/// Parse a condition expression string into a Condition tree.
///
/// Operator precedence (lowest to highest):
/// 1. `||` (logical OR)
/// 2. `&&` (logical AND)
/// 3. `!` (logical NOT, prefix)
/// 4. Parenthesized expressions and atoms (comparisons, literals)
pub fn parse_condition(input: &str) -> Result<Condition, ConditionError> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(ConditionError::Empty);
    }
    // Compute the byte offset of the trimmed content within the original input.
    let base_offset = input.len() - input.trim_start().len();
    parse_or(trimmed, base_offset)
}

/// Validate a condition expression without evaluating it.
///
/// Parses the expression and returns `Ok(())` if the syntax is valid,
/// or the first `ConditionError` encountered during parsing. This is
/// useful for lint/validation use cases where you want to check syntax
/// without providing a context for evaluation.
pub fn validate(expr: &str) -> Result<(), ConditionError> {
    parse_condition(expr).map(|_| ())
}

/// Evaluate a condition strictly, returning an error for undefined variables.
///
/// Unlike `evaluate_condition` which silently treats missing keys as false,
/// this function returns `ConditionError::UndefinedVariable` when a comparison
/// references a key that is not present in the context. Use this when you want
/// to enforce that all referenced variables are explicitly provided.
pub fn evaluate_condition_strict(
    condition: &Condition,
    context: &HashMap<String, String>,
) -> Result<bool, ConditionError> {
    match condition {
        Condition::True => Ok(true),
        Condition::False => Ok(false),
        Condition::Equals { key, value } => {
            let ctx_val = context
                .get(key)
                .ok_or_else(|| ConditionError::UndefinedVariable { name: key.clone() })?;
            Ok(ctx_val == value)
        }
        Condition::NotEquals { key, value } => {
            let ctx_val = context
                .get(key)
                .ok_or_else(|| ConditionError::UndefinedVariable { name: key.clone() })?;
            Ok(ctx_val != value)
        }
        Condition::GreaterThan { key, value } => {
            let ctx_val = context
                .get(key)
                .ok_or_else(|| ConditionError::UndefinedVariable { name: key.clone() })?;
            let numeric = ctx_val
                .parse::<f64>()
                .map_err(|_| ConditionError::ParseError {
                    message: format!(
                        "variable '{key}' has value '{ctx_val}' which is not a valid number"
                    ),
                    position: 0,
                })?;
            Ok(numeric > *value)
        }
        Condition::LessThan { key, value } => {
            let ctx_val = context
                .get(key)
                .ok_or_else(|| ConditionError::UndefinedVariable { name: key.clone() })?;
            let numeric = ctx_val
                .parse::<f64>()
                .map_err(|_| ConditionError::ParseError {
                    message: format!(
                        "variable '{key}' has value '{ctx_val}' which is not a valid number"
                    ),
                    position: 0,
                })?;
            Ok(numeric < *value)
        }
        Condition::And(a, b) => {
            let left = evaluate_condition_strict(a, context)?;
            let right = evaluate_condition_strict(b, context)?;
            Ok(left && right)
        }
        Condition::Or(a, b) => {
            let left = evaluate_condition_strict(a, context)?;
            let right = evaluate_condition_strict(b, context)?;
            Ok(left || right)
        }
        Condition::Not(inner) => {
            let val = evaluate_condition_strict(inner, context)?;
            Ok(!val)
        }
    }
}

/// Parse an OR expression: expr || expr
fn parse_or(input: &str, offset: usize) -> Result<Condition, ConditionError> {
    if let Some((left, right, right_offset)) = split_binary_op(input, "||", offset) {
        let left_cond = parse_or(left, offset)?;
        let right_cond = parse_or(right, right_offset)?;
        Ok(Condition::Or(Box::new(left_cond), Box::new(right_cond)))
    } else {
        parse_and(input, offset)
    }
}

/// Parse an AND expression: expr && expr
fn parse_and(input: &str, offset: usize) -> Result<Condition, ConditionError> {
    if let Some((left, right, right_offset)) = split_binary_op(input, "&&", offset) {
        let left_cond = parse_and(left, offset)?;
        let right_cond = parse_and(right, right_offset)?;
        Ok(Condition::And(Box::new(left_cond), Box::new(right_cond)))
    } else {
        parse_not(input, offset)
    }
}

/// Parse a NOT expression: !expr
fn parse_not(input: &str, offset: usize) -> Result<Condition, ConditionError> {
    let trimmed = input.trim();
    let trim_delta = input.len() - input.trim_start().len();
    let adj_offset = offset + trim_delta;
    if let Some(rest) = trimmed.strip_prefix('!') {
        let inner = parse_not(rest, adj_offset + 1)?;
        Ok(Condition::Not(Box::new(inner)))
    } else {
        parse_atom(trimmed, adj_offset)
    }
}

/// Parse an atom: parenthesized expression, literal, or comparison.
fn parse_atom(input: &str, offset: usize) -> Result<Condition, ConditionError> {
    let trimmed = input.trim();
    let trim_delta = input.len() - input.trim_start().len();
    let adj_offset = offset + trim_delta;
    if trimmed.is_empty() {
        return Err(ConditionError::ParseError {
            message: "unexpected end of expression".to_string(),
            position: adj_offset,
        });
    }

    // Parenthesized expression
    if trimmed.starts_with('(') {
        let inner = strip_outer_parens(trimmed).ok_or_else(|| ConditionError::ParseError {
            message: "unmatched parenthesis".to_string(),
            position: adj_offset,
        })?;
        return parse_or(inner, adj_offset + 1);
    }

    // Boolean literals
    if trimmed == "true" {
        return Ok(Condition::True);
    }
    if trimmed == "false" {
        return Ok(Condition::False);
    }

    // Comparisons: != must be checked before = to avoid ambiguity
    if let Some(pos) = trimmed.find("!=") {
        let key = trimmed[..pos].trim().to_string();
        let value = trimmed[pos + 2..].trim().to_string();
        if key.is_empty() || value.is_empty() {
            return Err(ConditionError::ParseError {
                message: format!("invalid comparison: '{trimmed}'"),
                position: adj_offset + pos,
            });
        }
        return Ok(Condition::NotEquals { key, value });
    }

    if let Some(pos) = trimmed.find('>') {
        let key = trimmed[..pos].trim().to_string();
        let val_str = trimmed[pos + 1..].trim();
        if key.is_empty() || val_str.is_empty() {
            return Err(ConditionError::ParseError {
                message: format!("invalid comparison: '{trimmed}'"),
                position: adj_offset + pos,
            });
        }
        let value = val_str
            .parse::<f64>()
            .map_err(|_| ConditionError::ParseError {
                message: format!("expected numeric value in '>' comparison, got '{val_str}'"),
                position: adj_offset + pos + 1,
            })?;
        return Ok(Condition::GreaterThan { key, value });
    }

    if let Some(pos) = trimmed.find('<') {
        let key = trimmed[..pos].trim().to_string();
        let val_str = trimmed[pos + 1..].trim();
        if key.is_empty() || val_str.is_empty() {
            return Err(ConditionError::ParseError {
                message: format!("invalid comparison: '{trimmed}'"),
                position: adj_offset + pos,
            });
        }
        let value = val_str
            .parse::<f64>()
            .map_err(|_| ConditionError::ParseError {
                message: format!("expected numeric value in '<' comparison, got '{val_str}'"),
                position: adj_offset + pos + 1,
            })?;
        return Ok(Condition::LessThan { key, value });
    }

    if let Some(pos) = trimmed.find('=') {
        let key = trimmed[..pos].trim().to_string();
        let value = trimmed[pos + 1..].trim().to_string();
        if key.is_empty() || value.is_empty() {
            return Err(ConditionError::ParseError {
                message: format!("invalid comparison: '{trimmed}'"),
                position: adj_offset + pos,
            });
        }
        return Ok(Condition::Equals { key, value });
    }

    Err(ConditionError::UnexpectedToken {
        token: trimmed.to_string(),
        position: adj_offset,
    })
}

/// Split an expression on a binary operator, respecting parenthesization.
///
/// Finds the *last* occurrence of `op` at parenthesis depth 0 so that the
/// split produces left-associative grouping. Returns the left and right
/// substrings (trimmed) along with the byte offset of the right substring
/// relative to the original input, or None if the operator is not found
/// at depth 0.
fn split_binary_op<'a>(
    input: &'a str,
    op: &str,
    base_offset: usize,
) -> Option<(&'a str, &'a str, usize)> {
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
        let right_raw = &input[pos + op_len..];
        let right = right_raw.trim();
        if !left.is_empty() && !right.is_empty() {
            // Compute the byte offset of right within the original input.
            let right_trim_delta = right_raw.len() - right_raw.trim_start().len();
            let right_offset = base_offset + pos + op_len + right_trim_delta;
            return Some((left, right, right_offset));
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
        Condition::NotEquals { key, value } => context.get(key).is_some_and(|v| v != value),
        Condition::GreaterThan { key, value } => context
            .get(key)
            .and_then(|v| v.parse::<f64>().ok())
            .is_some_and(|v| v > *value),
        Condition::LessThan { key, value } => context
            .get(key)
            .and_then(|v| v.parse::<f64>().ok())
            .is_some_and(|v| v < *value),
        Condition::And(a, b) => evaluate_condition(a, context) && evaluate_condition(b, context),
        Condition::Or(a, b) => evaluate_condition(a, context) || evaluate_condition(b, context),
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
        let cond = Condition::Or(Box::new(Condition::False), Box::new(Condition::True));
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

    // ---------------------------------------------------------------
    // Parse error position tests
    // ---------------------------------------------------------------

    #[test]
    fn parse_error_includes_position_for_unexpected_token() {
        let result = parse_condition("blarg");
        assert!(result.is_err());
        let err = result.unwrap_err();
        match err {
            ConditionError::UnexpectedToken {
                ref token,
                position,
            } => {
                assert_eq!(token, "blarg");
                assert_eq!(position, 0);
            }
            ref other => panic!("expected UnexpectedToken, got {other:?}"),
        }
        // Display message should contain the position
        let msg = err.to_string();
        assert!(msg.contains("position 0"), "message was: {msg}");
        assert!(msg.contains("blarg"), "message was: {msg}");
    }

    #[test]
    fn parse_error_includes_position_for_unmatched_paren() {
        let result = parse_condition("(a=b");
        assert!(result.is_err());
        let err = result.unwrap_err();
        match err {
            ConditionError::ParseError {
                ref message,
                position,
            } => {
                assert!(
                    message.contains("unmatched parenthesis"),
                    "message was: {message}"
                );
                assert_eq!(position, 0);
            }
            ref other => panic!("expected ParseError, got {other:?}"),
        }
    }

    #[test]
    fn parse_error_position_accounts_for_leading_whitespace() {
        // "  blarg" -> token starts at position 2
        let result = parse_condition("  blarg");
        assert!(result.is_err());
        let err = result.unwrap_err();
        match err {
            ConditionError::UnexpectedToken {
                ref token,
                position,
            } => {
                assert_eq!(token, "blarg");
                assert_eq!(position, 2);
            }
            ref other => panic!("expected UnexpectedToken, got {other:?}"),
        }
    }

    #[test]
    fn parse_error_position_for_bad_numeric_value() {
        // "score>abc" should error at the numeric part
        let result = parse_condition("score>abc");
        assert!(result.is_err());
        let err = result.unwrap_err();
        match err {
            ConditionError::ParseError {
                ref message,
                position,
            } => {
                assert!(
                    message.contains("expected numeric value"),
                    "message was: {message}"
                );
                // position should point to the character after '>'
                assert_eq!(position, 6);
            }
            ref other => panic!("expected ParseError, got {other:?}"),
        }
    }

    #[test]
    fn parse_error_position_in_right_side_of_and() {
        // "a=b && blarg" -> the error is in "blarg" which starts at position 7
        let result = parse_condition("a=b && blarg");
        assert!(result.is_err());
        let err = result.unwrap_err();
        match err {
            ConditionError::UnexpectedToken {
                ref token,
                position,
            } => {
                assert_eq!(token, "blarg");
                assert_eq!(position, 7);
            }
            ref other => panic!("expected UnexpectedToken, got {other:?}"),
        }
    }

    // ---------------------------------------------------------------
    // Undefined variable tests (evaluate_condition_strict)
    // ---------------------------------------------------------------

    #[test]
    fn strict_eval_returns_undefined_variable_for_missing_key() {
        let cond = parse_condition("status=done").unwrap();
        let ctx = HashMap::new();
        let result = evaluate_condition_strict(&cond, &ctx);
        assert!(result.is_err());
        let err = result.unwrap_err();
        match err {
            ConditionError::UndefinedVariable { ref name } => {
                assert_eq!(name, "status");
            }
            ref other => panic!("expected UndefinedVariable, got {other:?}"),
        }
        // Check the Display message is actionable
        let msg = err.to_string();
        assert!(msg.contains("status"), "message was: {msg}");
        assert!(msg.contains("provide a value"), "message was: {msg}");
    }

    #[test]
    fn strict_eval_returns_undefined_for_not_equals_missing_key() {
        let cond = parse_condition("mode!=fast").unwrap();
        let ctx = HashMap::new();
        let result = evaluate_condition_strict(&cond, &ctx);
        assert!(result.is_err());
        match result.unwrap_err() {
            ConditionError::UndefinedVariable { name } => assert_eq!(name, "mode"),
            other => panic!("expected UndefinedVariable, got {other:?}"),
        }
    }

    #[test]
    fn strict_eval_returns_undefined_for_greater_than_missing_key() {
        let cond = parse_condition("score>0.5").unwrap();
        let ctx = HashMap::new();
        let result = evaluate_condition_strict(&cond, &ctx);
        assert!(result.is_err());
        match result.unwrap_err() {
            ConditionError::UndefinedVariable { name } => assert_eq!(name, "score"),
            other => panic!("expected UndefinedVariable, got {other:?}"),
        }
    }

    #[test]
    fn strict_eval_returns_undefined_for_less_than_missing_key() {
        let cond = parse_condition("count<10").unwrap();
        let ctx = HashMap::new();
        let result = evaluate_condition_strict(&cond, &ctx);
        assert!(result.is_err());
        match result.unwrap_err() {
            ConditionError::UndefinedVariable { name } => assert_eq!(name, "count"),
            other => panic!("expected UndefinedVariable, got {other:?}"),
        }
    }

    #[test]
    fn strict_eval_succeeds_when_all_vars_present() {
        let cond = parse_condition("status=done && score>0.5").unwrap();
        let mut ctx = HashMap::new();
        ctx.insert("status".to_string(), "done".to_string());
        ctx.insert("score".to_string(), "0.8".to_string());
        let result = evaluate_condition_strict(&cond, &ctx).unwrap();
        assert!(result);
    }

    #[test]
    fn strict_eval_succeeds_with_false_result() {
        let cond = parse_condition("status=done").unwrap();
        let mut ctx = HashMap::new();
        ctx.insert("status".to_string(), "pending".to_string());
        let result = evaluate_condition_strict(&cond, &ctx).unwrap();
        assert!(!result);
    }

    #[test]
    fn strict_eval_undefined_in_and_expression() {
        // "a=1 && b=2" where b is missing -> UndefinedVariable for b
        let cond = parse_condition("a=1 && b=2").unwrap();
        let mut ctx = HashMap::new();
        ctx.insert("a".to_string(), "1".to_string());
        let result = evaluate_condition_strict(&cond, &ctx);
        assert!(result.is_err());
        match result.unwrap_err() {
            ConditionError::UndefinedVariable { name } => assert_eq!(name, "b"),
            other => panic!("expected UndefinedVariable, got {other:?}"),
        }
    }

    #[test]
    fn strict_eval_undefined_in_or_expression() {
        // "a=1 || missing=val" where missing is absent -> UndefinedVariable
        let cond = parse_condition("a=1 || missing=val").unwrap();
        let mut ctx = HashMap::new();
        ctx.insert("a".to_string(), "1".to_string());
        let result = evaluate_condition_strict(&cond, &ctx);
        assert!(result.is_err());
        match result.unwrap_err() {
            ConditionError::UndefinedVariable { name } => assert_eq!(name, "missing"),
            other => panic!("expected UndefinedVariable, got {other:?}"),
        }
    }

    #[test]
    fn strict_eval_undefined_in_not_expression() {
        let cond = parse_condition("!gone=yes").unwrap();
        let ctx = HashMap::new();
        let result = evaluate_condition_strict(&cond, &ctx);
        assert!(result.is_err());
        match result.unwrap_err() {
            ConditionError::UndefinedVariable { name } => assert_eq!(name, "gone"),
            other => panic!("expected UndefinedVariable, got {other:?}"),
        }
    }

    #[test]
    fn strict_eval_literals_need_no_context() {
        let ctx = HashMap::new();
        assert!(evaluate_condition_strict(&Condition::True, &ctx).unwrap());
        assert!(!evaluate_condition_strict(&Condition::False, &ctx).unwrap());
    }

    // ---------------------------------------------------------------
    // Validate function tests
    // ---------------------------------------------------------------

    #[test]
    fn validate_accepts_valid_expression() {
        assert!(validate("status=done").is_ok());
        assert!(validate("a=b && c=d").is_ok());
        assert!(validate("score>0.5 || count<10").is_ok());
        assert!(validate("!status=failed").is_ok());
        assert!(validate("(a=b)").is_ok());
        assert!(validate("true").is_ok());
        assert!(validate("false").is_ok());
    }

    #[test]
    fn validate_rejects_empty_expression() {
        let result = validate("");
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), ConditionError::Empty));
    }

    #[test]
    fn validate_rejects_invalid_token() {
        let result = validate("not_a_condition");
        assert!(result.is_err());
        match result.unwrap_err() {
            ConditionError::UnexpectedToken { ref token, .. } => {
                assert_eq!(token, "not_a_condition");
            }
            ref other => panic!("expected UnexpectedToken, got {other:?}"),
        }
    }

    #[test]
    fn validate_rejects_unmatched_parens() {
        let result = validate("(a=b");
        assert!(result.is_err());
        match result.unwrap_err() {
            ConditionError::ParseError { ref message, .. } => {
                assert!(
                    message.contains("unmatched parenthesis"),
                    "message was: {message}"
                );
            }
            ref other => panic!("expected ParseError, got {other:?}"),
        }
    }

    #[test]
    fn validate_rejects_bad_numeric() {
        let result = validate("score>not_a_number");
        assert!(result.is_err());
        match result.unwrap_err() {
            ConditionError::ParseError { ref message, .. } => {
                assert!(
                    message.contains("expected numeric value"),
                    "message was: {message}"
                );
            }
            ref other => panic!("expected ParseError, got {other:?}"),
        }
    }

    #[test]
    fn validate_rejects_incomplete_comparison() {
        let result = validate("=value");
        assert!(result.is_err());
        match result.unwrap_err() {
            ConditionError::ParseError { ref message, .. } => {
                assert!(
                    message.contains("invalid comparison"),
                    "message was: {message}"
                );
            }
            ref other => panic!("expected ParseError, got {other:?}"),
        }
    }

    // ---------------------------------------------------------------
    // Error Display tests
    // ---------------------------------------------------------------

    #[test]
    fn error_display_parse_error() {
        let err = ConditionError::ParseError {
            message: "unmatched parenthesis".to_string(),
            position: 5,
        };
        let msg = err.to_string();
        assert_eq!(msg, "parse error at position 5: unmatched parenthesis");
    }

    #[test]
    fn error_display_unexpected_token() {
        let err = ConditionError::UnexpectedToken {
            token: "blarg".to_string(),
            position: 3,
        };
        let msg = err.to_string();
        assert_eq!(msg, "unexpected token at position 3: 'blarg'");
    }

    #[test]
    fn error_display_empty() {
        let err = ConditionError::Empty;
        assert_eq!(err.to_string(), "empty condition expression");
    }

    #[test]
    fn error_display_undefined_variable() {
        let err = ConditionError::UndefinedVariable {
            name: "status".to_string(),
        };
        let msg = err.to_string();
        assert_eq!(
            msg,
            "undefined variable 'status' referenced in condition; provide a value for 'status' in the context"
        );
    }
}
