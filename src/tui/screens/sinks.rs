//! Sinks screen - Manage audio output sinks

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    BorderType, Cell, Clear, List, ListItem, ListState, Padding, Paragraph, Row, Table, TableState,
};
use tui_input::Input;

use crate::config::SinkConfig;
use crate::pipewire::{ActiveSink, ProfileSink};
use crate::style::colors;
use crate::tui::editor_state::EditorState;
use crate::tui::widgets::{
    centered_modal, field_line, modal_size, render_input, render_scrollbar, render_text_modal,
    rounded_block,
};

/// Rows of the sinks table above the first sink (header + its margin)
const TABLE_HEADER_ROWS: u16 = 2;

/// Sinks screen mode
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SinksMode {
    #[default]
    List,
    AddEdit,
    Delete,
    SelectSink,
    Inspect,
}

/// Field of the sink editor
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SinkField {
    /// Node name (text, or pick from the node selector with Space)
    #[default]
    Name,
    Desc,
    Icon,
    /// Default checkbox (Space toggles)
    Default,
}

impl SinkField {
    pub(crate) const fn next(self) -> Self {
        match self {
            Self::Name => Self::Desc,
            Self::Desc => Self::Icon,
            Self::Icon | Self::Default => Self::Default,
        }
    }

    pub(crate) const fn prev(self) -> Self {
        match self {
            Self::Name | Self::Desc => Self::Name,
            Self::Icon => Self::Desc,
            Self::Default => Self::Icon,
        }
    }

    /// Whether the field is a text input
    pub(crate) const fn is_text(self) -> bool {
        !matches!(self, Self::Default)
    }
}

/// Editor state for add/edit modal
#[derive(Debug)]
pub struct SinkEditor {
    pub name: EditorState,
    pub desc: EditorState,
    pub icon: EditorState,
    pub default: bool,
    pub focused_field: SinkField,
}

impl SinkEditor {
    pub(crate) fn new() -> Self {
        Self {
            name: EditorState::new(),
            desc: EditorState::new(),
            icon: EditorState::new(),
            default: false,
            focused_field: SinkField::Name,
        }
    }

    pub(crate) fn from_sink(sink: &SinkConfig) -> Self {
        Self {
            name: EditorState::from_string(sink.name.clone()),
            desc: EditorState::from_string(sink.desc.clone()),
            icon: EditorState::from_string(sink.icon.clone().unwrap_or_default()),
            default: sink.default,
            focused_field: SinkField::Name,
        }
    }

    pub(crate) const fn next_field(&mut self) {
        self.focused_field = self.focused_field.next();
    }

    pub(crate) const fn prev_field(&mut self) {
        self.focused_field = self.focused_field.prev();
    }

    /// The text input of the focused field (`None` on the checkbox)
    pub(crate) const fn focused_input_mut(&mut self) -> Option<&mut Input> {
        match self.focused_field {
            SinkField::Name => Some(&mut self.name.input),
            SinkField::Desc => Some(&mut self.desc.input),
            SinkField::Icon => Some(&mut self.icon.input),
            SinkField::Default => None,
        }
    }
}

/// Node names offered by the node selector, in display order (active sinks,
/// then sinks that need a profile switch).
pub(crate) fn selector_names<'a>(
    active: &'a [ActiveSink],
    profiles: &'a [ProfileSink],
) -> Vec<&'a str> {
    active
        .iter()
        .map(|s| s.name.as_str())
        .chain(profiles.iter().map(|s| s.predicted_name.as_str()))
        .collect()
}

/// Row of the node selector list showing the entry at `pos` of
/// [`selector_names`]: entries sit below the "Active" header, profile entries
/// additionally below a spacer and the "Profile" header.
pub(crate) const fn selector_list_index(pos: usize, active_len: usize) -> usize {
    if pos < active_len { 1 + pos } else { 3 + pos }
}

/// Sinks screen state
#[derive(Debug)]
pub struct SinksScreen {
    pub mode: SinksMode,
    pub selected: usize,
    pub editor: SinkEditor,
    pub editing_index: Option<usize>, // None = adding, Some(i) = editing
    /// Node name highlighted in the node selector. Kept by name because the
    /// lists behind the selector are replaced by every `PipeWire` poll.
    pub selector_node: Option<String>,
    /// State for sink selector dropdown
    pub sink_selector_state: ListState,
    /// Table scroll state
    pub state: TableState,
}

impl SinksScreen {
    pub(crate) fn new() -> Self {
        Self {
            mode: SinksMode::List,
            selected: 0,
            editor: SinkEditor::new(),
            editing_index: None,
            selector_node: None,
            sink_selector_state: ListState::default(),
            state: TableState::default(),
        }
    }

    pub(crate) const fn select_previous(&mut self, sink_count: usize) {
        if sink_count > 0 && self.selected > 0 {
            self.selected -= 1;
        }
    }

    pub(crate) const fn select_next(&mut self, sink_count: usize) {
        if sink_count > 0 && self.selected < sink_count - 1 {
            self.selected += 1;
        }
    }

    /// Keep the selection inside a list of `sink_count` sinks.
    pub(crate) const fn clamp_selection(&mut self, sink_count: usize) {
        if self.selected >= sink_count {
            self.selected = sink_count.saturating_sub(1);
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

    pub(crate) const fn start_delete(&mut self) {
        self.mode = SinksMode::Delete;
    }

    pub(crate) const fn start_inspect(&mut self) {
        self.mode = SinksMode::Inspect;
    }

    pub(crate) const fn cancel(&mut self) {
        self.mode = SinksMode::List;
    }

    /// Open the node selector on the edited node name if it is listed, else on
    /// the first entry.
    pub(crate) fn open_selector(&mut self, names: &[&str]) {
        let current = self.editor.name.value();
        self.selector_node = names
            .iter()
            .find(|n| **n == current)
            .or_else(|| names.first())
            .map(|n| (*n).to_string());
        self.mode = SinksMode::SelectSink;
    }

    /// Move the node selector highlight by one entry (`down` or up) within
    /// `names`; a highlight that disappeared restarts at the first entry.
    pub(crate) fn move_selector(&mut self, names: &[&str], down: bool) {
        let pos = self
            .selector_node
            .as_deref()
            .and_then(|node| names.iter().position(|n| *n == node));
        let new = match pos {
            None => 0,
            Some(p) if down => (p + 1).min(names.len().saturating_sub(1)),
            Some(p) => p.saturating_sub(1),
        };
        self.selector_node = names.get(new).map(|n| (*n).to_string());
    }
}

/// Context for rendering the sinks screen (bundles related parameters)
pub struct SinksRenderContext<'a> {
    pub sinks: &'a [SinkConfig],
    pub screen_state: &'a mut SinksScreen,
    pub active_sinks: &'a [String],
    pub active_sink_list: &'a [ActiveSink],
    pub profile_sink_list: &'a [ProfileSink],
    pub pipewire_available: bool,
}

/// Render the sinks screen
pub fn render_sinks(frame: &mut Frame, area: Rect, ctx: &mut SinksRenderContext) {
    // Always render the list first as background
    render_list(
        frame,
        area,
        ctx.sinks,
        ctx.screen_state,
        ctx.active_sinks,
        ctx.pipewire_available,
    );

    // Overlay modal on top if active
    match ctx.screen_state.mode {
        SinksMode::List => {}
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
    // Empty state: show guidance when no sinks configured
    if sinks.is_empty() {
        let guidance = Paragraph::new(vec![
            Line::from(""),
            Line::from(Span::styled(
                "No sinks configured",
                Style::default()
                    .fg(colors::UI_TEXT)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from(vec![
                Span::raw("Press "),
                Span::styled("a", Style::default().fg(colors::UI_HIGHLIGHT)),
                Span::raw(" to add your first sink."),
            ]),
            Line::from(""),
            Line::from(Span::styled(
                "Tip: Run 'pwsw list-sinks' in another terminal",
                Style::default().fg(colors::UI_SECONDARY),
            )),
            Line::from(Span::styled(
                "to discover available audio outputs.",
                Style::default().fg(colors::UI_SECONDARY),
            )),
        ])
        .alignment(ratatui::layout::Alignment::Center);
        frame.render_widget(guidance, area);
        return;
    }

    screen_state.clamp_selection(sinks.len());

    // If PipeWire is unavailable, show warning and adjust area for table
    let table_area = if pipewire_available {
        area
    } else {
        let [warning_area, table_area] =
            Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(area);
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
        frame.render_widget(warning, warning_area);
        table_area
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

            let status_cell = Cell::from(Line::from(vec![
                if is_selected {
                    Span::styled("▎", Style::default().fg(colors::UI_HIGHLIGHT))
                } else {
                    Span::raw(" ")
                },
                status_span,
            ]));

            // Description Cell (with icon if present)
            let desc_text = match &sink.icon {
                Some(icon) => format!("{icon} {}", sink.desc),
                None => sink.desc.clone(),
            };
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

    let table = Table::new(rows, [
        Constraint::Length(10),     // Status
        Constraint::Percentage(40), // Desc
        Constraint::Percentage(40), // Name
        Constraint::Length(10),     // Flags
    ])
    .header(
        Row::new(vec!["Status", "Description", "Node Name", "Flags"])
            .style(
                Style::default()
                    .fg(colors::UI_WARNING)
                    .add_modifier(Modifier::BOLD),
            )
            .bottom_margin(1),
    )
    .block(rounded_block().title(" Sinks "));

    // Sync state
    screen_state.state.select(Some(screen_state.selected));
    frame.render_stateful_widget(table, table_area, &mut screen_state.state);
    render_scrollbar(
        frame,
        table_area,
        TABLE_HEADER_ROWS,
        sinks.len(),
        screen_state.state.offset(),
    );
}

/// Render inspect modal
fn render_inspect_popup(
    frame: &mut Frame,
    area: Rect,
    sinks: &[SinkConfig],
    screen_state: &SinksScreen,
) {
    let Some(sink) = sinks.get(screen_state.selected) else {
        return;
    };

    let mut lines = vec![
        field_line("Name", &sink.name, false),
        field_line("Description", &sink.desc, false),
        match &sink.icon {
            Some(icon) => field_line("Icon", icon, false),
            None => field_line("Icon", "(auto-detected)", true),
        },
        Line::from(""),
    ];
    lines.push(if sink.default {
        Line::from(Span::styled(
            "✓ Default Sink",
            Style::default().fg(colors::UI_SUCCESS),
        ))
    } else {
        Line::from(Span::styled(
            "○ Not Default",
            Style::default().fg(colors::UI_SECONDARY),
        ))
    });

    render_text_modal(frame, area, " Sink Details ", BorderType::Rounded, lines);
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
    frame.render_widget(Clear, popup_area);

    let [name_area, desc_area, icon_area, default_area] = Layout::vertical([
        Constraint::Length(3), // Name field
        Constraint::Length(3), // Desc field
        Constraint::Length(3), // Icon field
        Constraint::Length(3), // Default checkbox
    ])
    .margin(2)
    .areas(popup_area);

    // Background block
    let block = rounded_block()
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
        screen_state.editor.focused_field == SinkField::Name,
    );

    // Desc field
    render_input(
        frame,
        desc_area,
        "Description:",
        &screen_state.editor.desc.input,
        screen_state.editor.focused_field == SinkField::Desc,
    );

    // Icon field
    render_input(
        frame,
        icon_area,
        "Icon (optional):",
        &screen_state.editor.icon.input,
        screen_state.editor.focused_field == SinkField::Icon,
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

    let border_style = crate::tui::widgets::focus_border_style(
        screen_state.editor.focused_field == SinkField::Default,
    );
    let checkbox = Paragraph::new(Line::from(checkbox_spans))
        .block(rounded_block().border_style(border_style));
    frame.render_widget(checkbox, default_area);
}

/// Render delete confirmation modal
fn render_delete_confirmation(
    frame: &mut Frame,
    area: Rect,
    sinks: &[SinkConfig],
    screen_state: &SinksScreen,
) {
    let Some(sink) = sinks.get(screen_state.selected) else {
        return;
    };

    let text = vec![
        Line::from(""),
        Line::from(vec![Span::styled(
            "Are you sure you want to delete this sink?",
            Style::default()
                .fg(colors::UI_ERROR)
                .add_modifier(Modifier::BOLD),
        )]),
        Line::from(""),
        field_line("Description", &sink.desc, false),
        field_line("Node Name", &sink.name, false),
    ];
    render_text_modal(frame, area, " Delete Sink ", BorderType::Double, text);
}

/// One node selector entry: "  description (node name)"
fn selector_item(description: &str, node: &str, max_desc_width: u16) -> ListItem<'static> {
    ListItem::new(Line::from(vec![
        Span::raw("  "),
        Span::styled(
            crate::tui::widgets::truncate_desc(description, max_desc_width),
            Style::default().fg(colors::UI_TEXT),
        ),
        Span::styled(" (", Style::default().fg(colors::UI_SECONDARY)),
        Span::styled(
            crate::tui::widgets::truncate_node_name(node, 35),
            Style::default()
                .fg(colors::UI_SECONDARY)
                .add_modifier(Modifier::DIM),
        ),
        Span::styled(")", Style::default().fg(colors::UI_SECONDARY)),
    ]))
}

/// Render sink selector modal for adding sinks
fn render_sink_selector(
    frame: &mut Frame,
    area: Rect,
    active_sinks: &[ActiveSink],
    profile_sinks: &[ProfileSink],
    screen_state: &mut SinksScreen,
) {
    let popup_area = centered_modal(modal_size::MEDIUM, area);
    frame.render_widget(Clear, popup_area);

    // Calculate max width for text (accounting for borders, margin, and formatting)
    let available_width = popup_area.width.saturating_sub(6); // 2 borders + 2 prefix + 2 margin
    let max_desc_width = available_width.saturating_sub(40); // Reserve space for node name in parens

    let mut items: Vec<ListItem> = vec![ListItem::new(Line::from(Span::styled(
        "── Active Sinks ──",
        Style::default()
            .fg(colors::UI_HIGHLIGHT)
            .add_modifier(Modifier::BOLD),
    )))];
    items.extend(
        active_sinks
            .iter()
            .map(|s| selector_item(&s.description, &s.name, max_desc_width)),
    );
    if !profile_sinks.is_empty() {
        items.push(ListItem::new(Line::from("")));
        items.push(ListItem::new(Line::from(Span::styled(
            "── Profile Sinks (require switching) ──",
            Style::default()
                .fg(colors::UI_WARNING)
                .add_modifier(Modifier::BOLD),
        ))));
        items.extend(
            profile_sinks
                .iter()
                .map(|s| selector_item(&s.description, &s.predicted_name, max_desc_width)),
        );
    }
    let total_items = items.len();

    let list = List::new(items)
        .block(
            rounded_block()
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

    // Highlight the selected node wherever it is in the current lists
    let names = selector_names(active_sinks, profile_sinks);
    let row = screen_state
        .selector_node
        .as_deref()
        .and_then(|node| names.iter().position(|n| *n == node))
        .map(|pos| selector_list_index(pos, active_sinks.len()));
    screen_state.sink_selector_state.select(row);

    frame.render_stateful_widget(list, popup_area, &mut screen_state.sink_selector_state);
    render_scrollbar(
        frame,
        popup_area,
        0,
        total_items,
        screen_state.sink_selector_state.offset(),
    );
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    fn active(name: &str) -> ActiveSink {
        ActiveSink {
            name: name.to_string(),
            description: format!("{name} desc"),
            is_default: false,
        }
    }

    fn profile(name: &str) -> ProfileSink {
        ProfileSink {
            predicted_name: name.to_string(),
            description: format!("{name} desc"),
            device_name: "dev".to_string(),
            device_id: 1,
            profile_name: "p".to_string(),
            profile_index: 1,
        }
    }

    #[test]
    fn test_selector_list_index_skips_headers() {
        // [Active header] a0 a1 [spacer] [Profile header] p0 p1
        assert_eq!(selector_list_index(0, 2), 1);
        assert_eq!(selector_list_index(1, 2), 2);
        assert_eq!(selector_list_index(2, 2), 5);
        assert_eq!(selector_list_index(3, 2), 6);
        // Only profile sinks: [Active header] [spacer] [Profile header] p0
        assert_eq!(selector_list_index(0, 0), 3);
    }

    #[test]
    fn test_selector_follows_node_when_lists_change() {
        let mut screen = SinksScreen::new();
        let before = [active("alsa.a"), active("alsa.b")];
        let names = selector_names(&before, &[]);
        screen.open_selector(&names);
        screen.move_selector(&names, true);
        assert_eq!(screen.selector_node.as_deref(), Some("alsa.b"));

        // A Bluetooth sink appears in front: the highlight stays on alsa.b
        let after = [active("bluez.x"), active("alsa.a"), active("alsa.b")];
        let profiles = [profile("alsa.c")];
        let names = selector_names(&after, &profiles);
        assert_eq!(screen.selector_node.as_deref(), Some("alsa.b"));
        screen.move_selector(&names, true);
        assert_eq!(screen.selector_node.as_deref(), Some("alsa.c"));
        screen.move_selector(&names, true);
        assert_eq!(
            screen.selector_node.as_deref(),
            Some("alsa.c"),
            "stops at end"
        );
        screen.move_selector(&names, false);
        assert_eq!(screen.selector_node.as_deref(), Some("alsa.b"));
    }

    #[test]
    fn test_open_selector_preselects_edited_node() {
        let mut screen = SinksScreen::new();
        screen.editor.name.set_value("alsa.b".to_string());
        let sinks = [active("alsa.a"), active("alsa.b")];
        screen.open_selector(&selector_names(&sinks, &[]));
        assert_eq!(screen.selector_node.as_deref(), Some("alsa.b"));

        let mut empty = SinksScreen::new();
        empty.open_selector(&[]);
        assert_eq!(empty.selector_node, None);
    }
}
