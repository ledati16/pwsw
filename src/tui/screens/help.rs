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
pub(crate) fn get_help_row_count(
    current_screen: Screen,
    collapsed_sections: &std::collections::HashSet<String>,
) -> usize {
    build_help_rows(current_screen, collapsed_sections).len()
}

/// Get the maximum scroll offset for the help screen given viewport height
pub(crate) fn get_help_max_offset(
    current_screen: Screen,
    collapsed_sections: &std::collections::HashSet<String>,
    viewport_height: usize,
) -> usize {
    let total_rows = get_help_row_count(current_screen, collapsed_sections);
    total_rows.saturating_sub(viewport_height)
}

/// Render help overlay on top of the current screen
pub(crate) fn render_help(
    frame: &mut Frame,
    area: Rect,
    current_screen: Screen,
    scroll_state: &mut TableState,
    viewport_height: &mut usize,
    collapsed_sections: &std::collections::HashSet<String>,
) {
    // Create centered modal
    let popup_area = centered_modal(modal_size::HELP, area);

    // Clear background to prevent bleed-through from underlying screens
    frame.render_widget(Clear, popup_area);

    // Build help content
    let rows = build_help_rows(current_screen, collapsed_sections);
    let _total_rows = rows.len();

    let table = Table::new(
        rows,
        [
            Constraint::Length(22), // Key column (accommodate long combinations like "Ctrl+W, Alt+Backspace")
            Constraint::Min(50),    // Description column (ensure minimum width)
        ],
    )
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Help "),
    )
    .column_spacing(2) // Add spacing between columns for better readability
    .row_highlight_style(
        Style::default()
            .bg(colors::UI_SELECTED_BG)
            .fg(colors::UI_HIGHLIGHT)
            .add_modifier(Modifier::BOLD),
    );

    // Ensure a row is selected for cursor visibility
    if scroll_state.selected().is_none() {
        scroll_state.select(Some(0));
    }

    frame.render_stateful_widget(table, popup_area, scroll_state);

    // Calculate scroll indicators
    let inner = popup_area.inner(ratatui::layout::Margin {
        vertical: 1,
        horizontal: 0,
    });
    let view_height = inner.height as usize;
    *viewport_height = view_height; // Update cached viewport height for scroll handling
    let total_rows = _total_rows;
    let current_offset = scroll_state.offset();

    // Simple scroll indicator logic: check if we can scroll in each direction
    let has_above = current_offset > 0;
    let has_below = current_offset + view_height < total_rows;

    // Calculate scroll indicators
    // Top: shows how many lines are hidden above (counting up from start)
    // Bottom: shows how many lines remain below (counting down to end)
    let lines_above = current_offset;
    let lines_below = total_rows.saturating_sub(current_offset + view_height);

    // Draw top arrow if there's more above
    if has_above {
        let arrow_text = format!("↑{}", lines_above);
        let arrow_width = arrow_text.len() as u16;
        let r = Rect {
            x: inner.x + inner.width.saturating_sub(arrow_width),
            y: inner.y,
            width: arrow_width,
            height: 1,
        };
        let p = Paragraph::new(Span::styled(
            arrow_text,
            Style::default().fg(colors::UI_WARNING),
        ));
        frame.render_widget(p, r);
    }

    // Draw bottom arrow if there's more below
    if has_below {
        let arrow_text = format!("↓{}", lines_below);
        let arrow_width = arrow_text.len() as u16;
        let r = Rect {
            x: inner.x + inner.width.saturating_sub(arrow_width),
            y: inner.y + inner.height.saturating_sub(1),
            width: arrow_width,
            height: 1,
        };
        let p = Paragraph::new(Span::styled(
            arrow_text,
            Style::default().fg(colors::UI_WARNING),
        ));
        frame.render_widget(p, r);
    }
}

/// Metadata for help rows to track section boundaries
#[derive(Clone)]
struct HelpRowMeta {
    is_section_header: bool,
    section_name: Option<String>,
}

/// Find the next section header row index starting from current row
pub(crate) fn find_next_section_header(
    current_screen: Screen,
    collapsed_sections: &std::collections::HashSet<String>,
    current_row: usize,
) -> Option<usize> {
    let row_count = get_help_row_count(current_screen, collapsed_sections);

    for row in (current_row + 1)..row_count {
        if get_section_at_row(current_screen, collapsed_sections, row).is_some() {
            return Some(row);
        }
    }
    None
}

/// Find the previous section header row index starting from current row
pub(crate) fn find_prev_section_header(
    current_screen: Screen,
    collapsed_sections: &std::collections::HashSet<String>,
    current_row: usize,
) -> Option<usize> {
    if current_row == 0 {
        return None;
    }

    for row in (0..current_row).rev() {
        if get_section_at_row(current_screen, collapsed_sections, row).is_some() {
            return Some(row);
        }
    }
    None
}

/// Get the section name at a given row index if it's a section header
pub(crate) fn get_section_at_row(
    current_screen: Screen,
    collapsed_sections: &std::collections::HashSet<String>,
    row_index: usize,
) -> Option<String> {
    // We need to rebuild with metadata to check if row is a section header
    // This is inefficient but simple for now
    let mut metadata: Vec<HelpRowMeta> = Vec::new();

    // Helper to track metadata (simplified version)
    let add_meta_keybind = |meta: &mut Vec<HelpRowMeta>| {
        meta.push(HelpRowMeta {
            is_section_header: false,
            section_name: None,
        });
    };

    let add_meta_section = |meta: &mut Vec<HelpRowMeta>, name: &str| {
        meta.push(HelpRowMeta {
            is_section_header: true,
            section_name: Some(name.to_string()),
        });
    };

    // Rebuild metadata structure (must match build_help_rows exactly)
    // Note: No hint line anymore, first row is a section header

    // Screen section
    let screen_section_name = format!("{} Screen", current_screen.name());
    add_meta_section(&mut metadata, &screen_section_name);

    if !collapsed_sections.contains(&screen_section_name) {
        match current_screen {
            Screen::Dashboard => {
                for _ in 0..6 { add_meta_keybind(&mut metadata); }
            }
            Screen::Sinks => {
                for _ in 0..9 { add_meta_keybind(&mut metadata); }
            }
            Screen::Rules => {
                // 9 keybinds + 2 hints (empty + "Regex Examples:") + 4 regex examples = 15
                for _ in 0..15 { add_meta_keybind(&mut metadata); }
            }
            Screen::Settings => {
                for _ in 0..3 { add_meta_keybind(&mut metadata); }
            }
        }
    }

    // Text Input section
    let text_input_section = "Text Input Fields".to_string();
    add_meta_section(&mut metadata, &text_input_section);
    if !collapsed_sections.contains(&text_input_section) {
        for _ in 0..8 { add_meta_keybind(&mut metadata); }
    }

    // Global section
    let global_section = "Global Shortcuts".to_string();
    add_meta_section(&mut metadata, &global_section);
    if !collapsed_sections.contains(&global_section) {
        for _ in 0..7 { add_meta_keybind(&mut metadata); }
    }

    metadata.get(row_index).and_then(|m| {
        if m.is_section_header {
            m.section_name.clone()
        } else {
            None
        }
    })
}

/// Build the list of rows for the help table with collapsible sections
#[allow(clippy::too_many_lines)]
fn build_help_rows(
    current_screen: Screen,
    collapsed_sections: &std::collections::HashSet<String>,
) -> Vec<Row<'static>> {
    let mut rows = Vec::new();
    let mut metadata: Vec<HelpRowMeta> = Vec::new();

    // Helper to add a keybind row
    let add_keybind = |rows: &mut Vec<Row>, meta: &mut Vec<HelpRowMeta>, key: &str, desc: &str| {
        rows.push(Row::new(vec![
            Cell::from(Span::styled(
                key.to_string(),
                Style::default().fg(colors::UI_SUCCESS),
            )),
            Cell::from(Span::raw(desc.to_string())),
        ]));
        meta.push(HelpRowMeta {
            is_section_header: false,
            section_name: None,
        });
    };

    // Helper to add a collapsible section header
    let add_section_header =
        |rows: &mut Vec<Row>, meta: &mut Vec<HelpRowMeta>, name: &str, is_collapsed: bool| {
            let indicator = if is_collapsed { "›" } else { "▾" };
            let action = if is_collapsed { "expand" } else { "collapse" };

            // Build hint with highlighted "space" key
            let hint_line = Line::from(vec![
                Span::styled(
                    "<",
                    Style::default()
                        .fg(colors::UI_SECONDARY)
                        .add_modifier(Modifier::ITALIC),
                ),
                Span::styled(
                    "space",
                    Style::default()
                        .fg(colors::UI_SUCCESS)
                        .add_modifier(Modifier::ITALIC),
                ),
                Span::styled(
                    format!(" to {}>", action),
                    Style::default()
                        .fg(colors::UI_SECONDARY)
                        .add_modifier(Modifier::ITALIC),
                ),
            ]);

            rows.push(Row::new(vec![
                Cell::from(Span::styled(
                    format!("{} {}", indicator, name),
                    Style::default()
                        .fg(colors::UI_WARNING)
                        .add_modifier(Modifier::BOLD),
                )),
                Cell::from(hint_line),
            ]));
            meta.push(HelpRowMeta {
                is_section_header: true,
                section_name: Some(name.to_string()),
            });
        };

    // Helper to add compact hint text (for regex examples in Rules screen)
    let add_hint = |rows: &mut Vec<Row>, meta: &mut Vec<HelpRowMeta>, text: &str| {
        rows.push(Row::new(vec![
            Cell::from(""),
            Cell::from(Span::styled(
                text.to_string(),
                Style::default().fg(colors::UI_SECONDARY),
            )),
        ]));
        meta.push(HelpRowMeta {
            is_section_header: false,
            section_name: None,
        });
    };

    // Screen-specific section
    let screen_section_name = format!("{} Screen", current_screen.name());
    let screen_collapsed = collapsed_sections.contains(&screen_section_name);
    add_section_header(&mut rows, &mut metadata, &screen_section_name, screen_collapsed);

    if !screen_collapsed {
        match current_screen {
            Screen::Dashboard => {
                add_keybind(&mut rows, &mut metadata, "w", "Toggle between Logs ↔ Windows view");
                add_keybind(&mut rows, &mut metadata, "←/→", "Navigate daemon actions");
                add_keybind(
                    &mut rows,
                    &mut metadata,
                    "Enter",
                    "Execute selected action (start/stop/restart/enable/disable)",
                );
                add_keybind(&mut rows, &mut metadata, "↑/↓", "Scroll logs line by line");
                add_keybind(&mut rows, &mut metadata, "PageUp/PageDown", "Page scroll");
                add_keybind(&mut rows, &mut metadata, "Home", "Jump to latest (logs) / top (windows)");
            }
            Screen::Sinks => {
                add_keybind(&mut rows, &mut metadata, "↑/↓", "Navigate list");
                add_keybind(&mut rows, &mut metadata, "Shift+↑/↓", "Reorder items");
                add_keybind(&mut rows, &mut metadata, "a", "Add new sink");
                add_keybind(&mut rows, &mut metadata, "e", "Edit selected sink");
                add_keybind(&mut rows, &mut metadata, "x", "Delete selected sink");
                add_keybind(&mut rows, &mut metadata, "Space", "Set as default sink");
                add_keybind(&mut rows, &mut metadata, "Tab/Shift+Tab", "Switch field (in editor)");
                add_keybind(&mut rows, &mut metadata, "Enter", "Save / Open node selector");
                add_keybind(&mut rows, &mut metadata, "Esc", "Cancel editing");
            }
            Screen::Rules => {
                add_keybind(&mut rows, &mut metadata, "↑/↓", "Navigate list");
                add_keybind(&mut rows, &mut metadata, "Shift+↑/↓", "Reorder items");
                add_keybind(&mut rows, &mut metadata, "a", "Add new rule");
                add_keybind(&mut rows, &mut metadata, "e", "Edit selected rule");
                add_keybind(&mut rows, &mut metadata, "x", "Delete selected rule");
                add_keybind(&mut rows, &mut metadata, "Tab/Shift+Tab", "Switch field (in editor)");
                add_keybind(&mut rows, &mut metadata, "Enter", "Save / Open sink selector");
                add_keybind(&mut rows, &mut metadata, "Space", "Cycle notify setting");
                add_keybind(&mut rows, &mut metadata, "Esc", "Cancel editing");
                add_hint(&mut rows, &mut metadata, "");
                add_hint(&mut rows, &mut metadata, "Regex Examples:");
                add_keybind(&mut rows, &mut metadata, "firefox", "Matches anywhere in text");
                add_keybind(&mut rows, &mut metadata, "^steam$", "Exact match only");
                add_keybind(&mut rows, &mut metadata, "^(mpv|vlc)$", "Match mpv OR vlc");
                add_keybind(&mut rows, &mut metadata, "(?i)discord", "Case-insensitive");
            }
            Screen::Settings => {
                add_keybind(&mut rows, &mut metadata, "↑/↓", "Navigate settings");
                add_keybind(&mut rows, &mut metadata, "Enter/Space", "Toggle setting / Open dropdown");
                add_keybind(&mut rows, &mut metadata, "Esc", "Cancel dropdown");
            }
        }
    }

    // Text Input Fields section (collapsed by default)
    let text_input_section = "Text Input Fields".to_string();
    let text_collapsed = collapsed_sections.contains(&text_input_section);
    add_section_header(&mut rows, &mut metadata, &text_input_section, text_collapsed);

    if !text_collapsed {
        add_keybind(&mut rows, &mut metadata, "←/→", "Move cursor left/right");
        add_keybind(&mut rows, &mut metadata, "Home/End, Ctrl+A/E", "Jump to start/end");
        add_keybind(&mut rows, &mut metadata, "Alt+B/F, Alt+←/→", "Move by word");
        add_keybind(&mut rows, &mut metadata, "Backspace/Del", "Delete character");
        add_keybind(&mut rows, &mut metadata, "Ctrl+W, Alt+Backspace", "Delete previous word");
        add_keybind(&mut rows, &mut metadata, "Alt+D", "Delete next word");
        add_keybind(&mut rows, &mut metadata, "Ctrl+U", "Clear entire line");
        add_keybind(&mut rows, &mut metadata, "Ctrl+K", "Delete from cursor to end");
    }

    // Global Shortcuts section
    let global_section = "Global Shortcuts".to_string();
    let global_collapsed = collapsed_sections.contains(&global_section);
    add_section_header(&mut rows, &mut metadata, &global_section, global_collapsed);

    if !global_collapsed {
        add_keybind(&mut rows, &mut metadata, "Tab", "Next screen");
        add_keybind(&mut rows, &mut metadata, "Shift+Tab", "Previous screen");
        add_keybind(&mut rows, &mut metadata, "1-4", "Jump directly to screen");
        add_keybind(&mut rows, &mut metadata, "Ctrl+S", "Save configuration");
        add_keybind(&mut rows, &mut metadata, "q, Ctrl+C", "Quit application");
        add_keybind(&mut rows, &mut metadata, "Esc", "Clear status / Cancel quit");
        add_keybind(&mut rows, &mut metadata, "?", "Toggle this help");
    }

    rows
}
