//! The daemon's event loop
//!
//! Inputs (window events, `IPC` commands, config-file changes, signals) update the
//! [`State`] reducer; after each one the loop *reconciles*: when the sink the rules
//! want *changes*, it submits an activation to the worker and moves on. Results come
//! back as just another input, so the loop never waits on `PipeWire` and keeps
//! answering `IPC`, signals and window events during a slow profile switch.
//!
//! Switching only on a change of the rules' answer means pwsw never fights the user:
//! a default changed outside pwsw (e.g. `pavucontrol`, a `WirePlumber` auto-switch)
//! stays until the rules want something else. At startup the rules' answer is the
//! baseline - taken once the compositor has reported the windows already open (its
//! [`WindowEvent::InitialSync`], waited for at most [`INITIAL_SYNC_TIMEOUT`]), so they
//! do not each switch as they arrive; with `default_on_startup` it is activated,
//! otherwise nothing happens until it changes. Window events already queued when the
//! loop wakes up are applied together before one reconcile. A failed rule-driven
//! activation is retried with backoff, then once more on the next event or reload.
//!
//! `applied` is pwsw's view of the default sink: what it last activated, found at
//! startup, or saw when a manual request probed `PipeWire`. Manual requests are
//! decided by the worker against the real default (see [`crate::manual`]).
//!
//! The rules' answer is a list of candidates (a rule may name fallback sinks); the
//! worker activates the first reachable one. "Changed" means the list changed, so a
//! rule whose first choice is unreachable does not flip-flop. When a rule-driven
//! switch had to settle for a fallback, the next window event or reload (at most
//! every [`UPGRADE_CHECK_INTERVAL`]) asks the worker to *upgrade*: it switches only if
//! a higher-priority candidate is reachable by then, and otherwise touches nothing.
//! There is no polling: a sink that comes back without any window event is picked up
//! on the next one.
//!
//! With `switch_delay_ms` (the *settle delay*), a change of the rules' answer is not
//! acted on at once: the switch is submitted only if the answer still holds when the
//! delay has passed. Another change restarts the wait for the new answer, and a change
//! back to the sink that is applied cancels it, so a splash screen or a title that
//! flickers for a moment switches nothing. Manual requests, retries, upgrades and the
//! startup activation are never delayed.
//!
//! *Pausing* (`pwsw pause`) stops rule-driven switching: windows are still tracked,
//! reloads still apply, manual requests still work, but no rule-driven job is
//! submitted (pending retries and settle waits are dropped). Resuming hands control
//! back to the rules as if their answer had just changed: if it is not what is applied,
//! pwsw switches at once (no settle delay), also over a manual choice. The pause state
//! is kept in memory only; a restarted daemon starts unpaused.
//!
//! *Sources* (microphones, only when `[[sources]]` are configured) are a second,
//! independent dimension with exactly the same machinery: each [`Endpoint`] has its
//! own [`Lane`] (applied value, last answer, retries, settle wait, upgrade checks,
//! manual choice). The source answer comes from the highest-priority matching window
//! whose rule has a `source`, so a rule without one never moves the microphone. Jobs
//! of both lanes submitted in the same step reach the worker together and are
//! activated jointly (one snapshot, one profile switch for a shared card).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::time::Instant;
use tracing::{debug, error, info, warn};

use super::notifier::{Notice, NoticeKind, Notifier};
use super::worker::{self, Job, JobResult, Outcome, Pending, SinkSwitcher, Target};
use crate::compositor::{Capabilities, ProtocolPreference, WindowEvent};
use crate::config::{Config, ConfigNotFound, Endpoint, SinkConfig};
use crate::ipc::{EndpointStatus, FallbackSink, Response, RuleWindow, TrackedInfo, WindowInfo};
pub(crate) use crate::manual::ManualSwitch;
use crate::notification::{get_app_icon, get_sink_icon, get_source_icon};
use crate::state::{Departure, State};

/// Quiet period after the last config-file event before reloading (trailing edge)
pub(crate) const RELOAD_DEBOUNCE: Duration = Duration::from_millis(300);
/// Delays before retrying a failed rule-driven activation; then one more attempt on
/// the next window event or reload
pub(crate) const RETRY_BACKOFF: [Duration; 2] = [Duration::from_secs(2), Duration::from_secs(5)];
/// Minimum time between two checks for a higher-priority candidate while a rule's
/// fallback sink is applied (each check takes a `pw-dump`; title changes can be
/// frequent)
pub(crate) const UPGRADE_CHECK_INTERVAL: Duration = Duration::from_secs(2);

/// Most clients following the status stream at once ([`Command::Subscribe`])
pub(crate) const MAX_STATUS_SUBSCRIBERS: usize = 16;

/// How long startup waits for the compositor's list of already-open windows
/// ([`WindowEvent::InitialSync`]) before taking the baseline without it
pub(crate) const INITIAL_SYNC_TIMEOUT: Duration = Duration::from_secs(2);

/// Most window events applied in one step before reconciling: everything already
/// queued when the loop wakes up for one (a window opening on a monitor, a burst of
/// title changes) is folded into the state first, so it switches at most once
const WINDOW_EVENT_BATCH: usize = 256;

/// Notification title for sink switches
const SWITCH_TITLE: &str = "Audio Output";
/// Notification title for source (microphone) switches
const SOURCE_SWITCH_TITLE: &str = "Audio Input";

/// A signal the loop reacts to
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Signal {
    /// `SIGINT`: shut down
    Interrupt,
    /// `SIGTERM`: shut down
    Terminate,
    /// `SIGHUP`: reload the config now
    Hangup,
}

/// A request from an `IPC` connection to the loop
#[derive(Debug)]
pub(crate) enum Command {
    /// Read-only view of the daemon's state
    Query(oneshot::Sender<Snapshot>),
    /// Switch sinks (or the source, for [`ManualSwitch::SetSource`]); the reply is
    /// sent once the activation finished
    Switch {
        request: ManualSwitch,
        reply: oneshot::Sender<Response>,
    },
    /// Shut down gracefully
    Shutdown,
    /// Pause or resume rule-driven switching; answered with [`Response::PauseState`]
    Pause {
        request: PauseRequest,
        reply: oneshot::Sender<Response>,
    },
    /// Follow the status ([`LiveStatus`]); `None` when [`MAX_STATUS_SUBSCRIBERS`]
    /// clients follow it already
    Subscribe(oneshot::Sender<Option<watch::Receiver<LiveStatus>>>),
}

/// What the status stream reports, published with the status line (see
/// `publish_status`); compared as a whole, so subscribers only hear of changes
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct LiveStatus {
    pub paused: bool,
    /// The applied sink; `None` while unknown
    pub sink: Option<EndpointStatus>,
    /// The applied source; `None` while unknown or without `[[sources]]`
    pub source: Option<EndpointStatus>,
    pub sources_configured: bool,
}

/// What a pause request asks for
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PauseRequest {
    Pause,
    Resume,
    Toggle,
}

/// Read-only view of the daemon's state for `IPC` queries
#[derive(Debug, Clone)]
pub(crate) struct Snapshot {
    pub config: Arc<Config>,
    /// Sink pwsw last applied (or found at startup)
    pub applied: Option<String>,
    /// The rules' first choice right now
    #[cfg(test)]
    pub desired: String,
    /// Whether `applied` is a lower-priority candidate a rule settled for
    pub fallback: bool,
    /// "trigger: sink" of the window deciding the desired sink
    pub active_window: Option<String>,
    /// Windows matching a rule
    pub tracked_windows: usize,
    /// All open windows (sorted by id), with tracking info for matching ones
    pub windows: Vec<WindowInfo>,
    /// Whether a manual choice currently holds
    #[cfg(test)]
    pub manual_override: bool,
    /// Rule-driven switching is paused
    pub paused: bool,
    /// The candidates of a switch waiting for the settle delay
    #[cfg(test)]
    pub settling: Option<Vec<String>>,
    /// Source pwsw last applied (or found at startup); `None` while unknown
    pub applied_source: Option<String>,
    /// Whether `applied_source` is a fallback a rule settled for
    pub source_fallback: bool,
    /// Whether sources are configured (pwsw manages the default source)
    pub sources_configured: bool,
    /// The rules' first-choice source right now (`None` without sources)
    #[cfg(test)]
    pub desired_source: Option<String>,
    /// Whether a manual source choice currently holds
    #[cfg(test)]
    pub source_manual_override: bool,
    /// The candidates of a source switch waiting for the settle delay
    #[cfg(test)]
    pub source_settling: Option<Vec<String>>,
    /// How many jobs (of both lanes) the loop has submitted so far
    #[cfg(test)]
    pub jobs_submitted: u64,
    /// Whether a sink job is submitted and its result not in yet
    #[cfg(test)]
    pub sink_in_flight: bool,
    /// When the sink lane last submitted an upgrade check
    #[cfg(test)]
    pub last_upgrade_check: Option<Instant>,
}

/// Everything the loop listens to
pub(crate) struct Inputs {
    pub windows: mpsc::Receiver<WindowEvent>,
    pub commands: mpsc::Receiver<Command>,
    pub config_changed: mpsc::UnboundedReceiver<()>,
    pub signals: mpsc::Receiver<Signal>,
}

/// Callback that applies a config `log_level`
pub(crate) type LevelSetter = Box<dyn Fn(&str) + Send>;
/// Callback that observes each new `STATUS=` text
pub(crate) type StatusHook = Box<dyn Fn(&str) + Send>;
/// Callback run after every reload attempt
pub(crate) type ReloadHook = Box<dyn FnMut() + Send>;

/// Side effects that only make sense in a real daemon
#[derive(Default)]
pub(crate) struct Hooks {
    /// Send `sd_notify` messages (no-op without `NOTIFY_SOCKET`)
    pub systemd: bool,
    /// Re-apply the config's `log_level` after a reload
    pub set_log_level: Option<LevelSetter>,
    /// Observe the status line (also sent as `STATUS=` when `systemd` is set)
    pub status: Option<StatusHook>,
    /// After each reload attempt (e.g. re-resolve a symlinked config for the watcher)
    pub after_reload: Option<ReloadHook>,
}

/// The compositor connection the daemon runs on: what it provides, and the protocol
/// preference it was spawned with (chosen from the startup config)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CompositorInfo {
    pub capabilities: Capabilities,
    pub preference: ProtocolPreference,
}

/// Why rules with `output` cannot match on this compositor connection, if they cannot
/// (`None` when there are none, or the backend reports monitors)
///
/// Spawned with [`ProtocolPreference::PreferWlr`] (the startup config had such rules)
/// and still on `ext`, the compositor offers nothing else. Spawned without (the rules
/// came with a reload), a restart picks `wlr` if the compositor has it; the connection
/// is not swapped while running.
pub(crate) fn output_rules_warning(
    compositor: CompositorInfo,
    has_output_rules: bool,
) -> Option<&'static str> {
    if !has_output_rules || compositor.capabilities.monitor_info() {
        return None;
    }
    Some(match compositor.preference {
        ProtocolPreference::PreferWlr => {
            "Monitor rules need wlr-foreign-toplevel-management; this compositor only offers \
             ext-foreign-toplevel-list, so rules with `output` will never match"
        }
        ProtocolPreference::Auto => {
            "Rules with `output` need monitor information, which the current compositor \
             connection (ext-foreign-toplevel-list) does not report: restart pwsw to enable \
             them (systemctl --user restart pwsw); they need wlr-foreign-toplevel-management"
        }
    })
}

/// Who asked for an activation
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Origin {
    Rule,
    /// A check whether a higher-priority candidate than the applied fallback is
    /// reachable now (a rule-driven job that may change nothing)
    Upgrade,
    /// `restore_override`: the override state to return to if the request switches
    /// nothing (it failed, or there was nothing to do)
    Manual {
        restore_override: bool,
    },
    /// The source lane's answer riding along with a sink job ([`Target::Keep`]), so a
    /// profile switch for the sink keeps the microphone (or says that it did not)
    Keep,
}

/// The notification for a rule-driven switch, completed once the worker said which
/// candidate it activated
#[derive(Debug, Clone, PartialEq, Eq)]
struct RuleNotice {
    /// What decided the switch (the window's trigger, `Firefox closed`), if anything
    reason: Option<String>,
    /// The deciding window's app icon; `None` uses the sink's icon
    app_icon: Option<String>,
}

/// The latest job handed to the worker, until its result arrives
#[derive(Debug)]
struct Submitted {
    generation: u64,
    /// The rules' candidate list the job serves (rule-driven jobs)
    candidates: Option<Vec<String>>,
    origin: Origin,
    notice: Option<RuleNotice>,
}

/// A scheduled retry of a failed rule-driven activation
#[derive(Debug)]
struct Retry {
    candidates: Vec<String>,
    at: Instant,
    notice: Option<RuleNotice>,
}

/// A rule-driven switch waiting for the settle delay (`switch_delay_ms`): submitted
/// at `at` if the rules' answer is still `candidates` then
#[derive(Debug)]
struct Settle {
    candidates: Vec<String>,
    at: Instant,
    notice: Option<RuleNotice>,
}

/// A rule-driven job replaced by a manual request before its result came in (or a
/// settle wait the request took over)
#[derive(Debug)]
struct Superseded {
    candidates: Vec<String>,
    notice: Option<RuleNotice>,
    /// When the settle wait it was would have ended (`None`: a submitted job)
    settle_at: Option<Instant>,
}

/// One default pwsw manages: the sink, or the source when sources are configured
///
/// Everything the loop keeps per dimension lives here; the two lanes never look at
/// each other (except that their jobs travel to the worker together).
#[derive(Debug, Default)]
struct Lane {
    /// What pwsw last activated successfully (or found at startup)
    applied: Option<String>,
    /// `applied` is a lower-priority candidate a rule-driven switch settled for
    applied_fallback: bool,
    /// When the last upgrade check was submitted (see [`UPGRADE_CHECK_INTERVAL`])
    last_upgrade_check: Option<Instant>,
    /// The rules' candidates at the last reconcile; a change marks a rule-driven
    /// switch. `None` before the first answer (and for an inactive source lane).
    last_desired: Option<Vec<String>>,
    /// A manual switch holds until the desired answer changes
    manual_override: bool,
    /// The rules' target failed and retries ran out: try once more on the next event
    stalled: bool,
    /// A rule-driven job that a manual request replaced (or that failed while the
    /// manual request was queued behind it): if the manual request switches nothing,
    /// it is submitted again at once, since no later change of the rules' answer
    /// would bring it back
    superseded: Option<Superseded>,
    /// A changed answer of the rules waiting for the settle delay
    settle: Option<Settle>,
    /// The latest job of this lane handed to the worker, until its result arrives
    submitted: Option<Submitted>,
    retry: Option<Retry>,
    /// Retries scheduled for the current target so far
    retries: usize,
}

impl Lane {
    /// Forget the rules' answer (the lane has none any more: sources were removed)
    fn deactivate(&mut self) {
        self.last_desired = None;
        self.applied_fallback = false;
        self.manual_override = false;
        self.stalled = false;
        self.superseded = None;
        self.settle = None;
        self.retry = None;
        self.retries = 0;
    }
}

/// The daemon's event loop state
pub(crate) struct Daemon {
    state: State,
    config_path: PathBuf,
    jobs: watch::Sender<Pending>,
    results: mpsc::Receiver<JobResult>,
    notifier: Notifier,
    hooks: Hooks,
    /// Per [`Endpoint`] (index [`Endpoint::index`]): the sink lane, the source lane
    lanes: [Lane; 2],
    /// Jobs submitted since the last hand-over to the worker (sent together, so the
    /// worker activates both halves of one step jointly)
    outbox: Pending,
    /// Last status line published
    status: Option<String>,
    /// The status stream (no receiver of its own: subscribers hold them all)
    live: watch::Sender<LiveStatus>,
    /// Rule-driven switching is paused (in memory only)
    paused: bool,
    next_generation: u64,
    /// `IPC` replies waiting for the job of that lane and generation
    replies: Vec<(Endpoint, u64, oneshot::Sender<Response>)>,
    /// The compositor connection (`None` in tests that do not set one)
    compositor: Option<CompositorInfo>,
    /// Wait (at most this long) for [`WindowEvent::InitialSync`] before the startup
    /// baseline; `None`: take it at once (tests that do not model the marker)
    initial_sync: Option<Duration>,
    /// Counts the loop's steps (every input handled except a [`Command::Query`]), so
    /// tests can wait for the loop to act instead of polling on a clock
    #[cfg(test)]
    steps: watch::Sender<u64>,
}

impl Daemon {
    /// Build the loop and start its activation worker
    ///
    /// `applied` is the current default sink if known (queried at startup).
    pub(crate) fn new<S: SinkSwitcher>(
        state: State,
        config_path: PathBuf,
        switcher: Arc<S>,
        notifier: Notifier,
        hooks: Hooks,
        applied: Option<String>,
    ) -> Self {
        let (jobs, results) = worker::spawn(switcher);
        let mut lanes: [Lane; 2] = Default::default();
        lanes[Endpoint::Sink.index()].applied = applied;
        Self {
            state,
            config_path,
            jobs,
            results,
            notifier,
            hooks,
            lanes,
            outbox: [None, None],
            status: None,
            live: watch::Sender::new(LiveStatus::default()),
            paused: false,
            next_generation: 0,
            replies: Vec::new(),
            compositor: None,
            initial_sync: None,
            #[cfg(test)]
            steps: watch::Sender::new(0),
        }
    }

    /// Follows the loop's step count (see [`Daemon::steps`]); queries do not count,
    /// so polling with snapshots never wakes the poller itself
    #[cfg(test)]
    pub(crate) fn steps(&self) -> watch::Receiver<u64> {
        self.steps.subscribe()
    }

    /// Apply the windows the compositor reports as already open (up to its
    /// [`WindowEvent::InitialSync`], waiting at most `timeout`) before taking the
    /// startup baseline, so existing windows do not each switch once as they arrive
    #[must_use]
    pub(crate) const fn with_initial_sync(mut self, timeout: Duration) -> Self {
        self.initial_sync = Some(timeout);
        self
    }

    /// The compositor connection the window events come from (for the reload check
    /// of rules with `output`)
    #[must_use]
    pub(crate) fn with_compositor(mut self, compositor: CompositorInfo) -> Self {
        self.compositor = Some(compositor);
        self
    }

    /// The current default source, if known (queried at startup)
    #[must_use]
    pub(crate) fn with_applied_source(mut self, source: Option<String>) -> Self {
        self.lane_mut(Endpoint::Source).applied = source;
        self
    }

    fn lane(&self, kind: Endpoint) -> &Lane {
        &self.lanes[kind.index()]
    }

    fn lane_mut(&mut self, kind: Endpoint) -> &mut Lane {
        &mut self.lanes[kind.index()]
    }

    /// The rules' candidates for `kind` right now; `None` when the lane is inactive
    /// (no sources configured)
    fn desired_names(&self, kind: Endpoint) -> Option<Vec<String>> {
        self.state.desired_for(kind).map(|d| d.names())
    }

    /// Run until shutdown
    ///
    /// # Errors
    /// Returns an error if the compositor connection is lost (the window event
    /// channel closes), so the process exits non-zero and a supervisor restarts it.
    pub(crate) async fn run(mut self, mut inputs: Inputs) -> Result<()> {
        let result = match self.await_initial_windows(&mut inputs.windows).await {
            Ok(()) => {
                self.start();
                self.event_loop(&mut inputs).await
            }
            Err(e) => Err(e),
        };

        self.sd_notify(&[sd_notify::NotifyState::Stopping]);
        if result.is_ok() && self.state.config().settings.notify_manual {
            self.notifier.send(Notice {
                kind: NoticeKind::Daemon,
                summary: "PWSW Stopped".to_string(),
                body: "Audio switcher stopped".to_string(),
                icon: None,
            });
        }
        let Self { notifier, jobs, .. } = self;
        drop(jobs);
        notifier.shutdown().await;
        result
    }

    /// Startup, before the baseline: apply window events up to the compositor's
    /// [`WindowEvent::InitialSync`] (with [`Daemon::with_initial_sync`]; at most its
    /// timeout, then carry on with a warning)
    ///
    /// # Errors
    /// Returns an error if the window event channel closes meanwhile.
    async fn await_initial_windows(
        &mut self,
        windows: &mut mpsc::Receiver<WindowEvent>,
    ) -> Result<()> {
        let Some(timeout) = self.initial_sync else {
            return Ok(());
        };
        let deadline = Instant::now() + timeout;
        let mut events = 0usize;
        loop {
            match tokio::time::timeout_at(deadline, windows.recv()).await {
                Err(_) => {
                    warn!(
                        "The compositor did not finish listing the open windows within {} ms; \
                         starting with the {events} window events received so far",
                        timeout.as_millis()
                    );
                    return Ok(());
                }
                Ok(None) => return Err(compositor_lost()),
                Ok(Some(WindowEvent::InitialSync)) => {
                    debug!("Startup: open windows known ({events} window events)");
                    return Ok(());
                }
                Ok(Some(event)) => {
                    events += 1;
                    self.state.apply(event);
                }
            }
        }
    }

    /// The main loop, after startup; returns how it ended
    async fn event_loop(&mut self, inputs: &mut Inputs) -> Result<()> {
        let mut reload_at: Option<Instant> = None;

        loop {
            self.flush_jobs();
            let retry_at = self
                .lanes
                .iter()
                .filter_map(|l| l.retry.as_ref().map(|r| r.at))
                .min();
            let settle_at = self
                .lanes
                .iter()
                .filter_map(|l| l.settle.as_ref().map(|s| s.at))
                .min();
            tokio::select! {
                event = inputs.windows.recv() => {
                    let Some(event) = event else {
                        break Err(compositor_lost());
                    };
                    let departure = self.apply_window_events(event, &mut inputs.windows);
                    self.reconcile(departure.as_ref());
                }
                Some(result) = self.results.recv() => self.on_result(result),
                Some(command) = inputs.commands.recv() => match command {
                    Command::Query(reply) => {
                        let _ = reply.send(self.snapshot());
                        // Read-only: not a step
                        #[cfg(test)]
                        continue;
                    }
                    Command::Switch { request, reply } => self.on_manual(request, reply),
                    Command::Pause { request, reply } => {
                        let _ = reply.send(self.on_pause(request));
                    }
                    Command::Subscribe(reply) => {
                        let full = self.live.receiver_count() >= MAX_STATUS_SUBSCRIBERS;
                        let _ = reply.send((!full).then(|| self.live.subscribe()));
                    }
                    Command::Shutdown => {
                        info!("Shutting down (IPC request)");
                        break Ok(());
                    }
                },
                Some(()) = inputs.config_changed.recv() => {
                    reload_at = Some(Instant::now() + RELOAD_DEBOUNCE);
                }
                () = sleep_until(reload_at), if reload_at.is_some() => {
                    reload_at = None;
                    info!("Config file changed, reloading");
                    self.reload().await;
                }
                () = sleep_until(retry_at), if retry_at.is_some() => self.on_retry_due(),
                () = sleep_until(settle_at), if settle_at.is_some() => self.on_settle_due(),
                Some(signal) = inputs.signals.recv() => match signal {
                    Signal::Hangup => {
                        info!("SIGHUP received, reloading config");
                        reload_at = None;
                        self.reload().await;
                    }
                    Signal::Interrupt => {
                        info!("Shutting down (SIGINT)");
                        break Ok(());
                    }
                    Signal::Terminate => {
                        info!("Shutting down (SIGTERM)");
                        break Ok(());
                    }
                },
            }
            #[cfg(test)]
            self.steps.send_modify(|n| *n += 1);
        }
    }

    /// Apply `first` and every window event already queued behind it (at most
    /// [`WINDOW_EVENT_BATCH`] in all), so one reconcile sees their combined effect
    ///
    /// Returns the last departure among them (the window whose end is named if the
    /// rules fall back to the default).
    fn apply_window_events(
        &mut self,
        first: WindowEvent,
        windows: &mut mpsc::Receiver<WindowEvent>,
    ) -> Option<Departure> {
        let mut departure = self.state.apply(first);
        for _ in 1..WINDOW_EVENT_BATCH {
            // Empty, or closed (the next `recv` reports that)
            let Ok(event) = windows.try_recv() else {
                break;
            };
            if let Some(departed) = self.state.apply(event) {
                departure = Some(departed);
            }
        }
        departure
    }

    /// Hand the jobs submitted since the last call to the worker, all at once
    fn flush_jobs(&mut self) {
        if self.outbox.iter().all(Option::is_none) {
            return;
        }
        self.attach_source_keep();
        let outbox = std::mem::take(&mut self.outbox);
        self.jobs.send_modify(|pending| {
            for (slot, job) in pending.iter_mut().zip(outbox) {
                if job.is_some() {
                    *slot = job;
                }
            }
        });
    }

    /// A sink job going out alone while sources are configured takes the source lane's
    /// current answer along ([`Target::Keep`]): a sink's profile switch on the card of
    /// the managed microphone is then planned together with it (one profile keeping
    /// both, if any), and the source lane learns whether the microphone survived
    ///
    /// The answer is the rules' candidates while the lane follows them, else just the
    /// source that is applied (a manual choice, a settle wait, paused). Nothing rides
    /// along while the lane has a job of its own pending or running.
    fn attach_source_keep(&mut self) {
        let (sink, source) = (Endpoint::Sink.index(), Endpoint::Source.index());
        if self.outbox[sink].is_none()
            || self.outbox[source].is_some()
            || self.state.config().sources.is_empty()
        {
            return;
        }
        let lane = self.lane(Endpoint::Source);
        if lane.submitted.is_some() {
            return;
        }
        let following = !self.paused && !lane.manual_override && lane.settle.is_none();
        let candidates = match (&lane.last_desired, &lane.applied) {
            (Some(desired), _) if following => desired.clone(),
            (_, Some(applied)) => vec![applied.clone()],
            _ => return,
        };
        self.submit_job(
            Endpoint::Source,
            Target::Keep {
                candidates: candidates.clone(),
            },
            Some(candidates),
            Origin::Keep,
            None,
        );
    }

    /// Startup: take the rules' answers as the baseline and, with
    /// `default_on_startup`, activate them (failures retry with backoff)
    fn start(&mut self) {
        for kind in Endpoint::ALL {
            let Some(names) = self.desired_names(kind) else {
                continue;
            };
            self.lane_mut(kind).last_desired = Some(names.clone());
            if self.state.config().settings.default_on_startup
                && self.lane(kind).applied.as_deref() != names.first().map(String::as_str)
            {
                info!(
                    "Startup: switching to {} (what the rules want now)",
                    self.candidates_desc(kind, &names)
                );
                self.submit(kind, names, None);
            }
        }
        self.publish_status();
    }

    /// Whether the `kind` default is, or is about to be, what `candidates` asks for:
    /// a pending rule-driven job serves that list, or (nothing pending) `applied` is
    /// its first choice
    fn serves(&self, kind: Endpoint, candidates: &[String]) -> bool {
        let lane = self.lane(kind);
        match &lane.submitted {
            Some(Submitted {
                origin: Origin::Rule | Origin::Upgrade,
                candidates: Some(pending),
                ..
            }) => pending == candidates,
            // A manual job: its target is not chosen yet
            Some(_) => false,
            None => lane.applied.as_deref() == candidates.first().map(String::as_str),
        }
    }

    /// Candidates for the log: `Desc (name)`, or `first reachable of A, B` (sources:
    /// `source Desc (name)`)
    fn candidates_desc(&self, kind: Endpoint, candidates: &[String]) -> String {
        let prefix = match kind {
            Endpoint::Sink => "",
            Endpoint::Source => "source ",
        };
        match candidates {
            [single] => format!("{prefix}{} ({single})", self.endpoint_desc(kind, single)),
            _ => format!(
                "first reachable {prefix}of {}",
                candidates
                    .iter()
                    .map(|c| self.endpoint_desc(kind, c))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        }
    }

    /// Where the applied value is in the rules' current candidates, when it is a
    /// fallback a rule-driven switch settled for
    fn applied_fallback_rank(&self, kind: Endpoint) -> Option<usize> {
        let lane = self.lane(kind);
        if !lane.applied_fallback {
            return None;
        }
        let applied = lane.applied.as_deref()?;
        lane.last_desired
            .as_ref()?
            .iter()
            .position(|c| c == applied)
            .filter(|&rank| rank > 0)
    }

    fn endpoint_desc(&self, kind: Endpoint, name: &str) -> String {
        self.state.config().endpoint_desc(kind, name)
    }

    /// The configured entry of `kind` named `name`
    fn configured(&self, kind: Endpoint, name: &str) -> Option<&SinkConfig> {
        self.state
            .config()
            .endpoints(kind)
            .iter()
            .find(|s| s.name == name)
    }

    /// Whether the status line may be refreshed now: nothing in flight and the
    /// default sink known
    fn settled(&self) -> bool {
        self.lanes.iter().all(|l| l.submitted.is_none())
            && self.lane(Endpoint::Sink).applied.is_some()
    }

    /// Switch when the rules changed their mind (or a stalled target gets another try,
    /// or an applied fallback may be upgraded), for each lane
    fn reconcile(&mut self, departure: Option<&Departure>) {
        for kind in Endpoint::ALL {
            // A lane whose endpoints were removed is dropped even while paused (a
            // manual choice or retry for sources that no longer exist must not linger)
            if self.desired_names(kind).is_none() {
                if self.lane(kind).last_desired.is_some() {
                    debug!(
                        "No {} configured any more; not managing them",
                        kind.plural()
                    );
                    self.lane_mut(kind).deactivate();
                }
            } else if !self.paused {
                self.reconcile_lane(kind, departure);
            }
        }
        // Windows and config stay current while paused; `resume` compares with them.
        // Nothing to switch: keep the status line current (e.g. a reload renamed it).
        if self.settled() {
            self.publish_status();
        }
    }

    /// Reconcile an active lane (it has an answer)
    fn reconcile_lane(&mut self, kind: Endpoint, departure: Option<&Departure>) {
        let Some(names) = self.desired_names(kind) else {
            return;
        };
        let lane = self.lane(kind);
        if lane.last_desired.as_deref() != Some(names.as_slice()) {
            debug!(
                "Desired {} changed: [{}] → [{}]",
                kind.plural(),
                lane.last_desired
                    .as_deref()
                    .map_or_else(|| "(none)".to_string(), |d| d.join(", ")),
                names.join(", ")
            );
            let delay = self.switch_delay();
            self.adopt(kind, names, departure, delay);
        } else if lane.stalled
            && !lane.manual_override
            && lane.submitted.is_none()
            && lane.retry.is_none()
        {
            info!("Trying {} again", self.candidates_desc(kind, &names));
            let lane = self.lane_mut(kind);
            lane.stalled = false;
            lane.retries = 0;
            let notice = self.rule_notice(kind, departure);
            self.submit(kind, names, notice);
        } else if let Some(rank) = self.upgrade_due(kind) {
            debug!(
                "Applied {} is fallback #{rank}; checking for a higher-priority one",
                kind.noun()
            );
            self.lane_mut(kind).last_upgrade_check = Some(Instant::now());
            let notice = self.rule_notice(kind, departure);
            self.submit_job(
                kind,
                Target::Rule {
                    candidates: names.clone(),
                    upgrade_from: Some(rank),
                },
                Some(names),
                Origin::Upgrade,
                notice,
            );
        }
    }

    /// The configured settle delay, `None` when rule-driven switches are immediate
    fn switch_delay(&self) -> Option<Duration> {
        match self.state.config().settings.switch_delay_ms {
            0 => None,
            ms => Some(Duration::from_millis(u64::from(ms))),
        }
    }

    /// Take `names` as the rules' new answer for `kind` (it changed, or `resume` hands
    /// control back to the rules): what held for the old answer (manual choice,
    /// retries, a settle wait) is dropped, and unless the default already is (or is
    /// about to be) `names`' first choice, a switch is submitted, after `delay` if
    /// given
    ///
    /// Returns whether a job was submitted now.
    fn adopt(
        &mut self,
        kind: Endpoint,
        names: Vec<String>,
        departure: Option<&Departure>,
        delay: Option<Duration>,
    ) -> bool {
        let lane = self.lane_mut(kind);
        lane.last_desired = Some(names.clone());
        lane.manual_override = false;
        lane.retry = None;
        lane.stalled = false;
        lane.superseded = None;
        lane.retries = 0;
        let settling = lane.settle.take();
        if self.serves(kind, &names) {
            if let Some(settle) = settling {
                debug!(
                    "Rules want the current {} again; not switching to {}",
                    kind.noun(),
                    self.candidates_desc(kind, &settle.candidates)
                );
            }
            let lane = self.lane_mut(kind);
            if lane.submitted.is_none() {
                // The applied value is the new list's first choice
                lane.applied_fallback = false;
            }
            return false;
        }
        let notice = self.rule_notice(kind, departure);
        if let Some(delay) = delay {
            debug!(
                "Switching to {} in {} ms if the rules still want it then",
                self.candidates_desc(kind, &names),
                delay.as_millis()
            );
            self.lane_mut(kind).settle = Some(Settle {
                candidates: names,
                at: Instant::now() + delay,
                notice,
            });
            return false;
        }
        self.submit(kind, names, notice);
        true
    }

    /// A settle delay passed: switch if the rules' answer is still the one that
    /// started it (any change since replaced or cancelled the wait); each lane waits
    /// on its own
    fn on_settle_due(&mut self) {
        let now = Instant::now();
        for kind in Endpoint::ALL {
            let lane = self.lane_mut(kind);
            if lane.settle.as_ref().is_none_or(|s| s.at > now) {
                continue;
            }
            let Some(settle) = lane.settle.take() else {
                continue;
            };
            let lane = self.lane(kind);
            if self.paused
                || lane.manual_override
                || lane.last_desired.as_ref() != Some(&settle.candidates)
                || self.serves(kind, &settle.candidates)
            {
                continue;
            }
            self.submit(kind, settle.candidates, settle.notice);
        }
    }

    /// Pause or resume rule-driven switching
    fn on_pause(&mut self, request: PauseRequest) -> Response {
        let paused = match request {
            PauseRequest::Pause => true,
            PauseRequest::Resume => false,
            PauseRequest::Toggle => !self.paused,
        };
        let changed = paused != self.paused;
        if changed {
            self.paused = paused;
            if paused {
                self.pause();
            } else {
                self.resume();
            }
        }
        Response::PauseState {
            paused: self.paused,
            changed,
        }
    }

    /// Stop rule-driven switching: pending retries and settle waits are dropped (a
    /// job already handed to the worker still completes)
    fn pause(&mut self) {
        info!("Paused: no rule-driven switches until resumed (windows are still tracked)");
        for lane in &mut self.lanes {
            lane.settle = None;
            lane.retry = None;
            lane.stalled = false;
            lane.superseded = None;
        }
        if self.state.config().settings.notify_manual {
            self.notify_daemon("Switching Paused", "Rules will not switch the audio output");
        }
        self.publish_status();
    }

    /// Hand control back to the rules: switch at once if their current answer is not
    /// what is applied (counted as a change of the rules' answer, so it also ends a
    /// manual choice)
    fn resume(&mut self) {
        info!("Resumed: rule-driven switching is on again");
        if self.state.config().settings.notify_manual {
            self.notify_daemon("Switching Resumed", "Rules switch the audio output again");
        }
        for kind in Endpoint::ALL {
            if let Some(names) = self.desired_names(kind) {
                self.adopt(kind, names, None, None);
            }
        }
        self.publish_status();
    }

    /// The applied fallback's rank, when it is time to look for a better candidate:
    /// nothing pending or held, and the last check is [`UPGRADE_CHECK_INTERVAL`] ago
    fn upgrade_due(&self, kind: Endpoint) -> Option<usize> {
        let lane = self.lane(kind);
        if lane.manual_override
            || self.paused
            || lane.settle.is_some()
            || lane.stalled
            || lane.submitted.is_some()
            || lane.retry.is_some()
            || lane
                .last_upgrade_check
                .is_some_and(|at| at.elapsed() < UPGRADE_CHECK_INTERVAL)
        {
            return None;
        }
        self.applied_fallback_rank(kind)
    }

    /// The notification for a rule-driven switch to the desired `kind`, if wanted
    ///
    /// The flag and the reason come from the window that decides the target; when
    /// falling back to the default, from the window that departed.
    fn rule_notice(&self, kind: Endpoint, departure: Option<&Departure>) -> Option<RuleNotice> {
        if let Some(window) = self.state.desired_for(kind).and_then(|d| d.window) {
            return window.notify.then(|| RuleNotice {
                reason: Some(window.trigger.to_string()),
                app_icon: Some(get_app_icon(window.app_id)),
            });
        }
        let notify = departure.map_or(self.state.config().settings.notify_rules, |d| d.notify);
        notify.then(|| RuleNotice {
            reason: departure.map(|d| {
                let what = if d.closed { "closed" } else { "ended" };
                format!("{} {what}", d.trigger)
            }),
            app_icon: None,
        })
    }

    /// The icon of a configured sink or source
    fn endpoint_icon(kind: Endpoint, entry: &SinkConfig) -> String {
        match kind {
            Endpoint::Sink => get_sink_icon(entry),
            Endpoint::Source => get_source_icon(entry),
        }
    }

    /// Notification kind and title for switches of `kind`
    const fn switch_notice(kind: Endpoint) -> (NoticeKind, &'static str) {
        match kind {
            Endpoint::Sink => (NoticeKind::Switch, SWITCH_TITLE),
            Endpoint::Source => (NoticeKind::SourceSwitch, SOURCE_SWITCH_TITLE),
        }
    }

    /// Complete a rule-driven switch's notification for what it activated
    fn complete_notice(
        &self,
        kind: Endpoint,
        notice: &RuleNotice,
        name: &str,
        fallback: bool,
    ) -> Notice {
        let configured = self.configured(kind, name);
        let mut desc = configured.map_or_else(|| name.to_string(), |s| s.desc.clone());
        if fallback {
            desc.push_str(" (fallback)");
        }
        let (notice_kind, title) = Self::switch_notice(kind);
        Notice {
            kind: notice_kind,
            summary: title.to_string(),
            body: match &notice.reason {
                Some(reason) => format!("{desc} → {reason}"),
                None => desc,
            },
            icon: notice
                .app_icon
                .clone()
                .or_else(|| configured.map(|c| Self::endpoint_icon(kind, c))),
        }
    }

    /// The notification for a manual switch to `name`, if wanted
    fn manual_notice(&self, kind: Endpoint, name: &str) -> Option<Notice> {
        if !self.state.config().settings.notify_manual {
            return None;
        }
        let entry = self.configured(kind, name)?;
        let (notice_kind, title) = Self::switch_notice(kind);
        Some(Notice {
            kind: notice_kind,
            summary: title.to_string(),
            body: entry.desc.clone(),
            icon: Some(Self::endpoint_icon(kind, entry)),
        })
    }

    /// Submit a rule-driven activation of the first reachable of `candidates`
    fn submit(
        &mut self,
        kind: Endpoint,
        candidates: Vec<String>,
        notice: Option<RuleNotice>,
    ) -> u64 {
        info!("Switching: {}", self.candidates_desc(kind, &candidates));
        self.submit_job(
            kind,
            Target::Rule {
                candidates: candidates.clone(),
                upgrade_from: None,
            },
            Some(candidates),
            Origin::Rule,
            notice,
        )
    }

    /// Queue a job for the worker (replacing any of the lane not yet started; sent at
    /// the end of this step) and return its generation
    fn submit_job(
        &mut self,
        kind: Endpoint,
        target: Target,
        candidates: Option<Vec<String>>,
        origin: Origin,
        notice: Option<RuleNotice>,
    ) -> u64 {
        self.next_generation += 1;
        let generation = self.next_generation;
        self.outbox[kind.index()] = Some(Job {
            generation,
            kind,
            target,
        });
        self.lane_mut(kind).submitted = Some(Submitted {
            generation,
            candidates,
            origin,
            notice,
        });
        generation
    }

    fn on_result(&mut self, result: JobResult) {
        let kind = result.kind;
        let noun = kind.noun();
        let is_latest = self
            .lane(kind)
            .submitted
            .as_ref()
            .is_some_and(|s| s.generation == result.generation);
        let submitted = if is_latest {
            self.lane_mut(kind).submitted.take()
        } else {
            None
        };

        // A manual request looked at the real default first: that is the truth now
        if let Some(observed) = result.observed
            && self.lane(kind).applied.as_deref() != Some(observed.as_str())
        {
            debug!(
                "Default {noun} is {observed} (last known: {})",
                self.lane(kind).applied.as_deref().unwrap_or("unknown")
            );
            let lane = self.lane_mut(kind);
            lane.applied = Some(observed);
            lane.applied_fallback = false;
        }

        let tried = result.tried;
        let response = match result.outcome {
            Outcome::Activated { index, node_name } if index < tried.len() => {
                let name = tried[index].clone();
                let fallback = index > 0;
                let mut desc = self.endpoint_desc(kind, &name);
                let what = match kind {
                    Endpoint::Sink => "Audio output",
                    Endpoint::Source => "Audio input",
                };
                info!(
                    "{what} is now {desc}{} ({node_name})",
                    if fallback { " (fallback)" } else { "" }
                );
                let notice = match &submitted {
                    Some(Submitted {
                        origin: origin @ (Origin::Rule | Origin::Upgrade),
                        notice,
                        ..
                    }) => {
                        self.lane_mut(kind).stalled = false;
                        // An upgrade whose fallback chain ended on the same target
                        // changed nothing audible
                        let changed = *origin == Origin::Rule
                            || self.lane(kind).applied.as_deref() != Some(name.as_str());
                        notice
                            .as_ref()
                            .filter(|_| changed)
                            .map(|n| self.complete_notice(kind, n, &name, fallback))
                    }
                    Some(Submitted {
                        origin: Origin::Manual { .. },
                        ..
                    }) => {
                        // The user's choice holds now, until the rules' answer changes
                        self.lane_mut(kind).superseded = None;
                        self.manual_notice(kind, &name)
                    }
                    // The microphone was set again after the sink's profile switch
                    Some(Submitted {
                        origin: Origin::Keep,
                        ..
                    })
                    | None => None,
                };
                let lane = self.lane_mut(kind);
                if lane
                    .superseded
                    .as_ref()
                    .is_some_and(|s| s.candidates == tried)
                {
                    // The superseded rule job had already started, and it succeeded
                    lane.superseded = None;
                }
                if let Some(notice) = notice {
                    self.notifier.send(notice);
                }
                let lane = self.lane_mut(kind);
                lane.applied = Some(name);
                lane.applied_fallback = fallback;
                if fallback {
                    desc.push_str(" (fallback)");
                }
                Response::SinkSwitched { desc, node_name }
            }
            Outcome::Activated { node_name, .. } => {
                // Not produced by the worker; keep the loop consistent anyway
                warn!("Activation result without its {noun} (node {node_name})");
                Response::Error {
                    message: format!("Internal error: activation result without its {noun}"),
                }
            }
            Outcome::Unchanged if submitted.as_ref().is_some_and(|s| s.origin == Origin::Keep) => {
                Response::Ok {
                    message: format!("The sink's activation left the {noun} alone"),
                }
            }
            Outcome::Unchanged => {
                let message = format!(
                    "No higher-priority {noun} reachable; staying on {}",
                    self.lane(kind).applied.as_deref().map_or_else(
                        || format!("the current {noun}"),
                        |a| self.endpoint_desc(kind, a)
                    )
                );
                debug!("{message}");
                Response::Ok { message }
            }
            Outcome::Done(message) => {
                info!("{message}");
                self.settle_manual(kind, submitted.as_ref());
                Response::Ok { message }
            }
            Outcome::Failed(message)
                if submitted.as_ref().is_some_and(|s| s.origin == Origin::Keep) =>
            {
                self.keep_failed(kind, &tried, &message);
                Response::Error { message }
            }
            Outcome::Failed(message) => {
                let message = match tried.as_slice() {
                    [] => message,
                    [single] => {
                        format!(
                            "Failed to activate {noun} '{}': {message}",
                            self.endpoint_desc(kind, single)
                        )
                    }
                    several => format!(
                        "Failed to activate any of {}: {message}",
                        several
                            .iter()
                            .map(|s| format!("'{}'", self.endpoint_desc(kind, s)))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                };
                error!("{message}");
                match submitted {
                    Some(Submitted {
                        origin: Origin::Rule | Origin::Upgrade,
                        candidates: Some(candidates),
                        notice,
                        ..
                    }) => self.schedule_retry(kind, candidates, notice),
                    manual @ Some(Submitted {
                        origin: Origin::Manual { .. },
                        ..
                    }) => self.settle_manual(kind, manual.as_ref()),
                    _ => {}
                }
                Response::Error { message }
            }
        };
        self.publish_status();
        self.answer(kind, result.generation, &response);
    }

    /// The sink's profile switch did not keep the `kind` (microphone) `tried` it rode
    /// along with: pwsw no longer knows what the default is; while the lane follows
    /// the rules, their target gets another try on the next event or reload
    fn keep_failed(&mut self, kind: Endpoint, tried: &[String], message: &str) {
        warn!(
            "The sink's profile switch did not keep the {} ({}): {message}",
            kind.noun(),
            self.candidates_desc(kind, tried)
        );
        let following = !self.paused;
        let lane = self.lane_mut(kind);
        lane.applied = None;
        lane.applied_fallback = false;
        if following && !lane.manual_override && lane.last_desired.as_deref() == Some(tried) {
            lane.stalled = true;
        }
    }

    /// A manual request that switched nothing: the default stays where it was, and
    /// so does the override state from before the request
    ///
    /// If the request had replaced a rule-driven switch (or that switch failed while
    /// the request waited behind it), that switch is submitted again now: the rules'
    /// answer already changed, so no later reconcile would bring it back. If it had
    /// taken over a settle wait, the wait resumes for the time it had left (and the
    /// switch is submitted at once only if that time has passed).
    fn settle_manual(&mut self, kind: Endpoint, submitted: Option<&Submitted>) {
        let Some(Submitted {
            origin: Origin::Manual { restore_override },
            ..
        }) = submitted
        else {
            return;
        };
        let lane = self.lane_mut(kind);
        lane.manual_override = *restore_override;
        let Some(superseded) = lane.superseded.take() else {
            return;
        };
        let lane = self.lane(kind);
        if !lane.manual_override
            && !self.paused
            && lane.submitted.is_none()
            && lane.last_desired.as_ref() == Some(&superseded.candidates)
            && lane.applied.as_deref() != superseded.candidates.first().map(String::as_str)
        {
            if let Some(at) = superseded.settle_at.filter(|at| *at > Instant::now()) {
                debug!(
                    "Resuming the settle wait for {} the manual request replaced",
                    self.candidates_desc(kind, &superseded.candidates)
                );
                self.lane_mut(kind).settle = Some(Settle {
                    candidates: superseded.candidates,
                    at,
                    notice: superseded.notice,
                });
                return;
            }
            info!(
                "Resuming the switch to {} the manual request replaced",
                self.candidates_desc(kind, &superseded.candidates)
            );
            self.submit(kind, superseded.candidates, superseded.notice);
        }
    }

    fn schedule_retry(
        &mut self,
        kind: Endpoint,
        candidates: Vec<String>,
        notice: Option<RuleNotice>,
    ) {
        let lane = self.lane(kind);
        // Paused: `resume` switches again if the rules still want it
        if lane.manual_override || self.paused || lane.last_desired.as_ref() != Some(&candidates) {
            return;
        }
        let what = self.candidates_desc(kind, &candidates);
        let lane = self.lane_mut(kind);
        if let Some(delay) = RETRY_BACKOFF.get(lane.retries) {
            lane.retries += 1;
            warn!(
                "Retrying {what} in {}s (attempt {} of {})",
                delay.as_secs(),
                lane.retries + 1,
                RETRY_BACKOFF.len() + 1
            );
            lane.retry = Some(Retry {
                candidates,
                at: Instant::now() + *delay,
                notice,
            });
        } else {
            warn!("Giving up on {what} until the next window event or reload");
            lane.stalled = true;
        }
    }

    /// A retry is due: submit it if the rules still want its target (per lane)
    fn on_retry_due(&mut self) {
        let now = Instant::now();
        for kind in Endpoint::ALL {
            let lane = self.lane_mut(kind);
            if lane.retry.as_ref().is_none_or(|r| r.at > now) {
                continue;
            }
            let Some(retry) = lane.retry.take() else {
                continue;
            };
            let lane = self.lane(kind);
            if lane.manual_override
                || self.paused
                || lane.last_desired.as_ref() != Some(&retry.candidates)
                || self.serves(kind, &retry.candidates)
            {
                continue;
            }
            self.submit(kind, retry.candidates, retry.notice);
        }
    }

    /// Reply to every `IPC` request of lane `kind` settled by the job of `generation`
    ///
    /// Older requests were coalesced away (the worker only runs the latest job of a
    /// lane).
    fn answer(&mut self, kind: Endpoint, generation: u64, response: &Response) {
        let (settled, waiting): (Vec<_>, Vec<_>) = std::mem::take(&mut self.replies)
            .into_iter()
            .partition(|(k, g, _)| *k == kind && *g <= generation);
        self.replies = waiting;
        for (_, g, reply) in settled {
            let response = if g == generation {
                response.clone()
            } else {
                Response::Ok {
                    message: "Superseded by a newer switch request".to_string(),
                }
            };
            let _ = reply.send(response);
        }
    }

    /// A manual request: answered at once if the config alone decides it, otherwise
    /// handed to the worker, which decides against the real default
    fn on_manual(&mut self, request: ManualSwitch, reply: oneshot::Sender<Response>) {
        let config = Arc::clone(self.state.config());
        if let Some(answer) = crate::manual::answer_without_pipewire(&config, &request) {
            let _ = reply.send(match answer {
                Ok(message) => Response::Ok { message },
                Err(message) => Response::Error { message },
            });
            return;
        }

        info!("Manual switch requested: {request:?}");
        let kind = request.kind();
        let lane = self.lane_mut(kind);
        // Coalesced manual requests share the state from before the first of them
        let restore_override = match &lane.submitted {
            Some(Submitted {
                origin: Origin::Manual { restore_override },
                ..
            }) => *restore_override,
            _ => lane.manual_override,
        };
        lane.manual_override = true;
        // A rule job not yet settled is replaced (if queued) or may still fail (if
        // running), and a switch waiting for the settle delay is newer still: remember
        // it, so a manual request that switches nothing resumes it
        if let Some(settle) = lane.settle.take() {
            lane.superseded = Some(Superseded {
                candidates: settle.candidates,
                notice: settle.notice,
                settle_at: Some(settle.at),
            });
        } else if let Some(Submitted {
            origin: Origin::Rule | Origin::Upgrade,
            candidates: Some(candidates),
            notice,
            ..
        }) = &lane.submitted
        {
            lane.superseded = Some(Superseded {
                candidates: candidates.clone(),
                notice: notice.clone(),
                settle_at: None,
            });
        }
        // A pending rule retry becomes a stall: if this request switches nothing,
        // the rules' target gets its next try on the next event
        if lane.retry.take().is_some() {
            lane.stalled = true;
        }
        let generation = self.submit_job(
            kind,
            Target::Manual { request, config },
            None,
            Origin::Manual { restore_override },
            None,
        );
        self.replies.push((kind, generation, reply));
    }

    /// `Sink: X`, `Mic: Y`: one part of the status line
    ///
    /// Descriptions and node names are sanitised: the line goes to `sd_notify`, where a
    /// newline would start another `KEY=value` assignment.
    fn status_part(&self, kind: Endpoint, label: &str) -> String {
        use crate::sanitize::for_log;
        let lane = self.lane(kind);
        match lane.applied.as_deref() {
            Some(name) => match self.configured(kind, name) {
                Some(entry) if lane.applied_fallback => {
                    format!("{label}: {} (fallback)", for_log(&entry.desc))
                }
                Some(entry) => format!("{label}: {}", for_log(&entry.desc)),
                None => format!("{label}: {} (not in config)", for_log(name)),
            },
            None => format!("{label}: unknown"),
        }
    }

    /// The status line for what pwsw believes are the defaults (the microphone only
    /// when sources are configured)
    fn status_text(&self) -> String {
        let mut text = self.status_part(Endpoint::Sink, "Sink");
        if !self.state.config().sources.is_empty() {
            text.push_str(" · ");
            text.push_str(&self.status_part(Endpoint::Source, "Mic"));
        }
        if self.paused {
            text.push_str(" (paused)");
        }
        text
    }

    /// What the status stream reports for `kind` (`None` while its default is unknown)
    fn live_endpoint(&self, kind: Endpoint) -> Option<EndpointStatus> {
        let lane = self.lane(kind);
        let node = lane.applied.clone()?;
        // The deciding window, when the applied value is what its rule wants (not a
        // manual choice or a default found at startup that the rules differ from).
        // No title: it changes with every browser tab (a new stream line each time)
        // and would put page titles into status bars.
        let window = self
            .state
            .desired_for(kind)
            .filter(|d| !lane.manual_override && d.candidates.iter().any(|c| c.name == node))
            .and_then(|d| d.window)
            .map(|w| {
                let rule_index = w.rule_index + 1;
                let desc = self
                    .state
                    .config()
                    .rules
                    .get(w.rule_index)
                    .and_then(|r| r.desc.as_deref());
                RuleWindow {
                    app_id: crate::sanitize::for_log(w.app_id).into_owned(),
                    rule: desc.map_or_else(
                        || format!("#{rule_index}"),
                        |d| crate::sanitize::for_log(d).into_owned(),
                    ),
                    rule_index,
                }
            });
        Some(EndpointStatus {
            desc: self.endpoint_desc(kind, &node),
            fallback: lane.applied_fallback,
            node,
            window,
        })
    }

    fn live_status(&self) -> LiveStatus {
        let sources_configured = !self.state.config().sources.is_empty();
        LiveStatus {
            paused: self.paused,
            sink: self.live_endpoint(Endpoint::Sink),
            source: self
                .live_endpoint(Endpoint::Source)
                .filter(|_| sources_configured),
            sources_configured,
        }
    }

    /// Publish the status line (`STATUS=` for systemd) and the status stream, each
    /// only if it changed
    fn publish_status(&mut self) {
        let live = self.live_status();
        self.live.send_if_modified(|current| {
            let changed = *current != live;
            if changed {
                *current = live;
            }
            changed
        });
        let text = self.status_text();
        if self.status.as_deref() == Some(text.as_str()) {
            return;
        }
        self.sd_notify(&[sd_notify::NotifyState::Status(&text)]);
        if let Some(hook) = &self.hooks.status {
            hook(&text);
        }
        self.status = Some(text);
    }

    fn snapshot(&self) -> Snapshot {
        let desired = self.state.desired();
        let sink = self.lane(Endpoint::Sink);
        let source = self.lane(Endpoint::Source);
        // Name the candidate actually in use when it is the rules' fallback
        let deciding_sink = match (
            self.applied_fallback_rank(Endpoint::Sink),
            sink.applied.as_deref(),
        ) {
            (Some(_), Some(applied)) => format!("{applied} (fallback)"),
            _ => desired.first().name.clone(),
        };
        let config = self.state.config();
        let sources_configured = !config.sources.is_empty();
        let fallbacks = |entries: &[&SinkConfig]| -> Vec<FallbackSink> {
            entries
                .iter()
                .map(|s| FallbackSink {
                    name: s.name.clone(),
                    desc: s.desc.clone(),
                })
                .collect()
        };
        Snapshot {
            config: Arc::clone(config),
            applied: sink.applied.clone(),
            #[cfg(test)]
            desired: desired.first().name.clone(),
            fallback: sink.applied_fallback,
            active_window: desired
                .window
                .map(|w| format!("{}: {deciding_sink}", w.trigger)),
            tracked_windows: self.state.tracked_count(),
            windows: self
                .state
                .windows()
                .into_iter()
                .map(|w| WindowInfo {
                    id: Some(w.id),
                    app_id: w.app_id.to_string(),
                    title: w.title.to_string(),
                    outputs: w.outputs.to_vec(),
                    minimized: w.minimized,
                    matched_on: None,
                    tracked: w.sinks.split_first().map(|(first, rest)| TrackedInfo {
                        sink_name: first.name.clone(),
                        sink_desc: first.desc.clone(),
                        fallbacks: fallbacks(rest),
                        sources: fallbacks(&w.sources),
                    }),
                })
                .collect(),
            #[cfg(test)]
            manual_override: sink.manual_override,
            paused: self.paused,
            #[cfg(test)]
            settling: sink.settle.as_ref().map(|s| s.candidates.clone()),
            applied_source: source.applied.clone().filter(|_| sources_configured),
            source_fallback: sources_configured && source.applied_fallback,
            sources_configured,
            #[cfg(test)]
            desired_source: self
                .state
                .desired_for(Endpoint::Source)
                .map(|d| d.first().name.clone()),
            #[cfg(test)]
            source_manual_override: source.manual_override,
            #[cfg(test)]
            source_settling: source.settle.as_ref().map(|s| s.candidates.clone()),
            #[cfg(test)]
            jobs_submitted: self.next_generation,
            #[cfg(test)]
            sink_in_flight: sink.submitted.is_some(),
            #[cfg(test)]
            last_upgrade_check: sink.last_upgrade_check,
        }
    }

    /// Reload the config file; keeps the current config on any problem
    async fn reload(&mut self) {
        let mut reloading = vec![sd_notify::NotifyState::Reloading];
        if let Ok(now) = sd_notify::NotifyState::monotonic_usec_now() {
            reloading.push(now);
        }
        self.sd_notify(&reloading);

        let path = self.config_path.clone();
        let loaded = tokio::task::spawn_blocking(move || Config::load_from_path(&path)).await;
        let notify_manual = self.state.config().settings.notify_manual;
        match loaded {
            Err(e) => error!("Config reload task failed: {e}"),
            Ok(Err(e)) if e.downcast_ref::<ConfigNotFound>().is_some() => {
                info!("Config file missing (save in progress?); keeping current config");
            }
            Ok(Err(e)) => {
                error!("Failed to reload config: {e:#}");
                if notify_manual {
                    self.notify_daemon("Reload Failed", &format!("Config error: {e:#}"));
                }
            }
            Ok(Ok(config)) => {
                let applied = if config.sinks.is_empty() {
                    Err(anyhow::anyhow!("no sinks configured"))
                } else {
                    self.state.reconfigure(Arc::new(config))
                };
                if let Err(e) = applied {
                    warn!("Config reload rejected ({e:#}); keeping previous config");
                    if notify_manual {
                        self.notify_daemon(
                            "Reload Rejected",
                            &format!("{e:#} - keeping previous config"),
                        );
                    }
                } else {
                    let config = Arc::clone(self.state.config());
                    if let Some(set_level) = &self.hooks.set_log_level {
                        set_level(&config.settings.log_level);
                    }
                    let sources = if config.sources.is_empty() {
                        String::new()
                    } else {
                        format!(", {} sources", config.sources.len())
                    };
                    info!(
                        "Configuration reloaded: {} sinks{sources}, {} rules",
                        config.sinks.len(),
                        config.rules.len()
                    );
                    if let Some(warning) = self
                        .compositor
                        .and_then(|c| output_rules_warning(c, config.has_output_rules()))
                    {
                        warn!("{warning}");
                    }
                    if config.settings.notify_manual {
                        self.notify_daemon(
                            "Configuration Reloaded",
                            "New settings applied successfully",
                        );
                    }
                    self.reconcile(None);
                }
            }
        }

        if let Some(after_reload) = &mut self.hooks.after_reload {
            after_reload();
        }
        self.sd_notify(&[sd_notify::NotifyState::Ready]);
    }

    fn notify_daemon(&self, summary: &str, body: &str) {
        self.notifier.send(Notice {
            kind: NoticeKind::Daemon,
            summary: summary.to_string(),
            body: body.to_string(),
            icon: None,
        });
    }

    fn sd_notify(&self, states: &[sd_notify::NotifyState<'_>]) {
        if self.hooks.systemd
            && let Err(e) = sd_notify::notify(states)
        {
            debug!("sd_notify failed: {e}");
        }
    }
}

/// The compositor's window event channel closed (logged here)
fn compositor_lost() -> anyhow::Error {
    error!("Compositor connection lost (window event channel closed)");
    anyhow::anyhow!("Compositor connection lost: the window event channel closed")
}

async fn sleep_until(at: Option<Instant>) {
    match at {
        Some(at) => tokio::time::sleep_until(at).await,
        None => std::future::pending().await,
    }
}

#[cfg(test)]
mod tests;
