//! Dashboard screen - Overview and quick actions

use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph},
    Frame,
};

use crate::config::Config;
use crate::style::colors;

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

/// Dashboard screen state
pub(crate) struct DashboardScreen {
    pub selected_action: usize,   // 0 = start, 1 = stop, 2 = restart
    pub log_scroll_offset: usize, // Lines scrolled back from the end (0 = showing latest)
}

impl DashboardScreen {
    pub(crate) fn new() -> Self {
        Self {
            selected_action: 0,
            log_scroll_offset: 0,
        }
    }

    pub(crate) fn select_next(&mut self) {
        if self.selected_action < 2 {
            self.selected_action += 1;
        }
    }

    pub(crate) fn select_previous(&mut self) {
        if self.selected_action > 0 {
            self.selected_action -= 1;
        }
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
}

/// Render the dashboard screen
pub(crate) fn render_dashboard(
    frame: &mut Frame,
    area: Rect,
    config: &Config,
    screen_state: &DashboardScreen,
    daemon_running: bool,
    window_count: usize,
    daemon_logs: &[String],
) {
    // Split screen into sections: Header (Status/Control), Cards, and Logs
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(8),  // Daemon status + controls
            Constraint::Length(10), // Info cards (reduced from Min(0))
            Constraint::Min(0),     // Daemon logs
        ])
        .split(area);

    // Daemon Status Section
    render_daemon_section(frame, chunks[0], screen_state, daemon_running);

    // Info Grid (Horizontal split)
    let card_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .margin(1) // Add margin between top section and cards
        .split(chunks[1]);

    // Current Sink Card
    render_sink_card(frame, card_chunks[0], config);

    // Stats Card
    render_stats_card(frame, card_chunks[1], window_count);

    // Daemon Logs
    render_log_viewer(
        frame,
        chunks[2],
        daemon_logs,
        daemon_running,
        screen_state.log_scroll_offset,
    );
}

/// Render daemon status widget with control buttons
fn render_daemon_section(
    frame: &mut Frame,
    area: Rect,
    screen_state: &DashboardScreen,
    daemon_running: bool,
) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" System Control ");
    frame.render_widget(block.clone(), area);

    let inner = block.inner(area);

    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
        .split(inner);

    // Left: Status Indicator
    let (status_text, status_color, status_icon) = if daemon_running {
        ("RUNNING", colors::UI_SUCCESS, "●")
    } else {
        ("STOPPED", colors::UI_ERROR, "○")
    };

    let status_content = vec![
        Line::from(""),
        Line::from(vec![Span::styled(
            "Daemon Status",
            Style::default().fg(colors::UI_SECONDARY),
        )]),
        Line::from(vec![
            Span::styled(status_icon, Style::default().fg(status_color)),
            Span::raw(" "),
            Span::styled(
                status_text,
                Style::default()
                    .fg(status_color)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
    ];

    let status_paragraph = Paragraph::new(status_content).alignment(Alignment::Center);
    frame.render_widget(status_paragraph, chunks[0]);

    // Right: Actions List
    let actions = ["Start Daemon", "Stop Daemon", "Restart Daemon"];
    let items: Vec<ListItem> = actions
        .iter()
        .enumerate()
        .map(|(i, action)| {
            let is_selected = i == screen_state.selected_action;
            let style = if is_selected {
                Style::default()
                    .fg(colors::UI_SELECTED)
                    .add_modifier(Modifier::BOLD)
                    .bg(colors::UI_SELECTED_BG)
            } else {
                Style::default().fg(colors::UI_TEXT)
            };

            let prefix = if is_selected { " ▶ " } else { "   " };
            ListItem::new(Line::from(vec![
                Span::styled(prefix, Style::default().fg(colors::UI_HIGHLIGHT)),
                Span::styled(*action, style),
            ]))
        })
        .collect();

    let controls_list = List::new(items).block(
        Block::default()
            .borders(Borders::LEFT)
            .title(" Actions ([Enter] to execute) "),
    );
    frame.render_widget(controls_list, chunks[1]);
}

/// Render current sink card
fn render_sink_card(frame: &mut Frame, area: Rect, config: &Config) {
    let current_sink_name = crate::pipewire::PipeWire::get_default_sink_name().ok();

    let (sink_desc, sink_name) = current_sink_name
        .as_ref()
        .and_then(|name| {
            config.sinks.iter().find(|s| &s.name == name).map(|s| {
                (
                    s.desc.clone(),
                    s.icon.clone().unwrap_or_else(|| "🔊".to_string()),
                )
            })
        })
        .unwrap_or(("Unknown Sink".to_string(), "?".to_string()));

    let text = vec![
        Line::from(""),
        Line::from(vec![
            Span::styled(sink_name, Style::default().fg(colors::UI_HIGHLIGHT)),
            Span::raw(" "),
            Span::styled(
                sink_desc,
                Style::default()
                    .fg(colors::UI_TEXT)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(""),
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
        .alignment(Alignment::Center);

    frame.render_widget(paragraph, area);
}

/// Render active windows card
fn render_stats_card(frame: &mut Frame, area: Rect, window_count: usize) {
    let text = vec![
        Line::from(""),
        Line::from(vec![
            Span::styled(
                window_count.to_string(),
                Style::default()
                    .fg(colors::UI_STAT)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" windows"),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "Currently Tracked",
            Style::default().fg(colors::UI_SECONDARY),
        )),
    ];

    let paragraph = Paragraph::new(text)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Statistics ")
                .border_style(Style::default().fg(colors::UI_STAT)),
        )
        .alignment(Alignment::Center);

    frame.render_widget(paragraph, area);
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

    // Build title with scroll indicator
    let title = if scroll_offset > 0 {
        if daemon_running {
            format!(" Daemon Logs (Live) - ↑{scroll_offset} ")
        } else {
            format!(" Daemon Logs (Stopped) - ↑{scroll_offset} ")
        }
    } else if daemon_running {
        " Daemon Logs (Live) ".to_string()
    } else {
        " Daemon Logs (Stopped) ".to_string()
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
