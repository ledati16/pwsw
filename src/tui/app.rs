//! TUI application state machine
//!
//! Manages screen navigation, user input, and application state.

use anyhow::Result;

use super::screens::{DashboardScreen, RulesScreen, SettingsScreen, SinksScreen};
use crate::config::Config;
use std::sync::Arc;

// Type aliases to reduce complex type signatures for TUI preview channel
type CompiledRegex = Arc<regex::Regex>;
type PreviewInMsg = (
    String,
    Option<String>,
    Option<CompiledRegex>,
    Option<CompiledRegex>,
);

/// Active screen in the TUI
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Dashboard,
    Sinks,
    Rules,
    Settings,
}

impl Screen {
    /// Get all available screens in display order
    pub fn all() -> &'static [Screen] {
        &[
            Screen::Dashboard,
            Screen::Sinks,
            Screen::Rules,
            Screen::Settings,
        ]
    }

    /// Get the display name for this screen
    pub const fn name(self) -> &'static str {
        match self {
            Screen::Dashboard => "Dashboard",
            Screen::Sinks => "Sinks",
            Screen::Rules => "Rules",
            Screen::Settings => "Settings",
        }
    }

    /// Get the keyboard shortcut key for this screen
    pub const fn key(self) -> char {
        match self {
            Screen::Dashboard => 'd',
            Screen::Sinks => 's',
            Screen::Rules => 'r',
            Screen::Settings => 't',
        }
    }

    /// Get the next screen in the cycle
    pub fn next(self) -> Self {
        match self {
            Screen::Dashboard => Screen::Sinks,
            Screen::Sinks => Screen::Rules,
            Screen::Rules => Screen::Settings,
            Screen::Settings => Screen::Dashboard,
        }
    }

    /// Get the previous screen in the cycle
    pub fn prev(self) -> Self {
        match self {
            Screen::Dashboard => Screen::Settings,
            Screen::Sinks => Screen::Dashboard,
            Screen::Rules => Screen::Sinks,
            Screen::Settings => Screen::Rules,
        }
    }
}

/// Application state
/// Messages sent from background worker to UI
pub enum AppUpdate {
    /// Full sink data including active and profile sinks
    SinksData {
        active: Vec<crate::pipewire::ActiveSink>,
        profiles: Vec<crate::pipewire::ProfileSink>,
        names: Vec<String>, // For backwards compat
    },
    DaemonState {
        running: bool,
        windows: Vec<crate::ipc::WindowInfo>,
    },
    ActionResult(String),
    /// Live-preview started (pending)
    PreviewPending {
        app_pattern: String,
        title_pattern: Option<String>,
    },
    /// Live-preview results for the rules editor
    PreviewMatches {
        app_pattern: String,
        title_pattern: Option<String>,
        matches: Vec<String>,
        timed_out: bool,
    },
}

/// Commands sent from UI to background worker
pub enum BgCommand {
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
pub struct PreviewResult {
    pub app_pattern: String,
    pub title_pattern: Option<String>,
    pub matches: Vec<String>,
    pub timed_out: bool,
    pub pending: bool,
}

pub struct App {
    /// Channel sender to send commands to background worker (bounded, non-blocking `try_send`)
    pub bg_cmd_tx: Option<tokio::sync::mpsc::Sender<BgCommand>>,
    /// Channel receiver to accept background updates (set by run())
    pub bg_update_rx: Option<tokio::sync::mpsc::UnboundedReceiver<AppUpdate>>,
    /// Unbounded preview input sender. Input handlers push preview requests here.
    pub preview_in_tx: Option<tokio::sync::mpsc::UnboundedSender<PreviewInMsg>>,

    /// Currently active screen
    pub current_screen: Screen,
    /// Whether the application should quit
    pub should_quit: bool,
    /// Configuration (loaded at startup, editable in TUI)
    pub config: Config,
    /// Status message to display (errors, confirmations)
    pub status_message: Option<String>,
    /// Last preview results from background worker
    pub preview: Option<PreviewResult>,
    /// Spinner frame index for small loading animations
    pub spinner_idx: usize,

    /// Dashboard screen state
    pub dashboard_screen: DashboardScreen,
    /// Settings screen state
    pub settings_screen: SettingsScreen,
    /// Sinks screen state
    pub sinks_screen: SinksScreen,
    /// Rules screen state
    pub rules_screen: RulesScreen,
    /// Whether config has unsaved changes
    pub config_dirty: bool,
    /// Whether to show help overlay
    pub show_help: bool,
    /// Scroll state for help overlay
    pub help_scroll_state: ratatui::widgets::TableState,
    /// Whether user requested quit (waiting for confirmation if config_dirty)
    pub confirm_quit: bool,
    /// Cached daemon running status (updated by background worker)
    pub daemon_running: bool,
    /// Cached window count (updated by background worker)
    pub window_count: usize,
    /// Cached window list for live preview (updated by background worker)
    pub windows: Vec<crate::ipc::WindowInfo>,
    /// Cached active sinks snapshot (updated by background worker)
    pub active_sinks: Vec<String>,
    /// Cached full active sinks with descriptions
    pub active_sink_list: Vec<crate::pipewire::ActiveSink>,
    /// Cached profile sinks for sink selector
    pub profile_sink_list: Vec<crate::pipewire::ProfileSink>,
    /// Pending daemon action to execute (set by input handler, executed by main loop)
    pub pending_daemon_action: Option<DaemonAction>,
    /// Whether the UI needs to be redrawn
    pub dirty: bool,
}

/// Daemon control action to execute
#[derive(Debug, Clone, Copy)]
pub enum DaemonAction {
    Start,
    Stop,
    Restart,
}

impl App {
    /// Create a new application instance
    ///
    /// # Errors
    /// Returns an error if config loading fails.
    pub fn new() -> Result<Self> {
        let config = Config::load()?;
        // bg_update channels initialized by caller (run()), set to None here
        let dashboard_screen = DashboardScreen::new();
        let settings_screen = SettingsScreen::new(&config.settings);
        let mut sinks_screen = SinksScreen::new();
        let rules_screen = RulesScreen::new();
        // Initialize sinks display cache from loaded config
        sinks_screen.update_display_descs(&config.sinks);
        Ok(Self {
            current_screen: Screen::Dashboard,
            should_quit: false,
            config,
            status_message: None,
            preview: None,
            spinner_idx: 0,
            dashboard_screen,
            settings_screen,
            sinks_screen,
            rules_screen,
            config_dirty: false,
            show_help: false,
            help_scroll_state: ratatui::widgets::TableState::default(),
            confirm_quit: false,
            daemon_running: false,
            window_count: 0,
            windows: Vec::new(),
            active_sinks: Vec::new(),
            active_sink_list: Vec::new(),
            profile_sink_list: Vec::new(),
            pending_daemon_action: None,
            bg_cmd_tx: None,
            bg_update_rx: None,
            preview_in_tx: None,
            dirty: true,
        })
    }

    /// Execute pending daemon action if any
    #[allow(dead_code)]
    pub async fn execute_pending_daemon_action(&mut self) {
        use crate::tui::daemon_control::DaemonManager;

        if let Some(action) = self.pending_daemon_action.take() {
            let daemon_manager = DaemonManager::detect();
            let action_name = match action {
                DaemonAction::Start => "Start",
                DaemonAction::Stop => "Stop",
                DaemonAction::Restart => "Restart",
            };

            // Show immediate feedback
            self.set_status(format!(
                "{}ing daemon via {}...",
                action_name,
                daemon_manager.display_name()
            ));

            // Execute the action
            let result = match action {
                DaemonAction::Start => daemon_manager.start().await,
                DaemonAction::Stop => daemon_manager.stop().await,
                DaemonAction::Restart => daemon_manager.restart().await,
            };

            match result {
                Ok(msg) => self.set_status(msg),
                Err(e) => self.set_status(format!(
                    "Failed to {} daemon: {:#}",
                    action_name.to_lowercase(),
                    e
                )),
            }
        }
    }

    /// Update cached daemon state (call before rendering)
    #[allow(dead_code)]
    pub async fn update_daemon_state(&mut self) {
        use crate::tui::daemon_control::DaemonManager;

        let daemon_manager = DaemonManager::detect();
        self.daemon_running = daemon_manager.is_running().await;

        // Fetch window list if daemon is running
        if self.daemon_running {
            if let Ok(crate::ipc::Response::Windows { windows }) =
                crate::ipc::send_request(crate::ipc::Request::ListWindows).await
            {
                self.window_count = windows.len();
                self.windows = windows;
                return;
            }
        }

        // Daemon not running or request failed
        self.window_count = 0;
        self.windows.clear();
    }

    /// Navigate to a specific screen
    pub fn goto_screen(&mut self, screen: Screen) {
        self.current_screen = screen;
        self.clear_status();
    }

    /// Navigate to the next screen
    pub fn next_screen(&mut self) {
        self.current_screen = self.current_screen.next();
        self.clear_status();
    }

    /// Navigate to the previous screen
    pub fn prev_screen(&mut self) {
        self.current_screen = self.current_screen.prev();
        self.clear_status();
    }

    /// Set a status message to display to the user
    pub fn set_status(&mut self, message: String) {
        self.status_message = Some(message);
        self.dirty = true;
    }

    /// Clear the current status message
    pub fn clear_status(&mut self) {
        self.status_message = None;
        self.dirty = true;
    }

    /// Request application quit
    pub fn quit(&mut self) {
        self.should_quit = true;
        self.dirty = true;
    }

    /// Request quit (with unsaved changes check)
    pub fn request_quit(&mut self) {
        if self.config_dirty {
            self.confirm_quit = true;
            self.set_status("Unsaved changes! Press 'q' again to quit, Esc to cancel".to_string());
        } else {
            self.should_quit = true;
            self.dirty = true;
        }
    }

    /// Confirm quit (when user presses 'q' again)
    pub fn confirm_quit_action(&mut self) {
        self.should_quit = true;
        self.dirty = true;
    }

    /// Cancel quit confirmation
    pub fn cancel_quit(&mut self) {
        self.confirm_quit = false;
        self.clear_status();
        self.dirty = true;
    }

    /// Mark config as modified
    pub fn mark_dirty(&mut self) {
        self.config_dirty = true;
        self.dirty = true;
    }

    /// Save configuration to disk
    ///
    /// # Errors
    /// Returns an error if config save fails.
    pub fn save_config(&mut self) -> Result<()> {
        self.config.save()?;
        self.config_dirty = false;
        self.set_status("Configuration saved successfully".to_string());
        Ok(())
    }
}
