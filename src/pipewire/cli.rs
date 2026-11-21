//! Process execution for the `PipeWire` command-line tools. The only place in the
//! `pipewire` module that spawns processes.
//!
//! Every invocation has a hard deadline (the child is killed and reaped when it
//! passes), a cap on captured output, and an exit-status check.

use std::io::{self, Read};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use tracing::{debug, warn};

use super::activate::AudioBackend;
use super::model::Snapshot;
use super::parse::parse_dump;
use crate::config::Endpoint;

/// Deadline for `pw-dump` (a full graph dump on a busy system takes well under 1 s).
pub(crate) const PW_DUMP_TIMEOUT: Duration = Duration::from_secs(5);
/// Deadline for `pw-metadata` and `pw-cli` calls.
pub(crate) const PW_COMMAND_TIMEOUT: Duration = Duration::from_secs(3);
/// Cap on `pw-dump` output (real dumps are 100 KiB – a few MiB).
const MAX_DUMP_BYTES: usize = 64 * 1024 * 1024;
/// Cap on the output of the other tools.
const MAX_COMMAND_BYTES: usize = 1024 * 1024;
/// Cap on captured `stderr` (the rest is drained and discarded).
const MAX_STDERR_BYTES: usize = 64 * 1024;
/// How long to wait for the output pipes to close after the child exited.
const PIPE_GRACE: Duration = Duration::from_millis(500);
/// Upper bound of the exit-polling interval.
const MAX_POLL: Duration = Duration::from_millis(10);

/// Result of a capped read: the bytes and whether the cap was exceeded.
type Capped = io::Result<(Vec<u8>, bool)>;

/// Read up to `cap` bytes. On overflow, `drain` keeps reading (discarding) so the
/// child never blocks on a full pipe; otherwise the pipe is closed early.
fn read_capped(mut reader: impl Read, cap: usize, drain: bool) -> Capped {
    let mut buf = Vec::new();
    let limit = u64::try_from(cap).unwrap_or(u64::MAX).saturating_add(1);
    (&mut reader).take(limit).read_to_end(&mut buf)?;
    if buf.len() <= cap {
        return Ok((buf, false));
    }
    buf.truncate(cap);
    if drain {
        io::copy(&mut reader, &mut io::sink())?;
    }
    Ok((buf, true))
}

fn spawn_reader(
    reader: impl Read + Send + 'static,
    cap: usize,
    drain: bool,
    overflow: Option<Arc<AtomicBool>>,
) -> mpsc::Receiver<Capped> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let result = read_capped(reader, cap, drain);
        if let (Ok((_, true)), Some(flag)) = (&result, overflow) {
            flag.store(true, Ordering::SeqCst);
        }
        // The receiver may be gone after a timeout; nothing to do then.
        let _ = tx.send(result);
    });
    rx
}

fn kill_and_reap(child: &mut Child) {
    if let Err(e) = child.kill() {
        debug!("kill failed (child likely already exited): {e}");
    }
    if let Err(e) = child.wait() {
        warn!("Failed to reap child process: {e}");
    }
}

fn describe(program: &str, args: &[&str]) -> String {
    if args.is_empty() {
        program.to_string()
    } else {
        format!("{program} {}", args.join(" "))
    }
}

/// Run `program args…`, returning its `stdout`.
///
/// # Errors
/// Returns an error if the program cannot be spawned, does not exit within `timeout`
/// (it is then killed and reaped), writes more than `max_stdout` bytes, or exits
/// unsuccessfully (the error includes its `stderr`).
pub(crate) fn run(
    program: &str,
    args: &[&str],
    timeout: Duration,
    max_stdout: usize,
) -> Result<Vec<u8>> {
    let what = describe(program, args);
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| {
            format!("Failed to run `{what}`. Is PipeWire installed and `{program}` in PATH?")
        })?;

    let overflow = Arc::new(AtomicBool::new(false));
    let stdout_rx = child
        .stdout
        .take()
        .map(|out| spawn_reader(out, max_stdout, false, Some(Arc::clone(&overflow))));
    let stderr_rx = child
        .stderr
        .take()
        .map(|err| spawn_reader(err, MAX_STDERR_BYTES, true, None));

    let deadline = Instant::now() + timeout;
    let mut poll = Duration::from_millis(1);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {}
            Err(e) => {
                kill_and_reap(&mut child);
                return Err(e).with_context(|| format!("Failed to wait for `{what}`"));
            }
        }
        if overflow.load(Ordering::SeqCst) {
            kill_and_reap(&mut child);
            anyhow::bail!("`{what}` produced more than {max_stdout} bytes of output; aborted");
        }
        let now = Instant::now();
        if now >= deadline {
            kill_and_reap(&mut child);
            anyhow::bail!(
                "`{what}` did not finish within {} ms and was killed (is PipeWire responsive?)",
                timeout.as_millis()
            );
        }
        thread::sleep(poll.min(deadline - now));
        poll = (poll * 2).min(MAX_POLL);
    };

    let collect = |rx: Option<mpsc::Receiver<Capped>>, stream: &str| -> Result<(Vec<u8>, bool)> {
        let Some(rx) = rx else {
            return Ok((Vec::new(), false));
        };
        match rx.recv_timeout(PIPE_GRACE) {
            Ok(result) => result.with_context(|| format!("Failed to read {stream} of `{what}`")),
            Err(_) => anyhow::bail!("`{what}` exited but its {stream} stayed open"),
        }
    };
    let (stdout, stdout_overflow) = collect(stdout_rx, "stdout")?;
    let (stderr, _) = collect(stderr_rx, "stderr")?;
    let stderr = String::from_utf8_lossy(&stderr);

    if stdout_overflow {
        anyhow::bail!("`{what}` produced more than {max_stdout} bytes of output; aborted");
    }
    if !status.success() {
        anyhow::bail!("`{what}` failed ({status}): {}", stderr.trim());
    }
    if !stderr.trim().is_empty() {
        debug!("`{}` stderr: {}", what, stderr.trim());
    }
    Ok(stdout)
}

/// `tool --version`'s output, under the command deadline.
///
/// # Errors
/// Returns an error if the tool cannot be run, times out or fails (see [`run`]).
pub(crate) fn tool_version(tool: &str) -> Result<String> {
    let out = run(tool, &["--version"], PW_COMMAND_TIMEOUT, MAX_COMMAND_BYTES)?;
    Ok(String::from_utf8_lossy(&out).into_owned())
}

/// Raw `pw-dump` output, under its deadline and output cap.
///
/// # Errors
/// Returns an error if `pw-dump` cannot be run, times out or fails (see [`run`]).
pub(crate) fn dump_bytes() -> Result<Vec<u8>> {
    run("pw-dump", &[], PW_DUMP_TIMEOUT, MAX_DUMP_BYTES)
}

/// Whether `tool --version` runs and exits successfully.
pub(crate) fn tool_works(tool: &str) -> bool {
    run(tool, &["--version"], PW_COMMAND_TIMEOUT, MAX_COMMAND_BYTES)
        .inspect_err(|e| debug!("Tool check failed for {tool}: {e:#}"))
        .is_ok()
}

/// [`AudioBackend`] backed by `pw-dump`, `pw-metadata` and `pw-cli`.
pub(crate) struct CliBackend;

impl AudioBackend for CliBackend {
    fn snapshot(&self) -> Result<Snapshot> {
        parse_dump(&dump_bytes()?)
    }

    /// Writes `default.configured.audio.sink` (or `….audio.source`, as `wpctl
    /// set-default` does); `WirePlumber` derives the effective `default.audio.sink`
    /// (`default.audio.source`) from it.
    fn set_default(&self, kind: Endpoint, node_name: &str) -> Result<()> {
        let key = match kind {
            Endpoint::Sink => "default.configured.audio.sink",
            Endpoint::Source => "default.configured.audio.source",
        };
        let value = serde_json::json!({ "name": node_name }).to_string();
        run(
            "pw-metadata",
            &["0", key, &value, "Spa:String:JSON"],
            PW_COMMAND_TIMEOUT,
            MAX_COMMAND_BYTES,
        )
        .with_context(|| format!("Failed to set default {} to '{node_name}'", kind.noun()))?;
        debug!("Set configured default {}: {}", kind.noun(), node_name);
        Ok(())
    }

    /// `save: true` makes `WirePlumber` store the choice so it does not restore the
    /// previous profile on the next rescan.
    fn set_profile(&self, device_id: u32, profile_index: u32) -> Result<()> {
        let param = serde_json::json!({ "index": profile_index, "save": true }).to_string();
        run(
            "pw-cli",
            &["set-param", &device_id.to_string(), "Profile", &param],
            PW_COMMAND_TIMEOUT,
            MAX_COMMAND_BYTES,
        )
        .with_context(|| format!("Failed to set device {device_id} to profile {profile_index}"))?;
        debug!("Set device {} to profile {}", device_id, profile_index);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_run_returns_stdout() {
        let out = run("echo", &["hello"], Duration::from_secs(5), 1024).expect("echo runs");
        assert_eq!(out, b"hello\n");
    }

    #[test]
    fn test_run_kills_on_timeout() {
        let start = Instant::now();
        let err = run("sleep", &["5"], Duration::from_millis(100), 1024).unwrap_err();
        assert!(
            format!("{err}").contains("did not finish within 100 ms"),
            "{err}"
        );
        assert!(
            start.elapsed() < Duration::from_secs(2),
            "child was not killed promptly"
        );
    }

    #[test]
    fn test_run_reports_failure_status_and_stderr() {
        let err = run(
            "ls",
            &["/nonexistent-pwsw-test-path"],
            Duration::from_secs(5),
            1024,
        )
        .unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("failed"), "{msg}");
        assert!(
            msg.contains("nonexistent-pwsw-test-path"),
            "stderr missing: {msg}"
        );
    }

    #[test]
    fn test_run_caps_stdout() {
        let err = run(
            "head",
            &["-c", "100000", "/dev/zero"],
            Duration::from_secs(5),
            1000,
        )
        .unwrap_err();
        assert!(format!("{err}").contains("more than 1000 bytes"), "{err}");
    }

    #[test]
    fn test_run_missing_program() {
        let err = run("pwsw-no-such-tool", &[], Duration::from_secs(1), 1024).unwrap_err();
        assert!(format!("{err}").contains("Failed to run"), "{err}");
    }

    #[test]
    fn test_read_capped_drains() {
        let data = [7u8; 50];
        let (buf, over) = read_capped(&data[..], 10, true).expect("reads");
        assert_eq!(buf.len(), 10);
        assert!(over);
        let (buf, over) = read_capped(&data[..], 50, false).expect("reads");
        assert_eq!(buf.len(), 50);
        assert!(!over);
    }
}
