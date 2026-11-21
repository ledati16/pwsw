//! Daemon control integration - systemd with fallback to direct execution

use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

use crate::daemon_manager::{DaemonManager, SYSTEMD_UNIT, status_with_timeout};
use crate::ipc;
use crate::tui::worker::detached;

/// How long the old daemon may take to release its socket during a restart.
const RESTART_EXIT_TIMEOUT: Duration = Duration::from_secs(5);

/// How long a cached `systemctl is-enabled` answer is reused by the poll loop.
const IS_ENABLED_CACHE_TTL: Duration = Duration::from_secs(10);

/// Timeout for `systemctl --user is-enabled`.
const IS_ENABLED_TIMEOUT: Duration = Duration::from_secs(2);

/// Last `is-enabled` answer and when it was obtained (the `TUI` polls often;
/// spawning `systemctl` each time is wasteful).
static IS_ENABLED_CACHE: Mutex<Option<(Instant, bool)>> = Mutex::new(None);

fn cached_is_enabled() -> Option<bool> {
    let guard = IS_ENABLED_CACHE.lock().ok()?;
    guard.and_then(|(at, value)| (at.elapsed() < IS_ENABLED_CACHE_TTL).then_some(value))
}

fn store_is_enabled(value: bool) {
    if let Ok(mut guard) = IS_ENABLED_CACHE.lock() {
        *guard = Some((Instant::now(), value));
    }
}

/// How long a cached "unit installed" answer is reused by the poll loop.
const UNIT_INSTALLED_CACHE_TTL: Duration = Duration::from_secs(10);

/// Last "unit installed" answer and when it was obtained.
static UNIT_INSTALLED_CACHE: Mutex<Option<(Instant, bool)>> = Mutex::new(None);

impl DaemonManager {
    /// Decide how the `TUI` should control the daemon (start/stop/restart).
    ///
    /// A running daemon reports how it was started over `IPC`; that answer wins.
    /// Only when no daemon answers does the installed unit decide: if
    /// `pwsw.service` is installed, start it via systemd, otherwise run it directly.
    pub async fn for_control() -> Self {
        if let Ok(ipc::Response::ManagerInfo { daemon_manager }) =
            ipc::send_request(ipc::Request::GetManagerInfo).await
        {
            return daemon_manager;
        }
        Self::for_unit_management().await
    }

    /// [`DaemonManager::Systemd`] if the `pwsw.service` unit file is installed,
    /// else [`DaemonManager::Direct`]. Used for enable/disable, which act on the
    /// unit regardless of how a currently running daemon was started.
    pub async fn for_unit_management() -> Self {
        let installed = detached(Self::unit_installed).await.unwrap_or(false);
        if installed {
            Self::Systemd
        } else {
            Self::Direct
        }
    }

    /// Whether the `pwsw.service` unit file is installed, cached for a few
    /// seconds (the `TUI` asks on every status poll).
    pub async fn unit_installed_cached() -> bool {
        let cached = UNIT_INSTALLED_CACHE.lock().ok().and_then(|guard| {
            guard.and_then(|(at, value)| (at.elapsed() < UNIT_INSTALLED_CACHE_TTL).then_some(value))
        });
        if let Some(value) = cached {
            return value;
        }
        let value = detached(Self::unit_installed).await.unwrap_or(false);
        if let Ok(mut guard) = UNIT_INSTALLED_CACHE.lock() {
            *guard = Some((Instant::now(), value));
        }
        value
    }

    /// Start the daemon
    ///
    /// # Errors
    /// Returns an error if the daemon fails to start.
    pub async fn start(self) -> Result<String> {
        match self {
            Self::Systemd => {
                let output = detached(move || {
                    Command::new("systemctl")
                        .args(["--user", "start", "pwsw.service"])
                        .output()
                })
                .await?
                .context("Failed to execute systemctl start")?;

                if output.status.success() {
                    // With Type=notify, systemd waits for ready signal
                    Ok("Daemon started via systemd".to_string())
                } else {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    anyhow::bail!("systemctl start failed: {stderr}");
                }
            }
            Self::Direct => {
                // Run `pwsw daemon`: it spawns the detached daemon, waits until it
                // answers on the socket and exits non-zero (with the reason on
                // stderr) if start-up fails. Running it to completion also reaps it.
                let pwsw_path = detached(std::env::current_exe)
                    .await?
                    .context("Failed to get current executable path")?;

                // Forward an explicit config selection so the daemon uses the same file
                let config_override = crate::config::Config::config_path_override()?;

                let output = detached(move || {
                    let mut cmd = Command::new(&pwsw_path);
                    if let Some(config_path) = config_override {
                        cmd.arg("--config").arg(config_path);
                    }
                    cmd.arg("daemon").stdin(Stdio::null()).output()
                })
                .await?
                .context("Failed to run `pwsw daemon`")?;

                if output.status.success() {
                    Ok("Daemon started directly".to_string())
                } else {
                    anyhow::bail!(
                        "`pwsw daemon` failed ({}): {}",
                        output.status,
                        summarize_stderr(&output.stderr)
                    );
                }
            }
        }
    }

    /// Stop the daemon
    ///
    /// # Errors
    /// Returns an error if the daemon fails to stop.
    pub async fn stop(self) -> Result<String> {
        match self {
            Self::Systemd => {
                let output = detached(move || {
                    Command::new("systemctl")
                        .args(["--user", "stop", "pwsw.service"])
                        .output()
                })
                .await?
                .context("Failed to execute systemctl stop")?;

                if output.status.success() {
                    Ok("Daemon stopped via systemd".to_string())
                } else {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    anyhow::bail!("systemctl stop failed: {stderr}");
                }
            }
            Self::Direct => {
                // Send shutdown request via IPC
                match ipc::send_request(ipc::Request::Shutdown).await {
                    Ok(_) => Ok("Daemon shutdown requested".to_string()),
                    Err(e) => anyhow::bail!("Failed to send shutdown request: {e:#}"),
                }
            }
        }
    }

    /// Restart the daemon
    ///
    /// # Errors
    /// Returns an error if the daemon fails to restart.
    pub async fn restart(self) -> Result<String> {
        match self {
            Self::Systemd => {
                let output = detached(move || {
                    Command::new("systemctl")
                        .args(["--user", "restart", "pwsw.service"])
                        .output()
                })
                .await?
                .context("Failed to execute systemctl restart")?;

                if output.status.success() {
                    // With Type=notify, systemd waits for ready signal
                    Ok("Daemon restarted via systemd".to_string())
                } else {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    anyhow::bail!("systemctl restart failed: {stderr}");
                }
            }
            Self::Direct => {
                // Stop, wait for the old daemon to release its socket, then start
                self.stop().await?;
                ipc::wait_for_daemon_exit(RESTART_EXIT_TIMEOUT)
                    .await
                    .context("Old daemon did not exit")?;
                self.start().await
            }
        }
    }

    /// Enable the daemon service (only supported for systemd)
    ///
    /// # Errors
    /// Returns an error if the service fails to enable or if using direct mode.
    pub async fn enable(self) -> Result<String> {
        match self {
            Self::Systemd => {
                let output = detached(move || {
                    Command::new("systemctl")
                        .args(["--user", "enable", "pwsw.service"])
                        .output()
                })
                .await?
                .context("Failed to execute systemctl enable")?;

                if output.status.success() {
                    store_is_enabled(true);
                    Ok("Service enabled (will start on login)".to_string())
                } else {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    anyhow::bail!("systemctl enable failed: {stderr}");
                }
            }
            Self::Direct => {
                anyhow::bail!("Enable/disable only supported with systemd service")
            }
        }
    }

    /// Disable the daemon service (only supported for systemd)
    ///
    /// # Errors
    /// Returns an error if the service fails to disable or if using direct mode.
    pub async fn disable(self) -> Result<String> {
        match self {
            Self::Systemd => {
                let output = detached(move || {
                    Command::new("systemctl")
                        .args(["--user", "disable", "pwsw.service"])
                        .output()
                })
                .await?
                .context("Failed to execute systemctl disable")?;

                if output.status.success() {
                    store_is_enabled(false);
                    Ok("Service disabled (won't start on login)".to_string())
                } else {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    anyhow::bail!("systemctl disable failed: {stderr}");
                }
            }
            Self::Direct => {
                anyhow::bail!("Enable/disable only supported with systemd service")
            }
        }
    }

    /// Check if the daemon service is enabled (only for systemd)
    ///
    /// The answer is cached for a few seconds (and updated by
    /// [`DaemonManager::enable`]/[`DaemonManager::disable`]) because the `TUI`
    /// polls it on every refresh.
    pub async fn is_enabled(self) -> bool {
        match self {
            Self::Systemd => {
                if let Some(value) = cached_is_enabled() {
                    return value;
                }
                let value = detached(|| {
                    let mut cmd = Command::new("systemctl");
                    cmd.args(["--user", "is-enabled", SYSTEMD_UNIT]);
                    status_with_timeout(cmd, IS_ENABLED_TIMEOUT).is_some_and(|s| s.success())
                })
                .await
                .unwrap_or(false);
                store_is_enabled(value);
                value
            }
            Self::Direct => false,
        }
    }
}

/// Condense a child's stderr (an `error: cause: cause` line, or with backtraces
/// requested `anyhow`'s full report) into one line for the `TUI` status bar: strip
/// `ANSI` escapes, drop blank lines and the backtrace, join the rest with `" | "`.
fn summarize_stderr(stderr: &[u8]) -> String {
    const MAX_CHARS: usize = 400;

    let text = crate::sanitize::strip_ansi(&String::from_utf8_lossy(stderr));
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .take_while(|l| !l.starts_with("Stack backtrace:"))
        .collect();
    if lines.is_empty() {
        return "no error output".to_string();
    }
    crate::sanitize::sanitize_line(&lines.join(" | "), MAX_CHARS).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_summarize_stderr_error_line() {
        let line = "\u{1b}[31merror:\u{1b}[0m Failed to load config: No sinks configured\n";
        assert_eq!(
            summarize_stderr(line.as_bytes()),
            "error: Failed to load config: No sinks configured"
        );
    }

    #[test]
    fn test_summarize_stderr_full_report() {
        let report = "Error: Failed to load config\n\nCaused by:\n    No sinks configured\n\n\
                      Stack backtrace:\n   0: anyhow::error::<impl anyhow::Error>::msg\n";
        assert_eq!(
            summarize_stderr(report.as_bytes()),
            "Error: Failed to load config | Caused by: | No sinks configured"
        );
    }

    #[test]
    fn test_summarize_stderr_empty() {
        assert_eq!(summarize_stderr(b"\n  \n"), "no error output");
    }
}
