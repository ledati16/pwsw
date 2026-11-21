//! Terminal User Interface (TUI) for PWSW
//!
//! Provides an interactive terminal interface for managing PWSW configuration,
//! monitoring daemon status, and controlling sinks.
//!
//! Rendering never performs I/O: everything shown comes from `App`, which the
//! background worker (`worker`) keeps up to date. The event loop sleeps until
//! an input event or a worker update arrives; it only wakes periodically while a
//! spinner is animating.

use std::sync::Arc;
use std::time::Duration;

use color_eyre::eyre::{self, Result};
use crossterm::event::EventStream;
use futures_util::StreamExt;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::{Frame, Terminal};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use tokio::time::{Instant, MissedTickBehavior};

use crate::tui::app::{AppUpdate, BgCommand, StatusKind};

mod app;
mod chrome;
mod daemon_control;
mod editor_state;
mod input;
mod keymap;
mod log_tailer;
mod preview;
mod screens;
mod terminal;
mod theme;
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
                        Err(e) => app.set_status(
                            StatusKind::Warning,
                            format!("Could not read daemon log: {e:#}"),
                        ),
                    }
                    Some(tailer)
                }
                Err(e) => {
                    app.set_status(
                        StatusKind::Warning,
                        format!("Daemon log unavailable: {e:#}"),
                    );
                    None
                }
            }
        }
        Err(e) => {
            app.set_status(
                StatusKind::Warning,
                format!("Daemon log unavailable: {e:#}"),
            );
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
        true,
    )
    .await;
    // Restore the terminal before anything else can print.
    drop(session);
    result
}

/// Main application loop
///
/// Draws when the state changed (at most every [`MIN_FRAME_TIME`]) and otherwise
/// sleeps until an input event, a worker update, a status message expiring or —
/// only while work is pending — a spinner tick. With `synchronized`, each frame
/// is wrapped in a synchronized update (stdout is the terminal) so terminals
/// that support it never show a half-drawn frame.
async fn run_app<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    app: &mut App,
    mut updates: mpsc::UnboundedReceiver<AppUpdate>,
    sinks_visible: &watch::Sender<bool>,
    worker: &mut JoinHandle<()>,
    synchronized: bool,
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
                    if synchronized {
                        // Unsupported terminals ignore it; a failed write shows
                        // up in the draw below
                        let _ = crossterm::execute!(
                            std::io::stdout(),
                            crossterm::terminal::BeginSynchronizedUpdate
                        );
                    }
                    let drawn = terminal.draw(|frame| render_ui(frame, app)).map(|_| ());
                    if synchronized {
                        let _ = crossterm::execute!(
                            std::io::stdout(),
                            crossterm::terminal::EndSynchronizedUpdate
                        );
                    }
                    drawn?;
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
        let status_deadline = app.status_deadline();
        tokio::select! {
            () = tokio::time::sleep_until(redraw_at.unwrap_or_else(Instant::now)),
                if redraw_at.is_some() => {}
            // A quit waiting for an in-flight save gives up (the app stays open)
            () = tokio::time::sleep_until(quit_deadline.unwrap_or_else(Instant::now)),
                if quit_deadline.is_some() => app.quit_save_wait_expired(),
            // A success/info message has been shown long enough
            () = tokio::time::sleep_until(status_deadline.unwrap_or_else(Instant::now)),
                if status_deadline.is_some() => app.expire_status(),
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

/// Render the complete UI: header, screen, footer, then dialogs on top
pub(crate) fn render_ui(frame: &mut ratatui::Frame, app: &mut App) {
    // Cap TUI width for better proportions on wide screens
    const MAX_TUI_WIDTH: u16 = 180;

    let size = frame.area();
    if chrome::too_small(size) {
        chrome::render_too_small(frame, size);
        return;
    }
    let content_area = if size.width > MAX_TUI_WIDTH {
        size.centered_horizontally(Constraint::Length(MAX_TUI_WIDTH))
    } else {
        size
    };

    let [header_area, main_area, footer_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(1),
    ])
    .areas(content_area);

    chrome::render_header(frame, header_area, app.current_screen, app.config_dirty);

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

    if app.save_conflict {
        render_conflict_dialog(frame, main_area);
    }
    if app.show_help {
        render_help(
            frame,
            main_area,
            app.current_screen,
            &mut app.help_scroll_state,
            &mut app.help_viewport_height,
            &app.help_collapsed_sections,
        );
    }

    // Footer last: its hints describe whatever is on top
    let contexts = app.active_contexts();
    let unsaved = app.config_dirty;
    let (status, busy, throbber) = app.footer_parts();
    let footer = chrome::Footer {
        status,
        busy,
        contexts: &contexts,
        unsaved,
    };
    chrome::render_footer(frame, footer_area, &footer, throbber);
}

/// Render the "config changed on disk" dialog shown when a save was refused
fn render_conflict_dialog(frame: &mut Frame, area: Rect) {
    let option = |key: &'static str, text: &'static str| {
        Line::from(vec![
            Span::styled(format!("{key:<4}"), theme::key()),
            Span::raw(text),
        ])
    };
    let lines = vec![
        Line::from(Span::styled(
            "The config file changed on disk since it was loaded.",
            theme::warning(),
        )),
        Line::from("Saving now would overwrite those changes."),
        Line::from(""),
        option("o", "Overwrite the file with your changes"),
        option("r", "Reload from disk (discards your unsaved edits)"),
        option("Esc", "Cancel (keep editing, nothing saved)"),
    ];
    widgets::render_text_dialog(frame, area, "Config Changed on Disk", lines, &[]);
}
