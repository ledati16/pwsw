//! TUI application state machine
//!
//! Manages screen navigation, user input, and application state.

use anyhow::Result;
use throbber_widgets_tui::ThrobberState;

use super::screens::{DashboardScreen, RulesScreen, SettingsScreen, SinksScreen};
use crate::config::Config;
use std::sync::Arc;

// Type aliases to reduce complex type signatures for TUI preview channel
pub(crate) type CompiledRegex = Arc<regex::Regex>;
pub(crate) type PreviewInMsg = (
    String,
    Option<String>,
    Option<CompiledRegex>,
    Option<CompiledRegex>,
);

/// Active screen in the TUI
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Screen {
    Dashboard,
    Sinks,
    Rules,
    Settings,
}

/// Screen mode for context-aware UI elements
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ScreenMode {
    /// Normal list/view mode
    List,
    /// Modal/dialog/editor mode
    Modal,
}

impl Screen {
    /// Get all available screens in display order
    pub(crate) fn all() -> &'static [Screen] {
        &[
            Screen::Dashboard,
            Screen::Sinks,
            Screen::Rules,
            Screen::Settings,
        ]
    }

    /// Get the display name for this screen
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Screen::Dashboard => "Dashboard",
            Screen::Sinks => "Sinks",
            Screen::Rules => "Rules",
            Screen::Settings => "Settings",
        }
    }

    /// Get the keyboard shortcut key for this screen
    pub(crate) const fn key(self) -> char {
        match self {
            Screen::Dashboard => '1',
            Screen::Sinks => '2',
            Screen::Rules => '3',
            Screen::Settings => '4',
        }
    }

    /// Get the next screen in the cycle
    pub(crate) fn next(self) -> Self {
        match self {
            Screen::Dashboard => Screen::Sinks,
            Screen::Sinks => Screen::Rules,
            Screen::Rules => Screen::Settings,
            Screen::Settings => Screen::Dashboard,
        }
    }

    /// Get the previous screen in the cycle
    pub(crate) fn prev(self) -> Self {
        match self {
            Screen::Dashboard => Screen::Settings,
            Screen::Sinks => Screen::Dashboard,
            Screen::Rules => Screen::Sinks,
            Screen::Settings => Screen::Rules,
        }
    }
}

/// Messages sent from background worker to UI
pub(crate) enum AppUpdate {
    /// Full sink data including active and profile sinks
    ///
    /// Sent by background poller every 1s with current PipeWire state snapshot.
    SinksData {
        active: Vec<crate::pipewire::ActiveSink>,
        profiles: Vec<crate::pipewire::ProfileSink>,
        names: Vec<String>, // For backwards compat
    },
    /// Daemon state snapshot including running status, tracked windows, and manager info
    ///
    /// Sent by background poller every 1s and immediately after daemon control actions
    /// (start/stop/restart/enable/disable) complete.
    DaemonState {
        running: bool,
        windows: Vec<crate::ipc::WindowInfo>,
        daemon_manager: Option<crate::daemon_manager::DaemonManager>, // None if daemon not running
        service_enabled: Option<bool>, // None for direct mode, Some(bool) for systemd
    },
    /// Result message from daemon control action (start/stop/restart/enable/disable)
    ///
    /// Sent immediately after a daemon action completes, containing success or error message.
    ActionResult(String),
    /// Live-preview started (pending)
    ///
    /// Sent by background preview worker when a new preview request begins execution.
    /// Used to show loading state in the rules editor.
    PreviewPending {
        app_pattern: String,
        title_pattern: Option<String>,
    },
    /// Live-preview results for the rules editor
    ///
    /// Sent by background preview worker when matching completes (or times out).
    /// Contains up to max_results matching windows.
    PreviewMatches {
        app_pattern: String,
        title_pattern: Option<String>,
        matches: Vec<String>,
        timed_out: bool,
    },
    /// New daemon log lines
    ///
    /// Sent by log tail worker when new lines are appended to daemon log file.
    /// Used to update the dashboard log viewer in real-time.
    DaemonLogs(Vec<String>),
}

/// Commands sent from UI to background worker
#[derive(Debug)]
pub(crate) enum BgCommand {
    DaemonAction(DaemonAction),
    /// Request a live-preview match for given patterns. Optionally include compiled regex caches.
    PreviewRequest {
        app_pattern: String,
        title_pattern: Option<String>,
        compiled_app: Option<std::sync::Arc<regex::Regex>>,
        compiled_title: Option<std::sync::Arc<regex::Regex>>,
    },
}

/// Preview result stored in app state
#[derive(Clone)]
pub(crate) struct PreviewResult {
    pub(crate) app_pattern: String,
    pub(crate) title_pattern: Option<String>,
    pub(crate) matches: Vec<String>,
    pub(crate) timed_out: bool,
    pub(crate) pending: bool,
}

// TUI state with multiple independent boolean flags for UI state tracking
#[allow(clippy::struct_excessive_bools)]
pub(crate) struct App {
    /// Channel sender to send commands to background worker (bounded, non-blocking `try_send`)
    pub(crate) bg_cmd_tx: Option<tokio::sync::mpsc::Sender<BgCommand>>,
    /// Channel receiver to accept background updates (set by `run()`)
    pub(crate) bg_update_rx: Option<tokio::sync::mpsc::UnboundedReceiver<AppUpdate>>,
    /// Unbounded preview input sender. Input handlers push preview requests here.
    pub(crate) preview_in_tx: Option<tokio::sync::mpsc::UnboundedSender<PreviewInMsg>>,

    /// Currently active screen
    pub(crate) current_screen: Screen,
    /// Whether the application should quit
    pub(crate) should_quit: bool,
    /// Configuration (loaded at startup, editable in TUI)
    pub(crate) config: Config,
    /// Status message to display (errors, confirmations)
    status_message: Option<String>,
    /// Last preview results from background worker
    pub(crate) preview: Option<PreviewResult>,
    /// State for throbber animation
    throbber_state: ThrobberState,
    /// Dashboard screen state
    pub(crate) dashboard_screen: DashboardScreen,
    /// Settings screen state
    pub(crate) settings_screen: SettingsScreen,
    /// Sinks screen state
    pub(crate) sinks_screen: SinksScreen,
    /// Rules screen state
    pub(crate) rules_screen: RulesScreen,
    /// Whether config has unsaved changes
    pub(crate) config_dirty: bool,
    /// Whether to show help overlay
    pub(crate) show_help: bool,
    /// Scroll state for help overlay
    pub(crate) help_scroll_state: ratatui::widgets::TableState,
    /// Viewport height for help overlay (updated during rendering)
    pub(crate) help_viewport_height: usize,
    /// Collapsed sections in help overlay (section names)
    pub(crate) help_collapsed_sections: std::collections::HashSet<String>,
    /// Whether user requested quit (waiting for confirmation if `config_dirty`)
    pub(crate) confirm_quit: bool,
    /// Cached daemon running status (updated by background worker)
    pub(crate) daemon_running: bool,
    /// Cached window count (updated by background worker)
    pub(crate) window_count: usize,
    /// Cached window list for live preview (updated by background worker)
    pub(crate) windows: Vec<crate::ipc::WindowInfo>,
    /// Whether a daemon action (start/stop/restart) is pending
    pub(crate) daemon_action_pending: bool,
    /// Cached active sinks snapshot (updated by background worker)
    pub(crate) active_sinks: Vec<String>,
    /// Cached full active sinks with descriptions
    pub(crate) active_sink_list: Vec<crate::pipewire::ActiveSink>,
    /// Cached profile sinks for sink selector
    pub(crate) profile_sink_list: Vec<crate::pipewire::ProfileSink>,
    /// Daemon log lines (tailed from log file)
    pub(crate) daemon_log_lines: Vec<String>,

    /// Whether the UI needs to be redrawn
    pub(crate) dirty: bool,
}

/// Daemon control action to execute
#[derive(Debug, Clone, Copy)]
pub(crate) enum DaemonAction {
    Start,
    Stop,
    Restart,
    Enable,
    Disable,
}

impl App {
    /// Create a new application instance with a pre-loaded config
    #[must_use]
    pub(crate) fn with_config(config: Config) -> Self {
        // bg_update channels initialized by caller (run()), set to None here
        let dashboard_screen = DashboardScreen::new();
        let settings_screen = SettingsScreen::new(&config.settings);
        let mut sinks_screen = SinksScreen::new();
        let rules_screen = RulesScreen::new();
        // Initialize sinks display cache from loaded config
        sinks_screen.update_display_descs(&config.sinks);
        Self {
            current_screen: Screen::Dashboard,
            should_quit: false,
            config,
            status_message: None,
            preview: None,
            throbber_state: ThrobberState::default(),
            dashboard_screen,
            settings_screen,
            sinks_screen,
            rules_screen,
            config_dirty: false,
            show_help: false,
            help_scroll_state: ratatui::widgets::TableState::default(),
            help_viewport_height: 30, // Default value, updated during first render
            help_collapsed_sections: std::collections::HashSet::new(),
            confirm_quit: false,
            daemon_running: false,
            window_count: 0,
            windows: Vec::new(),
            active_sinks: Vec::new(),
            active_sink_list: Vec::new(),
            profile_sink_list: Vec::new(),
            daemon_action_pending: false,
            daemon_log_lines: Vec::new(),

            bg_cmd_tx: None,
            bg_update_rx: None,
            preview_in_tx: None,
            dirty: true,
        }
    }

    /// Navigate to a specific screen
    pub(crate) fn goto_screen(&mut self, screen: Screen) {
        self.current_screen = screen;
        self.clear_status();
    }

    /// Navigate to the next screen
    pub(crate) fn next_screen(&mut self) {
        self.current_screen = self.current_screen.next();
        self.clear_status();
    }

    /// Navigate to the previous screen
    pub(crate) fn prev_screen(&mut self) {
        self.current_screen = self.current_screen.prev();
        self.clear_status();
    }

    /// Get the current screen mode (list vs modal)
    pub(crate) fn get_screen_mode(&self) -> ScreenMode {
        use super::screens::rules::RulesMode;
        use super::screens::sinks::SinksMode;

        match self.current_screen {
            Screen::Dashboard => ScreenMode::List,
            Screen::Sinks => {
                if self.sinks_screen.mode == SinksMode::List {
                    ScreenMode::List
                } else {
                    ScreenMode::Modal
                }
            }
            Screen::Rules => {
                if self.rules_screen.mode == RulesMode::List {
                    ScreenMode::List
                } else {
                    ScreenMode::Modal
                }
            }
            Screen::Settings => {
                if self.settings_screen.editing_log_level {
                    ScreenMode::Modal
                } else {
                    ScreenMode::List
                }
            }
        }
    }

    /// Set a status message to display to the user
    pub(crate) fn set_status(&mut self, message: String) {
        self.status_message = Some(message);
        self.dirty = true;
    }

    /// Clear the current status message
    pub(crate) fn clear_status(&mut self) {
        self.status_message = None;
        self.dirty = true;
    }

    /// Read-only accessor for status message
    pub(crate) fn status_message(&self) -> Option<&String> {
        self.status_message.as_ref()
    }

    /// Mutable accessor for `throbber_state` (keeps field private)
    ///
    /// # Panics
    /// Never panics - returns mutable reference to owned field
    pub(crate) fn throbber_state_mut(&mut self) -> &mut ThrobberState {
        &mut self.throbber_state
    }

    /// Borrow mutable references to rules screen and throbber together
    ///
    /// This helper avoids borrow-checker conflicts when rendering rules
    /// that need both mutable state (throbber) and screen state.
    ///
    /// # Panics
    /// Never panics - returns mutable references to owned fields
    pub(crate) fn borrow_rules_and_throbber(&mut self) -> (&mut RulesScreen, &mut ThrobberState) {
        (&mut self.rules_screen, &mut self.throbber_state)
    }

    /// Set preview result
    pub(crate) fn set_preview(&mut self, pr: PreviewResult) {
        self.preview = Some(pr);
        self.dirty = true;
    }

    /// Request application quit
    pub(crate) fn quit(&mut self) {
        self.should_quit = true;
        self.dirty = true;
    }

    /// Request quit (with unsaved changes check)
    pub(crate) fn request_quit(&mut self) {
        if self.config_dirty {
            self.confirm_quit = true;
            self.set_status("Unsaved changes! Press 'q' again to quit, Esc to cancel".to_string());
        } else {
            self.should_quit = true;
            self.dirty = true;
        }
    }

    /// Confirm quit (when user presses 'q' again)
    pub(crate) fn confirm_quit_action(&mut self) {
        self.should_quit = true;
        self.dirty = true;
    }

    /// Cancel quit confirmation
    pub(crate) fn cancel_quit(&mut self) {
        self.confirm_quit = false;
        self.clear_status();
        self.dirty = true;
    }

    /// Mark config as modified
    pub(crate) fn mark_dirty(&mut self) {
        self.config_dirty = true;
        self.dirty = true;
    }

    /// Save configuration to disk
    ///
    /// # Errors
    /// Returns an error if config save fails.
    pub(crate) fn save_config(&mut self) -> Result<()> {
        self.config.save()?;
        self.config_dirty = false;
        self.set_status("Configuration saved successfully".to_string());
        Ok(())
    }
}
