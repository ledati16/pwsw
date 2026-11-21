//! Desktop notifications off the async runtime
//!
//! `notify-rust` shows a notification with a blocking D-Bus call, so the daemon hands
//! notices to a dedicated thread over a small bounded channel and never waits:
//! - a full queue drops the notice (`try_send`)
//! - queued notices of the same kind coalesce: only the latest is shown
//! - each kind is shown at most once per [`MIN_INTERVAL`]; a notice that arrives
//!   sooner waits (and may be replaced by a newer one)

use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TrySendError};
use std::time::{Duration, Instant};

use tracing::{debug, warn};

use crate::notification::send_notification;

/// Minimum time between two shown notifications of the same kind
pub(crate) const MIN_INTERVAL: Duration = Duration::from_secs(1);
/// Queue capacity; notices beyond it are dropped
const QUEUE_CAPACITY: usize = 8;
/// How long shutdown waits for queued notices (e.g. "stopped") to be shown
pub(crate) const FLUSH_TIMEOUT: Duration = Duration::from_secs(2);

/// What a notice is about; coalescing and rate limiting are per kind
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum NoticeKind {
    /// The audio output changed (rule-driven or manual)
    Switch,
    /// The audio input (microphone) changed
    SourceSwitch,
    /// Daemon lifecycle and config reloads
    Daemon,
}

/// A notification to show
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Notice {
    pub kind: NoticeKind,
    pub summary: String,
    pub body: String,
    pub icon: Option<String>,
}

/// Sending side of the notification thread
pub(crate) struct Notifier {
    tx: Option<SyncSender<Notice>>,
    done: Option<Receiver<()>>,
}

impl Notifier {
    /// Start the notification thread, showing notices with [`send_notification`]
    ///
    /// A failure is logged as a warning once (no notification server is a common,
    /// lasting condition), then at debug until a notification works again.
    pub(crate) fn spawn() -> Self {
        let mut failures = Failures::default();
        Self::spawn_with(MIN_INTERVAL, move |notice: &Notice| {
            let result = send_notification(&notice.summary, &notice.body, notice.icon.as_deref());
            if failures.record(result.is_ok()) {
                if let Err(e) = result {
                    warn!(
                        "Notification failed: {}; further failures are logged at debug \
                         until one works again",
                        crate::sanitize::error_line(&e)
                    );
                }
            } else if let Err(e) = result {
                debug!("Notification failed: {e:#}");
            }
        })
    }

    /// Start the notification thread with a custom `show` function and rate limit
    pub(crate) fn spawn_with<F>(min_interval: Duration, show: F) -> Self
    where
        F: FnMut(&Notice) + Send + 'static,
    {
        let (tx, rx) = mpsc::sync_channel(QUEUE_CAPACITY);
        let (done_tx, done_rx) = mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name("pwsw-notify".to_string())
            .spawn(move || {
                run(&rx, min_interval, show);
                let _ = done_tx.send(());
            });
        match spawned {
            Ok(_) => Self {
                tx: Some(tx),
                done: Some(done_rx),
            },
            Err(e) => {
                warn!("Could not start notification thread, notifications disabled: {e}");
                Self::disabled()
            }
        }
    }

    /// A notifier that queues into a channel the caller reads (tests)
    #[cfg(test)]
    pub(crate) fn channel() -> (Self, Receiver<Notice>) {
        let (tx, rx) = mpsc::sync_channel(64);
        (
            Self {
                tx: Some(tx),
                done: None,
            },
            rx,
        )
    }

    /// A notifier that drops everything
    pub(crate) fn disabled() -> Self {
        Self {
            tx: None,
            done: None,
        }
    }

    /// Queue a notice without blocking; dropped if the queue is full
    pub(crate) fn send(&self, notice: Notice) {
        let Some(tx) = &self.tx else { return };
        match tx.try_send(notice) {
            Ok(()) => {}
            Err(TrySendError::Full(n)) => debug!(
                "Notification queue full, dropping: {}",
                crate::sanitize::for_log(&n.body)
            ),
            Err(TrySendError::Disconnected(_)) => debug!("Notification thread gone"),
        }
    }

    /// Stop accepting notices and wait (bounded by [`FLUSH_TIMEOUT`]) for the queue
    /// to be shown
    pub(crate) async fn shutdown(mut self) {
        self.tx = None;
        let Some(done) = self.done.take() else { return };
        let _ = tokio::task::spawn_blocking(move || done.recv_timeout(FLUSH_TIMEOUT)).await;
    }
}

/// Whether notifications are failing, so a lasting failure is warned about once
#[derive(Debug, Default)]
struct Failures {
    failing: bool,
}

impl Failures {
    /// Record one attempt; returns whether a failure deserves a warning (the first
    /// one, or the first after a notification worked)
    fn record(&mut self, ok: bool) -> bool {
        if ok {
            if self.failing {
                debug!("Notifications work again");
            }
            self.failing = false;
            return false;
        }
        !std::mem::replace(&mut self.failing, true)
    }
}

/// Keep at most one pending notice per kind: a newer one replaces the older
fn enqueue(pending: &mut Vec<Notice>, notice: Notice) {
    pending.retain(|n| n.kind != notice.kind);
    pending.push(notice);
}

/// The notification thread: coalesce, rate-limit, show; flush and exit on disconnect
fn run<F: FnMut(&Notice)>(rx: &Receiver<Notice>, min_interval: Duration, mut show: F) {
    let mut pending: Vec<Notice> = Vec::new();
    let mut last_shown: HashMap<NoticeKind, Instant> = HashMap::new();
    let mut connected = true;

    loop {
        if pending.is_empty() {
            if !connected {
                return;
            }
            match rx.recv() {
                Ok(n) => enqueue(&mut pending, n),
                Err(_) => return,
            }
        }
        while let Ok(n) = rx.try_recv() {
            enqueue(&mut pending, n);
        }

        let now = Instant::now();
        let wait_for = |kind: &NoticeKind| {
            last_shown.get(kind).map_or(Duration::ZERO, |t| {
                (*t + min_interval).saturating_duration_since(now)
            })
        };

        // Once the sender is gone, flush what is left without rate limiting
        let ready = pending
            .iter()
            .position(|n| !connected || wait_for(&n.kind).is_zero());
        if let Some(index) = ready {
            let notice = pending.remove(index);
            show(&notice);
            last_shown.insert(notice.kind, Instant::now());
            continue;
        }

        let wait = pending
            .iter()
            .map(|n| wait_for(&n.kind))
            .min()
            .unwrap_or(min_interval);
        match rx.recv_timeout(wait) {
            Ok(n) => enqueue(&mut pending, n),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => connected = false,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use pretty_assertions::assert_eq;

    use super::*;

    /// A lasting failure (no notification server) is warned about once, and again
    /// only after a notification worked in between
    #[test]
    fn test_failures_warn_once_per_outage() {
        let mut failures = Failures::default();
        assert!(failures.record(false), "first failure warns");
        assert!(!failures.record(false), "repeats do not");
        assert!(!failures.record(false));
        assert!(!failures.record(true), "a success never warns");
        assert!(failures.record(false), "a new outage warns again");
        assert!(!failures.record(true));
        assert!(!failures.record(true));
    }

    fn notice(kind: NoticeKind, body: &str) -> Notice {
        Notice {
            kind,
            summary: "s".to_string(),
            body: body.to_string(),
            icon: None,
        }
    }

    #[test]
    fn test_coalesces_queued_notices_of_same_kind() {
        let (tx, rx) = mpsc::sync_channel(QUEUE_CAPACITY);
        for body in ["a", "b", "c"] {
            tx.send(notice(NoticeKind::Switch, body)).expect("queue");
        }
        tx.send(notice(NoticeKind::Daemon, "d")).expect("queue");
        drop(tx);

        let mut shown = Vec::new();
        run(&rx, Duration::from_millis(30), |n| {
            shown.push(n.body.clone());
        });
        assert_eq!(shown, vec!["c".to_string(), "d".to_string()]);
    }

    /// Upper bound for the notification thread to react (real time)
    const WAIT: Duration = Duration::from_secs(30);

    #[test]
    fn test_rate_limits_per_kind_and_keeps_latest() {
        let shown = Arc::new(Mutex::new(Vec::<String>::new()));
        let sink = Arc::clone(&shown);
        let (first_shown, first) = mpsc::channel();
        // An interval no test run comes near: "second"/"third" can only be shown by
        // the final flush, however slowly this test runs
        let notifier = Notifier::spawn_with(Duration::from_hours(24), move |n| {
            sink.lock().expect("lock").push(n.body.clone());
            let _ = first_shown.send(());
        });

        notifier.send(notice(NoticeKind::Switch, "first"));
        // Once the first is shown, the next ones hit the rate limit
        first.recv_timeout(WAIT).expect("first notice shown");
        notifier.send(notice(NoticeKind::Switch, "second"));
        notifier.send(notice(NoticeKind::Switch, "third"));
        // Another kind is not held up by the switch notices' limit
        notifier.send(notice(NoticeKind::Daemon, "daemon"));
        first.recv_timeout(WAIT).expect("other kind shown");
        assert_eq!(shown.lock().expect("lock").clone(), vec!["first", "daemon"]);

        // Dropping flushes the latest pending notice
        let done = notifier.done;
        drop(notifier.tx);
        done.expect("done channel")
            .recv_timeout(WAIT)
            .expect("thread finished");

        let shown = shown.lock().expect("lock");
        assert_eq!(*shown, vec!["first", "daemon", "third"]);
    }

    /// A full queue drops the notice; `send` returns at once (bounded here, so a
    /// regression fails instead of hanging the test run)
    #[test]
    fn test_send_never_blocks_when_full() {
        let (tx, rx) = mpsc::sync_channel(1);
        let (finished, done) = mpsc::channel();
        std::thread::spawn(move || {
            let notifier = Notifier {
                tx: Some(tx),
                done: None,
            };
            for i in 0..10 {
                notifier.send(notice(NoticeKind::Switch, &i.to_string()));
            }
            let _ = finished.send(());
        });
        done.recv_timeout(WAIT)
            .expect("send returned although the queue was full");
        let queued: Vec<String> = rx.try_iter().map(|n| n.body).collect();
        assert_eq!(queued, vec!["0"], "the rest was dropped");
    }
}
