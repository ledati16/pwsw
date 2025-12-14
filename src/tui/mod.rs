//! Terminal User Interface (TUI) for PWSW
//!
//! Provides an interactive terminal interface for managing PWSW configuration,
//! monitoring daemon status, and controlling sinks.

use anyhow::{Context, Result};
use crossterm::{
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Tabs},
    Terminal,
};
use std::io;

mod app;
mod daemon_control;
mod input;
mod screens;
mod widgets;

use app::{App, Screen};
use input::handle_events;
use screens::{render_dashboard, render_help, render_rules, render_settings, render_sinks};

/// Run the TUI application
///
/// # Errors
/// Returns an error if TUI initialization fails or terminal operations fail.
pub async fn run() -> Result<()> {
    // Initialize terminal
    enable_raw_mode().context("Failed to enable raw mode")?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen).context("Failed to enter alternate screen")?;

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend).context("Failed to create terminal")?;

    // Create app state
    let mut app = App::new()?;

    // Main event loop
    let result = run_app(&mut terminal, &mut app).await;

    // Restore terminal
    disable_raw_mode().context("Failed to disable raw mode")?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)
        .context("Failed to leave alternate screen")?;
    terminal.show_cursor().context("Failed to show cursor")?;

    result
}

/// Main application loop
async fn run_app<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    app: &mut App,
) -> Result<()> {
    loop {
        // Execute any pending daemon action first
        app.execute_pending_daemon_action().await;

        // Update daemon state before rendering (async operation)
        app.update_daemon_state().await;

        // Render UI
        terminal.draw(|frame| render_ui(frame, app))?;

        // Handle input events
        handle_events(app)?;

        // Check if we should quit
        if app.should_quit {
            break;
        }
    }

    Ok(())
}

/// Render the complete UI
fn render_ui(frame: &mut ratatui::Frame, app: &App) {
    let size = frame.area();

    // Create main layout: [Header (tabs) | Content | Footer (status)]
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),  // Header with tabs
            Constraint::Min(0),     // Content area
            Constraint::Length(1),  // Footer
        ])
        .split(size);

    // Render header (tab bar)
    render_header(frame, chunks[0], app.current_screen, app.config_dirty);

    // Render screen content
    match app.current_screen {
        Screen::Dashboard => render_dashboard(frame, chunks[1], &app.config, &app.dashboard_screen, app.daemon_running, app.window_count),
        Screen::Sinks => render_sinks(frame, chunks[1], &app.config.sinks, &app.sinks_screen),
        Screen::Rules => render_rules(frame, chunks[1], &app.config.rules, &app.config.sinks, &app.rules_screen, &app.windows),
        Screen::Settings => render_settings(frame, chunks[1], &app.config.settings, &app.settings_screen),
    }

    // Render footer
    render_footer(frame, chunks[2], &app.status_message);

    // Render help overlay on top if active
    if app.show_help {
        render_help(frame, size, app.current_screen);
    }
}

/// Render the header with tab navigation
fn render_header(frame: &mut ratatui::Frame, area: Rect, current_screen: Screen, config_dirty: bool) {
    let titles: Vec<_> = Screen::all()
        .iter()
        .map(|s| format!("[{}]{}", s.key(), s.name()))
        .collect();

    let selected = Screen::all()
        .iter()
        .position(|&s| s == current_screen)
        .unwrap_or(0);

    // Build title with unsaved indicator if needed
    let title = if config_dirty {
        format!("PWSW {} [unsaved]", crate::version_string())
    } else {
        format!("PWSW {}", crate::version_string())
    };

    let tabs = Tabs::new(titles)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(title),
        )
        .select(selected)
        .style(Style::default().fg(Color::White))
        .highlight_style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        );

    frame.render_widget(tabs, area);
}

/// Render the footer with keyboard shortcuts and status
fn render_footer(frame: &mut ratatui::Frame, area: Rect, status_message: &Option<String>) {
    let text = if let Some(msg) = status_message {
        Line::from(vec![
            Span::styled("● ", Style::default().fg(Color::Yellow)),
            Span::styled(msg, Style::default().fg(Color::White)),
        ])
    } else {
        Line::from(vec![
            Span::raw("[q] Quit  "),
            Span::styled("[?]", Style::default().fg(Color::Cyan)),
            Span::raw(" Help  [Tab] Next  "),
            Span::styled("[d]", Style::default().fg(Color::Cyan)),
            Span::raw("ashboard  "),
            Span::styled("[s]", Style::default().fg(Color::Cyan)),
            Span::raw("inks  "),
            Span::styled("[r]", Style::default().fg(Color::Cyan)),
            Span::raw("ules  Se"),
            Span::styled("[t]", Style::default().fg(Color::Cyan)),
            Span::raw("tings  "),
            Span::styled("Ctrl+S", Style::default().fg(Color::Green)),
            Span::raw(" Save"),
        ])
    };

    let footer = Paragraph::new(text);
    frame.render_widget(footer, area);
}

