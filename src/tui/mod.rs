//! Terminal User Interface (TUI) for PWSW
//!
//! Provides an interactive terminal interface for managing PWSW configuration,
//! monitoring daemon status, and controlling sinks.
//!
//! Rendering never performs I/O: everything shown comes from [`App`], which the
//! background worker ([`worker`]) keeps up to date. The event loop sleeps until
//! an input event or a worker update arrives; it only wakes periodically while a
//! spinner is animating.

use std::sync::Arc;
use std::time::Duration;

use color_eyre::eyre::{self, Result};
use crossterm::event::EventStream;
use futures_util::StreamExt;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{BorderType, Paragraph, Tabs};
use ratatui::{Frame, Terminal};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use tokio::time::{Instant, MissedTickBehavior};

use crate::style::colors;
use crate::tui::app::{AppUpdate, BgCommand};

mod app;
mod daemon_control;
mod editor_state;
mod input;
mod keymap;
mod log_tailer;
mod preview;
mod screens;
mod terminal;
mod widgets;
mod worker;

#[cfg(test)]
mod tests;

use app::{App, Screen};
use input::handle_event;
use screens::{
    RulesRenderContext, SinksRenderContext, render_dashboard, render_help, render_rules,
    render_settings, render_sinks,
};

/// Capacity of the UI → worker command queue (commands are rare, user-driven).
const COMMAND_QUEUE: usize = 16;
/// Minimum time between two frames (caps rendering at ~60 FPS).
const MIN_FRAME_TIME: Duration = Duration::from_millis(16);
/// Spinner frame interval while background work is pending.
const ANIM_INTERVAL: Duration = Duration::from_millis(120);

/// Aborts the background worker when dropped (any exit path of [`run`]).
struct AbortOnDrop(JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Whether `screen` shows `PipeWire` sink state (the worker only takes
/// snapshots while one does).
const fn shows_sinks(screen: Screen) -> bool {
    matches!(screen, Screen::Dashboard | Screen::Sinks)
}

/// Run the TUI application
///
/// # Errors
/// Returns an error if the config cannot be loaded, the terminal cannot be set
/// up or drawn to, or a background task panicked.
pub async fn run() -> Result<()> {
    // Everything that may print or block happens before the terminal is taken
    // over, so first-run messages appear normally and Ctrl+C still works. The
    // TUI is the one place that creates the commented default config.
    let config_path = crate::config::Config::get_config_path()?;
    let config = crate::config::Config::load_or_init()?;
    let mut app = App::new(config, config_path);

    // Enable/disable are offered whenever the systemd unit is installed
    let unit_installed = crate::daemon_manager::DaemonManager::unit_installed_cached().await;
    app.dashboard_screen.set_max_actions(unit_installed);

    let logs = match log_tailer::LogTailer::new() {
        Ok(mut tailer) => {
            // File I/O: off the runtime
            match worker::detached(move || {
                let initial = tailer.read_initial(100);
                (tailer, initial)
            })
            .await
            {
                Ok((tailer, initial)) => {
                    match initial {
                        Ok(lines) if !lines.is_empty() => {
                            app.handle_update(AppUpdate::DaemonLogs(lines));
                        }
                        Ok(_) => {}
                        Err(e) => app.set_status(format!("Could not read daemon log: {e:#}")),
                    }
                    Some(tailer)
                }
                Err(e) => {
                    app.set_status(format!("Daemon log unavailable: {e:#}"));
                    None
                }
            }
        }
        Err(e) => {
            app.set_status(format!("Daemon log unavailable: {e:#}"));
            None
        }
    };

    let (update_tx, update_rx) = mpsc::unbounded_channel::<AppUpdate>();
    let (cmd_tx, cmd_rx) = mpsc::channel::<BgCommand>(COMMAND_QUEUE);
    let (visible_tx, visible_rx) = watch::channel(shows_sinks(app.current_screen));
    app.bg_cmd_tx = Some(cmd_tx);
    let mut worker = AbortOnDrop(tokio::spawn(worker::run(
        Arc::new(worker::System),
        cmd_rx,
        update_tx,
        visible_rx,
        logs,
    )));

    let mut session = terminal::TerminalSession::enter()?;
    let result = run_app(
        &mut session.terminal,
        &mut app,
        update_rx,
        &visible_tx,
        &mut worker.0,
    )
    .await;
    // Restore the terminal before anything else can print.
    drop(session);
    result
}

/// Main application loop
///
/// Draws when the state changed (at most every [`MIN_FRAME_TIME`]) and otherwise
/// sleeps until an input event, a worker update or — only while work is
/// pending — a spinner tick.
async fn run_app<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    app: &mut App,
    mut updates: mpsc::UnboundedReceiver<AppUpdate>,
    sinks_visible: &watch::Sender<bool>,
    worker: &mut JoinHandle<()>,
) -> Result<()>
where
    B::Error: Send + Sync + 'static,
{
    let mut events = EventStream::new();
    let mut anim = tokio::time::interval(ANIM_INTERVAL);
    anim.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut last_frame: Option<Instant> = None;
    app.dirty = true;

    loop {
        if terminal::panic_seen() {
            eyre::bail!("a background task panicked (see the report above)");
        }

        // Draw if needed and allowed by the frame cap; otherwise remember when.
        let mut redraw_at = None;
        if app.dirty {
            let now = Instant::now();
            match last_frame.map(|t| t + MIN_FRAME_TIME) {
                Some(next) if next > now => redraw_at = Some(next),
                _ => {
                    app.refresh_preview();
                    terminal.draw(|frame| render_ui(frame, app))?;
                    app.dirty = false;
                    last_frame = Some(now);
                }
            }
        }

        if app.should_quit {
            return Ok(());
        }

        let animating = app.is_busy();
        let quit_deadline = app.quit_save_deadline();
        tokio::select! {
            () = tokio::time::sleep_until(redraw_at.unwrap_or_else(Instant::now)),
                if redraw_at.is_some() => {}
            // A quit waiting for an in-flight save gives up (the app stays open)
            () = tokio::time::sleep_until(quit_deadline.unwrap_or_else(Instant::now)),
                if quit_deadline.is_some() => app.quit_save_wait_expired(),
            _ = anim.tick(), if animating => app.advance_throbber(),
            event = events.next() => match event {
                Some(Ok(event)) => {
                    handle_event(app, &event);
                    let shows = shows_sinks(app.current_screen);
                    sinks_visible.send_if_modified(|v| std::mem::replace(v, shows) != shows);
                }
                Some(Err(e)) => {
                    return Err(eyre::Report::new(e).wrap_err("Failed to read terminal input"));
                }
                None => return Ok(()), // input closed
            },
            update = updates.recv() => match update {
                Some(update) => app.handle_update(update),
                // The worker dropped its sender: report how it ended
                None => return Err(worker_ended((&mut *worker).await)),
            },
            joined = &mut *worker => return Err(worker_ended(joined)),
        }
    }
}

/// The error for a background worker that ended while the UI was running.
fn worker_ended(joined: Result<(), tokio::task::JoinError>) -> eyre::Report {
    if joined.is_err_and(|e| e.is_panic()) {
        eyre::eyre!("background worker panicked (see the report above)")
    } else {
        eyre::eyre!("background worker stopped")
    }
}

/// Render the complete UI
pub(crate) fn render_ui(frame: &mut ratatui::Frame, app: &mut App) {
    // Cap TUI width for better proportions on wide screens
    // This prevents content from stretching excessively and looking lost
    const MAX_TUI_WIDTH: u16 = 180;

    let size = frame.area();
    let content_area = if size.width > MAX_TUI_WIDTH {
        let margin = (size.width - MAX_TUI_WIDTH) / 2;
        Rect {
            x: size.x + margin,
            y: size.y,
            width: MAX_TUI_WIDTH,
            height: size.height,
        }
    } else {
        size
    };

    // Get context bar content
    let context_text = app.context_bar_text();

    // Create main layout: [Header (tabs) | Content | Context Bar | Footer (status)]
    let [header_area, main_area, ctx_bar_area, footer_area] = Layout::vertical([
        Constraint::Length(3), // Header with tabs
        Constraint::Min(0),    // Content area (now gets more space)
        Constraint::Length(3), // Context bar with border (fixed 3 lines)
        Constraint::Length(1), // Footer (temporary, for status - Phase 1 only)
    ])
    .areas(content_area);

    // Render header (tab bar)
    render_header(frame, header_area, app.current_screen);

    // Render context bar (at bottom, with border)
    render_context_bar(frame, ctx_bar_area, context_text);

    // Render screen content
    match app.current_screen {
        Screen::Dashboard => {
            let mut ctx = screens::DashboardRenderContext {
                config: &app.config,
                screen_state: &mut app.dashboard_screen,
                daemon_running: app.daemon_running,
                window_count: app.window_count,
                daemon_logs: &app.daemon_log_lines,
                windows: &app.windows,
                active_sinks: &app.active_sink_list,
                pipewire_available: app.pipewire_available,
            };
            render_dashboard(frame, main_area, &mut ctx);
        }
        Screen::Sinks => render_sinks(frame, main_area, &mut SinksRenderContext {
            sinks: &app.config.sinks,
            screen_state: &mut app.sinks_screen,
            active_sinks: &app.active_sinks,
            active_sink_list: &app.active_sink_list,
            profile_sink_list: &app.profile_sink_list,
            pipewire_available: app.pipewire_available,
        }),
        Screen::Rules => render_rules(frame, main_area, &mut RulesRenderContext {
            rules: &app.config.rules,
            sinks: &app.config.sinks,
            screen_state: &mut app.rules_screen,
            preview: app.preview.view(),
            daemon_running: app.daemon_running,
        }),
        Screen::Settings => render_settings(
            frame,
            main_area,
            &app.config.settings,
            &mut app.settings_screen,
        ),
    }

    // Render footer (status message, spinner while background work is pending)
    let (status, busy, throbber) = app.footer_parts();
    render_footer(frame, footer_area, status, busy, throbber);

    if app.save_conflict {
        render_conflict_dialog(frame, content_area);
    }

    // Render help overlay on top if active
    if app.show_help {
        render_help(
            frame,
            content_area,
            app.current_screen,
            &mut app.help_scroll_state,
            &mut app.help_viewport_height,
            &app.help_collapsed_sections,
        );
    }
}

/// Render the "config changed on disk" dialog shown when a save was refused
fn render_conflict_dialog(frame: &mut Frame, area: Rect) {
    let key = |k: &'static str| Span::styled(k, Style::default().fg(colors::UI_HIGHLIGHT));
    let lines = vec![
        Line::from(""),
        Line::from(Span::styled(
            "The config file changed on disk since it was loaded.",
            Style::default()
                .fg(colors::UI_WARNING)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from("Saving now would overwrite those changes."),
        Line::from(""),
        Line::from(vec![
            key("[o]"),
            Span::raw(" Overwrite the file with your changes"),
        ]),
        Line::from(vec![
            key("[r]"),
            Span::raw(" Reload from disk (discards your unsaved edits)"),
        ]),
        Line::from(vec![
            key("[Esc]"),
            Span::raw(" Cancel (keep editing, nothing saved)"),
        ]),
    ];
    widgets::render_text_modal(
        frame,
        area,
        " Config Changed on Disk ",
        BorderType::Double,
        lines,
    );
}

/// Render the header with tab navigation
fn render_header(frame: &mut ratatui::Frame, area: Rect, current_screen: Screen) {
    // Build tab titles with gray styling (highlight_style will override for selected)
    let titles: Vec<Line> = Screen::all()
        .iter()
        .map(|s| {
            let name = s.name();
            let mut t = String::with_capacity(1 + 1 + 1 + name.len()); // +1 for space
            t.push('[');
            t.push(s.key());
            t.push(']');
            t.push(' '); // Add space
            t.push_str(name);
            Line::styled(t, Style::default().fg(colors::UI_SECONDARY))
        })
        .collect();

    let selected = Screen::all()
        .iter()
        .position(|&s| s == current_screen)
        .unwrap_or(0);

    // Build left title (version only - save indicator moved to context bar)
    let version = crate::version_string();
    let left_title = Line::from(vec![Span::raw("PWSW "), Span::raw(&version)]);

    // Minimal right title
    let right_title = Line::from(vec![
        Span::styled("[F1/?]", Style::default().fg(colors::UI_HIGHLIGHT)),
        Span::raw(" Help "),
    ])
    .alignment(Alignment::Right);

    let tabs = Tabs::new(titles)
        .block(
            widgets::rounded_block()
                .title(left_title)
                .title_top(right_title),
        )
        .select(selected)
        .divider(Span::styled(
            "·",
            Style::default().add_modifier(Modifier::DIM),
        ))
        .highlight_style(
            Style::default()
                .fg(colors::UI_FOCUS)
                .add_modifier(Modifier::BOLD),
        );

    frame.render_widget(tabs, area);
}

/// Render the context bar with border at bottom of screen
fn render_context_bar(frame: &mut Frame, area: Rect, content: Line) {
    let block = widgets::rounded_block().title_bottom(
        Line::from(vec![
            Span::styled("[q]", Style::default().fg(colors::UI_HIGHLIGHT)),
            Span::raw(" Quit "),
        ])
        .alignment(Alignment::Right),
    );

    let paragraph = Paragraph::new(content)
        .block(block)
        .style(Style::default().fg(colors::UI_TEXT));

    frame.render_widget(paragraph, area);
}

/// Render the footer with status message (Phase 1: simplified, quit hint moved to context bar)
fn render_footer(
    frame: &mut ratatui::Frame,
    area: Rect,
    status_message: Option<&str>,
    busy: bool,
    throbber_state: &mut throbber_widgets_tui::ThrobberState,
) {
    use throbber_widgets_tui::Throbber;

    // Split footer: [Throbber (3) | Status (Min 0)]
    let [throbber_area, status_area] = Layout::horizontal([
        Constraint::Length(3), // Throbber/Dot area
        Constraint::Min(0),    // Status message
    ])
    .areas(area);

    // 1. Throbber/Status Dot
    if busy {
        let throb = Throbber::default().style(Style::default().fg(colors::UI_WARNING));
        frame.render_stateful_widget(throb, throbber_area, throbber_state);
    } else if status_message.is_some() {
        let dot = Paragraph::new(Span::styled("● ", Style::default().fg(colors::UI_WARNING)));
        frame.render_widget(dot, throbber_area);
    }

    // 2. Status Message
    let status_text = if let Some(msg) = status_message {
        Span::styled(msg, Style::default().fg(colors::UI_TEXT))
    } else {
        Span::raw("")
    };
    frame.render_widget(Paragraph::new(status_text), status_area);
}
