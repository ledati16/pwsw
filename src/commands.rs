//! CLI commands
//!
//! Implements both local commands (list-sinks, validate) and IPC-based commands
//! that communicate with the daemon (status, list-windows, test-rule). Manual
//! switching (set-sink, next-sink, prev-sink) goes through the daemon when it runs.

use std::collections::HashSet;
use std::fmt::Write;

use color_eyre::eyre::{self, Result};
use tracing::{info, warn};

use crate::config::{Config, Endpoint};
use crate::ipc::{self, Request, Response};
use crate::manual::{self, ManualSwitch, ManualTarget, Probe};
use crate::notification::{get_sink_icon, get_source_icon, send_notification};
use crate::pipewire::{
    ActiveSink, ActiveSinkJson, ConfiguredSinkJson, ListSinksJson, PipeWire, ProfileSink,
    ProfileSinkJson,
};
use crate::style::PwswStyle;

// ============================================================================
// Local Commands (no daemon needed)
// ============================================================================

/// Helper to determine a sink's status (active, requires profile switch, or not found)
fn get_sink_status(
    sink_name: &str,
    active: &[ActiveSink],
    profile: &[ProfileSink],
) -> &'static str {
    if active.iter().any(|a| a.name == sink_name) {
        "active"
    } else if profile.iter().any(|p| p.predicted_name == sink_name) {
        "requires_profile_switch"
    } else {
        "not_found"
    }
}

/// What `PipeWire` offers for one kind of endpoint (sinks or sources)
struct Listing {
    kind: Endpoint,
    active: Vec<ActiveSink>,
    profile: Vec<ProfileSink>,
}

impl Listing {
    fn new(snapshot: &crate::pipewire::Snapshot, kind: Endpoint) -> Self {
        let active = PipeWire::get_active(snapshot, kind);
        let profile = match kind {
            Endpoint::Sink => PipeWire::get_profile_sinks(snapshot, &active),
            Endpoint::Source => PipeWire::get_profile_sources(snapshot, &active),
        };
        Self {
            kind,
            active,
            profile,
        }
    }

    fn current_default(&self) -> Option<String> {
        self.active
            .iter()
            .find(|s| s.is_default)
            .map(|s| s.name.clone())
    }

    fn active_json(&self, config: Option<&Config>) -> Vec<ActiveSinkJson> {
        let configured: HashSet<&str> = config.map_or_default(|c| {
            c.endpoints(self.kind)
                .iter()
                .map(|s| s.name.as_str())
                .collect()
        });
        self.active
            .iter()
            .map(|s| ActiveSinkJson {
                name: s.name.clone(),
                description: s.description.clone(),
                is_default: s.is_default,
                configured: configured.contains(s.name.as_str()),
            })
            .collect()
    }

    fn profile_json(&self) -> Vec<ProfileSinkJson> {
        self.profile
            .iter()
            .map(|s| ProfileSinkJson {
                predicted_name: s.predicted_name.clone(),
                description: s.description.clone(),
                device_name: s.device_name.clone(),
                profile_name: s.profile_name.clone(),
                profile_index: s.profile_index,
            })
            .collect()
    }

    fn configured_json(&self, config: Option<&Config>) -> Vec<ConfiguredSinkJson> {
        config.map_or_default(|c| {
            c.endpoints(self.kind)
                .iter()
                .enumerate()
                .map(|(i, s)| ConfiguredSinkJson {
                    index: i + 1,
                    name: s.name.clone(),
                    desc: s.desc.clone(),
                    icon: s.icon.clone(),
                    is_default_config: s.default,
                    status: get_sink_status(&s.name, &self.active, &self.profile).to_string(),
                })
                .collect()
        })
    }

    /// The human-readable sections: active, reachable by profile switch, configured
    fn print(&self, config: Option<&Config>, titles: [&str; 3]) {
        let [active_title, profile_title, configured_title] = titles;
        let configured_marker = |name: &str| {
            config
                .and_then(|c| c.endpoints(self.kind).iter().find(|s| s.name == name))
                .map_or_default(|s| {
                    let mut m = String::with_capacity(3 + s.desc.len());
                    m.push_str(" [");
                    m.push_str(&s.desc);
                    m.push(']');
                    m
                })
        };
        crate::outln!("{}", active_title.header());
        crate::outln!("{}", "-".repeat(active_title.chars().count()));
        if self.active.is_empty() {
            crate::outln!("  {}", "(none)".dim());
        } else {
            for node in &self.active {
                let marker = if node.is_default { "* " } else { "  " };
                crate::outln!(
                    "{}{}{}",
                    marker,
                    node.name.as_str().bold(),
                    configured_marker(&node.name)
                );
                crate::outln!("    {}", node.description.as_str().dim());
            }
            crate::outln!("\n  {} = current default", "*".dim());
        }

        if !self.profile.is_empty() {
            crate::outln!("\n{}", profile_title.header());
            crate::outln!("{}", "-".repeat(profile_title.chars().count()));
            for node in &self.profile {
                crate::outln!(
                    "  {} {}{}",
                    "~".dim(),
                    node.predicted_name.as_str().bold(),
                    configured_marker(&node.predicted_name)
                );
                crate::outln!(
                    "    {} (profile: {})",
                    node.description.as_str().dim(),
                    node.profile_name.as_str().technical()
                );
            }
        }

        let Some(cfg) = config else {
            return;
        };
        let entries = cfg.endpoints(self.kind);
        if self.kind == Endpoint::Source && entries.is_empty() {
            return;
        }
        crate::outln!("\n{}", configured_title.header());
        crate::outln!("{}", "-".repeat(configured_title.chars().count()));
        for (i, entry) in entries.iter().enumerate() {
            let default_marker = if entry.default {
                let mut m = String::with_capacity(3 + "DEFAULT".len());
                let _ = write!(m, " [{}]", "DEFAULT".dim());
                m
            } else {
                String::new()
            };
            let status = match get_sink_status(&entry.name, &self.active, &self.profile) {
                "active" => "active".success().to_string(),
                "requires_profile_switch" => "profile switch".warning().to_string(),
                _ => "not found".error().to_string(),
            };
            crate::outln!(
                "  {}. \"{}\"{} - {}",
                (i + 1).to_string().dim(),
                entry.desc,
                default_marker,
                status
            );
            crate::outln!("     {}", entry.name.as_str().bold());
        }
    }
}

/// List all available sinks (active and profile-switch), then the input devices
/// (sources), so one command shows everything a config can name
///
/// # Errors
/// Returns an error if `PipeWire` query fails or `JSON` serialization fails.
pub fn list_sinks(config: Option<&Config>, json_output: bool) -> Result<()> {
    let snapshot = PipeWire::dump()?;
    let sinks = Listing::new(&snapshot, Endpoint::Sink);
    let sources = Listing::new(&snapshot, Endpoint::Source);

    if json_output {
        let output = ListSinksJson {
            active_sinks: sinks.active_json(config),
            profile_sinks: sinks.profile_json(),
            configured_sinks: sinks.configured_json(config),
            current_default: sinks.current_default(),
            active_sources: sources.active_json(config),
            profile_sources: sources.profile_json(),
            configured_sources: sources.configured_json(config),
            current_default_source: sources.current_default(),
        };
        crate::outln!("{}", serde_json::to_string_pretty(&output)?);
    } else {
        sinks.print(config, [
            "ACTIVE SINKS:",
            "AVAILABLE VIA PROFILE SWITCH:",
            "CONFIGURED SINKS:",
        ]);
        crate::outln!();
        sources.print(config, [
            "INPUT DEVICES / SOURCES:",
            "INPUTS AVAILABLE VIA PROFILE SWITCH:",
            "CONFIGURED SOURCES:",
        ]);

        if let Ok(path) = Config::get_config_path() {
            crate::outln!("\n{} {}", "Config:".dim(), path.display());
        }
    }

    Ok(())
}

/// Direction for sink cycling
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Next,
    Prev,
}

/// Switch to a sink (`set-sink`), with smart toggle support
///
/// When the daemon runs, the switch goes through it (`IPC`), so its view of the
/// current sink stays correct and its rules see the manual choice; otherwise the
/// switch happens locally.
///
/// # Errors
/// Returns an error if the sink reference is invalid, the daemon reports a failure,
/// or local sink activation fails.
pub async fn set_sink(config: &Config, sink_ref: &str) -> Result<()> {
    if ipc::is_daemon_running().await {
        return switch_via_daemon(Request::SetSink {
            sink: sink_ref.to_string(),
        })
        .await;
    }
    set_sink_smart(config, sink_ref)
}

/// Switch the default source (microphone) to a configured source (`set-source`), with
/// smart toggle support like [`set_sink`]
///
/// Goes through the daemon when it runs (its rules then see the manual choice),
/// otherwise switches locally.
///
/// # Errors
/// Returns an error if no sources are configured, the reference names no configured
/// source, the daemon reports a failure, or local activation fails.
pub async fn set_source(config: &Config, source_ref: &str) -> Result<()> {
    if ipc::is_daemon_running().await {
        return switch_via_daemon(Request::SetSource {
            source: source_ref.to_string(),
        })
        .await;
    }
    switch_locally(config, &ManualSwitch::SetSource(source_ref.to_string()))
}

/// Cycle to the next/previous sink (`next-sink` / `prev-sink`)
///
/// Goes through the daemon when it runs (see [`set_sink`]), otherwise switches locally.
///
/// # Errors
/// Returns an error if the daemon reports a failure, or the local sink query or
/// activation fails.
pub async fn cycle(config: &Config, direction: Direction) -> Result<()> {
    if ipc::is_daemon_running().await {
        let request = match direction {
            Direction::Next => Request::NextSink,
            Direction::Prev => Request::PrevSink,
        };
        return switch_via_daemon(request).await;
    }
    cycle_sink(config, direction)
}

/// Send a manual switch request to the daemon and print the outcome
async fn switch_via_daemon(request: Request) -> Result<()> {
    match ipc::send_request(request).await? {
        Response::SinkSwitched { desc, .. } => {
            crate::outln!("{} {}", "Switched to:".success(), desc.as_str().bold());
            Ok(())
        }
        Response::Ok { message } => {
            crate::outln!("{message}");
            Ok(())
        }
        Response::Error { message } => eyre::bail!("{message}"),
        other => Err(unexpected_response(&other)),
    }
}

/// Error for a reply that does not fit the request (likely a version mismatch)
fn unexpected_response(response: &Response) -> eyre::Report {
    eyre::eyre!(
        "Unexpected response from daemon ({}); {}",
        response_kind(response),
        ipc::RESTART_HINT
    )
}

/// Name of a response variant, for error messages
fn response_kind(response: &Response) -> &'static str {
    match response {
        Response::Status { .. } => "Status",
        Response::Ok { .. } => "Ok",
        Response::Error { .. } => "Error",
        Response::Windows { .. } => "Windows",
        Response::RuleMatches { .. } => "RuleMatches",
        Response::ManagerInfo { .. } => "ManagerInfo",
        Response::SinkSwitched { .. } => "SinkSwitched",
        Response::PauseState { .. } => "PauseState",
    }
}

/// Set sink with smart toggle support, switching locally (no daemon)
///
/// The smart toggle compares with the real default sink.
///
/// # Errors
/// Returns an error if the sink reference is invalid, `PipeWire` cannot be queried,
/// or sink activation fails.
fn set_sink_smart(config: &Config, sink_ref: &str) -> Result<()> {
    switch_locally(config, &ManualSwitch::Set(sink_ref.to_string()))
}

/// Cycle through configured sinks, switching locally (no daemon)
///
/// Starts from the real default sink and skips sinks that cannot be activated right
/// now, like the daemon does.
///
/// # Errors
/// Returns an error if `PipeWire` cannot be queried, no other sink can be activated,
/// or sink activation fails.
fn cycle_sink(config: &Config, direction: Direction) -> Result<()> {
    let request = match direction {
        Direction::Next => ManualSwitch::Next,
        Direction::Prev => ManualSwitch::Prev,
    };
    switch_locally(config, &request)
}

/// Carry out a manual request without the daemon: probe `PipeWire`, decide with the
/// daemon's own rules ([`manual::plan`]), activate, print and notify
fn switch_locally(config: &Config, request: &ManualSwitch) -> Result<()> {
    if let Some(answer) = manual::answer_without_pipewire(config, request) {
        let message = answer.map_err(|message| eyre::eyre!("{message}"))?;
        crate::outln!("{}", message.warning());
        return Ok(());
    }

    let probe = Probe::from_snapshot(&PipeWire::dump()?, config);
    let target = match manual::plan(config, request, &probe) {
        Ok(ManualTarget::Switch(target)) => target,
        Ok(ManualTarget::Done(message)) => {
            crate::outln!("{message}");
            return Ok(());
        }
        Err(message) => eyre::bail!("{message}"),
    };

    let kind = request.kind();
    info!("Switching to: {}", target.desc);
    PipeWire::activate(kind, &target.name)?;
    crate::outln!(
        "{} {}",
        "Switched to:".success(),
        target.desc.as_str().bold()
    );

    if config.settings.notify_manual {
        let (title, icon) = match kind {
            Endpoint::Sink => ("Audio Output", get_sink_icon(target)),
            Endpoint::Source => ("Audio Input", get_source_icon(target)),
        };
        if let Err(e) = send_notification(title, &target.desc, Some(&icon)) {
            warn!("Notification failed: {}", e);
        }
    }
    Ok(())
}

// ============================================================================
// IPC-based Commands (require daemon)
// ============================================================================

/// What the daemon said about itself (see [`status`])
#[derive(Debug, Default)]
struct DaemonStatus {
    running: bool,
    error: Option<String>,
    version: Option<String>,
    protocol: Option<u32>,
    uptime_secs: Option<u64>,
    daemon_sink: Option<String>,
    daemon_sink_fallback: Option<bool>,
    daemon_source: Option<String>,
    daemon_source_fallback: Option<bool>,
    active_window: Option<String>,
    tracked_windows: Option<usize>,
    paused: Option<bool>,
    /// Compositor protocol in use (`"ext"`/`"wlr"`)
    backend: Option<String>,
    /// Whether the backend reports monitors (rules with `output` can match)
    monitor_info: Option<bool>,
}

impl DaemonStatus {
    /// Ask the daemon (if one runs) for its status
    async fn query() -> Self {
        if !ipc::is_daemon_running().await {
            return Self::default();
        }
        let error = |message: String| Self {
            running: true,
            error: Some(message),
            ..Self::default()
        };
        match ipc::send_request(Request::Status).await {
            Ok(Response::Status {
                version,
                protocol,
                uptime_secs,
                current_sink,
                current_sink_fallback,
                active_window,
                tracked_windows,
                paused,
                current_source,
                current_source_fallback,
                sources_configured: _,
                backend,
                monitor_info,
            }) => Self {
                running: true,
                error: (protocol != ipc::PROTOCOL_VERSION).then(|| {
                    format!(
                        "Daemon {version} speaks IPC protocol {protocol}, this pwsw speaks \
                         protocol {}; {}",
                        ipc::PROTOCOL_VERSION,
                        ipc::RESTART_HINT
                    )
                }),
                version: Some(version),
                protocol: Some(protocol),
                uptime_secs: Some(uptime_secs),
                daemon_sink_fallback: current_sink.as_ref().map(|_| current_sink_fallback),
                daemon_sink: current_sink,
                daemon_source_fallback: current_source.as_ref().map(|_| current_source_fallback),
                daemon_source: current_source,
                active_window,
                tracked_windows: Some(tracked_windows),
                paused: Some(paused),
                // Older daemons do not report the backend; then monitor info is unknown
                monitor_info: backend.as_ref().map(|_| monitor_info),
                backend,
            },
            Ok(Response::Error { message }) => error(message),
            Ok(other) => error(format!("{:#}", unexpected_response(&other))),
            Err(e) => error(format!("{e:#}")),
        }
    }

    fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "running": self.running,
            "error": self.error,
            "version": self.version,
            "protocol": self.protocol,
            "uptime_secs": self.uptime_secs,
            "uptime_human": self.uptime_secs.map(crate::format_uptime),
            "daemon_sink": self.daemon_sink,
            "daemon_sink_fallback": self.daemon_sink_fallback,
            "daemon_source": self.daemon_source,
            "daemon_source_fallback": self.daemon_source_fallback,
            "active_window": self.active_window,
            "tracked_windows": self.tracked_windows,
            "paused": self.paused,
            "backend": self.backend,
            "monitor_info": self.monitor_info,
        })
    }
}

/// Show the current audio output and daemon status (works without a daemon)
///
/// Never fails as a whole: a part that cannot be determined (config, `PipeWire`,
/// daemon) is reported as such, and the rest is still shown. With `json_output`
/// the output is always one valid `JSON` document (schema in pwsw(1)); otherwise
/// problems are also printed to stderr as `error:` lines.
///
/// Returns whether everything could be determined (no part errored; a daemon that
/// is not running is not an error). Always `true` with `json_output`: errors are part
/// of the document, and status bars treat a failing command as "hide the module".
pub async fn status(config: Result<Config>, json_output: bool) -> bool {
    let config_path = Config::get_config_path().ok();
    let (config, config_error) = match config {
        Ok(config) => (Some(config), None),
        Err(e) => (None, Some(format!("{e:#}"))),
    };

    // `PipeWire` is queried directly (works with or without daemon)
    let defaults = match tokio::task::spawn_blocking(PipeWire::get_defaults).await {
        Ok(result) => result,
        Err(e) => Err(eyre::eyre!("PipeWire query task failed: {e}")),
    };
    let describe = |kind: Endpoint, name: String| {
        let description = config
            .as_ref()
            .map_or_else(|| name.clone(), |c| c.endpoint_desc(kind, &name));
        (name, description)
    };
    let (current_sink, current_source, pipewire_error) = match defaults {
        Ok((Some(sink), source)) => (
            Some(describe(Endpoint::Sink, sink)),
            source.map(|s| describe(Endpoint::Source, s)),
            None,
        ),
        Ok((None, source)) => (
            None,
            source.map(|s| describe(Endpoint::Source, s)),
            Some("No default sink found in PipeWire metadata".to_string()),
        ),
        Err(e) => (None, None, Some(format!("{e:#}"))),
    };
    let sources_configured = config.as_ref().is_some_and(|c| !c.sources.is_empty());

    let daemon = DaemonStatus::query().await;

    let healthy = config_error.is_none() && pipewire_error.is_none() && daemon.error.is_none();

    if json_output {
        let named = |pair: &Option<(String, String)>| {
            pair.as_ref().map(
                |(name, description)| serde_json::json!({ "name": name, "description": description }),
            )
        };
        let current_sink_json = named(&current_sink);
        let current_source_json = named(&current_source);
        let output = serde_json::json!({
            "current_sink": current_sink_json,
            "current_source": current_source_json,
            "sources_configured": sources_configured,
            "daemon": daemon.to_json(),
            "pipewire": {
                "current_sink": current_sink_json,
                "current_source": current_source_json,
                "error": pipewire_error,
            },
            "config": {
                "path": config_path.as_ref().map(|p| p.display().to_string()),
                "error": config_error,
            },
        });
        // A `Value` always serializes; fall back to compact output just in case
        let text = serde_json::to_string_pretty(&output).unwrap_or_else(|_| output.to_string());
        crate::outln!("{text}");
        return true;
    }

    for error in [&config_error, &pipewire_error, &daemon.error]
        .into_iter()
        .flatten()
    {
        crate::errln!("{} {error}", "error:".error().to_stderr());
    }

    crate::outln!("{}", "Audio Output".header());
    crate::outln!("{}", "-".repeat(12));
    match &current_sink {
        Some((_, description)) => {
            crate::outln!("{} {}", "Current:".dim(), description.as_str().bold());
        }
        None => crate::outln!(
            "{} {}",
            "Current:".dim(),
            "unavailable (PipeWire error)".error()
        ),
    }
    if sources_configured {
        match &current_source {
            Some((_, description)) => {
                crate::outln!("{} {}", "Input:".dim(), description.as_str().bold());
            }
            None => crate::outln!("{} {}", "Input:".dim(), "unknown".dim()),
        }
    }
    crate::outln!();
    crate::outln!("{}", "Daemon".header());
    crate::outln!("{}", "-".repeat(6));

    if !daemon.running {
        crate::outln!("{} {}", "Status:".dim(), "Not running".error());
        crate::outln!("  Start with: {}", "pwsw daemon".technical());
    } else if let Some(uptime_secs) = daemon.uptime_secs {
        crate::outln!(
            "{} {}",
            "Status:".dim(),
            format!("Running (uptime: {})", crate::format_uptime(uptime_secs)).success()
        );
        if let Some(version) = &daemon.version {
            crate::outln!("{} {}", "Version:".dim(), version);
        }
        if let Some(backend) = &daemon.backend {
            let monitors = if daemon.monitor_info == Some(true) {
                "monitor info available"
            } else {
                "no monitor info"
            };
            crate::outln!("{} {backend} ({monitors})", "Compositor:".dim());
        }
        if daemon.paused == Some(true) {
            crate::outln!(
                "{} {}",
                "Switching:".dim(),
                "Paused (rules do not switch; resume with: pwsw resume)".warning()
            );
        }
        if daemon.error.is_some() {
            crate::outln!(
                "  {}",
                "IPC protocol mismatch: restart the daemon".warning()
            );
        }
        if let Some(rule) = &daemon.active_window {
            crate::outln!("{} {}", "Active Rule:".dim(), rule.as_str().technical());
        }
        if let Some(source) = &daemon.daemon_source {
            let fallback = if daemon.daemon_source_fallback == Some(true) {
                " (fallback)"
            } else {
                ""
            };
            crate::outln!(
                "{} {}{fallback}",
                "Microphone:".dim(),
                source.as_str().technical()
            );
        }
        if let Some(tracked) = daemon.tracked_windows {
            crate::outln!(
                "{} {}",
                "Tracked Windows:".dim(),
                tracked.to_string().technical()
            );
        }
    } else {
        crate::outln!(
            "{} {}",
            "Status:".dim(),
            "Running, but its status could not be read (see error)".error()
        );
    }

    healthy
}

/// Gracefully shutdown the daemon
///
/// # Errors
/// Returns an error if no daemon is running or IPC communication fails.
pub async fn shutdown() -> Result<()> {
    if !ipc::is_daemon_running().await {
        eyre::bail!("Daemon is not running");
    }

    let response = ipc::send_request(Request::Shutdown).await?;

    match response {
        Response::Ok { message } => {
            crate::outln!("{}", message.success());
            Ok(())
        }
        Response::Error { message } => {
            eyre::bail!("{message}");
        }
        other => Err(unexpected_response(&other)),
    }
}

/// What [`pause`] asks the daemon for
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PauseAction {
    Pause,
    Resume,
    Toggle,
}

/// Pause, resume or toggle the daemon's rule-driven switching
///
/// # Errors
/// Returns an error if no daemon is running, `IPC` communication fails, or the daemon
/// does not support pausing (an older version: restart it).
pub async fn pause(action: PauseAction) -> Result<()> {
    if !ipc::is_daemon_running().await {
        eyre::bail!("Daemon is not running. Start it with: pwsw daemon");
    }
    let request = match action {
        PauseAction::Pause => Request::Pause,
        PauseAction::Resume => Request::Resume,
        PauseAction::Toggle => Request::TogglePause,
    };
    match ipc::send_request(request).await? {
        Response::PauseState { paused, changed } => {
            crate::outln!("{}", pause_message(paused, changed));
            Ok(())
        }
        Response::Error { message } => eyre::bail!("{message}"),
        other => Err(unexpected_response(&other)),
    }
}

/// The line `pause`/`resume`/`toggle-pause` print for the daemon's answer
fn pause_message(paused: bool, changed: bool) -> crate::style::Painted<&'static str> {
    match (paused, changed) {
        (true, true) => {
            "Paused: rules will not switch the audio output (resume with: pwsw resume)".warning()
        }
        (true, false) => "Already paused (resume with: pwsw resume)".warning(),
        (false, true) => "Resumed: rules switch the audio output again".success(),
        (false, false) => "Not paused: rules are switching the audio output".success(),
    }
}

/// Get list of windows currently tracked by daemon
///
/// # Errors
/// Returns an error if no daemon is running or IPC communication fails.
pub async fn list_windows(json_output: bool) -> Result<()> {
    if !ipc::is_daemon_running().await {
        eyre::bail!("Daemon is not running. Start it with: pwsw daemon");
    }

    let response = ipc::send_request(Request::ListWindows).await?;

    match response {
        Response::Windows { windows } => {
            if json_output {
                crate::outln!("{}", serde_json::to_string_pretty(&windows)?);
            } else if windows.is_empty() {
                crate::outln!("{}", "No windows currently open.".dim());
            } else {
                let tracked: Vec<_> = windows.iter().filter(|w| w.tracked.is_some()).collect();
                let untracked: Vec<_> = windows.iter().filter(|w| w.tracked.is_none()).collect();

                let header = format!(
                    "All Windows ({} open, {} tracked):",
                    windows.len(),
                    tracked.len()
                );
                let header_len = header.len();
                crate::outln!("{}", header.header());
                crate::outln!("{}", "-".repeat(header_len));

                if !tracked.is_empty() {
                    crate::outln!(
                        "\n{} ({}):",
                        "Tracked".success(),
                        tracked.len().to_string().technical()
                    );
                    for window in &tracked {
                        if let Some(ref track_info) = window.tracked {
                            crate::outln!(
                                "  {} {}: {}",
                                "•".success(),
                                "app_id".dim(),
                                window.app_id
                            );
                            crate::outln!("    {}: {}", "title".dim(), window.title);
                            print_placement(window, "    ");
                            crate::outln!(
                                "    {} {}: {}",
                                "→".success(),
                                "sink".dim(),
                                track_info.sink_desc.as_str().bold()
                            );
                            print_fallbacks(track_info, "    ");
                            print_sources(track_info, "    ");
                        }
                    }
                }

                if !untracked.is_empty() {
                    crate::outln!(
                        "\n{} ({}):",
                        "Untracked".dim(),
                        untracked.len().to_string().technical()
                    );
                    for window in &untracked {
                        crate::outln!("  {} {}: {}", "•".dim(), "app_id".dim(), window.app_id);
                        crate::outln!("    {}: {}", "title".dim(), window.title);
                        print_placement(window, "    ");
                    }
                }
            }
            Ok(())
        }
        Response::Error { message } => {
            eyre::bail!("{message}");
        }
        other => Err(unexpected_response(&other)),
    }
}

/// Print the monitors a window is on (and whether it is minimized), when known
fn print_placement(window: &ipc::WindowInfo, indent: &str) {
    if let Some(line) = placement_text(window) {
        crate::outln!("{indent}{}: {}", "monitor".dim(), line.as_str().technical());
    }
}

/// `HDMI-A-1, DP-1`, with ` (minimized)` appended; `None` when the daemon knows no
/// placement (the `ext` backend reports none)
fn placement_text(window: &ipc::WindowInfo) -> Option<String> {
    let minimized = if window.minimized { " (minimized)" } else { "" };
    match (window.outputs.as_slice(), window.minimized) {
        ([], false) => None,
        ([], true) => Some("none (minimized)".to_string()),
        (outputs, _) => Some(format!("{}{minimized}", outputs.join(", "))),
    }
}

/// Print a tracked window's fallback sinks (in priority order), if its rule has any
fn print_fallbacks(tracked: &ipc::TrackedInfo, indent: &str) {
    if tracked.fallbacks.is_empty() {
        return;
    }
    let descs: Vec<&str> = tracked.fallbacks.iter().map(|f| f.desc.as_str()).collect();
    crate::outln!(
        "{indent}  {}: {}",
        "then".dim(),
        descs.join(", ").as_str().bold()
    );
}

/// Print a tracked window's sources (first choice, then fallbacks), if its rule has
/// any
fn print_sources(tracked: &ipc::TrackedInfo, indent: &str) {
    if tracked.sources.is_empty() {
        return;
    }
    let descs: Vec<&str> = tracked.sources.iter().map(|f| f.desc.as_str()).collect();
    crate::outln!(
        "{indent}{} {}: {}",
        "→".success(),
        "source".dim(),
        descs.join(", then ").as_str().bold()
    );
}

/// Test a regex pattern against current windows
///
/// # Errors
/// Returns an error if the regex is invalid, no daemon is running, or IPC fails.
pub async fn test_rule(pattern: &str, json_output: bool) -> Result<()> {
    if !ipc::is_daemon_running().await {
        eyre::bail!("Daemon is not running. Start it with: pwsw daemon");
    }

    let response = ipc::send_request(Request::TestRule {
        pattern: pattern.to_string(),
    })
    .await?;

    match response {
        Response::RuleMatches { pattern, matches } => {
            if json_output {
                crate::outln!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "pattern": pattern,
                        "matches": matches,
                    }))?
                );
            } else {
                let pattern_len = pattern.len();
                crate::outln!("Testing pattern: {}", pattern.technical());
                crate::outln!("{}", "=".repeat(16 + pattern_len));
                if matches.is_empty() {
                    crate::outln!("{}", "No matches found.".dim());
                } else {
                    crate::outln!(
                        "{} ({}):",
                        "Matches".header(),
                        matches.len().to_string().technical()
                    );
                    for (i, window) in matches.iter().enumerate() {
                        let matched_on = window.matched_on.as_deref().unwrap_or("unknown");
                        crate::outln!(
                            "{}. {}: {}{}",
                            (i + 1).to_string().dim(),
                            "app_id".dim(),
                            window.app_id,
                            if matched_on == "app_id" || matched_on == "both" {
                                format!(" {}", "✓".success())
                            } else {
                                String::new()
                            }
                        );
                        crate::outln!(
                            "   {}: {}{}",
                            "title".dim(),
                            window.title,
                            if matched_on == "title" || matched_on == "both" {
                                format!(" {}", "✓".success())
                            } else {
                                String::new()
                            }
                        );
                        if let Some(tracked) = &window.tracked {
                            crate::outln!(
                                "   {}: {}",
                                "current rule's sink".dim(),
                                tracked.sink_desc.as_str().bold()
                            );
                            print_fallbacks(tracked, "   ");
                            print_sources(tracked, "   ");
                        }
                    }
                }
            }
            Ok(())
        }
        Response::Error { message } => {
            eyre::bail!("{message}");
        }
        other => Err(unexpected_response(&other)),
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    fn window(outputs: &[&str], minimized: bool) -> ipc::WindowInfo {
        ipc::WindowInfo {
            id: Some(1),
            app_id: "mpv".to_string(),
            title: "video".to_string(),
            outputs: outputs.iter().map(|o| (*o).to_string()).collect(),
            minimized,
            matched_on: None,
            tracked: None,
        }
    }

    #[test]
    fn test_placement_text() {
        assert_eq!(placement_text(&window(&[], false)), None);
        assert_eq!(
            placement_text(&window(&["HDMI-A-1"], false)).as_deref(),
            Some("HDMI-A-1")
        );
        assert_eq!(
            placement_text(&window(&["DP-1", "HDMI-A-1"], true)).as_deref(),
            Some("DP-1, HDMI-A-1 (minimized)")
        );
        assert_eq!(
            placement_text(&window(&[], true)).as_deref(),
            Some("none (minimized)")
        );
    }
}
