//! Rules screen - Manage window matching rules

use std::borrow::Cow;

use ratatui::Frame;
use ratatui::layout::{Constraint, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Cell, Clear, HighlightSpacing, List, ListItem, ListState, Paragraph, Row, Table, TableState,
};
use tui_input::Input;

use crate::config::{Rule, SinkConfig};
use crate::tui::editor_state::EditorState;
use crate::tui::preview::{MAX_SHOWN, PreviewView, build_preview_lines};
use crate::tui::screens::sinks::render_centered;
use crate::tui::theme;
use crate::tui::widgets::{
    FieldValue, FormField, MAX_DIALOG_WIDTH, border_hints, column_widths, detail_lines,
    dialog_block, ellipsize, modal_rect, open_dialog, picker_lines, render_form, render_scrollbar,
    render_text_dialog, rounded_block, text_width,
};

/// Rows of the rules table above the first rule (the header)
const TABLE_HEADER_ROWS: u16 = 1;

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
    /// App ID, title, sink, description and notify when the editor opened
    initial: RuleValues,
}

/// The editable values of a rule, to tell whether the editor changed any
type RuleValues = (String, String, String, String, Option<bool>);

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
            initial: RuleValues::default(),
        }
    }

    pub(crate) fn from_rule(rule: &Rule) -> Self {
        let mut editor = Self {
            app_id_pattern: EditorState::from_string(rule.app_id_pattern.clone()),
            title_pattern: EditorState::from_string(rule.title_pattern.clone().unwrap_or_default()),
            sink_ref: rule.sink_ref.clone(),
            desc: EditorState::from_string(rule.desc.clone().unwrap_or_default()),
            notify: rule.notify,
            focused_field: RuleField::AppId,
            sink_dropdown_index: 0,
            sink_selector_state: ListState::default(),
            initial: RuleValues::default(),
        };
        editor.initial = editor.values();
        editor
    }

    fn values(&self) -> RuleValues {
        (
            self.app_id_pattern.value().to_string(),
            self.title_pattern.value().to_string(),
            self.sink_ref.clone(),
            self.desc.value().to_string(),
            self.notify,
        )
    }

    /// Whether anything was changed since the editor opened
    pub(crate) fn is_modified(&self) -> bool {
        self.values() != self.initial
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
    /// The editor asks "discard changes?" (Esc was pressed with edits)
    pub confirm_discard: bool,
}

impl RulesScreen {
    pub(crate) fn new() -> Self {
        Self {
            mode: RulesMode::List,
            selected: 0,
            editor: RuleEditor::new(),
            editing_index: None,
            state: TableState::default(),
            confirm_discard: false,
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
        self.confirm_discard = false;
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

/// Rows of the rule form (one per field)
const FORM_ROWS: u16 = 5;

/// Render the rules screen
pub fn render_rules(frame: &mut Frame, area: Rect, ctx: &mut RulesRenderContext) {
    // Always render the list first as background
    render_list(frame, area, ctx.rules, ctx.sinks, ctx.screen_state);

    // Overlay modal on top if active
    match ctx.screen_state.mode {
        RulesMode::List => {}
        RulesMode::AddEdit => {
            render_editor(
                frame,
                area,
                ctx.sinks,
                ctx.screen_state,
                ctx.preview,
                ctx.daemon_running,
            );
            if ctx.screen_state.confirm_discard {
                render_text_dialog(
                    frame,
                    area,
                    "Discard changes?",
                    vec![Line::from("Your changes to this rule are not saved.")],
                    &[("y", "discard"), ("n", "keep editing")],
                );
            }
        }
        RulesMode::Delete => render_delete_confirmation(frame, area, ctx.rules, ctx.screen_state),
        RulesMode::SelectSink => {
            render_sink_selector(frame, area, ctx.sinks, &mut ctx.screen_state.editor);
        }
        RulesMode::Inspect => {
            render_inspect_popup(frame, area, ctx.rules, ctx.sinks, ctx.screen_state);
        }
    }
}

/// The description of the sink a rule's `sink_ref` names (or the reference itself)
fn sink_display<'a>(sinks: &'a [SinkConfig], sink_ref: &'a str) -> &'a str {
    sinks
        .iter()
        .find(|s| s.name == sink_ref || s.desc == sink_ref)
        .map_or(sink_ref, |s| s.desc.as_str())
}

/// Render the rules table (or the empty-state guidance)
fn render_list(
    frame: &mut Frame,
    area: Rect,
    rules: &[Rule],
    sinks: &[SinkConfig],
    screen_state: &mut RulesScreen,
) {
    let title = if rules.is_empty() {
        Line::from(Span::styled(" Rules ", theme::bold()))
    } else {
        Line::from(vec![
            Span::styled(" Rules ", theme::bold()),
            Span::styled(format!("· {} ", rules.len()), theme::dim()),
        ])
    };
    let block = rounded_block().title(title);

    if rules.is_empty() {
        let inner = block.inner(area);
        frame.render_widget(block, area);
        let lines = if sinks.is_empty() {
            vec![
                Line::from(Span::styled("No rules yet", theme::bold())),
                Line::from(""),
                Line::from(vec![
                    Span::raw("Add a sink first ("),
                    Span::styled("2", theme::key()),
                    Span::raw(" Sinks → "),
                    Span::styled("a", theme::key()),
                    Span::raw(")"),
                ]),
            ]
        } else {
            vec![
                Line::from(vec![
                    Span::styled("No rules yet", theme::bold()),
                    Span::raw(" — press "),
                    Span::styled("a", theme::key()),
                    Span::raw(" to send a window's audio to a sink"),
                ]),
                Line::from(""),
                Line::from(Span::styled(
                    "Windows no rule matches play on the fallback sink",
                    theme::dim(),
                )),
            ]
        };
        render_centered(frame, inner, lines);
        return;
    }

    screen_state.clamp_selection(rules.len());

    let widths = [
        Constraint::Length(3),
        Constraint::Fill(2),
        Constraint::Fill(2),
        Constraint::Fill(3),
        Constraint::Fill(2),
    ];
    let selection = u16::try_from(text_width(theme::SELECTED_SYMBOL)).unwrap_or(2);
    let cols = column_widths(&widths, area.width.saturating_sub(2), selection);
    let cut = |text: &str, col: usize| ellipsize(text, cols[col]).into_owned();

    let rows: Vec<Row> = rules
        .iter()
        .enumerate()
        .map(|(i, rule)| {
            let title = rule.title_pattern.as_ref().map_or_else(
                || Cell::from(Span::styled("any", theme::dim())),
                |t| Cell::from(cut(t, 2)),
            );
            Row::new(vec![
                Cell::from(Span::styled((i + 1).to_string(), theme::dim())),
                Cell::from(cut(&rule.app_id_pattern, 1)),
                title,
                Cell::from(Span::styled(
                    cut(sink_display(sinks, &rule.sink_ref), 3),
                    theme::accent(),
                )),
                Cell::from(Span::styled(
                    cut(rule.desc.as_deref().unwrap_or(""), 4),
                    theme::dim(),
                )),
            ])
        })
        .collect();

    let table = Table::new(rows, widths)
        .header(Row::new(vec!["#", "App ID", "Title", "Sink", "Description"]).style(theme::bold()))
        .block(block)
        .row_highlight_style(theme::selected())
        .highlight_symbol(theme::SELECTED_SYMBOL)
        .highlight_spacing(HighlightSpacing::Always);

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

/// How a rule's notify setting reads
const fn notify_text(notify: Option<bool>) -> &'static str {
    match notify {
        Some(true) => "on",
        Some(false) => "off",
        None => "default (from Settings)",
    }
}

/// Render the rule details dialog
fn render_inspect_popup(
    frame: &mut Frame,
    area: Rect,
    rules: &[Rule],
    sinks: &[SinkConfig],
    screen_state: &RulesScreen,
) {
    let Some(rule) = rules.get(screen_state.selected) else {
        return;
    };
    let (title, any_title) = rule
        .title_pattern
        .as_deref()
        .map_or(("any title", true), |t| (t, false));
    let (desc, no_desc) = rule.desc.as_deref().map_or(("none", true), |d| (d, false));
    let lines = detail_lines(&[
        ("App ID", &rule.app_id_pattern, false),
        ("Title", title, any_title),
        ("Sink", sink_display(sinks, &rule.sink_ref), false),
        ("Description", desc, no_desc),
        ("Notify", notify_text(rule.notify), rule.notify.is_none()),
    ]);
    render_text_dialog(frame, area, "Rule Details", lines, &[("Enter", "close")]);
}

/// Mark for a pattern field: valid, invalid or nothing while empty
fn validation_mark(value: &str, error: bool) -> Option<Span<'static>> {
    if value.is_empty() {
        None
    } else if error {
        Some(Span::styled("✗ invalid", theme::error()))
    } else {
        Some(Span::styled("✓", theme::success()))
    }
}

/// Render the add/edit form with the live preview below the fields
fn render_editor(
    frame: &mut Frame,
    area: Rect,
    sinks: &[SinkConfig],
    screen_state: &RulesScreen,
    preview: &PreviewView,
    daemon_running: bool,
) {
    let editor = &screen_state.editor;
    let title = if screen_state.editing_index.is_some() {
        "Edit Rule"
    } else {
        "Add Rule"
    };
    let focused = |field| editor.focused_field == field && !screen_state.confirm_discard;
    let hint = |field, text| focused(field).then(|| Span::styled(text, theme::dim()));

    let sink_value = if editor.sink_ref.is_empty() {
        Line::from(Span::styled(
            "none chosen",
            theme::dim().add_modifier(ratatui::style::Modifier::ITALIC),
        ))
    } else {
        Line::from(Span::styled(
            sink_display(sinks, &editor.sink_ref),
            theme::accent(),
        ))
    };
    let notify_value = Line::from(Span::styled(
        notify_text(editor.notify),
        if editor.notify.is_none() {
            theme::dim()
        } else {
            theme::text()
        },
    ));

    let fields = [
        FormField {
            label: "App ID",
            value: FieldValue::Text {
                input: &editor.app_id_pattern.input,
                placeholder: Cow::Borrowed("regex, e.g. ^firefox$"),
            },
            trailing: validation_mark(editor.app_id_pattern.value(), preview.app_error.is_some()),
            focused: focused(RuleField::AppId),
        },
        FormField {
            label: "Title",
            value: FieldValue::Text {
                input: &editor.title_pattern.input,
                placeholder: Cow::Borrowed("any title (optional regex)"),
            },
            trailing: validation_mark(editor.title_pattern.value(), preview.title_error.is_some()),
            focused: focused(RuleField::Title),
        },
        FormField {
            label: "Sink",
            value: FieldValue::Fixed(sink_value),
            trailing: hint(RuleField::Sink, "␣ choose"),
            focused: focused(RuleField::Sink),
        },
        FormField {
            label: "Description",
            value: FieldValue::Text {
                input: &editor.desc.input,
                placeholder: Cow::Borrowed("optional note"),
            },
            trailing: None,
            focused: focused(RuleField::Desc),
        },
        FormField {
            label: "Notify",
            value: FieldValue::Fixed(notify_value),
            trailing: hint(RuleField::Notify, "␣ cycle"),
            focused: focused(RuleField::Notify),
        },
    ];

    let preview_lines = live_preview_lines(
        editor.app_id_pattern.value().is_empty(),
        preview,
        daemon_running,
    );
    // Fields, a separator row, the preview
    let wanted = FORM_ROWS + 1 + u16::try_from(preview_lines.len()).unwrap_or(u16::MAX) + 2;
    let rect = modal_rect(area, MAX_DIALOG_WIDTH, wanted);
    let block =
        dialog_block(title).title_bottom(border_hints(&[("Enter", "save"), ("Esc", "cancel")]));
    let inner = open_dialog(frame, rect, block);

    let form_rows = inner.height.min(FORM_ROWS);
    render_form(
        frame,
        Rect {
            height: form_rows,
            ..inner
        },
        &fields,
    );

    // The preview gets what is left, if at least its heading and one line fit
    let remaining = inner.height - form_rows;
    if remaining >= 2 {
        let separator = Rect {
            y: inner.y + form_rows,
            height: 1,
            ..inner
        };
        let heading = format!(
            "Matching windows{}",
            if preview.total > 0 {
                format!(" ({})", preview.total)
            } else {
                String::new()
            }
        );
        let rule = "─".repeat(usize::from(inner.width).saturating_sub(text_width(&heading) + 3));
        frame.render_widget(
            Line::from(vec![
                Span::styled("─ ", theme::dim()),
                Span::styled(heading, theme::bold()),
                Span::styled(format!(" {rule}"), theme::dim()),
            ]),
            separator,
        );
        let list = Rect {
            y: separator.y + 1,
            height: remaining - 1,
            ..inner
        };
        frame.render_widget(Paragraph::new(preview_lines), list);
    }
}

/// The lines of the live preview: matching windows, or why there are none
fn live_preview_lines(
    app_pattern_empty: bool,
    preview: &PreviewView,
    daemon_running: bool,
) -> Vec<Line<'_>> {
    let line = |text: String, style| Line::from(Span::styled(text, style));

    let mut lines = Vec::new();
    if app_pattern_empty {
        // Users who want to match all windows should use `.*` explicitly.
        lines.push(line(
            "Type an app ID pattern to see which windows it matches".to_string(),
            theme::dim(),
        ));
    } else if preview.app_error.is_some() || preview.title_error.is_some() {
        if let Some(e) = &preview.app_error {
            lines.push(line(format!("Invalid app_id regex: {e}"), theme::error()));
        }
        if let Some(e) = &preview.title_error {
            lines.push(line(format!("Invalid title regex: {e}"), theme::error()));
        }
    } else if !daemon_running {
        lines.push(line(
            "Daemon not running: no window list to match against".to_string(),
            theme::dim(),
        ));
    } else if preview.total == 0 {
        lines.push(line("No matching windows".to_string(), theme::warning()));
    } else {
        lines.extend(build_preview_lines(&preview.shown));
        if preview.total > MAX_SHOWN {
            let remaining = preview.total - preview.shown.len();
            lines.push(line(format!("…and {remaining} more"), theme::dim()));
        }
    }
    lines
}

/// Render the target sink selector opened from the editor's Sink field
fn render_sink_selector(
    frame: &mut Frame,
    area: Rect,
    sinks: &[SinkConfig],
    editor: &mut RuleEditor,
) {
    let selection = text_width(theme::SELECTED_SYMBOL);
    let widest = sinks
        .iter()
        .map(|s| text_width(&s.desc) + 2 + text_width(&s.name))
        .max()
        .unwrap_or(0);
    let width = u16::try_from(widest + selection + 4)
        .unwrap_or(u16::MAX)
        .clamp(30, MAX_DIALOG_WIDTH);
    let height = u16::try_from(sinks.len().max(1) + 2).unwrap_or(u16::MAX);
    let rect = modal_rect(area, width, height);
    let content_width = usize::from(rect.width.saturating_sub(4)).saturating_sub(selection);

    let entries: Vec<(&str, &str)> = sinks
        .iter()
        .map(|s| (s.desc.as_str(), s.name.as_str()))
        .collect();
    let items: Vec<ListItem> = picker_lines(&entries, content_width)
        .into_iter()
        .map(ListItem::new)
        .collect();

    let block = dialog_block("Choose a Sink")
        .title_bottom(border_hints(&[("Enter", "use"), ("Esc", "back")]));
    let list = List::new(items)
        .block(block)
        .highlight_style(theme::selected())
        .highlight_symbol(theme::SELECTED_SYMBOL)
        .highlight_spacing(HighlightSpacing::Always)
        .scroll_padding(1);

    // Sync state (the sink list may have shrunk since the index was chosen)
    editor.sink_dropdown_index = editor
        .sink_dropdown_index
        .min(sinks.len().saturating_sub(1));
    editor
        .sink_selector_state
        .select((!sinks.is_empty()).then_some(editor.sink_dropdown_index));
    frame.render_widget(Clear, rect);
    frame.render_stateful_widget(list, rect, &mut editor.sink_selector_state);
    render_scrollbar(
        frame,
        rect,
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
    let (title, any_title) = rule
        .title_pattern
        .as_deref()
        .map_or(("any title", true), |t| (t, false));
    let mut lines = vec![
        Line::from(Span::styled("Delete this rule?", theme::bold())),
        Line::from(""),
    ];
    lines.extend(detail_lines(&[
        ("App ID", &rule.app_id_pattern, false),
        ("Title", title, any_title),
        ("Sink", &rule.sink_ref, false),
    ]));
    render_text_dialog(frame, area, "Delete Rule", lines, &[
        ("Enter", "delete"),
        ("Esc", "cancel"),
    ]);
}
