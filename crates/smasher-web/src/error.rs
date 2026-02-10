// ABOUTME: Web error types mapping domain errors to HTTP status codes.
// ABOUTME: Provides axum IntoResponse implementation for ergonomic error handling in handlers.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

#[derive(Debug, thiserror::Error)]
pub enum WebError {
    #[error("not found: {0}")]
    NotFound(String),

    #[error("bad request: {0}")]
    BadRequest(String),

    #[error("internal error: {0}")]
    Internal(String),

    #[error("pipeline error: {0}")]
    Pipeline(#[from] smasher_attractor::engine::EngineError),

    #[error("parse error: {0}")]
    Parse(#[from] smasher_attractor::dot::parser::ParseError),

    #[error("graph error: {0}")]
    Graph(#[from] smasher_attractor::graph::ResolutionError),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
}

impl IntoResponse for WebError {
    fn into_response(self) -> Response {
        let status = match &self {
            WebError::NotFound(_) => StatusCode::NOT_FOUND,
            WebError::BadRequest(_) => StatusCode::BAD_REQUEST,
            WebError::Internal(_) | WebError::Pipeline(_) | WebError::Io(_) | WebError::Json(_) => {
                StatusCode::INTERNAL_SERVER_ERROR
            }
            WebError::Parse(_) | WebError::Graph(_) => StatusCode::UNPROCESSABLE_ENTITY,
        };

        let body = serde_json::json!({
            "error": self.to_string(),
        });

        (status, axum::Json(body)).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn not_found_returns_404() {
        let err = WebError::NotFound("run xyz".into());
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn bad_request_returns_400() {
        let err = WebError::BadRequest("missing field".into());
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[test]
    fn internal_returns_500() {
        let err = WebError::Internal("something broke".into());
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[test]
    fn error_display_includes_message() {
        let err = WebError::NotFound("run abc".into());
        assert_eq!(err.to_string(), "not found: run abc");
    }

    #[test]
    fn json_error_returns_500() {
        let err: Result<serde_json::Value, _> = serde_json::from_str("not json");
        let web_err = WebError::Json(err.unwrap_err());
        let response = web_err.into_response();
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }
}
