//! Daemon manager type detection
//!
//! Determines whether the daemon is running under systemd supervision or directly.

use std::process::{Command, Stdio};

/// How the daemon is being managed
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum DaemonManager {
    /// Managed by systemd user service (systemctl --user)
    Systemd,
    /// Direct execution (not under systemd)
    Direct,
}

impl DaemonManager {
    /// Detect which daemon manager is in use
    ///
    /// Checks if running under systemd supervision by examining the `INVOCATION_ID`
    /// environment variable (set by systemd for all supervised processes).
    /// Falls back to checking if `pwsw.service` exists for compatibility.
    ///
    /// This should be called once at daemon startup to determine how the daemon
    /// was started. The TUI queries this information via IPC rather than detecting
    /// independently.
    #[must_use]
    pub fn detect() -> Self {
        // Method 1: Check if running under systemd supervision (most reliable)
        // systemd sets INVOCATION_ID for all supervised processes
        if std::env::var("INVOCATION_ID").is_ok() {
            return Self::Systemd;
        }

        // Method 2: Fallback - check if service is installed
        // This handles detection from TUI/CLI when daemon isn't running yet
        if Self::check_systemd_available() {
            Self::Systemd
        } else {
            Self::Direct
        }
    }

    /// Check if systemd user service is available and `pwsw.service` exists
    fn check_systemd_available() -> bool {
        Command::new("systemctl")
            .args(["--user", "cat", "pwsw.service"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    }
}
