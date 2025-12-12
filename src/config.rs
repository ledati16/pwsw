//! Configuration management
//!
//! Handles loading, parsing, and validating the TOML configuration file.
//! Supports settings, sink definitions, and window matching rules.

use anyhow::{Context, Result};
use regex::Regex;
use serde::Deserialize;
use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;
use tracing::{info, warn};

// ============================================================================
// Public Configuration Types
// ============================================================================

/// Main configuration structure
#[derive(Debug, Clone)]
pub struct Config {
    pub settings: Settings,
    pub sinks: Vec<SinkConfig>,
    pub rules: Vec<Rule>,
}

/// Global settings
#[derive(Debug, Clone)]
pub struct Settings {
    pub reset_on_startup: bool,
    pub set_smart_toggle: bool,
    pub notify_daemon: bool,
    pub notify_switch: bool,
    pub notify_set: bool,
    /// If true, custom sink icons only apply to `--get-sink --json`.
    /// Notifications will use auto-detected `FreeDesktop` icons for compatibility.
    pub status_bar_icons: bool,
    pub log_level: String,
}

/// Audio sink configuration
#[derive(Debug, Clone)]
pub struct SinkConfig {
    /// `PipeWire` node name (e.g., `"alsa_output.pci-0000_0c_00.4.iec958-stereo"`)
    pub name: String,
    /// Human-readable description
    pub desc: String,
    /// Optional icon for status bars (if not set, auto-detected)
    pub icon: Option<String>,
    /// Whether this is the default fallback sink
    pub default: bool,
}

/// Window matching rule
#[derive(Debug, Clone)]
pub struct Rule {
    pub app_id_regex: Regex,
    pub title_regex: Option<Regex>,
    pub sink_ref: String,
    pub desc: Option<String>,
    pub notify: bool,
    // Original patterns for display
    pub app_id_pattern: String,
    pub title_pattern: Option<String>,
}

// ============================================================================
// Config File Deserialization (TOML)
// ============================================================================

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
    set_smart_toggle: bool,
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
            set_smart_toggle: true,
            notify_daemon: true,
            notify_switch: true,
            notify_set: true,
            status_bar_icons: false,
            log_level: "info".to_string(),
        }
    }
}

// ============================================================================
// Config Implementation
// ============================================================================

impl Config {
    /// Load configuration from the default XDG config path
    pub fn load() -> Result<Self> {
        let config_path = Self::get_config_path()?;

        if !config_path.exists() {
            info!("Creating default config at {:?}", config_path);
            Self::create_default_config(&config_path)?;
        }

        let contents = fs::read_to_string(&config_path)
            .with_context(|| format!("Failed to read config: {config_path:?}"))?;

        let config_file: ConfigFile = toml::from_str(&contents)
            .with_context(|| format!("Failed to parse config: {config_path:?}"))?;

        Self::from_config_file(config_file)
    }

    fn from_config_file(config_file: ConfigFile) -> Result<Self> {
        if config_file.sinks.is_empty() {
            anyhow::bail!("No sinks defined. Add at least one [[sinks]] section to config.");
        }

        let settings = Settings {
            reset_on_startup: config_file.settings.reset_on_startup,
            set_smart_toggle: config_file.settings.set_smart_toggle,
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
                "Invalid log_level '{level}'. Must be: error, warn, info, debug, or trace"
            ),
        }

        // Exactly one default sink
        let default_count = self.sinks.iter().filter(|s| s.default).count();
        match default_count {
            0 => anyhow::bail!("No default sink. Mark one sink with 'default = true'"),
            1 => {}
            n => anyhow::bail!("{n} default sinks found. Only one allowed."),
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

    /// Get the XDG config path for PWSW
    pub fn get_config_path() -> Result<PathBuf> {
        let config_dir = dirs::config_dir()
            .context("Could not determine config directory")?
            .join("pwsw");
        fs::create_dir_all(&config_dir)
            .with_context(|| format!("Failed to create config dir: {config_dir:?}"))?;
        Ok(config_dir.join("config.toml"))
    }

    fn create_default_config(path: &PathBuf) -> Result<()> {
        let default_config = r#"# PWSW (PipeWire Switcher) Configuration
#
# Automatically switches audio sinks based on active windows.
# Uses PipeWire native tools for audio control.
# Supports profile switching for analog/digital outputs.

[settings]
reset_on_startup = true    # Reset to default sink on daemon start
set_smart_toggle = true    # set-sink toggles back to default if already active
notify_daemon = true       # Notifications for daemon start/stop
notify_switch = true       # Notifications for rule-triggered switches (per-rule notify must also be true)
notify_set = true          # Notifications for set-sink, next-sink, and prev-sink commands
status_bar_icons = false   # If true, custom icons only apply to --get-sink --json
log_level = "info"         # error, warn, info, debug, trace

# Audio sinks
# Find available sinks with: pwsw --list-sinks
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
# Find app_id and title with compositor-specific tools:
#   Sway/River/etc: swaymsg -t get_tree
#   Hyprland: hyprctl clients
#   Niri: niri msg windows
#   KDE Plasma: Use KDE's window inspector
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
            .with_context(|| format!("Failed to write config: {path:?}"))?;

        // Inform user that we created the config
        eprintln!("Created default config at: {path:?}");
        eprintln!();
        eprintln!("Next steps:");
        eprintln!("  1. Run 'pwsw list-sinks' to see available audio outputs");
        eprintln!("  2. Edit the config file to customize sinks and rules");
        eprintln!("  3. Run 'pwsw validate' to check your config");
        eprintln!("  4. Run 'pwsw daemon' to start");
        eprintln!();

        Ok(())
    }

    /// Print a human-readable summary of the configuration
    pub fn print_summary(&self) {
        println!("✓ Configuration valid\n");

        println!("Settings:");
        println!("  reset_on_startup: {}", self.settings.reset_on_startup);
        println!("  set_smart_toggle: {}", self.settings.set_smart_toggle);
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
                println!("     icon: {icon}");
            }
        }

        if self.rules.is_empty() {
            println!("\nNo rules configured.");
        } else {
            println!("\nRules ({}):", self.rules.len());
            for (i, rule) in self.rules.iter().enumerate() {
                println!("  {}. app_id: {}", i + 1, rule.app_id_pattern);
                if let Some(ref title) = rule.title_pattern {
                    println!("     title: {title}");
                }
                println!("     sink: {} (notify: {})", rule.sink_ref, rule.notify);
            }
        }

        if let Ok(path) = Self::get_config_path() {
            println!("\nConfig: {path:?}");
        }
    }

    /// Resolve a sink reference (by position, desc, or name)
    #[must_use]
    pub fn resolve_sink(&self, sink_ref: &str) -> Option<&SinkConfig> {
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

    /// Get the configured default sink
    ///
    /// Returns None if no default sink is configured (which should never happen after
    /// successful config validation, but defensive programming is good practice).
    #[must_use]
    pub fn get_default_sink(&self) -> Option<&SinkConfig> {
        self.sinks.iter().find(|s| s.default)
    }

    /// Check if notifications should be sent for a rule-triggered switch
    #[must_use]
    pub fn should_notify_switch(&self, rule_notify: bool) -> bool {
        self.settings.notify_switch && rule_notify
    }
}
