//! Daemon mode
//!
//! Startup (single-instance lock, optional detach, logging, compositor, `IPC`) and the
//! wiring of the event loop in [`event_loop`], which tracks windows, reconciles the
//! default sink with the rules, and never waits on `PipeWire` itself.

mod event_loop;
mod ipc_handler;
mod lock;
mod notifier;
mod worker;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use color_eyre::eyre::{self, Context, ContextCompat, Result};
use notify::{Event, RecursiveMode, Watcher};
use tokio::sync::mpsc;
use tracing::{error, info, warn};

use self::event_loop::{Daemon, Hooks, Inputs, Signal};
use self::ipc_handler::ServerInfo;
use self::lock::InstanceLock;
use self::notifier::{Notice, NoticeKind, Notifier};
use self::worker::PipeWireSwitcher;
use crate::compositor;
use crate::config::Config;
use crate::ipc::{self, IpcServer};
use crate::pipewire::PipeWire;
use crate::state::State;
use crate::style::PwswStyle;

// ============================================================================
// Public API
// ============================================================================

/// Get the daemon log file path
///
/// Returns `~/.local/share/pwsw/daemon.log`
///
/// # Errors
/// Returns an error if the local data directory cannot be determined.
pub fn get_log_file_path() -> Result<PathBuf> {
    let log_dir = dirs::data_local_dir()
        .context("Failed to get local data directory")?
        .join("pwsw");
    Ok(log_dir.join("daemon.log"))
}

/// Open the daemon log for appending (creating it and its directory, mode `0600`).
///
/// Used as the background daemon's stderr so that anything printed before
/// logging is initialised (or a final error report) ends up next to its logs
/// instead of vanishing into `/dev/null`.
///
/// # Errors
/// Returns an error if the directory or file cannot be created/opened.
fn open_log_for_stderr(log_path: &std::path::Path) -> Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;

    if let Some(dir) = log_path.parent() {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("Failed to create log directory: {}", dir.display()))?;
    }
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(log_path)
        .with_context(|| format!("Failed to open daemon log: {}", log_path.display()))
}

/// Read up to `max_lines` trailing lines written to `log_path` after byte `offset`.
///
/// Best effort: returns an empty list if the file cannot be read.
fn log_lines_since(log_path: &std::path::Path, offset: u64, max_lines: usize) -> Vec<String> {
    use std::io::{Read, Seek, SeekFrom};

    let Ok(mut file) = std::fs::File::open(log_path) else {
        return Vec::new();
    };
    let mut buf = Vec::new();
    if file.seek(SeekFrom::Start(offset)).is_err() || file.read_to_end(&mut buf).is_err() {
        return Vec::new();
    }
    let text = String::from_utf8_lossy(&buf);
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    lines[lines.len().saturating_sub(max_lines)..]
        .iter()
        .map(|l| crate::sanitize::strip_ansi(l))
        .collect()
}

/// Spawn the real daemon as a detached background process and wait until it
/// answers on the `IPC` socket.
///
/// The child gets its own process group so it survives the launching terminal
/// (or `TUI`) closing and is not hit by `Ctrl-C` aimed at the foreground job;
/// its stdin/stdout go to `/dev/null` and its stderr is appended to the daemon
/// log, so start-up errors are kept.
///
/// # Errors
/// Returns an error if the child cannot be spawned, exits during start-up, or
/// does not answer within the start-up window.
async fn spawn_background_daemon() -> Result<()> {
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};
    use std::time::Duration;

    // Wait for daemon to initialize by checking if it's responding to IPC
    // Try for up to 2 seconds (20 attempts * 100ms)
    const MAX_ATTEMPTS: u32 = 20;
    const RETRY_DELAY_MS: u64 = 100;
    /// How many log lines to quote when the child fails
    const FAILURE_LOG_LINES: usize = 8;

    let exe = std::env::current_exe()?;
    let log_path = get_log_file_path()?;
    let log_file = open_log_for_stderr(&log_path)?;
    // Everything the child writes from here on is about this start attempt
    let log_offset = log_file.metadata().map_or(0, |m| m.len());

    // Spawn detached daemon process WITHOUT --foreground so it logs to file
    // Pass environment variable to prevent child from spawning another process
    let mut child_cmd = Command::new(&exe);
    // Forward an explicit config selection so the child loads the same file
    if let Some(config_path) = Config::config_path_override()? {
        child_cmd.arg("--config").arg(config_path);
    }
    let mut child = child_cmd
        .arg("daemon")
        .env("PWSW_DAEMON_CHILD", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(log_file))
        // New process group: no SIGHUP/SIGINT from the launching terminal's job control
        .process_group(0)
        .spawn()
        .with_context(|| {
            format!(
                "Failed to spawn background daemon: exe={}, working_dir={}",
                exe.display(),
                std::env::current_dir()
                    .ok()
                    .as_ref()
                    .map_or_else(|| "unknown".to_string(), |p| p.display().to_string())
            )
        })?;

    let pid = child.id();
    println!("Starting daemon (PID: {})...", pid.to_string().technical());

    for _attempt in 1..=MAX_ATTEMPTS {
        tokio::time::sleep(Duration::from_millis(RETRY_DELAY_MS)).await;

        if ipc::is_daemon_running().await {
            println!(
                "{} {}",
                "✓".success(),
                "Daemon started successfully".success()
            );
            println!("Use {} to check daemon status", "pwsw status".technical());
            println!("Use {} to stop the daemon", "pwsw shutdown".technical());
            return Ok(());
        }

        // Check if child process is still alive (reaps it if it exited)
        match child.try_wait() {
            Ok(Some(status)) => {
                let tail = log_lines_since(&log_path, log_offset, FAILURE_LOG_LINES);
                let tail = if tail.is_empty() {
                    String::new()
                } else {
                    format!("\n\nLast log lines:\n  {}", tail.join("\n  "))
                };
                eyre::bail!(
                    "Daemon process (PID: {pid}) exited during startup with status: {status}{tail}\n\n\
                     Full log: {}",
                    log_path.display()
                );
            }
            Ok(None) => {} // Still running
            Err(e) => {
                warn!("Could not check daemon process status: {}", e);
            }
        }
    }

    eyre::bail!(
        "Daemon failed to start within {} seconds.\n\
         Process may still be initializing. Check with: pwsw status\n\
         Log: {}",
        (u64::from(MAX_ATTEMPTS) * RETRY_DELAY_MS) / 1000,
        log_path.display()
    );
}

// ============================================================================
// Constants
// ============================================================================

/// Notification title when daemon starts
const NOTIFICATION_STARTED_TITLE: &str = "PWSW Started";
/// Notification message when daemon starts
const NOTIFICATION_STARTED_MSG: &str = "Audio switcher running";
/// Capacity of the `IPC` → event loop command channel
const COMMAND_CHANNEL_CAPACITY: usize = 32;

/// The "already running" error, naming the check that detected it
fn already_running(check: &str) -> eyre::Report {
    eyre::eyre!(
        "Another PWSW daemon is already running ({check}).\n\
         Socket: {}\n\n\
         To stop the existing daemon, run:\n  \
         pwsw shutdown",
        ipc::get_socket_path().display()
    )
}

/// Run the daemon with the given configuration
///
/// # Errors
/// Returns an error if another daemon is running, initialization fails, compositor
/// connection fails or is lost, or a signal handler cannot be installed.
///
/// # Returns
/// Returns `Ok(())` on a requested shutdown (signal or `IPC`).
pub async fn run(config: Arc<Config>, foreground: bool) -> Result<()> {
    // Refuse early if another daemon holds the instance lock. This probe is released
    // right away; the daemon process takes the lock for good below.
    let lock_path = lock::lock_path()?;
    if InstanceLock::try_acquire(&lock_path)?.is_none() {
        return Err(already_running("instance lock"));
    }

    // Also ask the socket (catches a daemon from an older version without the lock)
    if ipc::is_daemon_running().await {
        return Err(already_running("IPC check"));
    }

    // Background mode: spawn detached process and wait for successful startup
    if !foreground && std::env::var("PWSW_DAEMON_CHILD").is_err() {
        return spawn_background_daemon().await;
    }

    // From here on this process is the daemon: hold the lock for its whole lifetime
    let Some(_instance_lock) = InstanceLock::try_acquire(&lock_path)? else {
        return Err(already_running("instance lock"));
    };

    // Validate required `PipeWire` tools are available
    // Run in spawn_blocking to avoid blocking the tokio runtime during startup
    tokio::task::spawn_blocking(PipeWire::validate_tools)
        .await
        .map_err(|e| eyre::eyre!("Join error during tool validation: {e:#}"))??;

    // Logging: always to the log file (TUI log view); also stderr in foreground mode
    // (systemd / debugging). Kept alive until `run` returns so no lines are lost.
    let log_path = get_log_file_path()?;
    let log_dir = log_path
        .parent()
        .context("Log file path has no parent directory")?;
    let logging =
        crate::logging::init_daemon_logging(log_dir, foreground, &config.settings.log_level)?;

    info!("Starting PWSW daemon {}", crate::version_string());
    info!(
        "Loaded {} sinks, {} rules",
        config.sinks.len(),
        config.rules.len()
    );
    let started = std::time::Instant::now();

    // Detect how the daemon is being managed (runs an external command)
    let daemon_manager = tokio::task::spawn_blocking(crate::daemon_manager::DaemonManager::detect)
        .await
        .map_err(|e| eyre::eyre!("Join error during manager detection: {e:#}"))?;
    info!("Daemon manager: {:?}", daemon_manager);

    let state = State::new(Arc::clone(&config))?;

    // The current default, so startup does not switch needlessly
    let applied = match tokio::task::spawn_blocking(PipeWire::get_default_sink_name).await {
        Ok(Ok(name)) => {
            info!("Current default sink: {name}");
            Some(name)
        }
        Ok(Err(e)) => {
            warn!("Could not query default sink: {e:#}");
            None
        }
        Err(e) => {
            warn!("Could not query default sink: {e}");
            None
        }
    };

    let window_events = compositor::spawn_compositor_thread()?;
    info!("Compositor event thread started");

    let ipc_server = IpcServer::bind().await?;
    info!(
        "IPC server listening on {}",
        ipc_server.socket_path().display()
    );
    let (command_tx, command_rx) = mpsc::channel(COMMAND_CHANNEL_CAPACITY);
    let ipc_task = ipc_handler::spawn(
        ipc_server,
        command_tx,
        Arc::new(ServerInfo {
            version: crate::version_string(),
            started,
            daemon_manager,
        }),
    );

    // Config hot-reload: the loop debounces these events
    let config_path = Config::get_config_path()?;
    let (config_tx, config_rx) = mpsc::unbounded_channel();
    let _watcher = watch_config(&config_path, config_tx)?;

    let signals = forward_signals()?;

    let notifier = Notifier::spawn();
    if config.settings.notify_manual {
        notifier.send(Notice {
            kind: NoticeKind::Daemon,
            summary: NOTIFICATION_STARTED_TITLE.to_string(),
            body: NOTIFICATION_STARTED_MSG.to_string(),
            icon: None,
        });
    }

    let hooks = Hooks {
        systemd: true,
        set_log_level: Some(Box::new(logging.level_setter())),
    };
    let daemon = Daemon::new(
        state,
        config_path,
        Arc::new(PipeWireSwitcher),
        notifier,
        hooks,
        applied,
    );

    // No-op unless started by systemd with `Type=notify`
    if let Err(e) = sd_notify::notify(&[sd_notify::NotifyState::Ready]) {
        warn!("Failed to notify systemd: {e}");
    }
    info!("Daemon initialization complete, entering event loop");

    let result = daemon
        .run(Inputs {
            windows: window_events,
            commands: command_rx,
            config_changed: config_rx,
            signals,
        })
        .await;

    // Stop serving IPC; dropping the server removes the socket
    ipc_task.abort();
    let _ = ipc_task.await;

    if let Err(e) = &result {
        error!("Daemon exiting with error: {e:#}");
    }
    result
}

/// Watch the config file's directory and send `()` for changes to the file itself
fn watch_config(
    config_path: &Path,
    tx: mpsc::UnboundedSender<()>,
) -> Result<notify::RecommendedWatcher> {
    let config_dir = config_path.parent().unwrap_or(config_path).to_path_buf();
    let path = config_path.to_path_buf();
    let mut watcher =
        notify::recommended_watcher(move |res: Result<Event, notify::Error>| match res {
            Ok(event) => {
                if (event.kind.is_modify() || event.kind.is_create())
                    && event.paths.iter().any(|p| p == &path)
                {
                    let _ = tx.send(());
                }
            }
            Err(e) => error!("Config watch error: {e:?}"),
        })?;
    if let Err(e) = watcher.watch(&config_dir, RecursiveMode::NonRecursive) {
        warn!("Failed to watch config directory for hot-reload: {e}");
    }
    Ok(watcher)
}

/// Install `SIGINT`/`SIGTERM`/`SIGHUP` handlers once and forward them to the loop
fn forward_signals() -> Result<mpsc::Receiver<Signal>> {
    use tokio::signal::unix::{SignalKind, signal};

    let mut sigint = signal(SignalKind::interrupt()).context("Failed to install SIGINT handler")?;
    let mut sigterm =
        signal(SignalKind::terminate()).context("Failed to install SIGTERM handler")?;
    let mut sighup = signal(SignalKind::hangup()).context("Failed to install SIGHUP handler")?;

    let (tx, rx) = mpsc::channel(8);
    tokio::spawn(async move {
        loop {
            let received = tokio::select! {
                Some(()) = sigint.recv() => Signal::Interrupt,
                Some(()) = sigterm.recv() => Signal::Terminate,
                Some(()) = sighup.recv() => Signal::Hangup,
                else => break,
            };
            if tx.send(received).await.is_err() {
                break;
            }
        }
    });
    Ok(rx)
}
