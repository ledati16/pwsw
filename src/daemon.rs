//! Daemon mode
//!
//! Runs the main event loop, listening to compositor window events,
//! IPC requests, and switching audio sinks based on configured rules.

use color_eyre::eyre::{self, Context, ContextCompat, Result};
use notify::{Event, RecursiveMode, Watcher};
use std::path::PathBuf;
use std::time::Instant;
use tokio::signal;
use tokio::sync::broadcast;
use tracing::{error, info, warn};

use crate::compositor;
use crate::config::Config;
use crate::ipc::{self, IpcServer, Request, Response, WindowInfo};
use crate::notification::send_notification;
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

/// Get the daemon PID file path
///
/// Returns `$XDG_RUNTIME_DIR/pwsw.pid` or `/tmp/pwsw-$USER.pid` as fallback
///
/// # Errors
/// Returns an error if the runtime directory cannot be determined and temp fallback fails.
pub fn get_pid_file_path() -> Result<PathBuf> {
    let runtime_dir = std::env::var("XDG_RUNTIME_DIR").ok();

    if let Some(dir) = runtime_dir {
        Ok(PathBuf::from(dir).join("pwsw.pid"))
    } else {
        // Fallback to /tmp with username suffix for multi-user safety
        let user = std::env::var("USER").unwrap_or_else(|_| "unknown".to_string());
        Ok(PathBuf::from("/tmp").join(format!("pwsw-{user}.pid")))
    }
}

/// Write the daemon PID file
///
/// Creates a file with the current process ID, with user-only permissions (0o600).
///
/// # Errors
/// Returns an error if the PID file cannot be written or permissions cannot be set.
fn write_pid_file() -> Result<()> {
    let pid_path = get_pid_file_path()?;
    let pid = std::process::id();

    std::fs::write(&pid_path, pid.to_string())
        .with_context(|| format!("Failed to write PID file: {}", pid_path.display()))?;

    // Set user-only permissions on Unix
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&pid_path, std::fs::Permissions::from_mode(0o600)).with_context(
            || format!("Failed to set PID file permissions: {}", pid_path.display()),
        )?;
    }

    info!("PID file written: {} (PID: {})", pid_path.display(), pid);
    Ok(())
}

/// Remove the daemon PID file
///
/// Attempts to remove the PID file. Logs warnings on failure but does not return errors
/// since cleanup failure is not critical during shutdown.
fn remove_pid_file() {
    if let Ok(pid_path) = get_pid_file_path()
        && pid_path.exists()
    {
        if let Err(e) = std::fs::remove_file(&pid_path) {
            warn!("Failed to remove PID file {}: {}", pid_path.display(), e);
        } else {
            info!("PID file removed: {}", pid_path.display());
        }
    }
}

/// Check if a stale PID file exists and clean it up if the process is not running
///
/// Returns `Ok(true)` if a running daemon was detected, `Ok(false)` otherwise.
///
/// # Errors
/// Returns an error if PID file reading fails or process checking encounters errors.
fn check_and_cleanup_stale_pid_file() -> Result<bool> {
    let pid_path = get_pid_file_path()?;

    if !pid_path.exists() {
        return Ok(false);
    }

    // Read PID from file
    let pid_str = std::fs::read_to_string(&pid_path)
        .with_context(|| format!("Failed to read PID file: {}", pid_path.display()))?;

    let pid: u32 = pid_str
        .trim()
        .parse()
        .with_context(|| format!("Invalid PID in file {}: '{}'", pid_path.display(), pid_str))?;

    // Check if process is still running using kill(pid, 0) on Unix
    #[cfg(unix)]
    {
        use nix::sys::signal::kill;
        use nix::unistd::Pid;

        // Signal 0 is a special case: checks if process exists without sending a signal
        match kill(Pid::from_raw(pid as i32), None) {
            Ok(()) => {
                // Process exists
                info!("Daemon already running with PID {}", pid);
                Ok(true)
            }
            Err(nix::errno::Errno::ESRCH) => {
                // Process does not exist - stale PID file
                warn!("Stale PID file found (PID {} not running), removing", pid);
                std::fs::remove_file(&pid_path).with_context(|| {
                    format!("Failed to remove stale PID file: {}", pid_path.display())
                })?;
                Ok(false)
            }
            Err(e) => {
                eyre::bail!("Failed to check if process {} exists: {}", pid, e);
            }
        }
    }

    // On non-Unix, we can't reliably check if process exists, so assume it's stale if old enough
    #[cfg(not(unix))]
    {
        warn!(
            "Cannot verify PID {} on non-Unix system, assuming stale",
            pid
        );
        std::fs::remove_file(&pid_path)
            .with_context(|| format!("Failed to remove PID file: {}", pid_path.display()))?;
        Ok(false)
    }
}

// ============================================================================
// Constants
// ============================================================================

/// Notification title when daemon starts
const NOTIFICATION_STARTED_TITLE: &str = "PWSW Started";
/// Notification message when daemon starts
const NOTIFICATION_STARTED_MSG: &str = "Audio switcher running";
/// Notification title when daemon stops
const NOTIFICATION_STOPPED_TITLE: &str = "PWSW Stopped";
/// Notification message when daemon stops
const NOTIFICATION_STOPPED_MSG: &str = "Audio switcher stopped";

// ============================================================================
// Data Structures
// ============================================================================

/// Context data passed to IPC request handlers
struct IpcContext {
    version: String,
    uptime_secs: u64,
    current_sink_name: String,
    active_window: Option<String>,
    daemon_manager: crate::daemon_manager::DaemonManager,
    // tracked: (id, app_id, title, sink_name, sink_desc)
    tracked_with_sinks: Vec<(u64, String, String, String, String)>,
    // all windows: (id, app_id, title)
    all_windows: Vec<(u64, String, String)>,
    config: Config,
    shutdown_tx: broadcast::Sender<()>,
}

/// Run the daemon with the given configuration
///
/// # Errors
/// Returns an error if another daemon is running, initialization fails, compositor
/// connection fails, or any critical component encounters an error.
///
/// # Panics
/// Panics if the SIGTERM signal handler cannot be installed. This is a platform-level
/// failure that prevents graceful shutdown handling.
// Main daemon event loop - cohesive logic hard to split; constants scoped in spawn blocks
pub async fn run(config: Config, foreground: bool) -> Result<()> {
    use std::process::Command;
    use std::time::Duration;
    // Check if a daemon is already running BEFORE any initialization
    if ipc::is_daemon_running().await {
        let socket_path = ipc::get_socket_path()?;
        eyre::bail!(
            "Another PWSW daemon is already running.\n\
             Socket: {}\n\n\
             To stop the existing daemon, run:\n  \
             pwsw shutdown",
            socket_path.display()
        );
    }

    // Check for stale PID file and clean it up if process is not running
    // This must happen after IPC check to avoid race conditions
    if check_and_cleanup_stale_pid_file()? {
        // A running daemon was detected via PID file but not via IPC
        // This shouldn't happen, but handle it gracefully
        warn!(
            "PID file indicates running daemon, but IPC check failed. Daemon may be starting up or in bad state."
        );
    }

    // Background mode: spawn detached process and wait for successful startup
    if !foreground && std::env::var("PWSW_DAEMON_CHILD").is_err() {
        // We're the initial process - spawn a background child
        let exe = std::env::current_exe()?;

        // Spawn detached daemon process WITHOUT --foreground so it logs to file
        // Pass environment variable to prevent child from spawning another process
        let mut child = Command::new(&exe)
            .arg("daemon")
            .env("PWSW_DAEMON_CHILD", "1")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .with_context(|| {
                format!(
                    "Failed to spawn background daemon: exe={}, working_dir={}",
                    exe.display(),
                    std::env::current_dir()
                        .ok()
                        .as_ref()
                        .map(|p| p.display().to_string())
                        .unwrap_or_else(|| "unknown".to_string())
                )
            })?;

        let pid = child.id();
        println!("Starting daemon (PID: {})...", pid.to_string().technical());

        // Wait for daemon to initialize by checking if it's responding to IPC
        // Try for up to 2 seconds (20 attempts * 100ms)
        const MAX_ATTEMPTS: u32 = 20;
        const RETRY_DELAY_MS: u64 = 100;

        for attempt in 1..=MAX_ATTEMPTS {
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

            // Check if child process is still alive (every 500ms)
            if attempt % 5 == 0 {
                match child.try_wait() {
                    Ok(Some(status)) => {
                        eyre::bail!(
                            "Daemon process (PID: {pid}) exited during startup with status: {status}\n\
                             Check logs with: journalctl --user -xe | grep pwsw"
                        );
                    }
                    Ok(None) => {} // Still running
                    Err(e) => {
                        warn!("Could not check daemon process status: {}", e);
                    }
                }
            }
        }

        eyre::bail!(
            "Daemon failed to start within {} seconds.\n\
             Process may still be initializing. Check with: pwsw status",
            (u64::from(MAX_ATTEMPTS) * RETRY_DELAY_MS) / 1000
        );
    }

    // Validate required PipeWire tools are available
    PipeWire::validate_tools().context("PipeWire tools validation failed")?;

    // Initialize logging with config log_level
    // Filter format: "pwsw=LEVEL" ensures only our crate logs at the configured level
    // At this point, we're either:
    // - Running with --foreground flag (user debugging)
    // - Running as spawned background child (foreground=false, PWSW_DAEMON_CHILD set)
    let filter = tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        tracing_subscriber::EnvFilter::new(format!("pwsw={}", config.settings.log_level))
    });

    // Always write to log file for TUI integration
    let log_path = get_log_file_path()?;
    let log_dir = log_path
        .parent()
        .context("Log file path has no parent directory")?;

    std::fs::create_dir_all(log_dir)
        .with_context(|| format!("Failed to create log directory: {}", log_dir.display()))?;

    let file_appender = tracing_appender::rolling::never(log_dir, "daemon.log");
    let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);

    if foreground {
        // Foreground mode (systemd or user debugging): log to BOTH stderr and file
        // This ensures TUI log viewer works with systemd, and journalctl captures logs
        use tracing_subscriber::Layer;
        use tracing_subscriber::layer::SubscriberExt;
        use tracing_subscriber::util::SubscriberInitExt;

        let stderr_layer = tracing_subscriber::fmt::layer().with_filter(filter.clone());
        let file_layer = tracing_subscriber::fmt::layer()
            .with_writer(non_blocking)
            .with_ansi(false) // No ANSI colors in log file
            .with_filter(filter);

        tracing_subscriber::registry()
            .with(stderr_layer)
            .with(file_layer)
            .init();
    } else {
        // Background daemon (spawned child): log to file only
        tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_writer(non_blocking)
            .with_ansi(false) // No ANSI colors in log file
            .init();
    }

    // Store guard to prevent it from being dropped (would close log file)
    // SAFETY: Guard must live for entire daemon lifetime
    std::mem::forget(guard);

    // Write PID file now that daemon is fully initialized
    write_pid_file()?;

    info!("Starting PWSW daemon {}", crate::version_string());
    info!(
        "Loaded {} sinks, {} rules",
        config.sinks.len(),
        config.rules.len()
    );

    let start_time = Instant::now();

    // Detect how the daemon is being managed (systemd vs direct)
    let daemon_manager = crate::daemon_manager::DaemonManager::detect();
    info!("Daemon manager: {:?}", daemon_manager);

    let mut state = State::new(config, daemon_manager)?;

    // Create shutdown channel with larger buffer to handle concurrent subscribers
    let (shutdown_tx, mut shutdown_rx) = broadcast::channel::<()>(8);

    // Switch to default on startup if configured
    if state.config.settings.default_on_startup {
        let default = state
            .config
            .get_default_sink()
            .ok_or_else(|| eyre::eyre!("No default sink configured"))?;
        if state.current_sink_name != default.name {
            info!("Switching to default sink: {}", default.desc);

            // Run activation in blocking thread pool to avoid blocking the async runtime
            let name_clone = default.name.clone();
            let desc_clone = default.desc.clone();
            let join = tokio::task::spawn_blocking(move || {
                crate::state::switch_audio_blocking(&name_clone, &desc_clone, None, None, false)
            });

            let inner = join.await.map_err(|e| eyre::eyre!("Join error: {e:#}"))?;
            inner?;

            state.current_sink_name.clone_from(&default.name);
        }
    }

    // Spawn compositor event thread
    let mut window_events = compositor::spawn_compositor_thread()?;
    info!("Compositor event thread started");

    // Start IPC server
    let ipc_server = IpcServer::bind().await?;
    info!("IPC server listening on {:?}", ipc_server.socket_path());

    // Setup config file watcher (hot-reload)
    let config_path = Config::get_config_path()?;
    let config_dir = config_path.parent().unwrap_or(&config_path);
    let (config_tx, mut config_rx) = tokio::sync::mpsc::channel::<()>(1);
    let config_tx_clone = config_tx.clone();

    // Only notify reloads for changes to the exact config file and avoid blocking the
    // watcher thread by using `try_send` (channel capacity 1 coalesces rapid events).
    let config_path_clone = config_path.clone();
    let mut watcher = notify::recommended_watcher(move |res: Result<Event, notify::Error>| {
        match res {
            Ok(event) => {
                if (event.kind.is_modify() || event.kind.is_create())
                    && event.paths.iter().any(|p| p == &config_path_clone)
                {
                    // Don't block the watcher thread; if the channel is full, drop the event
                    let _ = config_tx_clone.try_send(());
                }
            }
            Err(e) => error!("Config watch error: {:?}", e),
        }
    })?;

    if let Err(e) = watcher.watch(config_dir, RecursiveMode::NonRecursive) {
        warn!("Failed to watch config directory for hot-reload: {}", e);
    }

    if state.config.settings.notify_manual
        && let Err(e) =
            send_notification(NOTIFICATION_STARTED_TITLE, NOTIFICATION_STARTED_MSG, None)
    {
        warn!("Could not send startup notification: {}", e);
    }

    // Setup signal handlers for graceful shutdown
    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .expect("Failed to install SIGTERM handler");

    // Notify systemd that daemon is ready
    #[cfg(unix)]
    {
        if let Ok(true) = sd_notify::booted()
            && let Err(e) = sd_notify::notify(true, &[sd_notify::NotifyState::Ready])
        {
            warn!("Failed to notify systemd: {}", e);
        } else if let Ok(true) = sd_notify::booted() {
            info!("Notified systemd that daemon is ready");
        }
    }

    info!("Daemon initialization complete, entering event loop");

    // Main event loop
    loop {
        tokio::select! {
            result = window_events.recv() => {
                let Some(event) = result else {
                    error!("Compositor connection lost (event channel closed)");
                    break;
                };

                if let Err(e) = state.process_event(event).await {
                    error!("Event processing error: {e:#}", e = e);
                }
            }

            Some(mut stream) = ipc_server.accept() => {
                // Handle IPC request - clone what we need for the task
                // Tracked windows: (id, app_id, title, sink_name, sink_desc)
                let tracked_with_sinks = state.get_tracked_windows_with_sinks();
                // All windows: (id, app_id, title)
                let all_windows = state.get_all_windows();

                let ctx = IpcContext {
                    version: crate::version_string(),
                    uptime_secs: start_time.elapsed().as_secs(),
                    current_sink_name: state.current_sink_name.clone(),
                    active_window: state.get_most_recent_window()
                        .map(|w| format!("{}: {}", w.trigger_desc, w.sink_name)),
                    daemon_manager: state.daemon_manager,
                    tracked_with_sinks,
                    all_windows,
                    config: state.config.clone(),
                    shutdown_tx: shutdown_tx.clone(),
                };

                tokio::spawn(async move {
                    if let Err(e) = handle_ipc_request(&mut stream, ctx).await {
                        // Check if this is a benign health check connection (client disconnects before sending data)
                        let is_health_check = e.chain()
                            .any(|cause| {
                                cause.downcast_ref::<std::io::Error>()
                                    .is_some_and(|io_err| io_err.kind() == std::io::ErrorKind::UnexpectedEof)
                            });

                        if !is_health_check {
                            // Don't log health checks (TUI polls every 700ms)
                            error!("IPC request handling error: {e:#}", e = e);
                        }
                    }
                });
            }

            _ = config_rx.recv() => {
                // Debounce happens partly due to select! loop speed, but we should be careful.
                info!("Config file changed, attempting reload...");
                match Config::load() {
                    Ok(new_config) => {
                        let notify_enabled = state.config.settings.notify_manual;
                        state.reload_config(new_config);

                        // Re-evaluate all active windows against new rules
                        if let Err(e) = state.reevaluate_all_windows().await {
                            error!("Failed to re-evaluate windows after config reload: {e:#}", e = e);
                        }

                        if notify_enabled {
                            let _ = send_notification("Configuration Reloaded", "New settings applied successfully", None);
                        }
                    },
                    Err(e) => {
                        error!("Failed to reload config: {e:#}", e = e);
                        if state.config.settings.notify_manual {
                            let _ = send_notification("Reload Failed", &format!("Config error: {e:#}"), None);
                        }
                    }
                }
            }

            _ = signal::ctrl_c() => {
                info!("Shutting down (Ctrl-C)");
                if state.config.settings.notify_manual {
                    let _ = send_notification(NOTIFICATION_STOPPED_TITLE, NOTIFICATION_STOPPED_MSG, None);
                }
                break;
            }

            _ = sigterm.recv() => {
                info!("Shutting down (SIGTERM from systemd)");
                if state.config.settings.notify_manual {
                    let _ = send_notification(NOTIFICATION_STOPPED_TITLE, NOTIFICATION_STOPPED_MSG, None);
                }
                break;
            }

            _ = shutdown_rx.recv() => {
                info!("Shutting down (IPC request)");
                if state.config.settings.notify_manual {
                    let _ = send_notification(NOTIFICATION_STOPPED_TITLE, NOTIFICATION_STOPPED_MSG, None);
                }
                break;
            }
        }
    }

    // Cleanup PID file on shutdown
    remove_pid_file();

    Ok(())
}

/// Handle a single IPC request from a client
// IPC request handler - cohesive dispatch logic for all request types
async fn handle_ipc_request(stream: &mut tokio::net::UnixStream, ctx: IpcContext) -> Result<()> {
    let request = ipc::read_request(stream).await?;

    let response = match request {
        Request::Status => {
            // Get current sink description
            let current_sink = ctx
                .config
                .sinks
                .iter()
                .find(|s| s.name == ctx.current_sink_name)
                .map_or_else(|| ctx.current_sink_name.clone(), |s| s.desc.clone());

            Response::Status {
                version: ctx.version,
                uptime_secs: ctx.uptime_secs,
                current_sink,
                active_window: ctx.active_window,
                tracked_windows: ctx.tracked_with_sinks.len(),
            }
        }

        Request::ListWindows => {
            use std::collections::HashMap;

            // Build a map of tracked windows by id for quick lookup
            let tracked_map: HashMap<u64, (String, String)> = ctx
                .tracked_with_sinks
                .iter()
                .map(|(id, _app_id, _title, sink_name, sink_desc)| {
                    (*id, (sink_name.clone(), sink_desc.clone()))
                })
                .collect();

            // Build WindowInfo for all windows with tracking status using ids from all_windows
            let windows = ctx
                .all_windows
                .iter()
                .map(|(id, app_id, title)| {
                    // Find tracked info by id
                    let tracked_opt =
                        tracked_map
                            .get(id)
                            .map(|(sink_name, sink_desc)| ipc::TrackedInfo {
                                sink_name: sink_name.clone(),
                                sink_desc: sink_desc.clone(),
                            });

                    WindowInfo {
                        id: Some(*id),
                        app_id: app_id.clone(),
                        title: title.clone(),
                        matched_on: None,
                        tracked: tracked_opt,
                    }
                })
                .collect();

            Response::Windows { windows }
        }

        Request::TestRule { pattern } => match regex::Regex::new(&pattern) {
            Ok(regex) => {
                let matches = ctx
                    .all_windows
                    .iter()
                    .filter_map(|(id, app_id, title)| {
                        let app_id_match = regex.is_match(app_id);
                        let title_match = regex.is_match(title);

                        if app_id_match || title_match {
                            let matched_on = match (app_id_match, title_match) {
                                (true, true) => "both",
                                (true, false) => "app_id",
                                (false, true) => "title",
                                (false, false) => unreachable!("Already filtered by outer if"),
                            };

                            Some(WindowInfo {
                                id: Some(*id),
                                app_id: app_id.clone(),
                                title: title.clone(),
                                matched_on: Some(matched_on.to_string()),
                                tracked: None,
                            })
                        } else {
                            None
                        }
                    })
                    .collect();

                Response::RuleMatches { pattern, matches }
            }
            Err(e) => Response::Error {
                message: format!("Invalid regex pattern: {e}"),
            },
        },

        Request::GetManagerInfo => Response::ManagerInfo {
            daemon_manager: ctx.daemon_manager,
        },

        Request::SetSink { sink } => {
            // Resolve sink reference (name, description, or 1-indexed position)
            if let Some(target) = ctx.config.resolve_sink(&sink) {
                // Attempt to activate the sink via PipeWire
                match PipeWire::activate_sink(&target.name) {
                    Ok(()) => Response::Ok {
                        message: format!("Switched to sink: {}", target.desc),
                    },
                    Err(e) => Response::Error {
                        message: format!(
                            "Failed to activate sink '{target_desc}': {e:#}",
                            target_desc = target.desc,
                            e = e
                        ),
                    },
                }
            } else {
                // Build helpful error message with available sinks
                let available: Vec<_> = ctx
                    .config
                    .sinks
                    .iter()
                    .enumerate()
                    .map(|(i, s)| format!("{}. '{}'", i + 1, s.desc))
                    .collect();
                Response::Error {
                    message: format!(
                        "Unknown sink '{}'. Available: [{}]",
                        sink,
                        available.join(", ")
                    ),
                }
            }
        }

        Request::Shutdown => {
            info!("Shutdown requested via IPC");
            // Send response before shutting down
            ipc::write_response(
                stream,
                &Response::Ok {
                    message: "Daemon shutting down...".to_string(),
                },
            )
            .await?;

            // Signal shutdown to main loop
            let _ = ctx.shutdown_tx.send(());

            // Return without sending another response
            return Ok(());
        }
    };

    ipc::write_response(stream, &response).await?;
    Ok(())
}
