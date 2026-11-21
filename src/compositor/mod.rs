//! Compositor abstraction layer
//!
//! Provides window event streams from Wayland compositors using standard protocols:
//! - `ext-foreign-toplevel-list-v1` (Standard, preferred)
//! - `wlr-foreign-toplevel-management` (Legacy/wlroots-specific: Sway, Hyprland, etc.)
//!
//! Only `wlr` reports which monitors a window is on ([`WindowEvent::Placement`]); the
//! daemon asks for it with [`ProtocolPreference::PreferWlr`].

mod ext_toplevel;
mod output;
mod tracker;
mod wlr_toplevel;

use std::sync::Arc;
use std::sync::mpsc::{RecvTimeoutError, SyncSender};
use std::time::Duration;

use anyhow::{Context, Result};
use tokio::sync::mpsc;
use tracing::{debug, error, info};
use tracker::{EventOutbox, WindowTracker};
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::wl_registry;
use wayland_client::{Connection, EventQueue, QueueHandle};

/// Supported window management protocols (the toplevel backend in use)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    /// `ext-foreign-toplevel-list-v1` (Standard; no placement information)
    Ext,
    /// `wlr-foreign-toplevel-management` (Legacy/wlroots; reports placement)
    Wlr,
}

impl Backend {
    /// The Wayland interface name of this protocol's global
    #[must_use]
    pub const fn interface(self) -> &'static str {
        match self {
            Self::Ext => ext_toplevel::INTERFACE,
            Self::Wlr => wlr_toplevel::INTERFACE,
        }
    }

    /// Short name for status output and logs: `"ext"` or `"wlr"`
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Ext => "ext",
            Self::Wlr => "wlr",
        }
    }

    /// Whether windows tracked with this backend get [`WindowEvent::Placement`] events
    #[must_use]
    pub const fn reports_placement(self) -> bool {
        matches!(self, Self::Wlr)
    }
}

/// Which toplevel protocol to bind when the compositor advertises both
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ProtocolPreference {
    /// `ext` if advertised, else `wlr` (the default)
    #[default]
    Auto,
    /// `wlr` if advertised, else `ext`: for callers that need
    /// [`WindowEvent::Placement`] (monitor names, minimized state)
    PreferWlr,
}

/// What the compositor connection provides, as chosen at spawn time
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Capabilities {
    backend: Backend,
}

impl Capabilities {
    /// The toplevel backend in use
    #[must_use]
    pub const fn backend(self) -> Backend {
        self.backend
    }

    /// Whether [`WindowEvent::Placement`] events are reported (monitor info available)
    #[must_use]
    pub const fn monitor_info(self) -> bool {
        self.backend.reports_placement()
    }

    /// One line for `doctor` / `list-windows`, e.g. `monitor info: available (wlr)`
    #[must_use]
    pub const fn monitor_info_summary(self) -> &'static str {
        match self.backend {
            Backend::Wlr => "monitor info: available (wlr)",
            Backend::Ext => "monitor info: unavailable (ext)",
        }
    }
}

/// What a connection would provide, given the advertised global interface names and a
/// preference (the same choice the spawn functions make); `None` if no supported
/// toplevel protocol is advertised
///
/// With a `probe_protocols` result, pass the `advertised` interfaces: under
/// [`ProtocolPreference::PreferWlr`] this says whether monitor info can be had at all.
#[must_use]
pub fn detected_capabilities<'a>(
    interfaces: impl IntoIterator<Item = &'a str>,
    preference: ProtocolPreference,
) -> Option<Capabilities> {
    choose_protocol(interfaces, preference).map(|backend| Capabilities { backend })
}

/// Window event from a compositor
#[derive(Debug, Clone)]
pub enum WindowEvent {
    /// A new window was opened, with its initial placement
    ///
    /// `outputs` and `minimized` are as in [`WindowEvent::Placement`] (empty and `false`
    /// with the `ext` backend, or for a window that is on no output yet). Carrying them
    /// here means a window that opens on a monitor arrives as one event, so the daemon
    /// never acts on the window before it knows where it is.
    Opened {
        id: u64,
        app_id: String,
        title: String,
        /// Connector names of the monitors the window is on when it opens (sorted,
        /// deduplicated; see [`WindowEvent::Placement`])
        outputs: Vec<String>,
        /// The compositor's `minimized` flag when the window opens
        minimized: bool,
    },
    /// An existing window's `app_id` or title changed
    Changed {
        id: u64,
        app_id: String,
        title: String,
    },
    /// A window's placement changed: the monitors it is on, and whether it is minimized
    ///
    /// Only the `wlr` backend reports placement ([`Backend::reports_placement`]). The
    /// initial placement is part of the window's `Opened`; this is sent whenever it
    /// changes after that (on the compositor's `done`, or right away when an output the
    /// window is on is removed).
    Placement {
        id: u64,
        /// Connector names (`HDMI-A-1`, `DP-2`), sorted and deduplicated. An output
        /// without a name (`wl_output` older than version 4) appears as
        /// `output-<registry global name>`.
        outputs: Vec<String>,
        /// The compositor's `minimized` state flag
        minimized: bool,
    },
    /// A window was closed
    Closed { id: u64 },
    /// Every window that existed when the connection was set up has been reported
    ///
    /// Sent exactly once, early: after the toplevel protocol is bound, the Wayland thread
    /// makes [`INITIAL_SYNC_ROUNDTRIPS`] roundtrips (the compositor announces existing
    /// windows, with their first `done`, in reply to the bind), queues the windows it
    /// learnt of, then this marker. Lets the daemon decide its startup baseline from the
    /// windows that are already open instead of switching once per window as they
    /// trickle in. A window whose first `done` comes later is reported after it.
    InitialSync,
}

/// Roundtrips after binding the toplevel protocol before [`WindowEvent::InitialSync`]
///
/// One is enough for compositors that announce existing toplevels (and their `done`)
/// right on bind, as `wlroots` does; the second covers ones that send the `done` in a
/// later batch.
pub const INITIAL_SYNC_ROUNDTRIPS: usize = 2;

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
    spawn_compositor_thread_with_preference(ProtocolPreference::Auto).map(|(rx, _)| rx)
}

/// Like [`spawn_compositor_thread`], choosing the protocol by `preference` and also
/// returning what the chosen backend provides
///
/// # Errors
///
/// As [`spawn_compositor_thread`].
pub fn spawn_compositor_thread_with_preference(
    preference: ProtocolPreference,
) -> Result<(mpsc::Receiver<WindowEvent>, Capabilities)> {
    let conn = Connection::connect_to_env()
        .context("Failed to connect to Wayland display. Is a Wayland compositor running?")?;

    info!("Connected to Wayland display");

    spawn_compositor_thread_on_with_preference(conn, preference)
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
/// [`spawn_compositor_thread_on_with_preference`] can prefer `wlr` instead.
///
/// GNOME/Mutter and KDE Plasma 6 expose neither and are not supported.
///
/// # Event Semantics
///
/// - Window IDs are assigned by `pwsw` from a counter and are never reused, even when
///   the compositor recycles protocol object IDs.
/// - `Opened` is sent on a window's first `done`, with its initial placement;
///   `Changed` only when its `app_id` or title differ from what was last sent
///   (focus/maximise-only updates are dropped). A window closed before its first
///   `done` produces no events at all.
/// - `Placement` (`wlr` only) comes when the window's sorted output names or minimized
///   flag change from what was last sent (in `Opened` or a previous `Placement`).
/// - [`WindowEvent::InitialSync`] is sent once, after the windows that existed when the
///   connection was set up.
///   With `wlr`, `wl_output` globals are bound (and rebound on hotplug); when one is
///   removed, windows on it get a `Placement` without it.
/// - The Wayland thread never blocks on the daemon. Events go to an in-memory outbox
///   and a forwarder thread moves them into the bounded channel. While the daemon lags,
///   the outbox coalesces per window: a newer `Changed` replaces an undelivered one (in
///   its queue position), a newer `Placement` likewise replaces an undelivered
///   `Placement`, and `Closed` cancels an undelivered `Opened` (both are dropped) or an
///   undelivered `Changed`/`Placement`; a `Placement` for a window whose `Opened` is
///   undelivered is folded into it. Surviving events keep the order in which they
///   were first queued. No event is ever lost that would leave the daemon with a stale
///   view: the daemon always ends up with each window's latest state, never sees
///   `Changed` or `Placement` before `Opened` or after `Closed`, and every `Opened` it
///   sees is followed by that window's `Closed`. `Changed` and `Placement` coalesce
///   independently, so their relative order carries no meaning.
///
/// # Channel Closure
///
/// The receiver yields `None` once the stream is over, after any queued events:
/// - the compositor connection is lost (compositor crash/restart, protocol error);
/// - the compositor sends `finished` (it will not report windows any more).
///
/// Each case is logged as an error on the Wayland thread. A panic on either thread
/// aborts the process in release builds (`panic = "abort"`), like a panic anywhere else
/// in the daemon; the supervisor restarts it. Callers should treat closure
/// as fatal. Dropping the receiver stops both threads (the Wayland thread notices on its
/// next compositor event).
///
/// # Errors
/// Returns an error if no supported window management protocol is advertised, binding
/// it fails, the compositor does not answer the registry roundtrip within 10 seconds, or
/// a thread cannot be spawned.
pub fn spawn_compositor_thread_on(conn: Connection) -> Result<mpsc::Receiver<WindowEvent>> {
    spawn_compositor_thread_on_with_preference(conn, ProtocolPreference::Auto).map(|(rx, _)| rx)
}

/// Like [`spawn_compositor_thread_on`], choosing the protocol by `preference` and also
/// returning what the chosen backend provides
///
/// # Errors
///
/// As [`spawn_compositor_thread_on`].
pub fn spawn_compositor_thread_on_with_preference(
    conn: Connection,
    preference: ProtocolPreference,
) -> Result<(mpsc::Receiver<WindowEvent>, Capabilities)> {
    let (tx, rx) = mpsc::channel(WINDOW_EVENT_CHANNEL_CAPACITY);
    let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
    let outbox = Arc::new(EventOutbox::new());

    let thread_outbox = Arc::clone(&outbox);
    let tx_probe = tx.clone();
    std::thread::Builder::new()
        .name("pwsw-wayland".into())
        .spawn(move || wayland_thread(&conn, preference, &thread_outbox, &tx_probe, &ready_tx))
        .context("Failed to spawn Wayland thread")?;

    let protocol = match ready_rx.recv_timeout(SETUP_TIMEOUT) {
        Ok(result) => result?,
        Err(RecvTimeoutError::Timeout) => anyhow::bail!(
            "Compositor did not answer the Wayland registry roundtrip within {}s",
            SETUP_TIMEOUT.as_secs()
        ),
        Err(RecvTimeoutError::Disconnected) => {
            anyhow::bail!("Wayland thread exited during protocol detection")
        }
    };
    let why = match (preference, protocol) {
        (ProtocolPreference::Auto, Backend::Ext) => "the standard one",
        (ProtocolPreference::Auto, Backend::Wlr) => "ext is not offered",
        (ProtocolPreference::PreferWlr, Backend::Wlr) => "the config has output rules",
        (ProtocolPreference::PreferWlr, Backend::Ext) => {
            "the config has output rules, but wlr is not offered"
        }
    };
    info!("Using the {} protocol ({why})", protocol.name());

    std::thread::Builder::new()
        .name("pwsw-wayland-fwd".into())
        .spawn(move || tracker::forward_events(&outbox, &tx))
        .context("Failed to spawn window event forwarder thread")?;

    Ok((rx, Capabilities { backend: protocol }))
}

/// Body of the Wayland thread: run the event loop, log how it ended, then close the
/// outbox so the forwarder delivers what is left and closes the channel
///
/// There is no unwinding to handle: release builds abort on panic.
fn wayland_thread(
    conn: &Connection,
    preference: ProtocolPreference,
    outbox: &Arc<EventOutbox>,
    tx_probe: &mpsc::Sender<WindowEvent>,
    ready: &SyncSender<Result<Backend>>,
) {
    match run_wayland(conn, preference, outbox, tx_probe, ready) {
        Ok(()) => info!("Wayland thread stopped"),
        Err(e) => error!(
            "Wayland event loop ended: {}",
            crate::sanitize::error_line(&e)
        ),
    }
    outbox.close();
}

/// Detect and bind a protocol, report the outcome on `ready`, then dispatch until the end
///
/// Returns `Ok` when the receiver was dropped (or nobody waits for setup any more), and
/// an error when the event stream ended on the compositor's side. Setup errors are
/// returned to [`spawn_compositor_thread_on`] through `ready` instead.
fn run_wayland(
    conn: &Connection,
    preference: ProtocolPreference,
    outbox: &Arc<EventOutbox>,
    tx_probe: &mpsc::Sender<WindowEvent>,
    ready: &SyncSender<Result<Backend>>,
) -> Result<()> {
    let (mut queue, mut tracker) = match setup(conn, preference, Arc::clone(outbox)) {
        Ok((queue, tracker, protocol)) => {
            if ready.send(Ok(protocol)).is_err() {
                debug!("Nobody waiting for Wayland setup, stopping");
                return Ok(());
            }
            (queue, tracker)
        }
        Err(e) => {
            // The caller reports the error; nothing more to do here
            let _ = ready.send(Err(e));
            return Ok(());
        }
    };

    // Existing windows: the compositor announces them in reply to the bind
    for _ in 0..INITIAL_SYNC_ROUNDTRIPS {
        queue
            .roundtrip(&mut tracker)
            .context("Lost the Wayland connection while listing existing windows")?;
        if tracker.is_finished() {
            let _ = conn.flush();
            anyhow::bail!("Compositor stopped reporting windows (`finished` event)");
        }
    }
    debug!("Initial window list complete");
    outbox.push(WindowEvent::InitialSync);

    loop {
        // Flushes our requests (handle destroys), waits for and dispatches events.
        // Returns an error when the connection is gone, so this never spins.
        queue
            .blocking_dispatch(&mut tracker)
            .context("Lost the Wayland connection (compositor exited or disconnected us)")?;

        if tracker.is_finished() {
            // Best effort: send the list `destroy` before the connection is dropped
            let _ = conn.flush();
            anyhow::bail!("Compositor stopped reporting windows (`finished` event)");
        }
        if tx_probe.is_closed() {
            debug!("Window event receiver dropped, stopping Wayland thread");
            return Ok(());
        }
    }
}

/// Enumerate globals (one roundtrip), pick a protocol and bind it
///
/// For `wlr`, the `wl_output` globals are bound first, so the compositor knows them
/// before it announces toplevels (whose `output_enter` events refer to them).
fn setup(
    conn: &Connection,
    preference: ProtocolPreference,
    outbox: Arc<EventOutbox>,
) -> Result<(EventQueue<WindowTracker>, WindowTracker, Backend)> {
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
        choose_protocol(list.iter().map(|g| g.interface.as_str()), preference)
    });

    let Some(protocol) = protocol else {
        anyhow::bail!(
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

    let mut tracker = WindowTracker::new(outbox, protocol.reports_placement());
    if tracker.tracks_outputs() {
        let outputs: Vec<(u32, u32)> = globals.contents().with_list(|list| {
            list.iter()
                .filter(|g| g.interface == output::INTERFACE)
                .map(|g| (g.name, g.version))
                .collect()
        });
        for (global, version) in outputs {
            let proxy = output::bind(globals.registry(), global, version, &qh);
            tracker.output_added(proxy, global);
        }
    }

    // Dropping the bound proxy does not destroy the protocol object; events keep flowing
    match protocol {
        Backend::Ext => {
            ext_toplevel::bind(&globals, &qh)?;
        }
        Backend::Wlr => {
            wlr_toplevel::bind(&globals, &qh)?;
        }
    }
    debug!("Bound {protocol:?} toplevel protocol");

    Ok((queue, tracker, protocol))
}

/// A toplevel protocol global the compositor advertises (see [`probe_protocols`])
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AdvertisedGlobal {
    pub(crate) interface: String,
    pub(crate) version: u32,
}

/// What a compositor offers `pwsw`: the supported toplevel protocols it advertises
/// (which one the daemon binds depends on its config: [`ProtocolProbe::capabilities`])
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProtocolProbe {
    pub(crate) advertised: Vec<AdvertisedGlobal>,
}

impl ProtocolProbe {
    /// What a daemon asking with `preference` would get (`None`: nothing supported)
    pub(crate) fn capabilities(&self, preference: ProtocolPreference) -> Option<Capabilities> {
        detected_capabilities(
            self.advertised.iter().map(|g| g.interface.as_str()),
            preference,
        )
    }
}

/// Registry state of [`probe_protocols`] (it binds nothing)
struct ProbeState;

impl wayland_client::Dispatch<wl_registry::WlRegistry, GlobalListContents> for ProbeState {
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

/// Connect to the Wayland display named by the environment and list the supported
/// toplevel protocols it advertises. Binds nothing; read-only. Blocking (one registry roundtrip, at most `timeout`).
///
/// # Errors
/// Returns an error if no Wayland connection can be made, the registry roundtrip
/// fails, or the compositor does not answer within `timeout`.
pub(crate) fn probe_protocols(timeout: Duration) -> Result<ProtocolProbe> {
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    // On a thread, so a compositor that never answers cannot hang the caller
    std::thread::Builder::new()
        .name("pwsw-wayland-probe".into())
        .spawn(move || {
            let _ = tx.send(probe_on_env());
        })
        .context("Failed to spawn Wayland probe thread")?;
    match rx.recv_timeout(timeout) {
        Ok(result) => result,
        Err(RecvTimeoutError::Timeout) => anyhow::bail!(
            "Compositor did not answer the Wayland registry roundtrip within {}s",
            timeout.as_secs()
        ),
        Err(RecvTimeoutError::Disconnected) => anyhow::bail!("Wayland probe thread exited"),
    }
}

/// Body of [`probe_protocols`]
fn probe_on_env() -> Result<ProtocolProbe> {
    let conn = Connection::connect_to_env().context("Failed to connect to Wayland display")?;
    let (globals, _queue) = registry_queue_init::<ProbeState>(&conn)
        .context("Failed to initialize Wayland registry")?;
    Ok(globals.contents().with_list(|list| {
        let supported = [ext_toplevel::INTERFACE, wlr_toplevel::INTERFACE];
        ProtocolProbe {
            advertised: list
                .iter()
                .filter(|g| supported.contains(&g.interface.as_str()))
                .map(|g| AdvertisedGlobal {
                    interface: g.interface.clone(),
                    version: g.version,
                })
                .collect(),
        }
    }))
}

/// Pick a protocol among the advertised global interface names
fn choose_protocol<'a>(
    interfaces: impl IntoIterator<Item = &'a str>,
    preference: ProtocolPreference,
) -> Option<Backend> {
    let (mut ext, mut wlr) = (false, false);
    for interface in interfaces {
        ext |= interface == ext_toplevel::INTERFACE;
        wlr |= interface == wlr_toplevel::INTERFACE;
    }
    match (preference, ext, wlr) {
        (ProtocolPreference::Auto, true, _) | (ProtocolPreference::PreferWlr, true, false) => {
            Some(Backend::Ext)
        }
        (_, _, true) => Some(Backend::Wlr),
        (_, false, false) => None,
    }
}

impl wayland_client::Dispatch<wl_registry::WlRegistry, GlobalListContents> for WindowTracker {
    fn event(
        state: &mut Self,
        proxy: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _data: &GlobalListContents,
        _conn: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        // Only outputs come and go while compositors run (monitor hotplug); the bound
        // toplevel protocol global is never removed.
        match event {
            wl_registry::Event::Global {
                name,
                interface,
                version,
            } if interface == output::INTERFACE && state.tracks_outputs() => {
                let output = output::bind(proxy, name, version, qh);
                state.output_added(output, name);
            }
            wl_registry::Event::GlobalRemove { name } => state.output_global_removed(name),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_choose_protocol_auto_prefers_ext() {
        let auto = ProtocolPreference::Auto;
        let wlr = wlr_toplevel::INTERFACE;
        let ext = ext_toplevel::INTERFACE;
        assert_eq!(
            choose_protocol(["wl_output", wlr, ext], auto),
            Some(Backend::Ext)
        );
        assert_eq!(choose_protocol([ext, wlr], auto), Some(Backend::Ext));
        assert_eq!(choose_protocol([wlr], auto), Some(Backend::Wlr));
        assert_eq!(choose_protocol(["wl_seat"], auto), None);
        assert_eq!(ProtocolPreference::default(), auto);
    }

    #[test]
    fn test_choose_protocol_prefer_wlr() {
        let prefer = ProtocolPreference::PreferWlr;
        let wlr = wlr_toplevel::INTERFACE;
        let ext = ext_toplevel::INTERFACE;
        assert_eq!(
            choose_protocol(["wl_output", ext, wlr], prefer),
            Some(Backend::Wlr)
        );
        assert_eq!(choose_protocol([ext], prefer), Some(Backend::Ext));
        assert_eq!(choose_protocol([wlr], prefer), Some(Backend::Wlr));
        assert_eq!(choose_protocol(["wl_seat"], prefer), None);
    }

    #[test]
    fn test_detected_capabilities() {
        let both = [ext_toplevel::INTERFACE, wlr_toplevel::INTERFACE];
        let auto = detected_capabilities(both, ProtocolPreference::Auto).expect("ext");
        assert_eq!(auto.backend(), Backend::Ext);
        assert!(!auto.monitor_info());
        assert_eq!(
            auto.monitor_info_summary(),
            "monitor info: unavailable (ext)"
        );

        let wlr = detected_capabilities(both, ProtocolPreference::PreferWlr).expect("wlr");
        assert_eq!(wlr.backend(), Backend::Wlr);
        assert!(wlr.monitor_info());
        assert_eq!(wlr.monitor_info_summary(), "monitor info: available (wlr)");

        let ext_only = [ext_toplevel::INTERFACE];
        assert!(
            !detected_capabilities(ext_only, ProtocolPreference::PreferWlr)
                .expect("ext")
                .monitor_info()
        );
        assert_eq!(
            detected_capabilities(["wl_seat"], ProtocolPreference::PreferWlr),
            None
        );
    }
}
