//! Protocol-independent window tracking shared by both toplevel backends
//!
//! [`WindowTracker`] is the `wayland-client` dispatch state for the Wayland thread. The
//! backends (`ext_toplevel`, `wlr_toplevel`) only translate protocol events into calls on
//! it; ID assignment, change detection and event delivery live here.
//!
//! Delivery goes through an [`EventOutbox`]: the Wayland thread pushes into it without
//! ever blocking, and a separate forwarder thread moves events from it into the bounded
//! `tokio` channel the daemon reads. While the daemon is slow, superseded events are
//! coalesced in the outbox (see [`coalesce_push`]), so memory stays bounded by the number
//! of windows rather than by the number of protocol events.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};

use tokio::sync::mpsc;
use tracing::{debug, trace};
use wayland_client::backend::ObjectId;

use super::WindowEvent;
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

/// Closes the outbox when dropped, so the forwarder stops even if the Wayland thread panics
pub(super) struct CloseOnDrop(pub(super) Arc<EventOutbox>);

impl Drop for CloseOnDrop {
    fn drop(&mut self) {
        self.0.close();
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

const fn event_id(event: &WindowEvent) -> u64 {
    match event {
        WindowEvent::Opened { id, .. }
        | WindowEvent::Changed { id, .. }
        | WindowEvent::Closed { id } => *id,
    }
}

/// Append `event` to `queue`, dropping events it makes redundant
///
/// Rules (all per window id):
/// - `Opened` is always appended.
/// - `Changed` replaces a queued `Changed` for the same window **in place** (the newest
///   state wins, but keeps the queue slot of the oldest undelivered change); with no
///   queued `Changed` it is appended. At most one `Changed` per window is ever queued.
/// - `Closed` removes any queued `Changed`. If the window's `Opened` is still queued, the
///   daemon never learnt about the window, so the `Opened` is removed and no `Closed` is
///   queued either. Otherwise `Closed` is appended.
///
/// Ordering guarantee: events that survive coalescing are delivered in the order they
/// were first queued, so no surviving event ever moves behind an event that was queued
/// after it. In particular a window whose (since superseded) change was queued before
/// another window's `Opened` is still processed before that `Opened`, so the daemon - which
/// renews a window's priority when it starts matching a rule - never ranks it above a
/// window that opened later merely because the channel was backed up. The cost is that
/// the coalesced change carries the latest state at the earliest change's position.
///
/// Consequently the consumer never sees `Changed` after `Closed`, and every `Opened` it
/// does see is eventually followed by that window's `Closed`.
fn coalesce_push(queue: &mut VecDeque<WindowEvent>, event: WindowEvent) {
    let id = event_id(&event);
    match event {
        WindowEvent::Opened { .. } => queue.push_back(event),
        WindowEvent::Changed { .. } => {
            if let Some(slot) = queue
                .iter_mut()
                .find(|e| matches!(e, WindowEvent::Changed { id: other, .. } if *other == id))
            {
                *slot = event;
            } else {
                queue.push_back(event);
            }
        }
        WindowEvent::Closed { .. } => {
            queue.retain(|e| !matches!(e, WindowEvent::Changed { id: other, .. } if *other == id));
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
    /// The compositor sent `finished`: no further events will arrive
    finished: bool,
}

impl WindowTracker {
    pub(super) fn new(outbox: Arc<EventOutbox>) -> Self {
        Self {
            outbox,
            next_id: 1,
            windows: HashMap::new(),
            finished: false,
        }
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

    /// The compositor finished a batch of updates for this handle
    ///
    /// Emits `Opened` on the first `done`, then `Changed` only when `app_id` or title
    /// differ from what was last reported (state-only updates such as focus or
    /// maximisation emit nothing).
    pub(super) fn done(&mut self, key: &ObjectId) {
        let Some(window) = self.windows.get_mut(key) else {
            return;
        };
        let event = match &window.reported {
            None => {
                debug!(
                    "Window opened: id={}, app_id='{}', title='{}'",
                    window.id,
                    for_log(&window.app_id),
                    for_log(&window.title)
                );
                WindowEvent::Opened {
                    id: window.id,
                    app_id: window.app_id.clone(),
                    title: window.title.clone(),
                }
            }
            Some((app_id, title)) if *app_id == window.app_id && *title == window.title => {
                trace!("Window {} done without app_id/title change", window.id);
                return;
            }
            Some(_) => {
                trace!(
                    "Window changed: id={}, app_id='{}', title='{}'",
                    window.id,
                    for_log(&window.app_id),
                    for_log(&window.title)
                );
                WindowEvent::Changed {
                    id: window.id,
                    app_id: window.app_id.clone(),
                    title: window.title.clone(),
                }
            }
        };
        window.reported = Some((window.app_id.clone(), window.title.clone()));
        self.outbox.push(event);
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

    /// The compositor will send no more toplevel events
    pub(super) fn set_finished(&mut self) {
        self.finished = true;
    }

    pub(super) const fn is_finished(&self) -> bool {
        self.finished
    }
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
        }
    }

    fn changed(id: u64, title: &str) -> WindowEvent {
        WindowEvent::Changed {
            id,
            app_id: "app".into(),
            title: title.into(),
        }
    }

    fn render(queue: &VecDeque<WindowEvent>) -> Vec<String> {
        queue
            .iter()
            .map(|e| match e {
                WindowEvent::Opened { id, title, .. } => format!("O{id}:{title}"),
                WindowEvent::Changed { id, title, .. } => format!("C{id}:{title}"),
                WindowEvent::Closed { id } => format!("X{id}"),
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
