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
//! baseline; with `default_on_startup` it is activated, otherwise nothing happens
//! until it changes. A failed rule-driven activation is retried with backoff, then
//! once more on the next event or reload.
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

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use color_eyre::eyre::{self, Result};
use tokio::sync::{mpsc, oneshot, watch};
use tokio::time::Instant;
use tracing::{debug, error, info, warn};

use super::notifier::{Notice, NoticeKind, Notifier};
use super::worker::{self, Job, JobResult, Outcome, SinkSwitcher, Target};
use crate::compositor::WindowEvent;
use crate::config::{Config, ConfigNotFound};
use crate::ipc::{FallbackSink, Response, TrackedInfo, WindowInfo};
pub(crate) use crate::manual::ManualSwitch;
use crate::notification::{get_app_icon, get_sink_icon};
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

/// Notification title for sink switches
const SWITCH_TITLE: &str = "Audio Output";

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
    /// Switch sinks; the reply is sent once the activation finished
    Switch {
        request: ManualSwitch,
        reply: oneshot::Sender<Response>,
    },
    /// Shut down gracefully
    Shutdown,
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

/// A rule-driven job replaced by a manual request before its result came in
#[derive(Debug)]
struct Superseded {
    candidates: Vec<String>,
    notice: Option<RuleNotice>,
}

/// The daemon's event loop state
pub(crate) struct Daemon {
    state: State,
    config_path: PathBuf,
    jobs: watch::Sender<Option<Job>>,
    results: mpsc::Receiver<JobResult>,
    notifier: Notifier,
    hooks: Hooks,
    /// Sink pwsw last activated successfully (or found at startup)
    applied: Option<String>,
    /// `applied` is a lower-priority candidate a rule-driven switch settled for
    applied_fallback: bool,
    /// When the last upgrade check was submitted (see [`UPGRADE_CHECK_INTERVAL`])
    last_upgrade_check: Option<Instant>,
    /// The rules' candidates at the last reconcile; a change marks a rule-driven switch
    last_desired: Option<Vec<String>>,
    /// A manual switch holds until the desired sink changes
    manual_override: bool,
    /// The rules' sink failed and retries ran out: try once more on the next event
    stalled: bool,
    /// A rule-driven job that a manual request replaced (or that failed while the
    /// manual request was queued behind it): if the manual request switches nothing,
    /// it is submitted again at once, since no later change of the rules' answer
    /// would bring it back
    superseded: Option<Superseded>,
    /// Last status line published
    status: Option<String>,
    next_generation: u64,
    submitted: Option<Submitted>,
    retry: Option<Retry>,
    /// Retries scheduled for the current target so far
    retries: usize,
    /// `IPC` replies waiting for the job of that generation
    replies: Vec<(u64, oneshot::Sender<Response>)>,
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
        Self {
            state,
            config_path,
            jobs,
            results,
            notifier,
            hooks,
            applied,
            applied_fallback: false,
            last_upgrade_check: None,
            last_desired: None,
            manual_override: false,
            stalled: false,
            superseded: None,
            status: None,
            next_generation: 0,
            submitted: None,
            retry: None,
            retries: 0,
            replies: Vec::new(),
        }
    }

    /// Run until shutdown
    ///
    /// # Errors
    /// Returns an error if the compositor connection is lost (the window event
    /// channel closes), so the process exits non-zero and a supervisor restarts it.
    pub(crate) async fn run(mut self, mut inputs: Inputs) -> Result<()> {
        self.start();
        let mut reload_at: Option<Instant> = None;

        let result = loop {
            let retry_at = self.retry.as_ref().map(|r| r.at);
            tokio::select! {
                event = inputs.windows.recv() => {
                    let Some(event) = event else {
                        error!("Compositor connection lost (window event channel closed)");
                        break Err(eyre::eyre!(
                            "Compositor connection lost: the window event channel closed"
                        ));
                    };
                    let departure = self.state.apply(event);
                    self.reconcile(departure.as_ref());
                }
                Some(result) = self.results.recv() => self.on_result(result),
                Some(command) = inputs.commands.recv() => match command {
                    Command::Query(reply) => {
                        let _ = reply.send(self.snapshot());
                    }
                    Command::Switch { request, reply } => self.on_manual(request, reply),
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

    /// Startup: take the rules' answer as the baseline and, with
    /// `default_on_startup`, activate it (failures retry with backoff)
    fn start(&mut self) {
        let names = self.state.desired().names();
        self.last_desired = Some(names.clone());
        if self.state.config().settings.default_on_startup
            && self.applied.as_deref() != names.first().map(String::as_str)
        {
            info!(
                "Startup: switching to {} (what the rules want now)",
                self.candidates_desc(&names)
            );
            self.submit(names, None);
        }
        self.publish_status();
    }

    /// Whether the default sink is, or is about to be, what `candidates` asks for: a
    /// pending rule-driven job serves that list, or (nothing pending) `applied` is its
    /// first choice
    fn serves(&self, candidates: &[String]) -> bool {
        match &self.submitted {
            Some(Submitted {
                origin: Origin::Rule | Origin::Upgrade,
                candidates: Some(pending),
                ..
            }) => pending == candidates,
            // A manual job: its sink is not chosen yet
            Some(_) => false,
            None => self.applied.as_deref() == candidates.first().map(String::as_str),
        }
    }

    /// Candidates for the log: `Desc (name)`, or `first reachable of A, B`
    fn candidates_desc(&self, candidates: &[String]) -> String {
        match candidates {
            [single] => format!("{} ({single})", self.sink_desc(single)),
            _ => format!(
                "first reachable of {}",
                candidates
                    .iter()
                    .map(|c| self.sink_desc(c))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        }
    }

    /// Where the applied sink is in the rules' current candidates, when it is a
    /// fallback a rule-driven switch settled for
    fn applied_fallback_rank(&self) -> Option<usize> {
        if !self.applied_fallback {
            return None;
        }
        let applied = self.applied.as_deref()?;
        self.last_desired
            .as_ref()?
            .iter()
            .position(|c| c == applied)
            .filter(|&rank| rank > 0)
    }

    fn sink_desc(&self, name: &str) -> String {
        self.state
            .config()
            .sinks
            .iter()
            .find(|s| s.name == name)
            .map_or_else(|| name.to_string(), |s| s.desc.clone())
    }

    /// Switch when the rules changed their mind (or a stalled target gets another try,
    /// or an applied fallback may be upgraded)
    fn reconcile(&mut self, departure: Option<&Departure>) {
        let names = self.state.desired().names();
        if self.last_desired.as_deref() != Some(names.as_slice()) {
            debug!(
                "Desired sinks changed: [{}] → [{}]",
                self.last_desired
                    .as_deref()
                    .map_or_else(|| "(none)".to_string(), |d| d.join(", ")),
                names.join(", ")
            );
            self.last_desired = Some(names.clone());
            self.manual_override = false;
            self.retry = None;
            self.stalled = false;
            self.superseded = None;
            self.retries = 0;
            if !self.serves(&names) {
                let notice = self.rule_notice(departure);
                self.submit(names, notice);
                return;
            }
            if self.submitted.is_none() {
                // The applied sink is the new list's first choice
                self.applied_fallback = false;
            }
        } else if self.stalled
            && !self.manual_override
            && self.submitted.is_none()
            && self.retry.is_none()
        {
            info!("Trying {} again", self.candidates_desc(&names));
            self.stalled = false;
            self.retries = 0;
            let notice = self.rule_notice(departure);
            self.submit(names, notice);
            return;
        } else if let Some(rank) = self.upgrade_due() {
            debug!("Applied sink is fallback #{rank}; checking for a higher-priority one");
            self.last_upgrade_check = Some(Instant::now());
            let notice = self.rule_notice(departure);
            self.submit_job(
                Target::Rule {
                    candidates: names.clone(),
                    upgrade_from: Some(rank),
                },
                Some(names),
                Origin::Upgrade,
                notice,
            );
            return;
        }
        // Nothing to switch: keep the status line current (e.g. a reload renamed it)
        if self.submitted.is_none() && self.applied.is_some() {
            self.publish_status();
        }
    }

    /// The applied fallback's rank, when it is time to look for a better candidate:
    /// nothing pending or held, and the last check is [`UPGRADE_CHECK_INTERVAL`] ago
    fn upgrade_due(&self) -> Option<usize> {
        if self.manual_override
            || self.stalled
            || self.submitted.is_some()
            || self.retry.is_some()
            || self
                .last_upgrade_check
                .is_some_and(|at| at.elapsed() < UPGRADE_CHECK_INTERVAL)
        {
            return None;
        }
        self.applied_fallback_rank()
    }

    /// The notification for a rule-driven switch to the desired sinks, if wanted
    ///
    /// The flag and the reason come from the window that decides the target; when
    /// falling back to the default, from the window that departed.
    fn rule_notice(&self, departure: Option<&Departure>) -> Option<RuleNotice> {
        if let Some(window) = self.state.desired().window {
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

    /// Complete a rule-driven switch's notification for the sink it activated
    fn complete_notice(&self, notice: &RuleNotice, sink: &str, fallback: bool) -> Notice {
        let config = self.state.config();
        let configured = config.sinks.iter().find(|s| s.name == sink);
        let mut desc = configured.map_or_else(|| sink.to_string(), |s| s.desc.clone());
        if fallback {
            desc.push_str(" (fallback)");
        }
        Notice {
            kind: NoticeKind::Switch,
            summary: SWITCH_TITLE.to_string(),
            body: match &notice.reason {
                Some(reason) => format!("{desc} → {reason}"),
                None => desc,
            },
            icon: notice
                .app_icon
                .clone()
                .or_else(|| configured.map(get_sink_icon)),
        }
    }

    /// The notification for a manual switch to `sink`, if wanted
    fn manual_notice(&self, sink: &str) -> Option<Notice> {
        let config = self.state.config();
        if !config.settings.notify_manual {
            return None;
        }
        let sink = config.sinks.iter().find(|s| s.name == sink)?;
        Some(Notice {
            kind: NoticeKind::Switch,
            summary: SWITCH_TITLE.to_string(),
            body: sink.desc.clone(),
            icon: Some(get_sink_icon(sink)),
        })
    }

    /// Submit a rule-driven activation of the first reachable of `candidates`
    fn submit(&mut self, candidates: Vec<String>, notice: Option<RuleNotice>) -> u64 {
        info!("Switching: {}", self.candidates_desc(&candidates));
        self.submit_job(
            Target::Rule {
                candidates: candidates.clone(),
                upgrade_from: None,
            },
            Some(candidates),
            Origin::Rule,
            notice,
        )
    }

    /// Hand a job to the worker (replacing any not yet started) and return its generation
    fn submit_job(
        &mut self,
        target: Target,
        candidates: Option<Vec<String>>,
        origin: Origin,
        notice: Option<RuleNotice>,
    ) -> u64 {
        self.next_generation += 1;
        let generation = self.next_generation;
        self.jobs.send_replace(Some(Job { generation, target }));
        self.submitted = Some(Submitted {
            generation,
            candidates,
            origin,
            notice,
        });
        generation
    }

    fn on_result(&mut self, result: JobResult) {
        let is_latest = self
            .submitted
            .as_ref()
            .is_some_and(|s| s.generation == result.generation);
        let submitted = if is_latest {
            self.submitted.take()
        } else {
            None
        };

        // A manual request looked at the real default first: that is the truth now
        if let Some(observed) = result.observed
            && self.applied.as_deref() != Some(observed.as_str())
        {
            debug!(
                "Default sink is {observed} (last known: {})",
                self.applied.as_deref().unwrap_or("unknown")
            );
            self.applied = Some(observed);
            self.applied_fallback = false;
        }

        let tried = result.tried;
        let response = match result.outcome {
            Outcome::Activated { index, node_name } if index < tried.len() => {
                let sink = tried[index].clone();
                let fallback = index > 0;
                let mut desc = self.sink_desc(&sink);
                info!(
                    "Audio output is now {desc}{} ({node_name})",
                    if fallback { " (fallback)" } else { "" }
                );
                let notice = match &submitted {
                    Some(Submitted {
                        origin: origin @ (Origin::Rule | Origin::Upgrade),
                        notice,
                        ..
                    }) => {
                        self.stalled = false;
                        // An upgrade whose fallback chain ended on the same sink
                        // changed nothing audible
                        let changed = *origin == Origin::Rule
                            || self.applied.as_deref() != Some(sink.as_str());
                        notice
                            .as_ref()
                            .filter(|_| changed)
                            .map(|n| self.complete_notice(n, &sink, fallback))
                    }
                    Some(Submitted {
                        origin: Origin::Manual { .. },
                        ..
                    }) => {
                        // The user's choice holds now, until the rules' answer changes
                        self.superseded = None;
                        self.manual_notice(&sink)
                    }
                    None => None,
                };
                if self
                    .superseded
                    .as_ref()
                    .is_some_and(|s| s.candidates == tried)
                {
                    // The superseded rule job had already started, and it succeeded
                    self.superseded = None;
                }
                if let Some(notice) = notice {
                    self.notifier.send(notice);
                }
                self.applied = Some(sink);
                self.applied_fallback = fallback;
                if fallback {
                    desc.push_str(" (fallback)");
                }
                Response::SinkSwitched { desc, node_name }
            }
            Outcome::Activated { node_name, .. } => {
                // Not produced by the worker; keep the loop consistent anyway
                warn!("Activation result without its sink (node {node_name})");
                Response::Error {
                    message: "Internal error: activation result without its sink".to_string(),
                }
            }
            Outcome::Unchanged => {
                let message = format!(
                    "No higher-priority sink reachable; staying on {}",
                    self.applied
                        .as_deref()
                        .map_or_else(|| "the current sink".to_string(), |a| self.sink_desc(a))
                );
                debug!("{message}");
                Response::Ok { message }
            }
            Outcome::Done(message) => {
                info!("{message}");
                self.settle_manual(submitted.as_ref());
                Response::Ok { message }
            }
            Outcome::Failed(message) => {
                let message = match tried.as_slice() {
                    [] => message,
                    [sink] => {
                        format!(
                            "Failed to activate sink '{}': {message}",
                            self.sink_desc(sink)
                        )
                    }
                    several => format!(
                        "Failed to activate any of {}: {message}",
                        several
                            .iter()
                            .map(|s| format!("'{}'", self.sink_desc(s)))
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
                    }) => self.schedule_retry(candidates, notice),
                    manual @ Some(Submitted {
                        origin: Origin::Manual { .. },
                        ..
                    }) => self.settle_manual(manual.as_ref()),
                    _ => {}
                }
                Response::Error { message }
            }
        };
        self.publish_status();
        self.answer(result.generation, &response);
    }

    /// A manual request that switched nothing: audio stays where it was, and so does
    /// the override state from before the request
    ///
    /// If the request had replaced a rule-driven switch (or that switch failed while
    /// the request waited behind it), that switch is submitted again now: the rules'
    /// answer already changed, so no later reconcile would bring it back.
    fn settle_manual(&mut self, submitted: Option<&Submitted>) {
        let Some(Submitted {
            origin: Origin::Manual { restore_override },
            ..
        }) = submitted
        else {
            return;
        };
        self.manual_override = *restore_override;
        let Some(superseded) = self.superseded.take() else {
            return;
        };
        if !self.manual_override
            && self.submitted.is_none()
            && self.last_desired.as_ref() == Some(&superseded.candidates)
            && self.applied.as_deref() != superseded.candidates.first().map(String::as_str)
        {
            info!(
                "Resuming the switch to {} the manual request replaced",
                self.candidates_desc(&superseded.candidates)
            );
            self.submit(superseded.candidates, superseded.notice);
        }
    }

    fn schedule_retry(&mut self, candidates: Vec<String>, notice: Option<RuleNotice>) {
        if self.manual_override || self.last_desired.as_ref() != Some(&candidates) {
            return;
        }
        let what = self.candidates_desc(&candidates);
        if let Some(delay) = RETRY_BACKOFF.get(self.retries) {
            self.retries += 1;
            warn!(
                "Retrying {what} in {}s (attempt {} of {})",
                delay.as_secs(),
                self.retries + 1,
                RETRY_BACKOFF.len() + 1
            );
            self.retry = Some(Retry {
                candidates,
                at: Instant::now() + *delay,
                notice,
            });
        } else {
            warn!("Giving up on {what} until the next window event or reload");
            self.stalled = true;
        }
    }

    fn on_retry_due(&mut self) {
        let Some(retry) = self.retry.take() else {
            return;
        };
        if self.manual_override
            || self.last_desired.as_ref() != Some(&retry.candidates)
            || self.serves(&retry.candidates)
        {
            return;
        }
        self.submit(retry.candidates, retry.notice);
    }

    /// Reply to every `IPC` request settled by the job of `generation`
    ///
    /// Older requests were coalesced away (the worker only runs the latest job).
    fn answer(&mut self, generation: u64, response: &Response) {
        let (settled, waiting): (Vec<_>, Vec<_>) = std::mem::take(&mut self.replies)
            .into_iter()
            .partition(|(g, _)| *g <= generation);
        self.replies = waiting;
        for (g, reply) in settled {
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
    /// handed to the worker, which decides against the real default sink
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
        // Coalesced manual requests share the state from before the first of them
        let restore_override = match &self.submitted {
            Some(Submitted {
                origin: Origin::Manual { restore_override },
                ..
            }) => *restore_override,
            _ => self.manual_override,
        };
        self.manual_override = true;
        // A rule job not yet settled is replaced (if queued) or may still fail (if
        // running): remember it, so a manual request that switches nothing resumes it
        if let Some(Submitted {
            origin: Origin::Rule | Origin::Upgrade,
            candidates: Some(candidates),
            notice,
            ..
        }) = &self.submitted
        {
            self.superseded = Some(Superseded {
                candidates: candidates.clone(),
                notice: notice.clone(),
            });
        }
        // A pending rule retry becomes a stall: if this request switches nothing,
        // the rules' sink gets its next try on the next event
        if self.retry.take().is_some() {
            self.stalled = true;
        }
        let generation = self.submit_job(
            Target::Manual { request, config },
            None,
            Origin::Manual { restore_override },
            None,
        );
        self.replies.push((generation, reply));
    }

    /// The status line for the sink pwsw believes is the default
    fn status_text(&self) -> String {
        match self.applied.as_deref() {
            Some(name) => match self.state.config().sinks.iter().find(|s| s.name == name) {
                Some(sink) if self.applied_fallback => format!("Sink: {} (fallback)", sink.desc),
                Some(sink) => format!("Sink: {}", sink.desc),
                None => format!("Sink: {name} (not in config)"),
            },
            None => "Sink: unknown".to_string(),
        }
    }

    /// Publish the status line (`STATUS=` for systemd) if it changed
    fn publish_status(&mut self) {
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
        // Name the candidate actually in use when it is the rules' fallback
        let deciding_sink = match (self.applied_fallback_rank(), self.applied.as_deref()) {
            (Some(_), Some(applied)) => format!("{applied} (fallback)"),
            _ => desired.first().name.clone(),
        };
        Snapshot {
            config: Arc::clone(self.state.config()),
            applied: self.applied.clone(),
            #[cfg(test)]
            desired: desired.first().name.clone(),
            fallback: self.applied_fallback,
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
                    matched_on: None,
                    tracked: w.sinks.split_first().map(|(first, rest)| TrackedInfo {
                        sink_name: first.name.clone(),
                        sink_desc: first.desc.clone(),
                        fallbacks: rest
                            .iter()
                            .map(|s| FallbackSink {
                                name: s.name.clone(),
                                desc: s.desc.clone(),
                            })
                            .collect(),
                    }),
                })
                .collect(),
            #[cfg(test)]
            manual_override: self.manual_override,
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
                    Err(eyre::eyre!("no sinks configured"))
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
                    info!(
                        "Configuration reloaded: {} sinks, {} rules",
                        config.sinks.len(),
                        config.rules.len()
                    );
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

async fn sleep_until(at: Option<Instant>) {
    match at {
        Some(at) => tokio::time::sleep_until(at).await,
        None => std::future::pending().await,
    }
}

#[cfg(test)]
mod tests;
