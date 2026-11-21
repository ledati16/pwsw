//! Test-only helpers.
//!
//! Tests must never modify process environment variables: doing so is unsound while
//! other test threads run. Isolate file-system state by passing explicit paths instead
//! (e.g. `tempfile::tempdir()` plus `Config::load_from_path` / `Config::save_to`).
#![allow(dead_code)]

/// Shared test fixtures for building test data structures.
///
/// These helpers provide a consistent way to construct `Config`, `SinkConfig`, and `Rule`
/// objects for use in unit and integration tests. They use sensible defaults to
/// minimize test boilerplate while allowing customization of relevant fields.
#[cfg(test)]
pub(crate) mod fixtures {
    use crate::config::{Config, Rule, Settings, SinkConfig, compile_rule_regex};

    /// Create a test `Config` with the given sinks and rules.
    ///
    /// Settings use standard test defaults (all features enabled, `log_level` = "info").
    pub fn make_config(sinks: Vec<SinkConfig>, rules: Vec<Rule>) -> Config {
        Config::new(
            Settings {
                default_on_startup: true,
                set_smart_toggle: true,
                notify_manual: true,
                notify_rules: true,
                match_by_index: false,
                log_level: "info".to_string(),
            },
            sinks,
            rules,
        )
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
            app_id_regex: compile_rule_regex(app_id).expect("Invalid app_id regex in test fixture"),
            title_regex: title
                .map(|t| compile_rule_regex(t).expect("Invalid title regex in test fixture")),
            sink_ref: sink_ref.to_string(),
            fallback_refs: Vec::new(),
            desc: None,
            notify: None,
            app_id_pattern: app_id.to_string(),
            title_pattern: title.map(String::from),
        }
    }

    /// Like [`make_rule`], with fallback sinks: `sinks[0]` first, then the rest in order.
    ///
    /// # Panics
    ///
    /// Panics if `sinks` is empty or a regex pattern is invalid.
    pub fn make_rule_with_fallbacks(app_id: &str, title: Option<&str>, sinks: &[&str]) -> Rule {
        let (first, rest) = sinks.split_first().expect("at least one sink");
        let mut rule = make_rule(app_id, title, first);
        rule.fallback_refs = rest.iter().map(|s| (*s).to_string()).collect();
        rule
    }
}
