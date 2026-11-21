//! Dashboard screen - Overview and quick actions

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};

use crate::config::Config;
use crate::style::colors;
use crate::tui::widgets::{rounded_block, truncate_node_name};

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
pub enum DashboardView {
    Logs,
    Windows,
}

/// Dashboard screen state
///
/// Scroll limits and page sizes depend on the viewport, so rendering records
/// them here and the input handlers use them.
#[derive(Debug)]
pub struct DashboardScreen {
    pub selected_action: usize, // 0-4: start, stop, restart, enable, disable
    /// Log lines scrolled back from the end (0 = live, following the newest)
    pub log_scroll_offset: usize,
    /// First window shown in the window list
    pub window_scroll_offset: usize,
    pub current_view: DashboardView, // Toggle between Logs and Windows
    pub max_action_index: usize,     // Maximum action index (2 without unit, 4 with)
    pub service_enabled: Option<bool>, // None without unit, Some(true/false) with it
    /// Rows of the log viewport at the last render
    log_viewport: usize,
    /// Largest useful `log_scroll_offset` at the last render
    log_max_offset: usize,
    /// Windows that fit on one page at the last render
    window_page: usize,
    /// Largest useful `window_scroll_offset` at the last render
    window_max_offset: usize,
}

impl DashboardScreen {
    pub(crate) const fn new() -> Self {
        Self {
            selected_action: 0,
            log_scroll_offset: 0,
            window_scroll_offset: 0,
            current_view: DashboardView::Logs, // Default to logs
            max_action_index: 2,               // Default to 3 actions (start/stop/restart)
            service_enabled: None,             // Updated by background worker
            log_viewport: 10,
            log_max_offset: usize::MAX,
            window_page: 5,
            window_max_offset: usize::MAX,
        }
    }

    /// Show enable/disable only if the systemd unit is installed
    pub(crate) const fn set_max_actions(&mut self, unit_installed: bool) {
        self.max_action_index = if unit_installed { 4 } else { 2 };
        // Clamp current selection if it's out of range
        if self.selected_action > self.max_action_index {
            self.selected_action = 0;
        }
    }

    pub(crate) const fn select_next(&mut self) {
        if self.selected_action < self.max_action_index {
            self.selected_action += 1;
        } else {
            self.selected_action = 0; // Wrap to first
        }
    }

    pub(crate) const fn select_previous(&mut self) {
        if self.selected_action > 0 {
            self.selected_action -= 1;
        } else {
            self.selected_action = self.max_action_index; // Wrap to last
        }
    }

    /// Toggle between logs and windows view
    pub(crate) const fn toggle_view(&mut self) {
        self.current_view = match self.current_view {
            DashboardView::Logs => DashboardView::Windows,
            DashboardView::Windows => DashboardView::Logs,
        };
    }

    /// Lines one page of the current view scrolls by
    pub(crate) const fn page_size(&self) -> usize {
        let page = match self.current_view {
            DashboardView::Logs => self.log_viewport,
            DashboardView::Windows => self.window_page,
        };
        if page == 0 { 1 } else { page }
    }

    /// Scroll the current view towards older logs / the top of the window list
    pub(crate) fn scroll_up(&mut self, lines: usize) {
        match self.current_view {
            DashboardView::Logs => {
                self.log_scroll_offset = self
                    .log_scroll_offset
                    .saturating_add(lines)
                    .min(self.log_max_offset);
            }
            DashboardView::Windows => {
                self.window_scroll_offset = self.window_scroll_offset.saturating_sub(lines);
            }
        }
    }

    /// Scroll the current view towards newer logs / the bottom of the window list
    pub(crate) fn scroll_down(&mut self, lines: usize) {
        match self.current_view {
            DashboardView::Logs => {
                self.log_scroll_offset = self.log_scroll_offset.saturating_sub(lines);
            }
            DashboardView::Windows => {
                self.window_scroll_offset = self
                    .window_scroll_offset
                    .saturating_add(lines)
                    .min(self.window_max_offset);
            }
        }
    }

    /// Jump to the oldest logs / the first window
    pub(crate) const fn scroll_to_top(&mut self) {
        match self.current_view {
            DashboardView::Logs => self.log_scroll_offset = self.log_max_offset,
            DashboardView::Windows => self.window_scroll_offset = 0,
        }
    }

    /// Jump back to live logs / the last window
    pub(crate) const fn scroll_to_bottom(&mut self) {
        match self.current_view {
            DashboardView::Logs => self.log_scroll_offset = 0,
            DashboardView::Windows => self.window_scroll_offset = self.window_max_offset,
        }
    }

    /// After `added` lines were appended (buffer now `len` lines long), keep a
    /// scrolled-back log view on the same lines; the live view follows.
    pub(crate) fn keep_log_position(&mut self, added: usize, len: usize) {
        if self.log_scroll_offset > 0 {
            self.log_scroll_offset = self
                .log_scroll_offset
                .saturating_add(added)
                .min(len.saturating_sub(1));
        }
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
pub struct DashboardRenderContext<'a> {
    pub config: &'a Config,
    /// Rendering records viewport sizes in it for scrolling
    pub screen_state: &'a mut DashboardScreen,
    pub daemon_running: bool,
    pub window_count: usize,
    pub daemon_logs: &'a [String],
    pub windows: &'a [crate::ipc::WindowInfo],
    /// Active sinks from the last `PipeWire` snapshot (one has `is_default`)
    pub active_sinks: &'a [crate::pipewire::ActiveSink],
    /// Whether the last `PipeWire` snapshot succeeded
    pub pipewire_available: bool,
}

/// Render the dashboard screen
pub fn render_dashboard(frame: &mut Frame, area: Rect, ctx: &mut DashboardRenderContext) {
    // Two-section layout (top section + toggleable bottom)
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

    render_sink_card(
        frame,
        sink_area,
        ctx.config,
        ctx.active_sinks,
        ctx.pipewire_available,
    );
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
                ctx.screen_state,
            );
        }
        DashboardView::Windows => {
            render_window_tracking(
                frame,
                bottom_section,
                ctx.windows,
                matched_count,
                ctx.screen_state,
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
    let block = rounded_block().title(" Daemon ");
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
            spans.push(Span::styled(
                label,
                Style::default().fg(colors::UI_SECONDARY),
            ));
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
    let block = rounded_block().title(" Windows ");
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

/// Render current sink card from the last `PipeWire` snapshot (no I/O here)
fn render_sink_card(
    frame: &mut Frame,
    area: Rect,
    config: &Config,
    active_sinks: &[crate::pipewire::ActiveSink],
    pipewire_available: bool,
) {
    let default_sink = active_sinks.iter().find(|s| s.is_default);
    let (sink_desc, sink_icon, node_name) = match default_sink {
        Some(active) => match config.sinks.iter().find(|s| s.name == active.name) {
            Some(configured) => (
                configured.desc.as_str(),
                configured.icon.as_deref().unwrap_or("🔊"),
                active.name.as_str(),
            ),
            // Default sink not in the config: show what PipeWire calls it
            None => (active.description.as_str(), "?", active.name.as_str()),
        },
        None if !pipewire_available => ("PipeWire unavailable", "?", "unknown"),
        None => ("Unknown Sink", "?", "unknown"),
    };

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
                truncate_node_name(node_name, 35),
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
            rounded_block()
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

    let paragraph = Paragraph::new(lines).block(rounded_block().title(" Overview "));

    frame.render_widget(paragraph, area);
}

/// One row of the window list
fn window_line(win: &crate::ipc::WindowInfo) -> Line<'_> {
    let matched = win.tracked.is_some();
    let mut spans = if matched {
        vec![
            Span::styled("● ", Style::default().fg(colors::UI_MATCHED)),
            Span::styled(
                truncate(&win.app_id, WINDOW_APPID_MAX_WIDTH),
                Style::default()
                    .fg(colors::UI_TEXT)
                    .add_modifier(Modifier::BOLD),
            ),
        ]
    } else {
        vec![
            Span::styled("○ ", Style::default().fg(colors::UI_UNMATCHED)),
            Span::styled(
                truncate(&win.app_id, WINDOW_APPID_MAX_WIDTH),
                Style::default().fg(colors::UI_SECONDARY),
            ),
        ]
    };

    // Add title inline if present
    if !win.title.is_empty() {
        spans.push(Span::styled(
            " · ",
            Style::default().add_modifier(Modifier::DIM),
        ));
        spans.push(Span::styled(
            truncate(&win.title, WINDOW_TITLE_MAX_WIDTH),
            Style::default().fg(colors::UI_SECONDARY),
        ));
    }

    if let Some(tracked) = &win.tracked {
        spans.push(Span::raw(" → "));
        spans.push(Span::styled(
            tracked.sink_desc.as_str(),
            Style::default().fg(colors::UI_HIGHLIGHT),
        ));
    }
    Line::from(spans)
}

/// Render window tracking section (full width bottom section when view is Windows)
fn render_window_tracking(
    frame: &mut Frame,
    area: Rect,
    windows: &[crate::ipc::WindowInfo],
    matched_count: usize,
    state: &mut DashboardScreen,
) {
    /// Rows above the list: the "Matched" line and a spacer
    const HEADER_ROWS: usize = 2;

    let title = " Window Tracking - [w] to toggle back to Logs ";
    let block = rounded_block()
        .title(title)
        .border_style(Style::default().fg(colors::UI_HIGHLIGHT));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let total = windows.len();
    let mut lines = vec![
        Line::from(vec![
            Span::styled("Matched: ", Style::default().fg(colors::UI_SECONDARY)),
            Span::styled(
                format!("{matched_count}/{total} windows"),
                Style::default()
                    .fg(colors::UI_STAT)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(""),
    ];

    if windows.is_empty() {
        lines.push(Line::from(Span::styled(
            "No windows tracked",
            Style::default().fg(colors::UI_SECONDARY),
        )));
    }

    // Matched windows first, then the rest
    let ordered = windows
        .iter()
        .filter(|w| w.tracked.is_some())
        .chain(windows.iter().filter(|w| w.tracked.is_none()));

    // Rows for the list; one is given up for the position indicator if it overflows
    let list_rows = usize::from(inner.height).saturating_sub(HEADER_ROWS);
    let overflows = total > list_rows;
    let visible = if overflows {
        list_rows.saturating_sub(1)
    } else {
        list_rows
    };
    state.window_page = visible;
    state.window_max_offset = total.saturating_sub(visible);
    state.window_scroll_offset = state.window_scroll_offset.min(state.window_max_offset);
    let start = state.window_scroll_offset;

    lines.extend(ordered.skip(start).take(visible).map(window_line));

    if overflows && visible > 0 {
        let end = (start + visible).min(total);
        lines.push(Line::from(vec![
            Span::styled(
                format!("  [{}-{end}/{total}] ", start + 1),
                Style::default().fg(colors::UI_STAT),
            ),
            Span::styled(
                "↑↓/PgUp/PgDn/Home/End to scroll",
                Style::default().fg(colors::UI_SECONDARY),
            ),
        ]));
    }

    frame.render_widget(Paragraph::new(lines), inner);
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
    matches.sort_by(|(start_a, end_a, ..), (start_b, end_b, ..)| {
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
            .any(|(f_start, f_end, ..)| start < *f_end && end > *f_start);
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

    // The level may be padded ("…Z  INFO msg")
    let Some((full_timestamp, rest)) = line.split_once(' ') else {
        return Line::from(Span::raw(line));
    };
    let Some((level, message)) = rest.trim_start().split_once(' ') else {
        // Malformed line, return as-is
        return Line::from(Span::raw(line));
    };

    // Extract time-only from ISO 8601 timestamp (HH:MM:SS)
    // Format: "2025-12-17T10:00:00.123456Z" -> "10:00:00". Character-based: the
    // line may contain anything, so never cut inside a character.
    let timestamp = full_timestamp
        .split_once('T')
        .map_or(full_timestamp, |(_, time)| {
            let time = time.split('.').next().unwrap_or(time);
            time.char_indices().nth(8).map_or(time, |(i, _)| &time[..i])
        });

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

/// Display width of one character (0 for control characters).
fn char_width(c: char) -> usize {
    let mut buf = [0u8; 4];
    Span::raw(&*c.encode_utf8(&mut buf)).width()
}

/// Split `line` into rows of at most `width` columns, breaking anywhere.
///
/// Wrapping by hand (instead of `Paragraph::wrap`) makes the number of rows
/// each log line takes exact, so the newest line can be kept on screen.
fn wrap_line(line: Line<'_>, width: usize) -> Vec<Line<'static>> {
    let width = width.max(1);
    let mut rows = Vec::new();
    let mut row: Vec<Span<'static>> = Vec::new();
    let mut row_width = 0;
    for span in line.spans {
        let mut text = String::new();
        for c in span.content.chars() {
            let w = char_width(c);
            if row_width + w > width && row_width > 0 {
                if !text.is_empty() {
                    row.push(Span::styled(std::mem::take(&mut text), span.style));
                }
                rows.push(Line::from(std::mem::take(&mut row)));
                row_width = 0;
            }
            text.push(c);
            row_width += w;
        }
        if !text.is_empty() {
            row.push(Span::styled(text, span.style));
        }
    }
    rows.push(Line::from(row));
    rows
}

/// The rows to show for `logs` in a `width`×`height` viewport, scrolled back
/// `offset` lines from the newest, plus the largest useful offset.
///
/// The newest shown line always ends on the last row; older lines fill the
/// rows above it (the oldest shown may be cut at the top).
fn log_rows(
    logs: &[String],
    offset: usize,
    width: usize,
    height: usize,
) -> (Vec<Line<'static>>, usize) {
    let rows_of = |line: &String| wrap_line(style_log_line(line), width);

    // Scrolling further back than the point where the oldest line reaches the
    // top would only leave empty rows at the top.
    let mut filled = 0;
    let mut first_full_end = logs.len();
    for (i, line) in logs.iter().enumerate() {
        filled += rows_of(line).len();
        if filled >= height {
            first_full_end = i + 1;
            break;
        }
    }
    let max_offset = logs.len() - first_full_end;

    let end = logs.len() - offset.min(max_offset);
    let mut rows: Vec<Line<'static>> = Vec::new();
    for line in logs[..end].iter().rev() {
        if rows.len() >= height {
            break;
        }
        let mut wrapped = rows_of(line);
        wrapped.append(&mut rows);
        rows = wrapped;
    }
    let excess = rows.len().saturating_sub(height);
    rows.drain(..excess);
    (rows, max_offset)
}

/// Render daemon log viewer
fn render_log_viewer(
    frame: &mut Frame,
    area: Rect,
    daemon_logs: &[String],
    daemon_running: bool,
    state: &mut DashboardScreen,
) {
    let (status_text, status_color) = if daemon_running {
        ("Live", colors::UI_SUCCESS)
    } else {
        ("Stopped", colors::UI_ERROR)
    };
    let border_color = if daemon_running {
        colors::UI_BORDER_ACTIVE
    } else {
        colors::UI_BORDER_INACTIVE
    };

    // Measure the viewport first: scroll limits depend on it
    let block: Block = rounded_block().border_style(Style::default().fg(border_color));
    let inner = block.inner(area);
    let width = usize::from(inner.width);
    let height = usize::from(inner.height);
    let (rows, max_offset) = log_rows(daemon_logs, state.log_scroll_offset, width, height);
    state.log_viewport = height;
    state.log_max_offset = max_offset;
    state.log_scroll_offset = state.log_scroll_offset.min(max_offset);
    let scroll_offset = state.log_scroll_offset;

    // Title with scroll position and toggle hint
    let mut title = vec![
        Span::raw(" Daemon Logs ("),
        Span::styled(status_text, Style::default().fg(status_color)),
        Span::raw(")"),
    ];
    if scroll_offset > 0 {
        title.push(Span::raw(" - "));
        title.push(Span::styled(
            format!("↑{scroll_offset} (End: live)"),
            Style::default().fg(colors::UI_WARNING),
        ));
    }
    title.push(Span::raw(" - [w] to toggle to Windows "));

    let log_text = if daemon_logs.is_empty() {
        vec![Line::from(Span::styled(
            "No logs available. Start the daemon to see logs here.",
            Style::default().fg(colors::UI_SECONDARY),
        ))]
    } else {
        rows
    };

    let paragraph = Paragraph::new(log_text).block(block.title(Line::from(title)));
    frame.render_widget(paragraph, area);
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    fn text(rows: &[Line]) -> Vec<String> {
        rows.iter().map(ToString::to_string).collect()
    }

    fn logs(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("l{i}")).collect()
    }

    #[test]
    fn test_log_rows_live_shows_newest_at_bottom() {
        let logs = logs(100);
        let (rows, max_offset) = log_rows(&logs, 0, 20, 5);
        assert_eq!(text(&rows), vec!["l95", "l96", "l97", "l98", "l99"]);
        assert_eq!(max_offset, 95);
    }

    #[test]
    fn test_log_rows_scrolled_and_clamped() {
        let logs = logs(100);
        let (rows, _) = log_rows(&logs, 10, 20, 3);
        assert_eq!(text(&rows), vec!["l87", "l88", "l89"]);
        // Scrolling past the oldest line stops with the oldest at the top
        let (rows, max_offset) = log_rows(&logs, 1000, 20, 3);
        assert_eq!(max_offset, 97);
        assert_eq!(text(&rows), vec!["l0", "l1", "l2"]);
    }

    #[test]
    fn test_log_rows_wrapped_line_keeps_newest_visible() {
        let mut logs = logs(10);
        logs.push("x".repeat(25)); // wraps into 3 rows of width 10
        logs.push("newest".to_string());
        let (rows, _) = log_rows(&logs, 0, 10, 4);
        assert_eq!(text(&rows), vec![
            "xxxxxxxxxx",
            "xxxxxxxxxx",
            "xxxxx",
            "newest"
        ]);
    }

    #[test]
    fn test_log_rows_fewer_lines_than_viewport() {
        let (rows, max_offset) = log_rows(&logs(3), 5, 20, 10);
        assert_eq!(max_offset, 0);
        assert_eq!(text(&rows), vec!["l0", "l1", "l2"]);
        let (rows, max_offset) = log_rows(&[], 0, 20, 10);
        assert_eq!(text(&rows), Vec::<String>::new());
        assert_eq!(max_offset, 0);
    }

    #[test]
    fn test_wrap_line_counts_wide_characters() {
        let rows = wrap_line(Line::from("日本語テキスト"), 4);
        assert_eq!(text(&rows), vec!["日本", "語テ", "キス", "ト"]);
        assert_eq!(wrap_line(Line::from(""), 4).len(), 1);
    }

    #[test]
    fn test_scroll_state_uses_rendered_limits() {
        let mut state = DashboardScreen::new();
        state.log_viewport = 7;
        state.log_max_offset = 10;
        assert_eq!(state.page_size(), 7);
        state.scroll_up(state.page_size());
        state.scroll_up(state.page_size());
        assert_eq!(state.log_scroll_offset, 10, "clamped to the oldest");
        state.scroll_down(3);
        assert_eq!(state.log_scroll_offset, 7);
        state.scroll_to_bottom();
        assert_eq!(state.log_scroll_offset, 0, "End returns to live");

        state.toggle_view();
        state.window_page = 4;
        state.window_max_offset = 6;
        state.scroll_down(state.page_size());
        assert_eq!(
            state.window_scroll_offset, 4,
            "PageDown moves down the list"
        );
        state.scroll_down(state.page_size());
        assert_eq!(state.window_scroll_offset, 6);
        state.scroll_up(1);
        assert_eq!(state.window_scroll_offset, 5);
        state.scroll_to_top();
        assert_eq!(state.window_scroll_offset, 0);
    }

    #[test]
    fn test_keep_log_position_when_scrolled_back() {
        let mut state = DashboardScreen::new();
        state.keep_log_position(5, 100);
        assert_eq!(state.log_scroll_offset, 0, "live view follows new lines");
        state.log_scroll_offset = 3;
        state.keep_log_position(5, 100);
        assert_eq!(state.log_scroll_offset, 8);
    }

    #[test]
    fn test_style_log_line_non_ascii_timestamp_does_not_panic() {
        for line in [
            "2025-12-17T10:00:0é.5Z INFO x",
            "xTé INFO y",
            "2025-12-17Tääääääääää INFO z",
            "日本T語語語語語語語語語.語 WARN ü",
        ] {
            let rendered = style_log_line(line);
            assert!(!rendered.spans.is_empty(), "{line}");
        }
        let line = style_log_line("2025-12-17Tääääääääää INFO z");
        assert_eq!(line.spans[0].content, "ääääääää");
    }

    #[test]
    fn test_style_log_line_padded_level() {
        let line = style_log_line("2025-12-17T10:00:00.1Z  INFO Starting");
        let spans: Vec<_> = line.spans.iter().map(|s| s.content.to_string()).collect();
        assert_eq!(spans[0], "10:00:00");
        assert_eq!(spans[2], "INFO");
    }

    #[test]
    fn test_truncate_utf8_window_titles() {
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

    // ============================================================================
    // Log Parsing Tests
    // ============================================================================

    #[test]
    fn test_style_log_line_valid_format() {
        // Standard log format: "TIMESTAMP LEVEL message"
        let line = style_log_line("2025-12-17T10:00:00.123456Z INFO Starting daemon");
        let spans: Vec<_> = line.spans.iter().map(|s| s.content.to_string()).collect();

        // Should extract timestamp (time only), level, and message
        assert!(spans[0].contains("10:00:00"), "Expected time extraction");
        assert!(spans[2].contains("INFO"), "Expected level");
        // Message spans come after level
        assert!(
            spans.iter().any(|s| s.contains("Starting")),
            "Expected message content"
        );
    }

    #[test]
    fn test_style_log_line_malformed() {
        // Less than 3 parts after split
        let line = style_log_line("malformed log");
        // Should return as-is without crashing
        assert_eq!(line.spans.len(), 1);
        assert_eq!(line.spans[0].content, "malformed log");

        // Single word
        let single = style_log_line("WARN");
        assert_eq!(single.spans.len(), 1);
        assert_eq!(single.spans[0].content, "WARN");

        // Empty string
        let empty = style_log_line("");
        assert_eq!(empty.spans.len(), 1);
    }

    #[test]
    fn test_style_log_line_timestamp_formats() {
        // Standard ISO 8601 with microseconds
        let line1 = style_log_line("2025-12-17T14:30:45.123456Z INFO test");
        let spans1: Vec<_> = line1.spans.iter().map(|s| s.content.to_string()).collect();
        assert_eq!(spans1[0], "14:30:45", "Should extract HH:MM:SS");

        // Without microseconds
        let line2 = style_log_line("2025-12-17T14:30:45Z INFO test");
        let spans2: Vec<_> = line2.spans.iter().map(|s| s.content.to_string()).collect();
        assert_eq!(spans2[0], "14:30:45", "Should extract HH:MM:SS");

        // Without 'T' separator (fallback to full timestamp)
        let line3 = style_log_line("20251217 INFO test");
        let spans3: Vec<_> = line3.spans.iter().map(|s| s.content.to_string()).collect();
        assert_eq!(
            spans3[0], "20251217",
            "Should use full timestamp as fallback"
        );
    }

    #[test]
    fn test_highlight_message_patterns() {
        // Test pattern highlighting
        let spans = highlight_message("Rule matched: firefox -> headphones");
        // Should have highlighted "Rule matched:" portion
        assert!(
            spans.len() > 1,
            "Should split message into highlighted parts"
        );

        // Test multiple patterns in one message
        let spans2 = highlight_message("Window opened: app_id=firefox title=Browser");
        // Should highlight "Window opened:", "app_id=", and "title="
        assert!(
            spans2.len() >= 3,
            "Should have multiple highlighted segments"
        );
    }

    #[test]
    fn test_highlight_message_no_patterns() {
        // Message with no highlight patterns
        let spans = highlight_message("Just a plain message");
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].content, "Just a plain message");
    }

    #[test]
    fn test_highlight_message_overlapping_patterns() {
        // "Loaded" and "rules" are both patterns - they shouldn't overlap
        let spans = highlight_message("Loaded 5 sinks, 3 rules");
        // Should handle without crashing
        assert!(!spans.is_empty(), "expected at least one span");
        // The full message content should be preserved
        let full: String = spans.iter().map(|s| s.content.to_string()).collect();
        assert_eq!(full, "Loaded 5 sinks, 3 rules");
    }

    #[test]
    fn test_style_log_line_all_levels() {
        // Test all log levels are parsed correctly
        for level in &["TRACE", "DEBUG", "INFO", "WARN", "ERROR"] {
            let log_str = format!("2025-01-01T00:00:00Z {level} test");
            let line = style_log_line(&log_str);
            let spans: Vec<_> = line.spans.iter().map(|s| s.content.to_string()).collect();
            assert!(
                spans.iter().any(|s| s.trim() == *level),
                "Level {level} should be parsed"
            );
        }
    }
}
