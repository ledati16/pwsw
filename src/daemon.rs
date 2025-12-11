//! Daemon mode
//!
//! Runs the main event loop, listening to compositor window events,
//! IPC requests, and switching audio sinks based on configured rules.

use anyhow::Result;
use std::time::Instant;
use tokio::signal;
use tracing::{error, info, warn};

use crate::compositor;
use crate::config::Config;
use crate::ipc::{self, IpcServer, Request, Response, WindowInfo};
use crate::notification::send_notification;
use crate::pipewire::PipeWire;
use crate::state::State;

/// Run the daemon with the given configuration
pub async fn run(config: Config) -> Result<()> {
    // Initialize logging with config log_level
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

    // Reset to default on startup if configured
    if state.config.settings.reset_on_startup {
        let default = state.config.get_default_sink();
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
                let tracked_windows = state.get_tracked_windows();
                let most_recent_window = state.get_most_recent_window()
                    .map(|w| format!("{}: {}", w.trigger_desc, w.sink_name));
                
                tokio::spawn(async move {
                    if let Err(e) = handle_ipc_request(
                        &mut stream,
                        version,
                        uptime_secs,
                        current_sink_name,
                        most_recent_window,
                        tracked_windows,
                        config,
                    ).await {
                        error!("IPC request handling error: {:#}", e);
                    }
                });
            }
            
            _ = signal::ctrl_c() => {
                info!("Shutting down");
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
    tracked_windows: Vec<(String, String)>,
    config: Config,
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
            }
        }
        
        Request::Reload => {
            match Config::load() {
                Ok(_new_config) => {
                    info!("Config reloaded successfully");
                    // Note: We can't actually update the state here because we're in a separate task.
                    // A full implementation would require a channel to send reload requests to the main loop.
                    Response::Ok {
                        message: "Config validated. Note: Full reload requires daemon restart for now.".to_string(),
                    }
                }
                Err(e) => {
                    warn!("Config reload failed: {}", e);
                    Response::Error {
                        message: format!("Config validation failed: {}", e),
                    }
                }
            }
        }
        
        Request::ListWindows => {
            let windows = tracked_windows
                .iter()
                .map(|(app_id, title)| WindowInfo {
                    app_id: app_id.clone(),
                    title: title.clone(),
                })
                .collect();
            
            Response::Windows { windows }
        }
        
        Request::TestRule { pattern } => {
            match regex::Regex::new(&pattern) {
                Ok(regex) => {
                    let matches = tracked_windows
                        .iter()
                        .filter(|(app_id, _)| regex.is_match(app_id))
                        .map(|(app_id, title)| WindowInfo {
                            app_id: app_id.clone(),
                            title: title.clone(),
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
            
            // Exit the process
            std::process::exit(0);
        }
    };
    
    ipc::write_response(stream, &response).await?;
    Ok(())
}
