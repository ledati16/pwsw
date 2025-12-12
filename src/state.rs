//! Application state management
//!
//! Tracks active windows, rule matching, and audio sink state
//! for the daemon mode event loop.

use anyhow::Result;
use std::collections::HashMap;
use std::time::Instant;
use tracing::{debug, info, warn};

use crate::config::{Config, Rule};
use crate::notification::{get_app_icon, get_notification_sink_icon, send_notification};
use crate::pipewire::PipeWire;
use crate::compositor::WindowEvent;

/// Error message for missing default sink (should be caught by config validation)
const BUG_NO_DEFAULT_SINK: &str = "BUG: No default sink found (config validation should prevent this)";

/// Main application state for daemon mode
pub struct State {
    pub config: Config,
    pub current_sink_name: String,
    /// Tracks windows that matched rules. Entries are removed on window close.
    active_windows: HashMap<u64, ActiveWindow>,
    /// Tracks ALL currently open windows (removed on close). Used for test-rule command.
    all_windows: HashMap<u64, (String, String)>, // (app_id, title)
}

/// Tracked window that matched a rule
#[derive(Debug)]
pub struct ActiveWindow {
    pub sink_name: String,
    /// Description of what triggered this (e.g., "Steam Big Picture")
    pub trigger_desc: String,
    pub opened_at: Instant,
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
            warn!("Could not query default sink: {}. Using configured default.", e);
            config.get_default_sink()
                .expect(BUG_NO_DEFAULT_SINK)
                .name.clone()
        });

        info!("Current default sink: {}", current_sink_name);

        Ok(Self {
            config,
            current_sink_name,
            active_windows: HashMap::new(),
            all_windows: HashMap::new(),
        })
    }

    /// Find a rule that matches the given `app_id` and title
    #[must_use]
    pub fn find_matching_rule(&self, app_id: &str, title: &str) -> Option<&Rule> {
        self.config.rules.iter().find(|rule| {
            rule.app_id_regex.is_match(app_id)
                && rule.title_regex.as_ref()
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

    /// Determine target sink based on active windows (most recent takes priority)
    ///
    /// # Panics
    /// Panics if no default sink is configured (should be prevented by config validation).
    #[must_use]
    pub fn determine_target_sink(&self) -> String {
        self.active_windows.iter()
            .max_by_key(|(_, w)| w.opened_at)
            .map_or_else(|| {
                self.config.get_default_sink()
                    .expect(BUG_NO_DEFAULT_SINK)
                    .name.clone()
            }, |(_, w)| w.sink_name.clone())
    }

    /// Check if a window is currently tracked
    #[must_use]
    pub fn is_window_tracked(&self, id: u64) -> bool {
        self.active_windows.contains_key(&id)
    }

    /// Track a new window
    pub fn track_window(&mut self, id: u64, sink_name: String, trigger_desc: String, app_id: String, title: String) {
        self.active_windows.insert(id, ActiveWindow {
            sink_name,
            trigger_desc,
            opened_at: Instant::now(),
            app_id,
            title,
        });
    }

    /// Remove a tracked window, returning its info if it existed
    pub fn untrack_window(&mut self, id: u64) -> Option<ActiveWindow> {
        self.active_windows.remove(&id)
    }

    /// Process a window event from the compositor
    ///
    /// # Errors
    /// Returns an error if sink activation fails or rule processing encounters issues.
    pub fn process_event(&mut self, event: WindowEvent) -> Result<()> {
        match event {
            WindowEvent::Opened { id, app_id, title } |
            WindowEvent::Changed { id, app_id, title } => {
                self.handle_window_open_or_change(id, &app_id, &title)?;
            }
            WindowEvent::Closed { id } => {
                self.handle_window_close(id)?;
            }
        }
        Ok(())
    }

    fn handle_window_open_or_change(&mut self, id: u64, app_id: &str, title: &str) -> Result<()> {
        debug!("Window: id={}, app_id='{}', title='{}'", id, app_id, title);

        // Track all windows for test-rule command
        self.all_windows.insert(id, (app_id.to_string(), title.to_string()));

        // Extract rule data before mutating state (borrow checker)
        let matched = if let Some(rule) = self.find_matching_rule(app_id, title) {
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
            ))
        } else {
            None
        };

        let was_tracked = self.is_window_tracked(id);

        if let Some((sink_name, sink_desc, trigger_desc, rule_notify)) = matched {
            info!("Rule matched: '{}' → {}", app_id, sink_desc);

            // Only update opened_at for new windows, preserve original time for existing
            if !was_tracked {
                self.track_window(id, sink_name.clone(), trigger_desc.clone(), app_id.to_string(), title.to_string());

                if self.should_switch_sink(&sink_name) {
                    let notify = self.config.should_notify_switch(rule_notify);
                    // Use `app_id` as icon (e.g., "steam" shows Steam icon)
                    let app_icon = get_app_icon(app_id);
                    switch_audio(&sink_name, &sink_desc, Some(&trigger_desc), Some(&app_icon), notify)?;
                    self.update_sink(sink_name);
                }
            }
            // If already tracked and still matches, do nothing (keep original opened_at)
        } else if was_tracked {
            // Window was tracked but no longer matches (e.g., title changed)
            if let Some(old_window) = self.untrack_window(id) {
                debug!("Window no longer matches rule: {} (was: {})", id, old_window.trigger_desc);

                let target = self.determine_target_sink();
                if self.should_switch_sink(&target) {
                    let context = format!("{} ended", old_window.trigger_desc);
                    self.switch_to_target(target, &context)?;
                }
            }
        }

        Ok(())
    }

    fn handle_window_close(&mut self, id: u64) -> Result<()> {
        // Remove from all_windows tracking
        self.all_windows.remove(&id);

        if let Some(closed_window) = self.untrack_window(id) {
            debug!("Tracked window closed: {} (was: {})", id, closed_window.trigger_desc);

            let target = self.determine_target_sink();
            if self.should_switch_sink(&target) {
                let context = format!("{} closed", closed_window.trigger_desc);
                self.switch_to_target(target, &context)?;
            }
        }

        Ok(())
    }
    
    /// Get the most recent active window (for status reporting)
    #[must_use]
    pub fn get_most_recent_window(&self) -> Option<&ActiveWindow> {
        self.active_windows.values()
            .max_by_key(|w| w.opened_at)
    }
    
    /// Get a list of tracked windows (`app_id`, title pairs)
    #[must_use]
    pub fn get_tracked_windows(&self) -> Vec<(String, String)> {
        self.active_windows.values()
            .map(|w| (w.app_id.clone(), w.title.clone()))
            .collect()
    }

    /// Get a list of ALL currently open windows (for test-rule command)
    #[must_use]
    pub fn get_all_windows(&self) -> Vec<(String, String)> {
        self.all_windows.values()
            .map(|(app_id, title)| (app_id.clone(), title.clone()))
            .collect()
    }

    /// Get tracked windows with sink information (for `list-windows` command)
    #[must_use]
    pub fn get_tracked_windows_with_sinks(&self) -> Vec<(String, String, String, String)> {
        // Returns: (`app_id`, title, `sink_name`, `sink_desc`)
        self.active_windows.values()
            .map(|w| {
                let sink_desc = self.config.sinks.iter()
                    .find(|s| s.name == w.sink_name)
                    .map_or_else(|| w.sink_name.clone(), |s| s.desc.clone());
                (w.app_id.clone(), w.title.clone(), w.sink_name.clone(), sink_desc)
            })
            .collect()
    }

    /// Helper to switch to target sink with notification logic for window state changes
    fn switch_to_target(&mut self, target: String, context: &str) -> Result<()> {
        let target_sink = self.config.sinks.iter().find(|s| s.name == target);
        let desc = target_sink.map_or(target.as_str(), |s| s.desc.as_str());
        let status_bar_icons = self.config.settings.status_bar_icons;
        let icon = target_sink.map(|s| get_notification_sink_icon(s, status_bar_icons));
        let default_sink = self.config.get_default_sink()
            .expect(BUG_NO_DEFAULT_SINK);
        let is_default = default_sink.name == target;
        let notify = self.config.settings.notify_switch && is_default;

        switch_audio(&target, desc, Some(context), icon.as_deref(), notify)?;
        self.update_sink(target);
        Ok(())
    }
}

/// Switch audio output and optionally notify
///
/// # Errors
/// Returns an error if `PipeWire` sink activation fails.
pub fn switch_audio(
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
