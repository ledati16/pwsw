//! Configuration management
//!
//! Handles loading, parsing, validating and saving the TOML configuration file.
//! Supports settings, sink definitions, and window matching rules.
//!
//! Saving is designed so the file can never be corrupted by `pwsw` itself:
//! - every save validates the config and checks that the written text loads back to
//!   the same config,
//! - edits are applied into the document that was loaded (via `toml_edit`), so comments
//!   and formatting of untouched entries survive,
//! - the write is atomic (temp file + rename next to the real file, following symlinks),
//!   keeps a single `.bak` copy, and refuses to clobber changes made on disk since load.

use std::collections::HashSet;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::{fmt, fs};

use color_eyre::eyre::{self, Context, ContextCompat, Result};
use regex::{Regex, RegexBuilder};
use serde::Deserialize;
use serde::de::{self, Deserializer, Visitor};
use toml_edit::{Array, ArrayOfTables, DocumentMut, Item, Table, TableLike, Value};
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
// Rule Regex Compilation
// ============================================================================

/// Upper bound on the compiled size of one rule regex (`regex` crate `size_limit`).
///
/// Rust's `regex` matches in linear time, so there is no catastrophic backtracking to
/// guard against; the only cost a pattern can impose is its compiled size (for example
/// huge counted repetitions such as `\w{100000}`), which this limit caps.
pub const RULE_REGEX_SIZE_LIMIT: usize = 1 << 20;

/// Cache budget of the lazy DFA used when matching one rule regex.
pub const RULE_REGEX_DFA_SIZE_LIMIT: usize = 2 << 20;

/// Largest accepted `switch_delay_ms` (5 s): a longer settle delay would make the
/// daemon look unresponsive
pub const MAX_SWITCH_DELAY_MS: u32 = 5000;

/// Compile a window rule pattern (`app_id`, title, or a `test-rule` pattern).
///
/// This is the single place rule regexes are built: config loading, the TUI rule
/// editor and its live preview, and the daemon's `test-rule` handler all call it, so a
/// pattern accepted in one place is accepted everywhere.
///
/// # Errors
/// Returns the `regex` error if the pattern is invalid or its compiled form exceeds
/// [`RULE_REGEX_SIZE_LIMIT`].
pub fn compile_rule_regex(pattern: &str) -> std::result::Result<Regex, regex::Error> {
    RegexBuilder::new(pattern)
        .size_limit(RULE_REGEX_SIZE_LIMIT)
        .dfa_size_limit(RULE_REGEX_DFA_SIZE_LIMIT)
        .build()
}

// ============================================================================
// Error Types
// ============================================================================

/// The config file does not exist.
///
/// Returned (inside the `eyre` report) by [`Config::load`] and [`Config::load_from_path`];
/// detect it with `report.downcast_ref::<ConfigNotFound>()`.
#[derive(Debug, Clone)]
pub struct ConfigNotFound {
    /// The path that was looked up.
    pub path: PathBuf,
}

impl fmt::Display for ConfigNotFound {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Config file not found: {}\n\nCreate one with:\n  pwsw init   (commented \
             template)\n  pwsw tui    (interactive setup)",
            self.path.display()
        )
    }
}

impl std::error::Error for ConfigNotFound {}

/// The config file changed on disk after it was loaded, so saving would discard
/// those changes.
///
/// Returned by [`Config::save_to`]; [`Config::save_force_to`] skips the check.
#[derive(Debug, Clone)]
pub struct ConfigModifiedOnDisk {
    /// The file that changed.
    pub path: PathBuf,
}

impl fmt::Display for ConfigModifiedOnDisk {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Config file {} was changed on disk after it was loaded; refusing to overwrite \
             those changes (reload the config, or force the save)",
            self.path.display()
        )
    }
}

impl std::error::Error for ConfigModifiedOnDisk {}

// ============================================================================
// Public Configuration Types
// ============================================================================

/// Main configuration structure
///
/// Besides the parsed values, a `Config` loaded from disk remembers the original
/// document (to preserve comments on save) and what the file looked like at load (to
/// detect external edits). Prefer the mutation methods ([`Config::add_sink`],
/// [`Config::update_sink`], ...) over editing `sinks`/`rules` directly: they keep rule
/// references pointing at the same sinks and let saves keep each entry's comments.
#[derive(Debug, Clone)]
pub struct Config {
    pub settings: Settings,
    pub sinks: Vec<SinkConfig>,
    pub rules: Vec<Rule>,
    origin: Origin,
}

impl PartialEq for Config {
    /// Compares the configuration values only (not where they were loaded from).
    fn eq(&self, other: &Self) -> bool {
        self.settings == other.settings && self.sinks == other.sinks && self.rules == other.rules
    }
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
    /// Settle delay for rule-driven switches in milliseconds (`0` = switch at once):
    /// a change of the rules' answer is acted on only if it still holds this long
    pub switch_delay_ms: u32,
}

impl Default for Settings {
    /// The values used for keys that are absent from the config file.
    fn default() -> Self {
        Self {
            default_on_startup: false, // safer for new users with example sink names
            set_smart_toggle: true,
            notify_manual: true,
            notify_rules: true,
            match_by_index: false,
            log_level: "info".to_string(),
            switch_delay_ms: 0,
        }
    }
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
    /// Sink reference: a sink `desc` (what the TUI writes), a node `name`, or a
    /// deprecated 1-based position (`"2"`; an integer `sink = 2` in the file).
    ///
    /// The rule's first choice; see [`Rule::fallback_refs`].
    pub sink_ref: String,
    /// Further sink references to fall back to, in priority order, when the earlier
    /// ones cannot be activated (`sink = ["A", "B"]` in the file; empty for a plain
    /// `sink = "A"`). Same reference forms as [`Rule::sink_ref`].
    pub fallback_refs: Vec<String>,
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
            && self.fallback_refs == other.fallback_refs
            && self.desc == other.desc
            && self.notify == other.notify
    }
}

impl Rule {
    /// The rule's sink references in priority order: [`Rule::sink_ref`], then
    /// [`Rule::fallback_refs`]
    pub fn sink_refs(&self) -> impl Iterator<Item = &str> {
        std::iter::once(self.sink_ref.as_str()).chain(self.fallback_refs.iter().map(String::as_str))
    }

    fn sink_refs_mut(&mut self) -> impl Iterator<Item = &mut String> {
        std::iter::once(&mut self.sink_ref).chain(self.fallback_refs.iter_mut())
    }
}

/// Where a `Config` came from: the loaded document and the on-disk state.
#[derive(Debug, Clone, Default)]
struct Origin {
    /// The document as loaded (`None` for configs built in memory). Saves apply the
    /// current values into a copy of it, so it never changes after load.
    doc: Option<Arc<DocumentMut>>,
    /// For each entry of `Config::sinks`, the index of its `[[sinks]]` table in `doc`.
    sink_slots: Vec<Option<usize>>,
    /// For each entry of `Config::rules`, the index of its `[[rules]]` table in `doc`.
    rule_slots: Vec<Option<usize>>,
    /// What the file looked like when last loaded or saved. Shared between clones so a
    /// save made through one clone (e.g. the TUI's background save) is seen by all.
    disk: Arc<Mutex<Option<DiskSnapshot>>>,
}

#[derive(Debug, Clone)]
struct DiskSnapshot {
    /// Canonical (symlink-resolved) path of the file.
    path: PathBuf,
    /// Exact file contents.
    contents: Vec<u8>,
}

// ============================================================================
// Config File Deserialization (TOML)
// ============================================================================

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfigFile {
    #[serde(default)]
    settings: SettingsFile,
    #[serde(default)]
    sinks: Vec<SinkConfigFile>,
    #[serde(default)]
    rules: Vec<RuleConfigFile>,
}

// TOML file format - mirrors Settings with serde defaults
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
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
    #[serde(default)]
    switch_delay_ms: u32,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SinkConfigFile {
    name: String,
    desc: String,
    #[serde(default)]
    icon: Option<String>,
    #[serde(default)]
    default: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RuleConfigFile {
    app_id: String,
    #[serde(default)]
    title: Option<String>,
    #[serde(deserialize_with = "deserialize_sink_refs")]
    sink: Vec<String>,
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
        let s = Settings::default();
        Self {
            default_on_startup: s.default_on_startup,
            set_smart_toggle: s.set_smart_toggle,
            notify_manual: s.notify_manual,
            notify_rules: s.notify_rules,
            match_by_index: s.match_by_index,
            log_level: s.log_level,
            switch_delay_ms: s.switch_delay_ms,
        }
    }
}

/// What a single sink reference may be in the file
const SINK_REF_EXPECTED: &str = "a sink description, node name, or 1-based position";

/// One sink reference: a string (`desc`/`name`/position) or an integer position.
struct SinkRef(String);

impl<'de> Deserialize<'de> for SinkRef {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        struct SinkRefVisitor;

        impl Visitor<'_> for SinkRefVisitor {
            type Value = SinkRef;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(SINK_REF_EXPECTED)
            }

            fn visit_str<E: de::Error>(self, v: &str) -> std::result::Result<SinkRef, E> {
                Ok(SinkRef(v.to_string()))
            }

            fn visit_string<E: de::Error>(self, v: String) -> std::result::Result<SinkRef, E> {
                Ok(SinkRef(v))
            }

            fn visit_i64<E: de::Error>(self, v: i64) -> std::result::Result<SinkRef, E> {
                Ok(SinkRef(v.to_string()))
            }

            fn visit_u64<E: de::Error>(self, v: u64) -> std::result::Result<SinkRef, E> {
                Ok(SinkRef(v.to_string()))
            }
        }

        deserializer.deserialize_any(SinkRefVisitor)
    }
}

/// Accept a rule's `sink` as one reference or an array of them (in priority order).
///
/// An empty array is accepted here and rejected when the rule is built, so the error
/// names the rule.
fn deserialize_sink_refs<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Vec<String>, D::Error> {
    struct SinkRefsVisitor;

    impl<'de> Visitor<'de> for SinkRefsVisitor {
        type Value = Vec<String>;

        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "{SINK_REF_EXPECTED}, or an array of them")
        }

        fn visit_str<E: de::Error>(self, v: &str) -> std::result::Result<Vec<String>, E> {
            Ok(vec![v.to_string()])
        }

        fn visit_string<E: de::Error>(self, v: String) -> std::result::Result<Vec<String>, E> {
            Ok(vec![v])
        }

        fn visit_i64<E: de::Error>(self, v: i64) -> std::result::Result<Vec<String>, E> {
            Ok(vec![v.to_string()])
        }

        fn visit_u64<E: de::Error>(self, v: u64) -> std::result::Result<Vec<String>, E> {
            Ok(vec![v.to_string()])
        }

        fn visit_seq<A: de::SeqAccess<'de>>(
            self,
            mut seq: A,
        ) -> std::result::Result<Vec<String>, A::Error> {
            let mut refs = Vec::with_capacity(seq.size_hint().unwrap_or(0));
            while let Some(SinkRef(r)) = seq.next_element()? {
                refs.push(r);
            }
            Ok(refs)
        }
    }

    deserializer.deserialize_any(SinkRefsVisitor)
}

/// The 1-based position a sink reference names, if it is positional (`"2"` or `2`).
fn positional_ref(sink_ref: &str) -> Option<usize> {
    sink_ref.parse::<usize>().ok()
}

// ============================================================================
// Default Config Template
// ============================================================================

/// The commented template written by `pwsw init` (and on first `pwsw tui`).
const DEFAULT_CONFIG_TEMPLATE: &str = r#"# PWSW (PipeWire Switcher) Configuration
#
# Run `pwsw tui` for interactive setup, or configure manually below.
# Find available sinks with: pwsw list-sinks
# Comments in this file are kept when the TUI saves changes.

[settings]
default_on_startup = false # Switch to default sink on daemon start
set_smart_toggle = true    # set-sink toggles back to default if already active
notify_manual = true       # Desktop notifications for manual switches
notify_rules = true        # Desktop notifications for rule-triggered switches
match_by_index = false     # Priority: false = recent window, true = rule order
log_level = "info"         # error, warn, info, debug, trace
switch_delay_ms = 0        # Wait this long (ms) for the rules' answer to settle before switching

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
# sink = "HDMI"                 # The sink's desc (or its node name)
# desc = "Steam Gaming"         # Optional: custom notification text
#
# [[rules]]
# app_id = "^discord$"
# sink = ["Headphones", "Speakers"]  # Fallbacks: the first one available now wins
"#;

// ============================================================================
// Config Implementation
// ============================================================================

impl Config {
    /// Build a config in memory (not tied to any file).
    ///
    /// Saving it generates the file from scratch. No validation happens here; it runs
    /// on save and in every mutation method.
    #[must_use]
    pub fn new(settings: Settings, sinks: Vec<SinkConfig>, rules: Vec<Rule>) -> Self {
        let origin = Origin {
            sink_slots: vec![None; sinks.len()],
            rule_slots: vec![None; rules.len()],
            ..Origin::default()
        };
        Self {
            settings,
            sinks,
            rules,
            origin,
        }
    }

    // ------------------------------------------------------------------------
    // Loading
    // ------------------------------------------------------------------------

    /// Load configuration from the active config path (see [`Config::get_config_path`]).
    ///
    /// Never creates anything on disk.
    ///
    /// # Errors
    /// Returns [`ConfigNotFound`] (inside the report) if the file does not exist, or an
    /// error if the config path cannot be determined or the file cannot be read, parsed,
    /// or is invalid.
    pub fn load() -> Result<Self> {
        let config_path = Self::get_config_path()?;
        Self::load_from_path(&config_path)
    }

    /// Load the config, first writing the commented default template if no file
    /// exists yet. Only interactive setup (`pwsw tui`) should use this.
    ///
    /// # Errors
    /// Returns an error if the template cannot be written, or if loading fails as in
    /// [`Config::load`].
    pub fn load_or_init() -> Result<Self> {
        let config_path = Self::get_config_path()?;
        match Self::load_from_path(&config_path) {
            Err(e) if e.downcast_ref::<ConfigNotFound>().is_some() => {
                info!("Creating default config at {}", config_path.display());
                Self::init_at(&config_path, false)?;
                Self::print_init_guidance(&config_path);
                Self::load_from_path(&config_path)
            }
            other => other,
        }
    }

    /// Load configuration from a specific path. Useful for tests to avoid relying on XDG env.
    ///
    /// Logs one deprecation warning if any rule references a sink by position.
    ///
    /// # Errors
    /// Returns [`ConfigNotFound`] (inside the report) if the file does not exist, or an
    /// error if it cannot be read, parsed (including unknown keys), or validated.
    pub fn load_from_path<P: AsRef<Path>>(path: P) -> Result<Self> {
        let path = path.as_ref();
        let contents = match fs::read_to_string(path) {
            Ok(contents) => contents,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(eyre::Report::new(ConfigNotFound {
                    path: path.to_path_buf(),
                }));
            }
            Err(e) => {
                return Err(e)
                    .with_context(|| format!("Failed to read config: {}", path.display()));
            }
        };

        let config = Self::parse_str(&contents, path)?;
        config.warn_positional_refs();

        let canonical = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        *config
            .origin
            .disk
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(DiskSnapshot {
            path: canonical,
            contents: contents.into_bytes(),
        });
        Ok(config)
    }

    /// Parse and validate config text. `path` is only used in error messages.
    fn parse_str(contents: &str, path: &Path) -> Result<Self> {
        let parse_error = |e: &dyn fmt::Display| {
            eyre::eyre!(
                "Failed to parse config: {path_display}\n{e}",
                path_display = path.display()
            )
        };
        // Parsed once: the spanned document feeds serde (errors quote line and
        // column) and becomes the editable document that saves write back into
        let parsed =
            toml_edit::Document::parse(contents.to_owned()).map_err(|e| parse_error(&e))?;
        let config_file: ConfigFile =
            toml_edit::de::from_document(parsed.clone()).map_err(|e| parse_error(&e))?;
        let doc: DocumentMut = parsed.into_mut();

        let mut config = Self::from_config_file(config_file)
            .wrap_err_with(|| format!("Invalid config: {}", path.display()))?;
        config.origin.sink_slots = (0..config.sinks.len()).map(Some).collect();
        config.origin.rule_slots = (0..config.rules.len()).map(Some).collect();
        config.origin.doc = Some(Arc::new(doc));
        Ok(config)
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
            switch_delay_ms: config_file.settings.switch_delay_ms,
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
        for (i, r) in config_file.rules.into_iter().enumerate() {
            let app_id_regex = compile_rule_regex(&r.app_id)
                .map_err(|e| eyre::eyre!("Rule {}: invalid app_id regex: {e}", i + 1))?;
            let title_regex = r
                .title
                .as_deref()
                .map(compile_rule_regex)
                .transpose()
                .map_err(|e| eyre::eyre!("Rule {}: invalid title regex: {e}", i + 1))?;
            let mut refs = r.sink.into_iter();
            let sink_ref = refs.next().ok_or_else(|| {
                eyre::eyre!(
                    "Rule {}: `sink` is an empty list; name at least one sink",
                    i + 1
                )
            })?;
            rules.push(Rule {
                app_id_regex,
                title_regex,
                sink_ref,
                fallback_refs: refs.collect(),
                desc: r.desc,
                notify: r.notify,
                app_id_pattern: r.app_id,
                title_pattern: r.title,
            });
        }

        let config = Self::new(settings, sinks, rules);
        config.validate()?;
        Ok(config)
    }

    /// Log a single deprecation warning for rules that reference sinks by position.
    fn warn_positional_refs(&self) {
        let positional: Vec<String> = self
            .rules
            .iter()
            .enumerate()
            .filter(|(_, r)| {
                r.sink_refs()
                    .any(|sink_ref| positional_ref(sink_ref).is_some())
            })
            .map(|(i, _)| (i + 1).to_string())
            .collect();
        if !positional.is_empty() {
            warn!(
                "Rule(s) {} reference a sink by position; positional references are deprecated \
                 (they silently retarget when sinks are reordered). Use the sink's desc instead.",
                positional.join(", ")
            );
        }
    }

    // ------------------------------------------------------------------------
    // Creating the default config
    // ------------------------------------------------------------------------

    /// Write the commented default template to the active config path.
    ///
    /// Returns the path written.
    ///
    /// # Errors
    /// Returns an error if a config already exists there and `force` is false, or if
    /// the file cannot be written.
    pub fn init(force: bool) -> Result<PathBuf> {
        let config_path = Self::get_config_path()?;
        Self::init_at(&config_path, force)?;
        Ok(config_path)
    }

    /// Write the commented default template to `path` (atomically, mode `0600`).
    ///
    /// With `force`, an existing file is replaced and its old contents kept as
    /// `<name>.bak`.
    ///
    /// # Errors
    /// Returns an error if `path` exists and `force` is false, or if the file cannot be
    /// written.
    pub fn init_at(path: &Path, force: bool) -> Result<()> {
        if !force && fs::symlink_metadata(path).is_ok() {
            eyre::bail!(
                "Config already exists: {}\nUse `pwsw init --force` to replace it (the old file \
                 is kept as {}).",
                path.display(),
                backup_path(path).display()
            );
        }
        let target = resolve_write_target(path)?;
        Self::write_atomic(&target, DEFAULT_CONFIG_TEMPLATE, path)
    }

    /// Print the "created default config / next steps" message to stderr.
    pub fn print_init_guidance(path: &Path) {
        crate::errln!(
            "{} {}",
            "✓".success().to_stderr(),
            format!("Created default config at: {}", path.display())
                .success()
                .to_stderr()
        );
        crate::errln!();
        crate::errln!("{}", "Next steps:".header().to_stderr());
        crate::errln!(
            "  Run {} for interactive setup (recommended)",
            "pwsw tui".technical().to_stderr()
        );
        crate::errln!();
        crate::errln!("  Or configure manually:");
        crate::errln!(
            "    {}  - discover audio outputs",
            "pwsw list-sinks".technical().to_stderr()
        );
        crate::errln!("    Edit config file - add sinks and rules");
        crate::errln!(
            "    {}    - check config",
            "pwsw validate".technical().to_stderr()
        );
        crate::errln!(
            "    {}      - start the daemon",
            "pwsw daemon".technical().to_stderr()
        );
        crate::errln!();
    }

    // ------------------------------------------------------------------------
    // Saving
    // ------------------------------------------------------------------------

    /// Save configuration to `path` (normally [`Config::get_config_path`])
    ///
    /// # Errors
    /// Returns an error if the config is invalid, if the file changed on disk since it
    /// was loaded or last saved by this config ([`ConfigModifiedOnDisk`]), or if it
    /// cannot be written.
    pub fn save_to<P: AsRef<Path>>(&self, path: P) -> Result<()> {
        self.save_impl(path.as_ref(), false)
    }

    /// Like [`Config::save_to`], but overwrite the file even if it changed on disk since
    /// it was loaded.
    ///
    /// # Errors
    /// Returns an error if the config is invalid or cannot be written.
    pub fn save_force_to<P: AsRef<Path>>(&self, path: P) -> Result<()> {
        self.save_impl(path.as_ref(), true)
    }

    fn save_impl(&self, path: &Path, force: bool) -> Result<()> {
        self.validate()?;
        self.validate_patterns()?;

        let contents = self.render();

        // Defensive: never write text that would not load back to this exact config.
        let reparsed = Self::parse_str(&contents, path)
            .wrap_err("Internal error: generated config does not load; refusing to save")?;
        if reparsed != *self {
            eyre::bail!("Internal error: generated config does not round-trip; refusing to save");
        }

        let target = resolve_write_target(path)?;

        let mut disk = self
            .origin
            .disk
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if !force
            && let Some(snapshot) = disk.as_ref()
            && snapshot.path == target
        {
            match fs::read(&target) {
                Ok(current) if current != snapshot.contents => {
                    return Err(eyre::Report::new(ConfigModifiedOnDisk {
                        path: path.to_path_buf(),
                    }));
                }
                // Unchanged, or deleted (nothing to clobber)
                Ok(_) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => {
                    return Err(e)
                        .with_context(|| format!("Failed to read config: {}", target.display()));
                }
            }
        }

        Self::write_atomic(&target, &contents, path)?;

        *disk = Some(DiskSnapshot {
            path: fs::canonicalize(&target).unwrap_or(target),
            contents: contents.into_bytes(),
        });
        Ok(())
    }

    /// Atomically replace `target` with `contents` (mode `0600`).
    ///
    /// `requested` is the path the caller asked for (may be a symlink to `target`); the
    /// previous file content is copied to `<requested>.bak` first.
    fn write_atomic(target: &Path, contents: &str, requested: &Path) -> Result<()> {
        let dir = target
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));

        // Ensure the target directory exists (tests may create temp dirs but callers
        // can race or remove them); create_dir_all is idempotent.
        fs::create_dir_all(dir)
            .with_context(|| format!("Failed to create config dir: {}", dir.display()))?;

        let mut tmp = Self::write_temp_file_with_contents(dir, contents)?;

        Self::ensure_not_empty_overwrite(&mut tmp, target)?;

        // Ensure user-only permissions on Unix for the temp file
        Self::ensure_unix_permissions(tmp.path())?;

        Self::backup_existing(requested);

        // Persist atomically
        tmp.persist(target).with_context(|| {
            format!(
                "Failed to persist temporary config file to {}",
                target.display()
            )
        })?;

        // Ensure final permissions as well
        Self::ensure_unix_permissions(target)?;

        // Sync parent directory to ensure rename is durable
        #[cfg(unix)]
        if let Ok(dir_file) = std::fs::File::open(dir) {
            dir_file
                .sync_all()
                .with_context(|| format!("Failed to sync directory: {}", dir.display()))?;
        }

        Ok(())
    }

    fn write_temp_file_with_contents(
        dir: &Path,
        contents: &str,
    ) -> Result<tempfile::NamedTempFile> {
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
        config_path: &Path,
    ) -> Result<()> {
        let written_len = tmp
            .as_file_mut()
            .metadata()
            .context("Failed to stat temporary config file")?
            .len();
        if written_len == 0
            && let Ok(meta) = fs::metadata(config_path)
            && meta.is_file()
            && meta.len() > 0
        {
            eyre::bail!("Refusing to overwrite non-empty config with empty data");
        }
        Ok(())
    }

    fn ensure_unix_permissions(path: &Path) -> Result<()> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
                .with_context(|| format!("Failed to set permissions: {}", path.display()))?;
        }
        Ok(())
    }

    /// Keep a single `<name>.bak` copy of the file at `path` before it is overwritten.
    ///
    /// Only one backup is kept (overwrites previous) since atomic writes already prevent
    /// corruption - this is just for user recovery ("undo"). Backup failures are logged,
    /// never fatal.
    fn backup_existing(path: &Path) {
        if let Ok(metadata) = fs::metadata(path)
            && metadata.is_file()
        {
            let bak_path = backup_path(path);
            if let Err(e) = fs::copy(path, &bak_path) {
                warn!(
                    "Failed to create config backup at {}: {}",
                    bak_path.display(),
                    e
                );
            }
        }
    }

    #[cfg(test)]
    /// Test helper to call the private atomic write with a raw string
    pub(crate) fn save_str_for_test(contents: &str, path: &Path) -> Result<()> {
        Self::write_atomic(path, contents, path)
    }

    // ------------------------------------------------------------------------
    // Rendering (comment-preserving)
    // ------------------------------------------------------------------------

    /// Render the config as TOML, applying the current values into the loaded document
    /// (or into an empty one for in-memory configs).
    ///
    /// Formatting is kept except for a UTF-8 byte-order mark, which the parser drops
    /// (and the file is written without), and the array placement noted on
    /// [`rebuild_array`].
    fn render(&self) -> String {
        let fresh = self.origin.doc.is_none();
        let mut doc = self
            .origin
            .doc
            .as_deref()
            .cloned()
            .unwrap_or_else(DocumentMut::new);
        let (sink_slots, rule_slots) = self.effective_slots();

        apply_settings(&mut doc, &self.settings, fresh);
        rebuild_array(&mut doc, "sinks", &self.sinks, &sink_slots, apply_sink);
        rebuild_array(&mut doc, "rules", &self.rules, &rule_slots, apply_rule);
        doc.to_string()
    }

    /// The document slot of every sink and rule.
    ///
    /// If `sinks`/`rules` were resized directly (bypassing the mutation methods), the
    /// recorded slots no longer line up; entries are then matched to document tables by
    /// node name (sinks) or patterns (rules), and unmatched ones are treated as new.
    fn effective_slots(&self) -> (Vec<Option<usize>>, Vec<Option<usize>>) {
        let doc = self.origin.doc.as_deref();
        let sink_slots = if self.origin.sink_slots.len() == self.sinks.len() {
            self.origin.sink_slots.clone()
        } else {
            let keys = doc_table_keys(doc, "sinks", |t| {
                t.get("name").and_then(Item::as_str).map(str::to_string)
            });
            match_slots(&keys, self.sinks.iter().map(|s| s.name.clone()))
        };
        let rule_slots = if self.origin.rule_slots.len() == self.rules.len() {
            self.origin.rule_slots.clone()
        } else {
            let keys = doc_table_keys(doc, "rules", |t| {
                let app_id = t.get("app_id").and_then(Item::as_str)?;
                let title = t.get("title").and_then(Item::as_str).unwrap_or_default();
                Some(rule_key(app_id, title))
            });
            match_slots(
                &keys,
                self.rules.iter().map(|r| {
                    rule_key(
                        &r.app_id_pattern,
                        r.title_pattern.as_deref().unwrap_or_default(),
                    )
                }),
            )
        };
        (sink_slots, rule_slots)
    }

    // ------------------------------------------------------------------------
    // Invariant-keeping mutations
    // ------------------------------------------------------------------------

    /// Apply `f` to a copy of the config, validate it, and only then commit.
    fn transact(&mut self, f: impl FnOnce(&mut Self) -> Result<()>) -> Result<()> {
        let mut next = self.clone();
        let (sink_slots, rule_slots) = next.effective_slots();
        next.origin.sink_slots = sink_slots;
        next.origin.rule_slots = rule_slots;
        f(&mut next)?;
        next.validate()?;
        *self = next;
        Ok(())
    }

    /// Run a change to the sinks list while keeping positional rule references pointing
    /// at the same sink: any positional reference whose sink moved is rewritten to that
    /// sink's `desc`.
    fn keeping_positional_targets(&mut self, f: impl FnOnce(&mut Self)) {
        // Every reference of every rule (fallbacks too): its target if positional
        let targets: Vec<Option<(usize, String)>> = self
            .rules
            .iter()
            .flat_map(Rule::sink_refs)
            .map(|sink_ref| {
                let pos = positional_ref(sink_ref)?;
                let sink = self.sinks.get(pos.checked_sub(1)?)?;
                Some((pos - 1, sink.name.clone()))
            })
            .collect();
        f(self);
        let sinks = &self.sinks;
        let refs = self.rules.iter_mut().flat_map(Rule::sink_refs_mut);
        for (sink_ref, target) in refs.zip(targets) {
            if let Some((old_index, name)) = target
                && let Some(new_index) = sinks.iter().position(|s| s.name == name)
                && new_index != old_index
            {
                sink_ref.clone_from(&sinks[new_index].desc);
            }
        }
    }

    fn check_sink_unique(&self, sink: &SinkConfig, skip: Option<usize>) -> Result<()> {
        for (i, other) in self.sinks.iter().enumerate() {
            if Some(i) == skip {
                continue;
            }
            if other.name == sink.name {
                eyre::bail!(
                    "Sink {} ('{}') already uses node name '{}'",
                    i + 1,
                    other.desc,
                    sink.name
                );
            }
            if other.desc == sink.desc {
                eyre::bail!("Sink {} already uses description '{}'", i + 1, sink.desc);
            }
        }
        Ok(())
    }

    /// Append a sink.
    ///
    /// The first sink always becomes the default; a new default sink un-defaults the
    /// others.
    ///
    /// # Errors
    /// Returns an error (and changes nothing) if another sink has the same node name or
    /// description, or the result would be invalid (e.g. empty name).
    pub fn add_sink(&mut self, mut sink: SinkConfig) -> Result<()> {
        self.transact(|c| {
            c.check_sink_unique(&sink, None)?;
            if c.sinks.is_empty() {
                sink.default = true;
            }
            if sink.default {
                for s in &mut c.sinks {
                    s.default = false;
                }
            }
            c.sinks.push(sink);
            c.origin.sink_slots.push(None);
            Ok(())
        })
    }

    /// Replace the sink at `index` (0-based).
    ///
    /// Rules that referenced the old `desc` or node name are updated to the new one. If
    /// the sink becomes the default, the others are un-defaulted; if that would leave no
    /// default, the first sink becomes the default.
    ///
    /// # Errors
    /// Returns an error (and changes nothing) if `index` is out of range, another sink
    /// has the same node name or description, or the result would be invalid.
    pub fn update_sink(&mut self, index: usize, sink: SinkConfig) -> Result<()> {
        self.transact(|c| {
            let old = c
                .sinks
                .get(index)
                .cloned()
                .with_context(|| format!("No sink at position {}", index + 1))?;
            c.check_sink_unique(&sink, Some(index))?;
            for sink_ref in c.rules.iter_mut().flat_map(Rule::sink_refs_mut) {
                if *sink_ref == old.desc {
                    sink_ref.clone_from(&sink.desc);
                } else if *sink_ref == old.name {
                    sink_ref.clone_from(&sink.name);
                }
            }
            if sink.default {
                for s in &mut c.sinks {
                    s.default = false;
                }
            }
            c.sinks[index] = sink;
            if !c.sinks.iter().any(|s| s.default) {
                c.sinks[0].default = true;
            }
            Ok(())
        })
    }

    /// Make the sink at `index` (0-based) the only default sink.
    ///
    /// # Errors
    /// Returns an error if `index` is out of range.
    pub fn set_default_sink(&mut self, index: usize) -> Result<()> {
        self.transact(|c| {
            if index >= c.sinks.len() {
                eyre::bail!("No sink at position {}", index + 1);
            }
            for (i, s) in c.sinks.iter_mut().enumerate() {
                s.default = i == index;
            }
            Ok(())
        })
    }

    /// Remove the sink at `index` (0-based).
    ///
    /// If it was the default, the first remaining sink becomes the default. Positional
    /// references to sinks after it are rewritten so they keep their target.
    ///
    /// # Errors
    /// Returns an error (and changes nothing) if `index` is out of range or any rule
    /// references the sink (the message names those rules).
    pub fn remove_sink(&mut self, index: usize) -> Result<()> {
        self.transact(|c| {
            let sink = c
                .sinks
                .get(index)
                .cloned()
                .with_context(|| format!("No sink at position {}", index + 1))?;
            let users: Vec<String> = c
                .rules
                .iter()
                .enumerate()
                .filter(|(_, r)| {
                    r.sink_refs()
                        .any(|sink_ref| c.resolve_sink_index(sink_ref) == Some(index))
                })
                .map(|(i, r)| format!("{} ({})", i + 1, r.app_id_pattern))
                .collect();
            if !users.is_empty() {
                eyre::bail!(
                    "Cannot remove sink '{}': used by rule {}. Retarget or delete those rules \
                     first.",
                    sink.desc,
                    users.join(", ")
                );
            }
            c.keeping_positional_targets(|c| {
                c.sinks.remove(index);
                c.origin.sink_slots.remove(index);
            });
            if sink.default
                && let Some(first) = c.sinks.first_mut()
            {
                first.default = true;
            }
            Ok(())
        })
    }

    /// Move the sink at `from` to position `to` (both 0-based).
    ///
    /// Positional rule references are rewritten so every rule still targets the same
    /// sink.
    ///
    /// # Errors
    /// Returns an error if either index is out of range.
    pub fn move_sink(&mut self, from: usize, to: usize) -> Result<()> {
        self.transact(|c| {
            if from >= c.sinks.len() || to >= c.sinks.len() {
                eyre::bail!("Sink position out of range");
            }
            c.keeping_positional_targets(|c| {
                let sink = c.sinks.remove(from);
                c.sinks.insert(to, sink);
                let slot = c.origin.sink_slots.remove(from);
                c.origin.sink_slots.insert(to, slot);
            });
            Ok(())
        })
    }

    /// Rewrite each of a rule's sink references to the target sink's `desc`.
    ///
    /// A fallback naming the same sink as the first choice is dropped: editing only
    /// the first choice (as the TUI does) to a sink that was a fallback moves that sink
    /// to the front. Other duplicates are left for validation to reject.
    fn canonicalize_rule_refs(&self, rule: &mut Rule) -> Result<()> {
        for sink_ref in rule.sink_refs_mut() {
            let sink = self
                .resolve_sink(sink_ref)
                .with_context(|| format!("Rule references unknown sink '{sink_ref}'"))?;
            sink_ref.clone_from(&sink.desc);
        }
        let first = rule.sink_ref.clone();
        rule.fallback_refs.retain(|r| *r != first);
        Ok(())
    }

    /// Append a rule. Its sink references are stored as the target sinks' `desc`.
    ///
    /// # Errors
    /// Returns an error (and changes nothing) if a sink reference does not resolve or
    /// the rule is invalid (e.g. empty `app_id` pattern, a sink listed twice).
    pub fn add_rule(&mut self, mut rule: Rule) -> Result<()> {
        self.transact(|c| {
            c.canonicalize_rule_refs(&mut rule)?;
            c.rules.push(rule);
            c.origin.rule_slots.push(None);
            Ok(())
        })
    }

    /// Replace the rule at `index` (0-based). Its sink references are stored as the
    /// target sinks' `desc` (see [`Rule::fallback_refs`]; a caller that edits only the
    /// first choice passes the old rule's fallbacks along).
    ///
    /// # Errors
    /// Returns an error (and changes nothing) if `index` is out of range, a sink
    /// reference does not resolve, or the rule is invalid.
    pub fn update_rule(&mut self, index: usize, mut rule: Rule) -> Result<()> {
        self.transact(|c| {
            if index >= c.rules.len() {
                eyre::bail!("No rule at position {}", index + 1);
            }
            c.canonicalize_rule_refs(&mut rule)?;
            c.rules[index] = rule;
            Ok(())
        })
    }

    /// Remove the rule at `index` (0-based).
    ///
    /// # Errors
    /// Returns an error if `index` is out of range.
    pub fn remove_rule(&mut self, index: usize) -> Result<()> {
        self.transact(|c| {
            if index >= c.rules.len() {
                eyre::bail!("No rule at position {}", index + 1);
            }
            c.rules.remove(index);
            c.origin.rule_slots.remove(index);
            Ok(())
        })
    }

    /// Move the rule at `from` to position `to` (both 0-based).
    ///
    /// # Errors
    /// Returns an error if either index is out of range.
    pub fn move_rule(&mut self, from: usize, to: usize) -> Result<()> {
        self.transact(|c| {
            if from >= c.rules.len() || to >= c.rules.len() {
                eyre::bail!("Rule position out of range");
            }
            let rule = c.rules.remove(from);
            c.rules.insert(to, rule);
            let slot = c.origin.rule_slots.remove(from);
            c.origin.rule_slots.insert(to, slot);
            Ok(())
        })
    }

    // ------------------------------------------------------------------------
    // Validation
    // ------------------------------------------------------------------------

    fn validate(&self) -> Result<()> {
        // Validate log level
        match self.settings.log_level.as_str() {
            "error" | "warn" | "info" | "debug" | "trace" => {}
            level => eyre::bail!(
                "Invalid log_level '{level}'. Must be: error, warn, info, debug, or trace"
            ),
        }
        if self.settings.switch_delay_ms > MAX_SWITCH_DELAY_MS {
            eyre::bail!(
                "Invalid switch_delay_ms {}. Must be between 0 and {MAX_SWITCH_DELAY_MS} \
                 (milliseconds)",
                self.settings.switch_delay_ms
            );
        }

        for (i, sink) in self.sinks.iter().enumerate() {
            if sink.name.trim().is_empty() {
                eyre::bail!("Sink {} has an empty name", i + 1);
            }
            if sink.desc.trim().is_empty() {
                eyre::bail!("Sink {} has an empty desc", i + 1);
            }
            // A numeric desc would be read as a position by rule references
            if sink.desc.trim().parse::<i128>().is_ok() {
                eyre::bail!(
                    "Sink {} desc '{}' is a number; it would be read as a sink position. Use a \
                     descriptive name.",
                    i + 1,
                    sink.desc
                );
            }
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
        }

        // A string that is one sink's name and another's desc makes references ambiguous
        for (i, a) in self.sinks.iter().enumerate() {
            if let Some(j) = self
                .sinks
                .iter()
                .enumerate()
                .position(|(j, b)| j != i && b.desc == a.name)
            {
                eyre::bail!(
                    "'{}' is the name of sink {} and the desc of sink {}; rule references to it \
                     would be ambiguous",
                    a.name,
                    i + 1,
                    j + 1
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

            let mut targets = Vec::with_capacity(1 + rule.fallback_refs.len());
            for sink_ref in rule.sink_refs() {
                if sink_ref.trim().is_empty() {
                    eyre::bail!("Rule {} has an empty sink reference", i + 1);
                }

                let Some(target) = self.resolve_sink_index(sink_ref) else {
                    let available: Vec<_> = self
                        .sinks
                        .iter()
                        .enumerate()
                        .map(|(idx, s)| format!("{}. '{}'", idx + 1, s.desc))
                        .collect();
                    eyre::bail!(
                        "Rule {} references unknown sink '{}'. Available: [{}]",
                        i + 1,
                        sink_ref,
                        available.join(", ")
                    );
                };
                if targets.contains(&target) {
                    eyre::bail!(
                        "Rule {} lists sink '{}' more than once",
                        i + 1,
                        self.sinks[target].desc
                    );
                }
                targets.push(target);
            }
        }

        Ok(())
    }

    /// Check that every rule pattern compiles exactly as config loading would compile it.
    fn validate_patterns(&self) -> Result<()> {
        for (i, rule) in self.rules.iter().enumerate() {
            compile_rule_regex(&rule.app_id_pattern)
                .map_err(|e| eyre::eyre!("Rule {}: invalid app_id regex: {e}", i + 1))?;
            if let Some(title) = &rule.title_pattern {
                compile_rule_regex(title)
                    .map_err(|e| eyre::eyre!("Rule {}: invalid title regex: {e}", i + 1))?;
            }
        }
        Ok(())
    }

    // ------------------------------------------------------------------------
    // Paths
    // ------------------------------------------------------------------------

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

    // ------------------------------------------------------------------------
    // Queries
    // ------------------------------------------------------------------------

    /// Print a human-readable summary of the configuration
    pub fn print_summary(&self) {
        crate::outln!("{} {}\n", "✓".success(), "Configuration valid".success());

        crate::outln!("{}", "Settings:".header());
        crate::outln!(
            "  {}: {}",
            "default_on_startup".dim(),
            self.settings.default_on_startup
        );
        crate::outln!(
            "  {}: {}",
            "set_smart_toggle".dim(),
            self.settings.set_smart_toggle
        );
        crate::outln!(
            "  {}: {}",
            "notify_manual".dim(),
            self.settings.notify_manual
        );
        crate::outln!("  {}: {}", "notify_rules".dim(), self.settings.notify_rules);
        crate::outln!(
            "  {}: {}",
            "match_by_index".dim(),
            self.settings.match_by_index
        );
        crate::outln!(
            "  {}: {}",
            "log_level".dim(),
            self.settings.log_level.as_str().technical()
        );
        crate::outln!(
            "  {}: {}",
            "switch_delay_ms".dim(),
            self.settings.switch_delay_ms
        );

        crate::outln!(
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
            crate::outln!(
                "  {}. {}{}",
                (i + 1).to_string().dim(),
                sink.desc.as_str().bold(),
                marker
            );
            crate::outln!("     {}: {}", "name".dim(), sink.name);
            if let Some(ref icon) = sink.icon {
                crate::outln!("     {}: {}", "icon".dim(), icon.as_str().technical());
            }
        }

        if self.rules.is_empty() {
            crate::outln!("\n{}", "No rules configured.".dim());
        } else {
            crate::outln!(
                "\n{} ({}):",
                "Rules".header(),
                self.rules.len().to_string().technical()
            );
            for (i, rule) in self.rules.iter().enumerate() {
                crate::outln!(
                    "  {}. {}: {}",
                    (i + 1).to_string().dim(),
                    "app_id".dim(),
                    rule.app_id_pattern.as_str().technical()
                );
                if let Some(ref title) = rule.title_pattern {
                    crate::outln!("     {}: {}", "title".dim(), title.as_str().technical());
                }
                let effective_notify = rule.notify.unwrap_or(self.settings.notify_rules);
                let source = if rule.notify.is_some() {
                    "override"
                } else {
                    "default"
                };
                crate::outln!(
                    "     {}: {} ({}: {} - {})",
                    "sink".dim(),
                    rule.sink_ref.as_str().bold(),
                    "notify".dim(),
                    effective_notify,
                    source.dim()
                );
                if !rule.fallback_refs.is_empty() {
                    crate::outln!(
                        "     {}: {}",
                        "fallback".dim(),
                        rule.fallback_refs.join(", ").as_str().bold()
                    );
                }
            }
        }

        if let Ok(path) = Self::get_config_path() {
            crate::outln!("\n{} {}", "Config:".dim(), path.display());
        }
    }

    /// Resolve a sink reference (by position, desc, or name) to its 0-based index.
    pub(crate) fn resolve_sink_index(&self, sink_ref: &str) -> Option<usize> {
        // Try position first (1-indexed)
        if let Some(pos) = positional_ref(sink_ref) {
            return (pos > 0 && pos <= self.sinks.len()).then(|| pos - 1);
        }
        // Then try desc or name
        self.sinks
            .iter()
            .position(|s| s.desc == sink_ref || s.name == sink_ref)
    }

    /// Resolve every sink reference of `rule`, in priority order, to 0-based sink
    /// indices; `None` if any does not resolve
    pub(crate) fn resolve_rule_sinks(&self, rule: &Rule) -> Option<Vec<usize>> {
        rule.sink_refs()
            .map(|sink_ref| self.resolve_sink_index(sink_ref))
            .collect()
    }

    /// Resolve a sink reference (by position, desc, or name)
    #[must_use]
    pub fn resolve_sink(&self, sink_ref: &str) -> Option<&SinkConfig> {
        self.resolve_sink_index(sink_ref).map(|i| &self.sinks[i])
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

// ============================================================================
// File helpers
// ============================================================================

/// `<path>.bak` (e.g. `config.toml.bak`), next to `path`.
fn backup_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".bak");
    path.with_file_name(name)
}

/// The file a save to `path` must replace: the symlink-resolved path when it exists
/// (so a symlinked config stays a symlink), else `path` itself.
fn resolve_write_target(path: &Path) -> Result<PathBuf> {
    match fs::symlink_metadata(path) {
        Ok(_) => fs::canonicalize(path).with_context(|| {
            format!(
                "Cannot resolve config path {} (dangling symlink?)",
                path.display()
            )
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(path.to_path_buf()),
        Err(e) => Err(e).with_context(|| format!("Failed to stat config: {}", path.display())),
    }
}

// ============================================================================
// Document editing helpers (`toml_edit`)
// ============================================================================

fn rule_key(app_id: &str, title: &str) -> String {
    format!("{app_id}\u{0}{title}")
}

/// A matching key for every table of the array `key` in `doc`.
fn doc_table_keys(
    doc: Option<&DocumentMut>,
    key: &str,
    extract: impl Fn(&Table) -> Option<String>,
) -> Vec<Option<String>> {
    doc.and_then(|d| d.get(key))
        .and_then(Item::as_array_of_tables)
        .map_or_default(|a| a.iter().map(&extract).collect())
}

/// Match items to document tables by key; each table is used at most once.
fn match_slots(
    doc_keys: &[Option<String>],
    items: impl Iterator<Item = String>,
) -> Vec<Option<usize>> {
    let mut used = vec![false; doc_keys.len()];
    items
        .map(|key| {
            let slot = doc_keys
                .iter()
                .enumerate()
                .position(|(i, k)| !used[i] && k.as_deref() == Some(key.as_str()));
            if let Some(i) = slot {
                used[i] = true;
            }
            slot
        })
        .collect()
}

fn same_value(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::String(x), Value::String(y)) => x.value() == y.value(),
        (Value::Boolean(x), Value::Boolean(y)) => x.value() == y.value(),
        (Value::Integer(x), Value::Integer(y)) => x.value() == y.value(),
        _ => false,
    }
}

/// Set `key` to `value`, leaving it untouched if it already holds that value and
/// keeping the old value's surrounding whitespace and trailing comment otherwise.
fn set_value<T: TableLike + ?Sized>(table: &mut T, key: &str, value: impl Into<Value>) {
    let value = value.into();
    if let Some(Item::Value(old)) = table.get_mut(key) {
        if same_value(old, &value) {
            return;
        }
        let decor = old.decor().clone();
        *old = value;
        *old.decor_mut() = decor;
    } else {
        table.insert(key, Item::Value(value));
    }
}

/// Set `key` to `value`, or remove it for `None`.
fn set_optional<T: TableLike + ?Sized>(table: &mut T, key: &str, value: Option<&str>) {
    match value {
        Some(v) => set_value(table, key, v),
        None => {
            table.remove(key);
        }
    }
}

/// Write a setting, omitting keys that are absent and still at their default.
fn sync_setting<T: TableLike + ?Sized>(
    table: &mut T,
    key: &str,
    value: impl Into<Value>,
    is_default: bool,
    write_all: bool,
) {
    if !write_all && is_default && table.get(key).is_none() {
        return;
    }
    set_value(table, key, value);
}

fn apply_settings(doc: &mut DocumentMut, s: &Settings, write_all: bool) {
    let d = Settings::default();
    if doc.get("settings").is_none() {
        if !write_all && *s == d {
            return;
        }
        // Place a new [settings] table before every other table
        let first = doc
            .iter()
            .flat_map(|(_, item)| match item {
                Item::Table(t) => vec![t.position()],
                Item::ArrayOfTables(a) => a.iter().map(Table::position).collect(),
                _ => Vec::new(),
            })
            .flatten()
            .min();
        let mut table = Table::new();
        table.set_position(first.map(|p| p - 1));
        doc.insert("settings", Item::Table(table));
        if let Some(first) = first {
            blank_line_before_table_at(doc, first);
        }
    }
    let Some(table) = doc.get_mut("settings").and_then(Item::as_table_like_mut) else {
        return;
    };
    let w = write_all;
    let b = |value: bool, default: bool| (value, value == default);
    for (key, (value, is_default)) in [
        (
            "default_on_startup",
            b(s.default_on_startup, d.default_on_startup),
        ),
        (
            "set_smart_toggle",
            b(s.set_smart_toggle, d.set_smart_toggle),
        ),
        ("notify_manual", b(s.notify_manual, d.notify_manual)),
        ("notify_rules", b(s.notify_rules, d.notify_rules)),
        ("match_by_index", b(s.match_by_index, d.match_by_index)),
    ] {
        sync_setting(table, key, value, is_default, w);
    }
    sync_setting(
        table,
        "log_level",
        s.log_level.as_str(),
        s.log_level == d.log_level,
        w,
    );
    sync_setting(
        table,
        "switch_delay_ms",
        i64::from(s.switch_delay_ms),
        s.switch_delay_ms == d.switch_delay_ms,
        w,
    );
}

/// Start the table at document `position` with a blank line (unless it already does),
/// so a table inserted right before it does not run into it
fn blank_line_before_table_at(doc: &mut DocumentMut, position: isize) {
    let keys: Vec<String> = doc.iter().map(|(key, _)| key.to_string()).collect();
    for key in keys {
        let tables: Vec<&mut Table> = match doc.get_mut(&key) {
            Some(Item::Table(table)) => vec![table],
            Some(Item::ArrayOfTables(array)) => array.iter_mut().collect(),
            _ => Vec::new(),
        };
        for table in tables {
            if table.position() != Some(position) {
                continue;
            }
            let decor = table.decor_mut();
            let prefix = decor
                .prefix()
                .and_then(|p| p.as_str())
                .unwrap_or_default()
                .to_string();
            if !prefix.starts_with('\n') {
                decor.set_prefix(format!("\n{prefix}"));
            }
        }
    }
}

fn apply_sink(table: &mut Table, sink: &SinkConfig) {
    set_value(table, "name", sink.name.as_str());
    set_value(table, "desc", sink.desc.as_str());
    set_optional(table, "icon", sink.icon.as_deref());
    if sink.default || table.contains_key("default") {
        set_value(table, "default", sink.default);
    }
}

fn apply_rule(table: &mut Table, rule: &Rule) {
    set_value(table, "app_id", rule.app_id_pattern.as_str());
    set_optional(table, "title", rule.title_pattern.as_deref());
    // Keep the value as written (integer positions, a one-entry array) if it still
    // says the same thing
    let refs: Vec<&str> = rule.sink_refs().collect();
    let unchanged = table
        .get("sink")
        .and_then(Item::as_value)
        .is_some_and(|v| sink_value_says(v, &refs));
    if !unchanged {
        if let [single] = refs.as_slice() {
            set_value(table, "sink", *single);
        } else {
            set_value(table, "sink", refs.iter().copied().collect::<Array>());
        }
    }
    set_optional(table, "desc", rule.desc.as_deref());
    match rule.notify {
        Some(notify) => set_value(table, "notify", notify),
        None => {
            table.remove("notify");
        }
    }
}

/// Whether a rule's `sink` value in the document names exactly `refs`, in order
fn sink_value_says(value: &Value, refs: &[&str]) -> bool {
    fn one(value: &Value, sink_ref: &str) -> bool {
        match value {
            Value::String(s) => s.value() == sink_ref,
            Value::Integer(n) => n.value().to_string() == sink_ref,
            _ => false,
        }
    }
    match (value, refs) {
        (Value::Array(array), _) => {
            array.len() == refs.len() && array.iter().zip(refs).all(|(v, r)| one(v, r))
        }
        (_, [single]) => one(value, single),
        _ => false,
    }
}

/// Rebuild the array of tables `key` from `items`.
///
/// Each item reuses its original table (`slots[i]`), so the comments above it and on its
/// lines move with it; new items get fresh tables; tables of removed items are dropped.
///
/// All entries render together at the position of the first original table, so
/// entries of another array written between them (e.g. a `[[rules]]` table between two
/// `[[sinks]]`) end up after the whole array. An inline array (`sinks = [...]`) is
/// rewritten as an array of tables; its comments are not kept.
fn rebuild_array<T>(
    doc: &mut DocumentMut,
    key: &str,
    items: &[T],
    slots: &[Option<usize>],
    apply: fn(&mut Table, &T),
) {
    let originals: Vec<Table> = doc
        .get(key)
        .and_then(Item::as_array_of_tables)
        .map_or_default(|a| a.iter().cloned().collect());
    // All entries share the first original position so they render in array order
    let position = originals.iter().filter_map(Table::position).min();

    let mut array = ArrayOfTables::new();
    for (item, slot) in items.iter().zip(slots) {
        let mut table = slot
            .and_then(|i| originals.get(i))
            .cloned()
            .unwrap_or_default();
        apply(&mut table, item);
        if position.is_some() {
            table.set_position(position);
        }
        array.push(table);
    }

    if array.is_empty() {
        doc.remove(key);
    } else if let Some((mut doc_key, existing)) = doc.as_table_mut().get_key_value_mut(key) {
        if !existing.is_array_of_tables() {
            // `sinks = [...]`: the key's spacing around `=` would end up inside the
            // table header (`[[sinks ]]`)
            doc_key.leaf_decor_mut().clear();
            doc_key.dotted_decor_mut().clear();
        }
        *existing = Item::ArrayOfTables(array);
    } else {
        doc.insert(key, Item::ArrayOfTables(array));
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;
    use crate::test_utils::fixtures::{
        make_config, make_rule, make_rule_with_fallbacks, make_sink,
    };

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
    #[case("empty_sink_name",
           vec![make_sink("", "Sink 1", true)],
           vec![],
           None,
           "empty name")]
    #[case("empty_sink_desc",
           vec![make_sink("sink1", " ", true)],
           vec![],
           None,
           "empty desc")]
    #[case("numeric_sink_desc",
           vec![make_sink("sink1", "2", true)],
           vec![],
           None,
           "is a number")]
    #[case("empty_rule_sink_ref",
           vec![make_sink("sink1", "Sink 1", true)],
           vec![make_rule("firefox", None, "")],
           None,
           "empty sink reference")]
    #[case("name_desc_ambiguity",
           vec![make_sink("alpha", "Speakers", true), make_sink("beta", "alpha", false)],
           vec![],
           None,
           "ambiguous")]
    #[case("positional_out_of_range",
           vec![make_sink("sink1", "Sink 1", true)],
           vec![make_rule("firefox", None, "3")],
           None,
           "unknown sink '3'")]
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

    #[rstest]
    #[case(0, true)]
    #[case(300, true)]
    #[case(MAX_SWITCH_DELAY_MS, true)]
    #[case(MAX_SWITCH_DELAY_MS + 1, false)]
    fn test_validate_switch_delay_range(#[case] delay: u32, #[case] valid: bool) {
        let mut config = make_config(vec![make_sink("sink1", "Sink 1", true)], vec![]);
        config.settings.switch_delay_ms = delay;
        let result = config.validate();
        assert_eq!(
            result.is_ok(),
            valid,
            "switch_delay_ms = {delay}: {result:?}"
        );
        if let Err(e) = result {
            assert!(e.to_string().contains("switch_delay_ms"), "{e}");
        }
    }

    #[test]
    fn test_switch_delay_loads_and_rejects_out_of_range() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(
            &dir,
            "ok.toml",
            &format!("[settings]\nswitch_delay_ms = 300\n{TWO_SINKS}"),
        );
        let config = Config::load_from_path(&path).expect("valid delay");
        assert_eq!(config.settings.switch_delay_ms, 300);

        let path = write(
            &dir,
            "big.toml",
            &format!("[settings]\nswitch_delay_ms = 5001\n{TWO_SINKS}"),
        );
        let err = format!("{:#}", Config::load_from_path(&path).unwrap_err());
        assert!(err.contains("switch_delay_ms"), "{err}");

        let path = write(
            &dir,
            "negative.toml",
            &format!("[settings]\nswitch_delay_ms = -1\n{TWO_SINKS}"),
        );
        assert!(Config::load_from_path(&path).is_err());
    }

    #[test]
    fn test_validate_allows_name_equal_to_own_desc() {
        let config = make_config(vec![make_sink("same", "same", true)], vec![]);
        assert!(config.validate().is_ok());
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
            assert_eq!(loaded, cfg);
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

    // ------------------------------------------------------------------------
    // Regex compilation
    // ------------------------------------------------------------------------

    #[rstest]
    #[case("^firefox$")]
    #[case("(mpv|vlc)")]
    #[case("^(.+)+-beta$")]
    #[case("(.*)*")]
    #[case("(?i)discord")]
    fn test_compile_rule_regex_accepts_valid_patterns(#[case] pattern: &str) {
        assert!(
            compile_rule_regex(pattern).is_ok(),
            "{pattern} should compile"
        );
    }

    #[test]
    fn test_compile_rule_regex_rejects_oversized_pattern() {
        let err = compile_rule_regex(r"\w{50000}").expect_err("huge repetition must be rejected");
        assert!(err.to_string().contains("size limit"), "got: {err}");
    }

    #[test]
    fn test_load_rejects_oversized_regex_naming_rule() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(
            &dir,
            "c.toml",
            "[[sinks]]\nname = \"a\"\ndesc = \"A\"\ndefault = true\n\n[[rules]]\napp_id = \
             '\\w{50000}'\nsink = \"A\"\n",
        );
        let err = format!("{:#}", Config::load_from_path(&path).unwrap_err());
        assert!(err.contains("Rule 1: invalid app_id regex"), "got: {err}");
    }

    #[test]
    fn test_save_rejects_pattern_load_would_reject() {
        let dir = tempfile::tempdir().unwrap();
        let mut rule = make_rule("firefox", None, "A");
        // Simulate an editor that compiled with a laxer builder
        rule.app_id_pattern = r"\w{50000}".to_string();
        let cfg = make_config(vec![make_sink("a", "A", true)], vec![rule]);
        let path = dir.path().join("c.toml");
        assert!(cfg.save_to(&path).is_err());
        assert!(!path.exists());
    }

    // ------------------------------------------------------------------------
    // Parsing: sink refs, unknown fields
    // ------------------------------------------------------------------------

    fn write(dir: &tempfile::TempDir, name: &str, contents: &str) -> PathBuf {
        let path = dir.path().join(name);
        std::fs::write(&path, contents).expect("write test config");
        path
    }

    const TWO_SINKS: &str = r#"[[sinks]]
name = "alsa_output.speakers"
desc = "Speakers"
default = true

[[sinks]]
name = "alsa_output.hdmi"
desc = "HDMI"
"#;

    #[test]
    fn test_sink_ref_accepts_integer_and_strings() {
        let dir = tempfile::tempdir().unwrap();
        let contents = format!(
            "{TWO_SINKS}\n[[rules]]\napp_id = \"a\"\nsink = 2\n\n[[rules]]\napp_id = \"b\"\nsink \
             = \"1\"\n\n[[rules]]\napp_id = \"c\"\nsink = \"HDMI\"\n\n[[rules]]\napp_id = \
             \"d\"\nsink = \"alsa_output.speakers\"\n"
        );
        let path = write(&dir, "c.toml", &contents);
        let cfg = Config::load_from_path(&path).expect("int and string refs should load");
        let refs: Vec<_> = cfg.rules.iter().map(|r| r.sink_ref.as_str()).collect();
        assert_eq!(refs, ["2", "1", "HDMI", "alsa_output.speakers"]);
        let targets: Vec<_> = cfg
            .rules
            .iter()
            .map(|r| cfg.resolve_sink(&r.sink_ref).unwrap().desc.as_str())
            .collect();
        assert_eq!(targets, ["HDMI", "Speakers", "HDMI", "Speakers"]);
    }

    #[test]
    fn test_sink_ref_rejects_other_types() {
        let dir = tempfile::tempdir().unwrap();
        let contents = format!("{TWO_SINKS}\n[[rules]]\napp_id = \"a\"\nsink = true\n");
        let path = write(&dir, "c.toml", &contents);
        let err = Config::load_from_path(&path).unwrap_err().to_string();
        assert!(
            err.contains("sink description, node name, or 1-based position"),
            "{err}"
        );
    }

    #[rstest]
    #[case("[settings]\ncolour = true\n", "colour")]
    #[case("[[sinks]]\nname = \"a\"\ndesc = \"A\"\ndefualt = true\n", "defualt")]
    #[case(
        "[[sinks]]\nname = \"a\"\ndesc = \"A\"\ndefault = true\n[[rules]]\napp_id = \"x\"\nsink = \
         \"A\"\ntitel = \"y\"\n",
        "titel"
    )]
    #[case("[setting]\nlog_level = \"info\"\n", "setting")]
    fn test_unknown_fields_rejected_with_key_and_path(#[case] contents: &str, #[case] key: &str) {
        let dir = tempfile::tempdir().unwrap();
        let path = write(&dir, "typo.toml", contents);
        let err = Config::load_from_path(&path).unwrap_err().to_string();
        assert!(err.contains(&format!("unknown field `{key}`")), "{err}");
        assert!(err.contains(&path.display().to_string()), "{err}");
    }

    #[test]
    fn test_load_missing_file_errors_without_creating() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pwsw").join("config.toml");
        let err = Config::load_from_path(&path).unwrap_err();
        assert!(err.downcast_ref::<ConfigNotFound>().is_some());
        assert!(err.to_string().contains("pwsw init"), "{err}");
        assert!(!path.exists());
        assert!(!dir.path().join("pwsw").exists());
    }

    #[test]
    fn test_default_template_loads_and_init_refuses_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pwsw").join("config.toml");
        Config::init_at(&path, false).expect("init into empty dir");
        let cfg = Config::load_from_path(&path).expect("template must load");
        assert_eq!(cfg.sinks, Vec::<SinkConfig>::new());
        assert_eq!(cfg.settings, Settings::default());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }

        std::fs::write(&path, "# mine\n").unwrap();
        assert!(Config::init_at(&path, false).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "# mine\n");
        Config::init_at(&path, true).expect("forced init");
        assert_eq!(
            std::fs::read_to_string(dir.path().join("pwsw").join("config.toml.bak")).unwrap(),
            "# mine\n"
        );
    }

    // ------------------------------------------------------------------------
    // Mutation methods
    // ------------------------------------------------------------------------

    fn three_sinks() -> Config {
        make_config(
            vec![
                make_sink("n.speakers", "Speakers", true),
                make_sink("n.hdmi", "HDMI", false),
                make_sink("n.phones", "Headphones", false),
            ],
            vec![
                make_rule("by_desc", None, "HDMI"),
                make_rule("by_name", None, "n.hdmi"),
                make_rule("by_pos", None, "3"),
            ],
        )
    }

    #[test]
    fn test_add_sink_rejects_duplicates_and_defaults_first() {
        let mut cfg = make_config(vec![], vec![]);
        cfg.add_sink(make_sink("n.a", "A", false)).unwrap();
        assert!(cfg.sinks[0].default, "first sink becomes default");

        let err = cfg.add_sink(make_sink("n.a", "Other", false)).unwrap_err();
        assert!(err.to_string().contains("node name 'n.a'"), "{err}");
        let err = cfg.add_sink(make_sink("n.b", "A", false)).unwrap_err();
        assert!(err.to_string().contains("description 'A'"), "{err}");
        assert_eq!(cfg.sinks.len(), 1, "rejected adds change nothing");

        cfg.add_sink(make_sink("n.b", "B", true)).unwrap();
        assert!(!cfg.sinks[0].default && cfg.sinks[1].default);
    }

    #[test]
    fn test_add_sink_rejects_ambiguous_name() {
        let mut cfg = make_config(vec![make_sink("n.a", "A", true)], vec![]);
        let err = cfg.add_sink(make_sink("A", "B", false)).unwrap_err();
        assert!(err.to_string().contains("ambiguous"), "{err}");
        assert_eq!(cfg.sinks.len(), 1);
    }

    #[test]
    fn test_update_sink_cascades_renames_into_rules() {
        let mut cfg = three_sinks();
        let mut hdmi = cfg.sinks[1].clone();
        hdmi.desc = "Television".to_string();
        hdmi.name = "n.tv".to_string();
        cfg.update_sink(1, hdmi).unwrap();

        let refs: Vec<_> = cfg.rules.iter().map(|r| r.sink_ref.as_str()).collect();
        assert_eq!(refs, ["Television", "n.tv", "3"]);
        for rule in &cfg.rules[..2] {
            assert_eq!(cfg.resolve_sink(&rule.sink_ref).unwrap().name, "n.tv");
        }
    }

    #[test]
    fn test_update_sink_keeps_exactly_one_default() {
        let mut cfg = three_sinks();
        let mut phones = cfg.sinks[2].clone();
        phones.default = true;
        cfg.update_sink(2, phones).unwrap();
        let defaults: Vec<_> = cfg.sinks.iter().map(|s| s.default).collect();
        assert_eq!(defaults, [false, false, true]);

        // Un-defaulting the default falls back to the first sink
        let mut phones = cfg.sinks[2].clone();
        phones.default = false;
        cfg.update_sink(2, phones).unwrap();
        let defaults: Vec<_> = cfg.sinks.iter().map(|s| s.default).collect();
        assert_eq!(defaults, [true, false, false]);
    }

    #[test]
    fn test_update_sink_rejects_duplicate() {
        let mut cfg = three_sinks();
        let mut hdmi = cfg.sinks[1].clone();
        hdmi.desc = "Speakers".to_string();
        assert!(cfg.update_sink(1, hdmi).is_err());
        assert_eq!(cfg.sinks[1].desc, "HDMI");
    }

    #[test]
    fn test_remove_sink_refuses_when_referenced() {
        let mut cfg = three_sinks();
        let err = cfg.remove_sink(1).unwrap_err().to_string();
        assert!(err.contains("Cannot remove sink 'HDMI'"), "{err}");
        assert!(
            err.contains("1 (by_desc)") && err.contains("2 (by_name)"),
            "{err}"
        );
        let err = cfg.remove_sink(2).unwrap_err().to_string();
        assert!(err.contains("3 (by_pos)"), "{err}");
        assert_eq!(cfg.sinks.len(), 3);
    }

    #[test]
    fn test_remove_sink_rewrites_shifted_positional_refs() {
        let mut cfg = three_sinks();
        cfg.rules.clear();
        cfg.add_rule(make_rule("p", None, "n.phones")).unwrap();
        // Positional ref to the last sink, which shifts when the first is removed
        cfg.rules[0].sink_ref = "3".to_string();
        cfg.remove_sink(0).unwrap();
        assert_eq!(cfg.rules[0].sink_ref, "Headphones");
        assert!(
            cfg.sinks[0].default,
            "removing the default promotes the first sink"
        );
    }

    #[test]
    fn test_move_sink_rewrites_positional_refs() {
        let mut cfg = three_sinks();
        cfg.rules.push(make_rule("first", None, "1"));
        cfg.move_sink(2, 0).unwrap();
        let names: Vec<_> = cfg.sinks.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["n.phones", "n.speakers", "n.hdmi"]);
        let targets: Vec<_> = cfg
            .rules
            .iter()
            .map(|r| cfg.resolve_sink(&r.sink_ref).unwrap().name.as_str())
            .collect();
        assert_eq!(targets, ["n.hdmi", "n.hdmi", "n.phones", "n.speakers"]);
        assert_eq!(cfg.rules[2].sink_ref, "Headphones");
        assert_eq!(cfg.rules[3].sink_ref, "Speakers");
    }

    #[test]
    fn test_rule_methods_store_desc_refs() {
        let mut cfg = three_sinks();
        cfg.add_rule(make_rule("new", None, "n.phones")).unwrap();
        assert_eq!(cfg.rules[3].sink_ref, "Headphones");
        cfg.update_rule(2, make_rule("by_pos", None, "2")).unwrap();
        assert_eq!(cfg.rules[2].sink_ref, "HDMI");
        assert!(cfg.add_rule(make_rule("x", None, "nope")).is_err());
        assert!(cfg.add_rule(make_rule("", None, "HDMI")).is_err());
        assert_eq!(cfg.rules.len(), 4);

        cfg.move_rule(3, 0).unwrap();
        assert_eq!(cfg.rules[0].app_id_pattern, "new");
        cfg.remove_rule(0).unwrap();
        assert_eq!(cfg.rules.len(), 3);
        assert!(cfg.remove_rule(9).is_err());
    }

    #[test]
    fn test_set_default_sink() {
        let mut cfg = three_sinks();
        cfg.set_default_sink(1).unwrap();
        assert_eq!(cfg.get_default_sink().unwrap().name, "n.hdmi");
        assert!(cfg.set_default_sink(7).is_err());
    }

    // ------------------------------------------------------------------------
    // Saving: validation, comments, symlinks, backups, external edits
    // ------------------------------------------------------------------------

    #[test]
    fn test_save_refuses_invalid_config() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("c.toml");
        let cfg = make_config(
            vec![make_sink("a", "A", true), make_sink("b", "B", true)],
            vec![],
        );
        let err = cfg.save_to(&path).unwrap_err().to_string();
        assert!(err.contains("default sinks found"), "{err}");
        assert!(!path.exists());
    }

    const COMMENTED: &str = r#"# My pwsw config
# (hand written)

[settings]
notify_manual = false # I find these noisy
log_level = "debug"

# --- outputs ---
[[sinks]]
name = "alsa_output.speakers"
desc = "Speakers" # desk speakers
default = true

# the TV
[[sinks]]
name = "alsa_output.hdmi"
desc = "HDMI"

# --- rules ---
[[rules]]
app_id = "^mpv$" # video player
sink = 2

# games
[[rules]]
app_id = "^steam$"
sink = "HDMI"

# trailing notes survive too
"#;

    #[test]
    fn test_save_unchanged_is_byte_identical() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(&dir, "c.toml", COMMENTED);
        let cfg = Config::load_from_path(&path).unwrap();
        cfg.save_to(&path).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), COMMENTED);
    }

    #[test]
    fn test_save_preserves_comments_through_edits() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(&dir, "c.toml", COMMENTED);
        let mut cfg = Config::load_from_path(&path).unwrap();

        cfg.settings.notify_manual = true;
        cfg.settings.match_by_index = true;
        let mut hdmi = cfg.sinks[1].clone();
        hdmi.desc = "Television".to_string();
        cfg.update_sink(1, hdmi).unwrap();
        cfg.add_sink(make_sink("bluez_output.phones", "Headphones", false))
            .unwrap();
        cfg.add_rule(make_rule("^spotify$", None, "Headphones"))
            .unwrap();
        cfg.save_to(&path).unwrap();

        let out = std::fs::read_to_string(&path).unwrap();
        for needle in [
            "# My pwsw config\n# (hand written)\n",
            "notify_manual = true # I find these noisy",
            "log_level = \"debug\"",
            "match_by_index = true",
            "# --- outputs ---\n[[sinks]]",
            "desc = \"Speakers\" # desk speakers",
            "# the TV\n[[sinks]]\nname = \"alsa_output.hdmi\"\ndesc = \"Television\"",
            "# --- rules ---",
            "app_id = \"^mpv$\" # video player\nsink = 2\n",
            "# games\n[[rules]]\napp_id = \"^steam$\"\nsink = \"Television\"",
            "# trailing notes survive too",
            "name = \"bluez_output.phones\"",
            "app_id = \"^spotify$\"\nsink = \"Headphones\"",
        ] {
            assert!(out.contains(needle), "missing {needle:?} in:\n{out}");
        }
        // New sink is placed with the other sinks, before the rules
        let phones = out.find("bluez_output.phones").unwrap();
        assert!(phones < out.find("# --- rules ---").unwrap(), "{out}");
        assert_eq!(Config::load_from_path(&path).unwrap(), cfg);
    }

    /// A `[settings]` table added on save is separated from the table after it
    #[test]
    fn test_save_new_settings_table_gets_a_blank_line_after_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(
            &dir,
            "c.toml",
            "[[sinks]]\nname = \"a\"\ndesc = \"A\"\ndefault = true\n",
        );
        let mut cfg = Config::load_from_path(&path).unwrap();
        cfg.settings.log_level = "debug".to_string();
        cfg.save_to(&path).unwrap();
        let out = std::fs::read_to_string(&path).unwrap();
        assert_eq!(
            out,
            "[settings]\nlog_level = \"debug\"\n\n[[sinks]]\nname = \"a\"\ndesc = \"A\"\ndefault = \
             true\n"
        );
        assert_eq!(Config::load_from_path(&path).unwrap(), cfg);
    }

    /// An inline `sinks = [...]` array becomes an array of tables with a clean header
    #[test]
    fn test_save_inline_sinks_array_gets_clean_table_headers() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(
            &dir,
            "c.toml",
            "sinks = [ { name = \"a\", desc = \"A\", default = true } ]\n",
        );
        let mut cfg = Config::load_from_path(&path).unwrap();
        cfg.add_sink(make_sink("b", "B", false)).unwrap();
        cfg.save_to(&path).unwrap();
        let out = std::fs::read_to_string(&path).unwrap();
        assert!(!out.contains("[[sinks ]]"), "{out}");
        assert_eq!(out.matches("[[sinks]]\n").count(), 2, "{out}");
        assert_eq!(Config::load_from_path(&path).unwrap(), cfg);
    }

    #[test]
    fn test_save_moves_and_removes_entries_with_their_comments() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(&dir, "c.toml", COMMENTED);
        let mut cfg = Config::load_from_path(&path).unwrap();
        cfg.move_rule(1, 0).unwrap();
        cfg.move_sink(1, 0).unwrap();
        cfg.remove_rule(1).unwrap(); // the mpv rule
        cfg.save_to(&path).unwrap();

        let out = std::fs::read_to_string(&path).unwrap();
        assert!(!out.contains("^mpv$"), "{out}");
        assert!(!out.contains("video player"), "{out}");
        let tv = out.find("# the TV").unwrap();
        let speakers = out.find("desk speakers").unwrap();
        assert!(tv < speakers, "moved sink keeps its comment:\n{out}");
        assert_eq!(Config::load_from_path(&path).unwrap(), cfg);
    }

    #[test]
    fn test_save_after_direct_vec_mutation_matches_by_name() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(&dir, "c.toml", COMMENTED);
        let mut cfg = Config::load_from_path(&path).unwrap();
        cfg.rules.clear();
        cfg.sinks.remove(0);
        cfg.sinks[0].default = true;
        cfg.save_to(&path).unwrap();
        let out = std::fs::read_to_string(&path).unwrap();
        assert!(out.contains("# the TV\n[[sinks]]"), "{out}");
        assert!(!out.contains("[[rules]]"), "{out}");
        assert_eq!(Config::load_from_path(&path).unwrap(), cfg);
    }

    #[test]
    fn test_save_through_symlink_updates_target() {
        let dir = tempfile::tempdir().unwrap();
        let real = write(&dir, "real.toml", TWO_SINKS);
        let link_dir = dir.path().join("pwsw");
        std::fs::create_dir(&link_dir).unwrap();
        let link = link_dir.join("config.toml");
        std::os::unix::fs::symlink(&real, &link).unwrap();

        let mut cfg = Config::load_from_path(&link).unwrap();
        cfg.set_default_sink(1).unwrap();
        cfg.save_to(&link).unwrap();

        assert!(
            std::fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink(),
            "symlink must survive the save"
        );
        assert_eq!(std::fs::read_link(&link).unwrap(), real);
        let loaded = Config::load_from_path(&real).unwrap();
        assert_eq!(loaded.get_default_sink().unwrap().desc, "HDMI");
        // Backup sits next to the path that was saved
        assert_eq!(
            std::fs::read_to_string(link_dir.join("config.toml.bak")).unwrap(),
            TWO_SINKS
        );
    }

    #[test]
    fn test_backup_written_next_to_custom_path() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(&dir, "custom.toml", TWO_SINKS);
        let mut cfg = Config::load_from_path(&path).unwrap();
        cfg.settings.log_level = "warn".to_string();
        cfg.save_to(&path).unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join("custom.toml.bak")).unwrap(),
            TWO_SINKS
        );
    }

    #[test]
    fn test_save_refuses_when_file_changed_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(&dir, "c.toml", TWO_SINKS);
        let mut cfg = Config::load_from_path(&path).unwrap();
        let external = format!("{TWO_SINKS}# edited in vim\n");
        std::fs::write(&path, &external).unwrap();

        cfg.settings.log_level = "warn".to_string();
        let err = cfg.save_to(&path).unwrap_err();
        assert!(
            err.downcast_ref::<ConfigModifiedOnDisk>().is_some(),
            "{err}"
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), external);

        cfg.save_impl(&path, true).expect("forced save overwrites");
        assert!(
            std::fs::read_to_string(&path)
                .unwrap()
                .contains("log_level = \"warn\"")
        );
    }

    #[test]
    fn test_consecutive_saves_through_clones_are_not_conflicts() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(&dir, "c.toml", TWO_SINKS);
        let mut cfg = Config::load_from_path(&path).unwrap();
        // The TUI saves a clone in the background, then keeps editing the original
        let snapshot = cfg.clone();
        cfg.settings.log_level = "warn".to_string();
        snapshot.save_to(&path).unwrap();
        cfg.save_to(&path)
            .expect("second save must not see its own write as a conflict");
        cfg.add_sink(make_sink("n.x", "X", false)).unwrap();
        cfg.save_to(&path).unwrap();
        assert_eq!(Config::load_from_path(&path).unwrap(), cfg);
    }

    #[test]
    fn test_fresh_config_generates_complete_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("c.toml");
        let cfg = three_sinks();
        cfg.save_to(&path).unwrap();
        let out = std::fs::read_to_string(&path).unwrap();
        assert!(out.starts_with("[settings]\n"), "{out}");
        assert!(out.contains("log_level = \"info\""), "{out}");
        assert_eq!(Config::load_from_path(&path).unwrap(), cfg);
    }

    // ------------------------------------------------------------------------
    // Fallback sinks (`sink = [...]`)
    // ------------------------------------------------------------------------

    const THREE_SINKS: &str = r#"[[sinks]]
name = "alsa_output.speakers"
desc = "Speakers"
default = true

[[sinks]]
name = "alsa_output.hdmi"
desc = "HDMI"

[[sinks]]
name = "bluez_output.phones"
desc = "BT Headphones"
"#;

    fn refs(cfg: &Config, index: usize) -> Vec<&str> {
        cfg.rules[index].sink_refs().collect()
    }

    /// Set all sinks of rule `index` through `update_rule`, like a list editor would
    fn set_sinks(cfg: &mut Config, index: usize, sinks: &[&str]) -> Result<()> {
        let mut rule = cfg.rules[index].clone();
        let (first, rest) = sinks.split_first().expect("at least one sink");
        rule.sink_ref = (*first).to_string();
        rule.fallback_refs = rest.iter().map(|s| (*s).to_string()).collect();
        cfg.update_rule(index, rule)
    }

    #[test]
    fn test_sink_list_parses_strings_integers_and_single_entries() {
        let dir = tempfile::tempdir().unwrap();
        let contents = format!(
            "{THREE_SINKS}\n[[rules]]\napp_id = \"a\"\nsink = [\"BT Headphones\", \"HDMI\"]\n\n\
             [[rules]]\napp_id = \"b\"\nsink = [3, \"alsa_output.speakers\"]\n\n[[rules]]\napp_id \
             = \"c\"\nsink = [\"HDMI\"]\n\n[[rules]]\napp_id = \"d\"\nsink = \"HDMI\"\n"
        );
        let path = write(&dir, "c.toml", &contents);
        let cfg = Config::load_from_path(&path).expect("sink lists load");
        assert_eq!(refs(&cfg, 0), ["BT Headphones", "HDMI"]);
        assert_eq!(cfg.rules[0].sink_ref, "BT Headphones", "first choice");
        assert_eq!(cfg.rules[0].fallback_refs, ["HDMI"]);
        assert_eq!(refs(&cfg, 1), ["3", "alsa_output.speakers"]);
        assert_eq!(cfg.resolve_rule_sinks(&cfg.rules[1]), Some(vec![2, 0]));
        assert_eq!(refs(&cfg, 2), ["HDMI"]);
        assert_eq!(cfg.rules[2].fallback_refs, [] as [String; 0]);
        assert_eq!(refs(&cfg, 3), ["HDMI"]);
    }

    #[rstest]
    #[case::empty("sink = []", "empty list")]
    #[case::unknown("sink = [\"HDMI\", \"Nope\"]", "unknown sink 'Nope'")]
    #[case::unknown_position("sink = [\"HDMI\", 7]", "unknown sink '7'")]
    #[case::duplicate(
        "sink = [\"HDMI\", \"alsa_output.hdmi\"]",
        "lists sink 'HDMI' more than once"
    )]
    #[case::duplicate_position("sink = [2, \"HDMI\"]", "more than once")]
    #[case::empty_entry("sink = [\"HDMI\", \" \"]", "empty sink reference")]
    #[case::nested(
        "sink = [[\"HDMI\"]]",
        "sink description, node name, or 1-based position"
    )]
    #[case::boolean_entry("sink = [\"HDMI\", true]", "1-based position")]
    fn test_sink_list_rejections(#[case] sink: &str, #[case] expected: &str) {
        let dir = tempfile::tempdir().unwrap();
        let contents = format!("{THREE_SINKS}\n[[rules]]\napp_id = \"a\"\n{sink}\n");
        let path = write(&dir, "c.toml", &contents);
        let err = format!("{:#}", Config::load_from_path(&path).unwrap_err());
        assert!(err.contains(expected), "{err}");
    }

    const WITH_LISTS: &str = r#"[[sinks]]
name = "alsa_output.speakers"
desc = "Speakers"
default = true

[[sinks]]
name = "alsa_output.hdmi"
desc = "HDMI"

[[sinks]]
name = "bluez_output.phones"
desc = "BT Headphones"

[[rules]]
app_id = "^discord$"
sink = ["BT Headphones", 2] # phones first

[[rules]]
app_id = "^mpv$"
sink = [ "HDMI" ]

[[rules]]
app_id = "^steam$"
sink = "HDMI"
"#;

    #[test]
    fn test_save_keeps_lists_and_strings_as_written() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(&dir, "c.toml", WITH_LISTS);
        let mut cfg = Config::load_from_path(&path).unwrap();
        cfg.save_to(&path).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), WITH_LISTS);

        // An unrelated edit leaves every rule's text alone
        cfg.settings.log_level = "debug".to_string();
        cfg.save_to(&path).unwrap();
        let out = std::fs::read_to_string(&path).unwrap();
        assert!(
            out.ends_with(&WITH_LISTS[WITH_LISTS.find("[[rules]]").unwrap()..]),
            "{out}"
        );
        assert_eq!(Config::load_from_path(&path).unwrap(), cfg);
    }

    #[test]
    fn test_save_writes_a_string_for_one_sink_and_an_array_for_several() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(&dir, "c.toml", WITH_LISTS);
        let mut cfg = Config::load_from_path(&path).unwrap();
        set_sinks(&mut cfg, 0, &["HDMI"]).unwrap();
        set_sinks(&mut cfg, 2, &["alsa_output.hdmi", "Speakers"]).unwrap();
        cfg.save_to(&path).unwrap();
        let out = std::fs::read_to_string(&path).unwrap();
        assert!(
            out.contains("app_id = \"^discord$\"\nsink = \"HDMI\" # phones first\n"),
            "single target is a string again, comment kept:\n{out}"
        );
        assert!(
            out.contains("app_id = \"^steam$\"\nsink = [\"HDMI\", \"Speakers\"]\n"),
            "several targets are an inline array of descs:\n{out}"
        );
        let reloaded = Config::load_from_path(&path).unwrap();
        assert_eq!(reloaded, cfg);
        assert_eq!(refs(&reloaded, 2), ["HDMI", "Speakers"]);

        // A fresh in-memory config writes lists too
        let fresh = Config::new(Settings::default(), cfg.sinks.clone(), vec![
            make_rule_with_fallbacks("x", None, &["BT Headphones", "HDMI"]),
        ]);
        let fresh_path = dir.path().join("fresh.toml");
        fresh.save_to(&fresh_path).unwrap();
        let out = std::fs::read_to_string(&fresh_path).unwrap();
        assert!(
            out.contains("sink = [\"BT Headphones\", \"HDMI\"]"),
            "{out}"
        );
        assert_eq!(Config::load_from_path(&fresh_path).unwrap(), fresh);
    }

    #[test]
    fn test_rule_methods_validate_and_canonicalize_lists() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(&dir, "c.toml", WITH_LISTS);
        let mut cfg = Config::load_from_path(&path).unwrap();
        set_sinks(&mut cfg, 1, &["1", "bluez_output.phones"]).unwrap();
        assert_eq!(refs(&cfg, 1), ["Speakers", "BT Headphones"]);
        for bad in [&["Nope"][..], &["HDMI", "Nope"], &["Speakers", "HDMI", "2"]] {
            assert!(set_sinks(&mut cfg, 1, bad).is_err(), "{bad:?}");
        }
        assert_eq!(refs(&cfg, 1), ["Speakers", "BT Headphones"], "unchanged");

        cfg.add_rule(make_rule_with_fallbacks("new", None, &[
            "3",
            "alsa_output.hdmi",
        ]))
        .unwrap();
        assert_eq!(refs(&cfg, 3), ["BT Headphones", "HDMI"]);
        let dup = make_rule_with_fallbacks("dup", None, &["HDMI", "Speakers", "1"]);
        assert!(cfg.add_rule(dup).is_err());
        assert_eq!(cfg.rules.len(), 4);
    }

    #[test]
    fn test_sink_rename_and_move_cascade_into_fallbacks() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(&dir, "c.toml", WITH_LISTS);
        let mut cfg = Config::load_from_path(&path).unwrap();
        let mut hdmi = cfg.sinks[1].clone();
        hdmi.desc = "Television".to_string();
        cfg.update_sink(1, hdmi).unwrap();
        // The positional fallback `2` still points at the renamed sink
        assert_eq!(refs(&cfg, 0), ["BT Headphones", "2"]);
        assert_eq!(refs(&cfg, 1), ["Television"]);

        let mut phones = cfg.sinks[2].clone();
        phones.desc = "Headset".to_string();
        cfg.update_sink(2, phones).unwrap();
        assert_eq!(refs(&cfg, 0), ["Headset", "2"]);

        // Moving the sinks rewrites the positional fallback so it keeps its target
        cfg.move_sink(1, 0).unwrap();
        assert_eq!(refs(&cfg, 0), ["Headset", "Television"]);
        assert_eq!(cfg.resolve_rule_sinks(&cfg.rules[0]), Some(vec![2, 0]));

        cfg.save_to(&path).unwrap();
        let out = std::fs::read_to_string(&path).unwrap();
        assert!(
            out.contains("sink = [\"Headset\", \"Television\"] # phones first"),
            "{out}"
        );
        assert_eq!(Config::load_from_path(&path).unwrap(), cfg);
    }

    #[test]
    fn test_remove_sink_refused_while_a_fallback_uses_it() {
        let dir = tempfile::tempdir().unwrap();
        let contents = format!(
            "{THREE_SINKS}\n[[rules]]\napp_id = \"^discord$\"\nsink = [\"HDMI\", \"BT \
             Headphones\"]\n"
        );
        let path = write(&dir, "c.toml", &contents);
        let mut cfg = Config::load_from_path(&path).unwrap();
        let err = cfg.remove_sink(2).unwrap_err().to_string();
        assert!(
            err.contains("Cannot remove sink 'BT Headphones'") && err.contains("1 (^discord$)"),
            "{err}"
        );
        assert_eq!(cfg.sinks.len(), 3);
        // Once the rule no longer lists it, it can go
        set_sinks(&mut cfg, 0, &["HDMI"]).unwrap();
        cfg.remove_sink(2).unwrap();
        assert_eq!(cfg.sinks.len(), 2);
    }

    /// What the TUI's rule editor does: it edits the first choice and passes the
    /// existing rule's fallbacks along to `update_rule`
    fn tui_edit(cfg: &mut Config, index: usize, edit: impl FnOnce(&mut Rule)) -> Result<()> {
        let old = &cfg.rules[index];
        let mut rule = make_rule(
            &old.app_id_pattern,
            old.title_pattern.as_deref(),
            &old.sink_ref,
        );
        rule.desc.clone_from(&old.desc);
        rule.notify = old.notify;
        rule.fallback_refs.clone_from(&old.fallback_refs);
        edit(&mut rule);
        cfg.update_rule(index, rule)
    }

    #[test]
    fn test_rule_edits_keep_fallbacks() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(&dir, "c.toml", WITH_LISTS);
        let mut cfg = Config::load_from_path(&path).unwrap();

        // Editing the description keeps the fallbacks (stored as descs now)
        tui_edit(&mut cfg, 0, |r| r.desc = Some("Voice".to_string())).unwrap();
        assert_eq!(refs(&cfg, 0), ["BT Headphones", "HDMI"]);
        // Editing the pattern keeps them too
        tui_edit(&mut cfg, 0, |r| {
            r.app_id_pattern = "^(discord|vesktop)$".to_string();
            r.app_id_regex = compile_rule_regex(&r.app_id_pattern).unwrap();
        })
        .unwrap();
        assert_eq!(refs(&cfg, 0), ["BT Headphones", "HDMI"]);
        // Promoting a fallback to first choice drops it from the fallbacks
        tui_edit(&mut cfg, 0, |r| r.sink_ref = "alsa_output.hdmi".to_string()).unwrap();
        assert_eq!(refs(&cfg, 0), ["HDMI"]);
        set_sinks(&mut cfg, 0, &["BT Headphones", "HDMI"]).unwrap();

        // Fallbacks follow their rule through moves and removals
        cfg.move_rule(0, 2).unwrap();
        assert_eq!(cfg.rules[2].app_id_pattern, "^(discord|vesktop)$");
        assert_eq!(refs(&cfg, 2), ["BT Headphones", "HDMI"]);
        cfg.remove_rule(0).unwrap();
        assert_eq!(refs(&cfg, 1), ["BT Headphones", "HDMI"]);

        cfg.save_to(&path).unwrap();
        let out = std::fs::read_to_string(&path).unwrap();
        assert!(
            out.contains(
                "app_id = \"^(discord|vesktop)$\"\nsink = [\"BT Headphones\", \"HDMI\"] # phones \
                 first\ndesc = \"Voice\""
            ),
            "{out}"
        );
        assert_eq!(Config::load_from_path(&path).unwrap(), cfg);
    }

    /// Rules with identical patterns keep their own fallbacks through saves, cascades
    /// and moves
    #[test]
    fn test_identical_patterns_keep_their_own_fallbacks() {
        let dir = tempfile::tempdir().unwrap();
        let contents = format!(
            "{THREE_SINKS}\n[[rules]]\napp_id = \"^mpv$\"\nsink = [\"HDMI\", \"Speakers\"]\n\n\
             [[rules]]\napp_id = \"^mpv$\"\nsink = [\"BT Headphones\", 2]\n"
        );
        let path = write(&dir, "c.toml", &contents);
        let mut cfg = Config::load_from_path(&path).unwrap();
        assert_eq!(refs(&cfg, 0), ["HDMI", "Speakers"]);
        assert_eq!(refs(&cfg, 1), ["BT Headphones", "2"]);

        cfg.save_to(&path).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), contents);

        let mut hdmi = cfg.sinks[1].clone();
        hdmi.desc = "Television".to_string();
        cfg.update_sink(1, hdmi).unwrap();
        cfg.move_sink(1, 2).unwrap();
        cfg.move_rule(1, 0).unwrap();
        assert_eq!(refs(&cfg, 0), ["BT Headphones", "Television"]);
        assert_eq!(refs(&cfg, 1), ["Television", "Speakers"]);

        cfg.save_to(&path).unwrap();
        let reloaded = Config::load_from_path(&path).unwrap();
        assert_eq!(reloaded, cfg);
        assert_eq!(refs(&reloaded, 0), ["BT Headphones", "Television"]);
        assert_eq!(refs(&reloaded, 1), ["Television", "Speakers"]);
    }
}
