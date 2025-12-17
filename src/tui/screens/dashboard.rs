//! Dashboard screen - Overview and quick actions

use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph},
    Frame,
};

use crate::config::Config;

/// Dashboard screen state
pub(crate) struct DashboardScreen {
    pub selected_action: usize, // 0 = start, 1 = stop, 2 = restart
}

impl DashboardScreen {
    pub(crate) fn new() -> Self {
        Self { selected_action: 0 }
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
            Constraint::Length(8), // Daemon status + controls
            Constraint::Length(10), // Info cards (reduced from Min(0))
            Constraint::Min(0),    // Daemon logs
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
    render_log_viewer(frame, chunks[2], daemon_logs, daemon_running);
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
        ("RUNNING", Color::Green, "●")
    } else {
        ("STOPPED", Color::Red, "○")
    };

    let status_content = vec![
        Line::from(""),
        Line::from(vec![Span::styled(
            "Daemon Status",
            Style::default().fg(Color::Gray),
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
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
                    .bg(Color::DarkGray)
            } else {
                Style::default().fg(Color::White)
            };

            let prefix = if is_selected { " ▶ " } else { "   " };
            ListItem::new(Line::from(vec![
                Span::styled(prefix, Style::default().fg(Color::Cyan)),
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
            Span::styled(sink_name, Style::default().fg(Color::Cyan)),
            Span::raw(" "),
            Span::styled(
                sink_desc,
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "Active Audio Output",
            Style::default().fg(Color::Gray),
        )),
    ];

    let paragraph = Paragraph::new(text)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Active Sink ")
                .border_style(Style::default().fg(Color::Cyan)),
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
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" windows"),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "Currently Tracked",
            Style::default().fg(Color::Gray),
        )),
    ];

    let paragraph = Paragraph::new(text)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Statistics ")
                .border_style(Style::default().fg(Color::Yellow)),
        )
        .alignment(Alignment::Center);

    frame.render_widget(paragraph, area);
}

/// Render daemon log viewer
fn render_log_viewer(
    frame: &mut Frame,
    area: Rect,
    daemon_logs: &[String],
    daemon_running: bool,
) {
    let title = if daemon_running {
        " Daemon Logs (Live) "
    } else {
        " Daemon Logs (Stopped) "
    };

    let border_color = if daemon_running {
        Color::Green
    } else {
        Color::Gray
    };

    // Show last N lines that fit in the area
    let available_height = area.height.saturating_sub(2) as usize; // Account for borders
    let start_index = daemon_logs.len().saturating_sub(available_height);
    let visible_logs: Vec<Line> = daemon_logs
        .iter()
        .skip(start_index)
        .map(|line| {
            // Simple log line styling: dim for timestamps, normal for the rest
            if line.contains("INFO") {
                Line::from(Span::styled(line, Style::default().fg(Color::Gray)))
            } else if line.contains("WARN") {
                Line::from(Span::styled(line, Style::default().fg(Color::Yellow)))
            } else if line.contains("ERROR") {
                Line::from(Span::styled(line, Style::default().fg(Color::Red)))
            } else {
                Line::from(Span::raw(line))
            }
        })
        .collect();

    let log_text = if visible_logs.is_empty() {
        vec![Line::from(Span::styled(
            "No logs available. Start the daemon to see logs here.",
            Style::default().fg(Color::Gray),
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
