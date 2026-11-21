//! The key bindings of the TUI, in one table.
//!
//! The help overlay is rendered from [`BINDINGS`] (and its section navigation
//! counts rows of the same table), so documentation cannot drift from what the
//! overlay shows. Each binding also lists the concrete key presses it documents;
//! a test feeds every one of them to the input handler in the binding's context
//! and fails if any does nothing, so documented keys cannot drift from the code
//! either.

use crossterm::event::{KeyCode, KeyModifiers};

use super::app::Screen;

/// Where a binding applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Context {
    /// On every screen while no dialog is open
    Global,
    /// A screen's main view (no dialog open)
    List(Screen),
    /// The add/edit dialog of a screen
    Editor(Screen),
    /// A selection list opened from a screen (node, target sink, log level)
    Dropdown(Screen),
    /// The delete confirmation and details dialogs of a screen
    Dialog(Screen),
    /// A focused text field in an editor
    TextInput,
    /// The help overlay
    Help,
}

impl Context {
    /// The screen whose help section shows this binding (`None`: a shared section)
    pub(crate) const fn screen(self) -> Option<Screen> {
        match self {
            Self::List(s) | Self::Editor(s) | Self::Dropdown(s) | Self::Dialog(s) => Some(s),
            Self::Global | Self::TextInput | Self::Help => None,
        }
    }

    /// Sub-heading shown above this context's bindings inside a screen section
    pub(crate) const fn heading(self) -> Option<&'static str> {
        match self {
            Self::Editor(_) => Some("In the add/edit dialog:"),
            Self::Dropdown(_) => Some("In a selection list:"),
            Self::Dialog(_) => Some("In the delete/details dialog:"),
            Self::List(_) | Self::Global | Self::TextInput | Self::Help => None,
        }
    }
}

/// One documented binding.
pub(crate) struct Binding {
    pub(crate) context: Context,
    /// The keys as shown in the help
    pub(crate) keys: &'static str,
    pub(crate) description: &'static str,
    /// The key presses `keys` stands for (empty for non-key actions like paste);
    /// only the keymap consistency test reads them
    #[cfg(test)]
    pub(crate) chords: &'static [(KeyCode, KeyModifiers)],
}

const NONE: KeyModifiers = KeyModifiers::NONE;
const SHIFT: KeyModifiers = KeyModifiers::SHIFT;
const CTRL: KeyModifiers = KeyModifiers::CONTROL;
const ALT: KeyModifiers = KeyModifiers::ALT;

const fn b(
    context: Context,
    keys: &'static str,
    description: &'static str,
    chords: &'static [(KeyCode, KeyModifiers)],
) -> Binding {
    #[cfg(not(test))]
    let _ = chords;
    Binding {
        context,
        keys,
        description,
        #[cfg(test)]
        chords,
    }
}

const UP_DOWN: &[(KeyCode, KeyModifiers)] = &[(KeyCode::Up, NONE), (KeyCode::Down, NONE)];
const SHIFT_UP_DOWN: &[(KeyCode, KeyModifiers)] = &[(KeyCode::Up, SHIFT), (KeyCode::Down, SHIFT)];
const SWITCH_FIELD: &[(KeyCode, KeyModifiers)] = &[
    (KeyCode::Up, NONE),
    (KeyCode::Down, NONE),
    (KeyCode::Tab, NONE),
    (KeyCode::BackTab, SHIFT),
];
const PAGE: &[(KeyCode, KeyModifiers)] = &[(KeyCode::PageUp, NONE), (KeyCode::PageDown, NONE)];
const HOME_END: &[(KeyCode, KeyModifiers)] = &[(KeyCode::Home, NONE), (KeyCode::End, NONE)];
const SPACE: &[(KeyCode, KeyModifiers)] = &[(KeyCode::Char(' '), NONE)];
const ENTER: &[(KeyCode, KeyModifiers)] = &[(KeyCode::Enter, NONE)];
const ESC: &[(KeyCode, KeyModifiers)] = &[(KeyCode::Esc, NONE)];
const SPACE_ENTER: &[(KeyCode, KeyModifiers)] =
    &[(KeyCode::Char(' '), NONE), (KeyCode::Enter, NONE)];

/// Every key binding, in help display order.
pub(crate) static BINDINGS: &[Binding] = &[
    // --- Dashboard ---
    b(
        Context::List(Screen::Dashboard),
        "←/→",
        "Select daemon action",
        &[(KeyCode::Left, NONE), (KeyCode::Right, NONE)],
    ),
    b(
        Context::List(Screen::Dashboard),
        "Enter",
        "Run selected action (start/stop/restart; enable/disable when the systemd unit is \
         installed)",
        ENTER,
    ),
    b(
        Context::List(Screen::Dashboard),
        "w",
        "Toggle between Logs ↔ Windows view",
        &[(KeyCode::Char('w'), NONE)],
    ),
    b(
        Context::List(Screen::Dashboard),
        "↑/↓",
        "Scroll one line",
        UP_DOWN,
    ),
    b(
        Context::List(Screen::Dashboard),
        "PgUp/PgDn",
        "Scroll one page",
        PAGE,
    ),
    b(
        Context::List(Screen::Dashboard),
        "Home/End",
        "Oldest/newest logs (End returns to live) · first/last window",
        HOME_END,
    ),
    // --- Sinks ---
    b(
        Context::List(Screen::Sinks),
        "↑/↓",
        "Navigate list",
        UP_DOWN,
    ),
    b(
        Context::List(Screen::Sinks),
        "Shift+↑/↓",
        "Move sink up/down",
        SHIFT_UP_DOWN,
    ),
    b(Context::List(Screen::Sinks), "a", "Add new sink", &[(
        KeyCode::Char('a'),
        NONE,
    )]),
    b(Context::List(Screen::Sinks), "e", "Edit selected sink", &[
        (KeyCode::Char('e'), NONE),
    ]),
    b(
        Context::List(Screen::Sinks),
        "x",
        "Delete selected sink",
        &[(KeyCode::Char('x'), NONE)],
    ),
    b(
        Context::List(Screen::Sinks),
        "Space",
        "Make selected sink the default",
        SPACE,
    ),
    b(
        Context::List(Screen::Sinks),
        "Enter",
        "Show sink details",
        ENTER,
    ),
    b(
        Context::Editor(Screen::Sinks),
        "↑/↓, Tab/Shift+Tab",
        "Switch field",
        SWITCH_FIELD,
    ),
    b(
        Context::Editor(Screen::Sinks),
        "Space",
        "On Node Name: pick from PipeWire nodes · on Default: toggle",
        SPACE,
    ),
    b(Context::Editor(Screen::Sinks), "Enter", "Save sink", ENTER),
    b(Context::Editor(Screen::Sinks), "Esc", "Cancel editing", ESC),
    b(
        Context::Dropdown(Screen::Sinks),
        "↑/↓",
        "Move highlight",
        UP_DOWN,
    ),
    b(
        Context::Dropdown(Screen::Sinks),
        "Space/Enter",
        "Use highlighted node",
        SPACE_ENTER,
    ),
    b(
        Context::Dropdown(Screen::Sinks),
        "Esc",
        "Back to the editor",
        ESC,
    ),
    b(
        Context::Dialog(Screen::Sinks),
        "Enter",
        "Confirm delete / close details",
        ENTER,
    ),
    b(
        Context::Dialog(Screen::Sinks),
        "Esc",
        "Cancel / close details",
        ESC,
    ),
    // --- Rules ---
    b(
        Context::List(Screen::Rules),
        "↑/↓",
        "Navigate list",
        UP_DOWN,
    ),
    b(
        Context::List(Screen::Rules),
        "Shift+↑/↓",
        "Move rule up/down (earlier rules win with \"Match by Rule Index\")",
        SHIFT_UP_DOWN,
    ),
    b(Context::List(Screen::Rules), "a", "Add new rule", &[(
        KeyCode::Char('a'),
        NONE,
    )]),
    b(Context::List(Screen::Rules), "e", "Edit selected rule", &[
        (KeyCode::Char('e'), NONE),
    ]),
    b(
        Context::List(Screen::Rules),
        "x",
        "Delete selected rule",
        &[(KeyCode::Char('x'), NONE)],
    ),
    b(
        Context::List(Screen::Rules),
        "Enter",
        "Show rule details",
        ENTER,
    ),
    b(
        Context::Editor(Screen::Rules),
        "↑/↓, Tab/Shift+Tab",
        "Switch field",
        SWITCH_FIELD,
    ),
    b(
        Context::Editor(Screen::Rules),
        "Space",
        "On Target Sink: open sink list · on Notify: cycle default/on/off",
        SPACE,
    ),
    b(Context::Editor(Screen::Rules), "Enter", "Save rule", ENTER),
    b(Context::Editor(Screen::Rules), "Esc", "Cancel editing", ESC),
    b(
        Context::Dropdown(Screen::Rules),
        "↑/↓",
        "Move highlight",
        UP_DOWN,
    ),
    b(
        Context::Dropdown(Screen::Rules),
        "Space/Enter",
        "Use highlighted sink",
        SPACE_ENTER,
    ),
    b(
        Context::Dropdown(Screen::Rules),
        "Esc",
        "Back to the editor",
        ESC,
    ),
    b(
        Context::Dialog(Screen::Rules),
        "Enter",
        "Confirm delete / close details",
        ENTER,
    ),
    b(
        Context::Dialog(Screen::Rules),
        "Esc",
        "Cancel / close details",
        ESC,
    ),
    // --- Settings ---
    b(
        Context::List(Screen::Settings),
        "↑/↓",
        "Navigate settings",
        UP_DOWN,
    ),
    b(
        Context::List(Screen::Settings),
        "Enter/Space",
        "Toggle setting / open log level list",
        SPACE_ENTER,
    ),
    b(
        Context::List(Screen::Settings),
        "PgUp/PgDn",
        "Scroll the description",
        PAGE,
    ),
    b(
        Context::Dropdown(Screen::Settings),
        "↑/↓",
        "Choose log level",
        UP_DOWN,
    ),
    b(
        Context::Dropdown(Screen::Settings),
        "Enter/Space",
        "Apply log level",
        SPACE_ENTER,
    ),
    b(Context::Dropdown(Screen::Settings), "Esc", "Cancel", ESC),
    // --- Text fields ---
    b(Context::TextInput, "←/→", "Move cursor", &[
        (KeyCode::Left, NONE),
        (KeyCode::Right, NONE),
    ]),
    b(Context::TextInput, "Ctrl+←/→", "Move by word", &[
        (KeyCode::Left, CTRL),
        (KeyCode::Right, CTRL),
    ]),
    b(
        Context::TextInput,
        "Home/End, Ctrl+A/E",
        "Jump to start/end",
        &[
            (KeyCode::Home, NONE),
            (KeyCode::End, NONE),
            (KeyCode::Char('a'), CTRL),
            (KeyCode::Char('e'), CTRL),
        ],
    ),
    b(
        Context::TextInput,
        "Backspace/Delete",
        "Delete character",
        &[(KeyCode::Backspace, NONE), (KeyCode::Delete, NONE)],
    ),
    b(
        Context::TextInput,
        "Ctrl+W, Alt+Backspace",
        "Delete previous word",
        &[(KeyCode::Char('w'), CTRL), (KeyCode::Backspace, ALT)],
    ),
    b(Context::TextInput, "Ctrl+Delete", "Delete next word", &[(
        KeyCode::Delete,
        CTRL,
    )]),
    b(Context::TextInput, "Ctrl+U", "Clear the field", &[(
        KeyCode::Char('u'),
        CTRL,
    )]),
    b(
        Context::TextInput,
        "Ctrl+K",
        "Delete from cursor to end",
        &[(KeyCode::Char('k'), CTRL)],
    ),
    b(
        Context::TextInput,
        "Paste",
        "Inserts the text (line breaks are dropped)",
        &[],
    ),
    // --- Global ---
    b(Context::Global, "Tab", "Next screen", &[(
        KeyCode::Tab,
        NONE,
    )]),
    b(Context::Global, "Shift+Tab", "Previous screen", &[(
        KeyCode::BackTab,
        SHIFT,
    )]),
    b(Context::Global, "1-4", "Jump directly to screen", &[
        (KeyCode::Char('1'), NONE),
        (KeyCode::Char('2'), NONE),
        (KeyCode::Char('3'), NONE),
        (KeyCode::Char('4'), NONE),
    ]),
    b(Context::Global, "Ctrl+S", "Save configuration", &[(
        KeyCode::Char('s'),
        CTRL,
    )]),
    b(
        Context::Global,
        "q, Ctrl+C",
        "Quit (with unsaved changes: press again to confirm)",
        &[(KeyCode::Char('q'), NONE), (KeyCode::Char('c'), CTRL)],
    ),
    b(Context::Global, "Esc", "Clear status message", ESC),
    b(Context::Global, "F1, ?", "Show this help", &[
        (KeyCode::F(1), NONE),
        (KeyCode::Char('?'), NONE),
    ]),
    b(Context::Help, "↑/↓", "Previous/next section", UP_DOWN),
    b(Context::Help, "Space", "Collapse/expand section", SPACE),
    b(Context::Help, "PgUp/PgDn, Home/End", "Scroll help", &[
        (KeyCode::PageUp, NONE),
        (KeyCode::PageDown, NONE),
        (KeyCode::Home, NONE),
        (KeyCode::End, NONE),
    ]),
    b(Context::Help, "Esc, q, F1, ?", "Close help", &[
        (KeyCode::Esc, NONE),
        (KeyCode::Char('q'), NONE),
        (KeyCode::F(1), NONE),
        (KeyCode::Char('?'), NONE),
    ]),
];

/// Regex examples listed in the Rules help section.
pub(crate) static REGEX_EXAMPLES: &[(&str, &str)] = &[
    ("firefox", "Matches anywhere in text"),
    ("^steam$", "Exact match only"),
    ("^chrome", "Starts with chrome"),
    ("^(mpv|vlc)$", "Match mpv OR vlc"),
    (".*", "Match all windows"),
    ("(?i)discord", "Case-insensitive"),
];
