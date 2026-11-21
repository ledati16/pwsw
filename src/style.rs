//! Terminal styling utilities
//!
//! CLI output goes through [`PwswStyle`], which wraps a value in [`Painted`]: it
//! carries a style and prints escape sequences only when colour is enabled for the
//! stream it is written to. Whether colour is enabled is decided once at startup by
//! [`init`] (`--color`, `NO_COLOR`, `CLICOLOR_FORCE`, and whether the stream is a
//! terminal); until then nothing is styled.
//!
//! Colour scheme ("Moderate+"):
//! - Semantic colours for status (green / yellow / red)
//! - Cyan for headers and technical terms
//! - Bold for important identifiers, dim for secondary information
//!
//! The TUI uses the [`colors`] palette with `ratatui` instead.

use std::ffi::OsStr;
use std::fmt;
use std::io::{IsTerminal, Write as _};
use std::sync::atomic::{AtomicBool, Ordering};

use crossterm::style::{Attribute, Color, ContentStyle, StyledContent};

// ============================================================================
// Writing output without panicking
// ============================================================================

/// Print a line to stdout like `println!`, but never panic (see [`write_stdout`])
#[macro_export]
macro_rules! outln {
    () => {
        $crate::style::write_stdout(::std::format_args!("\n"))
    };
    ($($arg:tt)*) => {
        $crate::style::write_stdout(::std::format_args!("{}\n", ::std::format_args!($($arg)*)))
    };
}

/// Print a line to stderr like `eprintln!`, but never panic (see [`write_stderr`])
#[macro_export]
macro_rules! errln {
    () => {
        $crate::style::write_stderr(::std::format_args!("\n"))
    };
    ($($arg:tt)*) => {
        $crate::style::write_stderr(::std::format_args!("{}\n", ::std::format_args!($($arg)*)))
    };
}

/// Write to stdout; never panics
///
/// `println!` panics when stdout is a pipe whose reader went away
/// (`pwsw completions bash | head -1`), and with `panic = "abort"` that is a
/// `SIGABRT` and a core dump. Here a vanished reader ends the process quietly with
/// status 0, since nothing more can be delivered; any other write error is reported
/// on stderr and ends the process with status 1.
pub fn write_stdout(args: fmt::Arguments<'_>) {
    let result = std::io::stdout().lock().write_fmt(args);
    stdout_written(result);
}

/// Write raw bytes to stdout and flush; never panics (see [`write_stdout`])
pub fn write_stdout_bytes(bytes: &[u8]) {
    let mut out = std::io::stdout().lock();
    let result = out.write_all(bytes).and_then(|()| out.flush());
    stdout_written(result);
}

fn stdout_written(result: std::io::Result<()>) {
    match result {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => std::process::exit(0),
        Err(e) => {
            write_stderr(format_args!("error: cannot write to stdout: {e}\n"));
            std::process::exit(1);
        }
    }
}

/// Write to stderr; never panics
///
/// Errors are ignored: stderr is where they would be reported.
pub fn write_stderr(args: fmt::Arguments<'_>) {
    let _ = std::io::stderr().lock().write_fmt(args);
}

// ============================================================================
// Colour decision
// ============================================================================

/// When to colour output (`--color`)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, clap::ValueEnum)]
pub enum ColorChoice {
    /// Colour when the stream is a terminal and `NO_COLOR` is unset or empty
    /// (`CLICOLOR_FORCE` forces colour on)
    #[default]
    Auto,
    /// Always colour, even when piped or with `NO_COLOR` set
    Always,
    /// Never colour
    Never,
}

/// Output stream a styled value is written to
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stream {
    Stdout,
    Stderr,
}

static STDOUT_COLOR: AtomicBool = AtomicBool::new(false);
static STDERR_COLOR: AtomicBool = AtomicBool::new(false);

/// Decide once, at startup, whether stdout and stderr get colour
///
/// With [`ColorChoice::Always`] this also overrides `crossterm`'s own `NO_COLOR`
/// handling, so the choice holds for everything drawn with `crossterm`.
pub fn init(choice: ColorChoice) {
    let no_color = std::env::var_os("NO_COLOR");
    let force = std::env::var_os("CLICOLOR_FORCE");
    let decide =
        |is_terminal| should_color(choice, is_terminal, no_color.as_deref(), force.as_deref());
    STDOUT_COLOR.store(decide(std::io::stdout().is_terminal()), Ordering::Relaxed);
    STDERR_COLOR.store(decide(std::io::stderr().is_terminal()), Ordering::Relaxed);
    if choice == ColorChoice::Always {
        crossterm::style::force_color_output(true);
    }
}

/// Whether styled output to `stream` emits escape sequences (see [`init`])
#[must_use]
pub fn color_enabled(stream: Stream) -> bool {
    match stream {
        Stream::Stdout => STDOUT_COLOR.load(Ordering::Relaxed),
        Stream::Stderr => STDERR_COLOR.load(Ordering::Relaxed),
    }
}

/// The colour rule: `--color` wins; otherwise a non-empty `NO_COLOR` turns colour
/// off, `CLICOLOR_FORCE` (set and not `0`) turns it on, and else it follows whether
/// the stream is a terminal.
fn should_color(
    choice: ColorChoice,
    is_terminal: bool,
    no_color: Option<&OsStr>,
    clicolor_force: Option<&OsStr>,
) -> bool {
    match choice {
        ColorChoice::Always => true,
        ColorChoice::Never => false,
        ColorChoice::Auto => {
            if no_color.is_some_and(|v| !v.is_empty()) {
                false
            } else if clicolor_force.is_some_and(|v| !v.is_empty() && v != "0") {
                true
            } else {
                is_terminal
            }
        }
    }
}

// ============================================================================
// Styled values
// ============================================================================

/// A value with a style, printed plain when colour is off for its stream
#[derive(Debug, Clone)]
#[must_use]
pub struct Painted<T> {
    value: T,
    style: ContentStyle,
    stream: Stream,
}

impl<T> Painted<T> {
    /// Mark this value as written to stderr (styled per stderr's colour decision)
    pub fn to_stderr(mut self) -> Self {
        self.stream = Stream::Stderr;
        self
    }
}

impl<T: fmt::Display> fmt::Display for Painted<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if color_enabled(self.stream) {
            StyledContent::new(self.style, &self.value).fmt(f)
        } else {
            self.value.fmt(f)
        }
    }
}

fn paint<T>(value: T, color: Option<Color>, attribute: Option<Attribute>) -> Painted<T> {
    let mut style = ContentStyle::new();
    style.foreground_color = color;
    if let Some(attribute) = attribute {
        style.attributes.set(attribute);
    }
    Painted {
        value,
        style,
        stream: Stream::Stdout,
    }
}

/// Semantic styling for CLI output
///
/// Styled values go to stdout by default; call [`Painted::to_stderr`] for values
/// printed to stderr.
///
/// # Examples
///
/// ```
/// use pwsw::style::PwswStyle;
///
/// println!("{}", "Section Header".header());
/// println!("{}", "Success message".success());
/// eprintln!("{}", "Something failed".error().to_stderr());
/// ```
pub trait PwswStyle: Sized + fmt::Display {
    /// Section headers like "ACTIVE SINKS:" (cyan bold)
    fn header(self) -> Painted<Self> {
        paint(self, Some(Color::Cyan), Some(Attribute::Bold))
    }

    /// Positive states: "Running", "active", success messages (green)
    fn success(self) -> Painted<Self> {
        paint(self, Some(Color::Green), None)
    }

    /// Problems: "not found", "Not running", error messages (red)
    fn error(self) -> Painted<Self> {
        paint(self, Some(Color::Red), None)
    }

    /// Warnings or partial states: "profile switch" (yellow)
    fn warning(self) -> Painted<Self> {
        paint(self, Some(Color::Yellow), None)
    }

    /// Technical content: regex patterns, counts, paths (cyan)
    fn technical(self) -> Painted<Self> {
        paint(self, Some(Color::Cyan), None)
    }

    /// Important identifiers: node names, descriptions (bold)
    fn bold(self) -> Painted<Self> {
        paint(self, None, Some(Attribute::Bold))
    }

    /// Secondary information: labels, markers (dim)
    fn dim(self) -> Painted<Self> {
        paint(self, None, Some(Attribute::Dim))
    }
}

impl<T: fmt::Display> PwswStyle for T {}

// ============================================================================
// Ratatui TUI Color Helpers
// ============================================================================

/// Semantic color palette for TUI use with `ratatui`
#[cfg(feature = "tui")]
pub mod colors {
    use ratatui::style::Color;

    // Log styling colors

    /// Color for log timestamps (dark gray)
    pub const LOG_TIMESTAMP: Color = Color::DarkGray;

    /// Color for INFO log level (green)
    pub const LOG_LEVEL_INFO: Color = Color::Green;

    /// Color for DEBUG log level (cyan)
    pub const LOG_LEVEL_DEBUG: Color = Color::Cyan;

    /// Color for TRACE log level (magenta)
    pub const LOG_LEVEL_TRACE: Color = Color::Magenta;

    /// Color for WARN log level (yellow, use with bold modifier)
    pub const LOG_LEVEL_WARN: Color = Color::Yellow;

    /// Color for ERROR log level (red, use with bold modifier)
    pub const LOG_LEVEL_ERROR: Color = Color::Red;

    /// Color for log keywords (`app_id=`, `title=`, etc.)
    pub const LOG_KEYWORD: Color = Color::Cyan;

    /// Color for log events ("Rule matched:", "Switching:", use with bold)
    pub const LOG_EVENT: Color = Color::Green;

    /// Color for log close events ("Window closed:", use with bold)
    pub const LOG_EVENT_CLOSE: Color = Color::Yellow;

    /// Color for log message text (default readable)
    pub const LOG_MESSAGE: Color = Color::White;

    // UI element colors

    /// Color for active/live UI borders (green)
    pub const UI_BORDER_ACTIVE: Color = Color::Green;

    /// Color for inactive/stopped UI borders (gray)
    pub const UI_BORDER_INACTIVE: Color = Color::Gray;

    /// Color for selected UI elements (cyan, use with bold)
    pub const UI_SELECTED: Color = Color::Cyan;

    /// Background color for selected UI elements (dark gray)
    pub const UI_SELECTED_BG: Color = Color::DarkGray;

    /// Color for UI highlights (cyan)
    pub const UI_HIGHLIGHT: Color = Color::Cyan;

    /// Color for statistics/counts (yellow, use with bold)
    pub const UI_STAT: Color = Color::Yellow;

    /// Color for success states (green)
    pub const UI_SUCCESS: Color = Color::Green;

    /// Color for error states (red)
    pub const UI_ERROR: Color = Color::Red;

    /// Color for warning states (yellow)
    pub const UI_WARNING: Color = Color::Yellow;

    /// Color for secondary/dimmed text (gray)
    pub const UI_SECONDARY: Color = Color::Gray;

    /// Color for normal UI text (white)
    pub const UI_TEXT: Color = Color::White;

    /// Color for focused input fields (magenta, use with bold for borders)
    pub const UI_FOCUS: Color = Color::Magenta;

    /// Color for valid input/state (green, lighter than success)
    pub const UI_VALID: Color = Color::Green;

    /// Color for invalid input/state (red, for validation feedback)
    pub const UI_INVALID: Color = Color::Red;

    /// Color for matched/active items (green, for items that match rules)
    pub const UI_MATCHED: Color = Color::Green;

    /// Color for unmatched/inactive items (dark gray, dimmed)
    pub const UI_UNMATCHED: Color = Color::DarkGray;

    /// Background color for modals, overlays, and popups (black for contrast)
    pub const UI_MODAL_BG: Color = Color::Black;
}

#[cfg(test)]
mod tests {
    use test_case::test_case;

    use super::*;

    #[test_case(ColorChoice::Auto, true, None, None, true ; "auto terminal")]
    #[test_case(ColorChoice::Auto, false, None, None, false ; "auto piped")]
    #[test_case(ColorChoice::Auto, true, Some("1"), None, false ; "no_color on terminal")]
    #[test_case(ColorChoice::Auto, true, Some(""), None, true ; "empty no_color ignored")]
    #[test_case(ColorChoice::Auto, false, None, Some("1"), true ; "clicolor_force piped")]
    #[test_case(ColorChoice::Auto, false, None, Some("0"), false ; "clicolor_force zero")]
    #[test_case(ColorChoice::Auto, true, Some("1"), Some("1"), false ; "no_color beats force")]
    #[test_case(ColorChoice::Always, false, Some("1"), None, true ; "always wins")]
    #[test_case(ColorChoice::Never, true, None, Some("1"), false ; "never wins")]
    fn test_should_color(
        choice: ColorChoice,
        is_terminal: bool,
        no_color: Option<&str>,
        force: Option<&str>,
        expected: bool,
    ) {
        assert_eq!(
            should_color(
                choice,
                is_terminal,
                no_color.map(OsStr::new),
                force.map(OsStr::new)
            ),
            expected
        );
    }

    #[test]
    fn test_painted_is_plain_when_colour_off() {
        // `init` is never called in unit tests, so both streams are uncoloured
        assert_eq!("x".header().to_string(), "x");
        assert_eq!("y".dim().to_stderr().to_string(), "y");
    }
}
