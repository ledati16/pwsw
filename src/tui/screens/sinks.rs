//! Sinks screen - Manage audio output sinks

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph},
    Frame,
};

use crate::config::SinkConfig;

use crate::tui::editor_state::SimpleEditor;
use crate::tui::textfield::render_text_field;
use crate::tui::widgets::centered_rect;

/// Sinks screen mode
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SinksMode {
    List,
    AddEdit,
    Delete,
}

/// Editor state for add/edit modal
pub struct SinkEditor {
    pub name: SimpleEditor,
    pub desc: SimpleEditor,
    pub icon: SimpleEditor,
    pub default: bool,
    pub focused_field: usize, // 0=name, 1=desc, 2=icon, 3=default
}

impl SinkEditor {
    pub fn new() -> Self {
        Self {
            name: SimpleEditor::new(),
            desc: SimpleEditor::new(),
            icon: SimpleEditor::new(),
            default: false,
            focused_field: 0,
        }
    }

    pub fn from_sink(sink: &SinkConfig) -> Self {
        Self {
            name: SimpleEditor::from_string(sink.name.clone()),
            desc: SimpleEditor::from_string(sink.desc.clone()),
            icon: SimpleEditor::from_string(sink.icon.clone().unwrap_or_default()),
            default: sink.default,
            focused_field: 0,
        }
    }

    pub fn next_field(&mut self) {
        if self.focused_field < 3 {
            self.focused_field += 1;
        }
    }

    pub fn prev_field(&mut self) {
        if self.focused_field > 0 {
            self.focused_field -= 1;
        }
    }
}

/// Sinks screen state
pub struct SinksScreen {
    pub mode: SinksMode,
    pub selected: usize,
    pub editor: SinkEditor,
    pub editing_index: Option<usize>, // None = adding, Some(i) = editing
    /// Cached padded descriptions for aligned display (updated when sinks change)
    pub display_descs: Vec<String>,
}

impl SinksScreen {
    /// Update cached padded descriptions for the list. Call when `sinks` changed.
    pub fn update_display_descs(&mut self, sinks: &[SinkConfig]) {
        // Compute max desc length and produce left-aligned padded strings
        let max_len = sinks.iter().map(|s| s.desc.len()).max().unwrap_or(0);
        self.display_descs = sinks
            .iter()
            .map(|s| {
                if s.desc.len() >= max_len {
                    s.desc.clone()
                } else {
                    let mut st = s.desc.clone();
                    st.push_str(&" ".repeat(max_len - s.desc.len()));
                    st
                }
            })
            .collect();
    }
    pub fn new() -> Self {
        Self {
            mode: SinksMode::List,
            selected: 0,
            editor: SinkEditor::new(),
            editing_index: None,
            display_descs: Vec::new(),
        }
    }

    pub fn select_previous(&mut self, sink_count: usize) {
        if sink_count > 0 && self.selected > 0 {
            self.selected -= 1;
        }
    }

    pub fn select_next(&mut self, sink_count: usize) {
        if sink_count > 0 && self.selected < sink_count - 1 {
            self.selected += 1;
        }
    }

    pub fn start_add(&mut self) {
        self.mode = SinksMode::AddEdit;
        self.editor = SinkEditor::new();
        self.editing_index = None;
    }

    pub fn start_edit(&mut self, sinks: &[SinkConfig]) {
        if self.selected < sinks.len() {
            self.mode = SinksMode::AddEdit;
            self.editor = SinkEditor::from_sink(&sinks[self.selected]);
            self.editing_index = Some(self.selected);
        }
    }

    pub fn start_delete(&mut self) {
        self.mode = SinksMode::Delete;
    }

    pub fn cancel(&mut self) {
        self.mode = SinksMode::List;
    }
}

/// Render the sinks screen
pub fn render_sinks(
    frame: &mut Frame,
    area: Rect,
    sinks: &[SinkConfig],
    screen_state: &SinksScreen,
    active_sinks: &[String],
) {
    match screen_state.mode {
        SinksMode::List => render_list(frame, area, sinks, screen_state, active_sinks),
        SinksMode::AddEdit => render_editor(frame, area, screen_state),
        SinksMode::Delete => render_delete_confirmation(frame, area, sinks, screen_state),
    }
}

/// Render the sinks list
fn render_list(
    frame: &mut Frame,
    area: Rect,
    sinks: &[SinkConfig],
    screen_state: &SinksScreen,
    active_sinks: &[String],
) {
    let items: Vec<ListItem> = sinks
        .iter()
        .enumerate()
        .map(|(i, sink)| {
            let is_selected = i == screen_state.selected;
            let is_active = active_sinks.contains(&sink.name);

            let status = if is_active {
                Span::styled("● active", Style::default().fg(Color::Green))
            } else {
                Span::styled("○ inactive", Style::default().fg(Color::Gray))
            };

            let default_marker = if sink.default {
                Span::styled(" [DEFAULT]", Style::default().fg(Color::Yellow))
            } else {
                Span::raw("")
            };

            let style = if is_selected {
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            };

            let line = Line::from(vec![
                Span::styled(
                    if is_selected { "> " } else { "  " },
                    Style::default().fg(Color::Cyan),
                ),
                Span::styled(screen_state.display_descs.get(i).map(|s| s.as_str()).unwrap_or(sink.desc.as_str()), style),
                status,
                default_marker,
            ]);

            ListItem::new(vec![
                line,
                Line::from(vec![
                    Span::raw("    "),
                    Span::styled(&sink.name, Style::default().fg(Color::Gray)),
                ]),
            ])
        })
        .collect();

    let list = List::new(items).block(
        Block::default()
            .borders(Borders::ALL)
            .title("Sinks ([a]dd, [e]dit, [x] delete, [Space] toggle default, Ctrl+S save)"),
    );

    frame.render_widget(list, area);
}

/// Render the add/edit modal
fn render_editor(frame: &mut Frame, area: Rect, screen_state: &SinksScreen) {
    let title = if screen_state.editing_index.is_some() {
        "Edit Sink"
    } else {
        "Add Sink"
    };

    // Create modal in center
    let popup_area = centered_rect(70, 60, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(2)
        .constraints([
            Constraint::Length(3), // Name field
            Constraint::Length(3), // Desc field
            Constraint::Length(3), // Icon field
            Constraint::Length(3), // Default checkbox
            Constraint::Min(0),    // Help text
        ])
        .split(popup_area);

    // Background block
    let block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .style(Style::default().bg(Color::Black));
    frame.render_widget(block, popup_area);

    // Name field
    render_text_field(
        frame,
        chunks[0],
        "Node Name:",
        &screen_state.editor.name.value,
        screen_state.editor.focused_field == 0,
        Some(screen_state.editor.name.cursor),
    );

    // Desc field
    render_text_field(
        frame,
        chunks[1],
        "Description:",
        &screen_state.editor.desc.value,
        screen_state.editor.focused_field == 1,
        Some(screen_state.editor.desc.cursor),
    );

    // Icon field
    render_text_field(
        frame,
        chunks[2],
        "Icon (optional):",
        &screen_state.editor.icon.value,
        screen_state.editor.focused_field == 2,
        Some(screen_state.editor.icon.cursor),
    );

    // Default checkbox
    let checkbox_text = if screen_state.editor.default {
        "✓ Default Sink"
    } else {
        "✗ Default Sink"
    };
    let checkbox_style = if screen_state.editor.focused_field == 3 {
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::White)
    };
    let checkbox = Paragraph::new(checkbox_text).style(checkbox_style);
    frame.render_widget(checkbox, chunks[3]);

    // Help text
    let help = vec![
        Line::from(""),
        Line::from(vec![
            Span::raw("Tab/Shift+Tab: Next/Prev field  |  "),
            Span::raw("Enter: Save  |  "),
            Span::raw("Esc: Cancel"),
        ]),
    ];
    let help_widget = Paragraph::new(help).style(Style::default().fg(Color::Gray));
    frame.render_widget(help_widget, chunks[4]);
}

/// Render delete confirmation modal
fn render_delete_confirmation(
    frame: &mut Frame,
    area: Rect,
    sinks: &[SinkConfig],
    screen_state: &SinksScreen,
) {
    if screen_state.selected >= sinks.len() {
        return;
    }

    let sink = &sinks[screen_state.selected];
    let popup_area = centered_rect(50, 30, area);

    let text = vec![
        Line::from(""),
        Line::from(vec![Span::styled(
            "Are you sure you want to delete this sink?",
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        )]),
        Line::from(""),
        Line::from(vec![
            Span::raw("Description: "),
            Span::styled(&sink.desc, Style::default().fg(Color::White)),
        ]),
        Line::from(vec![
            Span::raw("Node Name: "),
            Span::styled(&sink.name, Style::default().fg(Color::Gray)),
        ]),
        Line::from(""),
        Line::from(vec![Span::styled(
            "Press Enter to confirm, Esc to cancel",
            Style::default().fg(Color::Yellow),
        )]),
    ];

    let block = Block::default()
        .borders(Borders::ALL)
        .title("Delete Sink")
        .style(Style::default().bg(Color::Black));

    let paragraph = Paragraph::new(text).block(block);
    frame.render_widget(paragraph, popup_area);
}
