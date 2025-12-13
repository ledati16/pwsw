//! Compositor abstraction layer
//!
//! Provides window event streams from Wayland compositors using standard protocols:
//! - wlr-foreign-toplevel-management (Sway, Hyprland, Niri, River, labwc, dwl, hikari, Wayfire)

mod wlr_toplevel;

use anyhow::{Context, Result};
use tokio::sync::mpsc;
use tracing::{error, info};
use wayland_client::{protocol::wl_registry, Connection};

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
    Closed { id: u64 },
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
///
/// **Note:** GNOME/Mutter and KDE Plasma do not expose window management protocols and are not supported.
pub fn spawn_compositor_thread() -> Result<mpsc::UnboundedReceiver<WindowEvent>> {
    // Connect to Wayland display
    let conn = Connection::connect_to_env()
        .context("Failed to connect to Wayland display. Is a Wayland compositor running?")?;

    info!("Connected to Wayland display");

    // Pre-detect available protocols by checking the registry
    detect_available_protocol(&conn)?;

    // Create channel for sending events from Wayland thread to tokio runtime
    let (tx, rx) = mpsc::unbounded_channel();

    // Spawn dedicated thread for Wayland event loop
    std::thread::spawn(move || {
        info!("Using wlr-foreign-toplevel-management protocol");
        let result = wlr_toplevel::run_event_loop(conn, tx);

        if let Err(e) = result {
            error!("Wayland event loop error: {:#}", e);
        }
    });

    Ok(rx)
}

/// Detect which window management protocol is available on this compositor
///
/// This function queries the Wayland registry to check if the wlr-foreign-toplevel-management
/// protocol is advertised by the compositor.
fn detect_available_protocol(conn: &Connection) -> Result<()> {
    use tracing::debug;
    use wayland_client::globals::{registry_queue_init, GlobalListContents};

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

    let mut state = RegistryState;

    // Do a roundtrip to get all globals
    event_queue
        .roundtrip(&mut state)
        .context("Failed to roundtrip registry")?;

    // Check the GlobalList for available protocols
    let contents = globals.contents();
    let mut has_wlr_foreign_toplevel = false;

    contents.with_list(|list| {
        for global in list {
            debug!(
                "Found global: {} (version {})",
                global.interface, global.version
            );
            if global.interface.as_str() == "zwlr_foreign_toplevel_manager_v1" {
                has_wlr_foreign_toplevel = true;
            }
        }
    });

    // Check if protocol is available
    if has_wlr_foreign_toplevel {
        return Ok(());
    }

    // No supported protocol found
    anyhow::bail!(
        "No supported window management protocol found.\n\
         \n\
         Supported protocols:\n\
         - zwlr_foreign_toplevel_manager_v1 (Sway, Hyprland, Niri, River, Wayfire, labwc, dwl, hikari)\n\
         \n\
         Unsupported compositors:\n\
         - GNOME/Mutter (no window management protocol exposed)\n\
         - KDE Plasma (removed protocol support in Plasma 6)"
    )
}
