//! Application state management
//!
//! Tracks active windows, rule matching, and audio sink state
//! for the daemon mode event loop.

use anyhow::Result;
use std::collections::HashMap;
use std::time::Instant;
use tracing::{debug, info, warn};

use crate::compositor::WindowEvent;
use crate::config::{Config, Rule};
use crate::notification::{get_app_icon, get_sink_icon, send_notification};
use crate::pipewire::PipeWire;

/// Error message for missing default sink (should be caught by config validation)
const BUG_NO_DEFAULT_SINK: &str =
    "BUG: No default sink found (config validation should prevent this)";

/// Main application state for daemon mode
pub struct State {
    pub config: Config,
    pub current_sink_name: String,
    /// Tracks windows that matched rules. Entries are removed on window close.
    active_windows: HashMap<u64, ActiveWindow>,
    /// Tracks ALL currently open windows (removed on close). Used for test-rule command.
    all_windows: HashMap<u64, (String, String)>, // (app_id, title)
    /// Lookup table for fast sink description retrieval (sink name -> description)
    sink_lookup: HashMap<String, String>,
}

/// Tracked window that matched a rule
#[derive(Debug)]
pub struct ActiveWindow {
    pub sink_name: String,
    /// Description of what triggered this (e.g., "Steam Big Picture")
    pub trigger_desc: String,
    pub opened_at: Instant,
    pub rule_index: usize,
    pub app_id: String,
    pub title: String,
}

impl State {
    /// Create new state, querying current default sink from `PipeWire`
    ///
    /// # Errors
    /// Returns an error if `PipeWire` query fails (non-fatal, uses configured default).
    ///
    /// # Panics
    /// Panics if no default sink is configured (should be prevented by config validation).
    pub fn new(config: Config) -> Result<Self> {
        let current_sink_name = PipeWire::get_default_sink_name().unwrap_or_else(|e| {
            warn!(
                "Could not query default sink: {}. Using configured default.",
                e
            );
            config
                .get_default_sink()
                .expect(BUG_NO_DEFAULT_SINK)
                .name
                .clone()
        });

        info!("Current default sink: {}", current_sink_name);

        // Build sink lookup table for O(1) description retrieval
        let sink_lookup = config
            .sinks
            .iter()
            .map(|s| (s.name.clone(), s.desc.clone()))
            .collect();

        Ok(Self {
            config,
            current_sink_name,
            active_windows: HashMap::new(),
            all_windows: HashMap::new(),
            sink_lookup,
        })
    }

    /// Create a State for testing without `PipeWire` dependency
    #[cfg(test)]
    pub(crate) fn new_for_testing(config: Config, current_sink_name: String) -> Self {
        // Build sink lookup table for O(1) description retrieval
        let sink_lookup = config
            .sinks
            .iter()
            .map(|s| (s.name.clone(), s.desc.clone()))
            .collect();

        Self {
            config,
            current_sink_name,
            active_windows: HashMap::new(),
            all_windows: HashMap::new(),
            sink_lookup,
        }
    }

    /// Reload configuration at runtime
    pub fn reload_config(&mut self, new_config: Config) {
        info!("Reloading configuration...");
        self.config = new_config;

        // Rebuild sink lookup table
        self.sink_lookup = self
            .config
            .sinks
            .iter()
            .map(|s| (s.name.clone(), s.desc.clone()))
            .collect();

        info!(
            "Configuration reloaded: {} sinks, {} rules",
            self.config.sinks.len(),
            self.config.rules.len()
        );
    }

    /// Find a rule that matches the given `app_id` and title, returning the rule and its index
    #[must_use]
    pub fn find_matching_rule(&self, app_id: &str, title: &str) -> Option<(usize, &Rule)> {
        self.config.rules.iter().enumerate().find(|(_, rule)| {
            rule.app_id_regex.is_match(app_id)
                && rule
                    .title_regex
                    .as_ref()
                    .map_or(true, |r| r.is_match(title))
        })
    }

    /// Check if switching to a new sink is needed
    #[must_use]
    pub fn should_switch_sink(&self, new_sink_name: &str) -> bool {
        self.current_sink_name != new_sink_name
    }

    /// Update the current sink name in state
    pub fn update_sink(&mut self, new_sink_name: String) {
        debug!("State: {} → {}", self.current_sink_name, new_sink_name);
        self.current_sink_name = new_sink_name;
    }

    /// Determine target sink based on active windows
    ///
    /// Priority depends on `match_by_index` setting:
    /// - `false` (default): Most recently opened window wins
    /// - `true`: Lowest rule index (highest priority) wins, with most recent as tiebreaker
    ///
    /// # Panics
    /// Panics if no default sink is configured (should be prevented by config validation).
    #[must_use]
    pub fn determine_target_sink(&self) -> String {
        let winner = if self.config.settings.match_by_index {
            // Index-based priority: lower index = higher priority
            // Tiebreaker: most recent window wins when rule indices are equal
            self.active_windows.iter().min_by(|(_, a), (_, b)| {
                a.rule_index
                    .cmp(&b.rule_index)
                    .then_with(|| b.opened_at.cmp(&a.opened_at))
            })
        } else {
            // Time-based priority: most recent window wins
            self.active_windows.iter().max_by_key(|(_, w)| w.opened_at)
        };

        winner.map_or_else(
            || {
                self.config
                    .get_default_sink()
                    .expect(BUG_NO_DEFAULT_SINK)
                    .name
                    .clone()
            },
            |(_, w)| w.sink_name.clone(),
        )
    }

    /// Check if a window is currently tracked
    #[must_use]
    pub fn is_window_tracked(&self, id: u64) -> bool {
        self.active_windows.contains_key(&id)
    }

    /// Track a new window
    pub fn track_window(
        &mut self,
        id: u64,
        sink_name: String,
        trigger_desc: String,
        rule_index: usize,
        app_id: String,
        title: String,
    ) {
        self.active_windows.insert(
            id,
            ActiveWindow {
                sink_name,
                trigger_desc,
                opened_at: Instant::now(),
                rule_index,
                app_id,
                title,
            },
        );
    }

    /// Remove a tracked window, returning its info if it existed
    pub fn untrack_window(&mut self, id: u64) -> Option<ActiveWindow> {
        self.active_windows.remove(&id)
    }

    /// Process a window event from the compositor
    ///
    /// # Errors
    /// Returns an error if sink activation fails or rule processing encounters issues.
    pub async fn process_event(&mut self, event: WindowEvent) -> Result<()> {
        match event {
            WindowEvent::Opened { id, app_id, title }
            | WindowEvent::Changed { id, app_id, title } => {
                self.handle_window_open_or_change(id, &app_id, &title)
                    .await?;
            }
            WindowEvent::Closed { id } => {
                self.handle_window_close(id).await?;
            }
        }
        Ok(())
    }

    async fn handle_window_open_or_change(
        &mut self,
        id: u64,
        app_id: &str,
        title: &str,
    ) -> Result<()> {
        debug!("Window: id={}, app_id='{}', title='{}'", id, app_id, title);

        // Track all windows for test-rule command
        self.all_windows
            .insert(id, (app_id.to_string(), title.to_string()));

        // Extract rule data before mutating state (borrow checker)
        let matched = if let Some((rule_index, rule)) = self.find_matching_rule(app_id, title) {
            let sink = self.config.resolve_sink(&rule.sink_ref)
                .ok_or_else(|| anyhow::anyhow!(
                    "BUG: Rule references non-existent sink '{}' (should have been caught in config validation)",
                    rule.sink_ref
                ))?;
            // Use rule desc if set, otherwise use window title
            let trigger = rule.desc.clone().unwrap_or_else(|| title.to_string());
            Some((
                sink.name.clone(),
                sink.desc.clone(),
                trigger,
                rule.notify,
                rule_index,
            ))
        } else {
            None
        };

        let was_tracked = self.is_window_tracked(id);

        if let Some((sink_name, sink_desc, trigger_desc, rule_notify, rule_index)) = matched {
            info!("Rule matched: '{}' → {}", app_id, sink_desc);

            // Only update opened_at for new windows, preserve original time for existing
            #[allow(clippy::if_not_else)]
            // More readable: new window (track) is main path, update is edge case
            if !was_tracked {
                self.track_window(
                    id,
                    sink_name.clone(),
                    trigger_desc.clone(),
                    rule_index,
                    app_id.to_string(),
                    title.to_string(),
                );

                if self.should_switch_sink(&sink_name) {
                    let notify = rule_notify.unwrap_or(self.config.settings.notify_rules);
                    // Use `app_id` as icon (e.g., "steam" shows Steam icon)
                    let app_icon = get_app_icon(app_id);
                    // Run blocking PipeWire activation in spawn_blocking to avoid blocking tokio runtime
                    let sink_to_activate = sink_name.clone();
                    let desc_clone = sink_desc.clone();
                    let app_icon_clone = app_icon.clone();
                    let custom_desc = trigger_desc.clone();

                    let app_icon_str = app_icon_clone.clone();
                    let join = tokio::task::spawn_blocking(move || {
                        crate::state::switch_audio_blocking(
                            &sink_to_activate,
                            &desc_clone,
                            Some(&custom_desc),
                            Some(app_icon_str.as_str()),
                            notify,
                        )
                    });

                    let inner = join
                        .await
                        .map_err(|e| anyhow::anyhow!("Join error: {e:#}"))?;
                    inner?;

                    // Only update state on success
                    self.update_sink(sink_name);
                }
            } else {
                // Window is already tracked and still matches - update app_id and title in case they changed
                // but preserve opened_at to maintain priority ordering
                if let Some(window) = self.active_windows.get_mut(&id) {
                    window.app_id.clone_from(&app_id.to_string());
                    window.title.clone_from(&title.to_string());
                }
            }
        } else if was_tracked {
            // Window was tracked but no longer matches (e.g., title changed)
            if let Some(old_window) = self.untrack_window(id) {
                info!(
                    "⚠️  UNTRACKED: Window {} no longer matches rule (was: '{}', now: app_id='{}' title='{}')",
                    id, old_window.trigger_desc, app_id, title
                );

                let target = self.determine_target_sink();
                if self.should_switch_sink(&target) {
                    let mut context = old_window.trigger_desc.clone();
                    context.push_str(" ended");
                    self.switch_to_target(target, &context).await?;
                }
            }
        }

        Ok(())
    }

    async fn handle_window_close(&mut self, id: u64) -> Result<()> {
        // Remove from all_windows tracking
        self.all_windows.remove(&id);

        if let Some(closed_window) = self.untrack_window(id) {
            debug!(
                "Tracked window closed: {} (was: {})",
                id, closed_window.trigger_desc
            );

            let target = self.determine_target_sink();
            if self.should_switch_sink(&target) {
                let context = format!("{} closed", closed_window.trigger_desc);
                self.switch_to_target(target, &context).await?;
            }
        }

        Ok(())
    }

    /// Get the most recent active window (for status reporting)
    #[must_use]
    pub fn get_most_recent_window(&self) -> Option<&ActiveWindow> {
        self.active_windows.values().max_by_key(|w| w.opened_at)
    }

    /// Get a list of tracked windows (`app_id`, title pairs)
    #[must_use]
    pub fn get_tracked_windows(&self) -> Vec<(String, String)> {
        self.active_windows
            .values()
            .map(|w| (w.app_id.clone(), w.title.clone()))
            .collect()
    }

    /// Get a list of ALL currently open windows (for test-rule command)
    #[must_use]
    pub fn get_all_windows(&self) -> Vec<(u64, String, String)> {
        self.all_windows
            .iter()
            .map(|(id, (app_id, title))| (*id, app_id.clone(), title.clone()))
            .collect()
    }

    /// Get tracked windows with sink information (for `list-windows` command)
    #[must_use]
    pub fn get_tracked_windows_with_sinks(&self) -> Vec<(u64, String, String, String, String)> {
        // Returns: (id, `app_id`, title, `sink_name`, `sink_desc`)
        self.active_windows
            .iter()
            .map(|(id, w)| {
                let sink_desc = self
                    .sink_lookup
                    .get(&w.sink_name)
                    .map_or_else(|| w.sink_name.clone(), Clone::clone);
                (
                    *id,
                    w.app_id.clone(),
                    w.title.clone(),
                    w.sink_name.clone(),
                    sink_desc,
                )
            })
            .collect()
    }

    /// Helper to switch to target sink with notification logic for window state changes
    async fn switch_to_target(&mut self, target: String, context: &str) -> Result<()> {
        let target_sink = self.config.sinks.iter().find(|s| s.name == target);
        let desc = target_sink.map_or(target.as_str(), |s| s.desc.as_str());
        let icon = target_sink.map(get_sink_icon);
        let default_sink = self.config.get_default_sink().expect(BUG_NO_DEFAULT_SINK);
        let is_default = default_sink.name == target;
        let notify = self.config.settings.notify_rules && is_default;

        // Run blocking activation inside spawn_blocking
        let target_clone = target.clone();
        let desc_clone = desc.to_string();
        let icon_clone = icon.clone();
        let context_clone = context.to_string();

        let join = tokio::task::spawn_blocking(move || {
            crate::state::switch_audio_blocking(
                &target_clone,
                &desc_clone,
                Some(&context_clone),
                icon_clone.as_deref(),
                notify,
            )
        });

        let inner = join
            .await
            .map_err(|e| anyhow::anyhow!("Join error: {e:#}"))?;
        inner?;

        self.update_sink(target);
        Ok(())
    }
}

/// Switch audio output and optionally notify
///
/// # Errors
/// Returns an error if `PipeWire` sink activation fails.
pub fn switch_audio_blocking(
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
            Some(d) => format!("{desc} → {d}"),
            None => desc.to_string(),
        };
        if let Err(e) = send_notification("Audio Output", &message, icon) {
            warn!("Notification failed: {}", e);
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Settings, SinkConfig};
    use regex::Regex;

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

    // find_matching_rule() tests
    #[test]
    fn test_find_matching_rule_matches_app_id_only() {
        let config = make_config(
            vec![make_sink("sink1", "Sink 1", true)],
            vec![make_rule("firefox", None, "sink1")],
        );
        let state = State::new_for_testing(config, "sink1".to_string());

        let result = state.find_matching_rule("firefox", "Any Title");
        assert!(result.is_some());
        let (idx, _) = result.unwrap();
        assert_eq!(idx, 0);
    }

    #[test]
    fn test_find_matching_rule_matches_app_id_and_title() {
        let config = make_config(
            vec![make_sink("sink1", "Sink 1", true)],
            vec![make_rule("steam", Some("Big Picture"), "sink1")],
        );
        let state = State::new_for_testing(config, "sink1".to_string());

        let result = state.find_matching_rule("steam", "Steam Big Picture Mode");
        assert!(result.is_some());
    }

    #[test]
    fn test_find_matching_rule_no_match_wrong_app_id() {
        let config = make_config(
            vec![make_sink("sink1", "Sink 1", true)],
            vec![make_rule("firefox", None, "sink1")],
        );
        let state = State::new_for_testing(config, "sink1".to_string());

        let result = state.find_matching_rule("chrome", "Any Title");
        assert!(result.is_none());
    }

    #[test]
    fn test_find_matching_rule_no_match_wrong_title() {
        let config = make_config(
            vec![make_sink("sink1", "Sink 1", true)],
            vec![make_rule("steam", Some("Big Picture"), "sink1")],
        );
        let state = State::new_for_testing(config, "sink1".to_string());

        let result = state.find_matching_rule("steam", "Steam Library");
        assert!(result.is_none());
    }

    #[test]
    fn test_find_matching_rule_returns_first_match() {
        let config = make_config(
            vec![make_sink("sink1", "Sink 1", true)],
            vec![
                make_rule("firefox", None, "sink1"),
                make_rule("fire.*", None, "sink1"),
            ],
        );
        let state = State::new_for_testing(config, "sink1".to_string());

        let result = state.find_matching_rule("firefox", "Title");
        assert!(result.is_some());
        let (idx, _) = result.unwrap();
        assert_eq!(idx, 0);
    }

    #[test]
    fn test_find_matching_rule_regex_partial_match() {
        let config = make_config(
            vec![make_sink("sink1", "Sink 1", true)],
            vec![make_rule("firefox", None, "sink1")],
        );
        let state = State::new_for_testing(config, "sink1".to_string());

        let result = state.find_matching_rule("org.mozilla.firefox", "Title");
        assert!(result.is_some());
    }

    #[test]
    fn test_find_matching_rule_regex_anchored() {
        let config = make_config(
            vec![make_sink("sink1", "Sink 1", true)],
            vec![make_rule("^steam$", None, "sink1")],
        );
        let state = State::new_for_testing(config, "sink1".to_string());

        let result = state.find_matching_rule("steamapp", "Title");
        assert!(result.is_none());

        let result2 = state.find_matching_rule("steam", "Title");
        assert!(result2.is_some());
    }

    // should_switch_sink() tests
    #[test]
    fn test_should_switch_sink_different_returns_true() {
        let config = make_config(vec![make_sink("sink1", "Sink 1", true)], vec![]);
        let state = State::new_for_testing(config, "sink1".to_string());

        assert!(state.should_switch_sink("sink2"));
    }

    #[test]
    fn test_should_switch_sink_same_returns_false() {
        let config = make_config(vec![make_sink("sink1", "Sink 1", true)], vec![]);
        let state = State::new_for_testing(config, "sink1".to_string());

        assert!(!state.should_switch_sink("sink1"));
    }

    // determine_target_sink() tests
    #[test]
    fn test_determine_target_sink_empty_returns_default() {
        let config = make_config(
            vec![
                make_sink("default_sink", "Default", true),
                make_sink("other_sink", "Other", false),
            ],
            vec![],
        );
        let state = State::new_for_testing(config, "default_sink".to_string());

        assert_eq!(state.determine_target_sink(), "default_sink");
    }

    #[test]
    fn test_determine_target_sink_single_window() {
        let config = make_config(
            vec![
                make_sink("default_sink", "Default", true),
                make_sink("firefox_sink", "Firefox", false),
            ],
            vec![make_rule("firefox", None, "firefox_sink")],
        );
        let mut state = State::new_for_testing(config, "default_sink".to_string());

        // Track a window
        state.track_window(
            1,
            "firefox_sink".to_string(),
            "Firefox".to_string(),
            0,
            "firefox".to_string(),
            "Browser".to_string(),
        );

        assert_eq!(state.determine_target_sink(), "firefox_sink");
    }

    #[test]
    fn test_determine_target_sink_most_recent_wins() {
        let config = make_config(
            vec![
                make_sink("default_sink", "Default", true),
                make_sink("firefox_sink", "Firefox", false),
                make_sink("mpv_sink", "MPV", false),
            ],
            vec![
                make_rule("firefox", None, "firefox_sink"),
                make_rule("mpv", None, "mpv_sink"),
            ],
        );
        let mut state = State::new_for_testing(config, "default_sink".to_string());
        state.config.settings.match_by_index = false;

        // Track firefox first
        state.track_window(
            1,
            "firefox_sink".to_string(),
            "Firefox".to_string(),
            0,
            "firefox".to_string(),
            "Browser".to_string(),
        );

        // Sleep briefly to ensure different timestamps
        std::thread::sleep(std::time::Duration::from_millis(10));

        // Track mpv second (more recent)
        state.track_window(
            2,
            "mpv_sink".to_string(),
            "MPV".to_string(),
            1,
            "mpv".to_string(),
            "Video".to_string(),
        );

        // Most recent (mpv) should win
        assert_eq!(state.determine_target_sink(), "mpv_sink");
    }
}
