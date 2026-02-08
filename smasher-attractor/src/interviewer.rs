// ABOUTME: Human-in-the-loop trait and implementations for pipeline interaction points.
// ABOUTME: Supports auto-approve, queue-based, callback, console, and recording interviewer strategies.

use std::collections::VecDeque;
use std::io::{BufRead, Write};
use std::sync::{Arc, Mutex};

use serde_json::json;

use crate::graph::{GraphNode, NodeAttrValue, NodeType};
use crate::handler::{Handler, HandlerError};
use crate::state::{Context, Outcome};

/// Errors that can occur during an interview interaction.
#[derive(Debug, thiserror::Error)]
pub enum InterviewerError {
    #[error("interview cancelled by user")]
    Cancelled,
    #[error("interview timed out")]
    Timeout,
    #[error("interviewer error: {0}")]
    Other(String),
}

/// Abstraction for getting human input during pipeline execution.
#[async_trait::async_trait]
pub trait Interviewer: Send + Sync {
    /// Present a question to the human and get a response.
    async fn ask(&self, question: &str, context: &Context) -> Result<String, InterviewerError>;

    /// Present a question with predefined options.
    async fn ask_with_options(
        &self,
        question: &str,
        options: &[String],
        context: &Context,
    ) -> Result<String, InterviewerError>;

    /// Request approval (yes/no) from the human.
    async fn approve(&self, message: &str, context: &Context) -> Result<bool, InterviewerError>;
}

// ---------------------------------------------------------------------------
// AutoApproveInterviewer
// ---------------------------------------------------------------------------

/// An interviewer that always approves and returns a configurable default response.
pub struct AutoApproveInterviewer {
    default_response: String,
}

impl AutoApproveInterviewer {
    /// Create a new AutoApproveInterviewer with the default response "approved".
    pub fn new() -> Self {
        Self {
            default_response: "approved".to_string(),
        }
    }

    /// Create a new AutoApproveInterviewer with a custom default response.
    pub fn with_response(response: impl Into<String>) -> Self {
        Self {
            default_response: response.into(),
        }
    }
}

impl Default for AutoApproveInterviewer {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl Interviewer for AutoApproveInterviewer {
    async fn ask(&self, _question: &str, _context: &Context) -> Result<String, InterviewerError> {
        Ok(self.default_response.clone())
    }

    async fn ask_with_options(
        &self,
        _question: &str,
        options: &[String],
        _context: &Context,
    ) -> Result<String, InterviewerError> {
        Ok(options
            .first()
            .cloned()
            .unwrap_or_else(|| self.default_response.clone()))
    }

    async fn approve(
        &self,
        _message: &str,
        _context: &Context,
    ) -> Result<bool, InterviewerError> {
        Ok(true)
    }
}

// ---------------------------------------------------------------------------
// QueueInterviewer
// ---------------------------------------------------------------------------

/// An interviewer that uses pre-loaded responses from a FIFO queue.
pub struct QueueInterviewer {
    responses: Arc<Mutex<VecDeque<String>>>,
    approvals: Arc<Mutex<VecDeque<bool>>>,
}

impl QueueInterviewer {
    /// Create a new QueueInterviewer with empty queues.
    pub fn new() -> Self {
        Self {
            responses: Arc::new(Mutex::new(VecDeque::new())),
            approvals: Arc::new(Mutex::new(VecDeque::new())),
        }
    }

    /// Enqueue a text response.
    pub fn push_response(&self, response: impl Into<String>) {
        let mut queue = self.responses.lock().expect("response queue lock poisoned");
        queue.push_back(response.into());
    }

    /// Enqueue an approval response.
    pub fn push_approval(&self, approved: bool) {
        let mut queue = self.approvals.lock().expect("approval queue lock poisoned");
        queue.push_back(approved);
    }
}

impl Default for QueueInterviewer {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl Interviewer for QueueInterviewer {
    async fn ask(&self, _question: &str, _context: &Context) -> Result<String, InterviewerError> {
        let mut queue = self
            .responses
            .lock()
            .map_err(|e| InterviewerError::Other(format!("lock poisoned: {e}")))?;
        queue
            .pop_front()
            .ok_or_else(|| InterviewerError::Other("response queue is empty".to_string()))
    }

    async fn ask_with_options(
        &self,
        _question: &str,
        _options: &[String],
        _context: &Context,
    ) -> Result<String, InterviewerError> {
        let mut queue = self
            .responses
            .lock()
            .map_err(|e| InterviewerError::Other(format!("lock poisoned: {e}")))?;
        queue
            .pop_front()
            .ok_or_else(|| InterviewerError::Other("response queue is empty".to_string()))
    }

    async fn approve(
        &self,
        _message: &str,
        _context: &Context,
    ) -> Result<bool, InterviewerError> {
        let mut queue = self
            .approvals
            .lock()
            .map_err(|e| InterviewerError::Other(format!("lock poisoned: {e}")))?;
        queue
            .pop_front()
            .ok_or_else(|| InterviewerError::Other("approval queue is empty".to_string()))
    }
}

// ---------------------------------------------------------------------------
// CallbackInterviewer
// ---------------------------------------------------------------------------

/// An interviewer that delegates to user-supplied closures.
pub struct CallbackInterviewer {
    ask_fn: Box<dyn Fn(&str) -> String + Send + Sync>,
    approve_fn: Box<dyn Fn(&str) -> bool + Send + Sync>,
}

impl CallbackInterviewer {
    /// Create a new CallbackInterviewer with the given closures.
    pub fn new(
        ask_fn: impl Fn(&str) -> String + Send + Sync + 'static,
        approve_fn: impl Fn(&str) -> bool + Send + Sync + 'static,
    ) -> Self {
        Self {
            ask_fn: Box::new(ask_fn),
            approve_fn: Box::new(approve_fn),
        }
    }
}

#[async_trait::async_trait]
impl Interviewer for CallbackInterviewer {
    async fn ask(&self, question: &str, _context: &Context) -> Result<String, InterviewerError> {
        Ok((self.ask_fn)(question))
    }

    async fn ask_with_options(
        &self,
        question: &str,
        _options: &[String],
        _context: &Context,
    ) -> Result<String, InterviewerError> {
        Ok((self.ask_fn)(question))
    }

    async fn approve(
        &self,
        message: &str,
        _context: &Context,
    ) -> Result<bool, InterviewerError> {
        Ok((self.approve_fn)(message))
    }
}

// ---------------------------------------------------------------------------
// ConsoleInterviewer
// ---------------------------------------------------------------------------

/// An interviewer that reads from an input reader and writes to an output writer.
/// Useful for CLI-based human-in-the-loop interaction.
pub struct ConsoleInterviewer {
    input: Arc<Mutex<Box<dyn BufRead + Send>>>,
    output: Arc<Mutex<Box<dyn Write + Send>>>,
}

impl ConsoleInterviewer {
    /// Create a new ConsoleInterviewer with the given input reader and output writer.
    pub fn new(input: impl BufRead + Send + 'static, output: impl Write + Send + 'static) -> Self {
        Self {
            input: Arc::new(Mutex::new(Box::new(input))),
            output: Arc::new(Mutex::new(Box::new(output))),
        }
    }

    /// Create a ConsoleInterviewer that reads from stdin and writes to stdout.
    pub fn from_stdio() -> Self {
        Self::new(std::io::BufReader::new(std::io::stdin()), std::io::stdout())
    }

    /// Write a string to the output and flush it.
    fn write_and_flush(&self, text: &str) -> Result<(), InterviewerError> {
        let mut out = self
            .output
            .lock()
            .map_err(|e| InterviewerError::Other(format!("output lock poisoned: {e}")))?;
        out.write_all(text.as_bytes())
            .map_err(|e| InterviewerError::Other(format!("write error: {e}")))?;
        out.flush()
            .map_err(|e| InterviewerError::Other(format!("flush error: {e}")))?;
        Ok(())
    }

    /// Read a line from the input, trimming the trailing newline.
    fn read_line(&self) -> Result<String, InterviewerError> {
        let mut inp = self
            .input
            .lock()
            .map_err(|e| InterviewerError::Other(format!("input lock poisoned: {e}")))?;
        let mut line = String::new();
        inp.read_line(&mut line)
            .map_err(|e| InterviewerError::Other(format!("read error: {e}")))?;
        Ok(line.trim_end_matches('\n').trim_end_matches('\r').to_string())
    }
}

#[async_trait::async_trait]
impl Interviewer for ConsoleInterviewer {
    async fn ask(&self, question: &str, _context: &Context) -> Result<String, InterviewerError> {
        self.write_and_flush(question)?;
        self.read_line()
    }

    async fn ask_with_options(
        &self,
        question: &str,
        options: &[String],
        _context: &Context,
    ) -> Result<String, InterviewerError> {
        let mut prompt = format!("{question}\n");
        for (i, option) in options.iter().enumerate() {
            prompt.push_str(&format!("  {}. {}\n", i + 1, option));
        }
        prompt.push_str("Choice: ");
        self.write_and_flush(&prompt)?;
        let input = self.read_line()?;

        // If the input is a valid number matching an option index, return that option.
        if let Ok(num) = input.trim().parse::<usize>()
            && num >= 1
            && num <= options.len()
        {
            return Ok(options[num - 1].clone());
        }

        // Otherwise return the raw input.
        Ok(input)
    }

    async fn approve(
        &self,
        message: &str,
        _context: &Context,
    ) -> Result<bool, InterviewerError> {
        self.write_and_flush(&format!("{message} (yes/no): "))?;
        let input = self.read_line()?;
        let trimmed = input.trim().to_lowercase();
        Ok(trimmed == "yes" || trimmed == "y")
    }
}

// ---------------------------------------------------------------------------
// RecordingInterviewer
// ---------------------------------------------------------------------------

/// The type of interaction that was recorded.
#[derive(Debug, Clone, PartialEq)]
pub enum InteractionType {
    Ask,
    AskWithOptions,
    Approve,
}

/// A single recorded interview interaction.
#[derive(Debug, Clone)]
pub struct InterviewRecord {
    pub question: String,
    pub response: String,
    pub interaction_type: InteractionType,
}

/// Records all questions and responses from a wrapped Interviewer.
pub struct RecordingInterviewer {
    inner: Arc<dyn Interviewer>,
    recordings: Arc<Mutex<Vec<InterviewRecord>>>,
}

impl RecordingInterviewer {
    /// Create a new RecordingInterviewer wrapping the given inner interviewer.
    pub fn new(inner: Arc<dyn Interviewer>) -> Self {
        Self {
            inner,
            recordings: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Return a clone of all recorded interactions.
    pub fn recordings(&self) -> Vec<InterviewRecord> {
        self.recordings
            .lock()
            .expect("recordings lock poisoned")
            .clone()
    }

    /// Return the number of recorded interactions.
    pub fn recording_count(&self) -> usize {
        self.recordings
            .lock()
            .expect("recordings lock poisoned")
            .len()
    }

    /// Clear all recorded interactions.
    pub fn clear(&self) {
        self.recordings
            .lock()
            .expect("recordings lock poisoned")
            .clear();
    }

    /// Record an interaction.
    fn record(&self, question: &str, response: &str, interaction_type: InteractionType) {
        let mut recordings = self
            .recordings
            .lock()
            .expect("recordings lock poisoned");
        recordings.push(InterviewRecord {
            question: question.to_string(),
            response: response.to_string(),
            interaction_type,
        });
    }
}

#[async_trait::async_trait]
impl Interviewer for RecordingInterviewer {
    async fn ask(&self, question: &str, context: &Context) -> Result<String, InterviewerError> {
        let response = self.inner.ask(question, context).await?;
        self.record(question, &response, InteractionType::Ask);
        Ok(response)
    }

    async fn ask_with_options(
        &self,
        question: &str,
        options: &[String],
        context: &Context,
    ) -> Result<String, InterviewerError> {
        let response = self.inner.ask_with_options(question, options, context).await?;
        self.record(question, &response, InteractionType::AskWithOptions);
        Ok(response)
    }

    async fn approve(
        &self,
        message: &str,
        context: &Context,
    ) -> Result<bool, InterviewerError> {
        let approved = self.inner.approve(message, context).await?;
        let response = if approved { "yes" } else { "no" };
        self.record(message, response, InteractionType::Approve);
        Ok(approved)
    }
}

// ---------------------------------------------------------------------------
// InterviewerHandler
// ---------------------------------------------------------------------------

/// A Handler that wraps an Interviewer, bridging human-in-the-loop interactions
/// into the pipeline's node execution model.
pub struct InterviewerHandler {
    interviewer: Arc<dyn Interviewer>,
}

impl InterviewerHandler {
    /// Create a new InterviewerHandler wrapping the given Interviewer.
    pub fn new(interviewer: Arc<dyn Interviewer>) -> Self {
        Self { interviewer }
    }
}

#[async_trait::async_trait]
impl Handler for InterviewerHandler {
    fn name(&self) -> &str {
        "interviewer"
    }

    async fn execute(
        &self,
        node: &GraphNode,
        context: &Context,
    ) -> Result<Outcome, HandlerError> {
        // Determine the question: explicit attribute first, then label fallback.
        let question = match node.attrs.get("question") {
            Some(NodeAttrValue::String(s)) => s.clone(),
            _ => match &node.label {
                Some(label) => label.clone(),
                None => {
                    return Ok(Outcome::failure("no question specified for interviewer node"));
                }
            },
        };

        // Determine the interaction mode and execute.
        if let Some(NodeAttrValue::Bool(true)) = node.attrs.get("approve") {
            // Approval mode: yes/no question.
            match self.interviewer.approve(&question, context).await {
                Ok(approved) => {
                    let response = if approved { "yes" } else { "no" };
                    context.set(
                        format!("_interview_{}", node.id),
                        json!(response),
                    );
                    Ok(Outcome::success_with(json!({"response": response})))
                }
                Err(InterviewerError::Cancelled) => Ok(Outcome::skip("interview cancelled")),
                Err(e) => Ok(Outcome::failure(e.to_string())),
            }
        } else if let Some(NodeAttrValue::String(opts_str)) = node.attrs.get("options") {
            // Options mode: present predefined choices.
            let options: Vec<String> = opts_str.split(',').map(|s| s.trim().to_string()).collect();
            match self
                .interviewer
                .ask_with_options(&question, &options, context)
                .await
            {
                Ok(response) => {
                    context.set(
                        format!("_interview_{}", node.id),
                        json!(&response),
                    );
                    Ok(Outcome::success_with(json!({"response": response})))
                }
                Err(InterviewerError::Cancelled) => Ok(Outcome::skip("interview cancelled")),
                Err(e) => Ok(Outcome::failure(e.to_string())),
            }
        } else {
            // Free-form question mode.
            match self.interviewer.ask(&question, context).await {
                Ok(response) => {
                    context.set(
                        format!("_interview_{}", node.id),
                        json!(&response),
                    );
                    Ok(Outcome::success_with(json!({"response": response})))
                }
                Err(InterviewerError::Cancelled) => Ok(Outcome::skip("interview cancelled")),
                Err(e) => Ok(Outcome::failure(e.to_string())),
            }
        }
    }

    fn handles(&self, node_type: &NodeType) -> bool {
        matches!(node_type, NodeType::Interviewer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    // -- Test helpers -------------------------------------------------------

    /// Build a minimal GraphNode with the given type and optional attributes.
    fn make_node(id: &str, node_type: NodeType) -> GraphNode {
        GraphNode {
            id: id.to_string(),
            node_type,
            label: None,
            attrs: HashMap::new(),
        }
    }

    /// Build a GraphNode with a label.
    fn make_node_with_label(id: &str, node_type: NodeType, label: &str) -> GraphNode {
        GraphNode {
            id: id.to_string(),
            node_type,
            label: Some(label.to_string()),
            attrs: HashMap::new(),
        }
    }

    /// An interviewer that always returns Cancelled for all methods.
    struct CancellingInterviewer;

    #[async_trait::async_trait]
    impl Interviewer for CancellingInterviewer {
        async fn ask(
            &self,
            _question: &str,
            _context: &Context,
        ) -> Result<String, InterviewerError> {
            Err(InterviewerError::Cancelled)
        }

        async fn ask_with_options(
            &self,
            _question: &str,
            _options: &[String],
            _context: &Context,
        ) -> Result<String, InterviewerError> {
            Err(InterviewerError::Cancelled)
        }

        async fn approve(
            &self,
            _message: &str,
            _context: &Context,
        ) -> Result<bool, InterviewerError> {
            Err(InterviewerError::Cancelled)
        }
    }

    /// An interviewer that always returns a Timeout error.
    struct TimingOutInterviewer;

    #[async_trait::async_trait]
    impl Interviewer for TimingOutInterviewer {
        async fn ask(
            &self,
            _question: &str,
            _context: &Context,
        ) -> Result<String, InterviewerError> {
            Err(InterviewerError::Timeout)
        }

        async fn ask_with_options(
            &self,
            _question: &str,
            _options: &[String],
            _context: &Context,
        ) -> Result<String, InterviewerError> {
            Err(InterviewerError::Timeout)
        }

        async fn approve(
            &self,
            _message: &str,
            _context: &Context,
        ) -> Result<bool, InterviewerError> {
            Err(InterviewerError::Timeout)
        }
    }

    // ---------------------------------------------------------------
    // AutoApproveInterviewer tests
    // ---------------------------------------------------------------

    #[tokio::test]
    async fn auto_approve_ask_returns_default_response() {
        let interviewer = AutoApproveInterviewer::new();
        let ctx = Context::new();
        let response = interviewer.ask("What is your name?", &ctx).await.unwrap();
        assert_eq!(response, "approved");
    }

    #[tokio::test]
    async fn auto_approve_with_custom_response() {
        let interviewer = AutoApproveInterviewer::with_response("custom default");
        let ctx = Context::new();
        let response = interviewer.ask("anything?", &ctx).await.unwrap();
        assert_eq!(response, "custom default");
    }

    #[tokio::test]
    async fn auto_approve_ask_with_options_returns_first_option() {
        let interviewer = AutoApproveInterviewer::new();
        let ctx = Context::new();
        let options = vec!["red".to_string(), "blue".to_string(), "green".to_string()];
        let response = interviewer
            .ask_with_options("Pick a color", &options, &ctx)
            .await
            .unwrap();
        assert_eq!(response, "red");
    }

    #[tokio::test]
    async fn auto_approve_ask_with_empty_options_returns_default() {
        let interviewer = AutoApproveInterviewer::with_response("fallback");
        let ctx = Context::new();
        let options: Vec<String> = vec![];
        let response = interviewer
            .ask_with_options("Pick something", &options, &ctx)
            .await
            .unwrap();
        assert_eq!(response, "fallback");
    }

    #[tokio::test]
    async fn auto_approve_approve_returns_true() {
        let interviewer = AutoApproveInterviewer::new();
        let ctx = Context::new();
        let approved = interviewer.approve("Deploy to prod?", &ctx).await.unwrap();
        assert!(approved);
    }

    // ---------------------------------------------------------------
    // QueueInterviewer tests
    // ---------------------------------------------------------------

    #[tokio::test]
    async fn queue_interviewer_ask_pops_from_queue() {
        let interviewer = QueueInterviewer::new();
        interviewer.push_response("first answer");
        interviewer.push_response("second answer");

        let ctx = Context::new();
        let r1 = interviewer.ask("q1?", &ctx).await.unwrap();
        let r2 = interviewer.ask("q2?", &ctx).await.unwrap();
        assert_eq!(r1, "first answer");
        assert_eq!(r2, "second answer");
    }

    #[tokio::test]
    async fn queue_interviewer_empty_queue_returns_error() {
        let interviewer = QueueInterviewer::new();
        let ctx = Context::new();
        let result = interviewer.ask("question?", &ctx).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("response queue is empty"));
    }

    #[tokio::test]
    async fn queue_interviewer_approval_queue() {
        let interviewer = QueueInterviewer::new();
        interviewer.push_approval(true);
        interviewer.push_approval(false);

        let ctx = Context::new();
        let a1 = interviewer.approve("approve 1?", &ctx).await.unwrap();
        let a2 = interviewer.approve("approve 2?", &ctx).await.unwrap();
        assert!(a1);
        assert!(!a2);
    }

    #[tokio::test]
    async fn queue_interviewer_empty_approval_queue_returns_error() {
        let interviewer = QueueInterviewer::new();
        let ctx = Context::new();
        let result = interviewer.approve("approve?", &ctx).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("approval queue is empty"));
    }

    #[tokio::test]
    async fn queue_interviewer_ask_with_options_pops_from_responses() {
        let interviewer = QueueInterviewer::new();
        interviewer.push_response("option_b");

        let ctx = Context::new();
        let options = vec!["option_a".to_string(), "option_b".to_string()];
        let response = interviewer
            .ask_with_options("choose?", &options, &ctx)
            .await
            .unwrap();
        assert_eq!(response, "option_b");
    }

    #[tokio::test]
    async fn queue_interviewer_thread_safety() {
        let interviewer = Arc::new(QueueInterviewer::new());

        // Push responses from multiple threads.
        let handles: Vec<_> = (0..10)
            .map(|i| {
                let interviewer = Arc::clone(&interviewer);
                std::thread::spawn(move || {
                    interviewer.push_response(format!("response_{i}"));
                })
            })
            .collect();

        for h in handles {
            h.join().unwrap();
        }

        // All 10 responses should be dequeueable.
        let ctx = Context::new();
        let mut responses = Vec::new();
        for _ in 0..10 {
            responses.push(interviewer.ask("q?", &ctx).await.unwrap());
        }
        assert_eq!(responses.len(), 10);

        // The 11th should fail.
        let result = interviewer.ask("q?", &ctx).await;
        assert!(result.is_err());
    }

    // ---------------------------------------------------------------
    // CallbackInterviewer tests
    // ---------------------------------------------------------------

    #[tokio::test]
    async fn callback_interviewer_delegates_ask() {
        let interviewer = CallbackInterviewer::new(
            |q| format!("answer to: {q}"),
            |_| true,
        );
        let ctx = Context::new();
        let response = interviewer.ask("what?", &ctx).await.unwrap();
        assert_eq!(response, "answer to: what?");
    }

    #[tokio::test]
    async fn callback_interviewer_delegates_approve() {
        let interviewer = CallbackInterviewer::new(
            |_| "yes".to_string(),
            |msg| msg.contains("deploy"),
        );
        let ctx = Context::new();

        assert!(interviewer.approve("deploy to prod?", &ctx).await.unwrap());
        assert!(!interviewer.approve("delete everything?", &ctx).await.unwrap());
    }

    #[tokio::test]
    async fn callback_interviewer_ask_with_options_delegates_to_ask_fn() {
        let interviewer = CallbackInterviewer::new(
            |q| format!("chosen for: {q}"),
            |_| false,
        );
        let ctx = Context::new();
        let options = vec!["a".to_string(), "b".to_string()];
        let response = interviewer
            .ask_with_options("pick?", &options, &ctx)
            .await
            .unwrap();
        assert_eq!(response, "chosen for: pick?");
    }

    // ---------------------------------------------------------------
    // InterviewerHandler tests
    // ---------------------------------------------------------------

    #[tokio::test]
    async fn handler_with_question_attribute() {
        let interviewer = Arc::new(AutoApproveInterviewer::new());
        let handler = InterviewerHandler::new(interviewer);

        let mut node = make_node("iv1", NodeType::Interviewer);
        node.attrs.insert(
            "question".to_string(),
            NodeAttrValue::String("What is your favorite color?".to_string()),
        );

        let ctx = Context::new();
        let result = handler.execute(&node, &ctx).await.unwrap();
        assert!(result.is_success());
        match result {
            Outcome::Success { data: Some(data) } => {
                assert_eq!(data["response"], "approved");
            }
            other => panic!("expected success with data, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn handler_with_label_fallback() {
        let interviewer = Arc::new(AutoApproveInterviewer::with_response("yes please"));
        let handler = InterviewerHandler::new(interviewer);

        let node = make_node_with_label("iv2", NodeType::Interviewer, "Do you agree?");

        let ctx = Context::new();
        let result = handler.execute(&node, &ctx).await.unwrap();
        assert!(result.is_success());
        match result {
            Outcome::Success { data: Some(data) } => {
                assert_eq!(data["response"], "yes please");
            }
            other => panic!("expected success with data, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn handler_no_question_or_label_returns_failure() {
        let interviewer = Arc::new(AutoApproveInterviewer::new());
        let handler = InterviewerHandler::new(interviewer);

        let node = make_node("iv_nq", NodeType::Interviewer);

        let ctx = Context::new();
        let result = handler.execute(&node, &ctx).await.unwrap();
        assert!(result.is_failure());
        match result {
            Outcome::Failure { error, .. } => {
                assert!(error.contains("no question specified"));
            }
            other => panic!("expected failure, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn handler_with_options_attribute() {
        let queue = Arc::new(QueueInterviewer::new());
        queue.push_response("blue");
        let handler = InterviewerHandler::new(queue);

        let mut node = make_node("iv3", NodeType::Interviewer);
        node.attrs.insert(
            "question".to_string(),
            NodeAttrValue::String("Pick a color".to_string()),
        );
        node.attrs.insert(
            "options".to_string(),
            NodeAttrValue::String("red, blue, green".to_string()),
        );

        let ctx = Context::new();
        let result = handler.execute(&node, &ctx).await.unwrap();
        assert!(result.is_success());
        match result {
            Outcome::Success { data: Some(data) } => {
                assert_eq!(data["response"], "blue");
            }
            other => panic!("expected success with data, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn handler_with_approve_attribute() {
        let interviewer = Arc::new(AutoApproveInterviewer::new());
        let handler = InterviewerHandler::new(interviewer);

        let mut node = make_node("iv4", NodeType::Interviewer);
        node.attrs.insert(
            "question".to_string(),
            NodeAttrValue::String("Deploy to production?".to_string()),
        );
        node.attrs.insert(
            "approve".to_string(),
            NodeAttrValue::Bool(true),
        );

        let ctx = Context::new();
        let result = handler.execute(&node, &ctx).await.unwrap();
        assert!(result.is_success());
        match result {
            Outcome::Success { data: Some(data) } => {
                assert_eq!(data["response"], "yes");
            }
            other => panic!("expected success with data, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn handler_stores_response_in_context() {
        let queue = Arc::new(QueueInterviewer::new());
        queue.push_response("user input here");
        let handler = InterviewerHandler::new(queue);

        let mut node = make_node("iv5", NodeType::Interviewer);
        node.attrs.insert(
            "question".to_string(),
            NodeAttrValue::String("Tell me something".to_string()),
        );

        let ctx = Context::new();
        handler.execute(&node, &ctx).await.unwrap();

        let stored = ctx.get_string("_interview_iv5");
        assert_eq!(stored, Some("user input here".to_string()));
    }

    #[tokio::test]
    async fn handler_approve_stores_response_in_context() {
        let interviewer = Arc::new(AutoApproveInterviewer::new());
        let handler = InterviewerHandler::new(interviewer);

        let mut node = make_node("iv_app", NodeType::Interviewer);
        node.attrs.insert(
            "question".to_string(),
            NodeAttrValue::String("Approve?".to_string()),
        );
        node.attrs.insert(
            "approve".to_string(),
            NodeAttrValue::Bool(true),
        );

        let ctx = Context::new();
        handler.execute(&node, &ctx).await.unwrap();

        let stored = ctx.get_string("_interview_iv_app");
        assert_eq!(stored, Some("yes".to_string()));
    }

    #[tokio::test]
    async fn handler_cancelled_returns_skip() {
        let interviewer = Arc::new(CancellingInterviewer);
        let handler = InterviewerHandler::new(interviewer);

        let mut node = make_node("iv6", NodeType::Interviewer);
        node.attrs.insert(
            "question".to_string(),
            NodeAttrValue::String("Continue?".to_string()),
        );

        let ctx = Context::new();
        let result = handler.execute(&node, &ctx).await.unwrap();
        match result {
            Outcome::Skip { reason } => {
                assert_eq!(reason, "interview cancelled");
            }
            other => panic!("expected skip, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn handler_timeout_returns_failure() {
        let interviewer = Arc::new(TimingOutInterviewer);
        let handler = InterviewerHandler::new(interviewer);

        let mut node = make_node("iv_to", NodeType::Interviewer);
        node.attrs.insert(
            "question".to_string(),
            NodeAttrValue::String("Waiting...".to_string()),
        );

        let ctx = Context::new();
        let result = handler.execute(&node, &ctx).await.unwrap();
        assert!(result.is_failure());
        match result {
            Outcome::Failure { error, .. } => {
                assert!(error.contains("timed out"));
            }
            other => panic!("expected failure, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn handler_handles_only_interviewer_nodes() {
        let interviewer = Arc::new(AutoApproveInterviewer::new());
        let handler = InterviewerHandler::new(interviewer);

        assert!(handler.handles(&NodeType::Interviewer));
        assert!(!handler.handles(&NodeType::Start));
        assert!(!handler.handles(&NodeType::Exit));
        assert!(!handler.handles(&NodeType::Codergen));
        assert!(!handler.handles(&NodeType::Conditional));
        assert!(!handler.handles(&NodeType::Tool));
        assert!(!handler.handles(&NodeType::Parallel));
        assert!(!handler.handles(&NodeType::Manager));
        assert!(!handler.handles(&NodeType::Generic));
    }

    #[test]
    fn handler_name_is_interviewer() {
        let interviewer = Arc::new(AutoApproveInterviewer::new());
        let handler = InterviewerHandler::new(interviewer);
        assert_eq!(handler.name(), "interviewer");
    }

    // ---------------------------------------------------------------
    // InterviewerError display formatting
    // ---------------------------------------------------------------

    #[test]
    fn interviewer_error_display_cancelled() {
        let err = InterviewerError::Cancelled;
        assert_eq!(err.to_string(), "interview cancelled by user");
    }

    #[test]
    fn interviewer_error_display_timeout() {
        let err = InterviewerError::Timeout;
        assert_eq!(err.to_string(), "interview timed out");
    }

    #[test]
    fn interviewer_error_display_other() {
        let err = InterviewerError::Other("custom problem".to_string());
        assert_eq!(err.to_string(), "interviewer error: custom problem");
    }

    // ---------------------------------------------------------------
    // Cancelled in options mode returns skip
    // ---------------------------------------------------------------

    #[tokio::test]
    async fn handler_cancelled_with_options_returns_skip() {
        let interviewer = Arc::new(CancellingInterviewer);
        let handler = InterviewerHandler::new(interviewer);

        let mut node = make_node("iv7", NodeType::Interviewer);
        node.attrs.insert(
            "question".to_string(),
            NodeAttrValue::String("Pick one".to_string()),
        );
        node.attrs.insert(
            "options".to_string(),
            NodeAttrValue::String("a, b, c".to_string()),
        );

        let ctx = Context::new();
        let result = handler.execute(&node, &ctx).await.unwrap();
        match result {
            Outcome::Skip { reason } => {
                assert_eq!(reason, "interview cancelled");
            }
            other => panic!("expected skip, got {other:?}"),
        }
    }

    // ---------------------------------------------------------------
    // Cancelled in approve mode returns skip
    // ---------------------------------------------------------------

    #[tokio::test]
    async fn handler_cancelled_approve_returns_skip() {
        let interviewer = Arc::new(CancellingInterviewer);
        let handler = InterviewerHandler::new(interviewer);

        let mut node = make_node("iv8", NodeType::Interviewer);
        node.attrs.insert(
            "question".to_string(),
            NodeAttrValue::String("Approve?".to_string()),
        );
        node.attrs.insert(
            "approve".to_string(),
            NodeAttrValue::Bool(true),
        );

        let ctx = Context::new();
        let result = handler.execute(&node, &ctx).await.unwrap();
        match result {
            Outcome::Skip { reason } => {
                assert_eq!(reason, "interview cancelled");
            }
            other => panic!("expected skip, got {other:?}"),
        }
    }

    // ---------------------------------------------------------------
    // ConsoleInterviewer tests
    // ---------------------------------------------------------------

    /// A shared buffer for capturing output in tests.
    #[derive(Clone)]
    struct SharedBuffer {
        inner: Arc<Mutex<Vec<u8>>>,
    }

    impl SharedBuffer {
        fn new() -> Self {
            Self {
                inner: Arc::new(Mutex::new(Vec::new())),
            }
        }

        fn contents(&self) -> String {
            let buf = self.inner.lock().unwrap();
            String::from_utf8(buf.clone()).unwrap()
        }
    }

    impl std::io::Write for SharedBuffer {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            let mut inner = self.inner.lock().unwrap();
            inner.extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn console_ask_writes_question_and_reads_response() {
        let input = std::io::Cursor::new(b"hello world\n".to_vec());
        let output = SharedBuffer::new();
        let output_clone = output.clone();
        let interviewer = ConsoleInterviewer::new(input, output);

        let ctx = Context::new();
        let response = interviewer.ask("What is your name? ", &ctx).await.unwrap();
        assert_eq!(response, "hello world");

        // Verify the question was written to output.
        let written = output_clone.contents();
        assert!(written.contains("What is your name?"));
    }

    #[tokio::test]
    async fn console_ask_with_options_displays_numbered_options() {
        // User types "2" to select the second option.
        let input = std::io::Cursor::new(b"2\n".to_vec());
        let output = SharedBuffer::new();
        let output_clone = output.clone();
        let interviewer = ConsoleInterviewer::new(input, output);

        let ctx = Context::new();
        let options = vec!["red".to_string(), "blue".to_string(), "green".to_string()];
        let response = interviewer
            .ask_with_options("Pick a color", &options, &ctx)
            .await
            .unwrap();
        assert_eq!(response, "blue");

        // Verify the options were displayed with numbers.
        let written = output_clone.contents();
        assert!(written.contains("1. red"));
        assert!(written.contains("2. blue"));
        assert!(written.contains("3. green"));
    }

    #[tokio::test]
    async fn console_approve_yes_returns_true() {
        let input = std::io::Cursor::new(b"yes\n".to_vec());
        let output = SharedBuffer::new();
        let output_clone = output.clone();
        let interviewer = ConsoleInterviewer::new(input, output);

        let ctx = Context::new();
        let approved = interviewer.approve("Deploy?", &ctx).await.unwrap();
        assert!(approved);

        // Verify the prompt was written.
        let written = output_clone.contents();
        assert!(written.contains("Deploy? (yes/no): "));
    }

    #[tokio::test]
    async fn console_approve_no_returns_false() {
        let input = std::io::Cursor::new(b"no\n".to_vec());
        let output = SharedBuffer::new();
        let interviewer = ConsoleInterviewer::new(input, output);

        let ctx = Context::new();
        let approved = interviewer.approve("Deploy?", &ctx).await.unwrap();
        assert!(!approved);
    }

    #[tokio::test]
    async fn console_approve_case_insensitive() {
        // "YES" should be treated as approval.
        let input = std::io::Cursor::new(b"YES\n".to_vec());
        let output = SharedBuffer::new();
        let interviewer = ConsoleInterviewer::new(input, output);

        let ctx = Context::new();
        let approved = interviewer.approve("Continue?", &ctx).await.unwrap();
        assert!(approved);

        // "Y" should also be treated as approval.
        let input2 = std::io::Cursor::new(b"Y\n".to_vec());
        let output2 = SharedBuffer::new();
        let interviewer2 = ConsoleInterviewer::new(input2, output2);
        let approved2 = interviewer2.approve("Continue?", &ctx).await.unwrap();
        assert!(approved2);

        // "No" should be treated as rejection.
        let input3 = std::io::Cursor::new(b"No\n".to_vec());
        let output3 = SharedBuffer::new();
        let interviewer3 = ConsoleInterviewer::new(input3, output3);
        let approved3 = interviewer3.approve("Continue?", &ctx).await.unwrap();
        assert!(!approved3);
    }

    // ---------------------------------------------------------------
    // RecordingInterviewer tests
    // ---------------------------------------------------------------

    #[tokio::test]
    async fn recording_captures_ask_interaction() {
        let inner = Arc::new(AutoApproveInterviewer::with_response("42"));
        let recorder = RecordingInterviewer::new(inner);

        let ctx = Context::new();
        let response = recorder.ask("What is the answer?", &ctx).await.unwrap();
        assert_eq!(response, "42");

        let recordings = recorder.recordings();
        assert_eq!(recordings.len(), 1);
        assert_eq!(recordings[0].question, "What is the answer?");
        assert_eq!(recordings[0].response, "42");
        assert_eq!(recordings[0].interaction_type, InteractionType::Ask);
    }

    #[tokio::test]
    async fn recording_captures_approve_interaction() {
        let inner = Arc::new(AutoApproveInterviewer::new());
        let recorder = RecordingInterviewer::new(inner);

        let ctx = Context::new();
        let approved = recorder.approve("Deploy to prod?", &ctx).await.unwrap();
        assert!(approved);

        let recordings = recorder.recordings();
        assert_eq!(recordings.len(), 1);
        assert_eq!(recordings[0].question, "Deploy to prod?");
        assert_eq!(recordings[0].response, "yes");
        assert_eq!(recordings[0].interaction_type, InteractionType::Approve);
    }

    #[tokio::test]
    async fn recording_captures_ask_with_options() {
        let inner = Arc::new(AutoApproveInterviewer::new());
        let recorder = RecordingInterviewer::new(inner);

        let ctx = Context::new();
        let options = vec!["alpha".to_string(), "beta".to_string()];
        let response = recorder
            .ask_with_options("Choose:", &options, &ctx)
            .await
            .unwrap();
        assert_eq!(response, "alpha");

        let recordings = recorder.recordings();
        assert_eq!(recordings.len(), 1);
        assert_eq!(recordings[0].question, "Choose:");
        assert_eq!(recordings[0].response, "alpha");
        assert_eq!(recordings[0].interaction_type, InteractionType::AskWithOptions);
    }

    #[tokio::test]
    async fn recording_count_tracks_interactions() {
        let inner = Arc::new(AutoApproveInterviewer::new());
        let recorder = RecordingInterviewer::new(inner);

        let ctx = Context::new();
        assert_eq!(recorder.recording_count(), 0);

        recorder.ask("q1?", &ctx).await.unwrap();
        assert_eq!(recorder.recording_count(), 1);

        recorder.approve("approve?", &ctx).await.unwrap();
        assert_eq!(recorder.recording_count(), 2);

        recorder
            .ask_with_options("pick?", &["a".to_string()], &ctx)
            .await
            .unwrap();
        assert_eq!(recorder.recording_count(), 3);
    }

    #[tokio::test]
    async fn recording_clear_removes_all_records() {
        let inner = Arc::new(AutoApproveInterviewer::new());
        let recorder = RecordingInterviewer::new(inner);

        let ctx = Context::new();
        recorder.ask("q1?", &ctx).await.unwrap();
        recorder.ask("q2?", &ctx).await.unwrap();
        assert_eq!(recorder.recording_count(), 2);

        recorder.clear();
        assert_eq!(recorder.recording_count(), 0);
        assert!(recorder.recordings().is_empty());
    }

    #[tokio::test]
    async fn recording_delegates_to_inner() {
        let queue = Arc::new(QueueInterviewer::new());
        queue.push_response("specific answer");
        queue.push_approval(false);

        let recorder = RecordingInterviewer::new(queue);

        let ctx = Context::new();

        // Verify the inner interviewer's behavior is preserved.
        let response = recorder.ask("question?", &ctx).await.unwrap();
        assert_eq!(response, "specific answer");

        let approved = recorder.approve("approve?", &ctx).await.unwrap();
        assert!(!approved);

        // Verify recordings captured the inner results.
        let recordings = recorder.recordings();
        assert_eq!(recordings.len(), 2);
        assert_eq!(recordings[0].response, "specific answer");
        assert_eq!(recordings[1].response, "no");
        assert_eq!(recordings[1].interaction_type, InteractionType::Approve);
    }
}
