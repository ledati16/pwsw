//! Help overlay - Context-aware keyboard shortcut reference
//!
//! Everything shown comes from [`crate::tui::keymap::BINDINGS`]; the row layout
//! computed by [`help_layout`] is shared by rendering and section navigation.

use std::collections::HashSet;

use ratatui::Frame;
use ratatui::layout::{Constraint, Rect};
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Cell, Clear, Row, Table, TableState};

use crate::tui::app::Screen;
use crate::tui::keymap::{BINDINGS, Binding, Context, REGEX_EXAMPLES};
use crate::tui::theme;
use crate::tui::widgets::{border_hints, dialog_block, modal_rect, render_scrollbar};

/// Widest the help overlay gets
const HELP_WIDTH: u16 = 100;

/// Width of the key column
const KEY_COLUMN_WIDTH: u16 = 22;

/// Name of the shared text-input section
const TEXT_INPUT_SECTION: &str = "Text Input Fields";
/// Name of the shared global section
const GLOBAL_SECTION: &str = "Global Shortcuts";

/// One row of the help table.
#[derive(Debug, Clone, PartialEq, Eq)]
enum HelpRow {
    /// Collapsible section header
    Section { name: String, collapsed: bool },
    /// Sub-heading or spacer inside a section
    Hint(&'static str),
    /// A key binding
    Key(&'static str, &'static str),
}

/// Name of the screen-specific section
fn screen_section(screen: Screen) -> String {
    format!("{} Screen", screen.name())
}

fn push_bindings<'a>(rows: &mut Vec<HelpRow>, bindings: impl Iterator<Item = &'a Binding>) {
    let mut last_heading = None;
    for binding in bindings {
        let heading = binding.context.heading();
        if heading.is_some() && heading != last_heading {
            rows.push(HelpRow::Hint(""));
            rows.extend(heading.map(HelpRow::Hint));
        }
        last_heading = heading;
        rows.push(HelpRow::Key(binding.keys, binding.description));
    }
}

/// The rows of the help overlay for `screen` with `collapsed` sections folded.
fn help_layout(screen: Screen, collapsed: &HashSet<String>) -> Vec<HelpRow> {
    let mut rows = Vec::new();
    let section = |rows: &mut Vec<HelpRow>, name: String| -> bool {
        let is_collapsed = collapsed.contains(&name);
        rows.push(HelpRow::Section {
            name,
            collapsed: is_collapsed,
        });
        !is_collapsed
    };

    if section(&mut rows, screen_section(screen)) {
        push_bindings(
            &mut rows,
            BINDINGS
                .iter()
                .filter(|b| b.context.screen() == Some(screen)),
        );
        if screen == Screen::Rules {
            rows.push(HelpRow::Hint(""));
            rows.push(HelpRow::Hint("Regex Examples:"));
            rows.extend(REGEX_EXAMPLES.iter().map(|(p, d)| HelpRow::Key(p, d)));
        }
    }
    if section(&mut rows, TEXT_INPUT_SECTION.to_string()) {
        push_bindings(
            &mut rows,
            BINDINGS.iter().filter(|b| b.context == Context::TextInput),
        );
    }
    if section(&mut rows, GLOBAL_SECTION.to_string()) {
        push_bindings(
            &mut rows,
            BINDINGS
                .iter()
                .filter(|b| matches!(b.context, Context::Global | Context::Help)),
        );
    }
    rows
}

/// Get the number of rows in the help content for a given screen
pub fn get_help_row_count(current_screen: Screen, collapsed_sections: &HashSet<String>) -> usize {
    help_layout(current_screen, collapsed_sections).len()
}

/// Get the maximum scroll offset for the help screen given viewport height
pub fn get_help_max_offset(
    current_screen: Screen,
    collapsed_sections: &HashSet<String>,
    viewport_height: usize,
) -> usize {
    get_help_row_count(current_screen, collapsed_sections).saturating_sub(viewport_height)
}

/// Find the next section header row index starting from current row
pub fn find_next_section_header(
    current_screen: Screen,
    collapsed_sections: &HashSet<String>,
    current_row: usize,
) -> Option<usize> {
    help_layout(current_screen, collapsed_sections)
        .iter()
        .enumerate()
        .skip(current_row + 1)
        .find(|(_, row)| matches!(row, HelpRow::Section { .. }))
        .map(|(i, _)| i)
}

/// Find the previous section header row index starting from current row
pub fn find_prev_section_header(
    current_screen: Screen,
    collapsed_sections: &HashSet<String>,
    current_row: usize,
) -> Option<usize> {
    help_layout(current_screen, collapsed_sections)
        .iter()
        .enumerate()
        .take(current_row)
        .rev()
        .find(|(_, row)| matches!(row, HelpRow::Section { .. }))
        .map(|(i, _)| i)
}

/// Get the section name at a given row index if it's a section header
pub fn get_section_at_row(
    current_screen: Screen,
    collapsed_sections: &HashSet<String>,
    row_index: usize,
) -> Option<String> {
    match help_layout(current_screen, collapsed_sections)
        .into_iter()
        .nth(row_index)
    {
        Some(HelpRow::Section { name, .. }) => Some(name),
        _ => None,
    }
}

/// Word-wrap `text` to `width` columns; returns the text and its row count.
fn wrap_text(text: &str, width: usize) -> (String, u16) {
    let mut lines: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut current_width = 0;
    for word in text.split_whitespace() {
        let word_width = word.chars().count();
        let space = usize::from(current_width != 0);
        if current_width + space + word_width <= width || current_width == 0 {
            if space > 0 {
                current.push(' ');
            }
            current.push_str(word);
            current_width += space + word_width;
        } else {
            lines.push(std::mem::take(&mut current));
            current.push_str(word);
            current_width = word_width;
        }
    }
    lines.push(current);
    let height = u16::try_from(lines.len()).unwrap_or(u16::MAX);
    (lines.join("\n"), height)
}

/// Turn the layout into table rows, wrapping descriptions to `desc_width`
fn build_help_rows(layout: Vec<HelpRow>, desc_width: usize) -> Vec<Row<'static>> {
    layout
        .into_iter()
        .map(|row| match row {
            HelpRow::Section { name, collapsed } => {
                let indicator = if collapsed { "›" } else { "▾" };
                let action = if collapsed { "expand" } else { "collapse" };
                let hint_style = theme::dim().add_modifier(Modifier::ITALIC);
                Row::new(vec![
                    Cell::from(Span::styled(
                        format!("{indicator} {name}"),
                        theme::bold().add_modifier(Modifier::UNDERLINED),
                    )),
                    Cell::from(Line::from(vec![
                        Span::styled("Space", theme::key()),
                        Span::styled(format!(" to {action}"), hint_style),
                    ])),
                ])
            }
            HelpRow::Hint(text) => Row::new(vec![
                Cell::from(""),
                Cell::from(Span::styled(text, theme::dim())),
            ]),
            HelpRow::Key(keys, description) => {
                let (wrapped, height) = wrap_text(description, desc_width);
                Row::new(vec![
                    Cell::from(Span::styled(keys, theme::key())),
                    Cell::from(wrapped),
                ])
                .height(height)
            }
        })
        .collect()
}

/// Render help overlay on top of the current screen
pub fn render_help(
    frame: &mut Frame,
    area: Rect,
    current_screen: Screen,
    scroll_state: &mut TableState,
    viewport_height: &mut usize,
    collapsed_sections: &HashSet<String>,
) {
    // As wide as useful, as tall as the screen allows (it scrolls)
    let popup_area = modal_rect(area, HELP_WIDTH, area.height);

    // Clear background to prevent bleed-through from underlying screens
    frame.render_widget(Clear, popup_area);

    // Description column width: border (2) + padding (2) + key column + spacing (2)
    let desc_width = usize::from(
        popup_area
            .width
            .saturating_sub(2 + 2 + KEY_COLUMN_WIDTH + 2),
    )
    .max(1);

    let rows = build_help_rows(help_layout(current_screen, collapsed_sections), desc_width);
    let total_rows = rows.len();

    let table = Table::new(rows, [
        Constraint::Length(KEY_COLUMN_WIDTH),
        Constraint::Fill(1), // Description column (take remaining space and wrap)
    ])
    .block(
        dialog_block("Help").title_bottom(border_hints(&[("↑/↓", "sections"), ("Esc", "close")])),
    )
    .column_spacing(2)
    .row_highlight_style(theme::selected());

    // Ensure a row is selected for cursor visibility
    if scroll_state.selected().is_none() {
        scroll_state.select(Some(0));
    }

    frame.render_stateful_widget(table, popup_area, scroll_state);

    // Cache the viewport height for scroll handling
    *viewport_height = usize::from(popup_area.height.saturating_sub(2));
    render_scrollbar(frame, popup_area, 0, total_rows, scroll_state.offset());
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    fn sections(screen: Screen, collapsed: &HashSet<String>) -> Vec<(usize, String)> {
        (0..get_help_row_count(screen, collapsed))
            .filter_map(|row| get_section_at_row(screen, collapsed, row).map(|s| (row, s)))
            .collect()
    }

    #[test]
    fn test_sections_follow_the_keymap() {
        let none = HashSet::new();
        for &screen in Screen::all() {
            let found = sections(screen, &none);
            let names: Vec<&str> = found.iter().map(|(_, n)| n.as_str()).collect();
            assert_eq!(names, vec![
                screen_section(screen).as_str(),
                TEXT_INPUT_SECTION,
                GLOBAL_SECTION
            ]);

            // The global section holds exactly the global and help bindings
            let global_row = found[2].0;
            let global_keys = BINDINGS
                .iter()
                .filter(|b| matches!(b.context, Context::Global | Context::Help))
                .count();
            assert_eq!(
                get_help_row_count(screen, &none) - global_row - 1,
                global_keys,
                "{screen:?}"
            );

            // Collapsing a section removes exactly its rows and keeps the others found
            let mut collapsed = HashSet::new();
            collapsed.insert(screen_section(screen));
            let folded = sections(screen, &collapsed);
            assert_eq!(folded[0], (0, screen_section(screen)));
            assert_eq!(folded[1].0, 1);
            assert_eq!(folded[2].1, GLOBAL_SECTION);
        }
    }

    #[test]
    fn test_section_navigation() {
        let none = HashSet::new();
        let found = sections(Screen::Sinks, &none);
        assert_eq!(
            find_next_section_header(Screen::Sinks, &none, 0),
            Some(found[1].0)
        );
        assert_eq!(
            find_prev_section_header(Screen::Sinks, &none, found[2].0),
            Some(found[1].0)
        );
        assert_eq!(find_prev_section_header(Screen::Sinks, &none, 0), None);
        assert_eq!(
            find_next_section_header(Screen::Sinks, &none, found[2].0),
            None
        );
    }

    #[test]
    fn test_wrap_text() {
        assert_eq!(wrap_text("aa bb cc", 5), ("aa bb\ncc".to_string(), 2));
        assert_eq!(wrap_text("ääää ö", 4), ("ääää\nö".to_string(), 2));
        assert_eq!(wrap_text("", 4), (String::new(), 1));
    }
}
