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
        // Check if the service unit is loaded (exists in systemd)
        // LoadState will be "loaded" if the service exists, "not-found" if it doesn't
        let output = Command::new("systemctl")
            .args(["--user", "show", "pwsw.service", "--property=LoadState"])
            .output();

        if let Ok(output) = output {
            if output.status.success() {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let is_loaded = stdout.contains("LoadState=loaded");
                eprintln!("DEBUG: systemctl output: {:?}, is_loaded: {}", stdout.trim(), is_loaded);
                return is_loaded;
            }
        }
        eprintln!("DEBUG: systemctl command failed or didn't succeed");
        false
    }
}
