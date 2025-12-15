//! Help overlay - Context-aware keyboard shortcut reference

use ratatui::{
    layout::{Constraint, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, Borders, Cell, Clear, Row, Scrollbar, ScrollbarOrientation, ScrollbarState, Table,
        TableState,
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
    let total_rows = rows.len();

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

    // Render scrollbar
    let scrollbar = Scrollbar::default()
        .orientation(ScrollbarOrientation::VerticalRight)
        .begin_symbol(Some("▲"))
        .end_symbol(Some("▼"));

    // Compute visible viewport height for scrollbar: inner height minus top/bottom margins (2)
    let inner = popup_area.inner(ratatui::layout::Margin { vertical: 1, horizontal: 0 });
    let view_height = inner.height as usize;

    let mut scrollbar_state = ScrollbarState::default()
        .content_length(total_rows)
        .position(scroll_state.offset())
        .viewport_length(view_height);

    frame.render_stateful_widget(
        scrollbar,
        popup_area.inner(ratatui::layout::Margin {
            vertical: 1,
            horizontal: 0,
        }),
        &mut scrollbar_state,
    );
}

/// Build the list of rows for the help table
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
