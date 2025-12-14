//! Shared TUI widget helpers

use ratatui::layout::{Constraint, Direction, Layout, Rect};

/// Helper to create centered rect for modals
///
/// Creates a centered rectangle with the given percentage of screen width/height.
/// Used for modal dialogs and overlays.
///
/// # Arguments
/// * `percent_x` - Width as percentage of screen (0-100)
/// * `percent_y` - Height as percentage of screen (0-100)
/// * `r` - The area to center within
pub fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
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
