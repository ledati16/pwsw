//! Settings screen - Configure PWSW behavior

use std::borrow::Cow;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{HighlightSpacing, List, ListItem, ListState, Padding, Paragraph};
use tui_input::Input;

use crate::config::{MAX_SWITCH_DELAY_MS, Settings};
use crate::tui::theme;
use crate::tui::widgets::{
    FieldValue, FormField, border_hints, dialog_block, form_width, modal_rect, open_dialog,
    pick_list_rect, render_form, render_pick_list, render_scrollbar, rounded_block, text_width,
    wrap_line,
};

/// Log levels offered by the dropdown, least verbose first
pub(crate) const LOG_LEVELS: [&str; 5] = ["error", "warn", "info", "debug", "trace"];

/// Selected setting item
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingItem {
    DefaultOnStartup,
    SetSmartToggle,
    NotifyManual,
    NotifyRules,
    MatchByIndex,
    SwitchDelay,
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
            Self::SwitchDelay,
            Self::LogLevel,
        ]
    }

    /// Position of this setting in [`Self::all`]
    #[cfg(test)]
    pub(crate) fn index(self) -> usize {
        Self::all()
            .iter()
            .position(|item| *item == self)
            .unwrap_or_default()
    }

    /// Get the display name for this setting
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::DefaultOnStartup => "Default on Startup",
            Self::SetSmartToggle => "Smart Toggle",
            Self::NotifyManual => "Manual Switch Notifications",
            Self::NotifyRules => "Rule-Based Notifications",
            Self::MatchByIndex => "Match by Rule Index",
            Self::SwitchDelay => "Switch Delay",
            Self::LogLevel => "Log Level",
        }
    }

    /// Get short description for this setting
    pub(crate) const fn description(self) -> &'static str {
        match self {
            Self::DefaultOnStartup => "Apply the rules' sink when the daemon starts",
            Self::SetSmartToggle => "Intelligent toggling for manual sink switches",
            Self::NotifyManual => "Show notifications for manual switches and daemon events",
            Self::NotifyRules => "Show notifications for rule-triggered switches",
            Self::MatchByIndex => "Rule priority strategy for window matching",
            Self::SwitchDelay => "Delay before rule-triggered switches",
            Self::LogLevel => "Logging verbosity level",
        }
    }

    /// Get detailed description with examples for this setting
    pub(crate) const fn detailed_description(self) -> &'static str {
        match self {
            Self::DefaultOnStartup => {
                "Switches to the sink the rules want when the daemon starts.\n\
                 \n\
                 When enabled: On startup, activates the sink of the highest-priority\n\
                 open window that matches a rule, or the default sink if none matches.\n\
                 When disabled: Leaves the current sink alone until the rules want a\n\
                 different one (e.g. a matching window opens).\n\
                 \n\
                 Either way the daemon first waits (up to 2 s) for the list of open\n\
                 windows, so startup switches at most once.\n\
                 \n\
                 Default: disabled"
            }
            Self::SetSmartToggle => {
                "Intelligent toggling behavior for manual sink switches via CLI.\n\
                 \n\
                 When enabled: Running 'pwsw set-sink <name>' toggles back to default\n\
                 if the sink is already active ('pwsw set-source' likewise for mics).\n\
                 When disabled: Asking for the sink that is already active does nothing.\n\
                 \n\
                 Example: If headphones are active:\n\
                 • Enabled: 'pwsw set-sink headphones' → switches to default sink\n\
                 • Disabled: 'pwsw set-sink headphones' → stays on headphones\n\
                 \n\
                 Default: enabled"
            }
            Self::NotifyManual => {
                "Desktop notifications for manual switches and daemon events.\n\
                 \n\
                 When enabled: Shows notifications for:\n\
                 • Manual switches: 'pwsw set-sink <name>', 'pwsw set-source <name>'\n\
                 • Cycling commands: 'pwsw prev-sink' and 'pwsw next-sink'\n\
                 • Daemon events: start/stop, config reload/rejection, pause/resume\n\
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
                 A rule's own Notify option overrides this setting for that rule.\n\
                 Useful for debugging rules or understanding why switches occur.\n\
                 \n\
                 Default: enabled"
            }
            Self::MatchByIndex => {
                "Rule priority strategy when multiple windows match different rules.\n\
                 \n\
                 When enabled: Uses rule priority - higher priority rules always win.\n\
                 Rules at the top of the list have higher priority than those below.\n\
                 When disabled: The most recently opened matching window decides the sink.\n\
                 \n\
                 Example: Firefox (rule 1) and Discord (rule 2) are both open:\n\
                 • Enabled: Firefox's sink stays active whichever window opened last\n\
                 • Disabled: Whichever window opened (or started matching) last wins\n\
                 \n\
                 Tip: Reorder rules in the Rules tab (arrow keys + Shift+Up/Down) to adjust priority.\n\
                 \n\
                 Default: disabled (most recent window)"
            }
            Self::SwitchDelay => {
                "Delay before a rule-triggered switch, in milliseconds.\n\
                 \n\
                 When 0: Switches as soon as the rules want a different sink.\n\
                 When set: Waits this long; switches only if that sink is still wanted.\n\
                 Stops launchers and splash screens from flipping your audio. Manual\n\
                 switches, retries and the startup switch are never delayed.\n\
                 \n\
                 Default: 0 (switch immediately)"
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
                 Log file: ~/.local/share/pwsw/daemon.log\n\
                 Or in TUI: the Dashboard log view ([w] toggles Logs/Windows)\n\
                 \n\
                 Default: info"
            }
        }
    }
}

/// Settings screen state
#[derive(Debug)]
pub struct SettingsScreen {
    /// Currently selected item
    pub selected: usize,
    /// Whether we're editing the log level (dropdown open)
    pub editing_log_level: bool,
    /// Selected log level index (0-4 for error/warn/info/debug/trace)
    pub log_level_index: usize,
    /// List scroll state
    pub state: ListState,
    /// Description panel scroll offset
    pub desc_scroll: u16,
    /// The switch delay being typed (`Some` while its editor is open)
    pub delay_input: Option<Input>,
}

impl SettingsScreen {
    /// Create a new settings screen
    pub(crate) fn new(settings: &Settings) -> Self {
        // Unknown levels fall back to info
        let log_level_index = LOG_LEVELS
            .iter()
            .position(|l| *l == settings.log_level)
            .unwrap_or(2);

        Self {
            selected: 0,
            editing_log_level: false,
            log_level_index,
            state: ListState::default(),
            desc_scroll: 0,
            delay_input: None,
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

    /// Toggle the currently selected boolean setting (or open the editor of
    /// the log level or the switch delay; returns `false` then)
    pub(crate) fn toggle_current(&mut self, settings: &mut Settings) -> bool {
        if self.editing_log_level || self.delay_input.is_some() {
            return false; // Don't toggle while a dialog is open
        }

        match SettingItem::all()[self.selected] {
            SettingItem::DefaultOnStartup => {
                settings.default_on_startup.toggle();
                true
            }
            SettingItem::SetSmartToggle => {
                settings.set_smart_toggle.toggle();
                true
            }
            SettingItem::NotifyManual => {
                settings.notify_manual.toggle();
                true
            }
            SettingItem::NotifyRules => {
                settings.notify_rules.toggle();
                true
            }
            SettingItem::MatchByIndex => {
                settings.match_by_index.toggle();
                true
            }
            SettingItem::SwitchDelay => {
                self.delay_input = Some(Input::new(settings.switch_delay_ms.to_string()));
                false
            }
            SettingItem::LogLevel => {
                // Open dropdown for editing
                self.editing_log_level = true;
                false
            }
        }
    }

    /// Whether a dialog (log level list or switch delay editor) is open
    pub(crate) const fn dialog_open(&self) -> bool {
        self.editing_log_level || self.delay_input.is_some()
    }

    /// Get the current selected item
    pub(crate) fn current_item(&self) -> SettingItem {
        SettingItem::all()[self.selected]
    }
}

/// Parse a switch delay typed in milliseconds, checked against the range
/// [`Settings::switch_delay_ms`] accepts (`0` to [`MAX_SWITCH_DELAY_MS`]).
///
/// # Errors
/// Returns the message to show when the text is empty, not a whole number or
/// out of range.
pub(crate) fn parse_switch_delay(text: &str) -> Result<u32, String> {
    let text = text.trim();
    let range = format!("between 0 and {MAX_SWITCH_DELAY_MS} ms");
    if text.is_empty() {
        return Err(format!("Enter a switch delay {range}"));
    }
    if !text.chars().all(|c| c.is_ascii_digit()) {
        return Err(format!(
            "Switch delay must be a whole number of milliseconds, {range}"
        ));
    }
    match text.parse::<u32>() {
        Ok(ms) if ms <= MAX_SWITCH_DELAY_MS => Ok(ms),
        _ => Err(format!("Switch delay must be {range}")),
    }
}

/// Render the settings screen
pub fn render_settings(
    frame: &mut Frame,
    area: Rect,
    settings: &Settings,
    screen_state: &mut SettingsScreen,
) {
    // The list takes what its items need, the description the rest
    let list_height = u16::try_from(SettingItem::all().len() + 2).unwrap_or(u16::MAX);
    let [list_area, desc_area] =
        Layout::vertical([Constraint::Length(list_height), Constraint::Fill(1)]).areas(area);

    render_settings_list(frame, list_area, settings, screen_state);
    render_description(frame, desc_area, screen_state);

    if screen_state.editing_log_level && screen_state.current_item() == SettingItem::LogLevel {
        render_log_level_dropdown(frame, area, screen_state);
    }
    if let Some(input) = &screen_state.delay_input {
        render_delay_editor(frame, area, input);
    }
}

/// Render the switch delay editor: one text field and the accepted range,
/// centred on `area`
fn render_delay_editor(frame: &mut Frame, area: Rect, input: &Input) {
    const TITLE: &str = "Switch Delay";
    const HINTS: &[(&str, &str)] = &[("Enter", "apply"), ("Esc", "cancel")];
    let note = format!("0 to {MAX_SWITCH_DELAY_MS}; 0 = switch immediately");
    let fields = [FormField {
        label: "Milliseconds",
        value: FieldValue::Text {
            input,
            placeholder: Cow::Borrowed("0"),
        },
        trailing: None,
        focused: true,
    }];
    // Borders and padding take 4 columns, the borders 2 rows
    let content = usize::from(form_width(&fields, 8))
        .max(text_width(&note))
        .max(border_hints(HINTS).width())
        .max(text_width(TITLE) + 2);
    let width = u16::try_from(content + 4).unwrap_or(u16::MAX);
    let rect = modal_rect(area, width, 4);
    let inner = open_dialog(
        frame,
        rect,
        dialog_block(TITLE).title_bottom(border_hints(HINTS)),
    );
    let [field_row, note_row] =
        Layout::vertical([Constraint::Length(1), Constraint::Length(1)]).areas(inner);
    render_form(frame, field_row, &fields);
    frame.render_widget(Span::styled(note, theme::dim()), note_row);
}

/// Whether a boolean setting is on (`None` for the log level and the delay)
const fn bool_value(item: SettingItem, settings: &Settings) -> Option<bool> {
    match item {
        SettingItem::DefaultOnStartup => Some(settings.default_on_startup),
        SettingItem::SetSmartToggle => Some(settings.set_smart_toggle),
        SettingItem::NotifyManual => Some(settings.notify_manual),
        SettingItem::NotifyRules => Some(settings.notify_rules),
        SettingItem::MatchByIndex => Some(settings.match_by_index),
        SettingItem::SwitchDelay | SettingItem::LogLevel => None,
    }
}

/// Render the settings list
fn render_settings_list(
    frame: &mut Frame,
    area: Rect,
    settings: &Settings,
    screen_state: &mut SettingsScreen,
) {
    let name_width = SettingItem::all()
        .iter()
        .map(|i| text_width(i.name()))
        .max()
        .unwrap_or(0);
    let items: Vec<ListItem> = SettingItem::all()
        .iter()
        .map(|item| {
            let pad = name_width - text_width(item.name()) + 3;
            let value = match (bool_value(*item, settings), item) {
                (Some(true), _) => Span::styled("[x] on", theme::success()),
                (Some(false), _) => Span::styled("[ ] off", theme::dim()),
                (None, SettingItem::SwitchDelay) if settings.switch_delay_ms == 0 => {
                    Span::styled("0 ms (immediate)", theme::dim())
                }
                (None, SettingItem::SwitchDelay) => {
                    Span::styled(format!("{} ms", settings.switch_delay_ms), theme::accent())
                }
                (None, _) => Span::styled(settings.log_level.clone(), theme::accent()),
            };
            ListItem::new(Line::from(vec![
                Span::raw(item.name()),
                Span::raw(" ".repeat(pad)),
                value,
            ]))
        })
        .collect();

    let list = List::new(items)
        .block(rounded_block().title(Span::styled(" Settings ", theme::bold())))
        .highlight_style(theme::selected())
        .highlight_symbol(theme::SELECTED_SYMBOL)
        .highlight_spacing(HighlightSpacing::Always)
        .scroll_padding(1);

    screen_state.state.select(Some(screen_state.selected));
    frame.render_stateful_widget(list, area, &mut screen_state.state);
    render_scrollbar(
        frame,
        area,
        0,
        SettingItem::all().len(),
        screen_state.state.offset(),
    );
}

/// Render the log level dropdown, sized to its levels, centred on `area`
fn render_log_level_dropdown(frame: &mut Frame, area: Rect, screen_state: &SettingsScreen) {
    const TITLE: &str = "Log Level";
    const HINTS: &[(&str, &str)] = &[("Enter", "apply"), ("Esc", "cancel")];
    let widest = LOG_LEVELS.iter().map(|l| text_width(l)).max().unwrap_or(0);
    let min_width = border_hints(HINTS).width().max(text_width(TITLE) + 4) + 4;
    let (rect, _) = pick_list_rect(
        area,
        widest,
        LOG_LEVELS.len(),
        u16::try_from(min_width).unwrap_or(u16::MAX),
    );
    let items: Vec<ListItem> = LOG_LEVELS.iter().map(|l| ListItem::new(*l)).collect();
    let mut state = ListState::default().with_selected(Some(screen_state.log_level_index));
    render_pick_list(frame, rect, TITLE, HINTS, items, &mut state);
}

/// Render the description panel
fn render_description(frame: &mut Frame, area: Rect, screen_state: &mut SettingsScreen) {
    let current_item = screen_state.current_item();
    let short_desc = current_item.description();
    let detailed_desc = current_item.detailed_description();

    let mut lines = vec![
        Line::from(Span::styled(short_desc, theme::bold())),
        Line::from(""),
    ];

    for line in detailed_desc.lines() {
        if let Some(rest) = line.strip_prefix("When enabled:") {
            lines.push(Line::from(vec![
                Span::styled("When enabled:", theme::success()),
                Span::raw(rest),
            ]));
        } else if let Some(rest) = line.strip_prefix("When disabled:") {
            lines.push(Line::from(vec![
                Span::styled("When disabled:", theme::dim().add_modifier(Modifier::BOLD)),
                Span::raw(rest),
            ]));
        } else if let Some(rest) = line.strip_prefix("• ") {
            lines.push(Line::from(vec![
                Span::styled("• ", theme::accent()),
                Span::raw(rest),
            ]));
        } else if line.starts_with("Default:") {
            lines.push(Line::from(Span::styled(
                line,
                theme::dim().add_modifier(Modifier::ITALIC),
            )));
        } else if line.ends_with(':') && !line.contains("pw") {
            lines.push(Line::from(Span::styled(line, theme::bold())));
        } else {
            lines.push(Line::from(line));
        }
    }

    let block = rounded_block()
        .padding(Padding::horizontal(1))
        .title(Span::styled(" About this setting ", theme::bold()));

    // Wrapped here rather than by the paragraph, so the scroll limit counts
    // exactly the rows drawn
    let inner_area = block.inner(area);
    let rows: Vec<Line> = lines
        .iter()
        .flat_map(|l| wrap_line(l, usize::from(inner_area.width)))
        .collect();
    let total_rows = rows.len();
    let max_scroll = u16::try_from(total_rows.saturating_sub(usize::from(inner_area.height)))
        .unwrap_or(u16::MAX);
    screen_state.desc_scroll = screen_state.desc_scroll.min(max_scroll);

    let paragraph = Paragraph::new(rows)
        .block(block)
        .scroll((screen_state.desc_scroll, 0));
    frame.render_widget(paragraph, area);
    render_scrollbar(
        frame,
        area,
        0,
        total_rows,
        usize::from(screen_state.desc_scroll),
    );
}
