//! The daemon's event loop
//!
//! Inputs (window events, `IPC` commands, config-file changes, signals) update the
//! [`State`] reducer; after each one the loop *reconciles*: if the sink the rules want
//! differs from what pwsw last applied, it submits an activation to the worker and
//! moves on. Results come back as just another input, so the loop never waits on
//! `PipeWire` and keeps answering `IPC`, signals and window events during a slow
//! profile switch.
//!
//! `applied` is pwsw's own view of the default sink: what it last activated (or found
//! at startup). Changes made outside pwsw (e.g. `pavucontrol`) are not tracked; the
//! next rule-driven change, or a manual `set-sink`, applies again.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use color_eyre::eyre::{self, Result};
use tokio::sync::{mpsc, oneshot, watch};
use tokio::time::Instant;
use tracing::{debug, error, info, warn};

use super::notifier::{Notice, NoticeKind, Notifier};
use super::worker::{self, Job, JobResult, SinkSwitcher};
use crate::compositor::WindowEvent;
use crate::config::{Config, ConfigNotFound, SinkConfig};
use crate::ipc::{Response, TrackedInfo, WindowInfo};
use crate::notification::{get_app_icon, get_sink_icon};
use crate::state::{Departure, State};

/// Quiet period after the last config-file event before reloading (trailing edge)
pub(crate) const RELOAD_DEBOUNCE: Duration = Duration::from_millis(300);
/// Delays before retrying a failed rule-driven activation; then give up until the
/// next window event or reload
pub(crate) const RETRY_BACKOFF: [Duration; 2] = [Duration::from_secs(2), Duration::from_secs(5)];

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

/// A manual switch request (`set-sink`, `next-sink`, `prev-sink`)
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ManualSwitch {
    Set(String),
    Next,
    Prev,
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
    /// Sink the rules want right now
    pub desired: String,
    /// "trigger: sink" of the window deciding the desired sink
    pub active_window: Option<String>,
    /// Windows matching a rule
    pub tracked_windows: usize,
    /// All open windows (sorted by id), with tracking info for matching ones
    pub windows: Vec<WindowInfo>,
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

/// Side effects that only make sense in a real daemon
#[derive(Default)]
pub(crate) struct Hooks {
    /// Send `sd_notify` messages (no-op without `NOTIFY_SOCKET`)
    pub systemd: bool,
    /// Re-apply the config's `log_level` after a reload
    pub set_log_level: Option<LevelSetter>,
}

/// Who asked for an activation
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Origin {
    Rule,
    Manual,
}

/// The latest job handed to the worker, until its result arrives
#[derive(Debug)]
struct Submitted {
    generation: u64,
    sink: String,
    origin: Origin,
    notice: Option<Notice>,
}

/// A scheduled retry of a failed rule-driven activation
#[derive(Debug)]
struct Retry {
    sink: String,
    at: Instant,
    notice: Option<Notice>,
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
    /// Desired sink at the last reconcile; a change marks a rule-driven switch
    last_desired: Option<String>,
    /// A manual switch holds until the desired sink changes
    manual_override: bool,
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
            last_desired: None,
            manual_override: false,
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
                    Command::Switch { request, reply } => self.on_manual(&request, reply),
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

    /// Startup: optionally switch to the default sink (failures retry with backoff)
    fn start(&mut self) {
        let desired = self.state.desired_sink();
        let (name, desc) = (desired.name.clone(), desired.desc.clone());
        self.last_desired = Some(name.clone());
        if self.state.config().settings.default_on_startup && self.applied.as_deref() != Some(&name)
        {
            info!("Switching to default sink: {desc}");
            self.submit(name, Origin::Rule, None);
        }
    }

    /// What the default sink is (or is about to be): the pending job, else `applied`
    fn target(&self) -> Option<&str> {
        self.submitted
            .as_ref()
            .map(|s| s.sink.as_str())
            .or(self.applied.as_deref())
    }

    fn sink_desc(&self, name: &str) -> String {
        self.state
            .config()
            .sinks
            .iter()
            .find(|s| s.name == name)
            .map_or_else(|| name.to_string(), |s| s.desc.clone())
    }

    /// Bring the default sink in line with the rules, if they changed their mind
    fn reconcile(&mut self, departure: Option<&Departure>) {
        let name = self.state.desired_sink().name.clone();
        if self.last_desired.as_deref() != Some(name.as_str()) {
            debug!(
                "Desired sink changed: {} → {name}",
                self.last_desired.as_deref().unwrap_or("(none)")
            );
            self.last_desired = Some(name.clone());
            self.manual_override = false;
            self.retry = None;
        }
        if self.manual_override || self.retry.is_some() || self.target() == Some(name.as_str()) {
            return;
        }
        let notice = self.rule_notice(departure);
        self.retries = 0;
        self.submit(name, Origin::Rule, notice);
    }

    /// The notification for a rule-driven switch to the desired sink, if wanted
    ///
    /// Content and the notify flag come from the window that decides the target; when
    /// falling back to the default, from the window that departed.
    fn rule_notice(&self, departure: Option<&Departure>) -> Option<Notice> {
        let desired = self.state.desired();
        if let Some(window) = desired.window {
            return window.notify.then(|| Notice {
                kind: NoticeKind::Switch,
                summary: SWITCH_TITLE.to_string(),
                body: format!("{} → {}", desired.sink.desc, window.trigger),
                icon: Some(get_app_icon(window.app_id)),
            });
        }
        let notify = departure.map_or(self.state.config().settings.notify_rules, |d| d.notify);
        notify.then(|| Notice {
            kind: NoticeKind::Switch,
            summary: SWITCH_TITLE.to_string(),
            body: departure.map_or_else(
                || desired.sink.desc.clone(),
                |d| {
                    let what = if d.closed { "closed" } else { "ended" };
                    format!("{} → {} {what}", desired.sink.desc, d.trigger)
                },
            ),
            icon: Some(get_sink_icon(desired.sink)),
        })
    }

    /// Hand a job to the worker (replacing any not yet started) and return its generation
    fn submit(&mut self, sink: String, origin: Origin, notice: Option<Notice>) -> u64 {
        self.next_generation += 1;
        let generation = self.next_generation;
        info!("Switching: {} ({sink})", self.sink_desc(&sink));
        self.jobs.send_replace(Some(Job {
            generation,
            sink: sink.clone(),
        }));
        self.submitted = Some(Submitted {
            generation,
            sink,
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
        let desc = self.sink_desc(&result.sink);

        let response = match result.outcome {
            Ok(node_name) => {
                info!("Audio output is now {desc} ({node_name})");
                self.applied = Some(result.sink);
                if let Some(notice) = submitted.and_then(|s| s.notice) {
                    self.notifier.send(notice);
                }
                self.sd_notify(&[sd_notify::NotifyState::Status(&format!("Sink: {desc}"))]);
                Response::SinkSwitched { desc, node_name }
            }
            Err(message) => {
                error!("Failed to switch to {desc}: {message}");
                let mut manual_failed = false;
                if let Some(submitted) = submitted {
                    match submitted.origin {
                        Origin::Rule => self.schedule_retry(submitted),
                        Origin::Manual => manual_failed = true,
                    }
                }
                let response = Response::Error {
                    message: format!("Failed to activate sink '{desc}': {message}"),
                };
                if manual_failed {
                    // The override never took effect: let the rules decide again
                    self.answer(result.generation, &response);
                    self.manual_override = false;
                    self.reconcile(None);
                    return;
                }
                response
            }
        };
        self.answer(result.generation, &response);
    }

    fn schedule_retry(&mut self, failed: Submitted) {
        if self.manual_override || self.last_desired.as_deref() != Some(failed.sink.as_str()) {
            return;
        }
        if let Some(delay) = RETRY_BACKOFF.get(self.retries) {
            self.retries += 1;
            warn!(
                "Retrying {} in {}s (attempt {} of {})",
                failed.sink,
                delay.as_secs(),
                self.retries + 1,
                RETRY_BACKOFF.len() + 1
            );
            self.retry = Some(Retry {
                sink: failed.sink,
                at: Instant::now() + *delay,
                notice: failed.notice,
            });
        } else {
            warn!(
                "Giving up on {} until the next window event or reload",
                failed.sink
            );
        }
    }

    fn on_retry_due(&mut self) {
        let Some(retry) = self.retry.take() else {
            return;
        };
        if self.manual_override
            || self.last_desired.as_deref() != Some(retry.sink.as_str())
            || self.target() == Some(retry.sink.as_str())
        {
            return;
        }
        self.submit(retry.sink, Origin::Rule, retry.notice);
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

    fn on_manual(&mut self, request: &ManualSwitch, reply: oneshot::Sender<Response>) {
        let config = Arc::clone(self.state.config());
        let default = self.state.default_sink().clone();
        let current = self.target().map(str::to_owned);
        let target = match resolve_manual(&config, &default, request, current.as_deref()) {
            Ok(ManualTarget::Switch(sink)) => sink,
            Ok(ManualTarget::Done(message)) => {
                let _ = reply.send(Response::Ok { message });
                return;
            }
            Err(message) => {
                let _ = reply.send(Response::Error { message });
                return;
            }
        };

        info!("Manual switch requested: {}", target.desc);
        self.manual_override = true;
        self.retry = None;
        let notice = config.settings.notify_manual.then(|| Notice {
            kind: NoticeKind::Switch,
            summary: SWITCH_TITLE.to_string(),
            body: target.desc.clone(),
            icon: Some(get_sink_icon(target)),
        });
        let generation = self.submit(target.name.clone(), Origin::Manual, notice);
        self.replies.push((generation, reply));
    }

    fn snapshot(&self) -> Snapshot {
        let desired = self.state.desired();
        Snapshot {
            config: Arc::clone(self.state.config()),
            applied: self.applied.clone(),
            desired: desired.sink.name.clone(),
            active_window: desired
                .window
                .map(|w| format!("{}: {}", w.trigger, desired.sink.name)),
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
                    tracked: w.sink.map(|s| TrackedInfo {
                        sink_name: s.name.clone(),
                        sink_desc: s.desc.clone(),
                    }),
                })
                .collect(),
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

/// Where a manual request leads
#[derive(Debug, PartialEq, Eq)]
enum ManualTarget<'a> {
    /// Switch to this sink
    Switch(&'a SinkConfig),
    /// Nothing to do; reply with this message
    Done(String),
}

/// Resolve a manual request against the config and the current sink (pure)
fn resolve_manual<'a>(
    config: &'a Config,
    default: &'a SinkConfig,
    request: &ManualSwitch,
    current: Option<&str>,
) -> std::result::Result<ManualTarget<'a>, String> {
    match request {
        ManualSwitch::Set(sink_ref) => {
            let Some(target) = config.resolve_sink(sink_ref) else {
                let available: Vec<String> = config
                    .sinks
                    .iter()
                    .enumerate()
                    .map(|(i, s)| format!("{}. '{}'", i + 1, s.desc))
                    .collect();
                return Err(format!(
                    "Unknown sink '{sink_ref}'. Available: [{}]",
                    available.join(", ")
                ));
            };
            if config.settings.set_smart_toggle && current == Some(target.name.as_str()) {
                if target.name == default.name {
                    return Ok(ManualTarget::Done(format!("Already on: {}", default.desc)));
                }
                return Ok(ManualTarget::Switch(default));
            }
            Ok(ManualTarget::Switch(target))
        }
        ManualSwitch::Next | ManualSwitch::Prev => {
            let count = config.sinks.len();
            if count < 2 {
                return Ok(ManualTarget::Done(
                    "Only one sink configured, nothing to cycle".to_string(),
                ));
            }
            let index = current
                .and_then(|name| config.sinks.iter().position(|s| s.name == name))
                .or_else(|| config.sinks.iter().position(|s| s.name == default.name))
                .unwrap_or(0);
            let next = if *request == ManualSwitch::Next {
                (index + 1) % count
            } else {
                (index + count - 1) % count
            };
            Ok(ManualTarget::Switch(&config.sinks[next]))
        }
    }
}

#[cfg(test)]
mod tests;
