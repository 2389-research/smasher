// ABOUTME: Generic SSE (Server-Sent Events) parser for streaming LLM responses.
// ABOUTME: Transforms a byte stream into structured SseEvent items for provider adapters.

use bytes::Bytes;
use futures::Stream;
use std::pin::Pin;

/// A parsed Server-Sent Event containing event type, data, and optional ID.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SseEvent {
    /// The event type, defaults to "message" per the SSE specification.
    pub event_type: String,
    /// The joined data payload from all `data:` lines in this event.
    pub data: String,
    /// Optional event ID from the `id:` field.
    pub id: Option<String>,
}

/// Parse a byte stream (typically from an HTTP response body) into a stream of SSE events.
///
/// This function handles the SSE wire protocol: buffering partial lines across chunk
/// boundaries, normalizing `\r\n` to `\n`, joining multiple `data:` lines, and yielding
/// complete events on empty-line delimiters.
pub fn parse_sse_stream(
    byte_stream: impl Stream<Item = Result<Bytes, reqwest::Error>> + Send + 'static,
) -> Pin<Box<dyn Stream<Item = Result<SseEvent, crate::types::Error>> + Send>> {
    use async_stream::try_stream;
    use futures::StreamExt;

    Box::pin(try_stream! {
        let mut byte_stream = std::pin::pin!(byte_stream);

        // Buffer for incomplete lines carried across chunk boundaries.
        let mut line_buf = String::new();

        // Accumulator for the current in-progress event.
        let mut event_type = String::from("message");
        let mut data_lines: Vec<String> = Vec::new();
        let mut event_id: Option<String> = None;

        while let Some(chunk_result) = byte_stream.next().await {
            let chunk = chunk_result.map_err(|e| crate::types::Error::StreamError {
                provider: "sse".into(),
                message: e.to_string(),
            })?;

            // Append raw bytes to the line buffer and normalize \r\n to \n.
            let text = String::from_utf8_lossy(&chunk);
            line_buf.push_str(&text);

            // Process all complete lines (terminated by \n).
            loop {
                let Some(newline_pos) = line_buf.find('\n') else {
                    break;
                };

                // Extract the line, stripping any trailing \r.
                let mut line = line_buf[..newline_pos].to_string();
                line_buf = line_buf[newline_pos + 1..].to_string();

                if line.ends_with('\r') {
                    line.pop();
                }

                if line.is_empty() {
                    // Empty line delimits an event. Only yield if we have data.
                    if !data_lines.is_empty() {
                        let event = SseEvent {
                            event_type: std::mem::replace(&mut event_type, String::from("message")),
                            data: data_lines.join("\n"),
                            id: event_id.take(),
                        };
                        data_lines.clear();
                        yield event;
                    } else {
                        // Reset state even when no data to yield.
                        event_type = String::from("message");
                        event_id = None;
                    }
                } else if let Some(rest) = line.strip_prefix("data:") {
                    // Strip optional leading space after the colon.
                    let value = rest.strip_prefix(' ').unwrap_or(rest);
                    data_lines.push(value.to_string());
                } else if let Some(rest) = line.strip_prefix("event:") {
                    let value = rest.strip_prefix(' ').unwrap_or(rest);
                    event_type = value.to_string();
                } else if let Some(rest) = line.strip_prefix("id:") {
                    let value = rest.strip_prefix(' ').unwrap_or(rest);
                    event_id = Some(value.to_string());
                } else if line.starts_with(':') {
                    // Comment line, ignore.
                }
                // Any other line without a recognized field name is ignored per spec.
            }
        }

        // If the stream ends with accumulated data but no trailing empty line, yield it.
        if !data_lines.is_empty() {
            yield SseEvent {
                event_type,
                data: data_lines.join("\n"),
                id: event_id,
            };
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use futures::stream;
    use futures::StreamExt;

    /// Helper: wrap string slices into a stream of Ok(Bytes) chunks.
    fn mock_stream(
        chunks: Vec<&str>,
    ) -> impl Stream<Item = Result<Bytes, reqwest::Error>> + Send + 'static {
        let owned: Vec<Result<Bytes, reqwest::Error>> = chunks
            .into_iter()
            .map(|s| Ok(Bytes::from(s.to_string())))
            .collect();
        stream::iter(owned)
    }

    /// Collect all events from a parsed SSE stream.
    async fn collect_events(
        stream: Pin<Box<dyn Stream<Item = Result<SseEvent, crate::types::Error>> + Send>>,
    ) -> Vec<SseEvent> {
        stream
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .map(|r| r.expect("unexpected error in SSE stream"))
            .collect()
    }

    #[tokio::test]
    async fn parse_simple_single_event() {
        let s = mock_stream(vec!["data: hello world\n\n"]);
        let events = collect_events(parse_sse_stream(s)).await;

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, "message");
        assert_eq!(events[0].data, "hello world");
        assert_eq!(events[0].id, None);
    }

    #[tokio::test]
    async fn parse_event_with_explicit_type() {
        let s = mock_stream(vec!["event: delta\ndata: some payload\n\n"]);
        let events = collect_events(parse_sse_stream(s)).await;

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, "delta");
        assert_eq!(events[0].data, "some payload");
    }

    #[tokio::test]
    async fn parse_multiple_events_in_one_chunk() {
        let s = mock_stream(vec!["data: first\n\ndata: second\n\ndata: third\n\n"]);
        let events = collect_events(parse_sse_stream(s)).await;

        assert_eq!(events.len(), 3);
        assert_eq!(events[0].data, "first");
        assert_eq!(events[1].data, "second");
        assert_eq!(events[2].data, "third");
    }

    #[tokio::test]
    async fn multiple_data_lines_joined_with_newline() {
        let s = mock_stream(vec!["data: line one\ndata: line two\ndata: line three\n\n"]);
        let events = collect_events(parse_sse_stream(s)).await;

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].data, "line one\nline two\nline three");
    }

    #[tokio::test]
    async fn comments_are_skipped() {
        let s = mock_stream(vec![
            ": this is a comment\ndata: visible\n: another comment\n\n",
        ]);
        let events = collect_events(parse_sse_stream(s)).await;

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].data, "visible");
    }

    #[tokio::test]
    async fn crlf_line_endings() {
        let s = mock_stream(vec!["data: hello\r\n\r\n"]);
        let events = collect_events(parse_sse_stream(s)).await;

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].data, "hello");
    }

    #[tokio::test]
    async fn empty_data_lines() {
        // "data:" with no value yields an empty string entry.
        let s = mock_stream(vec!["data:\ndata: after\n\n"]);
        let events = collect_events(parse_sse_stream(s)).await;

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].data, "\nafter");
    }

    #[tokio::test]
    async fn partial_chunks_across_boundaries() {
        // The line "data: split here" is delivered across two chunks.
        let s = mock_stream(vec!["data: spl", "it here\n\n"]);
        let events = collect_events(parse_sse_stream(s)).await;

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].data, "split here");
    }

    #[tokio::test]
    async fn stream_error_propagation() {
        // We need to produce a reqwest::Error. The easiest way is to attempt
        // building a request with an invalid URL and extracting the error.
        let err = reqwest::Client::new()
            .get("htt://bad")
            .send()
            .await
            .unwrap_err();

        let chunks: Vec<Result<Bytes, reqwest::Error>> =
            vec![Ok(Bytes::from("data: ok\n\n")), Err(err)];
        let s = stream::iter(chunks);

        let mut event_stream = parse_sse_stream(s);
        // First event should succeed.
        let first = event_stream.next().await.unwrap();
        assert!(first.is_ok());

        // Second item should be a StreamError.
        let second = event_stream.next().await.unwrap();
        assert!(second.is_err());
        let err = second.unwrap_err();
        match &err {
            crate::types::Error::StreamError { provider, .. } => {
                assert_eq!(provider, "sse");
            }
            other => panic!("expected StreamError, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn event_with_id_field() {
        let s = mock_stream(vec!["id: 42\ndata: payload\n\n"]);
        let events = collect_events(parse_sse_stream(s)).await;

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].id, Some("42".to_string()));
        assert_eq!(events[0].data, "payload");
    }

    #[tokio::test]
    async fn data_without_space_after_colon() {
        let s = mock_stream(vec!["data:nospace\n\n"]);
        let events = collect_events(parse_sse_stream(s)).await;

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].data, "nospace");
    }

    #[tokio::test]
    async fn done_sentinel_yielded_as_normal_event() {
        let s = mock_stream(vec!["data: [DONE]\n\n"]);
        let events = collect_events(parse_sse_stream(s)).await;

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].data, "[DONE]");
        assert_eq!(events[0].event_type, "message");
    }

    #[tokio::test]
    async fn event_with_no_data_not_yielded() {
        // An event block with only event type and id but no data lines should not yield.
        let s = mock_stream(vec!["event: ping\nid: 1\n\ndata: real\n\n"]);
        let events = collect_events(parse_sse_stream(s)).await;

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].data, "real");
    }

    #[tokio::test]
    async fn event_type_resets_between_events() {
        // After an event with a custom type, the next event should default back to "message".
        let s = mock_stream(vec![
            "event: custom\ndata: first\n\ndata: second\n\n",
        ]);
        let events = collect_events(parse_sse_stream(s)).await;

        assert_eq!(events.len(), 2);
        assert_eq!(events[0].event_type, "custom");
        assert_eq!(events[1].event_type, "message");
    }

    #[tokio::test]
    async fn trailing_data_without_empty_line() {
        // Stream ends without a final empty-line delimiter.
        let s = mock_stream(vec!["data: no trailing newline\n"]);
        let events = collect_events(parse_sse_stream(s)).await;

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].data, "no trailing newline");
    }
}
