//! Terminal User Interface (TUI) for PWSW
//!
//! Provides an interactive terminal interface for managing PWSW configuration,
//! monitoring daemon status, and controlling sinks.

use crate::style::colors;
use crate::tui::app::{AppUpdate, BgCommand};
use anyhow::{Context, Result};
use crossterm::cursor::Show;
use crossterm::event::EventStream;
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use futures_util::StreamExt;
use ratatui::{
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Tabs},
    Frame, Terminal,
};
use std::fmt::Write;
use std::io;
use tokio::sync::mpsc::unbounded_channel;

mod app;
mod daemon_control;
mod editor_state;
mod input;
mod log_tailer;
mod preview;
mod screens;
mod widgets;

#[cfg(test)]
mod tests;

use app::{App, Screen};
use input::handle_event;
use screens::{
    render_dashboard, render_help, render_rules, render_settings, render_sinks, RulesRenderContext,
};
use std::sync::Arc as StdArc;

// Aliases and small struct to keep complex types readable
type CompiledRegex = StdArc<regex::Regex>;
// Message payload for preview forwarder (app pattern, title pattern, optional compiled regexes)
type PreviewInMsg = (
    String,
    Option<String>,
    Option<CompiledRegex>,
    Option<CompiledRegex>,
);
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

#[must_use]
pub(crate) fn windows_fingerprint(windows: &[crate::ipc::WindowInfo]) -> u64 {
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
// TUI main event loop - cohesive logic hard to split; constants scoped for clarity
#[allow(clippy::too_many_lines, clippy::items_after_statements)]
pub async fn run() -> Result<()> {
    // Load config BEFORE entering alternate screen to ensure any first-run messages
    // (e.g., "Created default config", "Next steps...") appear normally on the terminal
    // rather than leaking into the TUI display
    let config = crate::config::Config::load()?;

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

    // Create app state (pass pre-loaded config)
    let mut app = App::with_config(config);

    // Query daemon for manager info, or detect locally if daemon not running
    let is_systemd = match crate::ipc::send_request(crate::ipc::Request::GetManagerInfo).await {
        Ok(crate::ipc::Response::ManagerInfo { daemon_manager }) => {
            daemon_manager == crate::daemon_manager::DaemonManager::Systemd
        }
        _ => {
            // Daemon not running, fall back to local detection for UI configuration
            crate::daemon_manager::DaemonManager::detect()
                == crate::daemon_manager::DaemonManager::Systemd
        }
    };
    app.dashboard_screen.set_max_actions(is_systemd);

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
    let (preview_in_tx, mut preview_in_rx) = unbounded_channel::<PreviewInMsg>();
    app.preview_in_tx = Some(preview_in_tx.clone());

    // Forwarder task: collapse rapid preview updates and attempt to flush to `cmd_tx`.
    let forward_cmd = cmd_tx.clone();
    let _preview_forwarder = tokio::spawn(async move {
        while let Some((app_pattern, title_pattern, compiled_app, compiled_title)) =
            preview_in_rx.recv().await
        {
            use tokio::time::{sleep, Duration};
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
        use std::time::Duration;
        // Debounce state for preview requests (capture last request, optional compiled regex caches, and timestamp)
        let mut last_preview_req: Option<PreviewReq> = None;
        let mut last_executed_preview: Option<PreviewExec> = None;
        let mut last_windows_fp: Option<u64> = None;
        let debounce_ms = Duration::from_millis(150);

        // Helper function to poll daemon state and send update
        let poll_daemon_state = || async {
            // Poll daemon state via IPC
            let daemon_manager = match crate::ipc::send_request(crate::ipc::Request::GetManagerInfo)
                .await
            {
                Ok(crate::ipc::Response::ManagerInfo { daemon_manager }) => Some(daemon_manager),
                _ => None, // Daemon not running or query failed
            };

            let running = daemon_manager.is_some();
            let service_enabled = daemon_manager.and_then(|dm| {
                if dm == crate::daemon_manager::DaemonManager::Systemd {
                    // Query enabled state (uses systemctl)
                    Some(dm.is_enabled())
                } else {
                    None
                }
            });

            let windows = if running {
                match crate::ipc::send_request(crate::ipc::Request::ListWindows).await {
                    Ok(crate::ipc::Response::Windows { windows }) => windows,
                    _ => Vec::new(),
                }
            } else {
                Vec::new()
            };

            // Compute fingerprint for window snapshot (for preview re-runs)
            let current_fp = windows_fingerprint(&windows);
            let _ = bg_tx.send(AppUpdate::DaemonState {
                running,
                windows: windows.clone(),
                daemon_manager,
                service_enabled,
            });

            (windows, current_fp)
        };

        // Create log tailer and read initial logs
        let mut log_tailer = match log_tailer::LogTailer::new() {
            Ok(mut tailer) => {
                // Read last 100 lines initially
                if let Err(e) = tailer.read_initial(100) {
                    tracing::warn!("Failed to read initial daemon logs: {e:#}");
                }
                // Send initial logs to UI
                let initial_logs = tailer.get_lines().to_vec();
                if !initial_logs.is_empty() {
                    let _ = bg_tx.send(AppUpdate::DaemonLogs(initial_logs));
                }
                Some(tailer)
            }
            Err(e) => {
                tracing::warn!("Failed to create log tailer: {e:#}");
                None
            }
        };

        loop {
            // Poll daemon state and send update to UI
            let (windows, current_fp) = poll_daemon_state().await;

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
                        let dm = crate::daemon_manager::DaemonManager::detect();
                        let res = match action {
                            crate::tui::app::DaemonAction::Start => dm.start().await,
                            crate::tui::app::DaemonAction::Stop => dm.stop().await,
                            crate::tui::app::DaemonAction::Restart => dm.restart().await,
                            crate::tui::app::DaemonAction::Enable => dm.enable(),
                            crate::tui::app::DaemonAction::Disable => dm.disable(),
                        };
                        match res {
                            Ok(msg) => {
                                let _ = bg_tx.send(AppUpdate::ActionResult(msg));
                                // Immediately poll daemon state for instant UI feedback on start/stop/restart
                                if matches!(
                                    action,
                                    crate::tui::app::DaemonAction::Start
                                        | crate::tui::app::DaemonAction::Stop
                                        | crate::tui::app::DaemonAction::Restart
                                ) {
                                    let _ = poll_daemon_state().await;
                                }
                            }
                            Err(e) => {
                                let _ = bg_tx.send(AppUpdate::ActionResult({
                                    let mut s = String::with_capacity(10);
                                    s.push_str("Failed: ");
                                    let _ = write!(s, "{e:#}");
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
                            ts: std::time::Instant::now(),
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
                            // use std::time::Duration; (moved to module imports)
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

            // Check for new daemon log lines (event-driven via file watcher)
            if let Some(ref mut tailer) = log_tailer {
                // Only read if file has changed (non-blocking check)
                if tailer.has_file_changed() {
                    match tailer.read_new_lines() {
                        Ok(new_lines) if !new_lines.is_empty() => {
                            let _ = bg_tx.send(AppUpdate::DaemonLogs(new_lines));
                        }
                        Err(e) => {
                            tracing::warn!("Failed to read new daemon logs: {e:#}");
                        }
                        _ => {}
                    }
                }
            }

            // Reduced polling frequency (was 700ms) - log reading is now event-driven
            tokio::time::sleep(Duration::from_millis(2500)).await;
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
#[allow(clippy::too_many_lines)] // Event loop logic is cohesive, hard to split meaningfully
async fn run_app<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    app: &mut App,
) -> Result<()> {
    use std::time::Instant;

    // Frame rate constants
    const TARGET_FPS: u64 = 60;
    const MIN_FRAME_TIME_MS: u64 = 1000 / TARGET_FPS; // 16ms (actual: ~62.5 FPS)
    const ANIM_MS: u64 = 120; // spinner frame every 120ms

    // Timing state
    let mut last_frame = Instant::now();
    let mut last_anim = Instant::now();

    // Ensure initial render happens
    app.dirty = true;

    // Tick provides 60 FPS baseline for animations and frame rate limiting
    // Rendering happens after every select! iteration if dirty and enough time elapsed
    let mut tick = tokio::time::interval(std::time::Duration::from_millis(MIN_FRAME_TIME_MS));

    // Event stream for async input handling
    let mut events = EventStream::new();

    loop {
        tokio::select! {
            _ = tick.tick() => {
                // Advance animation if enough time elapsed
                let now = Instant::now();
                if now.duration_since(last_anim).as_millis() >= u128::from(ANIM_MS) {
                    app.throbber_state_mut().calc_next();
                    last_anim = now;
                    app.dirty = true;
                }
                // Note: Rendering happens at end of loop, not here
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
                        AppUpdate::DaemonState {
                            running,
                            windows,
                            daemon_manager,
                            service_enabled,
                        } => {
                            app.daemon_running = running;
                            app.window_count = windows.len();
                            app.windows = windows;
                            app.dashboard_screen.service_enabled = service_enabled;
                            // Update max actions if daemon manager changed (e.g., service installed/removed)
                            if let Some(dm) = daemon_manager {
                                let is_systemd = dm == crate::daemon_manager::DaemonManager::Systemd;
                                app.dashboard_screen.set_max_actions(is_systemd);
                            }
                            app.dirty = true;
                        }
                        AppUpdate::ActionResult(msg) => {
                            app.set_status(msg);
                            // Clear daemon action pending flag when an action completes
                            app.daemon_action_pending = false;
                            // set_status sets dirty already
                        }
                        AppUpdate::PreviewPending { app_pattern, title_pattern } => {
                            // Only mark pending if it matches current editor content
                            if app.rules_screen.editor.app_id_pattern.value() == app_pattern && app.rules_screen.editor.title_pattern.value() == title_pattern.clone().unwrap_or_default() {
                                // Store a minimal PreviewResult with no matches but pending flag (timed_out=false)
                                app.set_preview(crate::tui::app::PreviewResult { app_pattern, title_pattern, matches: Vec::new(), timed_out: false, pending: true });
                            }
                        }
                        AppUpdate::PreviewMatches { app_pattern, title_pattern, matches, timed_out } => {
                            // Only apply preview if patterns match current editor content (avoid race)
                            if app.rules_screen.editor.app_id_pattern.value() == app_pattern && app.rules_screen.editor.title_pattern.value() == title_pattern.clone().unwrap_or_default() {
                                // Store preview in app.preview as a typed struct
                                app.set_preview(crate::tui::app::PreviewResult { app_pattern, title_pattern, matches, timed_out, pending: false });
                            }
                        }
                        AppUpdate::DaemonLogs(new_lines) => {
                            // Append new log lines to the buffer
                            app.daemon_log_lines.extend(new_lines);
                            // Keep only last 500 lines to avoid unbounded growth
                            if app.daemon_log_lines.len() > 500 {
                                let excess = app.daemon_log_lines.len() - 500;
                                app.daemon_log_lines.drain(0..excess);
                            }
                            app.dirty = true;
                        }
                    }
                }
            }
        }

        // Common render path: Execute after every select! branch
        // This ensures immediate visual feedback for all state changes
        if app.dirty {
            let now = Instant::now();
            let elapsed_since_last_frame = now.duration_since(last_frame);

            // Frame rate limiter: Only render if enough time has passed
            if elapsed_since_last_frame.as_millis() >= u128::from(MIN_FRAME_TIME_MS) {
                #[cfg(debug_assertions)]
                {
                    let start = Instant::now();
                    terminal.draw(|frame| render_ui(frame, app))?;
                    let render_time = start.elapsed();

                    // Log slow frames (threshold increased since we're targeting 60 FPS)
                    if render_time.as_millis() > u128::from(MIN_FRAME_TIME_MS) {
                        let run_ms = render_time.as_millis();
                        let screen_name = format!("{:?}", app.current_screen);
                        let preview_pending = app.preview.as_ref().is_some_and(|p| p.pending);
                        let windows = app.window_count;
                        tracing::debug!(
                            run_ms,
                            screen = %screen_name,
                            preview_pending,
                            windows,
                            "slow frame (exceeds 16ms target)"
                        );
                    }
                }
                #[cfg(not(debug_assertions))]
                {
                    terminal.draw(|frame| render_ui(frame, app))?;
                }

                app.dirty = false;
                last_frame = now;
            }
            // If frame rate limited, dirty flag stays true so we render next tick
        }

        // Check if we should quit (moved out of tick branch)
        if app.should_quit {
            break;
        }
    }

    Ok(())
}

/// Render the complete UI
fn render_ui(frame: &mut ratatui::Frame, app: &mut App) {
    let size = frame.area();

    // Create main layout: [Header (tabs) | Context Bar | Content | Footer (status)]
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // Header with tabs
            Constraint::Length(1), // Context bar
            Constraint::Min(0),    // Content area
            Constraint::Length(1), // Footer
        ])
        .split(size);

    // Render header (tab bar)
    render_header(frame, chunks[0], app.current_screen, app.config_dirty);

    // Render context bar
    render_context_bar(frame, chunks[1], app);

    // Render screen content
    match app.current_screen {
        Screen::Dashboard => {
            let ctx = screens::DashboardRenderContext {
                config: &app.config,
                screen_state: &app.dashboard_screen,
                daemon_running: app.daemon_running,
                window_count: app.window_count,
                daemon_logs: &app.daemon_log_lines,
                windows: &app.windows,
            };
            render_dashboard(frame, chunks[2], &ctx);
        }
        Screen::Sinks => render_sinks(
            frame,
            chunks[2],
            &app.config.sinks,
            &mut app.sinks_screen,
            &app.active_sinks,
            &app.active_sink_list,
            &app.profile_sink_list,
        ),
        Screen::Rules => {
            // Snapshot read-only items so we can take mutable borrows later (throbber, screen state)
            // This avoids overlapping borrows when calling `render_rules` which needs both
            // `&mut app.rules_screen` and a mutable throbber state reference.
            let rules_snapshot = app.config.rules.clone();
            let sinks_snapshot = app.config.sinks.clone();
            let windows_snapshot = app.windows.clone();
            let preview_snapshot = app.preview.clone();

            // Borrow rules screen and throbber together using App helper so we get
            // two mutable references from a single &mut self borrow, avoiding double-borrows.
            let (rules_screen_mut, throbber_state_mut) = app.borrow_rules_and_throbber();

            render_rules(
                frame,
                chunks[2],
                &mut RulesRenderContext {
                    rules: &rules_snapshot,
                    sinks: &sinks_snapshot,
                    screen_state: rules_screen_mut,
                    windows: &windows_snapshot,
                    preview: preview_snapshot.as_ref(),
                    throbber_state: throbber_state_mut,
                },
            );
        }
        Screen::Settings => render_settings(
            frame,
            chunks[2],
            &app.config.settings,
            &mut app.settings_screen,
        ),
    }

    // Render footer (include daemon action pending flag and throbber state)
    // Clone the status message first to avoid an immutable borrow overlapping the mutable throbber borrow.
    let status_clone = app.status_message().cloned();
    render_footer(
        frame,
        chunks[3],
        status_clone.as_ref(),
        app.daemon_action_pending,
        app.throbber_state_mut(),
    );

    // Render help overlay on top if active
    if app.show_help {
        render_help(
            frame,
            size,
            app.current_screen,
            &mut app.help_scroll_state,
            &mut app.help_viewport_height,
            &app.help_collapsed_sections,
        );
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
            let mut t = String::with_capacity(1 + 1 + 1 + name.len()); // +1 for space
            t.push('[');
            t.push(s.key());
            t.push(']');
            t.push(' '); // Add space
            t.push_str(name);
            t
        })
        .collect();

    let selected = Screen::all()
        .iter()
        .position(|&s| s == current_screen)
        .unwrap_or(0);

    // Build left title with optional styled [unsaved] indicator
    let version = crate::version_string();
    let left_title = if config_dirty {
        ratatui::widgets::block::Title::from(Line::from(vec![
            Span::raw("PWSW "),
            Span::raw(&version),
            Span::raw(" "),
            Span::styled(
                "[unsaved]",
                Style::default()
                    .fg(colors::UI_WARNING)
                    .add_modifier(Modifier::BOLD),
            ),
        ]))
    } else {
        ratatui::widgets::block::Title::from(Line::from(vec![
            Span::raw("PWSW "),
            Span::raw(&version),
        ]))
    };

    let tabs = Tabs::new(titles)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(left_title)
                .title_top(
                    Line::from(vec![
                        Span::styled("[?]", Style::default().fg(colors::UI_HIGHLIGHT)),
                        Span::raw(" Help "),
                    ])
                    .alignment(Alignment::Right),
                ),
        )
        .select(selected)
        .style(Style::default().fg(colors::UI_SECONDARY))
        .highlight_style(
            Style::default()
                .fg(colors::UI_FOCUS)
                .add_modifier(Modifier::BOLD),
        );

    frame.render_widget(tabs, area);
}

/// Render the context bar with screen-specific actions
// Context bar shows comprehensive keybinding hints for all screen modes
#[allow(clippy::too_many_lines)]
fn render_context_bar(frame: &mut Frame, area: Rect, app: &App) {
    use crate::tui::app::ScreenMode;
    use crate::tui::screens::rules::RulesMode;
    use crate::tui::screens::sinks::SinksMode;

    let mode = app.get_screen_mode();

    let text = match (app.current_screen, mode) {
        (app::Screen::Dashboard, ScreenMode::List) => {
            // Phase 9B: View-aware context bar for dashboard
            use crate::tui::screens::DashboardView;
            let mut spans = vec![
                Span::styled("[←→]", Style::default().fg(colors::UI_HIGHLIGHT)),
                Span::raw(" Select Action  "),
                Span::styled("[Enter]", Style::default().fg(colors::UI_HIGHLIGHT)),
                Span::raw(" Execute  "),
            ];

            // Add view-specific scrolling hints
            match app.dashboard_screen.current_view {
                DashboardView::Logs => {
                    spans.push(Span::styled(
                        "[↑↓/PgUp/PgDn]",
                        Style::default().fg(colors::UI_HIGHLIGHT),
                    ));
                    spans.push(Span::raw(" Scroll Logs  "));
                    spans.push(Span::styled(
                        "[w]",
                        Style::default().fg(colors::UI_HIGHLIGHT),
                    ));
                    spans.push(Span::raw(" View Windows"));
                }
                DashboardView::Windows => {
                    spans.push(Span::styled(
                        "[PgUp/PgDn]",
                        Style::default().fg(colors::UI_HIGHLIGHT),
                    ));
                    spans.push(Span::raw(" Scroll Windows  "));
                    spans.push(Span::styled(
                        "[w]",
                        Style::default().fg(colors::UI_HIGHLIGHT),
                    ));
                    spans.push(Span::raw(" View Logs"));
                }
            }

            Line::from(spans)
        }
        (app::Screen::Sinks, ScreenMode::List) => Line::from(vec![
            Span::raw("↑↓ Navigate  "),
            Span::styled("[Shift+↑↓]", Style::default().fg(colors::UI_HIGHLIGHT)),
            Span::raw(" Reorder  "),
            Span::styled("[a]", Style::default().fg(colors::UI_HIGHLIGHT)),
            Span::raw(" Add  "),
            Span::styled("[e]", Style::default().fg(colors::UI_HIGHLIGHT)),
            Span::raw(" Edit  "),
            Span::styled("[x]", Style::default().fg(colors::UI_HIGHLIGHT)),
            Span::raw(" Delete  "),
            Span::styled("[Space]", Style::default().fg(colors::UI_HIGHLIGHT)),
            Span::raw(" Default"),
        ]),
        (app::Screen::Sinks, ScreenMode::Modal) => {
            // Determine which modal we're in
            match app.sinks_screen.mode {
                SinksMode::AddEdit => Line::from(vec![
                    Span::raw("↑↓/"),
                    Span::styled("Tab", Style::default().fg(colors::UI_HIGHLIGHT)),
                    Span::raw(" Switch field  "),
                    Span::styled("[Enter]", Style::default().fg(colors::UI_HIGHLIGHT)),
                    Span::raw(" Save/Select  "),
                    Span::styled("[Esc]", Style::default().fg(colors::UI_HIGHLIGHT)),
                    Span::raw(" Cancel"),
                ]),
                SinksMode::Delete => Line::from(vec![
                    Span::styled("[Enter]", Style::default().fg(colors::UI_HIGHLIGHT)),
                    Span::raw(" Confirm  "),
                    Span::styled("[Esc]", Style::default().fg(colors::UI_HIGHLIGHT)),
                    Span::raw(" Cancel"),
                ]),
                SinksMode::SelectSink => Line::from(vec![
                    Span::raw("↑↓ Navigate  "),
                    Span::styled("[Enter]", Style::default().fg(colors::UI_HIGHLIGHT)),
                    Span::raw(" Select  "),
                    Span::styled("[Esc]", Style::default().fg(colors::UI_HIGHLIGHT)),
                    Span::raw(" Cancel"),
                ]),
                SinksMode::List => Line::from(""), // Should not happen in Modal mode
            }
        }
        (app::Screen::Rules, ScreenMode::List) => Line::from(vec![
            Span::raw("↑↓ Navigate  "),
            Span::styled("[Shift+↑↓]", Style::default().fg(colors::UI_HIGHLIGHT)),
            Span::raw(" Reorder  "),
            Span::styled("[a]", Style::default().fg(colors::UI_HIGHLIGHT)),
            Span::raw(" Add  "),
            Span::styled("[e]", Style::default().fg(colors::UI_HIGHLIGHT)),
            Span::raw(" Edit  "),
            Span::styled("[x]", Style::default().fg(colors::UI_HIGHLIGHT)),
            Span::raw(" Delete"),
        ]),
        (app::Screen::Rules, ScreenMode::Modal) => {
            // Determine which modal we're in
            match app.rules_screen.mode {
                RulesMode::AddEdit => {
                    let mut spans = vec![
                        Span::raw("↑↓/"),
                        Span::styled("Tab", Style::default().fg(colors::UI_HIGHLIGHT)),
                        Span::raw(" Switch field  "),
                        Span::styled("[Enter]", Style::default().fg(colors::UI_HIGHLIGHT)),
                        Span::raw(" Save/Select  "),
                        Span::styled("[Esc]", Style::default().fg(colors::UI_HIGHLIGHT)),
                        Span::raw(" Cancel"),
                    ];
                    // Show live preview indicator if preview is active
                    if app.preview.is_some() {
                        spans.push(Span::raw("  "));
                        spans.push(Span::styled(
                            "⚡ Live preview",
                            Style::default().fg(colors::UI_SUCCESS),
                        ));
                    }
                    Line::from(spans)
                }
                RulesMode::Delete => Line::from(vec![
                    Span::styled("[Enter]", Style::default().fg(colors::UI_HIGHLIGHT)),
                    Span::raw(" Confirm  "),
                    Span::styled("[Esc]", Style::default().fg(colors::UI_HIGHLIGHT)),
                    Span::raw(" Cancel"),
                ]),
                RulesMode::SelectSink => Line::from(vec![
                    Span::raw("↑↓ Navigate  "),
                    Span::styled("[Enter]", Style::default().fg(colors::UI_HIGHLIGHT)),
                    Span::raw(" Select  "),
                    Span::styled("[Esc]", Style::default().fg(colors::UI_HIGHLIGHT)),
                    Span::raw(" Cancel"),
                ]),
                RulesMode::List => Line::from(""), // Should not happen in Modal mode
            }
        }
        (app::Screen::Settings, ScreenMode::List) => Line::from(vec![
            Span::raw("↑↓ Navigate  "),
            Span::styled("[Enter/Space]", Style::default().fg(colors::UI_HIGHLIGHT)),
            Span::raw(" Toggle/Edit"),
        ]),
        (app::Screen::Settings, ScreenMode::Modal) => Line::from(vec![
            Span::raw("↑↓ Navigate  "),
            Span::styled("[Enter]", Style::default().fg(colors::UI_HIGHLIGHT)),
            Span::raw(" Confirm  "),
            Span::styled("[Esc]", Style::default().fg(colors::UI_HIGHLIGHT)),
            Span::raw(" Cancel"),
        ]),
        _ => Line::from(""),
    };

    let paragraph = Paragraph::new(text).style(
        Style::default()
            .fg(colors::UI_SECONDARY)
            .bg(ratatui::style::Color::Black),
    );
    frame.render_widget(paragraph, area);
}

/// Render the footer with keyboard shortcuts and status
fn render_footer(
    frame: &mut ratatui::Frame,
    area: Rect,
    status_message: Option<&String>,
    daemon_action_pending: bool,
    throbber_state: &mut throbber_widgets_tui::ThrobberState,
) {
    use throbber_widgets_tui::Throbber;

    // When a daemon action is pending, render a small throbber on the left and the
    // status message to the right. Otherwise render the normal footer text.
    if daemon_action_pending {
        // Split area into throbber (3 chars) and text
        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(3), Constraint::Min(0)])
            .split(area);

        let throb = Throbber::default().style(Style::default().fg(colors::UI_WARNING));
        frame.render_stateful_widget(throb, chunks[0], throbber_state);

        let text = if let Some(msg) = status_message {
            Line::from(vec![
                Span::raw(" "),
                Span::styled(msg, Style::default().fg(colors::UI_TEXT)),
            ])
        } else {
            Line::from(vec![Span::raw("Daemon action in progress...")])
        };
        let footer = Paragraph::new(text);
        frame.render_widget(footer, chunks[1]);
    } else {
        let text = if let Some(msg) = status_message {
            Line::from(vec![
                Span::styled("● ", Style::default().fg(colors::UI_WARNING)),
                Span::styled(msg, Style::default().fg(colors::UI_TEXT)),
            ])
        } else {
            Line::from(vec![
                Span::raw("[q] Quit  "),
                Span::styled("[Tab/Shift-Tab]", Style::default().fg(colors::UI_HIGHLIGHT)),
                Span::raw(" Cycle  "),
                Span::styled("Ctrl+S", Style::default().fg(colors::UI_SUCCESS)),
                Span::raw(" Save"),
            ])
        };

        let footer = Paragraph::new(text);
        frame.render_widget(footer, area);
    }
}
