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
    /// Checks if systemd user session is available AND `pwsw.service` exists.
    /// Falls back to direct execution otherwise.
    ///
    /// This should be called once at daemon startup to determine how the daemon
    /// was started. The TUI queries this information via IPC rather than detecting
    /// independently.
    #[must_use]
    pub fn detect() -> Self {
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
