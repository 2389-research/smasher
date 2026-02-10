// ABOUTME: HTTP utilities for provider adapters: rate limit header parsing and status code classification.
// ABOUTME: Provides helpers to extract retry-after delays, rate limit info, and map status codes to errors.

use chrono::{DateTime, Duration, Utc};
use reqwest::header::HeaderMap;

use crate::types::{Error, RateLimitInfo, StatusClass};

/// Classify an HTTP status code into a `StatusClass`.
///
/// Thin convenience wrapper around `StatusClass::from_status`.
pub fn classify_status(status_code: u16) -> StatusClass {
    StatusClass::from_status(status_code)
}

/// Try to read a header value as a UTF-8 string from the map, checking multiple
/// candidate header names (case-insensitive matching is handled by `HeaderMap`).
fn get_header_str<'a>(headers: &'a HeaderMap, names: &[&str]) -> Option<&'a str> {
    for name in names {
        if let Some(value) = headers.get(*name)
            && let Ok(s) = value.to_str()
        {
            return Some(s);
        }
    }
    None
}

/// Try to read a header value as `u32`, checking multiple candidate header names.
fn get_header_u32(headers: &HeaderMap, names: &[&str]) -> Option<u32> {
    get_header_str(headers, names).and_then(|s| s.trim().parse::<u32>().ok())
}

/// Parse a reset-time header value into a `DateTime<Utc>`.
///
/// Tries numeric (seconds from now) first, then RFC 3339 / ISO 8601 datetime.
fn parse_reset_time(value: &str) -> Option<DateTime<Utc>> {
    let trimmed = value.trim();

    // Try as integer seconds from now.
    if let Ok(seconds) = trimmed.parse::<i64>() {
        return Some(Utc::now() + Duration::seconds(seconds));
    }

    // Try as RFC 3339 datetime string.
    if let Ok(dt) = DateTime::parse_from_rfc3339(trimmed) {
        return Some(dt.with_timezone(&Utc));
    }

    None
}

/// Parse standard rate limit headers from an HTTP response.
///
/// Different LLM providers use different header names. This function checks the
/// most common variants used by OpenAI, Anthropic, and standards-track headers.
/// Returns `Some(RateLimitInfo)` if any rate-limit header is found, `None` otherwise.
pub fn parse_rate_limit_headers(headers: &HeaderMap) -> Option<RateLimitInfo> {
    let requests_remaining = get_header_u32(
        headers,
        &[
            "x-ratelimit-remaining-requests",
            "x-ratelimit-remaining",
            "ratelimit-remaining",
        ],
    );

    let requests_limit = get_header_u32(
        headers,
        &[
            "x-ratelimit-limit-requests",
            "x-ratelimit-limit",
            "ratelimit-limit",
        ],
    );

    let tokens_remaining = get_header_u32(headers, &["x-ratelimit-remaining-tokens"]);

    let tokens_limit = get_header_u32(headers, &["x-ratelimit-limit-tokens"]);

    let reset_at = get_header_str(
        headers,
        &[
            "x-ratelimit-reset-requests",
            "x-ratelimit-reset",
            "retry-after",
        ],
    )
    .and_then(parse_reset_time);

    // Only return Some if we found at least one header.
    if requests_remaining.is_none()
        && requests_limit.is_none()
        && tokens_remaining.is_none()
        && tokens_limit.is_none()
        && reset_at.is_none()
    {
        return None;
    }

    Some(RateLimitInfo {
        requests_remaining,
        requests_limit,
        tokens_remaining,
        tokens_limit,
        reset_at,
    })
}

/// Parse the `retry-after` header for retry delay in milliseconds.
///
/// - If the value is numeric, it is treated as seconds and converted to ms.
/// - If the value is an HTTP-date / RFC 3339, the diff from now is computed in ms.
/// - Returns `None` if the header is missing or unparseable.
pub fn parse_retry_after(headers: &HeaderMap) -> Option<u64> {
    let value = headers.get("retry-after")?.to_str().ok()?;
    let trimmed = value.trim();

    // Try as numeric seconds.
    if let Ok(seconds) = trimmed.parse::<u64>() {
        return Some(seconds * 1000);
    }

    // Try as RFC 3339 datetime and compute diff.
    if let Ok(dt) = DateTime::parse_from_rfc3339(trimmed) {
        let diff = dt.with_timezone(&Utc) - Utc::now();
        let ms = diff.num_milliseconds();
        if ms > 0 {
            return Some(ms as u64);
        }
        // If the date is in the past, return 0 (retry immediately).
        return Some(0);
    }

    None
}

/// Build an appropriate `Error` variant from a non-2xx HTTP status code.
///
/// Uses the response body and headers to populate error details such as
/// retry-after delays and model names.
pub fn build_error_from_status(
    provider: &str,
    status_code: u16,
    body: &str,
    headers: &HeaderMap,
) -> Error {
    match status_code {
        401 => Error::Authentication {
            provider: provider.to_string(),
            message: body.to_string(),
        },
        403 => Error::AccessDenied {
            provider: provider.to_string(),
            message: body.to_string(),
        },
        404 => {
            let model = if body.is_empty() {
                "unknown".to_string()
            } else {
                body.to_string()
            };
            Error::ModelNotFound {
                provider: provider.to_string(),
                model,
            }
        }
        429 => Error::RateLimited {
            provider: provider.to_string(),
            retry_after_ms: parse_retry_after(headers),
        },
        400 | 422 => Error::InvalidRequest {
            provider: provider.to_string(),
            message: body.to_string(),
        },
        500..=599 => Error::ServerError {
            provider: provider.to_string(),
            status_code,
            message: body.to_string(),
        },
        _ => Error::Other {
            message: format!("unexpected HTTP {status_code} from {provider}: {body}"),
            retryable: false,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header::HeaderMap;

    // ── classify_status ──────────────────────────────────────────────

    #[test]
    fn classify_status_delegates_correctly_for_200() {
        assert_eq!(classify_status(200), StatusClass::Success);
    }

    #[test]
    fn classify_status_delegates_correctly_for_401() {
        assert_eq!(classify_status(401), StatusClass::Authentication);
    }

    #[test]
    fn classify_status_delegates_correctly_for_429() {
        assert_eq!(classify_status(429), StatusClass::RateLimited);
    }

    #[test]
    fn classify_status_delegates_correctly_for_500() {
        assert_eq!(classify_status(500), StatusClass::ServerError);
    }

    // ── parse_rate_limit_headers ─────────────────────────────────────

    #[test]
    fn parse_rate_limit_headers_with_anthropic_style_headers() {
        let mut headers = HeaderMap::new();
        headers.insert("x-ratelimit-remaining-requests", "99".parse().unwrap());
        headers.insert("x-ratelimit-limit-requests", "100".parse().unwrap());
        headers.insert("x-ratelimit-remaining-tokens", "9000".parse().unwrap());
        headers.insert("x-ratelimit-limit-tokens", "10000".parse().unwrap());

        let info = parse_rate_limit_headers(&headers).expect("should parse");
        assert_eq!(info.requests_remaining, Some(99));
        assert_eq!(info.requests_limit, Some(100));
        assert_eq!(info.tokens_remaining, Some(9000));
        assert_eq!(info.tokens_limit, Some(10000));
    }

    #[test]
    fn parse_rate_limit_headers_with_openai_style_headers() {
        let mut headers = HeaderMap::new();
        headers.insert("x-ratelimit-remaining-requests", "45".parse().unwrap());
        headers.insert("x-ratelimit-limit-requests", "50".parse().unwrap());
        headers.insert("x-ratelimit-remaining-tokens", "80000".parse().unwrap());
        headers.insert("x-ratelimit-limit-tokens", "100000".parse().unwrap());
        headers.insert(
            "x-ratelimit-reset-requests",
            "2026-06-01T00:00:00Z".parse().unwrap(),
        );

        let info = parse_rate_limit_headers(&headers).expect("should parse");
        assert_eq!(info.requests_remaining, Some(45));
        assert_eq!(info.requests_limit, Some(50));
        assert_eq!(info.tokens_remaining, Some(80000));
        assert_eq!(info.tokens_limit, Some(100000));
        assert!(info.reset_at.is_some());
    }

    #[test]
    fn parse_rate_limit_headers_with_no_rate_limit_headers_returns_none() {
        let headers = HeaderMap::new();
        assert!(parse_rate_limit_headers(&headers).is_none());
    }

    #[test]
    fn parse_rate_limit_headers_with_unrelated_headers_returns_none() {
        let mut headers = HeaderMap::new();
        headers.insert("content-type", "application/json".parse().unwrap());
        headers.insert("x-request-id", "abc123".parse().unwrap());
        assert!(parse_rate_limit_headers(&headers).is_none());
    }

    #[test]
    fn parse_rate_limit_headers_with_partial_headers() {
        let mut headers = HeaderMap::new();
        headers.insert("ratelimit-remaining", "10".parse().unwrap());

        let info = parse_rate_limit_headers(&headers).expect("should parse");
        assert_eq!(info.requests_remaining, Some(10));
        assert_eq!(info.requests_limit, None);
        assert_eq!(info.tokens_remaining, None);
        assert_eq!(info.tokens_limit, None);
        assert!(info.reset_at.is_none());
    }

    #[test]
    fn parse_rate_limit_headers_with_reset_time_as_seconds() {
        let mut headers = HeaderMap::new();
        headers.insert("x-ratelimit-remaining-requests", "5".parse().unwrap());
        headers.insert("x-ratelimit-reset-requests", "60".parse().unwrap());

        let before = Utc::now();
        let info = parse_rate_limit_headers(&headers).expect("should parse");
        let after = Utc::now();

        assert_eq!(info.requests_remaining, Some(5));

        let reset = info.reset_at.expect("should have reset_at");
        // The reset time should be ~60 seconds from now.
        let expected_earliest = before + Duration::seconds(60);
        let expected_latest = after + Duration::seconds(60);
        assert!(
            reset >= expected_earliest && reset <= expected_latest,
            "reset_at {reset} should be between {expected_earliest} and {expected_latest}"
        );
    }

    // ── parse_retry_after ────────────────────────────────────────────

    #[test]
    fn parse_retry_after_with_numeric_value() {
        let mut headers = HeaderMap::new();
        headers.insert("retry-after", "30".parse().unwrap());

        let ms = parse_retry_after(&headers).expect("should parse");
        assert_eq!(ms, 30_000);
    }

    #[test]
    fn parse_retry_after_with_missing_header_returns_none() {
        let headers = HeaderMap::new();
        assert!(parse_retry_after(&headers).is_none());
    }

    #[test]
    fn parse_retry_after_with_unparseable_value_returns_none() {
        let mut headers = HeaderMap::new();
        headers.insert("retry-after", "not-a-number".parse().unwrap());
        assert!(parse_retry_after(&headers).is_none());
    }

    // ── build_error_from_status ──────────────────────────────────────

    #[test]
    fn build_error_from_status_401_yields_authentication() {
        let headers = HeaderMap::new();
        let err = build_error_from_status("anthropic", 401, "invalid api key", &headers);
        match err {
            Error::Authentication { provider, message } => {
                assert_eq!(provider, "anthropic");
                assert_eq!(message, "invalid api key");
            }
            other => panic!("expected Authentication, got {other:?}"),
        }
    }

    #[test]
    fn build_error_from_status_403_yields_access_denied() {
        let headers = HeaderMap::new();
        let err = build_error_from_status("openai", 403, "forbidden", &headers);
        match err {
            Error::AccessDenied { provider, message } => {
                assert_eq!(provider, "openai");
                assert_eq!(message, "forbidden");
            }
            other => panic!("expected AccessDenied, got {other:?}"),
        }
    }

    #[test]
    fn build_error_from_status_404_yields_model_not_found() {
        let headers = HeaderMap::new();
        let err = build_error_from_status("openai", 404, "gpt-5-turbo", &headers);
        match err {
            Error::ModelNotFound { provider, model } => {
                assert_eq!(provider, "openai");
                assert_eq!(model, "gpt-5-turbo");
            }
            other => panic!("expected ModelNotFound, got {other:?}"),
        }
    }

    #[test]
    fn build_error_from_status_404_with_empty_body_uses_unknown() {
        let headers = HeaderMap::new();
        let err = build_error_from_status("openai", 404, "", &headers);
        match err {
            Error::ModelNotFound { model, .. } => {
                assert_eq!(model, "unknown");
            }
            other => panic!("expected ModelNotFound, got {other:?}"),
        }
    }

    #[test]
    fn build_error_from_status_429_yields_rate_limited() {
        let mut headers = HeaderMap::new();
        headers.insert("retry-after", "5".parse().unwrap());

        let err = build_error_from_status("anthropic", 429, "too many requests", &headers);
        match err {
            Error::RateLimited {
                provider,
                retry_after_ms,
            } => {
                assert_eq!(provider, "anthropic");
                assert_eq!(retry_after_ms, Some(5000));
            }
            other => panic!("expected RateLimited, got {other:?}"),
        }
    }

    #[test]
    fn build_error_from_status_429_without_retry_after() {
        let headers = HeaderMap::new();
        let err = build_error_from_status("anthropic", 429, "slow down", &headers);
        match err {
            Error::RateLimited { retry_after_ms, .. } => {
                assert_eq!(retry_after_ms, None);
            }
            other => panic!("expected RateLimited, got {other:?}"),
        }
    }

    #[test]
    fn build_error_from_status_400_yields_invalid_request() {
        let headers = HeaderMap::new();
        let err = build_error_from_status("gemini", 400, "bad request body", &headers);
        match err {
            Error::InvalidRequest { provider, message } => {
                assert_eq!(provider, "gemini");
                assert_eq!(message, "bad request body");
            }
            other => panic!("expected InvalidRequest, got {other:?}"),
        }
    }

    #[test]
    fn build_error_from_status_422_yields_invalid_request() {
        let headers = HeaderMap::new();
        let err = build_error_from_status("openai", 422, "unprocessable", &headers);
        assert!(matches!(err, Error::InvalidRequest { .. }));
    }

    #[test]
    fn build_error_from_status_500_yields_server_error() {
        let headers = HeaderMap::new();
        let err = build_error_from_status("openai", 500, "internal server error", &headers);
        match err {
            Error::ServerError {
                provider,
                status_code,
                message,
            } => {
                assert_eq!(provider, "openai");
                assert_eq!(status_code, 500);
                assert_eq!(message, "internal server error");
            }
            other => panic!("expected ServerError, got {other:?}"),
        }
    }

    #[test]
    fn build_error_from_status_503_yields_server_error() {
        let headers = HeaderMap::new();
        let err = build_error_from_status("anthropic", 503, "service unavailable", &headers);
        assert!(matches!(
            err,
            Error::ServerError {
                status_code: 503,
                ..
            }
        ));
    }

    #[test]
    fn build_error_from_status_unknown_code_yields_other() {
        let headers = HeaderMap::new();
        let err = build_error_from_status("openai", 301, "moved", &headers);
        match err {
            Error::Other { message, retryable } => {
                assert!(!retryable);
                assert!(message.contains("301"));
                assert!(message.contains("openai"));
            }
            other => panic!("expected Other, got {other:?}"),
        }
    }
}
