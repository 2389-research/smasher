// ABOUTME: Ingestion subcommand that converts English requirements into DOT pipeline files.
// ABOUTME: Uses an LLM to decompose natural language into a valid smasher pipeline graph.

use clap::Args;

use crate::error::CliError;

/// The default skill file embedded at compile time.
const DEFAULT_SKILL: &str = include_str!("../../../skills/english-to-dotfile.md");

/// Convert English requirements into a DOT pipeline file using an LLM.
#[derive(Debug, Args)]
pub struct IngestArgs {
    /// The English-language requirements to convert into a pipeline.
    #[arg()]
    pub requirements: String,

    /// Output file path. Writes to stdout if omitted.
    #[arg(short, long)]
    pub output: Option<String>,

    /// Model identifier for the LLM.
    #[arg(long, default_value = "claude-sonnet-4-20250514")]
    pub model: String,

    /// Path to a custom skill file. Uses the built-in skill if omitted.
    #[arg(long)]
    pub skill: Option<String>,
}

/// Extract the first complete `digraph { ... }` block from LLM output text.
///
/// Scans for the literal string "digraph", then tracks brace depth to find
/// the matching closing brace. Quoted strings (double or single quotes) are
/// handled so that braces inside quotes do not affect depth counting.
///
/// Returns `None` if no valid digraph block is found.
pub fn extract_digraph(text: &str) -> Option<String> {
    let digraph_start = text.find("digraph")?;
    let remainder = &text[digraph_start..];
    let bytes = remainder.as_bytes();

    let mut depth: i32 = 0;
    let mut started = false;
    let mut in_double_quote = false;
    let mut in_single_quote = false;
    let mut escaped = false;
    let mut end_pos = 0;

    for (i, &b) in bytes.iter().enumerate() {
        if escaped {
            escaped = false;
            continue;
        }

        if b == b'\\' && (in_double_quote || in_single_quote) {
            escaped = true;
            continue;
        }

        if b == b'"' && !in_single_quote {
            in_double_quote = !in_double_quote;
            continue;
        }

        if b == b'\'' && !in_double_quote {
            in_single_quote = !in_single_quote;
            continue;
        }

        if in_double_quote || in_single_quote {
            continue;
        }

        if b == b'{' {
            depth += 1;
            started = true;
        } else if b == b'}' {
            depth -= 1;
            if started && depth == 0 {
                end_pos = i + 1;
                break;
            }
        }
    }

    if started && depth == 0 && end_pos > 0 {
        Some(remainder[..end_pos].to_string())
    } else {
        None
    }
}

/// Execute the ingest subcommand.
pub async fn run(args: IngestArgs) -> Result<(), CliError> {
    // Load the skill content.
    let skill_content = match &args.skill {
        Some(path) => std::fs::read_to_string(path)?,
        None => DEFAULT_SKILL.to_string(),
    };

    // Build the LLM client from environment.
    let client = smasher_llm::client::Client::from_env();
    if client.registered_providers().is_empty() {
        return Err(CliError::Other(
            "no API keys found. Set ANTHROPIC_API_KEY, OPENAI_API_KEY, or GEMINI_API_KEY.".into(),
        ));
    }

    // Build the request with skill as system prompt and requirements as user message.
    let messages = vec![smasher_llm::types::Message::user(&args.requirements)];
    let request = smasher_llm::types::Request::new(&args.model, messages)
        .system_prompt(&skill_content)
        .max_tokens(8192);

    // Call the LLM.
    let response = client.complete(request).await?;
    let response_text = response
        .text()
        .ok_or_else(|| CliError::Other("LLM returned no text content".into()))?;

    // Extract the digraph from the response.
    let dot_source = extract_digraph(&response_text).ok_or_else(|| {
        CliError::Other(format!(
            "could not extract a digraph from LLM response. Raw response:\n{response_text}"
        ))
    })?;

    // Validate by parsing and resolving the DOT.
    let dot_graph = smasher_attractor::dot::parser::parse(&dot_source)?;
    let _graph = smasher_attractor::graph::resolve(&dot_graph)?;

    // Output the validated DOT.
    match &args.output {
        Some(path) => {
            std::fs::write(path, &dot_source)?;
            eprintln!("Pipeline written to {path}");
        }
        None => {
            println!("{dot_source}");
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // extract_digraph tests
    // -----------------------------------------------------------------------

    #[test]
    fn extract_simple_digraph() {
        let text = r#"digraph simple {
    start [shape=circle];
    exit [shape=doublecircle];
    start -> exit;
}"#;
        let result = extract_digraph(text);
        assert!(result.is_some());
        let dot = result.unwrap();
        assert!(dot.starts_with("digraph simple {"));
        assert!(dot.ends_with('}'));
        assert!(dot.contains("start -> exit"));
    }

    #[test]
    fn extract_digraph_from_surrounding_text() {
        let text = r#"Here is your pipeline:

```dot
digraph my_pipeline {
    start [shape=Mdiamond, label="Start"];
    work [shape=box, label="Work"];
    done [shape=Msquare, label="Done"];
    start -> work -> done;
}
```

This pipeline should work for your needs."#;

        let result = extract_digraph(text);
        assert!(result.is_some());
        let dot = result.unwrap();
        assert!(dot.starts_with("digraph my_pipeline {"));
        assert!(dot.ends_with('}'));
        assert!(dot.contains("start -> work -> done"));
    }

    #[test]
    fn extract_digraph_with_quoted_braces() {
        let text = r#"digraph test {
    a [label="value with {braces} inside"];
    b [label="another {nested} one"];
    a -> b;
}"#;
        let result = extract_digraph(text);
        assert!(result.is_some());
        let dot = result.unwrap();
        assert!(dot.starts_with("digraph test {"));
        assert!(dot.ends_with('}'));
        assert!(dot.contains(r#"label="value with {braces} inside""#));
    }

    #[test]
    fn extract_digraph_with_single_quoted_strings() {
        let text = r#"digraph test {
    a [label='value with {braces} inside'];
    a -> b;
}"#;
        let result = extract_digraph(text);
        assert!(result.is_some());
        let dot = result.unwrap();
        assert!(dot.ends_with('}'));
    }

    #[test]
    fn extract_digraph_returns_none_when_missing() {
        let text = "This text has no digraph at all.";
        assert!(extract_digraph(text).is_none());
    }

    #[test]
    fn extract_digraph_returns_none_for_unclosed_brace() {
        let text = "digraph broken { a -> b;";
        assert!(extract_digraph(text).is_none());
    }

    #[test]
    fn extract_first_digraph_when_multiple_present() {
        let text = r#"digraph first {
    a -> b;
}

digraph second {
    c -> d;
}"#;
        let result = extract_digraph(text);
        assert!(result.is_some());
        let dot = result.unwrap();
        assert!(dot.starts_with("digraph first {"));
        assert!(dot.contains("a -> b"));
        assert!(!dot.contains("c -> d"));
    }

    #[test]
    fn extract_digraph_with_escaped_quotes() {
        let text = r#"digraph test {
    a [label="say \"hello\" world"];
    a -> b;
}"#;
        let result = extract_digraph(text);
        assert!(result.is_some());
        let dot = result.unwrap();
        assert!(dot.ends_with('}'));
        assert!(dot.contains("say \\\"hello\\\" world"));
    }

    #[test]
    fn extract_digraph_unnamed() {
        let text = r#"digraph {
    start -> exit;
}"#;
        let result = extract_digraph(text);
        assert!(result.is_some());
        let dot = result.unwrap();
        assert!(dot.starts_with("digraph {"));
    }

    #[test]
    fn extract_digraph_complex_multiline_prompt() {
        let text = r#"Here's a pipeline for your request:

digraph build_app {
    graph [goal="Build the app"]
    rankdir=LR

    start [shape=Mdiamond, label="Start"]
    done  [shape=Msquare, label="Done"]

    plan [
        shape=box,
        label="Plan",
        prompt="Create a detailed plan.\nInclude:\n- Architecture\n- Components\n- Tests"
    ]

    implement [
        shape=box,
        label="Implement",
        prompt="Implement the app based on the plan."
    ]

    start -> plan -> implement -> done
}

Let me know if you need changes!"#;

        let result = extract_digraph(text);
        assert!(result.is_some());
        let dot = result.unwrap();
        assert!(dot.starts_with("digraph build_app {"));
        assert!(dot.ends_with('}'));
        assert!(dot.contains("plan -> implement -> done"));
    }

    #[test]
    fn extract_digraph_with_nested_subgraph() {
        let text = r#"digraph outer {
    subgraph cluster_0 {
        a; b;
    }
    a -> b;
}"#;
        let result = extract_digraph(text);
        assert!(result.is_some());
        let dot = result.unwrap();
        assert!(dot.starts_with("digraph outer {"));
        assert!(dot.ends_with('}'));
        assert!(dot.contains("subgraph cluster_0 {"));
    }

    // -----------------------------------------------------------------------
    // Skill file embedding tests
    // -----------------------------------------------------------------------

    #[test]
    fn default_skill_is_not_empty() {
        assert!(!DEFAULT_SKILL.is_empty());
    }

    #[test]
    fn default_skill_contains_digraph_keyword() {
        assert!(
            DEFAULT_SKILL.contains("digraph"),
            "skill file should contain 'digraph' as reference material"
        );
    }

    #[test]
    fn default_skill_contains_shape_documentation() {
        assert!(
            DEFAULT_SKILL.contains("shape"),
            "skill file should document node shapes"
        );
        assert!(
            DEFAULT_SKILL.contains("Codergen"),
            "skill file should mention Codergen node type"
        );
        assert!(
            DEFAULT_SKILL.contains("Conditional"),
            "skill file should mention Conditional node type"
        );
    }

    #[test]
    fn default_skill_mentions_edge_attributes() {
        assert!(
            DEFAULT_SKILL.contains("loop_restart"),
            "skill file should document loop_restart edge attribute"
        );
        assert!(
            DEFAULT_SKILL.contains("condition"),
            "skill file should document condition edge attribute"
        );
    }

    // -----------------------------------------------------------------------
    // DOT validation round-trip tests
    // -----------------------------------------------------------------------

    #[test]
    fn extracted_dot_passes_parser() {
        let text = r#"digraph roundtrip {
    start [shape=circle, label="Start"];
    work [shape=box, label="Work", prompt="Do the thing"];
    done [shape=doublecircle, label="Done"];
    start -> work;
    work -> done;
}"#;
        let dot = extract_digraph(text).expect("should extract");
        let parsed = smasher_attractor::dot::parser::parse(&dot);
        assert!(
            parsed.is_ok(),
            "parser should accept the extracted DOT: {parsed:?}"
        );
    }

    #[test]
    fn extracted_dot_passes_resolver() {
        let text = r#"digraph roundtrip {
    start [shape=circle, label="Start"];
    work [shape=box, label="Work", prompt="Do the thing"];
    done [shape=doublecircle, label="Done"];
    start -> work;
    work -> done;
}"#;
        let dot = extract_digraph(text).expect("should extract");
        let parsed = smasher_attractor::dot::parser::parse(&dot).expect("should parse");
        let resolved = smasher_attractor::graph::resolve(&parsed);
        assert!(
            resolved.is_ok(),
            "resolver should accept the parsed graph: {resolved:?}"
        );
    }

    #[test]
    fn extracted_complex_pipeline_validates() {
        let text = r#"digraph complex {
    graph [goal="Build something great"]
    rankdir=LR

    start [shape=Mdiamond, label="Start"]
    done  [shape=Msquare, label="Done"]

    plan [shape=box, label="Plan", prompt="Make a plan"]
    implement [shape=box, label="Implement", prompt="Write code", goal_gate=true]
    test [shape=box, label="Test", prompt="Run tests"]
    check [shape=diamond, label="Tests Pass?"]

    start -> plan -> implement -> test -> check
    check -> done      [label="success"]
    check -> implement [label="failure", loop_restart=true]
}"#;
        let dot = extract_digraph(text).expect("should extract");
        let parsed = smasher_attractor::dot::parser::parse(&dot).expect("should parse");
        let graph = smasher_attractor::graph::resolve(&parsed).expect("should resolve");
        assert_eq!(graph.name, Some("complex".to_string()));
        assert!(!graph.nodes.is_empty());
        assert!(!graph.edges.is_empty());
    }

    // -----------------------------------------------------------------------
    // CLI arg parsing tests
    // -----------------------------------------------------------------------

    #[test]
    fn cli_args_parse_minimal() {
        use clap::Parser;

        #[derive(Parser)]
        struct TestCli {
            #[command(flatten)]
            args: IngestArgs,
        }

        let cli = TestCli::parse_from(["test", "Build a game"]);
        assert_eq!(cli.args.requirements, "Build a game");
        assert!(cli.args.output.is_none());
        assert_eq!(cli.args.model, "claude-sonnet-4-20250514");
        assert!(cli.args.skill.is_none());
    }

    #[test]
    fn cli_args_parse_with_output() {
        use clap::Parser;

        #[derive(Parser)]
        struct TestCli {
            #[command(flatten)]
            args: IngestArgs,
        }

        let cli = TestCli::parse_from(["test", "-o", "pipeline.dot", "Build a game"]);
        assert_eq!(cli.args.output, Some("pipeline.dot".to_string()));
    }

    #[test]
    fn cli_args_parse_with_model() {
        use clap::Parser;

        #[derive(Parser)]
        struct TestCli {
            #[command(flatten)]
            args: IngestArgs,
        }

        let cli = TestCli::parse_from(["test", "--model", "gpt-4o", "Build a game"]);
        assert_eq!(cli.args.model, "gpt-4o");
    }

    #[test]
    fn cli_args_parse_with_skill() {
        use clap::Parser;

        #[derive(Parser)]
        struct TestCli {
            #[command(flatten)]
            args: IngestArgs,
        }

        let cli = TestCli::parse_from(["test", "--skill", "/path/to/skill.md", "Build a game"]);
        assert_eq!(cli.args.skill, Some("/path/to/skill.md".to_string()));
    }

    #[test]
    fn cli_args_parse_all_options() {
        use clap::Parser;

        #[derive(Parser)]
        struct TestCli {
            #[command(flatten)]
            args: IngestArgs,
        }

        let cli = TestCli::parse_from([
            "test",
            "--model",
            "claude-opus-4-6",
            "--skill",
            "custom.md",
            "-o",
            "out.dot",
            "Build a solitaire game",
        ]);
        assert_eq!(cli.args.requirements, "Build a solitaire game");
        assert_eq!(cli.args.model, "claude-opus-4-6");
        assert_eq!(cli.args.skill, Some("custom.md".to_string()));
        assert_eq!(cli.args.output, Some("out.dot".to_string()));
    }
}
