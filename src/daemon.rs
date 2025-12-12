//! Daemon mode
//!
//! Runs the main event loop, listening to compositor window events,
//! IPC requests, and switching audio sinks based on configured rules.

use anyhow::{Context, Result};
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
    tracked_with_sinks: Vec<(String, String, String, String)>,
    all_windows: Vec<(String, String)>,
    config: Config,
    shutdown_tx: broadcast::Sender<()>,
}

/// Run the daemon with the given configuration
///
/// # Errors
/// Returns an error if another daemon is running, initialization fails, compositor
/// connection fails, or any critical component encounters an error.
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
        println!("Starting daemon (PID: {pid})...");

        // Wait for daemon to initialize by checking if it's responding to IPC
        // Try for up to 2 seconds (20 attempts * 100ms)
        const MAX_ATTEMPTS: u32 = 20;
        const RETRY_DELAY_MS: u64 = 100;

        for attempt in 1..=MAX_ATTEMPTS {
            tokio::time::sleep(Duration::from_millis(RETRY_DELAY_MS)).await;

            if ipc::is_daemon_running().await {
                println!("✓ Daemon started successfully");
                println!("Use 'pwsw status' to check daemon status");
                println!("Use 'pwsw shutdown' to stop the daemon");
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

    info!("Starting PWSW daemon");
    info!(
        "Loaded {} sinks, {} rules",
        config.sinks.len(),
        config.rules.len()
    );

    let start_time = Instant::now();
    let mut state = State::new(config)?;

    // Create shutdown channel
    let (shutdown_tx, mut shutdown_rx) = broadcast::channel::<()>(1);

    // Reset to default on startup if configured
    if state.config.settings.reset_on_startup {
        let default = state
            .config
            .get_default_sink()
            .ok_or_else(|| anyhow::anyhow!("No default sink configured"))?;
        if state.current_sink_name != default.name {
            info!("Resetting to default: {}", default.desc);
            PipeWire::activate_sink(&default.name)?;
            state.current_sink_name.clone_from(&default.name);
        }
    }

    // Spawn compositor event thread
    let mut window_events = compositor::spawn_compositor_thread()?;
    info!("Compositor event thread started");

    // Start IPC server
    let ipc_server = IpcServer::bind().await?;
    info!("IPC server listening on {:?}", ipc_server.socket_path());

    if state.config.settings.notify_daemon {
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
                    if let Err(e) = state.process_event(event) {
                        error!("Event processing error: {:#}", e);
                    }
                } else {
                    error!("Compositor connection lost (event channel closed)");
                    break;
                }
            }

            Some(mut stream) = ipc_server.accept() => {
                // Handle IPC request - clone what we need for the task
                let ctx = IpcContext {
                    version: env!("CARGO_PKG_VERSION").to_string(),
                    uptime_secs: start_time.elapsed().as_secs(),
                    current_sink_name: state.current_sink_name.clone(),
                    active_window: state.get_most_recent_window()
                        .map(|w| format!("{}: {}", w.trigger_desc, w.sink_name)),
                    tracked_with_sinks: state.get_tracked_windows_with_sinks(),
                    all_windows: state.get_all_windows(),
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
                            error!("IPC request handling error: {:#}", e);
                        }
                    }
                });
            }

            _ = signal::ctrl_c() => {
                info!("Shutting down (Ctrl-C)");
                if state.config.settings.notify_daemon {
                    let _ = send_notification(NOTIFICATION_STOPPED_TITLE, NOTIFICATION_STOPPED_MSG, None);
                }
                break;
            }

            _ = shutdown_rx.recv() => {
                info!("Shutting down (IPC request)");
                if state.config.settings.notify_daemon {
                    let _ = send_notification(NOTIFICATION_STOPPED_TITLE, NOTIFICATION_STOPPED_MSG, None);
                }
                break;
            }
        }
    }

    Ok(())
}

/// Handle a single IPC request from a client
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

            // Build a map of tracked windows for quick lookup
            let tracked_map: HashMap<(&str, &str), (&str, &str)> = ctx
                .tracked_with_sinks
                .iter()
                .map(|(app_id, title, sink_name, sink_desc)| {
                    (
                        (app_id.as_str(), title.as_str()),
                        (sink_name.as_str(), sink_desc.as_str()),
                    )
                })
                .collect();

            // Build WindowInfo for all windows with tracking status
            let windows = ctx
                .all_windows
                .iter()
                .map(|(app_id, title)| {
                    let tracked = tracked_map.get(&(app_id.as_str(), title.as_str())).map(
                        |(sink_name, sink_desc)| ipc::TrackedInfo {
                            sink_name: (*sink_name).to_string(),
                            sink_desc: (*sink_desc).to_string(),
                        },
                    );

                    WindowInfo {
                        app_id: app_id.clone(),
                        title: title.clone(),
                        matched_on: None,
                        tracked,
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
                    .filter_map(|(app_id, title)| {
                        let app_id_match = regex.is_match(app_id);
                        let title_match = regex.is_match(title);

                        if app_id_match || title_match {
                            let matched_on = match (app_id_match, title_match) {
                                (true, true) => "both",
                                (true, false) => "app_id",
                                (false, true) => "title",
                                _ => unreachable!(),
                            };

                            Some(WindowInfo {
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
