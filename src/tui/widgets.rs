//! Shared TUI widget helpers: blocks, content-sized dialogs, one-row forms,
//! scrollbars and display-width text helpers.

use std::borrow::Cow;

use ratatui::layout::{Alignment, Constraint, Flex, Layout, Rect};
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, BorderType, Clear, Padding, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState,
};
use ratatui::{Frame, symbols};
use tui_input::Input;

use crate::tui::theme;

/// Widest a dialog gets, however wide the terminal
pub(crate) const MAX_DIALOG_WIDTH: u16 = 76;

/// The bordered, rounded block used by every panel.
#[must_use]
pub fn rounded_block<'a>() -> Block<'a> {
    Block::bordered().border_type(BorderType::Rounded)
}

/// The block of a dialog: rounded border, bold title, one column of padding.
/// Dialogs paint no background of their own (the area is cleared first), so
/// they use the terminal's colours like everything else.
#[must_use]
pub fn dialog_block(title: &str) -> Block<'_> {
    rounded_block()
        .padding(Padding::horizontal(1))
        .title(Span::styled(format!(" {title} "), theme::bold()))
}

/// Area of a dialog `width`×`height` cells big (borders included), centred in
/// `area` and shrunk to fit it.
#[must_use]
pub fn modal_rect(area: Rect, width: u16, height: u16) -> Rect {
    area.centered(Constraint::Max(width), Constraint::Max(height))
}

/// Clear `rect` and draw `block` there; returns the area inside it.
pub fn open_dialog(frame: &mut Frame, rect: Rect, block: Block) -> Rect {
    frame.render_widget(Clear, rect);
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    inner
}

/// "Enter save · Esc cancel": keys highlighted, hints separated by dots
#[must_use]
pub fn hint_line<'a>(hints: &[(&'a str, &'a str)]) -> Line<'a> {
    let mut spans = Vec::with_capacity(hints.len() * 4);
    for (i, (key, label)) in hints.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled(" · ", theme::dim()));
        }
        spans.push(Span::styled(*key, theme::key()));
        spans.push(Span::raw(" "));
        spans.push(Span::raw(*label));
    }
    Line::from(spans)
}

/// Key hints for the bottom border of a dialog, right-aligned with a space
/// before the corner
#[must_use]
pub fn border_hints<'a>(hints: &[(&'a str, &'a str)]) -> Line<'a> {
    let mut line = hint_line(hints);
    line.spans.insert(0, Span::raw(" "));
    line.spans.push(Span::raw(" "));
    line.alignment(Alignment::Right)
}

/// Draw a dialog sized to its text: `lines`, a blank line and the `hints`
/// ("Enter delete · Esc cancel"). Lines are never wrapped, so every line,
/// including the hints, is visible whenever the terminal is tall enough.
pub fn render_text_dialog<'a>(
    frame: &mut Frame,
    area: Rect,
    title: &str,
    mut lines: Vec<Line<'a>>,
    hints: &[(&'a str, &'a str)],
) {
    if !hints.is_empty() {
        lines.push(Line::from(""));
        lines.push(hint_line(hints));
    }
    let content_width = lines
        .iter()
        .map(Line::width)
        .chain([text_width(title) + 2])
        .max()
        .unwrap_or(0);
    // Borders and padding take 4 columns, the borders 2 rows
    let width = u16::try_from(content_width + 4)
        .unwrap_or(u16::MAX)
        .min(MAX_DIALOG_WIDTH);
    let height = u16::try_from(lines.len() + 2).unwrap_or(u16::MAX);
    let rect = modal_rect(area, width, height);
    let inner = open_dialog(frame, rect, dialog_block(title));
    // Short of rows: drop lines above the hints rather than the hints
    let rows = usize::from(inner.height);
    if lines.len() > rows && rows > 0 && !hints.is_empty() {
        let hints = lines.pop();
        lines.truncate(rows - 1);
        lines.extend(hints);
    }
    frame.render_widget(Paragraph::new(lines), inner);
}

/// "Label  value" rows for detail and confirmation dialogs, labels aligned;
/// a `placeholder` value is dimmed.
#[must_use]
pub fn detail_lines<'a>(rows: &[(&'a str, &'a str, bool)]) -> Vec<Line<'a>> {
    let label_width = rows.iter().map(|(l, ..)| text_width(l)).max().unwrap_or(0);
    rows.iter()
        .map(|&(label, value, placeholder)| {
            let pad = label_width - text_width(label) + 2;
            Line::from(vec![
                Span::styled(label, theme::dim()),
                Span::raw(" ".repeat(pad)),
                Span::styled(
                    value,
                    if placeholder {
                        theme::dim().add_modifier(Modifier::ITALIC)
                    } else {
                        theme::text()
                    },
                ),
            ])
        })
        .collect()
}

/// Draw a vertical scrollbar over the right border of a bordered widget.
///
/// `area` is the widget's outer area (with borders), `header_rows` the rows
/// inside the border that do not scroll (e.g. a table header), `total` the
/// number of scrollable rows and `offset` the first visible one (as reported by
/// the widget's `ListState`/`TableState` after rendering). Nothing is drawn when
/// everything fits.
pub fn render_scrollbar(
    frame: &mut Frame,
    area: Rect,
    header_rows: u16,
    total: usize,
    offset: usize,
) {
    let track = Rect {
        y: area.y.saturating_add(1).saturating_add(header_rows),
        height: area.height.saturating_sub(2).saturating_sub(header_rows),
        ..area
    };
    let viewport = usize::from(track.height);
    if viewport == 0 || total <= viewport {
        return;
    }
    let max_offset = total - viewport;
    let mut state = ScrollbarState::new(max_offset + 1)
        .position(offset.min(max_offset))
        .viewport_content_length(viewport);
    let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
        .symbols(symbols::scrollbar::VERTICAL)
        .begin_symbol(None)
        .end_symbol(None)
        .thumb_style(theme::accent());
    frame.render_stateful_widget(scrollbar, track, &mut state);
}

// ============================================================================
// One-row forms
// ============================================================================

/// The value part of a form row
pub enum FieldValue<'a> {
    /// An editable text field; `placeholder` is shown (dimmed) while it is empty
    Text {
        input: &'a Input,
        placeholder: Cow<'a, str>,
    },
    /// A value that is not typed (a selector or a checkbox), drawn as given
    Fixed(Line<'a>),
}

/// One row of a form: marker, label column, value, optional trailing hint
pub struct FormField<'a> {
    pub label: &'a str,
    pub value: FieldValue<'a>,
    /// Shown at the right end of the row (a key hint or a validation mark)
    pub trailing: Option<Span<'a>>,
    pub focused: bool,
}

/// Width of a form's label column
fn label_column(fields: &[FormField]) -> u16 {
    let widest = fields
        .iter()
        .map(|f| text_width(f.label))
        .max()
        .unwrap_or(0);
    u16::try_from(widest).unwrap_or(u16::MAX)
}

/// Columns a form needs to show every label and `value_width` of value
#[must_use]
pub fn form_width(fields: &[FormField], value_width: u16) -> u16 {
    // marker + label + gap + value
    1 + label_column(fields) + 2 + value_width
}

/// Draw `fields` one per row into `area`. The focused row gets a marker, a
/// bold label and the cursor. When `area` is shorter than the form, the rows
/// scroll so the focused one stays visible.
pub fn render_form(frame: &mut Frame, area: Rect, fields: &[FormField]) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    let label_width = label_column(fields);
    let rows = usize::from(area.height);
    let focused = fields.iter().position(|f| f.focused).unwrap_or(0);
    let first = focused.saturating_sub(rows - 1);

    for (row, field) in fields.iter().skip(first).take(rows).enumerate() {
        let y = area.y + u16::try_from(row).unwrap_or(u16::MAX);
        let row_area = Rect::new(area.x, y, area.width, 1);
        let trailing_width = field
            .trailing
            .as_ref()
            .map_or(0, |s| u16::try_from(s.width() + 1).unwrap_or(u16::MAX));
        let [marker, label, value, trailing] = Layout::horizontal([
            Constraint::Length(1),
            Constraint::Length(label_width + 2),
            Constraint::Fill(1),
            Constraint::Length(trailing_width),
        ])
        .areas(row_area);

        if field.focused {
            frame.render_widget(Span::styled("▌", theme::focus()), marker);
        }
        let label_style = if field.focused {
            theme::bold()
        } else {
            theme::dim()
        };
        frame.render_widget(Span::styled(field.label, label_style), label);
        if let Some(span) = &field.trailing {
            frame.render_widget(
                Line::from(span.clone()).alignment(Alignment::Right),
                trailing,
            );
        }

        match &field.value {
            FieldValue::Fixed(line) => frame.render_widget(line.clone(), value),
            FieldValue::Text { input, placeholder } => {
                if input.value().is_empty() {
                    frame.render_widget(
                        Span::styled(
                            placeholder.as_ref(),
                            theme::dim().add_modifier(Modifier::ITALIC),
                        ),
                        value,
                    );
                    if field.focused {
                        frame.set_cursor_position((value.x, value.y));
                    }
                } else {
                    render_input_text(frame, value, input, field.focused);
                }
            }
        }
    }
}

/// Draw the text of `input` scrolled to keep the cursor in `area` (one row),
/// placing the terminal cursor if `focused`.
fn render_input_text(frame: &mut Frame, area: Rect, input: &Input, focused: bool) {
    let width = usize::from(area.width.max(1));
    // Leave the last column for the cursor at the end of the text
    let scroll = input.visual_scroll(width.saturating_sub(1).max(1));
    let scroll_u16 = u16::try_from(scroll).unwrap_or(u16::MAX);
    frame.render_widget(Paragraph::new(input.value()).scroll((0, scroll_u16)), area);
    if focused {
        let offset = input.visual_cursor().saturating_sub(scroll);
        let offset = u16::try_from(offset).unwrap_or(u16::MAX);
        frame.set_cursor_position((area.x + offset.min(area.width.saturating_sub(1)), area.y));
    }
}

/// A checkbox value: "[x] label" / "[ ] label"
#[must_use]
pub fn checkbox(checked: bool, label: &str) -> Line<'_> {
    let (mark, style) = if checked {
        ("[x] ", theme::success())
    } else {
        ("[ ] ", theme::dim())
    };
    Line::from(vec![Span::styled(mark, style), Span::raw(label)])
}

// ============================================================================
// Display-width text helpers
// ============================================================================

/// Display width of one character (0 for control characters)
#[must_use]
pub fn char_width(c: char) -> usize {
    let mut buf = [0u8; 4];
    Span::raw(&*c.encode_utf8(&mut buf)).width()
}

/// Display width of `text` in terminal columns
#[must_use]
pub fn text_width(text: &str) -> usize {
    Span::raw(text).width()
}

/// `text` cut to at most `max` columns, ending in `…` when it was cut.
#[must_use]
pub fn ellipsize(text: &str, max: usize) -> Cow<'_, str> {
    if text_width(text) <= max {
        return Cow::Borrowed(text);
    }
    if max == 0 {
        return Cow::Borrowed("");
    }
    let mut out = String::new();
    let mut width = 0;
    for c in text.chars() {
        let w = char_width(c);
        if width + w > max - 1 {
            break;
        }
        out.push(c);
        width += w;
    }
    // "Sony …" reads worse than "Sony…"
    out.truncate(out.trim_end().len());
    out.push('…');
    Cow::Owned(out)
}

/// The column widths a `Table` with these `constraints`, default flex and
/// spacing gives its columns in `width` columns (after `selection` columns for
/// the highlight symbol); used to cut cell text with [`ellipsize`] instead of
/// letting the table clip it.
#[must_use]
pub fn column_widths(constraints: &[Constraint], width: u16, selection: u16) -> Vec<usize> {
    let area = Rect::new(0, 0, width.saturating_sub(selection), 1);
    Layout::horizontal(constraints.iter().copied())
        .flex(Flex::Start)
        .spacing(1)
        .split(area)
        .iter()
        .map(|r| usize::from(r.width))
        .collect()
}

/// Rows for a picker list: descriptions in one column, node names (dimmed) in
/// a second, aligned, both cut to fit `width` columns together
#[must_use]
pub fn picker_lines(entries: &[(&str, &str)], width: usize) -> Vec<Line<'static>> {
    let widest_desc = entries
        .iter()
        .map(|(d, _)| text_width(d))
        .max()
        .unwrap_or(0);
    let widest_node = entries
        .iter()
        .map(|(_, n)| text_width(n))
        .max()
        .unwrap_or(0);
    let node_col = widest_node.min(width / 2);
    let desc_col = widest_desc.min(width.saturating_sub(node_col + 2));
    let node_room = width.saturating_sub(desc_col + 2);
    entries
        .iter()
        .map(|(desc, node)| {
            let desc = ellipsize(desc, desc_col);
            let pad = desc_col.saturating_sub(text_width(&desc)) + 2;
            Line::from(vec![
                Span::raw(format!("{desc}{}", " ".repeat(pad))),
                Span::styled(
                    truncate_node_name(node, node_room).into_owned(),
                    theme::dim(),
                ),
            ])
        })
        .collect()
}

/// Shorten a `PipeWire` node name to `max` columns, keeping what tells nodes
/// apart:
///
/// - ALSA (`alsa_output.pci-0000_00_1f.3.analog-stereo`): prefix and profile,
///   `alsa_output…analog-stereo`
/// - Bluetooth (`bluez_output.40_ED_98_1C_1D_08.1`): prefix, last two address
///   octets and device number, `bluez_output…1D_08.1`
/// - anything else, or when those still do not fit: cut with [`ellipsize`]
#[must_use]
pub fn truncate_node_name(text: &str, max: usize) -> Cow<'_, str> {
    if text_width(text) <= max {
        return Cow::Borrowed(text);
    }
    let parts: Vec<&str> = text.split('.').collect();
    let shortened = if (text.starts_with("alsa_output.") || text.starts_with("alsa_input."))
        && parts.len() >= 3
    {
        Some(format!("{}…{}", parts[0], parts[parts.len() - 1]))
    } else if (text.starts_with("bluez_output.") || text.starts_with("bluez_input."))
        && parts.len() >= 3
    {
        let mac = parts[1];
        // Last two octets ("1D_08"), on a character boundary
        let mac_suffix = mac
            .char_indices()
            .rev()
            .nth(4)
            .map_or(mac, |(i, _)| &mac[i..]);
        Some(format!("{}…{mac_suffix}.{}", parts[0], parts[2]))
    } else {
        None
    };
    match shortened {
        Some(s) if text_width(&s) <= max => Cow::Owned(s),
        Some(s) => Cow::Owned(ellipsize(&s, max).into_owned()),
        None => ellipsize(text, max),
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn test_ellipsize_by_display_width() {
        assert_eq!(ellipsize("short", 10), "short");
        assert_eq!(ellipsize("this is long", 8), "this is…");
        assert_eq!(ellipsize("abc", 3), "abc");
        assert_eq!(ellipsize("abcdef", 3), "ab…");
        assert_eq!(ellipsize("abcdef", 1), "…");
        assert_eq!(ellipsize("abcdef", 0), "");
        // Wide characters count two columns and are never split
        assert_eq!(ellipsize("日本語テキスト", 7), "日本語…");
        assert_eq!(ellipsize("日本語テキスト", 6), "日本…");
        assert_eq!(text_width(&ellipsize("🎵 music player", 6)), 6);
        assert_eq!(ellipsize("✳ sparkle", 5), "✳ sp…");
        assert_eq!(
            ellipsize("Sony WH-1000XM4", 6),
            "Sony…",
            "no space before …"
        );
    }

    #[test]
    fn test_truncate_node_name_keeps_what_tells_nodes_apart() {
        assert_eq!(truncate_node_name("short", 35), "short");
        assert_eq!(
            truncate_node_name("alsa_output.pci-0000_0c_00.4.analog-stereo", 35),
            "alsa_output…analog-stereo"
        );
        assert_eq!(
            truncate_node_name("alsa_input.pci-0000_0c_00.4.analog-stereo", 35),
            "alsa_input…analog-stereo"
        );
        assert_eq!(
            truncate_node_name("bluez_output.40_ED_98_1C_1D_08.1", 30),
            "bluez_output…1D_08.1"
        );
        assert_eq!(
            truncate_node_name("bluez_input.A0_B1_C2_D3_E4_F5.2", 30),
            "bluez_input…E4_F5.2"
        );
        assert_eq!(
            truncate_node_name("bluez_output.ÄÖ_ÜÄ_ÖÜ_ÄÖ_ÜÄ_ÖÜ.1", 25),
            "bluez_output…ÜÄ_ÖÜ.1"
        );
        assert_eq!(
            truncate_node_name("some_very_long_sink_name_that_needs_truncation", 20),
            "some_very_long_sink…"
        );
        // Never wider than asked, whatever the shape
        for name in [
            "alsa_output.pci-0000_0c_00.4.iec958-ac3-surround-51-analog-stereo",
            "bluez_output.40_ED_98_1C_1D_08.1",
            "alsa_output.device.test✓profile✳suffix",
            "device✳with✓special★chars",
        ] {
            for max in [1, 5, 12, 20] {
                assert!(
                    text_width(&truncate_node_name(name, max)) <= max,
                    "{name} {max}"
                );
            }
        }
    }

    #[test]
    fn test_column_widths_match_fill_constraints() {
        let widths = column_widths(
            &[
                Constraint::Length(3),
                Constraint::Fill(1),
                Constraint::Fill(1),
            ],
            30,
            2,
        );
        assert_eq!(widths.iter().sum::<usize>() + 2, 28);
        assert_eq!(widths[0], 3);
    }

    #[test]
    fn test_render_scrollbar_only_when_overflowing() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let draw = |total: usize, offset: usize| {
            let mut terminal = Terminal::new(TestBackend::new(10, 6)).expect("terminal");
            terminal
                .draw(|f| {
                    f.render_widget(rounded_block(), f.area());
                    render_scrollbar(f, f.area(), 0, total, offset);
                })
                .expect("draw");
            let buffer = terminal.backend().buffer().clone();
            (1..5)
                .map(|y| buffer[(9, y)].symbol().to_string())
                .collect::<Vec<_>>()
        };
        // Fits: plain border
        assert_eq!(draw(4, 0), vec!["│"; 4]);
        // Overflowing, scrolled to the end: thumb at the bottom
        let bottom = draw(8, 4);
        assert_eq!(bottom[3], "█", "{bottom:?}");
        assert_eq!(bottom[0], "│", "{bottom:?}");
        // At the top: thumb at the top
        let top = draw(8, 0);
        assert_eq!(top[0], "█", "{top:?}");
        assert_eq!(top[3], "│", "{top:?}");
    }
}
