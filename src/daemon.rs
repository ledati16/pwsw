//! Daemon mode
//!
//! Runs the main event loop, listening to compositor window events
//! and switching audio sinks based on configured rules.

use anyhow::Result;
use tokio::signal;
use tracing::{error, info, warn};

use crate::compositor;
use crate::config::Config;
use crate::notification::send_notification;
use crate::pipewire::PipeWire;
use crate::state::State;

/// Run the daemon with the given configuration
pub async fn run(config: Config) -> Result<()> {
    // Initialize logging with config log_level
    // Filter format: "nasw=LEVEL" ensures only our crate logs at the configured level
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| {
            tracing_subscriber::EnvFilter::new(format!("nasw={}", config.settings.log_level))
        });

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .init();

    info!("Starting NASW daemon");
    info!("Loaded {} sinks, {} rules", config.sinks.len(), config.rules.len());

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

    if state.config.settings.notify_daemon {
        if let Err(e) = send_notification("NASW Started", "Audio switcher running", None) {
            warn!("Could not send startup notification: {}", e);
        }
    }

    info!("Monitoring window events...");

    // Main event loop
    loop {
        tokio::select! {
            Some(event) = window_events.recv() => {
                if let Err(e) = state.process_event(event) {
                    error!("Event processing error: {:#}", e);
                }
            }
            _ = signal::ctrl_c() => {
                info!("Shutting down");
                if state.config.settings.notify_daemon {
                    let _ = send_notification("NASW Stopped", "Audio switcher stopped", None);
                }
                break;
            }
        }
    }

    Ok(())
}
