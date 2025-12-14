//! Shared text-field helpers (clipping, cursor-aware rendering)

use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::Line,
    widgets::Paragraph,
    Frame,
};

/// Helper: compute displayed substring and cursor relative index for text field clipping.
/// Returns (display_string, cursor_relative_index, truncated_left).
pub fn compute_display_window(
    value: &str,
    cursor: usize,
    max_value_len: usize,
) -> (String, usize, bool, usize) {
    if max_value_len == 0 {
        return (String::new(), 0, false, 0);
    }

    let chars: Vec<char> = value.chars().collect();
    let len = chars.len();
    let cursor = cursor.min(len);

    if len <= max_value_len {
        return (chars.into_iter().collect(), cursor, false, 0);
    }

    let half = max_value_len / 2;
    let start = if cursor <= half {
        0
    } else if cursor + half >= len {
        len.saturating_sub(max_value_len)
    } else {
        cursor.saturating_sub(half)
    };

    // When truncated on the left we reserve one slot for ellipsis
    let mut take = max_value_len;
    if start > 0 && take > 0 {
        if take > 1 {
            take -= 1;
        } else {
            take = 0;
        }
    }

    let display_chars: String = chars.iter().skip(start).take(take).collect();
    let displayed_len = display_chars.chars().count();
    let cursor_rel = if cursor <= start {
        0
    } else if cursor >= start + displayed_len {
        displayed_len
    } else {
        cursor - start
    };
    let truncated_left = start > 0;
    (display_chars, cursor_rel, truncated_left, start)
}

/// Render text field (cursor-aware, clipping, ellipsis)
pub fn render_text_field(
    frame: &mut Frame,
    area: Rect,
    label: &str,
    value: &str,
    focused: bool,
    cursor_pos: Option<usize>,
) {
    // Build spans for label, value and cursor to avoid a single allocation and allow clipping.
    let label_span =
        ratatui::text::Span::styled(format!("{} ", label), Style::default().fg(Color::Gray));
    let value_style = if focused {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::White)
    };

    // Compute available width for value (area.width is u16)
    let area_width = area.width as usize;
    // Rough estimate of label width in chars
    let label_len = label.len() + 1; // including space
    let mut max_value_len = area_width.saturating_sub(label_len);

    // Reserve one char for cursor when focused
    if focused && max_value_len > 0 {
        if max_value_len > 1 {
            max_value_len -= 1
        } else {
            max_value_len = 0
        }
    }

    // Use compute_display_window helper to compute displayed substring and cursor relative index
    let cursor = cursor_pos.unwrap_or_else(|| value.chars().count());
    let (display_substr, cursor_rel, truncated_left, _start) =
        compute_display_window(value, cursor, max_value_len);

    let mut spans = vec![label_span];

    if truncated_left {
        spans.push(ratatui::text::Span::raw("…"));
    }

    // left part
    let left: String = display_substr.chars().take(cursor_rel).collect();
    spans.push(ratatui::text::Span::styled(left, value_style));

    if focused {
        let cur_char = display_substr.chars().nth(cursor_rel).unwrap_or(' ');
        spans.push(ratatui::text::Span::styled(
            cur_char.to_string(),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ));
    }

    // right part
    let right_start = cursor_rel + if focused { 1 } else { 0 };
    if right_start <= display_substr.chars().count() {
        let right: String = display_substr.chars().skip(right_start).collect();
        spans.push(ratatui::text::Span::styled(right, value_style));
    }

    let paragraph = Paragraph::new(Line::from(spans));
    frame.render_widget(paragraph, area);
}
