//! Help overlay - Context-aware keyboard shortcut reference

use ratatui::{
    layout::{Constraint, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, Clear, Paragraph, Row, Table, TableState},
    Frame,
};

use crate::style::colors;
use crate::tui::app::Screen;
use crate::tui::widgets::{centered_modal, modal_size};

/// Get the number of rows in the help content for a given screen
pub(crate) fn get_help_row_count(current_screen: Screen) -> usize {
    build_help_rows(current_screen).len()
}

/// Render help overlay on top of the current screen
pub(crate) fn render_help(
    frame: &mut Frame,
    area: Rect,
    current_screen: Screen,
    scroll_state: &mut TableState,
) {
    // Create centered modal
    let popup_area = centered_modal(modal_size::HELP, area);

    // Clear background to prevent bleed-through from underlying screens
    frame.render_widget(Clear, popup_area);

    // Build help content
    let rows = build_help_rows(current_screen);
    let _total_rows = rows.len();

    let table = Table::new(
        rows,
        [
            Constraint::Length(15), // Key column
            Constraint::Min(10),    // Description column
        ],
    )
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Help ")
            .style(Style::default().bg(colors::UI_MODAL_BG).fg(colors::UI_TEXT)),
    );

    // No selection enforced - we control offset manually for view scrolling

    frame.render_stateful_widget(table, popup_area, scroll_state);

    // Compute visible viewport height and prepare for indicators
    let inner = popup_area.inner(ratatui::layout::Margin {
        vertical: 1,
        horizontal: 0,
    });
    let view_height = inner.height as usize;

    // Compute visual line counts for help rows so we can detect "has above/below" even with wrapping.
    let content_width = inner.width as usize;
    let left_col = 15usize;
    let right_col = if content_width > left_col {
        content_width - left_col
    } else {
        1
    };

    let items = build_help_items(current_screen);
    let mut per_row_lines: Vec<usize> = Vec::with_capacity(items.len());
    for (key, desc) in &items {
        let lines = if key.is_empty() && desc.is_empty() {
            1usize
        } else if key.is_empty() {
            // Header or single-span row: wrap across full width
            let w = content_width.max(1);
            (desc.len().saturating_add(w - 1)) / w
        } else {
            let lw = left_col.max(1);
            let rw = right_col.max(1);
            let l_lines = (key.len().saturating_add(lw - 1)) / lw;
            let r_lines = (desc.len().saturating_add(rw - 1)) / rw;
            l_lines.max(r_lines).max(1)
        };
        per_row_lines.push(lines);
    }

    let total_visual_lines: usize = per_row_lines.iter().sum();

    // Replace scrollbar with simple up/down arrow indicators
    let raw_row_offset = scroll_state.offset();

    // Map TableState logical row offset -> visual line position by summing heights of preceding rows
    let mut visual_pos = 0usize;
    for line in per_row_lines
        .iter()
        .take(raw_row_offset.min(per_row_lines.len()))
    {
        visual_pos += *line;
    }

    let has_above = visual_pos > 0;
    let has_below = (visual_pos + view_height) < total_visual_lines;

    // Draw top arrow if there's more above
    if has_above {
        let r = Rect {
            x: inner.x + inner.width.saturating_sub(2),
            y: inner.y,
            width: 1,
            height: 1,
        };
        let p = Paragraph::new(Span::styled("↑", Style::default().fg(colors::UI_WARNING)));
        frame.render_widget(p, r);
    }

    // Draw bottom arrow if there's more below
    if has_below {
        let r = Rect {
            x: inner.x + inner.width.saturating_sub(2),
            y: inner.y + inner.height.saturating_sub(1),
            width: 1,
            height: 1,
        };
        let p = Paragraph::new(Span::styled("↓", Style::default().fg(colors::UI_WARNING)));
        frame.render_widget(p, r);
    }
}

/// Build a simple vector of (key, desc) strings for measuring visual lines.
fn build_help_items(current_screen: Screen) -> Vec<(String, String)> {
    let mut items = Vec::new();

    // Helper to add a section header
    let add_header = |items: &mut Vec<(String, String)>, text: &str| {
        items.push((String::new(), text.to_string()));
        items.push((String::new(), String::new())); // Spacer
    };

    // Helper to add a keybind row
    let add_keybind = |items: &mut Vec<(String, String)>, key: &str, desc: &str| {
        items.push((key.to_string(), desc.to_string()));
    };

    // Helper to add a sub-header
    let add_subheader = |items: &mut Vec<(String, String)>, text: &str| {
        items.push((String::new(), String::new())); // Spacer
        items.push((String::new(), text.to_string()));
    };

    match current_screen {
        Screen::Dashboard => {
            add_header(&mut items, "Dashboard Screen");
            add_keybind(&mut items, "↑/↓", "Navigate daemon control actions");
            add_keybind(&mut items, "Enter", "Execute selected action");
        }
        Screen::Sinks => {
            add_header(&mut items, "Sinks Screen");
            add_keybind(&mut items, "↑/↓", "Navigate sinks");
            add_keybind(&mut items, "Shift+↑/↓", "Reorder sinks");
            add_keybind(&mut items, "a", "Add new sink");
            add_keybind(&mut items, "e", "Edit selected sink");
            add_keybind(&mut items, "x", "Delete selected sink");
            add_keybind(&mut items, "Space", "Toggle default status");
            add_subheader(&mut items, "In Editor (Add/Edit)");
            add_keybind(&mut items, "Tab", "Next field");
            add_keybind(&mut items, "Shift+Tab", "Previous field");
            add_keybind(&mut items, "Space", "Toggle checkbox");
            add_keybind(&mut items, "Enter", "Save");
            add_keybind(&mut items, "Esc", "Cancel");
        }
        Screen::Rules => {
            add_header(&mut items, "Rules Screen");
            add_keybind(&mut items, "↑/↓", "Navigate rules");
            add_keybind(&mut items, "Shift+↑/↓", "Reorder rules");
            add_keybind(&mut items, "a", "Add new rule");
            add_keybind(&mut items, "e", "Edit selected rule");
            add_keybind(&mut items, "x", "Delete selected rule");
            add_subheader(&mut items, "In Editor (Add/Edit)");
            add_keybind(&mut items, "Tab", "Next field");
            add_keybind(&mut items, "Shift+Tab", "Previous field");
            add_keybind(&mut items, "Space", "Cycle notify option");
            add_keybind(&mut items, "Enter", "Save / Open sink selector");
            add_keybind(&mut items, "Esc", "Cancel");
            items.push((
                "Live Preview".to_string(),
                "Shows matching windows as you type".to_string(),
            ));
        }
        Screen::Settings => {
            add_header(&mut items, "Settings Screen");
            add_keybind(&mut items, "↑/↓", "Navigate settings");
            add_keybind(&mut items, "Enter/Space", "Toggle setting / Dropdown");
            add_subheader(&mut items, "In Log Level Dropdown");
            add_keybind(&mut items, "↑/↓", "Navigate log levels");
            add_keybind(&mut items, "Enter", "Confirm selection");
            add_keybind(&mut items, "Esc", "Cancel");
        }
    }

    // Global shortcuts
    items.push((String::new(), String::new()));
    add_header(&mut items, "Global Shortcuts");
    add_keybind(&mut items, "q/Ctrl+C", "Quit application");
    add_keybind(&mut items, "Tab", "Next screen");
    add_keybind(&mut items, "Shift+Tab", "Previous screen");
    add_keybind(&mut items, "1", "Go to Dashboard");
    add_keybind(&mut items, "2", "Go to Sinks");
    add_keybind(&mut items, "3", "Go to Rules");
    add_keybind(&mut items, "4", "Go to Settings");
    add_keybind(&mut items, "Ctrl+S", "Save configuration");
    add_keybind(&mut items, "Esc", "Clear status message");
    add_keybind(&mut items, "?", "Toggle help");

    // Close instruction
    items.push((String::new(), String::new()));
    items.push((String::new(), "Press ? or Esc to close help".to_string()));

    items
}

/// Build the list of rows for the help table (unchanged rendering semantics)
// Help table builder - comprehensive keybinding list for all screens
#[allow(clippy::too_many_lines)]
fn build_help_rows(current_screen: Screen) -> Vec<Row<'static>> {
    let mut rows = Vec::new();

    // Helper to add a main screen header (Level 1: yellow + bold)
    let add_header = |rows: &mut Vec<Row>, text: &str| {
        rows.push(Row::new(vec![Cell::from(Span::styled(
            text.to_string(),
            Style::default()
                .fg(colors::UI_WARNING)
                .add_modifier(Modifier::BOLD),
        ))]));
        rows.push(Row::new(vec![Cell::from("")])); // Spacer
    };

    // Helper to add a section header (Level 2: cyan + bold)
    let add_section = |rows: &mut Vec<Row>, text: &str| {
        rows.push(Row::new(vec![Cell::from("")])); // Spacer before section
        rows.push(Row::new(vec![Cell::from(Span::styled(
            text.to_string(),
            Style::default()
                .fg(colors::UI_HIGHLIGHT)
                .add_modifier(Modifier::BOLD),
        ))]));
    };

    // Helper to add a subsection header (Level 3: gray)
    let add_subsection = |rows: &mut Vec<Row>, text: &str| {
        rows.push(Row::new(vec![Cell::from(Span::styled(
            text.to_string(),
            Style::default().fg(colors::UI_SECONDARY),
        ))]));
    };

    // Helper to add a keybind row
    let add_keybind = |rows: &mut Vec<Row>, key: &str, desc: &str| {
        rows.push(Row::new(vec![
            Cell::from(Span::styled(
                key.to_string(),
                Style::default().fg(colors::UI_SUCCESS),
            )),
            Cell::from(Span::raw(desc.to_string())),
        ]));
    };

    // Helper to add compact inline keybinds (saves vertical space)
    let add_compact = |rows: &mut Vec<Row>, text: &str| {
        rows.push(Row::new(vec![Cell::from(Span::styled(
            text.to_string(),
            Style::default().fg(colors::UI_SECONDARY),
        ))]));
    };

    // Helper to add an empty row (visual separation)
    let add_empty = |rows: &mut Vec<Row>| {
        rows.push(Row::new(vec![Cell::from("")]));
    };

    // Add help navigation hint at the top
    add_compact(
        &mut rows,
        "↑↓ Scroll • PgUp/PgDn Page • Home/End Jump • Esc/q/? Close",
    );
    add_empty(&mut rows);

    // Populate rows based on screen
    match current_screen {
        Screen::Dashboard => {
            add_header(&mut rows, "Dashboard Screen");

            add_section(&mut rows, "View Toggle");
            add_keybind(&mut rows, "w", "Toggle between Logs ↔ Windows view");

            add_section(&mut rows, "Daemon Control");
            add_keybind(&mut rows, "←/→", "Navigate daemon actions");
            add_keybind(
                &mut rows,
                "Enter",
                "Execute selected action (start/stop/restart/enable/disable)",
            );

            add_section(&mut rows, "Scrolling");
            add_subsection(&mut rows, "Logs View:");
            add_keybind(&mut rows, "↑/↓", "Scroll logs line by line");
            add_keybind(&mut rows, "PageUp/PageDown", "Page scroll");
            add_keybind(&mut rows, "Home", "Jump to latest (bottom)");
            add_subsection(&mut rows, "Windows View:");
            add_keybind(&mut rows, "PageUp/PageDown", "Scroll window list");
            add_keybind(&mut rows, "Home", "Jump to top");
        }
        Screen::Sinks => {
            add_header(&mut rows, "Sinks Screen");

            add_section(&mut rows, "Navigation & List Management");
            add_compact(
                &mut rows,
                "↑↓ Navigate • Shift+↑↓ Reorder • a Add • e Edit • x Delete • Space Set Default",
            );

            add_section(&mut rows, "Editor - Field Navigation");
            add_keybind(&mut rows, "Tab/Shift+Tab, ↑/↓", "Switch field");
            add_keybind(&mut rows, "Enter", "Save / Open node selector (Name field)");
            add_keybind(&mut rows, "Esc", "Cancel");

            add_section(&mut rows, "Editor - Editing");
            add_keybind(
                &mut rows,
                "Space",
                "Toggle default checkbox (Default Sink field)",
            );
        }
        Screen::Rules => {
            add_header(&mut rows, "Rules Screen");

            add_section(&mut rows, "Navigation & List Management");
            add_compact(
                &mut rows,
                "↑↓ Navigate • Shift+↑↓ Reorder • a Add • e Edit • x Delete",
            );

            add_section(&mut rows, "Editor - Field Navigation");
            add_keybind(&mut rows, "Tab/Shift+Tab, ↑/↓", "Switch field");
            add_keybind(
                &mut rows,
                "Enter",
                "Save / Open sink selector (Target Sink field)",
            );
            add_keybind(&mut rows, "Esc", "Cancel");

            add_section(&mut rows, "Editor - Notify Setting");
            add_keybind(
                &mut rows,
                "Space",
                "Cycle: Default → Enabled → Disabled (Notify field)",
            );

            add_section(&mut rows, "Regex Pattern Syntax");
            add_subsection(&mut rows, "Common Patterns:");
            add_keybind(&mut rows, "firefox", "Matches 'firefox' anywhere in text");
            add_keybind(&mut rows, "^steam$", "Exact match (^ = start, $ = end)");
            add_keybind(&mut rows, "^(mpv|vlc)$", "Match mpv OR vlc");
            add_keybind(&mut rows, "(?i)discord", "Case-insensitive match");
            add_keybind(&mut rows, "[Ff]irefox", "Match Firefox or firefox");
            add_keybind(&mut rows, "\\d+", "One or more digits");

            add_subsection(&mut rows, "Special Characters:");
            add_compact(
                &mut rows,
                "^  Start • $  End • .  Any char • *  0+ • +  1+ • ?  0-1",
            );
            add_compact(
                &mut rows,
                "\\d Digit • \\w Word • \\s Space • \\b Word boundary",
            );

            add_subsection(&mut rows, "💡 Tip:");
            add_compact(
                &mut rows,
                "Use the live preview panel below editor to test your patterns!",
            );
        }
        Screen::Settings => {
            add_header(&mut rows, "Settings Screen");

            add_section(&mut rows, "Navigation");
            add_keybind(&mut rows, "↑/↓", "Navigate settings");
            add_keybind(&mut rows, "Enter/Space", "Toggle setting / Open dropdown");

            add_section(&mut rows, "Log Level Dropdown");
            add_keybind(&mut rows, "↑/↓", "Navigate log levels");
            add_keybind(&mut rows, "Enter", "Confirm selection");
            add_keybind(&mut rows, "Esc", "Cancel");
        }
    }

    // Text Input Fields (shared across all screens with editors)
    add_empty(&mut rows);
    add_header(&mut rows, "Text Input Fields");
    add_subsection(&mut rows, "Cursor Movement:");
    add_keybind(&mut rows, "←/→", "Move cursor left/right");
    add_keybind(&mut rows, "Home/End, Ctrl+A/E", "Jump to start/end");
    add_keybind(&mut rows, "Alt+B/F, Alt+←/→", "Move by word");

    add_subsection(&mut rows, "Editing:");
    add_keybind(&mut rows, "Backspace/Del", "Delete character");
    add_keybind(&mut rows, "Ctrl+W, Alt+Backspace", "Delete previous word");
    add_keybind(&mut rows, "Alt+D", "Delete next word");
    add_keybind(&mut rows, "Ctrl+U", "Clear entire line");
    add_keybind(&mut rows, "Ctrl+K", "Delete from cursor to end");

    // Global shortcuts
    add_empty(&mut rows);
    add_header(&mut rows, "Global Shortcuts");
    add_subsection(&mut rows, "Screen Navigation:");
    add_compact(
        &mut rows,
        "Tab Next • Shift+Tab Previous • 1-4 Direct screen access",
    );

    add_subsection(&mut rows, "Actions:");
    add_keybind(&mut rows, "Ctrl+S", "Save configuration");
    add_keybind(&mut rows, "q, Ctrl+C", "Quit application");
    add_keybind(&mut rows, "Esc", "Clear status message / Cancel quit");
    add_keybind(&mut rows, "?", "Toggle this help");

    // Close instruction
    rows.push(Row::new(vec![Cell::from("")]));
    rows.push(Row::new(vec![Cell::from(Line::from(vec![
        Span::styled("Press ", Style::default().fg(colors::UI_SECONDARY)),
        Span::styled(
            "?",
            Style::default()
                .fg(colors::UI_HIGHLIGHT)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" or ", Style::default().fg(colors::UI_SECONDARY)),
        Span::styled(
            "Esc",
            Style::default()
                .fg(colors::UI_HIGHLIGHT)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" to close help", Style::default().fg(colors::UI_SECONDARY)),
    ]))]));

    rows
}
