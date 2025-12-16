//! Sinks screen - Manage audio output sinks

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, Borders, Cell, Clear, ListItem, ListState, Paragraph, Row, Table, TableState,
    },
    Frame,
};
use std::fmt::Write;

use crate::config::SinkConfig;

use crate::tui::editor_state::SimpleEditor;
use crate::tui::widgets::{centered_modal, modal_size, render_input};

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
    /// State for sink selector dropdown
    pub sink_selector_state: ListState,
    /// Table scroll state
    pub state: TableState,
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
            sink_selector_state: ListState::default(),
            state: TableState::default(),
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
    screen_state: &mut SinksScreen,
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
            screen_state,
        ),
    }
}

/// Render the sinks list
fn render_list(
    frame: &mut Frame,
    area: Rect,
    sinks: &[SinkConfig],
    screen_state: &mut SinksScreen,
    active_sinks: &[String],
) {
    let rows: Vec<Row> = sinks
        .iter()
        .enumerate()
        .map(|(i, sink)| {
            let is_selected = i == screen_state.selected;
            let is_active = active_sinks.contains(&sink.name);

            // Status Cell
            let status_cell = if is_active {
                Cell::from(Span::styled("● Active", Style::default().fg(Color::Green)))
            } else {
                Cell::from(Span::styled("○", Style::default().fg(Color::DarkGray)))
            };

            // Description Cell (with icon if present)
            let mut desc_text = sink.desc.clone();
            if let Some(icon) = &sink.icon {
                let mut tmp = String::with_capacity(icon.len() + 1 + desc_text.len());
                let _ = write!(tmp, "{icon} {desc_text}");
                desc_text = tmp;
            }
            let desc_cell = Cell::from(Span::styled(
                desc_text,
                if is_selected {
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::White)
                },
            ));

            // Name Cell (Technical ID)
            let name_cell = Cell::from(Span::styled(&sink.name, Style::default().fg(Color::Gray)));

            // Flags Cell
            let flags_cell = if sink.default {
                Cell::from(Span::styled("DEFAULT", Style::default().fg(Color::Yellow)))
            } else {
                Cell::from("")
            };

            let row_style = if is_selected {
                Style::default().bg(Color::DarkGray)
            } else {
                Style::default()
            };

            Row::new(vec![status_cell, desc_cell, name_cell, flags_cell])
                .style(row_style)
                .height(1)
        })
        .collect();

    let table = Table::new(
        rows,
        [
            Constraint::Length(10),     // Status
            Constraint::Percentage(40), // Desc
            Constraint::Percentage(40), // Name
            Constraint::Length(10),     // Flags
        ],
    )
    .header(
        Row::new(vec!["Status", "Description", "Node Name", "Flags"])
            .style(
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )
            .bottom_margin(1),
    )
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Sinks ([a]dd [e]dit [x]delete [Space]toggle [Ctrl+S]save) "),
    );

    // Sync state
    screen_state.state.select(Some(screen_state.selected));
    frame.render_stateful_widget(table, area, &mut screen_state.state);

    // Compute visible viewport (inner area) for arrow indicators
    let inner = area.inner(ratatui::layout::Margin {
        vertical: 1,
        horizontal: 0,
    });
    let view_height = inner.height as usize;

    let raw_offset = screen_state.state.offset();
    let total = sinks.len();
    let has_above = raw_offset > 0;
    let has_below = raw_offset + view_height < total;

    // Render scroll arrows using helper
    crate::tui::widgets::render_scroll_arrows(frame, inner, has_above, has_below);
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

    // Dynamic layout: hide help if height is too small
    let show_help = area.height > 20;

    let constraints = if show_help {
        vec![
            Constraint::Length(3), // Name field
            Constraint::Length(3), // Desc field
            Constraint::Length(3), // Icon field
            Constraint::Length(3), // Default checkbox
            Constraint::Min(0),    // Help text
        ]
    } else {
        vec![
            Constraint::Length(3), // Name field
            Constraint::Length(3), // Desc field
            Constraint::Length(3), // Icon field
            Constraint::Length(3), // Default checkbox
        ]
    };

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(2)
        .constraints(constraints)
        .split(popup_area);

    // Background block
    let block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .style(Style::default().bg(Color::Black));
    frame.render_widget(block, popup_area);

    // Name field - use button-like selector
    let name_display = if screen_state.editor.name.value().is_empty() {
        None
    } else {
        Some(screen_state.editor.name.value())
    };

    crate::tui::widgets::render_selector_button(
        frame,
        chunks[0],
        "Node Name",
        name_display,
        screen_state.editor.focused_field == 0,
    );

    // Desc field
    render_input(
        frame,
        chunks[1],
        "Description:",
        &screen_state.editor.desc.input,
        screen_state.editor.focused_field == 1,
    );

    // Icon field
    render_input(
        frame,
        chunks[2],
        "Icon (optional):",
        &screen_state.editor.icon.input,
        screen_state.editor.focused_field == 2,
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

    // Help text (only if space allows)
    if show_help && chunks.len() > 4 {
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
#[allow(clippy::too_many_lines)]
fn render_sink_selector(
    frame: &mut Frame,
    area: Rect,
    active_sinks: &[crate::pipewire::ActiveSink],
    profile_sinks: &[crate::pipewire::ProfileSink],
    screen_state: &mut SinksScreen,
) {
    let popup_area = centered_modal(modal_size::MEDIUM, area);
    frame.render_widget(Clear, popup_area);

    // Calculate max width for text (accounting for borders, margin, and formatting)
    let available_width = popup_area.width.saturating_sub(6); // 2 borders + 2 prefix + 2 margin
    let max_desc_width = available_width.saturating_sub(40); // Reserve space for node name in parens

    // Use shared truncation helpers from `tui::widgets` to avoid duplication
    // (truncate_desc and truncate_node_name present in src/tui/widgets.rs)

    // Build list items from both active and profile sinks
    let mut items: Vec<ListItem> = Vec::new();

    // Active sinks header
    items.push(ListItem::new(Line::from(Span::styled(
        "── Active Sinks ──",
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    ))));

    for sink in active_sinks {
        let desc_text = crate::tui::widgets::truncate_desc(&sink.description, max_desc_width);
        let name_text = crate::tui::widgets::truncate_node_name(&sink.name, 35);

        let line = Line::from(vec![
            Span::raw("  "),
            Span::styled(desc_text, Style::default().fg(Color::White)),
            Span::styled(" (", Style::default().fg(Color::DarkGray)),
            Span::styled(name_text, Style::default().fg(Color::DarkGray)),
            Span::styled(")", Style::default().fg(Color::DarkGray)),
        ]);
        items.push(ListItem::new(line));
    }

    // Profile sinks header (if any) — List items already appended above
    if !profile_sinks.is_empty() {
        items.push(ListItem::new(Line::from("")));
        items.push(ListItem::new(Line::from(Span::styled(
            "── Profile Sinks (require switching) ──",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ))));

        for sink in profile_sinks {
            let desc_text = crate::tui::widgets::truncate_desc(&sink.description, max_desc_width);
            let name_text = crate::tui::widgets::truncate_node_name(&sink.predicted_name, 35);

            let line = Line::from(vec![
                Span::raw("  "),
                Span::styled(desc_text.clone(), Style::default().fg(Color::White)),
                Span::styled(" (", Style::default().fg(Color::DarkGray)),
                Span::styled(name_text.clone(), Style::default().fg(Color::DarkGray)),
                Span::styled(")", Style::default().fg(Color::DarkGray)),
            ]);
            items.push(ListItem::new(line));
        }
    }

    // Compute visual viewport and line counts
    let inner = popup_area.inner(ratatui::layout::Margin {
        vertical: 1,
        horizontal: 0,
    });
    let view_height = inner.height as usize;

    // Build a vector of displayed strings (headers, spacers, and items) to measure wrapping
    let mut visual_items: Vec<String> = Vec::new();
    visual_items.push("── Active Sinks ──".to_string());
    for sink in active_sinks {
        let desc_text = crate::tui::widgets::truncate_desc(&sink.description, max_desc_width);
        let name_text = crate::tui::widgets::truncate_node_name(&sink.name, 35);
        visual_items.push({
            let mut tmp = String::with_capacity(2 + desc_text.len() + 3 + name_text.len());
            let _ = write!(tmp, "  {desc_text} ({name_text})");
            tmp
        });
    }

    let active_len = active_sinks.len();

    if !profile_sinks.is_empty() {
        visual_items.push(String::new()); // spacer
        visual_items.push("── Profile Sinks (require switching) ──".to_string());
        for sink in profile_sinks {
            let desc_text = crate::tui::widgets::truncate_desc(&sink.description, max_desc_width);
            let name_text = crate::tui::widgets::truncate_node_name(&sink.predicted_name, 35);
            visual_items.push({
                let mut tmp = String::with_capacity(2 + desc_text.len() + 3 + name_text.len());
                let _ = write!(tmp, "  {desc_text} ({name_text})");
                tmp
            });
        }
    }

    // Build and render the list widget (was removed accidentally during refactor)
    let list = ratatui::widgets::List::new(items.clone())
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("Select Node (↑/↓, Enter to confirm, Esc to cancel)")
                .style(Style::default().bg(Color::Black)),
        )
        .highlight_style(Style::default().bg(Color::DarkGray))
        .highlight_symbol("");

    // Map our logical selector index (skipping headers) to the list item index
    let total_selectable = active_sinks.len() + profile_sinks.len();
    if total_selectable > 0 {
        let sel = screen_state.sink_selector_index.min(total_selectable - 1);
        let list_index = if sel < active_len {
            // Active sink: after the active header
            1 + sel
        } else {
            // Profile sink: account for active items + spacer + profile header
            let profile_idx = sel - active_len;
            3 + active_len + profile_idx
        };
        screen_state.sink_selector_state.select(Some(list_index));
    } else {
        screen_state.sink_selector_state.select(None);
    }

    frame.render_stateful_widget(list, popup_area, &mut screen_state.sink_selector_state);

    // Compute visual line counts per item using the inner width
    let content_width = inner.width as usize;
    let (has_above, has_below) = crate::tui::widgets::compute_has_above_below(
        &visual_items,
        content_width,
        screen_state.sink_selector_state.offset(),
        view_height,
    );

    // Render scroll arrows
    crate::tui::widgets::render_scroll_arrows(frame, inner, has_above, has_below);

    if !profile_sinks.is_empty() {
        visual_items.push(String::new()); // spacer
        visual_items.push("── Profile Sinks (require switching) ──".to_string());
        for sink in profile_sinks {
            let desc_text = crate::tui::widgets::truncate_desc(&sink.description, max_desc_width);
            let name_text = crate::tui::widgets::truncate_node_name(&sink.predicted_name, 35);
            visual_items.push({
                let mut tmp = String::with_capacity(2 + desc_text.len() + 3 + name_text.len());
                let _ = write!(tmp, "  {desc_text} ({name_text})");
                tmp
            });
        }
    }

    // Compute content width and current logical offset
    let content_width = inner.width as usize;
    let raw_offset = screen_state.sink_selector_state.offset();

    // Use helper to compute whether content exists above/below (accounts for wrapping)
    let (has_above, has_below) = crate::tui::widgets::compute_has_above_below(
        &visual_items,
        content_width,
        raw_offset,
        view_height,
    );

    // Render scroll arrows
    crate::tui::widgets::render_scroll_arrows(frame, inner, has_above, has_below);
}
