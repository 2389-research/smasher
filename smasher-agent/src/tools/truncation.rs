// ABOUTME: Output truncation logic for tool results that exceed length limits.
// ABOUTME: Preserves the beginning and end of output while inserting a truncation marker.

/// Find a valid UTF-8 boundary at or before the given byte index.
fn floor_char_boundary(s: &str, index: usize) -> usize {
    let index = index.min(s.len());
    let mut boundary = index;
    while boundary > 0 && !s.is_char_boundary(boundary) {
        boundary -= 1;
    }
    boundary
}

/// Find a valid UTF-8 boundary at or after the given byte index.
fn ceil_char_boundary(s: &str, index: usize) -> usize {
    let index = index.min(s.len());
    let mut boundary = index;
    while boundary < s.len() && !s.is_char_boundary(boundary) {
        boundary += 1;
    }
    boundary
}

/// Truncate output to fit within a character limit.
///
/// Strategy:
/// 1. If content fits, return as-is
/// 2. Keep the first ~2/3 and the last ~1/4, inserting a truncation marker in between.
///    This preserves the beginning (usually most important) and the end (often error messages).
pub fn truncate_output(content: &str, max_chars: usize) -> String {
    if content.len() <= max_chars {
        return content.to_string();
    }

    // Calculate how much content was omitted for the marker text.
    let preliminary_head = max_chars * 2 / 3;
    let preliminary_tail = max_chars / 4;
    let omitted = content
        .len()
        .saturating_sub(preliminary_head + preliminary_tail);
    let marker = format!("\n\n[... truncated {} characters ...]\n\n", omitted);

    // Now recalculate head and tail sizes accounting for the marker itself.
    let available = max_chars.saturating_sub(marker.len());
    let head_size = available * 2 / 3;
    let tail_size = available.saturating_sub(head_size);

    // Snap to valid UTF-8 boundaries.
    let head_end = floor_char_boundary(content, head_size);
    let tail_start = ceil_char_boundary(content, content.len().saturating_sub(tail_size));

    format!(
        "{}{}{}",
        &content[..head_end],
        marker,
        &content[tail_start..]
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_content_returned_as_is() {
        let content = "hello world";
        let result = truncate_output(content, 100);
        assert_eq!(result, content);
    }

    #[test]
    fn long_content_is_truncated_with_marker() {
        let content = "a".repeat(1000);
        let result = truncate_output(&content, 200);
        assert!(result.contains("[... truncated"));
        assert!(result.contains("characters ...]"));
    }

    #[test]
    fn truncated_output_is_within_max_length() {
        let content = "x".repeat(10_000);
        let max = 500;
        let result = truncate_output(&content, max);
        // Allow some slack for the marker, but should be roughly within max.
        // The result should be less than max + marker overhead.
        assert!(
            result.len() <= max + 50,
            "result len {} exceeded max {} by more than margin",
            result.len(),
            max
        );
    }

    #[test]
    fn truncated_output_preserves_beginning() {
        let content = format!("BEGINNING{}", "x".repeat(10_000));
        let result = truncate_output(&content, 500);
        assert!(
            result.starts_with("BEGINNING"),
            "result should start with 'BEGINNING', got: {}",
            &result[..20.min(result.len())]
        );
    }

    #[test]
    fn truncated_output_preserves_end() {
        let content = format!("{}ENDING", "x".repeat(10_000));
        let result = truncate_output(&content, 500);
        assert!(
            result.ends_with("ENDING"),
            "result should end with 'ENDING', got: {}",
            &result[result.len().saturating_sub(20)..]
        );
    }

    #[test]
    fn multibyte_characters_do_not_cause_panics() {
        // Each emoji is 4 bytes in UTF-8.
        let content = "\u{1F600}".repeat(500); // 2000 bytes
        let result = truncate_output(&content, 200);
        // Should not panic and should be valid UTF-8.
        assert!(!result.is_empty());
        // Verify it's valid UTF-8 by iterating chars.
        let _: Vec<char> = result.chars().collect();
    }

    #[test]
    fn empty_string_returns_empty() {
        let result = truncate_output("", 100);
        assert_eq!(result, "");
    }

    #[test]
    fn content_exactly_at_limit_is_not_truncated() {
        let content = "a".repeat(100);
        let result = truncate_output(&content, 100);
        assert_eq!(result, content);
    }
}
