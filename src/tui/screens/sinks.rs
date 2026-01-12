//! Sinks screen - Manage audio output sinks

use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, BorderType, Borders, Cell, Clear, ListItem, ListState, Padding, Paragraph, Row,
        Table, TableState,
    },
};
use std::fmt::Write;

use crate::config::SinkConfig;
use crate::style::colors;
use crate::tui::editor_state::EditorState;
use crate::tui::widgets::{centered_modal, modal_size, render_input};

/// Sinks screen mode
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum SinksMode {
    #[default]
    List,
    AddEdit,
    Delete,
    SelectSink,
    Inspect,
}

/// Editor state for add/edit modal
pub(crate) struct SinkEditor {
    pub name: EditorState,
    pub desc: EditorState,
    pub icon: EditorState,
    pub default: bool,
    pub focused_field: usize, // 0=name, 1=desc, 2=icon, 3=default
}

impl SinkEditor {
    pub(crate) fn new() -> Self {
        Self {
            name: EditorState::new(),
            desc: EditorState::new(),
            icon: EditorState::new(),
            default: false,
            focused_field: 0,
        }
    }

    pub(crate) fn from_sink(sink: &SinkConfig) -> Self {
        Self {
            name: EditorState::from_string(sink.name.clone()),
            desc: EditorState::from_string(sink.desc.clone()),
            icon: EditorState::from_string(sink.icon.clone().unwrap_or_default()),
            default: sink.default,
            focused_field: 0,
        }
    }

    pub(crate) fn next_field(&mut self) {
        if self.focused_field < 3 {
            self.focused_field += 1;
        }
    }

    pub(crate) fn prev_field(&mut self) {
        if self.focused_field > 0 {
            self.focused_field -= 1;
        }
    }
}

/// Sinks screen state
pub(crate) struct SinksScreen {
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
    pub(crate) fn update_display_descs(&mut self, sinks: &[SinkConfig]) {
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
    pub(crate) fn new() -> Self {
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

    pub(crate) fn select_previous(&mut self, sink_count: usize) {
        if sink_count > 0 && self.selected > 0 {
            self.selected -= 1;
        }
    }

    pub(crate) fn select_next(&mut self, sink_count: usize) {
        if sink_count > 0 && self.selected < sink_count - 1 {
            self.selected += 1;
        }
    }

    pub(crate) fn start_add(&mut self) {
        self.mode = SinksMode::AddEdit;
        self.editor = SinkEditor::new();
        self.editing_index = None;
    }

    pub(crate) fn start_edit(&mut self, sinks: &[SinkConfig]) {
        if self.selected < sinks.len() {
            self.mode = SinksMode::AddEdit;
            self.editor = SinkEditor::from_sink(&sinks[self.selected]);
            self.editing_index = Some(self.selected);
        }
    }

    pub(crate) fn start_delete(&mut self) {
        self.mode = SinksMode::Delete;
    }

    pub(crate) fn start_inspect(&mut self) {
        self.mode = SinksMode::Inspect;
    }

    pub(crate) fn cancel(&mut self) {
        self.mode = SinksMode::List;
    }
}

/// Context for rendering the sinks screen (bundles related parameters)
pub(crate) struct SinksRenderContext<'a> {
    pub sinks: &'a [SinkConfig],
    pub screen_state: &'a mut SinksScreen,
    pub active_sinks: &'a [String],
    pub active_sink_list: &'a [crate::pipewire::ActiveSink],
    pub profile_sink_list: &'a [crate::pipewire::ProfileSink],
    pub pipewire_available: bool,
}

/// Render the sinks screen
pub(crate) fn render_sinks(frame: &mut Frame, area: Rect, ctx: &mut SinksRenderContext) {
    match ctx.screen_state.mode {
        SinksMode::List => {
            render_list(
                frame,
                area,
                ctx.sinks,
                ctx.screen_state,
                ctx.active_sinks,
                ctx.pipewire_available,
            );
        }
        SinksMode::AddEdit => render_editor(frame, area, ctx.screen_state),
        SinksMode::Delete => render_delete_confirmation(frame, area, ctx.sinks, ctx.screen_state),
        SinksMode::SelectSink => render_sink_selector(
            frame,
            area,
            ctx.active_sink_list,
            ctx.profile_sink_list,
            ctx.screen_state,
        ),
        SinksMode::Inspect => render_inspect_popup(frame, area, ctx.sinks, ctx.screen_state),
    }
}

/// Render the sinks list
fn render_list(
    frame: &mut Frame,
    area: Rect,
    sinks: &[SinkConfig],
    screen_state: &mut SinksScreen,
    active_sinks: &[String],
    pipewire_available: bool,
) {
    // If PipeWire is unavailable, show warning and adjust area for table
    let table_area = if pipewire_available {
        area
    } else {
        let chunks = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).split(area);
        let warning = Paragraph::new(Line::from(vec![
            Span::styled("⚠ ", Style::default().fg(colors::UI_WARNING)),
            Span::styled(
                "PipeWire unavailable",
                Style::default().fg(colors::UI_WARNING),
            ),
            Span::styled(
                " - sink status may be stale",
                Style::default().fg(colors::UI_SECONDARY),
            ),
        ]));
        frame.render_widget(warning, chunks[0]);
        chunks[1]
    };

    let rows: Vec<Row> = sinks
        .iter()
        .enumerate()
        .map(|(i, sink)| {
            let is_selected = i == screen_state.selected;
            let is_active = active_sinks.contains(&sink.name);

            // Status Cell
            let status_span = if is_active {
                Span::styled("● Active", Style::default().fg(colors::UI_SUCCESS))
            } else {
                Span::styled("○", Style::default().fg(colors::UI_SECONDARY))
            };

            let status_cell = if is_selected {
                Cell::from(Line::from(vec![
                    Span::styled("▎", Style::default().fg(colors::UI_HIGHLIGHT)),
                    status_span,
                ]))
            } else {
                Cell::from(status_span)
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
                        .fg(colors::UI_TEXT)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(colors::UI_TEXT)
                },
            ));

            // Name Cell (Technical ID)
            let name_cell = Cell::from(Span::styled(
                &sink.name,
                Style::default().fg(colors::UI_SECONDARY),
            ));

            // Flags Cell
            let flags_cell = if sink.default {
                Cell::from(Span::styled(
                    "DEFAULT",
                    Style::default().fg(colors::UI_WARNING),
                ))
            } else {
                Cell::from("")
            };

            let row_style = if is_selected {
                Style::default().bg(colors::UI_SELECTED_BG)
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
                    .fg(colors::UI_WARNING)
                    .add_modifier(Modifier::BOLD),
            )
            .bottom_margin(1),
    )
    .block(
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .title(" Sinks "),
    );

    // Sync state
    screen_state.state.select(Some(screen_state.selected));
    frame.render_stateful_widget(table, table_area, &mut screen_state.state);

    // Compute visible viewport (inner area) for arrow indicators
    let inner = table_area.inner(ratatui::layout::Margin {
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

/// Render inspect modal
fn render_inspect_popup(
    frame: &mut Frame,
    area: Rect,
    sinks: &[SinkConfig],
    screen_state: &SinksScreen,
) {
    if screen_state.selected >= sinks.len() {
        return;
    }
    let sink = &sinks[screen_state.selected];

    let popup_area = centered_modal(modal_size::MEDIUM, area);
    frame.render_widget(Clear, popup_area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .title(" Sink Details ")
        .style(Style::default().bg(colors::UI_MODAL_BG));
    frame.render_widget(block.clone(), popup_area);

    let inner = block.inner(popup_area);

    let mut lines = Vec::new();

    // Helper for fields
    let mut add_field = |label: &str, value: &str| {
        lines.push(Line::from(vec![
            Span::styled(
                format!("{label}: "),
                Style::default().fg(colors::UI_SECONDARY),
            ),
            Span::styled(value.to_string(), Style::default().fg(colors::UI_TEXT)),
        ]));
        lines.push(Line::from(""));
    };

    add_field("Name", &sink.name);
    add_field("Description", &sink.desc);

    if let Some(icon) = &sink.icon {
        add_field("Icon", icon);
    } else {
        add_field("Icon", "(auto-detected)");
    }

    if sink.default {
        lines.push(Line::from(Span::styled(
            "✓ Default Sink",
            Style::default().fg(colors::UI_SUCCESS),
        )));
    } else {
        lines.push(Line::from(Span::styled(
            "✗ Not Default",
            Style::default().fg(colors::UI_SECONDARY),
        )));
    }

    // Hint at bottom
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "Press [Enter] or [Esc] to close",
        Style::default().fg(colors::UI_HIGHLIGHT),
    )));

    let paragraph = Paragraph::new(lines).wrap(ratatui::widgets::Wrap { trim: false });

    frame.render_widget(paragraph, inner);
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

    let [name_area, desc_area, icon_area, default_area] = Layout::vertical([
        Constraint::Length(3), // Name field
        Constraint::Length(3), // Desc field
        Constraint::Length(3), // Icon field
        Constraint::Length(3), // Default checkbox
    ])
    .margin(2)
    .areas(popup_area);

    // Background block
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .padding(Padding::horizontal(1))
        .title(title)
        .style(Style::default().bg(colors::UI_MODAL_BG));
    frame.render_widget(block, popup_area);

    // Name field - use button-like selector
    let name_display = if screen_state.editor.name.value().is_empty() {
        None
    } else {
        Some(screen_state.editor.name.value())
    };

    crate::tui::widgets::render_selector_button(
        frame,
        name_area,
        "Node Name",
        name_display,
        screen_state.editor.focused_field == 0,
    );

    // Desc field
    render_input(
        frame,
        desc_area,
        "Description:",
        &screen_state.editor.desc.input,
        screen_state.editor.focused_field == 1,
    );

    // Icon field
    render_input(
        frame,
        icon_area,
        "Icon (optional):",
        &screen_state.editor.icon.input,
        screen_state.editor.focused_field == 2,
    );

    // Default checkbox with border-based focus
    let mut checkbox_spans = Vec::new();
    if screen_state.editor.default {
        checkbox_spans.push(Span::styled("✓ ", Style::default().fg(colors::UI_SUCCESS)));
        checkbox_spans.push(Span::raw("Default Sink"));
    } else {
        checkbox_spans.push(Span::styled("✗ ", Style::default().fg(colors::UI_ERROR)));
        checkbox_spans.push(Span::raw("Default Sink"));
    }

    let border_style =
        crate::tui::widgets::focus_border_style(screen_state.editor.focused_field == 3);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(border_style);

    let checkbox = Paragraph::new(Line::from(checkbox_spans)).block(block);
    frame.render_widget(checkbox, default_area);
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
            Style::default()
                .fg(colors::UI_ERROR)
                .add_modifier(Modifier::BOLD),
        )]),
        Line::from(""),
        Line::from(vec![
            Span::raw("Description: "),
            Span::styled(&sink.desc, Style::default().fg(colors::UI_TEXT)),
        ]),
        Line::from(vec![
            Span::raw("Node Name: "),
            Span::styled(&sink.name, Style::default().fg(colors::UI_SECONDARY)),
        ]),
    ];

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Double)
        .padding(Padding::horizontal(1))
        .title("Delete Sink")
        .style(Style::default().bg(colors::UI_MODAL_BG));

    let paragraph = Paragraph::new(text).block(block);
    frame.render_widget(paragraph, popup_area);
}

/// Render sink selector modal for adding sinks
// Sink selector modal rendering - complex table with multiple sections
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
            .fg(colors::UI_HIGHLIGHT)
            .add_modifier(Modifier::BOLD),
    ))));

    for sink in active_sinks {
        let desc_text = crate::tui::widgets::truncate_desc(&sink.description, max_desc_width);
        let name_text = crate::tui::widgets::truncate_node_name(&sink.name, 35);

        let line = Line::from(vec![
            Span::raw("  "),
            Span::styled(desc_text, Style::default().fg(colors::UI_TEXT)),
            Span::styled(" (", Style::default().fg(colors::UI_SECONDARY)),
            Span::styled(
                name_text,
                Style::default()
                    .fg(colors::UI_SECONDARY)
                    .add_modifier(Modifier::DIM),
            ),
            Span::styled(")", Style::default().fg(colors::UI_SECONDARY)),
        ]);
        items.push(ListItem::new(line));
    }

    // Profile sinks header (if any) — List items already appended above
    if !profile_sinks.is_empty() {
        items.push(ListItem::new(Line::from("")));
        items.push(ListItem::new(Line::from(Span::styled(
            "── Profile Sinks (require switching) ──",
            Style::default()
                .fg(colors::UI_WARNING)
                .add_modifier(Modifier::BOLD),
        ))));

        for sink in profile_sinks {
            let desc_text = crate::tui::widgets::truncate_desc(&sink.description, max_desc_width);
            let name_text = crate::tui::widgets::truncate_node_name(&sink.predicted_name, 35);

            let line = Line::from(vec![
                Span::raw("  "),
                Span::styled(desc_text.clone(), Style::default().fg(colors::UI_TEXT)),
                Span::styled(" (", Style::default().fg(colors::UI_SECONDARY)),
                Span::styled(
                    name_text.clone(),
                    Style::default()
                        .fg(colors::UI_SECONDARY)
                        .add_modifier(Modifier::DIM),
                ),
                Span::styled(")", Style::default().fg(colors::UI_SECONDARY)),
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
                .border_type(BorderType::Rounded)
                .padding(Padding::horizontal(1))
                .title("Select Node")
                .style(Style::default().bg(colors::UI_MODAL_BG)),
        )
        .highlight_style(
            Style::default()
                .fg(colors::UI_HIGHLIGHT)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol(" →")
        .scroll_padding(1);

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
}

#[cfg(test)]
mod tests {
    /// Helper to calculate the list index from a logical selector index
    /// This mirrors the logic in `render_sink_selector_popup` (lines 556-571)
    fn calculate_list_index(
        selector_index: usize,
        active_len: usize,
        profile_len: usize,
    ) -> Option<usize> {
        let total_selectable = active_len + profile_len;
        if total_selectable == 0 {
            return None;
        }

        let sel = selector_index.min(total_selectable - 1);
        let list_index = if sel < active_len {
            // Active sink: after the active header (line 0 is header, sinks start at line 1)
            1 + sel
        } else {
            // Profile sink: account for active items + spacer + profile header
            // Layout: [Active header] [active sinks...] [spacer] [Profile header] [profile sinks...]
            // Indices: 0              1..active_len+1    active_len+1  active_len+2      active_len+3...
            let profile_idx = sel - active_len;
            3 + active_len + profile_idx
        };
        Some(list_index)
    }

    #[test]
    fn test_sink_selector_index_mapping_empty_lists() {
        // Case 1: Empty lists → no selection
        assert_eq!(calculate_list_index(0, 0, 0), None);
    }

    #[test]
    fn test_sink_selector_index_mapping_only_active() {
        // Case 2: Only active sinks (3 items)
        // Layout: [Header(0)] [Active0(1)] [Active1(2)] [Active2(3)]
        assert_eq!(calculate_list_index(0, 3, 0), Some(1)); // First active → list index 1
        assert_eq!(calculate_list_index(1, 3, 0), Some(2)); // Second active → list index 2
        assert_eq!(calculate_list_index(2, 3, 0), Some(3)); // Third active → list index 3
    }

    #[test]
    fn test_sink_selector_index_mapping_only_profile() {
        // Case 3: Only profile sinks (2 items)
        // Layout: [Active Header(0)] [Spacer(1)] [Profile Header(2)] [Profile0(3)] [Profile1(4)]
        assert_eq!(calculate_list_index(0, 0, 2), Some(3)); // First profile → list index 3
        assert_eq!(calculate_list_index(1, 0, 2), Some(4)); // Second profile → list index 4
    }

    #[test]
    fn test_sink_selector_index_mapping_mixed() {
        // Case 4: Mixed (2 active, 2 profile)
        // Layout: [Active Header(0)] [Active0(1)] [Active1(2)] [Spacer(3)] [Profile Header(4)] [Profile0(5)] [Profile1(6)]
        assert_eq!(calculate_list_index(0, 2, 2), Some(1)); // First active → list index 1
        assert_eq!(calculate_list_index(1, 2, 2), Some(2)); // Second active → list index 2
        assert_eq!(calculate_list_index(2, 2, 2), Some(5)); // First profile → list index 5 (3 + 2 + 0)
        assert_eq!(calculate_list_index(3, 2, 2), Some(6)); // Second profile → list index 6 (3 + 2 + 1)
    }

    #[test]
    fn test_sink_selector_index_mapping_out_of_bounds() {
        // Case 5: Selector index exceeds total → should clamp to last item
        assert_eq!(calculate_list_index(10, 2, 2), Some(6)); // Clamped to last profile (index 3)
    }
}
