//! Compositor abstraction layer
//!
//! Provides window event streams from Wayland compositors using standard protocols:
//! - wlr-foreign-toplevel-management (Sway, Hyprland, Niri, River, labwc, dwl, hikari, Wayfire)
//! - plasma-window-management (KDE Plasma/KWin)

mod wlr_toplevel;
mod plasma;

use anyhow::{Context, Result};
use tokio::sync::mpsc;
use tracing::{error, info};
use wayland_client::Connection;

/// Window event from a compositor
#[derive(Debug, Clone)]
pub enum WindowEvent {
    /// A new window was opened
    Opened {
        id: u64,
        app_id: String,
        title: String,
    },
    /// An existing window's properties changed
    Changed {
        id: u64,
        app_id: String,
        title: String,
    },
    /// A window was closed
    Closed {
        id: u64,
    },
}

/// Spawn a dedicated thread for Wayland event processing
///
/// This function connects to the Wayland display, detects which protocols are available,
/// and spawns a dedicated thread to run the Wayland event loop. Window events are sent
/// back to the caller via an unbounded mpsc channel.
///
/// # Returns
///
/// An unbounded receiver for `WindowEvent`s from the compositor.
///
/// # Errors
///
/// Returns an error if:
/// - No Wayland display connection can be established
/// - No supported window management protocol is available
///
/// # Protocol Detection
///
/// The function tries protocols in this order:
/// 1. `zwlr_foreign_toplevel_manager_v1` (wlroots-based compositors)
/// 2. `org_kde_plasma_window_management` (KDE Plasma/KWin)
///
/// # Supported Compositors
///
/// - **Sway** - wlr-foreign-toplevel
/// - **Hyprland** - wlr-foreign-toplevel  
/// - **Niri** - wlr-foreign-toplevel
/// - **River** - wlr-foreign-toplevel
/// - **Wayfire** - wlr-foreign-toplevel
/// - **labwc** - wlr-foreign-toplevel
/// - **dwl** - wlr-foreign-toplevel
/// - **hikari** - wlr-foreign-toplevel
/// - **KDE Plasma/KWin** - plasma-window-management
///
/// **Note:** GNOME/Mutter does not expose any window management protocol and is not supported.
pub fn spawn_compositor_thread() -> Result<mpsc::UnboundedReceiver<WindowEvent>> {
    // Connect to Wayland display
    let conn = Connection::connect_to_env()
        .context("Failed to connect to Wayland display. Is a Wayland compositor running?")?;

    info!("Connected to Wayland display");

    // Create channel for sending events from Wayland thread to tokio runtime
    let (tx, rx) = mpsc::unbounded_channel();

    // Try to detect which protocol is available
    // We do this by attempting to bind to each protocol in order of preference
    
    // Clone connection for protocol detection
    let conn_clone = conn.clone();

    // Spawn dedicated thread for Wayland event loop
    std::thread::spawn(move || {
        // Try wlr-foreign-toplevel first (most widely supported)
        match wlr_toplevel::run_event_loop(conn_clone.clone(), tx.clone()) {
            Ok(()) => {
                info!("wlr-foreign-toplevel event loop ended normally");
            }
            Err(e) => {
                // Check if this is a "protocol not available" error
                let error_msg = format!("{:#}", e);
                let is_protocol_unavailable = error_msg.contains("not available") 
                    || error_msg.contains("protocol not available")
                    || error_msg.contains("not advertised");
                
                if is_protocol_unavailable {
                    info!("wlr-foreign-toplevel not available, trying plasma protocol...");
                    
                    match plasma::run_event_loop(conn_clone, tx) {
                        Ok(()) => {
                            info!("Plasma window management event loop ended normally");
                        }
                        Err(plasma_err) => {
                            error!(
                                "No supported window management protocol found.\n\
                                 Tried:\n\
                                 - zwlr_foreign_toplevel_manager_v1: {}\n\
                                 - org_kde_plasma_window_management: {}\n\n\
                                 Supported compositors:\n\
                                 - Sway, Hyprland, Niri, River, Wayfire, labwc, dwl, hikari (wlr-foreign-toplevel)\n\
                                 - KDE Plasma/KWin (plasma-window-management)\n\n\
                                 GNOME/Mutter is not supported (no window management protocol exposed).",
                                e, plasma_err
                            );
                        }
                    }
                } else {
                    // Some other error occurred (connection issue, etc.)
                    error!("Wayland event loop error: {:#}", e);
                }
            }
        }
    });

    Ok(rx)
}
