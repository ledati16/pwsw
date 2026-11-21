//! Sinks screen - Manage the audio outputs (sinks) and inputs (sources, shown as
//! "mics"); `i` switches between the two lists, which share every view and form.

use std::borrow::Cow;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Cell, ListItem, ListState, Row};
use tui_input::Input;

use crate::config::{Endpoint, SinkConfig};
use crate::pipewire::{ActiveSink, ProfileSink};
use crate::tui::screens::list::{
    FormEditor, ListMode, ListScreen, open_editor_dialog, optional, render_delete_dialog,
    render_discard_dialog, render_empty, render_table, table_columns,
};
use crate::tui::theme;
use crate::tui::widgets::{
    FieldValue, FormField, MAX_DIALOG_WIDTH, checkbox, detail_lines, ellipsize, form_width,
    pick_list_rect, picker_lines, render_form, render_pick_list, render_text_dialog, rounded_block,
    text_width, truncate_node_name,
};

/// "sink" / "mic": what one entry of the `kind` list is called in the TUI
pub(crate) const fn noun(kind: Endpoint) -> &'static str {
    match kind {
        Endpoint::Sink => "sink",
        Endpoint::Source => "mic",
    }
}

/// "Sink" / "Mic"
pub(crate) const fn title(kind: Endpoint) -> &'static str {
    match kind {
        Endpoint::Sink => "Sink",
        Endpoint::Source => "Mic",
    }
}

/// "Sinks" / "Mics"
pub(crate) const fn plural(kind: Endpoint) -> &'static str {
    match kind {
        Endpoint::Sink => "Sinks",
        Endpoint::Source => "Mics",
    }
}

/// The other list of the screen (what `i` switches to)
pub(crate) const fn other(kind: Endpoint) -> Endpoint {
    match kind {
        Endpoint::Sink => Endpoint::Source,
        Endpoint::Source => Endpoint::Sink,
    }
}

/// Field of the sink editor
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SinkField {
    /// Node name (text, or pick from the node selector with Space)
    #[default]
    Name,
    Desc,
    Icon,
    /// Fallback checkbox (Space toggles)
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
}

/// The sink add/edit form
#[derive(Debug, Default)]
pub struct SinkEditor {
    pub name: Input,
    pub desc: Input,
    pub icon: Input,
    pub default: bool,
    pub focused_field: SinkField,
    /// Node name highlighted in the node selector. Kept by name because the
    /// lists behind the selector are replaced by every `PipeWire` poll.
    pub selector_node: Option<String>,
    /// Scroll state of the node selector
    pub selector_state: ListState,
    /// Name, description, icon and fallback flag when the editor opened
    initial: (String, String, String, bool),
}

impl FormEditor for SinkEditor {
    type Item = SinkConfig;

    fn blank() -> Self {
        Self::default()
    }

    fn from_item(sink: &SinkConfig) -> Self {
        let icon = sink.icon.clone().unwrap_or_default();
        Self {
            name: Input::new(sink.name.clone()),
            desc: Input::new(sink.desc.clone()),
            icon: Input::new(icon.clone()),
            default: sink.default,
            initial: (sink.name.clone(), sink.desc.clone(), icon, sink.default),
            ..Self::default()
        }
    }

    fn is_modified(&self) -> bool {
        let (name, desc, icon, default) = &self.initial;
        self.name.value() != name
            || self.desc.value() != desc
            || self.icon.value() != icon
            || self.default != *default
    }

    fn next_field(&mut self) {
        self.focused_field = self.focused_field.next();
    }

    fn prev_field(&mut self) {
        self.focused_field = self.focused_field.prev();
    }

    fn text_focused(&self) -> bool {
        self.focused_field != SinkField::Default
    }

    fn focused_input_mut(&mut self) -> Option<&mut Input> {
        match self.focused_field {
            SinkField::Name => Some(&mut self.name),
            SinkField::Desc => Some(&mut self.desc),
            SinkField::Icon => Some(&mut self.icon),
            SinkField::Default => None,
        }
    }
}

impl SinkEditor {
    /// The sink the form describes
    ///
    /// # Errors
    /// When the node name or the description is empty
    pub(crate) fn to_sink(&self) -> Result<SinkConfig, &'static str> {
        if self.name.value().is_empty() || self.desc.value().is_empty() {
            return Err("Node and Description are required");
        }
        Ok(SinkConfig {
            name: self.name.value().to_string(),
            desc: self.desc.value().to_string(),
            icon: optional(&self.icon),
            default: self.default,
        })
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
pub type SinksScreen = ListScreen<SinkEditor>;

impl SinksScreen {
    /// Open the node selector on the edited node name if it is listed, else on
    /// the first entry.
    pub(crate) fn open_selector(&mut self, names: &[&str]) {
        let current = self.editor.name.value();
        self.editor.selector_node = names
            .iter()
            .find(|n| **n == current)
            .or_else(|| names.first())
            .map(|n| (*n).to_string());
        self.mode = ListMode::SelectSink;
    }

    /// Move the node selector highlight by one entry (`down` or up) within
    /// `names`; a highlight that disappeared restarts at the first entry.
    pub(crate) fn move_selector(&mut self, names: &[&str], down: bool) {
        let pos = self
            .editor
            .selector_node
            .as_deref()
            .and_then(|node| names.iter().position(|n| *n == node));
        let new = match pos {
            None => 0,
            Some(p) if down => (p + 1).min(names.len().saturating_sub(1)),
            Some(p) => p.saturating_sub(1),
        };
        self.editor.selector_node = names.get(new).map(|n| (*n).to_string());
    }
}

/// Context for rendering the sinks screen (bundles related parameters)
///
/// Everything is of the list shown (`kind`): the configured sinks or sources, the
/// state of that list, and the `PipeWire` nodes of that direction.
pub struct SinksRenderContext<'a> {
    pub kind: Endpoint,
    pub sinks: &'a [SinkConfig],
    pub screen_state: &'a mut SinksScreen,
    pub active_sink_list: &'a [ActiveSink],
    pub profile_sink_list: &'a [ProfileSink],
    pub pipewire_available: bool,
}

/// Where a configured sink (or source) stands in the last `PipeWire` snapshot
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SinkState {
    /// It is the current default sink (source): audio plays (is recorded) here now
    Playing,
    /// The node exists
    Present,
    /// The node appears after a device profile switch
    ViaProfile,
    /// Not found
    Missing,
}

impl SinkState {
    /// State of the sink with node `name`
    pub(crate) fn of(name: &str, active: &[ActiveSink], profiles: &[ProfileSink]) -> Self {
        match active.iter().find(|s| s.name == name) {
            Some(s) if s.is_default => Self::Playing,
            Some(_) => Self::Present,
            None if profiles.iter().any(|p| p.predicted_name == name) => Self::ViaProfile,
            None => Self::Missing,
        }
    }

    /// How the state reads in a list of `kind` entries
    pub(crate) const fn label(self, kind: Endpoint) -> &'static str {
        match self {
            Self::Playing if matches!(kind, Endpoint::Source) => "● in use",
            Self::Playing => "▶ playing",
            Self::Present => "present",
            Self::ViaProfile => "via profile",
            Self::Missing => "missing",
        }
    }

    fn style(self) -> ratatui::style::Style {
        match self {
            Self::Playing => theme::success().add_modifier(ratatui::style::Modifier::BOLD),
            Self::Present => theme::text(),
            Self::ViaProfile => theme::dim(),
            Self::Missing => theme::error(),
        }
    }

    /// Whether the node can be used right now without a profile switch
    pub(crate) const fn is_present(self) -> bool {
        matches!(self, Self::Playing | Self::Present)
    }
}

/// Marker of the fallback sink in the table
pub(crate) const FALLBACK_MARKER: &str = "★ default";

/// Render the sinks screen
pub fn render_sinks(frame: &mut Frame, area: Rect, ctx: &mut SinksRenderContext) {
    // Always render the list first as background
    render_list(frame, area, ctx);

    // Overlay modal on top if active
    match ctx.screen_state.mode {
        ListMode::List => {}
        ListMode::AddEdit => {
            render_editor(frame, area, ctx.kind, ctx.screen_state);
            if ctx.screen_state.confirm_discard {
                render_discard_dialog(frame, area, noun(ctx.kind));
            }
        }
        ListMode::Delete => {
            if let Some(sink) = ctx.sinks.get(ctx.screen_state.selected) {
                render_delete_dialog(frame, area, title(ctx.kind), &[
                    ("Description", &sink.desc, false),
                    ("Node", &sink.name, false),
                ]);
            }
        }
        ListMode::SelectSink => render_sink_selector(
            frame,
            area,
            ctx.kind,
            ctx.active_sink_list,
            ctx.profile_sink_list,
            &mut ctx.screen_state.editor,
        ),
        ListMode::Inspect => render_inspect_popup(frame, area, ctx),
    }
}

/// Render the sinks table (or the empty-state guidance)
fn render_list(frame: &mut Frame, area: Rect, ctx: &mut SinksRenderContext) {
    let sinks = ctx.sinks;
    let states: Vec<SinkState> = sinks
        .iter()
        .map(|s| SinkState::of(&s.name, ctx.active_sink_list, ctx.profile_sink_list))
        .collect();
    let present = states.iter().filter(|s| s.is_present()).count();

    // PipeWire unavailable: say so above the table
    let area = if ctx.pipewire_available {
        area
    } else {
        let [warning_area, rest] =
            Layout::vertical([Constraint::Length(1), Constraint::Fill(1)]).areas(area);
        frame.render_widget(
            Line::from(vec![
                Span::styled("⚠ PipeWire unavailable", theme::warning()),
                Span::styled(" · states may be out of date", theme::dim()),
            ]),
            warning_area,
        );
        rest
    };

    let kind = ctx.kind;
    let name = Span::styled(format!(" {} ", plural(kind)), theme::bold());
    let title = if sinks.is_empty() {
        Line::from(name)
    } else {
        Line::from(vec![
            name,
            Span::styled(
                format!("· {} configured · {present} present ", sinks.len()),
                theme::dim(),
            ),
        ])
    };
    // The other list, one key away
    let switch = Line::from(vec![
        Span::styled("i", theme::key()),
        Span::raw(format!(" {} ", plural(other(kind)).to_lowercase())),
    ]);
    let block = rounded_block()
        .title(title)
        .title_top(switch.alignment(ratatui::layout::Alignment::Right));

    if sinks.is_empty() {
        render_empty(frame, area, block, empty_lines(kind));
        return;
    }

    ctx.screen_state.clamp_selection(sinks.len());

    let widths = [
        Constraint::Fill(1),
        Constraint::Fill(1),
        Constraint::Length(11),
        Constraint::Length(u16::try_from(text_width(FALLBACK_MARKER)).unwrap_or(10)),
    ];
    let cols = table_columns(&widths, area);

    let rows: Vec<Row> = sinks
        .iter()
        .zip(&states)
        .map(|(sink, state)| {
            Row::new(vec![
                Cell::from(ellipsize(&sink.desc, cols[0]).into_owned()),
                Cell::from(Span::styled(
                    truncate_node_name(&sink.name, cols[1]).into_owned(),
                    theme::dim(),
                )),
                Cell::from(Span::styled(state.label(kind), state.style())),
                if sink.default {
                    Cell::from(Span::styled(FALLBACK_MARKER, theme::accent()))
                } else {
                    Cell::from("")
                },
            ])
        })
        .collect();

    render_table(
        frame,
        area,
        block,
        vec!["Description", "Node", "State", ""],
        rows,
        &widths,
        ctx.screen_state,
    );
}

/// The guidance shown while the `kind` list is empty
fn empty_lines(kind: Endpoint) -> Vec<Line<'static>> {
    let pick = |what: &'static str| {
        Line::from(vec![
            Span::raw("Press "),
            Span::styled("a", theme::key()),
            Span::raw(", then "),
            Span::styled("Space", theme::key()),
            Span::raw(format!(" to pick {what} from PipeWire")),
        ])
    };
    match kind {
        Endpoint::Sink => vec![
            Line::from(Span::styled("No sinks yet", theme::bold())),
            Line::from(""),
            pick("an output"),
        ],
        Endpoint::Source => vec![
            Line::from(Span::styled("No mics yet", theme::bold())),
            Line::from(""),
            Line::from(Span::styled(
                "Without mics pwsw never touches the microphone.",
                theme::dim(),
            )),
            Line::from(Span::styled(
                "Add some to let rules choose the input too.",
                theme::dim(),
            )),
            Line::from(""),
            pick("an input"),
        ],
    }
}

/// The icon name a sink (source) without one gets automatically
fn auto_icon(kind: Endpoint, desc: &str) -> String {
    let entry = SinkConfig {
        name: String::new(),
        desc: desc.to_string(),
        icon: None,
        default: false,
    };
    match kind {
        Endpoint::Sink => crate::notification::get_sink_icon(&entry),
        Endpoint::Source => crate::notification::get_source_icon(&entry),
    }
}

/// Render the sink details dialog
fn render_inspect_popup(frame: &mut Frame, area: Rect, ctx: &SinksRenderContext) {
    let Some(sink) = ctx.sinks.get(ctx.screen_state.selected) else {
        return;
    };
    let kind = ctx.kind;
    let state = SinkState::of(&sink.name, ctx.active_sink_list, ctx.profile_sink_list);
    let auto = format!("(auto: {})", auto_icon(kind, &sink.desc));
    let (icon, icon_auto) = sink
        .icon
        .as_deref()
        .map_or((auto.as_str(), true), |i| (i, false));
    let fallback = match (sink.default, kind) {
        (true, Endpoint::Sink) => "yes · used when no rule matches",
        (true, Endpoint::Source) => "yes · used when no rule chooses a mic",
        (false, _) => "no",
    };
    let lines = detail_lines(&[
        ("Node", &sink.name, false),
        ("Description", &sink.desc, false),
        ("Icon", icon, icon_auto),
        ("Default", fallback, false),
        ("State", state.label(kind), false),
    ]);
    let heading = format!("{} Details", title(kind));
    render_text_dialog(frame, area, &heading, lines, &[("Enter", "close")]);
}

/// Render the add/edit form
fn render_editor(frame: &mut Frame, area: Rect, kind: Endpoint, screen_state: &SinksScreen) {
    let editor = &screen_state.editor;
    let focused = |field| editor.focused_field == field && !screen_state.confirm_discard;
    let hint = |field, text| focused(field).then(|| Span::styled(text, theme::dim()));
    let auto = format!("(auto: {})", auto_icon(kind, editor.desc.value()));
    let fallback = match kind {
        Endpoint::Sink => "used when no rule matches",
        Endpoint::Source => "used when no rule chooses a mic",
    };

    let fields = [
        FormField {
            label: "Node",
            value: FieldValue::Text {
                input: &editor.name,
                placeholder: Cow::Borrowed("type a node name, or Space to pick"),
            },
            trailing: hint(SinkField::Name, "␣ pick"),
            focused: focused(SinkField::Name),
        },
        FormField {
            label: "Description",
            value: FieldValue::Text {
                input: &editor.desc,
                placeholder: Cow::Borrowed("name shown in notifications"),
            },
            trailing: None,
            focused: focused(SinkField::Desc),
        },
        FormField {
            label: "Icon",
            value: FieldValue::Text {
                input: &editor.icon,
                placeholder: Cow::Owned(auto),
            },
            trailing: None,
            focused: focused(SinkField::Icon),
        },
        FormField {
            label: "Default",
            value: FieldValue::Fixed(checkbox(editor.default, fallback)),
            trailing: hint(SinkField::Default, "␣ toggle"),
            focused: focused(SinkField::Default),
        },
    ];

    let width = (form_width(&fields, 52) + 4).min(MAX_DIALOG_WIDTH);
    let heading = screen_state.editor_title(title(kind));
    let inner = open_editor_dialog(frame, area, &heading, width, 4 + 2);
    render_form(frame, inner, &fields);
}

/// Render the node selector opened from the editor's Node field
fn render_sink_selector(
    frame: &mut Frame,
    area: Rect,
    kind: Endpoint,
    active_sinks: &[ActiveSink],
    profile_sinks: &[ProfileSink],
    editor: &mut SinkEditor,
) {
    const PROFILE_HEADER: &str = "After a profile switch";
    let active_header = match kind {
        Endpoint::Sink => "Sinks in PipeWire",
        Endpoint::Source => "Inputs in PipeWire",
    };

    let entries: Vec<(&str, &str)> = active_sinks
        .iter()
        .map(|s| (s.description.as_str(), s.name.as_str()))
        .chain(
            profile_sinks
                .iter()
                .map(|s| (s.description.as_str(), s.predicted_name.as_str())),
        )
        .collect();
    let widest = entries
        .iter()
        .map(|(d, n)| text_width(d) + 2 + text_width(n))
        .chain([text_width(PROFILE_HEADER), text_width(active_header)])
        .max()
        .unwrap_or(0);
    let rows = 1
        + active_sinks.len()
        + if profile_sinks.is_empty() {
            0
        } else {
            2 + profile_sinks.len()
        };
    let (rect, content_width) = pick_list_rect(area, widest, rows, 0);

    let heading = |text: &'static str| ListItem::new(Line::from(Span::styled(text, theme::bold())));
    let mut lines = picker_lines(&entries, content_width).into_iter();
    let mut items: Vec<ListItem> = vec![heading(active_header)];
    items.extend(lines.by_ref().take(active_sinks.len()).map(ListItem::new));
    if !profile_sinks.is_empty() {
        items.push(ListItem::new(Line::from("")));
        items.push(heading(PROFILE_HEADER));
        items.extend(lines.map(ListItem::new));
    }

    // Highlight the selected node wherever it is in the current lists
    let names = selector_names(active_sinks, profile_sinks);
    let row = editor
        .selector_node
        .as_deref()
        .and_then(|node| names.iter().position(|n| *n == node))
        .map(|pos| selector_list_index(pos, active_sinks.len()));
    editor.selector_state.select(row);
    render_pick_list(
        frame,
        rect,
        "Pick a Node",
        &[("Enter", "use"), ("Esc", "back")],
        items,
        &mut editor.selector_state,
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
        assert_eq!(screen.editor.selector_node.as_deref(), Some("alsa.b"));

        // A Bluetooth sink appears in front: the highlight stays on alsa.b
        let after = [active("bluez.x"), active("alsa.a"), active("alsa.b")];
        let profiles = [profile("alsa.c")];
        let names = selector_names(&after, &profiles);
        assert_eq!(screen.editor.selector_node.as_deref(), Some("alsa.b"));
        screen.move_selector(&names, true);
        assert_eq!(screen.editor.selector_node.as_deref(), Some("alsa.c"));
        screen.move_selector(&names, true);
        assert_eq!(
            screen.editor.selector_node.as_deref(),
            Some("alsa.c"),
            "stops at end"
        );
        screen.move_selector(&names, false);
        assert_eq!(screen.editor.selector_node.as_deref(), Some("alsa.b"));
    }

    #[test]
    fn test_open_selector_preselects_edited_node() {
        let mut screen = SinksScreen::new();
        screen.editor.name = Input::new("alsa.b".to_string());
        let sinks = [active("alsa.a"), active("alsa.b")];
        screen.open_selector(&selector_names(&sinks, &[]));
        assert_eq!(screen.editor.selector_node.as_deref(), Some("alsa.b"));

        let mut empty = SinksScreen::new();
        empty.open_selector(&[]);
        assert_eq!(empty.editor.selector_node, None);
    }
}
