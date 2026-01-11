//! Dashboard screen - Overview and quick actions

use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph},
};

use crate::config::Config;
use crate::style::colors;
use crate::tui::widgets::truncate_node_name;

/// Patterns to highlight in log messages (keyword, color, bold)
const HIGHLIGHT_PATTERNS: &[(&str, Color, bool)] = &[
    // Important events (bold)
    ("Rule matched:", colors::LOG_EVENT, true),
    ("Rule unmatched:", colors::UI_WARNING, true),
    ("Switching:", colors::LOG_EVENT, true),
    ("Switching profile:", colors::UI_HIGHLIGHT, true),
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
///
/// Uses character-based truncation (not byte-based) to safely handle UTF-8 strings.
fn truncate(s: &str, max_len: usize) -> String {
    let char_count = s.chars().count();
    if char_count <= max_len {
        s.to_string()
    } else {
        let truncated: String = s.chars().take(max_len.saturating_sub(1)).collect();
        format!("{truncated}…")
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
    let [top_section, bottom_section] = Layout::vertical([
        Constraint::Length(10), // Top section (daemon + sink + summary)
        Constraint::Min(0),     // Bottom section (logs OR windows - toggleable)
    ])
    .areas(area);

    // Split top section horizontally (left: daemon+summary, right: sink+stats)
    let [left_col, right_col] =
        Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)])
            .areas(top_section);

    // Split left column vertically (daemon above, summary below)
    let [daemon_area, summary_area] = Layout::vertical([
        Constraint::Length(6), // Daemon control
        Constraint::Length(4), // Window summary
    ])
    .areas(left_col);

    // Split right column vertically (sink above, stats below)
    let [sink_area, stats_area] = Layout::vertical([
        Constraint::Length(6), // Active sink details
        Constraint::Length(4), // Statistics
    ])
    .areas(right_col);

    // Render top section components
    render_daemon_section(frame, daemon_area, ctx.screen_state, ctx.daemon_running);

    // Calculate matched windows count
    let matched_count = ctx.windows.iter().filter(|w| w.tracked.is_some()).count();

    render_window_summary(
        frame,
        summary_area,
        ctx.window_count,
        matched_count,
        ctx.screen_state.current_view,
    );

    render_sink_card(frame, sink_area, ctx.config);
    render_statistics_card(
        frame,
        stats_area,
        ctx.config,
        matched_count,
        ctx.window_count,
    );

    // Bottom section: render logs OR windows based on current view
    match ctx.screen_state.current_view {
        DashboardView::Logs => {
            render_log_viewer(
                frame,
                bottom_section,
                ctx.daemon_logs,
                ctx.daemon_running,
                ctx.screen_state.log_scroll_offset,
            );
        }
        DashboardView::Windows => {
            render_window_tracking(
                frame,
                bottom_section,
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
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .title(" Daemon ");
    frame.render_widget(block.clone(), area);

    let inner = block.inner(area);

    let (status_text, status_color, status_icon) = if daemon_running {
        ("RUNNING", colors::UI_SUCCESS, "●")
    } else {
        ("STOPPED", colors::UI_ERROR, "○")
    };

    // Build compact status line
    let mut status_spans = vec![
        Span::styled(status_icon, Style::default().fg(status_color)),
        Span::raw(" "),
        Span::styled(
            status_text,
            Style::default()
                .fg(status_color)
                .add_modifier(Modifier::BOLD),
        ),
    ];

    // Append systemd status if available
    if let Some(enabled) = screen_state.service_enabled {
        status_spans.push(Span::styled(
            " · ",
            Style::default().fg(colors::UI_SECONDARY),
        ));
        let (unit_icon, unit_text, unit_color) = if enabled {
            ("✓", "enabled", colors::UI_SUCCESS)
        } else {
            ("✗", "disabled", colors::UI_ERROR)
        };
        status_spans.push(Span::styled(unit_icon, Style::default().fg(unit_color)));
        status_spans.push(Span::raw(" "));
        status_spans.push(Span::styled(unit_text, Style::default().fg(unit_color)));
    }

    let mut lines = vec![
        Line::from(status_spans),
        Line::from(""), // Spacing
    ];

    // Helper for buttons
    let render_button = |label: &'static str, index: usize| -> Vec<Span> {
        let is_selected = index == screen_state.selected_action;
        let mut spans = Vec::new();
        if is_selected {
            spans.push(Span::styled("[", Style::default().fg(colors::UI_HIGHLIGHT)));
            spans.push(Span::styled(
                label,
                Style::default()
                    .fg(colors::UI_SELECTED)
                    .add_modifier(Modifier::BOLD),
            ));
            spans.push(Span::styled("]", Style::default().fg(colors::UI_HIGHLIGHT)));
        } else {
            spans.push(Span::raw(" "));
            spans.push(Span::styled(label, Style::default().fg(colors::UI_TEXT)));
            spans.push(Span::raw(" "));
        }
        spans
    };

    // Row 1: Runtime actions
    let mut row1 = Vec::new();
    row1.extend(render_button("START", 0));
    row1.push(Span::raw("  "));
    row1.extend(render_button("STOP", 1));
    row1.push(Span::raw("  "));
    row1.extend(render_button("RESTART", 2));

    lines.push(Line::from(row1));

    // Row 2: Persistence actions (only if systemd available)
    if screen_state.max_action_index == 4 {
        let mut row2 = Vec::new();
        row2.extend(render_button("ENABLE", 3));
        row2.push(Span::raw("   "));
        row2.extend(render_button("DISABLE", 4));
        lines.push(Line::from(row2));
    }

    let paragraph = Paragraph::new(lines).alignment(Alignment::Center);
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
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .title(" Windows ");
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
                .border_type(BorderType::Rounded)
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

    let paragraph = Paragraph::new(lines).block(
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .title(" Overview "),
    );

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
        .border_type(BorderType::Rounded)
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

    let paragraph = Paragraph::new(log_text)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .title(title)
                .border_style(Style::default().fg(border_color)),
        )
        .wrap(ratatui::widgets::Wrap { trim: false });

    frame.render_widget(paragraph, area);
}

#[cfg(test)]
mod tests {
    /// Helper to calculate log viewport indices
    /// This mirrors the logic in `render_log_viewer` (lines 752-754)
    fn calculate_log_viewport(
        total_lines: usize,
        scroll_offset: usize,
        available_height: usize,
    ) -> (usize, usize) {
        let end_index = total_lines.saturating_sub(scroll_offset);
        let start_index = end_index.saturating_sub(available_height);
        (start_index, end_index)
    }

    #[test]
    fn test_log_scroll_viewport_empty_logs() {
        // Empty logs → no content to display
        let (start, end) = calculate_log_viewport(0, 0, 20);
        assert_eq!((start, end), (0, 0));
        assert_eq!(end - start, 0); // No lines to display
    }

    #[test]
    fn test_log_scroll_viewport_logs_smaller_than_viewport() {
        // 5 logs, 20 line viewport → show all 5
        let (start, end) = calculate_log_viewport(5, 0, 20);
        assert_eq!((start, end), (0, 5));
        assert_eq!(end - start, 5);
    }

    #[test]
    fn test_log_scroll_viewport_scroll_offset_exceeds_total() {
        // 10 logs, scroll_offset=100 (way too much) → end_index becomes 0
        // When end_index = 0, no logs are visible
        let (start, end) = calculate_log_viewport(10, 100, 20);
        assert_eq!((start, end), (0, 0));
        assert_eq!(end - start, 0); // No lines visible when scrolled way beyond available logs
    }

    #[test]
    fn test_log_scroll_viewport_exactly_viewport_size() {
        // 20 logs, 20 line viewport, no scroll → show all
        let (start, end) = calculate_log_viewport(20, 0, 20);
        assert_eq!((start, end), (0, 20));
        assert_eq!(end - start, 20);
    }

    #[test]
    fn test_log_scroll_viewport_normal_scroll() {
        // 100 logs, 20 line viewport, scroll_offset=10 → show lines 70-90
        let (start, end) = calculate_log_viewport(100, 10, 20);
        assert_eq!((start, end), (70, 90));
        assert_eq!(end - start, 20);
    }

    #[test]
    fn test_log_scroll_viewport_max_scroll() {
        // 100 logs, 20 line viewport, scroll_offset=80 → show lines 0-20 (oldest)
        let (start, end) = calculate_log_viewport(100, 80, 20);
        assert_eq!((start, end), (0, 20));
        assert_eq!(end - start, 20);
    }

    /// Helper to calculate window viewport indices
    /// This mirrors the logic in `render_window_tracking` (line 598)
    fn calculate_window_viewport(
        total_lines: usize,
        scroll_offset: usize,
        visible_count: usize,
    ) -> (usize, usize) {
        let start_idx = scroll_offset.min(total_lines.saturating_sub(visible_count));
        let end_idx = (start_idx + visible_count).min(total_lines);
        (start_idx, end_idx)
    }

    #[test]
    fn test_window_scroll_viewport_empty() {
        let (start, end) = calculate_window_viewport(0, 0, 20);
        assert_eq!((start, end), (0, 0));
    }

    #[test]
    fn test_window_scroll_viewport_smaller_than_page() {
        let (start, end) = calculate_window_viewport(5, 0, 20);
        assert_eq!((start, end), (0, 5));
    }

    #[test]
    fn test_window_scroll_viewport_scroll_beyond_end() {
        // 10 windows, scroll_offset=50 → clamp to show last page
        let (start, end) = calculate_window_viewport(10, 50, 20);
        assert_eq!((start, end), (0, 10));
    }

    #[test]
    fn test_window_scroll_viewport_normal_pagination() {
        // 100 windows, page_size=20, scroll_offset=40 → show lines 40-60
        let (start, end) = calculate_window_viewport(100, 40, 20);
        assert_eq!((start, end), (40, 60));
        assert_eq!(end - start, 20);
    }

    #[test]
    fn test_truncate_utf8_window_titles() {
        use super::truncate;

        // Test UTF-8 multibyte characters in window titles
        // ✳ is 3 bytes, but 1 character
        assert_eq!(
            truncate("sparkling-crab | ✳ Git Commit", 30),
            "sparkling-crab | ✳ Git Commit"
        );
        assert_eq!(
            truncate("sparkling-crab | ✳ Git Commit", 20),
            "sparkling-crab | ✳ …"
        ); // 19 chars + ellipsis

        // Emoji test (4-byte characters)
        assert_eq!(truncate("🎵 Music Player", 20), "🎵 Music Player");
        assert_eq!(truncate("🎵 Music Player", 10), "🎵 Music P…"); // 9 chars: "🎵 Music P" + ellipsis

        // Mixed ASCII and UTF-8 (this was the crash scenario)
        let window_title = "browser | ✓ Logged In";
        assert_eq!(truncate(window_title, 25), "browser | ✓ Logged In");
        assert_eq!(truncate(window_title, 15), "browser | ✓ Lo…"); // 14 chars + ellipsis

        // Edge case: exactly at character boundary
        assert_eq!(truncate("test✳", 5), "test✳");
        assert_eq!(truncate("test✳", 4), "tes…"); // 3 chars + ellipsis

        // Multiple emoji
        assert_eq!(truncate("🎮 Game | 🎯 Target", 20), "🎮 Game | 🎯 Target");
        assert_eq!(truncate("🎮 Game | 🎯 Target", 12), "🎮 Game | 🎯 …"); // 11 chars: "🎮 Game | 🎯 " + ellipsis
    }
}
