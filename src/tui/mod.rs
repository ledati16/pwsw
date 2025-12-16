//! Terminal User Interface (TUI) for PWSW
//!
//! Provides an interactive terminal interface for managing PWSW configuration,
//! monitoring daemon status, and controlling sinks.

use anyhow::{Context, Result};
// `Write` import removed — unused in this module
use crate::tui::app::{AppUpdate, BgCommand};
use crossterm::cursor::Show;
use crossterm::event::EventStream;
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use futures_util::StreamExt;
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Tabs},
    Terminal,
};
use std::io;
use tokio::sync::mpsc::unbounded_channel;

mod app;
mod daemon_control;
mod editor_state;
mod input;
mod preview;
mod screens;
mod widgets;

use app::{App, Screen};
use input::handle_event;
use screens::{render_dashboard, render_help, render_rules, render_settings, render_sinks};
use std::sync::Arc as StdArc;

// Aliases and small struct to keep complex types readable
type CompiledRegex = StdArc<regex::Regex>;
#[derive(Clone)]
struct PreviewReq {
    app_pattern: String,
    title_pattern: Option<String>,
    compiled_app: Option<CompiledRegex>,
    compiled_title: Option<CompiledRegex>,
    ts: std::time::Instant,
}

#[derive(Clone)]
struct PreviewExec {
    app_pattern: String,
    title_pattern: Option<String>,
    compiled_app: Option<CompiledRegex>,
    compiled_title: Option<CompiledRegex>,
}

pub fn windows_fingerprint(windows: &[crate::ipc::WindowInfo]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for w in windows {
        w.app_id.hash(&mut hasher);
        w.title.hash(&mut hasher);
    }
    hasher.finish()
}

/// Run the TUI application
///
/// # Errors
/// Returns an error if TUI initialization fails or terminal operations fail.
pub async fn run() -> Result<()> {
    // Install a panic hook to restore terminal on panic (best-effort).
    // This wraps the existing hook (likely color-eyre from main) to ensure
    // the terminal is reset before the error report is printed.
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let mut stdout = std::io::stdout();
        let _ = execute!(stdout, LeaveAlternateScreen);
        let _ = execute!(std::io::stdout(), Show);
        // Delegate to the original hook (color-eyre) to preserve normal panic output
        original_hook(info);
    }));

    // Initialize terminal
    enable_raw_mode().context("Failed to enable raw mode")?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen).context("Failed to enter alternate screen")?;

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend).context("Failed to create terminal")?;

    // Create app state
    let mut app = App::new()?;

    // Terminal guard to ensure we restore terminal state on panic/return
    struct TerminalGuard;
    impl Drop for TerminalGuard {
        fn drop(&mut self) {
            // Best-effort restore; ignore errors here
            let _ = disable_raw_mode();
            let mut stdout = std::io::stdout();
            let _ = execute!(stdout, LeaveAlternateScreen);
            let _ = execute!(std::io::stdout(), Show);
        }
    }
    let _term_guard = TerminalGuard;

    // Setup background update channels and spawn worker
    // We'll keep an unbounded AppUpdate channel for updates back to UI, a bounded command channel for rare daemon actions,
    // and a small bounded preview-only channel used by input handlers to avoid allocating large queues while typing.
    let (bg_tx, bg_rx) = unbounded_channel::<AppUpdate>();
    let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::channel::<BgCommand>(64);
    app.bg_update_rx = Some(bg_rx);

    // Set the bounded `cmd_tx` into app.bg_cmd_tx so UI can use non-blocking `try_send` directly.
    app.bg_cmd_tx = Some(cmd_tx.clone());

    // Create an unbounded preview input channel and store sender in App so input handlers
    // can push preview requests quickly without blocking. We'll spawn a forwarder task that
    // collapses rapid preview updates (keeps latest) and forwards them to the bounded `cmd_tx`.
    type CompiledRegex = std::sync::Arc<regex::Regex>;
    type PreviewInMsg = (
        String,
        Option<String>,
        Option<CompiledRegex>,
        Option<CompiledRegex>,
    );
    let (preview_in_tx, mut preview_in_rx) = unbounded_channel::<PreviewInMsg>();
    app.preview_in_tx = Some(preview_in_tx.clone());

    // Forwarder task: collapse rapid preview updates and attempt to flush to `cmd_tx`.
    let forward_cmd = cmd_tx.clone();
    let _preview_forwarder = tokio::spawn(async move {
        use tokio::time::{sleep, Duration};
        while let Some((app_pattern, title_pattern, compiled_app, compiled_title)) =
            preview_in_rx.recv().await
        {
            // Try to flush immediately (a few retries). If unable to send, next recv will overwrite the pending request.
            for _ in 0..3 {
                if forward_cmd
                    .try_send(crate::tui::app::BgCommand::PreviewRequest {
                        app_pattern: app_pattern.clone(),
                        title_pattern: title_pattern.clone(),
                        compiled_app: compiled_app.clone(),
                        compiled_title: compiled_title.clone(),
                    })
                    .is_ok()
                {
                    break;
                }
                sleep(Duration::from_millis(20)).await;
            }
            // If still not sent, loop will continue and the next recv will overwrite the previous request.
        }
    });

    // Spawn background worker that polls daemon state and PipeWire every interval
    let bg_handle = tokio::spawn(async move {
        use std::time::{Duration, Instant};
        // Debounce state for preview requests (capture last request, optional compiled regex caches, and timestamp)
        let mut last_preview_req: Option<PreviewReq> = None;
        let mut last_executed_preview: Option<PreviewExec> = None;
        let mut last_windows_fp: Option<u64> = None;
        let debounce_ms = Duration::from_millis(150);
        loop {
            // Poll daemon state in blocking-friendly way
            let running = crate::tui::daemon_control::DaemonManager::detect()
                .is_running()
                .await;
            let windows = if running {
                match crate::ipc::send_request(crate::ipc::Request::ListWindows).await {
                    Ok(crate::ipc::Response::Windows { windows }) => windows,
                    _ => Vec::new(),
                }
            } else {
                Vec::new()
            };

            // Compute a fingerprint for the current window snapshot so we can re-run previews
            let current_fp = windows_fingerprint(&windows);
            let _ = bg_tx.send(AppUpdate::DaemonState {
                running,
                windows: windows.clone(),
            });

            // Poll PipeWire sinks snapshot using spawn_blocking to avoid blocking the tokio worker
            let pipewire_tx = bg_tx.clone();
            let _ = tokio::task::spawn_blocking(move || {
                if let Ok(objects) = crate::pipewire::PipeWire::dump() {
                    let active = crate::pipewire::PipeWire::get_active_sinks(&objects);
                    let profiles = crate::pipewire::PipeWire::get_profile_sinks(&objects, &active);
                    let names = active.iter().map(|s| s.name.clone()).collect();
                    let _ = pipewire_tx.send(AppUpdate::SinksData {
                        active,
                        profiles,
                        names,
                    });
                }
            })
            .await;

            // Process any incoming commands sent from UI (non-blocking checks)
            while let Ok(cmd) = cmd_rx.try_recv() {
                match cmd {
                    BgCommand::DaemonAction(action) => {
                        // execute action and send result back
                        let dm = crate::tui::daemon_control::DaemonManager::detect();
                        let res = match action {
                            crate::tui::app::DaemonAction::Start => dm.start().await,
                            crate::tui::app::DaemonAction::Stop => dm.stop().await,
                            crate::tui::app::DaemonAction::Restart => dm.restart().await,
                        };
                        match res {
                            Ok(msg) => {
                                let _ = bg_tx.send(AppUpdate::ActionResult(msg));
                            }
                            Err(e) => {
                                let _ = bg_tx.send(AppUpdate::ActionResult({
                                    let mut s = String::with_capacity(10);
                                    s.push_str("Failed: ");
                                    s.push_str(&format!("{e:#}"));
                                    s
                                }));
                            }
                        }
                    }
                    BgCommand::PreviewRequest {
                        app_pattern,
                        title_pattern,
                        compiled_app,
                        compiled_title,
                    } => {
                        // Update last_preview_req (debounce). We don't spawn matching yet.
                        last_preview_req = Some(PreviewReq {
                            app_pattern,
                            title_pattern,
                            compiled_app,
                            compiled_title,
                            ts: Instant::now(),
                        });
                    }
                }
            }

            // If we have a pending preview request and it has aged enough, execute it
            if let Some(req) = last_preview_req.clone() {
                if req.ts.elapsed() >= debounce_ms {
                    // Clear last request before running to avoid races
                    last_preview_req = None;

                    let tx = bg_tx.clone();
                    let windows_clone = windows.clone();

                    // Send pending update so UI can show spinner after a short visual delay
                    let _ = tx.send(AppUpdate::PreviewPending {
                        app_pattern: req.app_pattern.clone(),
                        title_pattern: req.title_pattern.clone(),
                    });

                    // Clone patterns and compiled caches for the closure and for the message
                    let app_pat_send = req.app_pattern.clone();
                    let title_pat_send = req.title_pattern.clone();
                    let compiled_app_send = req.compiled_app.clone();
                    let compiled_title_send = req.compiled_title.clone();

                    // Record this as the last executed preview (for auto re-run on window changes)
                    last_executed_preview = Some(PreviewExec {
                        app_pattern: app_pat_send.clone(),
                        title_pattern: title_pat_send.clone(),
                        compiled_app: compiled_app_send.clone(),
                        compiled_title: compiled_title_send.clone(),
                    });
                    last_windows_fp = Some(current_fp);

                    tokio::spawn(async move {
                        use std::time::Duration;
                        let timeout = Duration::from_millis(200);

                        // Use the new execute_preview helper which handles spawn_blocking + timeout and accepts optional compiled regexes
                        let (matches_out, timed_out) = crate::tui::preview::execute_preview(
                            app_pat_send.clone(),
                            title_pat_send.clone(),
                            windows_clone,
                            100,
                            timeout,
                            compiled_app_send,
                            compiled_title_send,
                        )
                        .await;

                        let _ = tx.send(AppUpdate::PreviewMatches {
                            app_pattern: app_pat_send.clone(),
                            title_pattern: title_pat_send.clone(),
                            matches: matches_out.into_iter().take(10).collect(),
                            timed_out,
                        });
                    });
                }
            }

            // Auto-retrigger previews when window snapshot changes and no user preview pending
            if last_windows_fp != Some(current_fp) {
                // Update last_windows_fp first to avoid repeated triggers for the same snapshot
                last_windows_fp = Some(current_fp);

                // Only auto-re-run when there is no pending user request (debounce) and we have a previously executed preview
                if last_preview_req.is_none() {
                    if let Some(exec) = last_executed_preview.clone() {
                        let tx = bg_tx.clone();
                        let windows_clone = windows.clone();

                        // Send pending update so UI can show spinner
                        let _ = tx.send(AppUpdate::PreviewPending {
                            app_pattern: exec.app_pattern.clone(),
                            title_pattern: exec.title_pattern.clone(),
                        });

                        let app_pat_send = exec.app_pattern.clone();
                        let title_pat_send = exec.title_pattern.clone();
                        let compiled_app_send = exec.compiled_app.clone();
                        let compiled_title_send = exec.compiled_title.clone();

                        tokio::spawn(async move {
                            use std::time::Duration;
                            let timeout = Duration::from_millis(200);

                            let (matches_out, timed_out) = crate::tui::preview::execute_preview(
                                app_pat_send.clone(),
                                title_pat_send.clone(),
                                windows_clone,
                                100,
                                timeout,
                                compiled_app_send,
                                compiled_title_send,
                            )
                            .await;

                            let _ = tx.send(AppUpdate::PreviewMatches {
                                app_pattern: app_pat_send.clone(),
                                title_pattern: title_pat_send.clone(),
                                matches: matches_out.into_iter().take(10).collect(),
                                timed_out,
                            });
                        });
                    }
                }
            }

            tokio::time::sleep(Duration::from_millis(700)).await;
        }
    });

    // Main event loop
    let result = run_app(&mut terminal, &mut app).await;

    // Restore terminal
    disable_raw_mode().context("Failed to disable raw mode")?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)
        .context("Failed to leave alternate screen")?;
    terminal.show_cursor().context("Failed to show cursor")?;

    // Background worker: abort when exiting
    bg_handle.abort();

    result
}

/// Main application loop
async fn run_app<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    app: &mut App,
) -> Result<()> {
    // Use small tick for rendering; background updates arrive via app.bg_update_rx
    let mut tick = tokio::time::interval(std::time::Duration::from_millis(80));

    // Animation timing (time-based spinner)
    use std::time::Instant;
    let mut last_anim = Instant::now();
    const ANIM_MS: u64 = 120; // spinner frame every 120ms

    // Event stream for async input handling
    let mut events = EventStream::new();

    loop {
        tokio::select! {
            _ = tick.tick() => {
                // Advance animation if enough time elapsed
                let now = Instant::now();
                if now.duration_since(last_anim).as_millis() >= ANIM_MS as u128 {
                    app.throbber_state.calc_next();
                    last_anim = now;
                    app.dirty = true;
                }

                // Note: Input handling is now done in the `events.next()` branch

                // Only redraw when needed (dirty) or when animation advanced
                if app.dirty {
                    #[cfg(debug_assertions)]
                    {
                        use std::time::Instant as Ti;
                        let start = Ti::now();
                        terminal.draw(|frame| render_ui(frame, app))?;
                        let elapsed = start.elapsed();

                        // Extra context for slow-frame logs
                        if elapsed.as_millis() > 15 {
                            let run_ms = elapsed.as_millis();
                            let screen_name = format!("{:?}", app.current_screen);
                            let preview_pending = app.preview.as_ref().is_some_and(|p| p.pending);
                            let windows = app.window_count;
                            eprintln!(
                                "[tui] {} ms [{}] slow frame: {} ms preview_pending={} windows={}",
                                run_ms,
                                screen_name,
                                elapsed.as_millis(),
                                preview_pending,
                                windows
                            );
                        }
                    }
                    #[cfg(not(debug_assertions))]
                    {
                        terminal.draw(|frame| render_ui(frame, app))?;
                    }
                    app.dirty = false;
                }

                // Check if we should quit
                if app.should_quit {
                    break;
                }
            }
            // Handle input events
            Some(Ok(event)) = events.next() => {
                // `handle_event` expects a reference to `Event`; it is infallible and returns `()`.
                handle_event(app, &event);
            }
            // Process background updates if any
            maybe_update = async {
                if let Some(rx) = &mut app.bg_update_rx { rx.recv().await } else { None }
            } => {
                if let Some(update) = maybe_update {
                    match update {
                        AppUpdate::SinksData { active, profiles, names } => {
                            app.active_sink_list = active;
                            app.profile_sink_list = profiles;
                            app.active_sinks = names;
                            app.dirty = true;
                        }
                        AppUpdate::DaemonState { running, windows } => {
                            app.daemon_running = running;
                            app.window_count = windows.len();
                            app.windows = windows;
                            app.dirty = true;
                        }
                        AppUpdate::ActionResult(msg) => {
                            app.set_status(msg);
                            // set_status sets dirty already
                        }
                        AppUpdate::PreviewPending { app_pattern, title_pattern } => {
                            // Only mark pending if it matches current editor content
                            if app.rules_screen.editor.app_id_pattern.value() == app_pattern && app.rules_screen.editor.title_pattern.value() == title_pattern.clone().unwrap_or_default() {
                                // Store a minimal PreviewResult with no matches but pending flag (timed_out=false)
                                app.preview = Some(crate::tui::app::PreviewResult { app_pattern, title_pattern, matches: Vec::new(), timed_out: false, pending: true });
                                app.dirty = true;
                            }
                        }
                        AppUpdate::PreviewMatches { app_pattern, title_pattern, matches, timed_out } => {
                            // Only apply preview if patterns match current editor content (avoid race)
                            if app.rules_screen.editor.app_id_pattern.value() == app_pattern && app.rules_screen.editor.title_pattern.value() == title_pattern.clone().unwrap_or_default() {
                                // Store preview in app.preview as a typed struct
                                app.preview = Some(crate::tui::app::PreviewResult { app_pattern, title_pattern, matches, timed_out, pending: false });
                                app.dirty = true;
                            }
                        }
                    }
                }
            }
        }
    }

    Ok(())
}

/// Render the complete UI
fn render_ui(frame: &mut ratatui::Frame, app: &mut App) {
    let size = frame.area();

    // Create main layout: [Header (tabs) | Content | Footer (status)]
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // Header with tabs
            Constraint::Min(0),    // Content area
            Constraint::Length(1), // Footer
        ])
        .split(size);

    // Render header (tab bar)
    render_header(frame, chunks[0], app.current_screen, app.config_dirty);

    // Render screen content
    match app.current_screen {
        Screen::Dashboard => render_dashboard(
            frame,
            chunks[1],
            &app.config,
            &app.dashboard_screen,
            app.daemon_running,
            app.window_count,
        ),
        Screen::Sinks => render_sinks(
            frame,
            chunks[1],
            &app.config.sinks,
            &mut app.sinks_screen,
            &app.active_sinks,
            &app.active_sink_list,
            &app.profile_sink_list,
        ),
        Screen::Rules => render_rules(
            frame,
            chunks[1],
            &app.config.rules,
            &app.config.sinks,
            &mut app.rules_screen,
            &app.windows,
            app.preview.as_ref(),
            &mut app.throbber_state,
        ),
        Screen::Settings => render_settings(
            frame,
            chunks[1],
            &app.config.settings,
            &mut app.settings_screen,
        ),
    }

    // Render footer
    render_footer(frame, chunks[2], &app.status_message);

    // Render help overlay on top if active
    if app.show_help {
        render_help(frame, size, app.current_screen, &mut app.help_scroll_state);
    }
}

/// Render the header with tab navigation
fn render_header(
    frame: &mut ratatui::Frame,
    area: Rect,
    current_screen: Screen,
    config_dirty: bool,
) {
    let titles: Vec<_> = Screen::all()
        .iter()
        .map(|s| {
            let name = s.name();
            let mut t = String::with_capacity(1 + 1 + name.len());
            t.push('[');
            t.push(s.key());
            t.push(']');
            t.push_str(name);
            t
        })
        .collect();

    let selected = Screen::all()
        .iter()
        .position(|&s| s == current_screen)
        .unwrap_or(0);

    // Build title with optional styled [unsaved] indicator
    let version = crate::version_string();
    let title_line = if config_dirty {
        Line::from(vec![
            Span::raw("PWSW "),
            Span::raw(&version),
            Span::raw(" "),
            Span::styled(
                "[unsaved]",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
        ])
    } else {
        Line::from(vec![Span::raw("PWSW "), Span::raw(&version)])
    };

    let tabs = Tabs::new(titles)
        .block(Block::default().borders(Borders::ALL).title(title_line))
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
