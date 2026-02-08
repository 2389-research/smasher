// ABOUTME: Shared utilities for the LLM client: SSE parsing, HTTP helpers, and retry logic.
// ABOUTME: Provides reusable infrastructure used by all provider adapters.

pub mod http;
pub mod retry;
pub mod sse;

pub use http::{build_error_from_status, classify_status, parse_rate_limit_headers, parse_retry_after};
pub use retry::{RetryPolicy, retry};
pub use sse::{SseEvent, parse_sse_stream};
