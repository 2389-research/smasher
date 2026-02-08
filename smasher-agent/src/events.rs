// ABOUTME: Broadcast-based event emitter for delivering SessionEvent to multiple subscribers.
// ABOUTME: Enables UI layers, loggers, and other consumers to react to agent actions in real-time.

use tokio::sync::broadcast;

use crate::types::SessionEvent;

/// Delivers `SessionEvent` to multiple subscribers via a `tokio::sync::broadcast` channel.
///
/// Consumers call [`subscribe`](EventEmitter::subscribe) to obtain a receiver, then
/// await events as the agent session progresses. Events that arrive before any
/// subscriber exists are silently dropped.
pub struct EventEmitter {
    sender: broadcast::Sender<SessionEvent>,
}

impl EventEmitter {
    /// Create an emitter with the given channel capacity.
    ///
    /// A capacity of 256 is a reasonable default for most sessions.
    pub fn new(capacity: usize) -> Self {
        let (sender, _) = broadcast::channel(capacity);
        Self { sender }
    }

    /// Send an event to all active subscribers.
    ///
    /// If no subscribers exist the event is silently dropped.
    pub fn emit(&self, event: SessionEvent) {
        let _ = self.sender.send(event);
    }

    /// Create a new subscription that will receive future events.
    pub fn subscribe(&self) -> broadcast::Receiver<SessionEvent> {
        self.sender.subscribe()
    }

    /// Return the number of active subscribers.
    ///
    /// Note: `broadcast::Sender::receiver_count` tracks receivers that have not
    /// been dropped. Dropping a `Receiver` reduces this count.
    pub fn subscriber_count(&self) -> usize {
        self.sender.receiver_count()
    }
}

impl Default for EventEmitter {
    fn default() -> Self {
        Self::new(256)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Construction ─────────────────────────────────────────────────

    #[test]
    fn new_creates_emitter() {
        let emitter = EventEmitter::new(64);
        assert_eq!(emitter.subscriber_count(), 0);
    }

    #[test]
    fn default_creates_emitter() {
        let emitter = EventEmitter::default();
        assert_eq!(emitter.subscriber_count(), 0);
    }

    // ── Subscriptions ────────────────────────────────────────────────

    #[test]
    fn subscribe_returns_receiver() {
        let emitter = EventEmitter::new(16);
        let _rx = emitter.subscribe();
        assert_eq!(emitter.subscriber_count(), 1);
    }

    #[test]
    fn subscriber_count_reflects_active_subscriptions() {
        let emitter = EventEmitter::new(16);
        let _rx1 = emitter.subscribe();
        let _rx2 = emitter.subscribe();
        let _rx3 = emitter.subscribe();
        assert_eq!(emitter.subscriber_count(), 3);
    }

    #[test]
    fn dropped_receiver_reduces_subscriber_count() {
        let emitter = EventEmitter::new(16);
        let rx1 = emitter.subscribe();
        let _rx2 = emitter.subscribe();
        assert_eq!(emitter.subscriber_count(), 2);

        drop(rx1);
        assert_eq!(emitter.subscriber_count(), 1);
    }

    // ── Emitting ─────────────────────────────────────────────────────

    #[test]
    fn emit_with_no_subscribers_does_not_panic() {
        let emitter = EventEmitter::new(16);
        emitter.emit(SessionEvent::TurnStarted { turn_number: 1 });
    }

    #[tokio::test]
    async fn emitted_event_is_received_by_subscriber() {
        let emitter = EventEmitter::new(16);
        let mut rx = emitter.subscribe();

        emitter.emit(SessionEvent::SessionStarted {
            session_id: "s1".into(),
        });

        let event = rx.recv().await.expect("should receive event");
        match event {
            SessionEvent::SessionStarted { session_id } => {
                assert_eq!(session_id, "s1");
            }
            other => panic!("unexpected event: {:?}", other),
        }
    }

    #[tokio::test]
    async fn multiple_subscribers_receive_same_event() {
        let emitter = EventEmitter::new(16);
        let mut rx1 = emitter.subscribe();
        let mut rx2 = emitter.subscribe();

        emitter.emit(SessionEvent::TurnStarted { turn_number: 42 });

        let e1 = rx1.recv().await.expect("rx1 should receive event");
        let e2 = rx2.recv().await.expect("rx2 should receive event");

        match (e1, e2) {
            (
                SessionEvent::TurnStarted { turn_number: n1 },
                SessionEvent::TurnStarted { turn_number: n2 },
            ) => {
                assert_eq!(n1, 42);
                assert_eq!(n2, 42);
            }
            other => panic!("unexpected events: {:?}", other),
        }
    }

    // ── Sequence and ordering ────────────────────────────────────────

    #[tokio::test]
    async fn events_received_in_emit_order() {
        let emitter = EventEmitter::new(16);
        let mut rx = emitter.subscribe();

        emitter.emit(SessionEvent::TurnStarted { turn_number: 1 });
        emitter.emit(SessionEvent::TurnStarted { turn_number: 2 });
        emitter.emit(SessionEvent::TurnStarted { turn_number: 3 });

        let e1 = rx.recv().await.unwrap();
        let e2 = rx.recv().await.unwrap();
        let e3 = rx.recv().await.unwrap();

        match (e1, e2, e3) {
            (
                SessionEvent::TurnStarted { turn_number: n1 },
                SessionEvent::TurnStarted { turn_number: n2 },
                SessionEvent::TurnStarted { turn_number: n3 },
            ) => {
                assert_eq!(n1, 1);
                assert_eq!(n2, 2);
                assert_eq!(n3, 3);
            }
            other => panic!("unexpected events: {:?}", other),
        }
    }

    #[tokio::test]
    async fn late_subscriber_does_not_receive_prior_events() {
        let emitter = EventEmitter::new(16);

        // Emit before subscribing
        emitter.emit(SessionEvent::TurnStarted { turn_number: 1 });

        let mut rx = emitter.subscribe();

        // Emit after subscribing
        emitter.emit(SessionEvent::TurnStarted { turn_number: 2 });

        let event = rx.recv().await.unwrap();
        match event {
            SessionEvent::TurnStarted { turn_number } => {
                assert_eq!(turn_number, 2, "late subscriber should only see events after subscription");
            }
            other => panic!("unexpected event: {:?}", other),
        }
    }

    // ── Different event variants ─────────────────────────────────────

    #[tokio::test]
    async fn emit_session_started_event() {
        let emitter = EventEmitter::new(16);
        let mut rx = emitter.subscribe();

        emitter.emit(SessionEvent::SessionStarted {
            session_id: "test-sess-123".into(),
        });

        match rx.recv().await.unwrap() {
            SessionEvent::SessionStarted { session_id } => {
                assert_eq!(session_id, "test-sess-123");
            }
            other => panic!("unexpected: {:?}", other),
        }
    }

    #[tokio::test]
    async fn emit_text_delta_event() {
        let emitter = EventEmitter::new(16);
        let mut rx = emitter.subscribe();

        emitter.emit(SessionEvent::TextDelta {
            text: "Hello, ".into(),
        });

        match rx.recv().await.unwrap() {
            SessionEvent::TextDelta { text } => {
                assert_eq!(text, "Hello, ");
            }
            other => panic!("unexpected: {:?}", other),
        }
    }

    #[tokio::test]
    async fn emit_tool_call_started_event() {
        let emitter = EventEmitter::new(16);
        let mut rx = emitter.subscribe();

        emitter.emit(SessionEvent::ToolCallStarted {
            tool_name: "bash".into(),
            tool_call_id: "call_001".into(),
        });

        match rx.recv().await.unwrap() {
            SessionEvent::ToolCallStarted {
                tool_name,
                tool_call_id,
            } => {
                assert_eq!(tool_name, "bash");
                assert_eq!(tool_call_id, "call_001");
            }
            other => panic!("unexpected: {:?}", other),
        }
    }

    #[tokio::test]
    async fn emit_tool_call_completed_event() {
        let emitter = EventEmitter::new(16);
        let mut rx = emitter.subscribe();

        emitter.emit(SessionEvent::ToolCallCompleted {
            tool_name: "read_file".into(),
            tool_call_id: "call_002".into(),
            result: "file contents here".into(),
            is_error: false,
            duration_ms: 55,
        });

        match rx.recv().await.unwrap() {
            SessionEvent::ToolCallCompleted {
                tool_name,
                tool_call_id,
                result,
                is_error,
                duration_ms,
            } => {
                assert_eq!(tool_name, "read_file");
                assert_eq!(tool_call_id, "call_002");
                assert_eq!(result, "file contents here");
                assert!(!is_error);
                assert_eq!(duration_ms, 55);
            }
            other => panic!("unexpected: {:?}", other),
        }
    }

    #[tokio::test]
    async fn emit_tool_call_completed_with_error() {
        let emitter = EventEmitter::new(16);
        let mut rx = emitter.subscribe();

        emitter.emit(SessionEvent::ToolCallCompleted {
            tool_name: "shell".into(),
            tool_call_id: "call_err".into(),
            result: "command not found".into(),
            is_error: true,
            duration_ms: 12,
        });

        match rx.recv().await.unwrap() {
            SessionEvent::ToolCallCompleted {
                is_error, result, ..
            } => {
                assert!(is_error);
                assert_eq!(result, "command not found");
            }
            other => panic!("unexpected: {:?}", other),
        }
    }

    #[tokio::test]
    async fn emit_steering_applied_event() {
        let emitter = EventEmitter::new(16);
        let mut rx = emitter.subscribe();

        emitter.emit(SessionEvent::SteeringApplied {
            text: "focus on tests".into(),
        });

        match rx.recv().await.unwrap() {
            SessionEvent::SteeringApplied { text } => {
                assert_eq!(text, "focus on tests");
            }
            other => panic!("unexpected: {:?}", other),
        }
    }

    #[tokio::test]
    async fn emit_session_completed_event() {
        let emitter = EventEmitter::new(16);
        let mut rx = emitter.subscribe();

        emitter.emit(SessionEvent::SessionCompleted {
            session_id: "done-sess".into(),
            total_turns: 15,
            total_usage: smasher_llm::types::Usage {
                input_tokens: 1000,
                output_tokens: 500,
                cache_read_tokens: None,
                cache_creation_tokens: None,
                reasoning_tokens: None,
                total_tokens: None,
                raw: None,
            },
        });

        match rx.recv().await.unwrap() {
            SessionEvent::SessionCompleted {
                session_id,
                total_turns,
                total_usage,
            } => {
                assert_eq!(session_id, "done-sess");
                assert_eq!(total_turns, 15);
                assert_eq!(total_usage.input_tokens, 1000);
                assert_eq!(total_usage.output_tokens, 500);
            }
            other => panic!("unexpected: {:?}", other),
        }
    }

    #[tokio::test]
    async fn emit_session_error_event() {
        let emitter = EventEmitter::new(16);
        let mut rx = emitter.subscribe();

        emitter.emit(SessionEvent::SessionError {
            session_id: "err-sess".into(),
            error: "rate limit exceeded".into(),
        });

        match rx.recv().await.unwrap() {
            SessionEvent::SessionError { session_id, error } => {
                assert_eq!(session_id, "err-sess");
                assert_eq!(error, "rate limit exceeded");
            }
            other => panic!("unexpected: {:?}", other),
        }
    }

    #[tokio::test]
    async fn emit_loop_detected_event() {
        let emitter = EventEmitter::new(16);
        let mut rx = emitter.subscribe();

        emitter.emit(SessionEvent::LoopDetected {
            pattern: "bash->bash->bash".into(),
            window_size: 3,
        });

        match rx.recv().await.unwrap() {
            SessionEvent::LoopDetected {
                pattern,
                window_size,
            } => {
                assert_eq!(pattern, "bash->bash->bash");
                assert_eq!(window_size, 3);
            }
            other => panic!("unexpected: {:?}", other),
        }
    }

    // ── Subscriber count edge cases ──────────────────────────────────

    #[test]
    fn subscriber_count_zero_after_all_dropped() {
        let emitter = EventEmitter::new(16);
        let rx1 = emitter.subscribe();
        let rx2 = emitter.subscribe();
        assert_eq!(emitter.subscriber_count(), 2);

        drop(rx1);
        drop(rx2);
        assert_eq!(emitter.subscriber_count(), 0);
    }

    #[test]
    fn subscribe_after_emit_still_works() {
        let emitter = EventEmitter::new(16);
        emitter.emit(SessionEvent::TurnStarted { turn_number: 1 });

        // Subscribing after emit should still give a valid receiver
        let _rx = emitter.subscribe();
        assert_eq!(emitter.subscriber_count(), 1);
    }

    // ── Channel capacity / lagged ────────────────────────────────────

    #[tokio::test]
    async fn receiver_reports_lagged_when_capacity_exceeded() {
        // Capacity of 2 means buffer holds 2 events
        let emitter = EventEmitter::new(2);
        let mut rx = emitter.subscribe();

        // Emit 4 events — first ones will be dropped for this receiver
        emitter.emit(SessionEvent::TurnStarted { turn_number: 1 });
        emitter.emit(SessionEvent::TurnStarted { turn_number: 2 });
        emitter.emit(SessionEvent::TurnStarted { turn_number: 3 });
        emitter.emit(SessionEvent::TurnStarted { turn_number: 4 });

        // The receiver should report a Lagged error because it missed events
        let result = rx.recv().await;
        match result {
            Err(broadcast::error::RecvError::Lagged(n)) => {
                assert!(n > 0, "should have lagged by at least 1 event");
            }
            Ok(event) => {
                // If we got an event, it should be one of the later ones
                // (broadcast may keep the most recent events)
                match event {
                    SessionEvent::TurnStarted { turn_number } => {
                        assert!(
                            turn_number >= 2,
                            "should have skipped early events, got turn {turn_number}"
                        );
                    }
                    other => panic!("unexpected event: {:?}", other),
                }
            }
            Err(other) => panic!("unexpected error: {:?}", other),
        }
    }

    // ── Mixed event types in sequence ────────────────────────────────

    #[tokio::test]
    async fn mixed_event_types_received_in_order() {
        let emitter = EventEmitter::new(16);
        let mut rx = emitter.subscribe();

        emitter.emit(SessionEvent::SessionStarted {
            session_id: "s1".into(),
        });
        emitter.emit(SessionEvent::TurnStarted { turn_number: 1 });
        emitter.emit(SessionEvent::TextDelta {
            text: "hello".into(),
        });

        // Verify order by checking each event type sequentially
        assert!(matches!(
            rx.recv().await.unwrap(),
            SessionEvent::SessionStarted { .. }
        ));
        assert!(matches!(
            rx.recv().await.unwrap(),
            SessionEvent::TurnStarted { turn_number: 1 }
        ));
        assert!(matches!(
            rx.recv().await.unwrap(),
            SessionEvent::TextDelta { .. }
        ));
    }

    // ── Dropped sender / closed channel ──────────────────────────────

    #[tokio::test]
    async fn receiver_gets_closed_error_when_emitter_dropped() {
        let emitter = EventEmitter::new(16);
        let mut rx = emitter.subscribe();

        // Emit one event, then drop the emitter
        emitter.emit(SessionEvent::TurnStarted { turn_number: 1 });
        drop(emitter);

        // Should still receive the buffered event
        let event = rx.recv().await.unwrap();
        assert!(matches!(event, SessionEvent::TurnStarted { turn_number: 1 }));

        // Next recv should fail with Closed
        let result = rx.recv().await;
        assert!(
            matches!(result, Err(broadcast::error::RecvError::Closed)),
            "expected Closed error after emitter dropped"
        );
    }
}
