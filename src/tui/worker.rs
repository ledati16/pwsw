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
//! - A panic in any task is re-raised in the worker, whose end the event loop
//!   observes.

use std::collections::VecDeque;
use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::time::Duration;

use color_eyre::eyre::{self, Result};
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinSet;
use tokio::time::{MissedTickBehavior, interval};
use tracing::debug;

use super::app::{AppUpdate, BgCommand, DaemonAction};
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
        .map_err(|e| eyre::eyre!("Failed to spawn worker thread: {e}"))?;
    match rx.await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(panic)) => std::panic::resume_unwind(panic),
        Err(_) => eyre::bail!("worker thread ended without a result"),
    }
}

/// The slow operations the worker performs (a trait so tests can fake them).
pub(crate) trait Backend: Send + Sync + 'static {
    /// Poll daemon status and windows; yields [`AppUpdate::DaemonState`].
    fn daemon_state(&self) -> impl Future<Output = AppUpdate> + Send;
    /// Take a `PipeWire` snapshot; yields [`AppUpdate::SinksData`] or
    /// [`AppUpdate::PipeWireUnavailable`].
    fn sinks_snapshot(&self) -> impl Future<Output = AppUpdate> + Send;
    /// Run a daemon control action, returning a message for the status line.
    fn daemon_action(&self, action: DaemonAction) -> impl Future<Output = Result<String>> + Send;
    /// Save the config atomically.
    fn save_config(&self, config: Config) -> impl Future<Output = Result<()>> + Send;
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
        let service_enabled = match daemon_manager {
            Some(dm @ DaemonManager::Systemd) => Some(dm.is_enabled().await),
            _ => None,
        };
        let windows = if running {
            match send_request(Request::ListWindows).await {
                Ok(Response::Windows { windows }) => windows,
                _ => Vec::new(),
            }
        } else {
            Vec::new()
        };
        AppUpdate::DaemonState {
            running,
            windows,
            daemon_manager,
            service_enabled,
        }
    }

    async fn sinks_snapshot(&self) -> AppUpdate {
        use crate::pipewire::PipeWire;

        match detached(PipeWire::dump).await {
            Ok(Ok(snapshot)) => {
                let active = PipeWire::get_active_sinks(&snapshot);
                let profiles = PipeWire::get_profile_sinks(&snapshot, &active);
                AppUpdate::SinksData { active, profiles }
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

    async fn save_config(&self, config: Config) -> Result<()> {
        detached(move || config.save()).await?
    }
}

/// Which task finished.
enum Done {
    Status,
    Sinks,
    Action,
    Save,
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
    let mut next_save: Option<Config> = None;
    let mut visible = *sinks_visible.borrow_and_update();

    loop {
        tokio::select! {
            cmd = commands.recv() => match cmd {
                Some(BgCommand::DaemonAction(action)) => actions.push_back(action),
                Some(BgCommand::SaveConfig(config)) => next_save = Some(config),
                None => break,
            },
            Some(done) = tasks.join_next() => match done {
                Ok(Done::Status) => status_busy = false,
                Ok(Done::Sinks) => sinks_busy = false,
                Ok(Done::Action) => action_busy = false,
                Ok(Done::Save) => save_busy = false,
                Err(e) if e.is_panic() => std::panic::resume_unwind(e.into_panic()),
                Err(_) => {}
            },
            _ = status_tick.tick() => {
                if !status_busy {
                    status_busy = true;
                    let (backend, updates) = (Arc::clone(&backend), updates.clone());
                    tasks.spawn(async move {
                        let _ = updates.send(backend.daemon_state().await);
                        Done::Status
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
        if !save_busy && let Some(config) = next_save.take() {
            save_busy = true;
            let (backend, updates) = (Arc::clone(&backend), updates.clone());
            tasks.spawn(async move {
                let update = match backend.save_config(config).await {
                    Ok(()) => AppUpdate::ConfigSaved {
                        success: true,
                        message: "Configuration saved successfully".to_string(),
                    },
                    Err(e) => AppUpdate::ConfigSaved {
                        success: false,
                        message: format!("Failed to save config: {e:#}"),
                    },
                };
                let _ = updates.send(update);
                Done::Save
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
    let (message, refresh) = match backend.daemon_action(action).await {
        Ok(msg) => (
            msg,
            matches!(
                action,
                DaemonAction::Start | DaemonAction::Stop | DaemonAction::Restart
            ),
        ),
        Err(e) => (format!("Failed: {e:#}"), false),
    };
    let _ = updates.send(AppUpdate::ActionResult(message));
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
