//! KDE Plasma window-management protocol implementation
//!
//! This protocol is supported by KWin (KDE Plasma's window manager).
//!
//! ## Testing Note
//!
//! The Plasma protocol implementation uses `proxy.id().protocol_id()` to track windows,
//! which may not always match the window IDs from the manager's `Window` event.
//! This implementation needs testing on actual KDE Plasma to verify correctness.

use anyhow::{Context, Result};
use tokio::sync::mpsc;
use tracing::{debug, trace, warn};
use wayland_client::{
    Connection, Dispatch, Proxy, QueueHandle,
    globals::{registry_queue_init, GlobalListContents},
    protocol::{wl_registry, wl_output},
};
use wayland_protocols_plasma::plasma_window_management::client::{
    org_kde_plasma_window_management, org_kde_plasma_window,
};

use super::WindowEvent;
use std::collections::HashMap;

/// State for handling KDE Plasma windows
pub struct PlasmaState {
    /// Channel to send window events back to tokio runtime
    tx: mpsc::UnboundedSender<WindowEvent>,
    /// Windows being tracked
    windows: HashMap<u32, PlasmaWindow>,
}

/// A single Plasma window being tracked
#[derive(Debug, Default)]
struct PlasmaWindow {
    /// Unique ID for this window
    id: u64,
    /// Application ID
    app_id: String,
    /// Window title
    title: String,
    /// Whether window is active/ready
    ready: bool,
}

impl PlasmaState {
    pub fn new(tx: mpsc::UnboundedSender<WindowEvent>) -> Self {
        Self {
            tx,
            windows: HashMap::new(),
        }
    }

    /// Send a window event to the daemon
    fn send_event(&self, event: WindowEvent) {
        if let Err(e) = self.tx.send(event) {
            warn!("Failed to send window event (receiver dropped): {}", e);
        }
    }
}

// Implement Dispatch for the plasma window manager
impl Dispatch<org_kde_plasma_window_management::OrgKdePlasmaWindowManagement, ()> for PlasmaState {
    fn event(
        state: &mut Self,
        _proxy: &org_kde_plasma_window_management::OrgKdePlasmaWindowManagement,
        event: org_kde_plasma_window_management::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        use org_kde_plasma_window_management::Event;

        match event {
            Event::Window { id: window_id } => {
                // New window announced - window_id is a u32 directly
                debug!("New plasma window: {}", window_id);
                
                state.windows.insert(window_id, PlasmaWindow {
                    id: window_id as u64,
                    ..Default::default()
                });
            }
            _ => {}
        }
    }
}

// Implement Dispatch for individual plasma windows
impl Dispatch<org_kde_plasma_window::OrgKdePlasmaWindow, ()> for PlasmaState {
    fn event(
        state: &mut Self,
        proxy: &org_kde_plasma_window::OrgKdePlasmaWindow,
        event: org_kde_plasma_window::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        use org_kde_plasma_window::Event;

        let handle_id = proxy.id().protocol_id();

        match event {
            Event::TitleChanged { title } => {
                trace!("Plasma window {} title: {}", handle_id, title);
                if let Some(window) = state.windows.get_mut(&handle_id) {
                    window.title = title;
                    if window.ready {
                        let event = WindowEvent::Changed {
                            id: window.id,
                            app_id: window.app_id.clone(),
                            title: window.title.clone(),
                        };
                        state.send_event(event);
                    }
                }
            }
            Event::AppIdChanged { app_id } => {
                trace!("Plasma window {} app_id: {}", handle_id, app_id);
                if let Some(window) = state.windows.get_mut(&handle_id) {
                    window.app_id = app_id;
                    if window.ready {
                        let event = WindowEvent::Changed {
                            id: window.id,
                            app_id: window.app_id.clone(),
                            title: window.title.clone(),
                        };
                        state.send_event(event);
                    }
                }
            }
            Event::InitialState => {
                // All initial properties sent
                if let Some(window) = state.windows.get_mut(&handle_id) {
                    window.ready = true;
                    let event = WindowEvent::Opened {
                        id: window.id,
                        app_id: window.app_id.clone(),
                        title: window.title.clone(),
                    };
                    state.send_event(event);
                }
            }
            Event::Unmapped => {
                debug!("Plasma window {} unmapped (closed)", handle_id);
                if let Some(window) = state.windows.remove(&handle_id) {
                    state.send_event(WindowEvent::Closed { id: window.id });
                }
            }
            _ => {}
        }
    }
}

// Stub implementations for registry, output, and surface (required for event queue)
impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for PlasmaState {
    fn event(
        _state: &mut Self,
        _proxy: &wl_registry::WlRegistry,
        _event: wl_registry::Event,
        _data: &GlobalListContents,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        // No-op
    }
}

impl Dispatch<wl_output::WlOutput, ()> for PlasmaState {
    fn event(
        _state: &mut Self,
        _proxy: &wl_output::WlOutput,
        _event: wl_output::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        // No-op
    }
}

/// Run the Wayland event loop for KDE Plasma window management
///
/// This function runs in a dedicated thread and dispatches Wayland events.
pub fn run_event_loop(
    conn: Connection,
    tx: mpsc::UnboundedSender<WindowEvent>,
) -> Result<()> {
    let (globals, mut event_queue) = registry_queue_init::<PlasmaState>(&conn)
        .context("Failed to initialize Wayland registry")?;
    
    let qh = event_queue.handle();

    // Bind to the plasma window management protocol
    let _manager: org_kde_plasma_window_management::OrgKdePlasmaWindowManagement = globals
        .bind(&qh, 1..=16, ())
        .context("org_kde_plasma_window_management protocol not available")?;

    debug!("Successfully bound to KDE Plasma window management protocol");

    let mut state = PlasmaState::new(tx);

    // Main event loop - dispatches Wayland events
    // This will return an error if the compositor disconnects or the connection breaks
    loop {
        match event_queue.blocking_dispatch(&mut state) {
            Ok(_) => continue,
            Err(e) => {
                // Connection closed or compositor shut down
                debug!("Wayland event dispatch ended: {}", e);
                return Err(e).context("Wayland event dispatch failed");
            }
        }
    }
}
