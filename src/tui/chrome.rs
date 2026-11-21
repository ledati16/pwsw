//! The one-row header and footer around every screen.
//!
//! Header: `pwsw <version>`, the screen tabs, the paused and unsaved-changes
//! markers and the help key. Footer: the status message (icon and colour by kind), a spinner
//! while background work runs, and key hints generated from
//! [`crate::tui::keymap::BINDINGS`] for what is on screen.

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};

use crate::tui::app::{Screen, Status, StatusKind};
use crate::tui::keymap::{Context, HintState, footer_hints};
use crate::tui::theme;
use crate::tui::widgets::{ellipsize, text_width};

/// The marker shown in the header while the config has unsaved changes
pub(crate) const UNSAVED_MARKER: &str = "● unsaved";

/// The marker shown in the header while the daemon's rule-driven switching is
/// paused
pub(crate) const PAUSED_MARKER: &str = "⏸ paused";
/// [`PAUSED_MARKER`] when the header has no room for the word
const PAUSED_MARKER_SHORT: &str = "⏸";

/// Frames of the footer spinner shown while background work runs
pub(crate) const SPINNER: [&str; 6] = ["⠷", "⠯", "⠟", "⠻", "⠽", "⠾"];

/// Columns between two footer hints
pub(crate) const HINT_GAP: &str = "  ";

/// Draw the header: brand, tabs, paused and unsaved markers, help key. When the
/// terminal is narrow the version goes first, then the help key, then the name,
/// then the word "paused" (its symbol stays).
pub(crate) fn render_header(
    frame: &mut Frame,
    area: Rect,
    current: Screen,
    unsaved: bool,
    paused: bool,
) {
    let mut tabs: Vec<Span> = Vec::new();
    for &screen in Screen::all() {
        let key = screen.key().to_string();
        if screen == current {
            tabs.push(Span::styled(
                format!(" {key} {} ", screen.name()),
                theme::selected().add_modifier(Modifier::BOLD),
            ));
        } else {
            tabs.push(Span::raw(" "));
            tabs.push(Span::styled(key, theme::key()));
            tabs.push(Span::raw(format!(" {} ", screen.name())));
        }
    }
    let tabs_width: usize = tabs.iter().map(Span::width).sum();

    let version = format!(" {}", crate::version_string());
    let unsaved_width = if unsaved {
        text_width(UNSAVED_MARKER) + 2
    } else {
        0
    };
    let help_width = text_width("? help") + 1;
    let width = usize::from(area.width);
    let paused_marker = if !paused {
        None
    } else if tabs_width + unsaved_width + text_width(PAUSED_MARKER) + 3 <= width {
        Some(PAUSED_MARKER)
    } else {
        Some(PAUSED_MARKER_SHORT)
    };
    let paused_width = paused_marker.map_or(0, |m| text_width(m) + 2);
    let mut used = tabs_width + unsaved_width + paused_width + 1;
    let show_name = used + 6 <= width;
    used += if show_name { 6 } else { 0 };
    let show_help = used + help_width <= width;
    used += if show_help { help_width } else { 0 };
    let show_version = show_name && used + text_width(&version) <= width;

    let mut left = Vec::new();
    if show_name {
        left.push(Span::styled(" pwsw", theme::bold()));
        if show_version {
            left.push(Span::styled(version, theme::dim()));
        }
        left.push(Span::raw(" "));
    }
    left.extend(tabs);

    let mut right = Vec::new();
    if let Some(marker) = paused_marker {
        right.push(Span::styled(marker, theme::warning()));
        right.push(Span::raw("  "));
    }
    if unsaved {
        right.push(Span::styled(UNSAVED_MARKER, theme::warning()));
        right.push(Span::raw("  "));
    }
    if show_help {
        right.push(Span::styled("?", theme::key()));
        right.push(Span::raw(" help "));
    }
    let right = Line::from(right);
    let right_width = u16::try_from(right.width()).unwrap_or(u16::MAX);
    let [left_area, right_area] =
        Layout::horizontal([Constraint::Fill(1), Constraint::Length(right_width)]).areas(area);
    frame.render_widget(Line::from(left), left_area);
    frame.render_widget(right.alignment(Alignment::Right), right_area);
}

/// Icon and style of a status message
fn status_look(kind: StatusKind) -> (&'static str, ratatui::style::Style) {
    match kind {
        StatusKind::Info => ("•", theme::accent()),
        StatusKind::Success => ("✓", theme::success()),
        StatusKind::Warning => ("⚠", theme::warning()),
        StatusKind::Error => ("✗", theme::error()),
    }
}

/// Everything the footer shows
pub(crate) struct Footer<'a> {
    pub(crate) status: Option<&'a Status>,
    /// The spinner frame to show (`None`: nothing is running)
    pub(crate) spinner: Option<&'static str>,
    pub(crate) contexts: &'a [Context],
    /// What decides the hints besides the contexts (unsaved changes, daemon state)
    pub(crate) hints: HintState,
}

/// Draw the footer: spinner, status message on the left, key hints on the
/// right in whatever room the message leaves (hints are dropped whole, least
/// important first).
pub(crate) fn render_footer(frame: &mut Frame, area: Rect, footer: &Footer) {
    let mut x = area.x;
    let mut width = usize::from(area.width);
    if let Some(spinner) = footer.spinner
        && width >= 2
    {
        frame.render_widget(
            Span::styled(spinner, theme::warning()),
            Rect::new(x, area.y, 2, 1),
        );
        x += 2;
        width -= 2;
    } else if width >= 1 {
        x += 1;
        width -= 1;
    }

    let mut status_width = 0;
    if let Some(status) = footer.status {
        let (icon, style) = status_look(status.kind);
        // The last column stays free (the hints' trailing space lands there), so a
        // clipped message keeps its ellipsis
        let room = width.saturating_sub(text_width(icon) + 2);
        let text = ellipsize(&status.text, room).into_owned();
        let line = Line::from(vec![
            Span::styled(icon, style),
            Span::raw(" "),
            Span::styled(text, style),
        ]);
        status_width = line.width();
        frame.render_widget(line, Rect::new(x, area.y, area.width - (x - area.x), 1));
    }

    // Hints after the message with a gap, never touching the right edge
    let reserved = if status_width > 0 {
        status_width + 3
    } else {
        1
    };
    let room = width.saturating_sub(reserved);
    let hints = footer_hints(footer.contexts, footer.hints, room);
    let mut spans = Vec::new();
    for (i, hint) in hints.iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw(HINT_GAP));
        }
        spans.push(Span::styled(hint.keys, theme::key()));
        spans.push(Span::raw(" "));
        spans.push(Span::styled(hint.label, theme::text()));
    }
    spans.push(Span::raw(" "));
    let line = Line::from(spans);
    let line_width = u16::try_from(line.width())
        .unwrap_or(u16::MAX)
        .min(area.width);
    let hints_area = Rect::new(area.right() - line_width, area.y, line_width, 1);
    frame.render_widget(line, hints_area);
}

/// Smallest terminal the TUI lays out for
pub(crate) const MIN_WIDTH: u16 = 60;
pub(crate) const MIN_HEIGHT: u16 = 16;

/// Whether `area` is too small for the layout
pub(crate) const fn too_small(area: Rect) -> bool {
    area.width < MIN_WIDTH || area.height < MIN_HEIGHT
}

/// Draw the "terminal too small" notice in place of the UI
pub(crate) fn render_too_small(frame: &mut Frame, area: Rect) {
    let lines = vec![
        Line::from(vec![
            Span::styled("Terminal too small", theme::bold()),
            Span::raw(format!(" — resize to at least {MIN_WIDTH}x{MIN_HEIGHT}")),
        ]),
        Line::from(Span::styled(
            format!("(now {}x{})", area.width, area.height),
            theme::dim(),
        )),
    ];
    crate::tui::widgets::render_centered(frame, area, lines);
}
