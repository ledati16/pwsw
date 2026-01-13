//! TUI application state machine
//!
//! Manages screen navigation, user input, and application state.

use color_eyre::eyre::Result;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use throbber_widgets_tui::ThrobberState;

use super::screens::{DashboardScreen, RulesScreen, SettingsScreen, SinksScreen};
use crate::config::Config;
use crate::style::colors;
use std::sync::Arc;

// Type aliases to reduce complex type signatures for TUI preview channel
pub type CompiledRegex = Arc<regex::Regex>;
pub type PreviewInMsg = (
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

/// Screen mode for context-aware UI elements
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScreenMode {
    /// Normal list/view mode
    List,
    /// Modal/dialog/editor mode
    Modal,
}

impl Screen {
    /// Get all available screens in display order
    pub(crate) const fn all() -> &'static [Self] {
        &[Self::Dashboard, Self::Sinks, Self::Rules, Self::Settings]
    }

    /// Get the display name for this screen
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::Dashboard => "Dashboard",
            Self::Sinks => "Sinks",
            Self::Rules => "Rules",
            Self::Settings => "Settings",
        }
    }

    /// Get the keyboard shortcut key for this screen
    pub(crate) const fn key(self) -> char {
        match self {
            Self::Dashboard => '1',
            Self::Sinks => '2',
            Self::Rules => '3',
            Self::Settings => '4',
        }
    }

    /// Get the next screen in the cycle
    pub(crate) const fn next(self) -> Self {
        match self {
            Self::Dashboard => Self::Sinks,
            Self::Sinks => Self::Rules,
            Self::Rules => Self::Settings,
            Self::Settings => Self::Dashboard,
        }
    }

    /// Get the previous screen in the cycle
    pub(crate) const fn prev(self) -> Self {
        match self {
            Self::Dashboard => Self::Settings,
            Self::Sinks => Self::Dashboard,
            Self::Rules => Self::Sinks,
            Self::Settings => Self::Rules,
        }
    }
}

/// Messages sent from background worker to UI
pub enum AppUpdate {
    /// Full sink data including active and profile sinks
    ///
    /// Sent by background poller every 1s with current `PipeWire` state snapshot.
    SinksData {
        active: Vec<crate::pipewire::ActiveSink>,
        profiles: Vec<crate::pipewire::ProfileSink>,
        names: Vec<String>, // For backwards compat
    },
    /// `PipeWire` is unavailable (pw-dump failed)
    ///
    /// Sent by background poller when `PipeWire::dump()` fails.
    /// UI should show a warning that sink status may be stale.
    PipeWireUnavailable,
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
    /// Config save result (success or failure with message)
    ///
    /// Sent by background worker after `SaveConfig` command completes.
    /// On success, App updates `original_config` to match current config.
    ConfigSaved { success: bool, message: String },
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
    /// Contains up to `max_results` matching windows.
    PreviewMatches {
        app_pattern: String,
        title_pattern: Option<String>,
        matches: Vec<String>,
        timed_out: bool,
        regex_error: Option<String>,
    },
    /// New daemon log lines
    ///
    /// Sent by log tail worker when new lines are appended to daemon log file.
    /// Used to update the dashboard log viewer in real-time.
    DaemonLogs(Vec<String>),
}

/// Commands sent from UI to background worker
#[derive(Debug)]
pub enum BgCommand {
    DaemonAction(DaemonAction),
    /// Request an atomic config save
    SaveConfig(Config),
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
pub struct PreviewResult {
    pub(crate) app_pattern: String,
    pub(crate) title_pattern: Option<String>,
    pub(crate) matches: Vec<String>,
    pub(crate) timed_out: bool,
    pub(crate) pending: bool,
    pub(crate) regex_error: Option<String>,
}

// TUI state with multiple independent boolean flags for UI state tracking
pub struct App {
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
    /// Snapshot of config at last save (for dirty comparison)
    original_config: Config,
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
    /// Whether `PipeWire` is available (last poll succeeded)
    pub(crate) pipewire_available: bool,
    /// Daemon log lines (tailed from log file)
    pub(crate) daemon_log_lines: Vec<String>,

    /// Whether the UI needs to be redrawn
    pub(crate) dirty: bool,
}

/// Daemon control action to execute
#[derive(Debug, Clone, Copy)]
pub enum DaemonAction {
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
        // Clone config for original_config before moving it
        let original_config = config.clone();
        Self {
            current_screen: Screen::Dashboard,
            should_quit: false,
            config,
            original_config,
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
            pipewire_available: true, // Optimistic default until first poll
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

    /// Check if any text input field is currently focused
    pub(crate) fn is_input_focused(&self) -> bool {
        use super::screens::rules::RulesMode;
        use super::screens::sinks::SinksMode;

        match self.current_screen {
            Screen::Dashboard | Screen::Settings => false,
            Screen::Sinks => {
                self.sinks_screen.mode == SinksMode::AddEdit
                    && self.sinks_screen.editor.focused_field < 3 // name, desc, icon are inputs
            }
            Screen::Rules => {
                self.rules_screen.mode == RulesMode::AddEdit
                    && [0, 1, 3].contains(&self.rules_screen.editor.focused_field) // app_id, title, desc are inputs
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
    pub(crate) const fn status_message(&self) -> Option<&String> {
        self.status_message.as_ref()
    }

    /// Mutable accessor for `throbber_state` (keeps field private)
    ///
    /// # Panics
    /// Never panics - returns mutable reference to owned field
    pub(crate) const fn throbber_state_mut(&mut self) -> &mut ThrobberState {
        &mut self.throbber_state
    }

    /// Borrow mutable references to rules screen and throbber together
    ///
    /// This helper avoids borrow-checker conflicts when rendering rules
    /// that need both mutable state (throbber) and screen state.
    ///
    /// # Panics
    /// Never panics - returns mutable references to owned fields
    pub(crate) const fn borrow_rules_and_throbber(
        &mut self,
    ) -> (&mut RulesScreen, &mut ThrobberState) {
        (&mut self.rules_screen, &mut self.throbber_state)
    }

    /// Set preview result
    pub(crate) fn set_preview(&mut self, pr: PreviewResult) {
        self.preview = Some(pr);
        self.dirty = true;
    }

    /// Request application quit
    pub(crate) const fn quit(&mut self) {
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
    pub(crate) const fn confirm_quit_action(&mut self) {
        self.should_quit = true;
        self.dirty = true;
    }

    /// Cancel quit confirmation
    pub(crate) fn cancel_quit(&mut self) {
        self.confirm_quit = false;
        self.clear_status();
        self.dirty = true;
    }

    /// Reset help overlay scroll position to top
    pub(crate) const fn reset_help_scroll(&mut self) {
        self.help_scroll_state.select(Some(0));
        *self.help_scroll_state.offset_mut() = 0;
    }

    /// Mark config as potentially modified (compares to original)
    pub(crate) fn mark_dirty(&mut self) {
        self.config_dirty = self.config != self.original_config;
        self.dirty = true;
    }

    /// Save configuration to disk
    ///
    /// # Errors
    /// Returns an error if config save fails.
    pub(crate) fn save_config(&mut self) -> Result<()> {
        self.config.save()?;
        self.original_config.clone_from(&self.config);
        self.config_dirty = false;
        self.set_status("Configuration saved successfully".to_string());
        Ok(())
    }

    /// Generate the context bar text based on current app state
    pub(crate) fn context_bar_text(&self) -> Line<'static> {
        use super::screens::rules::RulesMode;
        use super::screens::sinks::SinksMode;

        let mode = self.get_screen_mode();

        // Build screen-specific content
        let base_line = match (self.current_screen, mode) {
            (Screen::Dashboard, ScreenMode::List) => {
                // Phase 9B: View-aware context bar for dashboard
                use crate::tui::screens::DashboardView;
                let mut spans = vec![
                    Span::styled("[←→]", Style::default().fg(colors::UI_HIGHLIGHT)),
                    Span::raw(" Select Action  "),
                    Span::styled("[Enter]", Style::default().fg(colors::UI_HIGHLIGHT)),
                    Span::raw(" Execute  "),
                ];

                // Add view-specific scrolling hints
                match self.dashboard_screen.current_view {
                    DashboardView::Logs => {
                        spans.push(Span::styled(
                            "[↑↓/PgUp/PgDn]",
                            Style::default().fg(colors::UI_HIGHLIGHT),
                        ));
                        spans.push(Span::raw(" Scroll Logs  "));
                        spans.push(Span::styled(
                            "[w]",
                            Style::default().fg(colors::UI_HIGHLIGHT),
                        ));
                        spans.push(Span::raw(" View Windows"));
                    }
                    DashboardView::Windows => {
                        spans.push(Span::styled(
                            "[PgUp/PgDn]",
                            Style::default().fg(colors::UI_HIGHLIGHT),
                        ));
                        spans.push(Span::raw(" Scroll Windows  "));
                        spans.push(Span::styled(
                            "[w]",
                            Style::default().fg(colors::UI_HIGHLIGHT),
                        ));
                        spans.push(Span::raw(" View Logs"));
                    }
                }

                Line::from(spans)
            }
            (Screen::Sinks, ScreenMode::List) => Line::from(vec![
                Span::raw("↑↓ Navigate  "),
                Span::styled("[Shift+↑↓]", Style::default().fg(colors::UI_HIGHLIGHT)),
                Span::raw(" Reorder  "),
                Span::styled("[a]", Style::default().fg(colors::UI_HIGHLIGHT)),
                Span::raw(" Add  "),
                Span::styled("[e]", Style::default().fg(colors::UI_HIGHLIGHT)),
                Span::raw(" Edit  "),
                Span::styled("[x]", Style::default().fg(colors::UI_HIGHLIGHT)),
                Span::raw(" Delete  "),
                Span::styled("[Space]", Style::default().fg(colors::UI_HIGHLIGHT)),
                Span::raw(" Default  "),
                Span::styled("[Enter]", Style::default().fg(colors::UI_HIGHLIGHT)),
                Span::raw(" Inspect"),
            ]),
            (Screen::Sinks, ScreenMode::Modal) => {
                // Determine which modal we're in
                match self.sinks_screen.mode {
                    SinksMode::AddEdit => Line::from(vec![
                        Span::raw("↑↓/"),
                        Span::styled("Tab", Style::default().fg(colors::UI_HIGHLIGHT)),
                        Span::raw(" Switch field  "),
                        Span::styled("[Space]", Style::default().fg(colors::UI_HIGHLIGHT)),
                        Span::raw(" Select  "),
                        Span::styled("[Enter]", Style::default().fg(colors::UI_HIGHLIGHT)),
                        Span::raw(" Save  "),
                        Span::styled("[Esc]", Style::default().fg(colors::UI_HIGHLIGHT)),
                        Span::raw(" Cancel"),
                    ]),
                    SinksMode::Delete => Line::from(vec![
                        Span::styled("[Enter]", Style::default().fg(colors::UI_HIGHLIGHT)),
                        Span::raw(" Confirm  "),
                        Span::styled("[Esc]", Style::default().fg(colors::UI_HIGHLIGHT)),
                        Span::raw(" Cancel"),
                    ]),
                    SinksMode::SelectSink => Line::from(vec![
                        Span::raw("↑↓ Navigate  "),
                        Span::styled("[Enter]", Style::default().fg(colors::UI_HIGHLIGHT)),
                        Span::raw(" Select  "),
                        Span::styled("[Esc]", Style::default().fg(colors::UI_HIGHLIGHT)),
                        Span::raw(" Cancel"),
                    ]),
                    SinksMode::Inspect => Line::from(vec![
                        Span::styled("[Enter/Esc]", Style::default().fg(colors::UI_HIGHLIGHT)),
                        Span::raw(" Close Details"),
                    ]),
                    SinksMode::List => Line::from(""), // Should not happen in Modal mode
                }
            }
            (Screen::Rules, ScreenMode::List) => Line::from(vec![
                Span::raw("↑↓ Navigate  "),
                Span::styled("[Shift+↑↓]", Style::default().fg(colors::UI_HIGHLIGHT)),
                Span::raw(" Reorder  "),
                Span::styled("[a]", Style::default().fg(colors::UI_HIGHLIGHT)),
                Span::raw(" Add  "),
                Span::styled("[e]", Style::default().fg(colors::UI_HIGHLIGHT)),
                Span::raw(" Edit  "),
                Span::styled("[x]", Style::default().fg(colors::UI_HIGHLIGHT)),
                Span::raw(" Delete  "),
                Span::styled("[Enter]", Style::default().fg(colors::UI_HIGHLIGHT)),
                Span::raw(" Inspect"),
            ]),
            (Screen::Rules, ScreenMode::Modal) => {
                // Determine which modal we're in
                match self.rules_screen.mode {
                    RulesMode::AddEdit => {
                        let mut spans = vec![
                            Span::raw("↑↓/"),
                            Span::styled("Tab", Style::default().fg(colors::UI_HIGHLIGHT)),
                            Span::raw(" Switch field  "),
                            Span::styled("[Space]", Style::default().fg(colors::UI_HIGHLIGHT)),
                            Span::raw(" Select  "),
                            Span::styled("[Enter]", Style::default().fg(colors::UI_HIGHLIGHT)),
                            Span::raw(" Save  "),
                            Span::styled("[Esc]", Style::default().fg(colors::UI_HIGHLIGHT)),
                            Span::raw(" Cancel"),
                        ];
                        // Show live preview indicator if preview is active
                        if self.preview.is_some() {
                            spans.push(Span::raw("  "));
                            spans.push(Span::styled(
                                "⚡ Live preview",
                                Style::default().fg(colors::UI_SUCCESS),
                            ));
                        }
                        Line::from(spans)
                    }
                    RulesMode::Delete => Line::from(vec![
                        Span::styled("[Enter]", Style::default().fg(colors::UI_HIGHLIGHT)),
                        Span::raw(" Confirm  "),
                        Span::styled("[Esc]", Style::default().fg(colors::UI_HIGHLIGHT)),
                        Span::raw(" Cancel"),
                    ]),
                    RulesMode::SelectSink => Line::from(vec![
                        Span::raw("↑↓ Navigate  "),
                        Span::styled("[Enter]", Style::default().fg(colors::UI_HIGHLIGHT)),
                        Span::raw(" Select  "),
                        Span::styled("[Esc]", Style::default().fg(colors::UI_HIGHLIGHT)),
                        Span::raw(" Cancel"),
                    ]),
                    RulesMode::Inspect => Line::from(vec![
                        Span::styled("[Enter/Esc]", Style::default().fg(colors::UI_HIGHLIGHT)),
                        Span::raw(" Close Details"),
                    ]),
                    RulesMode::List => Line::from(""), // Should not happen in Modal mode
                }
            }
            (Screen::Settings, ScreenMode::List) => Line::from(vec![
                Span::raw("↑↓ Navigate  "),
                Span::styled("[PgUp/PgDn]", Style::default().fg(colors::UI_HIGHLIGHT)),
                Span::raw(" Scroll Info  "),
                Span::styled("[Enter/Space]", Style::default().fg(colors::UI_HIGHLIGHT)),
                Span::raw(" Toggle/Edit"),
            ]),
            (Screen::Settings, ScreenMode::Modal) => Line::from(vec![
                Span::raw("↑↓ Navigate  "),
                Span::styled("[Enter]", Style::default().fg(colors::UI_HIGHLIGHT)),
                Span::raw(" Confirm  "),
                Span::styled("[Esc]", Style::default().fg(colors::UI_HIGHLIGHT)),
                Span::raw(" Cancel"),
            ]),
            _ => Line::from(""),
        };

        // Append save indicator if config has unsaved changes
        if self.config_dirty {
            let mut spans = base_line.spans;
            spans.push(Span::raw("  "));
            spans.push(Span::styled(
                "[Ctrl+S]",
                Style::default()
                    .fg(colors::UI_WARNING)
                    .add_modifier(Modifier::BOLD),
            ));
            spans.push(Span::styled(
                " Save",
                Style::default().fg(colors::UI_WARNING),
            ));
            Line::from(spans)
        } else {
            base_line
        }
    }

    /// Process a background update message
    pub(crate) fn handle_update(&mut self, update: AppUpdate) {
        match update {
            AppUpdate::SinksData {
                active,
                profiles,
                names,
            } => {
                self.active_sink_list = active;
                self.profile_sink_list = profiles;
                self.active_sinks = names;
                self.pipewire_available = true;
                self.dirty = true;
            }
            AppUpdate::PipeWireUnavailable => {
                self.pipewire_available = false;
                self.dirty = true;
            }
            AppUpdate::DaemonState {
                running,
                windows,
                daemon_manager,
                service_enabled,
            } => {
                self.daemon_running = running;
                self.window_count = windows.len();
                self.windows = windows;
                self.dashboard_screen.service_enabled = service_enabled;
                // Update max actions if daemon manager changed (e.g., service installed/removed)
                if let Some(dm) = daemon_manager {
                    let is_systemd = dm == crate::daemon_manager::DaemonManager::Systemd;
                    self.dashboard_screen.set_max_actions(is_systemd);
                }
                self.dirty = true;
            }
            AppUpdate::ActionResult(msg) => {
                self.set_status(msg);
                // Clear daemon action pending flag when an action completes
                self.daemon_action_pending = false;
                // set_status sets dirty already
            }
            AppUpdate::ConfigSaved { success, message } => {
                self.set_status(message);
                if success {
                    self.original_config.clone_from(&self.config);
                    self.config_dirty = false;
                }
                // On failure: dirty flag stays true - user still sees save indicator
            }
            AppUpdate::PreviewPending {
                app_pattern,
                title_pattern,
            } => {
                // Only mark pending if it matches current editor content
                if self.rules_screen.editor.app_id_pattern.value() == app_pattern
                    && self.rules_screen.editor.title_pattern.value()
                        == title_pattern.clone().unwrap_or_default()
                {
                    // Store a minimal PreviewResult with no matches but pending flag (timed_out=false)
                    self.set_preview(PreviewResult {
                        app_pattern,
                        title_pattern,
                        matches: Vec::new(),
                        timed_out: false,
                        pending: true,
                        regex_error: None,
                    });
                }
            }
            AppUpdate::PreviewMatches {
                app_pattern,
                title_pattern,
                matches,
                timed_out,
                regex_error,
            } => {
                // Only apply preview if patterns match current editor content (avoid race)
                if self.rules_screen.editor.app_id_pattern.value() == app_pattern
                    && self.rules_screen.editor.title_pattern.value()
                        == title_pattern.clone().unwrap_or_default()
                {
                    // Store preview in app.preview as a typed struct
                    self.set_preview(PreviewResult {
                        app_pattern,
                        title_pattern,
                        matches,
                        timed_out,
                        pending: false,
                        regex_error,
                    });
                }
            }
            AppUpdate::DaemonLogs(new_lines) => {
                // Limit burst size before extending to avoid temporary memory spikes
                const MAX_LOG_LINES: usize = 500;
                let available_space = MAX_LOG_LINES.saturating_sub(self.daemon_log_lines.len());
                let safe_new_lines = if new_lines.len() > available_space {
                    // Take last N lines if burst is too large
                    &new_lines[new_lines.len().saturating_sub(MAX_LOG_LINES)..]
                } else {
                    &new_lines
                };

                // Append new log lines to the buffer
                self.daemon_log_lines.extend_from_slice(safe_new_lines);

                // Keep only last 500 lines to avoid unbounded growth (defensive)
                if self.daemon_log_lines.len() > MAX_LOG_LINES {
                    let excess = self.daemon_log_lines.len() - MAX_LOG_LINES;
                    self.daemon_log_lines.drain(0..excess);
                }
                self.dirty = true;
            }
        }
    }
}
