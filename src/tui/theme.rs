//! Every style the TUI draws with, in one place.
//!
//! Body text keeps the terminal's own foreground and background, so it reads on
//! light and dark themes alike; colour only marks accents and states. Selection
//! is reverse video, which every terminal shows. When colour is off
//! (`--color never`, `NO_COLOR`; decided once by [`crate::style::init`]) every
//! style here drops its colours and keeps only modifiers (bold, dim, reversed).

use ratatui::style::{Color, Modifier, Style};

/// Whether styles may carry colours
#[cfg(not(test))]
fn colors_enabled() -> bool {
    crate::style::colors_enabled()
}

#[cfg(test)]
thread_local! {
    /// Tests render with colour unless [`without_colors`] says otherwise
    static NO_COLOR: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[cfg(test)]
fn colors_enabled() -> bool {
    !NO_COLOR.with(std::cell::Cell::get)
}

/// Run `f` with colour turned off on this thread (tests only)
#[cfg(test)]
pub(crate) fn without_colors<R>(f: impl FnOnce() -> R) -> R {
    struct Restore;
    impl Drop for Restore {
        fn drop(&mut self) {
            NO_COLOR.with(|c| c.set(false));
        }
    }
    NO_COLOR.with(|c| c.set(true));
    let _restore = Restore;
    f()
}

/// `base` in `color` when colour is enabled, `base` plus `fallback` otherwise
fn paint(color: Color, base: Style, fallback: Modifier) -> Style {
    if colors_enabled() {
        base.fg(color)
    } else {
        base.add_modifier(fallback)
    }
}

/// Normal text: the terminal's own colours
pub(crate) const fn text() -> Style {
    Style::new()
}

/// Secondary text: labels, hints, placeholders
pub(crate) const fn dim() -> Style {
    Style::new().add_modifier(Modifier::DIM)
}

/// Emphasised text: titles, table headers, the focused label
pub(crate) const fn bold() -> Style {
    Style::new().add_modifier(Modifier::BOLD)
}

/// Things to notice: values, sink names in the window list
pub(crate) fn accent() -> Style {
    paint(Color::Cyan, Style::new(), Modifier::empty())
}

/// A key in a hint or dialog ("Enter", "a")
pub(crate) fn key() -> Style {
    paint(Color::Cyan, bold(), Modifier::empty())
}

/// Good states: running, playing, saved, a valid pattern
pub(crate) fn success() -> Style {
    paint(Color::Green, Style::new(), Modifier::empty())
}

/// Attention: unsaved changes, warnings
pub(crate) fn warning() -> Style {
    paint(Color::Yellow, Style::new(), Modifier::BOLD)
}

/// Problems: stopped, failed, an invalid pattern
pub(crate) fn error() -> Style {
    paint(Color::Red, Style::new(), Modifier::BOLD)
}

/// The border of the focused panel or field marker
pub(crate) fn focus() -> Style {
    paint(Color::Magenta, Style::new(), Modifier::BOLD)
}

/// The selected row of a list or table, the active tab, the chosen button
pub(crate) const fn selected() -> Style {
    Style::new().add_modifier(Modifier::REVERSED)
}

/// Symbol in front of the selected row (lists and tables reserve its width always)
pub(crate) const SELECTED_SYMBOL: &str = "▌ ";

/// Style of a log level
pub(crate) fn log_level(level: &str) -> Style {
    match level {
        "ERROR" => error().add_modifier(Modifier::BOLD),
        "WARN" => warning().add_modifier(Modifier::BOLD),
        "INFO" => success(),
        "DEBUG" => accent(),
        "TRACE" => paint(Color::Magenta, Style::new(), Modifier::empty()),
        _ => text(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_styles_have_no_colors_when_disabled() {
        let all = || {
            [
                text(),
                dim(),
                bold(),
                accent(),
                key(),
                success(),
                warning(),
                error(),
                focus(),
                selected(),
                log_level("ERROR"),
                log_level("TRACE"),
            ]
        };
        assert!(all().iter().any(|s| s.fg.is_some()), "colours by default");
        without_colors(|| {
            for style in all() {
                assert_eq!((style.fg, style.bg), (None, None), "{style:?}");
            }
        });
        assert!(all().iter().any(|s| s.fg.is_some()), "restored afterwards");
    }
}
