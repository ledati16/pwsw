#![allow(dead_code)]

#[cfg(test)]
use std::ffi::OsString;

#[cfg(test)]
/// RAII helper: set `XDG_CONFIG_HOME` to a tempdir for the lifetime of this guard.
pub(crate) struct XdgTemp {
    prev: Option<OsString>,
    dir: tempfile::TempDir,
}

#[cfg(test)]
impl XdgTemp {
    /// Create and activate a temporary `XDG_CONFIG_HOME`.
    ///
    /// # Panics
    ///
    /// Panics if a temporary directory cannot be created.
    #[must_use]
    pub fn new() -> Self {
        let dir = tempfile::tempdir().expect("failed to create tempdir for XDG_CONFIG_HOME");
        let prev = std::env::var_os("XDG_CONFIG_HOME");
        // SAFETY: Test-only code, single-threaded test execution, no concurrent env access
        unsafe {
            std::env::set_var("XDG_CONFIG_HOME", dir.path());
        }
        Self { prev, dir }
    }

    /// Path to the temporary `XDG_CONFIG_HOME` directory.
    #[must_use]
    pub fn path(&self) -> &std::path::Path {
        self.dir.path()
    }
}

#[cfg(test)]
impl Default for XdgTemp {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
impl Drop for XdgTemp {
    fn drop(&mut self) {
        // SAFETY: Test-only code, restoring environment after test
        unsafe {
            if let Some(ref val) = self.prev {
                std::env::set_var("XDG_CONFIG_HOME", val);
            } else {
                std::env::remove_var("XDG_CONFIG_HOME");
            }
        }
        // TempDir will be removed when dropped
    }
}

/// Shared test fixtures for building test data structures.
///
/// These helpers provide a consistent way to construct `Config`, `SinkConfig`, and `Rule`
/// objects for use in unit and integration tests. They use sensible defaults to
/// minimize test boilerplate while allowing customization of relevant fields.
#[cfg(test)]
pub(crate) mod fixtures {
    use crate::config::{Config, Rule, Settings, SinkConfig};
    use regex::Regex;

    /// Create a test `Config` with the given sinks and rules.
    ///
    /// Settings use standard test defaults (all features enabled, `log_level` = "info").
    pub fn make_config(sinks: Vec<SinkConfig>, rules: Vec<Rule>) -> Config {
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

    /// Create a test `SinkConfig` with the given name, description, and default status.
    ///
    /// Icon is set to None by default.
    pub fn make_sink(name: &str, desc: &str, default: bool) -> SinkConfig {
        SinkConfig {
            name: name.to_string(),
            desc: desc.to_string(),
            icon: None,
            default,
        }
    }

    /// Create a test `SinkConfig` with an explicit icon.
    ///
    /// Useful for testing icon-related functionality in sinks and notifications.
    pub fn make_sink_with_icon(name: &str, desc: &str, default: bool, icon: &str) -> SinkConfig {
        SinkConfig {
            name: name.to_string(),
            desc: desc.to_string(),
            icon: Some(icon.to_string()),
            default,
        }
    }

    /// Create a test `Rule` with the given `app_id` pattern, optional title pattern, and sink reference.
    ///
    /// Compiles regex patterns from strings. Uses None for desc and notify fields.
    ///
    /// # Panics
    ///
    /// Panics if the `app_id` or title regex patterns are invalid.
    pub fn make_rule(app_id: &str, title: Option<&str>, sink_ref: &str) -> Rule {
        Rule {
            app_id_regex: Regex::new(app_id).expect("Invalid app_id regex in test fixture"),
            title_regex: title.map(|t| Regex::new(t).expect("Invalid title regex in test fixture")),
            sink_ref: sink_ref.to_string(),
            desc: None,
            notify: None,
            app_id_pattern: app_id.to_string(),
            title_pattern: title.map(String::from),
        }
    }
}
