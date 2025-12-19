//! Daemon control integration - systemd with fallback to direct execution

use anyhow::{Context, Result};
use std::process::{Command, Stdio};

use crate::daemon_manager::DaemonManager;
use crate::ipc;

impl DaemonManager {
    /// Start the daemon
    ///
    /// # Errors
    /// Returns an error if the daemon fails to start.
    pub async fn start(self) -> Result<String> {
        match self {
            DaemonManager::Systemd => {
                let output = Command::new("systemctl")
                    .args(["--user", "start", "pwsw.service"])
                    .output()
                    .context("Failed to execute systemctl start")?;

                if output.status.success() {
                    // Wait a moment for daemon to start
                    tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
                    Ok("Daemon started via systemd".to_string())
                } else {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    anyhow::bail!("systemctl start failed: {stderr}");
                }
            }
            DaemonManager::Direct => {
                // Spawn daemon process in background
                let pwsw_path =
                    std::env::current_exe().context("Failed to get current executable path")?;

                Command::new(&pwsw_path)
                    .arg("daemon")
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn()
                    .context("Failed to spawn daemon process")?;

                // Wait a moment for daemon to start
                tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
                Ok("Daemon started directly".to_string())
            }
        }
    }

    /// Stop the daemon
    ///
    /// # Errors
    /// Returns an error if the daemon fails to stop.
    pub async fn stop(self) -> Result<String> {
        match self {
            DaemonManager::Systemd => {
                let output = Command::new("systemctl")
                    .args(["--user", "stop", "pwsw.service"])
                    .output()
                    .context("Failed to execute systemctl stop")?;

                if output.status.success() {
                    Ok("Daemon stopped via systemd".to_string())
                } else {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    anyhow::bail!("systemctl stop failed: {stderr}");
                }
            }
            DaemonManager::Direct => {
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
            DaemonManager::Systemd => {
                let output = Command::new("systemctl")
                    .args(["--user", "restart", "pwsw.service"])
                    .output()
                    .context("Failed to execute systemctl restart")?;

                if output.status.success() {
                    // Wait a moment for daemon to restart
                    tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
                    Ok("Daemon restarted via systemd".to_string())
                } else {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    anyhow::bail!("systemctl restart failed: {stderr}");
                }
            }
            DaemonManager::Direct => {
                // Stop then start
                self.stop().await?;
                tokio::time::sleep(tokio::time::Duration::from_millis(300)).await;
                self.start().await
            }
        }
    }

    /// Check if the daemon is currently running
    pub async fn is_running(self) -> bool {
        match self {
            DaemonManager::Systemd => {
                // Check systemd service status
                Command::new("systemctl")
                    .args(["--user", "is-active", "pwsw.service"])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status()
                    .is_ok_and(|status| status.success())
            }
            DaemonManager::Direct => {
                // Check via IPC
                ipc::is_daemon_running().await
            }
        }
    }

    /// Enable the daemon service (only supported for systemd)
    ///
    /// # Errors
    /// Returns an error if the service fails to enable or if using direct mode.
    pub fn enable(self) -> Result<String> {
        match self {
            DaemonManager::Systemd => {
                let output = Command::new("systemctl")
                    .args(["--user", "enable", "pwsw.service"])
                    .output()
                    .context("Failed to execute systemctl enable")?;

                if output.status.success() {
                    Ok("Service enabled (will start on login)".to_string())
                } else {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    anyhow::bail!("systemctl enable failed: {stderr}");
                }
            }
            DaemonManager::Direct => {
                anyhow::bail!("Enable/disable only supported with systemd service")
            }
        }
    }

    /// Disable the daemon service (only supported for systemd)
    ///
    /// # Errors
    /// Returns an error if the service fails to disable or if using direct mode.
    pub fn disable(self) -> Result<String> {
        match self {
            DaemonManager::Systemd => {
                let output = Command::new("systemctl")
                    .args(["--user", "disable", "pwsw.service"])
                    .output()
                    .context("Failed to execute systemctl disable")?;

                if output.status.success() {
                    Ok("Service disabled (won't start on login)".to_string())
                } else {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    anyhow::bail!("systemctl disable failed: {stderr}");
                }
            }
            DaemonManager::Direct => {
                anyhow::bail!("Enable/disable only supported with systemd service")
            }
        }
    }

    /// Check if the daemon service is enabled (only for systemd)
    #[must_use]
    pub fn is_enabled(self) -> bool {
        match self {
            DaemonManager::Systemd => Command::new("systemctl")
                .args(["--user", "is-enabled", "pwsw.service"])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .is_ok_and(|status| status.success()),
            DaemonManager::Direct => false,
        }
    }
}
