//! Rules screen - Manage window matching rules

use std::borrow::Cow;

use ratatui::Frame;
use ratatui::layout::{Constraint, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Cell, ListItem, ListState, Paragraph, Row};
use tui_input::Input;

use crate::config::{Rule, SinkConfig, compile_rule_regex, sink_index_in};
use crate::ipc::WindowInfo;
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
    /// The ordered target sinks (Space opens the sink picker)
    Sinks,
    Desc,
    /// Notify toggle (Space cycles it)
    Notify,
}

impl RuleField {
    pub(crate) const fn next(self) -> Self {
        match self {
            Self::AppId => Self::Title,
            Self::Title => Self::Sinks,
            Self::Sinks => Self::Desc,
            Self::Desc | Self::Notify => Self::Notify,
        }
    }

    pub(crate) const fn prev(self) -> Self {
        match self {
            Self::AppId | Self::Title => Self::AppId,
            Self::Sinks => Self::Title,
            Self::Desc => Self::Sinks,
            Self::Notify => Self::Desc,
        }
    }
}

/// The rule add/edit form
#[derive(Debug, Clone, Default)]
pub struct RuleEditor {
    pub app_id_pattern: Input,
    pub title_pattern: Input,
    /// The rule's sinks in priority order: the first choice, then the fallbacks
    /// (sink references as in the config; empty for a new rule)
    pub sink_refs: Vec<String>,
    /// The rule's source references, carried through edits unchanged (the form does
    /// not edit them yet; empty for a new rule)
    pub source_refs: Vec<String>,
    pub desc: Input,
    pub notify: Option<bool>,
    pub focused_field: RuleField,
    /// The sinks chosen in the open sink picker, in priority order (indices into
    /// the configured sinks); Enter in the picker applies them to `sink_refs`
    pub picked: Vec<usize>,
    /// Row highlighted in the sink picker (see [`picker_rows`])
    pub sink_dropdown_index: usize,
    /// Scroll state of the sink picker
    pub sink_selector_state: ListState,
    /// Title of the window the form was opened from (shown as a hint to copy from)
    pub window_title: Option<String>,
    /// App ID, title, sinks, description and notify when the editor opened
    initial: RuleValues,
}

/// The editable values of a rule, to tell whether the editor changed any
type RuleValues = (String, String, Vec<String>, String, Option<bool>);

/// The rows of the sink picker, as indices into the `sink_count` configured
/// sinks: the picked sinks in priority order, then the others in config order
#[must_use]
pub(crate) fn picker_rows(picked: &[usize], sink_count: usize) -> Vec<usize> {
    picked
        .iter()
        .copied()
        .chain((0..sink_count).filter(|i| !picked.contains(i)))
        .collect()
}

impl RuleEditor {
    fn values(&self) -> RuleValues {
        (
            self.app_id_pattern.value().to_string(),
            self.title_pattern.value().to_string(),
            self.sink_refs.clone(),
            self.desc.value().to_string(),
            self.notify,
        )
    }

    /// The rule the form describes, its patterns compiled the way config
    /// loading compiles them
    ///
    /// # Errors
    /// When the app ID pattern or the target sinks are missing, or a pattern is
    /// not a valid regex
    pub(crate) fn to_rule(&self) -> Result<Rule, String> {
        let app_id = self.app_id_pattern.value();
        if app_id.is_empty() {
            return Err("App ID pattern is required".to_string());
        }
        let Some((first, fallbacks)) = self.sink_refs.split_first() else {
            return Err("Choose at least one sink (Space on Sinks)".to_string());
        };
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
            sink_ref: first.clone(),
            fallback_refs: fallbacks.to_vec(),
            source_refs: self.source_refs.clone(),
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

    /// The configured sinks `sink_refs` names, in order (unknown references left out)
    fn resolved_refs(&self, sinks: &[SinkConfig]) -> Vec<usize> {
        let mut out = Vec::with_capacity(self.sink_refs.len());
        for index in self
            .sink_refs
            .iter()
            .filter_map(|r| sink_index_in(sinks, r))
        {
            if !out.contains(&index) {
                out.push(index);
            }
        }
        out
    }

    /// Fill the picker from the form's sinks, highlighting the first row
    fn open_picker(&mut self, sinks: &[SinkConfig]) {
        self.picked = self.resolved_refs(sinks);
        self.sink_dropdown_index = 0;
    }

    /// ↑/↓ in the picker of `sink_count` sinks
    pub(crate) const fn picker_move(&mut self, down: bool, sink_count: usize) {
        if down {
            if self.sink_dropdown_index + 1 < sink_count {
                self.sink_dropdown_index += 1;
            }
        } else {
            self.sink_dropdown_index = self.sink_dropdown_index.saturating_sub(1);
        }
    }

    /// Space in the picker: add the highlighted sink at the end of the order, or
    /// remove it (the later ones move up: removing the first choice promotes the
    /// second). The highlight follows the sink to its new row.
    pub(crate) fn picker_toggle(&mut self, sink_count: usize) {
        let rows = picker_rows(&self.picked, sink_count);
        let Some(&sink) = rows.get(self.sink_dropdown_index) else {
            return;
        };
        if let Some(pos) = self.picked.iter().position(|&p| p == sink) {
            self.picked.remove(pos);
        } else {
            self.picked.push(sink);
        }
        self.sink_dropdown_index = picker_rows(&self.picked, sink_count)
            .iter()
            .position(|&r| r == sink)
            .unwrap_or(0);
    }

    /// Shift+↑/↓ in the picker: move the highlighted chosen sink one place
    /// earlier or later in the order (does nothing on a sink not chosen)
    pub(crate) fn picker_reorder(&mut self, later: bool) {
        let row = self.sink_dropdown_index;
        if row >= self.picked.len() {
            return;
        }
        let target = if later {
            Some(row + 1)
        } else {
            row.checked_sub(1)
        };
        if let Some(target) = target.filter(|&t| t < self.picked.len()) {
            self.picked.swap(row, target);
            self.sink_dropdown_index = target;
        }
    }

    /// Enter in the picker: make the chosen sinks the form's (stored by
    /// description). With none chosen, the highlighted sink is used.
    ///
    /// # Errors
    /// When there is no sink to choose
    pub(crate) fn apply_picks(&mut self, sinks: &[SinkConfig]) -> Result<(), &'static str> {
        if self.picked.is_empty() {
            let rows = picker_rows(&self.picked, sinks.len());
            let Some(&sink) = rows.get(self.sink_dropdown_index) else {
                return Err("No sink to choose: add one on the Sinks screen");
            };
            self.picked.push(sink);
        }
        // The same sinks in the same order: keep the references as they are written
        if self.picked != self.resolved_refs(sinks) {
            self.sink_refs = self
                .picked
                .iter()
                .filter_map(|&i| sinks.get(i))
                .map(|s| s.desc.clone())
                .collect();
        }
        Ok(())
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
            sink_refs: rule.sink_refs().map(str::to_string).collect(),
            source_refs: rule.source_refs.clone(),
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
            RuleField::Sinks | RuleField::Notify => None,
        }
    }
}

/// Rules screen state
pub type RulesScreen = ListScreen<RuleEditor>;

impl RulesScreen {
    /// Open the add form pre-filled for `window`: its app ID escaped and anchored,
    /// any title (its current title shown as a hint), the Sinks field focused and
    /// the sink chosen only when there is exactly one.
    pub(crate) fn start_add_for_window(&mut self, window: &WindowInfo, sinks: &[SinkConfig]) {
        self.start_add();
        let editor = &mut self.editor;
        editor.app_id_pattern = Input::new(format!("^{}$", regex::escape(&window.app_id)));
        if let [only] = sinks {
            editor.sink_refs = vec![only.desc.clone()];
        }
        editor.focused_field = RuleField::Sinks;
        editor.window_title = Some(window.title.clone());
        // Nothing typed yet: Esc closes without asking
        editor.initial = editor.values();
    }

    /// Open rule `index` of `rules` for editing, showing `window`'s title as a hint
    pub(crate) fn start_edit_for_window(
        &mut self,
        index: usize,
        rules: &[Rule],
        window: &WindowInfo,
    ) {
        self.selected = index;
        self.start_edit(rules);
        self.editor.window_title = Some(window.title.clone());
    }

    /// Open the sink picker with the form's sinks chosen.
    pub(crate) fn open_sink_selector(&mut self, sinks: &[SinkConfig]) {
        self.editor.open_picker(sinks);
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
                let targets = sink_detail_values(ctx.sinks, rule);
                let mut rows = vec![
                    ("App ID", rule.app_id_pattern.as_str(), false),
                    ("Title", title, any_title),
                ];
                rows.extend(sink_detail_rows(rule, &targets));
                render_delete_dialog(frame, area, "Rule", &rows);
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

/// The description of the sink a rule's sink reference names (or the reference itself)
fn sink_display<'a>(sinks: &'a [SinkConfig], sink_ref: &'a str) -> &'a str {
    sink_index_in(sinks, sink_ref).map_or(sink_ref, |i| sinks[i].desc.as_str())
}

/// Separator between a rule's sinks, first choice first
const SINK_ARROW: &str = " → ";

/// A rule's sinks in priority order: `BT Headphones → Headphones`
fn sinks_text<'a>(sinks: &[SinkConfig], refs: impl IntoIterator<Item = &'a str>) -> String {
    refs.into_iter()
        .map(|r| sink_display(sinks, r))
        .collect::<Vec<_>>()
        .join(SINK_ARROW)
}

/// A rule's sinks for a details dialog, one per row so none is cut: the first
/// choice, then `→ fallback` rows
fn sink_detail_values(sinks: &[SinkConfig], rule: &Rule) -> Vec<String> {
    rule.sink_refs()
        .enumerate()
        .map(|(i, r)| {
            let desc = sink_display(sinks, r);
            if i == 0 {
                desc.to_string()
            } else {
                format!("→ {desc}")
            }
        })
        .collect()
}

/// The detail rows of `values` (from [`sink_detail_values`]), labelled once
fn sink_detail_rows<'a>(
    rule: &Rule,
    values: &'a [String],
) -> impl Iterator<Item = (&'static str, &'a str, bool)> {
    let label = sinks_label(rule);
    values
        .iter()
        .enumerate()
        .map(move |(i, v)| (if i == 0 { label } else { "" }, v.as_str(), false))
}

/// "Sink" for a rule with one sink, "Sinks" for one with fallbacks
const fn sinks_label(rule: &Rule) -> &'static str {
    if rule.fallback_refs.is_empty() {
        "Sink"
    } else {
        "Sinks"
    }
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
                Line::from(vec![
                    Span::raw("or pick a window in Dashboard → "),
                    Span::styled("w", theme::key()),
                    Span::raw(" → "),
                    Span::styled("n", theme::key()),
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
                    cut(&sinks_text(sinks, rule.sink_refs()), 3),
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
        vec!["#", "App ID", "Title", "Sinks", "Description"],
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
    let targets = sink_detail_values(sinks, rule);
    let mut rows = vec![
        ("App ID", rule.app_id_pattern.as_str(), false),
        ("Title", title, any_title),
    ];
    rows.extend(sink_detail_rows(rule, &targets));
    rows.extend([
        ("Description", desc, no_desc),
        ("Notify", notify_text(rule.notify), rule.notify.is_none()),
    ]);
    let lines = detail_lines(&rows);
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

    let sink_value = if editor.sink_refs.is_empty() {
        Line::from(Span::styled(
            "none chosen",
            theme::dim().add_modifier(ratatui::style::Modifier::ITALIC),
        ))
    } else {
        Line::from(Span::styled(
            sinks_text(sinks, editor.sink_refs.iter().map(String::as_str)),
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
            label: "Sinks",
            value: FieldValue::Fixed(sink_value),
            trailing: hint(RuleField::Sinks, "␣ choose"),
            focused: focused(RuleField::Sinks),
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
    let hint_rows = u16::from(editor.window_title.is_some());
    // Fields, the title hint, a separator row, the preview
    let wanted =
        FORM_ROWS + hint_rows + 1 + u16::try_from(preview_lines.len()).unwrap_or(u16::MAX) + 2;
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
    let mut used = form_rows;

    // The window the form was opened from: its title, to copy part of it
    if let Some(window_title) = &editor.window_title
        && inner.height > used
    {
        const LABEL: &str = "Current title: ";
        let room = usize::from(inner.width).saturating_sub(text_width(LABEL));
        let value = if window_title.is_empty() {
            Span::styled("(empty)", theme::dim())
        } else {
            Span::raw(ellipsize(window_title, room))
        };
        frame.render_widget(
            Line::from(vec![Span::styled(LABEL, theme::dim()), value]),
            Rect {
                y: inner.y + used,
                height: 1,
                ..inner
            },
        );
        used += 1;
    }

    // The preview gets what is left, if at least its heading and one line fit
    let remaining = inner.height - used;
    if remaining >= 2 {
        let separator = Rect {
            y: inner.y + used,
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

/// Render the sink picker opened from the editor's Sinks field: the chosen
/// sinks numbered in priority order, then the others
fn render_sink_selector(
    frame: &mut Frame,
    area: Rect,
    sinks: &[SinkConfig],
    editor: &mut RuleEditor,
) {
    // Sync state (the sink list may have shrunk since the picker opened)
    editor.picked.retain(|&i| i < sinks.len());
    editor.sink_dropdown_index = editor
        .sink_dropdown_index
        .min(sinks.len().saturating_sub(1));

    let rows = picker_rows(&editor.picked, sinks.len());
    let entries: Vec<(&str, &str)> = rows
        .iter()
        .map(|&i| (sinks[i].desc.as_str(), sinks[i].name.as_str()))
        .collect();
    // "1 " before the chosen sinks, as wide as the largest number
    let number_width = editor.picked.len().to_string().len() + 1;
    let widest = entries
        .iter()
        .map(|(d, n)| text_width(d) + 2 + text_width(n))
        .max()
        .unwrap_or(0)
        + number_width;
    let (rect, content_width) = pick_list_rect(area, widest, rows.len().max(1), 46);
    let items: Vec<ListItem> = picker_lines(&entries, content_width.saturating_sub(number_width))
        .into_iter()
        .enumerate()
        .map(|(row, mut line)| {
            let mark = if row < editor.picked.len() {
                Span::styled(format!("{:<number_width$}", row + 1), theme::accent())
            } else {
                Span::raw(" ".repeat(number_width))
            };
            line.spans.insert(0, mark);
            ListItem::new(line)
        })
        .collect();

    editor
        .sink_selector_state
        .select((!sinks.is_empty()).then_some(editor.sink_dropdown_index));
    render_pick_list(
        frame,
        rect,
        "Choose Sinks (in order)",
        &[("Space", "pick"), ("Shift+↑/↓", "order"), ("Enter", "done")],
        items,
        &mut editor.sink_selector_state,
    );
}

#[cfg(test)]
mod editor_tests {
    use pretty_assertions::assert_eq;

    use super::*;
    use crate::test_utils::fixtures::{
        make_rule, make_rule_with_fallbacks, make_sink, with_sources,
    };

    fn sinks() -> Vec<SinkConfig> {
        vec![
            make_sink("spk", "Speakers", true),
            make_sink("hp", "Headphones", false),
            make_sink("bt", "BT Headphones", false),
        ]
    }

    /// An editor on `rule` with its picker open
    fn picker_on(rule: &Rule) -> RuleEditor {
        let mut editor = RuleEditor::from_item(rule);
        editor.open_picker(&sinks());
        editor
    }

    /// The form does not edit sources yet: an edit must keep them as they were
    #[test]
    fn test_edit_keeps_the_rules_sources() {
        let rule = with_sources(make_rule("^discord$", None, "Speakers"), &["Headset Mic"]);
        let mut editor = RuleEditor::from_item(&rule);
        editor.sink_refs = vec!["Headphones".to_string()];
        let edited = editor.to_rule().expect("valid rule");
        assert_eq!(edited.source_refs, ["Headset Mic"]);
        assert_eq!(edited.sink_ref, "Headphones");
        assert_eq!(
            RuleEditor::from_item(&rule).to_rule().expect("valid rule"),
            rule
        );
        assert_eq!(RuleEditor::blank().source_refs, Vec::<String>::new());
    }

    #[test]
    fn test_round_trip_with_one_sink() {
        let rule = make_rule("^mpv$", None, "Speakers");
        let editor = RuleEditor::from_item(&rule);
        assert_eq!(editor.sink_refs, ["Speakers"]);
        let back = editor.to_rule().expect("valid rule");
        assert_eq!(back, rule);
        assert!(!editor.is_modified());
    }

    #[test]
    fn test_round_trip_with_three_sinks() {
        let rule =
            make_rule_with_fallbacks("^discord$", None, &["BT Headphones", "Headphones", "spk"]);
        let mut editor = picker_on(&rule);
        assert_eq!(editor.picked, [2, 1, 0]);
        // Applying the unchanged picks keeps the references as written
        editor.apply_picks(&sinks()).expect("picks");
        assert!(!editor.is_modified());
        editor.desc = Input::new("Voice chat".to_string());

        let edited = editor.to_rule().expect("valid rule");
        assert_eq!(edited.sink_ref, "BT Headphones");
        assert_eq!(edited.fallback_refs, ["Headphones", "spk"]);
    }

    #[test]
    fn test_reorder_moves_the_highlighted_sink() {
        let rule = make_rule_with_fallbacks("x", None, &["BT Headphones", "Headphones"]);
        let mut editor = picker_on(&rule);
        // Row 1 is Headphones: move it first
        editor.picker_move(true, 3);
        editor.picker_reorder(false);
        assert_eq!(editor.picked, [1, 2]);
        assert_eq!(
            editor.sink_dropdown_index, 0,
            "the highlight follows the sink"
        );
        // Already first: nothing happens; an unchosen sink cannot be reordered
        editor.picker_reorder(false);
        editor.sink_dropdown_index = 2;
        editor.picker_reorder(false);
        assert_eq!(editor.picked, [1, 2]);

        editor.apply_picks(&sinks()).expect("picks");
        assert_eq!(editor.sink_refs, ["Headphones", "BT Headphones"]);
        assert!(editor.is_modified());
    }

    #[test]
    fn test_removing_the_first_choice_promotes_the_next() {
        let rule =
            make_rule_with_fallbacks("x", None, &["BT Headphones", "Headphones", "Speakers"]);
        let mut editor = picker_on(&rule);
        editor.picker_toggle(3); // row 0: BT Headphones
        assert_eq!(editor.picked, [1, 0]);
        assert_eq!(
            picker_rows(&editor.picked, 3)[editor.sink_dropdown_index],
            2,
            "the highlight follows the removed sink"
        );
        editor.apply_picks(&sinks()).expect("picks");
        let rule = editor.to_rule().expect("valid rule");
        assert_eq!(rule.sink_ref, "Headphones");
        assert_eq!(rule.fallback_refs, ["Speakers"]);
    }

    #[test]
    fn test_picking_appends_in_order_of_selection() {
        let mut editor = RuleEditor::blank();
        editor.app_id_pattern = Input::new("^mpv$".to_string());
        editor.open_picker(&sinks());
        editor.sink_dropdown_index = 2;
        editor.picker_toggle(3); // BT Headphones, now row 0
        assert_eq!(editor.sink_dropdown_index, 0);
        editor.sink_dropdown_index = 1; // Speakers
        editor.picker_toggle(3);
        assert_eq!(editor.picked, [2, 0]);
        assert_eq!(picker_rows(&editor.picked, 3), [2, 0, 1]);
        editor.apply_picks(&sinks()).expect("picks");
        assert_eq!(editor.sink_refs, ["BT Headphones", "Speakers"]);
    }

    #[test]
    fn test_enter_with_nothing_chosen_uses_the_highlighted_sink() {
        let mut editor = RuleEditor::blank();
        editor.open_picker(&sinks());
        editor.picker_move(true, 3);
        editor.apply_picks(&sinks()).expect("picks");
        assert_eq!(editor.sink_refs, ["Headphones"]);
        assert!(editor.apply_picks(&[]).is_ok(), "keeps the chosen sink");
        let mut empty = RuleEditor::blank();
        assert!(empty.apply_picks(&[]).is_err(), "no sink to choose");
    }

    #[test]
    fn test_a_rule_needs_a_sink() {
        let mut editor = RuleEditor::blank();
        editor.app_id_pattern = Input::new("^mpv$".to_string());
        assert!(editor.to_rule().is_err());
        editor.sink_refs = vec!["Speakers".to_string()];
        let rule = editor.to_rule().expect("valid rule");
        assert_eq!(rule.fallback_refs, Vec::<String>::new());
    }

    #[test]
    fn test_sinks_text_joins_in_priority_order() {
        let rule = make_rule_with_fallbacks("x", None, &["bt", "Headphones", "1"]);
        assert_eq!(
            sinks_text(&sinks(), rule.sink_refs()),
            "BT Headphones → Headphones → Speakers"
        );
    }
}
