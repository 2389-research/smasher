// ABOUTME: HTTP-backed interviewer that queues questions for external clients via REST.
// ABOUTME: Provides GET /api/v1/questions and POST /api/v1/questions/{id}/answer endpoints.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;

use crate::interviewer::{Interviewer, InterviewerError};
use crate::server::{HttpMethod, Route};
use crate::state::Context;

// ---------------------------------------------------------------------------
// Question types
// ---------------------------------------------------------------------------

/// Classifies what kind of interview interaction produced this question.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuestionKind {
    /// Free-form text question from `Interviewer::ask`.
    FreeForm,
    /// Multiple-choice question from `Interviewer::ask_with_options`.
    MultipleChoice,
    /// Yes/no approval from `Interviewer::approve`.
    Approval,
}

// ---------------------------------------------------------------------------
// PendingQuestion
// ---------------------------------------------------------------------------

/// A question waiting for an answer from an HTTP client.
///
/// The `answer_tx` oneshot sender is used to deliver the answer back to the
/// `HttpInterviewer::ask` call that is `.await`ing on the receiver end.
pub struct PendingQuestion {
    /// Unique identifier for this question.
    pub id: String,
    /// The question text presented to the human.
    pub question: String,
    /// Available choices (empty for free-form and approval questions).
    pub choices: Vec<String>,
    /// What kind of question this is.
    pub kind: QuestionKind,
    /// When this question was enqueued.
    pub created_at: Instant,
    /// Channel to send the answer back to the awaiting interviewer call.
    pub answer_tx: Option<oneshot::Sender<String>>,
}

impl std::fmt::Debug for PendingQuestion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PendingQuestion")
            .field("id", &self.id)
            .field("question", &self.question)
            .field("choices", &self.choices)
            .field("kind", &self.kind)
            .field("has_answer_tx", &self.answer_tx.is_some())
            .finish()
    }
}

// ---------------------------------------------------------------------------
// QuestionQueue
// ---------------------------------------------------------------------------

/// Thread-safe queue of pending questions awaiting HTTP answers.
#[derive(Debug, Clone)]
pub struct QuestionQueue {
    inner: Arc<Mutex<VecDeque<PendingQuestion>>>,
}

impl QuestionQueue {
    /// Create an empty question queue.
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(VecDeque::new())),
        }
    }

    /// Push a new pending question into the queue, returning its assigned ID.
    pub fn push(&self, question: PendingQuestion) -> String {
        let id = question.id.clone();
        let mut queue = self.inner.lock().expect("question queue lock poisoned");
        queue.push_back(question);
        id
    }

    /// List all pending questions as serializable summaries.
    pub fn list(&self) -> Vec<QuestionSummary> {
        let queue = self.inner.lock().expect("question queue lock poisoned");
        queue
            .iter()
            .map(|pq| QuestionSummary {
                id: pq.id.clone(),
                question: pq.question.clone(),
                choices: pq.choices.clone(),
                kind: pq.kind.clone(),
            })
            .collect()
    }

    /// Remove a question by ID and return it (with its answer channel).
    ///
    /// Returns `None` if no question with the given ID exists.
    pub fn take(&self, question_id: &str) -> Option<PendingQuestion> {
        let mut queue = self.inner.lock().expect("question queue lock poisoned");
        if let Some(pos) = queue.iter().position(|pq| pq.id == question_id) {
            queue.remove(pos)
        } else {
            None
        }
    }

    /// Return the number of pending questions.
    pub fn len(&self) -> usize {
        let queue = self.inner.lock().expect("question queue lock poisoned");
        queue.len()
    }

    /// Return true if there are no pending questions.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl Default for QuestionQueue {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// API request/response types
// ---------------------------------------------------------------------------

/// Serializable summary of a pending question (no internal channels exposed).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct QuestionSummary {
    /// Unique identifier for this question.
    pub id: String,
    /// The question text.
    pub question: String,
    /// Available choices (empty for free-form and approval questions).
    pub choices: Vec<String>,
    /// What kind of question this is.
    pub kind: QuestionKind,
}

/// Response body for GET /api/v1/questions.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ListQuestionsResponse {
    /// All currently pending questions.
    pub questions: Vec<QuestionSummary>,
}

/// Request body for POST /api/v1/questions/{id}/answer.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AnswerQuestionRequest {
    /// The answer to the question.
    pub answer: String,
}

/// Response body for POST /api/v1/questions/{id}/answer.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AnswerQuestionResponse {
    /// Whether the answer was successfully delivered.
    pub success: bool,
    /// Optional error message if the answer could not be delivered.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

// ---------------------------------------------------------------------------
// HttpInterviewer
// ---------------------------------------------------------------------------

/// An `Interviewer` implementation that queues questions for HTTP clients.
///
/// When a pipeline node calls `ask()`, `ask_with_options()`, or `approve()`,
/// the question is placed into a shared `QuestionQueue` and the call blocks
/// (via `.await`) until an HTTP client submits an answer through the
/// corresponding REST endpoint.
#[derive(Debug, Clone)]
pub struct HttpInterviewer {
    queue: QuestionQueue,
}

impl HttpInterviewer {
    /// Create a new HttpInterviewer with its own question queue.
    pub fn new() -> Self {
        Self {
            queue: QuestionQueue::new(),
        }
    }

    /// Create a new HttpInterviewer backed by the given shared queue.
    pub fn with_queue(queue: QuestionQueue) -> Self {
        Self { queue }
    }

    /// Return a reference to the underlying question queue.
    ///
    /// HTTP endpoint handlers use this to list questions and deliver answers.
    pub fn queue(&self) -> &QuestionQueue {
        &self.queue
    }

    /// List all pending questions as a `ListQuestionsResponse`.
    pub fn list_questions(&self) -> ListQuestionsResponse {
        ListQuestionsResponse {
            questions: self.queue.list(),
        }
    }

    /// Submit an answer for a pending question by its ID.
    ///
    /// Returns an `AnswerQuestionResponse` indicating success or failure.
    pub fn answer_question(&self, question_id: &str, answer: &str) -> AnswerQuestionResponse {
        match self.queue.take(question_id) {
            Some(mut pending) => {
                if let Some(tx) = pending.answer_tx.take() {
                    match tx.send(answer.to_string()) {
                        Ok(()) => AnswerQuestionResponse {
                            success: true,
                            error: None,
                        },
                        Err(_) => AnswerQuestionResponse {
                            success: false,
                            error: Some(
                                "receiver dropped; question may have timed out".to_string(),
                            ),
                        },
                    }
                } else {
                    AnswerQuestionResponse {
                        success: false,
                        error: Some("answer channel already consumed".to_string()),
                    }
                }
            }
            None => AnswerQuestionResponse {
                success: false,
                error: Some(format!("question not found: {question_id}")),
            },
        }
    }

    /// Return the canonical API routes for the HTTP interviewer endpoints.
    pub fn routes() -> Vec<Route> {
        vec![
            Route {
                method: HttpMethod::Get,
                path: "/api/v1/questions".to_string(),
                description: "List pending interview questions".to_string(),
            },
            Route {
                method: HttpMethod::Post,
                path: "/api/v1/questions/{id}/answer".to_string(),
                description: "Submit an answer to a pending question".to_string(),
            },
        ]
    }

    /// Internal helper: enqueue a question and return a receiver for the answer.
    fn enqueue(
        &self,
        question: &str,
        choices: Vec<String>,
        kind: QuestionKind,
    ) -> oneshot::Receiver<String> {
        let (tx, rx) = oneshot::channel();
        let id = uuid::Uuid::new_v4().to_string();
        let pending = PendingQuestion {
            id,
            question: question.to_string(),
            choices,
            kind,
            created_at: Instant::now(),
            answer_tx: Some(tx),
        };
        self.queue.push(pending);
        rx
    }
}

impl Default for HttpInterviewer {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl Interviewer for HttpInterviewer {
    async fn ask(&self, question: &str, _context: &Context) -> Result<String, InterviewerError> {
        let rx = self.enqueue(question, vec![], QuestionKind::FreeForm);
        rx.await
            .map_err(|_| InterviewerError::Other("answer channel closed".to_string()))
    }

    async fn ask_with_options(
        &self,
        question: &str,
        options: &[String],
        _context: &Context,
    ) -> Result<String, InterviewerError> {
        let rx = self.enqueue(question, options.to_vec(), QuestionKind::MultipleChoice);
        rx.await
            .map_err(|_| InterviewerError::Other("answer channel closed".to_string()))
    }

    async fn approve(&self, message: &str, _context: &Context) -> Result<bool, InterviewerError> {
        let rx = self.enqueue(message, vec![], QuestionKind::Approval);
        let answer = rx
            .await
            .map_err(|_| InterviewerError::Other("answer channel closed".to_string()))?;
        let normalized = answer.trim().to_lowercase();
        Ok(normalized == "yes" || normalized == "y" || normalized == "true")
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // ---------------------------------------------------------------
    // QuestionKind serde tests
    // ---------------------------------------------------------------

    #[test]
    fn question_kind_serializes_as_snake_case() {
        assert_eq!(
            serde_json::to_string(&QuestionKind::FreeForm).unwrap(),
            "\"free_form\""
        );
        assert_eq!(
            serde_json::to_string(&QuestionKind::MultipleChoice).unwrap(),
            "\"multiple_choice\""
        );
        assert_eq!(
            serde_json::to_string(&QuestionKind::Approval).unwrap(),
            "\"approval\""
        );
    }

    #[test]
    fn question_kind_serde_roundtrip() {
        for kind in [
            QuestionKind::FreeForm,
            QuestionKind::MultipleChoice,
            QuestionKind::Approval,
        ] {
            let json_str = serde_json::to_string(&kind).unwrap();
            let restored: QuestionKind = serde_json::from_str(&json_str).unwrap();
            assert_eq!(kind, restored);
        }
    }

    // ---------------------------------------------------------------
    // QuestionQueue tests
    // ---------------------------------------------------------------

    #[test]
    fn queue_starts_empty() {
        let queue = QuestionQueue::new();
        assert!(queue.is_empty());
        assert_eq!(queue.len(), 0);
        assert!(queue.list().is_empty());
    }

    #[test]
    fn queue_push_and_list() {
        let queue = QuestionQueue::new();
        let (tx, _rx) = oneshot::channel();
        let pq = PendingQuestion {
            id: "q1".to_string(),
            question: "What color?".to_string(),
            choices: vec!["red".to_string(), "blue".to_string()],
            kind: QuestionKind::MultipleChoice,
            created_at: Instant::now(),
            answer_tx: Some(tx),
        };
        queue.push(pq);

        assert_eq!(queue.len(), 1);
        assert!(!queue.is_empty());

        let summaries = queue.list();
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].id, "q1");
        assert_eq!(summaries[0].question, "What color?");
        assert_eq!(summaries[0].choices, vec!["red", "blue"]);
        assert_eq!(summaries[0].kind, QuestionKind::MultipleChoice);
    }

    #[test]
    fn queue_take_removes_question() {
        let queue = QuestionQueue::new();
        let (tx, _rx) = oneshot::channel();
        let pq = PendingQuestion {
            id: "q1".to_string(),
            question: "Approve?".to_string(),
            choices: vec![],
            kind: QuestionKind::Approval,
            created_at: Instant::now(),
            answer_tx: Some(tx),
        };
        queue.push(pq);

        let taken = queue.take("q1");
        assert!(taken.is_some());
        assert_eq!(taken.unwrap().id, "q1");
        assert!(queue.is_empty());
    }

    #[test]
    fn queue_take_nonexistent_returns_none() {
        let queue = QuestionQueue::new();
        assert!(queue.take("nonexistent").is_none());
    }

    #[test]
    fn queue_take_correct_question_from_multiple() {
        let queue = QuestionQueue::new();

        for i in 1..=3 {
            let (tx, _rx) = oneshot::channel();
            let pq = PendingQuestion {
                id: format!("q{i}"),
                question: format!("Question {i}"),
                choices: vec![],
                kind: QuestionKind::FreeForm,
                created_at: Instant::now(),
                answer_tx: Some(tx),
            };
            queue.push(pq);
        }

        assert_eq!(queue.len(), 3);

        // Take the middle one.
        let taken = queue.take("q2");
        assert!(taken.is_some());
        assert_eq!(taken.unwrap().id, "q2");
        assert_eq!(queue.len(), 2);

        // Remaining should be q1 and q3.
        let summaries = queue.list();
        let ids: Vec<&str> = summaries.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, vec!["q1", "q3"]);
    }

    #[test]
    fn queue_default_is_empty() {
        let queue = QuestionQueue::default();
        assert!(queue.is_empty());
    }

    // ---------------------------------------------------------------
    // API type serde tests
    // ---------------------------------------------------------------

    #[test]
    fn question_summary_serde_roundtrip() {
        let summary = QuestionSummary {
            id: "abc-123".to_string(),
            question: "Pick a color".to_string(),
            choices: vec!["red".to_string(), "blue".to_string()],
            kind: QuestionKind::MultipleChoice,
        };

        let json_str = serde_json::to_string(&summary).unwrap();
        let restored: QuestionSummary = serde_json::from_str(&json_str).unwrap();
        assert_eq!(summary, restored);
    }

    #[test]
    fn list_questions_response_serde_roundtrip_empty() {
        let response = ListQuestionsResponse { questions: vec![] };

        let json_str = serde_json::to_string(&response).unwrap();
        let restored: ListQuestionsResponse = serde_json::from_str(&json_str).unwrap();
        assert_eq!(response, restored);
    }

    #[test]
    fn list_questions_response_serde_roundtrip_with_questions() {
        let response = ListQuestionsResponse {
            questions: vec![
                QuestionSummary {
                    id: "q1".to_string(),
                    question: "Name?".to_string(),
                    choices: vec![],
                    kind: QuestionKind::FreeForm,
                },
                QuestionSummary {
                    id: "q2".to_string(),
                    question: "Deploy?".to_string(),
                    choices: vec![],
                    kind: QuestionKind::Approval,
                },
            ],
        };

        let json_str = serde_json::to_string(&response).unwrap();
        let restored: ListQuestionsResponse = serde_json::from_str(&json_str).unwrap();
        assert_eq!(response, restored);
    }

    #[test]
    fn answer_question_request_serde_roundtrip() {
        let request = AnswerQuestionRequest {
            answer: "42".to_string(),
        };

        let json_str = serde_json::to_string(&request).unwrap();
        let restored: AnswerQuestionRequest = serde_json::from_str(&json_str).unwrap();
        assert_eq!(request, restored);
    }

    #[test]
    fn answer_question_response_success_serde() {
        let response = AnswerQuestionResponse {
            success: true,
            error: None,
        };

        let json_str = serde_json::to_string(&response).unwrap();
        assert!(!json_str.contains("error"));
        let restored: AnswerQuestionResponse = serde_json::from_str(&json_str).unwrap();
        assert_eq!(response, restored);
    }

    #[test]
    fn answer_question_response_failure_serde() {
        let response = AnswerQuestionResponse {
            success: false,
            error: Some("question not found".to_string()),
        };

        let json_str = serde_json::to_string(&response).unwrap();
        assert!(json_str.contains("error"));
        let restored: AnswerQuestionResponse = serde_json::from_str(&json_str).unwrap();
        assert_eq!(response, restored);
    }

    // ---------------------------------------------------------------
    // HttpInterviewer construction tests
    // ---------------------------------------------------------------

    #[test]
    fn http_interviewer_new_has_empty_queue() {
        let iv = HttpInterviewer::new();
        assert!(iv.queue().is_empty());
    }

    #[test]
    fn http_interviewer_default_has_empty_queue() {
        let iv = HttpInterviewer::default();
        assert!(iv.queue().is_empty());
    }

    #[test]
    fn http_interviewer_with_shared_queue() {
        let queue = QuestionQueue::new();
        let (tx, _rx) = oneshot::channel();
        queue.push(PendingQuestion {
            id: "pre-existing".to_string(),
            question: "Already here?".to_string(),
            choices: vec![],
            kind: QuestionKind::FreeForm,
            created_at: Instant::now(),
            answer_tx: Some(tx),
        });

        let iv = HttpInterviewer::with_queue(queue);
        assert_eq!(iv.queue().len(), 1);
    }

    // ---------------------------------------------------------------
    // HttpInterviewer::list_questions tests
    // ---------------------------------------------------------------

    #[test]
    fn list_questions_returns_empty_initially() {
        let iv = HttpInterviewer::new();
        let response = iv.list_questions();
        assert!(response.questions.is_empty());
    }

    // ---------------------------------------------------------------
    // HttpInterviewer::answer_question tests
    // ---------------------------------------------------------------

    #[test]
    fn answer_question_not_found() {
        let iv = HttpInterviewer::new();
        let response = iv.answer_question("nonexistent", "answer");
        assert!(!response.success);
        assert!(response.error.as_ref().unwrap().contains("not found"));
    }

    #[tokio::test]
    async fn answer_question_delivers_answer() {
        let iv = HttpInterviewer::new();

        // Manually enqueue a question with a known ID.
        let (tx, rx) = oneshot::channel();
        iv.queue().push(PendingQuestion {
            id: "test-q".to_string(),
            question: "What?".to_string(),
            choices: vec![],
            kind: QuestionKind::FreeForm,
            created_at: Instant::now(),
            answer_tx: Some(tx),
        });

        // Answer it.
        let response = iv.answer_question("test-q", "the answer");
        assert!(response.success);
        assert!(response.error.is_none());

        // The receiver should have the answer.
        let answer = rx.await.unwrap();
        assert_eq!(answer, "the answer");
    }

    #[test]
    fn answer_question_removes_from_queue() {
        let iv = HttpInterviewer::new();

        let (tx, _rx) = oneshot::channel();
        iv.queue().push(PendingQuestion {
            id: "test-q".to_string(),
            question: "What?".to_string(),
            choices: vec![],
            kind: QuestionKind::FreeForm,
            created_at: Instant::now(),
            answer_tx: Some(tx),
        });

        assert_eq!(iv.queue().len(), 1);
        iv.answer_question("test-q", "answer");
        assert_eq!(iv.queue().len(), 0);
    }

    #[test]
    fn answer_question_twice_fails_second_time() {
        let iv = HttpInterviewer::new();

        let (tx, _rx) = oneshot::channel();
        iv.queue().push(PendingQuestion {
            id: "test-q".to_string(),
            question: "What?".to_string(),
            choices: vec![],
            kind: QuestionKind::FreeForm,
            created_at: Instant::now(),
            answer_tx: Some(tx),
        });

        let first = iv.answer_question("test-q", "first");
        assert!(first.success);

        let second = iv.answer_question("test-q", "second");
        assert!(!second.success);
        assert!(second.error.as_ref().unwrap().contains("not found"));
    }

    // ---------------------------------------------------------------
    // HttpInterviewer Interviewer trait: ask()
    // ---------------------------------------------------------------

    #[tokio::test]
    async fn ask_enqueues_question_and_awaits_answer() {
        let iv = HttpInterviewer::new();
        let iv_clone = iv.clone();
        let ctx = Context::new();

        // Spawn a task that answers the question after a brief delay.
        let handle = tokio::spawn(async move {
            // Wait for the question to appear.
            loop {
                if !iv_clone.queue().is_empty() {
                    break;
                }
                tokio::task::yield_now().await;
            }

            let questions = iv_clone.list_questions();
            assert_eq!(questions.questions.len(), 1);
            assert_eq!(questions.questions[0].question, "What is your name?");
            assert_eq!(questions.questions[0].kind, QuestionKind::FreeForm);
            assert!(questions.questions[0].choices.is_empty());

            let id = &questions.questions[0].id;
            let response = iv_clone.answer_question(id, "Turbosaurus Rex");
            assert!(response.success);
        });

        let answer = iv.ask("What is your name?", &ctx).await.unwrap();
        assert_eq!(answer, "Turbosaurus Rex");

        handle.await.unwrap();
    }

    // ---------------------------------------------------------------
    // HttpInterviewer Interviewer trait: ask_with_options()
    // ---------------------------------------------------------------

    #[tokio::test]
    async fn ask_with_options_enqueues_with_choices() {
        let iv = HttpInterviewer::new();
        let iv_clone = iv.clone();
        let ctx = Context::new();

        let handle = tokio::spawn(async move {
            loop {
                if !iv_clone.queue().is_empty() {
                    break;
                }
                tokio::task::yield_now().await;
            }

            let questions = iv_clone.list_questions();
            assert_eq!(questions.questions.len(), 1);
            assert_eq!(questions.questions[0].question, "Pick a color");
            assert_eq!(questions.questions[0].kind, QuestionKind::MultipleChoice);
            assert_eq!(questions.questions[0].choices, vec!["red", "blue", "green"]);

            let id = &questions.questions[0].id;
            let response = iv_clone.answer_question(id, "blue");
            assert!(response.success);
        });

        let options = vec!["red".to_string(), "blue".to_string(), "green".to_string()];
        let answer = iv
            .ask_with_options("Pick a color", &options, &ctx)
            .await
            .unwrap();
        assert_eq!(answer, "blue");

        handle.await.unwrap();
    }

    // ---------------------------------------------------------------
    // HttpInterviewer Interviewer trait: approve()
    // ---------------------------------------------------------------

    #[tokio::test]
    async fn approve_yes_returns_true() {
        let iv = HttpInterviewer::new();
        let iv_clone = iv.clone();
        let ctx = Context::new();

        let handle = tokio::spawn(async move {
            loop {
                if !iv_clone.queue().is_empty() {
                    break;
                }
                tokio::task::yield_now().await;
            }

            let questions = iv_clone.list_questions();
            assert_eq!(questions.questions[0].kind, QuestionKind::Approval);

            let id = &questions.questions[0].id;
            iv_clone.answer_question(id, "yes");
        });

        let approved = iv.approve("Deploy to prod?", &ctx).await.unwrap();
        assert!(approved);

        handle.await.unwrap();
    }

    #[tokio::test]
    async fn approve_no_returns_false() {
        let iv = HttpInterviewer::new();
        let iv_clone = iv.clone();
        let ctx = Context::new();

        let handle = tokio::spawn(async move {
            loop {
                if !iv_clone.queue().is_empty() {
                    break;
                }
                tokio::task::yield_now().await;
            }

            let id = &iv_clone.list_questions().questions[0].id;
            iv_clone.answer_question(id, "no");
        });

        let approved = iv.approve("Deploy to prod?", &ctx).await.unwrap();
        assert!(!approved);

        handle.await.unwrap();
    }

    #[tokio::test]
    async fn approve_case_insensitive_y() {
        let iv = HttpInterviewer::new();
        let iv_clone = iv.clone();
        let ctx = Context::new();

        let handle = tokio::spawn(async move {
            loop {
                if !iv_clone.queue().is_empty() {
                    break;
                }
                tokio::task::yield_now().await;
            }

            let id = &iv_clone.list_questions().questions[0].id;
            iv_clone.answer_question(id, "Y");
        });

        let approved = iv.approve("Continue?", &ctx).await.unwrap();
        assert!(approved);

        handle.await.unwrap();
    }

    #[tokio::test]
    async fn approve_true_string_returns_true() {
        let iv = HttpInterviewer::new();
        let iv_clone = iv.clone();
        let ctx = Context::new();

        let handle = tokio::spawn(async move {
            loop {
                if !iv_clone.queue().is_empty() {
                    break;
                }
                tokio::task::yield_now().await;
            }

            let id = &iv_clone.list_questions().questions[0].id;
            iv_clone.answer_question(id, "TRUE");
        });

        let approved = iv.approve("Approve?", &ctx).await.unwrap();
        assert!(approved);

        handle.await.unwrap();
    }

    #[tokio::test]
    async fn approve_arbitrary_string_returns_false() {
        let iv = HttpInterviewer::new();
        let iv_clone = iv.clone();
        let ctx = Context::new();

        let handle = tokio::spawn(async move {
            loop {
                if !iv_clone.queue().is_empty() {
                    break;
                }
                tokio::task::yield_now().await;
            }

            let id = &iv_clone.list_questions().questions[0].id;
            iv_clone.answer_question(id, "maybe");
        });

        let approved = iv.approve("Continue?", &ctx).await.unwrap();
        assert!(!approved);

        handle.await.unwrap();
    }

    // ---------------------------------------------------------------
    // Channel closed error path
    // ---------------------------------------------------------------

    #[tokio::test]
    async fn ask_returns_error_when_channel_closed() {
        let iv = HttpInterviewer::new();
        let ctx = Context::new();

        // Manually enqueue a question, then drop the sender to simulate
        // channel closure. We need to take the question back and drop its tx.
        let (tx, _rx_unused) = oneshot::channel::<String>();
        drop(tx);

        // Directly push a question with no sender to simulate the scenario.
        // Actually, we need the receiver that ask() would hold. Let's do it
        // the proper way: enqueue via the trait, then drop the pending question.
        let iv_clone = iv.clone();

        let handle = tokio::spawn(async move {
            loop {
                if !iv_clone.queue().is_empty() {
                    break;
                }
                tokio::task::yield_now().await;
            }

            // Take the question and drop it without sending, closing the channel.
            let id = &iv_clone.list_questions().questions[0].id;
            let taken = iv_clone.queue().take(id);
            drop(taken);
        });

        let result = iv.ask("Will this fail?", &ctx).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("answer channel closed"));

        handle.await.unwrap();
    }

    // ---------------------------------------------------------------
    // Multiple concurrent questions
    // ---------------------------------------------------------------

    #[tokio::test]
    async fn multiple_concurrent_questions() {
        let iv = HttpInterviewer::new();
        let ctx = Context::new();

        let iv1 = iv.clone();
        let ctx1 = ctx.clone();
        let ask1 = tokio::spawn(async move { iv1.ask("Question 1?", &ctx1).await });

        let iv2 = iv.clone();
        let ctx2 = ctx.clone();
        let ask2 = tokio::spawn(async move { iv2.ask("Question 2?", &ctx2).await });

        // Wait for both questions to be enqueued.
        loop {
            if iv.queue().len() >= 2 {
                break;
            }
            tokio::task::yield_now().await;
        }

        let questions = iv.list_questions();
        assert_eq!(questions.questions.len(), 2);

        // Answer them both.
        for q in &questions.questions {
            let answer = if q.question == "Question 1?" {
                "Answer 1"
            } else {
                "Answer 2"
            };
            let resp = iv.answer_question(&q.id, answer);
            assert!(resp.success);
        }

        let r1 = ask1.await.unwrap().unwrap();
        let r2 = ask2.await.unwrap().unwrap();

        assert_eq!(r1, "Answer 1");
        assert_eq!(r2, "Answer 2");
    }

    // ---------------------------------------------------------------
    // Routes tests
    // ---------------------------------------------------------------

    #[test]
    fn routes_returns_two_endpoints() {
        let routes = HttpInterviewer::routes();
        assert_eq!(routes.len(), 2);
    }

    #[test]
    fn routes_has_list_questions_endpoint() {
        let routes = HttpInterviewer::routes();
        let list = routes
            .iter()
            .find(|r| r.method == HttpMethod::Get && r.path == "/api/v1/questions")
            .expect("should have GET /api/v1/questions");
        assert!(!list.description.is_empty());
    }

    #[test]
    fn routes_has_answer_question_endpoint() {
        let routes = HttpInterviewer::routes();
        let answer = routes
            .iter()
            .find(|r| r.method == HttpMethod::Post && r.path == "/api/v1/questions/{id}/answer")
            .expect("should have POST /api/v1/questions/{id}/answer");
        assert!(!answer.description.is_empty());
    }

    // ---------------------------------------------------------------
    // PendingQuestion Debug implementation
    // ---------------------------------------------------------------

    #[test]
    fn pending_question_debug_format() {
        let (tx, _rx) = oneshot::channel();
        let pq = PendingQuestion {
            id: "debug-test".to_string(),
            question: "Debug?".to_string(),
            choices: vec![],
            kind: QuestionKind::FreeForm,
            created_at: Instant::now(),
            answer_tx: Some(tx),
        };

        let debug_str = format!("{pq:?}");
        assert!(debug_str.contains("debug-test"));
        assert!(debug_str.contains("has_answer_tx"));
        assert!(debug_str.contains("true"));
    }

    #[test]
    fn pending_question_debug_without_sender() {
        let pq = PendingQuestion {
            id: "no-sender".to_string(),
            question: "No sender?".to_string(),
            choices: vec![],
            kind: QuestionKind::FreeForm,
            created_at: Instant::now(),
            answer_tx: None,
        };

        let debug_str = format!("{pq:?}");
        assert!(debug_str.contains("false"));
    }

    // ---------------------------------------------------------------
    // Thread safety: QuestionQueue across threads
    // ---------------------------------------------------------------

    #[test]
    fn queue_is_thread_safe() {
        let queue = QuestionQueue::new();
        let handles: Vec<_> = (0..5)
            .map(|i| {
                let q = queue.clone();
                std::thread::spawn(move || {
                    let (tx, _rx) = oneshot::channel();
                    q.push(PendingQuestion {
                        id: format!("thread-q-{i}"),
                        question: format!("Thread question {i}"),
                        choices: vec![],
                        kind: QuestionKind::FreeForm,
                        created_at: Instant::now(),
                        answer_tx: Some(tx),
                    });
                })
            })
            .collect();

        for h in handles {
            h.join().unwrap();
        }

        assert_eq!(queue.len(), 5);
    }

    // ---------------------------------------------------------------
    // Integration: HttpInterviewer + answer_question with receiver dropped
    // ---------------------------------------------------------------

    #[test]
    fn answer_question_with_dropped_receiver_reports_error() {
        let iv = HttpInterviewer::new();

        let (tx, rx) = oneshot::channel::<String>();
        iv.queue().push(PendingQuestion {
            id: "drop-test".to_string(),
            question: "Will receiver be dropped?".to_string(),
            choices: vec![],
            kind: QuestionKind::FreeForm,
            created_at: Instant::now(),
            answer_tx: Some(tx),
        });

        // Drop the receiver before answering.
        drop(rx);

        let response = iv.answer_question("drop-test", "too late");
        assert!(!response.success);
        assert!(
            response
                .error
                .as_ref()
                .unwrap()
                .contains("receiver dropped")
        );
    }
}
