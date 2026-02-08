// ABOUTME: Retry logic with exponential backoff and jitter for transient LLM provider errors.
// ABOUTME: Provides a configurable RetryPolicy and a generic async retry executor.

use rand::Rng;
use std::time::Duration;

/// Configuration for retry behavior with exponential backoff.
#[derive(Debug, Clone)]
pub struct RetryPolicy {
    /// Maximum number of retry attempts (not counting the initial attempt).
    pub max_retries: u32,
    /// Initial backoff delay in milliseconds.
    pub initial_backoff_ms: u64,
    /// Maximum backoff delay in milliseconds (cap).
    pub max_backoff_ms: u64,
    /// Backoff multiplier (typically 2.0 for exponential).
    pub backoff_multiplier: f64,
    /// Whether to add random jitter to the delay.
    pub jitter: bool,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_retries: 3,
            initial_backoff_ms: 1000,
            max_backoff_ms: 60_000,
            backoff_multiplier: 2.0,
            jitter: true,
        }
    }
}

impl RetryPolicy {
    /// Create a policy that never retries.
    pub fn no_retry() -> Self {
        Self {
            max_retries: 0,
            ..Default::default()
        }
    }

    /// Compute the delay for a given attempt number (0-indexed).
    ///
    /// Uses the formula: `min(initial_backoff_ms * backoff_multiplier^attempt, max_backoff_ms)`.
    /// When jitter is enabled, the delay is multiplied by a random factor in `[0.5, 1.0)`.
    pub fn compute_delay(&self, attempt: u32) -> Duration {
        let base_ms = self.initial_backoff_ms as f64 * self.backoff_multiplier.powi(attempt as i32);
        let capped_ms = base_ms.min(self.max_backoff_ms as f64);

        let final_ms = if self.jitter {
            let jitter_factor = rand::rng().random_range(0.5..1.0);
            capped_ms * jitter_factor
        } else {
            capped_ms
        };

        Duration::from_millis(final_ms as u64)
    }

    /// Builder method to set the maximum number of retries.
    pub fn with_max_retries(mut self, n: u32) -> Self {
        self.max_retries = n;
        self
    }

    /// Builder method to set the initial backoff delay in milliseconds.
    pub fn with_initial_backoff_ms(mut self, ms: u64) -> Self {
        self.initial_backoff_ms = ms;
        self
    }

    /// Builder method to set the maximum backoff delay in milliseconds.
    pub fn with_max_backoff_ms(mut self, ms: u64) -> Self {
        self.max_backoff_ms = ms;
        self
    }

    /// Builder method to enable or disable jitter.
    pub fn with_jitter(mut self, jitter: bool) -> Self {
        self.jitter = jitter;
        self
    }
}

/// Execute an async operation with retries according to the given policy.
///
/// On each failure the error is inspected: non-retryable errors are returned
/// immediately, while retryable errors trigger a backoff delay before the next
/// attempt. If the error carries a `retry_after_ms` hint (e.g. from a rate
/// limit response), that value is used as the delay instead of the computed
/// backoff.
pub async fn retry<F, Fut, T>(policy: &RetryPolicy, mut operation: F) -> crate::types::Result<T>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = crate::types::Result<T>>,
{
    let mut attempt: u32 = 0;

    loop {
        match operation().await {
            Ok(value) => return Ok(value),
            Err(err) => {
                if !err.retryable() || attempt >= policy.max_retries {
                    return Err(err);
                }

                let delay = if let Some(retry_after) = err.retry_after_ms() {
                    Duration::from_millis(retry_after)
                } else {
                    policy.compute_delay(attempt)
                };

                tracing::warn!(
                    attempt = attempt + 1,
                    max_retries = policy.max_retries,
                    delay_ms = delay.as_millis() as u64,
                    error = %err,
                    "retrying after transient error"
                );

                tokio::time::sleep(delay).await;
                attempt += 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Error;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};

    #[test]
    fn default_policy_has_expected_values() {
        let policy = RetryPolicy::default();
        assert_eq!(policy.max_retries, 3);
        assert_eq!(policy.initial_backoff_ms, 1000);
        assert_eq!(policy.max_backoff_ms, 60_000);
        assert_eq!(policy.backoff_multiplier, 2.0);
        assert!(policy.jitter);
    }

    #[test]
    fn compute_delay_without_jitter_follows_exponential_pattern() {
        let policy = RetryPolicy::default().with_jitter(false);

        let d0 = policy.compute_delay(0);
        let d1 = policy.compute_delay(1);
        let d2 = policy.compute_delay(2);

        assert_eq!(d0, Duration::from_millis(1000));
        assert_eq!(d1, Duration::from_millis(2000));
        assert_eq!(d2, Duration::from_millis(4000));
    }

    #[test]
    fn compute_delay_respects_max_backoff_cap() {
        let policy = RetryPolicy::default()
            .with_jitter(false)
            .with_max_backoff_ms(3000);

        let d0 = policy.compute_delay(0);
        let d2 = policy.compute_delay(2);
        let d10 = policy.compute_delay(10);

        assert_eq!(d0, Duration::from_millis(1000));
        assert_eq!(d2, Duration::from_millis(3000));
        assert_eq!(d10, Duration::from_millis(3000));
    }

    #[test]
    fn compute_delay_with_jitter_produces_values_in_expected_range() {
        let policy = RetryPolicy::default().with_jitter(true);

        // Run multiple times to check the jitter range for attempt 0
        for _ in 0..100 {
            let delay = policy.compute_delay(0);
            let ms = delay.as_millis() as u64;
            assert!(
                (500..1000).contains(&ms),
                "expected delay in [500, 1000) but got {ms}"
            );
        }
    }

    #[test]
    fn no_retry_has_zero_max_retries() {
        let policy = RetryPolicy::no_retry();
        assert_eq!(policy.max_retries, 0);
    }

    #[test]
    fn builder_methods_chain_correctly() {
        let policy = RetryPolicy::default()
            .with_max_retries(5)
            .with_initial_backoff_ms(200)
            .with_max_backoff_ms(10_000)
            .with_jitter(false);

        assert_eq!(policy.max_retries, 5);
        assert_eq!(policy.initial_backoff_ms, 200);
        assert_eq!(policy.max_backoff_ms, 10_000);
        assert!(!policy.jitter);
    }

    #[tokio::test]
    async fn retry_succeeds_on_first_attempt() {
        let policy = RetryPolicy::default()
            .with_initial_backoff_ms(1)
            .with_jitter(false);

        let call_count = Arc::new(AtomicU32::new(0));
        let count = call_count.clone();

        let result = retry(&policy, || {
            let count = count.clone();
            async move {
                count.fetch_add(1, Ordering::SeqCst);
                Ok::<_, Error>(42)
            }
        })
        .await;

        assert_eq!(result.unwrap(), 42);
        assert_eq!(call_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn retry_retries_on_retryable_error_and_eventually_succeeds() {
        let policy = RetryPolicy::default()
            .with_max_retries(3)
            .with_initial_backoff_ms(1)
            .with_jitter(false);

        let call_count = Arc::new(AtomicU32::new(0));
        let count = call_count.clone();

        let result = retry(&policy, || {
            let count = count.clone();
            async move {
                let n = count.fetch_add(1, Ordering::SeqCst);
                if n < 2 {
                    Err(Error::RateLimited {
                        provider: "test".into(),
                        retry_after_ms: Some(1),
                    })
                } else {
                    Ok(99)
                }
            }
        })
        .await;

        assert_eq!(result.unwrap(), 99);
        // Initial attempt + 2 retries = 3 calls total
        assert_eq!(call_count.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn retry_does_not_retry_on_non_retryable_error() {
        let policy = RetryPolicy::default()
            .with_max_retries(3)
            .with_initial_backoff_ms(1)
            .with_jitter(false);

        let call_count = Arc::new(AtomicU32::new(0));
        let count = call_count.clone();

        let result: crate::types::Result<i32> = retry(&policy, || {
            let count = count.clone();
            async move {
                count.fetch_add(1, Ordering::SeqCst);
                Err(Error::Authentication {
                    provider: "test".into(),
                    message: "bad key".into(),
                })
            }
        })
        .await;

        assert!(result.is_err());
        // Should only be called once since auth errors are not retryable
        assert_eq!(call_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn retry_gives_up_after_max_retries_exhausted() {
        let policy = RetryPolicy::default()
            .with_max_retries(2)
            .with_initial_backoff_ms(1)
            .with_jitter(false);

        let call_count = Arc::new(AtomicU32::new(0));
        let count = call_count.clone();

        let result: crate::types::Result<i32> = retry(&policy, || {
            let count = count.clone();
            async move {
                count.fetch_add(1, Ordering::SeqCst);
                Err(Error::RateLimited {
                    provider: "test".into(),
                    retry_after_ms: None,
                })
            }
        })
        .await;

        assert!(result.is_err());
        // Initial attempt + 2 retries = 3 calls total
        assert_eq!(call_count.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn retry_respects_error_retry_after_ms() {
        let policy = RetryPolicy::default()
            .with_max_retries(2)
            .with_initial_backoff_ms(50_000) // intentionally large to prove retry_after_ms wins
            .with_jitter(false);

        let call_count = Arc::new(AtomicU32::new(0));
        let count = call_count.clone();

        let start = tokio::time::Instant::now();

        let result = retry(&policy, || {
            let count = count.clone();
            async move {
                let n = count.fetch_add(1, Ordering::SeqCst);
                if n < 1 {
                    Err(Error::RateLimited {
                        provider: "test".into(),
                        retry_after_ms: Some(1),
                    })
                } else {
                    Ok("done")
                }
            }
        })
        .await;

        let elapsed = start.elapsed();

        assert_eq!(result.unwrap(), "done");
        assert_eq!(call_count.load(Ordering::SeqCst), 2);
        // Should have used the 1ms retry_after, not the 50s backoff
        assert!(
            elapsed < Duration::from_millis(500),
            "expected fast retry via retry_after_ms, but took {elapsed:?}"
        );
    }
}
