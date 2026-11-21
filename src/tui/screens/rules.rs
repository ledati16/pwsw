//! Rules screen - Manage window matching rules

use std::borrow::Cow;

use ratatui::Frame;
use ratatui::layout::{Constraint, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Cell, ListItem, ListState, Paragraph, Row};
use tui_input::Input;

use crate::config::{Rule, SinkConfig, compile_rule_regex};
use crate::tui::preview::{MAX_SHOWN, PreviewView, build_preview_lines};
use crate::tui::screens::list::{
    FormEditor, ListMode, ListScreen, open_editor_dialog, optional, render_delete_dialog,
    render_discard_dialog, render_empty, render_table, table_columns,
};
use crate::tui::theme;
use crate::tui::widgets::{
    FieldValue, FormField, MAX_DIALOG_WIDTH, detail_lines, ellipsize, pick_list_rect, picker_lines,
    render_form, render_pick_list, render_text_dialog, rounded_block, text_width,
};

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
}

/// The rule add/edit form
#[derive(Debug, Clone, Default)]
pub struct RuleEditor {
    pub app_id_pattern: Input,
    pub title_pattern: Input,
    pub sink_ref: String,
    pub desc: Input,
    pub notify: Option<bool>,
    pub focused_field: RuleField,
    /// Sink highlighted in the target sink selector
    pub sink_dropdown_index: usize,
    /// Scroll state of the target sink selector
    pub sink_selector_state: ListState,
    /// App ID, title, sink, description and notify when the editor opened
    initial: RuleValues,
}

/// The editable values of a rule, to tell whether the editor changed any
type RuleValues = (String, String, String, String, Option<bool>);

impl RuleEditor {
    fn values(&self) -> RuleValues {
        (
            self.app_id_pattern.value().to_string(),
            self.title_pattern.value().to_string(),
            self.sink_ref.clone(),
            self.desc.value().to_string(),
            self.notify,
        )
    }

    /// The rule the form describes, its patterns compiled the way config
    /// loading compiles them
    ///
    /// # Errors
    /// When the app ID pattern or the target sink is missing, or a pattern is
    /// not a valid regex
    pub(crate) fn to_rule(&self) -> Result<Rule, String> {
        let app_id = self.app_id_pattern.value();
        if app_id.is_empty() {
            return Err("App ID pattern is required".to_string());
        }
        if self.sink_ref.is_empty() {
            return Err("Target sink is required".to_string());
        }
        let app_id_regex =
            compile_rule_regex(app_id).map_err(|e| format!("Invalid app_id regex: {e}"))?;
        let title_pattern = optional(&self.title_pattern);
        let title_regex = title_pattern
            .as_deref()
            .map(compile_rule_regex)
            .transpose()
            .map_err(|e| format!("Invalid title regex: {e}"))?;
        Ok(Rule {
            app_id_regex,
            title_regex,
            sink_ref: self.sink_ref.clone(),
            desc: optional(&self.desc),
            notify: self.notify,
            app_id_pattern: app_id.to_string(),
            title_pattern,
        })
    }

    /// Space on the Notify field: default → on → off → default
    pub(crate) const fn cycle_notify(&mut self) {
        self.notify = match self.notify {
            None => Some(true),
            Some(true) => Some(false),
            Some(false) => None,
        };
    }
}

impl FormEditor for RuleEditor {
    type Item = Rule;

    fn blank() -> Self {
        Self::default()
    }

    fn from_item(rule: &Rule) -> Self {
        let mut editor = Self {
            app_id_pattern: Input::new(rule.app_id_pattern.clone()),
            title_pattern: Input::new(rule.title_pattern.clone().unwrap_or_default()),
            sink_ref: rule.sink_ref.clone(),
            desc: Input::new(rule.desc.clone().unwrap_or_default()),
            notify: rule.notify,
            ..Self::default()
        };
        editor.initial = editor.values();
        editor
    }

    fn is_modified(&self) -> bool {
        self.values() != self.initial
    }

    fn next_field(&mut self) {
        self.focused_field = self.focused_field.next();
    }

    fn prev_field(&mut self) {
        self.focused_field = self.focused_field.prev();
    }

    fn text_focused(&self) -> bool {
        matches!(
            self.focused_field,
            RuleField::AppId | RuleField::Title | RuleField::Desc
        )
    }

    fn focused_input_mut(&mut self) -> Option<&mut Input> {
        match self.focused_field {
            RuleField::AppId => Some(&mut self.app_id_pattern),
            RuleField::Title => Some(&mut self.title_pattern),
            RuleField::Desc => Some(&mut self.desc),
            RuleField::Sink | RuleField::Notify => None,
        }
    }
}

/// Rules screen state
pub type RulesScreen = ListScreen<RuleEditor>;

impl RulesScreen {
    /// Open the target sink selector on the sink the rule currently targets.
    pub(crate) fn open_sink_selector(&mut self, sinks: &[SinkConfig]) {
        let current = &self.editor.sink_ref;
        self.editor.sink_dropdown_index = sinks
            .iter()
            .position(|s| s.desc == *current || s.name == *current)
            .unwrap_or(0);
        self.mode = ListMode::SelectSink;
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
        ListMode::List => {}
        ListMode::AddEdit => {
            render_editor(
                frame,
                area,
                ctx.sinks,
                ctx.screen_state,
                ctx.preview,
                ctx.daemon_running,
            );
            if ctx.screen_state.confirm_discard {
                render_discard_dialog(frame, area, "rule");
            }
        }
        ListMode::Delete => {
            if let Some(rule) = ctx.rules.get(ctx.screen_state.selected) {
                let (title, any_title) = title_text(rule);
                render_delete_dialog(frame, area, "Rule", &[
                    ("App ID", &rule.app_id_pattern, false),
                    ("Title", title, any_title),
                    ("Sink", &rule.sink_ref, false),
                ]);
            }
        }
        ListMode::SelectSink => {
            render_sink_selector(frame, area, ctx.sinks, &mut ctx.screen_state.editor);
        }
        ListMode::Inspect => {
            render_inspect_popup(frame, area, ctx.rules, ctx.sinks, ctx.screen_state);
        }
    }
}

/// A rule's title pattern, or "any title" (a placeholder: `true`)
fn title_text(rule: &Rule) -> (&str, bool) {
    rule.title_pattern
        .as_deref()
        .map_or(("any title", true), |t| (t, false))
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
        render_empty(frame, area, block, lines);
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
    let cols = table_columns(&widths, area);
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

    render_table(
        frame,
        area,
        block,
        vec!["#", "App ID", "Title", "Sink", "Description"],
        rows,
        &widths,
        screen_state,
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
    let (title, any_title) = title_text(rule);
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
                input: &editor.app_id_pattern,
                placeholder: Cow::Borrowed("regex, e.g. ^firefox$"),
            },
            trailing: validation_mark(editor.app_id_pattern.value(), preview.app_error.is_some()),
            focused: focused(RuleField::AppId),
        },
        FormField {
            label: "Title",
            value: FieldValue::Text {
                input: &editor.title_pattern,
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
                input: &editor.desc,
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
    let title = screen_state.editor_title("Rule");
    let inner = open_editor_dialog(frame, area, &title, MAX_DIALOG_WIDTH, wanted);

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
    let entries: Vec<(&str, &str)> = sinks
        .iter()
        .map(|s| (s.desc.as_str(), s.name.as_str()))
        .collect();
    let widest = entries
        .iter()
        .map(|(d, n)| text_width(d) + 2 + text_width(n))
        .max()
        .unwrap_or(0);
    let (rect, content_width) = pick_list_rect(area, widest, sinks.len().max(1), 30);
    let items: Vec<ListItem> = picker_lines(&entries, content_width)
        .into_iter()
        .map(ListItem::new)
        .collect();

    // Sync state (the sink list may have shrunk since the index was chosen)
    editor.sink_dropdown_index = editor
        .sink_dropdown_index
        .min(sinks.len().saturating_sub(1));
    editor
        .sink_selector_state
        .select((!sinks.is_empty()).then_some(editor.sink_dropdown_index));
    render_pick_list(
        frame,
        rect,
        "Choose a Sink",
        &[("Enter", "use"), ("Esc", "back")],
        items,
        &mut editor.sink_selector_state,
    );
}
