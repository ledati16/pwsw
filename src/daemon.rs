//! Daemon mode
//!
//! Runs the main event loop, listening to compositor window events,
//! IPC requests, and switching audio sinks based on configured rules.

use anyhow::{Context, Result};
use notify::{Event, RecursiveMode, Watcher};
use std::time::Instant;
use tokio::signal;
use tokio::sync::broadcast;
use tracing::{error, info, warn};

use crate::compositor;
use crate::config::Config;
use crate::ipc::{self, IpcServer, Request, Response, WindowInfo};
use crate::notification::send_notification;
use crate::pipewire::PipeWire;
use crate::state::State;
use crate::style::PwswStyle;

// ============================================================================
// Constants
// ============================================================================

/// Notification title when daemon starts
const NOTIFICATION_STARTED_TITLE: &str = "PWSW Started";
/// Notification message when daemon starts
const NOTIFICATION_STARTED_MSG: &str = "Audio switcher running";
/// Notification title when daemon stops
const NOTIFICATION_STOPPED_TITLE: &str = "PWSW Stopped";
/// Notification message when daemon stops
const NOTIFICATION_STOPPED_MSG: &str = "Audio switcher stopped";

// ============================================================================
// Data Structures
// ============================================================================

/// Context data passed to IPC request handlers
struct IpcContext {
    version: String,
    uptime_secs: u64,
    current_sink_name: String,
    active_window: Option<String>,
    // tracked: (id, app_id, title, sink_name, sink_desc)
    tracked_with_sinks: Vec<(u64, String, String, String, String)>,
    // all windows: (id, app_id, title)
    all_windows: Vec<(u64, String, String)>,
    config: Config,
    shutdown_tx: broadcast::Sender<()>,
}

/// Run the daemon with the given configuration
///
/// # Errors
/// Returns an error if another daemon is running, initialization fails, compositor
/// connection fails, or any critical component encounters an error.
#[allow(clippy::too_many_lines, clippy::items_after_statements)]
pub async fn run(config: Config, foreground: bool) -> Result<()> {
    // Check if a daemon is already running BEFORE any initialization
    if ipc::is_daemon_running().await {
        let socket_path = ipc::get_socket_path()?;
        anyhow::bail!(
            "Another PWSW daemon is already running.\n\
             Socket: {}\n\n\
             To stop the existing daemon, run:\n  \
             pwsw shutdown",
            socket_path.display()
        );
    }

    // Background mode: spawn detached process and wait for successful startup
    if !foreground {
        use std::process::Command;
        use std::time::Duration;

        // Get current executable path
        let exe = std::env::current_exe()?;

        // Spawn detached daemon process with --foreground flag
        let mut child = Command::new(&exe)
            .arg("daemon")
            .arg("--foreground")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .with_context(|| "Failed to spawn background daemon")?;

        let pid = child.id();
        println!("Starting daemon (PID: {})...", pid.to_string().technical());

        // Wait for daemon to initialize by checking if it's responding to IPC
        // Try for up to 2 seconds (20 attempts * 100ms)
        const MAX_ATTEMPTS: u32 = 20;
        const RETRY_DELAY_MS: u64 = 100;

        for attempt in 1..=MAX_ATTEMPTS {
            tokio::time::sleep(Duration::from_millis(RETRY_DELAY_MS)).await;

            if ipc::is_daemon_running().await {
                println!(
                    "{} {}",
                    "✓".success(),
                    "Daemon started successfully".success()
                );
                println!("Use {} to check daemon status", "pwsw status".technical());
                println!("Use {} to stop the daemon", "pwsw shutdown".technical());
                return Ok(());
            }

            // Check if child process is still alive (every 500ms)
            if attempt % 5 == 0 {
                match child.try_wait() {
                    Ok(Some(status)) => {
                        anyhow::bail!(
                            "Daemon process (PID: {pid}) exited during startup with status: {status}\n\
                             Check logs with: journalctl --user -xe | grep pwsw"
                        );
                    }
                    Ok(None) => {} // Still running
                    Err(e) => {
                        warn!("Could not check daemon process status: {}", e);
                    }
                }
            }
        }

        anyhow::bail!(
            "Daemon failed to start within {} seconds.\n\
             Process may still be initializing. Check with: pwsw status",
            (u64::from(MAX_ATTEMPTS) * RETRY_DELAY_MS) / 1000
        );
    }

    // Validate required PipeWire tools are available
    PipeWire::validate_tools().context("PipeWire tools validation failed")?;

    // Initialize logging with config log_level (foreground mode only)
    // Filter format: "pwsw=LEVEL" ensures only our crate logs at the configured level
    let filter = tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        tracing_subscriber::EnvFilter::new(format!("pwsw={}", config.settings.log_level))
    });

    tracing_subscriber::fmt().with_env_filter(filter).init();

    info!("Starting PWSW daemon {}", crate::version_string());
    info!(
        "Loaded {} sinks, {} rules",
        config.sinks.len(),
        config.rules.len()
    );

    let start_time = Instant::now();
    let mut state = State::new(config)?;

    // Create shutdown channel with larger buffer to handle concurrent subscribers
    let (shutdown_tx, mut shutdown_rx) = broadcast::channel::<()>(8);

    // Switch to default on startup if configured
    if state.config.settings.default_on_startup {
        let default = state
            .config
            .get_default_sink()
            .ok_or_else(|| anyhow::anyhow!("No default sink configured"))?;
        if state.current_sink_name != default.name {
            info!("Switching to default sink: {}", default.desc);

            // Run activation in blocking thread pool to avoid blocking the async runtime
            let name_clone = default.name.clone();
            let desc_clone = default.desc.clone();
            let join = tokio::task::spawn_blocking(move || {
                crate::state::switch_audio_blocking(&name_clone, &desc_clone, None, None, false)
            });

            let inner = join
                .await
                .map_err(|e| anyhow::anyhow!("Join error: {e:#}"))?;
            inner?;

            state.current_sink_name.clone_from(&default.name);
        }
    }

    // Spawn compositor event thread
    let mut window_events = compositor::spawn_compositor_thread()?;
    info!("Compositor event thread started");

    // Start IPC server
    let ipc_server = IpcServer::bind().await?;
    info!("IPC server listening on {:?}", ipc_server.socket_path());

    // Setup config file watcher (hot-reload)
    let config_path = Config::get_config_path()?;
    let config_dir = config_path.parent().unwrap_or(&config_path);
    let (config_tx, mut config_rx) = tokio::sync::mpsc::channel::<()>(1);
    let config_tx_clone = config_tx.clone();

    // Only notify reloads for changes to the exact config file and avoid blocking the
    // watcher thread by using `try_send` (channel capacity 1 coalesces rapid events).
    let config_path_clone = config_path.clone();
    let mut watcher = notify::recommended_watcher(move |res: Result<Event, notify::Error>| {
        match res {
            Ok(event) => {
                if (event.kind.is_modify() || event.kind.is_create())
                    && event.paths.iter().any(|p| p == &config_path_clone)
                {
                    // Don't block the watcher thread; if the channel is full, drop the event
                    let _ = config_tx_clone.try_send(());
                }
            }
            Err(e) => error!("Config watch error: {:?}", e),
        }
    })?;

    if let Err(e) = watcher.watch(config_dir, RecursiveMode::NonRecursive) {
        warn!("Failed to watch config directory for hot-reload: {}", e);
    }

    if state.config.settings.notify_manual {
        if let Err(e) =
            send_notification(NOTIFICATION_STARTED_TITLE, NOTIFICATION_STARTED_MSG, None)
        {
            warn!("Could not send startup notification: {}", e);
        }
    }

    info!("Monitoring window events...");

    // Main event loop
    loop {
        tokio::select! {
            result = window_events.recv() => {
                if let Some(event) = result {
                    if let Err(e) = state.process_event(event).await {
                        error!("Event processing error: {e:#}", e = e);
                    }
                } else {
                    error!("Compositor connection lost (event channel closed)");
                    break;
                }
            }

            Some(mut stream) = ipc_server.accept() => {
                // Handle IPC request - clone what we need for the task
                // Tracked windows: (id, app_id, title, sink_name, sink_desc)
                let tracked_with_sinks = state.get_tracked_windows_with_sinks();
                // All windows: (id, app_id, title)
                let all_windows = state.get_all_windows();

                let ctx = IpcContext {
                    version: crate::version_string(),
                    uptime_secs: start_time.elapsed().as_secs(),
                    current_sink_name: state.current_sink_name.clone(),
                    active_window: state.get_most_recent_window()
                        .map(|w| format!("{}: {}", w.trigger_desc, w.sink_name)),
                    tracked_with_sinks,
                    all_windows,
                    config: state.config.clone(),
                    shutdown_tx: shutdown_tx.clone(),
                };

                tokio::spawn(async move {
                    if let Err(e) = handle_ipc_request(&mut stream, ctx).await {
                        // Check if this is a benign health check connection (client disconnects before sending data)
                        let is_health_check = e.chain()
                            .any(|cause| {
                                cause.downcast_ref::<std::io::Error>()
                                    .is_some_and(|io_err| io_err.kind() == std::io::ErrorKind::UnexpectedEof)
                            });

                        if is_health_check {
                            tracing::debug!("Client disconnected without sending data (likely health check)");
                        } else {
                            error!("IPC request handling error: {e:#}", e = e);
                        }
                    }
                });
            }

            _ = config_rx.recv() => {
                // Debounce happens partly due to select! loop speed, but we should be careful.
                info!("Config file changed, attempting reload...");
                match Config::load() {
                    Ok(new_config) => {
                        let notify_enabled = state.config.settings.notify_manual;
                        state.reload_config(new_config);
                        if notify_enabled {
                            let _ = send_notification("Configuration Reloaded", "New settings applied successfully", None);
                        }
                    },
                    Err(e) => {
                        error!("Failed to reload config: {e:#}", e = e);
                        if state.config.settings.notify_manual {
                            let _ = send_notification("Reload Failed", &format!("Config error: {e:#}"), None);
                        }
                    }
                }
            }

            _ = signal::ctrl_c() => {
                info!("Shutting down (Ctrl-C)");
                if state.config.settings.notify_manual {
                    let _ = send_notification(NOTIFICATION_STOPPED_TITLE, NOTIFICATION_STOPPED_MSG, None);
                }
                break;
            }

            _ = shutdown_rx.recv() => {
                info!("Shutting down (IPC request)");
                if state.config.settings.notify_manual {
                    let _ = send_notification(NOTIFICATION_STOPPED_TITLE, NOTIFICATION_STOPPED_MSG, None);
                }
                break;
            }
        }
    }

    Ok(())
}

/// Handle a single IPC request from a client
#[allow(clippy::too_many_lines)]
async fn handle_ipc_request(stream: &mut tokio::net::UnixStream, ctx: IpcContext) -> Result<()> {
    let request = ipc::read_request(stream).await?;

    let response = match request {
        Request::Status => {
            // Get current sink description
            let current_sink = ctx
                .config
                .sinks
                .iter()
                .find(|s| s.name == ctx.current_sink_name)
                .map_or_else(|| ctx.current_sink_name.clone(), |s| s.desc.clone());

            Response::Status {
                version: ctx.version,
                uptime_secs: ctx.uptime_secs,
                current_sink,
                active_window: ctx.active_window,
                tracked_windows: ctx.tracked_with_sinks.len(),
            }
        }

        Request::ListWindows => {
            use std::collections::HashMap;

            // Build a map of tracked windows by id for quick lookup
            let tracked_map: HashMap<u64, (String, String)> = ctx
                .tracked_with_sinks
                .iter()
                .map(|(id, _app_id, _title, sink_name, sink_desc)| {
                    (*id, (sink_name.clone(), sink_desc.clone()))
                })
                .collect();

            // Build WindowInfo for all windows with tracking status using ids from all_windows
            let windows = ctx
                .all_windows
                .iter()
                .map(|(id, app_id, title)| {
                    // Find tracked info by id
                    let tracked_opt =
                        tracked_map
                            .get(id)
                            .map(|(sink_name, sink_desc)| ipc::TrackedInfo {
                                sink_name: sink_name.clone(),
                                sink_desc: sink_desc.clone(),
                            });

                    WindowInfo {
                        id: Some(*id),
                        app_id: app_id.clone(),
                        title: title.clone(),
                        matched_on: None,
                        tracked: tracked_opt,
                    }
                })
                .collect();

            Response::Windows { windows }
        }

        Request::TestRule { pattern } => match regex::Regex::new(&pattern) {
            Ok(regex) => {
                let matches = ctx
                    .all_windows
                    .iter()
                    .filter_map(|(id, app_id, title)| {
                        let app_id_match = regex.is_match(app_id);
                        let title_match = regex.is_match(title);

                        if app_id_match || title_match {
                            let matched_on = match (app_id_match, title_match) {
                                (true, true) => "both",
                                (true, false) => "app_id",
                                (false, true) => "title",
                                (false, false) => unreachable!("Already filtered by outer if"),
                            };

                            Some(WindowInfo {
                                id: Some(*id),
                                app_id: app_id.clone(),
                                title: title.clone(),
                                matched_on: Some(matched_on.to_string()),
                                tracked: None,
                            })
                        } else {
                            None
                        }
                    })
                    .collect();

                Response::RuleMatches { pattern, matches }
            }
            Err(e) => Response::Error {
                message: format!("Invalid regex pattern: {e}"),
            },
        },

        Request::SetSink { sink } => {
            // Resolve sink reference (name, description, or 1-indexed position)
            if let Some(target) = ctx.config.resolve_sink(&sink) {
                // Attempt to activate the sink via PipeWire
                match PipeWire::activate_sink(&target.name) {
                    Ok(()) => Response::Ok {
                        message: format!("Switched to sink: {}", target.desc),
                    },
                    Err(e) => Response::Error {
                        message: format!(
                            "Failed to activate sink '{target_desc}': {e:#}",
                            target_desc = target.desc,
                            e = e
                        ),
                    },
                }
            } else {
                // Build helpful error message with available sinks
                let available: Vec<_> = ctx
                    .config
                    .sinks
                    .iter()
                    .enumerate()
                    .map(|(i, s)| format!("{}. '{}'", i + 1, s.desc))
                    .collect();
                Response::Error {
                    message: format!(
                        "Unknown sink '{}'. Available: [{}]",
                        sink,
                        available.join(", ")
                    ),
                }
            }
        }

        Request::Shutdown => {
            info!("Shutdown requested via IPC");
            // Send response before shutting down
            ipc::write_response(
                stream,
                &Response::Ok {
                    message: "Daemon shutting down...".to_string(),
                },
            )
            .await?;

            // Signal shutdown to main loop
            let _ = ctx.shutdown_tx.send(());

            // Return without sending another response
            return Ok(());
        }
    };

    ipc::write_response(stream, &response).await?;
    Ok(())
}
