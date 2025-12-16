//! Settings screen - Configure PWSW behavior

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph},
    Frame,
};

use crate::config::Settings;
use crate::tui::widgets::{centered_modal, modal_size};

/// Selected setting item
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SettingItem {
    DefaultOnStartup,
    SetSmartToggle,
    NotifyManual,
    NotifyRules,
    MatchByIndex,
    LogLevel,
}

impl SettingItem {
    /// Get all settings in display order
    pub const fn all() -> &'static [SettingItem] {
        &[
            SettingItem::DefaultOnStartup,
            SettingItem::SetSmartToggle,
            SettingItem::NotifyManual,
            SettingItem::NotifyRules,
            SettingItem::MatchByIndex,
            SettingItem::LogLevel,
        ]
    }

    /// Get the display name for this setting
    pub const fn name(self) -> &'static str {
        match self {
            SettingItem::DefaultOnStartup => "Default on Startup",
            SettingItem::SetSmartToggle => "Smart Toggle",
            SettingItem::NotifyManual => "Manual Switch Notifications",
            SettingItem::NotifyRules => "Rule-Based Notifications",
            SettingItem::MatchByIndex => "Match by Rule Index",
            SettingItem::LogLevel => "Log Level",
        }
    }

    /// Get the description for this setting
    pub const fn description(self) -> &'static str {
        match self {
            SettingItem::DefaultOnStartup => "Switch to default sink when daemon starts",
            SettingItem::SetSmartToggle => "set-sink toggles back to default if already active",
            SettingItem::NotifyManual => "Show notifications for manual sink switches",
            SettingItem::NotifyRules => "Show notifications for rule-triggered switches",
            SettingItem::MatchByIndex => {
                "Prioritize by rule position (true) or most recent window (false)"
            }
            SettingItem::LogLevel => "Logging verbosity: error, warn, info, debug, trace",
        }
    }
}

/// Settings screen state
pub(crate) struct SettingsScreen {
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
        }
    }

    /// Move selection up
    pub(crate) fn select_previous(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
        }
    }

    /// Move selection down
    pub(crate) fn select_next(&mut self) {
        if self.selected < SettingItem::all().len() - 1 {
            self.selected += 1;
        }
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
pub(crate) fn render_settings(
    frame: &mut Frame,
    area: Rect,
    settings: &Settings,
    screen_state: &mut SettingsScreen,
) {
    // Split into [settings list | description]
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(10),   // Settings list
            Constraint::Length(4), // Description
        ])
        .split(area);

    // Render settings list
    render_settings_list(frame, chunks[0], settings, screen_state);

    // Render description
    render_description(frame, chunks[1], screen_state);
}

/// Render the settings list
#[allow(clippy::too_many_lines)]
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
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            };

            let padded_name = screen_state
                .padded_names
                .get(i)
                .map_or(item.name(), String::as_str);

            // Apply color styling to boolean toggles
            let value_span = match item {
                SettingItem::LogLevel => Span::styled(value_text, style),
                SettingItem::DefaultOnStartup => {
                    if settings.default_on_startup {
                        Span::styled("✓ enabled", Style::default().fg(Color::Green))
                    } else {
                        Span::styled("✗ disabled", Style::default().fg(Color::Red))
                    }
                }
                SettingItem::SetSmartToggle => {
                    if settings.set_smart_toggle {
                        Span::styled("✓ enabled", Style::default().fg(Color::Green))
                    } else {
                        Span::styled("✗ disabled", Style::default().fg(Color::Red))
                    }
                }
                SettingItem::NotifyManual => {
                    if settings.notify_manual {
                        Span::styled("✓ enabled", Style::default().fg(Color::Green))
                    } else {
                        Span::styled("✗ disabled", Style::default().fg(Color::Red))
                    }
                }
                SettingItem::NotifyRules => {
                    if settings.notify_rules {
                        Span::styled("✓ enabled", Style::default().fg(Color::Green))
                    } else {
                        Span::styled("✗ disabled", Style::default().fg(Color::Red))
                    }
                }
                SettingItem::MatchByIndex => {
                    if settings.match_by_index {
                        Span::styled("✓ enabled", Style::default().fg(Color::Green))
                    } else {
                        Span::styled("✗ disabled", Style::default().fg(Color::Red))
                    }
                }
            };

            let line = Line::from(vec![
                Span::styled(
                    if is_selected { "> " } else { "  " },
                    Style::default().fg(Color::Cyan),
                ),
                Span::styled(padded_name, style),
                Span::raw("  "),
                value_span,
            ]);

            ListItem::new(line)
        })
        .collect();

    let list = List::new(items).block(
        Block::default()
            .borders(Borders::ALL)
            .title("Settings ([↑/↓]select [Space]/[Enter]toggle)"),
    );

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
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            };

            let line = Line::from(vec![
                Span::styled(
                    if is_selected { "> " } else { "  " },
                    Style::default().fg(Color::Cyan),
                ),
                Span::styled(*level, style),
            ]);

            ListItem::new(line)
        })
        .collect();

    let list = List::new(items).block(
        Block::default()
            .borders(Borders::ALL)
            .title("Select Log Level (↑/↓, Enter to confirm, Esc to cancel)")
            .style(Style::default().bg(Color::Black)),
    );

    frame.render_widget(list, popup_area);
}

/// Render the description panel
fn render_description(frame: &mut Frame, area: Rect, screen_state: &SettingsScreen) {
    let current_item = screen_state.current_item();
    let description = current_item.description();

    let text = vec![
        Line::from(""),
        Line::from(vec![Span::styled(
            description,
            Style::default().fg(Color::Gray),
        )]),
    ];

    let paragraph =
        Paragraph::new(text).block(Block::default().borders(Borders::ALL).title("Description"));

    frame.render_widget(paragraph, area);
}
