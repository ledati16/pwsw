//! Dashboard screen - Overview and quick actions

use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

use crate::config::Config;
use crate::style::colors;
use crate::tui::widgets::truncate_node_name;

/// Patterns to highlight in log messages (keyword, color, bold)
const HIGHLIGHT_PATTERNS: &[(&str, Color, bool)] = &[
    // Important events (bold)
    ("Rule matched:", colors::LOG_EVENT, true),
    ("Switching:", colors::LOG_EVENT, true),
    ("Window opened:", colors::LOG_EVENT, true),
    ("Window closed:", colors::LOG_EVENT_CLOSE, true),
    ("Tracked window closed:", colors::LOG_EVENT_CLOSE, true),
    ("Set default sink:", colors::LOG_EVENT, true),
    // Daemon lifecycle events
    ("Starting PWSW daemon", colors::LOG_EVENT, true),
    ("Monitoring window events", colors::LOG_EVENT, true),
    ("Compositor event thread started", colors::UI_SUCCESS, false),
    ("IPC server listening on", colors::UI_SUCCESS, false),
    ("Loaded", colors::UI_SUCCESS, false),
    ("sinks,", colors::UI_HIGHLIGHT, false),
    ("rules", colors::UI_HIGHLIGHT, false),
    // Shutdown and warnings
    ("Shutting down", colors::UI_WARNING, false),
    ("Shutdown requested", colors::UI_WARNING, false),
    ("Config file changed", colors::UI_WARNING, false),
    // Field labels (not bold, just markers)
    ("app_id=", colors::LOG_KEYWORD, false),
    ("title=", colors::LOG_KEYWORD, false),
    ("id=", colors::LOG_KEYWORD, false),
];

/// Maximum display width for truncating `app_id` in window tracking display
const WINDOW_APPID_MAX_WIDTH: usize = 15;

/// Maximum display width for truncating window title in window tracking display
const WINDOW_TITLE_MAX_WIDTH: usize = 20;

/// Dashboard view mode (toggle between Logs and Windows)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DashboardView {
    Logs,
    Windows,
}

/// Dashboard screen state
pub(crate) struct DashboardScreen {
    pub selected_action: usize,   // 0-4: start, stop, restart, enable, disable
    pub log_scroll_offset: usize, // Lines scrolled back from the end (0 = showing latest)
    pub window_scroll_offset: usize, // Window list scroll offset
    pub current_view: DashboardView, // Toggle between Logs and Windows
    pub max_action_index: usize,  // Maximum action index (2 for direct, 4 for systemd)
    pub service_enabled: Option<bool>, // None for direct mode, Some(true/false) for systemd
}

impl DashboardScreen {
    pub(crate) fn new() -> Self {
        Self {
            selected_action: 0,
            log_scroll_offset: 0,
            window_scroll_offset: 0,
            current_view: DashboardView::Logs, // Default to logs
            max_action_index: 2,               // Default to 3 actions (start/stop/restart)
            service_enabled: None,             // Updated by background worker
        }
    }

    /// Update max action index based on daemon manager type
    pub(crate) fn set_max_actions(&mut self, is_systemd: bool) {
        self.max_action_index = if is_systemd { 4 } else { 2 };
        // Clamp current selection if it's out of range
        if self.selected_action > self.max_action_index {
            self.selected_action = 0;
        }
    }

    pub(crate) fn select_next(&mut self) {
        if self.selected_action < self.max_action_index {
            self.selected_action += 1;
        } else {
            self.selected_action = 0; // Wrap to first
        }
    }

    pub(crate) fn select_previous(&mut self) {
        if self.selected_action > 0 {
            self.selected_action -= 1;
        } else {
            self.selected_action = self.max_action_index; // Wrap to last
        }
    }

    /// Toggle between logs and windows view
    pub(crate) fn toggle_view(&mut self) {
        self.current_view = match self.current_view {
            DashboardView::Logs => DashboardView::Windows,
            DashboardView::Windows => DashboardView::Logs,
        };
    }

    /// Scroll logs up (show older logs)
    pub(crate) fn scroll_logs_up(&mut self, total_lines: usize, visible_lines: usize) {
        let max_offset = total_lines.saturating_sub(visible_lines);
        self.log_scroll_offset = (self.log_scroll_offset + 1).min(max_offset);
    }

    /// Scroll logs down (show newer logs)
    pub(crate) fn scroll_logs_down(&mut self) {
        self.log_scroll_offset = self.log_scroll_offset.saturating_sub(1);
    }

    /// Scroll logs up by page
    pub(crate) fn scroll_logs_page_up(&mut self, total_lines: usize, visible_lines: usize) {
        let max_offset = total_lines.saturating_sub(visible_lines);
        self.log_scroll_offset = (self.log_scroll_offset + visible_lines).min(max_offset);
    }

    /// Scroll logs down by page
    pub(crate) fn scroll_logs_page_down(&mut self, visible_lines: usize) {
        self.log_scroll_offset = self.log_scroll_offset.saturating_sub(visible_lines);
    }

    /// Reset scroll to show latest logs
    pub(crate) fn scroll_logs_to_bottom(&mut self) {
        self.log_scroll_offset = 0;
    }

    /// Scroll windows up by page
    pub(crate) fn scroll_windows_page_up(&mut self, page_size: usize, total_windows: usize) {
        let max_offset = total_windows.saturating_sub(page_size);
        self.window_scroll_offset = (self.window_scroll_offset + page_size).min(max_offset);
    }

    /// Scroll windows down by page
    pub(crate) fn scroll_windows_page_down(&mut self, page_size: usize) {
        self.window_scroll_offset = self.window_scroll_offset.saturating_sub(page_size);
    }

    /// Reset scroll to show top of window list
    pub(crate) fn scroll_windows_to_top(&mut self) {
        self.window_scroll_offset = 0;
    }
}

// Note: format_duration helper will be added when uptime/PID tracking is implemented
// with background polling infrastructure (Phase 9A future enhancement)

/// Truncate string with ellipsis if exceeds max length
fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}…", &s[..max_len.saturating_sub(1)])
    }
}


/// Context for rendering the dashboard screen
pub(crate) struct DashboardRenderContext<'a> {
    pub config: &'a Config,
    pub screen_state: &'a DashboardScreen,
    pub daemon_running: bool,
    pub window_count: usize,
    pub daemon_logs: &'a [String],
    pub windows: &'a [crate::ipc::WindowInfo],
}

/// Render the dashboard screen
pub(crate) fn render_dashboard(frame: &mut Frame, area: Rect, ctx: &DashboardRenderContext) {
    // Phase 9A/9B: Two-section layout (top section + toggleable bottom)
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(10), // Top section (daemon + sink + summary)
            Constraint::Min(0),     // Bottom section (logs OR windows - toggleable)
        ])
        .split(area);

    // Split top section horizontally (left: daemon+summary, right: sink+stats)
    let top_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(chunks[0]);

    // Split left column vertically (daemon above, summary below)
    let left_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(6), // Daemon control
            Constraint::Length(4), // Window summary
        ])
        .split(top_chunks[0]);

    // Split right column vertically (sink above, stats below)
    let right_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(6), // Active sink details
            Constraint::Length(4), // Statistics
        ])
        .split(top_chunks[1]);

    // Render top section components
    render_daemon_section(frame, left_chunks[0], ctx.screen_state, ctx.daemon_running);

    // Calculate matched windows count
    let matched_count = ctx.windows.iter().filter(|w| w.tracked.is_some()).count();

    render_window_summary(
        frame,
        left_chunks[1],
        ctx.window_count,
        matched_count,
        ctx.screen_state.current_view,
    );

    render_sink_card(frame, right_chunks[0], ctx.config);
    render_statistics_card(
        frame,
        right_chunks[1],
        ctx.config,
        matched_count,
        ctx.window_count,
    );

    // Bottom section: render logs OR windows based on current view
    match ctx.screen_state.current_view {
        DashboardView::Logs => {
            render_log_viewer(
                frame,
                chunks[1],
                ctx.daemon_logs,
                ctx.daemon_running,
                ctx.screen_state.log_scroll_offset,
            );
        }
        DashboardView::Windows => {
            render_window_tracking(
                frame,
                chunks[1],
                ctx.windows,
                matched_count,
                ctx.screen_state.window_scroll_offset,
            );
        }
    }
}

/// Render daemon status widget with control buttons (compact vertical layout)
fn render_daemon_section(
    frame: &mut Frame,
    area: Rect,
    screen_state: &DashboardScreen,
    daemon_running: bool,
) {
    let block = Block::default().borders(Borders::ALL).title(" Daemon ");
    frame.render_widget(block.clone(), area);

    let inner = block.inner(area);

    let (status_text, status_color, status_icon) = if daemon_running {
        ("RUNNING", colors::UI_SUCCESS, "●")
    } else {
        ("STOPPED", colors::UI_ERROR, "○")
    };

    // Build status line
    let status_line = Line::from(vec![
        Span::styled(status_icon, Style::default().fg(status_color)),
        Span::raw(" "),
        Span::styled(
            status_text,
            Style::default()
                .fg(status_color)
                .add_modifier(Modifier::BOLD),
        ),
    ]);

    let mut lines = vec![status_line];

    // Add systemd unit status if daemon is running and reports it
    if let Some(enabled) = screen_state.service_enabled {
        let (unit_icon, unit_text, unit_color) = if enabled {
            ("✓", "enabled", colors::UI_SUCCESS)
        } else {
            ("✗", "disabled", colors::UI_ERROR)
        };

        lines.push(Line::from(vec![
            Span::styled("SystemD unit: ", Style::default().fg(colors::UI_SECONDARY)),
            Span::styled(unit_icon, Style::default().fg(unit_color)),
            Span::raw(" "),
            Span::styled(unit_text, Style::default().fg(unit_color)),
        ]));
    }

    // Add spacing to push buttons to bottom
    lines.push(Line::from(""));
    if screen_state.service_enabled.is_none() {
        lines.push(Line::from("")); // Extra line if no systemd status
    }

    // Action buttons (all in a single row)
    let actions: &[&str] = if screen_state.max_action_index == 4 {
        &["START", "STOP", "RESTART", "ENABLE", "DISABLE"]
    } else {
        &["START", "STOP", "RESTART"]
    };

    let separator = Span::styled(" · ", Style::default().fg(colors::UI_SECONDARY));

    // Build button spans
    let mut button_spans = Vec::new();
    for (i, action) in actions.iter().enumerate() {
        let is_selected = i == screen_state.selected_action;

        // Add separator before (except first)
        if i > 0 {
            button_spans.push(separator.clone());
        }

        // Use brackets around selected item, reserve space on unselected for alignment
        if is_selected {
            button_spans.push(Span::styled(
                "[",
                Style::default().fg(colors::UI_HIGHLIGHT),
            ));
            button_spans.push(Span::styled(
                *action,
                Style::default()
                    .fg(colors::UI_SELECTED)
                    .add_modifier(Modifier::BOLD),
            ));
            button_spans.push(Span::styled(
                "]",
                Style::default().fg(colors::UI_HIGHLIGHT),
            ));
        } else {
            // Reserve space for brackets to prevent text shifting
            button_spans.push(Span::raw(" "));
            button_spans.push(Span::styled(*action, Style::default().fg(colors::UI_TEXT)));
            button_spans.push(Span::raw(" "));
        }
    }

    lines.push(Line::from(button_spans));

    let paragraph = Paragraph::new(lines);
    frame.render_widget(paragraph, inner);
}

/// Render window summary card (shows counts and toggle hint)
fn render_window_summary(
    frame: &mut Frame,
    area: Rect,
    window_count: usize,
    matched_count: usize,
    current_view: DashboardView,
) {
    let block = Block::default().borders(Borders::ALL).title(" Windows ");
    frame.render_widget(block.clone(), area);

    let inner = block.inner(area);

    let lines = vec![
        Line::from(vec![
            Span::styled("Matched: ", Style::default().fg(colors::UI_SECONDARY)),
            Span::styled(
                matched_count.to_string(),
                Style::default()
                    .fg(colors::UI_SUCCESS)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("/", Style::default().fg(colors::UI_STAT)),
            Span::styled(
                window_count.to_string(),
                Style::default()
                    .fg(colors::UI_STAT)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![Span::styled(
            match current_view {
                DashboardView::Logs => "Press [w] to view details",
                DashboardView::Windows => "Viewing below (press [w] for logs)",
            },
            Style::default().fg(colors::UI_SECONDARY),
        )]),
    ];

    let paragraph = Paragraph::new(lines);
    frame.render_widget(paragraph, inner);
}

/// Render current sink card
fn render_sink_card(frame: &mut Frame, area: Rect, config: &Config) {
    let current_sink_name = crate::pipewire::PipeWire::get_default_sink_name().ok();

    let (sink_desc, sink_icon, node_name) = current_sink_name
        .as_ref()
        .and_then(|name| {
            config.sinks.iter().find(|s| &s.name == name).map(|s| {
                (
                    s.desc.clone(),
                    s.icon.clone().unwrap_or_else(|| "🔊".to_string()),
                    name.clone(),
                )
            })
        })
        .unwrap_or((
            "Unknown Sink".to_string(),
            "?".to_string(),
            "unknown".to_string(),
        ));

    let text = vec![
        Line::from(vec![
            Span::styled(sink_icon, Style::default().fg(colors::UI_HIGHLIGHT)),
            Span::raw(" "),
            Span::styled(
                sink_desc,
                Style::default()
                    .fg(colors::UI_TEXT)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("Node: ", Style::default().fg(colors::UI_SECONDARY)),
            Span::styled(
                truncate_node_name(&node_name, 35),
                Style::default().fg(colors::UI_TEXT),
            ),
        ]),
        Line::from(Span::styled(
            "Active Audio Output",
            Style::default().fg(colors::UI_SECONDARY),
        )),
    ];

    let paragraph = Paragraph::new(text)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Active Sink ")
                .border_style(Style::default().fg(colors::UI_HIGHLIGHT)),
        )
        .alignment(Alignment::Left);

    frame.render_widget(paragraph, area);
}

/// Render statistics card showing quick overview
fn render_statistics_card(
    frame: &mut Frame,
    area: Rect,
    config: &Config,
    _matched_count: usize,
    _window_count: usize,
) {
    let rule_count = config.rules.len();
    let sink_count = config.sinks.len();

    let lines = vec![
        Line::from(vec![
            Span::styled("Rules: ", Style::default().fg(colors::UI_SECONDARY)),
            Span::styled(
                format!("{rule_count} active"),
                Style::default()
                    .fg(colors::UI_STAT)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::styled("Sinks: ", Style::default().fg(colors::UI_SECONDARY)),
            Span::styled(
                format!("{sink_count} available"),
                Style::default()
                    .fg(colors::UI_STAT)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
    ];

    let paragraph =
        Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title(" Overview "));

    frame.render_widget(paragraph, area);
}

/// Render window tracking section (full width bottom section when view is Windows)
fn render_window_tracking(
    frame: &mut Frame,
    area: Rect,
    windows: &[crate::ipc::WindowInfo],
    matched_count: usize,
    scroll_offset: usize,
) {
    let title = " Window Tracking - [w] to toggle back to Logs ";
    let block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .border_style(Style::default().fg(colors::UI_HIGHLIGHT));
    frame.render_widget(block.clone(), area);

    let inner = block.inner(area);
    let available_height = inner.height as usize;

    // Count matched vs total
    let total_count = windows.len();

    let mut lines = vec![
        Line::from(vec![
            Span::styled("Matched: ", Style::default().fg(colors::UI_SECONDARY)),
            Span::styled(
                format!("{matched_count}/{total_count} windows"),
                Style::default()
                    .fg(colors::UI_STAT)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(""),
    ];

    // Build window list with matched windows first
    let mut window_lines: Vec<Line> = Vec::new();

    // Add matched windows (use UI_MATCHED color - green for matched)
    for win in windows.iter().filter(|w| w.tracked.is_some()) {
        let rule_desc = win
            .tracked
            .as_ref()
            .map_or("Unknown", |t| t.sink_desc.as_str());

        let mut spans = vec![
            Span::styled("● ", Style::default().fg(colors::UI_MATCHED)),
            Span::styled(
                truncate(&win.app_id, WINDOW_APPID_MAX_WIDTH),
                Style::default()
                    .fg(colors::UI_TEXT)
                    .add_modifier(Modifier::BOLD),
            ),
        ];

        // Add title inline if present
        if !win.title.is_empty() {
            spans.push(Span::raw(" | "));
            spans.push(Span::styled(
                truncate(&win.title, WINDOW_TITLE_MAX_WIDTH),
                Style::default().fg(colors::UI_SECONDARY),
            ));
        }

        spans.push(Span::raw(" → "));
        spans.push(Span::styled(
            rule_desc,
            Style::default().fg(colors::UI_HIGHLIGHT),
        ));

        window_lines.push(Line::from(spans));
    }

    // Add unmatched windows (use UI_UNMATCHED color - dark gray)
    for win in windows.iter().filter(|w| w.tracked.is_none()) {
        let mut spans = vec![
            Span::styled("○ ", Style::default().fg(colors::UI_UNMATCHED)),
            Span::styled(
                truncate(&win.app_id, WINDOW_APPID_MAX_WIDTH),
                Style::default().fg(colors::UI_UNMATCHED),
            ),
        ];

        // Add title inline if present
        if !win.title.is_empty() {
            spans.push(Span::raw(" | "));
            spans.push(Span::styled(
                truncate(&win.title, WINDOW_TITLE_MAX_WIDTH),
                Style::default().fg(colors::UI_UNMATCHED),
            ));
        }

        window_lines.push(Line::from(spans));
    }

    // Calculate visible range based on scroll offset
    let total_lines = window_lines.len();
    let visible_count = available_height.saturating_sub(2); // Reserve space for header
    let start_idx = scroll_offset.min(total_lines.saturating_sub(visible_count));
    let end_idx = (start_idx + visible_count).min(total_lines);

    // Add visible window lines
    for line in window_lines
        .iter()
        .skip(start_idx)
        .take(end_idx - start_idx)
    {
        lines.push(line.clone());
    }

    // Add scroll indicator if needed
    if total_lines > visible_count {
        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::styled(
                format!("  [{}/{}] ", start_idx + 1, total_lines),
                Style::default().fg(colors::UI_SECONDARY),
            ),
            Span::styled(
                "PgUp/PgDn to scroll",
                Style::default().fg(colors::UI_SECONDARY),
            ),
        ]));
    }

    // Handle empty window list
    if window_lines.is_empty() {
        lines.push(Line::from(Span::styled(
            "No windows tracked",
            Style::default().fg(colors::UI_SECONDARY),
        )));
    }

    let paragraph = Paragraph::new(lines);
    frame.render_widget(paragraph, inner);
}

/// Highlight patterns in log message (`app_id`, `title`, `Rule matched`, etc.)
fn highlight_message(message: &str) -> Vec<Span<'_>> {
    let mut spans = Vec::new();
    let mut last_end = 0;

    // Track positions of all pattern matches
    let mut matches: Vec<(usize, usize, Color, bool)> = Vec::new();

    for (pattern, color, bold) in HIGHLIGHT_PATTERNS {
        let mut start = 0;
        while let Some(pos) = message[start..].find(pattern) {
            let abs_pos = start + pos;
            let end = abs_pos + pattern.len();
            matches.push((abs_pos, end, *color, *bold));
            start = end;
        }
    }

    // Sort matches by position, then by length (longer first for same position)
    matches.sort_by(|(start_a, end_a, _, _), (start_b, end_b, _, _)| {
        start_a
            .cmp(start_b)
            .then_with(|| (end_b - start_b).cmp(&(end_a - start_a)))
    });

    // Remove overlapping matches (keep first/longer match)
    let mut filtered_matches: Vec<(usize, usize, Color, bool)> = Vec::new();
    for (start, end, color, bold) in matches {
        // Check if this match overlaps with any already filtered match
        let overlaps = filtered_matches
            .iter()
            .any(|(f_start, f_end, _, _)| start < *f_end && end > *f_start);
        if !overlaps {
            filtered_matches.push((start, end, color, bold));
        }
    }

    // Build spans with highlighted patterns
    for (start, end, color, bold) in filtered_matches {
        // Add text before this match
        if last_end < start {
            spans.push(Span::styled(
                &message[last_end..start],
                Style::default().fg(colors::LOG_MESSAGE),
            ));
        }

        // Add highlighted match
        let style = if bold {
            Style::default().fg(color).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(color)
        };
        spans.push(Span::styled(&message[start..end], style));
        last_end = end;
    }

    // Add remaining text
    if last_end < message.len() {
        spans.push(Span::styled(
            &message[last_end..],
            Style::default().fg(colors::LOG_MESSAGE),
        ));
    }

    // If no matches, return whole message
    if spans.is_empty() {
        spans.push(Span::styled(
            message,
            Style::default().fg(colors::LOG_MESSAGE),
        ));
    }

    spans
}

/// Parse and style a log line with colored components
fn style_log_line(line: &str) -> Line<'_> {
    // Try to parse log format: "TIMESTAMP LEVEL message"
    // Example: "2025-12-17T10:00:00.123456Z  INFO message here"

    let parts: Vec<&str> = line.splitn(3, ' ').collect();

    if parts.len() < 3 {
        // Malformed line, return as-is
        return Line::from(Span::raw(line));
    }

    let full_timestamp = parts[0];
    let level = parts[1].trim();
    let message = parts[2];

    // Extract time-only from ISO 8601 timestamp (HH:MM:SS)
    // Format: "2025-12-17T10:00:00.123456Z" -> "10:00:00"
    let timestamp = if let Some(time_start) = full_timestamp.find('T') {
        let time_part = &full_timestamp[time_start + 1..];
        // Take up to the first '.' (milliseconds) or 8 chars (HH:MM:SS)
        if let Some(dot_pos) = time_part.find('.') {
            &time_part[..dot_pos.min(8)]
        } else {
            &time_part[..8.min(time_part.len())]
        }
    } else {
        // Fallback: use full timestamp if parsing fails
        full_timestamp
    };

    // Determine colors based on log level
    let (level_color, level_bold) = match level {
        "TRACE" => (colors::LOG_LEVEL_TRACE, false),
        "DEBUG" => (colors::LOG_LEVEL_DEBUG, false),
        "INFO" => (colors::LOG_LEVEL_INFO, false),
        "WARN" => (colors::LOG_LEVEL_WARN, true),
        "ERROR" => (colors::LOG_LEVEL_ERROR, true),
        _ => (colors::LOG_MESSAGE, false),
    };

    let level_style = if level_bold {
        Style::default()
            .fg(level_color)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(level_color)
    };

    // Build styled line with colored components
    let mut line_spans = vec![
        Span::styled(timestamp, Style::default().fg(colors::LOG_TIMESTAMP)),
        Span::raw(" "),
        Span::styled(level, level_style),
        Span::raw(" "),
    ];

    // Add highlighted message spans
    line_spans.extend(highlight_message(message));

    Line::from(line_spans)
}

/// Render daemon log viewer
fn render_log_viewer(
    frame: &mut Frame,
    area: Rect,
    daemon_logs: &[String],
    daemon_running: bool,
    scroll_offset: usize,
) {
    let available_height = area.height.saturating_sub(2) as usize; // Account for borders
    let total_lines = daemon_logs.len();

    // Calculate which lines to show based on scroll offset
    // scroll_offset=0 means showing latest logs (bottom)
    // scroll_offset>0 means scrolled back in history
    let end_index = total_lines.saturating_sub(scroll_offset);
    let start_index = end_index.saturating_sub(available_height);

    // Build title with scroll indicator and toggle hint (with colors)
    let title = if scroll_offset > 0 {
        let status_text = if daemon_running { "Live" } else { "Stopped" };
        let status_color = if daemon_running {
            colors::UI_SUCCESS
        } else {
            colors::UI_ERROR
        };

        Line::from(vec![
            Span::raw(" Daemon Logs ("),
            Span::styled(status_text, Style::default().fg(status_color)),
            Span::raw(") - "),
            Span::styled(
                format!("↑{scroll_offset}"),
                Style::default().fg(colors::UI_WARNING),
            ),
            Span::raw(" - [w] to toggle to Windows "),
        ])
    } else {
        let status_text = if daemon_running { "Live" } else { "Stopped" };
        let status_color = if daemon_running {
            colors::UI_SUCCESS
        } else {
            colors::UI_ERROR
        };

        Line::from(vec![
            Span::raw(" Daemon Logs ("),
            Span::styled(status_text, Style::default().fg(status_color)),
            Span::raw(") - [w] to toggle to Windows "),
        ])
    };

    let border_color = if daemon_running {
        colors::UI_BORDER_ACTIVE
    } else {
        colors::UI_BORDER_INACTIVE
    };

    let visible_logs: Vec<Line> = daemon_logs
        .iter()
        .skip(start_index)
        .take(end_index - start_index)
        .map(|line| style_log_line(line))
        .collect();

    let log_text = if visible_logs.is_empty() {
        vec![Line::from(Span::styled(
            "No logs available. Start the daemon to see logs here.",
            Style::default().fg(colors::UI_SECONDARY),
        ))]
    } else {
        visible_logs
    };

    let paragraph = Paragraph::new(log_text).block(
        Block::default()
            .borders(Borders::ALL)
            .title(title)
            .border_style(Style::default().fg(border_color)),
    );

    frame.render_widget(paragraph, area);
}
