// ABOUTME: Streaming structured JSON output from LLMs with partial object updates as they arrive.
// ABOUTME: Wraps client.stream with JSON schema enforcement and incremental serde deserialization.

use futures::StreamExt;
use serde::de::DeserializeOwned;

use crate::client::Client;
use crate::types::{Error, Request, ResponseFormat, Usage};

/// Event emitted during object streaming, containing partial parse attempts.
#[derive(Debug, Clone)]
pub struct PartialObjectEvent<T> {
    /// The partial object parsed so far, if the accumulated JSON is valid enough to deserialize.
    pub partial: Option<T>,
    /// Raw text accumulated so far from the stream.
    pub raw_text: String,
    /// Whether this is the final, complete object.
    pub is_complete: bool,
}

/// Final result of a stream_object call, containing the fully parsed object and usage stats.
#[derive(Debug)]
pub struct StreamObjectResult<T> {
    /// The fully deserialized object.
    pub object: T,
    /// Token usage statistics from the stream.
    pub usage: Usage,
}

/// Stream a structured object from an LLM, yielding partial parses as text deltas arrive.
///
/// Sets `request.response_format` to a strict JSON schema, initiates a streaming request,
/// and spawns a background task that accumulates text deltas, attempts to parse partial
/// JSON at each step, and sends `PartialObjectEvent`s through a channel. The returned
/// `JoinHandle` resolves to the final `StreamObjectResult` once the stream completes.
pub async fn stream_object<T>(
    client: &Client,
    mut request: Request,
    schema_name: &str,
    schema: serde_json::Value,
) -> Result<
    (
        tokio::sync::mpsc::Receiver<PartialObjectEvent<T>>,
        tokio::task::JoinHandle<Result<StreamObjectResult<T>, Error>>,
    ),
    Error,
>
where
    T: DeserializeOwned + Clone + Send + 'static,
{
    request.response_format = Some(ResponseFormat::JsonSchema {
        name: schema_name.to_string(),
        schema,
        strict: true,
    });

    let mut stream_result = super::stream::stream(client, &request).await?;
    let (tx, rx) = tokio::sync::mpsc::channel::<PartialObjectEvent<T>>(32);

    let handle = tokio::spawn(async move {
        let accumulator = &mut stream_result.accumulator;

        while let Some(event_result) = stream_result.events.next().await {
            let event = event_result?;
            accumulator.process_event(&event);

            let raw_text = accumulator.text().to_string();
            if raw_text.is_empty() {
                continue;
            }

            let partial: Option<T> = serde_json::from_str(&raw_text).ok();

            // Best-effort send; if the receiver is dropped we keep going to get the final result.
            let _ = tx
                .send(PartialObjectEvent {
                    partial,
                    raw_text,
                    is_complete: false,
                })
                .await;
        }

        let raw_text = accumulator.text().to_string();
        let usage = accumulator.usage().clone();

        let object: T = serde_json::from_str(&raw_text).map_err(|e| Error::ResponseParse {
            provider: "stream_object".into(),
            message: format!("failed to deserialize final JSON: {e}"),
        })?;

        // Send the final complete event.
        let _ = tx
            .send(PartialObjectEvent {
                partial: Some(object.clone()),
                raw_text,
                is_complete: true,
            })
            .await;

        Ok(StreamObjectResult { object, usage })
    });

    Ok((rx, handle))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::{ProviderAdapter, StreamResponse};
    use crate::types::{
        ContentPart, FinishReason, Message, Provider, Response, StreamEvent, Usage,
    };
    use async_trait::async_trait;
    use serde::Deserialize;
    use std::sync::Arc;

    #[derive(Debug, Clone, Deserialize, PartialEq)]
    struct Person {
        name: String,
        age: u32,
    }

    fn person_schema() -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "name": {"type": "string"},
                "age": {"type": "integer"}
            },
            "required": ["name", "age"]
        })
    }

    /// A mock adapter that returns a stream of pre-built events.
    struct StreamMockAdapter {
        events: Vec<StreamEvent>,
    }

    #[async_trait]
    impl ProviderAdapter for StreamMockAdapter {
        fn provider_name(&self) -> &str {
            "anthropic"
        }

        async fn complete(&self, _: &Request) -> Result<Response, Error> {
            Ok(Response {
                id: "test".into(),
                model: "test".into(),
                content: vec![ContentPart::text("{}")],
                finish_reason: Some(FinishReason::Stop),
                usage: Usage::default(),
                warnings: vec![],
                rate_limit: None,
                provider: None,
                raw: None,
            })
        }

        async fn stream(&self, _: &Request) -> Result<StreamResponse, Error> {
            let events: Vec<Result<StreamEvent, Error>> =
                self.events.iter().cloned().map(Ok).collect();
            Ok(Box::pin(futures::stream::iter(events)))
        }
    }

    fn test_client(events: Vec<StreamEvent>) -> Client {
        let mut client = Client::new();
        client.register_provider(
            Provider::Anthropic,
            Arc::new(StreamMockAdapter { events }),
        );
        client
    }

    fn test_request() -> Request {
        Request::new(
            "claude-sonnet-4-20250514",
            vec![Message::user("give me a person")],
        )
    }

    fn person_stream_events() -> Vec<StreamEvent> {
        vec![
            StreamEvent::start("resp_1", "claude-sonnet-4"),
            StreamEvent::text_delta(r#"{"name""#),
            StreamEvent::text_delta(r#": "Alice""#),
            StreamEvent::text_delta(r#", "age": 30}"#),
            StreamEvent::end(
                Some(FinishReason::Stop),
                Some(Usage {
                    input_tokens: 10,
                    output_tokens: 25,
                    ..Default::default()
                }),
            ),
        ]
    }

    // Test 1: The final StreamObjectResult contains a correctly parsed object.
    #[tokio::test]
    async fn stream_object_result_contains_parsed_object() {
        let client = test_client(person_stream_events());

        let (_rx, handle) =
            stream_object::<Person>(&client, test_request(), "person", person_schema())
                .await
                .unwrap();

        let result = handle.await.unwrap().unwrap();
        assert_eq!(
            result.object,
            Person {
                name: "Alice".into(),
                age: 30,
            }
        );
    }

    // Test 2: Partial events track raw_text as it accumulates.
    #[tokio::test]
    async fn partial_object_event_tracks_raw_text() {
        let client = test_client(person_stream_events());

        let (mut rx, handle) =
            stream_object::<Person>(&client, test_request(), "person", person_schema())
                .await
                .unwrap();

        let mut raw_texts: Vec<String> = Vec::new();
        while let Some(event) = rx.recv().await {
            raw_texts.push(event.raw_text.clone());
        }

        // We should have received multiple partial events with growing raw text.
        assert!(raw_texts.len() >= 2, "expected at least 2 partial events, got {}", raw_texts.len());

        // Each successive raw_text should be longer than or equal to the previous.
        for window in raw_texts.windows(2) {
            assert!(
                window[1].len() >= window[0].len(),
                "raw_text should grow monotonically: {:?} -> {:?}",
                window[0],
                window[1],
            );
        }

        // The last raw_text should contain the full JSON.
        let last = raw_texts.last().unwrap();
        assert!(last.contains("Alice"), "final raw text should contain 'Alice': {last}");
        assert!(last.contains("30"), "final raw text should contain '30': {last}");

        // Wait for the spawned task to complete.
        let _ = handle.await.unwrap().unwrap();
    }

    // Test 3: The final partial event has is_complete set to true.
    #[tokio::test]
    async fn partial_object_event_is_complete_flag() {
        let client = test_client(person_stream_events());

        let (mut rx, handle) =
            stream_object::<Person>(&client, test_request(), "person", person_schema())
                .await
                .unwrap();

        let mut events: Vec<PartialObjectEvent<Person>> = Vec::new();
        while let Some(event) = rx.recv().await {
            events.push(event);
        }

        // The last event should be marked as complete.
        let last = events.last().expect("should have at least one event");
        assert!(last.is_complete, "last event should be marked as complete");

        // All events before the last should not be marked as complete.
        for event in &events[..events.len() - 1] {
            assert!(!event.is_complete, "non-final event should not be marked as complete");
        }

        let _ = handle.await.unwrap().unwrap();
    }

    // Test 4: The final StreamObjectResult includes usage statistics.
    #[tokio::test]
    async fn stream_object_result_includes_usage() {
        let client = test_client(person_stream_events());

        let (_rx, handle) =
            stream_object::<Person>(&client, test_request(), "person", person_schema())
                .await
                .unwrap();

        let result = handle.await.unwrap().unwrap();
        assert_eq!(result.usage.input_tokens, 10);
        assert_eq!(result.usage.output_tokens, 25);
    }

    // Test 5: stream_object returns an error if the final JSON is not valid for the target type.
    #[tokio::test]
    async fn stream_object_error_on_invalid_json() {
        let events = vec![
            StreamEvent::start("resp_1", "claude-sonnet-4"),
            StreamEvent::text_delta(r#"{"color": "blue"}"#),
            StreamEvent::end(Some(FinishReason::Stop), None),
        ];
        let client = test_client(events);

        let (_rx, handle) =
            stream_object::<Person>(&client, test_request(), "person", person_schema())
                .await
                .unwrap();

        let result = handle.await.unwrap();
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, Error::ResponseParse { .. }));
    }

    // Test 6: Partial events may have None for partial when JSON is not yet parseable.
    #[tokio::test]
    async fn partial_is_none_when_json_incomplete() {
        let events = vec![
            StreamEvent::start("resp_1", "claude-sonnet-4"),
            StreamEvent::text_delta(r#"{"name""#),   // incomplete JSON
            StreamEvent::text_delta(r#": "Bob", "age": 42}"#), // completes it
            StreamEvent::end(Some(FinishReason::Stop), None),
        ];
        let client = test_client(events);

        let (mut rx, handle) =
            stream_object::<Person>(&client, test_request(), "person", person_schema())
                .await
                .unwrap();

        let mut events_received: Vec<PartialObjectEvent<Person>> = Vec::new();
        while let Some(event) = rx.recv().await {
            events_received.push(event);
        }

        // The first non-final event should have partial=None (incomplete JSON).
        let first = &events_received[0];
        assert!(
            first.partial.is_none(),
            "first partial event should be None for incomplete JSON, raw: {}",
            first.raw_text,
        );

        // A later event should have a successful parse.
        let has_some = events_received.iter().any(|e| e.partial.is_some());
        assert!(has_some, "at least one event should have a successful partial parse");

        let _ = handle.await.unwrap().unwrap();
    }

    // Test 7: The response_format is set on the request (verified via capturing adapter).
    #[tokio::test]
    async fn sets_response_format_on_request() {
        struct CapturingStreamAdapter {
            captured_request: std::sync::Mutex<Option<Request>>,
        }

        #[async_trait]
        impl ProviderAdapter for CapturingStreamAdapter {
            fn provider_name(&self) -> &str {
                "anthropic"
            }

            async fn complete(&self, _: &Request) -> Result<Response, Error> {
                unimplemented!()
            }

            async fn stream(&self, request: &Request) -> Result<StreamResponse, Error> {
                *self.captured_request.lock().unwrap() = Some(request.clone());
                let events: Vec<Result<StreamEvent, Error>> = vec![
                    Ok(StreamEvent::start("resp_1", "claude-sonnet-4")),
                    Ok(StreamEvent::text_delta(r#"{"name": "Test", "age": 1}"#)),
                    Ok(StreamEvent::end(Some(FinishReason::Stop), None)),
                ];
                Ok(Box::pin(futures::stream::iter(events)))
            }
        }

        let adapter = Arc::new(CapturingStreamAdapter {
            captured_request: std::sync::Mutex::new(None),
        });
        let mut client = Client::new();
        client.register_provider(Provider::Anthropic, adapter.clone());

        let schema = person_schema();
        let (_rx, handle) =
            stream_object::<Person>(&client, test_request(), "person", schema.clone())
                .await
                .unwrap();
        let _ = handle.await.unwrap().unwrap();

        let captured = adapter.captured_request.lock().unwrap();
        let req = captured.as_ref().expect("request should have been captured");

        match &req.response_format {
            Some(ResponseFormat::JsonSchema {
                name,
                schema: req_schema,
                strict,
            }) => {
                assert_eq!(name, "person");
                assert_eq!(req_schema, &schema);
                assert!(strict);
            }
            other => panic!("expected JsonSchema response_format, got {other:?}"),
        }
    }
}
