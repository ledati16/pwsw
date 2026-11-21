//! Help overlay - Context-aware keyboard shortcut reference
//!
//! A scrolled text view generated from [`crate::tui::keymap::BINDINGS`]: the
//! current screen's bindings, then the text input and global ones.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};

use crate::tui::app::Screen;
use crate::tui::keymap::{BINDINGS, Binding, Context, REGEX_EXAMPLES};
use crate::tui::theme;
use crate::tui::widgets::{
    border_hints, dialog_block, modal_rect, render_scrollbar, text_width, wrap_line,
};

/// Widest the help overlay gets
const HELP_WIDTH: u16 = 100;

/// Width of the key column, and the gap after it
const KEY_COLUMN_WIDTH: usize = 22;
const KEY_GAP: usize = 2;

/// Scroll position of the help overlay. The page size and the scroll limit
/// depend on the viewport, so rendering records them here.
#[derive(Debug)]
pub struct HelpScroll {
    /// First line shown
    pub offset: usize,
    /// Lines of the viewport at the last render
    page: usize,
    /// Largest useful `offset` at the last render
    max_offset: usize,
}

impl HelpScroll {
    pub(crate) const fn new() -> Self {
        Self {
            offset: 0,
            page: 10,
            max_offset: usize::MAX,
        }
    }

    /// Lines one page scrolls by
    pub(crate) const fn page(&self) -> usize {
        if self.page == 0 { 1 } else { self.page }
    }

    pub(crate) const fn up(&mut self, lines: usize) {
        self.offset = self.offset.saturating_sub(lines);
    }

    pub(crate) fn down(&mut self, lines: usize) {
        self.offset = self.offset.saturating_add(lines).min(self.max_offset);
    }

    pub(crate) const fn scroll_to_end(&mut self) {
        self.offset = self.max_offset;
    }
}

/// A section heading
fn section(name: &str) -> Line<'static> {
    Line::from(Span::styled(
        name.to_string(),
        theme::bold().add_modifier(Modifier::UNDERLINED),
    ))
}

/// A line in the description column (sub-heading)
fn hint(text: &'static str) -> Line<'static> {
    Line::from(vec![
        Span::raw(" ".repeat(KEY_COLUMN_WIDTH + KEY_GAP)),
        Span::styled(text, theme::dim()),
    ])
}

/// `keys` in the key column, `description` wrapped in the description column
fn key_lines(lines: &mut Vec<Line<'static>>, keys: &'static str, description: &str, width: usize) {
    let indent = KEY_COLUMN_WIDTH + KEY_GAP;
    for (i, row) in wrap_line(&Line::from(description), width.saturating_sub(indent))
        .into_iter()
        .enumerate()
    {
        let mut spans = if i == 0 {
            let pad = indent.saturating_sub(text_width(keys)).max(1);
            vec![Span::styled(keys, theme::key()), Span::raw(" ".repeat(pad))]
        } else {
            vec![Span::raw(" ".repeat(indent))]
        };
        spans.extend(row.spans);
        lines.push(Line::from(spans));
    }
}

/// Lines for `bindings`, a sub-heading above each context that has one
fn push_bindings<'a>(
    lines: &mut Vec<Line<'static>>,
    bindings: impl Iterator<Item = &'a Binding>,
    width: usize,
) {
    let mut last_heading = None;
    for binding in bindings {
        let heading = binding.context.heading();
        if heading.is_some() && heading != last_heading {
            lines.push(Line::from(""));
            lines.extend(heading.map(hint));
        }
        last_heading = heading;
        key_lines(lines, binding.keys, binding.description, width);
    }
}

/// The help text for `screen`, `width` columns wide: the screen's bindings,
/// then text input and global ones
fn help_lines(screen: Screen, width: usize) -> Vec<Line<'static>> {
    let mut lines = vec![section(&format!("{} Screen", screen.name()))];
    push_bindings(
        &mut lines,
        BINDINGS
            .iter()
            .filter(|b| b.context.screen() == Some(screen)),
        width,
    );
    if screen == Screen::Rules {
        lines.push(Line::from(""));
        lines.push(hint("Regex Examples:"));
        for (pattern, description) in REGEX_EXAMPLES {
            key_lines(&mut lines, pattern, description, width);
        }
    }
    lines.push(Line::from(""));
    lines.push(section("Text Input Fields"));
    push_bindings(
        &mut lines,
        BINDINGS.iter().filter(|b| b.context == Context::TextInput),
        width,
    );
    lines.push(Line::from(""));
    lines.push(section("Global Shortcuts"));
    push_bindings(
        &mut lines,
        BINDINGS
            .iter()
            .filter(|b| matches!(b.context, Context::Global | Context::Help)),
        width,
    );
    lines
}

/// Render help overlay on top of the current screen
pub fn render_help(frame: &mut Frame, area: Rect, screen: Screen, scroll: &mut HelpScroll) {
    // As wide as useful, as tall as the screen allows (it scrolls)
    let popup = modal_rect(area, HELP_WIDTH, area.height);
    let block =
        dialog_block("Help").title_bottom(border_hints(&[("↑/↓", "scroll"), ("Esc", "close")]));
    let inner = block.inner(popup);
    let lines = help_lines(screen, usize::from(inner.width));

    scroll.page = usize::from(inner.height);
    scroll.max_offset = lines.len().saturating_sub(scroll.page);
    scroll.offset = scroll.offset.min(scroll.max_offset);

    let total = lines.len();
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(lines)
            .block(block)
            .scroll((u16::try_from(scroll.offset).unwrap_or(u16::MAX), 0)),
        popup,
    );
    render_scrollbar(frame, popup, 0, total, scroll.offset);
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn test_help_shows_every_binding_of_the_screen() {
        for &screen in Screen::all() {
            // Wide enough that no description wraps
            let text: Vec<String> = help_lines(screen, 400)
                .iter()
                .map(ToString::to_string)
                .collect();
            let headings: Vec<&str> = text
                .iter()
                .filter(|l| {
                    l.ends_with("Screen") || l.ends_with("Fields") || l.ends_with("Shortcuts")
                })
                .map(String::as_str)
                .collect();
            let screen_heading = format!("{} Screen", screen.name());
            assert_eq!(headings, vec![
                screen_heading.as_str(),
                "Text Input Fields",
                "Global Shortcuts"
            ]);
            let shown = BINDINGS.iter().filter(|b| {
                b.context.screen() == Some(screen)
                    || matches!(
                        b.context,
                        Context::TextInput | Context::Global | Context::Help
                    )
            });
            for binding in shown {
                assert!(
                    text.iter()
                        .any(|l| l.starts_with(binding.keys) && l.contains(binding.description)),
                    "{screen:?}: {}",
                    binding.keys
                );
            }
        }
    }

    #[test]
    fn test_long_descriptions_wrap_under_the_description_column() {
        let lines = help_lines(Screen::Dashboard, 60);
        for line in &lines {
            assert!(text_width(line.to_string().trim_end()) <= 60, "{line}");
        }
        let indent = " ".repeat(KEY_COLUMN_WIDTH + KEY_GAP);
        assert!(
            lines
                .iter()
                .any(|l| l.to_string().starts_with(&indent) && !l.to_string().trim().is_empty()),
            "a wrapped continuation line"
        );
    }
}
