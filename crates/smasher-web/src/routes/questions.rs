// ABOUTME: Interviewer question and answer route handlers for human-in-the-loop pipelines.
// ABOUTME: Lists pending questions and submits answers to the HttpInterviewer queue.

use axum::extract::{Path, State};
use axum::routing::{get, post};
use axum::{Json, Router};

use smasher_attractor::http_interviewer::{
    AnswerQuestionRequest, AnswerQuestionResponse, ListQuestionsResponse,
};

use crate::error::WebError;
use crate::state::AppState;

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/runs/{id}/questions", get(list_questions))
        .route(
            "/api/runs/{id}/questions/{qid}/answer",
            post(answer_question),
        )
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

async fn list_questions(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<ListQuestionsResponse>, WebError> {
    let runs = state.runs.read().await;
    let record = runs
        .get(&id)
        .ok_or_else(|| WebError::NotFound(format!("run {id}")))?;
    Ok(Json(record.interviewer.list_questions()))
}

async fn answer_question(
    State(state): State<AppState>,
    Path((id, qid)): Path<(String, String)>,
    Json(req): Json<AnswerQuestionRequest>,
) -> Result<Json<AnswerQuestionResponse>, WebError> {
    let runs = state.runs.read().await;
    let record = runs
        .get(&id)
        .ok_or_else(|| WebError::NotFound(format!("run {id}")))?;
    Ok(Json(record.interviewer.answer_question(&qid, &req.answer)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    fn test_state() -> AppState {
        let client = smasher_llm::client::Client::from_env();
        AppState::new(client, "test-model".into(), "/tmp".into())
    }

    #[tokio::test]
    async fn list_questions_not_found() {
        let app = router().with_state(test_state());
        let req = Request::builder()
            .uri("/api/runs/nonexistent/questions")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn answer_question_run_not_found() {
        let app = router().with_state(test_state());
        let body = serde_json::json!({"answer": "yes"});
        let req = Request::builder()
            .method("POST")
            .uri("/api/runs/nonexistent/questions/q1/answer")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }
}
