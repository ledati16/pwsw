//! Daemon manager type detection
//!
//! Determines whether the daemon is running under systemd supervision or directly.

use std::io::Read;
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

/// What a command run by [`output_with_timeout`] printed, and how it exited
#[derive(Debug, Clone)]
pub struct CommandOutput {
    pub status: ExitStatus,
    pub stdout: String,
    pub stderr: String,
}

/// Run `cmd` (stdin on `/dev/null`) capturing its output, waiting at most `timeout`.
///
/// For commands with short output (a few lines, like `systemctl --user is-active`):
/// the output is read after the command exited. Returns `None` if it cannot be
/// spawned or does not finish in time (it is then killed and reaped). Blocking.
#[must_use]
pub fn output_with_timeout(mut cmd: Command, timeout: Duration) -> Option<CommandOutput> {
    const POLL: Duration = Duration::from_millis(10);

    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(POLL),
            Ok(None) | Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    };
    Some(CommandOutput {
        status,
        stdout: read_pipe(child.stdout.take()),
        stderr: read_pipe(child.stderr.take()),
    })
}

/// The text left in a finished child's output pipe (at most 64 KiB, lossy UTF-8)
fn read_pipe(pipe: Option<impl Read>) -> String {
    let mut bytes = Vec::new();
    if let Some(pipe) = pipe {
        let _ = pipe.take(64 * 1024).read_to_end(&mut bytes);
    }
    String::from_utf8_lossy(&bytes).into_owned()
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

    /// How long the "slow" test commands would run if they were not killed
    const SLOW_SECS: u64 = 600;

    /// A command that writes its process id to `pid_file`, then runs for
    /// [`SLOW_SECS`] (unless killed)
    fn slow_command(pid_file: &std::path::Path) -> Command {
        let mut slow = Command::new("sh");
        slow.arg("-c")
            .arg(format!("echo $$ > \"$1\"; exec sleep {SLOW_SECS}"))
            .arg("sh")
            .arg(pid_file);
        slow
    }

    /// The process `slow_command` started is gone: killed and reaped (a zombie would
    /// still have its `/proc` entry)
    fn assert_gone(pid_file: &std::path::Path) {
        let pid = std::fs::read_to_string(pid_file).expect("the command wrote its pid");
        let pid = pid.trim();
        assert!(!pid.is_empty(), "no pid written");
        assert!(
            !std::path::Path::new(&format!("/proc/{pid}")).exists(),
            "process {pid} still exists"
        );
    }

    /// Whether the command wrote its pid (it may be killed before it gets to)
    fn pid_written(pid_file: &std::path::Path) -> bool {
        std::fs::read_to_string(pid_file).is_ok_and(|p| p.ends_with('\n'))
    }

    #[test]
    fn test_status_with_timeout() {
        let ok = status_with_timeout(Command::new("true"), Duration::from_secs(5));
        assert!(ok.is_some_and(|s| s.success()), "{ok:?}");
        let failed = status_with_timeout(Command::new("false"), Duration::from_secs(5));
        assert_eq!(failed.and_then(|s| s.code()), Some(1));

        // A timeout long enough for the shell to write its pid on a loaded machine; the
        // command itself would run for ten minutes: returning at all means it was killed
        let dir = tempfile::tempdir().expect("tempdir");
        let pid_file = dir.path().join("pid");
        let start = Instant::now();
        let status = status_with_timeout(slow_command(&pid_file), Duration::from_secs(2));
        assert!(status.is_none(), "timed out, got {status:?}");
        assert!(start.elapsed() < Duration::from_secs(SLOW_SECS / 2));
        if pid_written(&pid_file) {
            assert_gone(&pid_file);
        }

        let missing = status_with_timeout(
            Command::new("/nonexistent/pwsw-test-binary"),
            Duration::from_secs(1),
        );
        assert!(missing.is_none(), "cannot spawn, got {missing:?}");
    }

    #[test]
    fn test_output_with_timeout() {
        let mut echo = Command::new("sh");
        echo.args(["-c", "echo out; echo err >&2; exit 3"]);
        let output = output_with_timeout(echo, Duration::from_secs(5)).expect("ran");
        assert_eq!(output.status.code(), Some(3));
        assert_eq!(output.stdout, "out\n");
        assert_eq!(output.stderr, "err\n");

        let dir = tempfile::tempdir().expect("tempdir");
        let pid_file = dir.path().join("pid");
        let start = Instant::now();
        let output = output_with_timeout(slow_command(&pid_file), Duration::from_secs(2));
        assert!(output.is_none(), "timed out, got {output:?}");
        assert!(start.elapsed() < Duration::from_secs(SLOW_SECS / 2));
        if pid_written(&pid_file) {
            assert_gone(&pid_file);
        }
    }
}
