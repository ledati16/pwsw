//! CLI commands
//!
//! Implements both local commands (list-sinks, validate) and IPC-based commands
//! that communicate with the daemon (status, list-windows, test-rule). Manual
//! switching (set-sink, next-sink, prev-sink) goes through the daemon when it runs.

use std::collections::HashSet;
use std::fmt::Write;

use color_eyre::eyre::{self, Result};
use tracing::{info, warn};

use crate::config::Config;
use crate::ipc::{self, Request, Response};
use crate::notification::{get_sink_icon, send_notification};
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

/// List all available sinks (active and profile-switch)
///
/// # Errors
/// Returns an error if `PipeWire` query fails or `JSON` serialization fails.
// Sink listing with both `JSON` and human-readable formatting - cohesive output logic
pub fn list_sinks(config: Option<&Config>, json_output: bool) -> Result<()> {
    let objects = PipeWire::dump()?;
    let active = PipeWire::get_active_sinks(&objects);
    let profile = PipeWire::get_profile_sinks(&objects, &active);

    let current_default = active.iter().find(|s| s.is_default).map(|s| s.name.clone());

    if json_output {
        let configured_names: HashSet<&str> =
            config.map_or_default(|c| c.sinks.iter().map(|s| s.name.as_str()).collect());

        let output = ListSinksJson {
            active_sinks: active
                .iter()
                .map(|s| ActiveSinkJson {
                    name: s.name.clone(),
                    description: s.description.clone(),
                    is_default: s.is_default,
                    configured: configured_names.contains(s.name.as_str()),
                })
                .collect(),
            profile_sinks: profile
                .iter()
                .map(|s| ProfileSinkJson {
                    predicted_name: s.predicted_name.clone(),
                    description: s.description.clone(),
                    device_name: s.device_name.clone(),
                    profile_name: s.profile_name.clone(),
                    profile_index: s.profile_index,
                })
                .collect(),
            configured_sinks: config.map_or_default(|c| {
                c.sinks
                    .iter()
                    .enumerate()
                    .map(|(i, s)| {
                        let status = get_sink_status(&s.name, &active, &profile);
                        ConfiguredSinkJson {
                            index: i + 1,
                            name: s.name.clone(),
                            desc: s.desc.clone(),
                            icon: s.icon.clone(),
                            is_default_config: s.default,
                            status: status.to_string(),
                        }
                    })
                    .collect()
            }),
            current_default,
        };

        println!("{}", serde_json::to_string_pretty(&output)?);
    } else {
        // Human-readable output
        println!("{}", "ACTIVE SINKS:".header());
        println!("{}", "-".repeat(13));
        if active.is_empty() {
            println!("  {}", "(none)".dim());
        } else {
            for sink in &active {
                let marker = if sink.is_default { "* " } else { "  " };
                let configured = config
                    .and_then(|c| c.sinks.iter().find(|s| s.name == sink.name))
                    .map_or_default(|s| {
                        let mut m = String::with_capacity(3 + s.desc.len());
                        m.push_str(" [");
                        m.push_str(&s.desc);
                        m.push(']');
                        m
                    });
                println!("{}{}{}", marker, sink.name.as_str().bold(), configured);
                println!("    {}", sink.description.as_str().dim());
            }
            println!("\n  {} = current default", "*".dim());
        }

        if !profile.is_empty() {
            println!("\n{}", "AVAILABLE VIA PROFILE SWITCH:".header());
            println!("{}", "-".repeat(30));
            for sink in &profile {
                let configured = config
                    .and_then(|c| c.sinks.iter().find(|s| s.name == sink.predicted_name))
                    .map_or_default(|s| {
                        let mut m = String::with_capacity(3 + s.desc.len());
                        m.push_str(" [");
                        m.push_str(&s.desc);
                        m.push(']');
                        m
                    });
                println!(
                    "  {} {}{}",
                    "~".dim(),
                    sink.predicted_name.as_str().bold(),
                    configured
                );
                println!(
                    "    {} (profile: {})",
                    sink.description.as_str().dim(),
                    sink.profile_name.as_str().technical()
                );
            }
        }

        if let Some(cfg) = config {
            println!("\n{}", "CONFIGURED SINKS:".header());
            println!("{}", "-".repeat(17));
            for (i, sink) in cfg.sinks.iter().enumerate() {
                let default_marker = if sink.default {
                    let mut m = String::with_capacity(3 + "DEFAULT".len());
                    let _ = write!(m, " [{}]", "DEFAULT".dim());
                    m
                } else {
                    String::new()
                };
                let status = match get_sink_status(&sink.name, &active, &profile) {
                    "active" => "active".success().to_string(),
                    "requires_profile_switch" => "profile switch".warning().to_string(),
                    _ => "not found".error().to_string(),
                };
                println!(
                    "  {}. \"{}\"{} - {}",
                    (i + 1).to_string().dim(),
                    sink.desc,
                    default_marker,
                    status
                );
                println!("     {}", sink.name.as_str().bold());
            }
        }

        if let Ok(path) = Config::get_config_path() {
            println!("\n{} {}", "Config:".dim(), path.display());
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
            println!("{} {}", "Switched to:".success(), desc.as_str().bold());
            Ok(())
        }
        Response::Ok { message } => {
            println!("{message}");
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
    }
}

/// Set sink with smart toggle support, switching locally (no daemon)
///
/// # Errors
/// Returns an error if the sink reference is invalid or sink activation fails.
pub fn set_sink_smart(config: &Config, sink_ref: &str) -> Result<()> {
    let target = config.resolve_sink(sink_ref).ok_or_else(|| {
        let available: Vec<_> = config
            .sinks
            .iter()
            .enumerate()
            .map(|(i, s)| format!("{}. '{}'", i + 1, s.desc))
            .collect();
        eyre::eyre!(
            "Unknown sink '{}'. Available: {}",
            sink_ref,
            available.join(", ")
        )
    })?;

    let current = PipeWire::get_default_sink_name()?;
    let default = config
        .get_default_sink()
        .ok_or_else(|| eyre::eyre!("No default sink configured"))?;

    if config.settings.set_smart_toggle && current == target.name {
        if target.name == default.name {
            println!("Already on: {}", default.desc.as_str().bold());
            return Ok(());
        }
        info!("Toggle → default: {}", default.desc);
        PipeWire::activate_sink(&default.name)?;
        println!(
            "{} {}",
            "Switched to:".success(),
            default.desc.as_str().bold()
        );

        if config.settings.notify_manual {
            let icon = get_sink_icon(default);
            if let Err(e) = send_notification("Audio Output", &default.desc, Some(&icon)) {
                warn!("Notification failed: {}", e);
            }
        }
    } else {
        info!("Switching to: {}", target.desc);
        PipeWire::activate_sink(&target.name)?;
        println!(
            "{} {}",
            "Switched to:".success(),
            target.desc.as_str().bold()
        );

        if config.settings.notify_manual {
            let icon = get_sink_icon(target);
            if let Err(e) = send_notification("Audio Output", &target.desc, Some(&icon)) {
                warn!("Notification failed: {}", e);
            }
        }
    }

    Ok(())
}

/// Cycle through configured sinks, switching locally (no daemon)
///
/// # Errors
/// Returns an error if sink query or activation fails.
pub fn cycle_sink(config: &Config, direction: Direction) -> Result<()> {
    // Need at least 2 sinks to cycle
    if config.sinks.len() < 2 {
        println!("{}", "Only one sink configured, nothing to cycle".warning());
        return Ok(());
    }

    let current = PipeWire::get_default_sink_name()?;

    // Find current sink's index in config, or start from default
    let current_index = config
        .sinks
        .iter()
        .position(|s| s.name == current)
        .unwrap_or_else(|| {
            // Current sink not in config, find default's index
            config.sinks.iter().position(|s| s.default).unwrap_or(0)
        });

    // Calculate next index with wrapping
    let next_index = match direction {
        Direction::Next => (current_index + 1) % config.sinks.len(),
        Direction::Prev => {
            if current_index == 0 {
                config.sinks.len() - 1
            } else {
                current_index - 1
            }
        }
    };

    let target = &config.sinks[next_index];

    // Already on target (shouldn't happen with >= 2 sinks, but be safe)
    if target.name == current {
        println!("Already on: {}", target.desc.as_str().bold());
        return Ok(());
    }

    info!("Cycling to: {}", target.desc);
    PipeWire::activate_sink(&target.name)?;
    println!(
        "{} {}",
        "Switched to:".success(),
        target.desc.as_str().bold()
    );

    if config.settings.notify_manual {
        let icon = get_sink_icon(target);
        if let Err(e) = send_notification("Audio Output", &target.desc, Some(&icon)) {
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
    active_window: Option<String>,
    tracked_windows: Option<usize>,
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
                active_window,
                tracked_windows,
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
                daemon_sink: Some(current_sink),
                active_window,
                tracked_windows: Some(tracked_windows),
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
            "active_window": self.active_window,
            "tracked_windows": self.tracked_windows,
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
    let current_sink = match tokio::task::spawn_blocking(PipeWire::get_default_sink_name).await {
        Ok(result) => result,
        Err(e) => Err(eyre::eyre!("PipeWire query task failed: {e}")),
    };
    let (current_sink, pipewire_error) = match current_sink {
        Ok(name) => {
            let description = config
                .as_ref()
                .and_then(|c| c.sinks.iter().find(|s| s.name == name))
                .map_or_else(|| name.clone(), |s| s.desc.clone());
            (Some((name, description)), None)
        }
        Err(e) => (None, Some(format!("{e:#}"))),
    };

    let daemon = DaemonStatus::query().await;

    let healthy = config_error.is_none() && pipewire_error.is_none() && daemon.error.is_none();

    if json_output {
        let current_sink_json = current_sink.as_ref().map(
            |(name, description)| serde_json::json!({ "name": name, "description": description }),
        );
        let output = serde_json::json!({
            "current_sink": current_sink_json,
            "daemon": daemon.to_json(),
            "pipewire": {
                "current_sink": current_sink_json,
                "error": pipewire_error,
            },
            "config": {
                "path": config_path.as_ref().map(|p| p.display().to_string()),
                "error": config_error,
            },
        });
        // A `Value` always serializes; fall back to compact output just in case
        let text = serde_json::to_string_pretty(&output).unwrap_or_else(|_| output.to_string());
        println!("{text}");
        return true;
    }

    for error in [&config_error, &pipewire_error, &daemon.error]
        .into_iter()
        .flatten()
    {
        eprintln!("{} {error}", "error:".error().to_stderr());
    }

    println!("{}", "Audio Output".header());
    println!("{}", "-".repeat(12));
    match &current_sink {
        Some((_, description)) => println!("{} {}", "Current:".dim(), description.as_str().bold()),
        None => println!(
            "{} {}",
            "Current:".dim(),
            "unavailable (PipeWire error)".error()
        ),
    }
    println!();
    println!("{}", "Daemon".header());
    println!("{}", "-".repeat(6));

    if !daemon.running {
        println!("{} {}", "Status:".dim(), "Not running".error());
        println!("  Start with: {}", "pwsw daemon".technical());
    } else if let Some(uptime_secs) = daemon.uptime_secs {
        println!(
            "{} {}",
            "Status:".dim(),
            format!("Running (uptime: {})", crate::format_uptime(uptime_secs)).success()
        );
        if let Some(version) = &daemon.version {
            println!("{} {}", "Version:".dim(), version);
        }
        if daemon.error.is_some() {
            println!(
                "  {}",
                "IPC protocol mismatch: restart the daemon".warning()
            );
        }
        if let Some(rule) = &daemon.active_window {
            println!("{} {}", "Active Rule:".dim(), rule.as_str().technical());
        }
        if let Some(tracked) = daemon.tracked_windows {
            println!(
                "{} {}",
                "Tracked Windows:".dim(),
                tracked.to_string().technical()
            );
        }
    } else {
        println!(
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
            println!("{}", message.success());
            Ok(())
        }
        Response::Error { message } => {
            eyre::bail!("{message}");
        }
        other => Err(unexpected_response(&other)),
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
                println!("{}", serde_json::to_string_pretty(&windows)?);
            } else if windows.is_empty() {
                println!("{}", "No windows currently open.".dim());
            } else {
                let tracked: Vec<_> = windows.iter().filter(|w| w.tracked.is_some()).collect();
                let untracked: Vec<_> = windows.iter().filter(|w| w.tracked.is_none()).collect();

                let header = format!(
                    "All Windows ({} open, {} tracked):",
                    windows.len(),
                    tracked.len()
                );
                let header_len = header.len();
                println!("{}", header.header());
                println!("{}", "-".repeat(header_len));

                if !tracked.is_empty() {
                    println!(
                        "\n{} ({}):",
                        "Tracked".success(),
                        tracked.len().to_string().technical()
                    );
                    for window in &tracked {
                        if let Some(ref track_info) = window.tracked {
                            println!("  {} {}: {}", "•".success(), "app_id".dim(), window.app_id);
                            println!("    {}: {}", "title".dim(), window.title);
                            println!(
                                "    {} {}: {}",
                                "→".success(),
                                "sink".dim(),
                                track_info.sink_desc.as_str().bold()
                            );
                        }
                    }
                }

                if !untracked.is_empty() {
                    println!(
                        "\n{} ({}):",
                        "Untracked".dim(),
                        untracked.len().to_string().technical()
                    );
                    for window in &untracked {
                        println!("  {} {}: {}", "•".dim(), "app_id".dim(), window.app_id);
                        println!("    {}: {}", "title".dim(), window.title);
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
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "pattern": pattern,
                        "matches": matches,
                    }))?
                );
            } else {
                let pattern_len = pattern.len();
                println!("Testing pattern: {}", pattern.technical());
                println!("{}", "=".repeat(16 + pattern_len));
                if matches.is_empty() {
                    println!("{}", "No matches found.".dim());
                } else {
                    println!(
                        "{} ({}):",
                        "Matches".header(),
                        matches.len().to_string().technical()
                    );
                    for (i, window) in matches.iter().enumerate() {
                        let matched_on = window.matched_on.as_deref().unwrap_or("unknown");
                        println!(
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
                        println!(
                            "   {}: {}{}",
                            "title".dim(),
                            window.title,
                            if matched_on == "title" || matched_on == "both" {
                                format!(" {}", "✓".success())
                            } else {
                                String::new()
                            }
                        );
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
