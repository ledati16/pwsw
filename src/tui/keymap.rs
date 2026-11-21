//! The key bindings of the TUI, in one table.
//!
//! The help overlay is rendered from [`BINDINGS`] (and its section navigation
//! counts rows of the same table), so documentation cannot drift from what the
//! overlay shows. Each binding also lists the concrete key presses it documents;
//! a test feeds every one of them to the input handler in the binding's context
//! and fails if any does nothing, so documented keys cannot drift from the code
//! either.
//!
//! The footer's key hints come from the same table: a binding with a [`Hint`]
//! shows it in the footer whenever its context is active (see [`footer_hints`]).

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
    /// The "discard changes?" question after Esc in an editor with edits
    Discard(Screen),
    /// The "config changed on disk" dialog shown when a save was refused
    Conflict,
    /// A focused text field in an editor
    TextInput,
    /// The help overlay
    Help,
}

impl Context {
    /// The screen whose help section shows this binding (`None`: a shared section)
    pub(crate) const fn screen(self) -> Option<Screen> {
        match self {
            Self::List(s)
            | Self::Editor(s)
            | Self::Dropdown(s)
            | Self::Dialog(s)
            | Self::Discard(s) => Some(s),
            Self::Global | Self::TextInput | Self::Help | Self::Conflict => None,
        }
    }

    /// Sub-heading shown above this context's bindings inside a screen section
    pub(crate) const fn heading(self) -> Option<&'static str> {
        match self {
            Self::Editor(_) => Some("In the add/edit dialog:"),
            Self::Dropdown(_) => Some("In a selection list:"),
            Self::Dialog(_) => Some("In the delete/details dialog:"),
            Self::Discard(_) => Some("When asked to discard changes:"),
            Self::List(_) | Self::Global | Self::TextInput | Self::Help | Self::Conflict => None,
        }
    }
}

/// The short form of a binding shown in the footer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Hint {
    /// The keys, short ("Enter", "a", "Ctrl+S")
    pub(crate) keys: &'static str,
    /// One or two words ("save", "add")
    pub(crate) label: &'static str,
    /// Lower is more important: when the footer is too narrow, hints with the
    /// highest numbers are left out first
    pub(crate) priority: u8,
    /// Only shown while the config has unsaved changes
    pub(crate) only_unsaved: bool,
}

impl Hint {
    /// Columns the hint takes in the footer ("keys label")
    pub(crate) fn width(&self) -> usize {
        crate::tui::widgets::text_width(self.keys) + 1 + crate::tui::widgets::text_width(self.label)
    }
}

/// One documented binding.
pub(crate) struct Binding {
    pub(crate) context: Context,
    /// The keys as shown in the help
    pub(crate) keys: &'static str,
    pub(crate) description: &'static str,
    /// Footer hint, if the binding is worth one
    pub(crate) hint: Option<Hint>,
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
        hint: None,
        #[cfg(test)]
        chords,
    }
}

impl Binding {
    /// Show this binding in the footer as `keys label`
    const fn hint(self, keys: &'static str, label: &'static str, priority: u8) -> Self {
        Self {
            hint: Some(Hint {
                keys,
                label,
                priority,
                only_unsaved: false,
            }),
            ..self
        }
    }

    /// Like [`Self::hint`], only while there are unsaved changes
    const fn hint_unsaved(self, keys: &'static str, label: &'static str, priority: u8) -> Self {
        Self {
            hint: Some(Hint {
                keys,
                label,
                priority,
                only_unsaved: true,
            }),
            ..self
        }
    }
}

/// The footer hints for the active `contexts` that fit in `width` columns
/// (hints are separated by two spaces), in table order. When not all fit, the
/// least important are dropped; a hint is never cut.
pub(crate) fn footer_hints(contexts: &[Context], unsaved: bool, width: usize) -> Vec<Hint> {
    let mut candidates: Vec<(usize, Hint)> = BINDINGS
        .iter()
        .enumerate()
        .filter(|(_, b)| contexts.contains(&b.context))
        .filter_map(|(i, b)| b.hint.map(|h| (i, h)))
        .filter(|(_, h)| unsaved || !h.only_unsaved)
        .collect();
    candidates.sort_by_key(|(i, h)| (h.priority, *i));
    let mut chosen = Vec::new();
    let mut used = 0;
    for (i, hint) in candidates {
        let needed = hint.width() + if chosen.is_empty() { 0 } else { 2 };
        if used + needed > width {
            break;
        }
        used += needed;
        chosen.push((i, hint));
    }
    chosen.sort_by_key(|(i, _)| *i);
    chosen.into_iter().map(|(_, h)| h).collect()
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
    )
    .hint("←/→", "action", 2),
    b(
        Context::List(Screen::Dashboard),
        "Enter",
        "Run selected action (start/stop/restart; enable/disable when the systemd unit is \
         installed)",
        ENTER,
    )
    .hint("Enter", "run", 1),
    b(
        Context::List(Screen::Dashboard),
        "w",
        "Toggle between Logs ↔ Windows view",
        &[(KeyCode::Char('w'), NONE)],
    )
    .hint("w", "logs/windows", 3),
    b(
        Context::List(Screen::Dashboard),
        "↑/↓",
        "Scroll one line",
        UP_DOWN,
    )
    .hint("↑/↓", "scroll", 5),
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
    )
    .hint("Shift+↑/↓", "move", 6),
    b(Context::List(Screen::Sinks), "a", "Add new sink", &[(
        KeyCode::Char('a'),
        NONE,
    )])
    .hint("a", "add", 1),
    b(Context::List(Screen::Sinks), "e", "Edit selected sink", &[
        (KeyCode::Char('e'), NONE),
    ])
    .hint("e", "edit", 2),
    b(
        Context::List(Screen::Sinks),
        "x",
        "Delete selected sink",
        &[(KeyCode::Char('x'), NONE)],
    )
    .hint("x", "delete", 4),
    b(
        Context::List(Screen::Sinks),
        "Space",
        "Make selected sink the fallback (used when no rule matches)",
        SPACE,
    )
    .hint("Space", "fallback", 5),
    b(
        Context::List(Screen::Sinks),
        "Enter",
        "Show sink details",
        ENTER,
    )
    .hint("Enter", "details", 7),
    b(
        Context::Editor(Screen::Sinks),
        "↑/↓, Tab/Shift+Tab",
        "Switch field",
        SWITCH_FIELD,
    )
    .hint("↑/↓", "field", 4),
    b(
        Context::Editor(Screen::Sinks),
        "Space",
        "On Node: pick from PipeWire nodes · on Fallback: toggle",
        SPACE,
    )
    .hint("Space", "pick/toggle", 3),
    b(Context::Editor(Screen::Sinks), "Enter", "Save sink", ENTER).hint("Enter", "save", 1),
    b(
        Context::Editor(Screen::Sinks),
        "Esc",
        "Cancel editing (asks first if anything was changed)",
        ESC,
    )
    .hint("Esc", "cancel", 2),
    b(
        Context::Dropdown(Screen::Sinks),
        "↑/↓",
        "Move highlight",
        UP_DOWN,
    )
    .hint("↑/↓", "move", 3),
    b(
        Context::Dropdown(Screen::Sinks),
        "Space/Enter",
        "Use highlighted node",
        SPACE_ENTER,
    )
    .hint("Enter", "use", 1),
    b(
        Context::Dropdown(Screen::Sinks),
        "Esc",
        "Back to the editor",
        ESC,
    )
    .hint("Esc", "back", 2),
    b(
        Context::Dialog(Screen::Sinks),
        "Enter",
        "Confirm delete / close details",
        ENTER,
    )
    .hint("Enter", "ok", 1),
    b(
        Context::Dialog(Screen::Sinks),
        "Esc",
        "Cancel / close details",
        ESC,
    )
    .hint("Esc", "back", 2),
    b(
        Context::Discard(Screen::Sinks),
        "y",
        "Discard the changes and close the editor",
        &[(KeyCode::Char('y'), NONE)],
    )
    .hint("y", "discard", 1),
    b(
        Context::Discard(Screen::Sinks),
        "n, Esc",
        "Keep editing",
        &[(KeyCode::Char('n'), NONE), (KeyCode::Esc, NONE)],
    )
    .hint("n", "keep editing", 2),
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
    )
    .hint("Shift+↑/↓", "move", 5),
    b(Context::List(Screen::Rules), "a", "Add new rule", &[(
        KeyCode::Char('a'),
        NONE,
    )])
    .hint("a", "add", 1),
    b(Context::List(Screen::Rules), "e", "Edit selected rule", &[
        (KeyCode::Char('e'), NONE),
    ])
    .hint("e", "edit", 2),
    b(
        Context::List(Screen::Rules),
        "x",
        "Delete selected rule",
        &[(KeyCode::Char('x'), NONE)],
    )
    .hint("x", "delete", 4),
    b(
        Context::List(Screen::Rules),
        "Enter",
        "Show rule details",
        ENTER,
    )
    .hint("Enter", "details", 6),
    b(
        Context::Editor(Screen::Rules),
        "↑/↓, Tab/Shift+Tab",
        "Switch field",
        SWITCH_FIELD,
    )
    .hint("↑/↓", "field", 4),
    b(
        Context::Editor(Screen::Rules),
        "Space",
        "On Sink: open sink list · on Notify: cycle default/on/off",
        SPACE,
    )
    .hint("Space", "choose", 3),
    b(Context::Editor(Screen::Rules), "Enter", "Save rule", ENTER).hint("Enter", "save", 1),
    b(
        Context::Editor(Screen::Rules),
        "Esc",
        "Cancel editing (asks first if anything was changed)",
        ESC,
    )
    .hint("Esc", "cancel", 2),
    b(
        Context::Dropdown(Screen::Rules),
        "↑/↓",
        "Move highlight",
        UP_DOWN,
    )
    .hint("↑/↓", "move", 3),
    b(
        Context::Dropdown(Screen::Rules),
        "Space/Enter",
        "Use highlighted sink",
        SPACE_ENTER,
    )
    .hint("Enter", "use", 1),
    b(
        Context::Dropdown(Screen::Rules),
        "Esc",
        "Back to the editor",
        ESC,
    )
    .hint("Esc", "back", 2),
    b(
        Context::Dialog(Screen::Rules),
        "Enter",
        "Confirm delete / close details",
        ENTER,
    )
    .hint("Enter", "ok", 1),
    b(
        Context::Dialog(Screen::Rules),
        "Esc",
        "Cancel / close details",
        ESC,
    )
    .hint("Esc", "back", 2),
    b(
        Context::Discard(Screen::Rules),
        "y",
        "Discard the changes and close the editor",
        &[(KeyCode::Char('y'), NONE)],
    )
    .hint("y", "discard", 1),
    b(
        Context::Discard(Screen::Rules),
        "n, Esc",
        "Keep editing",
        &[(KeyCode::Char('n'), NONE), (KeyCode::Esc, NONE)],
    )
    .hint("n", "keep editing", 2),
    // --- Settings ---
    b(
        Context::List(Screen::Settings),
        "↑/↓",
        "Navigate settings",
        UP_DOWN,
    )
    .hint("↑/↓", "select", 4),
    b(
        Context::List(Screen::Settings),
        "Enter/Space",
        "Toggle setting / open log level list",
        SPACE_ENTER,
    )
    .hint("Enter", "change", 1),
    b(
        Context::List(Screen::Settings),
        "PgUp/PgDn",
        "Scroll the description",
        PAGE,
    )
    .hint("PgUp/PgDn", "scroll text", 5),
    b(
        Context::Dropdown(Screen::Settings),
        "↑/↓",
        "Choose log level",
        UP_DOWN,
    )
    .hint("↑/↓", "choose", 3),
    b(
        Context::Dropdown(Screen::Settings),
        "Enter/Space",
        "Apply log level",
        SPACE_ENTER,
    )
    .hint("Enter", "apply", 1),
    b(Context::Dropdown(Screen::Settings), "Esc", "Cancel", ESC).hint("Esc", "cancel", 2),
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
    // --- Changed-on-disk dialog (its keys are listed in the dialog itself) ---
    b(
        Context::Conflict,
        "o",
        "Overwrite the file with your changes",
        &[(KeyCode::Char('o'), NONE)],
    )
    .hint("o", "overwrite", 1),
    b(
        Context::Conflict,
        "r",
        "Reload from disk (discards your unsaved edits)",
        &[(KeyCode::Char('r'), NONE)],
    )
    .hint("r", "reload", 1),
    b(
        Context::Conflict,
        "Esc",
        "Cancel (keep editing, nothing saved)",
        ESC,
    )
    .hint("Esc", "cancel", 1),
    // --- Global ---
    b(Context::Global, "Tab", "Next screen", &[(
        KeyCode::Tab,
        NONE,
    )])
    .hint("Tab", "next screen", 8),
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
    )])
    .hint_unsaved("Ctrl+S", "save", 0),
    b(
        Context::Global,
        "q, Ctrl+C",
        "Quit (with unsaved changes: press again to confirm)",
        &[(KeyCode::Char('q'), NONE), (KeyCode::Char('c'), CTRL)],
    )
    .hint("q", "quit", 1),
    b(Context::Global, "Esc", "Clear status message", ESC),
    b(Context::Global, "F1, ?", "Show this help", &[
        (KeyCode::F(1), NONE),
        (KeyCode::Char('?'), NONE),
    ])
    .hint("?", "help", 3),
    b(Context::Help, "↑/↓", "Previous/next section", UP_DOWN).hint("↑/↓", "sections", 2),
    b(Context::Help, "Space", "Collapse/expand section", SPACE).hint("Space", "fold", 3),
    b(Context::Help, "PgUp/PgDn, Home/End", "Scroll help", &[
        (KeyCode::PageUp, NONE),
        (KeyCode::PageDown, NONE),
        (KeyCode::Home, NONE),
        (KeyCode::End, NONE),
    ])
    .hint("PgUp/PgDn", "scroll", 4),
    b(Context::Help, "Esc, q, F1, ?", "Close help", &[
        (KeyCode::Esc, NONE),
        (KeyCode::Char('q'), NONE),
        (KeyCode::F(1), NONE),
        (KeyCode::Char('?'), NONE),
    ])
    .hint("Esc", "close", 1),
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
