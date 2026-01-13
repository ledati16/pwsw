//! Shared TUI widget helpers

use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph},
};
use tui_input::Input;

use crate::style::colors;

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
    let [_, row, _] = Layout::vertical([
        Constraint::Percentage((100 - percent_y) / 2),
        Constraint::Percentage(percent_y),
        Constraint::Percentage((100 - percent_y) / 2),
    ])
    .areas(r);

    let [_, center, _] = Layout::horizontal([
        Constraint::Percentage((100 - percent_x) / 2),
        Constraint::Percentage(percent_x),
        Constraint::Percentage((100 - percent_x) / 2),
    ])
    .areas(row);

    center
}

/// Standard modal sizes (width%, height%)
pub mod modal_size {
    /// Small modals - confirmations (50%x40%)
    pub(crate) const SMALL: (u16, u16) = (50, 40);
    pub(crate) const MEDIUM: (u16, u16) = (70, 65);
    pub(crate) const LARGE: (u16, u16) = (80, 85);
    pub(crate) const DROPDOWN: (u16, u16) = (40, 50);
    pub(crate) const HELP: (u16, u16) = (90, 80);
}

/// Helper to create centered modal with standard size
///
/// Returns the calculated `Rect` for the modal area. Callers must use this rect
/// to render the modal widget.
#[must_use]
pub(crate) fn centered_modal(size: (u16, u16), r: Rect) -> Rect {
    centered_rect(size.0, size.1, r)
}

/// Get focus-aware border style
///
/// Returns magenta (bold) border for focused elements, gray for unfocused.
/// This provides consistent visual feedback across all TUI widgets.
pub(crate) const fn focus_border_style(focused: bool) -> Style {
    if focused {
        Style::new().fg(colors::UI_FOCUS)
    } else {
        Style::new().fg(colors::UI_SECONDARY)
    }
}

/// Validation state for input fields
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ValidationState {
    /// Input is valid
    Valid,
    /// Input is invalid
    Invalid,
    /// Input has not been validated or is in neutral state
    Neutral,
}

/// Render a text input field with validation-aware border colors
///
/// Shows green border for valid input, red for invalid, magenta for focused.
/// If not focused and validation state is provided, shows green/red for valid/invalid.
pub(crate) fn render_validated_input(
    frame: &mut Frame,
    area: Rect,
    title: &str,
    input: &Input,
    focused: bool,
    validation: ValidationState,
) {
    let border_style = if focused {
        // Focused always gets magenta border
        Style::new().fg(colors::UI_FOCUS)
    } else {
        // When not focused, show validation state
        match validation {
            ValidationState::Valid => Style::new().fg(colors::UI_VALID),
            ValidationState::Invalid => Style::new().fg(colors::UI_INVALID),
            ValidationState::Neutral => Style::new().fg(colors::UI_SECONDARY),
        }
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
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

/// Render a text input field with a block and correct scrolling/cursor
pub(crate) fn render_input(
    frame: &mut Frame,
    area: Rect,
    title: &str,
    input: &Input,
    focused: bool,
) {
    render_validated_input(frame, area, title, input, focused, ValidationState::Neutral);
}

/// Render a button-like selector widget
///
/// Creates a bordered widget that looks clickable with clear selection state.
/// When focused, shows cyan border and "(Space to select)" hint.
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
                .fg(colors::UI_HIGHLIGHT)
                .add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(
            display_value,
            Style::default()
                .fg(colors::UI_HIGHLIGHT)
                .add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(
            " ]",
            Style::default()
                .fg(colors::UI_HIGHLIGHT)
                .add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(
            " ◄ Space to select",
            Style::default().fg(colors::UI_WARNING),
        ));
    } else {
        // Unfocused: Show as button with subtle dropdown arrow
        spans.push(Span::styled(
            "↓ [ ",
            Style::default().fg(colors::UI_SECONDARY),
        ));
        spans.push(Span::raw(display_value));
        spans.push(Span::styled(
            " ]",
            Style::default().fg(colors::UI_SECONDARY),
        ));
    }

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(border_style);

    let paragraph = Paragraph::new(Line::from(spans)).block(block);
    frame.render_widget(paragraph, area);
}

/// Truncate a description string to `max_width` characters, appending `...` when truncated.
///
/// This is a visual truncation helper for UI rendering. It operates on character counts
/// (not grapheme clusters) which is acceptable for typical sink descriptions.
/// Uses character-based truncation (not byte-based) to safely handle UTF-8 strings.
pub(crate) fn truncate_desc(text: &str, max_width: u16) -> String {
    let max = max_width as usize;
    let char_count = text.chars().count();
    if char_count <= max {
        text.to_string()
    } else if max <= 3 {
        text.chars().take(max).collect()
    } else {
        let take = max.saturating_sub(3);
        let truncated: String = text.chars().take(take).collect();
        format!("{truncated}...")
    }
}

/// Truncate a node/sink name intelligently to fit within display width
///
/// Handles different node types with appropriate truncation strategies:
///
/// **ALSA nodes** (e.g., `alsa_output.pci-0000_00_1f.3.analog-stereo`):
/// - Keeps prefix and profile suffix: `alsa_output...analog-stereo`
/// - If still too long, truncates suffix: `alsa_output...iec958-ac3-surround...`
///
/// **Bluetooth nodes** (e.g., `bluez_output.40_ED_98_1C_1D_08.1`):
/// - Keeps prefix, last 2 MAC octets, and device number: `bluez_output...1D_08.1`
/// - Format: `{prefix}...{last_2_mac_octets}.{device_num}`
///
/// **Other nodes**: Uses simple truncation with ellipsis.
pub(crate) fn truncate_node_name(text: &str, max_width: u16) -> String {
    let max_len = max_width as usize;

    if text.len() <= max_len {
        return text.to_string();
    }

    // For ALSA nodes, use intelligent truncation (prefix...suffix)
    if text.starts_with("alsa_output.") || text.starts_with("alsa_input.") {
        let parts: Vec<&str> = text.split('.').collect();
        if parts.len() >= 3 {
            let prefix = parts[0];
            let suffix = parts[parts.len() - 1];
            let combined = format!("{prefix}...{suffix}");

            // If the intelligent format fits, use it
            if combined.len() <= max_len {
                return combined;
            }

            // Otherwise, truncate the suffix to fit within max_width
            // Format: "prefix...truncated_suffix..."
            let prefix_len = prefix.len();
            let ellipsis_len = 3; // "..."
            let available_for_suffix =
                max_len.saturating_sub(prefix_len + ellipsis_len + ellipsis_len);

            if available_for_suffix > 3 {
                let suffix_char_count = suffix.chars().count();
                let truncated_suffix: String = suffix
                    .chars()
                    .take(available_for_suffix.min(suffix_char_count))
                    .collect();
                return format!("{prefix}...{truncated_suffix}...");
            }

            // If we can't fit anything meaningful, just use prefix
            return format!("{prefix}...");
        }
    }

    // For Bluetooth nodes, use intelligent truncation (prefix...last_mac_octets.device_num)
    // Format: bluez_output.XX_XX_XX_XX_XX_XX.N -> bluez_output...XX_XX.N
    if text.starts_with("bluez_output.") || text.starts_with("bluez_input.") {
        let parts: Vec<&str> = text.split('.').collect();
        if parts.len() >= 3 {
            let prefix = parts[0]; // "bluez_output" or "bluez_input"
            let mac = parts[1]; // "40_ED_98_1C_1D_08"
            let device_num = parts[2]; // "1"

            // Extract last 2 octets from MAC (last 5 chars: "1D_08")
            let mac_suffix = if mac.len() >= 5 {
                &mac[mac.len() - 5..]
            } else {
                mac
            };

            let combined = format!("{prefix}...{mac_suffix}.{device_num}");

            // If the intelligent format fits, use it
            if combined.len() <= max_len {
                return combined;
            }

            // If still too long, just show prefix and device number
            return format!("{prefix}...{device_num}");
        }
    }

    // Fallback to simple truncation for other node types
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
        let p = Paragraph::new(Span::styled("↑", Style::default().fg(colors::UI_WARNING)));
        frame.render_widget(p, r);
    }
    if has_below {
        let r = Rect {
            x: inner.x + inner.width.saturating_sub(2),
            y: inner.y + inner.height.saturating_sub(1),
            width: 1,
            height: 1,
        };
        let p = Paragraph::new(Span::styled("↓", Style::default().fg(colors::UI_WARNING)));
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

    #[test]
    fn test_truncate_node_name_no_truncation_needed() {
        // Short names should pass through unchanged
        assert_eq!(truncate_node_name("short", 35), "short");
        assert_eq!(
            truncate_node_name("alsa_output.analog-stereo", 35),
            "alsa_output.analog-stereo"
        );
    }

    #[test]
    fn test_truncate_node_name_alsa_intelligent() {
        // ALSA nodes should use intelligent truncation (prefix...suffix)
        let alsa_long = "alsa_output.pci-0000_0c_00.4.analog-stereo";
        assert_eq!(
            truncate_node_name(alsa_long, 35),
            "alsa_output...analog-stereo"
        );

        // ALSA input nodes
        let alsa_input = "alsa_input.pci-0000_0c_00.4.analog-stereo";
        assert_eq!(
            truncate_node_name(alsa_input, 35),
            "alsa_input...analog-stereo"
        );

        // Very long profile names should be truncated further
        let alsa_very_long = "alsa_output.pci-0000_0c_00.4.iec958-ac3-surround-51-analog-stereo";
        let result = truncate_node_name(alsa_very_long, 35);
        assert!(result.starts_with("alsa_output..."));
        assert!(result.len() <= 35);
    }

    #[test]
    fn test_truncate_node_name_bluetooth_intelligent() {
        // Bluetooth nodes should keep prefix, last 2 MAC octets, and device number when truncated
        // This node is 33 chars, so with max=30 it will be truncated
        let bt_output = "bluez_output.40_ED_98_1C_1D_08.1";
        assert_eq!(truncate_node_name(bt_output, 30), "bluez_output...1D_08.1");

        let bt_input = "bluez_input.A0_B1_C2_D3_E4_F5.2";
        assert_eq!(truncate_node_name(bt_input, 30), "bluez_input...E4_F5.2");

        // If under the limit, no truncation
        assert_eq!(truncate_node_name(bt_output, 35), bt_output);
    }

    #[test]
    fn test_truncate_node_name_bluetooth_very_short_width() {
        // If even the intelligent format is too long, fallback to prefix + device num
        let bt_output = "bluez_output.40_ED_98_1C_1D_08.1";
        assert_eq!(truncate_node_name(bt_output, 20), "bluez_output...1");
    }

    #[test]
    fn test_truncate_node_name_other_types() {
        // Non-ALSA, non-Bluetooth nodes should use simple truncation
        let other = "some_very_long_sink_name_that_needs_truncation";
        let result = truncate_node_name(other, 20);
        assert_eq!(result, "some_very_long_si...");
        assert!(result.len() <= 20);
    }

    #[test]
    fn test_truncate_desc_utf8() {
        // Test UTF-8 multibyte characters (emoji, special symbols)
        // ✳ is 3 bytes, but 1 character
        assert_eq!(truncate_desc("✳ sparkle", 10), "✳ sparkle");
        assert_eq!(truncate_desc("✳ sparkle", 5), "✳ ..."); // max=5: take 2 chars + "..."

        // Emoji test (4-byte characters)
        assert_eq!(truncate_desc("🎵 music", 10), "🎵 music");
        assert_eq!(truncate_desc("🎵 music", 4), "🎵...");

        // Mixed ASCII and UTF-8
        let mixed = "device ✓ ready";
        assert_eq!(truncate_desc(mixed, 20), "device ✓ ready");
        assert_eq!(truncate_desc(mixed, 10), "device ..."); // max=10: take 7 chars + "..." ("device " = 7 chars)

        // Edge case: exactly at boundary
        assert_eq!(truncate_desc("test✳", 5), "test✳");
        assert_eq!(truncate_desc("test✳", 4), "t...");
    }

    #[test]
    fn test_truncate_node_name_utf8() {
        // Test ALSA node with UTF-8 in profile suffix (unlikely but possible)
        // This tests the suffix truncation path with UTF-8
        let alsa_utf8 = "alsa_output.device.test✓profile✳suffix";
        let result = truncate_node_name(alsa_utf8, 20);
        // Should use ALSA intelligent truncation
        assert!(result.starts_with("alsa_output..."));
        assert!(result.len() <= 20);

        // Fallback path with UTF-8
        let generic_utf8 = "device✳with✓special★chars";
        let result = truncate_node_name(generic_utf8, 15);
        assert_eq!(result, "device✳with✓...");
    }
}
