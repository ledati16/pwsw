//! Background worker and command-queue tests (fake backend, paused clock).

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use tokio::sync::{mpsc, watch};
use tokio::time::Instant;

use crate::config::Config;
use crate::test_utils::fixtures::{make_config, make_sink};
use crate::tui::app::{App, AppUpdate, BgCommand, DaemonAction, SaveOutcome, Screen};
use crate::tui::input::simulate_key_event;
use crate::tui::worker::{self, Backend, STATUS_POLL};

/// Backend whose status poll is slow and whose save can be made to panic.
#[derive(Default)]
struct Fake {
    status_delay: Duration,
    panic_on_save: bool,
    /// The daemon's pause state; a status poll reads it when it starts
    paused: AtomicBool,
}

impl Backend for Fake {
    async fn daemon_state(&self) -> AppUpdate {
        let paused = self.paused.load(Ordering::SeqCst);
        tokio::time::sleep(self.status_delay).await;
        AppUpdate::DaemonState {
            running: true,
            windows: Vec::new(),
            unit_installed: false,
            service_enabled: None,
            paused,
        }
    }

    fn sinks_snapshot(&self) -> impl Future<Output = AppUpdate> + Send {
        std::future::ready(AppUpdate::PipeWireUnavailable)
    }

    fn daemon_action(&self, action: DaemonAction) -> impl Future<Output = Result<String>> + Send {
        std::future::ready(Ok(format!("{action:?} done")))
    }

    fn toggle_pause(&self) -> impl Future<Output = Result<bool>> + Send {
        std::future::ready(Ok(!self.paused.fetch_xor(true, Ordering::SeqCst)))
    }

    fn save_config(
        &self,
        _config: Config,
        _path: PathBuf,
        _force: bool,
    ) -> impl Future<Output = SaveOutcome> + Send {
        let panic = self.panic_on_save;
        async move {
            assert!(!panic, "save exploded");
            SaveOutcome::Saved
        }
    }

    fn load_config(
        &self,
        _path: PathBuf,
    ) -> impl Future<Output = Result<Box<Config>, String>> + Send {
        std::future::ready(Ok(Box::new(make_config(vec![], vec![]))))
    }
}

/// A save command for the fake backend (the path is never touched).
fn save_command() -> BgCommand {
    BgCommand::SaveConfig {
        config: Box::new(make_config(vec![], vec![])),
        path: PathBuf::from("unused"),
        force: false,
    }
}

struct Harness {
    commands: mpsc::Sender<BgCommand>,
    updates: mpsc::UnboundedReceiver<AppUpdate>,
    visible: watch::Sender<bool>,
    handle: tokio::task::JoinHandle<()>,
}

fn spawn_worker(fake: Fake, sinks_visible: bool) -> Harness {
    let (cmd_tx, cmd_rx) = mpsc::channel(8);
    let (update_tx, update_rx) = mpsc::unbounded_channel();
    let (visible_tx, visible_rx) = watch::channel(sinks_visible);
    let handle = tokio::spawn(worker::run(
        Arc::new(fake),
        cmd_rx,
        update_tx,
        visible_rx,
        None,
    ));
    Harness {
        commands: cmd_tx,
        updates: update_rx,
        visible: visible_tx,
        handle,
    }
}

/// Receive updates until one satisfies `pred`.
async fn recv_until(
    updates: &mut mpsc::UnboundedReceiver<AppUpdate>,
    pred: impl Fn(&AppUpdate) -> bool,
) -> AppUpdate {
    loop {
        let update = updates.recv().await.expect("worker alive");
        if pred(&update) {
            return update;
        }
    }
}

#[tokio::test(start_paused = true)]
async fn test_worker_runs_commands_immediately_despite_slow_poll() {
    // The first status poll starts at once and takes 30 s.
    let mut h = spawn_worker(
        Fake {
            status_delay: Duration::from_secs(30),
            panic_on_save: false,
            ..Fake::default()
        },
        false,
    );
    let start = Instant::now();

    h.commands.send(save_command()).await.expect("queued");
    let saved = recv_until(&mut h.updates, |u| {
        matches!(u, AppUpdate::ConfigSaved { .. })
    })
    .await;
    assert!(matches!(saved, AppUpdate::ConfigSaved {
        outcome: SaveOutcome::Saved,
        ..
    }));

    h.commands
        .send(BgCommand::ReloadConfig(PathBuf::from("unused")))
        .await
        .expect("queued");
    let reloaded = recv_until(&mut h.updates, |u| {
        matches!(u, AppUpdate::ConfigReloaded(_))
    })
    .await;
    assert!(matches!(reloaded, AppUpdate::ConfigReloaded(Ok(_))));

    h.commands
        .send(BgCommand::DaemonAction(DaemonAction::Enable))
        .await
        .expect("queued");
    let result = recv_until(&mut h.updates, |u| matches!(u, AppUpdate::ActionResult(_))).await;
    assert!(matches!(result, AppUpdate::ActionResult(Ok(ref m)) if m == "Enable done"));

    // No clock time passed: neither command waited for the poll or a tick.
    assert!(
        start.elapsed() < STATUS_POLL,
        "commands waited {:?}",
        start.elapsed()
    );
    h.handle.abort();
}

#[tokio::test(start_paused = true)]
async fn test_worker_snapshots_sinks_only_while_visible() {
    let mut h = spawn_worker(
        Fake {
            status_delay: Duration::ZERO,
            panic_on_save: false,
            ..Fake::default()
        },
        false,
    );

    tokio::time::sleep(Duration::from_secs(10)).await;
    let mut snapshots = 0;
    let mut polls = 0;
    while let Ok(update) = h.updates.try_recv() {
        match update {
            AppUpdate::PipeWireUnavailable | AppUpdate::SinksData { .. } => snapshots += 1,
            AppUpdate::DaemonState { .. } => polls += 1,
            _ => {}
        }
    }
    assert_eq!(snapshots, 0, "no pw-dump while no sink screen is visible");
    assert!(polls >= 10, "status polled every second, got {polls}");

    // Becoming visible triggers a snapshot right away
    let start = Instant::now();
    h.visible.send(true).expect("worker alive");
    recv_until(&mut h.updates, |u| {
        matches!(u, AppUpdate::PipeWireUnavailable)
    })
    .await;
    assert_eq!(start.elapsed(), Duration::ZERO);
    h.handle.abort();
}

#[tokio::test(start_paused = true)]
async fn test_worker_stops_when_ui_goes_away() {
    let h = spawn_worker(
        Fake {
            status_delay: Duration::ZERO,
            panic_on_save: false,
            ..Fake::default()
        },
        false,
    );
    drop(h.commands);
    tokio::time::timeout(Duration::from_secs(5), h.handle)
        .await
        .expect("worker exits")
        .expect("worker exits cleanly");
}

#[tokio::test(start_paused = true)]
async fn test_worker_reraises_task_panics() {
    let h = spawn_worker(
        Fake {
            status_delay: Duration::ZERO,
            panic_on_save: true,
            ..Fake::default()
        },
        false,
    );
    h.commands.send(save_command()).await.expect("queued");
    let joined = tokio::time::timeout(Duration::from_secs(5), h.handle)
        .await
        .expect("worker ends");
    assert!(joined.is_err_and(|e| e.is_panic()), "panic surfaced");
}

fn dirty_app(tx: mpsc::Sender<BgCommand>) -> App {
    let mut app = App::with_config(make_config(vec![make_sink("s1", "d1", true)], vec![]));
    app.bg_cmd_tx = Some(tx);
    app.config_path = Some(PathBuf::from("unused"));
    app.config_dirty = true;
    app
}

#[test]
fn test_failed_command_send_leaves_no_pending_state() {
    // Worker gone (receiver dropped)
    let (tx, rx) = mpsc::channel(1);
    drop(rx);
    let mut app = dirty_app(tx);

    simulate_key_event(
        &mut app,
        KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL),
    );
    assert!(!app.save_pending, "no save pending after failed send");
    assert!(!app.is_busy());

    app.current_screen = Screen::Dashboard;
    simulate_key_event(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(
        !app.daemon_action_pending,
        "no action pending after failed send"
    );
    let status = app.status_message().expect("failure reported");
    assert!(status.contains("worker stopped"), "{status}");
}

#[test]
fn test_full_command_queue_leaves_no_pending_state() {
    let (tx, _rx) = mpsc::channel(1);
    tx.try_send(BgCommand::DaemonAction(DaemonAction::Stop))
        .expect("fill the queue");
    let mut app = dirty_app(tx);

    simulate_key_event(
        &mut app,
        KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL),
    );
    assert!(!app.save_pending);
    let status = app.status_message().expect("failure reported");
    assert!(status.contains("Busy"), "{status}");
}

#[test]
fn test_successful_send_sets_pending_until_result() {
    let (tx, mut rx) = mpsc::channel(4);
    let mut app = dirty_app(tx);
    app.current_screen = Screen::Dashboard;

    simulate_key_event(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(app.daemon_action_pending);
    assert!(matches!(
        rx.try_recv(),
        Ok(BgCommand::DaemonAction(DaemonAction::Start))
    ));

    // A second request while one runs is refused, not queued
    simulate_key_event(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(rx.try_recv().is_err());

    app.handle_update(AppUpdate::ActionResult(Ok("ok".to_string())));
    assert!(!app.is_busy());
}

#[tokio::test(start_paused = true)]
async fn test_worker_toggles_pause_at_once_and_drops_stale_polls() {
    // The first status poll starts at once (reading "not paused") and takes 5 s
    let mut h = spawn_worker(
        Fake {
            status_delay: Duration::from_secs(5),
            ..Fake::default()
        },
        false,
    );
    tokio::task::yield_now().await;
    let start = Instant::now();

    h.commands
        .send(BgCommand::TogglePause)
        .await
        .expect("queued");
    let toggled = recv_until(&mut h.updates, |u| {
        matches!(
            u,
            AppUpdate::PauseToggled(_) | AppUpdate::DaemonState { .. }
        )
    })
    .await;
    assert!(
        matches!(toggled, AppUpdate::PauseToggled(Ok(true))),
        "the answer comes first"
    );
    assert!(start.elapsed() < STATUS_POLL, "did not wait for the poll");

    // The poll that started before the request is dropped: every state that
    // arrives afterwards says paused
    for _ in 0..3 {
        let state = recv_until(&mut h.updates, |u| {
            matches!(u, AppUpdate::DaemonState { .. })
        })
        .await;
        assert!(
            matches!(state, AppUpdate::DaemonState { paused: true, .. }),
            "stale pause state after the toggle"
        );
    }

    // And back
    h.commands
        .send(BgCommand::TogglePause)
        .await
        .expect("queued");
    let toggled = recv_until(&mut h.updates, |u| matches!(u, AppUpdate::PauseToggled(_))).await;
    assert!(matches!(toggled, AppUpdate::PauseToggled(Ok(false))));
    let state = recv_until(&mut h.updates, |u| {
        matches!(u, AppUpdate::DaemonState { .. })
    })
    .await;
    assert!(matches!(state, AppUpdate::DaemonState {
        paused: false,
        ..
    }));
    h.handle.abort();
}
