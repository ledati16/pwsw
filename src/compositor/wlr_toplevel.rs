//! wlr-foreign-toplevel-management protocol implementation
//!
//! This protocol is supported by Sway, Hyprland, Wayfire, River, labwc, dwl, hikari, Niri,
//! and other wlroots-based compositors.

use anyhow::{Context, Result};
use std::collections::HashMap;
use tokio::sync::mpsc;
use tracing::{debug, trace, warn};
use wayland_client::{
    Connection, Dispatch, Proxy, QueueHandle,
    globals::{registry_queue_init, GlobalListContents},
    protocol::{wl_registry, wl_output},
};
use wayland_protocols_wlr::foreign_toplevel::v1::client::{
    zwlr_foreign_toplevel_handle_v1, zwlr_foreign_toplevel_manager_v1,
};

use super::WindowEvent;

/// State for handling toplevel windows
pub struct WlrToplevelState {
    /// Channel to send window events back to tokio runtime
    tx: mpsc::UnboundedSender<WindowEvent>,
    /// Toplevels being tracked (handle object_id -> window state)
    toplevels: HashMap<u32, ToplevelWindow>,
}

/// A single toplevel window being tracked
#[derive(Debug, Default)]
struct ToplevelWindow {
    /// Unique ID for this window (derived from Wayland object ID)
    id: u64,
    /// Application ID (e.g., "firefox", "steam")
    app_id: String,
    /// Window title
    title: String,
    /// Whether we've received initial data (waiting for first 'done' event)
    done_received: bool,
}

impl WlrToplevelState {
    pub fn new(tx: mpsc::UnboundedSender<WindowEvent>) -> Self {
        Self {
            tx,
            toplevels: HashMap::new(),
        }
    }

    /// Send a window event to the daemon
    fn send_event(&self, event: WindowEvent) {
        if let Err(e) = self.tx.send(event) {
            warn!("Failed to send window event (receiver dropped): {}", e);
        }
    }
}

// Implement Dispatch for the foreign toplevel manager
impl Dispatch<zwlr_foreign_toplevel_manager_v1::ZwlrForeignToplevelManagerV1, ()> for WlrToplevelState {
    fn event(
        state: &mut Self,
        _proxy: &zwlr_foreign_toplevel_manager_v1::ZwlrForeignToplevelManagerV1,
        event: zwlr_foreign_toplevel_manager_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        use zwlr_foreign_toplevel_manager_v1::Event;

        match event {
            Event::Toplevel { toplevel } => {
                // New window handle received
                let id = toplevel.id().protocol_id();
                trace!("New toplevel handle: {}", id);

                // Register this handle with the event queue
                state.toplevels.insert(id, ToplevelWindow {
                    id: id as u64,
                    ..Default::default()
                });
            }
            Event::Finished => {
                debug!("Toplevel manager finished");
            }
            _ => {}
        }
    }
}

// Implement Dispatch for individual toplevel handles
impl Dispatch<zwlr_foreign_toplevel_handle_v1::ZwlrForeignToplevelHandleV1, ()> for WlrToplevelState {
    fn event(
        state: &mut Self,
        proxy: &zwlr_foreign_toplevel_handle_v1::ZwlrForeignToplevelHandleV1,
        event: zwlr_foreign_toplevel_handle_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        use zwlr_foreign_toplevel_handle_v1::Event;

        let handle_id = proxy.id().protocol_id();

        match event {
            Event::Title { title } => {
                trace!("Toplevel {} title: {}", handle_id, title);
                if let Some(window) = state.toplevels.get_mut(&handle_id) {
                    window.title = title;
                }
            }
            Event::AppId { app_id } => {
                trace!("Toplevel {} app_id: {}", handle_id, app_id);
                if let Some(window) = state.toplevels.get_mut(&handle_id) {
                    window.app_id = app_id;
                }
            }
            Event::Done => {
                // All properties have been sent, emit event
                if let Some(window) = state.toplevels.get_mut(&handle_id) {
                    if window.done_received {
                        // Subsequent done events = window changed
                        let id = window.id;
                        let app_id = window.app_id.clone();
                        let title = window.title.clone();
                        trace!("Window changed: id={}, app_id='{}', title='{}'", id, app_id, title);
                        state.send_event(WindowEvent::Changed {
                            id,
                            app_id,
                            title,
                        });
                    } else {
                        // First done event = window opened
                        window.done_received = true;
                        let id = window.id;
                        let app_id = window.app_id.clone();
                        let title = window.title.clone();
                        debug!("Window opened: id={}, app_id='{}', title='{}'", id, app_id, title);
                        state.send_event(WindowEvent::Opened {
                            id,
                            app_id,
                            title,
                        });
                    }
                }
            }
            Event::Closed => {
                if let Some(window) = state.toplevels.remove(&handle_id) {
                    // Only send Closed if we previously sent Opened
                    if window.done_received {
                        debug!("Window closed: id={}", window.id);
                        state.send_event(WindowEvent::Closed { id: window.id });
                    } else {
                        debug!("Window {} closed before initial done event, not emitting Closed", handle_id);
                    }
                }
            }
            Event::State { state: _ } => {
                // We don't care about state changes (minimized, maximized, etc.)
                trace!("Toplevel {} state changed", handle_id);
            }
            Event::OutputEnter { output: _ } => {
                // We don't care about which output the window is on
                trace!("Toplevel {} entered output", handle_id);
            }
            Event::OutputLeave { output: _ } => {
                // We don't care about which output the window is on
                trace!("Toplevel {} left output", handle_id);
            }
            _ => {}
        }
    }
}

// Stub implementations for registry and output (required for event queue)
impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for WlrToplevelState {
    fn event(
        _state: &mut Self,
        _proxy: &wl_registry::WlRegistry,
        _event: wl_registry::Event,
        _data: &GlobalListContents,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        // No-op: we handle registry in the init phase
    }
}

impl Dispatch<wl_output::WlOutput, ()> for WlrToplevelState {
    fn event(
        _state: &mut Self,
        _proxy: &wl_output::WlOutput,
        _event: wl_output::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        // No-op: we don't care about output events
    }
}

/// Run the Wayland event loop for wlr-foreign-toplevel-management
///
/// This function runs in a dedicated thread and dispatches Wayland events.
pub fn run_event_loop(
    conn: Connection,
    tx: mpsc::UnboundedSender<WindowEvent>,
) -> Result<()> {
    let (globals, mut event_queue) = registry_queue_init::<WlrToplevelState>(&conn)
        .context("Failed to initialize Wayland registry")?;
    
    let qh = event_queue.handle();

    // Bind to the foreign toplevel manager protocol
    let _manager: zwlr_foreign_toplevel_manager_v1::ZwlrForeignToplevelManagerV1 = globals
        .bind(&qh, 1..=3, ())
        .context("zwlr_foreign_toplevel_manager_v1 protocol not available")?;

    debug!("Successfully bound to wlr-foreign-toplevel-management protocol");

    let mut state = WlrToplevelState::new(tx);

    // Main event loop - dispatches Wayland events
    // This will return an error if the compositor disconnects or the connection breaks
    loop {
        match event_queue.blocking_dispatch(&mut state) {
            Ok(_) => {
                // Check if receiver dropped (daemon shutting down)
                if state.tx.is_closed() {
                    debug!("Event receiver dropped, shutting down Wayland thread");
                    return Ok(());
                }
            }
            Err(e) => {
                // Connection closed or compositor shut down
                debug!("Wayland event dispatch ended: {}", e);
                return Err(e).context("Wayland event dispatch failed");
            }
        }
    }
}
