# Spec Compliance Fixes Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Fix all Critical and High severity gaps between smasher and the upstream strongdm/attractor spec.

**Architecture:** Each task is scoped to a single crate and a single concern. Tasks are ordered by dependency — attractor engine types first (Outcome, shape mappings), then edge selection, then LLM client, then agent loop. Each task is independently testable and committable.

**Tech Stack:** Rust 1.92, tokio, serde, async-trait, thiserror

---

## Task 1: Expand Outcome enum with missing fields and variants

**Files:**
- Modify: `crates/smasher-attractor/src/state.rs:290-356`
- Test: same file, `#[cfg(test)] mod tests`

**Why:** The spec defines 5 outcome statuses (SUCCESS, PARTIAL_SUCCESS, RETRY, FAIL, SKIPPED) and additional fields (`preferred_label`, `suggested_next_ids`, `context_updates`, `notes`). We only have 3 variants and no extra fields.

**Step 1: Write failing tests**

```rust
#[test]
fn outcome_partial_success_variant() {
    let o = Outcome::partial_success();
    assert!(o.is_success()); // partial success counts as success for goal gates
    assert!(!o.is_failure());
}

#[test]
fn outcome_retry_variant() {
    let o = Outcome::retry("try again");
    assert!(!o.is_success());
    assert!(o.is_retryable());
}

#[test]
fn outcome_preferred_label() {
    let o = Outcome::success().with_preferred_label("deploy");
    assert_eq!(o.preferred_label(), Some("deploy"));
}

#[test]
fn outcome_suggested_next_ids() {
    let o = Outcome::success().with_suggested_next_ids(vec!["node_b".into()]);
    assert_eq!(o.suggested_next_ids(), Some(&vec!["node_b".to_string()]));
}

#[test]
fn outcome_context_updates() {
    let mut updates = HashMap::new();
    updates.insert("key".to_string(), json!("val"));
    let o = Outcome::success().with_context_updates(updates.clone());
    assert_eq!(o.context_updates(), Some(&updates));
}

#[test]
fn outcome_notes() {
    let o = Outcome::success().with_notes("completed code review");
    assert_eq!(o.notes(), Some("completed code review"));
}

#[test]
fn outcome_serde_roundtrip_partial_success() {
    let o = Outcome::partial_success();
    let json = serde_json::to_string(&o).unwrap();
    let back: Outcome = serde_json::from_str(&json).unwrap();
    assert_eq!(o, back);
}

#[test]
fn outcome_serde_roundtrip_retry() {
    let o = Outcome::retry("rate limited");
    let json = serde_json::to_string(&o).unwrap();
    let back: Outcome = serde_json::from_str(&json).unwrap();
    assert_eq!(o, back);
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test -p smasher-attractor outcome_partial_success -- --nocapture`
Expected: FAIL — `partial_success` not found

**Step 3: Implement the expanded Outcome**

Replace the existing Outcome enum with:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Outcome {
    Success {
        data: Option<serde_json::Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        preferred_label: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        suggested_next_ids: Option<Vec<String>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        context_updates: Option<HashMap<String, serde_json::Value>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        notes: Option<String>,
    },
    PartialSuccess {
        data: Option<serde_json::Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        preferred_label: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        suggested_next_ids: Option<Vec<String>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        context_updates: Option<HashMap<String, serde_json::Value>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        notes: Option<String>,
    },
    Retry {
        reason: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        notes: Option<String>,
    },
    Failure {
        error: String,
        retryable: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        notes: Option<String>,
    },
    Skip {
        reason: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        notes: Option<String>,
    },
}
```

Add builder methods:

```rust
impl Outcome {
    pub fn success() -> Self {
        Self::Success { data: None, preferred_label: None, suggested_next_ids: None, context_updates: None, notes: None }
    }
    pub fn success_with(data: serde_json::Value) -> Self {
        Self::Success { data: Some(data), preferred_label: None, suggested_next_ids: None, context_updates: None, notes: None }
    }
    pub fn partial_success() -> Self {
        Self::PartialSuccess { data: None, preferred_label: None, suggested_next_ids: None, context_updates: None, notes: None }
    }
    pub fn retry(reason: impl Into<String>) -> Self {
        Self::Retry { reason: reason.into(), notes: None }
    }
    pub fn failure(error: impl Into<String>) -> Self {
        Self::Failure { error: error.into(), retryable: false, notes: None }
    }
    pub fn retryable_failure(error: impl Into<String>) -> Self {
        Self::Failure { error: error.into(), retryable: true, notes: None }
    }
    pub fn skip(reason: impl Into<String>) -> Self {
        Self::Skip { reason: reason.into(), notes: None }
    }

    // Builder-style setters that work on any variant
    pub fn with_preferred_label(mut self, label: impl Into<String>) -> Self { ... }
    pub fn with_suggested_next_ids(mut self, ids: Vec<String>) -> Self { ... }
    pub fn with_context_updates(mut self, updates: HashMap<String, serde_json::Value>) -> Self { ... }
    pub fn with_notes(mut self, notes: impl Into<String>) -> Self { ... }

    // Accessors
    pub fn preferred_label(&self) -> Option<&str> { ... }
    pub fn suggested_next_ids(&self) -> Option<&Vec<String>> { ... }
    pub fn context_updates(&self) -> Option<&HashMap<String, serde_json::Value>> { ... }
    pub fn notes(&self) -> Option<&str> { ... }

    pub fn is_success(&self) -> bool {
        matches!(self, Self::Success { .. } | Self::PartialSuccess { .. })
    }
    pub fn is_failure(&self) -> bool {
        matches!(self, Self::Failure { .. })
    }
    pub fn is_retryable(&self) -> bool {
        matches!(self, Self::Failure { retryable: true, .. } | Self::Retry { .. })
    }
}
```

**Step 4: Fix all call sites across workspace**

The `Outcome::Success { data }` destructure pattern is used in tests and handlers throughout the codebase. Each must be updated to include the new fields or use `..` rest syntax. Grep for `Outcome::Success {` and `Outcome::Failure {` and `Outcome::Skip {` to find all sites.

**Step 5: Run tests to verify they pass**

Run: `cargo test --workspace`
Expected: ALL PASS

**Step 6: Commit**

```bash
git add -A && git commit -m "feat(attractor): expand Outcome with PartialSuccess, Retry, preferred_label, suggested_next_ids, context_updates, notes"
```

---

## Task 2: Fix shape-to-NodeType mappings

**Files:**
- Modify: `crates/smasher-attractor/src/graph/mod.rs:175-187`
- Test: same file, `#[cfg(test)] mod tests` (lines 437-467)

**Why:** Three shapes map to the wrong handler type. `component` should be Parallel, `tripleoctagon` should be a new FanIn type, `parallelogram` should be Tool.

**Step 1: Write failing tests**

```rust
#[test]
fn shape_parallelogram_maps_to_tool() {
    assert_eq!(node_type_from_shape("parallelogram"), NodeType::Tool);
}

#[test]
fn shape_component_maps_to_parallel() {
    assert_eq!(node_type_from_shape("component"), NodeType::Parallel);
}

#[test]
fn shape_tripleoctagon_maps_to_fan_in() {
    assert_eq!(node_type_from_shape("tripleoctagon"), NodeType::FanIn);
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test -p smasher-attractor shape_parallelogram`
Expected: FAIL

**Step 3: Add FanIn to NodeType enum and fix mappings**

In `graph/mod.rs`, add `FanIn` variant to NodeType:

```rust
pub enum NodeType {
    Start,
    Exit,
    Codergen,
    Conditional,
    Tool,
    Interviewer,
    Parallel,
    FanIn,
    Manager,
    SubPipeline,
    Generic,
}
```

Fix the mapping function:

```rust
fn node_type_from_shape(shape: &str) -> NodeType {
    match shape {
        "circle" | "point" | "Mdiamond" => NodeType::Start,
        "doublecircle" | "Msquare" => NodeType::Exit,
        "box" | "rectangle" => NodeType::Codergen,
        "diamond" => NodeType::Conditional,
        "hexagon" | "oval" | "ellipse" => NodeType::Interviewer,
        "parallelogram" => NodeType::Tool,
        "component" => NodeType::Parallel,
        "tripleoctagon" => NodeType::FanIn,
        "house" => NodeType::Manager,
        _ => NodeType::Generic,
    }
}
```

Note: `SubPipeline` is no longer assigned by shape — it's set via the `child_dotfile` attribute or explicit `type` attribute (handled elsewhere). Default to Generic for unknown shapes.

**Step 4: Fix all references to old mappings**

Grep for `NodeType::SubPipeline` and `NodeType::Parallel` across the codebase. Update handler `handles()` methods and tests to account for the new FanIn type and the removal of the `component`→SubPipeline mapping.

Update existing shape mapping tests that expected the old behavior.

**Step 5: Run tests**

Run: `cargo test --workspace`
Expected: ALL PASS

**Step 6: Commit**

```bash
git add -A && git commit -m "fix(attractor): correct shape-to-NodeType mappings per spec (component=Parallel, parallelogram=Tool, tripleoctagon=FanIn)"
```

---

## Task 3: Implement 5-step edge selection algorithm

**Files:**
- Modify: `crates/smasher-attractor/src/edge.rs:51-120`
- Test: same file, `#[cfg(test)] mod tests`

**Why:** The current algorithm doesn't implement preferred_label matching, suggested_next_ids, label normalization with accelerator stripping, or lexical tiebreaking. The spec's 5-step algorithm is the core routing logic.

**Step 1: Write failing tests**

```rust
#[test]
fn edge_selection_preferred_label_match() {
    // Outcome with preferred_label="deploy" should match edge labeled "deploy"
    let graph = build_test_graph_with_labeled_edges("A", &[("B", "deploy"), ("C", "rollback")]);
    let ctx = Context::new();
    let outcome = Outcome::success().with_preferred_label("deploy");
    let selected = select_edge(&graph, "A", &ctx, Some(&outcome)).unwrap().unwrap();
    assert_eq!(selected.to, "B");
}

#[test]
fn edge_selection_preferred_label_with_accelerator() {
    // "[Y] Yes, deploy" should match preferred_label "yes, deploy"
    let graph = build_test_graph_with_labeled_edges("A", &[("B", "[Y] Yes, deploy"), ("C", "[N] No")]);
    let ctx = Context::new();
    let outcome = Outcome::success().with_preferred_label("yes, deploy");
    let selected = select_edge(&graph, "A", &ctx, Some(&outcome)).unwrap().unwrap();
    assert_eq!(selected.to, "B");
}

#[test]
fn edge_selection_suggested_next_ids() {
    let graph = build_test_graph_with_edges("A", &["B", "C", "D"]);
    let ctx = Context::new();
    let outcome = Outcome::success().with_suggested_next_ids(vec!["C".into()]);
    let selected = select_edge(&graph, "A", &ctx, Some(&outcome)).unwrap().unwrap();
    assert_eq!(selected.to, "C");
}

#[test]
fn edge_selection_lexical_tiebreak() {
    // Two unconditional edges with same priority → alphabetically first target wins
    let graph = build_test_graph_with_edges("A", &["Z_node", "A_node"]);
    let ctx = Context::new();
    let selected = select_edge(&graph, "A", &ctx, None).unwrap().unwrap();
    assert_eq!(selected.to, "A_node");
}

#[test]
fn edge_selection_conditions_before_unconditional() {
    // Edges with true conditions take priority over unconditional edges
    let graph = build_graph_with_conditional_and_unconditional_edges();
    let mut ctx = Context::new();
    ctx.set("outcome", json!("success"));
    let selected = select_edge(&graph, "A", &ctx, None).unwrap().unwrap();
    assert_eq!(selected.to, "B"); // B has condition outcome=success
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test -p smasher-attractor edge_selection_preferred_label`
Expected: FAIL

**Step 3: Implement the spec's 5-step algorithm**

Replace `select_edge` with the spec-compliant version:

```rust
pub fn select_edge<'a>(
    graph: &'a Graph,
    node_id: &str,
    context: &Context,
    last_outcome: Option<&Outcome>,
) -> Result<Option<&'a GraphEdge>, EdgeSelectionError> {
    let candidates = graph.edges_from(node_id);
    if candidates.is_empty() {
        return Ok(None);
    }

    let ctx_map = context.to_string_map();

    // Step 1: Separate conditional from unconditional edges.
    // Evaluate conditions — only edges whose conditions are TRUE pass.
    let mut conditional_passing: Vec<&GraphEdge> = Vec::new();
    let mut unconditional: Vec<&GraphEdge> = Vec::new();

    for edge in &candidates {
        if has_explicit_condition(edge) {
            let cond_str = edge.condition.as_ref().unwrap();
            let parsed = parse_condition(cond_str).map_err(|e| EdgeSelectionError::ConditionParseError {
                from: edge.from.clone(),
                to: edge.to.clone(),
                message: e.to_string(),
            })?;
            if evaluate_condition(&parsed, &ctx_map) {
                conditional_passing.push(edge);
            }
        } else {
            unconditional.push(edge);
        }
    }

    // If any conditional edges pass, use only those.
    if !conditional_passing.is_empty() {
        return Ok(Some(pick_best(&conditional_passing)));
    }

    // Step 2: Preferred label match (from outcome).
    if let Some(outcome) = last_outcome {
        if let Some(label) = outcome.preferred_label() {
            let normalized = normalize_label(label);
            let matched: Vec<&GraphEdge> = unconditional
                .iter()
                .copied()
                .filter(|e| e.label.as_ref().map(|l| normalize_label(l) == normalized).unwrap_or(false))
                .collect();
            if !matched.is_empty() {
                return Ok(Some(pick_best(&matched)));
            }
        }

        // Step 3: Suggested next IDs.
        if let Some(ids) = outcome.suggested_next_ids() {
            let matched: Vec<&GraphEdge> = unconditional
                .iter()
                .copied()
                .filter(|e| ids.contains(&e.to))
                .collect();
            if !matched.is_empty() {
                return Ok(Some(pick_best(&matched)));
            }
        }
    }

    // Step 4 & 5: Weight-based selection with lexical tiebreak.
    if unconditional.is_empty() {
        return Ok(None);
    }
    Ok(Some(pick_best(&unconditional)))
}

/// Sort by priority descending, then by target node ID ascending (lexical tiebreak).
fn pick_best<'a>(edges: &[&'a GraphEdge]) -> &'a GraphEdge {
    let mut sorted: Vec<&GraphEdge> = edges.to_vec();
    sorted.sort_by(|a, b| {
        let pa = a.priority.unwrap_or(0);
        let pb = b.priority.unwrap_or(0);
        pb.cmp(&pa).then_with(|| a.to.cmp(&b.to))
    });
    sorted[0]
}

/// Normalize a label: lowercase, trim, strip accelerator prefixes like "[Y] ", "Y) ", "Y - ".
fn normalize_label(label: &str) -> String {
    let trimmed = label.trim().to_lowercase();
    // Strip patterns: [X] prefix, X) prefix, X - prefix
    let re_bracket = regex::Regex::new(r"^\[[a-z0-9]\]\s*").unwrap();
    let re_paren = regex::Regex::new(r"^[a-z0-9]\)\s*").unwrap();
    let re_dash = regex::Regex::new(r"^[a-z0-9]\s*-\s*").unwrap();
    let s = re_bracket.replace(&trimmed, "");
    let s = re_paren.replace(&s, "");
    let s = re_dash.replace(&s, "");
    s.to_string()
}
```

**Step 4: Run tests**

Run: `cargo test --workspace`
Expected: ALL PASS

**Step 5: Commit**

```bash
git add -A && git commit -m "feat(attractor): implement spec-compliant 5-step edge selection with preferred_label, suggested_next_ids, lexical tiebreak"
```

---

## Task 4: Fix ConditionalHandler to be a no-op

**Files:**
- Modify: `crates/smasher-attractor/src/handler.rs:155-193`
- Test: same file, tests

**Why:** Spec says ConditionalHandler is a no-op that returns SUCCESS. Routing is handled by the engine's edge selection algorithm via edge conditions, not by the handler evaluating a node condition.

**Step 1: Write failing test**

```rust
#[test]
async fn conditional_handler_returns_success_always() {
    let handler = ConditionalHandler;
    let node = make_node("cond1", NodeType::Conditional);
    let ctx = Context::new();
    let outcome = handler.execute(&node, &ctx).await.unwrap();
    assert!(outcome.is_success());
}
```

**Step 2: Simplify ConditionalHandler**

```rust
pub struct ConditionalHandler;

#[async_trait::async_trait]
impl Handler for ConditionalHandler {
    fn name(&self) -> &str { "conditional" }

    async fn execute(&self, _node: &GraphNode, _context: &Context) -> Result<Outcome, HandlerError> {
        Ok(Outcome::success())
    }

    fn handles(&self, node_type: &NodeType) -> bool {
        matches!(node_type, NodeType::Conditional)
    }
}
```

**Step 3: Remove old condition-evaluation tests, add new no-op test**

**Step 4: Run tests**

Run: `cargo test --workspace`
Expected: ALL PASS

**Step 5: Commit**

```bash
git add -A && git commit -m "fix(attractor): ConditionalHandler is no-op per spec — routing via edge conditions, not handler"
```

---

## Task 5: Fix goal gate attribute name and add retry_target routing

**Files:**
- Modify: `crates/smasher-attractor/src/goals.rs:54-100`
- Modify: `crates/smasher-attractor/src/engine.rs:645-650`
- Test: both files

**Why:** Spec says attribute is `goal_gate=true`, not `goal=true`. Also, unsatisfied goal gates at exit should route to `retry_target` instead of returning an error.

**Step 1: Write failing tests**

```rust
// goals.rs
#[test]
fn goal_gate_reads_goal_gate_attribute() {
    let mut attrs = HashMap::new();
    attrs.insert("goal_gate".into(), NodeAttrValue::Bool(true));
    let node = GraphNode { id: "n1".into(), node_type: NodeType::Codergen, label: None, attrs };
    let graph = Graph { nodes: vec![node], edges: vec![] };
    let gate = GoalGate::from_graph(&graph);
    assert_eq!(gate.goals(), &["n1"]);
}

// engine.rs
#[tokio::test]
async fn exit_with_unsatisfied_goals_routes_to_retry_target() {
    // Build graph: start -> codergen(goal_gate=true) -> exit(retry_target=codergen)
    // When codergen is skipped (not visited), exit should jump back to retry_target
    // ... test setup ...
    let result = engine.run(context).await.unwrap();
    assert!(result.visited_nodes.contains(&"codergen".to_string()));
}
```

**Step 2: Fix attribute name in GoalGate::from_graph**

Change `node.attrs.get("goal")` to `node.attrs.get("goal_gate")` at goals.rs:73. Keep backward compat by also checking `goal` with a fallback.

**Step 3: Add retry_target routing in engine exit handling**

At engine.rs:645, when we encounter an Exit node, check goal gates first. If unmet, look for `retry_target` attribute on the exit node or graph-level `fallback_retry_target`, and jump there instead of exiting.

**Step 4: Run tests, commit**

```bash
git add -A && git commit -m "fix(attractor): goal_gate attribute name and retry_target routing on unsatisfied gates"
```

---

## Task 6: Fix retry policy defaults and attribute names

**Files:**
- Modify: `crates/smasher-attractor/src/retry.rs:16-77`
- Test: same file

**Why:** Spec uses `max_retries` attribute (not `retries`), defaults to 0 retries (not 2).

**Step 1: Write failing tests**

```rust
#[test]
fn default_retry_policy_has_zero_retries() {
    let policy = RetryPolicy::default();
    assert_eq!(policy.max_attempts, 1); // 0 retries = 1 attempt
}

#[test]
fn from_node_reads_max_retries_attribute() {
    let mut attrs = HashMap::new();
    attrs.insert("max_retries".into(), NodeAttrValue::Number(5.0));
    let node = make_node_with_attrs("n1", NodeType::Codergen, attrs);
    let policy = RetryPolicy::from_node(&node);
    assert_eq!(policy.max_attempts, 6); // 5 retries = 6 attempts
}
```

**Step 2: Fix defaults and attribute name**

```rust
impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 1,  // 0 retries per spec
            base_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(30),
            jitter: true,
        }
    }
}
```

In `from_node()`, change `node.attrs.get("retries")` to `node.attrs.get("max_retries")`. Keep backward compat by checking both.

**Step 3: Run tests, commit**

```bash
git add -A && git commit -m "fix(attractor): retry defaults to 0 retries, read max_retries attribute per spec"
```

---

## Task 7: Wire provider_options into all three LLM adapters

**Files:**
- Modify: `crates/smasher-llm/src/provider/anthropic/mod.rs`
- Modify: `crates/smasher-llm/src/provider/anthropic/types.rs`
- Modify: `crates/smasher-llm/src/provider/openai/mod.rs`
- Modify: `crates/smasher-llm/src/provider/openai/types.rs`
- Modify: `crates/smasher-llm/src/provider/gemini/mod.rs`
- Modify: `crates/smasher-llm/src/provider/gemini/types.rs`
- Test: each provider's test module

**Why:** `provider_options` is defined on Request but never read by any adapter. The spec requires it as the escape hatch for provider-specific parameters.

**Step 1: Write failing tests**

```rust
// In anthropic/mod.rs tests:
#[test]
fn provider_options_anthropic_beta_header() {
    let mut opts = HashMap::new();
    opts.insert("anthropic".into(), json!({"beta_headers": ["prompt-caching-2024-07-31"]}));
    let req = Request::new("claude-sonnet-4-20250514", msgs()).provider_options(opts);
    let anthropic_req = convert_request(&req);
    // Verify the beta header value is extractable from the request
}
```

**Step 2: Implement provider_options consumption**

Each `convert_request()` function should check `request.provider_options` for its provider key and merge any recognized fields:

- **Anthropic**: Extract `beta_headers` → join as comma-separated `anthropic-beta` header. Extract other Anthropic-specific params.
- **OpenAI**: Extract and merge into native request fields.
- **Gemini**: Extract and merge.

In the adapter's `complete()` and `stream()` methods, extract the `anthropic-beta` header value and add it to the HTTP request.

**Step 3: Run tests, commit**

```bash
git add -A && git commit -m "feat(llm): wire provider_options escape hatch into all three adapters"
```

---

## Task 8: Add anthropic-beta header support

**Files:**
- Modify: `crates/smasher-llm/src/provider/anthropic/mod.rs:84-91,136-140`
- Test: same file

**Why:** Anthropic features like prompt caching, token counting, and extended thinking require the `anthropic-beta` header.

**Step 1: Write failing test**

```rust
#[test]
fn anthropic_adapter_sends_beta_headers_for_thinking() {
    // When thinking is enabled, anthropic-beta should include the thinking beta
}
```

**Step 2: Add beta header construction**

In `AnthropicAdapter`, add a method that builds the `anthropic-beta` header value based on request features (thinking enabled, prompt caching, etc.) and any explicit `beta_headers` from `provider_options`.

Add the header to both `complete()` and `stream()` HTTP requests.

**Step 3: Run tests, commit**

```bash
git add -A && git commit -m "feat(llm): add anthropic-beta header support for thinking, caching, and provider_options"
```

---

## Task 9: Add explicit provider routing in Client

**Files:**
- Modify: `crates/smasher-llm/src/client/mod.rs:84-96`
- Test: same file

**Why:** `Request.provider` is defined but `Client::adapter_for_model()` only uses model-name inference. The spec requires explicit provider to take precedence.

**Step 1: Write failing test**

```rust
#[test]
fn client_routes_to_explicit_provider() {
    let client = Client::new();
    // Register both anthropic and openai adapters
    let req = Request::new("some-custom-model", msgs()).provider("anthropic");
    // Should route to anthropic even though model name doesn't match a known prefix
}
```

**Step 2: Add provider field check before model inference**

```rust
fn adapter_for(&self, request: &Request) -> Result<Arc<dyn ProviderAdapter>, Error> {
    // Check explicit provider first
    if let Some(ref provider_name) = request.provider {
        let provider = Provider::from_str(provider_name)
            .map_err(|_| Error::ProviderNotConfigured { provider: provider_name.clone() })?;
        return self.providers.get(&provider).cloned()
            .ok_or_else(|| Error::ProviderNotConfigured { provider: provider_name.clone() });
    }
    // Fall back to model-name inference
    self.adapter_for_model(&request.model)
}
```

**Step 3: Run tests, commit**

```bash
git add -A && git commit -m "feat(llm): route by explicit request.provider before model-name inference"
```

---

## Task 10: Wire retry into Client

**Files:**
- Modify: `crates/smasher-llm/src/client/mod.rs:99-108`
- Test: same file

**Why:** The `RetryPolicy` utility exists but `Client::complete()` and `Client::stream()` don't use it. The spec requires automatic retry of retryable errors.

**Step 1: Write failing test**

```rust
#[tokio::test]
async fn client_retries_retryable_errors() {
    // Mock adapter that fails with RateLimited on first call, succeeds on second
    let mock = RetryMockAdapter::new(1); // fail 1 time
    let client = Client::new().with_adapter(Provider::Anthropic, Arc::new(mock));
    let req = Request::new("claude-sonnet-4-20250514", msgs());
    let result = client.complete(req).await;
    assert!(result.is_ok());
}
```

**Step 2: Wrap complete() and stream() with retry logic**

Use the existing `retry::retry()` utility to wrap the adapter call. Use a default retry policy (3 attempts, exponential backoff) that can be overridden via client config.

**Step 3: Run tests, commit**

```bash
git add -A && git commit -m "feat(llm): wire automatic retry for retryable errors in Client::complete and Client::stream"
```

---

## Task 11: Wire LoopDetector into agent session loop

**Files:**
- Modify: `crates/smasher-agent/src/session.rs:140-161,322-352`
- Test: same file

**Why:** LoopDetector is fully implemented but never instantiated or called in the session.

**Step 1: Write failing test**

```rust
#[tokio::test]
async fn session_emits_loop_detected_on_repetitive_tool_calls() {
    // Mock adapter that always returns the same tool call
    // After enough repetitions, LoopDetected event should fire
}
```

**Step 2: Integrate LoopDetector**

In `Session::new()`, create a `LoopDetector::default()`. In the tool execution loop (session.rs:322-352), after each tool call, record it in the detector and check for loops. If detected, emit `LoopDetected` event and inject a steering message warning the model.

**Step 3: Run tests, commit**

```bash
git add -A && git commit -m "feat(agent): wire LoopDetector into session tool execution loop"
```

---

## Task 12: Drain steering between tool rounds

**Files:**
- Modify: `crates/smasher-agent/src/session.rs:322-360`
- Test: same file

**Why:** Steering is only drained once at the start of `process_input()`. Spec requires draining after each tool execution round.

**Step 1: Write failing test**

```rust
#[tokio::test]
async fn steering_injected_during_tools_is_applied_before_next_llm_call() {
    // Start session, queue steering DURING tool execution
    // Verify the steering message appears in the conversation before the next LLM call
}
```

**Step 2: Add steering drain after tool execution**

After the `for tc in &tool_calls` loop (around line 352), drain steering again:

```rust
// Drain any steering that arrived during tool execution
let mid_steering = self.state.drain_steering();
for msg in &mid_steering {
    self.state.messages.push(Message::user(msg));
    self.state.turns.push(Turn::Steering { text: msg.clone() });
    self.event_emitter.emit(SessionEvent::SteeringApplied { text: msg.clone() });
}
```

**Step 3: Run tests, commit**

```bash
git add -A && git commit -m "feat(agent): drain steering between tool execution rounds per spec"
```

---

## Task 13: Differentiate provider profiles (tool lists and system prompts)

**Files:**
- Modify: `crates/smasher-agent/src/profile/mod.rs:96-105,171-179,246-254`
- Test: same file

**Why:** All three profiles return identical tool lists. OpenAI should use `apply_patch` instead of `edit_file`/`write_file`. Tool names and system prompts should be provider-aligned.

**Step 1: Write failing tests**

```rust
#[test]
fn openai_profile_includes_apply_patch() {
    let profile = OpenAiProfile;
    let tools = profile.tool_names();
    assert!(tools.contains(&"apply_patch"));
    assert!(!tools.contains(&"edit_file"));
    assert!(!tools.contains(&"write_file"));
}

#[test]
fn anthropic_profile_has_edit_file_not_apply_patch() {
    let profile = AnthropicProfile;
    let tools = profile.tool_names();
    assert!(tools.contains(&"edit_file"));
    assert!(tools.contains(&"write_file"));
    assert!(!tools.contains(&"apply_patch"));
}
```

**Step 2: Differentiate tool lists**

```rust
// AnthropicProfile
fn tool_names(&self) -> Vec<&str> {
    vec!["read_file", "write_file", "edit_file", "shell", "grep", "glob_files"]
}

// OpenAiProfile
fn tool_names(&self) -> Vec<&str> {
    vec!["read_file", "apply_patch", "shell", "grep", "glob_files"]
}

// GeminiProfile
fn tool_names(&self) -> Vec<&str> {
    vec!["read_file", "write_file", "edit_file", "shell", "grep", "glob_files"]
}
```

**Step 3: Run tests, commit**

```bash
git add -A && git commit -m "feat(agent): differentiate provider profiles — OpenAI gets apply_patch, not edit_file"
```

---

## Task 14: Add per-tool output truncation limits

**Files:**
- Modify: `crates/smasher-agent/src/tools/mod.rs:115-130`
- Modify: `crates/smasher-agent/src/tools/truncation.rs`
- Test: both files

**Why:** Spec defines different char limits per tool: read_file=50k, shell=30k, grep=20k, glob=20k, edit_file=10k. We use a single 100k limit.

**Step 1: Write failing test**

```rust
#[test]
fn tool_registry_applies_per_tool_limit() {
    let registry = ToolRegistry::new();
    assert_eq!(registry.output_limit_for("read_file"), 50_000);
    assert_eq!(registry.output_limit_for("shell"), 30_000);
    assert_eq!(registry.output_limit_for("grep"), 20_000);
}
```

**Step 2: Add per-tool limits**

Add a `tool_output_limits: HashMap<String, usize>` to `ToolRegistry` with spec defaults. In `execute()`, use the per-tool limit instead of the global max.

**Step 3: Run tests, commit**

```bash
git add -A && git commit -m "feat(agent): per-tool output truncation limits per spec (read=50k, shell=30k, grep=20k)"
```

---

## Task 15: Add offset/limit to read_file tool and line-numbered output

**Files:**
- Modify: `crates/smasher-agent/src/tools/shared.rs:31-82`
- Modify: `crates/smasher-agent/src/environment/mod.rs` (read_file signature)
- Test: both files

**Why:** Spec requires `offset` and `limit` params for partial file reads, and output in `NNN | content` line-numbered format.

**Step 1: Write failing tests**

```rust
#[tokio::test]
async fn read_file_adds_line_numbers() {
    let env = setup_test_env_with_file("test.txt", "line1\nline2\nline3");
    let tool = ReadFileTool::new(env);
    let output = tool.execute(r#"{"path": "test.txt"}"#).await;
    assert!(output.content.contains("  1 | line1"));
    assert!(output.content.contains("  2 | line2"));
}

#[tokio::test]
async fn read_file_with_offset_and_limit() {
    let env = setup_test_env_with_file("test.txt", "a\nb\nc\nd\ne");
    let tool = ReadFileTool::new(env);
    let output = tool.execute(r#"{"path": "test.txt", "offset": 2, "limit": 2}"#).await;
    assert!(output.content.contains("  2 | b"));
    assert!(output.content.contains("  3 | c"));
    assert!(!output.content.contains("  1 | a"));
}
```

**Step 2: Add params and line numbering**

Update `ReadFileTool::parameters_schema()` to include `offset` (optional integer) and `limit` (optional integer, default 2000). In `execute()`, apply offset/limit and format as `NNN | content`.

**Step 3: Run tests, commit**

```bash
git add -A && git commit -m "feat(agent): read_file tool with offset/limit params and line-numbered output"
```

---

## Task 16: Execute tool calls in parallel

**Files:**
- Modify: `crates/smasher-agent/src/session.rs:322-352`
- Test: same file

**Why:** Tool calls are executed sequentially in a for loop. Spec requires parallel execution when the provider supports it.

**Step 1: Write failing test**

```rust
#[tokio::test]
async fn multiple_tool_calls_execute_concurrently() {
    // Two tool calls that each sleep 100ms
    // Total time should be ~100ms, not ~200ms
}
```

**Step 2: Replace sequential loop with join_all**

```rust
use futures::future::join_all;

let tool_futures: Vec<_> = tool_calls.iter().map(|tc| {
    let name = tc.name.clone();
    let args = tc.arguments.clone();
    let registry = &self.tool_registry;
    async move {
        let output = registry.execute(&name, &args).await;
        (tc.clone(), output)
    }
}).collect();

let results = join_all(tool_futures).await;

for (tc, output) in results {
    // emit events, add tool results to conversation
}
```

**Step 3: Run tests, commit**

```bash
git add -A && git commit -m "feat(agent): execute parallel tool calls concurrently with join_all"
```

---

## Task 17: Emit full untruncated output in TOOL_CALL_END events

**Files:**
- Modify: `crates/smasher-agent/src/tools/mod.rs:115-130`
- Modify: `crates/smasher-agent/src/session.rs:329-337`
- Test: both files

**Why:** Spec says TOOL_CALL_END carries full untruncated output. We truncate before emitting.

**Step 1: Write failing test**

```rust
#[tokio::test]
async fn tool_call_completed_event_has_full_output() {
    // Register tool that returns 200k chars
    // Event should contain full 200k, but conversation should have truncated version
}
```

**Step 2: Split truncation from event emission**

In `ToolRegistry::execute()`, return both full and truncated output:

```rust
pub struct ToolExecutionResult {
    pub truncated: ToolOutput,  // Goes into conversation
    pub full_output: String,    // Goes into events
}
```

In `session.rs`, emit the event with `full_output` but add `truncated` to the conversation.

**Step 3: Run tests, commit**

```bash
git add -A && git commit -m "feat(agent): emit full untruncated output in ToolCallCompleted events per spec"
```

---

## Summary

| Task | Crate | Gap(s) Fixed | Priority |
|------|-------|-------------|----------|
| 1 | attractor | C3 — Outcome enum expansion | CRITICAL |
| 2 | attractor | C1 — Shape mappings | CRITICAL |
| 3 | attractor | C2 — Edge selection algorithm | CRITICAL |
| 4 | attractor | H8 — ConditionalHandler no-op | HIGH |
| 5 | attractor | H5, H6 — Goal gate attr + retry_target | HIGH |
| 6 | attractor | H10 — Retry defaults | HIGH |
| 7 | llm | C4 — provider_options wiring | CRITICAL |
| 8 | llm | C5 — anthropic-beta headers | CRITICAL |
| 9 | llm | H17 — Explicit provider routing | HIGH |
| 10 | llm | H11 — Client retry | HIGH |
| 11 | agent | H2 — LoopDetector integration | HIGH |
| 12 | agent | H3 — Steering between rounds | HIGH |
| 13 | agent | H1 — Provider profile differentiation | HIGH |
| 14 | agent | H14 — Per-tool truncation limits | HIGH |
| 15 | agent | H12 — read_file offset/limit/line numbers | HIGH |
| 16 | agent | H13 — Parallel tool execution | HIGH |
| 17 | agent | H15 — Full output in events | HIGH |

Tasks 1-6 (attractor) should be done first as they affect the core execution model. Tasks 7-10 (LLM) are independent. Tasks 11-17 (agent) are independent of each other but depend on Task 1 (Outcome changes).
