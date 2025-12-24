//! ext-foreign-toplevel-list-v1 protocol implementation
//!
//! This is the standard protocol for listing toplevel windows.
//! It provides read-only access to window metadata (title, `app_id`).

use color_eyre::eyre::{Context, Result};
use std::collections::HashMap;
use tokio::sync::mpsc;
use tracing::{debug, trace, warn};
use wayland_client::{
    Connection, Dispatch, Proxy, QueueHandle,
    globals::{GlobalListContents, registry_queue_init},
    protocol::{wl_output, wl_registry},
};
use wayland_protocols::ext::foreign_toplevel_list::v1::client::{
    ext_foreign_toplevel_handle_v1, ext_foreign_toplevel_list_v1,
};

use super::WindowEvent;

/// State for handling toplevel windows via ext-foreign-toplevel-list
pub struct ExtToplevelState {
    /// Channel to send window events back to tokio runtime
    tx: mpsc::Sender<WindowEvent>,
    /// Toplevels being tracked (handle `object_id` -> window state)
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

impl ExtToplevelState {
    pub fn new(tx: mpsc::Sender<WindowEvent>) -> Self {
        Self {
            tx,
            toplevels: HashMap::new(),
        }
    }

    /// Send a window event to the daemon
    fn send_event(&self, event: WindowEvent) {
        // Use blocking_send since we're in a dedicated thread (not async context)
        match self.tx.blocking_send(event) {
            Ok(()) => {}
            Err(e) => {
                warn!("Failed to send window event (receiver dropped): {}", e);
            }
        }
    }
}

// Implement Dispatch for the foreign toplevel list manager
impl Dispatch<ext_foreign_toplevel_list_v1::ExtForeignToplevelListV1, ()> for ExtToplevelState {
    fn event(
        state: &mut Self,
        _proxy: &ext_foreign_toplevel_list_v1::ExtForeignToplevelListV1,
        event: ext_foreign_toplevel_list_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        use ext_foreign_toplevel_list_v1::Event;

        match event {
            Event::Toplevel { toplevel } => {
                // New window handle received
                let id = toplevel.id().protocol_id();
                trace!("New ext-toplevel handle: {}", id);

                // Register this handle
                state.toplevels.insert(
                    id,
                    ToplevelWindow {
                        id: u64::from(id),
                        ..Default::default()
                    },
                );
            }
            Event::Finished => {
                debug!("Ext-toplevel list finished");
            }
            _ => {}
        }
    }

    wayland_client::event_created_child!(ExtToplevelState, ext_foreign_toplevel_list_v1::ExtForeignToplevelListV1, [
        ext_foreign_toplevel_list_v1::EVT_TOPLEVEL_OPCODE => (ext_foreign_toplevel_handle_v1::ExtForeignToplevelHandleV1, ())
    ]);
}

// Implement Dispatch for individual toplevel handles
impl Dispatch<ext_foreign_toplevel_handle_v1::ExtForeignToplevelHandleV1, ()> for ExtToplevelState {
    fn event(
        state: &mut Self,
        proxy: &ext_foreign_toplevel_handle_v1::ExtForeignToplevelHandleV1,
        event: ext_foreign_toplevel_handle_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        use ext_foreign_toplevel_handle_v1::Event;

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
                // All properties have been sent (or updated), emit event
                if let Some(window) = state.toplevels.get_mut(&handle_id) {
                    if window.done_received {
                        // Subsequent done events = window changed
                        let id = window.id;
                        let app_id = window.app_id.clone();
                        let title = window.title.clone();
                        trace!(
                            "Window changed: id={}, app_id='{}', title='{}'",
                            id, app_id, title
                        );
                        state.send_event(WindowEvent::Changed { id, app_id, title });
                    } else {
                        // First done event = window opened
                        window.done_received = true;
                        let id = window.id;
                        let app_id = window.app_id.clone();
                        let title = window.title.clone();
                        debug!(
                            "Window opened: id={}, app_id='{}', title='{}'",
                            id, app_id, title
                        );
                        state.send_event(WindowEvent::Opened { id, app_id, title });
                    }
                }
            }
            Event::Closed => {
                if let Some(window) = state.toplevels.remove(&handle_id) {
                    if window.done_received {
                        debug!("Window closed: id={}", window.id);
                        state.send_event(WindowEvent::Closed { id: window.id });
                    } else {
                        debug!("Window {} closed before initial done event", handle_id);
                    }
                }
            }
            _ => {}
        }
    }
}

// Stub implementations for registry and output
impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for ExtToplevelState {
    fn event(
        _state: &mut Self,
        _proxy: &wl_registry::WlRegistry,
        _event: wl_registry::Event,
        _data: &GlobalListContents,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_output::WlOutput, ()> for ExtToplevelState {
    fn event(
        _state: &mut Self,
        _proxy: &wl_output::WlOutput,
        _event: wl_output::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

/// Run the `Wayland` event loop for `ext-foreign-toplevel-list-v1`
pub fn run_event_loop(conn: &Connection, tx: mpsc::Sender<WindowEvent>) -> Result<()> {
    let (globals, mut event_queue) = registry_queue_init::<ExtToplevelState>(conn)
        .context("Failed to initialize Wayland registry")?;

    let qh = event_queue.handle();

    // Bind to the ext foreign toplevel list protocol
    // Version 1 is the current version
    let _list: ext_foreign_toplevel_list_v1::ExtForeignToplevelListV1 = globals
        .bind(&qh, 1..=1, ())
        .context("ext_foreign_toplevel_list_v1 protocol not available")?;

    debug!("Successfully bound to ext-foreign-toplevel-list-v1 protocol");

    let mut state = ExtToplevelState::new(tx);

    loop {
        match event_queue.blocking_dispatch(&mut state) {
            Ok(_) => {
                if state.tx.is_closed() {
                    debug!("Event receiver dropped, shutting down Wayland thread");
                    return Ok(());
                }
            }
            Err(e) => {
                debug!("Wayland event dispatch ended: {}", e);
                return Err(e).context("Wayland event dispatch failed");
            }
        }
    }
}
