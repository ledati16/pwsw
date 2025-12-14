//! Dashboard screen - Overview and quick actions

use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Margin, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph},
    Frame,
};

use crate::config::Config;

/// Dashboard screen state
pub struct DashboardScreen {
    pub selected_action: usize, // 0 = start, 1 = stop, 2 = restart
}

impl DashboardScreen {
    pub fn new() -> Self {
        Self { selected_action: 0 }
    }

    pub fn select_next(&mut self) {
        if self.selected_action < 2 {
            self.selected_action += 1;
        }
    }

    pub fn select_previous(&mut self) {
        if self.selected_action > 0 {
            self.selected_action -= 1;
        }
    }
}

/// Render the dashboard screen
pub fn render_dashboard(
    frame: &mut Frame,
    area: Rect,
    config: &Config,
    screen_state: &DashboardScreen,
    daemon_running: bool,
    window_count: usize,
) {
    // Split screen into sections
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(7), // Daemon status + controls
            Constraint::Length(5), // Current sink
            Constraint::Length(5), // Active windows
            Constraint::Min(0),    // Quick actions (future)
        ])
        .split(area);

    // Daemon Status Section
    render_daemon_status(frame, chunks[0], screen_state, daemon_running);

    // Current Sink Section
    render_current_sink(frame, chunks[1], config);

    // Active Windows Section
    render_active_windows(frame, chunks[2], window_count);
}

/// Render daemon status widget with control buttons
fn render_daemon_status(
    frame: &mut Frame,
    area: Rect,
    screen_state: &DashboardScreen,
    daemon_running: bool,
) {
    let (status_text, status_color) = if daemon_running {
        ("Running", Color::Green)
    } else {
        ("Not Running", Color::Red)
    };

    // Render outer block FIRST
    let block = Block::default()
        .borders(Borders::ALL)
        .title("PWSW Daemon ([↑/↓]select [Enter]execute)");
    frame.render_widget(block, area);

    // Get inner area for content (accounting for borders)
    let inner = area.inner(Margin {
        vertical: 1,
        horizontal: 1,
    });

    // Create horizontal layout: [Status | Controls]
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(inner);

    // Left: Status
    let status_text_widget = vec![
        Line::from(""),
        Line::from(vec![
            Span::styled("Status: ", Style::default().fg(Color::Cyan)),
            Span::styled(
                status_text,
                Style::default()
                    .fg(status_color)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
    ];

    let status_paragraph = Paragraph::new(status_text_widget).alignment(Alignment::Left);
    frame.render_widget(status_paragraph, chunks[0]);

    // Right: Control buttons
    let actions = ["Start", "Stop", "Restart"];
    let items: Vec<ListItem> = actions
        .iter()
        .enumerate()
        .map(|(i, action)| {
            let is_selected = i == screen_state.selected_action;
            let style = if is_selected {
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            };

            let prefix = if is_selected { "> " } else { "  " };
            ListItem::new(Line::from(vec![
                Span::styled(prefix, Style::default().fg(Color::Cyan)),
                Span::styled(*action, style),
            ]))
        })
        .collect();

    let controls_list = List::new(items);
    frame.render_widget(controls_list, chunks[1]);
}

/// Render current sink widget
fn render_current_sink(frame: &mut Frame, area: Rect, config: &Config) {
    let current_sink_name = crate::pipewire::PipeWire::get_default_sink_name().ok();

    let sink_desc = current_sink_name
        .as_ref()
        .and_then(|name| {
            config
                .sinks
                .iter()
                .find(|s| &s.name == name)
                .map(|s| s.desc.as_str())
        })
        .unwrap_or("Unknown");

    let text = vec![
        Line::from(""),
        Line::from(vec![
            Span::styled("Active Sink: ", Style::default().fg(Color::Cyan)),
            Span::styled(
                sink_desc,
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
    ];

    let paragraph = Paragraph::new(text)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("Current Audio Output"),
        )
        .alignment(Alignment::Left);

    frame.render_widget(paragraph, area);
}

/// Render active windows widget
fn render_active_windows(frame: &mut Frame, area: Rect, window_count: usize) {
    let text = vec![
        Line::from(""),
        Line::from(vec![
            Span::styled("Tracked Windows: ", Style::default().fg(Color::Cyan)),
            Span::styled(
                window_count.to_string(),
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
    ];

    let paragraph = Paragraph::new(text)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("Window Tracking"),
        )
        .alignment(Alignment::Left);

    frame.render_widget(paragraph, area);
}
