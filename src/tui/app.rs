//! TUI application state machine
//!
//! Manages screen navigation, user input, and application state.

use std::path::{Path, PathBuf};

use anyhow::Result;

use super::chrome::SPINNER;
use super::keymap::Context;
use super::preview::LivePreview;
use super::screens::list::FormEditor;
use super::screens::{DashboardScreen, HelpScroll, RulesScreen, SettingsScreen, SinksScreen};
use crate::config::{Config, ConfigModifiedOnDisk, Endpoint};
use crate::pipewire::{ActiveSink, ProfileSink};

/// Longest paste accepted into a text field (characters).
const MAX_PASTE_CHARS: usize = 4096;

/// Maximum number of daemon log lines kept for the log view.
pub(crate) const MAX_LOG_LINES: usize = 500;
/// How long a success or info status message stays in the footer.
pub(crate) const STATUS_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
/// How long quitting waits for an in-flight config save before giving up and staying open.
pub(crate) const QUIT_SAVE_WAIT: std::time::Duration = std::time::Duration::from_secs(3);

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

/// What a status message reports; decides its icon, colour and lifetime.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusKind {
    /// Something is going on ("Saving configuration…"); expires
    Info,
    /// Something worked; expires
    Success,
    /// Something was refused or needs attention; stays until the next key press
    Warning,
    /// Something failed; stays until the next key press
    Error,
}

impl StatusKind {
    /// Whether messages of this kind go away by themselves after [`STATUS_TIMEOUT`]
    pub(crate) const fn expires(self) -> bool {
        matches!(self, Self::Info | Self::Success)
    }
}

/// The message shown in the footer
#[derive(Debug, Clone)]
pub(crate) struct Status {
    pub(crate) text: String,
    pub(crate) kind: StatusKind,
    /// When an expiring message is removed
    expires_at: Option<tokio::time::Instant>,
}

/// Messages sent from background worker to UI
pub enum AppUpdate {
    /// Sinks and sources of one `PipeWire` snapshot: the nodes present and those a
    /// profile switch would create (sources without monitors)
    ///
    /// Sent by the background worker with each `PipeWire` snapshot.
    SinksData {
        active: Vec<ActiveSink>,
        profiles: Vec<ProfileSink>,
        sources: Vec<ActiveSink>,
        profile_sources: Vec<ProfileSink>,
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
        /// Whether rule-driven switching is paused (`false` while not running)
        paused: bool,
        /// Whether the daemon's compositor protocol reports monitors (`wlr`);
        /// `false` while not running
        monitor_info: bool,
    },
    /// Answer to [`BgCommand::TogglePause`]: whether rule-driven switching is
    /// paused now, or why the request failed
    PauseToggled(Result<bool, String>),
    /// Result of a daemon control action (start/stop/restart/enable/disable)
    ///
    /// Sent immediately after a daemon action completes: what was done, or why it failed.
    ActionResult(Result<String, String>),
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

/// `path` with a leading `home` directory written as `~` (shorter in the footer)
pub(crate) fn home_relative(path: &Path, home: Option<&std::ffi::OsStr>) -> String {
    match home
        .filter(|h| !h.is_empty())
        .and_then(|h| path.strip_prefix(h).ok())
    {
        Some(rest) => format!("~/{}", rest.display()),
        None => path.display().to_string(),
    }
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
    /// Pause rule-driven switching when it runs, resume it when paused
    /// (answered with [`AppUpdate::PauseToggled`])
    TogglePause,
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
    status: Option<Status>,
    /// Live preview of the rule editor's patterns
    pub(crate) preview: LivePreview,
    /// Frame of the footer spinner (advanced only while busy)
    spinner_frame: usize,
    /// Dashboard screen state
    pub(crate) dashboard_screen: DashboardScreen,
    /// Settings screen state
    pub(crate) settings_screen: SettingsScreen,
    /// Sinks screen state (its list of sinks)
    pub(crate) sinks_screen: SinksScreen,
    /// State of the Sinks screen's list of sources (mics)
    pub(crate) sources_screen: SinksScreen,
    /// Which list the Sinks screen shows (`i` switches)
    pub(crate) endpoint_view: Endpoint,
    /// Rules screen state
    pub(crate) rules_screen: RulesScreen,
    /// Whether config has unsaved changes
    pub(crate) config_dirty: bool,
    /// Whether to show help overlay
    pub(crate) show_help: bool,
    /// Scroll position of the help overlay
    pub(crate) help: HelpScroll,
    /// Whether user requested quit (waiting for confirmation if `config_dirty`)
    pub(crate) confirm_quit: bool,
    /// Cached daemon running status (updated by background worker)
    pub(crate) daemon_running: bool,
    /// Whether a daemon state snapshot arrived yet
    daemon_state_seen: bool,
    /// Cached window list for live preview (updated by background worker)
    pub(crate) windows: Vec<crate::ipc::WindowInfo>,
    /// Bumped whenever `windows` changes what rules match on (`app_id`, title,
    /// monitors, minimized; drives preview recompute)
    windows_generation: u64,
    /// Whether the daemon's rule-driven switching is paused (as last reported by it)
    pub(crate) daemon_paused: bool,
    /// Whether the running daemon reports which monitors windows are on (`wlr`)
    pub(crate) daemon_monitor_info: bool,
    /// Whether a pause/resume request sent to the background worker has not
    /// reported back yet
    pub(crate) pause_pending: bool,
    /// Whether a daemon action (start/stop/restart) is pending
    pub(crate) daemon_action_pending: bool,
    /// Whether a config save sent to the background worker has not reported back yet
    pub(crate) save_pending: bool,
    /// Quit was requested while a save was in flight: the app quits once the save
    /// succeeds, and gives up waiting (staying open) at this deadline
    quit_after_save: Option<tokio::time::Instant>,
    /// A quit already waited [`QUIT_SAVE_WAIT`] for the current save in vain; the next
    /// confirmed quit exits without waiting again
    quit_wait_expired: bool,
    /// Whether a config reload sent to the background worker has not reported back yet
    pub(crate) reload_pending: bool,
    /// Cached full active sinks with descriptions
    pub(crate) active_sink_list: Vec<ActiveSink>,
    /// Cached profile sinks for sink selector
    pub(crate) profile_sink_list: Vec<ProfileSink>,
    /// Sources present in the last snapshot (monitors excluded)
    pub(crate) active_source_list: Vec<ActiveSink>,
    /// Sources a profile switch would create
    pub(crate) profile_source_list: Vec<ProfileSink>,
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

impl DaemonAction {
    /// What the footer says while the action runs
    pub(crate) const fn progress(self) -> &'static str {
        match self {
            Self::Start => "Starting the daemon",
            Self::Stop => "Stopping the daemon",
            Self::Restart => "Restarting the daemon",
            Self::Enable => "Enabling the service",
            Self::Disable => "Disabling the service",
        }
    }
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
            status: None,
            preview: LivePreview::default(),
            spinner_frame: 0,
            dashboard_screen,
            settings_screen,
            sinks_screen,
            sources_screen: SinksScreen::new(),
            endpoint_view: Endpoint::Sink,
            rules_screen,
            config_dirty: false,
            show_help: false,
            help: HelpScroll::new(),
            confirm_quit: false,
            daemon_running: false,
            daemon_state_seen: false,
            windows: Vec::new(),
            windows_generation: 0,
            active_sink_list: Vec::new(),
            profile_sink_list: Vec::new(),
            active_source_list: Vec::new(),
            profile_source_list: Vec::new(),
            pipewire_available: true, // Optimistic default until first poll
            daemon_paused: false,
            daemon_monitor_info: false,
            pause_pending: false,
            daemon_action_pending: false,
            save_pending: false,
            quit_after_save: None,
            quit_wait_expired: false,
            reload_pending: false,
            daemon_log_lines: Vec::new(),

            bg_cmd_tx: None,
            dirty: true,
        }
    }

    /// Without sinks, open on the Sinks screen and say how to add one (`created`:
    /// the config file was just written)
    pub(crate) fn first_run_guidance(&mut self, created: bool) {
        if !self.config.sinks.is_empty() {
            return;
        }
        self.current_screen = Screen::Sinks;
        self.endpoint_view = Endpoint::Sink;
        let message = match (&self.config_path, created) {
            (Some(path), true) => format!(
                "Created {}. Press a, then Space to pick an output",
                home_relative(path, std::env::var_os("HOME").as_deref())
            ),
            _ => "No sinks yet. Press a, then Space to pick an output".to_string(),
        };
        self.set_status(StatusKind::Warning, message);
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

    /// The state of the list the Sinks screen shows (sinks or sources)
    pub(crate) const fn endpoint_screen(&self) -> &SinksScreen {
        match self.endpoint_view {
            Endpoint::Sink => &self.sinks_screen,
            Endpoint::Source => &self.sources_screen,
        }
    }

    /// [`Self::endpoint_screen`], mutable
    pub(crate) const fn endpoint_screen_mut(&mut self) -> &mut SinksScreen {
        match self.endpoint_view {
            Endpoint::Sink => &mut self.sinks_screen,
            Endpoint::Source => &mut self.sources_screen,
        }
    }

    /// The `PipeWire` nodes of `kind` from the last snapshot: present ones, and
    /// those a profile switch would create
    pub(crate) fn endpoint_lists(&self, kind: Endpoint) -> (&[ActiveSink], &[ProfileSink]) {
        match kind {
            Endpoint::Sink => (&self.active_sink_list, &self.profile_sink_list),
            Endpoint::Source => (&self.active_source_list, &self.profile_source_list),
        }
    }

    /// Switch the Sinks screen between its list of sinks and of sources
    pub(crate) fn toggle_endpoint_view(&mut self) {
        self.endpoint_view = super::screens::sinks::other(self.endpoint_view);
        self.clear_status();
    }

    /// Check if any text input field is currently focused
    pub(crate) fn is_input_focused(&self) -> bool {
        match self.current_screen {
            Screen::Dashboard => false,
            Screen::Settings => self.settings_screen.delay_input.is_some(),
            Screen::Sinks => self.endpoint_screen().text_focused(),
            Screen::Rules => self.rules_screen.text_focused(),
        }
    }

    /// The focused text input, if any (only while an editor modal is open and
    /// no overlay covers it)
    pub(crate) fn focused_input_mut(&mut self) -> Option<&mut tui_input::Input> {
        if self.show_help || self.save_conflict || !self.is_input_focused() {
            return None;
        }
        match self.current_screen {
            Screen::Sinks => self.endpoint_screen_mut().editor.focused_input_mut(),
            Screen::Rules => self.rules_screen.editor.focused_input_mut(),
            Screen::Settings => self.settings_screen.delay_input.as_mut(),
            Screen::Dashboard => None,
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

    /// Show a status message in the footer (replacing any other).
    ///
    /// Info and success messages expire after [`STATUS_TIMEOUT`]; warnings and
    /// errors stay until the next key press (see [`Self::dismiss_sticky_status`]).
    pub(crate) fn set_status(&mut self, kind: StatusKind, message: impl Into<String>) {
        let expires_at = kind
            .expires()
            .then(|| tokio::time::Instant::now() + STATUS_TIMEOUT);
        self.status = Some(Status {
            text: message.into(),
            kind,
            expires_at,
        });
        self.dirty = true;
    }

    /// Clear the current status message
    pub(crate) fn clear_status(&mut self) {
        self.status = None;
        self.dirty = true;
    }

    /// A key was pressed: a warning or error has been seen and goes away
    pub(crate) fn dismiss_sticky_status(&mut self) {
        if self.status.as_ref().is_some_and(|s| !s.kind.expires()) {
            self.clear_status();
        }
    }

    /// When the current status message expires (`None`: it does not, or there is none)
    pub(crate) fn status_deadline(&self) -> Option<tokio::time::Instant> {
        self.status.as_ref().and_then(|s| s.expires_at)
    }

    /// Remove the status message if its time is up
    pub(crate) fn expire_status(&mut self) {
        if self
            .status_deadline()
            .is_some_and(|t| t <= tokio::time::Instant::now())
        {
            self.clear_status();
        }
    }

    /// Read-only accessor for status message
    #[cfg(test)]
    pub(crate) fn status_message(&self) -> Option<&str> {
        self.status.as_ref().map(|s| s.text.as_str())
    }

    /// Kind of the current status message
    #[cfg(test)]
    pub(crate) fn status_kind(&self) -> Option<StatusKind> {
        self.status.as_ref().map(|s| s.kind)
    }

    /// The key binding contexts active now (they decide the footer hints)
    pub(crate) fn active_contexts(&self) -> Vec<Context> {
        if self.show_help {
            return vec![Context::Help];
        }
        if self.save_conflict {
            return vec![Context::Conflict];
        }
        let screen = self.current_screen;
        let modal = match screen {
            Screen::Dashboard => None,
            Screen::Sinks => self.endpoint_screen().context(screen),
            Screen::Rules => self.rules_screen.context(screen),
            Screen::Settings if self.settings_screen.delay_input.is_some() => {
                Some(Context::Editor(screen))
            }
            Screen::Settings => self
                .settings_screen
                .editing_log_level
                .then_some(Context::Dropdown(screen)),
        };
        match modal {
            Some(context) => vec![context],
            None if screen == Screen::Dashboard => vec![
                Context::List(screen),
                Context::DashboardView(self.dashboard_screen.current_view),
                Context::Global,
            ],
            None => vec![Context::List(screen), Context::Global],
        }
    }

    /// Whether background work the user is waiting for is in flight (drives the
    /// footer spinner; the UI only animates while this is true).
    pub(crate) const fn is_busy(&self) -> bool {
        self.daemon_action_pending || self.save_pending || self.reload_pending || self.pause_pending
    }

    /// Advance the footer spinner by one frame.
    pub(crate) const fn advance_spinner(&mut self) {
        self.spinner_frame = (self.spinner_frame + 1) % SPINNER.len();
        self.dirty = true;
    }

    /// What the footer renders: the status message and the spinner frame
    /// (`None` while nothing runs).
    pub(crate) fn footer_parts(&self) -> (Option<&Status>, Option<&'static str>) {
        let spinner = self.is_busy().then(|| SPINNER[self.spinner_frame]);
        (self.status.as_ref(), spinner)
    }

    /// Queue a command for the background worker.
    ///
    /// Returns `false` (after putting the reason in the status line) if there is
    /// no worker or its queue is full or closed; callers must then not enter any
    /// pending state.
    pub(crate) fn send_command(&mut self, cmd: BgCommand) -> bool {
        use tokio::sync::mpsc::error::TrySendError;

        let Some(tx) = &self.bg_cmd_tx else {
            self.set_status(StatusKind::Error, "Background worker not available");
            return false;
        };
        match tx.try_send(cmd) {
            Ok(()) => true,
            Err(TrySendError::Full(_)) => {
                self.set_status(
                    StatusKind::Warning,
                    "Busy: too many pending requests, try again",
                );
                false
            }
            Err(TrySendError::Closed(_)) => {
                self.set_status(
                    StatusKind::Error,
                    "Background worker stopped; restart the TUI",
                );
                false
            }
        }
    }

    /// Pause rule-driven switching, or resume it when paused.
    ///
    /// The request runs on the background worker; the shown state changes only
    /// when the daemon has answered ([`AppUpdate::PauseToggled`]). Refused with a
    /// status message while the daemon is not running or a request is in flight.
    pub(crate) fn request_toggle_pause(&mut self) {
        if !self.daemon_running {
            self.set_status(
                StatusKind::Warning,
                "Daemon not running: start it to pause or resume rule-driven switching",
            );
            return;
        }
        if self.pause_pending {
            self.set_status(StatusKind::Warning, "Already changing the pause state");
            return;
        }
        if self.send_command(BgCommand::TogglePause) {
            self.pause_pending = true;
            let progress = if self.daemon_paused {
                "Resuming rule-driven switching…"
            } else {
                "Pausing rule-driven switching…"
            };
            self.set_status(StatusKind::Info, progress);
        }
    }

    /// Recompute the rule editor's live preview if its patterns or the window
    /// list changed. Cheap when nothing changed; call before drawing.
    pub(crate) fn refresh_preview(&mut self) {
        if self.current_screen != Screen::Rules
            || self.rules_screen.mode != super::screens::list::ListMode::AddEdit
        {
            return;
        }
        let editor = &self.rules_screen.editor;
        self.preview.refresh(
            &super::preview::Patterns {
                app_id: editor.app_id_pattern.value(),
                title: editor.title_pattern.value(),
                output: editor.output_pattern.value(),
            },
            &self.windows,
            self.windows_generation,
        );
    }

    /// Quit (unsaved edits, if any, are discarded)
    ///
    /// While a config save is in flight the app does not exit at once: killing the
    /// save thread before its atomic rename would lose the save. Instead it waits up
    /// to [`QUIT_SAVE_WAIT`] for the result (see [`Self::quit_save_deadline`]) and
    /// exits only if the save succeeded.
    pub(crate) fn quit(&mut self) {
        self.dirty = true;
        if !self.save_pending || self.quit_wait_expired {
            self.should_quit = true;
            return;
        }
        if self.quit_after_save.is_none() {
            self.quit_after_save = Some(tokio::time::Instant::now() + QUIT_SAVE_WAIT);
        }
        self.set_status(
            StatusKind::Info,
            "Saving configuration… (quitting once it is written)",
        );
    }

    /// When a quit waiting for an in-flight save gives up; `None` when not waiting
    pub(crate) const fn quit_save_deadline(&self) -> Option<tokio::time::Instant> {
        self.quit_after_save
    }

    /// Whether a quit is waiting for an in-flight save (input is ignored meanwhile,
    /// except Esc, which cancels the quit)
    pub(crate) const fn is_quitting_after_save(&self) -> bool {
        self.quit_after_save.is_some()
    }

    /// Stop waiting to quit after the save; the app stays open
    pub(crate) fn cancel_quit_after_save(&mut self) {
        if self.quit_after_save.take().is_some() {
            self.set_status(
                StatusKind::Info,
                "Quit cancelled; the save is still running",
            );
        }
    }

    /// The save did not finish within [`QUIT_SAVE_WAIT`]: stay open rather than risk
    /// losing it. A further confirmed quit exits without waiting.
    pub(crate) fn quit_save_wait_expired(&mut self) {
        if self.quit_after_save.take().is_none() {
            return;
        }
        self.quit_wait_expired = true;
        self.set_status(
            StatusKind::Warning,
            "Save still running; not quitting so it is not lost. Quit again to exit anyway",
        );
    }

    /// Request quit: quits at once without unsaved changes, otherwise asks for
    /// confirmation (the next key decides, see `input::handle_key_event`)
    pub(crate) fn request_quit(&mut self) {
        if self.config_dirty {
            self.confirm_quit = true;
            self.set_status(
                StatusKind::Warning,
                "Unsaved changes: s save and quit · q quit without saving · other keys stay",
            );
        } else {
            self.quit();
        }
    }

    /// Answer `s` to the unsaved-changes prompt: save, then quit once the file is
    /// written (a failed or refused save keeps the app open and says why)
    pub(crate) fn save_and_quit(&mut self) {
        self.confirm_quit = false;
        self.quit_wait_expired = false;
        self.request_save(false);
        if self.save_pending {
            // Waits for the save and exits only if it succeeded
            self.quit();
        } else if !self.config_dirty {
            // Saved right away (no background worker)
            self.quit();
        }
    }

    /// Cancel quit confirmation
    pub(crate) fn cancel_quit(&mut self) {
        self.confirm_quit = false;
        self.clear_status();
        self.dirty = true;
    }

    /// Show the help overlay, scrolled to the top
    pub(crate) const fn open_help(&mut self) {
        self.show_help = true;
        self.help.offset = 0;
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
            self.set_status(StatusKind::Error, "No config file to save to");
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
                self.set_status(StatusKind::Info, "Saving configuration…");
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
            self.set_status(StatusKind::Error, "No config file to reload");
            return;
        };
        if self.bg_cmd_tx.is_some() {
            if self.send_command(BgCommand::ReloadConfig(path)) {
                self.reload_pending = true;
                self.set_status(StatusKind::Info, "Reloading configuration…");
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
        self.sources_screen = SinksScreen::new();
        self.rules_screen = RulesScreen::new();
        self.confirm_quit = false;
        self.dirty = true;
    }

    /// Process a background update message
    pub(crate) fn handle_update(&mut self, update: AppUpdate) {
        match update {
            AppUpdate::SinksData {
                active,
                profiles,
                sources,
                profile_sources,
            } => {
                self.active_sink_list = active;
                self.profile_sink_list = profiles;
                self.active_source_list = sources;
                self.profile_source_list = profile_sources;
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
                paused,
                monitor_info,
            } => {
                self.daemon_paused = running && paused;
                self.daemon_monitor_info = running && monitor_info;
                if running != self.daemon_running || !self.daemon_state_seen {
                    self.dashboard_screen.preselect_action(running);
                }
                self.daemon_state_seen = true;
                self.daemon_running = running;
                // Everything a rule matches on (the preview depends on it)
                let changed = windows.len() != self.windows.len()
                    || windows.iter().zip(&self.windows).any(|(a, b)| {
                        a.app_id != b.app_id
                            || a.title != b.title
                            || a.outputs != b.outputs
                            || a.minimized != b.minimized
                    });
                if changed {
                    self.windows_generation = self.windows_generation.wrapping_add(1);
                }
                // The window list's selection stays on the same window
                let selected_id = self
                    .dashboard_screen
                    .selected_window(&self.windows)
                    .and_then(|w| w.id);
                self.windows = windows;
                self.dashboard_screen
                    .follow_selected_window(selected_id, &self.windows);
                self.dashboard_screen.service_enabled = service_enabled;
                // Enable/disable act on the installed unit, however the daemon runs
                self.dashboard_screen.set_max_actions(unit_installed);
                self.dirty = true;
            }
            AppUpdate::ActionResult(result) => {
                match result {
                    Ok(msg) => self.set_status(StatusKind::Success, msg),
                    Err(msg) => self.set_status(StatusKind::Error, msg),
                }
                // Clear daemon action pending flag when an action completes
                self.daemon_action_pending = false;
                // set_status sets dirty already
            }
            AppUpdate::PauseToggled(result) => {
                self.pause_pending = false;
                match result {
                    Ok(paused) => {
                        self.daemon_paused = paused;
                        if paused {
                            self.set_status(
                                StatusKind::Success,
                                "Paused rule-driven switching (p resumes)",
                            );
                        } else {
                            self.set_status(StatusKind::Success, "Resumed rule-driven switching");
                        }
                    }
                    Err(message) => self.set_status(StatusKind::Error, message),
                }
            }
            AppUpdate::ConfigSaved { saved, outcome } => {
                self.save_pending = false;
                self.quit_wait_expired = false;
                let quitting = self.quit_after_save.take().is_some();
                let succeeded = outcome == SaveOutcome::Saved;
                match outcome {
                    SaveOutcome::Saved => {
                        // The snapshot is what is on disk now; anything edited
                        // since Ctrl+S stays unsaved.
                        self.original_config = *saved;
                        self.mark_dirty();
                        self.save_conflict = false;
                        if self.config_dirty {
                            self.set_status(
                                StatusKind::Warning,
                                "Configuration saved (changes made since are not saved yet)",
                            );
                        } else {
                            self.set_status(StatusKind::Success, "Configuration saved");
                        }
                    }
                    SaveOutcome::ModifiedOnDisk => {
                        self.save_conflict = true;
                        self.set_status(
                            StatusKind::Warning,
                            "Config file changed on disk; not saved (choose in the dialog)",
                        );
                    }
                    SaveOutcome::Failed(message) => {
                        // Dirty flag stays: the user still sees the save indicator
                        self.set_status(
                            StatusKind::Error,
                            format!("Failed to save config: {message}"),
                        );
                    }
                }
                // A quit waiting for this save: exit once it is on disk (the user
                // confirmed discarding edits made after Ctrl+S), otherwise stay open
                // and say why
                if quitting && succeeded {
                    self.should_quit = true;
                } else if quitting {
                    let (kind, reason) = self.status.take().map_or_else(
                        || (StatusKind::Warning, String::new()),
                        |s| (s.kind, s.text),
                    );
                    let kind = if kind.expires() {
                        StatusKind::Warning
                    } else {
                        kind
                    };
                    self.set_status(kind, format!("Not quitting: {reason}"));
                }
            }
            AppUpdate::ConfigReloaded(result) => {
                self.reload_pending = false;
                match result {
                    Ok(config) => {
                        self.replace_config(*config);
                        self.set_status(StatusKind::Success, "Configuration reloaded from disk");
                    }
                    Err(message) => self.set_status(
                        StatusKind::Error,
                        format!("Failed to reload config: {message}"),
                    ),
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
