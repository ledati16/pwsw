//! Rules screen - Manage window matching rules

use std::borrow::Cow;

use ratatui::Frame;
use ratatui::layout::{Constraint, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Cell, ListItem, ListState, Paragraph, Row};
use tui_input::Input;

use crate::config::{Endpoint, Rule, SinkConfig, compile_rule_regex, sink_index_in};
use crate::ipc::WindowInfo;
use crate::tui::preview::{MAX_SHOWN, PreviewView, build_preview_lines};
use crate::tui::screens::list::{
    FormEditor, ListMode, ListScreen, open_editor_dialog, optional, render_delete_dialog,
    render_discard_dialog, render_empty, render_table, table_columns,
};
use crate::tui::screens::sinks::plural;
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
    /// The monitor condition (regex on connector names; optional)
    Output,
    /// The ordered target sinks (Space opens the sink picker)
    Sinks,
    /// The ordered target sources (Space opens the mic picker; optional)
    Mics,
    Desc,
    /// Notify toggle (Space cycles it)
    Notify,
}

impl RuleField {
    pub(crate) const fn next(self) -> Self {
        match self {
            Self::AppId => Self::Title,
            Self::Title => Self::Output,
            Self::Output => Self::Sinks,
            Self::Sinks => Self::Mics,
            Self::Mics => Self::Desc,
            Self::Desc | Self::Notify => Self::Notify,
        }
    }

    pub(crate) const fn prev(self) -> Self {
        match self {
            Self::AppId | Self::Title => Self::AppId,
            Self::Output => Self::Title,
            Self::Sinks => Self::Output,
            Self::Mics => Self::Sinks,
            Self::Desc => Self::Mics,
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
    /// The rule's sources (mics) in priority order (source references as in the
    /// config; empty: the rule leaves the microphone alone)
    pub source_refs: Vec<String>,
    /// The rule's `output` pattern (empty: any monitor)
    pub output_pattern: Input,
    pub desc: Input,
    pub notify: Option<bool>,
    pub focused_field: RuleField,
    /// Whether the open picker chooses sources (mics) rather than sinks
    pub picking_sources: bool,
    /// The entries chosen in the open picker, in priority order (indices into the
    /// configured sinks or sources); Enter in the picker applies them to
    /// `sink_refs` or `source_refs`
    pub picked: Vec<usize>,
    /// Row highlighted in the picker (see [`picker_rows`])
    pub sink_dropdown_index: usize,
    /// Scroll state of the picker
    pub sink_selector_state: ListState,
    /// Title of the window the form was opened from (shown as a hint to copy from)
    pub window_title: Option<String>,
    /// The editable values when the editor opened
    initial: RuleValues,
}

/// The editable values of a rule, to tell whether the editor changed any
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct RuleValues {
    app_id: String,
    title: String,
    output: String,
    sinks: Vec<String>,
    sources: Vec<String>,
    desc: String,
    notify: Option<bool>,
}

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
    /// Which list the open picker chooses from
    pub(crate) const fn picker_kind(&self) -> Endpoint {
        if self.picking_sources {
            Endpoint::Source
        } else {
            Endpoint::Sink
        }
    }

    fn values(&self) -> RuleValues {
        RuleValues {
            app_id: self.app_id_pattern.value().to_string(),
            title: self.title_pattern.value().to_string(),
            output: self.output_pattern.value().to_string(),
            sinks: self.sink_refs.clone(),
            sources: self.source_refs.clone(),
            desc: self.desc.value().to_string(),
            notify: self.notify,
        }
    }

    /// The rule the form describes, its patterns compiled the way config
    /// loading compiles them
    ///
    /// # Errors
    /// When the app ID pattern (required unless the rule has an `output`) or the
    /// target sinks are missing, or a pattern is not a valid regex
    pub(crate) fn to_rule(&self) -> Result<Rule, String> {
        let app_id = self.app_id_pattern.value();
        let output_pattern = optional(&self.output_pattern);
        if app_id.is_empty() && output_pattern.is_none() {
            return Err(APP_ID_REQUIRED.to_string());
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
        let output_regex = output_pattern
            .as_deref()
            .map(compile_rule_regex)
            .transpose()
            .map_err(|e| format!("Invalid output regex: {e}"))?;
        Ok(Rule {
            app_id_regex,
            title_regex,
            output_regex,
            sink_ref: first.clone(),
            fallback_refs: fallbacks.to_vec(),
            source_refs: self.source_refs.clone(),
            desc: optional(&self.desc),
            notify: self.notify,
            app_id_pattern: app_id.to_string(),
            title_pattern,
            output_pattern,
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

    /// The form's references to `kind` entries (`sink_refs` or `source_refs`)
    const fn refs(&self, kind: Endpoint) -> &Vec<String> {
        match kind {
            Endpoint::Sink => &self.sink_refs,
            Endpoint::Source => &self.source_refs,
        }
    }

    /// The configured `entries` (sinks or sources) the form's `kind` references
    /// name, in order (unknown references left out)
    fn resolved_refs(&self, kind: Endpoint, entries: &[SinkConfig]) -> Vec<usize> {
        let refs = self.refs(kind);
        let mut out = Vec::with_capacity(refs.len());
        for index in refs.iter().filter_map(|r| sink_index_in(entries, r)) {
            if !out.contains(&index) {
                out.push(index);
            }
        }
        out
    }

    /// Fill the picker of `kind` from the form's references to `entries`,
    /// highlighting the first row
    fn open_picker(&mut self, kind: Endpoint, entries: &[SinkConfig]) {
        self.picking_sources = kind == Endpoint::Source;
        self.picked = self.resolved_refs(kind, entries);
        self.sink_dropdown_index = 0;
    }

    /// ↑/↓ in the picker of `sink_count` entries
    pub(crate) const fn picker_move(&mut self, down: bool, sink_count: usize) {
        if down {
            if self.sink_dropdown_index + 1 < sink_count {
                self.sink_dropdown_index += 1;
            }
        } else {
            self.sink_dropdown_index = self.sink_dropdown_index.saturating_sub(1);
        }
    }

    /// Space in the picker: add the highlighted entry at the end of the order, or
    /// remove it (the later ones move up: removing the first choice promotes the
    /// second). The highlight follows the entry to its new row.
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

    /// Shift+↑/↓ in the picker: move the highlighted chosen entry one place
    /// earlier or later in the order (does nothing on an entry not chosen)
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

    /// Enter in the picker: make the chosen entries of `entries` (the sinks or
    /// sources the picker shows) the form's, stored by description. With no sink
    /// chosen the highlighted sink is used (a rule needs one); with no source
    /// chosen the rule has none (it leaves the microphone alone).
    ///
    /// # Errors
    /// When the sink picker has no sink to choose
    pub(crate) fn apply_picks(&mut self, entries: &[SinkConfig]) -> Result<(), &'static str> {
        let kind = self.picker_kind();
        if self.picked.is_empty() && kind == Endpoint::Sink {
            let rows = picker_rows(&self.picked, entries.len());
            let Some(&sink) = rows.get(self.sink_dropdown_index) else {
                return Err("No sink to choose: add one on the Sinks screen");
            };
            self.picked.push(sink);
        }
        // The same entries in the same order: keep the references as they are written
        if self.picked != self.resolved_refs(kind, entries) {
            let refs = self
                .picked
                .iter()
                .filter_map(|&i| entries.get(i))
                .map(|s| s.desc.clone())
                .collect();
            match kind {
                Endpoint::Sink => self.sink_refs = refs,
                Endpoint::Source => self.source_refs = refs,
            }
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
            output_pattern: Input::new(rule.output_pattern.clone().unwrap_or_default()),
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
            RuleField::AppId | RuleField::Title | RuleField::Output | RuleField::Desc
        )
    }

    fn focused_input_mut(&mut self) -> Option<&mut Input> {
        match self.focused_field {
            RuleField::AppId => Some(&mut self.app_id_pattern),
            RuleField::Title => Some(&mut self.title_pattern),
            RuleField::Output => Some(&mut self.output_pattern),
            RuleField::Desc => Some(&mut self.desc),
            RuleField::Sinks | RuleField::Mics | RuleField::Notify => None,
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
        self.open_picker(Endpoint::Sink, sinks);
    }

    /// Open the picker of `kind` (choosing from `entries`, the configured sinks or
    /// sources) with the form's references to them chosen.
    pub(crate) fn open_picker(&mut self, kind: Endpoint, entries: &[SinkConfig]) {
        self.editor.open_picker(kind, entries);
        self.mode = ListMode::SelectSink;
    }
}

/// Context for rendering the rules screen (bundles related parameters)
pub struct RulesRenderContext<'a> {
    pub rules: &'a [Rule],
    pub sinks: &'a [SinkConfig],
    /// The configured sources (mics)
    pub sources: &'a [SinkConfig],
    /// The daemon's window list (for the monitors hint)
    pub windows: &'a [WindowInfo],
    pub screen_state: &'a mut RulesScreen,
    /// Live preview of the editor's patterns (kept current by `App::refresh_preview`)
    pub preview: &'a PreviewView,
    pub daemon_running: bool,
}

/// Rows of the rule form (one per field)
const FORM_ROWS: u16 = 7;

/// What `to_rule` says when the rule has neither an App ID nor an Output
pub(crate) const APP_ID_REQUIRED: &str = "App ID is required (or set an Output to match any app)";

/// Render the rules screen
pub fn render_rules(frame: &mut Frame, area: Rect, ctx: &mut RulesRenderContext) {
    // Always render the list first as background
    render_list(
        frame,
        area,
        ctx.rules,
        ctx.sinks,
        ctx.sources,
        ctx.screen_state,
    );

    // Overlay modal on top if active
    match ctx.screen_state.mode {
        ListMode::List => {}
        ListMode::AddEdit => {
            render_editor(frame, area, ctx);
            if ctx.screen_state.confirm_discard {
                render_discard_dialog(frame, area, "rule");
            }
        }
        ListMode::Delete => {
            if let Some(rule) = ctx.rules.get(ctx.screen_state.selected) {
                let details = RuleDetails::of(rule, ctx.sinks, ctx.sources);
                render_delete_dialog(frame, area, "Rule", &details.rows(false));
            }
        }
        ListMode::SelectSink => {
            let kind = ctx.screen_state.editor.picker_kind();
            let entries = match kind {
                Endpoint::Sink => ctx.sinks,
                Endpoint::Source => ctx.sources,
            };
            render_sink_selector(frame, area, kind, entries, &mut ctx.screen_state.editor);
        }
        ListMode::Inspect => {
            if let Some(rule) = ctx.rules.get(ctx.screen_state.selected) {
                let details = RuleDetails::of(rule, ctx.sinks, ctx.sources);
                let lines = detail_lines(&details.rows(true));
                render_text_dialog(frame, area, "Rule Details", lines, &[("Enter", "close")]);
            }
        }
    }
}

/// Shown for a rule without `app_id` (it matches every app on its monitors)
const ANY_APP: &str = "any app";

/// The description of the entry (sink or source) `entry_ref` names among
/// `entries` (or the reference itself)
fn sink_display<'a>(entries: &'a [SinkConfig], entry_ref: &'a str) -> &'a str {
    sink_index_in(entries, entry_ref).map_or(entry_ref, |i| entries[i].desc.as_str())
}

/// Separator between a rule's sinks, first choice first
const SINK_ARROW: &str = " → ";

/// A rule's sinks (sources) in priority order: `BT Headphones → Headphones`
fn sinks_text<'a>(entries: &[SinkConfig], refs: impl IntoIterator<Item = &'a str>) -> String {
    refs.into_iter()
        .map(|r| sink_display(entries, r))
        .collect::<Vec<_>>()
        .join(SINK_ARROW)
}

/// An output pattern as the table shows it: an anchored connector name
/// (`^HDMI-A-1$`) as the name, anything else as written
fn output_display(pattern: &str) -> &str {
    pattern
        .strip_prefix('^')
        .and_then(|p| p.strip_suffix('$'))
        .filter(|name| {
            !name.is_empty()
                && name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        })
        .unwrap_or(pattern)
}

/// A rule's conditions on the window, in one cell: `^mpv$`, `^mpv$ on ^HDMI-`,
/// `any app on HDMI-A-1`. `any app` is dimmed.
fn window_cell(rule: &Rule, width: usize) -> Line<'static> {
    let app = if rule.has_app_id() {
        rule.app_id_pattern.as_str()
    } else {
        ANY_APP
    };
    let text = match &rule.output_pattern {
        Some(output) => format!("{app} on {}", output_display(output)),
        None => app.to_string(),
    };
    let cut = ellipsize(&text, width).into_owned();
    if !rule.has_app_id()
        && let Some(rest) = cut.strip_prefix(ANY_APP)
    {
        let rest = rest.to_string();
        return Line::from(vec![Span::styled(ANY_APP, theme::dim()), Span::raw(rest)]);
    }
    Line::from(cut)
}

/// A rule's details as shown in its delete and details dialogs: every condition
/// and every target, one sink (source) per row so none is cut
struct RuleDetails<'a> {
    rule: &'a Rule,
    /// The first sink, then `→ fallback`s
    sinks: Vec<String>,
    /// The same for the sources (empty: none)
    sources: Vec<String>,
}

impl<'a> RuleDetails<'a> {
    fn of(rule: &'a Rule, sinks: &[SinkConfig], sources: &[SinkConfig]) -> Self {
        let values = |entries: &[SinkConfig], refs: Vec<&str>| -> Vec<String> {
            refs.into_iter()
                .enumerate()
                .map(|(i, r)| {
                    let desc = sink_display(entries, r);
                    if i == 0 {
                        desc.to_string()
                    } else {
                        format!("→ {desc}")
                    }
                })
                .collect()
        };
        Self {
            rule,
            sinks: values(sinks, rule.refs(Endpoint::Sink)),
            sources: values(sources, rule.refs(Endpoint::Source)),
        }
    }

    /// The rows (`label`, value, placeholder); `full` adds description and notify
    fn rows(&self, full: bool) -> Vec<(&'static str, &str, bool)> {
        let rule = self.rule;
        let placeholder =
            |value: Option<&'a str>, text: &'static str| value.map_or((text, true), |v| (v, false));
        let (app, any_app) = placeholder(
            rule.has_app_id().then_some(rule.app_id_pattern.as_str()),
            ANY_APP,
        );
        let (title, any_title) = placeholder(rule.title_pattern.as_deref(), "any title");
        let (output, any_output) = placeholder(rule.output_pattern.as_deref(), "any monitor");
        let mut rows = vec![
            ("App ID", app, any_app),
            ("Title", title, any_title),
            ("Output", output, any_output),
        ];
        let sinks_label = if self.sinks.len() > 1 {
            "Sinks"
        } else {
            "Sink"
        };
        rows.extend(labelled(sinks_label, &self.sinks));
        if self.sources.is_empty() {
            rows.push(("Mic", "none · the mic is left alone", true));
        } else {
            let label = if self.sources.len() > 1 {
                "Mics"
            } else {
                "Mic"
            };
            rows.extend(labelled(label, &self.sources));
        }
        if full {
            let (desc, no_desc) = placeholder(rule.desc.as_deref(), "none");
            rows.extend([
                ("Description", desc, no_desc),
                ("Notify", notify_text(rule.notify), rule.notify.is_none()),
            ]);
        }
        rows
    }
}

/// Detail rows for `values`, labelled once (on the first)
fn labelled<'v>(
    label: &'static str,
    values: &'v [String],
) -> impl Iterator<Item = (&'static str, &'v str, bool)> {
    values
        .iter()
        .enumerate()
        .map(move |(i, v)| (if i == 0 { label } else { "" }, v.as_str(), false))
}

/// Narrowest rules table that shows the Description column next to the Mic column
const WIDE_RULES_TABLE: u16 = 100;

/// Render the rules table (or the empty-state guidance)
fn render_list(
    frame: &mut Frame,
    area: Rect,
    rules: &[Rule],
    sinks: &[SinkConfig],
    sources: &[SinkConfig],
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
                    "Windows no rule matches play on the default sink",
                    theme::dim(),
                )),
            ]
        };
        render_empty(frame, area, block, lines);
        return;
    }

    screen_state.clamp_selection(rules.len());

    // The Mic column only while some rule chooses a mic; then a narrow table
    // leaves out the descriptions (the details dialog shows them)
    let with_mics = rules.iter().any(|r| !r.source_refs.is_empty());
    let with_desc = !with_mics || area.width >= WIDE_RULES_TABLE;
    let mut widths = vec![
        Constraint::Length(3),
        Constraint::Fill(if with_mics { 3 } else { 2 }),
        Constraint::Fill(2),
        Constraint::Fill(3),
    ];
    let mut header = vec!["#", "Window", "Title", "Sinks"];
    if with_mics {
        widths.push(Constraint::Fill(2));
        header.push("Mic");
    }
    if with_desc {
        widths.push(Constraint::Fill(2));
        header.push("Description");
    }
    let cols = table_columns(&widths, area);
    let cut = |text: &str, col: usize| ellipsize(text, cols[col]).into_owned();
    let desc_col = cols.len() - 1;

    let rows: Vec<Row> = rules
        .iter()
        .enumerate()
        .map(|(i, rule)| {
            let title = rule.title_pattern.as_ref().map_or_else(
                || Cell::from(Span::styled("any", theme::dim())),
                |t| Cell::from(cut(t, 2)),
            );
            let mut cells = vec![
                Cell::from(Span::styled((i + 1).to_string(), theme::dim())),
                Cell::from(window_cell(rule, cols[1])),
                title,
                Cell::from(Span::styled(
                    cut(&sinks_text(sinks, rule.sink_refs()), 3),
                    theme::accent(),
                )),
            ];
            if with_mics {
                cells.push(if rule.source_refs.is_empty() {
                    Cell::from(Span::styled("—", theme::dim()))
                } else {
                    Cell::from(Span::styled(
                        cut(&sinks_text(sources, rule.refs(Endpoint::Source)), 4),
                        theme::accent(),
                    ))
                });
            }
            if with_desc {
                cells.push(Cell::from(Span::styled(
                    cut(rule.desc.as_deref().unwrap_or(""), desc_col),
                    theme::dim(),
                )));
            }
            Row::new(cells)
        })
        .collect();

    render_table(frame, area, block, header, rows, &widths, screen_state);
}

/// How a rule's notify setting reads
const fn notify_text(notify: Option<bool>) -> &'static str {
    match notify {
        Some(true) => "on",
        Some(false) => "off",
        None => "default (from Settings)",
    }
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

/// A dimmed, italic note in place of a selector's value
fn note(text: &str) -> Line<'_> {
    Line::from(Span::styled(
        text,
        theme::dim().add_modifier(ratatui::style::Modifier::ITALIC),
    ))
}

/// The monitors the daemon reports windows on, sorted and without duplicates
fn seen_monitors(windows: &[WindowInfo]) -> Vec<&str> {
    let mut monitors: Vec<&str> = windows
        .iter()
        .flat_map(|w| w.outputs.iter().map(String::as_str))
        .collect();
    monitors.sort_unstable();
    monitors.dedup();
    monitors
}

/// The hint under the form naming the monitors an Output pattern can match
fn monitors_hint(windows: &[WindowInfo], daemon_running: bool, width: usize) -> Line<'static> {
    const LABEL: &str = "Monitors: ";
    let monitors = seen_monitors(windows);
    let value = if !monitors.is_empty() {
        Span::raw(
            ellipsize(
                &monitors.join(", "),
                width.saturating_sub(text_width(LABEL)),
            )
            .into_owned(),
        )
    } else if daemon_running {
        Span::styled(
            "none reported (only the wlr protocol reports them)",
            theme::dim(),
        )
    } else {
        Span::styled("unknown while the daemon is stopped", theme::dim())
    };
    Line::from(vec![Span::styled(LABEL, theme::dim()), value])
}

/// Render the add/edit form with the live preview below the fields
fn render_editor(frame: &mut Frame, area: Rect, ctx: &RulesRenderContext) {
    let screen_state = &*ctx.screen_state;
    let (sinks, sources, preview) = (ctx.sinks, ctx.sources, ctx.preview);
    let editor = &screen_state.editor;
    let focused = |field| editor.focused_field == field && !screen_state.confirm_discard;
    let hint = |field, text| focused(field).then(|| Span::styled(text, theme::dim()));

    let sink_value = if editor.sink_refs.is_empty() {
        note("none chosen")
    } else {
        Line::from(Span::styled(
            sinks_text(sinks, editor.sink_refs.iter().map(String::as_str)),
            theme::accent(),
        ))
    };
    let mic_value = if sources.is_empty() && editor.source_refs.is_empty() {
        note("configure mics first (2 Sinks → i)")
    } else if editor.source_refs.is_empty() {
        note("none · the mic is left alone")
    } else {
        Line::from(Span::styled(
            sinks_text(sources, editor.source_refs.iter().map(String::as_str)),
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
    let has_output = !editor.output_pattern.value().is_empty();
    let app_placeholder = if has_output {
        ANY_APP
    } else {
        "regex, e.g. ^firefox$"
    };

    let fields = [
        FormField {
            label: "App ID",
            value: FieldValue::Text {
                input: &editor.app_id_pattern,
                placeholder: Cow::Borrowed(app_placeholder),
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
            label: "Output",
            value: FieldValue::Text {
                input: &editor.output_pattern,
                placeholder: Cow::Borrowed("any monitor (optional regex, e.g. ^HDMI-)"),
            },
            trailing: validation_mark(
                editor.output_pattern.value(),
                preview.output_error.is_some(),
            ),
            focused: focused(RuleField::Output),
        },
        FormField {
            label: "Sinks",
            value: FieldValue::Fixed(sink_value),
            trailing: hint(RuleField::Sinks, "␣ choose"),
            focused: focused(RuleField::Sinks),
        },
        FormField {
            label: "Mics",
            value: FieldValue::Fixed(mic_value),
            trailing: if sources.is_empty() {
                None
            } else {
                hint(RuleField::Mics, "␣ choose")
            },
            focused: focused(RuleField::Mics),
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
        editor.app_id_pattern.value().is_empty() && !has_output,
        preview,
        ctx.daemon_running,
    );
    // The monitors hint, and the title of the window the form was opened from
    let hint_rows = 1 + u16::from(editor.window_title.is_some());
    // Fields, the hints, a separator row, the preview
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
    let mut hint_row = |line: Line, used: &mut u16| {
        if inner.height > *used {
            frame.render_widget(line, Rect {
                y: inner.y + *used,
                height: 1,
                ..inner
            });
            *used += 1;
        }
    };

    hint_row(
        monitors_hint(ctx.windows, ctx.daemon_running, usize::from(inner.width)),
        &mut used,
    );
    // The window the form was opened from: its title, to copy part of it
    if let Some(window_title) = &editor.window_title {
        const LABEL: &str = "Current title: ";
        let room = usize::from(inner.width).saturating_sub(text_width(LABEL));
        let value = if window_title.is_empty() {
            Span::styled("(empty)", theme::dim())
        } else {
            Span::raw(ellipsize(window_title, room))
        };
        hint_row(
            Line::from(vec![Span::styled(LABEL, theme::dim()), value]),
            &mut used,
        );
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
/// (`incomplete`: neither an app ID nor an output pattern is set)
fn live_preview_lines(
    incomplete: bool,
    preview: &PreviewView,
    daemon_running: bool,
) -> Vec<Line<'_>> {
    let line = |text: String, style| Line::from(Span::styled(text, style));

    let mut lines = Vec::new();
    let errors = [
        ("app_id", &preview.app_error),
        ("title", &preview.title_error),
        ("output", &preview.output_error),
    ];
    if incomplete {
        // Users who want to match all windows should use `.*` explicitly.
        lines.push(line(
            "Type an app ID or output pattern to see which windows it matches".to_string(),
            theme::dim(),
        ));
    } else if errors.iter().any(|(_, e)| e.is_some()) {
        for (what, error) in errors {
            if let Some(e) = error {
                lines.push(line(format!("Invalid {what} regex: {e}"), theme::error()));
            }
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

/// Render the picker opened from the editor's Sinks or Mics field: the chosen
/// entries numbered in priority order, then the others
fn render_sink_selector(
    frame: &mut Frame,
    area: Rect,
    kind: Endpoint,
    sinks: &[SinkConfig],
    editor: &mut RuleEditor,
) {
    // Sync state (the list may have shrunk since the picker opened)
    editor.picked.retain(|&i| i < sinks.len());
    editor.sink_dropdown_index = editor
        .sink_dropdown_index
        .min(sinks.len().saturating_sub(1));

    let rows = picker_rows(&editor.picked, sinks.len());
    let entries: Vec<(&str, &str)> = rows
        .iter()
        .map(|&i| (sinks[i].desc.as_str(), sinks[i].name.as_str()))
        .collect();
    // "1 " before the chosen entries, as wide as the largest number
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
    let title = format!("Choose {} (in order)", plural(kind));
    render_pick_list(
        frame,
        rect,
        &title,
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
        make_rule, make_rule_with_fallbacks, make_sink, with_output, with_sources,
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
        editor.open_picker(Endpoint::Sink, &sinks());
        editor
    }

    fn sources() -> Vec<SinkConfig> {
        vec![
            make_sink("alsa_input.headset", "Headset Mic", true),
            make_sink("alsa_input.webcam", "Webcam Mic", false),
        ]
    }

    /// An edit that does not touch the mics keeps them; the mic picker edits them
    #[test]
    fn test_edit_keeps_and_edits_the_rules_sources() {
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

        // The mic picker: add the webcam as a fallback, then reorder it first
        editor.open_picker(Endpoint::Source, &sources());
        assert_eq!(editor.picker_kind(), Endpoint::Source);
        assert_eq!(editor.picked, [0]);
        editor.picker_move(true, 2);
        editor.picker_toggle(2);
        editor.picker_reorder(false);
        editor.apply_picks(&sources()).expect("picks");
        assert_eq!(editor.source_refs, ["Webcam Mic", "Headset Mic"]);
        assert_eq!(editor.sink_refs, ["Headphones"], "sinks untouched");
        assert!(editor.is_modified());

        // Removing every mic: the rule leaves the microphone alone
        editor.open_picker(Endpoint::Source, &sources());
        for _ in 0..2 {
            editor.sink_dropdown_index = 0; // the first chosen one
            editor.picker_toggle(2);
        }
        assert_eq!(editor.picked, Vec::<usize>::new());
        editor.apply_picks(&sources()).expect("no mic is fine");
        assert_eq!(editor.source_refs, Vec::<String>::new());
        assert_eq!(
            editor.to_rule().expect("valid").source_refs,
            Vec::<String>::new()
        );
    }

    /// The Output field round-trips and is edited like the other patterns; App ID
    /// may be empty only while Output is set
    #[test]
    fn test_output_round_trips_and_makes_app_id_optional() {
        for app_id in ["^mpv$", ""] {
            let rule = with_output(make_rule(app_id, None, "Speakers"), "^HDMI-");
            let mut editor = RuleEditor::from_item(&rule);
            assert_eq!(editor.output_pattern.value(), "^HDMI-");
            assert!(!editor.is_modified());
            editor.sink_refs = vec!["Headphones".to_string()];
            let edited = editor.to_rule().expect("valid rule");
            assert_eq!(edited.output_pattern.as_deref(), Some("^HDMI-"));
            assert!(edited.output_regex.is_some_and(|r| r.is_match("HDMI-A-1")));
            assert_eq!(edited.app_id_pattern, app_id);
            assert_eq!(
                RuleEditor::from_item(&rule).to_rule().expect("valid rule"),
                rule
            );
        }
        assert_eq!(RuleEditor::blank().output_pattern.value(), "");

        let mut editor = RuleEditor::blank();
        editor.sink_refs = vec!["Speakers".to_string()];
        assert_eq!(editor.to_rule().expect_err("no app ID"), APP_ID_REQUIRED);
        editor.output_pattern = Input::new("DP-2".to_string());
        assert!(editor.is_modified());
        let rule = editor.to_rule().expect("output alone is enough");
        assert!(!rule.has_app_id());
        assert_eq!(rule.output_pattern.as_deref(), Some("DP-2"));
        // Clearing Output again makes the App ID required again
        editor.output_pattern = Input::default();
        assert_eq!(editor.to_rule().expect_err("no app ID"), APP_ID_REQUIRED);
        // An invalid output pattern is reported
        editor.output_pattern = Input::new("(".to_string());
        let error = editor.to_rule().expect_err("invalid regex");
        assert!(error.starts_with("Invalid output regex"), "{error}");
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
        editor.open_picker(Endpoint::Sink, &sinks());
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
        editor.open_picker(Endpoint::Sink, &sinks());
        editor.picker_move(true, 3);
        editor.apply_picks(&sinks()).expect("picks");
        assert_eq!(editor.sink_refs, ["Headphones"]);
        editor
            .apply_picks(&[])
            .expect("a sink is already picked: nothing to choose");
        let mut empty = RuleEditor::blank();
        assert_eq!(
            empty.apply_picks(&[]),
            Err("No sink to choose: add one on the Sinks screen")
        );
    }

    #[test]
    fn test_a_rule_needs_a_sink() {
        let mut editor = RuleEditor::blank();
        editor.app_id_pattern = Input::new("^mpv$".to_string());
        assert_eq!(
            editor.to_rule().expect_err("no sink"),
            "Choose at least one sink (Space on Sinks)"
        );
        editor.sink_refs = vec!["Speakers".to_string()];
        let rule = editor.to_rule().expect("valid rule");
        assert_eq!(rule.fallback_refs, Vec::<String>::new());
    }

    #[test]
    fn test_output_display_shows_anchored_names_plainly() {
        assert_eq!(output_display("^HDMI-A-1$"), "HDMI-A-1");
        assert_eq!(output_display("^HDMI-"), "^HDMI-");
        assert_eq!(output_display("^(DP-1|DP-2)$"), "^(DP-1|DP-2)$");
        assert_eq!(output_display("^$"), "^$");
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
