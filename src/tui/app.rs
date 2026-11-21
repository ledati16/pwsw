//! TUI application state machine
//!
//! Manages screen navigation, user input, and application state.

use std::path::{Path, PathBuf};

use color_eyre::eyre::Result;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use throbber_widgets_tui::ThrobberState;

use super::preview::LivePreview;
use super::screens::{DashboardScreen, RulesScreen, SettingsScreen, SinksScreen};
use crate::config::{Config, ConfigModifiedOnDisk};
use crate::style::colors;

/// Longest paste accepted into a text field (characters).
const MAX_PASTE_CHARS: usize = 4096;

/// Maximum number of daemon log lines kept for the log view.
pub(crate) const MAX_LOG_LINES: usize = 500;

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
    /// Sent by the background worker with each `PipeWire` snapshot.
    SinksData {
        active: Vec<crate::pipewire::ActiveSink>,
        profiles: Vec<crate::pipewire::ProfileSink>,
    },
    /// `PipeWire` is unavailable (pw-dump failed)
    ///
    /// Sent by background poller when `PipeWire::dump()` fails.
    /// UI should show a warning that sink status may be stale.
    PipeWireUnavailable,
    /// Daemon state snapshot including running status, tracked windows, and manager info
    ///
    /// Sent by the background worker every second and immediately after daemon
    /// control actions (start/stop/restart/enable/disable) complete.
    DaemonState {
        running: bool,
        windows: Vec<crate::ipc::WindowInfo>,
        /// Whether the `pwsw.service` user unit is installed (enables enable/disable,
        /// however the running daemon was started)
        unit_installed: bool,
        /// Whether the unit is enabled; `None` when it is not installed
        service_enabled: Option<bool>,
    },
    /// Result message from daemon control action (start/stop/restart/enable/disable)
    ///
    /// Sent immediately after a daemon action completes, containing success or error message.
    ActionResult(String),
    /// Config save result
    ///
    /// Sent by background worker after a `SaveConfig` command completes. `saved` is
    /// the snapshot that was written: on success it becomes the clean baseline, so
    /// edits made while the save ran stay marked unsaved.
    ConfigSaved {
        saved: Box<Config>,
        outcome: SaveOutcome,
    },
    /// Result of re-reading the config file (`ReloadConfig`)
    ConfigReloaded(Result<Box<Config>, String>),
    /// New daemon log lines
    ///
    /// Sent by log tail worker when new lines are appended to daemon log file.
    /// Used to update the dashboard log viewer in real-time.
    DaemonLogs(Vec<String>),
}

/// How a config save ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SaveOutcome {
    /// Written to disk
    Saved,
    /// Refused: the file changed on disk since it was loaded or last saved
    ModifiedOnDisk,
    /// Failed with this message
    Failed(String),
}

impl SaveOutcome {
    /// Classify the result of [`Config::save_to`] / [`Config::save_force_to`].
    pub(crate) fn from_result(result: Result<()>) -> Self {
        match result {
            Ok(()) => Self::Saved,
            Err(e) if e.downcast_ref::<ConfigModifiedOnDisk>().is_some() => Self::ModifiedOnDisk,
            Err(e) => Self::Failed(format!("{e:#}")),
        }
    }
}

/// Save `config` to `path`, overwriting external changes only if `force`.
pub(crate) fn save_blocking(config: &Config, path: &Path, force: bool) -> SaveOutcome {
    SaveOutcome::from_result(if force {
        config.save_force_to(path)
    } else {
        config.save_to(path)
    })
}

/// Load the config file at `path` for a reload.
pub(crate) fn load_blocking(path: &Path) -> Result<Box<Config>, String> {
    Config::load_from_path(path)
        .map(Box::new)
        .map_err(|e| format!("{e:#}"))
}

/// Commands sent from UI to background worker
#[derive(Debug)]
pub enum BgCommand {
    DaemonAction(DaemonAction),
    /// Save this snapshot atomically to `path` (`force` overwrites external changes)
    SaveConfig {
        config: Box<Config>,
        path: PathBuf,
        force: bool,
    },
    /// Re-read the config file at this path
    ReloadConfig(PathBuf),
}

// TUI state with multiple independent boolean flags for UI state tracking
pub struct App {
    /// Channel sender to send commands to background worker (bounded, non-blocking `try_send`)
    pub(crate) bg_cmd_tx: Option<tokio::sync::mpsc::Sender<BgCommand>>,

    /// Currently active screen
    pub(crate) current_screen: Screen,
    /// Whether the application should quit
    pub(crate) should_quit: bool,
    /// Configuration (loaded at startup, editable in TUI)
    pub(crate) config: Config,
    /// File the config is saved to and reloaded from (`None`: saving disabled)
    pub(crate) config_path: Option<PathBuf>,
    /// The last save was refused because the file changed on disk; the conflict
    /// dialog is open
    pub(crate) save_conflict: bool,
    /// Snapshot of config at last save (for dirty comparison)
    original_config: Config,
    /// Status message to display (errors, confirmations)
    status_message: Option<String>,
    /// Live preview of the rule editor's patterns
    pub(crate) preview: LivePreview,
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
    /// Bumped whenever `windows` changes `app_id`/title content (drives preview recompute)
    windows_generation: u64,
    /// Whether a daemon action (start/stop/restart) is pending
    pub(crate) daemon_action_pending: bool,
    /// Whether a config save sent to the background worker has not reported back yet
    pub(crate) save_pending: bool,
    /// Whether a config reload sent to the background worker has not reported back yet
    pub(crate) reload_pending: bool,
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
    /// Create an application instance for a config loaded from `config_path`
    #[must_use]
    pub(crate) fn new(config: Config, config_path: PathBuf) -> Self {
        let mut app = Self::with_config(config);
        app.config_path = Some(config_path);
        app
    }

    /// Create an application instance with a pre-loaded config that is not tied
    /// to a file (saving and reloading are refused until `config_path` is set)
    #[must_use]
    pub(crate) fn with_config(config: Config) -> Self {
        // bg_update channels initialized by caller (run()), set to None here
        let dashboard_screen = DashboardScreen::new();
        let settings_screen = SettingsScreen::new(&config.settings);
        let sinks_screen = SinksScreen::new();
        let rules_screen = RulesScreen::new();
        // Clone config for original_config before moving it
        let original_config = config.clone();
        Self {
            current_screen: Screen::Dashboard,
            should_quit: false,
            config,
            config_path: None,
            save_conflict: false,
            original_config,
            status_message: None,
            preview: LivePreview::default(),
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
            windows_generation: 0,
            active_sinks: Vec::new(),
            active_sink_list: Vec::new(),
            profile_sink_list: Vec::new(),
            pipewire_available: true, // Optimistic default until first poll
            daemon_action_pending: false,
            save_pending: false,
            reload_pending: false,
            daemon_log_lines: Vec::new(),

            bg_cmd_tx: None,
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
                    && self.sinks_screen.editor.focused_field.is_text()
            }
            Screen::Rules => {
                self.rules_screen.mode == RulesMode::AddEdit
                    && self.rules_screen.editor.focused_field.is_text()
            }
        }
    }

    /// The focused text input, if any (only while an editor modal is open and
    /// no overlay covers it)
    pub(crate) fn focused_input_mut(&mut self) -> Option<&mut tui_input::Input> {
        if self.show_help || self.save_conflict || !self.is_input_focused() {
            return None;
        }
        match self.current_screen {
            Screen::Sinks => self.sinks_screen.editor.focused_input_mut(),
            Screen::Rules => self.rules_screen.editor.focused_input_mut(),
            Screen::Dashboard | Screen::Settings => None,
        }
    }

    /// Insert pasted text into the focused text input.
    ///
    /// Newlines and other control characters are dropped (a pasted newline must
    /// not act as Enter) and very long pastes are cut off. Ignored when no text
    /// input has focus, so pasted text can never trigger key bindings.
    pub(crate) fn paste(&mut self, text: &str) {
        use tui_input::InputRequest;

        let Some(input) = self.focused_input_mut() else {
            return;
        };
        for c in text
            .chars()
            .filter(|c| !c.is_control())
            .take(MAX_PASTE_CHARS)
        {
            input.handle(InputRequest::InsertChar(c));
        }
        self.dirty = true;
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
    #[cfg(test)]
    pub(crate) fn status_message(&self) -> Option<&str> {
        self.status_message.as_deref()
    }

    /// Whether background work the user is waiting for is in flight (drives the
    /// footer spinner; the UI only animates while this is true).
    pub(crate) const fn is_busy(&self) -> bool {
        self.daemon_action_pending || self.save_pending || self.reload_pending
    }

    /// Advance the footer spinner by one frame.
    pub(crate) fn advance_throbber(&mut self) {
        self.throbber_state.calc_next();
        self.dirty = true;
    }

    /// Borrow what the footer renders: the status message, whether to spin,
    /// and the spinner state.
    pub(crate) fn footer_parts(&mut self) -> (Option<&str>, bool, &mut ThrobberState) {
        let busy = self.is_busy();
        (
            self.status_message.as_deref(),
            busy,
            &mut self.throbber_state,
        )
    }

    /// Queue a command for the background worker.
    ///
    /// Returns `false` (after putting the reason in the status line) if there is
    /// no worker or its queue is full or closed; callers must then not enter any
    /// pending state.
    pub(crate) fn send_command(&mut self, cmd: BgCommand) -> bool {
        use tokio::sync::mpsc::error::TrySendError;

        let Some(tx) = &self.bg_cmd_tx else {
            self.set_status("Background worker not available".to_string());
            return false;
        };
        match tx.try_send(cmd) {
            Ok(()) => true,
            Err(TrySendError::Full(_)) => {
                self.set_status("Busy: too many pending requests, try again".to_string());
                false
            }
            Err(TrySendError::Closed(_)) => {
                self.set_status("Background worker stopped; restart the TUI".to_string());
                false
            }
        }
    }

    /// Recompute the rule editor's live preview if its patterns or the window
    /// list changed. Cheap when nothing changed; call before drawing.
    pub(crate) fn refresh_preview(&mut self) {
        use super::screens::rules::RulesMode;

        if self.current_screen != Screen::Rules || self.rules_screen.mode != RulesMode::AddEdit {
            return;
        }
        let editor = &self.rules_screen.editor;
        self.preview.refresh(
            editor.app_id_pattern.value(),
            editor.title_pattern.value(),
            &self.windows,
            self.windows_generation,
        );
    }

    /// Request application quit
    pub(crate) const fn quit(&mut self) {
        self.should_quit = true;
        self.dirty = true;
    }

    /// Request quit: quits at once without unsaved changes, otherwise asks for
    /// confirmation (the next key decides, see `input::handle_key_event`)
    pub(crate) fn request_quit(&mut self) {
        if self.config_dirty {
            self.confirm_quit = true;
            self.set_status(
                "Unsaved changes! Press q or Ctrl+C again to quit without saving, any other key \
                 to stay"
                    .to_string(),
            );
        } else {
            self.quit();
        }
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

    /// Save the current config (a snapshot of it) to the config file.
    ///
    /// Runs on the background worker; without one (tests) it saves right away.
    /// `force` overwrites changes made to the file by someone else.
    pub(crate) fn request_save(&mut self, force: bool) {
        let Some(path) = self.config_path.clone() else {
            self.set_status("No config file to save to".to_string());
            return;
        };
        let snapshot = Box::new(self.config.clone());
        if self.bg_cmd_tx.is_some() {
            if self.send_command(BgCommand::SaveConfig {
                config: snapshot,
                path,
                force,
            }) {
                self.save_pending = true;
                self.set_status("Saving configuration...".to_string());
            }
        } else {
            let outcome = save_blocking(&snapshot, &path, force);
            self.handle_update(AppUpdate::ConfigSaved {
                saved: snapshot,
                outcome,
            });
        }
    }

    /// Re-read the config file, discarding unsaved edits once it loads.
    ///
    /// Runs on the background worker; without one (tests) it loads right away.
    pub(crate) fn request_reload(&mut self) {
        let Some(path) = self.config_path.clone() else {
            self.set_status("No config file to reload".to_string());
            return;
        };
        if self.bg_cmd_tx.is_some() {
            if self.send_command(BgCommand::ReloadConfig(path)) {
                self.reload_pending = true;
                self.set_status("Reloading configuration...".to_string());
            }
        } else {
            let result = load_blocking(&path);
            self.handle_update(AppUpdate::ConfigReloaded(result));
        }
    }

    /// Replace the edited config with `config` (fresh from disk): it becomes the
    /// clean baseline and all editor state is reset.
    fn replace_config(&mut self, config: Config) {
        self.original_config.clone_from(&config);
        self.settings_screen = SettingsScreen::new(&config.settings);
        self.config = config;
        self.config_dirty = false;
        self.sinks_screen = SinksScreen::new();
        self.rules_screen = RulesScreen::new();
        self.confirm_quit = false;
        self.dirty = true;
    }

    /// Generate the context bar text based on current app state
    pub(crate) fn context_bar_text(&self) -> Line<'static> {
        use super::screens::rules::RulesMode;
        use super::screens::sinks::SinksMode;

        let mode = self.get_screen_mode();
        let key = |k: &'static str| Span::styled(k, Style::default().fg(colors::UI_HIGHLIGHT));

        if self.save_conflict {
            return Line::from(vec![
                key("[o]"),
                Span::raw(" Overwrite  "),
                key("[r]"),
                Span::raw(" Reload from disk  "),
                key("[Esc]"),
                Span::raw(" Cancel"),
            ]);
        }

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
                        spans.push(key("[↑↓/PgUp/PgDn]"));
                        spans.push(Span::raw(" Scroll  "));
                        spans.push(key("[End]"));
                        spans.push(Span::raw(" Live  "));
                        spans.push(Span::styled(
                            "[w]",
                            Style::default().fg(colors::UI_HIGHLIGHT),
                        ));
                        spans.push(Span::raw(" View Windows"));
                    }
                    DashboardView::Windows => {
                        spans.push(key("[↑↓/PgUp/PgDn]"));
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
                        key("[Space/Enter]"),
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
                        // Show live preview indicator once there is a pattern to match
                        if !self.rules_screen.editor.app_id_pattern.value().is_empty() {
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
                        key("[Space/Enter]"),
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
            AppUpdate::SinksData { active, profiles } => {
                self.active_sinks = active.iter().map(|s| s.name.clone()).collect();
                self.active_sink_list = active;
                self.profile_sink_list = profiles;
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
                unit_installed,
                service_enabled,
            } => {
                self.daemon_running = running;
                self.window_count = windows.len();
                let changed = windows.len() != self.windows.len()
                    || windows
                        .iter()
                        .zip(&self.windows)
                        .any(|(a, b)| a.app_id != b.app_id || a.title != b.title);
                if changed {
                    self.windows_generation = self.windows_generation.wrapping_add(1);
                }
                self.windows = windows;
                self.dashboard_screen.service_enabled = service_enabled;
                // Enable/disable act on the installed unit, however the daemon runs
                self.dashboard_screen.set_max_actions(unit_installed);
                self.dirty = true;
            }
            AppUpdate::ActionResult(msg) => {
                self.set_status(msg);
                // Clear daemon action pending flag when an action completes
                self.daemon_action_pending = false;
                // set_status sets dirty already
            }
            AppUpdate::ConfigSaved { saved, outcome } => {
                self.save_pending = false;
                match outcome {
                    SaveOutcome::Saved => {
                        // The snapshot is what is on disk now; anything edited
                        // since Ctrl+S stays unsaved.
                        self.original_config = *saved;
                        self.mark_dirty();
                        self.save_conflict = false;
                        self.set_status(if self.config_dirty {
                            "Configuration saved (changes made since are not saved yet)".to_string()
                        } else {
                            "Configuration saved successfully".to_string()
                        });
                    }
                    SaveOutcome::ModifiedOnDisk => {
                        self.save_conflict = true;
                        self.set_status(
                            "Config file changed on disk; not saved (choose in the dialog)"
                                .to_string(),
                        );
                    }
                    SaveOutcome::Failed(message) => {
                        // Dirty flag stays: the user still sees the save indicator
                        self.set_status(format!("Failed to save config: {message}"));
                    }
                }
            }
            AppUpdate::ConfigReloaded(result) => {
                self.reload_pending = false;
                match result {
                    Ok(config) => {
                        self.replace_config(*config);
                        self.set_status("Configuration reloaded from disk".to_string());
                    }
                    Err(message) => self.set_status(format!("Failed to reload config: {message}")),
                }
            }
            AppUpdate::DaemonLogs(new_lines) => {
                // Only the newest lines of a large burst can be kept
                let skip = new_lines.len().saturating_sub(MAX_LOG_LINES);
                let added = new_lines.len() - skip;
                self.daemon_log_lines
                    .extend(new_lines.into_iter().skip(skip));

                if self.daemon_log_lines.len() > MAX_LOG_LINES {
                    let excess = self.daemon_log_lines.len() - MAX_LOG_LINES;
                    self.daemon_log_lines.drain(0..excess);
                }
                // While scrolled back, keep showing the same lines
                self.dashboard_screen
                    .keep_log_position(added, self.daemon_log_lines.len());
                self.dirty = true;
            }
        }
    }
}
