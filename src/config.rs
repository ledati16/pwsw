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
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use tracing::{info, warn};

use crate::style::PwswStyle;

// ============================================================================
// Config Path Selection
// ============================================================================

/// Environment variable that selects an alternative config file.
pub const CONFIG_PATH_ENV: &str = "PWSW_CONFIG";

/// Process-wide `--config <PATH>` override, set once from `main`.
static CONFIG_PATH_OVERRIDE: OnceLock<PathBuf> = OnceLock::new();

/// Make a user-supplied config path absolute (relative to the current directory).
fn absolute_config_path(path: &Path) -> Result<PathBuf> {
    if path.as_os_str().is_empty() {
        eyre::bail!("Config path must not be empty");
    }
    std::path::absolute(path)
        .with_context(|| format!("Failed to resolve config path: {}", path.display()))
}

/// Pick the explicit config path: the flag override wins, then a non-empty env value.
///
/// Pure function (no global or env access) so precedence can be unit tested.
fn resolve_config_override(
    flag: Option<&PathBuf>,
    env_value: Option<&std::ffi::OsStr>,
) -> Result<Option<PathBuf>> {
    if let Some(path) = flag {
        return Ok(Some(path.clone()));
    }
    match env_value {
        Some(value) if !value.is_empty() => absolute_config_path(Path::new(value)).map(Some),
        _ => Ok(None),
    }
}

// ============================================================================
// Public Configuration Types
// ============================================================================

/// Main configuration structure
#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub settings: Settings,
    pub sinks: Vec<SinkConfig>,
    pub rules: Vec<Rule>,
}

/// Global settings
// Multiple independent boolean flags for different features (not a state machine)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    pub default_on_startup: bool,
    pub set_smart_toggle: bool,
    pub notify_manual: bool,
    pub notify_rules: bool,
    pub match_by_index: bool,
    pub log_level: String,
}

/// Audio sink configuration
#[derive(Debug, Clone, PartialEq, Eq)]
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

impl PartialEq for Rule {
    fn eq(&self, other: &Self) -> bool {
        // Compare pattern strings instead of compiled Regex (which doesn't impl PartialEq).
        // The patterns are the source of truth; compiled regexes are derived from them.
        self.app_id_pattern == other.app_id_pattern
            && self.title_pattern == other.title_pattern
            && self.sink_ref == other.sink_ref
            && self.desc == other.desc
            && self.notify == other.notify
    }
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
    #[serde(default)] // false - safer for new users with example sink names
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

const fn default_true() -> bool {
    true
}

fn default_log_level() -> String {
    "info".to_string()
}

impl Default for SettingsFile {
    fn default() -> Self {
        Self {
            default_on_startup: false,
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
    /// Load configuration from the active config path (see [`Config::get_config_path`]).
    ///
    /// Creates a default config at that path if no file exists yet.
    ///
    /// # Errors
    /// Returns an error if the config path cannot be determined, or if the config file
    /// cannot be created, read, parsed, or is invalid.
    pub fn load() -> Result<Self> {
        let config_path = Self::get_config_path()?;

        if !config_path.exists() {
            info!("Creating default config at {}", config_path.display());
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
        // Note: Empty sinks is allowed at config level (TUI can work with it).
        // Daemon startup will check for sinks before running.

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

        let config = Self {
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
            return Err(last_err.expect("last_err should be Some if tmp is None"))
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

    /// Keep a single `config.toml.bak` copy of the user's default config before overwriting it.
    ///
    /// Only one backup is kept (overwrites previous) since atomic writes already prevent
    /// corruption - this is just for user recovery ("undo"). Backup failures are logged,
    /// never fatal.
    fn backup_user_config(config_path: &std::path::Path) {
        let Some(home_dir) = dirs::home_dir() else {
            return;
        };
        let home_cfg = home_dir.join(".config").join("pwsw").join("config.toml");
        if config_path != home_cfg {
            return;
        }
        if let Ok(metadata) = fs::metadata(config_path)
            && metadata.is_file()
            && let Some(parent) = config_path.parent()
        {
            let bak_path = parent.join("config.toml.bak");
            if let Err(e) = fs::copy(config_path, &bak_path) {
                warn!(
                    "Failed to create config backup at {}: {}",
                    bak_path.display(),
                    e
                );
            }
        }
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

        Self::backup_user_config(config_path);

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

        // If sinks exist, exactly one must be default
        // (Empty sinks is valid at config level - daemon will check at startup)
        if !self.sinks.is_empty() {
            let default_count = self.sinks.iter().filter(|s| s.default).count();
            match default_count {
                0 => eyre::bail!(
                    "Sinks configured but none marked as default. Add 'default = true' to one sink."
                ),
                1 => {}
                n => eyre::bail!("{n} default sinks found. Only one allowed."),
            }
        }

        // Rules require sinks to resolve against
        if self.sinks.is_empty() && !self.rules.is_empty() {
            eyre::bail!("Rules defined but no sinks configured. Add sinks or remove rules.");
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

        // All rule sinks must exist and patterns must be non-empty
        for (i, rule) in self.rules.iter().enumerate() {
            // Empty app_id pattern would match everything - require explicit `.*` for that
            if rule.app_id_pattern.is_empty() {
                eyre::bail!(
                    "Rule {} has empty app_id pattern. Use '.*' to match all windows.",
                    i + 1
                );
            }

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

    /// Get the active config file path without creating anything on disk.
    ///
    /// Resolution order (first match wins):
    /// 1. The `--config <PATH>` override registered with [`Config::set_config_path_override`]
    /// 2. The `PWSW_CONFIG` environment variable (ignored when empty)
    /// 3. `$XDG_CONFIG_HOME/pwsw/config.toml` (via the `dirs` crate)
    ///
    /// Overrides are made absolute against the current directory so that the daemon's
    /// file watcher and re-spawned child processes see the same path.
    ///
    /// This function intentionally does not create the directory; callers that write
    /// the config create the parent directory themselves (see `save`).
    ///
    /// # Errors
    /// Returns an error if an override path cannot be made absolute, or if no override
    /// is set and the XDG config directory cannot be determined.
    pub fn get_config_path() -> Result<PathBuf> {
        if let Some(path) = Self::config_path_override()? {
            return Ok(path);
        }
        Self::default_config_path()
    }

    /// Register the `--config <PATH>` override for this process.
    ///
    /// Must be called once, before anything loads or saves the config. Takes precedence
    /// over `PWSW_CONFIG`. The process environment is not modified.
    ///
    /// # Errors
    /// Returns an error if the path is empty, cannot be made absolute, or an override
    /// was already registered.
    pub fn set_config_path_override(path: &Path) -> Result<()> {
        let path = absolute_config_path(path)?;
        CONFIG_PATH_OVERRIDE.set(path).map_err(|rejected| {
            eyre::eyre!(
                "Config path override already set; refusing to change it to {}",
                rejected.display()
            )
        })
    }

    /// The explicitly selected config path, if any (`--config` flag, else `PWSW_CONFIG`).
    ///
    /// Returns `Ok(None)` when the default XDG path is in use. Used to forward the same
    /// selection to child processes that `pwsw` spawns of itself.
    ///
    /// # Errors
    /// Returns an error if `PWSW_CONFIG` is set but cannot be made absolute.
    pub fn config_path_override() -> Result<Option<PathBuf>> {
        let env_value = std::env::var_os(CONFIG_PATH_ENV);
        resolve_config_override(CONFIG_PATH_OVERRIDE.get(), env_value.as_deref())
    }

    /// Default config path: `$XDG_CONFIG_HOME/pwsw/config.toml`.
    fn default_config_path() -> Result<PathBuf> {
        let config_dir = dirs::config_dir()
            .context("Could not determine config directory")?
            .join("pwsw");

        let config_path = config_dir.join("config.toml");

        // Sanity check: Ensure the final path is still within the expected directory
        // This validates that the dirs crate didn't return a malicious path
        if !config_path.starts_with(&config_dir) {
            eyre::bail!(
                "Config path validation failed: path escapes config directory\n\
                 Config dir: {}\n\
                 Config path: {}",
                config_dir.display(),
                config_path.display()
            );
        }

        Ok(config_path)
    }

    fn create_default_config(path: &PathBuf) -> Result<()> {
        let default_config = r#"# PWSW (PipeWire Switcher) Configuration
#
# Run `pwsw tui` for interactive setup, or configure manually below.
# Find available sinks with: pwsw list-sinks

[settings]
default_on_startup = false # Switch to default sink on daemon start
set_smart_toggle = true    # set-sink toggles back to default if already active
notify_manual = true       # Desktop notifications for manual switches
notify_rules = true        # Desktop notifications for rule-triggered switches
match_by_index = false     # Priority: false = recent window, true = rule order
log_level = "info"         # error, warn, info, debug, trace

# Audio sinks - add at least one with default = true before starting daemon
# Find available sinks with: pwsw list-sinks
# Icons are auto-detected from description (e.g., "HDMI" → video-display)
#
# Example:
#
# [[sinks]]
# name = "alsa_output.pci-0000_00_1f.3.analog-stereo"  # from pwsw list-sinks
# desc = "Speakers"
# default = true
#
# [[sinks]]
# name = "alsa_output.pci-0000_00_1f.3.hdmi-stereo"
# desc = "HDMI"
# icon = "video-display"  # Optional: override auto-detected icon

# Window rules - match windows to sinks
# Find app_id and title with: pwsw list-windows (requires running daemon)
# Or use compositor tools: swaymsg -t get_tree, hyprctl clients, niri msg windows
#
# Regex patterns:
#   "firefox"     - matches anywhere in string
#   "^steam$"     - exact match only
#   "^(mpv|vlc)$" - matches mpv OR vlc
#   "(?i)discord" - case insensitive
#
# Example:
#
# [[rules]]
# app_id = "^steam$"
# title = "^Steam Big Picture"  # Optional: also match window title
# sink = "HDMI"                 # Reference by desc, name, or position (1, 2)
# desc = "Steam Gaming"         # Optional: custom notification text
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
            "  Run {} for interactive setup (recommended)",
            "pwsw tui".technical()
        );
        eprintln!();
        eprintln!("  Or configure manually:");
        eprintln!(
            "    {}  - discover audio outputs",
            "pwsw list-sinks".technical()
        );
        eprintln!("    Edit config file - add sinks and rules");
        eprintln!("    {}    - check config", "pwsw validate".technical());
        eprintln!("    {}      - start the daemon", "pwsw daemon".technical());
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

    /// Empty sinks is valid at config level (TUI can work with it, daemon checks at startup)
    #[test]
    fn test_validate_accepts_empty_sinks() {
        let config = make_config(vec![], vec![]);
        assert!(
            config.validate().is_ok(),
            "Config with no sinks and no rules should be valid"
        );
    }

    /// Parameterized test for validation rejection cases
    #[rstest]
    #[case("sinks_but_no_default",
           vec![make_sink("sink1", "Sink 1", false), make_sink("sink2", "Sink 2", false)],
           vec![],
           None,
           "none marked as default")]
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
    #[case("empty_app_id_pattern",
           vec![make_sink("sink1", "Sink 1", true)],
           vec![make_rule("", None, "sink1")],
           None,
           "empty app_id pattern")]
    #[case("rules_without_sinks",
           vec![],
           vec![make_rule("firefox", None, "nonexistent")],
           None,
           "no sinks configured")]
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
        let dir = tempfile::tempdir().expect("failed to create tempdir");
        {
            let cfg = make_config(vec![make_sink("sink1", "Sink 1", true)], vec![]);
            let path = dir.path().join("pwsw").join("config.toml");
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
        drop(dir);
    }

    #[test]
    fn test_save_and_load_roundtrip() {
        let dir = tempfile::tempdir().expect("failed to create tempdir");
        {
            let cfg = make_config(
                vec![
                    make_sink("sink1", "Sink 1", true),
                    make_sink("sink2", "Sink 2", false),
                ],
                vec![make_rule("firefox", None, "Sink 1")],
            );

            let path = dir.path().join("pwsw").join("config.toml");
            cfg.save_to(&path).unwrap();

            let loaded = Config::load_from_path(&path).unwrap();
            assert_eq!(loaded.sinks.len(), 2);
            assert!(loaded.resolve_sink("Sink 1").is_some());
            assert_eq!(loaded.rules.len(), 1);
            assert_eq!(loaded.rules[0].sink_ref, "Sink 1");
        }
        drop(dir);
    }

    #[test]
    fn test_config_override_flag_beats_env() {
        let flag = PathBuf::from("/flag/config.toml");
        let resolved =
            resolve_config_override(Some(&flag), Some(std::ffi::OsStr::new("/env/config.toml")))
                .expect("resolution should succeed");
        assert_eq!(resolved, Some(flag));
    }

    #[test]
    fn test_config_override_env_used_without_flag() {
        let resolved = resolve_config_override(None, Some(std::ffi::OsStr::new("/env/c.toml")))
            .expect("resolution should succeed");
        assert_eq!(resolved, Some(PathBuf::from("/env/c.toml")));
    }

    #[test]
    fn test_config_override_relative_env_is_made_absolute() {
        let resolved = resolve_config_override(None, Some(std::ffi::OsStr::new("rel/c.toml")))
            .expect("resolution should succeed")
            .expect("env override should be used");
        assert!(resolved.is_absolute(), "got {}", resolved.display());
        assert!(resolved.ends_with("rel/c.toml"));
    }

    #[test]
    fn test_config_override_absent_or_empty_env_falls_back_to_default() {
        assert_eq!(resolve_config_override(None, None).unwrap(), None);
        assert_eq!(
            resolve_config_override(None, Some(std::ffi::OsStr::new(""))).unwrap(),
            None
        );
    }

    #[test]
    fn test_absolute_config_path_rejects_empty() {
        assert!(absolute_config_path(Path::new("")).is_err());
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
