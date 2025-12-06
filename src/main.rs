//! NASW - Niri Audio Switcher
//!
//! Automatically switches audio sinks based on active windows in Niri compositor.
//! Uses PipeWire native tools (pw-dump, pw-metadata, pw-cli) for audio control.
//!
//! # Features
//! - Automatic sink switching based on window rules
//! - Profile switching for analog/digital outputs on the same card
//! - Status bar integration with JSON output
//! - Smart toggle between configured sinks

use anyhow::{Context, Result};
use clap::Parser;
use notify_rust::Notification;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::signal;
use tracing::{debug, error, info, trace, warn};

// ============================================================================
// Constants
// ============================================================================

/// Time to wait for a new sink node to appear after profile switch
const PROFILE_SWITCH_DELAY_MS: u64 = 150;

/// Maximum retries when waiting for sink after profile switch
const PROFILE_SWITCH_MAX_RETRIES: u32 = 5;

// ============================================================================
// CLI Arguments
// ============================================================================

#[derive(Parser)]
#[command(name = "nasw")]
#[command(version)]
#[command(about = "Niri Audio Switcher - Automatically switch audio based on active windows")]
#[command(after_help = "\
BEHAVIOR:
  - On startup, queries the current system default sink
  - When a matching window opens, switches to that rule's sink
  - When multiple windows match rules, the most recently opened takes priority
  - When all matching windows close, returns to the default sink
  - Supports profile switching for analog/digital outputs on the same card

ONE-SHOT COMMANDS:
  --check-config     Validate configuration and view settings
  --set-sink SINK    Switch audio output (toggles back to default if already active)
  --next-sink        Cycle to next configured sink
  --prev-sink        Cycle to previous configured sink
  --get-sink         Display current output (add --json for icon)
  --list-sinks       Discover available audio outputs including inactive profiles

SINK REFERENCES:
  Sinks can be referenced by description, node name, or position (1, 2, 3...).

PIPEWIRE INTEGRATION:
  Uses pw-dump for JSON queries, pw-metadata for setting defaults.
  Supports profile switching via pw-cli for analog/digital outputs.
  Node names are stable across reboots (unlike numeric IDs).

  Sinks marked with ~ require profile switching to activate.")]
struct Args {
    /// Validate configuration file and exit
    #[arg(long, group = "command")]
    check_config: bool,

    /// List available audio sinks (including those requiring profile switch)
    #[arg(long, group = "command")]
    list_sinks: bool,

    /// Set the default sink (by desc, node name, or position)
    #[arg(long, value_name = "SINK", group = "command")]
    set_sink: Option<String>,

    /// Get current default sink (plain text, or JSON with --json for icon)
    #[arg(long, group = "command")]
    get_sink: bool,

    /// Switch to next configured sink (wraps around)
    #[arg(long, group = "command")]
    next_sink: bool,

    /// Switch to previous configured sink (wraps around)
    #[arg(long, group = "command")]
    prev_sink: bool,

    /// Output in JSON format (for --get-sink and --list-sinks)
    #[arg(long)]
    json: bool,
}

// ============================================================================
// Configuration Structures
// ============================================================================

#[derive(Debug, Clone)]
struct Config {
    settings: Settings,
    sinks: Vec<SinkConfig>,
    rules: Vec<Rule>,
}

#[derive(Debug, Clone)]
struct Settings {
    reset_on_startup: bool,
    smart_toggle: bool,
    notify_daemon: bool,
    notify_switch: bool,
    notify_set: bool,
    /// If true, custom sink icons only apply to --get-sink --json.
    /// Notifications will use auto-detected FreeDesktop icons for compatibility.
    status_bar_icons: bool,
    log_level: String,
}

#[derive(Debug, Clone)]
struct SinkConfig {
    /// PipeWire node name (e.g., "alsa_output.pci-0000_0c_00.4.iec958-stereo")
    name: String,
    /// Human-readable description
    desc: String,
    /// Optional icon for status bars (if not set, auto-detected)
    icon: Option<String>,
    /// Whether this is the default fallback sink
    default: bool,
}

#[derive(Debug, Clone)]
struct Rule {
    app_id_regex: Regex,
    title_regex: Option<Regex>,
    sink_ref: String,
    desc: Option<String>,
    notify: bool,
    // Original patterns for display
    app_id_pattern: String,
    title_pattern: Option<String>,
}

// Config file deserialization structures
#[derive(Debug, Deserialize)]
struct ConfigFile {
    #[serde(default)]
    settings: SettingsFile,
    #[serde(default)]
    sinks: Vec<SinkConfigFile>,
    #[serde(default)]
    rules: Vec<RuleConfigFile>,
}

#[derive(Debug, Deserialize)]
struct SettingsFile {
    #[serde(default = "default_true")]
    reset_on_startup: bool,
    #[serde(default = "default_true")]
    smart_toggle: bool,
    #[serde(default = "default_true")]
    notify_daemon: bool,
    #[serde(default = "default_true")]
    notify_switch: bool,
    #[serde(default = "default_true")]
    notify_set: bool,
    #[serde(default)]
    status_bar_icons: bool,
    #[serde(default = "default_log_level")]
    log_level: String,
}

#[derive(Debug, Deserialize)]
struct SinkConfigFile {
    name: String,
    desc: String,
    #[serde(default)]
    icon: Option<String>,
    #[serde(default)]
    default: bool,
}

#[derive(Debug, Deserialize)]
struct RuleConfigFile {
    app_id: String,
    #[serde(default)]
    title: Option<String>,
    sink: String,
    #[serde(default)]
    desc: Option<String>,
    #[serde(default)]
    notify: bool,
}

fn default_true() -> bool {
    true
}

fn default_log_level() -> String {
    "info".to_string()
}

impl Default for SettingsFile {
    fn default() -> Self {
        Self {
            reset_on_startup: true,
            smart_toggle: true,
            notify_daemon: true,
            notify_switch: true,
            notify_set: true,
            status_bar_icons: false,
            log_level: "info".to_string(),
        }
    }
}

// ============================================================================
// PipeWire JSON Structures (from pw-dump)
// ============================================================================

/// Top-level PipeWire object from pw-dump output
#[derive(Debug, Deserialize)]
struct PwObject {
    id: u32,
    #[serde(rename = "type")]
    obj_type: String,
    #[serde(default)]
    info: Option<PwInfo>,
    #[serde(default)]
    props: Option<PwProps>,
    #[serde(default)]
    metadata: Option<Vec<PwMetadataEntry>>,
}

impl PwObject {
    /// Get props from either info.props or top-level props (metadata objects use top-level)
    fn get_props(&self) -> Option<&PwProps> {
        self.info
            .as_ref()
            .and_then(|i| i.props.as_ref())
            .or(self.props.as_ref())
    }
}

#[derive(Debug, Deserialize)]
struct PwInfo {
    #[serde(default)]
    props: Option<PwProps>,
    #[serde(default)]
    params: Option<PwParams>,
}

/// PipeWire object properties - uses permissive deserialization
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct PwProps {
    #[serde(rename = "node.name")]
    node_name: Option<String>,
    #[serde(rename = "node.description")]
    node_description: Option<String>,
    #[serde(rename = "node.nick")]
    node_nick: Option<String>,
    #[serde(rename = "media.class")]
    media_class: Option<String>,
    #[serde(rename = "metadata.name")]
    metadata_name: Option<String>,
    #[serde(rename = "device.name")]
    device_name: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct PwParams {
    #[serde(rename = "EnumProfile")]
    enum_profile: Option<Vec<PwProfile>>,
    #[serde(rename = "Profile")]
    profile: Option<Vec<PwProfile>>,
}

#[derive(Debug, Deserialize, Clone)]
struct PwProfile {
    index: Option<u32>,
    name: Option<String>,
    description: Option<String>,
    available: Option<String>,
}

#[derive(Debug, Deserialize)]
struct PwMetadataEntry {
    key: String,
    #[serde(default)]
    value: Option<serde_json::Value>,
}

impl PwMetadataEntry {
    /// Extract sink name from metadata value (handles multiple formats)
    fn get_name(&self) -> Option<String> {
        let value = self.value.as_ref()?;
        // Try object with "name" field first
        if let Some(obj) = value.as_object() {
            if let Some(name_val) = obj.get("name") {
                return name_val.as_str().map(String::from);
            }
        }
        // Fall back to plain string
        value.as_str().map(String::from)
    }
}

// ============================================================================
// Sink Discovery Types
// ============================================================================

/// A sink currently available in PipeWire
#[derive(Debug, Clone)]
struct ActiveSink {
    name: String,
    description: String,
    is_default: bool,
}

/// A sink that requires profile switching to become available
#[derive(Debug)]
struct ProfileSink {
    /// Predicted node name (based on device name + profile)
    predicted_name: String,
    /// Description from profile
    description: String,
    /// Device ID that owns this profile
    device_id: u32,
    /// Device name
    device_name: String,
    /// Profile index to switch to
    profile_index: u32,
    /// Profile name
    profile_name: String,
}

// ============================================================================
// JSON Output Structures
// ============================================================================

#[derive(Debug, Serialize)]
struct SinkInfoJson {
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<String>,
    icon: String,
}

#[derive(Debug, Serialize)]
struct ListSinksJson {
    active_sinks: Vec<ActiveSinkJson>,
    profile_sinks: Vec<ProfileSinkJson>,
    configured_sinks: Vec<ConfiguredSinkJson>,
    current_default: Option<String>,
}

#[derive(Debug, Serialize)]
struct ActiveSinkJson {
    name: String,
    description: String,
    is_default: bool,
    configured: bool,
}

#[derive(Debug, Serialize)]
struct ProfileSinkJson {
    predicted_name: String,
    description: String,
    device_name: String,
    profile_name: String,
    profile_index: u32,
}

#[derive(Debug, Serialize)]
struct ConfiguredSinkJson {
    index: usize,
    name: String,
    desc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    icon: Option<String>,
    is_default_config: bool,
    status: String,
}

// ============================================================================
// Config Implementation
// ============================================================================

impl Config {
    fn load() -> Result<Self> {
        let config_path = Self::get_config_path()?;

        if !config_path.exists() {
            info!("Creating default config at {:?}", config_path);
            Self::create_default_config(&config_path)?;
        }

        let contents = fs::read_to_string(&config_path)
            .with_context(|| format!("Failed to read config: {:?}", config_path))?;

        let config_file: ConfigFile = toml::from_str(&contents)
            .with_context(|| format!("Failed to parse config: {:?}", config_path))?;

        Self::from_config_file(config_file)
    }

    fn from_config_file(config_file: ConfigFile) -> Result<Self> {
        if config_file.sinks.is_empty() {
            anyhow::bail!("No sinks defined. Add at least one [[sinks]] section to config.");
        }

        let settings = Settings {
            reset_on_startup: config_file.settings.reset_on_startup,
            smart_toggle: config_file.settings.smart_toggle,
            notify_daemon: config_file.settings.notify_daemon,
            notify_switch: config_file.settings.notify_switch,
            notify_set: config_file.settings.notify_set,
            status_bar_icons: config_file.settings.status_bar_icons,
            log_level: config_file.settings.log_level,
        };

        let sinks: Vec<SinkConfig> = config_file
            .sinks
            .into_iter()
            .map(|s| SinkConfig {
                name: s.name,
                desc: s.desc,
                icon: s.icon,
                default: s.default,
            })
            .collect();

        let mut rules = Vec::with_capacity(config_file.rules.len());
        for (i, rule_config) in config_file.rules.iter().enumerate() {
            let app_id_regex = Regex::new(&rule_config.app_id)
                .with_context(|| format!("Invalid regex in rule {} app_id: '{}'", i + 1, rule_config.app_id))?;

            let title_regex = match &rule_config.title {
                Some(pattern) => Some(
                    Regex::new(pattern)
                        .with_context(|| format!("Invalid regex in rule {} title: '{}'", i + 1, pattern))?,
                ),
                None => None,
            };

            rules.push(Rule {
                app_id_regex,
                title_regex,
                sink_ref: rule_config.sink.clone(),
                desc: rule_config.desc.clone(),
                notify: rule_config.notify,
                app_id_pattern: rule_config.app_id.clone(),
                title_pattern: rule_config.title.clone(),
            });
        }

        let config = Config { settings, sinks, rules };
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> Result<()> {
        // Validate log level
        match self.settings.log_level.as_str() {
            "error" | "warn" | "info" | "debug" | "trace" => {}
            level => anyhow::bail!(
                "Invalid log_level '{}'. Must be: error, warn, info, debug, or trace",
                level
            ),
        }

        // Exactly one default sink
        let default_count = self.sinks.iter().filter(|s| s.default).count();
        match default_count {
            0 => anyhow::bail!("No default sink. Mark one sink with 'default = true'"),
            1 => {}
            n => anyhow::bail!("{} default sinks found. Only one allowed.", n),
        }

        // No duplicate descriptions or names
        let mut seen_descs = HashSet::with_capacity(self.sinks.len());
        let mut seen_names = HashSet::with_capacity(self.sinks.len());
        for sink in &self.sinks {
            if !seen_descs.insert(&sink.desc) {
                anyhow::bail!("Duplicate sink description: '{}'", sink.desc);
            }
            if !seen_names.insert(&sink.name) {
                anyhow::bail!("Duplicate sink name: '{}'", sink.name);
            }
            // Validate name doesn't look like a position number
            if sink.desc.parse::<usize>().is_ok() {
                warn!("Sink description '{}' looks like a number - this may cause confusion with position references", sink.desc);
            }
        }

        // All rule sinks must exist
        for (i, rule) in self.rules.iter().enumerate() {
            if self.resolve_sink(&rule.sink_ref).is_none() {
                let available: Vec<_> = self.sinks.iter()
                    .enumerate()
                    .map(|(idx, s)| format!("{}. '{}'", idx + 1, s.desc))
                    .collect();
                anyhow::bail!(
                    "Rule {} references unknown sink '{}'. Available: [{}]",
                    i + 1, rule.sink_ref, available.join(", ")
                );
            }
        }

        Ok(())
    }

    fn get_config_path() -> Result<PathBuf> {
        let config_dir = dirs::config_dir()
            .context("Could not determine config directory")?
            .join("nasw");
        fs::create_dir_all(&config_dir)
            .with_context(|| format!("Failed to create config dir: {:?}", config_dir))?;
        Ok(config_dir.join("config.toml"))
    }

    fn create_default_config(path: &PathBuf) -> Result<()> {
        let default_config = r#"# NASW (Niri Audio Switcher) Configuration
#
# Uses PipeWire native tools for audio control.
# Supports profile switching for analog/digital outputs.

[settings]
reset_on_startup = true    # Reset to default sink on daemon start
smart_toggle = true        # --set-sink toggles back to default if already active
notify_daemon = true       # Notifications for daemon start/stop
notify_switch = true       # Notifications for rule-triggered switches (per-rule notify must also be true)
notify_set = true          # Notifications for --set-sink, --next-sink, and --prev-sink commands
status_bar_icons = false   # If true, custom icons only apply to --get-sink --json
log_level = "info"         # error, warn, info, debug, trace

# Audio sinks
# Find available sinks with: nasw --list-sinks
#
# Icons are auto-detected from sink description (e.g., "HDMI" → video-display).
# Set 'icon' to override with any icon name your system supports.

[[sinks]]
name = "alsa_output.pci-0000_0c_00.4.iec958-stereo"
desc = "Optical Out"
icon = "audio-speakers"
default = true

[[sinks]]
name = "alsa_output.pci-0000_0c_00.4.analog-stereo"
desc = "Headphones"
icon = "audio-headphones"

# Window rules
# Find app_id and title with: niri msg windows
#
# Regex patterns (for app_id and title fields):
#   "firefox"     - matches anywhere in string
#   "^steam$"     - exact match only
#   "^(mpv|vlc)$" - matches mpv OR vlc
#   "(?i)discord" - case insensitive

[[rules]]
app_id = "^steam$"
title = "^Steam Big Picture Mode$"
sink = "Optical Out"       # Reference by: desc, name, or position (1, 2)
desc = "Steam Big Picture" # Custom name for notifications
notify = true

# [[rules]]
# app_id = "^mpv$"
# sink = 2
# notify = true
"#;
        fs::write(path, default_config)
            .with_context(|| format!("Failed to write config: {:?}", path))?;
        Ok(())
    }

    fn print_summary(&self) {
        println!("✓ Configuration valid\n");

        println!("Settings:");
        println!("  reset_on_startup: {}", self.settings.reset_on_startup);
        println!("  smart_toggle: {}", self.settings.smart_toggle);
        println!("  notify_daemon: {}", self.settings.notify_daemon);
        println!("  notify_switch: {}", self.settings.notify_switch);
        println!("  notify_set: {}", self.settings.notify_set);
        println!("  status_bar_icons: {}", self.settings.status_bar_icons);
        println!("  log_level: {}", self.settings.log_level);

        println!("\nSinks ({}):", self.sinks.len());
        for (i, sink) in self.sinks.iter().enumerate() {
            let marker = if sink.default { " [DEFAULT]" } else { "" };
            println!("  {}. {}{}", i + 1, sink.desc, marker);
            println!("     name: {}", sink.name);
            if let Some(ref icon) = sink.icon {
                println!("     icon: {}", icon);
            }
        }

        if self.rules.is_empty() {
            println!("\nNo rules configured.");
        } else {
            println!("\nRules ({}):", self.rules.len());
            for (i, rule) in self.rules.iter().enumerate() {
                println!("  {}. app_id: {}", i + 1, rule.app_id_pattern);
                if let Some(ref title) = rule.title_pattern {
                    println!("     title: {}", title);
                }
                println!("     sink: {} (notify: {})", rule.sink_ref, rule.notify);
            }
        }

        if let Ok(path) = Self::get_config_path() {
            println!("\nConfig: {:?}", path);
        }
    }

    /// Resolve a sink reference (by position, desc, or name)
    fn resolve_sink(&self, sink_ref: &str) -> Option<&SinkConfig> {
        // Try position first (1-indexed)
        if let Ok(pos) = sink_ref.parse::<usize>() {
            return if pos > 0 && pos <= self.sinks.len() {
                Some(&self.sinks[pos - 1])
            } else {
                None
            };
        }
        // Then try desc or name
        self.sinks.iter().find(|s| s.desc == sink_ref || s.name == sink_ref)
    }

    fn get_default_sink(&self) -> &SinkConfig {
        self.sinks.iter()
            .find(|s| s.default)
            .expect("Default sink validated at load")
    }

    fn should_notify_switch(&self, rule_notify: bool) -> bool {
        self.settings.notify_switch && rule_notify
    }
}

// ============================================================================
// PipeWire Interface
// ============================================================================

struct PipeWire;

impl PipeWire {
    /// Get all PipeWire objects via pw-dump
    fn dump() -> Result<Vec<PwObject>> {
        let output = Command::new("pw-dump")
            .output()
            .context("Failed to run pw-dump. Is PipeWire running?")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("pw-dump failed: {}", stderr.trim());
        }

        let objects: Vec<PwObject> = serde_json::from_slice(&output.stdout)
            .context("Failed to parse pw-dump JSON")?;

        trace!("pw-dump returned {} objects", objects.len());
        Ok(objects)
    }

    /// Get currently active audio sinks from PipeWire objects
    fn get_active_sinks(objects: &[PwObject]) -> Vec<ActiveSink> {
        let default_name = Self::get_default_sink_name_from_objects(objects);

        objects.iter()
            .filter(|obj| obj.obj_type == "PipeWire:Interface:Node")
            .filter_map(|obj| {
                let props = obj.get_props()?;

                // Only Audio/Sink nodes
                if props.media_class.as_deref() != Some("Audio/Sink") {
                    return None;
                }

                let name = props.node_name.clone()?;
                let description = props.node_description.clone()
                    .or_else(|| props.node_nick.clone())
                    .unwrap_or_else(|| name.clone());

                Some(ActiveSink {
                    name: name.clone(),
                    description,
                    is_default: default_name.as_ref() == Some(&name),
                })
            })
            .collect()
    }

    /// Get sinks available through profile switching
    fn get_profile_sinks(objects: &[PwObject], active_sinks: &[ActiveSink]) -> Vec<ProfileSink> {
        let active_names: HashSet<&str> = active_sinks.iter()
            .map(|s| s.name.as_str())
            .collect();

        let mut profile_sinks = Vec::new();

        for obj in objects {
            if obj.obj_type != "PipeWire:Interface:Device" {
                continue;
            }

            let Some(props) = obj.get_props() else { continue };
            let Some(info) = &obj.info else { continue };
            let Some(params) = &info.params else { continue };
            let Some(enum_profiles) = &params.enum_profile else { continue };

            // Only ALSA audio devices
            let device_name = match &props.device_name {
                Some(name) if name.starts_with("alsa_card.") => name,
                _ => continue,
            };

            // Get current profile to skip it
            let current_profile_index = params.profile.as_ref()
                .and_then(|p| p.first())
                .and_then(|p| p.index);

            for profile in enum_profiles {
                let Some(index) = profile.index else { continue };
                let Some(ref profile_name) = profile.name else { continue };

                // Skip "off" profile and currently active profile
                if profile_name == "off" || Some(index) == current_profile_index {
                    continue;
                }

                // Skip unavailable profiles
                if profile.available.as_deref() == Some("no") {
                    continue;
                }

                // Only output profiles (stereo, surround, etc.)
                let is_output = profile_name.contains("output:")
                    || profile_name.ends_with("-stereo")
                    || profile_name.ends_with("-surround-40")
                    || profile_name.ends_with("-surround-51")
                    || profile_name.ends_with("-surround-71");

                if !is_output {
                    continue;
                }

                // Predict node name: alsa_output.{device_suffix}.{profile_suffix}
                let device_suffix = device_name.strip_prefix("alsa_card.").unwrap_or(device_name);
                let profile_suffix = profile_name
                    .strip_prefix("output:")
                    .unwrap_or(profile_name)
                    .replace("+input:", "-");

                let predicted_name = format!("alsa_output.{}.{}", device_suffix, profile_suffix);

                // Skip if already active
                if active_names.contains(predicted_name.as_str()) {
                    continue;
                }

                let description = profile.description.clone()
                    .unwrap_or_else(|| profile_name.clone());

                profile_sinks.push(ProfileSink {
                    predicted_name,
                    description,
                    device_id: obj.id,
                    device_name: device_name.clone(),
                    profile_index: index,
                    profile_name: profile_name.clone(),
                });
            }
        }

        profile_sinks
    }

    /// Extract default sink name from metadata objects
    fn get_default_sink_name_from_objects(objects: &[PwObject]) -> Option<String> {
        for obj in objects {
            if obj.obj_type != "PipeWire:Interface:Metadata" {
                continue;
            }

            let Some(props) = obj.get_props() else { continue };
            if props.metadata_name.as_deref() != Some("default") {
                continue;
            }

            if let Some(metadata) = &obj.metadata {
                for entry in metadata {
                    if entry.key == "default.audio.sink" {
                        return entry.get_name();
                    }
                }
            }
        }
        None
    }

    /// Get current default sink name (fresh query)
    fn get_default_sink_name() -> Result<String> {
        let objects = Self::dump()?;
        Self::get_default_sink_name_from_objects(&objects)
            .ok_or_else(|| anyhow::anyhow!("No default sink found in PipeWire metadata"))
    }

    /// Set the default audio sink via pw-metadata
    fn set_default_sink(node_name: &str) -> Result<()> {
        let value = format!(r#"{{ "name": "{}" }}"#, node_name);

        let output = Command::new("pw-metadata")
            .args(["0", "default.audio.sink", &value, "Spa:String:JSON"])
            .output()  // Capture stdout/stderr instead of inheriting
            .context("Failed to run pw-metadata")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("pw-metadata failed: {}", stderr.trim());
        }

        debug!("Set default sink: {}", node_name);
        Ok(())
    }

    /// Switch device profile via pw-cli
    fn set_device_profile(device_id: u32, profile_index: u32) -> Result<()> {
        let profile_json = format!("{{ index: {} }}", profile_index);

        let output = Command::new("pw-cli")
            .args(["s", &device_id.to_string(), "Profile", &profile_json])
            .output()  // Capture stdout/stderr instead of inheriting
            .context("Failed to run pw-cli")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("pw-cli failed: {}", stderr.trim());
        }

        debug!("Set device {} to profile {}", device_id, profile_index);
        Ok(())
    }

    /// Find profile sink info if sink requires profile switching
    fn find_profile_sink(objects: &[PwObject], sink_name: &str) -> Option<ProfileSink> {
        let active = Self::get_active_sinks(objects);
        let profile_sinks = Self::get_profile_sinks(objects, &active);
        profile_sinks.into_iter().find(|s| s.predicted_name == sink_name)
    }

    /// Activate a sink, switching profiles if necessary
    fn activate_sink(sink_name: &str) -> Result<()> {
        let objects = Self::dump()?;

        // Check if sink is already active
        let active = Self::get_active_sinks(&objects);
        if active.iter().any(|s| s.name == sink_name) {
            return Self::set_default_sink(sink_name);
        }

        // Need profile switching?
        let profile_sink = Self::find_profile_sink(&objects, sink_name)
            .ok_or_else(|| anyhow::anyhow!(
                "Sink '{}' not found (not active and no profile switch available)",
                sink_name
            ))?;

        info!(
            "Switching profile: {} → {} (device: {})",
            profile_sink.profile_name, sink_name, profile_sink.device_name
        );

        Self::set_device_profile(profile_sink.device_id, profile_sink.profile_index)?;

        // Wait for the new node to appear with retries
        for attempt in 1..=PROFILE_SWITCH_MAX_RETRIES {
            std::thread::sleep(Duration::from_millis(PROFILE_SWITCH_DELAY_MS));

            let objects = Self::dump()?;
            let active = Self::get_active_sinks(&objects);

            if active.iter().any(|s| s.name == sink_name) {
                Self::set_default_sink(sink_name)?;
                return Ok(());
            }

            debug!("Waiting for sink '{}' (attempt {}/{})", sink_name, attempt, PROFILE_SWITCH_MAX_RETRIES);
        }

        // Try setting anyway - PipeWire might accept it
        warn!("Sink '{}' not visible after profile switch, attempting to set anyway", sink_name);
        Self::set_default_sink(sink_name)
    }
}

// ============================================================================
// Niri IPC Structures
// ============================================================================

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
enum NiriEvent {
    WindowOpenedOrChanged { window: WindowProps },
    WindowClosed { id: u64 },
    #[serde(untagged)]
    Other(#[allow(dead_code)] serde_json::Value),
}

#[derive(Debug, Deserialize)]
struct WindowProps {
    id: u64,
    title: String,
    app_id: String,
}

// ============================================================================
// Application State
// ============================================================================

struct State {
    config: Config,
    current_sink_name: String,
    /// Tracks windows that matched rules. Entries are removed on window close.
    /// HashMap capacity may grow over time but entries don't leak.
    active_windows: HashMap<u64, ActiveWindow>,
}

#[derive(Debug)]
struct ActiveWindow {
    sink_name: String,
    /// Description of what triggered this (e.g., "Steam Big Picture")
    trigger_desc: String,
    opened_at: Instant,
}

impl State {
    fn new(config: Config) -> Result<Self> {
        let current_sink_name = PipeWire::get_default_sink_name().unwrap_or_else(|e| {
            warn!("Could not query default sink: {}. Using configured default.", e);
            config.get_default_sink().name.clone()
        });

        info!("Current default sink: {}", current_sink_name);

        Ok(Self {
            config,
            current_sink_name,
            active_windows: HashMap::new(),
        })
    }

    fn find_matching_rule(&self, app_id: &str, title: &str) -> Option<&Rule> {
        self.config.rules.iter().find(|rule| {
            rule.app_id_regex.is_match(app_id)
                && rule.title_regex.as_ref()
                    .map(|r| r.is_match(title))
                    .unwrap_or(true)
        })
    }

    fn should_switch_sink(&self, new_sink_name: &str) -> bool {
        self.current_sink_name != new_sink_name
    }

    fn update_sink(&mut self, new_sink_name: String) {
        debug!("State: {} → {}", self.current_sink_name, new_sink_name);
        self.current_sink_name = new_sink_name;
    }

    fn determine_target_sink(&self) -> String {
        self.active_windows.iter()
            .max_by_key(|(_, w)| w.opened_at)
            .map(|(_, w)| w.sink_name.clone())
            .unwrap_or_else(|| self.config.get_default_sink().name.clone())
    }
}

// ============================================================================
// Main Application
// ============================================================================

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    // Determine if this is a one-shot command (don't init logging yet for daemon)
    let is_oneshot = args.list_sinks
        || args.set_sink.is_some()
        || args.get_sink
        || args.next_sink
        || args.prev_sink
        || args.check_config;

    // Initialize logging for one-shot commands only
    // Daemon mode will init after loading config to respect log_level setting
    if is_oneshot {
        tracing_subscriber::fmt()
            .with_env_filter(
                tracing_subscriber::EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
            )
            .init();
    }

    // One-shot commands
    if args.list_sinks {
        let config = Config::load().ok();
        return list_sinks(config.as_ref(), args.json);
    }

    if args.get_sink {
        let config = Config::load()?;
        return get_current_sink(&config, args.json);
    }

    let config = Config::load()?;

    if args.check_config {
        config.print_summary();
        return Ok(());
    }

    if let Some(ref sink_ref) = args.set_sink {
        return set_sink_smart(&config, sink_ref);
    }

    if args.next_sink {
        return cycle_sink(&config, Direction::Next);
    }

    if args.prev_sink {
        return cycle_sink(&config, Direction::Prev);
    }

    // Daemon mode
    run_daemon(config).await
}

async fn run_daemon(config: Config) -> Result<()> {
    // Initialize logging with config log_level (only for daemon mode)
    // Filter format: "nasw=LEVEL" ensures only our crate logs at the configured level
    // Other crates (zbus, etc.) are suppressed unless RUST_LOG overrides
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| {
            tracing_subscriber::EnvFilter::new(format!("nasw={}", config.settings.log_level))
        });

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .init();

    info!("Starting NASW daemon");
    info!("Loaded {} sinks, {} rules", config.sinks.len(), config.rules.len());

    let mut state = State::new(config)?;

    // Reset to default on startup if configured
    if state.config.settings.reset_on_startup {
        let default = state.config.get_default_sink();
        if state.current_sink_name != default.name {
            info!("Resetting to default: {}", default.desc);
            PipeWire::activate_sink(&default.name)?;
            state.current_sink_name = default.name.clone();
        }
    }

    // Connect to Niri
    let socket_path = env::var("NIRI_SOCKET")
        .context("NIRI_SOCKET not set. Is Niri running?")?;

    info!("Connecting to Niri: {}", socket_path);
    let stream = UnixStream::connect(&socket_path).await
        .with_context(|| format!("Failed to connect to Niri socket: {}", socket_path))?;

    let (reader, mut writer) = tokio::io::split(stream);
    writer.write_all(b"\"EventStream\"\n").await?;

    let mut lines = BufReader::new(reader).lines();

    if state.config.settings.notify_daemon {
        if let Err(e) = send_notification("NASW Started", "Audio switcher running", None) {
            warn!("Could not send startup notification: {}", e);
        }
    }

    info!("Monitoring window events...");

    loop {
        tokio::select! {
            result = lines.next_line() => {
                match result {
                    Ok(Some(line)) => {
                        if let Err(e) = process_event(&line, &mut state).await {
                            error!("Event error: {:#}", e);
                        }
                    }
                    Ok(None) => {
                        warn!("Niri event stream ended");
                        break;
                    }
                    Err(e) => {
                        error!("Stream error: {:#}", e);
                        break;
                    }
                }
            }
            _ = signal::ctrl_c() => {
                info!("Shutting down");
                if state.config.settings.notify_daemon {
                    let _ = send_notification("NASW Stopped", "Audio switcher stopped", None);
                }
                break;
            }
        }
    }

    Ok(())
}

async fn process_event(line: &str, state: &mut State) -> Result<()> {
    let event: NiriEvent = match serde_json::from_str(line) {
        Ok(e) => e,
        Err(e) => {
            trace!("Skipping unknown event: {} ({})", line.chars().take(50).collect::<String>(), e);
            return Ok(());
        }
    };

    match event {
        NiriEvent::WindowOpenedOrChanged { window: win } => {
            debug!("Window: id={}, app_id='{}', title='{}'", win.id, win.app_id, win.title);

            // Extract rule data before mutating state (borrow checker)
            let matched = state.find_matching_rule(&win.app_id, &win.title).map(|rule| {
                let sink = state.config.resolve_sink(&rule.sink_ref).expect("Validated at load");
                // Use rule desc if set, otherwise use window title
                let trigger = rule.desc.clone().unwrap_or_else(|| win.title.clone());
                (
                    sink.name.clone(),
                    sink.desc.clone(),
                    trigger,
                    rule.notify,
                )
            });

            let was_tracked = state.active_windows.contains_key(&win.id);

            if let Some((sink_name, sink_desc, trigger_desc, rule_notify)) = matched {
                info!("Rule matched: '{}' → {}", win.app_id, sink_desc);

                // Only update opened_at for new windows, preserve original time for existing
                if !was_tracked {
                    state.active_windows.insert(win.id, ActiveWindow {
                        sink_name: sink_name.clone(),
                        trigger_desc: trigger_desc.clone(),
                        opened_at: Instant::now(),
                    });

                    if state.should_switch_sink(&sink_name) {
                        let notify = state.config.should_notify_switch(rule_notify);
                        // Use app_id as icon (e.g., "steam" shows Steam icon)
                        let app_icon = get_app_icon(&win.app_id);
                        switch_audio(&sink_name, &sink_desc, Some(&trigger_desc), Some(&app_icon), notify)?;
                        state.update_sink(sink_name);
                    }
                }
                // If already tracked and still matches, do nothing (keep original opened_at)
            } else if was_tracked {
                // Window was tracked but no longer matches (e.g., title changed)
                if let Some(old_window) = state.active_windows.remove(&win.id) {
                    debug!("Window no longer matches rule: {} (was: {})", win.id, old_window.trigger_desc);

                    let target = state.determine_target_sink();
                    if state.should_switch_sink(&target) {
                        let target_sink = state.config.sinks.iter().find(|s| s.name == target);
                        let desc = target_sink.map(|s| s.desc.as_str()).unwrap_or(&target);
                        let status_bar_icons = state.config.settings.status_bar_icons;
                        let icon = target_sink.map(|s| get_notification_sink_icon(s, status_bar_icons));
                        let is_default = state.config.get_default_sink().name == target;
                        let notify = state.config.settings.notify_switch && is_default;

                        let return_context = format!("{} ended", old_window.trigger_desc);
                        switch_audio(&target, desc, Some(&return_context), icon.as_deref(), notify)?;
                        state.update_sink(target);
                    }
                }
            }
        }
        NiriEvent::WindowClosed { id } => {
            if let Some(closed_window) = state.active_windows.remove(&id) {
                debug!("Tracked window closed: {} (was: {})", id, closed_window.trigger_desc);

                let target = state.determine_target_sink();
                if state.should_switch_sink(&target) {
                    let target_sink = state.config.sinks.iter().find(|s| s.name == target);
                    let desc = target_sink.map(|s| s.desc.as_str()).unwrap_or(&target);
                    let status_bar_icons = state.config.settings.status_bar_icons;
                    let icon = target_sink.map(|s| get_notification_sink_icon(s, status_bar_icons));
                    let is_default = state.config.get_default_sink().name == target;
                    let notify = state.config.settings.notify_switch && is_default;

                    // Show what we're returning from in the notification
                    let return_context = format!("{} closed", closed_window.trigger_desc);
                    switch_audio(&target, desc, Some(&return_context), icon.as_deref(), notify)?;
                    state.update_sink(target);
                }
            }
        }
        NiriEvent::Other(_) => {}
    }

    Ok(())
}

fn switch_audio(
    name: &str,
    desc: &str,
    custom_desc: Option<&str>,
    icon: Option<&str>,
    notify: bool,
) -> Result<()> {
    info!("Switching: {} ({})", desc, name);
    PipeWire::activate_sink(name)?;

    if notify {
        let message = match custom_desc {
            Some(d) => format!("{} → {}", desc, d),
            None => desc.to_string(),
        };
        if let Err(e) = send_notification("Audio Output", &message, icon) {
            warn!("Notification failed: {}", e);
        }
    }

    Ok(())
}

// ============================================================================
// One-Shot Commands
// ============================================================================

fn list_sinks(config: Option<&Config>, json_output: bool) -> Result<()> {
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
        println!("─────────────");
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
            println!("──────────────────────────────");
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
            println!("─────────────────");
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

fn set_sink_smart(config: &Config, sink_ref: &str) -> Result<()> {
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

enum Direction {
    Next,
    Prev,
}

fn cycle_sink(config: &Config, direction: Direction) -> Result<()> {
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

fn get_current_sink(config: &Config, json_output: bool) -> Result<()> {
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

/// Determine icon for a sink (custom or auto-detected using FreeDesktop standard names)
fn get_sink_icon(sink: &SinkConfig) -> String {
    // Custom icon takes priority
    if let Some(ref icon) = sink.icon {
        return icon.clone();
    }
    get_sink_icon_auto(sink)
}

/// Get auto-detected sink icon using FreeDesktop standard names
/// Used for notifications when status_bar_icons is enabled
fn get_sink_icon_auto(sink: &SinkConfig) -> String {
    let desc_lower = sink.desc.to_lowercase();
    let name_lower = sink.name.to_lowercase();

    if desc_lower.contains("hdmi") || desc_lower.contains("tv") || desc_lower.contains("display")
        || name_lower.contains("hdmi") {
        "video-display".to_string()
    } else if desc_lower.contains("headphone") || desc_lower.contains("headset")
        || desc_lower.contains("bluetooth") || name_lower.contains("bluez") {
        "audio-headphones".to_string()
    } else {
        // Default for speakers, optical, digital, etc.
        "audio-speakers".to_string()
    }
}

/// Get sink icon for notifications (respects status_bar_icons setting)
fn get_notification_sink_icon(sink: &SinkConfig, status_bar_icons: bool) -> String {
    if status_bar_icons {
        // Custom icons only for status bar; use auto-detected for notifications
        get_sink_icon_auto(sink)
    } else {
        // Use custom icon everywhere
        get_sink_icon(sink)
    }
}

/// Convert app_id to icon name (handles common app_id formats)
fn get_app_icon(app_id: &str) -> String {
    // Handle common app_id patterns that don't directly match icon names
    match app_id {
        "org.mozilla.firefox" => "firefox".to_string(),
        "org.mozilla.Thunderbird" => "thunderbird".to_string(),
        "org.gnome.Nautilus" => "nautilus".to_string(),
        "org.telegram.desktop" => "telegram".to_string(),
        // Most app_ids can be used directly as icon names
        _ => app_id.to_string(),
    }
}

fn send_notification(summary: &str, body: &str, icon: Option<&str>) -> Result<()> {
    // Use provided icon, or fall back to generic audio icon
    let icon = icon.unwrap_or("audio-card");

    Notification::new()
        .summary(summary)
        .body(body)
        .appname("NASW")
        .icon(icon)
        .timeout(3000)
        .show()
        .context("Failed to show notification")?;

    Ok(())
}
