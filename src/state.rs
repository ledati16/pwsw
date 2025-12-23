//! Application state management
//!
//! Tracks active windows, rule matching, and audio sink state
//! for the daemon mode event loop.

use color_eyre::eyre::{self, Result};
use std::collections::HashMap;
use std::sync::Arc;
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
    pub config: Arc<Config>,
    pub current_sink_name: String,
    pub daemon_manager: crate::daemon_manager::DaemonManager,
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
    pub fn new(
        config: Arc<Config>,
        daemon_manager: crate::daemon_manager::DaemonManager,
    ) -> Result<Self> {
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
            daemon_manager,
            active_windows: HashMap::new(),
            all_windows: HashMap::new(),
            sink_lookup,
        })
    }

    /// Create a State for testing without `PipeWire` dependency
    #[cfg(test)]
    pub(crate) fn new_for_testing(config: Arc<Config>, current_sink_name: String) -> Self {
        // Build sink lookup table for O(1) description retrieval
        let sink_lookup = config
            .sinks
            .iter()
            .map(|s| (s.name.clone(), s.desc.clone()))
            .collect();

        Self {
            config,
            current_sink_name,
            daemon_manager: crate::daemon_manager::DaemonManager::Direct,
            active_windows: HashMap::new(),
            all_windows: HashMap::new(),
            sink_lookup,
        }
    }

    /// Reload configuration
    pub fn reload_config(&mut self, new_config: Arc<Config>) {
        info!("Applying new configuration");
        self.config = new_config;

        // Rebuild sink lookup table
        self.sink_lookup = self
            .config
            .sinks
            .iter()
            .map(|s| (s.name.clone(), s.desc.clone()))
            .collect();
    }

    /// Re-evaluate all tracked windows against current rules
    ///
    /// # Errors
    /// Returns an error if any sink activation fails during re-evaluation.
    pub async fn reevaluate_all_windows(&mut self) -> Result<()> {
        debug!("Re-evaluating all active windows against new rules");

        // Collect all window IDs to avoid borrow issues
        let window_ids: Vec<u64> = self.all_windows.keys().copied().collect();

        for window_id in window_ids {
            // Get window info (must clone to avoid borrow conflicts)
            if let Some((app_id, title)) = self.all_windows.get(&window_id) {
                let app_id = app_id.clone();
                let title = title.clone();

                // Process as Changed event to re-evaluate rules
                self.process_event(WindowEvent::Changed {
                    id: window_id,
                    app_id,
                    title,
                })
                .await?;
            }
        }

        Ok(())
    }

    /// Find a rule that matches the given `app_id` and title, returning the rule and its index
    #[must_use]
    pub fn find_matching_rule(&self, app_id: &str, title: &str) -> Option<(usize, &Rule)> {
        self.config.rules.iter().enumerate().find(|(_, rule)| {
            rule.app_id_regex.is_match(app_id)
                && rule.title_regex.as_ref().is_none_or(|r| r.is_match(title))
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
        let window = ActiveWindow {
            sink_name,
            trigger_desc,
            opened_at: Instant::now(),
            rule_index,
            app_id,
            title,
        };
        self.active_windows.insert(id, window);
    }

    /// Untrack a window
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
                .ok_or_else(|| eyre::eyre!(
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
            // Update opened_at for new windows, preserve original time for existing
            if was_tracked {
                // Window is already tracked - check if the rule match has changed
                if let Some(window) = self.active_windows.get_mut(&id) {
                    let rule_changed =
                        window.rule_index != rule_index || window.sink_name != sink_name;

                    if rule_changed {
                        debug!(
                            "Rule changed for window {}: {} (rule {}) → {} (rule {})",
                            id, window.sink_name, window.rule_index, sink_name, rule_index
                        );
                        window.sink_name.clone_from(&sink_name);
                        window.rule_index = rule_index;
                        window.trigger_desc.clone_from(&trigger_desc);
                    }

                    // Always update app_id and title in case they changed
                    window.app_id.clone_from(&app_id.to_string());
                    window.title.clone_from(&title.to_string());

                    // If rule changed, re-evaluate target and potentially switch
                    if rule_changed && self.should_switch_sink(&self.determine_target_sink()) {
                        let target = self.determine_target_sink();
                        let notify = rule_notify.unwrap_or(self.config.settings.notify_rules);
                        let app_icon = get_app_icon(app_id);

                        // Find target sink description for notification
                        let target_sink = self.config.sinks.iter().find(|s| s.name == target);
                        let desc = target_sink.map_or(target.as_str(), |s| s.desc.as_str());

                        // Run blocking activation
                        let target_clone = target.clone();
                        let desc_clone = desc.to_string();
                        let app_icon_clone = app_icon.clone();
                        let trigger_desc_clone = trigger_desc.clone();

                        let join = tokio::task::spawn_blocking(move || {
                            crate::state::switch_audio_blocking(
                                &target_clone,
                                &desc_clone,
                                Some(&trigger_desc_clone),
                                Some(&app_icon_clone),
                                notify,
                            )
                        });

                        let inner = join.await.map_err(|e| eyre::eyre!("Join error: {e:#}"))?;
                        inner?;

                        self.update_sink(target);
                    }
                }
            } else {
                // New window match - log and track it, potentially switch sink
                info!("Rule matched: '{}' → {}", app_id, sink_desc);
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

                    let inner = join.await.map_err(|e| eyre::eyre!("Join error: {e:#}"))?;
                    inner?;

                    // Only update state on success
                    self.update_sink(sink_name);
                }
            }
        } else if was_tracked {
            // Window was tracked but no longer matches (e.g., title changed)
            if let Some(old_window) = self.untrack_window(id) {
                info!(
                    "Rule unmatched: Window {} no longer matches rule (was: '{}', now: app_id='{}' title='{}')",
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
            info!(
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

    /// Get a list of ALL currently open windows (for test-rule command)
    #[must_use]
    pub fn get_all_windows(&self) -> Vec<(u64, String, String)> {
        self.all_windows
            .iter()
            .map(|(id, (app_id, title))| (*id, app_id.clone(), title.clone()))
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

        let inner = join.await.map_err(|e| eyre::eyre!("Join error: {e:#}"))?;
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
    if let Some(reason) = custom_desc {
        info!("Switching: {} ({}) [Reason: {}]", desc, name, reason);
    } else {
        info!("Switching: {} ({})", desc, name);
    }
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
    use crate::test_utils::fixtures::{make_config, make_rule, make_sink};
    use test_case::test_case;

    // find_matching_rule() parameterized tests
    #[test_case("firefox", None, "firefox", "Any Title", true ; "matches app_id only")]
    #[test_case("steam", Some("Big Picture"), "steam", "Steam Big Picture Mode", true ; "matches app_id and title")]
    #[test_case("firefox", None, "chrome", "Any Title", false ; "no match wrong app_id")]
    #[test_case("steam", Some("Big Picture"), "steam", "Steam Library", false ; "no match wrong title")]
    #[test_case("firefox", None, "org.mozilla.firefox", "Title", true ; "regex partial match")]
    #[test_case("^steam$", None, "steamapp", "Title", false ; "regex anchored no match")]
    #[test_case("^steam$", None, "steam", "Title", true ; "regex anchored exact match")]
    fn test_find_matching_rule(
        rule_app_id: &str,
        rule_title: Option<&str>,
        test_app_id: &str,
        test_title: &str,
        should_match: bool,
    ) {
        let config = make_config(
            vec![make_sink("sink1", "Sink 1", true)],
            vec![make_rule(rule_app_id, rule_title, "sink1")],
        );
        let state = State::new_for_testing(Arc::new(config), "sink1".to_string());

        let result = state.find_matching_rule(test_app_id, test_title);
        assert_eq!(result.is_some(), should_match);
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
        let state = State::new_for_testing(Arc::new(config), "sink1".to_string());

        let result = state.find_matching_rule("firefox", "Title");
        assert!(result.is_some());
        let (idx, _) = result.unwrap();
        assert_eq!(idx, 0);
    }

    // should_switch_sink() tests
    #[test]
    fn test_should_switch_sink_different_returns_true() {
        let config = make_config(vec![make_sink("sink1", "Sink 1", true)], vec![]);
        let state = State::new_for_testing(Arc::new(config), "sink1".to_string());

        assert!(state.should_switch_sink("sink2"));
    }

    #[test]
    fn test_should_switch_sink_same_returns_false() {
        let config = make_config(vec![make_sink("sink1", "Sink 1", true)], vec![]);
        let state = State::new_for_testing(Arc::new(config), "sink1".to_string());

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
        let state = State::new_for_testing(Arc::new(config), "default_sink".to_string());

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
        let mut state = State::new_for_testing(Arc::new(config), "default_sink".to_string());

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
    fn test_determine_target_sink_priority_time() {
        let config = make_config(
            vec![
                make_sink("default_sink", "Default", true),
                make_sink("sink1", "S1", false),
                make_sink("sink2", "S2", false),
            ],
            vec![
                make_rule("app1", None, "sink1"),
                make_rule("app2", None, "sink2"),
            ],
        );
        let mut state = State::new_for_testing(Arc::new(config), "default_sink".to_string());

        // Track window 1
        state.track_window(
            1,
            "sink1".to_string(),
            "App 1".to_string(),
            0,
            "app1".to_string(),
            "T1".to_string(),
        );
        // Track window 2 later
        state.track_window(
            2,
            "sink2".to_string(),
            "App 2".to_string(),
            1,
            "app2".to_string(),
            "T2".to_string(),
        );

        // Most recent (window 2) wins
        assert_eq!(state.determine_target_sink(), "sink2");
    }

    #[test]
    fn test_determine_target_sink_priority_index() {
        let mut config = make_config(
            vec![
                make_sink("default_sink", "Default", true),
                make_sink("sink1", "S1", false),
                make_sink("sink2", "S2", false),
            ],
            vec![
                make_rule("app1", None, "sink1"),
                make_rule("app2", None, "sink2"),
            ],
        );
        config.settings.match_by_index = true;
        let mut state = State::new_for_testing(Arc::new(config), "default_sink".to_string());

        // Track window 2 first
        state.track_window(
            2,
            "sink2".to_string(),
            "App 2".to_string(),
            1,
            "app2".to_string(),
            "T2".to_string(),
        );
        // Track window 1 later (higher priority index 0)
        state.track_window(
            1,
            "sink1".to_string(),
            "App 1".to_string(),
            0,
            "app1".to_string(),
            "T1".to_string(),
        );

        // Lower index (window 1) wins despite being older
        assert_eq!(state.determine_target_sink(), "sink1");
    }

    #[test]
    fn test_determine_target_sink_priority_index_tiebreaker() {
        let mut config = make_config(
            vec![
                make_sink("default_sink", "Default", true),
                make_sink("sink1", "S1", false),
            ],
            vec![make_rule("app", None, "sink1")],
        );
        config.settings.match_by_index = true;
        let mut state = State::new_for_testing(Arc::new(config), "default_sink".to_string());

        // Two windows matching same rule (same index)
        state.track_window(
            1,
            "sink1".to_string(),
            "App A".to_string(),
            0,
            "app".to_string(),
            "T1".to_string(),
        );
        state.track_window(
            2,
            "sink1".to_string(),
            "App B".to_string(),
            0,
            "app".to_string(),
            "T2".to_string(),
        );

        // Tied index, most recent (window 2) wins
        assert_eq!(state.determine_target_sink(), "sink1");
    }

    #[tokio::test]
    async fn test_all_windows_tracking() {
        let config = make_config(
            vec![make_sink("default_sink", "Default", true)],
            vec![make_rule(".*", None, "default_sink")],
        );
        let mut state = State::new_for_testing(Arc::new(config), "default_sink".to_string());

        // Add window to all_windows (happens during Opened event)
        state
            .all_windows
            .insert(1, ("firefox".to_string(), "Browser".to_string()));

        // Verify it's there
        assert_eq!(state.all_windows.len(), 1);
        assert!(state.all_windows.contains_key(&1));

        // Close the window (should remove from all_windows)
        let close_result = state.handle_window_close(1).await;
        assert!(close_result.is_ok());

        // Verify cleanup happened
        assert_eq!(state.all_windows.len(), 0);
        assert!(!state.all_windows.contains_key(&1));
    }

    #[tokio::test]
    async fn test_all_windows_cleanup_multiple_windows() {
        let config = make_config(
            vec![make_sink("default_sink", "Default", true)],
            vec![make_rule(".*", None, "default_sink")],
        );
        let mut state = State::new_for_testing(Arc::new(config), "default_sink".to_string());

        // Add multiple windows
        state
            .all_windows
            .insert(1, ("firefox".to_string(), "Browser".to_string()));
        state
            .all_windows
            .insert(2, ("chrome".to_string(), "Browser".to_string()));
        state
            .all_windows
            .insert(3, ("mpv".to_string(), "Video".to_string()));

        assert_eq!(state.all_windows.len(), 3);

        // Close window 2
        let close_result = state.handle_window_close(2).await;
        assert!(close_result.is_ok());

        // Verify only window 2 was removed
        assert_eq!(state.all_windows.len(), 2);
        assert!(state.all_windows.contains_key(&1));
        assert!(!state.all_windows.contains_key(&2));
        assert!(state.all_windows.contains_key(&3));
    }

    #[tokio::test]
    async fn test_rule_metadata_update() {
        let config = make_config(
            vec![
                make_sink("speakers", "Speakers", true),
                make_sink("headphones", "Headphones", false),
            ],
            vec![
                make_rule("kitty", Some("Music"), "speakers"),
                make_rule("kitty", None, "headphones"),
            ],
        );
        let mut state = State::new_for_testing(Arc::new(config), "speakers".to_string());

        // 1. Initial match (Rule 1: Headphones)
        // We use track_window to avoid side effects of process_event
        state.track_window(
            1,
            "headphones".to_string(),
            "Kitty".to_string(),
            1,
            "kitty".to_string(),
            "Shell".to_string(),
        );

        {
            let window = state.active_windows.get(&1).unwrap();
            assert_eq!(window.sink_name, "headphones");
            assert_eq!(window.rule_index, 1);
        }

        // 2. Simulate property change by calling process_event
        // To avoid real PipeWire calls, we'll set current_sink_name to what we expect it to become
        // so should_switch_sink returns false.
        state.current_sink_name = "speakers".to_string();

        state
            .process_event(WindowEvent::Changed {
                id: 1,
                app_id: "kitty".to_string(),
                title: "Music".to_string(),
            })
            .await
            .unwrap();

        // 3. Verify metadata updated
        let window = state.active_windows.get(&1).unwrap();
        assert_eq!(
            window.sink_name, "speakers",
            "Sink name should update in metadata"
        );
        assert_eq!(window.rule_index, 0, "Rule index should update in metadata");
        assert_eq!(window.title, "Music");
    }
}
