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

/// Run the daemon with the given configuration
pub async fn run(config: Config, foreground: bool) -> Result<()> {
    // Check if a daemon is already running BEFORE any initialization
    if ipc::is_daemon_running().await {
        let socket_path = ipc::get_socket_path()?;
        anyhow::bail!(
            "Another PWSW daemon is already running.\n\
             Socket: {:?}\n\n\
             To stop the existing daemon, run:\n  \
             pwsw shutdown",
            socket_path
        );
    }

    // Background mode: spawn detached process and exit
    if !foreground {
        use std::process::Command;

        // Get current executable path
        let exe = std::env::current_exe()?;

        // Spawn detached daemon process with --foreground flag
        let child = Command::new(&exe)
            .arg("daemon")
            .arg("--foreground")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .with_context(|| "Failed to spawn background daemon")?;

        println!("Daemon started in background (PID: {})", child.id());
        println!("Use 'pwsw status' to check daemon status");
        println!("Use 'pwsw shutdown' to stop the daemon");

        return Ok(());
    }

    // Validate required PipeWire tools are available
    PipeWire::validate_tools()
        .context("PipeWire tools validation failed")?;

    // Initialize logging with config log_level (foreground mode only)
    // Filter format: "pwsw=LEVEL" ensures only our crate logs at the configured level
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| {
            tracing_subscriber::EnvFilter::new(format!("pwsw={}", config.settings.log_level))
        });

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .init();

    info!("Starting PWSW daemon");
    info!("Loaded {} sinks, {} rules", config.sinks.len(), config.rules.len());

    let start_time = Instant::now();
    let mut state = State::new(config)?;
    
    // Create shutdown channel
    let (shutdown_tx, mut shutdown_rx) = broadcast::channel::<()>(1);

    // Reset to default on startup if configured
    if state.config.settings.reset_on_startup {
        let default = state.config.get_default_sink()
            .ok_or_else(|| anyhow::anyhow!("No default sink configured"))?;
        if state.current_sink_name != default.name {
            info!("Resetting to default: {}", default.desc);
            PipeWire::activate_sink(&default.name)?;
            state.current_sink_name = default.name.clone();
        }
    }

    // Spawn compositor event thread
    let mut window_events = compositor::spawn_compositor_thread()?;
    info!("Compositor event thread started");
    
    // Start IPC server
    let ipc_server = IpcServer::bind().await?;
    info!("IPC server listening on {:?}", ipc_server.socket_path());

    if state.config.settings.notify_daemon {
        if let Err(e) = send_notification("PWSW Started", "Audio switcher running", None) {
            warn!("Could not send startup notification: {}", e);
        }
    }

    info!("Monitoring window events...");

    // Main event loop
    loop {
        tokio::select! {
            result = window_events.recv() => {
                match result {
                    Some(event) => {
                        if let Err(e) = state.process_event(event) {
                            error!("Event processing error: {:#}", e);
                        }
                    }
                    None => {
                        error!("Compositor connection lost (event channel closed)");
                        break;
                    }
                }
            }
            
            Some(mut stream) = ipc_server.accept() => {
                // Handle IPC request - clone what we need for the task
                let uptime_secs = start_time.elapsed().as_secs();
                let version = env!("CARGO_PKG_VERSION").to_string();
                let current_sink_name = state.current_sink_name.clone();
                let config = state.config.clone();
                let tracked_with_sinks = state.get_tracked_windows_with_sinks();
                let all_windows = state.get_all_windows();
                let most_recent_window = state.get_most_recent_window()
                    .map(|w| format!("{}: {}", w.trigger_desc, w.sink_name));
                let shutdown_signal = shutdown_tx.clone();

                tokio::spawn(async move {
                    if let Err(e) = handle_ipc_request(
                        &mut stream,
                        version,
                        uptime_secs,
                        current_sink_name,
                        most_recent_window,
                        tracked_with_sinks,
                        all_windows,
                        config,
                        shutdown_signal,
                    ).await {
                        // "early eof" when reading message length is benign - it happens when
                        // clients connect just to check if daemon is running (is_daemon_running())
                        // or when they disconnect before sending data. Log at debug level.
                        let err_msg = format!("{:#}", e);
                        if err_msg.contains("early eof") && err_msg.contains("message length") {
                            tracing::debug!("Client disconnected without sending data (likely health check)");
                        } else {
                            error!("IPC request handling error: {}", err_msg);
                        }
                    }
                });
            }
            
            _ = signal::ctrl_c() => {
                info!("Shutting down (Ctrl-C)");
                if state.config.settings.notify_daemon {
                    let _ = send_notification("PWSW Stopped", "Audio switcher stopped", None);
                }
                break;
            }
            
            _ = shutdown_rx.recv() => {
                info!("Shutting down (IPC request)");
                if state.config.settings.notify_daemon {
                    let _ = send_notification("PWSW Stopped", "Audio switcher stopped", None);
                }
                break;
            }
        }
    }

    Ok(())
}

/// Handle a single IPC request from a client
async fn handle_ipc_request(
    stream: &mut tokio::net::UnixStream,
    version: String,
    uptime_secs: u64,
    current_sink_name: String,
    active_window: Option<String>,
    tracked_with_sinks: Vec<(String, String, String, String)>,
    all_windows: Vec<(String, String)>,
    config: Config,
    shutdown_tx: broadcast::Sender<()>,
) -> Result<()> {
    let request = ipc::read_request(stream).await?;
    
    let response = match request {
        Request::Status => {
            // Get current sink description
            let current_sink = config.sinks.iter()
                .find(|s| s.name == current_sink_name)
                .map(|s| s.desc.clone())
                .unwrap_or_else(|| current_sink_name.clone());

            Response::Status {
                version,
                uptime_secs,
                current_sink,
                active_window,
                tracked_windows: tracked_with_sinks.len(),
            }
        }

        Request::ListWindows => {
            use std::collections::HashMap;

            // Build a map of tracked windows for quick lookup
            let tracked_map: HashMap<(&str, &str), (&str, &str)> = tracked_with_sinks
                .iter()
                .map(|(app_id, title, sink_name, sink_desc)| {
                    ((app_id.as_str(), title.as_str()), (sink_name.as_str(), sink_desc.as_str()))
                })
                .collect();

            // Build WindowInfo for all windows with tracking status
            let windows = all_windows
                .iter()
                .map(|(app_id, title)| {
                    let tracked = tracked_map.get(&(app_id.as_str(), title.as_str()))
                        .map(|(sink_name, sink_desc)| ipc::TrackedInfo {
                            sink_name: sink_name.to_string(),
                            sink_desc: sink_desc.to_string(),
                        });

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
        
        Request::TestRule { pattern } => {
            match regex::Regex::new(&pattern) {
                Ok(regex) => {
                    let matches = all_windows
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
                Err(e) => {
                    Response::Error {
                        message: format!("Invalid regex pattern: {}", e),
                    }
                }
            }
        }
        
        Request::Shutdown => {
            info!("Shutdown requested via IPC");
            // Send response before shutting down
            ipc::write_response(stream, &Response::Ok {
                message: "Daemon shutting down...".to_string(),
            }).await?;
            
            // Signal shutdown to main loop
            let _ = shutdown_tx.send(());
            
            // Return without sending another response
            return Ok(());
        }
    };
    
    ipc::write_response(stream, &response).await?;
    Ok(())
}
