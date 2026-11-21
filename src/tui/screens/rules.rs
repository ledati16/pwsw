//! Rules screen - Manage window matching rules

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    BorderType, Cell, Clear, List, ListItem, ListState, Padding, Paragraph, Row, Table, TableState,
};
use tui_input::Input;

use crate::config::{Rule, SinkConfig};
use crate::style::colors;
use crate::tui::editor_state::EditorState;
use crate::tui::preview::{MAX_SHOWN, PreviewView, build_preview_lines};
use crate::tui::widgets::{
    ValidationState, centered_modal, field_line, modal_size, render_input, render_scrollbar,
    render_text_modal, render_validated_input, rounded_block,
};

/// Rows of the rules table above the first rule (header + its margin)
const TABLE_HEADER_ROWS: u16 = 2;

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

/// Field of the rule editor
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RuleField {
    #[default]
    AppId,
    Title,
    /// Target sink (Space opens the selector)
    Sink,
    Desc,
    /// Notify toggle (Space cycles it)
    Notify,
}

impl RuleField {
    pub(crate) const fn next(self) -> Self {
        match self {
            Self::AppId => Self::Title,
            Self::Title => Self::Sink,
            Self::Sink => Self::Desc,
            Self::Desc | Self::Notify => Self::Notify,
        }
    }

    pub(crate) const fn prev(self) -> Self {
        match self {
            Self::AppId | Self::Title => Self::AppId,
            Self::Sink => Self::Title,
            Self::Desc => Self::Sink,
            Self::Notify => Self::Desc,
        }
    }

    /// Whether the field is a text input
    pub(crate) const fn is_text(self) -> bool {
        matches!(self, Self::AppId | Self::Title | Self::Desc)
    }
}

/// Editor state for add/edit modal
#[derive(Debug, Clone)]
pub struct RuleEditor {
    pub app_id_pattern: EditorState,
    pub title_pattern: EditorState,
    pub sink_ref: String,
    pub desc: EditorState,
    pub notify: Option<bool>,
    pub focused_field: RuleField,
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
            focused_field: RuleField::AppId,
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
            focused_field: RuleField::AppId,
            sink_dropdown_index: 0,
            sink_selector_state: ListState::default(),
        }
    }

    pub(crate) const fn next_field(&mut self) {
        self.focused_field = self.focused_field.next();
    }

    pub(crate) const fn prev_field(&mut self) {
        self.focused_field = self.focused_field.prev();
    }

    /// The text input of the focused field (`None` on the selector and toggle)
    pub(crate) const fn focused_input_mut(&mut self) -> Option<&mut Input> {
        match self.focused_field {
            RuleField::AppId => Some(&mut self.app_id_pattern.input),
            RuleField::Title => Some(&mut self.title_pattern.input),
            RuleField::Desc => Some(&mut self.desc.input),
            RuleField::Sink | RuleField::Notify => None,
        }
    }
}

/// Rules screen state
#[derive(Debug)]
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

    /// Keep the selection inside a list of `rule_count` rules.
    pub(crate) const fn clamp_selection(&mut self, rule_count: usize) {
        if self.selected >= rule_count {
            self.selected = rule_count.saturating_sub(1);
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

    /// Open the target sink selector on the sink the rule currently targets.
    pub(crate) fn open_sink_selector(&mut self, sinks: &[SinkConfig]) {
        let current = &self.editor.sink_ref;
        self.editor.sink_dropdown_index = sinks
            .iter()
            .position(|s| s.desc == *current || s.name == *current)
            .unwrap_or(0);
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
    screen_state.clamp_selection(rules.len());
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
    .block(rounded_block().title(" Rules "));

    // Sync state
    screen_state.state.select(Some(screen_state.selected));
    frame.render_stateful_widget(table, area, &mut screen_state.state);
    render_scrollbar(
        frame,
        area,
        TABLE_HEADER_ROWS,
        rules.len(),
        screen_state.state.offset(),
    );
}

/// Render inspect modal
fn render_inspect_popup(frame: &mut Frame, area: Rect, rules: &[Rule], screen_state: &RulesScreen) {
    let Some(rule) = rules.get(screen_state.selected) else {
        return;
    };

    let mut lines = vec![
        field_line("App ID Pattern", &rule.app_id_pattern, false),
        match &rule.title_pattern {
            Some(title) => field_line("Title Pattern", title, false),
            None => field_line("Title Pattern", "(any title)", true),
        },
        field_line("Target Sink", &rule.sink_ref, false),
    ];
    if let Some(desc) = &rule.desc {
        lines.push(field_line("Description", desc, false));
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

    render_text_modal(frame, area, " Rule Details ", BorderType::Rounded, lines);
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

    let block = rounded_block()
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
        screen_state.editor.focused_field == RuleField::AppId,
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
        screen_state.editor.focused_field == RuleField::Title,
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
        screen_state.editor.focused_field == RuleField::Sink,
    );

    // Description field
    render_input(
        frame,
        desc_area,
        "Description (optional):",
        &screen_state.editor.desc.input,
        screen_state.editor.focused_field == RuleField::Desc,
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

    let border_style = crate::tui::widgets::focus_border_style(
        screen_state.editor.focused_field == RuleField::Notify,
    );
    let notify_widget =
        Paragraph::new(Line::from(notify_spans)).block(rounded_block().border_style(border_style));
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
        .block(rounded_block().title("Matching Windows"))
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
    let content_width = popup_area.width.saturating_sub(2).max(1);

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
            rounded_block()
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

    // Sync state (the sink list may have shrunk since the index was chosen)
    editor.sink_dropdown_index = editor
        .sink_dropdown_index
        .min(sinks.len().saturating_sub(1));
    editor
        .sink_selector_state
        .select((!sinks.is_empty()).then_some(editor.sink_dropdown_index));
    frame.render_stateful_widget(list, popup_area, &mut editor.sink_selector_state);
    render_scrollbar(
        frame,
        popup_area,
        0,
        sinks.len(),
        editor.sink_selector_state.offset(),
    );
}

/// Render delete confirmation
fn render_delete_confirmation(
    frame: &mut Frame,
    area: Rect,
    rules: &[Rule],
    screen_state: &RulesScreen,
) {
    let Some(rule) = rules.get(screen_state.selected) else {
        return;
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
        field_line("App ID", &rule.app_id_pattern, false),
        match &rule.title_pattern {
            Some(title) => field_line("Title", title, false),
            None => field_line("Title", "(any)", true),
        },
        field_line("Sink", &rule.sink_ref, false),
    ];
    render_text_modal(frame, area, " Delete Rule ", BorderType::Double, text);
}
