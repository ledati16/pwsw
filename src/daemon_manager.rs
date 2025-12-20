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
    /// Checks if the `pwsw.service` systemd unit is loaded, then verifies if the
    /// current process is running under systemd supervision via `INVOCATION_ID`.
    ///
    /// This should be called once at daemon startup to determine how the daemon
    /// was started. The TUI queries this information via IPC rather than detecting
    /// independently.
    ///
    /// # Detection Logic
    ///
    /// 1. Check if `pwsw.service` is loaded in systemd
    /// 2. If loaded AND `INVOCATION_ID` is set, return Systemd (supervised)
    /// 3. Otherwise return Direct (manual start or no service)
    ///
    /// Note: `INVOCATION_ID` alone is unreliable - it's set for all processes in
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

    /// Check if systemd user service is available and `pwsw.service` exists
    fn check_systemd_available() -> bool {
        // Check if the service unit is loaded (exists in systemd)
        // LoadState will be "loaded" if the service exists, "not-found" if it doesn't
        let output = Command::new("systemctl")
            .args(["--user", "show", "pwsw.service", "--property=LoadState"])
            .output();

        if let Ok(output) = output {
            if output.status.success() {
                let stdout = String::from_utf8_lossy(&output.stdout);
                return stdout.contains("LoadState=loaded");
            }
        }
        false
    }
}
