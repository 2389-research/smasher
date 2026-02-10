// ABOUTME: Detects when the agent is stuck in a repetitive tool-call loop.
// ABOUTME: Maintains a sliding window of tool signatures and checks for repeating patterns.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

/// A compact fingerprint of a single tool invocation.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ToolSignature {
    pub name: String,
    pub arguments_hash: u64,
}

/// Description of a detected repeating pattern.
#[derive(Debug, Clone)]
pub struct LoopPattern {
    /// The repeating sequence of tool signatures.
    pub pattern: Vec<ToolSignature>,
    /// How many times it repeated.
    pub repetitions: usize,
    /// Human-readable description of the pattern.
    pub description: String,
}

/// Watches recent tool calls for repetitive patterns.
pub struct LoopDetector {
    /// Recent tool call signatures.
    history: Vec<ToolSignature>,
    /// Maximum history length to maintain.
    window_size: usize,
    /// Minimum pattern repetitions to trigger detection.
    min_repetitions: usize,
}

impl Default for LoopDetector {
    fn default() -> Self {
        Self {
            history: Vec::new(),
            window_size: 20,
            min_repetitions: 3,
        }
    }
}

impl LoopDetector {
    /// Create a detector with custom window size and repetition threshold.
    pub fn new(window_size: usize, min_repetitions: usize) -> Self {
        Self {
            history: Vec::new(),
            window_size,
            min_repetitions,
        }
    }

    /// Record a tool call. Trims history to `window_size`.
    pub fn record(&mut self, name: &str, arguments: &str) {
        self.history.push(ToolSignature {
            name: name.to_string(),
            arguments_hash: hash_string(arguments),
        });
        if self.history.len() > self.window_size {
            let excess = self.history.len() - self.window_size;
            self.history.drain(..excess);
        }
    }

    /// Check for repeating patterns in the history.
    ///
    /// Returns the shortest pattern whose trailing consecutive repetitions
    /// meet or exceed `min_repetitions`, or `None` if no loop is found.
    pub fn detect_loop(&self) -> Option<LoopPattern> {
        if self.history.is_empty() {
            return None;
        }

        let max_pattern_len = self.history.len() / self.min_repetitions;

        for pattern_len in 1..=max_pattern_len {
            let reps = count_trailing_repetitions(&self.history, pattern_len);
            if reps >= self.min_repetitions {
                let pattern = self.history[self.history.len() - pattern_len..].to_vec();
                return Some(LoopPattern {
                    description: format!("{} repeated {} times", format_pattern(&pattern), reps,),
                    pattern,
                    repetitions: reps,
                });
            }
        }

        None
    }

    /// Clear all recorded history.
    pub fn reset(&mut self) {
        self.history.clear();
    }

    /// Read-only access to the current history (useful for tests).
    #[cfg(test)]
    fn history(&self) -> &[ToolSignature] {
        &self.history
    }
}

/// Count how many times the last `pattern_len` signatures repeat consecutively
/// at the tail of `history`.
fn count_trailing_repetitions(history: &[ToolSignature], pattern_len: usize) -> usize {
    if pattern_len == 0 || history.len() < pattern_len {
        return 0;
    }
    let pattern = &history[history.len() - pattern_len..];
    let mut count = 1;
    let mut pos = history.len() - pattern_len;
    while pos >= pattern_len {
        pos -= pattern_len;
        if &history[pos..pos + pattern_len] == pattern {
            count += 1;
        } else {
            break;
        }
    }
    count
}

/// Format a pattern for human readability, e.g. `"read_file -> edit_file"`.
pub fn format_pattern(pattern: &[ToolSignature]) -> String {
    pattern
        .iter()
        .map(|sig| sig.name.as_str())
        .collect::<Vec<_>>()
        .join(" -> ")
}

/// Deterministic hash of a string (used to fingerprint tool arguments).
fn hash_string(s: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    s.hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Construction ──────────────────────────────────────────────────

    #[test]
    fn new_with_custom_parameters() {
        let det = LoopDetector::new(10, 5);
        assert_eq!(det.window_size, 10);
        assert_eq!(det.min_repetitions, 5);
        assert!(det.history().is_empty());
    }

    #[test]
    fn default_has_expected_values() {
        let det = LoopDetector::default();
        assert_eq!(det.window_size, 20);
        assert_eq!(det.min_repetitions, 3);
        assert!(det.history().is_empty());
    }

    // ── Recording ─────────────────────────────────────────────────────

    #[test]
    fn record_adds_signatures_to_history() {
        let mut det = LoopDetector::new(10, 3);
        det.record("read_file", r#"{"path":"foo.rs"}"#);
        det.record("edit_file", r#"{"path":"foo.rs"}"#);
        assert_eq!(det.history().len(), 2);
        assert_eq!(det.history()[0].name, "read_file");
        assert_eq!(det.history()[1].name, "edit_file");
    }

    #[test]
    fn history_is_trimmed_to_window_size() {
        let mut det = LoopDetector::new(3, 2);
        for i in 0..5 {
            det.record("tool", &format!("arg_{i}"));
        }
        assert_eq!(det.history().len(), 3);
        // The oldest two entries should have been dropped.
        assert_eq!(det.history()[0].arguments_hash, hash_string("arg_2"));
    }

    // ── Loop detection ────────────────────────────────────────────────

    #[test]
    fn single_tool_repeated_detects_loop() {
        let mut det = LoopDetector::new(20, 3);
        for _ in 0..3 {
            det.record("bash", r#"{"cmd":"ls"}"#);
        }
        let lp = det.detect_loop().expect("should detect loop");
        assert_eq!(lp.pattern.len(), 1);
        assert_eq!(lp.pattern[0].name, "bash");
        assert_eq!(lp.repetitions, 3);
    }

    #[test]
    fn two_tool_pattern_detects_loop() {
        let mut det = LoopDetector::new(20, 3);
        for _ in 0..3 {
            det.record("read_file", r#"{"path":"a.rs"}"#);
            det.record("edit_file", r#"{"path":"a.rs"}"#);
        }
        let lp = det.detect_loop().expect("should detect loop");
        assert_eq!(lp.pattern.len(), 2);
        assert_eq!(lp.pattern[0].name, "read_file");
        assert_eq!(lp.pattern[1].name, "edit_file");
        assert_eq!(lp.repetitions, 3);
    }

    #[test]
    fn three_tool_pattern_detection() {
        let mut det = LoopDetector::new(30, 3);
        for _ in 0..3 {
            det.record("read_file", "a");
            det.record("edit_file", "b");
            det.record("bash", "c");
        }
        let lp = det.detect_loop().expect("should detect loop");
        assert_eq!(lp.pattern.len(), 3);
        assert_eq!(lp.repetitions, 3);
    }

    #[test]
    fn no_loop_when_insufficient_repetitions() {
        let mut det = LoopDetector::new(20, 3);
        // Only 2 repetitions; threshold is 3.
        det.record("bash", "ls");
        det.record("bash", "ls");
        assert!(det.detect_loop().is_none());
    }

    #[test]
    fn no_loop_on_empty_history() {
        let det = LoopDetector::default();
        assert!(det.detect_loop().is_none());
    }

    // ── Reset ─────────────────────────────────────────────────────────

    #[test]
    fn reset_clears_history() {
        let mut det = LoopDetector::default();
        det.record("bash", "ls");
        det.record("bash", "ls");
        det.reset();
        assert!(det.history().is_empty());
        assert!(det.detect_loop().is_none());
    }

    // ── Signature equality / hashing ──────────────────────────────────

    #[test]
    fn different_arguments_produce_different_signatures() {
        let mut det = LoopDetector::new(10, 3);
        det.record("bash", r#"{"cmd":"ls"}"#);
        det.record("bash", r#"{"cmd":"pwd"}"#);
        assert_ne!(
            det.history()[0].arguments_hash,
            det.history()[1].arguments_hash,
        );
    }

    #[test]
    fn same_arguments_produce_same_signatures() {
        let mut det = LoopDetector::new(10, 3);
        det.record("bash", r#"{"cmd":"ls"}"#);
        det.record("bash", r#"{"cmd":"ls"}"#);
        assert_eq!(
            det.history()[0].arguments_hash,
            det.history()[1].arguments_hash,
        );
        assert_eq!(det.history()[0], det.history()[1]);
    }

    // ── Shortest pattern wins ─────────────────────────────────────────

    #[test]
    fn pattern_detection_returns_shortest_pattern() {
        // A, A, A, A, A, A — pattern len 1 ([A]) is shorter than len 2 ([A, A]).
        let mut det = LoopDetector::new(20, 3);
        for _ in 0..6 {
            det.record("bash", "ls");
        }
        let lp = det.detect_loop().expect("should detect loop");
        assert_eq!(lp.pattern.len(), 1, "shortest pattern should be returned");
        assert_eq!(lp.repetitions, 6);
    }

    // ── format_pattern ────────────────────────────────────────────────

    #[test]
    fn format_pattern_produces_readable_string() {
        let sigs = vec![
            ToolSignature {
                name: "read_file".into(),
                arguments_hash: 0,
            },
            ToolSignature {
                name: "edit_file".into(),
                arguments_hash: 0,
            },
            ToolSignature {
                name: "read_file".into(),
                arguments_hash: 0,
            },
        ];
        assert_eq!(format_pattern(&sigs), "read_file -> edit_file -> read_file",);
    }

    // ── detect_loop returns None when no pattern ──────────────────────

    #[test]
    fn detect_loop_returns_none_when_no_pattern() {
        let mut det = LoopDetector::new(20, 3);
        det.record("read_file", "a");
        det.record("edit_file", "b");
        det.record("bash", "c");
        det.record("grep", "d");
        assert!(det.detect_loop().is_none());
    }

    // ── Mixed calls with pattern at end ───────────────────────────────

    #[test]
    fn mixed_calls_with_pattern_at_end() {
        let mut det = LoopDetector::new(20, 3);
        // Some noise first.
        det.record("grep", "search");
        det.record("write_file", "output");
        // Then a repeating pattern.
        for _ in 0..4 {
            det.record("read_file", "x");
            det.record("bash", "compile");
        }
        let lp = det.detect_loop().expect("should detect trailing loop");
        assert_eq!(lp.pattern.len(), 2);
        assert_eq!(lp.pattern[0].name, "read_file");
        assert_eq!(lp.pattern[1].name, "bash");
        assert_eq!(lp.repetitions, 4);
    }
}
