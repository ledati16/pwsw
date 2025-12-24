//! Configuration management
//!
//! Handles loading, parsing, and validating the TOML configuration file.
//! Supports settings, sink definitions, and window matching rules.

use color_eyre::eyre::{self, Context, ContextCompat, Result};
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
// Multiple independent boolean flags for different features (not a state machine)
#[derive(Debug, Clone)]
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
    /// `PipeWire` node name
    /// - ALSA: `"alsa_output.pci-0000_0c_00.4.iec958-stereo"`
    /// - Bluetooth: `"bluez_output.40_ED_98_1C_1D_08.1"`
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

// TOML serialization format - mirrors Settings structure with serde defaults
#[derive(Debug, Deserialize, Serialize)]
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
            // When running tests (`cargo test`) avoid creating a default config
            // in the user's real XDG config directory, since tests should not
            // modify user files. Cargo sets `RUST_TEST_THREADS` in test processes,
            // so use its presence as a heuristic for test mode.
            if std::env::var("RUST_TEST_THREADS").is_ok() {
                eyre::bail!(
                    "Config file not found at {} and test mode prevents creating it",
                    config_path.display()
                );
            }

            info!("Creating default config at {:?}", config_path);
            Self::create_default_config(&config_path)?;
        }

        Self::load_from_path(&config_path)
    }

    /// Load configuration from a specific path. Useful for tests to avoid relying on XDG env.
    ///
    /// # Errors
    /// Returns an error if the config file cannot be read, parsed, or if validation fails.
    pub fn load_from_path<P: AsRef<std::path::Path>>(path: P) -> Result<Self> {
        let path = path.as_ref();
        let contents = fs::read_to_string(path).with_context(|| {
            format!(
                "Failed to read config: {path_display}",
                path_display = path.display()
            )
        })?;
        let config_file: ConfigFile = toml::from_str(&contents).with_context(|| {
            format!(
                "Failed to parse config: {path_display}",
                path_display = path.display()
            )
        })?;
        Self::from_config_file(config_file)
    }

    fn from_config_file(config_file: ConfigFile) -> Result<Self> {
        if config_file.sinks.is_empty() {
            eyre::bail!("No sinks defined. Add at least one [[sinks]] section to config.");
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
    ///
    /// # Errors
    /// Returns an error if serialization fails or if the config cannot be written to disk.
    pub fn save_to<P: AsRef<std::path::Path>>(&self, path: P) -> Result<()> {
        let config_file = self.to_config_file();
        let toml_str =
            toml::to_string_pretty(&config_file).context("Failed to serialize config to TOML")?;
        Self::save_to_path_str(&toml_str, path.as_ref())
    }

    fn write_temp_file_with_contents(
        dir: &std::path::Path,
        contents: &str,
    ) -> Result<tempfile::NamedTempFile> {
        // Ensure dir exists right before creating the temp file to avoid races
        std::fs::create_dir_all(dir)
            .with_context(|| format!("Failed to create temp dir: {}", dir.display()))?;

        // Try creating the temp file with a small retry loop to tolerate transient races
        let mut last_err = None;
        let mut tmp = None;
        for attempt in 0..3 {
            match tempfile::NamedTempFile::new_in(dir) {
                Ok(f) => {
                    tmp = Some(f);
                    break;
                }
                Err(e) => {
                    last_err = Some(e);
                    std::thread::sleep(std::time::Duration::from_millis(20 * (attempt + 1)));
                    // retry
                }
            }
        }
        let Some(mut tmp) = tmp else {
            return Err(last_err.unwrap())
                .wrap_err("Failed to create temporary file for atomic config save");
        };
        tmp.as_file_mut()
            .write_all(contents.as_bytes())
            .context("Failed to write config to temporary file")?;
        tmp.as_file_mut()
            .flush()
            .context("Failed to flush temporary config file")?;
        tmp.as_file_mut()
            .sync_all()
            .context("Failed to sync temporary config file")?;
        Ok(tmp)
    }

    fn ensure_not_empty_overwrite(
        tmp: &mut tempfile::NamedTempFile,
        config_path: &std::path::Path,
    ) -> Result<()> {
        let written_len = tmp
            .as_file_mut()
            .metadata()
            .context("Failed to stat temporary config file")?
            .len();
        if written_len == 0
            && config_path.exists()
            && let Ok(meta) = fs::metadata(config_path)
            && meta.is_file()
            && meta.len() > 0
        {
            eyre::bail!("Refusing to overwrite non-empty config with empty data");
        }
        Ok(())
    }

    fn ensure_unix_permissions(path: &std::path::Path) -> Result<()> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
                .with_context(|| format!("Failed to set permissions: {}", path.display()))?;
        }
        Ok(())
    }

    fn ensure_write_allowed_and_backup(config_path: &std::path::Path) -> Result<()> {
        if let Some(home_dir) = dirs::home_dir() {
            let home_cfg = home_dir.join(".config").join("pwsw").join("config.toml");
            if config_path == home_cfg {
                // Create a timestamped backup of the existing file if present
                if config_path.exists()
                    && let Ok(metadata) = fs::metadata(config_path)
                    && metadata.is_file()
                {
                    use std::time::{SystemTime, UNIX_EPOCH};
                    if let Ok(n) = SystemTime::now().duration_since(UNIX_EPOCH) {
                        let bak_name = format!("config.toml.bak.{}", n.as_secs());
                        let bak_path = config_path.parent().unwrap().join(bak_name);
                        let _ = fs::copy(config_path, &bak_path);
                        // Best-effort: ignore copy errors but try to continue
                    }
                }

                // Logging: record attempted write details to a safe temp log
                let _ = (|| -> std::io::Result<()> {
                    use std::io::Write as _;
                    let mut f = std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open("/tmp/pwsw-config-write.log")?;
                    let pid = std::process::id();
                    let _ = writeln!(
                        f,
                        "[{pid}] Attempting to write real config at {}",
                        config_path.display()
                    );
                    let _ = writeln!(
                        f,
                        "  RUST_TEST_THREADS={:?}",
                        std::env::var("RUST_TEST_THREADS").ok()
                    );
                    let _ = writeln!(
                        f,
                        "  PWSW_ALLOW_CONFIG_WRITE={:?}",
                        std::env::var("PWSW_ALLOW_CONFIG_WRITE").ok()
                    );
                    Ok(())
                })();

                // Only enforce the env opt-in when running tests. In normal runtime
                // (TUI/daemon), allow writing the user's config without requiring
                // `PWSW_ALLOW_CONFIG_WRITE`.
                if std::env::var("RUST_TEST_THREADS").is_ok() {
                    match std::env::var("PWSW_ALLOW_CONFIG_WRITE") {
                        Ok(val) if val == "1" => {
                            // explicit allow; proceed
                        }
                        _ => {
                            eyre::bail!(
                                "Refusing to write real user config at {} without PWSW_ALLOW_CONFIG_WRITE=1",
                                config_path.display()
                            );
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn save_to_path_str(path_str: &str, config_path: &std::path::Path) -> Result<()> {
        let dir = config_path
            .parent()
            .expect("Config path must have a parent directory");

        // Ensure the target directory exists (tests may create temp dirs but callers
        // can race or remove them); create_dir_all is idempotent.
        fs::create_dir_all(dir)
            .with_context(|| format!("Failed to create config dir: {}", dir.display()))?;

        let mut tmp = Self::write_temp_file_with_contents(dir, path_str)?;

        Self::ensure_not_empty_overwrite(&mut tmp, config_path)?;

        // Ensure user-only permissions on Unix for the temp file
        Self::ensure_unix_permissions(tmp.path())?;

        Self::ensure_write_allowed_and_backup(config_path)?;

        // Persist atomically
        tmp.persist(config_path).with_context(|| {
            format!(
                "Failed to persist temporary config file to {}",
                config_path.display()
            )
        })?;

        // Ensure final permissions as well
        Self::ensure_unix_permissions(config_path)?;

        // Sync parent directory to ensure rename is durable
        #[cfg(unix)]
        if let Some(parent) = config_path.parent()
            && let Ok(dir_file) = std::fs::File::open(parent)
        {
            dir_file
                .sync_all()
                .with_context(|| format!("Failed to sync directory: {}", parent.display()))?;
        }

        Ok(())
    }

    #[cfg(test)]
    /// Test helper to call the private save path function with raw string
    pub(crate) fn save_str_for_test(path_str: &str, path: &std::path::Path) -> Result<()> {
        Self::save_to_path_str(path_str, path)
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
            level => eyre::bail!(
                "Invalid log_level '{level}'. Must be: error, warn, info, debug, or trace"
            ),
        }

        // Exactly one default sink
        let default_count = self.sinks.iter().filter(|s| s.default).count();
        match default_count {
            0 => eyre::bail!("No default sink. Mark one sink with 'default = true'"),
            1 => {}
            n => eyre::bail!("{n} default sinks found. Only one allowed."),
        }

        // No duplicate descriptions or names
        let mut seen_descs = HashSet::with_capacity(self.sinks.len());
        let mut seen_names = HashSet::with_capacity(self.sinks.len());
        for sink in &self.sinks {
            if !seen_descs.insert(&sink.desc) {
                eyre::bail!("Duplicate sink description: '{}'", sink.desc);
            }
            if !seen_names.insert(&sink.name) {
                eyre::bail!("Duplicate sink name: '{}'", sink.name);
            }
            // Validate name doesn't look like a position number
            if sink.desc.parse::<usize>().is_ok() {
                warn!(
                    "Sink description '{}' looks like a number - this may cause confusion with position references",
                    sink.desc
                );
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
                eyre::bail!(
                    "Rule {} references unknown sink '{}'. Available: [{}]",
                    i + 1,
                    rule.sink_ref,
                    available.join(", ")
                );
            }

            // Validate regex patterns for catastrophic backtracking
            Self::validate_regex_safe(&rule.app_id_pattern, "app_id", i + 1)?;
            if let Some(ref title_pattern) = rule.title_pattern {
                Self::validate_regex_safe(title_pattern, "title", i + 1)?;
            }
        }

        Ok(())
    }

    /// Validate that a regex pattern is safe from catastrophic backtracking
    ///
    /// Checks for known dangerous patterns that can cause exponential time complexity.
    ///
    /// # Errors
    /// Returns an error if the pattern contains dangerous constructs.
    fn validate_regex_safe(pattern: &str, field_name: &str, rule_num: usize) -> Result<()> {
        // Check for known catastrophic backtracking patterns
        let dangerous_patterns = [
            ("(.*)*", "nested quantifiers on wildcard"),
            ("(.*)+", "nested quantifiers on wildcard"),
            ("(.+)+", "nested quantifiers on wildcard"),
            ("(.+)*", "nested quantifiers on wildcard"),
            ("([^x]*)*", "nested quantifiers on negated character class"),
            ("([^x]+)+", "nested quantifiers on negated character class"),
        ];

        for (danger, reason) in &dangerous_patterns {
            if pattern.contains(danger) {
                eyre::bail!(
                    "Rule {}: {} pattern '{}' contains dangerous construct '{}' ({})",
                    rule_num,
                    field_name,
                    pattern,
                    danger,
                    reason
                );
            }
        }

        // Check for excessive alternations that might cause backtracking
        let alternation_count = pattern.matches('|').count();
        if alternation_count > 50 {
            warn!(
                "Rule {}: {} pattern has {} alternations - this may cause slow matching",
                rule_num, field_name, alternation_count
            );
        }

        // Check for excessive nested groups
        let open_paren_count = pattern.matches('(').count();
        if open_paren_count > 20 {
            warn!(
                "Rule {}: {} pattern has {} nested groups - this may cause slow compilation",
                rule_num, field_name, open_paren_count
            );
        }

        Ok(())
    }

    /// Get the XDG config path for PWSW
    ///
    /// # Errors
    /// Returns an error if the config directory cannot be determined or created.
    /// Get the configured XDG config path for PWSW without creating directories.
    ///
    /// This function intentionally does not create the directory on disk. Callers
    /// that intend to write the config should create the parent directory first
    /// (see `save` and internal helpers). Avoiding directory creation here prevents
    /// test code from accidentally touching the user's real XDG config when it only
    /// needs the path.
    pub fn get_config_path() -> Result<PathBuf> {
        // Compute the XDG config dir path for PWSW but do NOT create it here.
        // Creating the directory had the side-effect of touching the user's
        // real XDG config during tests when helper code only needed the path.
        // Directory creation is performed where needed (e.g., on save), so
        // returning the path without creating the directory avoids escaping test
        // sandboxes.
        let config_dir = dirs::config_dir()
            .context("Could not determine config directory")?
            .join("pwsw");
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
        // Ensure parent directory exists (tests may set a temp XDG_CONFIG_HOME)
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create config dir: {}", parent.display()))?;
        }

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
    use crate::test_utils::fixtures::{make_config, make_rule, make_sink};
    use rstest::rstest;

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

    /// Parameterized test for validation rejection cases
    #[rstest]
    #[case("no_default",
           vec![make_sink("sink1", "Sink 1", false), make_sink("sink2", "Sink 2", false)],
           vec![],
           None,
           "default sink")]
    #[case("multiple_defaults",
           vec![make_sink("sink1", "Sink 1", true), make_sink("sink2", "Sink 2", true)],
           vec![],
           None,
           "default sinks found")]
    #[case("duplicate_names",
           vec![make_sink("duplicate", "Sink 1", true), make_sink("duplicate", "Sink 2", false)],
           vec![],
           None,
           "Duplicate")]
    #[case("duplicate_descs",
           vec![make_sink("sink1", "Duplicate Desc", true), make_sink("sink2", "Duplicate Desc", false)],
           vec![],
           None,
           "Duplicate")]
    #[case("unknown_sink_ref",
           vec![make_sink("sink1", "Sink 1", true)],
           vec![make_rule("firefox", None, "nonexistent")],
           None,
           "unknown sink")]
    #[case("invalid_log_level",
           vec![make_sink("sink1", "Sink 1", true)],
           vec![],
           Some("invalid"),
           "log_level")]
    fn test_validate_rejection_cases(
        #[case] name: &str,
        #[case] sinks: Vec<SinkConfig>,
        #[case] rules: Vec<Rule>,
        #[case] log_level_override: Option<&str>,
        #[case] expected_error_substring: &str,
    ) {
        let mut config = make_config(sinks, rules);
        if let Some(level) = log_level_override {
            config.settings.log_level = level.to_string();
        }

        let result = config.validate();
        assert!(
            result.is_err(),
            "Expected validation to fail for case: {name}"
        );
        let error_msg = result.unwrap_err().to_string();
        assert!(
            error_msg.contains(expected_error_substring),
            "Expected error to contain '{expected_error_substring}', but got: {error_msg}"
        );
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
        use crate::test_utils::XdgTemp;

        let guard = XdgTemp::new();
        {
            let cfg = make_config(vec![make_sink("sink1", "Sink 1", true)], vec![]);
            // Use temp directory path directly to avoid race with parallel tests
            let path = guard.path().join("pwsw").join("config.toml");
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
        }
        drop(guard);
    }

    #[test]
    fn test_save_and_load_roundtrip() {
        use crate::test_utils::XdgTemp;

        let guard = XdgTemp::new();
        {
            let cfg = make_config(
                vec![
                    make_sink("sink1", "Sink 1", true),
                    make_sink("sink2", "Sink 2", false),
                ],
                vec![make_rule("firefox", None, "Sink 1")],
            );

            // Use temp directory path directly to avoid race with parallel tests
            let path = guard.path().join("pwsw").join("config.toml");
            cfg.save_to(&path).unwrap();

            let loaded = Config::load_from_path(&path).unwrap();
            assert_eq!(loaded.sinks.len(), 2);
            assert!(loaded.resolve_sink("Sink 1").is_some());
            assert_eq!(loaded.rules.len(), 1);
            assert_eq!(loaded.rules[0].sink_ref, "Sink 1");
        }
        drop(guard);
    }

    #[test]
    fn test_refuse_empty_overwrite() {
        use tempfile::tempdir;
        // Create tempdir and a non-empty file
        let dir = tempdir().unwrap();
        let cfg_dir = dir.path().join("pwsw");
        std::fs::create_dir_all(&cfg_dir).unwrap();
        let path = cfg_dir.join("config.toml");
        std::fs::write(&path, "non-empty").unwrap();

        // Attempt to overwrite with empty content should fail
        let r = Config::save_str_for_test("", &path);
        assert!(r.is_err());
    }

    // Regex validation tests
    #[test]
    fn test_validate_regex_safe_accepts_normal_patterns() {
        let result = Config::validate_regex_safe("^firefox$", "app_id", 1);
        assert!(result.is_ok());

        let result = Config::validate_regex_safe("(mpv|vlc)", "app_id", 1);
        assert!(result.is_ok());

        let result = Config::validate_regex_safe(".*steam.*", "title", 1);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_regex_safe_rejects_catastrophic_backtracking() {
        // Test all dangerous patterns
        let dangerous = ["(.*)*", "(.*)+", "(.+)+", "(.+)*", "([^x]*)*", "([^x]+)+"];

        for pattern in &dangerous {
            let result = Config::validate_regex_safe(pattern, "app_id", 1);
            assert!(
                result.is_err(),
                "Pattern '{pattern}' should be rejected as potentially catastrophic"
            );

            let err = result.unwrap_err().to_string();
            assert!(
                err.contains("dangerous construct"),
                "Error should mention dangerous constructs, got: {err}"
            );
        }
    }

    #[test]
    fn test_validate_regex_safe_embedded_in_larger_pattern() {
        // Dangerous pattern embedded in larger context should still be caught
        let result = Config::validate_regex_safe("^firefox(.*)*$", "app_id", 1);
        assert!(result.is_err());

        let result = Config::validate_regex_safe("steam|(.+)+|mpv", "app_id", 1);
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_with_catastrophic_regex() {
        // Integration test: config validation should reject catastrophic patterns
        let config = make_config(
            vec![make_sink("sink1", "Sink 1", true)],
            vec![make_rule("(.*)*", None, "sink1")],
        );

        let result = config.validate();
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("dangerous"));
    }

    #[test]
    fn test_validate_catastrophic_in_title_pattern() {
        // Test that title patterns are also validated
        let config = make_config(
            vec![make_sink("sink1", "Sink 1", true)],
            vec![make_rule("firefox", Some("(.+)+"), "sink1")],
        );

        let result = config.validate();
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("dangerous"));
        assert!(err.contains("title"));
    }
}
