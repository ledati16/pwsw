//! One-shot CLI commands
//!
//! Implements: --list-sinks, --get-sink, --set-sink, --next-sink, --prev-sink

use anyhow::Result;
use std::collections::HashSet;
use tracing::{info, warn};

use crate::config::Config;
use crate::notification::{get_notification_sink_icon, get_sink_icon, send_notification};
use crate::pipewire::{
    ActiveSinkJson, ConfiguredSinkJson, ListSinksJson, PipeWire, ProfileSinkJson, SinkInfoJson,
};

/// Direction for sink cycling
pub enum Direction {
    Next,
    Prev,
}

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

/// Get the current default sink
pub fn get_current_sink(config: &Config, json_output: bool) -> Result<()> {
    let current = PipeWire::get_default_sink_name()?;

    let sink = config.sinks.iter()
        .find(|s| s.name == current)
        .ok_or_else(|| anyhow::anyhow!(
            "Current sink '{}' not in config. Run 'nasw --list-sinks' to see available sinks.",
            current
        ))?;

    if json_output {
        println!("{}", serde_json::to_string(&SinkInfoJson {
            text: Some(sink.desc.clone()),
            icon: get_sink_icon(sink),
        })?);
    } else {
        println!("{}", sink.desc);
    }

    Ok(())
}

/// Set sink with smart toggle support
pub fn set_sink_smart(config: &Config, sink_ref: &str) -> Result<()> {
    let target = config.resolve_sink(sink_ref).ok_or_else(|| {
        let available: Vec<_> = config.sinks.iter()
            .enumerate()
            .map(|(i, s)| format!("{}. '{}'", i + 1, s.desc))
            .collect();
        anyhow::anyhow!("Unknown sink '{}'. Available: {}", sink_ref, available.join(", "))
    })?;

    let current = PipeWire::get_default_sink_name()?;
    let default = config.get_default_sink();

    if config.settings.smart_toggle && current == target.name {
        if target.name == default.name {
            println!("Already on: {}", default.desc);
            return Ok(());
        }
        info!("Toggle -> default: {}", default.desc);
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
pub fn cycle_sink(config: &Config, direction: Direction) -> Result<()> {
    // Need at least 2 sinks to cycle
    if config.sinks.len() < 2 {
        println!("Only one sink configured, nothing to cycle");
        return Ok(());
    }

    let current = PipeWire::get_default_sink_name()?;

    // Find current sink's index in config, or start from default
    let current_index = config.sinks.iter()
        .position(|s| s.name == current)
        .unwrap_or_else(|| {
            // Current sink not in config, find default's index
            config.sinks.iter()
                .position(|s| s.default)
                .unwrap_or(0)
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
