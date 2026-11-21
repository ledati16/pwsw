//! Compositor abstraction layer
//!
//! Provides window event streams from Wayland compositors using standard protocols:
//! - `ext-foreign-toplevel-list-v1` (Standard, preferred)
//! - `wlr-foreign-toplevel-management` (Legacy/wlroots-specific: Sway, Hyprland, etc.)

mod ext_toplevel;
mod tracker;
mod wlr_toplevel;

use std::sync::Arc;
use std::sync::mpsc::{RecvTimeoutError, SyncSender};
use std::time::Duration;

use color_eyre::eyre::{self, Context, Result};
use tokio::sync::mpsc;
use tracing::{debug, error, info};
use tracker::{CloseOnDrop, EventOutbox, WindowTracker};
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::wl_registry;
use wayland_client::{Connection, EventQueue, QueueHandle};

/// Supported window management protocols
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Protocol {
    /// ext-foreign-toplevel-list-v1 (Standard)
    Ext,
    /// wlr-foreign-toplevel-management (Legacy/wlroots)
    Wlr,
}

/// Window event from a compositor
#[derive(Debug, Clone)]
pub enum WindowEvent {
    /// A new window was opened
    Opened {
        id: u64,
        app_id: String,
        title: String,
    },
    /// An existing window's `app_id` or title changed
    Changed {
        id: u64,
        app_id: String,
        title: String,
    },
    /// A window was closed
    Closed { id: u64 },
}

/// Capacity of the channel between the forwarder thread and the daemon
///
/// Events beyond this wait (and are coalesced) in the outbox instead, so a small value
/// means a slow consumer sees fewer stale intermediate states.
const WINDOW_EVENT_CHANNEL_CAPACITY: usize = 16;

/// How long [`spawn_compositor_thread_on`] waits for protocol detection
///
/// Detection is a single registry roundtrip; a compositor that does not answer within
/// this time is treated as broken rather than hanging daemon startup forever.
const SETUP_TIMEOUT: Duration = Duration::from_secs(10);

/// Spawn a dedicated thread for Wayland event processing
///
/// Connects to the Wayland display named by the environment, then behaves like
/// [`spawn_compositor_thread_on`].
///
/// # Errors
///
/// Returns an error if:
/// - No Wayland display connection can be established
/// - No supported window management protocol is available (see
///   [`spawn_compositor_thread_on`])
pub fn spawn_compositor_thread() -> Result<mpsc::Receiver<WindowEvent>> {
    let conn = Connection::connect_to_env()
        .context("Failed to connect to Wayland display. Is a Wayland compositor running?")?;

    info!("Connected to Wayland display");

    spawn_compositor_thread_on(conn)
}

/// Like [`spawn_compositor_thread`], but uses an already-established Wayland connection.
///
/// Lets callers (e.g. tests with a mock compositor) choose the socket explicitly instead
/// of going through `WAYLAND_DISPLAY` / `XDG_RUNTIME_DIR`.
///
/// Protocol detection and binding happen on the Wayland thread; this function waits for
/// that (one registry roundtrip) so detection failures are still returned synchronously.
///
/// # Supported Protocols
///
/// 1. `ext-foreign-toplevel-list-v1` (Standard, preferred when both are advertised)
/// 2. `wlr-foreign-toplevel-management` (Legacy/wlroots)
///
/// GNOME/Mutter and KDE Plasma 6 expose neither and are not supported.
///
/// # Event Semantics
///
/// - Window IDs are assigned by `pwsw` from a counter and are never reused, even when
///   the compositor recycles protocol object IDs.
/// - `Opened` is sent on a window's first `done`; `Changed` only when its `app_id` or
///   title differ from what was last sent (focus/maximise-only updates are dropped).
///   A window closed before its first `done` produces no events at all.
/// - The Wayland thread never blocks on the daemon. Events go to an in-memory outbox
///   and a forwarder thread moves them into the bounded channel. While the daemon lags,
///   the outbox coalesces per window: a newer `Changed` replaces an undelivered one, and
///   `Closed` cancels an undelivered `Opened` (both are dropped) or an undelivered
///   `Changed`. No event is ever lost that would leave the daemon with a stale view:
///   the daemon always ends up with each window's latest state, never sees `Changed`
///   after `Closed`, and every `Opened` it sees is followed by that window's `Closed`.
///
/// # Channel Closure
///
/// The receiver yields `None` once the stream is over, after any queued events:
/// - the compositor connection is lost (compositor crash/restart, protocol error);
/// - the compositor sends `finished` (it will not report windows any more);
/// - the Wayland thread panics.
///
/// Each case is logged as an error on the Wayland thread. Callers should treat closure
/// as fatal. Dropping the receiver stops both threads (the Wayland thread notices on its
/// next compositor event).
///
/// # Errors
/// Returns an error if no supported window management protocol is advertised, binding
/// it fails, the compositor does not answer the registry roundtrip within 10 seconds, or
/// a thread cannot be spawned.
pub fn spawn_compositor_thread_on(conn: Connection) -> Result<mpsc::Receiver<WindowEvent>> {
    let (tx, rx) = mpsc::channel(WINDOW_EVENT_CHANNEL_CAPACITY);
    let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
    let outbox = Arc::new(EventOutbox::new());

    let thread_outbox = Arc::clone(&outbox);
    let tx_probe = tx.clone();
    std::thread::Builder::new()
        .name("pwsw-wayland".into())
        .spawn(move || wayland_thread(&conn, thread_outbox, &tx_probe, &ready_tx))
        .context("Failed to spawn Wayland thread")?;

    let protocol = match ready_rx.recv_timeout(SETUP_TIMEOUT) {
        Ok(result) => result?,
        Err(RecvTimeoutError::Timeout) => eyre::bail!(
            "Compositor did not answer the Wayland registry roundtrip within {}s",
            SETUP_TIMEOUT.as_secs()
        ),
        Err(RecvTimeoutError::Disconnected) => {
            eyre::bail!("Wayland thread exited during protocol detection")
        }
    };
    info!("Using {protocol:?} protocol");

    std::thread::Builder::new()
        .name("pwsw-wayland-fwd".into())
        .spawn(move || tracker::forward_events(&outbox, &tx))
        .context("Failed to spawn window event forwarder thread")?;

    Ok(rx)
}

/// Body of the Wayland thread: run the event loop and log how it ended
fn wayland_thread(
    conn: &Connection,
    outbox: Arc<EventOutbox>,
    tx_probe: &mpsc::Sender<WindowEvent>,
    ready: &SyncSender<Result<Protocol>>,
) {
    // Dropped last (also on panic): lets the forwarder drain and close the channel
    let _close = CloseOnDrop(Arc::clone(&outbox));

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        run_wayland(conn, outbox, tx_probe, ready)
    }));

    match result {
        Ok(Ok(())) => info!("Wayland thread stopped"),
        Ok(Err(e)) => error!("Wayland event loop ended: {e:#}"),
        Err(panic_info) => {
            let panic_msg = if let Some(&s) = panic_info.downcast_ref::<&str>() {
                s
            } else if let Some(s) = panic_info.downcast_ref::<String>() {
                s.as_str()
            } else {
                "Unknown panic"
            };
            error!("Wayland event loop panicked: {panic_msg}");
        }
    }
}

/// Detect and bind a protocol, report the outcome on `ready`, then dispatch until the end
///
/// Returns `Ok` when the receiver was dropped (or nobody waits for setup any more), and
/// an error when the event stream ended on the compositor's side. Setup errors are
/// returned to [`spawn_compositor_thread_on`] through `ready` instead.
fn run_wayland(
    conn: &Connection,
    outbox: Arc<EventOutbox>,
    tx_probe: &mpsc::Sender<WindowEvent>,
    ready: &SyncSender<Result<Protocol>>,
) -> Result<()> {
    let mut queue = match setup(conn) {
        Ok((queue, protocol)) => {
            if ready.send(Ok(protocol)).is_err() {
                debug!("Nobody waiting for Wayland setup, stopping");
                return Ok(());
            }
            queue
        }
        Err(e) => {
            // The caller reports the error; nothing more to do here
            let _ = ready.send(Err(e));
            return Ok(());
        }
    };

    let mut tracker = WindowTracker::new(outbox);
    loop {
        // Flushes our requests (handle destroys), waits for and dispatches events.
        // Returns an error when the connection is gone, so this never spins.
        queue
            .blocking_dispatch(&mut tracker)
            .context("Lost the Wayland connection (compositor exited or disconnected us)")?;

        if tracker.is_finished() {
            // Best effort: send the list `destroy` before the connection is dropped
            let _ = conn.flush();
            eyre::bail!("Compositor stopped reporting windows (`finished` event)");
        }
        if tx_probe.is_closed() {
            debug!("Window event receiver dropped, stopping Wayland thread");
            return Ok(());
        }
    }
}

/// Enumerate globals (one roundtrip), pick a protocol and bind it
fn setup(conn: &Connection) -> Result<(EventQueue<WindowTracker>, Protocol)> {
    let (globals, queue) = registry_queue_init::<WindowTracker>(conn)
        .context("Failed to initialize Wayland registry")?;
    let qh: QueueHandle<WindowTracker> = queue.handle();

    let protocol = globals.contents().with_list(|list| {
        for global in list {
            debug!(
                "Found global: {} (version {})",
                global.interface, global.version
            );
        }
        choose_protocol(list.iter().map(|g| g.interface.as_str()))
    });

    let Some(protocol) = protocol else {
        eyre::bail!(
            "No supported window management protocol found.\n\
             \n\
             Supported protocols:\n\
             - `ext_foreign_toplevel_list_v1` (Standard)\n\
             - `zwlr_foreign_toplevel_manager_v1` (Legacy: Sway, Hyprland, etc.)\n\
             \n\
             Unsupported compositors:\n\
             - GNOME/Mutter (no window management protocol exposed)\n\
             - KDE Plasma (removed protocol support in Plasma 6)"
        );
    };

    // Dropping the bound proxy does not destroy the protocol object; events keep flowing
    match protocol {
        Protocol::Ext => {
            ext_toplevel::bind(&globals, &qh)?;
        }
        Protocol::Wlr => {
            wlr_toplevel::bind(&globals, &qh)?;
        }
    }
    debug!("Bound {protocol:?} toplevel protocol");

    Ok((queue, protocol))
}

/// Pick the preferred protocol among the advertised global interface names
fn choose_protocol<'a>(interfaces: impl IntoIterator<Item = &'a str>) -> Option<Protocol> {
    let mut best = None;
    for interface in interfaces {
        if interface == ext_toplevel::INTERFACE {
            return Some(Protocol::Ext);
        }
        if interface == wlr_toplevel::INTERFACE {
            best = Some(Protocol::Wlr);
        }
    }
    best
}

impl wayland_client::Dispatch<wl_registry::WlRegistry, GlobalListContents> for WindowTracker {
    fn event(
        _state: &mut Self,
        _proxy: &wl_registry::WlRegistry,
        _event: wl_registry::Event,
        _data: &GlobalListContents,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        // Global add/remove after startup is irrelevant: the bound protocol global is
        // never removed by compositors while they run.
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_choose_protocol_prefers_ext() {
        let wlr = wlr_toplevel::INTERFACE;
        let ext = ext_toplevel::INTERFACE;
        assert_eq!(
            choose_protocol(["wl_output", wlr, ext]),
            Some(Protocol::Ext)
        );
        assert_eq!(choose_protocol([ext, wlr]), Some(Protocol::Ext));
        assert_eq!(choose_protocol([wlr]), Some(Protocol::Wlr));
        assert_eq!(choose_protocol(["wl_seat"]), None);
    }
}
