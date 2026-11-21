//! `pwsw status --watch`: follow the daemon's status, one line per change
//!
//! Made for status bars and shells: the process never exits because the daemon is
//! gone. When the connection fails or drops it prints one `stopped` line, retries with
//! backoff ([`BACKOFF_START`] doubling up to [`BACKOFF_MAX`]) and resumes with the
//! fresh state when the daemon is back. It ends on `SIGINT`/`SIGTERM` or when nobody
//! reads its output any more (exit status 0 in both cases).
//!
//! With `--json` every line is one `JSON` object (schema: STATUS STREAM in pwsw(1),
//! kept stable): the conventional bar fields `text`, `tooltip`, `class` and `alt`,
//! plus structured ones. Identical lines are never printed twice in a row.

use std::time::Duration;

use anyhow::{Context, Result};
use serde_json::{Value, json};
use tokio::signal::unix::{SignalKind, signal};

use crate::daemon_manager::DaemonManager;
use crate::ipc::{EndpointStatus, Response, StatusStream, StatusUpdate};

/// First wait before reconnecting after the daemon went away
pub const BACKOFF_START: Duration = Duration::from_millis(250);
/// Longest wait between two reconnection attempts
pub const BACKOFF_MAX: Duration = Duration::from_secs(5);
/// How often to check whether the reader of stdout went away while nothing changes
const STDOUT_CHECK_INTERVAL: Duration = Duration::from_secs(1);

/// What one output line reports
#[derive(Debug, Clone, Copy)]
enum Line<'a> {
    /// The daemon's current status
    Running(&'a StatusUpdate),
    /// No daemon to talk to
    Stopped,
    /// A daemon answered but refused the stream (too many clients, an older daemon
    /// that does not know it, ...)
    Error(&'a str),
}

/// The `class`/`alt` value: `paused` wins over `fallback`
fn class(update: &StatusUpdate) -> &'static str {
    let fallback = [&update.sink, &update.source]
        .into_iter()
        .flatten()
        .any(|e| e.fallback);
    if update.paused {
        "paused"
    } else if fallback {
        "fallback"
    } else {
        "active"
    }
}

fn fallback_mark(endpoint: &EndpointStatus) -> &'static str {
    if endpoint.fallback { " (fallback)" } else { "" }
}

/// Daemon-supplied text for a line or tooltip line: control characters escaped, so a
/// node name or description can never split or forge a line
fn safe(text: &str) -> std::borrow::Cow<'_, str> {
    crate::sanitize::for_log(text)
}

/// Multi-line tooltip: output, microphone, deciding rules, pause state
fn tooltip(update: &StatusUpdate) -> String {
    let mut lines = vec![match &update.sink {
        Some(sink) => format!(
            "Output: {} ({}){}",
            safe(&sink.desc),
            safe(&sink.node),
            fallback_mark(sink)
        ),
        None => "Output: unknown".to_string(),
    }];
    if let Some(source) = &update.source {
        lines.push(format!(
            "Mic: {}{}",
            safe(&source.desc),
            fallback_mark(source)
        ));
    } else if update.sources_configured {
        lines.push("Mic: unknown".to_string());
    }
    let sink_window = update.sink.as_ref().and_then(|s| s.window.as_ref());
    if let Some(window) = sink_window {
        lines.push(format!(
            "Rule: {} ({})",
            safe(&window.rule),
            safe(&window.app_id)
        ));
    }
    if let Some(window) = update.source.as_ref().and_then(|s| s.window.as_ref())
        && sink_window != Some(window)
    {
        lines.push(format!(
            "Mic rule: {} ({})",
            safe(&window.rule),
            safe(&window.app_id)
        ));
    }
    if update.paused {
        lines.push("Paused: rules do not switch".to_string());
    }
    lines.join("\n")
}

fn endpoint_json(endpoint: Option<&EndpointStatus>) -> Value {
    endpoint.map_or(Value::Null, |e| {
        json!({
            "desc": e.desc,
            "node": e.node,
            "fallback": e.fallback,
            "window": e.window.as_ref().map(|w| json!({
                "app_id": w.app_id,
                "rule": w.rule,
                "rule_index": w.rule_index,
            })),
        })
    })
}

/// The `JSON` object for one line (schema: STATUS STREAM in pwsw(1))
fn json_line(line: Line<'_>) -> Value {
    match line {
        Line::Running(update) => {
            let class = class(update);
            json!({
                "text": update.sink.as_ref().map_or("unknown", |s| s.desc.as_str()),
                "tooltip": tooltip(update),
                "class": class,
                "alt": class,
                "sink": endpoint_json(update.sink.as_ref()),
                "source": endpoint_json(update.source.as_ref()),
                "paused": update.paused,
                "daemon": "running",
                "version": update.version,
                "protocol": update.protocol,
                "managed_by": match update.daemon_manager {
                    DaemonManager::Systemd => "systemd",
                    DaemonManager::Direct => "direct",
                },
            })
        }
        Line::Stopped => json!({
            "text": "pwsw stopped",
            "tooltip": "pwsw daemon is not running",
            "class": "stopped",
            "alt": "stopped",
            "daemon": "stopped",
            "sink": null,
            "source": null,
            "paused": false,
        }),
        Line::Error(message) => json!({
            "text": "pwsw error",
            "tooltip": format!("pwsw daemon: {}", crate::sanitize::for_log(message)),
            "class": "stopped",
            "alt": "stopped",
            "daemon": "error",
            "sink": null,
            "source": null,
            "paused": false,
        }),
    }
}

/// The human-readable line: `Headphones · Mic: Headset Mic · paused`
fn human_line(line: Line<'_>) -> String {
    match line {
        Line::Running(update) => {
            let mut parts = vec![match &update.sink {
                Some(sink) => format!("{}{}", safe(&sink.desc), fallback_mark(sink)),
                None => "unknown output".to_string(),
            }];
            if let Some(source) = &update.source {
                parts.push(format!(
                    "Mic: {}{}",
                    safe(&source.desc),
                    fallback_mark(source)
                ));
            }
            if update.paused {
                parts.push("paused".to_string());
            }
            parts.join(" · ")
        }
        Line::Stopped => "pwsw stopped".to_string(),
        Line::Error(message) => format!("pwsw error: {}", crate::sanitize::for_log(message)),
    }
}

/// Prints lines, skipping one identical to the line before
struct Printer {
    json: bool,
    last: Option<String>,
}

impl Printer {
    fn print(&mut self, line: Line<'_>) {
        let text = if self.json {
            json_line(line).to_string()
        } else {
            human_line(line)
        };
        if self.last.as_deref() == Some(text.as_str()) {
            return;
        }
        // Flushed at once; a reader that went away ends the process with status 0
        crate::style::write_stdout_bytes(format!("{text}\n").as_bytes());
        self.last = Some(text);
    }
}

/// Follow the daemon's status until a signal arrives or stdout's reader goes away
///
/// # Errors
/// Returns an error if the signal handlers cannot be installed. Losing the daemon is
/// never an error.
pub async fn run(json: bool) -> Result<()> {
    let mut interrupt = signal(SignalKind::interrupt()).context("Failed to listen for SIGINT")?;
    let mut terminate = signal(SignalKind::terminate()).context("Failed to listen for SIGTERM")?;
    tokio::select! {
        () = follow(Printer { json, last: None }) => {}
        _ = interrupt.recv() => {}
        _ = terminate.recv() => {}
        () = stdout_hung_up() => {}
    }
    Ok(())
}

/// Print every update; reconnect with backoff whenever the daemon is gone
async fn follow(mut printer: Printer) {
    let mut delay = BACKOFF_START;
    loop {
        if let Ok(mut stream) = StatusStream::connect().await {
            loop {
                match stream.next().await {
                    Ok(Response::StatusUpdate(update)) => {
                        printer.print(Line::Running(&update));
                        delay = BACKOFF_START;
                    }
                    Ok(Response::Error { message }) => {
                        printer.print(Line::Error(&message));
                        break;
                    }
                    Ok(_) => {
                        printer.print(Line::Error(&format!(
                            "unexpected reply to the status stream; {}",
                            crate::ipc::RESTART_HINT
                        )));
                        break;
                    }
                    // The daemon hung up: it stopped (or is restarting)
                    Err(_) => {
                        printer.print(Line::Stopped);
                        break;
                    }
                }
            }
        } else {
            printer.print(Line::Stopped);
        }
        tokio::time::sleep(delay).await;
        delay = (delay * 2).min(BACKOFF_MAX);
    }
}

/// Resolves once stdout is a pipe (or socket) whose reader went away
///
/// Writing would notice too, but while the status does not change (or the daemon
/// is down) nothing is written, and an orphaned watcher would otherwise live on.
async fn stdout_hung_up() {
    let mut ticks = tokio::time::interval(STDOUT_CHECK_INTERVAL);
    loop {
        ticks.tick().await;
        if stdout_closed() {
            return;
        }
    }
}

fn stdout_closed() -> bool {
    use std::os::fd::AsFd;

    use rustix::event::{PollFd, PollFlags, Timespec, poll};

    let stdout = std::io::stdout();
    let fd = stdout.as_fd();
    let mut fds = [PollFd::new(&fd, PollFlags::empty())];
    let now = Timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    match poll(&mut fds, Some(&now)) {
        Ok(ready) if ready > 0 => fds[0]
            .revents()
            .intersects(PollFlags::ERR | PollFlags::HUP | PollFlags::NVAL),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;
    use crate::ipc::RuleWindow;

    fn endpoint(desc: &str, node: &str, fallback: bool) -> EndpointStatus {
        EndpointStatus {
            desc: desc.to_string(),
            node: node.to_string(),
            fallback,
            window: None,
        }
    }

    fn update(sink_fallback: bool, source: Option<EndpointStatus>, paused: bool) -> StatusUpdate {
        StatusUpdate {
            version: "v0.7.0+test".to_string(),
            protocol: crate::ipc::PROTOCOL_VERSION,
            daemon_manager: DaemonManager::Systemd,
            paused,
            sink: Some(EndpointStatus {
                window: Some(RuleWindow {
                    app_id: "discord".to_string(),
                    rule: "Voice chat".to_string(),
                    rule_index: 4,
                }),
                ..endpoint("Headphones", "alsa_output.usb", sink_fallback)
            }),
            sources_configured: source.is_some(),
            source,
        }
    }

    #[test_case::test_case(false, false, false, "active" ; "active")]
    #[test_case::test_case(true, false, false, "fallback" ; "sink fallback")]
    #[test_case::test_case(false, true, false, "fallback" ; "source fallback")]
    #[test_case::test_case(false, false, true, "paused" ; "paused")]
    #[test_case::test_case(true, true, true, "paused" ; "paused wins over fallback")]
    fn test_class(sink_fallback: bool, source_fallback: bool, paused: bool, expected: &str) {
        let source = Some(endpoint("Headset Mic", "alsa_input.usb", source_fallback));
        let line = json_line(Line::Running(&update(sink_fallback, source, paused)));
        assert_eq!(line["class"], expected);
        assert_eq!(line["alt"], expected);
        assert_eq!(line["paused"], paused);
    }

    #[test]
    fn test_running_line_has_bar_and_structured_fields() {
        let source = Some(endpoint("Headset Mic", "alsa_input.usb", false));
        let line = json_line(Line::Running(&update(false, source, false)));
        assert_eq!(
            line,
            json!({
                "text": "Headphones",
                "tooltip": "Output: Headphones (alsa_output.usb)\nMic: Headset Mic\nRule: Voice chat (discord)",
                "class": "active",
                "alt": "active",
                "sink": {
                    "desc": "Headphones",
                    "node": "alsa_output.usb",
                    "fallback": false,
                    "window": {"app_id": "discord", "rule": "Voice chat", "rule_index": 4},
                },
                "source": {
                    "desc": "Headset Mic",
                    "node": "alsa_input.usb",
                    "fallback": false,
                    "window": null,
                },
                "paused": false,
                "daemon": "running",
                "version": "v0.7.0+test",
                "protocol": crate::ipc::PROTOCOL_VERSION,
                "managed_by": "systemd",
            })
        );
    }

    #[test]
    fn test_source_is_null_without_sources_and_sink_null_while_unknown() {
        let mut status = update(true, None, true);
        let line = json_line(Line::Running(&status));
        assert_eq!(line["source"], Value::Null);
        assert_eq!(
            line["tooltip"],
            "Output: Headphones (alsa_output.usb) (fallback)\nRule: Voice chat (discord)\nPaused: rules do not switch"
        );
        status.sink = None;
        let line = json_line(Line::Running(&status));
        assert_eq!(line["sink"], Value::Null);
        assert_eq!(line["text"], "unknown");
    }

    #[test]
    fn test_stopped_and_error_lines() {
        assert_eq!(
            json_line(Line::Stopped),
            json!({
                "text": "pwsw stopped",
                "tooltip": "pwsw daemon is not running",
                "class": "stopped",
                "alt": "stopped",
                "daemon": "stopped",
                "sink": null,
                "source": null,
                "paused": false,
            })
        );
        let error = json_line(Line::Error("too many\nclients"));
        assert_eq!(error["class"], "stopped");
        assert_eq!(error["daemon"], "error");
        assert_eq!(error["tooltip"], "pwsw daemon: too many\\nclients");
    }

    #[test]
    fn test_human_lines() {
        let source = Some(endpoint("Headset Mic", "alsa_input.usb", false));
        assert_eq!(
            human_line(Line::Running(&update(false, source.clone(), true))),
            "Headphones · Mic: Headset Mic · paused"
        );
        assert_eq!(
            human_line(Line::Running(&update(true, None, false))),
            "Headphones (fallback)"
        );
        assert_eq!(human_line(Line::Stopped), "pwsw stopped");

        // Daemon-supplied names never split a line
        let mut status = update(false, None, false);
        if let Some(sink) = &mut status.sink {
            sink.desc = "Evil\nForged line".to_string();
            sink.node = "node\r\n".to_string();
        }
        let human = human_line(Line::Running(&status));
        assert!(!human.contains('\n'), "{human}");
        let tooltip = tooltip(&status);
        assert_eq!(tooltip.lines().count(), 2, "{tooltip}");
    }

    #[test]
    fn test_json_lines_are_single_lines() {
        let source = Some(endpoint("Headset Mic", "alsa_input.usb", true));
        for line in [
            Line::Running(&update(true, source.clone(), true)),
            Line::Stopped,
            Line::Error("a\nb"),
        ] {
            assert!(!json_line(line).to_string().contains('\n'));
        }
    }
}
