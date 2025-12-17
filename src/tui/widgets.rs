//! Shared TUI widget helpers

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};
use tui_input::Input;

/// Helper to create centered rect for modals
///
/// Creates a centered rectangle with the given percentage of screen width/height.
/// Used for modal dialogs and overlays.
///
/// # Arguments
/// * `percent_x` - Width as percentage of screen (0-100)
/// * `percent_y` - Height as percentage of screen (0-100)
/// * `r` - The area to center within
fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}

/// Standard modal sizes (width%, height%)
pub mod modal_size {
    /// Small modals - confirmations (50%x40%)
    pub(crate) const SMALL: (u16, u16) = (50, 40);
    pub(crate) const MEDIUM: (u16, u16) = (70, 65);
    pub(crate) const LARGE: (u16, u16) = (80, 85);
    pub(crate) const DROPDOWN: (u16, u16) = (40, 50);
    pub(crate) const HELP: (u16, u16) = (65, 75);
}

/// Helper to create centered modal with standard size
pub(crate) fn centered_modal(size: (u16, u16), r: Rect) -> Rect {
    centered_rect(size.0, size.1, r)
}

/// Get focus-aware border style
///
/// Returns cyan border for focused elements, dark gray for unfocused.
/// This provides consistent visual feedback across all TUI widgets.
pub(crate) const fn focus_border_style(focused: bool) -> Style {
    if focused {
        Style::new().fg(Color::Cyan)
    } else {
        Style::new().fg(Color::DarkGray)
    }
}

/// Render a text input field with a block and correct scrolling/cursor
pub(crate) fn render_input(
    frame: &mut Frame,
    area: Rect,
    title: &str,
    input: &Input,
    focused: bool,
) {
    let border_style = focus_border_style(focused);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title(title);

    let inner_area = block.inner(area);
    frame.render_widget(block, area);

    // Compute scrolling
    let width = inner_area.width.max(1) as usize;
    let scroll = input.visual_scroll(width);

    // Render input using Paragraph
    let scroll_u16 = u16::try_from(scroll).unwrap_or(u16::MAX);
    let p = Paragraph::new(input.value()).scroll((0, scroll_u16));
    frame.render_widget(p, inner_area);

    // Render cursor
    if focused {
        let cursor_offset = input.visual_cursor().max(scroll) - scroll;
        let cursor_offset_u16 = u16::try_from(cursor_offset).unwrap_or(u16::MAX);
        frame.set_cursor_position((inner_area.x + cursor_offset_u16, inner_area.y));
    }
}

/// Build modal help line with consistent formatting
///
/// Creates a help line with `[key] action | [key] action` format.
/// Uses static strings to avoid allocations in render path.
///
/// # Example
/// ```no_run
/// # use ratatui::text::Line;
/// # fn modal_help_line(items: &[(&'static str, &'static str)]) -> Line<'static> { Line::from("") }
/// modal_help_line(&[("Tab", "Next"), ("Esc", "Cancel")]);
/// ```
pub(crate) fn modal_help_line(items: &[(&'static str, &'static str)]) -> Line<'static> {
    let mut spans = Vec::new();
    for (i, (key, action)) in items.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled(" | ", Style::default().fg(Color::DarkGray)));
        }
        // Build "[key]" using three spans to avoid format!
        spans.push(Span::styled("[", Style::default().fg(Color::Cyan)));
        spans.push(Span::styled(*key, Style::default().fg(Color::Cyan)));
        spans.push(Span::styled("]", Style::default().fg(Color::Cyan)));
        spans.push(Span::raw(" "));
        spans.push(Span::raw(*action));
    }
    Line::from(spans)
}

/// Render a button-like selector widget
///
/// Creates a bordered widget that looks clickable with clear selection state.
/// When focused, shows cyan border and "(Enter to select)" hint.
///
/// # Arguments
/// * `frame` - Frame to render into
/// * `area` - Rect to render within
/// * `label` - Field label (e.g., "Target Sink")
/// * `value` - Current value, or None to show "Select..."
/// * `focused` - Whether this widget is currently focused
pub(crate) fn render_selector_button(
    frame: &mut Frame,
    area: Rect,
    label: &str,
    value: Option<&str>,
    focused: bool,
) {
    let border_style = focus_border_style(focused);

    // Build content spans without allocating intermediate strings
    let mut spans = vec![Span::raw(label), Span::raw(": ")];

    let display_value = value.unwrap_or("Select...");

    if focused {
        // Focused: Show as clickable button with dropdown arrow and hint
        spans.push(Span::styled(
            "↓ [ ",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(
            display_value,
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(
            " ]",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(
            " ◄ Enter to select",
            Style::default().fg(Color::Yellow),
        ));
    } else {
        // Unfocused: Show as button with subtle dropdown arrow
        spans.push(Span::styled("↓ [ ", Style::default().fg(Color::DarkGray)));
        spans.push(Span::raw(display_value));
        spans.push(Span::styled(" ]", Style::default().fg(Color::DarkGray)));
    }

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style);

    let paragraph = Paragraph::new(Line::from(spans)).block(block);
    frame.render_widget(paragraph, area);
}

/// Truncate a description string to `max_width` characters, appending `...` when truncated.
///
/// This is a visual truncation helper for UI rendering. It operates on character counts
/// (not grapheme clusters) which is acceptable for ASCII-based sink descriptions used here.
pub(crate) fn truncate_desc(text: &str, max_width: u16) -> String {
    let max = max_width as usize;
    if text.len() <= max {
        text.to_string()
    } else if max <= 3 {
        text[..max].to_string()
    } else {
        let take = max.saturating_sub(3);
        format!("{}...", &text[..take])
    }
}

/// Truncate a node/sink name similarly to `truncate_desc`.
pub(crate) fn truncate_node_name(text: &str, max_width: u16) -> String {
    truncate_desc(text, max_width)
}

/// Compute visual line counts for a list of items given `content_width`.
///
/// Returns a vector of per-item visual heights (in rows), accounting for wrapping at `content_width`.
fn compute_visual_line_counts(items: &[String], content_width: usize) -> Vec<usize> {
    let mut per_row_lines: Vec<usize> = Vec::with_capacity(items.len());
    for s in items {
        let w = content_width.max(1);
        let lines = (s.len().saturating_add(w - 1)) / w;
        per_row_lines.push(lines.max(1));
    }
    per_row_lines
}

/// Compute whether there is content above/below the current viewport
/// for a list of visual items that may wrap at `content_width`.
pub(crate) fn compute_has_above_below(
    items: &[String],
    content_width: usize,
    offset: usize,
    view_height: usize,
) -> (bool, bool) {
    let per_row_lines = compute_visual_line_counts(items, content_width);
    let total_visual_lines: usize = per_row_lines.iter().sum();
    let mut visual_pos = 0usize;
    for lines in per_row_lines.iter().take(offset.min(per_row_lines.len())) {
        visual_pos += *lines;
    }
    let has_above = visual_pos > 0;
    let has_below = (visual_pos + view_height) < total_visual_lines;
    (has_above, has_below)
}

/// Render small up/down arrows at the right edge of `inner` to indicate scroll.
pub(crate) fn render_scroll_arrows(
    frame: &mut Frame,
    inner: Rect,
    has_above: bool,
    has_below: bool,
) {
    if has_above {
        let r = Rect {
            x: inner.x + inner.width.saturating_sub(2),
            y: inner.y,
            width: 1,
            height: 1,
        };
        let p = Paragraph::new(Span::styled("↑", Style::default().fg(Color::Yellow)));
        frame.render_widget(p, r);
    }
    if has_below {
        let r = Rect {
            x: inner.x + inner.width.saturating_sub(2),
            y: inner.y + inner.height.saturating_sub(1),
            width: 1,
            height: 1,
        };
        let p = Paragraph::new(Span::styled("↓", Style::default().fg(Color::Yellow)));
        frame.render_widget(p, r);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_truncate_desc() {
        assert_eq!(truncate_desc("short", 10), "short");
        assert_eq!(truncate_desc("this is long", 8), "this ...");
        assert_eq!(truncate_desc("abc", 3), "abc");
        assert_eq!(truncate_desc("abcdef", 3), "abc");
    }

    #[test]
    fn test_compute_visual_line_counts() {
        let items = vec!["abcd".to_string(), "efghijkl".to_string()];
        // width=4 => first -> 1, second -> 2
        let counts = compute_visual_line_counts(&items, 4);
        assert_eq!(counts, vec![1usize, 2usize]);
    }
}
