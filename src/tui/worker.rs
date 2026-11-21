//! Background worker: polls daemon and `PipeWire` state and runs slow commands.
//!
//! The worker is a `select!` loop over the command channel, the poll intervals
//! and its own task set. Everything slow (`IPC` polls, `pw-dump`, daemon actions,
//! config saves) runs as a separate task that sends its result to the UI, so a
//! command starts the moment it arrives instead of waiting behind a poll.
//!
//! - Daemon status and the window list: every [`STATUS_POLL`].
//! - `PipeWire` snapshot: every [`SINKS_POLL`], only while a screen showing sinks
//!   is visible (and at once when one becomes visible).
//! - Polls of one kind never overlap; daemon actions run one at a time in order;
//!   saves run one at a time and a newer queued save replaces an older one.
//! - A pause/resume request runs at once; no status poll starts while it runs and
//!   the result of one that was running when it started is dropped, so the pause
//!   state the UI sees never goes back to what it was before the request.
//! - A panic in any task is re-raised in the worker, whose end the event loop
//!   observes.

use std::collections::VecDeque;
use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinSet;
use tokio::time::{MissedTickBehavior, interval};
use tracing::debug;

use super::app::{AppUpdate, BgCommand, DaemonAction, SaveOutcome, load_blocking, save_blocking};
use super::log_tailer::LogTailer;
use crate::config::Config;
use crate::daemon_manager::DaemonManager;

/// Interval between daemon status / window list polls.
pub(crate) const STATUS_POLL: Duration = Duration::from_secs(1);
/// Interval between `PipeWire` snapshots while sinks are on screen.
pub(crate) const SINKS_POLL: Duration = Duration::from_secs(2);

/// Run blocking work on a dedicated thread and await its result.
///
/// Unlike `spawn_blocking`, the thread is not part of tokio's blocking pool, so
/// shutting the runtime down (quitting the `TUI`) never waits for a slow child
/// process. A panic in `f` is re-raised in the awaiting task.
///
/// # Errors
/// Returns an error if the thread cannot be spawned.
pub(crate) async fn detached<T, F>(f: F) -> Result<T>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    let (tx, rx) = oneshot::channel();
    std::thread::Builder::new()
        .name("pwsw-tui-io".to_string())
        .spawn(move || {
            let _ = tx.send(std::panic::catch_unwind(AssertUnwindSafe(f)));
        })
        .map_err(|e| anyhow::anyhow!("Failed to spawn worker thread: {e}"))?;
    match rx.await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(panic)) => std::panic::resume_unwind(panic),
        Err(_) => anyhow::bail!("worker thread ended without a result"),
    }
}

/// The slow operations the worker performs (a trait so tests can fake them).
pub(crate) trait Backend: Send + Sync + 'static {
    /// Poll daemon status and windows; yields [`AppUpdate::DaemonState`].
    fn daemon_state(&self) -> impl Future<Output = AppUpdate> + Send;
    /// Take a `PipeWire` snapshot (sinks and sources); yields [`AppUpdate::SinksData`] or
    /// [`AppUpdate::PipeWireUnavailable`].
    fn sinks_snapshot(&self) -> impl Future<Output = AppUpdate> + Send;
    /// Run a daemon control action, returning a message for the status line.
    fn daemon_action(&self, action: DaemonAction) -> impl Future<Output = Result<String>> + Send;
    /// Toggle the daemon's pause, returning whether rule-driven switching is
    /// paused now.
    fn toggle_pause(&self) -> impl Future<Output = Result<bool>> + Send;
    /// Save the config atomically to `path` (`force` overwrites external changes).
    fn save_config(
        &self,
        config: Config,
        path: PathBuf,
        force: bool,
    ) -> impl Future<Output = SaveOutcome> + Send;
    /// Load the config file at `path`.
    fn load_config(
        &self,
        path: PathBuf,
    ) -> impl Future<Output = Result<Box<Config>, String>> + Send;
}

/// The real backend: `IPC`, `pw-dump`, `systemctl`, the config file.
pub(crate) struct System;

impl Backend for System {
    async fn daemon_state(&self) -> AppUpdate {
        use crate::ipc::{Request, Response, send_request};

        let daemon_manager = match send_request(Request::GetManagerInfo).await {
            Ok(Response::ManagerInfo { daemon_manager }) => Some(daemon_manager),
            _ => None, // Daemon not running or query failed
        };
        let running = daemon_manager.is_some();
        let unit_installed = DaemonManager::unit_installed_cached().await;
        let service_enabled = if unit_installed {
            Some(DaemonManager::Systemd.is_enabled().await)
        } else {
            None
        };
        let (windows, paused, monitor_info) = if running {
            let windows = match send_request(Request::ListWindows).await {
                Ok(Response::Windows { windows }) => windows,
                _ => Vec::new(),
            };
            let (paused, monitor_info) = match send_request(Request::Status).await {
                Ok(Response::Status {
                    paused,
                    monitor_info,
                    ..
                }) => (paused, Some(monitor_info)),
                // Unknown: the rule form must not claim the daemon is on ext
                _ => (false, None),
            };
            (windows, paused, monitor_info)
        } else {
            (Vec::new(), false, None)
        };
        AppUpdate::DaemonState {
            running,
            windows,
            unit_installed,
            service_enabled,
            paused,
            monitor_info,
        }
    }

    async fn sinks_snapshot(&self) -> AppUpdate {
        use crate::pipewire::PipeWire;

        match detached(PipeWire::dump).await {
            Ok(Ok(snapshot)) => {
                // Sinks and sources from the same snapshot (one `pw-dump`)
                let active = PipeWire::get_active_sinks(&snapshot);
                let profiles = PipeWire::get_profile_sinks(&snapshot, &active);
                let sources = PipeWire::get_active(&snapshot, crate::config::Endpoint::Source);
                let profile_sources = PipeWire::get_profile_sources(&snapshot, &sources);
                AppUpdate::SinksData {
                    active,
                    profiles,
                    sources,
                    profile_sources,
                }
            }
            Ok(Err(e)) | Err(e) => {
                debug!("PipeWire snapshot failed: {e:#}");
                AppUpdate::PipeWireUnavailable
            }
        }
    }

    async fn daemon_action(&self, action: DaemonAction) -> Result<String> {
        // start/stop/restart follow how the running daemon was started (reported
        // over IPC; the installed unit if none runs), enable/disable act on the
        // installed unit.
        let dm = match action {
            DaemonAction::Enable | DaemonAction::Disable => {
                DaemonManager::for_unit_management().await
            }
            _ => DaemonManager::for_control().await,
        };
        match action {
            DaemonAction::Start => dm.start().await,
            DaemonAction::Stop => dm.stop().await,
            DaemonAction::Restart => dm.restart().await,
            DaemonAction::Enable => dm.enable().await,
            DaemonAction::Disable => dm.disable().await,
        }
    }

    async fn toggle_pause(&self) -> Result<bool> {
        use crate::ipc::{Request, Response, send_request};

        match send_request(Request::TogglePause).await? {
            Response::PauseState { paused, .. } => Ok(paused),
            Response::Error { message } => anyhow::bail!("{message}"),
            _ => anyhow::bail!("unexpected response from the daemon"),
        }
    }

    async fn save_config(&self, config: Config, path: PathBuf, force: bool) -> SaveOutcome {
        detached(move || save_blocking(&config, &path, force))
            .await
            .unwrap_or_else(|e| SaveOutcome::Failed(format!("{e:#}")))
    }

    async fn load_config(&self, path: PathBuf) -> Result<Box<Config>, String> {
        detached(move || load_blocking(&path))
            .await
            .unwrap_or_else(|e| Err(format!("{e:#}")))
    }
}

/// Which task finished.
enum Done {
    /// A status poll, with its result and the pause generation it started in
    Status(Box<AppUpdate>, u64),
    Sinks,
    Action,
    Pause,
    Save,
    Reload,
}

/// Run the worker until the command channel closes (the UI is gone).
///
/// `sinks_visible` says whether a screen showing `PipeWire` sinks is on screen.
pub(crate) async fn run<B: Backend>(
    backend: Arc<B>,
    mut commands: mpsc::Receiver<BgCommand>,
    updates: mpsc::UnboundedSender<AppUpdate>,
    mut sinks_visible: watch::Receiver<bool>,
    mut logs: Option<LogTailer>,
) {
    let mut status_tick = interval(STATUS_POLL);
    status_tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut sinks_tick = interval(SINKS_POLL);
    sinks_tick.set_missed_tick_behavior(MissedTickBehavior::Delay);

    let mut tasks: JoinSet<Done> = JoinSet::new();
    let mut status_busy = false;
    let mut sinks_busy = false;
    let mut action_busy = false;
    let mut save_busy = false;
    let mut actions: VecDeque<DaemonAction> = VecDeque::new();
    let mut next_save: Option<(Box<Config>, PathBuf, bool)> = None;
    let mut next_reload: Option<PathBuf> = None;
    let mut reload_busy = false;
    let mut pause_queued = false;
    let mut pause_busy = false;
    // Bumped when a pause request starts: a status poll begun before then may
    // report the old pause state and is dropped
    let mut pause_generation: u64 = 0;
    let mut visible = *sinks_visible.borrow_and_update();

    loop {
        tokio::select! {
            cmd = commands.recv() => match cmd {
                Some(BgCommand::DaemonAction(action)) => actions.push_back(action),
                Some(BgCommand::SaveConfig { config, path, force }) => {
                    next_save = Some((config, path, force));
                }
                Some(BgCommand::ReloadConfig(path)) => next_reload = Some(path),
                Some(BgCommand::TogglePause) => pause_queued = true,
                None => break,
            },
            Some(done) = tasks.join_next() => match done {
                Ok(Done::Status(update, generation)) => {
                    status_busy = false;
                    if generation == pause_generation {
                        let _ = updates.send(*update);
                    }
                }
                Ok(Done::Pause) => pause_busy = false,
                Ok(Done::Sinks) => sinks_busy = false,
                Ok(Done::Action) => action_busy = false,
                Ok(Done::Save) => save_busy = false,
                Ok(Done::Reload) => reload_busy = false,
                Err(e) if e.is_panic() => std::panic::resume_unwind(e.into_panic()),
                Err(_) => {}
            },
            _ = status_tick.tick() => {
                if !status_busy && !pause_busy {
                    status_busy = true;
                    let backend = Arc::clone(&backend);
                    let generation = pause_generation;
                    tasks.spawn(async move {
                        Done::Status(Box::new(backend.daemon_state().await), generation)
                    });
                }
                if let Some(tailer) = logs.as_mut() {
                    forward_new_logs(tailer, &updates);
                }
            }
            _ = sinks_tick.tick(), if visible => {
                if !sinks_busy {
                    sinks_busy = true;
                    let (backend, updates) = (Arc::clone(&backend), updates.clone());
                    tasks.spawn(async move {
                        let _ = updates.send(backend.sinks_snapshot().await);
                        Done::Sinks
                    });
                }
            }
            changed = sinks_visible.changed() => {
                if changed.is_err() {
                    break; // UI gone
                }
                visible = *sinks_visible.borrow_and_update();
                if visible {
                    sinks_tick.reset_immediately();
                }
            }
        }

        if !action_busy && let Some(action) = actions.pop_front() {
            action_busy = true;
            let (backend, updates) = (Arc::clone(&backend), updates.clone());
            tasks.spawn(async move {
                run_action(&*backend, action, &updates).await;
                Done::Action
            });
        }
        if pause_queued && !pause_busy {
            pause_queued = false;
            pause_busy = true;
            pause_generation = pause_generation.wrapping_add(1);
            let (backend, updates) = (Arc::clone(&backend), updates.clone());
            tasks.spawn(async move {
                let result = backend
                    .toggle_pause()
                    .await
                    .map_err(|e| format!("Failed to pause or resume: {e:#}"));
                let refresh = result.is_ok();
                let _ = updates.send(AppUpdate::PauseToggled(result));
                // The pause shows up in the status right away, not a poll later
                if refresh {
                    let _ = updates.send(backend.daemon_state().await);
                }
                Done::Pause
            });
        }
        if !save_busy && let Some((config, path, force)) = next_save.take() {
            save_busy = true;
            let (backend, updates) = (Arc::clone(&backend), updates.clone());
            tasks.spawn(async move {
                let outcome = backend.save_config((*config).clone(), path, force).await;
                let _ = updates.send(AppUpdate::ConfigSaved {
                    saved: config,
                    outcome,
                });
                Done::Save
            });
        }
        // A reload waits for a running save (it must read what the save wrote)
        if !save_busy
            && !reload_busy
            && let Some(path) = next_reload.take()
        {
            reload_busy = true;
            let (backend, updates) = (Arc::clone(&backend), updates.clone());
            tasks.spawn(async move {
                let _ = updates.send(AppUpdate::ConfigReloaded(backend.load_config(path).await));
                Done::Reload
            });
        }
    }
}

/// Run one daemon action and report it; after a successful start/stop/restart
/// refresh the daemon state right away so the dashboard does not lag a poll.
async fn run_action<B: Backend>(
    backend: &B,
    action: DaemonAction,
    updates: &mpsc::UnboundedSender<AppUpdate>,
) {
    let (result, refresh) = match backend.daemon_action(action).await {
        Ok(msg) => (
            Ok(msg),
            matches!(
                action,
                DaemonAction::Start | DaemonAction::Stop | DaemonAction::Restart
            ),
        ),
        // A start that failed: `pwsw doctor` checks the whole setup
        Err(e) if matches!(action, DaemonAction::Start) => (
            Err(format!(
                "Daemon did not start (pwsw doctor checks the setup): {e:#}"
            )),
            false,
        ),
        // Its stop or its start may have failed: the error says which
        Err(e) if matches!(action, DaemonAction::Restart) => {
            (Err(format!("Restart failed: {e:#}")), false)
        }
        Err(e) => (Err(format!("Failed: {e:#}")), false),
    };
    let _ = updates.send(AppUpdate::ActionResult(result));
    if refresh {
        let _ = updates.send(backend.daemon_state().await);
    }
}

/// Send any new daemon log lines to the UI.
fn forward_new_logs(tailer: &mut LogTailer, updates: &mpsc::UnboundedSender<AppUpdate>) {
    if !tailer.has_file_changed() {
        return;
    }
    match tailer.read_new_lines() {
        Ok(lines) if !lines.is_empty() => {
            let _ = updates.send(AppUpdate::DaemonLogs(lines));
        }
        Ok(_) => {}
        Err(e) => debug!("Failed to read new daemon logs: {e:#}"),
    }
}
