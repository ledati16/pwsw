//! What the sinks and rules screens share: a table of items, an add/edit form,
//! delete and details dialogs and a selection list opened from the form.

use ratatui::Frame;
use ratatui::layout::{Constraint, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, HighlightSpacing, Row, Table, TableState};
use tui_input::Input;

use crate::tui::app::Screen;
use crate::tui::keymap::Context;
use crate::tui::theme;
use crate::tui::widgets::{
    border_hints, column_widths, detail_lines, dialog_block, modal_rect, open_dialog,
    render_centered, render_scrollbar, render_text_dialog, text_width,
};

/// Rows of a list table above the first item (the header)
const TABLE_HEADER_ROWS: u16 = 1;

/// What a list screen shows
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ListMode {
    #[default]
    List,
    AddEdit,
    Delete,
    /// The selection list opened from a field of the form
    SelectSink,
    Inspect,
}

/// The add/edit form of a list screen
pub(crate) trait FormEditor {
    /// What the form edits
    type Item;
    /// An empty form (adding)
    fn blank() -> Self;
    /// A form filled from `item` (editing)
    fn from_item(item: &Self::Item) -> Self;
    /// Whether anything was changed since the form opened
    fn is_modified(&self) -> bool;
    fn next_field(&mut self);
    fn prev_field(&mut self);
    /// Whether the focused field is a text input
    fn text_focused(&self) -> bool;
    /// The text input of the focused field (`None` on selectors and toggles)
    fn focused_input_mut(&mut self) -> Option<&mut Input>;
}

/// `None` for an empty text field, else its text
pub(crate) fn optional(input: &Input) -> Option<String> {
    (!input.value().is_empty()).then(|| input.value().to_string())
}

/// State of a list screen editing items with form `E`
#[derive(Debug)]
pub struct ListScreen<E> {
    pub mode: ListMode,
    pub selected: usize,
    pub editor: E,
    /// `None`: the form adds an item; `Some(i)`: it edits item `i`
    pub editing_index: Option<usize>,
    /// Table scroll state
    pub state: TableState,
    /// The form asks "discard changes?" (Esc was pressed with edits)
    pub confirm_discard: bool,
}

impl<E: FormEditor> ListScreen<E> {
    pub(crate) fn new() -> Self {
        Self {
            mode: ListMode::List,
            selected: 0,
            editor: E::blank(),
            editing_index: None,
            state: TableState::default(),
            confirm_discard: false,
        }
    }

    pub(crate) const fn select_previous(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }

    pub(crate) const fn select_next(&mut self, len: usize) {
        if self.selected + 1 < len {
            self.selected += 1;
        }
    }

    /// Keep the selection inside a list of `len` items.
    pub(crate) const fn clamp_selection(&mut self, len: usize) {
        if self.selected >= len {
            self.selected = len.saturating_sub(1);
        }
    }

    pub(crate) fn start_add(&mut self) {
        self.mode = ListMode::AddEdit;
        self.editor = E::blank();
        self.editing_index = None;
    }

    pub(crate) fn start_edit(&mut self, items: &[E::Item]) {
        if let Some(item) = items.get(self.selected) {
            self.mode = ListMode::AddEdit;
            self.editor = E::from_item(item);
            self.editing_index = Some(self.selected);
        }
    }

    pub(crate) const fn start_delete(&mut self) {
        self.mode = ListMode::Delete;
    }

    pub(crate) const fn start_inspect(&mut self) {
        self.mode = ListMode::Inspect;
    }

    /// Back to the list (closes any dialog or form)
    pub(crate) const fn cancel(&mut self) {
        self.mode = ListMode::List;
        self.confirm_discard = false;
    }

    /// Esc in the form: close it, asking first if anything was changed
    pub(crate) fn close_editor(&mut self) {
        if self.editor.is_modified() {
            self.confirm_discard = true;
        } else {
            self.cancel();
        }
    }

    /// Whether a text field of the form has the keyboard
    pub(crate) fn text_focused(&self) -> bool {
        self.mode == ListMode::AddEdit && !self.confirm_discard && self.editor.text_focused()
    }

    /// The key binding context of the open dialog (`None` on the list)
    pub(crate) const fn context(&self, screen: Screen) -> Option<Context> {
        match self.mode {
            ListMode::List => None,
            ListMode::AddEdit if self.confirm_discard => Some(Context::Discard(screen)),
            ListMode::AddEdit => Some(Context::Editor(screen)),
            ListMode::SelectSink => Some(Context::Dropdown(screen)),
            ListMode::Delete | ListMode::Inspect => Some(Context::Dialog(screen)),
        }
    }

    /// "Add Sink" / "Edit Sink"
    pub(crate) fn editor_title(&self, what: &str) -> String {
        let verb = if self.editing_index.is_some() {
            "Edit"
        } else {
            "Add"
        };
        format!("{verb} {what}")
    }
}

/// The widths the cells of a list table drawn in `area` get (to cut their text)
pub(crate) fn table_columns(widths: &[Constraint], area: Rect) -> Vec<usize> {
    let selection = u16::try_from(text_width(theme::SELECTED_SYMBOL)).unwrap_or(2);
    column_widths(widths, area.width.saturating_sub(2), selection)
}

/// Draw the table of a list screen in `block`, the selected row highlighted,
/// with a scrollbar when it overflows
pub(crate) fn render_table<E>(
    frame: &mut Frame,
    area: Rect,
    block: Block,
    header: Vec<&'static str>,
    rows: Vec<Row>,
    widths: &[Constraint],
    screen: &mut ListScreen<E>,
) {
    let len = rows.len();
    let table = Table::new(rows, widths.iter().copied())
        .header(Row::new(header).style(theme::bold()))
        .block(block)
        .row_highlight_style(theme::selected())
        .highlight_symbol(theme::SELECTED_SYMBOL)
        .highlight_spacing(HighlightSpacing::Always);
    screen.state.select(Some(screen.selected));
    frame.render_stateful_widget(table, area, &mut screen.state);
    render_scrollbar(frame, area, TABLE_HEADER_ROWS, len, screen.state.offset());
}

/// Draw `block` with the guidance `lines` centred in it (a list with no items)
pub(crate) fn render_empty(frame: &mut Frame, area: Rect, block: Block, lines: Vec<Line>) {
    let inner = block.inner(area);
    frame.render_widget(block, area);
    render_centered(frame, inner, lines);
}

/// Clear `width`×`height` cells centred in `area` for the form titled `title`;
/// returns the area inside its border
pub(crate) fn open_editor_dialog(
    frame: &mut Frame,
    area: Rect,
    title: &str,
    width: u16,
    height: u16,
) -> Rect {
    let block =
        dialog_block(title).title_bottom(border_hints(&[("Enter", "save"), ("Esc", "cancel")]));
    open_dialog(frame, modal_rect(area, width, height), block)
}

/// The "discard changes?" question over the form editing a `what` ("sink")
pub(crate) fn render_discard_dialog(frame: &mut Frame, area: Rect, what: &str) {
    render_text_dialog(
        frame,
        area,
        "Discard changes?",
        vec![Line::from(format!(
            "Your changes to this {what} are not saved."
        ))],
        &[("y", "discard"), ("n", "keep editing")],
    );
}

/// The delete confirmation for a `what` ("Sink"), showing `details` of it
pub(crate) fn render_delete_dialog(
    frame: &mut Frame,
    area: Rect,
    what: &str,
    details: &[(&str, &str, bool)],
) {
    let mut lines = vec![
        Line::from(Span::styled(
            format!("Delete this {}?", what.to_lowercase()),
            theme::bold(),
        )),
        Line::from(""),
    ];
    lines.extend(detail_lines(details));
    render_text_dialog(frame, area, &format!("Delete {what}"), lines, &[
        ("Enter", "delete"),
        ("Esc", "cancel"),
    ]);
}
