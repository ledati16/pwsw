//! Help overlay - Context-aware keyboard shortcut reference

use ratatui::{
    layout::{Constraint, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, Borders, Cell, Clear, Paragraph, Row, Table, TableState,
    },
    Frame,
};

use crate::tui::app::Screen;
use crate::tui::widgets::{centered_modal, modal_size};

/// Get the number of rows in the help content for a given screen
pub fn get_help_row_count(current_screen: Screen) -> usize {
    build_help_rows(current_screen).len()
}

/// Render help overlay on top of the current screen
pub fn render_help(
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
            .title("Help (↑/↓ to scroll)")
            .style(Style::default().bg(Color::Black).fg(Color::White)),
    );

    // No selection enforced - we control offset manually for view scrolling

    frame.render_stateful_widget(table, popup_area, scroll_state);

    // Compute visible viewport height and prepare for indicators
    let inner = popup_area.inner(ratatui::layout::Margin { vertical: 1, horizontal: 0 });
    let view_height = inner.height as usize;

    // Compute visual line counts for help rows so we can detect "has above/below" even with wrapping.
    let content_width = inner.width as usize;
    let left_col = 15usize;
    let right_col = if content_width > left_col { content_width - left_col } else { 1 };

    let items = build_help_items(current_screen);
    let mut per_row_lines: Vec<usize> = Vec::with_capacity(items.len());
    for (key, desc) in items.iter() {
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
    let has_above = raw_row_offset > 0;
    let has_below = (raw_row_offset as usize) + view_height < total_visual_lines;

    // Draw top arrow if there's more above
    if has_above {
        let r = Rect {
            x: inner.x + inner.width.saturating_sub(2),
            y: inner.y,
            width: 1,
            height: 1,
        };
        let p = Paragraph::new(Span::styled("▲", Style::default().fg(Color::Yellow)));
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
        let p = Paragraph::new(Span::styled("▼", Style::default().fg(Color::Yellow)));
        frame.render_widget(p, r);
    }
}

/// Build a simple vector of (key, desc) strings for measuring visual lines.
fn build_help_items(current_screen: Screen) -> Vec<(String, String)> {
    let mut items = Vec::new();

    // Helper to add a section header
    let add_header = |items: &mut Vec<(String, String)>, text: &str| {
        items.push(("".to_string(), text.to_string()));
        items.push(("".to_string(), "".to_string())); // Spacer
    };

    // Helper to add a keybind row
    let add_keybind = |items: &mut Vec<(String, String)>, key: &str, desc: &str| {
        items.push((key.to_string(), desc.to_string()));
    };

    // Helper to add a sub-header
    let add_subheader = |items: &mut Vec<(String, String)>, text: &str| {
        items.push(("".to_string(), "".to_string())); // Spacer
        items.push(("".to_string(), text.to_string()));
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
            add_keybind(&mut items, "a", "Add new rule");
            add_keybind(&mut items, "e", "Edit selected rule");
            add_keybind(&mut items, "x", "Delete selected rule");
            add_subheader(&mut items, "In Editor (Add/Edit)");
            add_keybind(&mut items, "Tab", "Next field");
            add_keybind(&mut items, "Shift+Tab", "Previous field");
            add_keybind(&mut items, "Space", "Cycle notify option");
            add_keybind(&mut items, "Enter", "Save / Open sink selector");
            add_keybind(&mut items, "Esc", "Cancel");
            items.push(("Live Preview".to_string(), "Shows matching windows as you type".to_string()));
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
    items.push(("".to_string(), "".to_string()));
    add_header(&mut items, "Global Shortcuts");
    add_keybind(&mut items, "q/Ctrl+C", "Quit application");
    add_keybind(&mut items, "Tab", "Next screen");
    add_keybind(&mut items, "Shift+Tab", "Previous screen");
    add_keybind(&mut items, "d", "Go to Dashboard");
    add_keybind(&mut items, "s", "Go to Sinks");
    add_keybind(&mut items, "r", "Go to Rules");
    add_keybind(&mut items, "t", "Go to Settings");
    add_keybind(&mut items, "Ctrl+S", "Save configuration");
    add_keybind(&mut items, "Esc", "Clear status message");
    add_keybind(&mut items, "?", "Toggle help");

    // Close instruction
    items.push(("".to_string(), "".to_string()));
    items.push(("".to_string(), "Press ? or Esc to close help".to_string()));

    items
}

/// Build the list of rows for the help table (unchanged rendering semantics)
fn build_help_rows(current_screen: Screen) -> Vec<Row<'static>> {
    let mut rows = Vec::new();

    // Helper to add a section header
    let add_header = |rows: &mut Vec<Row>, text: &str| {
        rows.push(Row::new(vec![Cell::from(Span::styled(
            text.to_string(),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ))]));
        rows.push(Row::new(vec![Cell::from("")])); // Spacer
    };

    // Helper to add a keybind row
    let add_keybind = |rows: &mut Vec<Row>, key: &str, desc: &str| {
        rows.push(Row::new(vec![
            Cell::from(Span::styled(
                key.to_string(),
                Style::default().fg(Color::Green),
            )),
            Cell::from(Span::raw(desc.to_string())),
        ]));
    };

    // Helper to add a sub-header
    let add_subheader = |rows: &mut Vec<Row>, text: &str| {
        rows.push(Row::new(vec![Cell::from("")])); // Spacer
        rows.push(Row::new(vec![Cell::from(Span::styled(
            text.to_string(),
            Style::default().fg(Color::Yellow),
        ))]));
    };

    // Populate rows based on screen
    match current_screen {
        Screen::Dashboard => {
            add_header(&mut rows, "Dashboard Screen");
            add_keybind(&mut rows, "↑/↓", "Navigate daemon control actions");
            add_keybind(&mut rows, "Enter", "Execute selected action");
        }
        Screen::Sinks => {
            add_header(&mut rows, "Sinks Screen");
            add_keybind(&mut rows, "↑/↓", "Navigate sinks");
            add_keybind(&mut rows, "a", "Add new sink");
            add_keybind(&mut rows, "e", "Edit selected sink");
            add_keybind(&mut rows, "x", "Delete selected sink");
            add_keybind(&mut rows, "Space", "Toggle default status");
            add_subheader(&mut rows, "In Editor (Add/Edit)");
            add_keybind(&mut rows, "Tab", "Next field");
            add_keybind(&mut rows, "Shift+Tab", "Previous field");
            add_keybind(&mut rows, "Space", "Toggle checkbox");
            add_keybind(&mut rows, "Enter", "Save");
            add_keybind(&mut rows, "Esc", "Cancel");
        }
        Screen::Rules => {
            add_header(&mut rows, "Rules Screen");
            add_keybind(&mut rows, "↑/↓", "Navigate rules");
            add_keybind(&mut rows, "a", "Add new rule");
            add_keybind(&mut rows, "e", "Edit selected rule");
            add_keybind(&mut rows, "x", "Delete selected rule");
            add_subheader(&mut rows, "In Editor (Add/Edit)");
            add_keybind(&mut rows, "Tab", "Next field");
            add_keybind(&mut rows, "Shift+Tab", "Previous field");
            add_keybind(&mut rows, "Space", "Cycle notify option");
            add_keybind(&mut rows, "Enter", "Save / Open sink selector");
            add_keybind(&mut rows, "Esc", "Cancel");
            rows.push(Row::new(vec![
                Cell::from(Span::styled(
                    "Live Preview",
                    Style::default().fg(Color::Green),
                )),
                Cell::from("Shows matching windows as you type"),
            ]));
        }
        Screen::Settings => {
            add_header(&mut rows, "Settings Screen");
            add_keybind(&mut rows, "↑/↓", "Navigate settings");
            add_keybind(&mut rows, "Enter/Space", "Toggle setting / Dropdown");
            add_subheader(&mut rows, "In Log Level Dropdown");
            add_keybind(&mut rows, "↑/↓", "Navigate log levels");
            add_keybind(&mut rows, "Enter", "Confirm selection");
            add_keybind(&mut rows, "Esc", "Cancel");
        }
    }

    // Global shortcuts
    rows.push(Row::new(vec![Cell::from("")]));
    add_header(&mut rows, "Global Shortcuts");
    add_keybind(&mut rows, "q/Ctrl+C", "Quit application");
    add_keybind(&mut rows, "Tab", "Next screen");
    add_keybind(&mut rows, "Shift+Tab", "Previous screen");
    add_keybind(&mut rows, "d", "Go to Dashboard");
    add_keybind(&mut rows, "s", "Go to Sinks");
    add_keybind(&mut rows, "r", "Go to Rules");
    add_keybind(&mut rows, "t", "Go to Settings");
    add_keybind(&mut rows, "Ctrl+S", "Save configuration");
    add_keybind(&mut rows, "Esc", "Clear status message");
    add_keybind(&mut rows, "?", "Toggle help");

    // Close instruction
    rows.push(Row::new(vec![Cell::from("")]));
    rows.push(Row::new(vec![Cell::from(Line::from(vec![
        Span::styled("Press ", Style::default().fg(Color::Gray)),
        Span::styled(
            "?",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" or ", Style::default().fg(Color::Gray)),
        Span::styled(
            "Esc",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" to close help", Style::default().fg(Color::Gray)),
    ]))]));

    rows
}
