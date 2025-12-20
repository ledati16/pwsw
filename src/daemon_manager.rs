//! Daemon manager type detection
//!
//! Determines whether the daemon is running under systemd supervision or directly.

use std::process::Command;

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
    /// Checks if the `pwsw.service` systemd unit file actually exists (using
    /// `systemctl cat`), then verifies if the current process is running under
    /// systemd supervision via `INVOCATION_ID`.
    ///
    /// This should be called once at daemon startup to determine how the daemon
    /// was started. The TUI queries this information via IPC rather than detecting
    /// independently.
    ///
    /// # Detection Logic
    ///
    /// 1. Check if `pwsw.service` file exists (via `systemctl cat`)
    /// 2. If exists AND `INVOCATION_ID` is set, return Systemd (supervised)
    /// 3. Otherwise return Direct (manual start or no service file)
    ///
    /// Note: We use `systemctl cat` instead of checking `LoadState` to avoid false
    /// positives from systemd's cached state after service file deletion.
    /// `INVOCATION_ID` alone is unreliable - it's set for all processes in
    /// a systemd user session, not just supervised services.
    #[must_use]
    pub fn detect() -> Self {
        // First check if the service unit is installed
        let service_loaded = Self::check_systemd_available();

        if service_loaded {
            // Service exists - check if we're actually supervised by it
            // INVOCATION_ID is set by systemd for supervised processes, but also
            // for all processes in a user session, so we only trust it if the
            // service is loaded
            if std::env::var("INVOCATION_ID").is_ok() {
                return Self::Systemd;
            }
        }

        // Either no service installed or running outside systemd supervision
        Self::Direct
    }

    /// Check if systemd user service file actually exists
    fn check_systemd_available() -> bool {
        // Use `systemctl cat` to verify the service file actually exists
        // This avoids false positives from cached/stale systemd state after
        // service file deletion. `cat` will fail with non-zero exit if the
        // service file doesn't exist, even if systemd has it cached.
        Command::new("systemctl")
            .args(["--user", "cat", "pwsw.service"])
            .output()
            .is_ok_and(|output| output.status.success())
    }
}
