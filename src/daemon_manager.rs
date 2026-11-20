//! Daemon manager type detection
//!
//! Determines whether the daemon is running under systemd supervision or directly.

use std::process::{Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

/// Name of the systemd user unit shipped in `contrib/`.
pub const SYSTEMD_UNIT: &str = "pwsw.service";

/// How long a `systemctl` query may take before it is abandoned.
const SYSTEMCTL_QUERY_TIMEOUT: Duration = Duration::from_secs(2);

/// How the daemon is being managed
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum DaemonManager {
    /// Managed by systemd user service (systemctl --user)
    Systemd,
    /// Direct execution (not under systemd)
    Direct,
}

impl DaemonManager {
    /// Detect how the *current process* is managed.
    ///
    /// Returns [`DaemonManager::Systemd`] only if this process lives in the
    /// `pwsw.service` cgroup (read from `/proc/self/cgroup`). Called once at
    /// daemon startup; the `TUI` learns the answer over `IPC`.
    ///
    /// `INVOCATION_ID` is deliberately not used: every process of a
    /// systemd-launched graphical session inherits it, so a daemon started from
    /// a terminal would wrongly claim to be supervised (and the `TUI` would then
    /// run `systemctl --user stop` on an inactive unit, leaving it running).
    #[must_use]
    pub fn detect() -> Self {
        match std::fs::read_to_string("/proc/self/cgroup") {
            Ok(content) if cgroup_is_unit(&content, SYSTEMD_UNIT) => Self::Systemd,
            _ => Self::Direct,
        }
    }

    /// Check whether the `pwsw.service` user unit file is installed.
    ///
    /// Uses `systemctl --user cat`, which fails if the unit file does not exist
    /// even when systemd still has stale state cached for it. Gives up (returns
    /// `false`) after a short timeout so an unresponsive user manager cannot
    /// hang the caller. Blocking: call from `spawn_blocking` in async code.
    #[must_use]
    pub fn unit_installed() -> bool {
        let mut cmd = Command::new("systemctl");
        cmd.args(["--user", "cat", SYSTEMD_UNIT]);
        status_with_timeout(cmd, SYSTEMCTL_QUERY_TIMEOUT).is_some_and(|s| s.success())
    }
}

/// Whether any cgroup path in a `/proc/<pid>/cgroup` listing has `unit` as one of
/// its components.
///
/// Handles both the cgroup v2 single line (`0::/user.slice/…/pwsw.service`) and
/// cgroup v1 multi-line listings (`N:controllers:/path`).
fn cgroup_is_unit(content: &str, unit: &str) -> bool {
    content
        .lines()
        .filter_map(|line| line.splitn(3, ':').nth(2))
        .any(|path| path.split('/').any(|component| component == unit))
}

/// Run `cmd` with all standard streams on `/dev/null`, waiting at most `timeout`.
///
/// Returns `None` if it cannot be spawned or does not finish in time (it is then
/// killed and reaped). Blocking.
#[must_use]
pub fn status_with_timeout(mut cmd: Command, timeout: Duration) -> Option<ExitStatus> {
    const POLL: Duration = Duration::from_millis(10);

    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Some(status),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(POLL),
            Ok(None) | Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use test_case::test_case;

    use super::*;

    #[test_case("0::/user.slice/user-1000.slice/user@1000.service/app.slice/pwsw.service\n", true ; "v2 in unit")]
    #[test_case("0::/user.slice/user-1000.slice/user@1000.service/session.slice/foo.scope\n", false ; "v2 elsewhere")]
    #[test_case("0::/user.slice/user-1000.slice/user@1000.service/app.slice/app-foot-pwsw.service.scope\n", false ; "v2 lookalike")]
    #[test_case("0::/user.slice/user-1000.slice/user@1000.service/app.slice/pwsw.service/sub\n", true ; "v2 sub-cgroup")]
    #[test_case("12:pids:/user.slice\n1:name=systemd:/user.slice/user@1000.service/pwsw.service\n0::/\n", true ; "v1 hybrid")]
    #[test_case("", false ; "empty")]
    #[test_case("garbage without colons", false ; "garbage")]
    fn test_cgroup_is_unit(content: &str, expected: bool) {
        assert_eq!(cgroup_is_unit(content, SYSTEMD_UNIT), expected);
    }

    #[test]
    fn test_status_with_timeout() {
        let ok = status_with_timeout(Command::new("true"), Duration::from_secs(5));
        assert!(ok.is_some_and(|s| s.success()));

        let mut slow = Command::new("sleep");
        slow.arg("5");
        let start = Instant::now();
        assert!(status_with_timeout(slow, Duration::from_millis(100)).is_none());
        assert!(
            start.elapsed() < Duration::from_secs(4),
            "child must be killed"
        );

        assert!(
            status_with_timeout(
                Command::new("/nonexistent/pwsw-test-binary"),
                Duration::from_secs(1)
            )
            .is_none()
        );
    }
}
