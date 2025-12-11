//! CLI commands
//!
//! Implements both local commands (list-sinks, validate) and IPC-based commands
//! that communicate with the daemon (status, reload, list-windows, test-rule).

use anyhow::Result;
use std::collections::HashSet;

use crate::config::Config;
use crate::ipc::{self, Request, Response};
use crate::pipewire::{
    ActiveSinkJson, ConfiguredSinkJson, ListSinksJson, PipeWire, ProfileSinkJson,
};

// ============================================================================
// Local Commands (no daemon needed)
// ============================================================================

/// List all available sinks (active and profile-switch)
pub fn list_sinks(config: Option<&Config>, json_output: bool) -> Result<()> {
    let objects = PipeWire::dump()?;
    let active = PipeWire::get_active_sinks(&objects);
    let profile = PipeWire::get_profile_sinks(&objects, &active);

    let current_default = active.iter()
        .find(|s| s.is_default)
        .map(|s| s.name.clone());

    if json_output {
        let configured_names: HashSet<&str> = config
            .map(|c| c.sinks.iter().map(|s| s.name.as_str()).collect())
            .unwrap_or_default();

        let output = ListSinksJson {
            active_sinks: active.iter().map(|s| ActiveSinkJson {
                name: s.name.clone(),
                description: s.description.clone(),
                is_default: s.is_default,
                configured: configured_names.contains(s.name.as_str()),
            }).collect(),
            profile_sinks: profile.iter().map(|s| ProfileSinkJson {
                predicted_name: s.predicted_name.clone(),
                description: s.description.clone(),
                device_name: s.device_name.clone(),
                profile_name: s.profile_name.clone(),
                profile_index: s.profile_index,
            }).collect(),
            configured_sinks: config.map(|c| {
                c.sinks.iter().enumerate().map(|(i, s)| {
                    let status = if active.iter().any(|a| a.name == s.name) {
                        "active"
                    } else if profile.iter().any(|p| p.predicted_name == s.name) {
                        "requires_profile_switch"
                    } else {
                        "not_found"
                    };
                    ConfiguredSinkJson {
                        index: i + 1,
                        name: s.name.clone(),
                        desc: s.desc.clone(),
                        icon: s.icon.clone(),
                        is_default_config: s.default,
                        status: status.to_string(),
                    }
                }).collect()
            }).unwrap_or_default(),
            current_default,
        };

        println!("{}", serde_json::to_string_pretty(&output)?);
    } else {
        // Human-readable output
        println!("ACTIVE SINKS:");
        println!("{}", "-".repeat(13));
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
            println!("\nAVAILABLE VIA PROFILE SWITCH:");
            println!("{}", "-".repeat(30));
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
            println!("\nCONFIGURED SINKS:");
            println!("{}", "-".repeat(17));
            for (i, sink) in cfg.sinks.iter().enumerate() {
                let default_marker = if sink.default { " [DEFAULT]" } else { "" };
                let status = if active.iter().any(|a| a.name == sink.name) {
                    "active"
                } else if profile.iter().any(|p| p.predicted_name == sink.name) {
                    "profile switch"
                } else {
                    "not found"
                };
                println!("  {}. \"{}\"{} - {}", i + 1, sink.desc, default_marker, status);
                println!("     {}", sink.name);
            }
        }

        println!("\nConfig: {:?}", Config::get_config_path()?);
    }

    Ok(())
}

// ============================================================================
// IPC-based Commands (require daemon)
// ============================================================================

/// Query daemon status
pub async fn status(json_output: bool) -> Result<()> {
    // Check if daemon is running first
    if !ipc::is_daemon_running().await {
        if json_output {
            println!("{}", serde_json::to_string_pretty(&serde_json::json!({
                "running": false,
                "error": "Daemon is not running"
            }))?);
        } else {
            eprintln!("Daemon is not running.\n");
            eprintln!("To start the daemon:");
            eprintln!("  pwsw daemon              (run in background)");
            eprintln!("  pwsw daemon --foreground (run in foreground with logs)");
        }
        anyhow::bail!("Daemon is not running");
    }

    let response = ipc::send_request(Request::Status).await?;

    match response {
        Response::Status {
            version,
            uptime_secs,
            current_sink,
            active_window,
        } => {
            if json_output {
                println!("{}", serde_json::to_string_pretty(&serde_json::json!({
                    "version": version,
                    "uptime_secs": uptime_secs,
                    "current_sink": current_sink,
                    "active_window": active_window,
                }))?);
            } else {
                println!("PWSW Daemon Status");
                println!("==================");
                println!("Version: {}", version);
                println!("Uptime: {} seconds", uptime_secs);
                println!("Current Sink: {}", current_sink);
                if let Some(window) = active_window {
                    println!("Active Window: {}", window);
                } else {
                    println!("Active Window: (none)");
                }
            }
            Ok(())
        }
        Response::Error { message } => {
            anyhow::bail!("Daemon error: {}", message);
        }
        _ => {
            anyhow::bail!("Unexpected response from daemon");
        }
    }
}

/// Gracefully shutdown the daemon
pub async fn shutdown() -> Result<()> {
    let response = ipc::send_request(Request::Shutdown).await?;
    
    match response {
        Response::Ok { message } => {
            println!("{}", message);
            Ok(())
        }
        Response::Error { message } => {
            anyhow::bail!("Shutdown failed: {}", message);
        }
        _ => {
            anyhow::bail!("Unexpected response from daemon");
        }
    }
}

/// Get list of windows currently tracked by daemon
pub async fn list_windows(json_output: bool) -> Result<()> {
    let response = ipc::send_request(Request::ListWindows).await?;

    match response {
        Response::Windows { windows } => {
            if json_output {
                println!("{}", serde_json::to_string_pretty(&windows)?);
            } else {
                if windows.is_empty() {
                    println!("No windows currently open.");
                } else {
                    let tracked: Vec<_> = windows.iter().filter(|w| w.tracked.is_some()).collect();
                    let untracked: Vec<_> = windows.iter().filter(|w| w.tracked.is_none()).collect();

                    println!("All Windows ({} open, {} tracked):", windows.len(), tracked.len());
                    println!("{}", "=".repeat(40));

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
            }
            Ok(())
        }
        Response::Error { message } => {
            anyhow::bail!("Daemon error: {}", message);
        }
        _ => {
            anyhow::bail!("Unexpected response from daemon");
        }
    }
}

/// Test a regex pattern against current windows
pub async fn test_rule(pattern: &str, json_output: bool) -> Result<()> {
    let response = ipc::send_request(Request::TestRule {
        pattern: pattern.to_string(),
    })
    .await?;

    match response {
        Response::RuleMatches { pattern, matches } => {
            if json_output {
                println!("{}", serde_json::to_string_pretty(&serde_json::json!({
                    "pattern": pattern,
                    "matches": matches,
                }))?);
            } else {
                println!("Testing pattern: {}", pattern);
                println!("================");
                if matches.is_empty() {
                    println!("No matches found.");
                } else {
                    println!("Matches ({}):", matches.len());
                    for (i, window) in matches.iter().enumerate() {
                        let matched_on = window.matched_on.as_deref().unwrap_or("unknown");
                        println!("{}. app_id: {}{}", i + 1, window.app_id,
                            if matched_on == "app_id" || matched_on == "both" { " ✓" } else { "" });
                        println!("   title: {}{}", window.title,
                            if matched_on == "title" || matched_on == "both" { " ✓" } else { "" });
                    }
                }
            }
            Ok(())
        }
        Response::Error { message } => {
            anyhow::bail!("Error: {}", message);
        }
        _ => {
            anyhow::bail!("Unexpected response from daemon");
        }
    }
}
