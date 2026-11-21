//! Protocol-independent window tracking shared by both toplevel backends
//!
//! [`WindowTracker`] is the `wayland-client` dispatch state for the Wayland thread. The
//! backends (`ext_toplevel`, `wlr_toplevel`) only translate protocol events into calls on
//! it; ID assignment, change detection and event delivery live here.
//!
//! Window placement (the monitors a window is on, and whether it is minimized) is tracked
//! only for the `wlr` backend; for `ext` windows it stays empty and no
//! [`WindowEvent::Placement`] is ever emitted.
//!
//! Delivery goes through an [`EventOutbox`]: the Wayland thread pushes into it without
//! ever blocking, and a separate forwarder thread moves events from it into the bounded
//! `tokio` channel the daemon reads. While the daemon is slow, superseded events are
//! coalesced in the outbox (see [`coalesce_push`]), so memory stays bounded by the number
//! of windows rather than by the number of protocol events.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};

use tokio::sync::mpsc;
use tracing::{debug, trace};
use wayland_client::Proxy;
use wayland_client::backend::ObjectId;
use wayland_client::protocol::wl_output::WlOutput;

use super::{WindowEvent, output};
use crate::sanitize::for_log;

/// Queue of window events waiting to be handed to the daemon
///
/// Shared between the Wayland thread (producer, never blocks for longer than a short
/// mutex hold) and the forwarder thread (consumer, blocks on the `tokio` channel).
pub(super) struct EventOutbox {
    inner: Mutex<OutboxInner>,
    ready: Condvar,
}

#[derive(Default)]
struct OutboxInner {
    queue: VecDeque<WindowEvent>,
    /// Set once the producer is gone; the consumer drains what is left and stops.
    closed: bool,
}

impl EventOutbox {
    pub(super) fn new() -> Self {
        Self {
            inner: Mutex::new(OutboxInner::default()),
            ready: Condvar::new(),
        }
    }

    fn lock(&self) -> MutexGuard<'_, OutboxInner> {
        // The queue holds plain data and is consistent after every operation, so a
        // poisoned lock (a panic elsewhere while holding it) is safe to keep using.
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Queue an event, coalescing it with any not-yet-forwarded events for the same window
    pub(super) fn push(&self, event: WindowEvent) {
        let mut inner = self.lock();
        if inner.closed {
            return;
        }
        coalesce_push(&mut inner.queue, event);
        drop(inner);
        self.ready.notify_one();
    }

    /// Mark the producer as finished; the forwarder delivers what is queued, then stops
    pub(super) fn close(&self) {
        self.lock().closed = true;
        self.ready.notify_all();
    }

    /// Wait for the next event; `None` once the outbox is closed and empty
    fn next_blocking(&self) -> Option<WindowEvent> {
        let mut inner = self.lock();
        loop {
            if let Some(event) = inner.queue.pop_front() {
                return Some(event);
            }
            if inner.closed {
                return None;
            }
            inner = self
                .ready
                .wait(inner)
                .unwrap_or_else(PoisonError::into_inner);
        }
    }

    #[cfg(test)]
    fn snapshot(&self) -> Vec<WindowEvent> {
        self.lock().queue.iter().cloned().collect()
    }
}

/// Forwarder thread body: move events from the outbox into the daemon's channel
///
/// Blocking here (when the daemon is slow) is harmless: the Wayland thread keeps
/// dispatching and the outbox coalesces in the meantime. Returns when the outbox is
/// closed and drained, or when the receiver has been dropped; returning drops `tx`, which
/// closes the channel once the Wayland thread has exited too.
pub(super) fn forward_events(outbox: &EventOutbox, tx: &mpsc::Sender<WindowEvent>) {
    while let Some(event) = outbox.next_blocking() {
        if tx.blocking_send(event).is_err() {
            debug!("Window event receiver dropped, stopping forwarder");
            outbox.close();
            return;
        }
    }
}

const fn event_id(event: &WindowEvent) -> Option<u64> {
    match event {
        WindowEvent::Opened { id, .. }
        | WindowEvent::Changed { id, .. }
        | WindowEvent::Placement { id, .. }
        | WindowEvent::Closed { id } => Some(*id),
        WindowEvent::InitialSync => None,
    }
}

/// Append `event` to `queue`, dropping events it makes redundant
///
/// Rules (all per window id):
/// - `Opened` is always appended, and so is [`WindowEvent::InitialSync`] (it belongs to
///   no window and is never dropped or moved).
/// - `Changed` replaces a queued `Changed` for the same window **in place** (the newest
///   state wins, but keeps the queue slot of the oldest undelivered change); with no
///   queued `Changed` it is appended. At most one `Changed` per window is ever queued.
/// - `Placement` for a window whose `Opened` is still queued is folded into that
///   `Opened` (which carries the window's initial placement), so the daemon learns of
///   the window and where it is in one event. Otherwise it follows the same rule among
///   `Placement`s as `Changed` among `Changed`s: it replaces a queued `Placement` for
///   the same window in place, or is appended. `Changed` and `Placement` never replace
///   each other (they carry different attributes).
/// - `Closed` removes any queued `Changed` and `Placement`. If the window's `Opened` is
///   still queued, the daemon never learnt about the window, so the `Opened` is removed
///   and no `Closed` is queued either. Otherwise `Closed` is appended.
///
/// Ordering guarantee: events that survive coalescing are delivered in the order they
/// were first queued, so no surviving event ever moves behind an event that was queued
/// after it. In particular a window whose (since superseded) change was queued before
/// another window's `Opened` is still processed before that `Opened`, so the daemon - which
/// renews a window's priority when it starts matching a rule - never ranks it above a
/// window that opened later merely because the channel was backed up. The cost is that
/// the coalesced change carries the latest state at the earliest change's position.
///
/// A window's `Changed` and `Placement` coalesce independently: each delivered one
/// carries the newest value of its own attributes, but their relative order says nothing
/// about which compositor batch came first. Once both are delivered the daemon has the
/// window's latest `app_id`, title and placement.
///
/// Consequently the consumer never sees `Changed` or `Placement` before the window's
/// `Opened` or after its `Closed`, and every `Opened` it does see is eventually followed
/// by that window's `Closed`.
fn coalesce_push(queue: &mut VecDeque<WindowEvent>, event: WindowEvent) {
    let Some(id) = event_id(&event) else {
        queue.push_back(event);
        return;
    };
    match event {
        WindowEvent::Opened { .. } | WindowEvent::InitialSync => queue.push_back(event),
        WindowEvent::Changed { .. } => replace_or_append(
            queue,
            event,
            |e| matches!(e, WindowEvent::Changed { id: other, .. } if *other == id),
        ),
        WindowEvent::Placement {
            id,
            outputs,
            minimized,
        } => {
            if let Some(WindowEvent::Opened {
                outputs: opened_outputs,
                minimized: opened_minimized,
                ..
            }) = queue
                .iter_mut()
                .find(|e| matches!(e, WindowEvent::Opened { id: other, .. } if *other == id))
            {
                *opened_outputs = outputs;
                *opened_minimized = minimized;
            } else {
                replace_or_append(
                    queue,
                    WindowEvent::Placement {
                        id,
                        outputs,
                        minimized,
                    },
                    |e| matches!(e, WindowEvent::Placement { id: other, .. } if *other == id),
                );
            }
        }
        WindowEvent::Closed { .. } => {
            queue.retain(|e| {
                !matches!(
                    e,
                    WindowEvent::Changed { id: other, .. }
                    | WindowEvent::Placement { id: other, .. } if *other == id
                )
            });
            if let Some(pos) = queue
                .iter()
                .position(|e| matches!(e, WindowEvent::Opened { id: other, .. } if *other == id))
            {
                queue.remove(pos);
            } else {
                queue.push_back(event);
            }
        }
    }
}

/// Overwrite the first queued event matching `supersedes` with `event` (in place), or
/// append `event` when none matches
fn replace_or_append(
    queue: &mut VecDeque<WindowEvent>,
    event: WindowEvent,
    supersedes: impl Fn(&WindowEvent) -> bool,
) {
    if let Some(slot) = queue.iter_mut().find(|e| supersedes(e)) {
        *slot = event;
    } else {
        queue.push_back(event);
    }
}

/// Where a window is, as reported in [`WindowEvent::Placement`]: the labels of the
/// outputs it is on (sorted, deduplicated) and whether it is minimized
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Placement {
    outputs: Vec<String>,
    minimized: bool,
}

/// A toplevel handle the compositor told us about
#[derive(Debug)]
struct TrackedWindow {
    /// ID reported to the daemon (from [`WindowTracker::next_id`], never reused)
    id: u64,
    /// Latest `app_id` received (applied on `done`)
    app_id: String,
    /// Latest title received (applied on `done`)
    title: String,
    /// `(app_id, title)` last reported to the daemon; `None` until the first `done`
    reported: Option<(String, String)>,
    /// Outputs (`wl_output` object IDs) as received from `output_enter` / `output_leave`
    pending_outputs: HashSet<ObjectId>,
    /// Minimized flag as received from the latest `state`
    pending_minimized: bool,
    /// Outputs as of the last `done` (minus outputs removed since)
    outputs: HashSet<ObjectId>,
    /// Minimized flag as of the last `done`
    minimized: bool,
    /// Placement last reported to the daemon (in `Opened`, then by `Placement`)
    reported_placement: Placement,
}

/// A bound `wl_output`
#[derive(Debug)]
struct TrackedOutput {
    proxy: WlOutput,
    /// Registry global name (identifies the output in `global_remove`)
    global: u32,
    /// Connector name (`HDMI-A-1`), from `wl_output` version 4's `name` event
    name: Option<String>,
}

impl TrackedOutput {
    /// What windows on this output report: its connector name, or a stable fallback
    fn label(&self) -> String {
        self.name
            .clone()
            .unwrap_or_else(|| output::fallback_label(self.global))
    }
}

/// Dispatch state for the Wayland thread, shared by both protocol backends
pub(super) struct WindowTracker {
    outbox: Arc<EventOutbox>,
    /// Next window ID to hand out. Protocol object IDs are recycled by the compositor once
    /// a handle is destroyed, so they cannot identify windows over the daemon's lifetime.
    next_id: u64,
    /// Live handles, keyed by `wayland-client`'s object ID (which, unlike the protocol
    /// ID, includes a serial and so never aliases a destroyed handle)
    windows: HashMap<ObjectId, TrackedWindow>,
    /// Bind `wl_output` globals (the backend reports window placement)
    tracks_outputs: bool,
    /// Bound outputs, keyed by object ID
    outputs: HashMap<ObjectId, TrackedOutput>,
    /// The compositor sent `finished`: no further events will arrive
    finished: bool,
}

impl WindowTracker {
    /// `tracks_outputs`: the backend reports placement, so `wl_output` globals are bound
    pub(super) fn new(outbox: Arc<EventOutbox>, tracks_outputs: bool) -> Self {
        Self {
            outbox,
            next_id: 1,
            windows: HashMap::new(),
            tracks_outputs,
            outputs: HashMap::new(),
            finished: false,
        }
    }

    /// Whether `wl_output` globals (including hotplugged ones) should be bound
    pub(super) const fn tracks_outputs(&self) -> bool {
        self.tracks_outputs
    }

    /// A new toplevel handle was announced
    pub(super) fn handle_created(&mut self, key: ObjectId) {
        let id = self.next_id;
        self.next_id += 1;
        trace!("New toplevel handle {key} -> window id {id}");
        self.windows.insert(key, TrackedWindow {
            id,
            app_id: String::new(),
            title: String::new(),
            reported: None,
            pending_outputs: HashSet::new(),
            pending_minimized: false,
            outputs: HashSet::new(),
            minimized: false,
            reported_placement: Placement::default(),
        });
    }

    pub(super) fn set_title(&mut self, key: &ObjectId, title: String) {
        trace!("Toplevel {key} title: {}", for_log(&title));
        if let Some(window) = self.windows.get_mut(key) {
            window.title = title;
        }
    }

    pub(super) fn set_app_id(&mut self, key: &ObjectId, app_id: String) {
        trace!("Toplevel {key} app_id: {}", for_log(&app_id));
        if let Some(window) = self.windows.get_mut(key) {
            window.app_id = app_id;
        }
    }

    /// The toplevel became visible on `output` (applied on `done`)
    pub(super) fn output_entered(&mut self, key: &ObjectId, output: ObjectId) {
        trace!("Toplevel {key} entered output {output}");
        if let Some(window) = self.windows.get_mut(key) {
            window.pending_outputs.insert(output);
        }
    }

    /// The toplevel stopped being visible on `output` (applied on `done`)
    pub(super) fn output_left(&mut self, key: &ObjectId, output: &ObjectId) {
        trace!("Toplevel {key} left output {output}");
        if let Some(window) = self.windows.get_mut(key) {
            window.pending_outputs.remove(output);
        }
    }

    /// The toplevel's state flags changed; only `minimized` matters (applied on `done`)
    pub(super) fn set_minimized(&mut self, key: &ObjectId, minimized: bool) {
        trace!("Toplevel {key} minimized: {minimized}");
        if let Some(window) = self.windows.get_mut(key) {
            window.pending_minimized = minimized;
        }
    }

    /// The compositor finished a batch of updates for this handle
    ///
    /// Emits `Opened` on the first `done`, carrying the window's initial placement (so
    /// a window opening on a monitor is one event, not an `Opened` the daemon could act
    /// on before the `Placement` that follows). After that, `Changed` only when `app_id`
    /// or title differ from what was last reported (state-only updates such as focus or
    /// maximisation emit nothing), then `Placement` when the window's outputs or
    /// minimized flag differ from what was last reported.
    pub(super) fn done(&mut self, key: &ObjectId) {
        let Some(window) = self.windows.get_mut(key) else {
            return;
        };
        window.outputs.clone_from(&window.pending_outputs);
        window.minimized = window.pending_minimized;
        let event = match &window.reported {
            None => {
                let placement = placement_of(&self.outputs, window);
                debug!(
                    "Window opened: id={}, app_id='{}', title='{}', outputs=[{}], minimized={}",
                    window.id,
                    for_log(&window.app_id),
                    for_log(&window.title),
                    for_log(&placement.outputs.join(", ")),
                    placement.minimized
                );
                window.reported_placement = placement.clone();
                Some(WindowEvent::Opened {
                    id: window.id,
                    app_id: window.app_id.clone(),
                    title: window.title.clone(),
                    outputs: placement.outputs,
                    minimized: placement.minimized,
                })
            }
            Some((app_id, title)) if *app_id == window.app_id && *title == window.title => {
                trace!("Window {} done without app_id/title change", window.id);
                None
            }
            Some(_) => {
                trace!(
                    "Window changed: id={}, app_id='{}', title='{}'",
                    window.id,
                    for_log(&window.app_id),
                    for_log(&window.title)
                );
                Some(WindowEvent::Changed {
                    id: window.id,
                    app_id: window.app_id.clone(),
                    title: window.title.clone(),
                })
            }
        };
        if let Some(event) = event {
            window.reported = Some((window.app_id.clone(), window.title.clone()));
            self.outbox.push(event);
        }
        report_placement(&self.outbox, &self.outputs, window);
    }

    /// The toplevel was closed; the caller must destroy the handle
    ///
    /// A handle closed before its first `done` was never reported, so nothing is emitted.
    pub(super) fn closed(&mut self, key: &ObjectId) {
        let Some(window) = self.windows.remove(key) else {
            return;
        };
        if window.reported.is_some() {
            debug!("Window closed: id={}", window.id);
            self.outbox.push(WindowEvent::Closed { id: window.id });
        } else {
            debug!(
                "Window {} closed before its first done event, not reported",
                window.id
            );
        }
    }

    /// A `wl_output` global (registry name `global`) was bound as `proxy`
    pub(super) fn output_added(&mut self, proxy: WlOutput, global: u32) {
        debug!(
            "Bound `wl_output` global {global} (version {})",
            proxy.version()
        );
        self.outputs.insert(proxy.id(), TrackedOutput {
            proxy,
            global,
            name: None,
        });
    }

    /// An output announced its connector name
    ///
    /// Windows already reported on it (under its fallback label) get a new `Placement`.
    pub(super) fn output_named(&mut self, key: &ObjectId, name: String) {
        let Some(output) = self.outputs.get_mut(key) else {
            return;
        };
        debug!(
            "`wl_output` global {} is `{}`",
            output.global,
            for_log(&name)
        );
        output.name = Some(name);
        for window in self.windows.values_mut() {
            if window.outputs.contains(key) {
                report_placement(&self.outbox, &self.outputs, window);
            }
        }
    }

    /// The registry removed global `global`; if it is one of our outputs (a monitor was
    /// unplugged), forget and release it
    ///
    /// Windows on it get a `Placement` without it right away: compositors are not
    /// required to send `output_leave` (and `done`) for a removed output.
    pub(super) fn output_global_removed(&mut self, global: u32) {
        let Some(key) = self
            .outputs
            .iter()
            .find_map(|(key, o)| (o.global == global).then(|| key.clone()))
        else {
            return;
        };
        let Some(removed) = self.outputs.remove(&key) else {
            return;
        };
        debug!(
            "`wl_output` global {global} (`{}`) removed",
            for_log(&removed.label())
        );
        output::release(&removed.proxy);
        for window in self.windows.values_mut() {
            window.pending_outputs.remove(&key);
            if window.outputs.remove(&key) {
                report_placement(&self.outbox, &self.outputs, window);
            }
        }
    }

    /// The compositor will send no more toplevel events
    pub(super) fn set_finished(&mut self) {
        self.finished = true;
    }

    pub(super) const fn is_finished(&self) -> bool {
        self.finished
    }
}

/// `window`'s applied placement: the labels of its (still bound) outputs, sorted and
/// deduplicated, and its minimized flag
fn placement_of(outputs: &HashMap<ObjectId, TrackedOutput>, window: &TrackedWindow) -> Placement {
    let mut labels: Vec<String> = window
        .outputs
        .iter()
        .filter_map(|key| outputs.get(key).map(TrackedOutput::label))
        .collect();
    labels.sort_unstable();
    labels.dedup();
    Placement {
        outputs: labels,
        minimized: window.minimized,
    }
}

/// Queue a `Placement` for `window` if its applied placement differs from the last one
/// reported (only after its `Opened`, which carries the initial placement)
fn report_placement(
    outbox: &EventOutbox,
    outputs: &HashMap<ObjectId, TrackedOutput>,
    window: &mut TrackedWindow,
) {
    if window.reported.is_none() {
        return;
    }
    let placement = placement_of(outputs, window);
    if placement == window.reported_placement {
        return;
    }
    debug!(
        "Window placement: id={}, outputs=[{}], minimized={}",
        window.id,
        for_log(&placement.outputs.join(", ")),
        placement.minimized
    );
    outbox.push(WindowEvent::Placement {
        id: window.id,
        outputs: placement.outputs.clone(),
        minimized: placement.minimized,
    });
    window.reported_placement = placement;
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    fn opened(id: u64, title: &str) -> WindowEvent {
        WindowEvent::Opened {
            id,
            app_id: "app".into(),
            title: title.into(),
            outputs: Vec::new(),
            minimized: false,
        }
    }

    fn changed(id: u64, title: &str) -> WindowEvent {
        WindowEvent::Changed {
            id,
            app_id: "app".into(),
            title: title.into(),
        }
    }

    fn placement(id: u64, outputs: &[&str], minimized: bool) -> WindowEvent {
        WindowEvent::Placement {
            id,
            outputs: outputs.iter().map(|o| (*o).to_string()).collect(),
            minimized,
        }
    }

    fn render(queue: &VecDeque<WindowEvent>) -> Vec<String> {
        queue
            .iter()
            .map(|e| match e {
                WindowEvent::Opened {
                    id,
                    title,
                    outputs,
                    minimized,
                    ..
                } => format!(
                    "O{id}:{title}{}{}",
                    if outputs.is_empty() {
                        String::new()
                    } else {
                        format!("@{}", outputs.join("+"))
                    },
                    if *minimized { ":min" } else { "" }
                ),
                WindowEvent::Changed { id, title, .. } => format!("C{id}:{title}"),
                WindowEvent::Placement {
                    id,
                    outputs,
                    minimized,
                } => format!(
                    "P{id}:{}{}",
                    outputs.join("+"),
                    if *minimized { ":min" } else { "" }
                ),
                WindowEvent::Closed { id } => format!("X{id}"),
                WindowEvent::InitialSync => "SYNC".to_string(),
            })
            .collect()
    }

    fn push_all(events: Vec<WindowEvent>) -> Vec<String> {
        let mut queue = VecDeque::new();
        for event in events {
            coalesce_push(&mut queue, event);
        }
        render(&queue)
    }

    #[test]
    fn test_changed_supersedes_queued_changed_in_place() {
        let got = push_all(vec![
            opened(1, "a"),
            changed(1, "b"),
            opened(2, "x"),
            changed(1, "c"),
        ]);
        assert_eq!(got, vec!["O1:a", "C1:c", "O2:x"]);
    }

    /// Window 1's `Opened` was already delivered; its change (queued before window 2
    /// opened) is superseded while the channel is backed up. The coalesced change must
    /// stay ahead of `Opened{2}`, or the daemon would renew window 1's priority after
    /// window 2 opened and let it win although window 2 is newer.
    #[test]
    fn test_coalesced_changed_does_not_overtake_later_opened() {
        let got = push_all(vec![changed(1, "b"), opened(2, "x"), changed(1, "c")]);
        assert_eq!(got, vec!["C1:c", "O2:x"]);

        // A change with nothing queued for the window is appended as usual
        let got = push_all(vec![opened(2, "x"), changed(1, "c")]);
        assert_eq!(got, vec!["O2:x", "C1:c"]);
    }

    #[test]
    fn test_closed_cancels_undelivered_opened() {
        let got = push_all(vec![
            opened(1, "a"),
            changed(1, "b"),
            opened(2, "x"),
            WindowEvent::Closed { id: 1 },
        ]);
        assert_eq!(got, vec!["O2:x"]);
    }

    #[test]
    fn test_closed_after_delivered_opened_is_kept() {
        // Opened for 1 was already forwarded (not in the queue)
        let got = push_all(vec![changed(1, "b"), WindowEvent::Closed { id: 1 }]);
        assert_eq!(got, vec!["X1"]);
    }

    #[test]
    fn test_placement_supersedes_queued_placement_in_place() {
        // Window 1's `Opened` was already delivered
        let got = push_all(vec![
            placement(1, &["HDMI-A-1"], false),
            opened(2, "x"),
            placement(1, &["DP-1"], false),
            placement(1, &["DP-1"], true),
        ]);
        // The newest placement, at the slot of the first undelivered one
        assert_eq!(got, vec!["P1:DP-1:min", "O2:x"]);
    }

    /// While a window's `Opened` is undelivered, its placements are folded into it: the
    /// daemon learns where a new window is together with the window itself
    #[test]
    fn test_placement_folds_into_queued_opened() {
        let got = push_all(vec![
            opened(1, "a"),
            opened(2, "x"),
            placement(1, &["HDMI-A-1"], false),
            changed(1, "b"),
            placement(1, &["DP-1"], true),
        ]);
        assert_eq!(got, vec!["O1:a@DP-1:min", "O2:x", "C1:b"]);
    }

    /// The startup marker is never dropped or moved, whatever is coalesced around it
    #[test]
    fn test_initial_sync_is_kept_in_order() {
        let got = push_all(vec![
            opened(1, "a"),
            WindowEvent::InitialSync,
            changed(1, "b"),
            placement(1, &["DP-1"], false),
            opened(2, "x"),
            WindowEvent::Closed { id: 2 },
        ]);
        assert_eq!(got, vec!["O1:a@DP-1", "SYNC", "C1:b"]);
    }

    /// `Changed` and `Placement` keep separate slots: neither replaces the other, and both
    /// keep the position of their first undelivered occurrence
    #[test]
    fn test_placement_and_changed_coalesce_independently_in_order() {
        let got = push_all(vec![
            placement(1, &["HDMI-A-1"], false),
            changed(1, "b"),
            opened(2, "x"),
            changed(2, "y"),
            placement(2, &["DP-1"], false),
            changed(1, "c"),
            placement(1, &["DP-1", "HDMI-A-1"], false),
        ]);
        assert_eq!(got, vec!["P1:DP-1+HDMI-A-1", "C1:c", "O2:x@DP-1", "C2:y"]);
    }

    #[test]
    fn test_closed_drops_queued_placement() {
        // Opened for 1 was already forwarded: the close is kept, the placement dropped
        let got = push_all(vec![
            placement(1, &["DP-1"], false),
            opened(2, "x"),
            changed(1, "b"),
            WindowEvent::Closed { id: 1 },
        ]);
        assert_eq!(got, vec!["O2:x", "X1"]);

        // Opened still queued: the window disappears without a trace
        let got = push_all(vec![
            opened(1, "a"),
            placement(1, &["DP-1"], false),
            opened(2, "x"),
            placement(2, &["DP-1"], true),
            WindowEvent::Closed { id: 1 },
        ]);
        assert_eq!(got, vec!["O2:x@DP-1:min"]);
    }

    #[test]
    fn test_outbox_drains_then_ends_after_close() {
        let outbox = EventOutbox::new();
        outbox.push(opened(1, "a"));
        outbox.close();
        outbox.push(opened(2, "late")); // ignored after close
        assert_eq!(outbox.snapshot().len(), 1);
        assert!(outbox.next_blocking().is_some());
        assert!(outbox.next_blocking().is_none());
    }
}
