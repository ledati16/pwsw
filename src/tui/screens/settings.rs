//! Settings screen - Configure PWSW behavior

use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, List, ListItem, ListState, Padding, Paragraph},
};

use crate::config::Settings;
use crate::style::colors;
use crate::tui::widgets::{centered_modal, modal_size};

/// Minimum height of the description panel at the bottom of the settings screen
const DESCRIPTION_PANEL_MIN_HEIGHT: u16 = 14;

/// Target percentage of the screen height for the description panel
const DESCRIPTION_PANEL_PERCENT: u16 = 40;

/// Selected setting item
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingItem {
    DefaultOnStartup,
    SetSmartToggle,
    NotifyManual,
    NotifyRules,
    MatchByIndex,
    LogLevel,
}

impl SettingItem {
    /// Get all settings in display order
    pub(crate) const fn all() -> &'static [Self] {
        &[
            Self::DefaultOnStartup,
            Self::SetSmartToggle,
            Self::NotifyManual,
            Self::NotifyRules,
            Self::MatchByIndex,
            Self::LogLevel,
        ]
    }

    /// Get the display name for this setting
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::DefaultOnStartup => "Default on Startup",
            Self::SetSmartToggle => "Smart Toggle",
            Self::NotifyManual => "Manual Switch Notifications",
            Self::NotifyRules => "Rule-Based Notifications",
            Self::MatchByIndex => "Match by Rule Index",
            Self::LogLevel => "Log Level",
        }
    }

    /// Get short description for this setting
    pub(crate) const fn description(self) -> &'static str {
        match self {
            Self::DefaultOnStartup => "Switch to default sink when daemon starts",
            Self::SetSmartToggle => "Intelligent toggling for manual sink switches",
            Self::NotifyManual => "Show notifications for manual sink switches",
            Self::NotifyRules => "Show notifications for rule-triggered switches",
            Self::MatchByIndex => "Rule priority strategy for window matching",
            Self::LogLevel => "Logging verbosity level",
        }
    }

    /// Get detailed description with examples for this setting
    pub(crate) const fn detailed_description(self) -> &'static str {
        match self {
            Self::DefaultOnStartup => {
                "Automatically switches to the configured default sink when the daemon starts.\n\
                 \n\
                 When enabled: Daemon activates default sink on startup.\n\
                 When disabled: Leaves the currently active sink unchanged.\n\
                 \n\
                 Useful for ensuring a consistent audio output when the daemon starts.\n\
                 \n\
                 Default: disabled"
            }
            Self::SetSmartToggle => {
                "Intelligent toggling behavior for manual sink switches via CLI.\n\
                 \n\
                 When enabled: Running 'pwsw set-sink <name>' toggles back to default\n\
                 if the sink is already active.\n\
                 When disabled: Always switches to the specified sink, even if already active.\n\
                 \n\
                 Example: If headphones are active:\n\
                 • Enabled: 'pwsw set-sink headphones' → switches to default sink\n\
                 • Disabled: 'pwsw set-sink headphones' → stays on headphones\n\
                 \n\
                 Default: disabled"
            }
            Self::NotifyManual => {
                "Desktop notifications for manual sink switches and daemon lifecycle events.\n\
                 \n\
                 When enabled: Shows notifications for:\n\
                 • Manual sink switches: 'pwsw set-sink <name>'\n\
                 • Cycling commands: 'pwsw prev-sink' and 'pwsw next-sink'\n\
                 • Daemon lifecycle: start and stop events\n\
                 \n\
                 When disabled: All manual operations happen silently.\n\
                 \n\
                 Requires a notification daemon (e.g., dunst, mako) to be running.\n\
                 \n\
                 Default: enabled"
            }
            Self::NotifyRules => {
                "Desktop notifications for automatic rule-triggered sink switches.\n\
                 \n\
                 When enabled: Shows notification when daemon switches sink due to a\n\
                 window matching a rule.\n\
                 When disabled: Rule-based switches happen silently.\n\
                 \n\
                 Useful for debugging rules or understanding why switches occur.\n\
                 \n\
                 Default: enabled"
            }
            Self::MatchByIndex => {
                "Rule priority strategy when multiple windows match different rules.\n\
                 \n\
                 When enabled: Uses rule priority - higher priority rules always win.\n\
                 Rules at the top of the list have higher priority than those below.\n\
                 When disabled: Most recently opened matching window determines active sink.\n\
                 \n\
                 Example: Firefox (rule 1, higher priority) and Discord (rule 2, lower priority):\n\
                 • Enabled: Firefox's sink stays active regardless of which window opened last\n\
                 • Disabled: Whichever window you focused most recently determines the sink\n\
                 \n\
                 Tip: Reorder rules in the Rules tab (arrow keys + Shift+Up/Down) to adjust priority.\n\
                 \n\
                 Default: disabled (most recent window)"
            }
            Self::LogLevel => {
                "Logging verbosity level for daemon output.\n\
                 \n\
                 Levels (from least to most verbose):\n\
                 • error: Only critical errors\n\
                 • warn: Warnings and errors\n\
                 • info: General information (recommended)\n\
                 • debug: Detailed debugging information\n\
                 • trace: Very verbose tracing (for development)\n\
                 \n\
                 View logs with: journalctl --user -u pwsw -f\n\
                 Or in TUI: Dashboard → [l] for logs view\n\
                 \n\
                 Default: info"
            }
        }
    }

    /// Check if this setting requires daemon restart to take effect
    pub(crate) const fn requires_restart(self) -> bool {
        matches!(self, Self::MatchByIndex | Self::LogLevel)
    }
}

/// Settings screen state
pub struct SettingsScreen {
    /// Currently selected item
    pub selected: usize,
    /// Whether we're editing the log level (dropdown open)
    pub editing_log_level: bool,
    /// Selected log level index (0-4 for error/warn/info/debug/trace)
    pub log_level_index: usize,
    /// Cached padded display names for settings (left-aligned)
    pub padded_names: Vec<String>,
    /// List scroll state
    pub state: ListState,
    /// Description panel scroll offset
    pub desc_scroll: u16,
}

impl SettingsScreen {
    /// Create a new settings screen
    pub(crate) fn new(settings: &Settings) -> Self {
        let log_level_index = match settings.log_level.as_str() {
            "error" => 0,
            "warn" => 1,
            "debug" => 3,
            "trace" => 4,
            _ => 2, // Default to info
        };

        // Build padded names cache based on longest setting name
        let names: Vec<String> = SettingItem::all()
            .iter()
            .map(|i| i.name().to_string())
            .collect();
        let max_len = names.iter().map(String::len).max().unwrap_or(0);
        let padded_names = names
            .into_iter()
            .map(|n| format!("{n:<max_len$}"))
            .collect::<Vec<_>>();

        Self {
            selected: 0,
            editing_log_level: false,
            log_level_index,
            padded_names,
            state: ListState::default(),
            desc_scroll: 0,
        }
    }

    /// Move selection up
    pub(crate) const fn select_previous(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
            self.desc_scroll = 0; // Reset description scroll on selection change
        }
    }

    /// Move selection down
    pub(crate) const fn select_next(&mut self) {
        if self.selected < SettingItem::all().len() - 1 {
            self.selected += 1;
            self.desc_scroll = 0; // Reset description scroll on selection change
        }
    }

    /// Scroll description up
    pub(crate) const fn scroll_desc_up(&mut self) {
        self.desc_scroll = self.desc_scroll.saturating_sub(1);
    }

    /// Scroll description down
    pub(crate) const fn scroll_desc_down(&mut self) {
        self.desc_scroll = self.desc_scroll.saturating_add(1);
    }

    /// Toggle the currently selected boolean setting
    pub(crate) fn toggle_current(&mut self, settings: &mut Settings) -> bool {
        if self.editing_log_level {
            return false; // Don't toggle while editing log level
        }

        match SettingItem::all()[self.selected] {
            SettingItem::DefaultOnStartup => {
                settings.default_on_startup = !settings.default_on_startup;
                true
            }
            SettingItem::SetSmartToggle => {
                settings.set_smart_toggle = !settings.set_smart_toggle;
                true
            }
            SettingItem::NotifyManual => {
                settings.notify_manual = !settings.notify_manual;
                true
            }
            SettingItem::NotifyRules => {
                settings.notify_rules = !settings.notify_rules;
                true
            }
            SettingItem::MatchByIndex => {
                settings.match_by_index = !settings.match_by_index;
                true
            }
            SettingItem::LogLevel => {
                // Open dropdown for editing
                self.editing_log_level = true;
                false
            }
        }
    }

    /// Get the current selected item
    pub(crate) fn current_item(&self) -> SettingItem {
        SettingItem::all()[self.selected]
    }
}

/// Render the settings screen
pub fn render_settings(
    frame: &mut Frame,
    area: Rect,
    settings: &Settings,
    screen_state: &mut SettingsScreen,
) {
    // Calculate dynamic height: 40% of screen, but at least 14 lines
    let desc_height =
        (area.height * DESCRIPTION_PANEL_PERCENT / 100).max(DESCRIPTION_PANEL_MIN_HEIGHT);

    // Split into [settings list | description]
    let [list_area, desc_area] = Layout::vertical([
        Constraint::Min(10),             // Settings list
        Constraint::Length(desc_height), // Description (dynamic height)
    ])
    .areas(area);

    // Render settings list
    render_settings_list(frame, list_area, settings, screen_state);

    // Render description
    render_description(frame, desc_area, screen_state);
}

/// Render the settings list
// Settings screen rendering - complex interactive list with multiple field types
fn render_settings_list(
    frame: &mut Frame,
    area: Rect,
    settings: &Settings,
    screen_state: &mut SettingsScreen,
) {
    let items: Vec<ListItem> = SettingItem::all()
        .iter()
        .enumerate()
        .map(|(i, item)| {
            let value_text = get_setting_value(*item, settings);
            let is_selected = i == screen_state.selected;

            let style = if is_selected {
                Style::default()
                    .fg(colors::UI_TEXT)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(colors::UI_TEXT)
            };

            let padded_name = screen_state
                .padded_names
                .get(i)
                .map_or(item.name(), String::as_str);

            // Apply color styling to boolean toggles
            let value_span = if *item == SettingItem::LogLevel {
                Span::styled(value_text, style)
            } else {
                let enabled = match item {
                    SettingItem::DefaultOnStartup => settings.default_on_startup,
                    SettingItem::SetSmartToggle => settings.set_smart_toggle,
                    SettingItem::NotifyManual => settings.notify_manual,
                    SettingItem::NotifyRules => settings.notify_rules,
                    SettingItem::MatchByIndex => settings.match_by_index,
                    SettingItem::LogLevel => false,
                };

                if enabled {
                    Span::styled("✓ enabled", Style::default().fg(colors::UI_SUCCESS))
                } else {
                    Span::styled("✗ disabled", Style::default().fg(colors::UI_ERROR))
                }
            };

            let mut spans = vec![];

            // Add standard Cyan indicator if selected
            if is_selected {
                spans.push(Span::styled("▎", Style::default().fg(colors::UI_HIGHLIGHT)));
            } else {
                spans.push(Span::raw(" "));
            }

            spans.push(Span::styled(padded_name, style));
            spans.push(Span::raw("     ")); // Separation
            spans.push(value_span);

            let line = Line::from(spans);

            let mut item_style = Style::default();
            if is_selected {
                item_style = item_style.bg(colors::UI_SELECTED_BG);
            }

            ListItem::new(line).style(item_style)
        })
        .collect();

    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .title(" Settings "),
        )
        .scroll_padding(1);

    // Sync state
    screen_state.state.select(Some(screen_state.selected));
    frame.render_stateful_widget(list, area, &mut screen_state.state);

    // Compute visible viewport (inner area) for arrow indicators
    let inner = area.inner(ratatui::layout::Margin {
        vertical: 1,
        horizontal: 0,
    });
    let view_height = inner.height as usize;

    let offset = screen_state.state.offset();
    let total = SettingItem::all().len();
    let has_above = offset > 0;
    let has_below = offset + view_height < total;

    // Render scroll arrows using helper
    crate::tui::widgets::render_scroll_arrows(frame, inner, has_above, has_below);

    // Render log level dropdown if editing
    if screen_state.editing_log_level && screen_state.current_item() == SettingItem::LogLevel {
        render_log_level_dropdown(frame, area, screen_state);
    }
}

/// Get the display value for a setting
fn get_setting_value(item: SettingItem, settings: &Settings) -> String {
    match item {
        SettingItem::DefaultOnStartup => format_bool(settings.default_on_startup),
        SettingItem::SetSmartToggle => format_bool(settings.set_smart_toggle),
        SettingItem::NotifyManual => format_bool(settings.notify_manual),
        SettingItem::NotifyRules => format_bool(settings.notify_rules),
        SettingItem::MatchByIndex => format_bool(settings.match_by_index),
        SettingItem::LogLevel => settings.log_level.clone(),
    }
}

/// Format boolean as colored text
fn format_bool(value: bool) -> String {
    if value {
        "✓ enabled".to_string()
    } else {
        "✗ disabled".to_string()
    }
}

/// Render the log level dropdown
fn render_log_level_dropdown(frame: &mut Frame, area: Rect, screen_state: &SettingsScreen) {
    let log_levels = ["error", "warn", "info", "debug", "trace"];

    // Create dropdown in center of screen
    let popup_area = centered_modal(modal_size::DROPDOWN, area);

    let items: Vec<ListItem> = log_levels
        .iter()
        .enumerate()
        .map(|(i, level)| {
            let is_selected = i == screen_state.log_level_index;
            let style = if is_selected {
                Style::default()
                    .fg(colors::UI_SELECTED)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(colors::UI_TEXT)
            };

            let mut spans = vec![];

            // Add arrow prefix only if selected
            if is_selected {
                spans.push(Span::styled(
                    " → ",
                    Style::default().fg(colors::UI_HIGHLIGHT),
                ));
            } else {
                spans.push(Span::raw("   "));
            }

            spans.push(Span::styled(*level, style));

            let line = Line::from(spans);

            ListItem::new(line)
        })
        .collect();

    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .padding(Padding::horizontal(1))
                .title("Select Log Level")
                .style(Style::default().bg(colors::UI_MODAL_BG)),
        )
        .scroll_padding(1);

    frame.render_widget(list, popup_area);
}

/// Render the description panel
fn render_description(frame: &mut Frame, area: Rect, screen_state: &mut SettingsScreen) {
    let current_item = screen_state.current_item();
    let short_desc = current_item.description();
    let detailed_desc = current_item.detailed_description();
    let needs_restart = current_item.requires_restart();

    let mut lines = vec![];

    // Short description at top (highlighted)
    lines.push(Line::from(Span::styled(
        short_desc,
        Style::default()
            .fg(colors::UI_HIGHLIGHT)
            .add_modifier(Modifier::BOLD),
    )));

    // Restart warning badge if needed (with extra space after emoji)
    if needs_restart {
        lines.push(Line::from(Span::styled(
            "⚠  Requires daemon restart",
            Style::default()
                .fg(colors::UI_WARNING)
                .add_modifier(Modifier::BOLD),
        )));
    }

    lines.push(Line::from("")); // Spacing

    // Parse the detailed description with simpler, consistent coloring
    for line in detailed_desc.lines() {
        if line.is_empty() {
            lines.push(Line::from(""));
        } else if line.starts_with("⚠") {
            // Skip hardcoded warnings - we show the badge dynamically now
            // (no action needed - line not added to output)
        } else if line.starts_with("When enabled:") {
            // "When enabled:" prefix - green, rest white
            let rest = line.trim_start_matches("When enabled:");
            lines.push(Line::from(vec![
                Span::styled("When enabled:", Style::default().fg(colors::UI_SUCCESS)),
                Span::styled(rest, Style::default().fg(colors::UI_TEXT)),
            ]));
        } else if line.starts_with("When disabled:") {
            // "When disabled:" prefix - red, rest white
            let rest = line.trim_start_matches("When disabled:");
            lines.push(Line::from(vec![
                Span::styled("When disabled:", Style::default().fg(colors::UI_ERROR)),
                Span::styled(rest, Style::default().fg(colors::UI_TEXT)),
            ]));
        } else if line.ends_with(':') && !line.contains("pw") && !line.starts_with("• ") {
            // Section headers (lines ending with colon) - bold white
            lines.push(Line::from(Span::styled(
                line,
                Style::default()
                    .fg(colors::UI_TEXT)
                    .add_modifier(Modifier::BOLD),
            )));
        } else if line.starts_with("• ") {
            // Bullet points - cyan bullet with white text
            let content = line.trim_start_matches("• ");
            lines.push(Line::from(vec![
                Span::styled("• ", Style::default().fg(colors::UI_HIGHLIGHT)),
                Span::styled(content, Style::default().fg(colors::UI_TEXT)),
            ]));
        } else if line.starts_with("Default:") {
            // Default value - dimmed
            lines.push(Line::from(Span::styled(
                line,
                Style::default()
                    .fg(colors::UI_SECONDARY)
                    .add_modifier(Modifier::ITALIC),
            )));
        } else {
            // All other lines - normal white text for readability
            lines.push(Line::from(Span::styled(
                line,
                Style::default().fg(colors::UI_TEXT),
            )));
        }
    }

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .padding(Padding::horizontal(1))
        .title(" Description ");

    let inner_area = block.inner(area);
    let width = inner_area.width as usize;
    let height = inner_area.height as usize;

    // Calculate total visual lines (accounting for wrapping)
    let mut total_visual_lines = 0;
    for line in &lines {
        let line_width = line.width();
        if line_width == 0 {
            total_visual_lines += 1;
        } else {
            // Ceiling division
            total_visual_lines += line_width.div_ceil(width);
        }
    }

    // Determine max scroll
    let max_scroll = u16::try_from(total_visual_lines.saturating_sub(height)).unwrap_or(u16::MAX);

    // Clamp scroll state
    screen_state.desc_scroll = screen_state.desc_scroll.min(max_scroll);

    let paragraph = Paragraph::new(lines)
        .block(block)
        .wrap(ratatui::widgets::Wrap { trim: false })
        .scroll((screen_state.desc_scroll, 0));

    frame.render_widget(paragraph, area);

    // Render scroll arrows
    let has_above = screen_state.desc_scroll > 0;
    let has_below = screen_state.desc_scroll < max_scroll;
    crate::tui::widgets::render_scroll_arrows(frame, inner_area, has_above, has_below);
}
