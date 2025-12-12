//! Compositor abstraction layer
//!
//! Provides window event streams from Wayland compositors using standard protocols:
//! - wlr-foreign-toplevel-management (Sway, Hyprland, Niri, River, labwc, dwl, hikari, Wayfire) ✓ Tested
//! - plasma-window-management (KDE Plasma/KWin) ⚠️  Experimental/Untested

mod wlr_toplevel;
mod plasma;

use anyhow::{Context, Result};
use tokio::sync::mpsc;
use tracing::{error, info, warn};
use wayland_client::{Connection, protocol::wl_registry};

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
/// # Channel Behavior
///
/// The returned receiver will yield `None` (channel closed) when:
/// - The compositor disconnects
/// - The Wayland thread encounters a fatal error
/// - The compositor shuts down
///
/// Callers should handle channel closure gracefully by breaking out of
/// their event loop and performing cleanup.
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

    // Pre-detect available protocols by checking the registry
    let protocol = detect_available_protocol(&conn)?;

    // Create channel for sending events from Wayland thread to tokio runtime
    let (tx, rx) = mpsc::unbounded_channel();

    // Spawn dedicated thread for Wayland event loop
    std::thread::spawn(move || {
        let result = match protocol {
            Protocol::WlrForeignToplevel => {
                info!("Using wlr-foreign-toplevel-management protocol");
                wlr_toplevel::run_event_loop(conn, tx)
            }
            Protocol::PlasmaWindowManagement => {
                info!("Using plasma-window-management protocol");
                plasma::run_event_loop(conn, tx)
            }
        };
        
        if let Err(e) = result {
            error!("Wayland event loop error: {:#}", e);
        }
    });

    Ok(rx)
}

/// Protocol variants that can be used for window management
#[derive(Debug, Clone, Copy)]
enum Protocol {
    WlrForeignToplevel,
    PlasmaWindowManagement,
}

/// Detect which window management protocol is available on this compositor
///
/// This function queries the Wayland registry to check which protocols are advertised
/// by the compositor. Priority is given to wlr-foreign-toplevel-management as it's more
/// widely supported.
fn detect_available_protocol(conn: &Connection) -> Result<Protocol> {
    use wayland_client::globals::{registry_queue_init, GlobalListContents};
    use tracing::debug;

    // Temporary state for registry detection
    #[derive(Default)]
    struct RegistryState;

    // Implement Dispatch for registry
    impl wayland_client::Dispatch<wl_registry::WlRegistry, GlobalListContents> for RegistryState {
        fn event(
            _state: &mut Self,
            _proxy: &wl_registry::WlRegistry,
            _event: wl_registry::Event,
            _data: &GlobalListContents,
            _conn: &Connection,
            _qh: &wayland_client::QueueHandle<Self>,
        ) {
            // Events are handled automatically by GlobalList
        }
    }

    // Initialize registry to enumerate available globals
    let (globals, mut event_queue) = registry_queue_init::<RegistryState>(conn)
        .context("Failed to initialize Wayland registry")?;

    let mut state = RegistryState::default();

    // Do a roundtrip to get all globals
    event_queue.roundtrip(&mut state)
        .context("Failed to roundtrip registry")?;

    // Check the GlobalList for available protocols
    let contents = globals.contents();
    let mut has_wlr_foreign_toplevel = false;
    let mut has_plasma_window_management = false;

    contents.with_list(|list| {
        for global in list {
            debug!("Found global: {} (version {})", global.interface, global.version);
            match global.interface.as_str() {
                "zwlr_foreign_toplevel_manager_v1" => {
                    has_wlr_foreign_toplevel = true;
                }
                "org_kde_plasma_window_management" => {
                    has_plasma_window_management = true;
                }
                _ => {}
            }
        }
    });

    // Check for protocols in order of preference
    if has_wlr_foreign_toplevel {
        return Ok(Protocol::WlrForeignToplevel);
    }

    if has_plasma_window_management {
        warn!("⚠️  Detected KDE Plasma - support is EXPERIMENTAL and may not work correctly");
        warn!("   If you experience issues, please report at https://github.com/ledati16/pwsw/issues");
        return Ok(Protocol::PlasmaWindowManagement);
    }

    // No supported protocol found
    anyhow::bail!(
        "No supported window management protocol found.\n\
         \n\
         Supported protocols:\n\
         - zwlr_foreign_toplevel_manager_v1 (Sway, Hyprland, Niri, River, Wayfire, labwc, dwl, hikari)\n\
         - org_kde_plasma_window_management (KDE Plasma/KWin)\n\
         \n\
         GNOME/Mutter is not supported (no window management protocol exposed)."
    )
}
