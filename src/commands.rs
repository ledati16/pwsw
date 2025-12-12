//! CLI commands
//!
//! Implements both local commands (list-sinks, validate) and IPC-based commands
//! that communicate with the daemon (status, reload, list-windows, test-rule).

use anyhow::Result;
use std::collections::HashSet;
use tracing::{info, warn};

use crate::config::Config;
use crate::ipc::{self, Request, Response};
use crate::notification::{get_notification_sink_icon, send_notification};
use crate::pipewire::{
    ActiveSink, ActiveSinkJson, ConfiguredSinkJson, ListSinksJson, PipeWire, ProfileSink,
    ProfileSinkJson,
};

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
/// Returns an error if `PipeWire` query fails or JSON serialization fails.
pub fn list_sinks(config: Option<&Config>, json_output: bool) -> Result<()> {
    let objects = PipeWire::dump()?;
    let active = PipeWire::get_active_sinks(&objects);
    let profile = PipeWire::get_profile_sinks(&objects, &active);

    let current_default = active.iter().find(|s| s.is_default).map(|s| s.name.clone());

    if json_output {
        let configured_names: HashSet<&str> = config
            .map(|c| c.sinks.iter().map(|s| s.name.as_str()).collect())
            .unwrap_or_default();

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
            configured_sinks: config
                .map(|c| {
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
                })
                .unwrap_or_default(),
            current_default,
        };

        println!("{}", serde_json::to_string_pretty(&output)?);
    } else {
        // Human-readable output
        let header = "ACTIVE SINKS:";
        println!("{header}");
        println!("{}", "-".repeat(header.len()));
        if active.is_empty() {
            println!("  (none)");
        } else {
            for sink in &active {
                let marker = if sink.is_default { "* " } else { "  " };
                let configured = config
                    .and_then(|c| c.sinks.iter().find(|s| s.name == sink.name))
                    .map(|s| format!(" [{}]", s.desc))
                    .unwrap_or_default();
                println!("{}{}{}", marker, sink.name, configured);
                println!("    {}", sink.description);
            }
            println!("\n  * = current default");
        }

        if !profile.is_empty() {
            let header = "AVAILABLE VIA PROFILE SWITCH:";
            println!("\n{header}");
            println!("{}", "-".repeat(header.len()));
            for sink in &profile {
                let configured = config
                    .and_then(|c| c.sinks.iter().find(|s| s.name == sink.predicted_name))
                    .map(|s| format!(" [{}]", s.desc))
                    .unwrap_or_default();
                println!("  ~ {}{}", sink.predicted_name, configured);
                println!("    {} (profile: {})", sink.description, sink.profile_name);
            }
        }

        if let Some(cfg) = config {
            let header = "CONFIGURED SINKS:";
            println!("\n{header}");
            println!("{}", "-".repeat(header.len()));
            for (i, sink) in cfg.sinks.iter().enumerate() {
                let default_marker = if sink.default { " [DEFAULT]" } else { "" };
                let status = match get_sink_status(&sink.name, &active, &profile) {
                    "active" => "active",
                    "requires_profile_switch" => "profile switch",
                    _ => "not found",
                };
                println!(
                    "  {}. \"{}\"{} - {}",
                    i + 1,
                    sink.desc,
                    default_marker,
                    status
                );
                println!("     {}", sink.name);
            }
        }

        if let Ok(path) = Config::get_config_path() {
            println!("\nConfig: {}", path.display());
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

/// Set sink with smart toggle support
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
        anyhow::anyhow!(
            "Unknown sink '{}'. Available: {}",
            sink_ref,
            available.join(", ")
        )
    })?;

    let current = PipeWire::get_default_sink_name()?;
    let default = config
        .get_default_sink()
        .ok_or_else(|| anyhow::anyhow!("No default sink configured"))?;

    if config.settings.set_smart_toggle && current == target.name {
        if target.name == default.name {
            println!("Already on: {}", default.desc);
            return Ok(());
        }
        info!("Toggle → default: {}", default.desc);
        PipeWire::activate_sink(&default.name)?;
        println!("Switched to: {}", default.desc);

        if config.settings.notify_set {
            let icon = get_notification_sink_icon(default, config.settings.status_bar_icons);
            if let Err(e) = send_notification("Audio Output", &default.desc, Some(&icon)) {
                warn!("Notification failed: {}", e);
            }
        }
    } else {
        info!("Switching to: {}", target.desc);
        PipeWire::activate_sink(&target.name)?;
        println!("Switched to: {}", target.desc);

        if config.settings.notify_set {
            let icon = get_notification_sink_icon(target, config.settings.status_bar_icons);
            if let Err(e) = send_notification("Audio Output", &target.desc, Some(&icon)) {
                warn!("Notification failed: {}", e);
            }
        }
    }

    Ok(())
}

/// Cycle through configured sinks
///
/// # Errors
/// Returns an error if sink query or activation fails.
pub fn cycle_sink(config: &Config, direction: Direction) -> Result<()> {
    // Need at least 2 sinks to cycle
    if config.sinks.len() < 2 {
        println!("Only one sink configured, nothing to cycle");
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
        println!("Already on: {}", target.desc);
        return Ok(());
    }

    info!("Cycling to: {}", target.desc);
    PipeWire::activate_sink(&target.name)?;
    println!("Switched to: {}", target.desc);

    if config.settings.notify_set {
        let icon = get_notification_sink_icon(target, config.settings.status_bar_icons);
        if let Err(e) = send_notification("Audio Output", &target.desc, Some(&icon)) {
            warn!("Notification failed: {}", e);
        }
    }

    Ok(())
}

/// Format uptime in human-readable form
fn format_uptime(secs: u64) -> String {
    const SECS_PER_MINUTE: u64 = 60;
    const SECS_PER_HOUR: u64 = 3600;

    if secs < SECS_PER_MINUTE {
        return format!("{secs}s");
    }
    if secs < SECS_PER_HOUR {
        return format!("{}m", secs / SECS_PER_MINUTE);
    }
    let hours = secs / SECS_PER_HOUR;
    let mins = (secs % SECS_PER_HOUR) / SECS_PER_MINUTE;
    if mins > 0 {
        format!("{hours}h {mins}m")
    } else {
        format!("{hours}h")
    }
}

// ============================================================================
// IPC-based Commands (require daemon)
// ============================================================================

/// Query system and daemon status (hybrid local+IPC command)
///
/// # Errors
/// Returns an error if `PipeWire` query fails or IPC communication fails.
pub async fn status(config: &Config, json_output: bool) -> Result<()> {
    // Always query PipeWire for current sink (works with or without daemon)
    let current_sink_name = PipeWire::get_default_sink_name()?;
    let current_sink_desc = config
        .sinks
        .iter()
        .find(|s| s.name == current_sink_name)
        .map_or(current_sink_name.as_str(), |s| s.desc.as_str());

    // Try to query daemon status (non-fatal if fails)
    let daemon_running = ipc::is_daemon_running().await;
    let daemon_info = if daemon_running {
        match ipc::send_request(Request::Status).await {
            Ok(Response::Status {
                version,
                uptime_secs,
                current_sink,
                active_window,
                tracked_windows,
            }) => Some((
                version,
                uptime_secs,
                current_sink,
                active_window,
                tracked_windows,
            )),
            _ => None,
        }
    } else {
        None
    };

    // Output
    if json_output {
        let daemon_json =
            if let Some((version, uptime_secs, daemon_sink, active_window, tracked_windows)) =
                daemon_info
            {
                serde_json::json!({
                    "running": true,
                    "version": version,
                    "uptime_secs": uptime_secs,
                    "uptime_human": format_uptime(uptime_secs),
                    "daemon_sink": daemon_sink,
                    "active_window": active_window,
                    "tracked_windows": tracked_windows,
                })
            } else {
                serde_json::json!({
                    "running": false,
                })
            };

        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "current_sink": {
                    "name": current_sink_name,
                    "description": current_sink_desc,
                },
                "daemon": daemon_json,
            }))?
        );
    } else {
        // Human-readable output
        let header = "Audio Output";
        println!("{header}");
        println!("{}", "-".repeat(header.len()));
        println!("Current: {current_sink_desc}");
        println!();
        let header = "Daemon";
        println!("{header}");
        println!("{}", "-".repeat(header.len()));

        if let Some((version, uptime_secs, _daemon_sink, active_window, tracked_windows)) =
            daemon_info
        {
            println!("Status: Running (uptime: {})", format_uptime(uptime_secs));
            println!("Version: {version}");
            if let Some(rule) = active_window {
                println!("Active Rule: {rule}");
            }
            println!("Tracked Windows: {tracked_windows}");
        } else {
            println!("Status: Not running");
            println!("  Start with: pwsw daemon");
        }
    }

    Ok(())
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
            println!("{message}");
            Ok(())
        }
        Response::Error { message } => {
            anyhow::bail!("Error: {message}");
        }
        _ => {
            anyhow::bail!("Unexpected response from daemon");
        }
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
                println!("{}", serde_json::to_string_pretty(&windows)?);
            } else if windows.is_empty() {
                println!("No windows currently open.");
            } else {
                let tracked: Vec<_> = windows.iter().filter(|w| w.tracked.is_some()).collect();
                let untracked: Vec<_> = windows.iter().filter(|w| w.tracked.is_none()).collect();

                let header = format!(
                    "All Windows ({} open, {} tracked):",
                    windows.len(),
                    tracked.len()
                );
                println!("{header}");
                println!("{}", "-".repeat(header.len()));

                if !tracked.is_empty() {
                    println!("\nTracked ({}):", tracked.len());
                    for window in &tracked {
                        if let Some(ref track_info) = window.tracked {
                            println!("  • {} → {}", window.app_id, track_info.sink_desc);
                            println!("    {}", window.title);
                        }
                    }
                }

                if !untracked.is_empty() {
                    println!("\nUntracked ({}):", untracked.len());
                    for window in &untracked {
                        println!("  • {}", window.app_id);
                        println!("    {}", window.title);
                    }
                }
            }
            Ok(())
        }
        Response::Error { message } => {
            anyhow::bail!("Error: {message}");
        }
        _ => {
            anyhow::bail!("Unexpected response from daemon");
        }
    }
}

/// Test a regex pattern against current windows
///
/// # Errors
/// Returns an error if the regex is invalid, no daemon is running, or IPC fails.
pub async fn test_rule(pattern: &str, json_output: bool) -> Result<()> {
    if !ipc::is_daemon_running().await {
        anyhow::bail!("Daemon is not running. Start it with: pwsw daemon");
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
                println!("Testing pattern: {pattern}");
                println!("================");
                if matches.is_empty() {
                    println!("No matches found.");
                } else {
                    println!("Matches ({}):", matches.len());
                    for (i, window) in matches.iter().enumerate() {
                        let matched_on = window.matched_on.as_deref().unwrap_or("unknown");
                        println!(
                            "{}. app_id: {}{}",
                            i + 1,
                            window.app_id,
                            if matched_on == "app_id" || matched_on == "both" {
                                " ✓"
                            } else {
                                ""
                            }
                        );
                        println!(
                            "   title: {}{}",
                            window.title,
                            if matched_on == "title" || matched_on == "both" {
                                " ✓"
                            } else {
                                ""
                            }
                        );
                    }
                }
            }
            Ok(())
        }
        Response::Error { message } => {
            anyhow::bail!("Error: {message}");
        }
        _ => {
            anyhow::bail!("Unexpected response from daemon");
        }
    }
}
