//! Rules screen - Manage window matching rules

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph},
    Frame,
};

use crate::config::{Rule, SinkConfig};
use crate::tui::widgets::centered_rect;
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
    pub app_id_pattern: String,
    pub title_pattern: String,
    pub sink_ref: String,
    pub desc: String,
    pub notify: Option<bool>,
    pub focused_field: usize, // 0=app_id, 1=title, 2=sink, 3=desc, 4=notify
    pub sink_dropdown_index: usize,
    // Cached compiled regexes to avoid recompiling on every render
    pub compiled_app_id: Option<Regex>,
    pub compiled_title: Option<Regex>,
    // Track which pattern strings the compiled regex corresponds to
    pub compiled_app_id_for: Option<String>,
    pub compiled_title_for: Option<String>,
}

impl RuleEditor {
    pub fn new() -> Self {
        Self {
            app_id_pattern: String::new(),
            title_pattern: String::new(),
            sink_ref: String::new(),
            desc: String::new(),
            notify: None,
            focused_field: 0,
            sink_dropdown_index: 0,
            compiled_app_id: None,
            compiled_title: None,
            compiled_app_id_for: None,
            compiled_title_for: None,
        }
    }

    pub fn from_rule(rule: &Rule) -> Self {
        let compiled_app_id = Regex::new(&rule.app_id_pattern).ok();
        let compiled_title = match &rule.title_pattern {
            Some(t) if !t.is_empty() => Regex::new(t).ok(),
            _ => None,
        };

        Self {
            app_id_pattern: rule.app_id_pattern.clone(),
            title_pattern: rule.title_pattern.clone().unwrap_or_default(),
            sink_ref: rule.sink_ref.clone(),
            desc: rule.desc.clone().unwrap_or_default(),
            notify: rule.notify,
            focused_field: 0,
            sink_dropdown_index: 0,
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
}

/// Rules screen state
pub struct RulesScreen {
    pub mode: RulesMode,
    pub selected: usize,
    pub editor: RuleEditor,
    pub editing_index: Option<usize>,
}

impl RulesScreen {
    pub fn new() -> Self {
        Self {
            mode: RulesMode::List,
            selected: 0,
            editor: RuleEditor::new(),
            editing_index: None,
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
    screen_state: &RulesScreen,
    windows: &[crate::ipc::WindowInfo],
    preview: Option<&crate::tui::app::PreviewResult>,
    spinner_idx: usize,
) {
    match screen_state.mode {
        RulesMode::List => render_list(frame, area, rules, sinks, screen_state),
        RulesMode::AddEdit => render_editor(frame, area, sinks, screen_state, windows, preview, spinner_idx),
        RulesMode::Delete => render_delete_confirmation(frame, area, rules, screen_state),
        RulesMode::SelectSink => render_sink_selector(frame, area, sinks, screen_state),
    }
}

/// Render the rules list
fn render_list(
    frame: &mut Frame,
    area: Rect,
    rules: &[Rule],
    sinks: &[SinkConfig],
    screen_state: &RulesScreen,
) {
    let items: Vec<ListItem> = rules
        .iter()
        .enumerate()
        .map(|(i, rule)| {
            let is_selected = i == screen_state.selected;

            // Find sink description
            let sink_desc = sinks
                .iter()
                .find(|s| s.name == rule.sink_ref || s.desc == rule.sink_ref)
                .map(|s| s.desc.as_str())
                .unwrap_or(&rule.sink_ref);

            let title_info = if let Some(ref title) = rule.title_pattern {
                format!(" + title: {}", title)
            } else {
                String::new()
            };

            let style = if is_selected {
                Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            };

            let mut lines = vec![
                Line::from(vec![
                    Span::styled(
                        if is_selected { "> " } else { "  " },
                        Style::default().fg(Color::Cyan),
                    ),
                    Span::styled(
                        format!("{}. app_id: {}{}", i + 1, rule.app_id_pattern, title_info),
                        style,
                    ),
                ]),
                Line::from(vec![
                    Span::raw("     → "),
                    Span::styled(sink_desc, Style::default().fg(Color::Yellow)),
                ]),
            ];

            // Add description if present
            if let Some(ref desc) = rule.desc {
                lines.push(Line::from(vec![
                    Span::raw("     "),
                    Span::styled(desc, Style::default().fg(Color::Gray)),
                ]));
            }

            ListItem::new(lines)
        })
        .collect();

    let list = List::new(items).block(
        Block::default()
            .borders(Borders::ALL)
            .title("Rules ([a]dd, [e]dit, [x] delete, [↑/↓] move priority, Ctrl+S save)"),
    );

    frame.render_widget(list, area);
}

/// Render the add/edit modal
fn render_editor(frame: &mut Frame, area: Rect, sinks: &[SinkConfig], screen_state: &RulesScreen, windows: &[crate::ipc::WindowInfo], preview: Option<&crate::tui::app::PreviewResult>, spinner_idx: usize) {
    let title = if screen_state.editing_index.is_some() {
        "Edit Rule"
    } else {
        "Add Rule"
    };

    let popup_area = centered_rect(80, 85, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(2)
        .constraints([
            Constraint::Length(3), // App ID pattern
            Constraint::Length(3), // Title pattern
            Constraint::Length(3), // Sink selector
            Constraint::Length(3), // Description
            Constraint::Length(3), // Notify toggle
            Constraint::Min(5),    // Live preview
            Constraint::Length(3), // Help text
        ])
        .split(popup_area);

    let block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .style(Style::default().bg(Color::Black));
    frame.render_widget(block, popup_area);

    // App ID pattern field
    render_text_field(
        frame,
        chunks[0],
        "App ID Pattern (regex):",
        &screen_state.editor.app_id_pattern,
        screen_state.editor.focused_field == 0,
    );

    // Title pattern field
    render_text_field(
        frame,
        chunks[1],
        "Title Pattern (optional regex):",
        &screen_state.editor.title_pattern,
        screen_state.editor.focused_field == 1,
    );

    // Sink selector
    let sink_display = if screen_state.editor.sink_ref.is_empty() {
        "<press Enter to select>".to_string()
    } else {
        // Try to find the sink and show its description
        sinks
            .iter()
            .find(|s| s.name == screen_state.editor.sink_ref || s.desc == screen_state.editor.sink_ref)
            .map(|s| format!("{} ({})", s.desc, s.name))
            .unwrap_or_else(|| screen_state.editor.sink_ref.clone())
    };

    let sink_style = if screen_state.editor.focused_field == 2 {
        Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::White)
    };
    let sink_widget = Paragraph::new(format!("Target Sink: {}", sink_display)).style(sink_style);
    frame.render_widget(sink_widget, chunks[2]);

    // Description field
    render_text_field(
        frame,
        chunks[3],
        "Description (optional):",
        &screen_state.editor.desc,
        screen_state.editor.focused_field == 3,
    );

    // Notify toggle
    let notify_text = match screen_state.editor.notify {
        Some(true) => "✓ Notify (enabled)",
        Some(false) => "✗ Notify (disabled)",
        None => "○ Notify (use global setting)",
    };
    let notify_style = if screen_state.editor.focused_field == 4 {
        Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::White)
    };
    let notify_widget = Paragraph::new(notify_text).style(notify_style);
    frame.render_widget(notify_widget, chunks[4]);

    // Live preview panel
    render_live_preview(frame, chunks[5], screen_state, windows, preview, spinner_idx);

    // Help text
    let help = vec![
        Line::from(vec![
            Span::raw("Tab/Shift+Tab: Next/Prev  |  "),
            Span::raw("Enter: Save/Select Sink  |  "),
            Span::raw("Space: Toggle  |  "),
            Span::raw("Esc: Cancel"),
        ]),
    ];
    let help_widget = Paragraph::new(help).style(Style::default().fg(Color::Gray));
    frame.render_widget(help_widget, chunks[6]);
}

/// Render live regex preview showing matching windows
fn render_live_preview(frame: &mut Frame, area: Rect, screen_state: &RulesScreen, windows: &[crate::ipc::WindowInfo], preview: Option<&crate::tui::app::PreviewResult>, spinner_idx: usize) {
    // If background worker supplied a preview and it matches current editor patterns, render it
    let mut preview_lines = vec![
        Line::from(vec![
            Span::styled("Live Preview: ", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
        ]),
    ];

    if let Some(res) = preview {
        // Ensure preview corresponds to current editor content
        if res.app_pattern == screen_state.editor.app_id_pattern && res.title_pattern.as_ref().map_or("".to_string(), |s| s.clone()) == screen_state.editor.title_pattern {
            // If background worker marked this preview as pending, show spinner (computing). Otherwise
            // fall through to normal display (No matches / timed out / results).
            if res.pending && res.matches.is_empty() && !res.timed_out {
                // Show spinner instead of static "Computing..."
                let spinner_frames = ["⠋","⠙","⠹","⠸","⠼","⠴","⠦","⠧","⠇","⠏"];
                // Use app-level spinner index (passed via rules screen state in App) — render frame from spinner_idx
                // to animate across UI ticks.
                preview_lines.push(Line::from(vec![Span::styled(format!("  {} Computing...", spinner_frames[spinner_idx % spinner_frames.len()]), Style::default().fg(Color::Yellow))]));

                let preview_widget = Paragraph::new(preview_lines)
                    .block(Block::default().borders(Borders::ALL).title("Matching Windows"));
                frame.render_widget(preview_widget, area);
                return;
            }

            if res.timed_out {
                preview_lines.push(Line::from(vec![Span::styled("  Preview timed out or invalid regex", Style::default().fg(Color::Red))]));
            } else if res.matches.is_empty() {
                preview_lines.push(Line::from(vec![Span::styled("  No matching windows", Style::default().fg(Color::Yellow))]));
            } else {
                for m in res.matches.iter().take(5) {
                    preview_lines.push(Line::from(vec![Span::styled("  ✓ ", Style::default().fg(Color::Green)), Span::raw(m)]));
                }
                if res.matches.len() > 5 {
                    preview_lines.push(Line::from(vec![Span::styled(format!("  ... and {} more", res.matches.len() - 5), Style::default().fg(Color::Gray))]));
                }
            }

            let preview_widget = Paragraph::new(preview_lines)
                .block(Block::default().borders(Borders::ALL).title("Matching Windows"));
            frame.render_widget(preview_widget, area);
            return;
        }
    }

    // Fallback: use local compiled regex matching (fast for small window lists).
    // Ensure compiled regexes correspond to current editor text; compile if needed.
    // Attempt to use cached compiled regex references, or compile temporary ones for this render.
    let app_id_regex: Option<Regex> = if screen_state.editor.app_id_pattern.is_empty() {
        None
    } else if screen_state.editor.compiled_app_id_for.as_ref() == Some(&screen_state.editor.app_id_pattern) {
        screen_state.editor.compiled_app_id.clone()
    } else {
        Regex::new(&screen_state.editor.app_id_pattern).ok()
    };

    let title_regex: Option<Regex> = if screen_state.editor.title_pattern.is_empty() {
        None
    } else if screen_state.editor.compiled_title_for.as_ref() == Some(&screen_state.editor.title_pattern) {
        screen_state.editor.compiled_title.clone()
    } else {
        Regex::new(&screen_state.editor.title_pattern).ok()
    };

    // Convert to Option<&Regex> for the matching code below
    let app_id_regex_ref = app_id_regex.as_ref();
    let title_regex_ref = title_regex.as_ref();

    if let Some(app_regex) = app_id_regex_ref {
        if !windows.is_empty() {
            let mut match_count = 0;
            let mut shown = 0;

            for window in windows.iter().take(10) {
                let app_id_match = app_regex.is_match(&window.app_id);
                let title_match = title_regex_ref.map_or(true, |r| r.is_match(&window.title));

                if app_id_match && title_match {
                    match_count += 1;
                    if shown < 5 {
                        preview_lines.push(Line::from(vec![Span::styled("  ✓ ", Style::default().fg(Color::Green)), Span::raw(format!("{} | {}", window.app_id, window.title))]));
                        shown += 1;
                    }
                }
            }

            if match_count == 0 {
                preview_lines.push(Line::from(vec![Span::styled("  No matching windows", Style::default().fg(Color::Yellow))]));
            } else if match_count > 5 {
                preview_lines.push(Line::from(vec![Span::styled(format!("  ... and {} more", match_count - 5), Style::default().fg(Color::Gray))]));
            }
        } else {
            preview_lines.push(Line::from(vec![Span::styled("  (daemon not running)", Style::default().fg(Color::Gray))]));
        }
    } else if !screen_state.editor.app_id_pattern.is_empty() {
        preview_lines.push(Line::from(vec![Span::styled("  Invalid regex pattern", Style::default().fg(Color::Red))]));
    } else {
        preview_lines.push(Line::from(vec![Span::styled("  Enter app_id pattern to see preview", Style::default().fg(Color::Gray))]));
    }

    let preview_widget = Paragraph::new(preview_lines)
        .block(Block::default().borders(Borders::ALL).title("Matching Windows"));
    frame.render_widget(preview_widget, area);
}

/// Render sink selector dropdown
fn render_sink_selector(frame: &mut Frame, area: Rect, sinks: &[SinkConfig], screen_state: &RulesScreen) {
    let popup_area = centered_rect(60, 50, area);

    let items: Vec<ListItem> = sinks
        .iter()
        .enumerate()
        .map(|(i, sink)| {
            let is_selected = i == screen_state.editor.sink_dropdown_index;
            let style = if is_selected {
                Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            };

            let line = Line::from(vec![
                Span::styled(
                    if is_selected { "> " } else { "  " },
                    Style::default().fg(Color::Cyan),
                ),
                Span::styled(&sink.desc, style),
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

    let list = List::new(items).block(
        Block::default()
            .borders(Borders::ALL)
            .title("Select Target Sink (↑/↓, Enter to confirm, Esc to cancel)")
            .style(Style::default().bg(Color::Black)),
    );

    frame.render_widget(list, popup_area);
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
    let popup_area = centered_rect(60, 40, area);

    let title_info = if let Some(ref title) = rule.title_pattern {
        format!("Title: {}", title)
    } else {
        "Title: (any)".to_string()
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
        Line::from(vec![
            Span::raw(&title_info),
        ]),
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

/// Render text field
fn render_text_field(frame: &mut Frame, area: Rect, label: &str, value: &str, focused: bool) {
    let style = if focused {
        Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::White)
    };

    let cursor = if focused { "█" } else { "" };
    let text = format!("{} {}{}", label, value, cursor);
    let paragraph = Paragraph::new(text).style(style);
    frame.render_widget(paragraph, area);
}
