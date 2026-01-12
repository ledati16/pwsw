//! Rules screen - Manage window matching rules

use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, BorderType, Borders, Cell, Clear, List, ListItem, ListState, Padding, Paragraph,
        Row, Table, TableState,
    },
};
use throbber_widgets_tui::{Throbber, ThrobberState};

use crate::config::{Rule, SinkConfig};
use crate::style::colors;
use crate::tui::editor_state::EditorState;
use crate::tui::widgets::{
    ValidationState, centered_modal, modal_size, render_input, render_validated_input,
};
use regex::Regex;
use std::fmt::Write;

/// Rules screen mode
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum RulesMode {
    #[default]
    List,
    AddEdit,
    Delete,
    SelectSink, // Dropdown for sink selection
    Inspect,
}

/// Editor state for add/edit modal
#[derive(Debug, Clone)]
pub(crate) struct RuleEditor {
    pub app_id_pattern: EditorState,
    pub title_pattern: EditorState,
    pub sink_ref: String,
    pub desc: EditorState,
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
    pub(crate) fn new() -> Self {
        Self {
            app_id_pattern: EditorState::new(),
            title_pattern: EditorState::new(),
            sink_ref: String::new(),
            desc: EditorState::new(),
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

    pub(crate) fn from_rule(rule: &Rule) -> Self {
        let compiled_app_id = Regex::new(&rule.app_id_pattern)
            .ok()
            .map(std::sync::Arc::new);
        let compiled_title = match &rule.title_pattern {
            Some(t) if !t.is_empty() => Regex::new(t).ok().map(std::sync::Arc::new),
            _ => None,
        };

        Self {
            app_id_pattern: EditorState::from_string(rule.app_id_pattern.clone()),
            title_pattern: EditorState::from_string(rule.title_pattern.clone().unwrap_or_default()),
            sink_ref: rule.sink_ref.clone(),
            desc: EditorState::from_string(rule.desc.clone().unwrap_or_default()),
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

    pub(crate) fn next_field(&mut self) {
        if self.focused_field < 4 {
            self.focused_field += 1;
        }
    }

    pub(crate) fn prev_field(&mut self) {
        if self.focused_field > 0 {
            self.focused_field -= 1;
        }
    }

    /// Ensure compiled regex caches are up-to-date for current editor patterns
    pub(crate) fn ensure_compiled(&mut self) {
        // Compile app id pattern if non-empty and store which string it corresponds to
        if self.app_id_pattern.value().is_empty() {
            self.compiled_app_id = None;
            self.compiled_app_id_for = None;
        } else if self.compiled_app_id_for.as_deref() != Some(self.app_id_pattern.value()) {
            self.compiled_app_id = Regex::new(self.app_id_pattern.value())
                .ok()
                .map(std::sync::Arc::new);
            // Only cache the pattern string if compilation succeeded
            if self.compiled_app_id.is_some() {
                self.compiled_app_id_for = Some(self.app_id_pattern.value().to_string());
            } else {
                // Don't cache invalid patterns
                self.compiled_app_id_for = None;
            }
        }

        // Compile title pattern if non-empty
        if self.title_pattern.value().is_empty() {
            self.compiled_title = None;
            self.compiled_title_for = None;
        } else if self.compiled_title_for.as_deref() != Some(self.title_pattern.value()) {
            self.compiled_title = Regex::new(self.title_pattern.value())
                .ok()
                .map(std::sync::Arc::new);
            // Only cache the pattern string if compilation succeeded
            if self.compiled_title.is_some() {
                self.compiled_title_for = Some(self.title_pattern.value().to_string());
            } else {
                // Don't cache invalid patterns
                self.compiled_title_for = None;
            }
        }
    }
}

/// Rules screen state
pub(crate) struct RulesScreen {
    pub mode: RulesMode,
    pub selected: usize,
    pub editor: RuleEditor,
    pub editing_index: Option<usize>,
    /// Table scroll state
    pub state: TableState,
}

impl RulesScreen {
    pub(crate) fn new() -> Self {
        Self {
            mode: RulesMode::List,
            selected: 0,
            editor: RuleEditor::new(),
            editing_index: None,
            state: TableState::default(),
        }
    }

    pub(crate) fn select_previous(&mut self, rule_count: usize) {
        if rule_count > 0 && self.selected > 0 {
            self.selected -= 1;
        }
    }

    pub(crate) fn select_next(&mut self, rule_count: usize) {
        if rule_count > 0 && self.selected < rule_count - 1 {
            self.selected += 1;
        }
    }

    pub(crate) fn start_add(&mut self) {
        self.mode = RulesMode::AddEdit;
        self.editor = RuleEditor::new();
        self.editing_index = None;
    }

    pub(crate) fn start_edit(&mut self, rules: &[Rule]) {
        if self.selected < rules.len() {
            self.mode = RulesMode::AddEdit;
            self.editor = RuleEditor::from_rule(&rules[self.selected]);
            self.editing_index = Some(self.selected);
        }
    }

    pub(crate) fn start_delete(&mut self) {
        self.mode = RulesMode::Delete;
    }

    pub(crate) fn open_sink_selector(&mut self) {
        self.mode = RulesMode::SelectSink;
    }

    pub(crate) fn start_inspect(&mut self) {
        self.mode = RulesMode::Inspect;
    }

    pub(crate) fn cancel(&mut self) {
        self.mode = RulesMode::List;
    }
}

/// Context for rendering the rules screen (bundles related parameters)
pub(crate) struct RulesRenderContext<'a> {
    pub rules: &'a [Rule],
    pub sinks: &'a [SinkConfig],
    pub screen_state: &'a mut RulesScreen,
    pub windows: &'a [crate::ipc::WindowInfo],
    pub preview: Option<&'a crate::tui::app::PreviewResult>,
    pub throbber_state: &'a mut ThrobberState,
}

/// Render the rules screen
pub(crate) fn render_rules(frame: &mut Frame, area: Rect, ctx: &mut RulesRenderContext) {
    match ctx.screen_state.mode {
        RulesMode::List => render_list(frame, area, ctx.rules, ctx.sinks, ctx.screen_state),
        RulesMode::AddEdit => render_editor(
            frame,
            area,
            ctx.sinks,
            ctx.screen_state,
            ctx.windows,
            ctx.preview,
            ctx.throbber_state,
        ),
        RulesMode::Delete => render_delete_confirmation(frame, area, ctx.rules, ctx.screen_state),
        RulesMode::SelectSink => {
            render_sink_selector(frame, area, ctx.sinks, &mut ctx.screen_state.editor);
        }
        RulesMode::Inspect => render_inspect_popup(frame, area, ctx.rules, ctx.screen_state),
    }
}

/// Render the rules list
// Rules list rendering - complex table with multiple columns and highlighting
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
            // Add selection indicator to index
            let index_val = (i + 1).to_string();
            let index_cell = if is_selected {
                Cell::from(Line::from(vec![
                    Span::styled("▎", Style::default().fg(colors::UI_HIGHLIGHT)),
                    Span::raw(index_val),
                ]))
            } else {
                Cell::from(index_val)
            };

            let app_id_cell = Cell::from(Span::styled(
                rule.app_id_pattern.as_str(),
                if is_selected {
                    Style::default()
                        .fg(colors::UI_TEXT)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(colors::UI_TEXT)
                },
            ));

            let title_cell = rule.title_pattern.as_ref().map_or_else(
                || Cell::from(Span::styled("*", Style::default().fg(colors::UI_SECONDARY))),
                |s| {
                    Cell::from(Span::styled(
                        s.as_str(),
                        Style::default().fg(colors::UI_SECONDARY),
                    ))
                },
            );

            let sink_cell = Cell::from(Span::styled(
                sink_display,
                Style::default().fg(colors::UI_HIGHLIGHT),
            ));

            let desc_cell = rule
                .desc
                .as_ref()
                .map_or_else(|| Cell::from(""), |s| Cell::from(s.as_str()));

            let row_style = if is_selected {
                Style::default().bg(colors::UI_SELECTED_BG)
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
                .fg(colors::UI_WARNING)
                .add_modifier(Modifier::BOLD),
        )
        .bottom_margin(1),
    )
    .block(
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .title(" Rules "),
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

    // Render scroll arrows using helper
    let (has_above, has_below) = (has_above, has_below);
    crate::tui::widgets::render_scroll_arrows(frame, inner, has_above, has_below);
}

/// Render inspect modal
fn render_inspect_popup(frame: &mut Frame, area: Rect, rules: &[Rule], screen_state: &RulesScreen) {
    if screen_state.selected >= rules.len() {
        return;
    }
    let rule = &rules[screen_state.selected];

    let popup_area = centered_modal(modal_size::MEDIUM, area);
    frame.render_widget(Clear, popup_area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .title(" Rule Details ")
        .style(Style::default().bg(colors::UI_MODAL_BG));
    frame.render_widget(block.clone(), popup_area);

    let inner = block.inner(popup_area);

    let mut lines = Vec::new();

    // Helper for fields
    let mut add_field = |label: &str, value: &str| {
        lines.push(Line::from(vec![
            Span::styled(
                format!("{label}: "),
                Style::default().fg(colors::UI_SECONDARY),
            ),
            Span::styled(value.to_string(), Style::default().fg(colors::UI_TEXT)),
        ]));
        lines.push(Line::from(""));
    };

    add_field("App ID Pattern", &rule.app_id_pattern);

    if let Some(title) = &rule.title_pattern {
        add_field("Title Pattern", title);
    } else {
        add_field("Title Pattern", "(any title)");
    }

    add_field("Target Sink", &rule.sink_ref);

    if let Some(desc) = &rule.desc {
        add_field("Description", desc);
    }

    // Notify status
    let notify_status = match rule.notify {
        Some(true) => "Enabled (override)",
        Some(false) => "Disabled (override)",
        None => "Default (use global setting)",
    };
    add_field("Notifications", notify_status);

    // Hint at bottom
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "Press [Enter] or [Esc] to close",
        Style::default().fg(colors::UI_HIGHLIGHT),
    )));

    let paragraph = Paragraph::new(lines).wrap(ratatui::widgets::Wrap { trim: false });

    frame.render_widget(paragraph, inner);
}

/// Render the add/edit modal
// Rule editor modal - complex form with multiple fields and validation
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

    let [
        app_id_area,
        title_area,
        sink_area,
        desc_area,
        notify_area,
        preview_area,
    ] = Layout::vertical([
        Constraint::Length(3), // App ID pattern
        Constraint::Length(3), // Title pattern
        Constraint::Length(3), // Sink selector
        Constraint::Length(3), // Description
        Constraint::Length(3), // Notify toggle
        Constraint::Min(6),    // Live preview
    ])
    .margin(2)
    .areas(popup_area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .padding(Padding::horizontal(1))
        .title(title)
        .style(Style::default().bg(colors::UI_MODAL_BG));
    frame.render_widget(block, popup_area);

    // App ID pattern field with real-time validation
    let app_id_validation = if screen_state.editor.app_id_pattern.value().is_empty() {
        ValidationState::Neutral
    } else if screen_state.editor.compiled_app_id.is_some() {
        ValidationState::Valid
    } else {
        ValidationState::Invalid
    };

    render_validated_input(
        frame,
        app_id_area,
        "App ID Pattern (regex):",
        &screen_state.editor.app_id_pattern.input,
        screen_state.editor.focused_field == 0,
        app_id_validation,
    );

    // Title pattern field with real-time validation
    let title_validation = if screen_state.editor.title_pattern.value().is_empty() {
        ValidationState::Neutral // Empty is OK for title (optional)
    } else if screen_state.editor.compiled_title.is_some() {
        ValidationState::Valid
    } else {
        ValidationState::Invalid
    };

    render_validated_input(
        frame,
        title_area,
        "Title Pattern (optional regex):",
        &screen_state.editor.title_pattern.input,
        screen_state.editor.focused_field == 1,
        title_validation,
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
        sink_area,
        "Target Sink",
        sink_display,
        screen_state.editor.focused_field == 2,
    );

    // Description field
    render_input(
        frame,
        desc_area,
        "Description (optional):",
        &screen_state.editor.desc.input,
        screen_state.editor.focused_field == 3,
    );

    // Notify toggle with border-based focus
    let mut notify_spans = Vec::new();
    match screen_state.editor.notify {
        Some(true) => {
            notify_spans.push(Span::styled("✓ ", Style::default().fg(colors::UI_SUCCESS)));
            notify_spans.push(Span::raw("Notify (enabled)"));
        }
        Some(false) => {
            notify_spans.push(Span::styled("✗ ", Style::default().fg(colors::UI_ERROR)));
            notify_spans.push(Span::raw("Notify (disabled)"));
        }
        None => {
            notify_spans.push(Span::styled(
                "○ ",
                Style::default().fg(colors::UI_SECONDARY),
            ));
            notify_spans.push(Span::raw("Notify (use global setting)"));
        }
    }

    let border_style =
        crate::tui::widgets::focus_border_style(screen_state.editor.focused_field == 4);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(border_style);

    let notify_widget = Paragraph::new(Line::from(notify_spans)).block(block);
    frame.render_widget(notify_widget, notify_area);

    // Live preview panel
    render_live_preview(
        frame,
        preview_area,
        screen_state,
        windows,
        preview,
        throbber_state,
    );
}

/// Render live regex preview showing matching windows
// Live preview rendering - complex async state display with multiple modes
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
                    .border_type(BorderType::Rounded)
                    .title("Matching Windows");
                let inner = block.inner(area);
                frame.render_widget(block, area);

                let throbber = Throbber::default()
                    .label("Computing matches...")
                    .style(Style::default().fg(colors::UI_WARNING))
                    .throbber_style(
                        Style::default()
                            .fg(colors::UI_WARNING)
                            .add_modifier(Modifier::BOLD),
                    );
                frame.render_stateful_widget(throbber, inner, throbber_state);
                return;
            }

            // Normal results display
            let mut preview_lines = vec![];

            if let Some(ref error) = res.regex_error {
                // Invalid regex error (distinct from timeout)
                preview_lines.push(Line::from(vec![Span::styled(
                    format!("  Invalid regex: {error}"),
                    Style::default().fg(colors::UI_ERROR),
                )]));
            } else if res.timed_out {
                // Timeout (no regex error)
                preview_lines.push(Line::from(vec![Span::styled(
                    "  Preview timed out (200ms)",
                    Style::default().fg(colors::UI_WARNING),
                )]));
            } else if res.matches.is_empty() {
                preview_lines.push(Line::from(vec![Span::styled(
                    "  No matching windows",
                    Style::default().fg(colors::UI_SECONDARY),
                )]));
            } else {
                // Use helper to convert strings -> Lines, preserving the limit of 5 shown items.
                let lines = crate::tui::preview::build_preview_lines_from_strings(
                    &res.matches[..res.matches.len().min(5)],
                );
                preview_lines.extend(lines);

                if res.matches.len() > 5 {
                    let remaining = res.matches.len() - 5;
                    let mut text = String::with_capacity(12);
                    let _ = write!(text, "  ...and {remaining} more");
                    preview_lines.push(Line::from(vec![Span::styled(
                        text,
                        Style::default().fg(colors::UI_SECONDARY),
                    )]));
                }
            }

            let preview_widget = Paragraph::new(preview_lines).block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
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

    if let Some(_app_regex) = app_id_regex_ref
        && windows.is_empty()
    {
        preview_lines.push(Line::from(vec![Span::styled(
            "  (daemon not running)",
            Style::default().fg(colors::UI_SECONDARY),
        )]));
    } else if let Some(app_regex) = app_id_regex_ref {
        // Use helper to perform matching with compiled regex refs and get both the preview strings and total count.
        let (matches_vec, total) = crate::tui::preview::match_windows_with_compiled_count(
            Some(app_regex),
            title_regex_ref,
            windows,
            5,
        );

        if matches_vec.is_empty() {
            preview_lines.push(Line::from(vec![Span::styled(
                "  No matching windows",
                Style::default().fg(colors::UI_WARNING),
            )]));
        } else {
            preview_lines.extend(crate::tui::preview::build_preview_lines_from_strings(
                &matches_vec,
            ));
            if total > 5 {
                let remaining = total - 5;
                let mut text = String::with_capacity(12);
                let _ = write!(text, "  ...and {remaining} more");
                preview_lines.push(Line::from(vec![Span::styled(
                    text,
                    Style::default().fg(colors::UI_SECONDARY),
                )]));
            }
        }
    } else if !screen_state.editor.app_id_pattern.value().is_empty() {
        preview_lines.push(Line::from(vec![Span::styled(
            "  Invalid regex pattern",
            Style::default().fg(colors::UI_ERROR),
        )]));
    } else {
        preview_lines.push(Line::from(vec![Span::styled(
            "  Enter app_id pattern to see preview",
            Style::default().fg(colors::UI_SECONDARY),
        )]));
    }

    let preview_widget = Paragraph::new(preview_lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .title("Matching Windows"),
        )
        .wrap(ratatui::widgets::Wrap { trim: false });
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

    // Use truncation helpers to avoid overly long sink descriptions/names
    let inner = popup_area.inner(ratatui::layout::Margin {
        vertical: 1,
        horizontal: 0,
    });
    let content_width = inner.width.max(1);

    let items: Vec<ListItem> = sinks
        .iter()
        .map(|sink| {
            let desc = crate::tui::widgets::truncate_desc(&sink.desc, content_width);
            let name = crate::tui::widgets::truncate_node_name(&sink.name, 35);

            let line = Line::from(vec![
                Span::raw("  "),
                Span::styled(desc, Style::default().fg(colors::UI_TEXT)),
                Span::styled(" (", Style::default().fg(colors::UI_SECONDARY)),
                Span::styled(
                    name,
                    Style::default()
                        .fg(colors::UI_SECONDARY)
                        .add_modifier(Modifier::DIM),
                ),
                Span::styled(")", Style::default().fg(colors::UI_SECONDARY)),
            ]);

            ListItem::new(line)
        })
        .collect();

    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .padding(Padding::horizontal(1))
                .title("Select Target Sink")
                .style(Style::default().bg(colors::UI_MODAL_BG)),
        )
        .highlight_style(
            Style::default()
                .fg(colors::UI_HIGHLIGHT)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol(" →")
        .scroll_padding(1);

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
    for sink in sinks {
        let desc_text = crate::tui::widgets::truncate_desc(&sink.desc, content_width);
        let name_text = crate::tui::widgets::truncate_node_name(&sink.name, 35);
        visual_items.push({
            let mut tmp = String::with_capacity(2 + desc_text.len() + 3 + name_text.len());
            let _ = write!(tmp, "  {desc_text} ({name_text})");
            tmp
        });
    }

    // Compute content width and logical offset
    let content_width = inner.width as usize;

    // Use helper to compute whether content exists above/below (accounts for wrapping)
    let (has_above, has_below) = crate::tui::widgets::compute_has_above_below(
        &visual_items,
        content_width,
        raw_offset,
        view_height,
    );

    // Render arrows via helper
    crate::tui::widgets::render_scroll_arrows(frame, inner, has_above, has_below);
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
            Span::styled(title.as_str(), Style::default().fg(colors::UI_TEXT)),
        ])
    } else {
        Line::from("Title: (any)")
    };

    let text = vec![
        Line::from(""),
        Line::from(vec![Span::styled(
            "Are you sure you want to delete this rule?",
            Style::default()
                .fg(colors::UI_ERROR)
                .add_modifier(Modifier::BOLD),
        )]),
        Line::from(""),
        Line::from(vec![
            Span::raw("App ID: "),
            Span::styled(&rule.app_id_pattern, Style::default().fg(colors::UI_TEXT)),
        ]),
        title_line,
        Line::from(vec![
            Span::raw("Sink: "),
            Span::styled(&rule.sink_ref, Style::default().fg(colors::UI_WARNING)),
        ]),
    ];

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Double)
        .padding(Padding::horizontal(1))
        .title("Delete Rule")
        .style(Style::default().bg(colors::UI_MODAL_BG));

    let paragraph = Paragraph::new(text).block(block);
    frame.render_widget(paragraph, popup_area);
}
