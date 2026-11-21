//! Daemon mode
//!
//! Startup (single-instance lock, optional detach, logging, compositor, `IPC`) and the
//! wiring of the event loop (`daemon/event_loop.rs`), which tracks windows, reconciles the
//! default sink with the rules, and never waits on `PipeWire` itself.

mod event_loop;
mod ipc_handler;
mod lock;
mod notifier;
mod worker;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use anyhow::{Context, Result};
use notify::{Event, RecursiveMode, Watcher};
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

use self::event_loop::{
    CompositorInfo, Daemon, Hooks, INITIAL_SYNC_TIMEOUT, Inputs, Signal, output_rules_warning,
};
use self::ipc_handler::ServerInfo;
use self::lock::InstanceLock;
use self::notifier::{Notice, NoticeKind, Notifier};
use self::worker::PipeWireSwitcher;
use crate::compositor::{self, ProtocolPreference};
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
/// Limitation: stderr stays bound to this file (inode) for the daemon's lifetime.
/// Once the log rotates it is `daemon.log.old`, and after the next rotation an
/// unlinked file, so late stderr output is lost. Everything that matters after
/// startup goes through `tracing` instead, which follows rotation (panics too, via
/// [`crate::logging::install_panic_hook`]).
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
    crate::outln!("Starting daemon (PID: {})...", pid.to_string().technical());

    for _attempt in 1..=MAX_ATTEMPTS {
        tokio::time::sleep(Duration::from_millis(RETRY_DELAY_MS)).await;

        if ipc::is_daemon_running().await {
            crate::outln!(
                "{} {}",
                "✓".success(),
                "Daemon started successfully".success()
            );
            crate::outln!("Use {} to check daemon status", "pwsw status".technical());
            crate::outln!("Use {} to stop the daemon", "pwsw shutdown".technical());
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
                anyhow::bail!(
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

    anyhow::bail!(
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
fn already_running(check: &str) -> anyhow::Error {
    anyhow::anyhow!(
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
        .map_err(|e| anyhow::anyhow!("Join error during tool validation: {e:#}"))??;

    // Logging: always to the log file (TUI log view); also stderr in foreground mode
    // (systemd / debugging). Kept alive until `run` returns so no lines are lost.
    let log_path = get_log_file_path()?;
    let log_dir = log_path
        .parent()
        .context("Log file path has no parent directory")?;
    let logging =
        crate::logging::init_daemon_logging(log_dir, foreground, &config.settings.log_level)?;
    // Panics go to the current log file too (stderr may point at a rotated-away file)
    crate::logging::install_panic_hook();

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
        .map_err(|e| anyhow::anyhow!("Join error during manager detection: {e:#}"))?;
    info!("Daemon manager: {:?}", daemon_manager);

    let state = State::new(Arc::clone(&config))?;

    // The current defaults, so startup does not switch needlessly
    let (applied, applied_source) = match tokio::task::spawn_blocking(PipeWire::get_defaults).await
    {
        Ok(Ok((sink, source))) => {
            if let Some(name) = &sink {
                info!("Current default sink: {name}");
            } else {
                warn!("Could not query default sink: none set in PipeWire metadata");
            }
            if !config.sources.is_empty() {
                info!(
                    "Current default source: {}",
                    source.as_deref().unwrap_or("unknown")
                );
            }
            (sink, source)
        }
        Ok(Err(e)) => {
            warn!("Could not query default sink: {e:#}");
            (None, None)
        }
        Err(e) => {
            warn!("Could not query default sink: {e}");
            (None, None)
        }
    };

    let preference = protocol_preference(&config);
    let (window_events, capabilities) =
        compositor::spawn_compositor_thread_with_preference(preference)?;
    info!(
        "Compositor event thread started: {} backend, {}",
        capabilities.backend().name(),
        capabilities.monitor_info_summary()
    );
    let compositor = CompositorInfo {
        capabilities,
        preference,
    };
    if let Some(warning) = output_rules_warning(compositor, config.has_output_rules()) {
        warn!("{warning}");
    }

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
            compositor: Some(capabilities),
        }),
    );

    // Config hot-reload: the loop debounces these events
    let config_path = Config::get_config_path()?;
    let (config_tx, config_rx) = mpsc::unbounded_channel();
    let mut config_watch = ConfigWatch::start(&config_path, config_tx)?;

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
        status: None,
        // The config symlink may have been pointed elsewhere
        after_reload: Some(Box::new(move || config_watch.refresh())),
    };
    let daemon = Daemon::new(
        state,
        config_path,
        Arc::new(PipeWireSwitcher),
        notifier,
        hooks,
        applied,
    )
    .with_applied_source(applied_source)
    .with_compositor(compositor)
    // The baseline (and `default_on_startup`) takes the windows already open into
    // account, instead of switching once per window as they are reported
    .with_initial_sync(INITIAL_SYNC_TIMEOUT);

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

/// The toplevel protocol to ask the compositor for: `wlr` when a rule has `output`
/// (only `wlr` reports which monitors windows are on), else the default (`ext` first)
///
/// Decided once at startup; a reload never reconnects (see [`output_rules_warning`]).
fn protocol_preference(config: &Config) -> ProtocolPreference {
    if config.has_output_rules() {
        ProtocolPreference::PreferWlr
    } else {
        ProtocolPreference::Auto
    }
}

/// Watches the config file for hot-reload, following a file-level symlink
///
/// Dotfile managers (`stow --no-folding`, `chezmoi`) make the config a symlink into
/// another directory. Editors and the `TUI` write the symlink's target, which produces
/// events only in the target's directory, so that directory is watched too and events
/// for either path count. [`ConfigWatch::refresh`] re-resolves the target (the symlink
/// may have been pointed elsewhere).
struct ConfigWatch {
    watcher: notify::RecommendedWatcher,
    config_path: PathBuf,
    /// Paths whose events mean "the config changed", shared with the event callback
    targets: Arc<Mutex<Vec<PathBuf>>>,
    /// Directories currently watched
    watched: Vec<PathBuf>,
    /// A test's marker file and where its events go instead (see the tests' `drain`)
    #[cfg(test)]
    fence: Fence,
}

/// A marker file whose events the callback reports on their own channel: events are
/// handled in order, so once the marker's arrives, every earlier one has been sent
#[cfg(test)]
type Fence = Arc<Mutex<Option<(PathBuf, mpsc::UnboundedSender<()>)>>>;

impl ConfigWatch {
    /// Start watching; each relevant event sends `()` on `tx`
    ///
    /// # Errors
    /// Returns an error if the watcher cannot be created. A directory that cannot be
    /// watched only logs a warning (hot-reload is then unavailable for it).
    fn start(config_path: &Path, tx: mpsc::UnboundedSender<()>) -> Result<Self> {
        let targets: Arc<Mutex<Vec<PathBuf>>> = Arc::new(Mutex::new(Vec::new()));
        let matching = Arc::clone(&targets);
        #[cfg(test)]
        let fence: Fence = Arc::default();
        #[cfg(test)]
        let fenced = Arc::clone(&fence);
        let watcher =
            notify::recommended_watcher(move |res: Result<Event, notify::Error>| match res {
                Ok(event) => {
                    #[cfg(test)]
                    if let Some((marker, tx)) =
                        &*fenced.lock().unwrap_or_else(PoisonError::into_inner)
                        && event.paths.contains(marker)
                    {
                        let _ = tx.send(());
                        return;
                    }
                    if !(event.kind.is_modify() || event.kind.is_create()) {
                        return;
                    }
                    let targets = matching.lock().unwrap_or_else(PoisonError::into_inner);
                    if event.paths.iter().any(|p| targets.contains(p)) {
                        let _ = tx.send(());
                    }
                }
                Err(e) => error!("Config watch error: {e:?}"),
            })?;
        let mut watch = Self {
            watcher,
            config_path: config_path.to_path_buf(),
            targets,
            watched: Vec::new(),
            #[cfg(test)]
            fence,
        };
        watch.refresh();
        Ok(watch)
    }

    /// Re-resolve the config's symlink target and watch the directories it needs
    fn refresh(&mut self) {
        let mut paths = vec![self.config_path.clone()];
        if let Ok(target) = std::fs::canonicalize(&self.config_path)
            && target != self.config_path
        {
            debug!("Config is a symlink to {}", target.display());
            paths.push(target);
        }
        let mut needed: Vec<PathBuf> = Vec::new();
        for dir in paths.iter().filter_map(|p| p.parent()) {
            if !needed.iter().any(|d| d == dir) {
                needed.push(dir.to_path_buf());
            }
        }

        for dir in std::mem::take(&mut self.watched) {
            if needed.contains(&dir) {
                self.watched.push(dir);
            } else if let Err(e) = self.watcher.unwatch(&dir) {
                debug!("Failed to stop watching {}: {e}", dir.display());
            }
        }
        for dir in needed {
            if self.watched.contains(&dir) {
                continue;
            }
            match self.watcher.watch(&dir, RecursiveMode::NonRecursive) {
                Ok(()) => self.watched.push(dir),
                Err(e) => warn!(
                    "Failed to watch {} for config hot-reload: {e}",
                    dir.display()
                ),
            }
        }
        *self.targets.lock().unwrap_or_else(PoisonError::into_inner) = paths;
    }
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

#[cfg(test)]
mod tests {
    use std::os::unix::fs::symlink;
    use std::time::Duration;

    use super::*;

    /// Upper bound for one watcher event to arrive
    const EVENT_WAIT: Duration = Duration::from_secs(10);

    /// Rules with `output` make the daemon prefer `wlr`; without them it keeps `ext` first
    #[test]
    fn test_protocol_preference_follows_output_rules() {
        use crate::test_utils::fixtures::{make_config, make_rule, make_sink, with_output};
        let sinks = vec![make_sink("tv", "TV", true)];
        let plain = make_config(sinks.clone(), vec![make_rule("^mpv$", None, "TV")]);
        assert_eq!(protocol_preference(&plain), ProtocolPreference::Auto);
        let empty = make_config(sinks.clone(), vec![]);
        assert_eq!(protocol_preference(&empty), ProtocolPreference::Auto);
        let monitor = make_config(sinks, vec![
            make_rule("^mpv$", None, "TV"),
            with_output(make_rule("", None, "TV"), "^HDMI-"),
        ]);
        assert_eq!(protocol_preference(&monitor), ProtocolPreference::PreferWlr);
    }

    async fn expect_event(rx: &mut mpsc::UnboundedReceiver<()>, what: &str) {
        let got = tokio::time::timeout(EVENT_WAIT, rx.recv()).await;
        assert!(
            matches!(got, Ok(Some(()))),
            "no config event after {what} within {EVENT_WAIT:?}"
        );
    }

    /// Drop every event of the steps so far, so a late one cannot satisfy the next
    /// [`expect_event`]: write a marker file into `dir` (a watched directory) and wait
    /// for its event, which the watcher handles after all earlier ones
    async fn drain(watch: &ConfigWatch, dir: &Path, rx: &mut mpsc::UnboundedReceiver<()>) {
        static MARKERS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = MARKERS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let marker = dir.join(format!(".fence-{n}"));
        let (tx, mut fenced) = mpsc::unbounded_channel();
        *watch.fence.lock().expect("lock") = Some((marker.clone(), tx));
        std::fs::write(&marker, "").expect("write marker");
        let got = tokio::time::timeout(EVENT_WAIT, fenced.recv()).await;
        assert!(
            matches!(got, Ok(Some(()))),
            "no event for the marker {} within {EVENT_WAIT:?}",
            marker.display()
        );
        *watch.fence.lock().expect("lock") = None;
        while rx.try_recv().is_ok() {}
    }

    /// A file-level symlinked config (`stow --no-folding`, `chezmoi`) is reloaded when
    /// its target is written, also after the symlink is pointed elsewhere
    #[tokio::test]
    async fn test_symlinked_config_target_changes_are_seen() {
        let root = tempfile::tempdir().expect("tempdir");
        // Canonical, so only the symlink under test differs from its target
        let base = root.path().canonicalize().expect("canonical tempdir");
        let (link_dir, a, b) = (
            base.join("config"),
            base.join("dotfiles-a"),
            base.join("dotfiles-b"),
        );
        for dir in [&link_dir, &a, &b] {
            std::fs::create_dir(dir).expect("mkdir");
        }
        let (target_a, target_b) = (a.join("config.toml"), b.join("config.toml"));
        std::fs::write(&target_a, "# a\n").expect("write");
        std::fs::write(&target_b, "# b\n").expect("write");
        let config_path = link_dir.join("config.toml");
        symlink(&target_a, &config_path).expect("symlink");

        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut watch = ConfigWatch::start(&config_path, tx).expect("watcher");
        drain(&watch, &link_dir, &mut rx).await;

        std::fs::write(&target_a, "# a, edited\n").expect("write");
        expect_event(&mut rx, "editing the symlink target").await;
        drain(&watch, &link_dir, &mut rx).await;

        // Point the symlink at another file (atomically, like `ln -sf` via rename)
        let tmp_link = link_dir.join(".config.toml.tmp");
        symlink(&target_b, &tmp_link).expect("symlink");
        std::fs::rename(&tmp_link, &config_path).expect("retarget");
        expect_event(&mut rx, "retargeting the symlink").await;
        watch.refresh();
        drain(&watch, &link_dir, &mut rx).await;

        std::fs::write(&target_b, "# b, edited\n").expect("write");
        expect_event(&mut rx, "editing the new symlink target").await;
        assert_eq!(
            watch.watched,
            vec![link_dir.clone(), b.clone()],
            "the old target's directory is no longer watched"
        );
    }

    /// A plain config file still reloads on change
    #[tokio::test]
    async fn test_plain_config_changes_are_seen() {
        let root = tempfile::tempdir().expect("tempdir");
        let base = root.path().canonicalize().expect("canonical tempdir");
        let config_path = base.join("config.toml");
        std::fs::write(&config_path, "# v1\n").expect("write");
        let (tx, mut rx) = mpsc::unbounded_channel();
        let watch = ConfigWatch::start(&config_path, tx).expect("watcher");
        drain(&watch, &base, &mut rx).await;
        std::fs::write(&config_path, "# v2\n").expect("write");
        expect_event(&mut rx, "editing the config").await;
        assert_eq!(watch.watched, vec![base]);
    }
}
