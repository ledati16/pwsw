//! Rules screen - Manage window matching rules

use std::fmt::Write;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, BorderType, Borders, Cell, Clear, List, ListItem, ListState, Padding, Paragraph, Row,
    Table, TableState,
};

use crate::config::{Rule, SinkConfig};
use crate::style::colors;
use crate::tui::editor_state::EditorState;
use crate::tui::preview::{MAX_SHOWN, PreviewView, build_preview_lines};
use crate::tui::widgets::{
    ValidationState, centered_modal, modal_size, render_input, render_validated_input,
};

/// Rules screen mode
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RulesMode {
    #[default]
    List,
    AddEdit,
    Delete,
    SelectSink, // Dropdown for sink selection
    Inspect,
}

/// Editor state for add/edit modal
#[derive(Debug, Clone)]
pub struct RuleEditor {
    pub app_id_pattern: EditorState,
    pub title_pattern: EditorState,
    pub sink_ref: String,
    pub desc: EditorState,
    pub notify: Option<bool>,
    pub focused_field: usize, // 0=app_id, 1=title, 2=sink, 3=desc, 4=notify
    pub sink_dropdown_index: usize,
    /// State for sink selector dropdown
    pub sink_selector_state: ListState,
}

impl RuleEditor {
    pub(crate) fn new() -> Self {
        Self {
            app_id_pattern: EditorState::new(),
            title_pattern: EditorState::new(),
            sink_ref: String::new(),
            desc: EditorState::new(),
            notify: None,
            focused_field: 0,
            sink_dropdown_index: 0,
            sink_selector_state: ListState::default(),
        }
    }

    pub(crate) fn from_rule(rule: &Rule) -> Self {
        Self {
            app_id_pattern: EditorState::from_string(rule.app_id_pattern.clone()),
            title_pattern: EditorState::from_string(rule.title_pattern.clone().unwrap_or_default()),
            sink_ref: rule.sink_ref.clone(),
            desc: EditorState::from_string(rule.desc.clone().unwrap_or_default()),
            notify: rule.notify,
            focused_field: 0,
            sink_dropdown_index: 0,
            sink_selector_state: ListState::default(),
        }
    }

    pub(crate) const fn next_field(&mut self) {
        if self.focused_field < 4 {
            self.focused_field += 1;
        }
    }

    pub(crate) const fn prev_field(&mut self) {
        if self.focused_field > 0 {
            self.focused_field -= 1;
        }
    }
}

/// Rules screen state
pub struct RulesScreen {
    pub mode: RulesMode,
    pub selected: usize,
    pub editor: RuleEditor,
    pub editing_index: Option<usize>,
    /// Table scroll state
    pub state: TableState,
}

impl RulesScreen {
    pub(crate) fn new() -> Self {
        Self {
            mode: RulesMode::List,
            selected: 0,
            editor: RuleEditor::new(),
            editing_index: None,
            state: TableState::default(),
        }
    }

    pub(crate) const fn select_previous(&mut self, rule_count: usize) {
        if rule_count > 0 && self.selected > 0 {
            self.selected -= 1;
        }
    }

    pub(crate) const fn select_next(&mut self, rule_count: usize) {
        if rule_count > 0 && self.selected < rule_count - 1 {
            self.selected += 1;
        }
    }

    pub(crate) fn start_add(&mut self) {
        self.mode = RulesMode::AddEdit;
        self.editor = RuleEditor::new();
        self.editing_index = None;
    }

    pub(crate) fn start_edit(&mut self, rules: &[Rule]) {
        if self.selected < rules.len() {
            self.mode = RulesMode::AddEdit;
            self.editor = RuleEditor::from_rule(&rules[self.selected]);
            self.editing_index = Some(self.selected);
        }
    }

    pub(crate) const fn start_delete(&mut self) {
        self.mode = RulesMode::Delete;
    }

    pub(crate) const fn open_sink_selector(&mut self) {
        self.mode = RulesMode::SelectSink;
    }

    pub(crate) const fn start_inspect(&mut self) {
        self.mode = RulesMode::Inspect;
    }

    pub(crate) const fn cancel(&mut self) {
        self.mode = RulesMode::List;
    }
}

/// Context for rendering the rules screen (bundles related parameters)
pub struct RulesRenderContext<'a> {
    pub rules: &'a [Rule],
    pub sinks: &'a [SinkConfig],
    pub screen_state: &'a mut RulesScreen,
    /// Live preview of the editor's patterns (kept current by `App::refresh_preview`)
    pub preview: &'a PreviewView,
    pub daemon_running: bool,
}

/// Render the rules screen
pub fn render_rules(frame: &mut Frame, area: Rect, ctx: &mut RulesRenderContext) {
    // Always render the list first as background
    render_list(frame, area, ctx.rules, ctx.sinks, ctx.screen_state);

    // Overlay modal on top if active
    match ctx.screen_state.mode {
        RulesMode::List => {}
        RulesMode::AddEdit => render_editor(
            frame,
            area,
            ctx.sinks,
            ctx.screen_state,
            ctx.preview,
            ctx.daemon_running,
        ),
        RulesMode::Delete => render_delete_confirmation(frame, area, ctx.rules, ctx.screen_state),
        RulesMode::SelectSink => {
            render_sink_selector(frame, area, ctx.sinks, &mut ctx.screen_state.editor);
        }
        RulesMode::Inspect => render_inspect_popup(frame, area, ctx.rules, ctx.screen_state),
    }
}

/// Render the rules list
// Rules list rendering - complex table with multiple columns and highlighting
fn render_list(
    frame: &mut Frame,
    area: Rect,
    rules: &[Rule],
    sinks: &[SinkConfig],
    screen_state: &mut RulesScreen,
) {
    let rows: Vec<Row> = rules
        .iter()
        .enumerate()
        .map(|(i, rule)| {
            let is_selected = i == screen_state.selected;

            // Resolve sink display name (rules may reference a sink by name or description)
            let sink_display = sinks
                .iter()
                .find(|s| s.name == rule.sink_ref || s.desc == rule.sink_ref)
                .map_or(rule.sink_ref.as_str(), |s| s.desc.as_str());

            // Prepare cells
            // Add selection indicator to index
            let index_val = (i + 1).to_string();
            let index_cell = Cell::from(Line::from(vec![
                if is_selected {
                    Span::styled("▎", Style::default().fg(colors::UI_HIGHLIGHT))
                } else {
                    Span::raw(" ")
                },
                Span::raw(index_val),
            ]));

            let app_id_cell = Cell::from(Span::styled(
                rule.app_id_pattern.as_str(),
                if is_selected {
                    Style::default()
                        .fg(colors::UI_TEXT)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(colors::UI_TEXT)
                },
            ));

            let title_cell = rule.title_pattern.as_ref().map_or_else(
                || Cell::from(Span::styled("*", Style::default().fg(colors::UI_SECONDARY))),
                |s| {
                    Cell::from(Span::styled(
                        s.as_str(),
                        Style::default().fg(colors::UI_SECONDARY),
                    ))
                },
            );

            let sink_cell = Cell::from(Span::styled(
                sink_display,
                Style::default().fg(colors::UI_HIGHLIGHT),
            ));

            let desc_cell = rule
                .desc
                .as_ref()
                .map_or_else(|| Cell::from(""), |s| Cell::from(s.as_str()));

            let row_style = if is_selected {
                Style::default().bg(colors::UI_SELECTED_BG)
            } else {
                Style::default()
            };

            Row::new(vec![
                index_cell,
                app_id_cell,
                title_cell,
                sink_cell,
                desc_cell,
            ])
            .style(row_style)
            .height(1)
        })
        .collect();

    let table = Table::new(rows, [
        Constraint::Length(3),      // #
        Constraint::Percentage(25), // App ID
        Constraint::Percentage(25), // Title
        Constraint::Percentage(20), // Target
        Constraint::Percentage(30), // Description
    ])
    .header(
        Row::new(vec![
            "#",
            "App ID Pattern",
            "Title Pattern",
            "Target Sink",
            "Description",
        ])
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
            .title(" Rules "),
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

    // Determine whether there is content above/below the current viewport
    let raw_offset = screen_state.state.offset();
    let total = rules.len();
    // For table rows which do not wrap, use logical offset
    let has_above = raw_offset > 0;
    let has_below = raw_offset + view_height < total;

    // Render scroll arrows using helper
    crate::tui::widgets::render_scroll_arrows(frame, inner, has_above, has_below);
}

/// Render inspect modal
fn render_inspect_popup(frame: &mut Frame, area: Rect, rules: &[Rule], screen_state: &RulesScreen) {
    if screen_state.selected >= rules.len() {
        return;
    }
    let rule = &rules[screen_state.selected];

    let popup_area = centered_modal(modal_size::SMALL, area);
    frame.render_widget(Clear, popup_area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .title(" Rule Details ")
        .style(Style::default().bg(colors::UI_MODAL_BG));
    frame.render_widget(block.clone(), popup_area);

    let inner = block.inner(popup_area);

    let mut lines = Vec::new();

    // Helper for fields with regular values
    let add_field = |lines: &mut Vec<Line>, label: &str, value: &str| {
        lines.push(Line::from(vec![
            Span::styled(
                format!("{label}: "),
                Style::default().fg(colors::UI_SECONDARY),
            ),
            Span::styled(value.to_string(), Style::default().fg(colors::UI_TEXT)),
        ]));
    };

    // Helper for fields with placeholder/auto values
    let add_field_placeholder = |lines: &mut Vec<Line>, label: &str, value: &str| {
        lines.push(Line::from(vec![
            Span::styled(
                format!("{label}: "),
                Style::default().fg(colors::UI_SECONDARY),
            ),
            Span::styled(
                value.to_string(),
                Style::default()
                    .fg(colors::UI_SECONDARY)
                    .add_modifier(Modifier::ITALIC),
            ),
        ]));
    };

    add_field(&mut lines, "App ID Pattern", &rule.app_id_pattern);

    if let Some(title) = &rule.title_pattern {
        add_field(&mut lines, "Title Pattern", title);
    } else {
        add_field_placeholder(&mut lines, "Title Pattern", "(any title)");
    }

    add_field(&mut lines, "Target Sink", &rule.sink_ref);

    if let Some(desc) = &rule.desc {
        add_field(&mut lines, "Description", desc);
    }

    lines.push(Line::from("")); // Space before notify status

    // Notify status with icon
    let (notify_icon, notify_text, notify_color) = match rule.notify {
        Some(true) => ("✓", "Notifications enabled", colors::UI_SUCCESS),
        Some(false) => ("✗", "Notifications disabled", colors::UI_ERROR),
        None => ("○", "Notifications: default", colors::UI_SECONDARY),
    };
    lines.push(Line::from(Span::styled(
        format!("{notify_icon} {notify_text}"),
        Style::default().fg(notify_color),
    )));

    let paragraph = Paragraph::new(lines).wrap(ratatui::widgets::Wrap { trim: false });

    frame.render_widget(paragraph, inner);
}

/// Render the add/edit modal
// Rule editor modal - complex form with multiple fields and validation
fn render_editor(
    frame: &mut Frame,
    area: Rect,
    sinks: &[SinkConfig],
    screen_state: &RulesScreen,
    preview: &PreviewView,
    daemon_running: bool,
) {
    let title = if screen_state.editing_index.is_some() {
        "Edit Rule"
    } else {
        "Add Rule"
    };

    let popup_area = centered_modal(modal_size::LARGE, area);
    frame.render_widget(Clear, popup_area);

    let [
        app_id_area,
        title_area,
        sink_area,
        desc_area,
        notify_area,
        preview_area,
    ] = Layout::vertical([
        Constraint::Length(3), // App ID pattern
        Constraint::Length(3), // Title pattern
        Constraint::Length(3), // Sink selector
        Constraint::Length(3), // Description
        Constraint::Length(3), // Notify toggle
        Constraint::Min(6),    // Live preview
    ])
    .margin(2)
    .areas(popup_area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .padding(Padding::horizontal(1))
        .title(title)
        .style(Style::default().bg(colors::UI_MODAL_BG));
    frame.render_widget(block, popup_area);

    // App ID pattern field with real-time validation
    let app_id_validation = if screen_state.editor.app_id_pattern.value().is_empty() {
        ValidationState::Neutral
    } else if preview.app_error.is_some() {
        ValidationState::Invalid
    } else {
        ValidationState::Valid
    };

    render_validated_input(
        frame,
        app_id_area,
        "App ID Pattern (regex):",
        &screen_state.editor.app_id_pattern.input,
        screen_state.editor.focused_field == 0,
        app_id_validation,
    );

    // Title pattern field with real-time validation
    let title_validation = if screen_state.editor.title_pattern.value().is_empty() {
        ValidationState::Neutral // Empty is OK for title (optional)
    } else if preview.title_error.is_some() {
        ValidationState::Invalid
    } else {
        ValidationState::Valid
    };

    render_validated_input(
        frame,
        title_area,
        "Title Pattern (optional regex):",
        &screen_state.editor.title_pattern.input,
        screen_state.editor.focused_field == 1,
        title_validation,
    );

    // Sink selector button - find sink description if set
    let sink_display = if screen_state.editor.sink_ref.is_empty() {
        None
    } else {
        sinks
            .iter()
            .find(|s| {
                s.name == screen_state.editor.sink_ref || s.desc == screen_state.editor.sink_ref
            })
            .map(|s| s.desc.as_str())
            .or(Some(screen_state.editor.sink_ref.as_str()))
    };

    crate::tui::widgets::render_selector_button(
        frame,
        sink_area,
        "Target Sink",
        sink_display,
        screen_state.editor.focused_field == 2,
    );

    // Description field
    render_input(
        frame,
        desc_area,
        "Description (optional):",
        &screen_state.editor.desc.input,
        screen_state.editor.focused_field == 3,
    );

    // Notify toggle with border-based focus
    let mut notify_spans = Vec::new();
    match screen_state.editor.notify {
        Some(true) => {
            notify_spans.push(Span::styled("✓ ", Style::default().fg(colors::UI_SUCCESS)));
            notify_spans.push(Span::raw("Notify (enabled)"));
        }
        Some(false) => {
            notify_spans.push(Span::styled("✗ ", Style::default().fg(colors::UI_ERROR)));
            notify_spans.push(Span::raw("Notify (disabled)"));
        }
        None => {
            notify_spans.push(Span::styled(
                "○ ",
                Style::default().fg(colors::UI_SECONDARY),
            ));
            notify_spans.push(Span::raw("Notify (use global setting)"));
        }
    }

    let border_style =
        crate::tui::widgets::focus_border_style(screen_state.editor.focused_field == 4);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(border_style);

    let notify_widget = Paragraph::new(Line::from(notify_spans)).block(block);
    frame.render_widget(notify_widget, notify_area);

    // Live preview panel
    render_live_preview(
        frame,
        preview_area,
        screen_state.editor.app_id_pattern.value().is_empty(),
        preview,
        daemon_running,
    );
}

/// Render live regex preview showing matching windows
fn render_live_preview(
    frame: &mut Frame,
    area: Rect,
    app_pattern_empty: bool,
    preview: &PreviewView,
    daemon_running: bool,
) {
    let line = |text: String, color| Line::from(Span::styled(text, Style::default().fg(color)));

    let mut lines = Vec::new();
    if app_pattern_empty {
        // Users who want to match all windows should use `.*` explicitly.
        lines.push(line(
            "  Enter app_id pattern to see preview".to_string(),
            colors::UI_SECONDARY,
        ));
    } else if preview.app_error.is_some() || preview.title_error.is_some() {
        if let Some(e) = &preview.app_error {
            lines.push(line(
                format!("  Invalid app_id regex: {e}"),
                colors::UI_ERROR,
            ));
        }
        if let Some(e) = &preview.title_error {
            lines.push(line(
                format!("  Invalid title regex: {e}"),
                colors::UI_ERROR,
            ));
        }
    } else if !daemon_running {
        lines.push(line(
            "  (daemon not running - no window list)".to_string(),
            colors::UI_SECONDARY,
        ));
    } else if preview.total == 0 {
        lines.push(line(
            "  No matching windows".to_string(),
            colors::UI_WARNING,
        ));
    } else {
        lines.extend(build_preview_lines(&preview.shown));
        if preview.total > MAX_SHOWN {
            let remaining = preview.total - preview.shown.len();
            lines.push(line(
                format!("  ...and {remaining} more"),
                colors::UI_SECONDARY,
            ));
        }
    }

    let preview_widget = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .title("Matching Windows"),
        )
        .wrap(ratatui::widgets::Wrap { trim: false });
    frame.render_widget(preview_widget, area);
}

/// Render sink selector dropdown
fn render_sink_selector(
    frame: &mut Frame,
    area: Rect,
    sinks: &[SinkConfig],
    editor: &mut RuleEditor,
) {
    let popup_area = centered_modal(modal_size::DROPDOWN, area);
    frame.render_widget(Clear, popup_area);

    // Use truncation helpers to avoid overly long sink descriptions/names
    let inner = popup_area.inner(ratatui::layout::Margin {
        vertical: 1,
        horizontal: 0,
    });
    let content_width = inner.width.max(1);

    let items: Vec<ListItem> = sinks
        .iter()
        .map(|sink| {
            let desc = crate::tui::widgets::truncate_desc(&sink.desc, content_width);
            let name = crate::tui::widgets::truncate_node_name(&sink.name, 35);

            let line = Line::from(vec![
                Span::raw("  "),
                Span::styled(desc, Style::default().fg(colors::UI_TEXT)),
                Span::styled(" (", Style::default().fg(colors::UI_SECONDARY)),
                Span::styled(
                    name,
                    Style::default()
                        .fg(colors::UI_SECONDARY)
                        .add_modifier(Modifier::DIM),
                ),
                Span::styled(")", Style::default().fg(colors::UI_SECONDARY)),
            ]);

            ListItem::new(line)
        })
        .collect();

    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .padding(Padding::horizontal(1))
                .title("Select Target Sink")
                .style(Style::default().bg(colors::UI_MODAL_BG)),
        )
        .highlight_style(
            Style::default()
                .fg(colors::UI_HIGHLIGHT)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol(" →")
        .scroll_padding(1);

    // Sync state
    editor
        .sink_selector_state
        .select(Some(editor.sink_dropdown_index));
    frame.render_stateful_widget(list, popup_area, &mut editor.sink_selector_state);

    // Compute visible viewport height for indicators in dropdown
    let inner = popup_area.inner(ratatui::layout::Margin {
        vertical: 1,
        horizontal: 0,
    });
    let view_height = inner.height as usize;

    let raw_offset = editor.sink_selector_state.offset();

    // Build visual_items to account for wrapping like in the selector rendering
    let mut visual_items: Vec<String> = Vec::new();
    for sink in sinks {
        let desc_text = crate::tui::widgets::truncate_desc(&sink.desc, content_width);
        let name_text = crate::tui::widgets::truncate_node_name(&sink.name, 35);
        visual_items.push({
            let mut tmp = String::with_capacity(2 + desc_text.len() + 3 + name_text.len());
            let _ = write!(tmp, "  {desc_text} ({name_text})");
            tmp
        });
    }

    // Compute content width and logical offset
    let content_width = inner.width as usize;

    // Use helper to compute whether content exists above/below (accounts for wrapping)
    let (has_above, has_below) = crate::tui::widgets::compute_has_above_below(
        &visual_items,
        content_width,
        raw_offset,
        view_height,
    );

    // Render arrows via helper
    crate::tui::widgets::render_scroll_arrows(frame, inner, has_above, has_below);
}

/// Render delete confirmation
fn render_delete_confirmation(
    frame: &mut Frame,
    area: Rect,
    rules: &[Rule],
    screen_state: &RulesScreen,
) {
    if screen_state.selected >= rules.len() {
        return;
    }

    let rule = &rules[screen_state.selected];
    let popup_area = centered_modal(modal_size::SMALL, area);
    frame.render_widget(Clear, popup_area);

    let title_line = if let Some(ref title) = rule.title_pattern {
        Line::from(vec![
            Span::raw("Title: "),
            Span::styled(title.as_str(), Style::default().fg(colors::UI_TEXT)),
        ])
    } else {
        Line::from("Title: (any)")
    };

    let text = vec![
        Line::from(""),
        Line::from(vec![Span::styled(
            "Are you sure you want to delete this rule?",
            Style::default()
                .fg(colors::UI_ERROR)
                .add_modifier(Modifier::BOLD),
        )]),
        Line::from(""),
        Line::from(vec![
            Span::raw("App ID: "),
            Span::styled(&rule.app_id_pattern, Style::default().fg(colors::UI_TEXT)),
        ]),
        title_line,
        Line::from(vec![
            Span::raw("Sink: "),
            Span::styled(&rule.sink_ref, Style::default().fg(colors::UI_WARNING)),
        ]),
    ];

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Double)
        .padding(Padding::horizontal(1))
        .title("Delete Rule")
        .style(Style::default().bg(colors::UI_MODAL_BG));

    let paragraph = Paragraph::new(text).block(block);
    frame.render_widget(paragraph, popup_area);
}
