//! CLI commands
//!
//! `list-sinks` is local (`PipeWire` and the config); `status` works with or without
//! the daemon (it asks one when it runs); `list-windows`, `test-rule`, `shutdown`
//! and `pause`/`resume`/`toggle-pause` need the daemon (`IPC`). Manual switching
//! (`set-sink`, `next-sink`, `prev-sink`, `set-source`) goes through the daemon when
//! it runs and switches `PipeWire` directly otherwise. (`validate`, `init` and
//! `doctor` live in `bin/pwsw.rs`, `config` and `doctor`.)

use std::collections::HashSet;
use std::fmt::Write;

use anyhow::Result;
use tracing::{debug, info};

use crate::config::{Config, Endpoint};
use crate::ipc::{self, Request, Response};
use crate::manual::{self, ManualSwitch, ManualTarget, Probe};
use crate::notification::{get_sink_icon, get_source_icon, send_notification};
use crate::pipewire::{
    ActiveSink, ActiveSinkJson, ConfiguredSinkJson, ListSinksJson, NodeState, PipeWire,
    ProfileSink, ProfileSinkJson, Snapshot,
};
use crate::style::PwswStyle;

// ============================================================================
// Local Commands (no daemon needed)
// ============================================================================

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
                    status: NodeState::of(&s.name, &self.active, &self.profile)
                        .json_status()
                        .to_string(),
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
                    let desc = term(&s.desc);
                    let mut m = String::with_capacity(3 + desc.len());
                    m.push_str(" [");
                    m.push_str(&desc);
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
                let marker = if node.is_default { "*" } else { " " };
                crate::outln!(
                    "  {} {}{}",
                    marker.success(),
                    term(&node.name).as_ref().bold(),
                    configured_marker(&node.name)
                );
                crate::outln!("      {}", term(&node.description).as_ref().dim());
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
                    term(&node.predicted_name).as_ref().bold(),
                    configured_marker(&node.predicted_name)
                );
                crate::outln!(
                    "      {} (profile: {})",
                    term(&node.description).as_ref().dim(),
                    term(&node.profile_name).as_ref().technical()
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
        if entries.is_empty() {
            crate::outln!(
                "  {}",
                format!("(none; add one with {})", config_command("tui")).dim()
            );
        }
        for (i, entry) in entries.iter().enumerate() {
            let default_marker = if entry.default {
                let mut m = String::with_capacity(3 + "DEFAULT".len());
                let _ = write!(m, " [{}]", "DEFAULT".dim());
                m
            } else {
                String::new()
            };
            let state = NodeState::of(&entry.name, &self.active, &self.profile);
            let status = match state {
                NodeState::Current | NodeState::Present => state.word().success().to_string(),
                NodeState::ViaProfile => state.word().warning().to_string(),
                NodeState::Missing => state.word().error().to_string(),
            };
            crate::outln!(
                "  {}. \"{}\"{} - {}",
                (i + 1).to_string().dim(),
                term(&entry.desc),
                default_marker,
                status
            );
            crate::outln!("     {}", term(&entry.name).as_ref().bold());
        }
    }
}

/// List all available sinks (active and profile-switch), then the mics (sources),
/// so one command shows everything a config can name
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
            "MICS (SOURCES):",
            "MICS AVAILABLE VIA PROFILE SWITCH:",
            "CONFIGURED MICS:",
        ]);

        if let Ok(path) = Config::get_config_path() {
            crate::outln!(
                "\n{} {}{}",
                "Config:".dim(),
                path.display(),
                config_path_note(&path)
            );
        }
    }

    Ok(())
}

/// What to add after a config path that has no file: `" (not created yet: pwsw init)"`
/// (with `--config` when needed), or the missing target of a dangling symlink; empty
/// when the file exists
fn config_path_note(path: &std::path::Path) -> String {
    if let Some(target) = crate::config::dangling_symlink_target(path) {
        return format!(" (symlink to missing {})", target.display())
            .warning()
            .to_string();
    }
    if std::fs::symlink_metadata(path).is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound) {
        let init = crate::config::pwsw_command(path, "init");
        return format!(" (not created yet: {init})").warning().to_string();
    }
    String::new()
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
    warn_positional(config, Endpoint::Sink, sink_ref);
    if ipc::is_daemon_running().await {
        return switch_via_daemon(Request::SetSink {
            sink: sink_ref.to_string(),
        })
        .await;
    }
    // The smart toggle compares with the real default sink
    switch_locally(config, &ManualSwitch::Set(sink_ref.to_string()))
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
    warn_positional(config, Endpoint::Source, source_ref);
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
    // Starts from the real default sink and skips sinks that cannot be activated
    // right now, like the daemon does
    let request = match direction {
        Direction::Next => ManualSwitch::Next,
        Direction::Prev => ManualSwitch::Prev,
    };
    switch_locally(config, &request)
}

/// Send a manual switch request to the daemon and print the outcome
async fn switch_via_daemon(request: Request) -> Result<()> {
    match ipc::send_request(request).await? {
        Response::SinkSwitched {
            desc, toggled_from, ..
        } => {
            match toggled_from {
                Some(from) => crate::outln!("{}", toggled_back_line(&from, &desc)),
                None => crate::outln!("{} {}", "Switched to:".success(), desc.as_str().bold()),
            }
            Ok(())
        }
        Response::Ok { message } => {
            crate::outln!("{message}");
            Ok(())
        }
        Response::Error { message } => anyhow::bail!("{message}"),
        other => Err(unexpected_response(&other)),
    }
}

/// The line for a smart toggle: `Headset was already active: toggled back to Speakers`
fn toggled_back_line(from: &str, to: &str) -> String {
    format!(
        "{} was already active: {} {}",
        from.bold(),
        "toggled back to".success(),
        to.bold()
    )
}

/// The deprecation warning for a `set-sink`/`set-source` reference by position (it
/// silently means another entry once the list is reordered); `None` for other
/// references
fn positional_warning(config: &Config, kind: Endpoint, r: &str) -> Option<String> {
    // Exactly what resolving the reference takes for a position (` 2` is none)
    let position = crate::config::positional_ref(r)?;
    let entry = config.endpoints(kind).get(position.checked_sub(1)?)?;
    let command = match kind {
        Endpoint::Sink => "set-sink",
        Endpoint::Source => "set-source",
    };
    Some(format!(
        "{command} {position} refers to a {} by position, which is deprecated (positions \
         change when entries are reordered); use its description instead: {:?}",
        kind.human_noun(),
        entry.desc
    ))
}

/// Print the deprecation warning for a positional reference, if it is one (stderr)
fn warn_positional(config: &Config, kind: Endpoint, r: &str) {
    if let Some(warning) = positional_warning(config, kind, r) {
        crate::errln!("{} {warning}", "warning:".warning().to_stderr());
    }
}

/// `pwsw <args>` for the config in use (with `--config <path>` when it is not the
/// default one), for hints
#[must_use]
pub fn config_command(args: &str) -> String {
    match Config::get_config_path() {
        Ok(path) => crate::config::pwsw_command(&path, args),
        Err(_) => format!("pwsw {args}"),
    }
}

/// Error for a reply that does not fit the request (likely a version mismatch)
fn unexpected_response(response: &Response) -> anyhow::Error {
    anyhow::anyhow!(
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
        Response::StatusUpdate(_) => "StatusUpdate",
    }
}

/// Carry out a manual request without the daemon: probe `PipeWire`, decide with the
/// daemon's own rules ([`manual::plan`]), activate, print and notify
fn switch_locally(config: &Config, request: &ManualSwitch) -> Result<()> {
    if let Some(answer) = manual::answer_without_pipewire(config, request) {
        let message = answer.map_err(|message| anyhow::anyhow!("{message}"))?;
        crate::outln!("{}", message.warning());
        return Ok(());
    }

    let probe = Probe::from_snapshot(&PipeWire::dump()?, config);
    let (target, toggled_from) = match manual::plan(config, request, &probe) {
        Ok(ManualTarget::Switch(target)) => (target, None),
        Ok(ManualTarget::ToggleBack { from, to }) => (to, Some(from)),
        Ok(ManualTarget::Done(message)) => {
            crate::outln!("{message}");
            return Ok(());
        }
        Err(message) => anyhow::bail!("{message}"),
    };

    let kind = request.kind();
    info!("Switching to: {}", target.desc);
    PipeWire::activate(kind, &target.name)?;
    match toggled_from {
        Some(from) => crate::outln!("{}", toggled_back_line(&from.desc, &target.desc)),
        None => crate::outln!(
            "{} {}",
            "Switched to:".success(),
            target.desc.as_str().bold()
        ),
    }

    if config.settings.notify_manual {
        let (title, icon) = match kind {
            Endpoint::Sink => ("Audio Output", get_sink_icon(target)),
            Endpoint::Source => ("Audio Input", get_source_icon(target)),
        };
        // The switch worked; a missing notification server is not worth a warning
        if let Err(e) = send_notification(title, &target.desc, Some(&icon)) {
            debug!("Notification failed: {e:#}");
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
    // A config that does not exist yet is not broken: `pwsw tui` creates it (unless
    // it is a symlink into a missing directory: that is a config problem to fix first)
    let config_ok = config.as_ref().map_or_else(
        |e| {
            e.downcast_ref::<crate::config::ConfigNotFound>()
                .is_some_and(|missing| crate::config::missing_symlink_dir(&missing.path).is_none())
        },
        |_| true,
    );
    let (config, config_error) = match config {
        Ok(config) => (Some(config), None),
        Err(e) => (None, Some(format!("{e:#}"))),
    };

    // `PipeWire` is queried directly (works with or without daemon)
    let snapshot = match tokio::task::spawn_blocking(PipeWire::dump).await {
        Ok(result) => result,
        Err(e) => Err(anyhow::anyhow!("PipeWire query task failed: {e}")),
    };
    let (current_sink, current_source, pipewire_error) = match snapshot {
        Ok(snapshot) => {
            let sink = describe_default(&snapshot, Endpoint::Sink, config.as_ref());
            let source = describe_default(&snapshot, Endpoint::Source, config.as_ref());
            let error = sink
                .is_none()
                .then(|| "No default sink found in PipeWire metadata".to_string());
            (sink, source, error)
        }
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
    let config_loaded = config.is_some();
    match &current_sink {
        Some((_, description)) => {
            crate::outln!(
                "{} {}{}",
                "Current:".dim(),
                term(description).as_ref().bold(),
                outside_note(description, &daemon, Endpoint::Sink, config_loaded)
            );
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
                crate::outln!(
                    "{} {}{}",
                    "Mic:".dim(),
                    term(description).as_ref().bold(),
                    outside_note(description, &daemon, Endpoint::Source, config_loaded)
                );
            }
            None => crate::outln!("{} {}", "Mic:".dim(), "unknown".dim()),
        }
    }
    crate::outln!();
    crate::outln!("{}", "Daemon".header());
    crate::outln!("{}", "-".repeat(6));

    if !daemon.running {
        crate::outln!("{} {}", "Status:".dim(), "Not running".error());
        let has_sinks = config.as_ref().is_some_and(|c| !c.sinks.is_empty());
        let unit_installed = has_sinks
            && tokio::task::spawn_blocking(crate::daemon_manager::DaemonManager::unit_installed)
                .await
                .unwrap_or(false);
        let hint = crate::daemon_manager::StartHint::new(config_ok, has_sinks, unit_installed);
        crate::outln!("  {}", hint.text().as_str().technical());
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
                paused_line(sources_configured).warning()
            );
        }
        if daemon.error.is_some() {
            crate::outln!(
                "  {}",
                "IPC protocol mismatch: restart the daemon".warning()
            );
        }
        if let Some(rule) = &daemon.active_window {
            // It holds the deciding window's title: untrusted text
            crate::outln!(
                "{} {}",
                "Active Rule:".dim(),
                active_rule_text(rule).technical()
            );
        }
        if let Some(source) = &daemon.daemon_source {
            let fallback = if daemon.daemon_source_fallback == Some(true) {
                " (fallback)"
            } else {
                ""
            };
            crate::outln!(
                "{} {}{fallback}",
                "Applied Mic:".dim(),
                term(source).as_ref().technical()
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

/// `pwsw status`'s `Active Rule:` value: the deciding window's trigger (its title when
/// the rule has no `desc`) escaped and shortened like other window text
fn active_rule_text(rule: &str) -> String {
    term(rule).into_owned()
}

/// The current `kind` default as `pwsw status` shows it: its node name, and its
/// description: the configured entry it is (by name, or a renamed node recognised
/// like activation does, see [`PipeWire::configured_default`]), else the node name
fn describe_default(
    snapshot: &Snapshot,
    kind: Endpoint,
    config: Option<&Config>,
) -> Option<(String, String)> {
    let name = snapshot.current_default_of(kind)?.to_string();
    let description = config
        .and_then(|c| PipeWire::configured_default(snapshot, kind, c.endpoints(kind)))
        .map_or_else(|| name.clone(), |entry| entry.desc.clone());
    Some((name, description))
}

/// ` (changed outside pwsw)` when a running daemon applied another `kind` than the
/// real `PipeWire` default (`description`, see [`describe_default`]); empty
/// otherwise, also when this command could not load the config (`config_loaded`
/// false: the default cannot be described as the daemon describes it)
fn outside_note(
    description: &str,
    daemon: &DaemonStatus,
    kind: Endpoint,
    config_loaded: bool,
) -> String {
    let applied = match kind {
        Endpoint::Sink => daemon.daemon_sink.as_deref(),
        Endpoint::Source => daemon.daemon_source.as_deref(),
    };
    if config_loaded
        && daemon.running
        && daemon.error.is_none()
        && applied.is_some_and(|applied| applied != description)
    {
        format!(" {}", "(changed outside pwsw)".warning())
    } else {
        String::new()
    }
}

/// Gracefully shutdown the daemon
///
/// # Errors
/// Returns an error if no daemon is running or IPC communication fails.
pub async fn shutdown() -> Result<()> {
    if !ipc::is_daemon_running().await {
        anyhow::bail!("Daemon is not running");
    }

    let response = ipc::send_request(Request::Shutdown).await?;

    match response {
        Response::Ok { message } => {
            crate::outln!("{}", message.success());
            Ok(())
        }
        Response::Error { message } => {
            anyhow::bail!("{message}");
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
        anyhow::bail!("Daemon is not running. Start it with: pwsw daemon");
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
        Response::Error { message } => anyhow::bail!("{message}"),
        other => Err(unexpected_response(&other)),
    }
}

/// `pwsw status`'s `Switching:` value while paused; names the mic only when sources
/// are configured
const fn paused_line(sources_configured: bool) -> &'static str {
    if sources_configured {
        "Paused (rules do not switch the output or mic; resume with: pwsw resume)"
    } else {
        "Paused (rules do not switch the audio output; resume with: pwsw resume)"
    }
}

/// The line `pause`/`resume`/`toggle-pause` print for the daemon's answer (worded
/// for any setup: these commands do not load the config)
fn pause_message(paused: bool, changed: bool) -> crate::style::Painted<&'static str> {
    match (paused, changed) {
        (true, true) => "Paused: rules will not switch audio (resume with: pwsw resume)".warning(),
        (true, false) => "Already paused (resume with: pwsw resume)".warning(),
        (false, true) => "Resumed: rules switch audio again".success(),
        (false, false) => "Not paused: rules are switching audio".success(),
    }
}

/// Get list of windows currently tracked by daemon
///
/// # Errors
/// Returns an error if no daemon is running or IPC communication fails.
pub async fn list_windows(json_output: bool) -> Result<()> {
    if !ipc::is_daemon_running().await {
        anyhow::bail!("Daemon is not running. Start it with: pwsw daemon");
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
                                term(&window.app_id)
                            );
                            crate::outln!("    {}: {}", "title".dim(), term(&window.title));
                            print_placement(window, "    ");
                            crate::outln!(
                                "    {} {}: {}",
                                "→".success(),
                                "sink".dim(),
                                rule_sink_text(track_info)
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
                        crate::outln!(
                            "  {} {}: {}",
                            "•".dim(),
                            "app_id".dim(),
                            term(&window.app_id)
                        );
                        crate::outln!("    {}: {}", "title".dim(), term(&window.title));
                        print_placement(window, "    ");
                    }
                }
            }
            Ok(())
        }
        Response::Error { message } => {
            anyhow::bail!("{message}");
        }
        other => Err(unexpected_response(&other)),
    }
}

/// A tracked window's rule sink, and for the window deciding the daemon's sink the
/// fallback that plays instead (`BT Headphones (fallback in use: Headset)`)
fn rule_sink_text(tracked: &ipc::TrackedInfo) -> String {
    let sink = tracked.sink_desc.as_str().bold().to_string();
    match &tracked.fallback_in_use {
        Some(fallback) => format!(
            "{sink} {}",
            format!("(fallback in use: {})", fallback.desc).warning()
        ),
        None => sink,
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
        (outputs, _) => Some(format!("{}{minimized}", term(&outputs.join(", ")))),
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
        "mic".dim(),
        descs.join(", then ").as_str().bold()
    );
}

/// The note `test-rule --output` prints when the daemon reports no monitors (the `ext`
/// backend, or a daemon from before protocol 2 that does not send them at all)
const fn no_monitor_info_hint(protocol: u32, monitor_info: bool) -> Option<&'static str> {
    if protocol >= 2 && monitor_info {
        return None;
    }
    Some(
        "Note: this daemon reports no monitor information (ext backend or older version), \
         so --output matches no window",
    )
}

/// Untrusted text (window title, `app_id`, monitor name, `PipeWire` node name or
/// description, configured string) for the terminal: control and bidirectional
/// characters escaped, so it can neither break the layout nor send escape sequences.
/// Display only: matching and the configuration use the raw value.
fn term(text: &str) -> std::borrow::Cow<'_, str> {
    crate::sanitize::for_log(text)
}

/// Test a regex pattern (against `app_id` and title) and/or an `output` regex (against
/// the monitors a window is on, as a rule's `output` condition) against current windows
///
/// # Errors
/// Returns an error if a regex is invalid, no daemon is running, or IPC fails.
pub async fn test_rule(
    pattern: Option<&str>,
    output: Option<&str>,
    json_output: bool,
) -> Result<()> {
    let output_regex = output
        .map(crate::config::compile_rule_regex)
        .transpose()
        .map_err(|e| anyhow::anyhow!("Invalid output regex: {e}"))?;
    if !ipc::is_daemon_running().await {
        anyhow::bail!("Daemon is not running. Start it with: pwsw daemon");
    }

    let request = match pattern {
        Some(pattern) => Request::TestRule {
            pattern: pattern.to_string(),
        },
        None => Request::ListWindows,
    };
    let (pattern, mut matches) = match ipc::send_request(request).await? {
        Response::RuleMatches { pattern, matches } => (Some(pattern), matches),
        Response::Windows { windows } => (None, windows),
        Response::Error { message } => anyhow::bail!("{message}"),
        other => return Err(unexpected_response(&other)),
    };
    if let Some(regex) = &output_regex {
        // Same semantics as `Rule::matches`: never a minimized or unplaced window
        matches.retain(|w| !w.minimized && w.outputs.iter().any(|o| regex.is_match(o)));
        // Without monitor information every window has no outputs: say why nothing
        // matches (on stderr, so `--json` stays parseable)
        if let Ok(Response::Status {
            protocol,
            monitor_info,
            ..
        }) = ipc::send_request(Request::Status).await
            && let Some(hint) = no_monitor_info_hint(protocol, monitor_info)
        {
            crate::errln!("{}", hint.warning().to_stderr());
        }
    }

    if json_output {
        crate::outln!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "pattern": pattern,
                "output": output,
                "matches": matches,
            }))?
        );
    } else {
        let heading = match (&pattern, output) {
            (Some(p), Some(o)) => format!("{p} on output {o}"),
            (Some(p), None) => p.clone(),
            (None, o) => format!("output {}", o.unwrap_or_default()),
        };
        crate::outln!("Testing pattern: {}", heading.as_str().technical());
        crate::outln!("{}", "-".repeat(17 + heading.chars().count()));
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
                    term(&window.app_id),
                    if matched_on == "app_id" || matched_on == "both" {
                        format!(" {}", "✓".success())
                    } else {
                        String::new()
                    }
                );
                crate::outln!(
                    "   {}: {}{}",
                    "title".dim(),
                    term(&window.title),
                    if matched_on == "title" || matched_on == "both" {
                        format!(" {}", "✓".success())
                    } else {
                        String::new()
                    }
                );
                if output.is_some() {
                    crate::outln!(
                        "   {}: {} {}",
                        "monitor".dim(),
                        term(&window.outputs.join(", ")),
                        "✓".success()
                    );
                }
                if let Some(tracked) = &window.tracked {
                    crate::outln!(
                        "   {}: {}",
                        "current rule's sink".dim(),
                        rule_sink_text(tracked)
                    );
                    print_fallbacks(tracked, "   ");
                    print_sources(tracked, "   ");
                }
            }
        }
    }
    Ok(())
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

    /// `pwsw status` names the mic while paused only when sources are configured
    #[test]
    fn test_paused_line_fits_the_setup() {
        assert_eq!(
            paused_line(false),
            "Paused (rules do not switch the audio output; resume with: pwsw resume)"
        );
        assert!(paused_line(true).contains("output or mic"));
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
        // A monitor name is untrusted text: it never reaches the terminal raw
        let shown = placement_text(&window(&["DP-1\n\u{1b}[2J"], false)).expect("placed");
        assert!(
            !shown.contains('\n') && !shown.contains('\u{1b}'),
            "{shown:?}"
        );
    }

    #[test]
    fn test_term_escapes_control_characters() {
        assert_eq!(term("Firefox"), "Firefox");
        let title = term("evil\ntitle\u{1b}]0;x\u{7}");
        assert!(!title.chars().any(char::is_control), "{title:?}");
    }

    /// B5: the deciding window's title in `Active Rule:` never reaches the terminal
    /// raw (escape sequences, BEL, bidi overrides) and is shortened
    #[test]
    fn test_active_rule_text_is_sanitised() {
        let shown = active_rule_text(&format!(
            "Evil\u{1b}[31mRED\u{7} \u{202e}Title {}: TV",
            "x".repeat(400)
        ));
        assert!(
            !shown.contains('\u{1b}') && !shown.contains('\u{7}') && !shown.contains('\u{202e}'),
            "{shown:?}"
        );
        // Cut after 200 characters (escapes spelled out count extra)
        assert!(shown.ends_with('…'), "{shown}");
        assert!(shown.chars().count() < 240, "{}", shown.chars().count());
        assert_eq!(
            active_rule_text("Voice chat: Headset"),
            "Voice chat: Headset"
        );
    }

    /// `pwsw status` marks a default that differs from what the daemon applied
    #[test]
    fn test_outside_note() {
        let daemon = DaemonStatus {
            running: true,
            daemon_sink: Some("TV".to_string()),
            ..DaemonStatus::default()
        };
        assert_eq!(
            outside_note("Speakers", &daemon, Endpoint::Sink, true),
            " (changed outside pwsw)"
        );
        assert_eq!(outside_note("TV", &daemon, Endpoint::Sink, true), "");
        assert_eq!(
            outside_note("Desk Mic", &daemon, Endpoint::Source, true),
            "",
            "unknown applied source"
        );
        assert_eq!(
            outside_note("Speakers", &DaemonStatus::default(), Endpoint::Sink, true),
            "",
            "no daemon"
        );
        // B3b: without the config the default can only be named by its node, which
        // says nothing about whether it is what the daemon applied
        assert_eq!(
            outside_note(
                "alsa_output.pci-0000_0c_00.4.analog-stereo",
                &daemon,
                Endpoint::Sink,
                false
            ),
            "",
            "config not loaded"
        );
    }

    /// B3a: the default is described as the configured entry it is, also when
    /// `WirePlumber` gave the node another name (recognised by device and profile),
    /// so `status` does not mark a sink pwsw itself set as changed outside pwsw
    #[test]
    fn test_describe_default_recognises_a_renamed_node() {
        use crate::test_utils::fixtures::{make_config, make_sink};
        let iec = "alsa_output.pci-0000_0c_00.4.iec958-stereo";
        let mut snapshot = crate::pipewire::parse_snapshot(include_bytes!(
            "../tests/fixtures/pwdump-alsa-profiles.json"
        ))
        .expect("fixture parses");
        let renamed = format!("{iec}.2");
        let node = snapshot
            .sinks
            .iter_mut()
            .find(|n| n.name == iec)
            .expect("digital output");
        node.name.clone_from(&renamed);
        snapshot.defaults.configured = Some(renamed.clone());
        snapshot.defaults.effective = Some(renamed.clone());
        let config = make_config(
            vec![
                make_sink(
                    "alsa_output.pci-0000_0c_00.4.analog-stereo",
                    "Speakers",
                    true,
                ),
                make_sink(iec, "Optical", false),
            ],
            vec![],
        );
        assert_eq!(
            describe_default(&snapshot, Endpoint::Sink, Some(&config)),
            Some((renamed.clone(), "Optical".to_string()))
        );
        let daemon = DaemonStatus {
            running: true,
            daemon_sink: Some("Optical".to_string()),
            ..DaemonStatus::default()
        };
        assert_eq!(outside_note("Optical", &daemon, Endpoint::Sink, true), "");
        // Without the config: the node name
        assert_eq!(
            describe_default(&snapshot, Endpoint::Sink, None),
            Some((renamed.clone(), renamed))
        );
    }

    #[test]
    fn test_positional_warning() {
        use crate::test_utils::fixtures::{make_config, make_sink};
        let config = make_config(
            vec![
                make_sink("speakers", "Speakers", true),
                make_sink("hdmi", "HDMI", false),
            ],
            vec![],
        );
        let warning = positional_warning(&config, Endpoint::Sink, "2").expect("warns");
        assert!(warning.contains("set-sink 2"), "{warning}");
        assert!(warning.contains("deprecated"), "{warning}");
        assert!(warning.contains("\"HDMI\""), "{warning}");
        assert_eq!(positional_warning(&config, Endpoint::Sink, "HDMI"), None);
        assert_eq!(
            positional_warning(&config, Endpoint::Sink, "9"),
            None,
            "no entry there: the unknown-sink error says enough"
        );
        assert_eq!(positional_warning(&config, Endpoint::Source, "1"), None);
        assert_eq!(
            positional_warning(&config, Endpoint::Sink, " 2"),
            None,
            "not a position when resolving either (no warning before an unknown-sink error)"
        );
    }

    /// `test-rule --output` explains an empty result when the daemon has no monitor
    /// information
    #[test]
    fn test_no_monitor_info_hint() {
        assert_eq!(no_monitor_info_hint(2, true), None);
        assert!(no_monitor_info_hint(2, false).is_some(), "ext backend");
        assert!(no_monitor_info_hint(1, false).is_some(), "older daemon");
        assert!(no_monitor_info_hint(0, true).is_some());
    }
}
