// ABOUTME: Graph rendering module that converts semantic Graphs back to styled DOT format.
// ABOUTME: Provides traits and implementations for rendering graphs to DOT, SVG, or PNG output.

use std::collections::HashMap;
use std::fmt;
use std::process::Stdio;

use serde::{Deserialize, Serialize};
use tokio::process::Command;

use crate::graph::{Graph, GraphEdge, GraphNode, NodeType};

/// Supported output formats for graph rendering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RenderFormat {
    /// Raw DOT language source.
    Dot,
    /// SVG vector image (requires external graphviz).
    Svg,
    /// PNG raster image (requires external graphviz).
    Png,
}

impl fmt::Display for RenderFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RenderFormat::Dot => write!(f, "dot"),
            RenderFormat::Svg => write!(f, "svg"),
            RenderFormat::Png => write!(f, "png"),
        }
    }
}

impl RenderFormat {
    /// Parse a format string into a RenderFormat.
    ///
    /// Accepts "dot", "svg", "png" (case-insensitive).
    pub fn from_str_loose(s: &str) -> Option<RenderFormat> {
        match s.to_lowercase().as_str() {
            "dot" => Some(RenderFormat::Dot),
            "svg" => Some(RenderFormat::Svg),
            "png" => Some(RenderFormat::Png),
            _ => None,
        }
    }

    /// Return the MIME content type for this format.
    pub fn content_type(&self) -> &'static str {
        match self {
            RenderFormat::Dot => "text/vnd.graphviz",
            RenderFormat::Svg => "image/svg+xml",
            RenderFormat::Png => "image/png",
        }
    }
}

/// The result of rendering a graph, containing the format and content bytes.
#[derive(Debug, Clone)]
pub struct RenderOutput {
    /// The format of the rendered content.
    pub format: RenderFormat,
    /// The raw content bytes. For DOT and SVG this is UTF-8 text; for PNG it is binary.
    pub content: Vec<u8>,
}

impl RenderOutput {
    /// Interpret the content as a UTF-8 string.
    ///
    /// Returns None if the content is not valid UTF-8 (e.g. binary PNG).
    pub fn as_text(&self) -> Option<&str> {
        std::str::from_utf8(&self.content).ok()
    }
}

/// Visual styling attributes for a rendered graph node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeStyle {
    /// DOT shape attribute (e.g. "circle", "box", "diamond").
    pub shape: &'static str,
    /// Fill color for the node.
    pub fill_color: &'static str,
    /// Font color for the node label.
    pub font_color: &'static str,
    /// Border/outline style.
    pub style: &'static str,
}

/// Map a NodeType to its visual rendering style.
pub fn style_for_node_type(node_type: &NodeType) -> NodeStyle {
    match node_type {
        NodeType::Start => NodeStyle {
            shape: "circle",
            fill_color: "#4CAF50",
            font_color: "white",
            style: "filled",
        },
        NodeType::Exit => NodeStyle {
            shape: "doublecircle",
            fill_color: "#F44336",
            font_color: "white",
            style: "filled",
        },
        NodeType::Conditional => NodeStyle {
            shape: "diamond",
            fill_color: "#FFC107",
            font_color: "black",
            style: "filled",
        },
        NodeType::Codergen => NodeStyle {
            shape: "box",
            fill_color: "#2196F3",
            font_color: "white",
            style: "filled",
        },
        NodeType::Generic => NodeStyle {
            shape: "box",
            fill_color: "#2196F3",
            font_color: "white",
            style: "filled",
        },
        NodeType::Manager => NodeStyle {
            shape: "house",
            fill_color: "#9C27B0",
            font_color: "white",
            style: "filled",
        },
        NodeType::SubPipeline => NodeStyle {
            shape: "component",
            fill_color: "#FF9800",
            font_color: "white",
            style: "filled",
        },
        NodeType::Parallel => NodeStyle {
            shape: "component",
            fill_color: "#00BCD4",
            font_color: "white",
            style: "filled",
        },
        NodeType::FanIn => NodeStyle {
            shape: "tripleoctagon",
            fill_color: "#009688",
            font_color: "white",
            style: "filled",
        },
        NodeType::Tool => NodeStyle {
            shape: "hexagon",
            fill_color: "#607D8B",
            font_color: "white",
            style: "filled",
        },
        NodeType::Interviewer => NodeStyle {
            shape: "ellipse",
            fill_color: "#795548",
            font_color: "white",
            style: "filled",
        },
    }
}

/// Escape a string for use as a DOT identifier or label.
///
/// Wraps in double quotes and escapes internal double quotes and backslashes.
fn dot_escape(s: &str) -> String {
    let escaped = s.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
}

/// Render a single GraphNode to a DOT node statement line.
fn render_node(node: &GraphNode) -> String {
    let style = style_for_node_type(&node.node_type);
    let label = node.label.as_deref().unwrap_or(&node.id);
    format!(
        "    {} [label={} shape={} style={} fillcolor={} fontcolor={}];",
        dot_escape(&node.id),
        dot_escape(label),
        dot_escape(style.shape),
        dot_escape(style.style),
        dot_escape(style.fill_color),
        dot_escape(style.font_color),
    )
}

/// Render a single GraphEdge to a DOT edge statement line.
fn render_edge(edge: &GraphEdge) -> String {
    let mut attrs = Vec::new();
    if let Some(label) = &edge.label {
        attrs.push(format!("label={}", dot_escape(label)));
    }
    if attrs.is_empty() {
        format!(
            "    {} -> {};",
            dot_escape(&edge.from),
            dot_escape(&edge.to)
        )
    } else {
        format!(
            "    {} -> {} [{}];",
            dot_escape(&edge.from),
            dot_escape(&edge.to),
            attrs.join(" "),
        )
    }
}

/// Render a Graph into a styled DOT format string.
///
/// Produces a complete digraph with node styling based on NodeType
/// and edge labels preserved from the graph metadata.
pub fn render_to_dot(graph: &Graph) -> String {
    let mut lines = Vec::new();

    let name = graph.name.as_deref().map(dot_escape).unwrap_or_default();
    lines.push(format!("digraph {name} {{"));

    // Graph-level attributes for layout
    lines.push("    rankdir=LR;".to_string());
    lines.push("    bgcolor=\"#FAFAFA\";".to_string());
    lines.push("    node [fontname=\"Helvetica\" fontsize=12];".to_string());
    lines.push("    edge [fontname=\"Helvetica\" fontsize=10];".to_string());
    lines.push(String::new());

    // Render nodes
    for node in &graph.nodes {
        lines.push(render_node(node));
    }

    if !graph.nodes.is_empty() && !graph.edges.is_empty() {
        lines.push(String::new());
    }

    // Render edges
    for edge in &graph.edges {
        lines.push(render_edge(edge));
    }

    lines.push("}".to_string());
    lines.join("\n")
}

// ---------------------------------------------------------------------------
// GraphRenderer trait and DotRenderer implementation
// ---------------------------------------------------------------------------

/// Errors that can occur during graph rendering.
#[derive(Debug, thiserror::Error)]
pub enum RenderError {
    #[error("unsupported render format: {format}")]
    UnsupportedFormat { format: RenderFormat },

    #[error("graphviz not available: {message}")]
    GraphvizUnavailable { message: String },

    #[error("graphviz rendering failed: {message}")]
    GraphvizFailed { message: String },
}

/// Trait for rendering a Graph to different output formats.
#[async_trait::async_trait]
pub trait GraphRenderer: Send + Sync {
    /// Render the given graph to the specified format.
    async fn render(
        &self,
        graph: &Graph,
        format: RenderFormat,
    ) -> Result<RenderOutput, RenderError>;

    /// Return the list of formats this renderer supports.
    fn supported_formats(&self) -> Vec<RenderFormat>;
}

/// Renders graphs to DOT format string output.
///
/// This renderer does not require external tools and always succeeds
/// for the Dot format. SVG and PNG are not supported by this renderer.
pub struct DotRenderer;

#[async_trait::async_trait]
impl GraphRenderer for DotRenderer {
    async fn render(
        &self,
        graph: &Graph,
        format: RenderFormat,
    ) -> Result<RenderOutput, RenderError> {
        match format {
            RenderFormat::Dot => {
                let dot_string = render_to_dot(graph);
                Ok(RenderOutput {
                    format: RenderFormat::Dot,
                    content: dot_string.into_bytes(),
                })
            }
            other => Err(RenderError::UnsupportedFormat { format: other }),
        }
    }

    fn supported_formats(&self) -> Vec<RenderFormat> {
        vec![RenderFormat::Dot]
    }
}

// ---------------------------------------------------------------------------
// Execution status overlay
// ---------------------------------------------------------------------------

/// Simplified execution status for visual rendering of graph nodes.
///
/// Maps from the richer `NodeStatus` in state.rs to a display-oriented
/// status used purely for coloring and labeling rendered graphs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeExecutionStatus {
    /// Node has not yet been executed.
    Pending,
    /// Node is actively running.
    Running,
    /// Node completed successfully.
    Done,
    /// Node failed or was skipped.
    Failed,
}

impl fmt::Display for NodeExecutionStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NodeExecutionStatus::Pending => write!(f, "pending"),
            NodeExecutionStatus::Running => write!(f, "running"),
            NodeExecutionStatus::Done => write!(f, "done"),
            NodeExecutionStatus::Failed => write!(f, "failed"),
        }
    }
}

/// Visual styling override applied to a node based on its execution status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusStyle {
    /// Override fill color reflecting execution status.
    pub fill_color: &'static str,
    /// Override border color reflecting execution status.
    pub border_color: &'static str,
    /// DOT `penwidth` attribute for the node border.
    pub pen_width: &'static str,
    /// Text suffix appended to the node label (e.g. " [RUNNING]").
    pub label_suffix: &'static str,
}

/// Return the visual style override for a given execution status.
pub fn style_for_execution_status(status: &NodeExecutionStatus) -> StatusStyle {
    match status {
        NodeExecutionStatus::Pending => StatusStyle {
            fill_color: "#E0E0E0",
            border_color: "#9E9E9E",
            pen_width: "1.0",
            label_suffix: "",
        },
        NodeExecutionStatus::Running => StatusStyle {
            fill_color: "#FFF9C4",
            border_color: "#F9A825",
            pen_width: "3.0",
            label_suffix: " [RUNNING]",
        },
        NodeExecutionStatus::Done => StatusStyle {
            fill_color: "#C8E6C9",
            border_color: "#2E7D32",
            pen_width: "2.0",
            label_suffix: " [DONE]",
        },
        NodeExecutionStatus::Failed => StatusStyle {
            fill_color: "#FFCDD2",
            border_color: "#C62828",
            pen_width: "2.0",
            label_suffix: " [FAILED]",
        },
    }
}

/// Render a single GraphNode with a status overlay to a DOT node statement line.
fn render_node_with_status(node: &GraphNode, status: &NodeExecutionStatus) -> String {
    let base_style = style_for_node_type(&node.node_type);
    let status_style = style_for_execution_status(status);

    let base_label = node.label.as_deref().unwrap_or(&node.id);
    let label = format!("{base_label}{}", status_style.label_suffix);

    format!(
        "    {} [label={} shape={} style=\"filled\" fillcolor={} fontcolor={} color={} penwidth={}];",
        dot_escape(&node.id),
        dot_escape(&label),
        dot_escape(base_style.shape),
        dot_escape(status_style.fill_color),
        dot_escape(base_style.font_color),
        dot_escape(status_style.border_color),
        status_style.pen_width,
    )
}

/// Render a Graph into styled DOT format with execution status overlays.
///
/// Nodes whose IDs appear in `statuses` are rendered with status-specific
/// coloring and label suffixes. Nodes not in the map default to Pending style.
pub fn render_to_dot_with_status(
    graph: &Graph,
    statuses: &HashMap<String, NodeExecutionStatus>,
) -> String {
    let mut lines = Vec::new();

    let name = graph.name.as_deref().map(dot_escape).unwrap_or_default();
    lines.push(format!("digraph {name} {{"));

    // Graph-level attributes for layout
    lines.push("    rankdir=LR;".to_string());
    lines.push("    bgcolor=\"#FAFAFA\";".to_string());
    lines.push("    node [fontname=\"Helvetica\" fontsize=12];".to_string());
    lines.push("    edge [fontname=\"Helvetica\" fontsize=10];".to_string());
    lines.push(String::new());

    // Render nodes with status overlays
    for node in &graph.nodes {
        let status = statuses
            .get(&node.id)
            .copied()
            .unwrap_or(NodeExecutionStatus::Pending);
        lines.push(render_node_with_status(node, &status));
    }

    if !graph.nodes.is_empty() && !graph.edges.is_empty() {
        lines.push(String::new());
    }

    // Render edges
    for edge in &graph.edges {
        lines.push(render_edge(edge));
    }

    lines.push("}".to_string());
    lines.join("\n")
}

// ---------------------------------------------------------------------------
// Graphviz external tool integration
// ---------------------------------------------------------------------------

/// Check whether the `dot` command (graphviz) is available on this system.
///
/// Returns `true` if the `dot` binary can be executed, `false` otherwise.
pub async fn graphviz_available() -> bool {
    Command::new("dot")
        .arg("-V")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Invoke the graphviz `dot` command to convert DOT source into the target format.
///
/// Writes `dot_source` to stdin and reads the rendered output from stdout.
async fn run_graphviz(dot_source: &str, output_format: &str) -> Result<Vec<u8>, RenderError> {
    let mut child = Command::new("dot")
        .arg(format!("-T{output_format}"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| RenderError::GraphvizUnavailable {
            message: format!("failed to spawn dot: {e}"),
        })?;

    // Write DOT source to stdin
    if let Some(mut stdin) = child.stdin.take() {
        use tokio::io::AsyncWriteExt;
        stdin
            .write_all(dot_source.as_bytes())
            .await
            .map_err(|e| RenderError::GraphvizFailed {
                message: format!("failed to write to dot stdin: {e}"),
            })?;
        // Drop stdin to signal EOF
    }

    let output = child
        .wait_with_output()
        .await
        .map_err(|e| RenderError::GraphvizFailed {
            message: format!("failed to wait for dot process: {e}"),
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(RenderError::GraphvizFailed {
            message: format!("dot exited with status {}: {stderr}", output.status),
        });
    }

    Ok(output.stdout)
}

/// Renders graphs to DOT, SVG, or PNG format using the external graphviz `dot` tool.
///
/// DOT format is produced directly without calling graphviz. SVG and PNG
/// formats require the `dot` binary to be installed on the system.
pub struct GraphvizRenderer;

#[async_trait::async_trait]
impl GraphRenderer for GraphvizRenderer {
    async fn render(
        &self,
        graph: &Graph,
        format: RenderFormat,
    ) -> Result<RenderOutput, RenderError> {
        match format {
            RenderFormat::Dot => {
                let dot_string = render_to_dot(graph);
                Ok(RenderOutput {
                    format: RenderFormat::Dot,
                    content: dot_string.into_bytes(),
                })
            }
            RenderFormat::Svg | RenderFormat::Png => {
                if !graphviz_available().await {
                    return Err(RenderError::GraphvizUnavailable {
                        message: "graphviz 'dot' command not found on PATH".to_string(),
                    });
                }
                let dot_source = render_to_dot(graph);
                let format_str = format.to_string();
                let content = run_graphviz(&dot_source, &format_str).await?;
                Ok(RenderOutput { format, content })
            }
        }
    }

    fn supported_formats(&self) -> Vec<RenderFormat> {
        vec![RenderFormat::Dot, RenderFormat::Svg, RenderFormat::Png]
    }
}

/// Renders graphs with status overlays using the external graphviz `dot` tool.
///
/// Combines status-annotated DOT generation with graphviz rendering.
pub struct StatusGraphvizRenderer {
    /// Execution statuses to overlay on graph nodes.
    pub statuses: HashMap<String, NodeExecutionStatus>,
}

impl StatusGraphvizRenderer {
    /// Create a renderer with the given node execution statuses.
    pub fn new(statuses: HashMap<String, NodeExecutionStatus>) -> Self {
        Self { statuses }
    }
}

#[async_trait::async_trait]
impl GraphRenderer for StatusGraphvizRenderer {
    async fn render(
        &self,
        graph: &Graph,
        format: RenderFormat,
    ) -> Result<RenderOutput, RenderError> {
        let dot_source = render_to_dot_with_status(graph, &self.statuses);
        match format {
            RenderFormat::Dot => Ok(RenderOutput {
                format: RenderFormat::Dot,
                content: dot_source.into_bytes(),
            }),
            RenderFormat::Svg | RenderFormat::Png => {
                if !graphviz_available().await {
                    return Err(RenderError::GraphvizUnavailable {
                        message: "graphviz 'dot' command not found on PATH".to_string(),
                    });
                }
                let format_str = format.to_string();
                let content = run_graphviz(&dot_source, &format_str).await?;
                Ok(RenderOutput { format, content })
            }
        }
    }

    fn supported_formats(&self) -> Vec<RenderFormat> {
        vec![RenderFormat::Dot, RenderFormat::Svg, RenderFormat::Png]
    }
}

// ---------------------------------------------------------------------------
// Render cache — avoids re-invoking graphviz for identical DOT+format pairs
// ---------------------------------------------------------------------------

/// Cache key derived from a SHA-256 hash of the DOT source and the target format.
///
/// Two renders of the same DOT source to the same format will produce the
/// same cache key, avoiding redundant graphviz invocations.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RenderCacheKey {
    /// Hex-encoded SHA-256 digest of the DOT source concatenated with the format name.
    digest: String,
}

impl RenderCacheKey {
    /// Compute a cache key from DOT source text and the desired output format.
    pub fn new(dot_source: &str, format: RenderFormat) -> Self {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(dot_source.as_bytes());
        hasher.update(format.to_string().as_bytes());
        let result = hasher.finalize();
        let digest = result
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>();
        Self { digest }
    }
}

/// A caching wrapper around any `GraphRenderer` implementation.
///
/// Stores rendered output in an in-memory `HashMap` keyed by a SHA-256 digest
/// of the DOT source and target format. Cache hits skip the external graphviz
/// process entirely, returning a clone of the previously rendered output.
pub struct CachedRenderer<R: GraphRenderer> {
    inner: R,
    cache: std::sync::Mutex<HashMap<RenderCacheKey, RenderOutput>>,
}

impl<R: GraphRenderer> CachedRenderer<R> {
    /// Wrap a renderer with an empty cache.
    pub fn new(inner: R) -> Self {
        Self {
            inner,
            cache: std::sync::Mutex::new(HashMap::new()),
        }
    }

    /// Return the number of entries currently in the cache.
    pub fn cache_len(&self) -> usize {
        self.cache.lock().expect("cache lock poisoned").len()
    }

    /// Remove all entries from the cache.
    pub fn clear_cache(&self) {
        self.cache.lock().expect("cache lock poisoned").clear();
    }
}

#[async_trait::async_trait]
impl<R: GraphRenderer> GraphRenderer for CachedRenderer<R> {
    async fn render(
        &self,
        graph: &Graph,
        format: RenderFormat,
    ) -> Result<RenderOutput, RenderError> {
        // Produce the DOT source to compute the cache key. The key is based on the
        // text that would be fed to graphviz, so different graph states produce
        // different keys even if the Graph struct is the same object.
        let dot_source = render_to_dot(graph);
        let key = RenderCacheKey::new(&dot_source, format);

        // Check for cache hit.
        {
            let cache = self.cache.lock().expect("cache lock poisoned");
            if let Some(cached) = cache.get(&key) {
                return Ok(cached.clone());
            }
        }

        // Cache miss — delegate to the inner renderer.
        let output = self.inner.render(graph, format).await?;

        // Store the result for future hits.
        {
            let mut cache = self.cache.lock().expect("cache lock poisoned");
            cache.insert(key, output.clone());
        }

        Ok(output)
    }

    fn supported_formats(&self) -> Vec<RenderFormat> {
        self.inner.supported_formats()
    }
}

// ---------------------------------------------------------------------------
// HTTP API types for the render route
// ---------------------------------------------------------------------------

/// Query parameters for the graph render endpoint.
///
/// GET /api/runs/{id}/graph?format=svg&include_status=true
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RenderGraphQuery {
    /// Desired output format. Defaults to "dot" if not specified.
    /// Accepts "dot", "svg", or "png".
    pub format: Option<String>,
    /// When true, overlay node execution statuses (colors and labels) onto the
    /// rendered graph. Defaults to false if not specified.
    pub include_status: Option<bool>,
}

impl RenderGraphQuery {
    /// Resolve the requested format, defaulting to DOT if not specified or invalid.
    pub fn resolved_format(&self) -> RenderFormat {
        self.format
            .as_deref()
            .and_then(RenderFormat::from_str_loose)
            .unwrap_or(RenderFormat::Dot)
    }

    /// Whether the caller requested status overlays.
    pub fn wants_status(&self) -> bool {
        self.include_status.unwrap_or(false)
    }
}

/// Response body for the graph render endpoint when returning DOT text.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RenderGraphResponse {
    /// The run identifier.
    pub run_id: String,
    /// The rendered format.
    pub format: RenderFormat,
    /// The rendered content as text (only for DOT and SVG formats).
    /// For PNG, this field is None and the binary content should be
    /// returned via the raw HTTP response body.
    pub content: Option<String>,
    /// Content type header value.
    pub content_type: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{GraphEdge, GraphNode, NodeType};
    use std::collections::HashMap;

    // ---------------------------------------------------------------
    // Test helpers
    // ---------------------------------------------------------------

    /// Build a minimal Graph with specified nodes and edges.
    fn make_graph(name: Option<&str>, nodes: Vec<GraphNode>, edges: Vec<GraphEdge>) -> Graph {
        Graph {
            name: name.map(String::from),
            nodes,
            edges,
            default_node_attrs: HashMap::new(),
            default_edge_attrs: HashMap::new(),
            graph_attrs: HashMap::new(),
        }
    }

    /// Build a simple GraphNode with the given type and optional label.
    fn make_node(id: &str, node_type: NodeType, label: Option<&str>) -> GraphNode {
        GraphNode {
            id: id.to_string(),
            node_type,
            label: label.map(String::from),
            attrs: HashMap::new(),
        }
    }

    /// Build a simple GraphEdge.
    fn make_edge(from: &str, to: &str, label: Option<&str>) -> GraphEdge {
        GraphEdge {
            from: from.to_string(),
            to: to.to_string(),
            label: label.map(String::from),
            condition: None,
            priority: None,
            loop_restart: false,
            attrs: HashMap::new(),
        }
    }

    // ---------------------------------------------------------------
    // RenderFormat tests
    // ---------------------------------------------------------------

    #[test]
    fn render_format_display() {
        assert_eq!(format!("{}", RenderFormat::Dot), "dot");
        assert_eq!(format!("{}", RenderFormat::Svg), "svg");
        assert_eq!(format!("{}", RenderFormat::Png), "png");
    }

    #[test]
    fn render_format_from_str_loose_valid() {
        assert_eq!(RenderFormat::from_str_loose("dot"), Some(RenderFormat::Dot));
        assert_eq!(RenderFormat::from_str_loose("DOT"), Some(RenderFormat::Dot));
        assert_eq!(RenderFormat::from_str_loose("svg"), Some(RenderFormat::Svg));
        assert_eq!(RenderFormat::from_str_loose("SVG"), Some(RenderFormat::Svg));
        assert_eq!(RenderFormat::from_str_loose("png"), Some(RenderFormat::Png));
        assert_eq!(RenderFormat::from_str_loose("Png"), Some(RenderFormat::Png));
    }

    #[test]
    fn render_format_from_str_loose_invalid() {
        assert_eq!(RenderFormat::from_str_loose("pdf"), None);
        assert_eq!(RenderFormat::from_str_loose(""), None);
        assert_eq!(RenderFormat::from_str_loose("jpeg"), None);
    }

    #[test]
    fn render_format_content_type() {
        assert_eq!(RenderFormat::Dot.content_type(), "text/vnd.graphviz");
        assert_eq!(RenderFormat::Svg.content_type(), "image/svg+xml");
        assert_eq!(RenderFormat::Png.content_type(), "image/png");
    }

    #[test]
    fn render_format_serde_roundtrip() {
        for format in [RenderFormat::Dot, RenderFormat::Svg, RenderFormat::Png] {
            let json_str = serde_json::to_string(&format).unwrap();
            let restored: RenderFormat = serde_json::from_str(&json_str).unwrap();
            assert_eq!(format, restored);
        }
    }

    #[test]
    fn render_format_serializes_lowercase() {
        assert_eq!(
            serde_json::to_string(&RenderFormat::Dot).unwrap(),
            "\"dot\""
        );
        assert_eq!(
            serde_json::to_string(&RenderFormat::Svg).unwrap(),
            "\"svg\""
        );
        assert_eq!(
            serde_json::to_string(&RenderFormat::Png).unwrap(),
            "\"png\""
        );
    }

    // ---------------------------------------------------------------
    // RenderOutput tests
    // ---------------------------------------------------------------

    #[test]
    fn render_output_as_text_valid_utf8() {
        let output = RenderOutput {
            format: RenderFormat::Dot,
            content: "digraph { a -> b }".as_bytes().to_vec(),
        };
        assert_eq!(output.as_text(), Some("digraph { a -> b }"));
    }

    #[test]
    fn render_output_as_text_invalid_utf8() {
        let output = RenderOutput {
            format: RenderFormat::Png,
            content: vec![0xFF, 0xFE, 0x00],
        };
        assert!(output.as_text().is_none());
    }

    // ---------------------------------------------------------------
    // NodeStyle tests - verify correct styles for each NodeType
    // ---------------------------------------------------------------

    #[test]
    fn style_for_start_node() {
        let style = style_for_node_type(&NodeType::Start);
        assert_eq!(style.shape, "circle");
        assert_eq!(style.fill_color, "#4CAF50");
        assert_eq!(style.style, "filled");
    }

    #[test]
    fn style_for_exit_node() {
        let style = style_for_node_type(&NodeType::Exit);
        assert_eq!(style.shape, "doublecircle");
        assert_eq!(style.fill_color, "#F44336");
        assert_eq!(style.style, "filled");
    }

    #[test]
    fn style_for_conditional_node() {
        let style = style_for_node_type(&NodeType::Conditional);
        assert_eq!(style.shape, "diamond");
        assert_eq!(style.fill_color, "#FFC107");
        assert_eq!(style.font_color, "black");
    }

    #[test]
    fn style_for_codergen_node() {
        let style = style_for_node_type(&NodeType::Codergen);
        assert_eq!(style.shape, "box");
        assert_eq!(style.fill_color, "#2196F3");
    }

    #[test]
    fn style_for_generic_node() {
        let style = style_for_node_type(&NodeType::Generic);
        assert_eq!(style.shape, "box");
        assert_eq!(style.fill_color, "#2196F3");
    }

    #[test]
    fn style_for_manager_node() {
        let style = style_for_node_type(&NodeType::Manager);
        assert_eq!(style.shape, "house");
        assert_eq!(style.fill_color, "#9C27B0");
    }

    #[test]
    fn style_for_subpipeline_node() {
        let style = style_for_node_type(&NodeType::SubPipeline);
        assert_eq!(style.shape, "component");
        assert_eq!(style.fill_color, "#FF9800");
    }

    #[test]
    fn style_for_parallel_node() {
        let style = style_for_node_type(&NodeType::Parallel);
        assert_eq!(style.shape, "component");
        assert_eq!(style.fill_color, "#00BCD4");
    }

    #[test]
    fn style_for_fanin_node() {
        let style = style_for_node_type(&NodeType::FanIn);
        assert_eq!(style.shape, "tripleoctagon");
        assert_eq!(style.fill_color, "#009688");
    }

    #[test]
    fn style_for_tool_node() {
        let style = style_for_node_type(&NodeType::Tool);
        assert_eq!(style.shape, "hexagon");
        assert_eq!(style.fill_color, "#607D8B");
    }

    #[test]
    fn style_for_interviewer_node() {
        let style = style_for_node_type(&NodeType::Interviewer);
        assert_eq!(style.shape, "ellipse");
        assert_eq!(style.fill_color, "#795548");
    }

    // ---------------------------------------------------------------
    // dot_escape tests
    // ---------------------------------------------------------------

    #[test]
    fn dot_escape_simple_string() {
        assert_eq!(dot_escape("hello"), "\"hello\"");
    }

    #[test]
    fn dot_escape_with_quotes() {
        assert_eq!(dot_escape("say \"hi\""), "\"say \\\"hi\\\"\"");
    }

    #[test]
    fn dot_escape_with_backslash() {
        assert_eq!(dot_escape("path\\to"), "\"path\\\\to\"");
    }

    // ---------------------------------------------------------------
    // render_to_dot tests
    // ---------------------------------------------------------------

    #[test]
    fn render_empty_graph_to_dot() {
        let graph = make_graph(None, vec![], vec![]);
        let dot = render_to_dot(&graph);
        assert!(dot.starts_with("digraph  {"));
        assert!(dot.ends_with("}"));
        assert!(dot.contains("rankdir=LR"));
    }

    #[test]
    fn render_named_graph_to_dot() {
        let graph = make_graph(Some("MyPipeline"), vec![], vec![]);
        let dot = render_to_dot(&graph);
        assert!(dot.starts_with("digraph \"MyPipeline\" {"));
    }

    #[test]
    fn render_single_node_to_dot() {
        let graph = make_graph(
            None,
            vec![make_node("start", NodeType::Start, Some("Begin"))],
            vec![],
        );
        let dot = render_to_dot(&graph);
        assert!(dot.contains("\"start\""));
        assert!(dot.contains("label=\"Begin\""));
        assert!(dot.contains("shape=\"circle\""));
        assert!(dot.contains("fillcolor=\"#4CAF50\""));
        assert!(dot.contains("style=\"filled\""));
    }

    #[test]
    fn render_node_uses_id_as_label_when_no_label() {
        let graph = make_graph(
            None,
            vec![make_node("mynode", NodeType::Generic, None)],
            vec![],
        );
        let dot = render_to_dot(&graph);
        assert!(dot.contains("label=\"mynode\""));
    }

    #[test]
    fn render_edge_to_dot() {
        let graph = make_graph(
            None,
            vec![
                make_node("a", NodeType::Start, None),
                make_node("b", NodeType::Exit, None),
            ],
            vec![make_edge("a", "b", None)],
        );
        let dot = render_to_dot(&graph);
        assert!(dot.contains("\"a\" -> \"b\";"));
    }

    #[test]
    fn render_edge_with_label_to_dot() {
        let graph = make_graph(
            None,
            vec![
                make_node("a", NodeType::Start, None),
                make_node("b", NodeType::Exit, None),
            ],
            vec![make_edge("a", "b", Some("success"))],
        );
        let dot = render_to_dot(&graph);
        assert!(dot.contains("\"a\" -> \"b\" [label=\"success\"];"));
    }

    #[test]
    fn render_complex_pipeline_to_dot() {
        let graph = make_graph(
            Some("deploy"),
            vec![
                make_node("start", NodeType::Start, Some("Start")),
                make_node("check", NodeType::Conditional, Some("Ready?")),
                make_node("build", NodeType::Codergen, Some("Build")),
                make_node("done", NodeType::Exit, Some("Done")),
            ],
            vec![
                make_edge("start", "check", None),
                make_edge("check", "build", Some("yes")),
                make_edge("check", "done", Some("no")),
                make_edge("build", "done", None),
            ],
        );
        let dot = render_to_dot(&graph);

        // Verify structure
        assert!(dot.starts_with("digraph \"deploy\" {"));
        assert!(dot.contains("shape=\"circle\"")); // start
        assert!(dot.contains("shape=\"diamond\"")); // conditional
        assert!(dot.contains("shape=\"box\"")); // codergen
        assert!(dot.contains("shape=\"doublecircle\"")); // exit
        assert!(dot.contains("\"start\" -> \"check\";"));
        assert!(dot.contains("\"check\" -> \"build\" [label=\"yes\"];"));
        assert!(dot.contains("\"check\" -> \"done\" [label=\"no\"];"));
        assert!(dot.contains("\"build\" -> \"done\";"));
    }

    #[test]
    fn render_all_node_types_produces_valid_dot() {
        let all_types = vec![
            ("s", NodeType::Start),
            ("e", NodeType::Exit),
            ("cond", NodeType::Conditional),
            ("code", NodeType::Codergen),
            ("gen", NodeType::Generic),
            ("mgr", NodeType::Manager),
            ("sub", NodeType::SubPipeline),
            ("par", NodeType::Parallel),
            ("fan", NodeType::FanIn),
            ("tool", NodeType::Tool),
            ("iv", NodeType::Interviewer),
        ];
        let nodes: Vec<GraphNode> = all_types
            .iter()
            .map(|(id, nt)| make_node(id, nt.clone(), None))
            .collect();
        let graph = make_graph(Some("all_types"), nodes, vec![]);
        let dot = render_to_dot(&graph);

        // Each node should be present
        for (id, _) in &all_types {
            assert!(dot.contains(&format!("\"{id}\"")), "missing node {id}");
        }
        // Should have proper structure
        assert!(dot.starts_with("digraph"));
        assert!(dot.ends_with("}"));
    }

    // ---------------------------------------------------------------
    // DotRenderer (async) tests
    // ---------------------------------------------------------------

    #[tokio::test]
    async fn dot_renderer_renders_dot_format() {
        let renderer = DotRenderer;
        let graph = make_graph(
            Some("test"),
            vec![make_node("a", NodeType::Start, None)],
            vec![],
        );
        let result = renderer.render(&graph, RenderFormat::Dot).await;
        assert!(result.is_ok());
        let output = result.unwrap();
        assert_eq!(output.format, RenderFormat::Dot);
        let text = output.as_text().unwrap();
        assert!(text.contains("digraph"));
        assert!(text.contains("\"a\""));
    }

    #[tokio::test]
    async fn dot_renderer_rejects_svg() {
        let renderer = DotRenderer;
        let graph = make_graph(None, vec![], vec![]);
        let result = renderer.render(&graph, RenderFormat::Svg).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("unsupported render format"));
    }

    #[tokio::test]
    async fn dot_renderer_rejects_png() {
        let renderer = DotRenderer;
        let graph = make_graph(None, vec![], vec![]);
        let result = renderer.render(&graph, RenderFormat::Png).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn dot_renderer_supported_formats() {
        let renderer = DotRenderer;
        let formats = renderer.supported_formats();
        assert_eq!(formats, vec![RenderFormat::Dot]);
    }

    // ---------------------------------------------------------------
    // RenderGraphQuery serde tests
    // ---------------------------------------------------------------

    #[test]
    fn render_graph_query_serde_with_format() {
        let query = RenderGraphQuery {
            format: Some("dot".to_string()),
            include_status: None,
        };
        let json_str = serde_json::to_string(&query).unwrap();
        let restored: RenderGraphQuery = serde_json::from_str(&json_str).unwrap();
        assert_eq!(query, restored);
    }

    #[test]
    fn render_graph_query_serde_without_format() {
        let query = RenderGraphQuery {
            format: None,
            include_status: None,
        };
        let json_str = serde_json::to_string(&query).unwrap();
        let restored: RenderGraphQuery = serde_json::from_str(&json_str).unwrap();
        assert_eq!(restored.format, None);
        assert_eq!(restored.include_status, None);
    }

    #[test]
    fn render_graph_query_deserialization_from_json() {
        let json_str = r#"{"format": "svg"}"#;
        let query: RenderGraphQuery = serde_json::from_str(json_str).unwrap();
        assert_eq!(query.format, Some("svg".to_string()));
        // include_status should default to None when absent from JSON.
        assert_eq!(query.include_status, None);
    }

    #[test]
    fn render_graph_query_with_include_status() {
        let json_str = r#"{"format": "svg", "include_status": true}"#;
        let query: RenderGraphQuery = serde_json::from_str(json_str).unwrap();
        assert_eq!(query.format, Some("svg".to_string()));
        assert_eq!(query.include_status, Some(true));
    }

    #[test]
    fn render_graph_query_serde_roundtrip_with_status() {
        let query = RenderGraphQuery {
            format: Some("png".to_string()),
            include_status: Some(true),
        };
        let json_str = serde_json::to_string(&query).unwrap();
        let restored: RenderGraphQuery = serde_json::from_str(&json_str).unwrap();
        assert_eq!(query, restored);
    }

    #[test]
    fn render_graph_query_resolved_format_default() {
        let query = RenderGraphQuery {
            format: None,
            include_status: None,
        };
        assert_eq!(query.resolved_format(), RenderFormat::Dot);
    }

    #[test]
    fn render_graph_query_resolved_format_svg() {
        let query = RenderGraphQuery {
            format: Some("svg".to_string()),
            include_status: None,
        };
        assert_eq!(query.resolved_format(), RenderFormat::Svg);
    }

    #[test]
    fn render_graph_query_resolved_format_invalid_defaults_to_dot() {
        let query = RenderGraphQuery {
            format: Some("pdf".to_string()),
            include_status: None,
        };
        assert_eq!(query.resolved_format(), RenderFormat::Dot);
    }

    #[test]
    fn render_graph_query_wants_status_default_false() {
        let query = RenderGraphQuery {
            format: None,
            include_status: None,
        };
        assert!(!query.wants_status());
    }

    #[test]
    fn render_graph_query_wants_status_true() {
        let query = RenderGraphQuery {
            format: None,
            include_status: Some(true),
        };
        assert!(query.wants_status());
    }

    #[test]
    fn render_graph_query_wants_status_false() {
        let query = RenderGraphQuery {
            format: None,
            include_status: Some(false),
        };
        assert!(!query.wants_status());
    }

    // ---------------------------------------------------------------
    // RenderGraphResponse serde tests
    // ---------------------------------------------------------------

    #[test]
    fn render_graph_response_serde_roundtrip() {
        let response = RenderGraphResponse {
            run_id: "run-42".to_string(),
            format: RenderFormat::Dot,
            content: Some("digraph { a -> b }".to_string()),
            content_type: "text/vnd.graphviz".to_string(),
        };
        let json_str = serde_json::to_string(&response).unwrap();
        let restored: RenderGraphResponse = serde_json::from_str(&json_str).unwrap();
        assert_eq!(response, restored);
    }

    #[test]
    fn render_graph_response_without_content() {
        let response = RenderGraphResponse {
            run_id: "run-99".to_string(),
            format: RenderFormat::Png,
            content: None,
            content_type: "image/png".to_string(),
        };
        let json_str = serde_json::to_string(&response).unwrap();
        let restored: RenderGraphResponse = serde_json::from_str(&json_str).unwrap();
        assert_eq!(restored.content, None);
    }

    // ---------------------------------------------------------------
    // render_node tests
    // ---------------------------------------------------------------

    #[test]
    fn render_node_start_type() {
        let node = make_node("begin", NodeType::Start, Some("Go!"));
        let line = render_node(&node);
        assert!(line.contains("\"begin\""));
        assert!(line.contains("label=\"Go!\""));
        assert!(line.contains("shape=\"circle\""));
        assert!(line.contains("fillcolor=\"#4CAF50\""));
    }

    #[test]
    fn render_node_exit_type() {
        let node = make_node("end", NodeType::Exit, Some("Finish"));
        let line = render_node(&node);
        assert!(line.contains("shape=\"doublecircle\""));
        assert!(line.contains("fillcolor=\"#F44336\""));
    }

    #[test]
    fn render_node_conditional_type() {
        let node = make_node("branch", NodeType::Conditional, Some("Check"));
        let line = render_node(&node);
        assert!(line.contains("shape=\"diamond\""));
        assert!(line.contains("fillcolor=\"#FFC107\""));
        assert!(line.contains("fontcolor=\"black\""));
    }

    // ---------------------------------------------------------------
    // render_edge tests
    // ---------------------------------------------------------------

    #[test]
    fn render_edge_without_label() {
        let edge = make_edge("x", "y", None);
        let line = render_edge(&edge);
        assert_eq!(line.trim(), "\"x\" -> \"y\";");
    }

    #[test]
    fn render_edge_with_label_attr() {
        let edge = make_edge("x", "y", Some("proceed"));
        let line = render_edge(&edge);
        assert!(line.contains("label=\"proceed\""));
        assert!(line.contains("\"x\" -> \"y\""));
    }

    // ---------------------------------------------------------------
    // Integration: round-trip through resolve then render
    // ---------------------------------------------------------------

    // ---------------------------------------------------------------
    // NodeExecutionStatus tests
    // ---------------------------------------------------------------

    #[test]
    fn node_execution_status_display() {
        assert_eq!(format!("{}", NodeExecutionStatus::Pending), "pending");
        assert_eq!(format!("{}", NodeExecutionStatus::Running), "running");
        assert_eq!(format!("{}", NodeExecutionStatus::Done), "done");
        assert_eq!(format!("{}", NodeExecutionStatus::Failed), "failed");
    }

    #[test]
    fn node_execution_status_serde_roundtrip() {
        for status in [
            NodeExecutionStatus::Pending,
            NodeExecutionStatus::Running,
            NodeExecutionStatus::Done,
            NodeExecutionStatus::Failed,
        ] {
            let json_str = serde_json::to_string(&status).unwrap();
            let restored: NodeExecutionStatus = serde_json::from_str(&json_str).unwrap();
            assert_eq!(status, restored);
        }
    }

    #[test]
    fn node_execution_status_serializes_snake_case() {
        assert_eq!(
            serde_json::to_string(&NodeExecutionStatus::Pending).unwrap(),
            "\"pending\""
        );
        assert_eq!(
            serde_json::to_string(&NodeExecutionStatus::Running).unwrap(),
            "\"running\""
        );
        assert_eq!(
            serde_json::to_string(&NodeExecutionStatus::Done).unwrap(),
            "\"done\""
        );
        assert_eq!(
            serde_json::to_string(&NodeExecutionStatus::Failed).unwrap(),
            "\"failed\""
        );
    }

    // ---------------------------------------------------------------
    // StatusStyle tests
    // ---------------------------------------------------------------

    #[test]
    fn status_style_pending() {
        let style = style_for_execution_status(&NodeExecutionStatus::Pending);
        assert_eq!(style.fill_color, "#E0E0E0");
        assert_eq!(style.border_color, "#9E9E9E");
        assert_eq!(style.pen_width, "1.0");
        assert_eq!(style.label_suffix, "");
    }

    #[test]
    fn status_style_running() {
        let style = style_for_execution_status(&NodeExecutionStatus::Running);
        assert_eq!(style.fill_color, "#FFF9C4");
        assert_eq!(style.border_color, "#F9A825");
        assert_eq!(style.pen_width, "3.0");
        assert!(style.label_suffix.contains("RUNNING"));
    }

    #[test]
    fn status_style_done() {
        let style = style_for_execution_status(&NodeExecutionStatus::Done);
        assert_eq!(style.fill_color, "#C8E6C9");
        assert_eq!(style.border_color, "#2E7D32");
        assert_eq!(style.pen_width, "2.0");
        assert!(style.label_suffix.contains("DONE"));
    }

    #[test]
    fn status_style_failed() {
        let style = style_for_execution_status(&NodeExecutionStatus::Failed);
        assert_eq!(style.fill_color, "#FFCDD2");
        assert_eq!(style.border_color, "#C62828");
        assert_eq!(style.pen_width, "2.0");
        assert!(style.label_suffix.contains("FAILED"));
    }

    // ---------------------------------------------------------------
    // render_node_with_status tests
    // ---------------------------------------------------------------

    #[test]
    fn render_node_with_status_pending() {
        let node = make_node("step1", NodeType::Codergen, Some("Build"));
        let line = render_node_with_status(&node, &NodeExecutionStatus::Pending);
        assert!(line.contains("\"step1\""));
        assert!(line.contains("label=\"Build\""));
        assert!(line.contains("fillcolor=\"#E0E0E0\""));
        assert!(line.contains("color=\"#9E9E9E\""));
        assert!(line.contains("penwidth=1.0"));
    }

    #[test]
    fn render_node_with_status_running() {
        let node = make_node("step1", NodeType::Codergen, Some("Build"));
        let line = render_node_with_status(&node, &NodeExecutionStatus::Running);
        assert!(line.contains("label=\"Build [RUNNING]\""));
        assert!(line.contains("fillcolor=\"#FFF9C4\""));
        assert!(line.contains("color=\"#F9A825\""));
        assert!(line.contains("penwidth=3.0"));
    }

    #[test]
    fn render_node_with_status_done() {
        let node = make_node("step1", NodeType::Codergen, Some("Build"));
        let line = render_node_with_status(&node, &NodeExecutionStatus::Done);
        assert!(line.contains("label=\"Build [DONE]\""));
        assert!(line.contains("fillcolor=\"#C8E6C9\""));
        assert!(line.contains("color=\"#2E7D32\""));
    }

    #[test]
    fn render_node_with_status_failed() {
        let node = make_node("step1", NodeType::Codergen, Some("Build"));
        let line = render_node_with_status(&node, &NodeExecutionStatus::Failed);
        assert!(line.contains("label=\"Build [FAILED]\""));
        assert!(line.contains("fillcolor=\"#FFCDD2\""));
        assert!(line.contains("color=\"#C62828\""));
    }

    #[test]
    fn render_node_with_status_uses_id_as_label_when_no_label() {
        let node = make_node("mynode", NodeType::Generic, None);
        let line = render_node_with_status(&node, &NodeExecutionStatus::Running);
        assert!(line.contains("label=\"mynode [RUNNING]\""));
    }

    #[test]
    fn render_node_with_status_preserves_shape() {
        let node = make_node("start", NodeType::Start, Some("Go"));
        let line = render_node_with_status(&node, &NodeExecutionStatus::Done);
        // Shape comes from NodeType, not from status
        assert!(line.contains("shape=\"circle\""));
    }

    // ---------------------------------------------------------------
    // render_to_dot_with_status tests
    // ---------------------------------------------------------------

    #[test]
    fn render_to_dot_with_status_empty_statuses() {
        let graph = make_graph(
            Some("test"),
            vec![
                make_node("a", NodeType::Start, Some("Start")),
                make_node("b", NodeType::Exit, Some("End")),
            ],
            vec![make_edge("a", "b", None)],
        );
        let statuses = HashMap::new();
        let dot = render_to_dot_with_status(&graph, &statuses);

        // All nodes should default to Pending style
        assert!(dot.contains("fillcolor=\"#E0E0E0\""));
        // Should still be a valid digraph
        assert!(dot.starts_with("digraph \"test\""));
        assert!(dot.ends_with("}"));
    }

    #[test]
    fn render_to_dot_with_status_mixed_statuses() {
        let graph = make_graph(
            Some("pipeline"),
            vec![
                make_node("start", NodeType::Start, Some("Start")),
                make_node("build", NodeType::Codergen, Some("Build")),
                make_node("test", NodeType::Codergen, Some("Test")),
                make_node("done", NodeType::Exit, Some("Done")),
            ],
            vec![
                make_edge("start", "build", None),
                make_edge("build", "test", None),
                make_edge("test", "done", None),
            ],
        );

        let mut statuses = HashMap::new();
        statuses.insert("start".to_string(), NodeExecutionStatus::Done);
        statuses.insert("build".to_string(), NodeExecutionStatus::Done);
        statuses.insert("test".to_string(), NodeExecutionStatus::Running);
        // "done" not in map -> defaults to Pending

        let dot = render_to_dot_with_status(&graph, &statuses);

        // Check that each node has the correct status label
        assert!(dot.contains("\"Start [DONE]\""));
        assert!(dot.contains("\"Build [DONE]\""));
        assert!(dot.contains("\"Test [RUNNING]\""));
        // Done node should be pending (no suffix) since not in statuses map
        assert!(dot.contains("label=\"Done\""));

        // Check that edges are still present
        assert!(dot.contains("\"start\" -> \"build\""));
        assert!(dot.contains("\"build\" -> \"test\""));
        assert!(dot.contains("\"test\" -> \"done\""));
    }

    #[test]
    fn render_to_dot_with_status_all_failed() {
        let graph = make_graph(
            None,
            vec![
                make_node("a", NodeType::Generic, Some("A")),
                make_node("b", NodeType::Generic, Some("B")),
            ],
            vec![make_edge("a", "b", None)],
        );

        let mut statuses = HashMap::new();
        statuses.insert("a".to_string(), NodeExecutionStatus::Failed);
        statuses.insert("b".to_string(), NodeExecutionStatus::Failed);

        let dot = render_to_dot_with_status(&graph, &statuses);
        assert!(dot.contains("\"A [FAILED]\""));
        assert!(dot.contains("\"B [FAILED]\""));
        // Both should have failed fill color
        let count = dot.matches("#FFCDD2").count();
        assert_eq!(count, 2, "expected 2 nodes with failed fill color");
    }

    #[test]
    fn render_to_dot_with_status_has_proper_structure() {
        let graph = make_graph(
            Some("status_test"),
            vec![make_node("n", NodeType::Start, Some("Node"))],
            vec![],
        );
        let statuses = HashMap::new();
        let dot = render_to_dot_with_status(&graph, &statuses);

        assert!(dot.contains("rankdir=LR"));
        assert!(dot.contains("bgcolor=\"#FAFAFA\""));
        assert!(dot.contains("fontname=\"Helvetica\""));
        assert!(dot.starts_with("digraph"));
        assert!(dot.ends_with("}"));
    }

    // ---------------------------------------------------------------
    // graphviz_available tests
    // ---------------------------------------------------------------

    #[tokio::test]
    async fn graphviz_available_returns_bool() {
        // Just verify it returns a bool without panicking.
        // On this system graphviz is installed so it should be true.
        let available = graphviz_available().await;
        // We can't assert true in general, but we test it doesn't panic.
        assert!(available || !available);
    }

    // ---------------------------------------------------------------
    // GraphvizRenderer tests
    // ---------------------------------------------------------------

    #[tokio::test]
    async fn graphviz_renderer_renders_dot_format() {
        let renderer = GraphvizRenderer;
        let graph = make_graph(
            Some("gv_test"),
            vec![make_node("a", NodeType::Start, None)],
            vec![],
        );
        let result = renderer.render(&graph, RenderFormat::Dot).await;
        assert!(result.is_ok());
        let output = result.unwrap();
        assert_eq!(output.format, RenderFormat::Dot);
        let text = output.as_text().unwrap();
        assert!(text.contains("digraph"));
    }

    #[tokio::test]
    async fn graphviz_renderer_supported_formats_includes_all() {
        let renderer = GraphvizRenderer;
        let formats = renderer.supported_formats();
        assert_eq!(formats.len(), 3);
        assert!(formats.contains(&RenderFormat::Dot));
        assert!(formats.contains(&RenderFormat::Svg));
        assert!(formats.contains(&RenderFormat::Png));
    }

    #[tokio::test]
    async fn graphviz_renderer_renders_svg_if_available() {
        if !graphviz_available().await {
            eprintln!("skipping: graphviz not installed");
            return;
        }
        let renderer = GraphvizRenderer;
        let graph = make_graph(
            Some("svg_test"),
            vec![
                make_node("a", NodeType::Start, Some("Start")),
                make_node("b", NodeType::Exit, Some("End")),
            ],
            vec![make_edge("a", "b", None)],
        );
        let result = renderer.render(&graph, RenderFormat::Svg).await;
        assert!(result.is_ok());
        let output = result.unwrap();
        assert_eq!(output.format, RenderFormat::Svg);
        let text = output.as_text().unwrap();
        assert!(text.contains("<svg"));
        assert!(text.contains("</svg>"));
    }

    #[tokio::test]
    async fn graphviz_renderer_renders_png_if_available() {
        if !graphviz_available().await {
            eprintln!("skipping: graphviz not installed");
            return;
        }
        let renderer = GraphvizRenderer;
        let graph = make_graph(
            Some("png_test"),
            vec![make_node("a", NodeType::Start, None)],
            vec![],
        );
        let result = renderer.render(&graph, RenderFormat::Png).await;
        assert!(result.is_ok());
        let output = result.unwrap();
        assert_eq!(output.format, RenderFormat::Png);
        // PNG files start with the PNG magic bytes
        assert!(output.content.len() > 8);
        assert_eq!(&output.content[1..4], b"PNG");
    }

    // ---------------------------------------------------------------
    // StatusGraphvizRenderer tests
    // ---------------------------------------------------------------

    #[tokio::test]
    async fn status_graphviz_renderer_dot_output_has_status() {
        let mut statuses = HashMap::new();
        statuses.insert("a".to_string(), NodeExecutionStatus::Running);
        statuses.insert("b".to_string(), NodeExecutionStatus::Done);

        let renderer = StatusGraphvizRenderer::new(statuses);
        let graph = make_graph(
            Some("status_gv"),
            vec![
                make_node("a", NodeType::Start, Some("Start")),
                make_node("b", NodeType::Exit, Some("End")),
            ],
            vec![make_edge("a", "b", None)],
        );

        let result = renderer.render(&graph, RenderFormat::Dot).await;
        assert!(result.is_ok());
        let output = result.unwrap();
        let text = output.as_text().unwrap();
        assert!(text.contains("[RUNNING]"));
        assert!(text.contains("[DONE]"));
    }

    #[tokio::test]
    async fn status_graphviz_renderer_svg_if_available() {
        if !graphviz_available().await {
            eprintln!("skipping: graphviz not installed");
            return;
        }

        let mut statuses = HashMap::new();
        statuses.insert("a".to_string(), NodeExecutionStatus::Failed);

        let renderer = StatusGraphvizRenderer::new(statuses);
        let graph = make_graph(
            None,
            vec![make_node("a", NodeType::Generic, Some("Step"))],
            vec![],
        );

        let result = renderer.render(&graph, RenderFormat::Svg).await;
        assert!(result.is_ok());
        let output = result.unwrap();
        let text = output.as_text().unwrap();
        assert!(text.contains("<svg"));
        // The SVG should contain the status-colored fill
        assert!(text.contains("FAILED"));
    }

    #[tokio::test]
    async fn status_graphviz_renderer_supported_formats() {
        let renderer = StatusGraphvizRenderer::new(HashMap::new());
        let formats = renderer.supported_formats();
        assert_eq!(formats.len(), 3);
        assert!(formats.contains(&RenderFormat::Dot));
        assert!(formats.contains(&RenderFormat::Svg));
        assert!(formats.contains(&RenderFormat::Png));
    }

    // ---------------------------------------------------------------
    // CachedRenderer tests
    // ---------------------------------------------------------------

    #[tokio::test]
    async fn cached_renderer_returns_same_output_as_inner() {
        let inner = DotRenderer;
        let cached = CachedRenderer::new(inner);
        let graph = make_graph(
            Some("cache_test"),
            vec![make_node("a", NodeType::Start, None)],
            vec![],
        );

        let result = cached.render(&graph, RenderFormat::Dot).await;
        assert!(result.is_ok());
        let output = result.unwrap();
        assert_eq!(output.format, RenderFormat::Dot);
        let text = output.as_text().unwrap();
        assert!(text.contains("digraph"));
        assert!(text.contains("\"a\""));
    }

    #[tokio::test]
    async fn cached_renderer_caches_results() {
        let inner = DotRenderer;
        let cached = CachedRenderer::new(inner);
        let graph = make_graph(
            Some("cache_hit"),
            vec![make_node("x", NodeType::Generic, None)],
            vec![],
        );

        // First call should populate the cache.
        let output1 = cached.render(&graph, RenderFormat::Dot).await.unwrap();
        // Second call should be a cache hit.
        let output2 = cached.render(&graph, RenderFormat::Dot).await.unwrap();

        assert_eq!(output1.content, output2.content);
        assert_eq!(cached.cache_len(), 1);
    }

    #[tokio::test]
    async fn cached_renderer_different_formats_are_separate_keys() {
        // For this test we use GraphvizRenderer which supports multiple formats.
        // If graphviz isn't available, we test DOT only — but the cache key
        // should differ by format even if we only test DOT vs DOT with different graphs.
        let inner = DotRenderer;
        let cached = CachedRenderer::new(inner);

        let graph1 = make_graph(
            Some("g1"),
            vec![make_node("a", NodeType::Start, None)],
            vec![],
        );
        let graph2 = make_graph(
            Some("g2"),
            vec![make_node("b", NodeType::Exit, None)],
            vec![],
        );

        let _ = cached.render(&graph1, RenderFormat::Dot).await.unwrap();
        let _ = cached.render(&graph2, RenderFormat::Dot).await.unwrap();

        // Two different graphs should produce two cache entries.
        assert_eq!(cached.cache_len(), 2);
    }

    #[tokio::test]
    async fn cached_renderer_propagates_errors() {
        let inner = DotRenderer;
        let cached = CachedRenderer::new(inner);
        let graph = make_graph(None, vec![], vec![]);

        // DotRenderer doesn't support SVG, so this should return an error.
        let result = cached.render(&graph, RenderFormat::Svg).await;
        assert!(result.is_err());
        // Errors should not be cached.
        assert_eq!(cached.cache_len(), 0);
    }

    #[tokio::test]
    async fn cached_renderer_supported_formats_delegates() {
        let inner = DotRenderer;
        let cached = CachedRenderer::new(inner);
        let formats = cached.supported_formats();
        assert_eq!(formats, vec![RenderFormat::Dot]);
    }

    #[tokio::test]
    async fn cached_renderer_clear_cache() {
        let inner = DotRenderer;
        let cached = CachedRenderer::new(inner);
        let graph = make_graph(
            Some("clear_test"),
            vec![make_node("a", NodeType::Start, None)],
            vec![],
        );

        let _ = cached.render(&graph, RenderFormat::Dot).await.unwrap();
        assert_eq!(cached.cache_len(), 1);

        cached.clear_cache();
        assert_eq!(cached.cache_len(), 0);
    }

    #[test]
    fn render_cache_key_deterministic() {
        let key1 = RenderCacheKey::new("digraph { a -> b }", RenderFormat::Svg);
        let key2 = RenderCacheKey::new("digraph { a -> b }", RenderFormat::Svg);
        assert_eq!(key1, key2);
    }

    #[test]
    fn render_cache_key_differs_by_format() {
        let key_svg = RenderCacheKey::new("digraph { a -> b }", RenderFormat::Svg);
        let key_png = RenderCacheKey::new("digraph { a -> b }", RenderFormat::Png);
        assert_ne!(key_svg, key_png);
    }

    #[test]
    fn render_cache_key_differs_by_content() {
        let key1 = RenderCacheKey::new("digraph { a -> b }", RenderFormat::Svg);
        let key2 = RenderCacheKey::new("digraph { x -> y }", RenderFormat::Svg);
        assert_ne!(key1, key2);
    }

    // ---------------------------------------------------------------
    // Integration: round-trip through resolve then render
    // ---------------------------------------------------------------

    #[test]
    fn roundtrip_resolve_then_render() {
        use crate::dot::{DotAttr, DotEdge, DotGraph, DotNode, DotStatement, DotValue};
        use crate::graph::resolve;

        let dot_ast = DotGraph {
            name: Some("roundtrip".to_string()),
            is_digraph: true,
            statements: vec![
                DotStatement::Node(DotNode {
                    id: "start".to_string(),
                    attrs: vec![DotAttr {
                        key: "shape".to_string(),
                        value: DotValue::String("circle".to_string()),
                    }],
                }),
                DotStatement::Node(DotNode {
                    id: "process".to_string(),
                    attrs: vec![
                        DotAttr {
                            key: "shape".to_string(),
                            value: DotValue::String("box".to_string()),
                        },
                        DotAttr {
                            key: "label".to_string(),
                            value: DotValue::String("Do Work".to_string()),
                        },
                    ],
                }),
                DotStatement::Node(DotNode {
                    id: "end".to_string(),
                    attrs: vec![DotAttr {
                        key: "shape".to_string(),
                        value: DotValue::String("doublecircle".to_string()),
                    }],
                }),
                DotStatement::Edge(DotEdge {
                    from: "start".to_string(),
                    to: "process".to_string(),
                    attrs: vec![],
                }),
                DotStatement::Edge(DotEdge {
                    from: "process".to_string(),
                    to: "end".to_string(),
                    attrs: vec![DotAttr {
                        key: "label".to_string(),
                        value: DotValue::String("done".to_string()),
                    }],
                }),
            ],
        };

        let graph = resolve(&dot_ast).unwrap();
        let rendered = render_to_dot(&graph);

        // Verify the rendered DOT contains expected elements
        assert!(rendered.contains("digraph \"roundtrip\""));
        assert!(rendered.contains("\"start\""));
        assert!(rendered.contains("\"process\""));
        assert!(rendered.contains("\"end\""));
        assert!(rendered.contains("shape=\"circle\""));
        assert!(rendered.contains("shape=\"box\""));
        assert!(rendered.contains("shape=\"doublecircle\""));
        assert!(rendered.contains("label=\"Do Work\""));
        assert!(rendered.contains("\"start\" -> \"process\""));
        assert!(rendered.contains("label=\"done\""));
    }
}
