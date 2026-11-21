//! Dashboard screen - Overview and quick actions

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Cell, Padding, Paragraph, Row, Table};

use crate::config::Config;
use crate::ipc::WindowInfo;
use crate::tui::theme;
use crate::tui::widgets::{
    char_width, column_widths, ellipsize, render_scrollbar, rounded_block, text_width,
    truncate_node_name,
};

/// Height of the daemon and "playing on" cards
const CARD_HEIGHT: u16 = 5;

/// Index of each daemon action in `DashboardScreen::selected_action`
const ACTION_START: usize = 0;
const ACTION_STOP: usize = 1;
const ACTION_RESTART: usize = 2;

/// Daemon action buttons, in selection order (the last two need the systemd unit)
const ACTIONS: [&str; 5] = ["Start", "Stop", "Restart", "Enable", "Disable"];

/// Dashboard view mode (toggle between Logs and Windows)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DashboardView {
    Logs,
    Windows,
}

/// Dashboard screen state
///
/// Scroll limits and page sizes depend on the viewport, so rendering records
/// them here and the input handlers use them.
#[derive(Debug)]
pub struct DashboardScreen {
    pub selected_action: usize, // 0-4: start, stop, restart, enable, disable
    /// Log lines scrolled back from the end (0 = live, following the newest)
    pub log_scroll_offset: usize,
    /// First window shown in the window list
    pub window_scroll_offset: usize,
    /// Selected row of the window list (in display order, see [`windows_in_display_order`])
    pub window_selected: usize,
    pub current_view: DashboardView, // Toggle between Logs and Windows
    pub max_action_index: usize,     // Maximum action index (2 without unit, 4 with)
    pub service_enabled: Option<bool>, // None without unit, Some(true/false) with it
    /// Rows of the log viewport at the last render
    log_viewport: usize,
    /// Largest useful `log_scroll_offset` at the last render
    log_max_offset: usize,
    /// Windows that fit on one page at the last render
    window_page: usize,
    /// Largest useful `window_scroll_offset` at the last render
    window_max_offset: usize,
    /// Windows in the list at the last render
    window_count: usize,
}

impl DashboardScreen {
    pub(crate) const fn new() -> Self {
        Self {
            selected_action: 0,
            log_scroll_offset: 0,
            window_scroll_offset: 0,
            window_selected: 0,
            current_view: DashboardView::Logs, // Default to logs
            max_action_index: 2,               // Default to 3 actions (start/stop/restart)
            service_enabled: None,             // Updated by background worker
            log_viewport: 10,
            log_max_offset: usize::MAX,
            window_page: 5,
            window_max_offset: usize::MAX,
            window_count: usize::MAX,
        }
    }

    /// Show enable/disable only if the systemd unit is installed
    pub(crate) const fn set_max_actions(&mut self, unit_installed: bool) {
        self.max_action_index = if unit_installed { 4 } else { 2 };
        // Clamp current selection if it's out of range
        if self.selected_action > self.max_action_index {
            self.selected_action = 0;
        }
    }

    pub(crate) const fn select_next(&mut self) {
        if self.selected_action < self.max_action_index {
            self.selected_action += 1;
        } else {
            self.selected_action = 0; // Wrap to first
        }
    }

    pub(crate) const fn select_previous(&mut self) {
        if self.selected_action > 0 {
            self.selected_action -= 1;
        } else {
            self.selected_action = self.max_action_index; // Wrap to last
        }
    }

    /// Select the action that makes sense for the daemon's new state: restart
    /// when it runs (instead of start), start when it does not (instead of
    /// stop/restart). Other choices are left alone.
    pub(crate) const fn preselect_action(&mut self, running: bool) {
        if running && self.selected_action == ACTION_START {
            self.selected_action = ACTION_RESTART;
        } else if !running
            && (self.selected_action == ACTION_STOP || self.selected_action == ACTION_RESTART)
        {
            self.selected_action = ACTION_START;
        }
    }

    /// Toggle between logs and windows view
    pub(crate) const fn toggle_view(&mut self) {
        self.current_view = match self.current_view {
            DashboardView::Logs => DashboardView::Windows,
            DashboardView::Windows => DashboardView::Logs,
        };
    }

    /// Lines one page of the current view scrolls by
    pub(crate) const fn page_size(&self) -> usize {
        let page = match self.current_view {
            DashboardView::Logs => self.log_viewport,
            DashboardView::Windows => self.window_page,
        };
        if page == 0 { 1 } else { page }
    }

    /// Scroll towards older logs / move the window selection up `lines` rows
    pub(crate) fn scroll_up(&mut self, lines: usize) {
        match self.current_view {
            DashboardView::Logs => {
                self.log_scroll_offset = self
                    .log_scroll_offset
                    .saturating_add(lines)
                    .min(self.log_max_offset);
            }
            DashboardView::Windows => {
                self.window_selected = self.window_selected.saturating_sub(lines);
                self.keep_window_selection_visible();
            }
        }
    }

    /// Scroll towards newer logs / move the window selection down `lines` rows
    pub(crate) fn scroll_down(&mut self, lines: usize) {
        match self.current_view {
            DashboardView::Logs => {
                self.log_scroll_offset = self.log_scroll_offset.saturating_sub(lines);
            }
            DashboardView::Windows => {
                self.window_selected = self
                    .window_selected
                    .saturating_add(lines)
                    .min(self.window_count.saturating_sub(1));
                self.keep_window_selection_visible();
            }
        }
    }

    /// One page towards older logs / up the window list (the list and its
    /// selection move together)
    pub(crate) fn page_up(&mut self) {
        let page = self.page_size();
        if self.current_view == DashboardView::Windows {
            self.window_scroll_offset = self.window_scroll_offset.saturating_sub(page);
        }
        self.scroll_up(page);
    }

    /// One page towards newer logs / down the window list
    pub(crate) fn page_down(&mut self) {
        let page = self.page_size();
        if self.current_view == DashboardView::Windows {
            self.window_scroll_offset = self
                .window_scroll_offset
                .saturating_add(page)
                .min(self.window_max_offset);
        }
        self.scroll_down(page);
    }

    /// Jump to the oldest logs / the first window
    pub(crate) const fn scroll_to_top(&mut self) {
        match self.current_view {
            DashboardView::Logs => self.log_scroll_offset = self.log_max_offset,
            DashboardView::Windows => {
                self.window_scroll_offset = 0;
                self.window_selected = 0;
            }
        }
    }

    /// Jump back to live logs / the last window
    pub(crate) const fn scroll_to_bottom(&mut self) {
        match self.current_view {
            DashboardView::Logs => self.log_scroll_offset = 0,
            DashboardView::Windows => {
                self.window_scroll_offset = self.window_max_offset;
                self.window_selected = self.window_count.saturating_sub(1);
            }
        }
    }

    /// Scroll the window list just enough to show the selected row
    const fn keep_window_selection_visible(&mut self) {
        if self.window_selected < self.window_scroll_offset {
            self.window_scroll_offset = self.window_selected;
        } else if self.window_page > 0
            && self.window_selected >= self.window_scroll_offset + self.window_page
        {
            self.window_scroll_offset = self.window_selected + 1 - self.window_page;
        }
    }

    /// The selected window of `windows` (`None` when there are none)
    pub(crate) fn selected_window<'a>(&self, windows: &'a [WindowInfo]) -> Option<&'a WindowInfo> {
        let last = windows.len().checked_sub(1)?;
        windows_in_display_order(windows).nth(self.window_selected.min(last))
    }

    /// The window list was replaced by `windows`: keep the selection on the
    /// window with ID `selected_id` (the one selected before), wherever it moved
    pub(crate) fn follow_selected_window(
        &mut self,
        selected_id: Option<u64>,
        windows: &[WindowInfo],
    ) {
        if let Some(id) = selected_id
            && let Some(pos) = windows_in_display_order(windows).position(|w| w.id == Some(id))
        {
            self.window_selected = pos;
        }
    }

    /// After `added` lines were appended (buffer now `len` lines long), keep a
    /// scrolled-back log view on the same lines; the live view follows.
    pub(crate) fn keep_log_position(&mut self, added: usize, len: usize) {
        if self.log_scroll_offset > 0 {
            self.log_scroll_offset = self
                .log_scroll_offset
                .saturating_add(added)
                .min(len.saturating_sub(1));
        }
    }
}

/// Context for rendering the dashboard screen
pub struct DashboardRenderContext<'a> {
    pub config: &'a Config,
    /// Rendering records viewport sizes in it for scrolling
    pub screen_state: &'a mut DashboardScreen,
    pub daemon_running: bool,
    pub daemon_logs: &'a [String],
    pub windows: &'a [WindowInfo],
    /// Active sinks from the last `PipeWire` snapshot (one has `is_default`)
    pub active_sinks: &'a [crate::pipewire::ActiveSink],
    /// Whether the last `PipeWire` snapshot succeeded
    pub pipewire_available: bool,
}

/// "1 rule", "8 rules"
fn count(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// Render the dashboard screen
pub fn render_dashboard(frame: &mut Frame, area: Rect, ctx: &mut DashboardRenderContext) {
    let [top, bottom] =
        Layout::vertical([Constraint::Length(CARD_HEIGHT), Constraint::Fill(1)]).areas(area);
    let [daemon_area, sink_area] =
        Layout::horizontal([Constraint::Fill(1), Constraint::Fill(1)]).areas(top);

    let matched_count = ctx.windows.iter().filter(|w| w.tracked.is_some()).count();
    render_daemon_card(frame, daemon_area, ctx, matched_count);
    render_sink_card(frame, sink_area, ctx);

    match ctx.screen_state.current_view {
        DashboardView::Logs => render_log_viewer(
            frame,
            bottom,
            ctx.daemon_logs,
            ctx.daemon_running,
            ctx.screen_state,
        ),
        DashboardView::Windows => {
            render_window_tracking(frame, bottom, ctx.windows, matched_count, ctx.screen_state);
        }
    }
}

/// Daemon card: state in the title, action buttons, window summary
fn render_daemon_card(
    frame: &mut Frame,
    area: Rect,
    ctx: &DashboardRenderContext,
    matched_count: usize,
) {
    let state = &ctx.screen_state;
    let mut title = vec![Span::styled(" Daemon ", theme::bold())];
    if ctx.daemon_running {
        title.push(Span::styled("● running", theme::success()));
    } else {
        title.push(Span::styled("○ stopped", theme::error()));
    }
    if let Some(enabled) = state.service_enabled {
        title.push(Span::styled(" · ", theme::dim()));
        title.push(if enabled {
            Span::styled("enabled", theme::success())
        } else {
            Span::styled("disabled", theme::dim())
        });
    }
    title.push(Span::raw(" "));
    let block = rounded_block()
        .padding(Padding::horizontal(1))
        .title(Line::from(title));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let button = |index: usize| -> Span<'static> {
        let label = format!(" {} ", ACTIONS[index]);
        if index == state.selected_action {
            Span::styled(label, theme::selected().add_modifier(Modifier::BOLD))
        } else {
            Span::raw(label)
        }
    };
    let buttons: Vec<usize> = (0..=state.max_action_index).collect();
    let row_width = |indices: &[usize]| -> usize {
        indices
            .iter()
            .map(|&i| text_width(ACTIONS[i]) + 2)
            .sum::<usize>()
            + indices.len()
    };
    let rows: Vec<&[usize]> = if row_width(&buttons) <= usize::from(inner.width) {
        vec![&buttons[..]]
    } else {
        buttons.chunks(3).collect()
    };
    let mut lines: Vec<Line> = rows
        .into_iter()
        .map(|row| {
            let mut spans = Vec::new();
            for (n, &i) in row.iter().enumerate() {
                if n > 0 {
                    spans.push(Span::raw(" "));
                }
                spans.push(button(i));
            }
            Line::from(spans)
        })
        .collect();

    lines.push(if ctx.daemon_running {
        Line::from(vec![
            Span::styled(
                format!("{matched_count}/{}", ctx.windows.len()),
                theme::bold(),
            ),
            Span::styled(" windows matched", theme::dim()),
        ])
    } else {
        Line::from(Span::styled(
            "no windows tracked while stopped",
            theme::dim(),
        ))
    });
    frame.render_widget(Paragraph::new(lines), inner);
}

/// "Playing on" card: the current default sink from the last `PipeWire`
/// snapshot (no I/O here) and what is configured
fn render_sink_card(frame: &mut Frame, area: Rect, ctx: &DashboardRenderContext) {
    let block = rounded_block()
        .padding(Padding::horizontal(1))
        .title(Span::styled(" Playing on ", theme::bold()));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let width = usize::from(inner.width);

    let config = ctx.config;
    let default_sink = ctx.active_sinks.iter().find(|s| s.is_default);
    let (first, second) = match default_sink {
        Some(active) => {
            let node = Line::from(Span::styled(
                truncate_node_name(&active.name, width).into_owned(),
                theme::dim(),
            ));
            let line = if let Some(configured) = config.sinks.iter().find(|s| s.name == active.name)
            {
                Line::from(Span::styled(
                    ellipsize(&configured.desc, width).into_owned(),
                    theme::bold(),
                ))
            } else {
                // Default sink not in the config: show what PipeWire calls it
                const NOTE: &str = " (not configured)";
                let room = width.saturating_sub(text_width(NOTE));
                Line::from(vec![
                    Span::styled(
                        ellipsize(&active.description, room).into_owned(),
                        theme::bold(),
                    ),
                    Span::styled(NOTE, theme::dim()),
                ])
            };
            (line, node)
        }
        None if !ctx.pipewire_available => (
            Line::from(Span::styled("⚠ PipeWire unavailable", theme::warning())),
            Line::from(Span::styled("sink state unknown", theme::dim())),
        ),
        None => (
            Line::from(Span::styled("No default sink", theme::dim())),
            Line::from(""),
        ),
    };

    let present = config
        .sinks
        .iter()
        .filter(|s| ctx.active_sinks.iter().any(|a| a.name == s.name))
        .count();
    let summary = format!(
        "{} · {present} present · {}",
        count(config.sinks.len(), "sink", "sinks"),
        count(config.rules.len(), "rule", "rules"),
    );
    let lines = vec![
        first,
        second,
        Line::from(Span::styled(
            ellipsize(&summary, width).into_owned(),
            theme::dim(),
        )),
    ];
    frame.render_widget(Paragraph::new(lines), inner);
}

/// The windows in the order the window list shows them: matched first, then the rest
pub(crate) fn windows_in_display_order(
    windows: &[WindowInfo],
) -> impl Iterator<Item = &WindowInfo> {
    windows
        .iter()
        .filter(|w| w.tracked.is_some())
        .chain(windows.iter().filter(|w| w.tracked.is_none()))
}

/// Columns of the window table: marker, app ID, title, sink
const WINDOW_COLUMNS: [Constraint; 4] = [
    Constraint::Length(1),
    Constraint::Fill(2),
    Constraint::Fill(3),
    Constraint::Fill(2),
];

/// One row of the window table, cells cut to `cols`
fn window_row<'a>(win: &'a WindowInfo, cols: &[usize]) -> Row<'a> {
    let cut = |text: &str, col: usize| ellipsize(text, cols[col]).into_owned();
    match &win.tracked {
        Some(tracked) => Row::new(vec![
            Cell::from(Span::styled("●", theme::success())),
            Cell::from(Span::styled(cut(&win.app_id, 1), theme::bold())),
            Cell::from(cut(&win.title, 2)),
            Cell::from(Span::styled(
                cut(&format!("→ {}", tracked.sink_desc), 3),
                theme::accent(),
            )),
        ]),
        None => Row::new(vec![
            Cell::from(Span::styled("○", theme::dim())),
            Cell::from(cut(&win.app_id, 1)),
            Cell::from(Span::styled(cut(&win.title, 2), theme::dim())),
            Cell::from(""),
        ]),
    }
}

/// Render the window table (bottom section when the view is Windows)
fn render_window_tracking(
    frame: &mut Frame,
    area: Rect,
    windows: &[WindowInfo],
    matched_count: usize,
    state: &mut DashboardScreen,
) {
    let total = windows.len();
    let block = rounded_block();
    let inner = block.inner(area);

    // One row for the header; the rest list windows
    let visible = usize::from(inner.height).saturating_sub(1);
    state.window_page = visible;
    state.window_count = total;
    state.window_max_offset = total.saturating_sub(visible);
    state.window_selected = state.window_selected.min(total.saturating_sub(1));
    state.keep_window_selection_visible();
    state.window_scroll_offset = state.window_scroll_offset.min(state.window_max_offset);
    let start = state.window_scroll_offset;
    let selected = state.window_selected;

    let mut right = Vec::new();
    if total > visible && visible > 0 {
        let end = (start + visible).min(total);
        right.push(Span::styled(
            format!("[{}-{end}/{total}] · ", start + 1),
            theme::dim(),
        ));
    }
    right.extend([Span::styled("w", theme::key()), Span::raw(" logs ")]);
    let block = block
        .title(Line::from(vec![
            Span::styled(" Windows ", theme::bold()),
            Span::styled(format!("· {matched_count}/{total} matched "), theme::dim()),
        ]))
        .title_top(Line::from(right).alignment(Alignment::Right));

    if windows.is_empty() {
        frame.render_widget(block, area);
        frame.render_widget(
            Paragraph::new(Span::styled(
                "No windows tracked (the daemon lists them while it runs)",
                theme::dim(),
            )),
            inner,
        );
        return;
    }

    let cols = column_widths(&WINDOW_COLUMNS, inner.width, 0);
    // Matched windows first, then the rest
    let rows: Vec<Row> = windows_in_display_order(windows)
        .enumerate()
        .skip(start)
        .take(visible)
        .map(|(i, w)| {
            let row = window_row(w, &cols);
            if i == selected {
                row.style(theme::selected())
            } else {
                row
            }
        })
        .collect();
    let table = Table::new(rows, WINDOW_COLUMNS)
        .header(Row::new(vec!["", "App ID", "Title", "Sink"]).style(theme::bold()))
        .block(block);
    frame.render_widget(table, area);
    render_scrollbar(frame, area, 1, total, start);
}

/// Whether `word` (up to `=`) is a log field name like `app_id`
fn is_field_name(word: &str) -> bool {
    !word.is_empty() && word.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Style the structure of a log message: the `tracing` target, `field=` names,
/// quoted values and arrows. Words are never highlighted for what they say.
fn highlight_message(message: &str) -> Vec<Span<'_>> {
    /// Push `text` unstyled unless it is empty
    fn plain<'a>(spans: &mut Vec<Span<'a>>, text: &'a str) {
        if !text.is_empty() {
            spans.push(Span::raw(text));
        }
    }

    let mut spans = Vec::new();
    let mut rest = message;

    // "pwsw::state: message": the target
    if let Some((target, _)) = rest.split_once(' ')
        && let Some(path) = target.strip_suffix(':')
        && path.contains("::")
        && path.split("::").all(is_field_name)
    {
        spans.push(Span::styled(&rest[..=target.len()], theme::dim()));
        rest = &rest[target.len() + 1..];
    }

    let mut plain_start = 0;
    let mut i = 0;
    let bytes = rest.as_bytes();
    while i < rest.len() {
        let tail = &rest[i..];
        let at_word_start = i == 0 || matches!(bytes[i - 1], b' ' | b'(' | b'[' | b'=' | b',');
        let token = if tail.starts_with('→') {
            Some(('→'.len_utf8(), theme::accent().add_modifier(Modifier::BOLD)))
        } else if tail.starts_with("->") || tail.starts_with("=>") {
            Some((2, theme::accent().add_modifier(Modifier::BOLD)))
        } else if at_word_start && (tail.starts_with('\'') || tail.starts_with('"')) {
            let quote = &tail[..1];
            tail[1..].find(quote).map(|end| (end + 2, theme::accent()))
        } else if at_word_start {
            tail.split_once('=')
                .filter(|(name, _)| is_field_name(name))
                .map(|(name, _)| (name.len() + 1, theme::dim()))
        } else {
            None
        };
        match token {
            Some((len, style)) => {
                plain(&mut spans, &rest[plain_start..i]);
                spans.push(Span::styled(&rest[i..i + len], style));
                i += len;
                plain_start = i;
            }
            None => i += tail.chars().next().map_or(1, char::len_utf8),
        }
    }
    plain(&mut spans, &rest[plain_start..]);
    if spans.is_empty() {
        spans.push(Span::raw(message));
    }
    spans
}

/// Parse and style a log line: time, padded level, message
fn style_log_line(line: &str) -> Line<'_> {
    // "2025-12-17T10:00:00.123456Z  INFO pwsw::state: message here"
    let Some((full_timestamp, rest)) = line.split_once(' ') else {
        return Line::from(Span::raw(line));
    };
    let Some((level, message)) = rest.trim_start().split_once(' ') else {
        // Malformed line, return as-is
        return Line::from(Span::raw(line));
    };

    // Time only ("10:00:00"); character-based: the line may contain anything,
    // so never cut inside a character.
    let timestamp = full_timestamp
        .split_once('T')
        .map_or(full_timestamp, |(_, time)| {
            let time = time.split('.').next().unwrap_or(time);
            time.char_indices().nth(8).map_or(time, |(i, _)| &time[..i])
        });

    let mut spans = vec![
        Span::styled(timestamp, theme::dim()),
        Span::raw(" "),
        // Levels padded to the same width so messages line up
        Span::styled(format!("{level:>5}"), theme::log_level(level)),
        Span::raw(" "),
    ];
    spans.extend(highlight_message(message));
    Line::from(spans)
}

/// Split `line` into rows of at most `width` columns, breaking anywhere.
///
/// Wrapping by hand (instead of `Paragraph::wrap`) makes the number of rows
/// each log line takes exact, so the newest line can be kept on screen.
fn wrap_line(line: Line<'_>, width: usize) -> Vec<Line<'static>> {
    let width = width.max(1);
    let mut rows = Vec::new();
    let mut row: Vec<Span<'static>> = Vec::new();
    let mut row_width = 0;
    for span in line.spans {
        let mut text = String::new();
        for c in span.content.chars() {
            let w = char_width(c);
            if row_width + w > width && row_width > 0 {
                if !text.is_empty() {
                    row.push(Span::styled(std::mem::take(&mut text), span.style));
                }
                rows.push(Line::from(std::mem::take(&mut row)));
                row_width = 0;
            }
            text.push(c);
            row_width += w;
        }
        if !text.is_empty() {
            row.push(Span::styled(text, span.style));
        }
    }
    rows.push(Line::from(row));
    rows
}

/// The rows to show for `logs` in a `width`×`height` viewport, scrolled back
/// `offset` lines from the newest, plus the largest useful offset.
///
/// The newest shown line always ends on the last row; older lines fill the
/// rows above it (the oldest shown may be cut at the top).
fn log_rows(
    logs: &[String],
    offset: usize,
    width: usize,
    height: usize,
) -> (Vec<Line<'static>>, usize) {
    let rows_of = |line: &String| wrap_line(style_log_line(line), width);

    // Scrolling further back than the point where the oldest line reaches the
    // top would only leave empty rows at the top.
    let mut filled = 0;
    let mut first_full_end = logs.len();
    for (i, line) in logs.iter().enumerate() {
        filled += rows_of(line).len();
        if filled >= height {
            first_full_end = i + 1;
            break;
        }
    }
    let max_offset = logs.len() - first_full_end;

    let end = logs.len() - offset.min(max_offset);
    let mut rows: Vec<Line<'static>> = Vec::new();
    for line in logs[..end].iter().rev() {
        if rows.len() >= height {
            break;
        }
        let mut wrapped = rows_of(line);
        wrapped.append(&mut rows);
        rows = wrapped;
    }
    let excess = rows.len().saturating_sub(height);
    rows.drain(..excess);
    (rows, max_offset)
}

/// Render daemon log viewer
fn render_log_viewer(
    frame: &mut Frame,
    area: Rect,
    daemon_logs: &[String],
    daemon_running: bool,
    state: &mut DashboardScreen,
) {
    // Measure the viewport first: scroll limits depend on it
    let block = rounded_block();
    let inner = block.inner(area);
    let width = usize::from(inner.width);
    let height = usize::from(inner.height);
    let (rows, max_offset) = log_rows(daemon_logs, state.log_scroll_offset, width, height);
    state.log_viewport = height;
    state.log_max_offset = max_offset;
    state.log_scroll_offset = state.log_scroll_offset.min(max_offset);
    let scroll_offset = state.log_scroll_offset;

    let mut title = vec![Span::styled(" Logs ", theme::bold())];
    if scroll_offset > 0 {
        title.push(Span::styled(
            format!("· ↑{scroll_offset}, End for live "),
            theme::warning(),
        ));
    } else if daemon_running {
        title.push(Span::styled("· live ", theme::success()));
    } else {
        title.push(Span::styled("· daemon stopped ", theme::dim()));
    }
    // The daemon logs in UTC
    title.push(Span::styled("· UTC ", theme::dim()));
    let block = block.title(Line::from(title)).title_top(
        Line::from(vec![
            Span::styled("w", theme::key()),
            Span::raw(" windows "),
        ])
        .alignment(Alignment::Right),
    );

    let log_text = if daemon_logs.is_empty() {
        vec![Line::from(Span::styled(
            "No logs yet. Start the daemon to see its log here.",
            theme::dim(),
        ))]
    } else {
        rows
    };
    frame.render_widget(Paragraph::new(log_text).block(block), area);
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    fn text(rows: &[Line]) -> Vec<String> {
        rows.iter().map(ToString::to_string).collect()
    }

    fn logs(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("l{i}")).collect()
    }

    #[test]
    fn test_log_rows_live_shows_newest_at_bottom() {
        let logs = logs(100);
        let (rows, max_offset) = log_rows(&logs, 0, 20, 5);
        assert_eq!(text(&rows), vec!["l95", "l96", "l97", "l98", "l99"]);
        assert_eq!(max_offset, 95);
    }

    #[test]
    fn test_log_rows_scrolled_and_clamped() {
        let logs = logs(100);
        let (rows, _) = log_rows(&logs, 10, 20, 3);
        assert_eq!(text(&rows), vec!["l87", "l88", "l89"]);
        // Scrolling past the oldest line stops with the oldest at the top
        let (rows, max_offset) = log_rows(&logs, 1000, 20, 3);
        assert_eq!(max_offset, 97);
        assert_eq!(text(&rows), vec!["l0", "l1", "l2"]);
    }

    #[test]
    fn test_log_rows_wrapped_line_keeps_newest_visible() {
        let mut logs = logs(10);
        logs.push("x".repeat(25)); // wraps into 3 rows of width 10
        logs.push("newest".to_string());
        let (rows, _) = log_rows(&logs, 0, 10, 4);
        assert_eq!(text(&rows), vec![
            "xxxxxxxxxx",
            "xxxxxxxxxx",
            "xxxxx",
            "newest"
        ]);
    }

    #[test]
    fn test_log_rows_fewer_lines_than_viewport() {
        let (rows, max_offset) = log_rows(&logs(3), 5, 20, 10);
        assert_eq!(max_offset, 0);
        assert_eq!(text(&rows), vec!["l0", "l1", "l2"]);
        let (rows, max_offset) = log_rows(&[], 0, 20, 10);
        assert_eq!(text(&rows), Vec::<String>::new());
        assert_eq!(max_offset, 0);
    }

    #[test]
    fn test_wrap_line_counts_wide_characters() {
        let rows = wrap_line(Line::from("日本語テキスト"), 4);
        assert_eq!(text(&rows), vec!["日本", "語テ", "キス", "ト"]);
        assert_eq!(wrap_line(Line::from(""), 4).len(), 1);
    }

    #[test]
    fn test_scroll_state_uses_rendered_limits() {
        let mut state = DashboardScreen::new();
        state.log_viewport = 7;
        state.log_max_offset = 10;
        assert_eq!(state.page_size(), 7);
        state.scroll_up(state.page_size());
        state.scroll_up(state.page_size());
        assert_eq!(state.log_scroll_offset, 10, "clamped to the oldest");
        state.scroll_down(3);
        assert_eq!(state.log_scroll_offset, 7);
        state.scroll_to_bottom();
        assert_eq!(state.log_scroll_offset, 0, "End returns to live");

        state.toggle_view();
        state.window_page = 4;
        state.window_max_offset = 6;
        state.window_count = 10;
        state.page_down();
        assert_eq!(
            (state.window_scroll_offset, state.window_selected),
            (4, 4),
            "PageDown moves the list and the selection down a page"
        );
        state.page_down();
        assert_eq!((state.window_scroll_offset, state.window_selected), (6, 8));
        state.scroll_down(5);
        assert_eq!(state.window_selected, 9, "clamped to the last window");
        state.scroll_up(4);
        assert_eq!(
            (state.window_scroll_offset, state.window_selected),
            (5, 5),
            "the list follows the selection up"
        );
        state.scroll_to_bottom();
        assert_eq!((state.window_scroll_offset, state.window_selected), (6, 9));
        state.scroll_to_top();
        assert_eq!((state.window_scroll_offset, state.window_selected), (0, 0));
    }

    fn win(id: u64, app_id: &str, matched: bool) -> WindowInfo {
        WindowInfo {
            id: Some(id),
            app_id: app_id.to_string(),
            title: String::new(),
            matched_on: None,
            tracked: matched.then(|| crate::ipc::TrackedInfo {
                sink_name: String::new(),
                sink_desc: String::new(),
                fallbacks: Vec::new(),
            }),
        }
    }

    #[test]
    fn test_selected_window_in_display_order_and_followed() {
        let mut state = DashboardScreen::new();
        assert!(state.selected_window(&[]).is_none());
        let windows = vec![win(1, "a", false), win(2, "b", true), win(3, "c", false)];
        assert_eq!(
            state.selected_window(&windows).map(|w| w.app_id.as_str()),
            Some("b"),
            "matched windows come first"
        );
        state.window_selected = 7;
        assert_eq!(
            state.selected_window(&windows).map(|w| w.app_id.as_str()),
            Some("c"),
            "past the end: the last window"
        );
        // Window 3 moves to the top once it is matched
        state.window_selected = 2;
        let updated = vec![win(1, "a", false), win(2, "b", true), win(3, "c", true)];
        state.follow_selected_window(Some(3), &updated);
        assert_eq!(state.window_selected, 1);
        assert_eq!(
            state.selected_window(&updated).map(|w| w.app_id.as_str()),
            Some("c")
        );
    }

    #[test]
    fn test_keep_log_position_when_scrolled_back() {
        let mut state = DashboardScreen::new();
        state.keep_log_position(5, 100);
        assert_eq!(state.log_scroll_offset, 0, "live view follows new lines");
        state.log_scroll_offset = 3;
        state.keep_log_position(5, 100);
        assert_eq!(state.log_scroll_offset, 8);
    }

    #[test]
    fn test_style_log_line_non_ascii_timestamp_does_not_panic() {
        for line in [
            "2025-12-17T10:00:0é.5Z INFO x",
            "xTé INFO y",
            "2025-12-17Tääääääääää INFO z",
            "日本T語語語語語語語語語.語 WARN ü",
        ] {
            let rendered = style_log_line(line);
            assert!(!rendered.spans.is_empty(), "{line}");
        }
        let line = style_log_line("2025-12-17Tääääääääää INFO z");
        assert_eq!(line.spans[0].content, "ääääääää");
    }

    #[test]
    fn test_style_log_line_padded_level() {
        let line = style_log_line("2025-12-17T10:00:00.1Z  INFO Starting");
        let spans: Vec<_> = line.spans.iter().map(|s| s.content.to_string()).collect();
        assert_eq!(spans[0], "10:00:00");
        assert_eq!(spans[2], " INFO", "levels are padded to five columns");
        let warn = style_log_line("2025-12-17T10:00:00.1Z  WARN x");
        assert_eq!(warn.spans[2].content, " WARN");
        let error = style_log_line("2025-12-17T10:00:00.1Z ERROR x");
        assert_eq!(error.spans[2].content, "ERROR");
    }

    // ============================================================================
    // Log Parsing Tests
    // ============================================================================

    #[test]
    fn test_style_log_line_valid_format() {
        // Standard log format: "TIMESTAMP LEVEL message"
        let line = style_log_line("2025-12-17T10:00:00.123456Z INFO Starting daemon");
        let spans: Vec<_> = line.spans.iter().map(|s| s.content.to_string()).collect();

        // Should extract timestamp (time only), level, and message
        assert!(spans[0].contains("10:00:00"), "Expected time extraction");
        assert!(spans[2].contains("INFO"), "Expected level");
        // Message spans come after level
        assert!(
            spans.iter().any(|s| s.contains("Starting")),
            "Expected message content"
        );
    }

    #[test]
    fn test_style_log_line_malformed() {
        // Less than 3 parts after split
        let line = style_log_line("malformed log");
        // Should return as-is without crashing
        assert_eq!(line.spans.len(), 1);
        assert_eq!(line.spans[0].content, "malformed log");

        // Single word
        let single = style_log_line("WARN");
        assert_eq!(single.spans.len(), 1);
        assert_eq!(single.spans[0].content, "WARN");

        // Empty string
        let empty = style_log_line("");
        assert_eq!(empty.spans.len(), 1);
    }

    #[test]
    fn test_style_log_line_timestamp_formats() {
        // Standard ISO 8601 with microseconds
        let line1 = style_log_line("2025-12-17T14:30:45.123456Z INFO test");
        let spans1: Vec<_> = line1.spans.iter().map(|s| s.content.to_string()).collect();
        assert_eq!(spans1[0], "14:30:45", "Should extract HH:MM:SS");

        // Without microseconds
        let line2 = style_log_line("2025-12-17T14:30:45Z INFO test");
        let spans2: Vec<_> = line2.spans.iter().map(|s| s.content.to_string()).collect();
        assert_eq!(spans2[0], "14:30:45", "Should extract HH:MM:SS");

        // Without 'T' separator (fallback to full timestamp)
        let line3 = style_log_line("20251217 INFO test");
        let spans3: Vec<_> = line3.spans.iter().map(|s| s.content.to_string()).collect();
        assert_eq!(
            spans3[0], "20251217",
            "Should use full timestamp as fallback"
        );
    }

    #[test]
    fn test_highlight_message_patterns() {
        // Test pattern highlighting
        let spans = highlight_message("Rule matched: firefox -> headphones");
        // Should have highlighted "Rule matched:" portion
        assert!(
            spans.len() > 1,
            "Should split message into highlighted parts"
        );

        // Test multiple patterns in one message
        let spans2 = highlight_message("Window opened: app_id=firefox title=Browser");
        // Should highlight "Window opened:", "app_id=", and "title="
        assert!(
            spans2.len() >= 3,
            "Should have multiple highlighted segments"
        );
    }

    #[test]
    fn test_highlight_message_no_patterns() {
        // Message with no highlight patterns
        let spans = highlight_message("Just a plain message");
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].content, "Just a plain message");
    }

    #[test]
    fn test_highlight_message_ignores_words() {
        // Words are not highlighted for what they say
        let spans = highlight_message("Loaded 5 sinks, 3 rules");
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].content, "Loaded 5 sinks, 3 rules");
        // An apostrophe inside a word is not a quote
        assert_eq!(highlight_message("the window's sink").len(), 1);
    }

    #[test]
    fn test_highlight_message_structure() {
        let text = |spans: &[Span]| {
            spans
                .iter()
                .map(|s| s.content.to_string())
                .collect::<Vec<_>>()
        };
        let spans = highlight_message("pwsw::state: Rule matched: 'firefox' → Sony (A2DP)");
        assert_eq!(text(&spans), vec![
            "pwsw::state: ",
            "Rule matched: ",
            "'firefox'",
            " ",
            "→",
            " Sony (A2DP)"
        ]);
        let spans = highlight_message("now: app_id='mpv' title='x y' id=3");
        assert_eq!(text(&spans), vec![
            "now: ", "app_id=", "'mpv'", " ", "title=", "'x y'", " ", "id=", "3"
        ]);
        // Unclosed quote and non-ASCII stay intact
        let full: String = text(&highlight_message("grüße 'open → ü")).concat();
        assert_eq!(full, "grüße 'open → ü");
    }

    #[test]
    fn test_style_log_line_all_levels() {
        // Test all log levels are parsed correctly
        for level in &["TRACE", "DEBUG", "INFO", "WARN", "ERROR"] {
            let log_str = format!("2025-01-01T00:00:00Z {level} test");
            let line = style_log_line(&log_str);
            let spans: Vec<_> = line.spans.iter().map(|s| s.content.to_string()).collect();
            assert!(
                spans.iter().any(|s| s.trim() == *level),
                "Level {level} should be parsed"
            );
        }
    }
}
