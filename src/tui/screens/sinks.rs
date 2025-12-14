//! Sinks screen - Manage audio output sinks

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph},
    Frame,
};

use crate::config::SinkConfig;

use crate::tui::editor_state::SimpleEditor;
use crate::tui::textfield::render_text_field;
use crate::tui::widgets::{centered_modal, modal_size};

/// Sinks screen mode
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SinksMode {
    List,
    AddEdit,
    Delete,
    SelectSink,
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
    /// Selected index in sink selector (0 = first header, skips headers during selection)
    pub sink_selector_index: usize,
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
            sink_selector_index: 0,
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
    active_sink_list: &[crate::pipewire::ActiveSink],
    profile_sink_list: &[crate::pipewire::ProfileSink],
) {
    match screen_state.mode {
        SinksMode::List => render_list(frame, area, sinks, screen_state, active_sinks),
        SinksMode::AddEdit => render_editor(frame, area, screen_state),
        SinksMode::Delete => render_delete_confirmation(frame, area, sinks, screen_state),
        SinksMode::SelectSink => render_sink_selector(
            frame,
            area,
            active_sink_list,
            profile_sink_list,
            screen_state.sink_selector_index,
        ),
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
                Span::styled(
                    screen_state
                        .display_descs
                        .get(i)
                        .map(|s| s.as_str())
                        .unwrap_or(sink.desc.as_str()),
                    style,
                ),
                Span::raw(" "),
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
            .title("Sinks ([a]dd [e]dit [x]delete [Space]toggle [Ctrl+S]save)"),
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
    let popup_area = centered_modal(modal_size::MEDIUM, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(2)
        .constraints([
            Constraint::Length(3), // Name field (bordered)
            Constraint::Length(3), // Desc field (bordered)
            Constraint::Length(3), // Icon field (bordered)
            Constraint::Length(3), // Default checkbox (bordered)
            Constraint::Min(0),    // Help text
        ])
        .split(popup_area);

    // Background block
    let block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .style(Style::default().bg(Color::Black));
    frame.render_widget(block, popup_area);

    // Name field - use button-like selector
    let name_display = if screen_state.editor.name.value.is_empty() {
        None
    } else {
        Some(screen_state.editor.name.value.as_str())
    };

    crate::tui::widgets::render_selector_button(
        frame,
        chunks[0],
        "Node Name",
        name_display,
        screen_state.editor.focused_field == 0,
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

    // Default checkbox with border-based focus
    let mut checkbox_spans = Vec::new();
    if screen_state.editor.default {
        checkbox_spans.push(Span::styled("✓ ", Style::default().fg(Color::Green)));
        checkbox_spans.push(Span::raw("Default Sink"));
    } else {
        checkbox_spans.push(Span::styled("✗ ", Style::default().fg(Color::Red)));
        checkbox_spans.push(Span::raw("Default Sink"));
    }

    let border_style =
        crate::tui::widgets::focus_border_style(screen_state.editor.focused_field == 3);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style);

    let checkbox = Paragraph::new(Line::from(checkbox_spans)).block(block);
    frame.render_widget(checkbox, chunks[3]);

    // Help text
    let help_line = crate::tui::widgets::modal_help_line(&[
        ("Tab", "Next"),
        ("Shift+Tab", "Prev"),
        ("Enter", "Save/Select"),
        ("Esc", "Cancel"),
    ]);

    let help_widget =
        Paragraph::new(vec![Line::from(""), help_line]).style(Style::default().fg(Color::Gray));
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
    let popup_area = centered_modal(modal_size::SMALL, area);

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

/// Render sink selector modal for adding sinks
fn render_sink_selector(
    frame: &mut Frame,
    area: Rect,
    active_sinks: &[crate::pipewire::ActiveSink],
    profile_sinks: &[crate::pipewire::ProfileSink],
    selected_index: usize,
) {
    let popup_area = centered_modal(modal_size::MEDIUM, area);
    frame.render_widget(Clear, popup_area);

    // Calculate max width for text (accounting for borders, margin, and formatting)
    let available_width = popup_area.width.saturating_sub(6); // 2 borders + 2 prefix + 2 margin
    let max_desc_width = available_width.saturating_sub(40); // Reserve space for node name in parens

    // Helper to truncate description (show start with ellipsis at end)
    let truncate_desc = |text: &str, max_width: u16| -> String {
        if text.len() > max_width as usize {
            let mut truncated = text
                .chars()
                .take(max_width.saturating_sub(3) as usize)
                .collect::<String>();
            truncated.push_str("...");
            truncated
        } else {
            text.to_string()
        }
    };

    // Helper to truncate node name (show END with ellipsis at start for distinguishability)
    let truncate_node_name = |text: &str, max_width: u16| -> String {
        if text.len() > max_width as usize {
            let skip = text.len() - (max_width.saturating_sub(3) as usize);
            let mut truncated = String::from("...");
            truncated.push_str(&text.chars().skip(skip).collect::<String>());
            truncated
        } else {
            text.to_string()
        }
    };

    // Build list items from both active and profile sinks
    let mut items: Vec<ListItem> = Vec::new();
    let mut item_index = 0;

    // Active sinks header
    items.push(ListItem::new(Line::from(Span::styled(
        "── Active Sinks ──",
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    ))));

    for sink in active_sinks {
        let is_selected = item_index == selected_index;
        let style = if is_selected {
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        };

        let desc_text = truncate_desc(&sink.description, max_desc_width);
        let name_text = truncate_node_name(&sink.name, 35);

        let line = Line::from(vec![
            Span::styled(
                if is_selected { "> " } else { "  " },
                Style::default().fg(Color::Cyan),
            ),
            Span::styled(desc_text, style),
            Span::styled(" (", Style::default().fg(Color::DarkGray)),
            Span::styled(name_text, Style::default().fg(Color::DarkGray)),
            Span::styled(")", Style::default().fg(Color::DarkGray)),
        ]);
        items.push(ListItem::new(line));
        item_index += 1;
    }

    // Profile sinks header (if any)
    if !profile_sinks.is_empty() {
        items.push(ListItem::new(Line::from("")));
        items.push(ListItem::new(Line::from(Span::styled(
            "── Profile Sinks (require switching) ──",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ))));

        for sink in profile_sinks {
            let is_selected = item_index == selected_index;
            let style = if is_selected {
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            };

            let desc_text = truncate_desc(&sink.description, max_desc_width);
            let name_text = truncate_node_name(&sink.predicted_name, 35);

            let line = Line::from(vec![
                Span::styled(
                    if is_selected { "> " } else { "  " },
                    Style::default().fg(Color::Cyan),
                ),
                Span::styled(desc_text, style),
                Span::styled(" (", Style::default().fg(Color::DarkGray)),
                Span::styled(name_text, Style::default().fg(Color::DarkGray)),
                Span::styled(")", Style::default().fg(Color::DarkGray)),
            ]);
            items.push(ListItem::new(line));
            item_index += 1;
        }
    }

    let list = List::new(items).block(
        Block::default()
            .borders(Borders::ALL)
            .title("Select Sink ([↑/↓]navigate [Enter]select [Esc]cancel)")
            .style(Style::default().bg(Color::Black)),
    );

    frame.render_widget(list, popup_area);
}
