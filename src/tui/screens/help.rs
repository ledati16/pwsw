//! Help overlay - Context-aware keyboard shortcut reference

use ratatui::{
    layout::{Alignment, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
    Frame,
};

use crate::tui::app::Screen;
use crate::tui::widgets::{centered_modal, modal_size};

/// Render help overlay on top of the current screen
pub fn render_help(frame: &mut Frame, area: Rect, current_screen: Screen) {
    // Create centered modal
    let popup_area = centered_modal(modal_size::HELP, area);

    // Clear background to prevent bleed-through from underlying screens
    frame.render_widget(Clear, popup_area);

    // Build help content based on current screen
    let help_lines = match current_screen {
        Screen::Dashboard => get_dashboard_help(),
        Screen::Sinks => get_sinks_help(),
        Screen::Rules => get_rules_help(),
        Screen::Settings => get_settings_help(),
    };

    // Add global shortcuts at the end
    let mut all_lines = help_lines;
    all_lines.push(Line::from(""));
    all_lines.push(Line::from(vec![Span::styled(
        "Global Shortcuts",
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    )]));
    all_lines.extend(get_global_shortcuts());
    all_lines.push(Line::from(""));
    all_lines.push(Line::from(vec![
        Span::styled("Press ", Style::default().fg(Color::Gray)),
        Span::styled(
            "?",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" or ", Style::default().fg(Color::Gray)),
        Span::styled(
            "Esc",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" to close help", Style::default().fg(Color::Gray)),
    ]));

    let block = Block::default()
        .borders(Borders::ALL)
        .title("Help")
        .style(Style::default().bg(Color::Black).fg(Color::White));

    let paragraph = Paragraph::new(all_lines)
        .block(block)
        .alignment(Alignment::Left);

    frame.render_widget(paragraph, popup_area);
}

/// Dashboard screen shortcuts
fn get_dashboard_help() -> Vec<Line<'static>> {
    vec![
        Line::from(vec![Span::styled(
            "Dashboard Screen",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )]),
        Line::from(""),
        help_line("↑/↓", "Navigate daemon control actions"),
        help_line("Enter", "Execute selected action (Start/Stop/Restart)"),
    ]
}

/// Sinks screen shortcuts
fn get_sinks_help() -> Vec<Line<'static>> {
    vec![
        Line::from(vec![Span::styled(
            "Sinks Screen",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )]),
        Line::from(""),
        help_line("↑/↓", "Navigate sinks"),
        help_line("a", "Add new sink"),
        help_line("e", "Edit selected sink"),
        help_line("x", "Delete selected sink"),
        help_line("Space", "Toggle default status"),
        Line::from(""),
        Line::from(vec![Span::styled(
            "In Editor (Add/Edit)",
            Style::default().fg(Color::Yellow),
        )]),
        help_line("Tab", "Next field"),
        help_line("Shift+Tab", "Previous field"),
        help_line("Space", "Toggle default checkbox (on checkbox field)"),
        help_line("Enter", "Save"),
        help_line("Esc", "Cancel"),
    ]
}

/// Rules screen shortcuts
fn get_rules_help() -> Vec<Line<'static>> {
    vec![
        Line::from(vec![Span::styled(
            "Rules Screen",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )]),
        Line::from(""),
        help_line("↑/↓", "Navigate rules"),
        help_line("a", "Add new rule"),
        help_line("e", "Edit selected rule"),
        help_line("x", "Delete selected rule"),
        Line::from(""),
        Line::from(vec![Span::styled(
            "In Editor (Add/Edit)",
            Style::default().fg(Color::Yellow),
        )]),
        help_line("Tab", "Next field"),
        help_line("Shift+Tab", "Previous field"),
        help_line("Space", "Cycle notify option (on notify field)"),
        help_line("Enter", "Save / Open sink selector (on sink field)"),
        help_line("Esc", "Cancel"),
        Line::from(""),
        Line::from(vec![
            Span::styled("Live Preview", Style::default().fg(Color::Green)),
            Span::raw(": Shows matching windows as you type regex patterns"),
        ]),
    ]
}

/// Settings screen shortcuts
fn get_settings_help() -> Vec<Line<'static>> {
    vec![
        Line::from(vec![Span::styled(
            "Settings Screen",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )]),
        Line::from(""),
        help_line("↑/↓", "Navigate settings"),
        help_line("Enter/Space", "Toggle setting / Open log level dropdown"),
        Line::from(""),
        Line::from(vec![Span::styled(
            "In Log Level Dropdown",
            Style::default().fg(Color::Yellow),
        )]),
        help_line("↑/↓", "Navigate log levels"),
        help_line("Enter", "Confirm selection"),
        help_line("Esc", "Cancel"),
    ]
}

/// Global shortcuts available on all screens
fn get_global_shortcuts() -> Vec<Line<'static>> {
    vec![
        help_line("q/Ctrl+C", "Quit application"),
        help_line("Tab", "Next screen"),
        help_line("Shift+Tab", "Previous screen"),
        help_line("d", "Go to Dashboard"),
        help_line("s", "Go to Sinks"),
        help_line("r", "Go to Rules"),
        help_line("t", "Go to Settings"),
        help_line("Ctrl+S", "Save configuration"),
        help_line("Esc", "Clear status message"),
        help_line("?", "Toggle help"),
    ]
}

/// Helper to create a formatted help line
fn help_line(key: &'static str, description: &'static str) -> Line<'static> {
    // Use fixed-width key column (12 chars) for proper alignment
    // format! is acceptable here since help renders on-demand, not every frame
    let padded_key = format!("{:<12}", key);
    Line::from(vec![
        Span::raw("  "),
        Span::styled(padded_key, Style::default().fg(Color::Green)),
        Span::raw(" - "),
        Span::raw(description),
    ])
}
