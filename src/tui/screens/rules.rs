//! Rules screen - Manage window matching rules

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, List, ListItem, ListState, Paragraph, Row, Table, TableState},
    Frame,
};
use throbber_widgets_tui::{Throbber, ThrobberState};

use crate::config::{Rule, SinkConfig};
use crate::tui::editor_state::SimpleEditor;
use crate::tui::widgets::{centered_modal, modal_size, render_input};
use regex::Regex;

/// Rules screen mode
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RulesMode {
    List,
    AddEdit,
    Delete,
    SelectSink, // Dropdown for sink selection
}

/// Editor state for add/edit modal
#[derive(Debug, Clone)]
pub struct RuleEditor {
    pub app_id_pattern: SimpleEditor,
    pub title_pattern: SimpleEditor,
    pub sink_ref: String,
    pub desc: SimpleEditor,
    pub notify: Option<bool>,
    pub focused_field: usize, // 0=app_id, 1=title, 2=sink, 3=desc, 4=notify
    pub sink_dropdown_index: usize,
    /// State for sink selector dropdown
    pub sink_selector_state: ListState,
    // Cached compiled regexes to avoid recompiling on every render
    pub compiled_app_id: Option<std::sync::Arc<Regex>>,
    pub compiled_title: Option<std::sync::Arc<Regex>>,
    // Track which pattern strings the compiled regex corresponds to
    pub compiled_app_id_for: Option<String>,
    pub compiled_title_for: Option<String>,
}

impl RuleEditor {
    pub fn new() -> Self {
        Self {
            app_id_pattern: SimpleEditor::new(),
            title_pattern: SimpleEditor::new(),
            sink_ref: String::new(),
            desc: SimpleEditor::new(),
            notify: None,
            focused_field: 0,
            sink_dropdown_index: 0,
            sink_selector_state: ListState::default(),
            compiled_app_id: None,
            compiled_title: None,
            compiled_app_id_for: None,
            compiled_title_for: None,
        }
    }

    pub fn from_rule(rule: &Rule) -> Self {
        let compiled_app_id = Regex::new(&rule.app_id_pattern)
            .ok()
            .map(std::sync::Arc::new);
        let compiled_title = match &rule.title_pattern {
            Some(t) if !t.is_empty() => Regex::new(t).ok().map(std::sync::Arc::new),
            _ => None,
        };

        Self {
            app_id_pattern: SimpleEditor::from_string(rule.app_id_pattern.clone()),
            title_pattern: SimpleEditor::from_string(
                rule.title_pattern.clone().unwrap_or_default(),
            ),
            sink_ref: rule.sink_ref.clone(),
            desc: SimpleEditor::from_string(rule.desc.clone().unwrap_or_default()),
            notify: rule.notify,
            focused_field: 0,
            sink_dropdown_index: 0,
            sink_selector_state: ListState::default(),
            compiled_app_id,
            compiled_title,
            compiled_app_id_for: Some(rule.app_id_pattern.clone()),
            compiled_title_for: rule.title_pattern.clone(),
        }
    }

    pub fn next_field(&mut self) {
        if self.focused_field < 4 {
            self.focused_field += 1;
        }
    }

    pub fn prev_field(&mut self) {
        if self.focused_field > 0 {
            self.focused_field -= 1;
        }
    }

    /// Ensure compiled regex caches are up-to-date for current editor patterns
    pub fn ensure_compiled(&mut self) {
        // Compile app id pattern if non-empty and store which string it corresponds to
        if self.app_id_pattern.value().is_empty() {
            self.compiled_app_id = None;
            self.compiled_app_id_for = None;
        } else if self.compiled_app_id_for.as_deref() != Some(self.app_id_pattern.value()) {
            self.compiled_app_id = Regex::new(self.app_id_pattern.value())
                .ok()
                .map(std::sync::Arc::new);
            self.compiled_app_id_for = Some(self.app_id_pattern.value().to_string());
        }

        // Compile title pattern if non-empty
        if self.title_pattern.value().is_empty() {
            self.compiled_title = None;
            self.compiled_title_for = None;
        } else if self.compiled_title_for.as_deref() != Some(self.title_pattern.value()) {
            self.compiled_title = Regex::new(self.title_pattern.value())
                .ok()
                .map(std::sync::Arc::new);
            self.compiled_title_for = Some(self.title_pattern.value().to_string());
        }
    }
}

/// Rules screen state
pub struct RulesScreen {
    pub mode: RulesMode,
    pub selected: usize,
    pub editor: RuleEditor,
    pub editing_index: Option<usize>,
    /// Table scroll state
    pub state: TableState,
}

impl RulesScreen {
    pub fn new() -> Self {
        Self {
            mode: RulesMode::List,
            selected: 0,
            editor: RuleEditor::new(),
            editing_index: None,
            state: TableState::default(),
        }
    }

    pub fn select_previous(&mut self, rule_count: usize) {
        if rule_count > 0 && self.selected > 0 {
            self.selected -= 1;
        }
    }

    pub fn select_next(&mut self, rule_count: usize) {
        if rule_count > 0 && self.selected < rule_count - 1 {
            self.selected += 1;
        }
    }

    pub fn start_add(&mut self) {
        self.mode = RulesMode::AddEdit;
        self.editor = RuleEditor::new();
        self.editing_index = None;
    }

    pub fn start_edit(&mut self, rules: &[Rule]) {
        if self.selected < rules.len() {
            self.mode = RulesMode::AddEdit;
            self.editor = RuleEditor::from_rule(&rules[self.selected]);
            self.editing_index = Some(self.selected);
        }
    }

    pub fn start_delete(&mut self) {
        self.mode = RulesMode::Delete;
    }

    pub fn open_sink_selector(&mut self) {
        self.mode = RulesMode::SelectSink;
    }

    pub fn cancel(&mut self) {
        self.mode = RulesMode::List;
    }
}

/// Render the rules screen
#[allow(clippy::too_many_arguments)]
pub fn render_rules(
    frame: &mut Frame,
    area: Rect,
    rules: &[Rule],
    sinks: &[SinkConfig],
    screen_state: &mut RulesScreen,
    windows: &[crate::ipc::WindowInfo],
    preview: Option<&crate::tui::app::PreviewResult>,
    throbber_state: &mut ThrobberState,
) {
    match screen_state.mode {
        RulesMode::List => render_list(frame, area, rules, sinks, screen_state),
        RulesMode::AddEdit => render_editor(
            frame,
            area,
            sinks,
            screen_state,
            windows,
            preview,
            throbber_state,
        ),
        RulesMode::Delete => render_delete_confirmation(frame, area, rules, screen_state),
        RulesMode::SelectSink => render_sink_selector(frame, area, sinks, &mut screen_state.editor),
    }
}

/// Render the rules list
fn render_list(
    frame: &mut Frame,
    area: Rect,
    rules: &[Rule],
    sinks: &[SinkConfig],
    screen_state: &mut RulesScreen,
) {
    // Build a lookup from sink name/desc -> padded display string once per render to avoid per-row formatting.
    let max_desc_len = sinks.iter().map(|s| s.desc.len()).max().unwrap_or(0);
    let mut sink_display_map: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    for s in sinks {
        let display = if s.desc.len() >= max_desc_len {
            s.desc.clone()
        } else {
            let mut st = s.desc.clone();
            st.push_str(&" ".repeat(max_desc_len - s.desc.len()));
            st
        };
        sink_display_map.insert(s.name.clone(), display.clone());
        sink_display_map.insert(s.desc.clone(), display);
    }

    let rows: Vec<Row> = rules
        .iter()
        .enumerate()
        .map(|(i, rule)| {
            let is_selected = i == screen_state.selected;

            // Resolve sink display name
            let sink_display = sink_display_map
                .get(&rule.sink_ref)
                .map_or(rule.sink_ref.as_str(), String::as_str);

            // Prepare cells
            let index_cell = Cell::from((i + 1).to_string());
            let app_id_cell = Cell::from(rule.app_id_pattern.as_str());
            let title_cell = rule.title_pattern.as_ref().map_or_else(
                || Cell::from(Span::styled("*", Style::default().fg(Color::DarkGray))),
                |s| Cell::from(s.as_str()),
            );
            let sink_cell =
                Cell::from(Span::styled(sink_display, Style::default().fg(Color::Cyan)));
            let desc_cell = rule
                .desc
                .as_ref()
                .map_or_else(|| Cell::from(""), |s| Cell::from(s.as_str()));

            let row_style = if is_selected {
                Style::default().bg(Color::DarkGray)
            } else {
                Style::default()
            };

            Row::new(vec![
                index_cell,
                app_id_cell,
                title_cell,
                sink_cell,
                desc_cell,
            ])
            .style(row_style)
            .height(1)
        })
        .collect();

    let table = Table::new(
        rows,
        [
            Constraint::Length(3),      // #
            Constraint::Percentage(25), // App ID
            Constraint::Percentage(25), // Title
            Constraint::Percentage(20), // Target
            Constraint::Percentage(30), // Description
        ],
    )
    .header(
        Row::new(vec![
            "#",
            "App ID Pattern",
            "Title Pattern",
            "Target Sink",
            "Description",
        ])
        .style(
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )
        .bottom_margin(1),
    )
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Rules ([a]dd [e]dit [x]delete [↑/↓]priority [Ctrl+S]save) "),
    );

    // Sync state
    screen_state.state.select(Some(screen_state.selected));
    frame.render_stateful_widget(table, area, &mut screen_state.state);

    // Compute visible viewport (inner area) for arrow indicators
    let inner = area.inner(ratatui::layout::Margin {
        vertical: 1,
        horizontal: 0,
    });
    let view_height = inner.height as usize;

    // Determine whether there is content above/below the current viewport
    let raw_offset = screen_state.state.offset();
    let total = rules.len();
    // For table rows which do not wrap, use logical offset
    let has_above = raw_offset > 0;
    let has_below = raw_offset + view_height < total;

    // Draw top arrow if there's more above
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

    // Draw bottom arrow if there's more below
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

/// Render the add/edit modal
fn render_editor(
    frame: &mut Frame,
    area: Rect,
    sinks: &[SinkConfig],
    screen_state: &RulesScreen,
    windows: &[crate::ipc::WindowInfo],
    preview: Option<&crate::tui::app::PreviewResult>,
    throbber_state: &mut ThrobberState,
) {
    let title = if screen_state.editing_index.is_some() {
        "Edit Rule"
    } else {
        "Add Rule"
    };

    let popup_area = centered_modal(modal_size::LARGE, area);

    // Dynamic layout: hide help if height is too small
    let show_help = area.height > 25;

    let constraints = if show_help {
        vec![
            Constraint::Length(3), // App ID pattern
            Constraint::Length(3), // Title pattern
            Constraint::Length(3), // Sink selector
            Constraint::Length(3), // Description
            Constraint::Length(3), // Notify toggle
            Constraint::Min(6),    // Live preview
            Constraint::Length(1), // Help text
        ]
    } else {
        vec![
            Constraint::Length(3), // App ID pattern
            Constraint::Length(3), // Title pattern
            Constraint::Length(3), // Sink selector
            Constraint::Length(3), // Description
            Constraint::Length(3), // Notify toggle
            Constraint::Min(3),    // Live preview (reduced min height)
        ]
    };

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(2)
        .constraints(constraints)
        .split(popup_area);

    let block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .style(Style::default().bg(Color::Black));
    frame.render_widget(block, popup_area);

    // App ID pattern field
    render_input(
        frame,
        chunks[0],
        "App ID Pattern (regex):",
        &screen_state.editor.app_id_pattern.input,
        screen_state.editor.focused_field == 0,
    );

    // Title pattern field
    render_input(
        frame,
        chunks[1],
        "Title Pattern (optional regex):",
        &screen_state.editor.title_pattern.input,
        screen_state.editor.focused_field == 1,
    );

    // Sink selector button - find sink description if set
    let sink_display = if screen_state.editor.sink_ref.is_empty() {
        None
    } else {
        sinks
            .iter()
            .find(|s| {
                s.name == screen_state.editor.sink_ref || s.desc == screen_state.editor.sink_ref
            })
            .map(|s| s.desc.as_str())
            .or(Some(screen_state.editor.sink_ref.as_str()))
    };

    crate::tui::widgets::render_selector_button(
        frame,
        chunks[2],
        "Target Sink",
        sink_display,
        screen_state.editor.focused_field == 2,
    );

    // Description field
    render_input(
        frame,
        chunks[3],
        "Description (optional):",
        &screen_state.editor.desc.input,
        screen_state.editor.focused_field == 3,
    );

    // Notify toggle with border-based focus
    let mut notify_spans = Vec::new();
    match screen_state.editor.notify {
        Some(true) => {
            notify_spans.push(Span::styled("✓ ", Style::default().fg(Color::Green)));
            notify_spans.push(Span::raw("Notify (enabled)"));
        }
        Some(false) => {
            notify_spans.push(Span::styled("✗ ", Style::default().fg(Color::Red)));
            notify_spans.push(Span::raw("Notify (disabled)"));
        }
        None => {
            notify_spans.push(Span::styled("○ ", Style::default().fg(Color::Gray)));
            notify_spans.push(Span::raw("Notify (use global setting)"));
        }
    }

    let border_style =
        crate::tui::widgets::focus_border_style(screen_state.editor.focused_field == 4);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style);

    let notify_widget = Paragraph::new(Line::from(notify_spans)).block(block);
    frame.render_widget(notify_widget, chunks[4]);

    // Live preview panel
    render_live_preview(
        frame,
        chunks[5],
        screen_state,
        windows,
        preview,
        throbber_state,
    );

    // Help text
    if show_help && chunks.len() > 6 {
        let help_line = crate::tui::widgets::modal_help_line(&[
            ("Tab", "Next"),
            ("Shift+Tab", "Prev"),
            ("Enter", "Save/Select"),
            ("Space", "Toggle"),
            ("Esc", "Cancel"),
        ]);
        let help_widget = Paragraph::new(vec![help_line]).style(Style::default().fg(Color::Gray));
        frame.render_widget(help_widget, chunks[6]);
    }
}

/// Render live regex preview showing matching windows
fn render_live_preview(
    frame: &mut Frame,
    area: Rect,
    screen_state: &RulesScreen,
    windows: &[crate::ipc::WindowInfo],
    preview: Option<&crate::tui::app::PreviewResult>,
    throbber_state: &mut ThrobberState,
) {
    if let Some(res) = preview {
        // Ensure preview corresponds to current editor content
        if res.app_pattern == screen_state.editor.app_id_pattern.value()
            && res.title_pattern.as_deref().unwrap_or("")
                == screen_state.editor.title_pattern.value()
        {
            // If background worker marked this preview as pending, show spinner (computing).
            if res.pending && res.matches.is_empty() && !res.timed_out {
                let block = Block::default()
                    .borders(Borders::ALL)
                    .title("Matching Windows");
                let inner = block.inner(area);
                frame.render_widget(block, area);

                let throbber = Throbber::default()
                    .label("Computing matches...")
                    .style(Style::default().fg(Color::Yellow))
                    .throbber_style(
                        Style::default()
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::BOLD),
                    );
                frame.render_stateful_widget(throbber, inner, throbber_state);
                return;
            }

            // Normal results display
            let mut preview_lines = vec![];

            if res.timed_out {
                preview_lines.push(Line::from(vec![Span::styled(
                    "  Preview timed out or invalid regex",
                    Style::default().fg(Color::Red),
                )]));
            } else if res.matches.is_empty() {
                preview_lines.push(Line::from(vec![Span::styled(
                    "  No matching windows",
                    Style::default().fg(Color::Yellow),
                )]));
            } else {
                for m in &res.matches[..res.matches.len().min(5)] {
                    preview_lines.push(Line::from(vec![
                        Span::styled("  ✓ ", Style::default().fg(Color::Green)),
                        Span::raw(m.as_str()),
                    ]));
                }
                if res.matches.len() > 5 {
                    let remaining = res.matches.len() - 5;
                    let text = format!("  ...and {remaining} more");
                    preview_lines.push(Line::from(vec![Span::styled(
                        text,
                        Style::default().fg(Color::DarkGray),
                    )]));
                }
            }

            let preview_widget = Paragraph::new(preview_lines).block(
                Block::default()
                    .borders(Borders::ALL)
                    .title("Matching Windows"),
            );
            frame.render_widget(preview_widget, area);
            return;
        }
    }

    // Fallback: use local compiled regex matching (fast for small window lists).
    // Ensure compiled regexes correspond to current editor text; compile if needed.
    // Attempt to use cached compiled regex references, or compile temporary ones for this render.
    let app_id_regex: Option<std::sync::Arc<Regex>> =
        if screen_state.editor.app_id_pattern.value().is_empty() {
            None
        } else if screen_state.editor.compiled_app_id_for.as_deref()
            == Some(screen_state.editor.app_id_pattern.value())
        {
            screen_state.editor.compiled_app_id.clone()
        } else {
            None
        };

    let title_regex: Option<std::sync::Arc<Regex>> =
        if screen_state.editor.title_pattern.value().is_empty() {
            None
        } else if screen_state.editor.compiled_title_for.as_deref()
            == Some(screen_state.editor.title_pattern.value())
        {
            screen_state.editor.compiled_title.clone()
        } else {
            None
        };

    // Convert to Option<&Regex> for the matching code below
    let app_id_regex_ref: Option<&Regex> = app_id_regex.as_ref().map(std::convert::AsRef::as_ref);
    let title_regex_ref: Option<&Regex> = title_regex.as_ref().map(std::convert::AsRef::as_ref);

    let mut preview_lines = vec![];

    if let Some(app_regex) = app_id_regex_ref {
        if windows.is_empty() {
            preview_lines.push(Line::from(vec![Span::styled(
                "  (daemon not running)",
                Style::default().fg(Color::Gray),
            )]));
        } else {
            let mut match_count = 0;
            let mut shown = 0;

            for window in &windows[..windows.len().min(10)] {
                let app_id_match = app_regex.is_match(&window.app_id);
                let title_match = title_regex_ref.map_or(true, |r| r.is_match(&window.title));

                if app_id_match && title_match {
                    match_count += 1;
                    if shown < 5 {
                        preview_lines.push(Line::from(vec![
                            Span::styled("  ✓ ", Style::default().fg(Color::Green)),
                            Span::raw(window.app_id.as_str()),
                            Span::raw(" | "),
                            Span::raw(window.title.as_str()),
                        ]));
                        shown += 1;
                    }
                }
            }

            if match_count == 0 {
                preview_lines.push(Line::from(vec![Span::styled(
                    "  No matching windows",
                    Style::default().fg(Color::Yellow),
                )]));
            } else if match_count > 5 {
                preview_lines.push(Line::from(vec![Span::styled(
                    (match_count - 5).to_string(),
                    Style::default().fg(Color::Gray),
                )]));
            }
        }
    } else if !screen_state.editor.app_id_pattern.value().is_empty() {
        preview_lines.push(Line::from(vec![Span::styled(
            "  Invalid regex pattern",
            Style::default().fg(Color::Red),
        )]));
    } else {
        preview_lines.push(Line::from(vec![Span::styled(
            "  Enter app_id pattern to see preview",
            Style::default().fg(Color::Gray),
        )]));
    }

    let preview_widget = Paragraph::new(preview_lines).block(
        Block::default()
            .borders(Borders::ALL)
            .title("Matching Windows"),
    );
    frame.render_widget(preview_widget, area);
}

/// Render sink selector dropdown
fn render_sink_selector(
    frame: &mut Frame,
    area: Rect,
    sinks: &[SinkConfig],
    editor: &mut RuleEditor,
) {
    let popup_area = centered_modal(modal_size::DROPDOWN, area);

    let items: Vec<ListItem> = sinks
        .iter()
        .map(|sink| {
            let line = Line::from(vec![
                Span::raw("  "),
                Span::styled(&sink.desc, Style::default().fg(Color::White)),
            ]);

            ListItem::new(vec![
                line,
                Line::from(vec![
                    Span::raw("    "),
                    Span::styled(&sink.name, Style::default().fg(Color::Gray)),
                ]),
            ])
        })
        .collect();

    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("Select Target Sink (↑/↓, Enter to confirm, Esc to cancel)")
                .style(Style::default().bg(Color::Black)),
        )
        .highlight_style(Style::default().bg(Color::DarkGray))
        .highlight_symbol(""); // Ensure no default symbol

    // Sync state
    editor
        .sink_selector_state
        .select(Some(editor.sink_dropdown_index));
    frame.render_stateful_widget(list, popup_area, &mut editor.sink_selector_state);

    // Compute visible viewport height for indicators in dropdown
    let inner = popup_area.inner(ratatui::layout::Margin {
        vertical: 1,
        horizontal: 0,
    });
    let view_height = inner.height as usize;

    let raw_offset = editor.sink_selector_state.offset();
    let _total = sinks.len();

    // Build visual_items to account for wrapping like in the selector rendering
    let mut visual_items: Vec<String> = Vec::new();
    visual_items.push("── Active Sinks ──".to_string());
    for sink in sinks {
        visual_items.push(format!("  {}", sink.desc));
    }

    // Compute per-row visual height using inner.width
    let content_width = inner.width as usize;
    let mut per_row_lines: Vec<usize> = Vec::with_capacity(visual_items.len());
    for s in &visual_items {
        let w = content_width.max(1);
        let lines = (s.len().saturating_add(w - 1)) / w;
        per_row_lines.push(lines.max(1));
    }

    let total_visual_lines: usize = per_row_lines.iter().sum();
    let mut visual_pos = 0usize;
    for lines in per_row_lines
        .iter()
        .take(raw_offset.min(per_row_lines.len()))
    {
        visual_pos += *lines;
    }

    let has_above = visual_pos > 0;
    let has_below = (visual_pos + view_height) < total_visual_lines;

    // Draw top arrow if there's more above
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

    // Draw bottom arrow if there's more below
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

/// Render delete confirmation
fn render_delete_confirmation(
    frame: &mut Frame,
    area: Rect,
    rules: &[Rule],
    screen_state: &RulesScreen,
) {
    if screen_state.selected >= rules.len() {
        return;
    }

    let rule = &rules[screen_state.selected];
    let popup_area = centered_modal(modal_size::SMALL, area);

    let title_line = if let Some(ref title) = rule.title_pattern {
        Line::from(vec![
            Span::raw("Title: "),
            Span::styled(title.as_str(), Style::default().fg(Color::White)),
        ])
    } else {
        Line::from("Title: (any)")
    };

    let text = vec![
        Line::from(""),
        Line::from(vec![Span::styled(
            "Are you sure you want to delete this rule?",
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        )]),
        Line::from(""),
        Line::from(vec![
            Span::raw("App ID: "),
            Span::styled(&rule.app_id_pattern, Style::default().fg(Color::White)),
        ]),
        title_line,
        Line::from(vec![
            Span::raw("Sink: "),
            Span::styled(&rule.sink_ref, Style::default().fg(Color::Yellow)),
        ]),
        Line::from(""),
        Line::from(vec![Span::styled(
            "Press Enter to confirm, Esc to cancel",
            Style::default().fg(Color::Yellow),
        )]),
    ];

    let block = Block::default()
        .borders(Borders::ALL)
        .title("Delete Rule")
        .style(Style::default().bg(Color::Black));

    let paragraph = Paragraph::new(text).block(block);
    frame.render_widget(paragraph, popup_area);
}
