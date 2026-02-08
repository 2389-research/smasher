// ABOUTME: Retry logic for pipeline node execution with exponential backoff and jitter.
// ABOUTME: Configurable per-node retry policies extracted from graph node attributes.

use std::time::Duration;

use rand::Rng;

use crate::graph::{GraphNode, NodeAttrValue};
use crate::state::Outcome;

/// Configuration for how to retry a failed node.
///
/// Controls the number of attempts, delay between retries, and whether
/// randomized jitter is applied to avoid thundering-herd effects.
#[derive(Debug, Clone)]
pub struct RetryPolicy {
    /// Total number of attempts (including the initial one).
    pub max_attempts: u32,
    /// Starting delay before the first retry.
    pub base_delay: Duration,
    /// Upper bound on computed delay (caps exponential growth).
    pub max_delay: Duration,
    /// When true, multiply the computed delay by a random factor in [0.5, 1.5).
    pub jitter: bool,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            base_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(30),
            jitter: true,
        }
    }
}

impl RetryPolicy {
    /// Create a policy that never retries (single attempt only).
    pub fn no_retry() -> Self {
        Self {
            max_attempts: 1,
            ..Default::default()
        }
    }

    /// Extract retry configuration from a graph node's attributes.
    ///
    /// Recognized attributes:
    /// - `retries` (Number): number of retries; max_attempts = retries + 1
    /// - `retry_delay` (Duration): base delay between retries
    /// - `max_retry_delay` (Duration): upper bound on delay
    /// - `retry_jitter` (Bool): whether to apply randomized jitter
    ///
    /// Any missing attribute falls back to the default value.
    pub fn from_node(node: &GraphNode) -> RetryPolicy {
        let mut policy = RetryPolicy::default();

        if let Some(NodeAttrValue::Number(n)) = node.attrs.get("retries") {
            policy.max_attempts = (*n as u32) + 1;
        }

        if let Some(NodeAttrValue::Duration(d)) = node.attrs.get("retry_delay") {
            policy.base_delay = *d;
        }

        if let Some(NodeAttrValue::Duration(d)) = node.attrs.get("max_retry_delay") {
            policy.max_delay = *d;
        }

        if let Some(NodeAttrValue::Bool(b)) = node.attrs.get("retry_jitter") {
            policy.jitter = *b;
        }

        policy
    }
}

/// Tracks retry attempts and last error for a single node execution.
#[derive(Debug, Clone, Default)]
pub struct RetryState {
    /// Number of attempts executed so far.
    pub attempts: u32,
    /// Error message from the most recent failure, if any.
    pub last_error: Option<String>,
}

impl RetryState {
    /// Create a fresh retry state with zero attempts.
    pub fn new() -> Self {
        Self {
            attempts: 0,
            last_error: None,
        }
    }

    /// Record an attempt, incrementing the counter and storing the error
    /// message if the outcome is a failure.
    pub fn record_attempt(&mut self, outcome: &Outcome) {
        self.attempts += 1;
        if let Outcome::Failure { error, .. } = outcome {
            self.last_error = Some(error.clone());
        }
    }

    /// Determine whether another retry should be attempted.
    ///
    /// Returns true only when the outcome is retryable AND the number
    /// of attempts so far is still below `policy.max_attempts`.
    pub fn should_retry(&self, policy: &RetryPolicy, outcome: &Outcome) -> bool {
        outcome.is_retryable() && self.attempts < policy.max_attempts
    }
}

/// Calculate the delay before the next retry attempt.
///
/// Uses exponential backoff: `base_delay * 2^(attempt - 1)`, capped at
/// `max_delay`. When jitter is enabled, the result is multiplied by a
/// random factor uniformly distributed in [0.5, 1.5).
pub fn compute_delay(policy: &RetryPolicy, attempt: u32) -> Duration {
    let exponent = attempt.saturating_sub(1);
    let multiplier = 2u64.saturating_pow(exponent);
    let base_nanos = policy.base_delay.as_nanos() as u64;
    let backoff_nanos = base_nanos.saturating_mul(multiplier);
    let max_nanos = policy.max_delay.as_nanos() as u64;
    let capped_nanos = backoff_nanos.min(max_nanos);

    if policy.jitter {
        let jitter_factor: f64 = rand::rng().random_range(0.5..1.5);
        let jittered = (capped_nanos as f64 * jitter_factor) as u64;
        Duration::from_nanos(jittered)
    } else {
        Duration::from_nanos(capped_nanos)
    }
}

/// Errors specific to the retry subsystem.
#[derive(Debug, thiserror::Error)]
pub enum RetryError {
    #[error("max retries exceeded for node '{node_id}' after {attempts} attempts: {last_error}")]
    MaxRetriesExceeded {
        node_id: String,
        attempts: u32,
        last_error: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use crate::graph::{GraphNode, NodeAttrValue, NodeType};

    /// Helper: build a GraphNode with the given attributes.
    fn make_node(attrs: HashMap<String, NodeAttrValue>) -> GraphNode {
        GraphNode {
            id: "test_node".to_string(),
            node_type: NodeType::Generic,
            label: None,
            attrs,
        }
    }

    // ---- Test 1: Default policy values ----
    #[test]
    fn default_policy_has_expected_values() {
        let policy = RetryPolicy::default();
        assert_eq!(policy.max_attempts, 3);
        assert_eq!(policy.base_delay, Duration::from_secs(1));
        assert_eq!(policy.max_delay, Duration::from_secs(30));
        assert!(policy.jitter);
    }

    // ---- Test 2: no_retry policy ----
    #[test]
    fn no_retry_policy_has_single_attempt() {
        let policy = RetryPolicy::no_retry();
        assert_eq!(policy.max_attempts, 1);
    }

    // ---- Test 3: from_node with all attributes present ----
    #[test]
    fn from_node_with_all_attributes() {
        let mut attrs = HashMap::new();
        attrs.insert("retries".to_string(), NodeAttrValue::Number(4.0));
        attrs.insert(
            "retry_delay".to_string(),
            NodeAttrValue::Duration(Duration::from_millis(500)),
        );
        attrs.insert(
            "max_retry_delay".to_string(),
            NodeAttrValue::Duration(Duration::from_secs(60)),
        );
        attrs.insert("retry_jitter".to_string(), NodeAttrValue::Bool(false));

        let node = make_node(attrs);
        let policy = RetryPolicy::from_node(&node);

        assert_eq!(policy.max_attempts, 5); // retries(4) + 1
        assert_eq!(policy.base_delay, Duration::from_millis(500));
        assert_eq!(policy.max_delay, Duration::from_secs(60));
        assert!(!policy.jitter);
    }

    // ---- Test 4: from_node with partial attributes (falls back to defaults) ----
    #[test]
    fn from_node_with_partial_attributes_falls_back_to_defaults() {
        let mut attrs = HashMap::new();
        attrs.insert("retries".to_string(), NodeAttrValue::Number(2.0));

        let node = make_node(attrs);
        let policy = RetryPolicy::from_node(&node);

        assert_eq!(policy.max_attempts, 3); // retries(2) + 1
        // These should remain at defaults
        assert_eq!(policy.base_delay, Duration::from_secs(1));
        assert_eq!(policy.max_delay, Duration::from_secs(30));
        assert!(policy.jitter);
    }

    // ---- Test 5: from_node with no retry attributes ----
    #[test]
    fn from_node_with_no_retry_attributes_returns_defaults() {
        let mut attrs = HashMap::new();
        attrs.insert(
            "timeout".to_string(),
            NodeAttrValue::Duration(Duration::from_secs(10)),
        );
        attrs.insert(
            "model".to_string(),
            NodeAttrValue::String("gpt-4".to_string()),
        );

        let node = make_node(attrs);
        let policy = RetryPolicy::from_node(&node);

        assert_eq!(policy.max_attempts, 3);
        assert_eq!(policy.base_delay, Duration::from_secs(1));
        assert_eq!(policy.max_delay, Duration::from_secs(30));
        assert!(policy.jitter);
    }

    // ---- Test 6: RetryState tracks attempts ----
    #[test]
    fn retry_state_tracks_attempts_and_errors() {
        let mut state = RetryState::new();
        assert_eq!(state.attempts, 0);
        assert!(state.last_error.is_none());

        let outcome = Outcome::retryable_failure("connection timeout");
        state.record_attempt(&outcome);
        assert_eq!(state.attempts, 1);
        assert_eq!(state.last_error, Some("connection timeout".to_string()));

        // Record a success; attempts still increase but last_error stays
        let success = Outcome::success();
        state.record_attempt(&success);
        assert_eq!(state.attempts, 2);
        // last_error is not cleared by success (only overwritten by new failure)
        assert_eq!(state.last_error, Some("connection timeout".to_string()));

        // Record another failure with a different message
        let outcome2 = Outcome::retryable_failure("dns failure");
        state.record_attempt(&outcome2);
        assert_eq!(state.attempts, 3);
        assert_eq!(state.last_error, Some("dns failure".to_string()));
    }

    // ---- Test 7: should_retry returns true when under limit and retryable ----
    #[test]
    fn should_retry_true_when_under_limit_and_retryable() {
        let policy = RetryPolicy {
            max_attempts: 3,
            ..Default::default()
        };
        let mut state = RetryState::new();
        let outcome = Outcome::retryable_failure("transient error");

        state.record_attempt(&outcome);
        assert!(state.should_retry(&policy, &outcome));

        state.record_attempt(&outcome);
        assert!(state.should_retry(&policy, &outcome));
    }

    // ---- Test 8: should_retry returns false when max attempts reached ----
    #[test]
    fn should_retry_false_when_max_attempts_reached() {
        let policy = RetryPolicy {
            max_attempts: 2,
            ..Default::default()
        };
        let mut state = RetryState::new();
        let outcome = Outcome::retryable_failure("error");

        state.record_attempt(&outcome);
        state.record_attempt(&outcome);
        // Now attempts == 2 == max_attempts, should not retry
        assert!(!state.should_retry(&policy, &outcome));
    }

    // ---- Test 9: should_retry returns false for non-retryable outcome ----
    #[test]
    fn should_retry_false_for_non_retryable_outcome() {
        let policy = RetryPolicy {
            max_attempts: 10,
            ..Default::default()
        };
        let mut state = RetryState::new();

        // Non-retryable failure
        let non_retryable = Outcome::failure("permanent error");
        state.record_attempt(&non_retryable);
        assert!(!state.should_retry(&policy, &non_retryable));

        // Success is also not retryable
        let success = Outcome::success();
        assert!(!state.should_retry(&policy, &success));

        // Skip is also not retryable
        let skip = Outcome::skip("not applicable");
        assert!(!state.should_retry(&policy, &skip));
    }

    // ---- Test 10: compute_delay exponential backoff ----
    #[test]
    fn compute_delay_exponential_backoff_without_jitter() {
        let policy = RetryPolicy {
            max_attempts: 5,
            base_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(120),
            jitter: false,
        };

        // attempt 1: base_delay * 2^0 = 1s
        assert_eq!(compute_delay(&policy, 1), Duration::from_secs(1));
        // attempt 2: base_delay * 2^1 = 2s
        assert_eq!(compute_delay(&policy, 2), Duration::from_secs(2));
        // attempt 3: base_delay * 2^2 = 4s
        assert_eq!(compute_delay(&policy, 3), Duration::from_secs(4));
        // attempt 4: base_delay * 2^3 = 8s
        assert_eq!(compute_delay(&policy, 4), Duration::from_secs(8));
    }

    // ---- Test 11: compute_delay capped at max_delay ----
    #[test]
    fn compute_delay_capped_at_max_delay() {
        let policy = RetryPolicy {
            max_attempts: 10,
            base_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(5),
            jitter: false,
        };

        // attempt 1: 1s (under cap)
        assert_eq!(compute_delay(&policy, 1), Duration::from_secs(1));
        // attempt 2: 2s (under cap)
        assert_eq!(compute_delay(&policy, 2), Duration::from_secs(2));
        // attempt 3: 4s (under cap)
        assert_eq!(compute_delay(&policy, 3), Duration::from_secs(4));
        // attempt 4: would be 8s, capped to 5s
        assert_eq!(compute_delay(&policy, 4), Duration::from_secs(5));
        // attempt 10: definitely capped
        assert_eq!(compute_delay(&policy, 10), Duration::from_secs(5));
    }

    // ---- Test 12: compute_delay with jitter stays within bounds ----
    #[test]
    fn compute_delay_with_jitter_stays_within_bounds() {
        let policy = RetryPolicy {
            max_attempts: 5,
            base_delay: Duration::from_secs(2),
            max_delay: Duration::from_secs(120),
            jitter: true,
        };

        // For attempt 1: base = 2s, jitter range [0.5, 1.5) => [1s, 3s)
        // Run many iterations to gain statistical confidence
        for _ in 0..200 {
            let delay = compute_delay(&policy, 1);
            let ms = delay.as_millis();
            assert!(
                ms >= 1000 && ms < 3000,
                "expected delay in [1000ms, 3000ms) for attempt 1, got {ms}ms"
            );
        }

        // For attempt 3: base = 2s * 4 = 8s, jitter range [0.5, 1.5) => [4s, 12s)
        for _ in 0..200 {
            let delay = compute_delay(&policy, 3);
            let ms = delay.as_millis();
            assert!(
                ms >= 4000 && ms < 12000,
                "expected delay in [4000ms, 12000ms) for attempt 3, got {ms}ms"
            );
        }
    }

    // ---- Test 13: compute_delay for attempt 0 returns base_delay ----
    #[test]
    fn compute_delay_attempt_zero_returns_base_delay() {
        let policy = RetryPolicy {
            max_attempts: 3,
            base_delay: Duration::from_millis(750),
            max_delay: Duration::from_secs(30),
            jitter: false,
        };

        // attempt 0: base_delay * 2^(0-1) but saturating_sub makes exponent 0
        // so it's base_delay * 2^0 = base_delay
        assert_eq!(compute_delay(&policy, 0), Duration::from_millis(750));
    }

    // ---- Test 14: RetryError display formatting ----
    #[test]
    fn retry_error_display_format() {
        let err = RetryError::MaxRetriesExceeded {
            node_id: "code_gen_1".to_string(),
            attempts: 5,
            last_error: "connection refused".to_string(),
        };
        let msg = err.to_string();
        assert!(msg.contains("code_gen_1"));
        assert!(msg.contains("5"));
        assert!(msg.contains("connection refused"));
        assert!(msg.contains("max retries exceeded"));
    }

    // ---- Test 15: from_node ignores wrong attribute types ----
    #[test]
    fn from_node_ignores_wrong_attribute_types() {
        let mut attrs = HashMap::new();
        // retries should be Number, but we provide a String - should be ignored
        attrs.insert(
            "retries".to_string(),
            NodeAttrValue::String("three".to_string()),
        );
        // retry_delay should be Duration, but we provide a Number - should be ignored
        attrs.insert("retry_delay".to_string(), NodeAttrValue::Number(500.0));
        // retry_jitter should be Bool, but we provide a String - should be ignored
        attrs.insert(
            "retry_jitter".to_string(),
            NodeAttrValue::String("yes".to_string()),
        );

        let node = make_node(attrs);
        let policy = RetryPolicy::from_node(&node);

        // All should remain at defaults since types didn't match
        assert_eq!(policy.max_attempts, 3);
        assert_eq!(policy.base_delay, Duration::from_secs(1));
        assert_eq!(policy.max_delay, Duration::from_secs(30));
        assert!(policy.jitter);
    }

    // ---- Test 16: should_retry with no_retry policy always returns false ----
    #[test]
    fn should_retry_with_no_retry_policy_always_false() {
        let policy = RetryPolicy::no_retry();
        let mut state = RetryState::new();
        let outcome = Outcome::retryable_failure("error");

        // Even before any attempts, max_attempts is 1 and attempts is 0,
        // so should_retry returns true. After recording 1 attempt, it returns false.
        state.record_attempt(&outcome);
        assert!(!state.should_retry(&policy, &outcome));
    }

    // ---- Test 17: compute_delay with jitter and max_delay cap ----
    #[test]
    fn compute_delay_jitter_respects_cap_before_jitter() {
        let policy = RetryPolicy {
            max_attempts: 5,
            base_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(4),
            jitter: true,
        };

        // attempt 5: base * 2^4 = 16s, capped to 4s, then jitter [0.5, 1.5) => [2s, 6s)
        for _ in 0..200 {
            let delay = compute_delay(&policy, 5);
            let ms = delay.as_millis();
            assert!(
                ms >= 2000 && ms < 6000,
                "expected delay in [2000ms, 6000ms) for capped+jittered attempt 5, got {ms}ms"
            );
        }
    }

    // ---- Test 18: RetryState::new starts at zero ----
    #[test]
    fn retry_state_new_starts_at_zero() {
        let state = RetryState::new();
        assert_eq!(state.attempts, 0);
        assert!(state.last_error.is_none());
    }
}
