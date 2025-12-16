//! Configuration management
//!
//! Handles loading, parsing, and validating the TOML configuration file.
//! Supports settings, sink definitions, and window matching rules.

use anyhow::{Context, Result};
use crossterm::style::Stylize;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use tracing::{info, warn};

use crate::style::PwswStyle;

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
#[allow(clippy::struct_excessive_bools)]
pub struct Settings {
    pub default_on_startup: bool,
    pub set_smart_toggle: bool,
    pub notify_manual: bool,
    pub notify_rules: bool,
    pub match_by_index: bool,
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
    pub notify: Option<bool>,
    // Original patterns for display
    pub app_id_pattern: String,
    pub title_pattern: Option<String>,
}

// ============================================================================
// Config File Deserialization (TOML)
// ============================================================================

#[derive(Debug, Deserialize, Serialize)]
struct ConfigFile {
    #[serde(default)]
    settings: SettingsFile,
    #[serde(default)]
    sinks: Vec<SinkConfigFile>,
    #[serde(default)]
    rules: Vec<RuleConfigFile>,
}

#[derive(Debug, Deserialize, Serialize)]
#[allow(clippy::struct_excessive_bools)]
struct SettingsFile {
    #[serde(default = "default_true")]
    default_on_startup: bool,
    #[serde(default = "default_true")]
    set_smart_toggle: bool,
    #[serde(default = "default_true")]
    notify_manual: bool,
    #[serde(default = "default_true")]
    notify_rules: bool,
    #[serde(default)]
    match_by_index: bool,
    #[serde(default = "default_log_level")]
    log_level: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct SinkConfigFile {
    name: String,
    desc: String,
    #[serde(default)]
    icon: Option<String>,
    #[serde(default)]
    default: bool,
}

#[derive(Debug, Deserialize, Serialize)]
struct RuleConfigFile {
    #[serde(with = "serde_regex")]
    app_id: Regex,
    #[serde(default, with = "serde_regex")]
    title: Option<Regex>,
    sink: String,
    #[serde(default)]
    desc: Option<String>,
    #[serde(default)]
    notify: Option<bool>,
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
            default_on_startup: true,
            set_smart_toggle: true,
            notify_manual: true,
            notify_rules: true,
            match_by_index: false,
            log_level: "info".to_string(),
        }
    }
}

// ============================================================================
// Config Implementation
// ============================================================================

impl Config {
    /// Load configuration from the default XDG config path
    ///
    /// # Errors
    /// Returns an error if the config file cannot be read, parsed, or is invalid.
    pub fn load() -> Result<Self> {
        let config_path = Self::get_config_path()?;

        if !config_path.exists() {
            info!("Creating default config at {:?}", config_path);
            Self::create_default_config(&config_path)?;
        }

        let contents = fs::read_to_string(&config_path)
            .with_context(|| format!("Failed to read config: {}", config_path.display()))?;

        let config_file: ConfigFile = toml::from_str(&contents)
            .with_context(|| format!("Failed to parse config: {}", config_path.display()))?;

        Self::from_config_file(config_file)
    }

    fn from_config_file(config_file: ConfigFile) -> Result<Self> {
        if config_file.sinks.is_empty() {
            anyhow::bail!("No sinks defined. Add at least one [[sinks]] section to config.");
        }

        let settings = Settings {
            default_on_startup: config_file.settings.default_on_startup,
            set_smart_toggle: config_file.settings.set_smart_toggle,
            notify_manual: config_file.settings.notify_manual,
            notify_rules: config_file.settings.notify_rules,
            match_by_index: config_file.settings.match_by_index,
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

        let rules = config_file
            .rules
            .into_iter()
            .map(|r| {
                let app_id_pattern = r.app_id.as_str().to_string();
                let title_pattern = r.title.as_ref().map(|t| t.as_str().to_string());
                Rule {
                    app_id_regex: r.app_id,
                    title_regex: r.title,
                    sink_ref: r.sink,
                    desc: r.desc,
                    notify: r.notify,
                    app_id_pattern,
                    title_pattern,
                }
            })
            .collect();

        let config = Config {
            settings,
            sinks,
            rules,
        };
        config.validate()?;
        Ok(config)
    }

    /// Save configuration to the default XDG config path
    ///
    /// # Errors
    /// Returns an error if the config cannot be serialized or written to disk.
    /// # Panics
    /// This function panics if the config path has no parent directory (should never happen).
    pub fn save(&self) -> Result<()> {
        let config_file = self.to_config_file();
        let toml_str =
            toml::to_string_pretty(&config_file).context("Failed to serialize config to TOML")?;

        let config_path = Self::get_config_path()?;
        Self::save_to_path_str(&toml_str, &config_path)
    }

    /// Save configuration to the specified path (used by tests to avoid touching XDG paths)
    pub fn save_to<P: AsRef<std::path::Path>>(&self, path: P) -> Result<()> {
        let config_file = self.to_config_file();
        let toml_str =
            toml::to_string_pretty(&config_file).context("Failed to serialize config to TOML")?;
        Self::save_to_path_str(&toml_str, path.as_ref())
    }

    fn save_to_path_str(path_str: &str, config_path: &std::path::Path) -> Result<()> {
        let dir = config_path
            .parent()
            .expect("Config path must have a parent directory");

        // Write to a temporary file in the same directory and then atomically rename.
        let mut tmp = tempfile::NamedTempFile::new_in(dir)
            .context("Failed to create temporary file for atomic config save")?;
        // Write bytes to temporary file
        tmp.as_file_mut()
            .write_all(path_str.as_bytes())
            .context("Failed to write config to temporary file")?;

        // Ensure user-only permissions on Unix
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(tmp.path(), std::fs::Permissions::from_mode(0o600))
                .with_context(|| {
                    format!(
                        "Failed to set temp file permissions: {}",
                        tmp.path().display()
                    )
                })?;
        }

        // Persist atomically
        tmp.persist(config_path).with_context(|| {
            format!(
                "Failed to persist temporary config file to {}",
                config_path.display()
            )
        })?;

        // Ensure final permissions as well
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(config_path, std::fs::Permissions::from_mode(0o600))
                .with_context(|| {
                    format!(
                        "Failed to set final config permissions: {}",
                        config_path.display()
                    )
                })?;
        }

        Ok(())
    }

    /// Convert runtime `Config` back to serializable `ConfigFile` format
    fn to_config_file(&self) -> ConfigFile {
        let settings = SettingsFile {
            default_on_startup: self.settings.default_on_startup,
            set_smart_toggle: self.settings.set_smart_toggle,
            notify_manual: self.settings.notify_manual,
            notify_rules: self.settings.notify_rules,
            match_by_index: self.settings.match_by_index,
            log_level: self.settings.log_level.clone(),
        };

        let sinks = self
            .sinks
            .iter()
            .map(|s| SinkConfigFile {
                name: s.name.clone(),
                desc: s.desc.clone(),
                icon: s.icon.clone(),
                default: s.default,
            })
            .collect();

        let rules = self
            .rules
            .iter()
            .map(|r| RuleConfigFile {
                app_id: r.app_id_regex.clone(),
                title: r.title_regex.clone(),
                sink: r.sink_ref.clone(),
                desc: r.desc.clone(),
                notify: r.notify,
            })
            .collect();

        ConfigFile {
            settings,
            sinks,
            rules,
        }
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
                let available: Vec<_> = self
                    .sinks
                    .iter()
                    .enumerate()
                    .map(|(idx, s)| format!("{}. '{}'", idx + 1, s.desc))
                    .collect();
                anyhow::bail!(
                    "Rule {} references unknown sink '{}'. Available: [{}]",
                    i + 1,
                    rule.sink_ref,
                    available.join(", ")
                );
            }
        }

        Ok(())
    }

    /// Get the XDG config path for PWSW
    ///
    /// # Errors
    /// Returns an error if the config directory cannot be determined or created.
    pub fn get_config_path() -> Result<PathBuf> {
        let config_dir = dirs::config_dir()
            .context("Could not determine config directory")?
            .join("pwsw");
        fs::create_dir_all(&config_dir)
            .with_context(|| format!("Failed to create config dir: {}", config_dir.display()))?;
        Ok(config_dir.join("config.toml"))
    }

    fn create_default_config(path: &PathBuf) -> Result<()> {
        let default_config = r#"# PWSW (PipeWire Switcher) Configuration
#
# Automatically switches audio sinks based on active windows.
# Uses PipeWire native tools for audio control.
# Supports profile switching for analog/digital outputs.

[settings]
default_on_startup = true  # Switch to default sink on daemon start
set_smart_toggle = true    # set-sink toggles back to default if already active
notify_manual = true       # Desktop notifications: Daemon start/stop + manual set-sink/next-sink/prev-sink
notify_rules = true        # Desktop notifications: Rule-triggered switches (default, override per-rule)
match_by_index = false     # Prioritize matches by [[rule]] position (true) or most recent window (false)
log_level = "info"         # error, warn, info, debug, trace

# Audio sinks
# Find available sinks with: pwsw list-sinks
#
# Icons are auto-detected from sink description (e.g., "HDMI" → video-display).
# Set 'icon' to override with any icon name your system supports.

[[sinks]]
name = "alsa_output.pci-0000_0c_00.4.iec958-stereo"
desc = "Optical Out"
default = true

[[sinks]]
name = "alsa_output.pci-0000_0c_00.4.analog-stereo"
desc = "Headphones"
# icon = "audio-headphones"  # Optional: override auto-detected icon

# Window rules
# Find app_id and title:
#   pwsw list-windows    # Show all open windows (requires daemon running)
#   pwsw test-rule ".*"  # Test pattern matching - .* shows all windows (requires daemon running)
#
# Compositor-specific alternatives:
#   Sway/River: swaymsg -t get_tree
#   Hyprland: hyprctl clients
#   Niri: niri msg windows
#   KDE Plasma: KDE window inspector
#
# Regex patterns (for app_id and title fields):
#   ".*"          - matches any window (useful for testing)
#   "firefox"     - matches anywhere in string
#   "^steam$"     - exact match only
#   "^(mpv|vlc)$" - matches mpv OR vlc
#   "(?i)discord" - case insensitive
#
# Title-only matching:
#   To match only by title (ignoring app_id), use app_id = ".*"
#   Example: Match any window with "YouTube" in title
#     app_id = ".*"
#     title = "YouTube"

[[rules]]
app_id = "^steam$"
title = "^Steam Big Picture Mode$"
sink = "Optical Out"       # Reference by: desc, name, or position (1, 2)
desc = "Steam Big Picture" # Custom name for notifications
# notify = false           # Optional: override notify_rules for this specific rule

# [[rules]]
# app_id = "^mpv$"
# sink = 2
"#;
        fs::write(path, default_config)
            .with_context(|| format!("Failed to write config: {}", path.display()))?;

        // Inform user that we created the config
        eprintln!(
            "{} {}",
            "✓".success(),
            format!("Created default config at: {}", path.display()).success()
        );
        eprintln!();
        eprintln!("{}", "Next steps:".header());
        eprintln!(
            "  1. Run {} to see available audio outputs",
            "pwsw list-sinks".technical()
        );
        eprintln!("  2. Edit the config file to customize sinks and rules");
        eprintln!(
            "  3. Run {} to check your config",
            "pwsw validate".technical()
        );
        eprintln!("  4. Run {} to start", "pwsw daemon".technical());
        eprintln!();

        Ok(())
    }

    /// Print a human-readable summary of the configuration
    pub fn print_summary(&self) {
        println!("{} {}\n", "✓".success(), "Configuration valid".success());

        println!("{}", "Settings:".header());
        println!(
            "  {}: {}",
            "default_on_startup".dim(),
            self.settings.default_on_startup
        );
        println!(
            "  {}: {}",
            "set_smart_toggle".dim(),
            self.settings.set_smart_toggle
        );
        println!(
            "  {}: {}",
            "notify_manual".dim(),
            self.settings.notify_manual
        );
        println!("  {}: {}", "notify_rules".dim(), self.settings.notify_rules);
        println!(
            "  {}: {}",
            "match_by_index".dim(),
            self.settings.match_by_index
        );
        println!(
            "  {}: {}",
            "log_level".dim(),
            self.settings.log_level.as_str().technical()
        );

        println!(
            "\n{} ({}):",
            "Sinks".header(),
            self.sinks.len().to_string().technical()
        );
        for (i, sink) in self.sinks.iter().enumerate() {
            let marker = if sink.default {
                let mut m = String::from(" [");
                m.push_str("DEFAULT".dim().to_string().as_str());
                m.push(']');
                m
            } else {
                String::new()
            };
            println!(
                "  {}. {}{}",
                (i + 1).to_string().dim(),
                sink.desc.as_str().bold(),
                marker
            );
            println!("     {}: {}", "name".dim(), sink.name);
            if let Some(ref icon) = sink.icon {
                println!("     {}: {}", "icon".dim(), icon.as_str().technical());
            }
        }

        if self.rules.is_empty() {
            println!("\n{}", "No rules configured.".dim());
        } else {
            println!(
                "\n{} ({}):",
                "Rules".header(),
                self.rules.len().to_string().technical()
            );
            for (i, rule) in self.rules.iter().enumerate() {
                println!(
                    "  {}. {}: {}",
                    (i + 1).to_string().dim(),
                    "app_id".dim(),
                    rule.app_id_pattern.as_str().technical()
                );
                if let Some(ref title) = rule.title_pattern {
                    println!("     {}: {}", "title".dim(), title.as_str().technical());
                }
                let effective_notify = rule.notify.unwrap_or(self.settings.notify_rules);
                let source = if rule.notify.is_some() {
                    "override"
                } else {
                    "default"
                };
                println!(
                    "     {}: {} ({}: {} - {})",
                    "sink".dim(),
                    rule.sink_ref.as_str().bold(),
                    "notify".dim(),
                    effective_notify,
                    source.dim()
                );
            }
        }

        if let Ok(path) = Self::get_config_path() {
            println!("\n{} {}", "Config:".dim(), path.display());
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
        self.sinks
            .iter()
            .find(|s| s.desc == sink_ref || s.name == sink_ref)
    }

    /// Get the configured default sink
    ///
    /// Returns None if no default sink is configured (which should never happen after
    /// successful config validation, but defensive programming is good practice).
    #[must_use]
    pub fn get_default_sink(&self) -> Option<&SinkConfig> {
        self.sinks.iter().find(|s| s.default)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Test helper functions
    fn make_config(sinks: Vec<SinkConfig>, rules: Vec<Rule>) -> Config {
        Config {
            settings: Settings {
                default_on_startup: true,
                set_smart_toggle: true,
                notify_manual: true,
                notify_rules: true,
                match_by_index: false,
                log_level: "info".to_string(),
            },
            sinks,
            rules,
        }
    }

    fn make_sink(name: &str, desc: &str, default: bool) -> SinkConfig {
        SinkConfig {
            name: name.to_string(),
            desc: desc.to_string(),
            icon: None,
            default,
        }
    }

    fn make_rule(app_id: &str, title: Option<&str>, sink_ref: &str) -> Rule {
        Rule {
            app_id_regex: Regex::new(app_id).unwrap(),
            title_regex: title.map(|t| Regex::new(t).unwrap()),
            sink_ref: sink_ref.to_string(),
            desc: None,
            notify: None,
            app_id_pattern: app_id.to_string(),
            title_pattern: title.map(String::from),
        }
    }

    // validate() tests
    #[test]
    fn test_validate_accepts_single_default_sink() {
        let config = make_config(
            vec![
                make_sink("sink1", "Sink 1", true),
                make_sink("sink2", "Sink 2", false),
            ],
            vec![],
        );
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_validate_rejects_no_default_sink() {
        let config = make_config(
            vec![
                make_sink("sink1", "Sink 1", false),
                make_sink("sink2", "Sink 2", false),
            ],
            vec![],
        );
        let result = config.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("default sink"));
    }

    #[test]
    fn test_validate_rejects_multiple_default_sinks() {
        let config = make_config(
            vec![
                make_sink("sink1", "Sink 1", true),
                make_sink("sink2", "Sink 2", true),
            ],
            vec![],
        );
        let result = config.validate();
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("default sinks found"));
    }

    #[test]
    fn test_validate_rejects_duplicate_sink_names() {
        let config = make_config(
            vec![
                make_sink("duplicate", "Sink 1", true),
                make_sink("duplicate", "Sink 2", false),
            ],
            vec![],
        );
        let result = config.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Duplicate"));
    }

    #[test]
    fn test_validate_rejects_duplicate_sink_descs() {
        let config = make_config(
            vec![
                make_sink("sink1", "Duplicate Desc", true),
                make_sink("sink2", "Duplicate Desc", false),
            ],
            vec![],
        );
        let result = config.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Duplicate"));
    }

    #[test]
    fn test_validate_rejects_unknown_rule_sink_ref() {
        let config = make_config(
            vec![make_sink("sink1", "Sink 1", true)],
            vec![make_rule("firefox", None, "nonexistent")],
        );
        let result = config.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("unknown sink"));
    }

    #[test]
    fn test_validate_rejects_invalid_log_level() {
        let mut config = make_config(vec![make_sink("sink1", "Sink 1", true)], vec![]);
        config.settings.log_level = "invalid".to_string();
        let result = config.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("log_level"));
    }

    #[test]
    fn test_validate_accepts_all_valid_log_levels() {
        for level in &["error", "warn", "info", "debug", "trace"] {
            let mut config = make_config(vec![make_sink("sink1", "Sink 1", true)], vec![]);
            config.settings.log_level = (*level).to_string();
            assert!(
                config.validate().is_ok(),
                "Log level '{level}' should be valid"
            );
        }
    }

    // resolve_sink() tests
    #[test]
    fn test_resolve_sink_by_position_one_indexed() {
        let config = make_config(
            vec![
                make_sink("sink1", "First Sink", true),
                make_sink("sink2", "Second Sink", false),
            ],
            vec![],
        );
        let result = config.resolve_sink("1");
        assert!(result.is_some());
        assert_eq!(result.unwrap().name, "sink1");

        let result2 = config.resolve_sink("2");
        assert!(result2.is_some());
        assert_eq!(result2.unwrap().name, "sink2");
    }

    #[test]
    fn test_resolve_sink_by_position_zero_returns_none() {
        let config = make_config(vec![make_sink("sink1", "Sink 1", true)], vec![]);
        assert!(config.resolve_sink("0").is_none());
    }

    #[test]
    fn test_resolve_sink_by_position_out_of_bounds() {
        let config = make_config(vec![make_sink("sink1", "Sink 1", true)], vec![]);
        assert!(config.resolve_sink("99").is_none());
    }

    #[test]
    fn test_resolve_sink_by_description() {
        let config = make_config(
            vec![
                make_sink("sink1", "HDMI Output", true),
                make_sink("sink2", "Speakers", false),
            ],
            vec![],
        );
        let result = config.resolve_sink("HDMI Output");
        assert!(result.is_some());
        assert_eq!(result.unwrap().name, "sink1");
    }

    #[test]
    fn test_resolve_sink_by_name() {
        let config = make_config(
            vec![
                make_sink("alsa_output.test.stereo", "Test", true),
                make_sink("sink2", "Sink 2", false),
            ],
            vec![],
        );
        let result = config.resolve_sink("alsa_output.test.stereo");
        assert!(result.is_some());
        assert_eq!(result.unwrap().desc, "Test");
    }

    #[test]
    fn test_resolve_sink_not_found() {
        let config = make_config(vec![make_sink("sink1", "Sink 1", true)], vec![]);
        assert!(config.resolve_sink("nonexistent").is_none());
    }

    // get_default_sink() tests
    #[test]
    fn test_get_default_sink_returns_correct_sink() {
        let config = make_config(
            vec![
                make_sink("sink1", "Sink 1", false),
                make_sink("sink2", "Default Sink", true),
                make_sink("sink3", "Sink 3", false),
            ],
            vec![],
        );
        let result = config.get_default_sink();
        assert!(result.is_some());
        assert_eq!(result.unwrap().desc, "Default Sink");
    }

    #[test]
    fn test_save_writes_file_and_permissions() {
        use tempfile::tempdir;

        let dir = tempdir().unwrap();
        let prev = std::env::var_os("XDG_CONFIG_HOME");
        std::env::set_var("XDG_CONFIG_HOME", dir.path());

        let cfg = make_config(vec![make_sink("sink1", "Sink 1", true)], vec![]);
        let path = Config::get_config_path().unwrap();
        // Use test-specific save_to to avoid touching global XDG paths
        cfg.save_to(&path).unwrap();

        assert!(path.exists());

        let contents = std::fs::read_to_string(&path).unwrap();
        assert!(contents.contains("Sink 1"));

        // On Unix, ensure permissions are 0o600
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }

        // Ensure only config.toml exists in the config dir
        let config_dir = path.parent().unwrap();
        let entries: Vec<_> = std::fs::read_dir(config_dir)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0], std::ffi::OsString::from("config.toml"));

        // Restore env
        if let Some(val) = prev {
            std::env::set_var("XDG_CONFIG_HOME", val);
        } else {
            std::env::remove_var("XDG_CONFIG_HOME");
        }
    }

    #[test]
    fn test_save_and_load_roundtrip() {
        use tempfile::tempdir;

        let dir = tempdir().unwrap();
        let prev = std::env::var_os("XDG_CONFIG_HOME");
        std::env::set_var("XDG_CONFIG_HOME", dir.path());

        let cfg = make_config(
            vec![
                make_sink("sink1", "Sink 1", true),
                make_sink("sink2", "Sink 2", false),
            ],
            vec![make_rule("firefox", None, "Sink 1")],
        );

        let path = Config::get_config_path().unwrap();
        cfg.save_to(&path).unwrap();

        let loaded = Config::load().unwrap();
        assert_eq!(loaded.sinks.len(), 2);
        assert!(loaded.resolve_sink("Sink 1").is_some());
        assert_eq!(loaded.rules.len(), 1);
        assert_eq!(loaded.rules[0].sink_ref, "Sink 1");

        // Restore env
        if let Some(val) = prev {
            std::env::set_var("XDG_CONFIG_HOME", val);
        } else {
            std::env::remove_var("XDG_CONFIG_HOME");
        }
    }
}
