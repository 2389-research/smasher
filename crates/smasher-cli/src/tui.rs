// ABOUTME: Boba-based TUI model for interactive pipeline execution visualization.
// ABOUTME: Implements the Elm Architecture (init/update/view) with boba widgets for nodes and logs.

use std::collections::HashMap;
use std::time::Duration;

use boba::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use boba::ratatui::Frame;
use boba::ratatui::layout::{Constraint, Layout, Rect};
use boba::ratatui::style::{Color, Modifier, Style};
use boba::ratatui::text::{Line, Span};
use boba::ratatui::widgets::{Block, Borders, List, ListItem};
use boba::widgets::spinner::{self, Spinner, frames};
use boba::widgets::status_bar::StatusBar;
use boba::widgets::stopwatch::{self, Stopwatch};
use boba::widgets::viewport::{self, Viewport};
use boba::{
    Command, Component, Model, OutputTarget, Program, ProgramError, ProgramOptions, Subscription,
    TerminalEvent, terminal_events,
};
use smasher_attractor::events::PipelineEvent;
use smasher_attractor::graph::Graph;

/// Initialization flags passed to PipelineTui at construction time.
pub struct TuiFlags {
    pub graph: Graph,
    pub run_id: String,
    pub pipeline_name: String,
    /// Node IDs that were already completed before this run (e.g. from a resumed checkpoint).
    pub completed_node_ids: Vec<String>,
}

/// Messages handled by the PipelineTui model.
#[derive(Debug, Clone)]
pub enum Msg {
    PipelineEvent(PipelineEvent),
    KeyPress(KeyEvent),
    Resize(u16, u16),
    SpinnerMsg(spinner::Message),
    StopwatchMsg(stopwatch::Message),
    ViewportMsg(viewport::Message),
    ConsoleViewportMsg(viewport::Message),
    MouseScroll { up: bool },
    PipelineDone,
    Quit,
}

/// Status of an individual pipeline node.
#[derive(Debug, Clone, PartialEq)]
pub enum NodeStatus {
    Pending,
    Running,
    Completed,
    Failed,
}

/// State for a single node in the pipeline.
pub struct NodeState {
    pub id: String,
    pub label: String,
    pub status: NodeStatus,
}

/// A single log entry in the execution log stream.
#[derive(Clone)]
pub struct LogEntry {
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub node_id: Option<String>,
    pub kind: LogKind,
    pub content: String,
}

/// Semantic category for a log entry, used to determine display styling.
#[derive(Clone)]
pub enum LogKind {
    PipelineStart,
    PipelineEnd,
    NodeStart,
    NodeComplete,
    NodeFail,
    EdgeTraversal,
    AgentTurn,
    ToolCallStart,
    ToolCallComplete,
    AgentText,
    HumanPrompt,
    HumanResponse,
    Info,
}

/// Which panel currently receives keyboard navigation input.
#[derive(Debug, Clone, PartialEq)]
pub enum PanelFocus {
    Nodes,
    Logs,
    Console,
}

/// Overall pipeline execution status.
#[derive(Debug, Clone, PartialEq)]
pub enum PipelineStatus {
    Starting,
    Running,
    Completed,
    Failed,
    Aborted,
}

/// The main TUI model for pipeline execution.
#[allow(dead_code)]
pub struct PipelineTui {
    nodes: Vec<NodeState>,
    node_index: HashMap<String, usize>,
    selected_node: usize,
    log_entries: Vec<LogEntry>,
    /// Unified stream of agent events across all nodes, shown in the bottom console panel.
    console_entries: Vec<LogEntry>,
    pipeline_name: String,
    run_id: String,
    status: PipelineStatus,
    done: bool,
    spinner: Spinner,
    /// Local frame index mirror so we can display the spinner char in the title bar.
    spinner_frame_idx: usize,
    stopwatch: Stopwatch,
    log_viewport: Viewport,
    /// Scrollable viewport for the unified console panel.
    console_viewport: Viewport,
    focus: PanelFocus,
    finished_node_count: usize,
    /// Cumulative input tokens across all nodes.
    total_input_tokens: u64,
    /// Cumulative output tokens across all nodes.
    total_output_tokens: u64,
    /// Cumulative cost in USD across all nodes.
    total_cost_usd: f64,
    /// Set of file paths touched by Write/Edit tool calls.
    files_touched: std::collections::HashSet<String>,
}

impl Model for PipelineTui {
    type Message = Msg;
    type Flags = TuiFlags;

    fn init(flags: TuiFlags) -> (Self, Command<Msg>) {
        let nodes: Vec<NodeState> = flags
            .graph
            .nodes
            .iter()
            .map(|n| NodeState {
                id: n.id.clone(),
                label: n.label.clone().unwrap_or_else(|| n.id.clone()),
                status: NodeStatus::Pending,
            })
            .collect();

        let node_index: HashMap<String, usize> = nodes
            .iter()
            .enumerate()
            .map(|(i, n)| (n.id.clone(), i))
            .collect();

        let mut spinner = Spinner::new("pipeline-spinner")
            .with_frames(frames::DOTS)
            .with_interval(Duration::from_millis(80));
        spinner.start();

        let mut stopwatch = Stopwatch::new("pipeline-elapsed");
        stopwatch.start();

        let log_viewport = Viewport::new("").with_mouse_wheel(true);
        let console_viewport = Viewport::new("").with_mouse_wheel(true);

        let mut model = PipelineTui {
            nodes,
            node_index,
            selected_node: 0,
            log_entries: Vec::new(),
            console_entries: Vec::new(),
            pipeline_name: flags.pipeline_name,
            run_id: flags.run_id,
            status: PipelineStatus::Starting,
            done: false,
            spinner,
            spinner_frame_idx: 0,
            stopwatch,
            log_viewport,
            console_viewport,
            focus: PanelFocus::Nodes,
            finished_node_count: 0,
            total_input_tokens: 0,
            total_output_tokens: 0,
            total_cost_usd: 0.0,
            files_touched: std::collections::HashSet::new(),
        };

        // Mark nodes that were already completed in a previous run (resume).
        for id in &flags.completed_node_ids {
            if let Some(&idx) = model.node_index.get(id) {
                model.nodes[idx].status = NodeStatus::Completed;
                model.finished_node_count += 1;
            }
        }

        (model, Command::none())
    }

    fn update(&mut self, msg: Msg) -> Command<Msg> {
        match msg {
            Msg::PipelineEvent(event) => self.handle_pipeline_event(event),

            Msg::KeyPress(key) => self.handle_key(key),

            Msg::Resize(_, _) => Command::none(),

            Msg::SpinnerMsg(m) => {
                if matches!(m, spinner::Message::Tick) {
                    self.spinner_frame_idx = (self.spinner_frame_idx + 1) % frames::DOTS.len();
                }
                self.spinner.update(m).map(Msg::SpinnerMsg)
            }

            Msg::StopwatchMsg(m) => self.stopwatch.update(m).map(Msg::StopwatchMsg),

            Msg::ViewportMsg(m) => self.log_viewport.update(m).map(Msg::ViewportMsg),

            Msg::ConsoleViewportMsg(m) => {
                self.console_viewport.update(m).map(Msg::ConsoleViewportMsg)
            }

            Msg::MouseScroll { up } => {
                let wheel = viewport::Message::MouseWheel { up };
                match self.focus {
                    PanelFocus::Console => self
                        .console_viewport
                        .update(wheel)
                        .map(Msg::ConsoleViewportMsg),
                    _ => self.log_viewport.update(wheel).map(Msg::ViewportMsg),
                }
            }

            Msg::PipelineDone => Command::quit(),

            Msg::Quit => Command::quit(),
        }
    }

    fn view(&self, frame: &mut Frame) {
        let area = frame.area();

        let rows = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .split(area);

        self.render_title_bar(frame, rows[0]);
        self.render_main(frame, rows[1]);
        self.render_status_bar(frame, rows[2]);
    }

    fn subscriptions(&self) -> Vec<Subscription<Msg>> {
        let mut subs = vec![terminal_events(|event| match event {
            TerminalEvent::Key(key) => Some(Msg::KeyPress(key)),
            TerminalEvent::Resize(w, h) => Some(Msg::Resize(w, h)),
            TerminalEvent::Mouse(m) => {
                use boba::crossterm::event::MouseEventKind;
                match m.kind {
                    MouseEventKind::ScrollUp => Some(Msg::MouseScroll { up: true }),
                    MouseEventKind::ScrollDown => Some(Msg::MouseScroll { up: false }),
                    _ => None,
                }
            }
            _ => None,
        })];

        if !self.done {
            subs.extend(
                self.spinner
                    .subscriptions()
                    .into_iter()
                    .map(|s| s.map(Msg::SpinnerMsg)),
            );
            subs.extend(
                self.stopwatch
                    .subscriptions()
                    .into_iter()
                    .map(|s| s.map(Msg::StopwatchMsg)),
            );
        }

        subs.extend(
            self.log_viewport
                .subscriptions()
                .into_iter()
                .map(|s| s.map(Msg::ViewportMsg)),
        );

        subs.extend(
            self.console_viewport
                .subscriptions()
                .into_iter()
                .map(|s| s.map(Msg::ConsoleViewportMsg)),
        );

        subs
    }
}

impl PipelineTui {
    fn handle_pipeline_event(&mut self, event: PipelineEvent) -> Command<Msg> {
        match &event {
            PipelineEvent::PipelineStarted {
                graph_name,
                timestamp,
            } => {
                self.status = PipelineStatus::Running;
                self.push_both(LogEntry {
                    timestamp: *timestamp,
                    node_id: None,
                    kind: LogKind::PipelineStart,
                    content: format!("Pipeline started: {graph_name}"),
                });
                Command::none()
            }

            PipelineEvent::PipelineCompleted {
                duration_ms,
                timestamp,
                ..
            } => {
                if self.status != PipelineStatus::Failed {
                    self.status = PipelineStatus::Completed;
                }
                self.spinner.stop();
                self.stopwatch.stop();
                self.done = true;
                self.push_both(LogEntry {
                    timestamp: *timestamp,
                    node_id: None,
                    kind: LogKind::PipelineEnd,
                    content: format!("Pipeline completed ({duration_ms}ms)"),
                });
                // Auto-quit after a brief pause so the user can see the final state.
                Command::tick(Duration::from_secs(2), |_| Msg::Quit)
            }

            PipelineEvent::PipelineAborted { reason, timestamp } => {
                self.status = PipelineStatus::Aborted;
                self.spinner.stop();
                self.stopwatch.stop();
                self.done = true;
                self.push_both(LogEntry {
                    timestamp: *timestamp,
                    node_id: None,
                    kind: LogKind::PipelineEnd,
                    content: format!("Pipeline aborted: {reason}"),
                });
                // Auto-quit quickly on abort — something went wrong.
                Command::tick(Duration::from_secs(1), |_| Msg::Quit)
            }

            PipelineEvent::NodeStarted {
                node_id, timestamp, ..
            } => {
                if let Some(&idx) = self.node_index.get(node_id) {
                    self.nodes[idx].status = NodeStatus::Running;
                }
                self.push_both(LogEntry {
                    timestamp: *timestamp,
                    node_id: Some(node_id.clone()),
                    kind: LogKind::NodeStart,
                    content: format!("Node '{node_id}' started"),
                });
                Command::none()
            }

            PipelineEvent::NodeCompleted {
                node_id,
                duration_ms,
                timestamp,
                ..
            } => {
                if let Some(&idx) = self.node_index.get(node_id) {
                    self.nodes[idx].status = NodeStatus::Completed;
                }
                self.finished_node_count += 1;
                self.push_both(LogEntry {
                    timestamp: *timestamp,
                    node_id: Some(node_id.clone()),
                    kind: LogKind::NodeComplete,
                    content: format!("Node '{node_id}' completed ({duration_ms}ms)"),
                });
                // Auto-select next running node if there is one
                if let Some(next) = self
                    .nodes
                    .iter()
                    .position(|n| n.status == NodeStatus::Running)
                {
                    self.selected_node = next;
                }
                Command::none()
            }

            PipelineEvent::NodeFailed {
                node_id,
                error,
                timestamp,
                ..
            } => {
                if let Some(&idx) = self.node_index.get(node_id) {
                    self.nodes[idx].status = NodeStatus::Failed;
                }
                self.finished_node_count += 1;
                self.status = PipelineStatus::Failed;
                self.push_both(LogEntry {
                    timestamp: *timestamp,
                    node_id: Some(node_id.clone()),
                    kind: LogKind::NodeFail,
                    content: format!("Node '{node_id}' failed: {error}"),
                });
                Command::none()
            }

            PipelineEvent::EdgeTraversed {
                from,
                to,
                timestamp,
                ..
            } => {
                self.push_both(LogEntry {
                    timestamp: *timestamp,
                    node_id: None,
                    kind: LogKind::EdgeTraversal,
                    content: format!("{from} -> {to}"),
                });
                Command::none()
            }

            PipelineEvent::AgentTurnStarted {
                node_id,
                turn_number,
                timestamp,
            } => {
                self.push_both(LogEntry {
                    timestamp: *timestamp,
                    node_id: Some(node_id.clone()),
                    kind: LogKind::AgentTurn,
                    content: format!("Turn {turn_number}"),
                });
                Command::none()
            }

            PipelineEvent::AgentToolCallStarted {
                node_id,
                tool_name,
                input_preview,
                timestamp,
                ..
            } => {
                // Track files touched by write/edit tool calls.
                let is_file_tool = matches!(
                    tool_name.as_str(),
                    "Write" | "write_file" | "Edit" | "edit_file"
                );
                if is_file_tool && !input_preview.is_empty() {
                    self.files_touched.insert(input_preview.clone());
                }

                let content = if input_preview.is_empty() {
                    format!("  [tool] {tool_name}...")
                } else {
                    format!("  [tool] {tool_name} {input_preview}...")
                };
                self.push_both(LogEntry {
                    timestamp: *timestamp,
                    node_id: Some(node_id.clone()),
                    kind: LogKind::ToolCallStart,
                    content,
                });
                Command::none()
            }

            PipelineEvent::AgentToolCallCompleted {
                node_id,
                tool_name,
                duration_ms,
                is_error,
                result_preview,
                timestamp,
                ..
            } => {
                let content = if *is_error && !result_preview.is_empty() {
                    let preview = if result_preview.chars().count() > 60 {
                        let t: String = result_preview.chars().take(60).collect();
                        format!("{t}…")
                    } else {
                        result_preview.clone()
                    };
                    format!("  [tool] {tool_name} ERR ({duration_ms}ms) \"{preview}\"")
                } else {
                    let status_str = if *is_error { "ERR" } else { "ok" };
                    format!("  [tool] {tool_name} {status_str} ({duration_ms}ms)")
                };
                self.push_both(LogEntry {
                    timestamp: *timestamp,
                    node_id: Some(node_id.clone()),
                    kind: LogKind::ToolCallComplete,
                    content,
                });
                Command::none()
            }

            PipelineEvent::AgentTokenUsage {
                input_tokens,
                output_tokens,
                cost_usd,
                ..
            } => {
                self.total_input_tokens += input_tokens;
                self.total_output_tokens += output_tokens;
                if *cost_usd > 0.0 {
                    self.total_cost_usd += cost_usd;
                }
                Command::none()
            }

            PipelineEvent::AgentMessage {
                node_id,
                text,
                timestamp,
            } => {
                let truncated = if text.chars().count() > 120 {
                    let t: String = text.chars().take(120).collect();
                    format!("{t}…")
                } else {
                    text.clone()
                };
                self.push_both(LogEntry {
                    timestamp: *timestamp,
                    node_id: Some(node_id.clone()),
                    kind: LogKind::AgentText,
                    content: format!("  [text] {truncated}"),
                });
                Command::none()
            }

            PipelineEvent::HumanPromptIssued {
                node_id,
                question,
                timestamp,
            } => {
                self.push_both(LogEntry {
                    timestamp: *timestamp,
                    node_id: Some(node_id.clone()),
                    kind: LogKind::HumanPrompt,
                    content: format!("  [?] {question}"),
                });
                Command::none()
            }

            PipelineEvent::HumanResponseReceived {
                node_id,
                response,
                timestamp,
            } => {
                self.push_both(LogEntry {
                    timestamp: *timestamp,
                    node_id: Some(node_id.clone()),
                    kind: LogKind::HumanResponse,
                    content: format!("  [>] {response}"),
                });
                Command::none()
            }

            PipelineEvent::LoopRestarted {
                from,
                to,
                restart_count,
                timestamp,
            } => {
                self.push_both(LogEntry {
                    timestamp: *timestamp,
                    node_id: None,
                    kind: LogKind::EdgeTraversal,
                    content: format!("Loop restarted: {from} -> {to} (attempt {restart_count})"),
                });
                Command::none()
            }

            // Low-verbosity events: silently ignore
            PipelineEvent::ContextUpdated { .. } | PipelineEvent::CheckpointCreated { .. } => {
                Command::none()
            }
        }
    }

    fn handle_key(&mut self, key: KeyEvent) -> Command<Msg> {
        // q or Esc quits
        if key.code == KeyCode::Char('q') && key.modifiers == KeyModifiers::NONE {
            return Command::quit();
        }
        if key.code == KeyCode::Esc {
            return Command::quit();
        }

        // Tab toggles focus between panels
        if key.code == KeyCode::Tab {
            self.toggle_focus();
            return Command::none();
        }

        match &self.focus {
            PanelFocus::Nodes => match key.code {
                KeyCode::Char('j') | KeyCode::Down => {
                    if self.selected_node + 1 < self.nodes.len() {
                        self.selected_node += 1;
                        self.refresh_viewport();
                    }
                    Command::none()
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    self.selected_node = self.selected_node.saturating_sub(1);
                    self.refresh_viewport();
                    Command::none()
                }
                KeyCode::Char('l') | KeyCode::Right => {
                    self.focus = PanelFocus::Logs;
                    self.log_viewport.focus();
                    Command::none()
                }
                KeyCode::Char('h') | KeyCode::Left => Command::none(),
                _ => Command::none(),
            },
            PanelFocus::Logs => match key.code {
                KeyCode::Char('h') | KeyCode::Left => {
                    self.focus = PanelFocus::Nodes;
                    self.log_viewport.blur();
                    Command::none()
                }
                _ => {
                    // Forward to viewport for vi-style scrolling (j/k/g/G/etc.)
                    self.log_viewport
                        .update(viewport::Message::KeyPress(key))
                        .map(Msg::ViewportMsg)
                }
            },
            PanelFocus::Console => match key.code {
                KeyCode::Char('h') | KeyCode::Left => {
                    self.focus = PanelFocus::Nodes;
                    self.console_viewport.blur();
                    Command::none()
                }
                _ => {
                    // Forward to console viewport for vi-style scrolling
                    self.console_viewport
                        .update(viewport::Message::KeyPress(key))
                        .map(Msg::ConsoleViewportMsg)
                }
            },
        }
    }

    fn toggle_focus(&mut self) {
        match self.focus {
            PanelFocus::Nodes => {
                self.focus = PanelFocus::Logs;
                self.log_viewport.focus();
            }
            PanelFocus::Logs => {
                self.focus = PanelFocus::Console;
                self.log_viewport.blur();
                self.console_viewport.focus();
            }
            PanelFocus::Console => {
                self.focus = PanelFocus::Nodes;
                self.console_viewport.blur();
            }
        }
    }

    /// Append a log entry and refresh the per-node viewport content.
    #[allow(dead_code)] // Used directly in tests; production code uses push_both.
    fn push_log(&mut self, entry: LogEntry) {
        self.log_entries.push(entry);
        self.refresh_viewport();
    }

    /// Append to both the per-node log and the unified console.
    fn push_both(&mut self, entry: LogEntry) {
        self.console_entries.push(entry.clone());
        self.log_entries.push(entry);
        self.refresh_viewport();
        self.refresh_console_viewport();
    }

    /// Rebuild viewport content from current log entries, preserving scroll position.
    fn refresh_viewport(&mut self) {
        let was_at_bottom = self.log_viewport.at_bottom();
        let old_offset = self.log_viewport.y_offset();
        let lines = self.render_log_lines();
        self.log_viewport.set_styled_content(lines);
        if was_at_bottom {
            self.log_viewport.goto_bottom();
        } else {
            self.log_viewport.set_y_offset(old_offset);
        }
    }

    /// Rebuild console viewport content from all console entries, preserving scroll position.
    fn refresh_console_viewport(&mut self) {
        let was_at_bottom = self.console_viewport.at_bottom();
        let old_offset = self.console_viewport.y_offset();
        let lines = self.render_console_lines();
        self.console_viewport.set_styled_content(lines);
        if was_at_bottom {
            self.console_viewport.goto_bottom();
        } else {
            self.console_viewport.set_y_offset(old_offset);
        }
    }

    /// Build styled log lines for the selected node (or all global entries when empty).
    fn render_log_lines(&self) -> Vec<Line<'static>> {
        let selected_id = self.nodes.get(self.selected_node).map(|n| n.id.as_str());

        self.log_entries
            .iter()
            .filter(|entry| match selected_id {
                Some(id) => {
                    // Show global entries (no node_id) and entries for the selected node
                    entry.node_id.is_none() || entry.node_id.as_deref() == Some(id)
                }
                None => true,
            })
            .map(|entry| {
                let style = style_for_log_entry(entry);
                Line::from(vec![Span::styled(entry.content.clone(), style)])
            })
            .collect()
    }

    /// Build styled lines for the unified console panel — all agents, prefixed with timestamp and [node_id].
    fn render_console_lines(&self) -> Vec<Line<'static>> {
        self.console_entries
            .iter()
            .map(|entry| {
                let ts = entry.timestamp.format("%H:%M:%S");
                let time_span = Span::styled(
                    format!("{ts} "),
                    Style::default()
                        .fg(Color::DarkGray)
                        .add_modifier(Modifier::DIM),
                );
                let prefix = match &entry.node_id {
                    Some(id) => {
                        Span::styled(format!("[{id}] "), Style::default().fg(Color::DarkGray))
                    }
                    None => Span::styled(
                        "[*] ".to_string(),
                        Style::default()
                            .fg(Color::DarkGray)
                            .add_modifier(Modifier::DIM),
                    ),
                };
                let style = style_for_log_entry(entry);
                Line::from(vec![
                    time_span,
                    prefix,
                    Span::styled(entry.content.clone(), style),
                ])
            })
            .collect()
    }

    fn render_title_bar(&self, frame: &mut Frame, area: Rect) {
        let elapsed = self.stopwatch.elapsed();
        let total_secs = elapsed.as_secs();
        let mins = total_secs / 60;
        let secs = total_secs % 60;
        let tenths = (elapsed.subsec_millis() / 100) as u64;
        let elapsed_str = format!("{mins:02}:{secs:02}.{tenths}");

        // Center section: colored status label + frozen elapsed time when done
        let center_line = match &self.status {
            PipelineStatus::Completed => Line::from(vec![
                Span::styled(
                    "COMPLETED",
                    Style::default()
                        .fg(Color::Green)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(format!("  {elapsed_str}")),
            ]),
            PipelineStatus::Failed => Line::from(vec![
                Span::styled(
                    "FAILED",
                    Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                ),
                Span::raw(format!("  {elapsed_str}")),
            ]),
            PipelineStatus::Aborted => Line::from(vec![
                Span::styled(
                    "ABORTED",
                    Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                ),
                Span::raw(format!("  {elapsed_str}")),
            ]),
            _ => {
                let frame_char = frames::DOTS[self.spinner_frame_idx % frames::DOTS.len()];
                Line::from(format!("{frame_char}  {elapsed_str}"))
            }
        };

        let left_line = Line::from(Span::styled(
            format!(" SMASHER: {} ", self.pipeline_name),
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ));
        let total_tokens = self.total_input_tokens + self.total_output_tokens;
        let mut parts: Vec<String> = Vec::new();
        if self.total_cost_usd > 0.0 {
            parts.push(format_cost(self.total_cost_usd));
        }
        if total_tokens > 0 {
            parts.push(format!("{}tok", format_token_count(total_tokens)));
        }
        if !self.files_touched.is_empty() {
            parts.push(format!("{}f", self.files_touched.len()));
        }
        parts.push(format!(
            "{}/{} nodes",
            self.finished_node_count,
            self.nodes.len()
        ));
        let right_text = format!("{} ", parts.join("  "));

        StatusBar::new()
            .left(left_line)
            .center(center_line)
            .right(right_text)
            .style(Style::default().bg(Color::DarkGray).fg(Color::White))
            .render(frame, area);
    }

    fn render_main(&self, frame: &mut Frame, area: Rect) {
        let rows =
            Layout::vertical([Constraint::Percentage(60), Constraint::Percentage(40)]).split(area);

        let cols = Layout::horizontal([Constraint::Percentage(30), Constraint::Percentage(70)])
            .split(rows[0]);

        self.render_nodes_panel(frame, cols[0]);
        self.render_log_panel(frame, cols[1]);
        self.render_console_panel(frame, rows[1]);
    }

    fn render_nodes_panel(&self, frame: &mut Frame, area: Rect) {
        let border_style = if self.focus == PanelFocus::Nodes {
            Style::default().fg(Color::White)
        } else {
            Style::default().fg(Color::DarkGray)
        };

        let block = Block::default()
            .borders(Borders::ALL)
            .title(" Nodes ")
            .border_style(border_style);

        let items: Vec<ListItem> = self
            .nodes
            .iter()
            .enumerate()
            .map(|(i, node)| {
                // Running nodes show the current spinner frame in green
                let icon_span = match node.status {
                    NodeStatus::Pending => Span::styled(
                        "○ ",
                        Style::default()
                            .fg(Color::DarkGray)
                            .add_modifier(Modifier::DIM),
                    ),
                    NodeStatus::Running => {
                        let frame_char = frames::DOTS[self.spinner_frame_idx % frames::DOTS.len()];
                        Span::styled(format!("{frame_char} "), Style::default().fg(Color::Green))
                    }
                    NodeStatus::Completed => Span::styled("✓ ", Style::default().fg(Color::Green)),
                    NodeStatus::Failed => Span::styled("✗ ", Style::default().fg(Color::Red)),
                };
                // Selected row gets a contrasting background highlight
                let label_style = if i == self.selected_node {
                    Style::default()
                        .bg(Color::DarkGray)
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                };
                ListItem::new(Line::from(vec![
                    icon_span,
                    Span::styled(node.label.clone(), label_style),
                ]))
            })
            .collect();

        let list = List::new(items).block(block);
        frame.render_widget(list, area);
    }

    fn render_log_panel(&self, frame: &mut Frame, area: Rect) {
        let border_style = if self.focus == PanelFocus::Logs {
            Style::default().fg(Color::White)
        } else {
            Style::default().fg(Color::DarkGray)
        };

        let block = Block::default()
            .borders(Borders::ALL)
            .title(" Output ")
            .border_style(border_style);

        let inner = block.inner(area);
        frame.render_widget(block, area);
        self.log_viewport.view(frame, inner);
    }

    fn render_console_panel(&self, frame: &mut Frame, area: Rect) {
        let border_style = if self.focus == PanelFocus::Console {
            Style::default().fg(Color::White)
        } else {
            Style::default().fg(Color::DarkGray)
        };

        let block = Block::default()
            .borders(Borders::ALL)
            .title(" Console (all agents) ")
            .border_style(border_style);

        let inner = block.inner(area);
        frame.render_widget(block, area);
        self.console_viewport.view(frame, inner);
    }

    fn render_status_bar(&self, frame: &mut Frame, area: Rect) {
        let status_text = match &self.status {
            PipelineStatus::Starting => " starting ",
            PipelineStatus::Running => " running ",
            PipelineStatus::Completed => " completed ",
            PipelineStatus::Failed => " FAILED ",
            PipelineStatus::Aborted => " ABORTED ",
        };

        StatusBar::new()
            .left(status_text)
            .right(" q quit | j/k nav | h nodes | tab cycle | g/G top/bottom ")
            .style(Style::default().bg(Color::DarkGray))
            .render(frame, area);
    }
}

/// Format a USD cost for compact display: "$0.02", "$1.23", "$12.3".
fn format_cost(usd: f64) -> String {
    if usd >= 10.0 {
        format!("${:.1}", usd)
    } else {
        format!("${:.2}", usd)
    }
}

/// Format a token count for compact display: "1.2k", "45.3k", "1.2M".
fn format_token_count(tokens: u64) -> String {
    if tokens >= 1_000_000 {
        format!("{:.1}M", tokens as f64 / 1_000_000.0)
    } else if tokens >= 1_000 {
        format!("{:.1}k", tokens as f64 / 1_000.0)
    } else {
        tokens.to_string()
    }
}

/// Map a log entry's kind to a ratatui Style for consistent coloring.
fn style_for_log_entry(entry: &LogEntry) -> Style {
    match entry.kind {
        LogKind::PipelineStart | LogKind::PipelineEnd => Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
        LogKind::NodeStart | LogKind::NodeComplete | LogKind::NodeFail => {
            Style::default().fg(Color::Yellow)
        }
        LogKind::EdgeTraversal => Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::DIM),
        LogKind::AgentTurn => Style::default().fg(Color::Cyan),
        LogKind::ToolCallStart => Style::default().fg(Color::Cyan),
        LogKind::ToolCallComplete => {
            if entry.content.contains("ERR") {
                Style::default().fg(Color::Red)
            } else {
                Style::default().fg(Color::Green)
            }
        }
        LogKind::AgentText => Style::default().fg(Color::White),
        LogKind::HumanPrompt => Style::default().fg(Color::Magenta),
        LogKind::HumanResponse => Style::default().fg(Color::Magenta),
        LogKind::Info => Style::default().fg(Color::Gray),
    }
}

/// Build a boba Program for the pipeline TUI, rendering to stderr.
///
/// Renders to stderr so that stdout remains clean for pipeline JSON output.
pub fn build_program(flags: TuiFlags) -> Result<Program<PipelineTui>, ProgramError> {
    let opts = ProgramOptions {
        output: OutputTarget::Stderr,
        alt_screen: true,
        title: Some(format!("SMASHER: {}", flags.pipeline_name)),
        ..ProgramOptions::default()
    };
    Program::<PipelineTui>::with_options(flags, opts)
}

#[cfg(test)]
mod tests {
    use super::*;
    use boba::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use chrono::Utc;
    use smasher_attractor::graph::{Graph, GraphNode, NodeType};

    fn make_graph(node_ids: &[&str]) -> Graph {
        Graph {
            name: Some("test".into()),
            nodes: node_ids
                .iter()
                .map(|id| GraphNode {
                    id: id.to_string(),
                    node_type: NodeType::Codergen,
                    label: Some(id.to_string()),
                    attrs: Default::default(),
                })
                .collect(),
            edges: vec![],
            default_node_attrs: Default::default(),
            default_edge_attrs: Default::default(),
        }
    }

    fn make_flags(pipeline_name: &str, node_ids: &[&str]) -> TuiFlags {
        TuiFlags {
            graph: make_graph(node_ids),
            run_id: "test-run".into(),
            pipeline_name: pipeline_name.into(),
            completed_node_ids: Vec::new(),
        }
    }

    #[test]
    fn init_creates_pending_nodes() {
        let (model, _cmd) = PipelineTui::init(make_flags("test", &["a", "b", "c"]));
        assert_eq!(model.nodes.len(), 3);
        assert!(model.nodes.iter().all(|n| n.status == NodeStatus::Pending));
        assert_eq!(model.pipeline_name, "test");
        assert_eq!(model.finished_node_count, 0);
        assert_eq!(model.status, PipelineStatus::Starting);
    }

    #[test]
    fn node_started_sets_running() {
        let (mut model, _) = PipelineTui::init(make_flags("test", &["node_a"]));
        model.handle_pipeline_event(PipelineEvent::NodeStarted {
            node_id: "node_a".into(),
            node_type: "codergen".into(),
            timestamp: Utc::now(),
        });
        assert_eq!(model.nodes[0].status, NodeStatus::Running);
        assert!(!model.log_entries.is_empty());
    }

    #[test]
    fn node_completed_increments_finished_count() {
        let (mut model, _) = PipelineTui::init(make_flags("test", &["node_a"]));
        model.handle_pipeline_event(PipelineEvent::NodeCompleted {
            node_id: "node_a".into(),
            outcome: smasher_attractor::state::Outcome::success(),
            duration_ms: 100,
            timestamp: Utc::now(),
        });
        assert_eq!(model.nodes[0].status, NodeStatus::Completed);
        assert_eq!(model.finished_node_count, 1);
    }

    #[test]
    fn node_failed_sets_pipeline_failed() {
        let (mut model, _) = PipelineTui::init(make_flags("test", &["node_a"]));
        model.handle_pipeline_event(PipelineEvent::NodeFailed {
            node_id: "node_a".into(),
            error: "something went wrong".into(),
            duration_ms: 50,
            timestamp: Utc::now(),
        });
        assert_eq!(model.nodes[0].status, NodeStatus::Failed);
        assert_eq!(model.status, PipelineStatus::Failed);
        assert_eq!(model.finished_node_count, 1);
    }

    #[test]
    fn pipeline_completed_stops_spinner_stopwatch_sets_done() {
        let (mut model, _) = PipelineTui::init(make_flags("test", &["node_a"]));
        model.handle_pipeline_event(PipelineEvent::PipelineCompleted {
            outcome: smasher_attractor::state::Outcome::success(),
            total_nodes: 1,
            duration_ms: 500,
            timestamp: Utc::now(),
        });
        assert!(model.done);
        assert!(!model.spinner.is_spinning());
        assert!(!model.stopwatch.running());
        assert_eq!(model.status, PipelineStatus::Completed);
    }

    #[test]
    fn pipeline_aborted_sets_aborted_status() {
        let (mut model, _) = PipelineTui::init(make_flags("test", &[]));
        model.handle_pipeline_event(PipelineEvent::PipelineAborted {
            reason: "user cancelled".into(),
            timestamp: Utc::now(),
        });
        assert!(model.done);
        assert_eq!(model.status, PipelineStatus::Aborted);
    }

    #[test]
    fn log_entries_appended_for_relevant_events() {
        let (mut model, _) = PipelineTui::init(make_flags("test", &["node_a"]));
        assert_eq!(model.log_entries.len(), 0);
        model.handle_pipeline_event(PipelineEvent::PipelineStarted {
            graph_name: "test".into(),
            timestamp: Utc::now(),
        });
        assert_eq!(model.log_entries.len(), 1);
        model.handle_pipeline_event(PipelineEvent::NodeStarted {
            node_id: "node_a".into(),
            node_type: "codergen".into(),
            timestamp: Utc::now(),
        });
        assert_eq!(model.log_entries.len(), 2);
    }

    #[test]
    fn context_and_checkpoint_events_are_silent() {
        let (mut model, _) = PipelineTui::init(make_flags("test", &["node_a"]));
        model.handle_pipeline_event(PipelineEvent::ContextUpdated {
            key: "some_key".into(),
            timestamp: Utc::now(),
        });
        model.handle_pipeline_event(PipelineEvent::CheckpointCreated {
            node_id: "node_a".into(),
            timestamp: Utc::now(),
        });
        // Neither event adds a log entry
        assert_eq!(model.log_entries.len(), 0);
    }

    #[test]
    fn panel_focus_toggles_via_toggle_focus() {
        let (mut model, _) = PipelineTui::init(make_flags("test", &["a"]));
        assert_eq!(model.focus, PanelFocus::Nodes);
        model.toggle_focus();
        assert_eq!(model.focus, PanelFocus::Logs);
        model.toggle_focus();
        assert_eq!(model.focus, PanelFocus::Console);
        model.toggle_focus();
        assert_eq!(model.focus, PanelFocus::Nodes);
    }

    #[test]
    fn node_navigation_stays_in_bounds() {
        let (mut model, _) = PipelineTui::init(make_flags("test", &["a", "b", "c"]));
        assert_eq!(model.selected_node, 0);
        // k at top stays at 0
        model.selected_node = model.selected_node.saturating_sub(1);
        assert_eq!(model.selected_node, 0);
        // j moves to 1
        model.selected_node += 1;
        assert_eq!(model.selected_node, 1);
        // Move to last
        model.selected_node = 2;
        // Would-be next beyond end
        let at_end = model.selected_node + 1 >= model.nodes.len();
        assert!(at_end);
    }

    #[test]
    fn render_log_lines_filters_by_selected_node() {
        let (mut model, _) = PipelineTui::init(make_flags("test", &["node_a", "node_b"]));

        model.push_log(LogEntry {
            timestamp: Utc::now(),
            node_id: Some("node_a".into()),
            kind: LogKind::NodeStart,
            content: "Node a started".into(),
        });
        model.push_log(LogEntry {
            timestamp: Utc::now(),
            node_id: Some("node_b".into()),
            kind: LogKind::NodeStart,
            content: "Node b started".into(),
        });
        model.push_log(LogEntry {
            timestamp: Utc::now(),
            node_id: None,
            kind: LogKind::PipelineStart,
            content: "Global entry".into(),
        });

        model.selected_node = 0; // selecting node_a
        let lines = model.render_log_lines();
        // Should show node_a entries + global entries, not node_b
        assert_eq!(lines.len(), 2); // node_a + global
    }

    #[test]
    fn key_j_moves_selection_down() {
        let (mut model, _) = PipelineTui::init(make_flags("test", &["a", "b", "c"]));
        assert_eq!(model.selected_node, 0);
        model.handle_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE));
        assert_eq!(model.selected_node, 1);
        model.handle_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE));
        assert_eq!(model.selected_node, 2);
        // At last node — j should not go past end
        model.handle_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE));
        assert_eq!(model.selected_node, 2);
    }

    #[test]
    fn key_k_moves_selection_up() {
        let (mut model, _) = PipelineTui::init(make_flags("test", &["a", "b", "c"]));
        model.selected_node = 2;
        model.handle_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE));
        assert_eq!(model.selected_node, 1);
        model.handle_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE));
        assert_eq!(model.selected_node, 0);
        // At first node — k should stay at 0
        model.handle_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE));
        assert_eq!(model.selected_node, 0);
    }

    #[test]
    fn key_q_returns_non_none_command() {
        let (mut model, _) = PipelineTui::init(make_flags("test", &["a"]));
        let cmd = model.handle_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));
        assert!(
            !cmd.is_none(),
            "q key should return a quit command, not Command::none()"
        );
    }

    #[test]
    fn key_tab_cycles_three_panel_focus() {
        let (mut model, _) = PipelineTui::init(make_flags("test", &["a"]));
        assert_eq!(model.focus, PanelFocus::Nodes);
        model.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(model.focus, PanelFocus::Logs);
        model.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(model.focus, PanelFocus::Console);
        model.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(model.focus, PanelFocus::Nodes);
    }

    #[test]
    fn agent_tool_call_started_adds_log_entry() {
        let (mut model, _) = PipelineTui::init(make_flags("test", &["node_a"]));
        let before = model.log_entries.len();
        model.handle_pipeline_event(PipelineEvent::AgentToolCallStarted {
            node_id: "node_a".into(),
            tool_name: "bash".into(),
            tool_call_id: "call_001".into(),
            input_preview: "cargo test".into(),
            timestamp: Utc::now(),
        });
        assert_eq!(model.log_entries.len(), before + 1);
        let entry = model.log_entries.last().unwrap();
        assert_eq!(entry.node_id.as_deref(), Some("node_a"));
        assert!(entry.content.contains("bash"));
        assert!(entry.content.contains("cargo test"));
    }

    #[test]
    fn agent_message_adds_log_entry() {
        let (mut model, _) = PipelineTui::init(make_flags("test", &["node_a"]));
        let before = model.log_entries.len();
        model.handle_pipeline_event(PipelineEvent::AgentMessage {
            node_id: "node_a".into(),
            text: "I'll implement this now".into(),
            timestamp: Utc::now(),
        });
        assert_eq!(model.log_entries.len(), before + 1);
        let entry = model.log_entries.last().unwrap();
        assert_eq!(entry.node_id.as_deref(), Some("node_a"));
        assert!(entry.content.contains("implement"));
    }

    #[test]
    fn log_entries_scoped_to_selected_node() {
        let (mut model, _) = PipelineTui::init(make_flags("test", &["node_a", "node_b"]));
        model.handle_pipeline_event(PipelineEvent::AgentMessage {
            node_id: "node_a".into(),
            text: "alpha".into(),
            timestamp: Utc::now(),
        });
        model.handle_pipeline_event(PipelineEvent::AgentMessage {
            node_id: "node_b".into(),
            text: "beta".into(),
            timestamp: Utc::now(),
        });

        model.selected_node = 0;
        let lines_a = model.render_log_lines();
        assert_eq!(lines_a.len(), 1);
        assert!(lines_a[0].to_string().contains("alpha"));

        model.selected_node = 1;
        let lines_b = model.render_log_lines();
        assert_eq!(lines_b.len(), 1);
        assert!(lines_b[0].to_string().contains("beta"));
    }

    #[test]
    fn console_entries_populated_by_pipeline_events() {
        let (mut model, _) = PipelineTui::init(make_flags("test", &["node_a", "node_b"]));
        assert_eq!(model.console_entries.len(), 0);

        model.handle_pipeline_event(PipelineEvent::AgentMessage {
            node_id: "node_a".into(),
            text: "working on alpha".into(),
            timestamp: Utc::now(),
        });
        model.handle_pipeline_event(PipelineEvent::AgentToolCallStarted {
            node_id: "node_b".into(),
            tool_name: "bash".into(),
            tool_call_id: "call_001".into(),
            input_preview: String::new(),
            timestamp: Utc::now(),
        });

        // Both events should appear in console (unified stream)
        assert_eq!(model.console_entries.len(), 2);
        assert_eq!(model.console_entries[0].node_id.as_deref(), Some("node_a"));
        assert_eq!(model.console_entries[1].node_id.as_deref(), Some("node_b"));
    }

    #[test]
    fn console_lines_show_all_nodes_with_prefix() {
        let (mut model, _) = PipelineTui::init(make_flags("test", &["node_a", "node_b"]));

        model.handle_pipeline_event(PipelineEvent::AgentMessage {
            node_id: "node_a".into(),
            text: "alpha work".into(),
            timestamp: Utc::now(),
        });
        model.handle_pipeline_event(PipelineEvent::AgentMessage {
            node_id: "node_b".into(),
            text: "beta work".into(),
            timestamp: Utc::now(),
        });

        let lines = model.render_console_lines();
        assert_eq!(lines.len(), 2);
        // Console lines should contain both node_a and node_b entries (unified)
        let text_0 = lines[0].to_string();
        let text_1 = lines[1].to_string();
        assert!(
            text_0.contains("node_a"),
            "expected node_a prefix, got: {text_0}"
        );
        assert!(
            text_1.contains("node_b"),
            "expected node_b prefix, got: {text_1}"
        );
    }

    #[test]
    fn tool_call_started_shows_empty_preview_without_extra_space() {
        let (mut model, _) = PipelineTui::init(make_flags("test", &["node_a"]));
        model.handle_pipeline_event(PipelineEvent::AgentToolCallStarted {
            node_id: "node_a".into(),
            tool_name: "bash".into(),
            tool_call_id: "call_001".into(),
            input_preview: String::new(),
            timestamp: Utc::now(),
        });
        let entry = model.log_entries.last().unwrap();
        assert_eq!(entry.content, "  [tool] bash...");
    }

    #[test]
    fn tool_call_completed_shows_error_preview() {
        let (mut model, _) = PipelineTui::init(make_flags("test", &["node_a"]));
        model.handle_pipeline_event(PipelineEvent::AgentToolCallCompleted {
            node_id: "node_a".into(),
            tool_name: "bash".into(),
            tool_call_id: "call_001".into(),
            duration_ms: 500,
            is_error: true,
            result_preview: "command not found: ffmpeg".into(),
            timestamp: Utc::now(),
        });
        let entry = model.log_entries.last().unwrap();
        assert!(entry.content.contains("ERR"));
        assert!(entry.content.contains("command not found: ffmpeg"));
    }

    #[test]
    fn tool_call_completed_ok_does_not_show_preview() {
        let (mut model, _) = PipelineTui::init(make_flags("test", &["node_a"]));
        model.handle_pipeline_event(PipelineEvent::AgentToolCallCompleted {
            node_id: "node_a".into(),
            tool_name: "Read".into(),
            tool_call_id: "call_001".into(),
            duration_ms: 12,
            is_error: false,
            result_preview: "file contents here".into(),
            timestamp: Utc::now(),
        });
        let entry = model.log_entries.last().unwrap();
        assert_eq!(entry.content, "  [tool] Read ok (12ms)");
    }

    #[test]
    fn token_usage_accumulates() {
        let (mut model, _) = PipelineTui::init(make_flags("test", &["node_a", "node_b"]));
        assert_eq!(model.total_input_tokens, 0);
        assert_eq!(model.total_output_tokens, 0);

        model.handle_pipeline_event(PipelineEvent::AgentTokenUsage {
            node_id: "node_a".into(),
            input_tokens: 1000,
            output_tokens: 500,
            cost_usd: 0.03,
            timestamp: Utc::now(),
        });
        assert_eq!(model.total_input_tokens, 1000);
        assert_eq!(model.total_output_tokens, 500);
        assert!((model.total_cost_usd - 0.03).abs() < f64::EPSILON);

        model.handle_pipeline_event(PipelineEvent::AgentTokenUsage {
            node_id: "node_b".into(),
            input_tokens: 2000,
            output_tokens: 800,
            cost_usd: 0.05,
            timestamp: Utc::now(),
        });
        assert_eq!(model.total_input_tokens, 3000);
        assert_eq!(model.total_output_tokens, 1300);
    }

    #[test]
    fn format_token_count_formats_correctly() {
        assert_eq!(format_token_count(500), "500");
        assert_eq!(format_token_count(1500), "1.5k");
        assert_eq!(format_token_count(45300), "45.3k");
        assert_eq!(format_token_count(1_200_000), "1.2M");
    }

    #[test]
    fn focus_toggle_cycles_nodes_logs_console() {
        let (mut model, _) = PipelineTui::init(make_flags("test", &["a"]));
        assert_eq!(model.focus, PanelFocus::Nodes);
        model.toggle_focus();
        assert_eq!(model.focus, PanelFocus::Logs);
        model.toggle_focus();
        assert_eq!(model.focus, PanelFocus::Console);
        model.toggle_focus();
        assert_eq!(model.focus, PanelFocus::Nodes);
    }

    #[test]
    fn resumed_nodes_marked_completed_on_init() {
        let flags = TuiFlags {
            graph: make_graph(&["a", "b", "c", "d"]),
            run_id: "test-run".into(),
            pipeline_name: "test".into(),
            completed_node_ids: vec!["a".into(), "b".into()],
        };
        let (model, _) = PipelineTui::init(flags);
        assert_eq!(model.nodes[0].status, NodeStatus::Completed); // a
        assert_eq!(model.nodes[1].status, NodeStatus::Completed); // b
        assert_eq!(model.nodes[2].status, NodeStatus::Pending); // c
        assert_eq!(model.nodes[3].status, NodeStatus::Pending); // d
        assert_eq!(model.finished_node_count, 2);
    }

    #[test]
    fn format_cost_formats_correctly() {
        assert_eq!(format_cost(0.01), "$0.01");
        assert_eq!(format_cost(0.123), "$0.12");
        assert_eq!(format_cost(1.5), "$1.50");
        assert_eq!(format_cost(9.99), "$9.99");
        assert_eq!(format_cost(10.0), "$10.0");
        assert_eq!(format_cost(42.567), "$42.6");
    }

    #[test]
    fn file_tracking_from_write_tool_calls() {
        let (mut model, _) = PipelineTui::init(make_flags("test", &["node_a"]));
        assert!(model.files_touched.is_empty());

        // Write tool should track file
        model.handle_pipeline_event(PipelineEvent::AgentToolCallStarted {
            node_id: "node_a".into(),
            tool_name: "Write".into(),
            tool_call_id: "call_001".into(),
            input_preview: "src/lib.rs".into(),
            timestamp: Utc::now(),
        });
        assert_eq!(model.files_touched.len(), 1);
        assert!(model.files_touched.contains("src/lib.rs"));

        // Edit tool should also track
        model.handle_pipeline_event(PipelineEvent::AgentToolCallStarted {
            node_id: "node_a".into(),
            tool_name: "edit_file".into(),
            tool_call_id: "call_002".into(),
            input_preview: "src/main.rs".into(),
            timestamp: Utc::now(),
        });
        assert_eq!(model.files_touched.len(), 2);

        // Same file again should not increase count (HashSet)
        model.handle_pipeline_event(PipelineEvent::AgentToolCallStarted {
            node_id: "node_a".into(),
            tool_name: "Write".into(),
            tool_call_id: "call_003".into(),
            input_preview: "src/lib.rs".into(),
            timestamp: Utc::now(),
        });
        assert_eq!(model.files_touched.len(), 2);

        // Non-file tools should not track
        model.handle_pipeline_event(PipelineEvent::AgentToolCallStarted {
            node_id: "node_a".into(),
            tool_name: "bash".into(),
            tool_call_id: "call_004".into(),
            input_preview: "cargo test".into(),
            timestamp: Utc::now(),
        });
        assert_eq!(model.files_touched.len(), 2);
    }

    #[test]
    fn file_tracking_ignores_empty_preview() {
        let (mut model, _) = PipelineTui::init(make_flags("test", &["node_a"]));

        // Write with empty preview should not track
        model.handle_pipeline_event(PipelineEvent::AgentToolCallStarted {
            node_id: "node_a".into(),
            tool_name: "Write".into(),
            tool_call_id: "call_001".into(),
            input_preview: String::new(),
            timestamp: Utc::now(),
        });
        assert!(model.files_touched.is_empty());
    }

    #[test]
    fn cost_accumulates_from_token_usage_events() {
        let (mut model, _) = PipelineTui::init(make_flags("test", &["node_a"]));
        assert!((model.total_cost_usd).abs() < f64::EPSILON);

        // Cost-only event (from result line: 0 tokens, nonzero cost)
        model.handle_pipeline_event(PipelineEvent::AgentTokenUsage {
            node_id: "node_a".into(),
            input_tokens: 0,
            output_tokens: 0,
            cost_usd: 0.05,
            timestamp: Utc::now(),
        });
        assert!((model.total_cost_usd - 0.05).abs() < f64::EPSILON);
        // Zero tokens should not change token totals
        assert_eq!(model.total_input_tokens, 0);
        assert_eq!(model.total_output_tokens, 0);

        // Mixed event with both tokens and cost
        model.handle_pipeline_event(PipelineEvent::AgentTokenUsage {
            node_id: "node_a".into(),
            input_tokens: 500,
            output_tokens: 200,
            cost_usd: 0.10,
            timestamp: Utc::now(),
        });
        assert!((model.total_cost_usd - 0.15).abs() < f64::EPSILON);
        assert_eq!(model.total_input_tokens, 500);
        assert_eq!(model.total_output_tokens, 200);

        // Zero cost should not change cost total
        model.handle_pipeline_event(PipelineEvent::AgentTokenUsage {
            node_id: "node_a".into(),
            input_tokens: 100,
            output_tokens: 50,
            cost_usd: 0.0,
            timestamp: Utc::now(),
        });
        assert!((model.total_cost_usd - 0.15).abs() < f64::EPSILON);
        assert_eq!(model.total_input_tokens, 600);
    }

    #[test]
    fn console_lines_include_timestamps() {
        let (mut model, _) = PipelineTui::init(make_flags("test", &["node_a"]));

        let ts = Utc::now();
        model.handle_pipeline_event(PipelineEvent::AgentMessage {
            node_id: "node_a".into(),
            text: "hello".into(),
            timestamp: ts,
        });

        let lines = model.render_console_lines();
        assert_eq!(lines.len(), 1);
        let rendered = lines[0].to_string();
        // Should contain HH:MM:SS timestamp format
        let expected_ts = ts.format("%H:%M:%S").to_string();
        assert!(
            rendered.contains(&expected_ts),
            "expected timestamp {expected_ts} in rendered line: {rendered}"
        );
    }
}
