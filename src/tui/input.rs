//! Input handling for keyboard and mouse events

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};
use std::time::Duration;

use super::app::{App, DaemonAction, Screen};
use super::screens::rules::RulesMode;
use super::screens::sinks::SinksMode;
use crate::config::{Rule, SinkConfig};
use regex::Regex;

/// Poll timeout for event checking (non-blocking)
const POLL_TIMEOUT: Duration = Duration::from_millis(100);

/// Handle keyboard and mouse input events
///
/// # Errors
/// Returns an error if event polling fails.
pub fn handle_events(app: &mut App) -> Result<()> {
    // Non-blocking event poll
    if event::poll(POLL_TIMEOUT)? {
        match event::read()? {
            Event::Key(key_event) => handle_key_event(app, key_event),
            Event::Mouse(mouse_event) => handle_mouse_event(app, mouse_event),
            Event::Resize(_, _) => {
                // Ratatui handles resize automatically, nothing to do
            }
            _ => {}
        }
    }
    Ok(())
}

/// Check if any modal or editor is currently active
fn is_modal_active(app: &App) -> bool {
    // Check if help overlay is shown
    if app.show_help {
        return true;
    }

    // Check screen-specific modals
    match app.current_screen {
        Screen::Sinks => app.sinks_screen.mode != SinksMode::List,
        Screen::Rules => app.rules_screen.mode != RulesMode::List,
        Screen::Settings => app.settings_screen.editing_log_level,
        Screen::Dashboard => false,
    }
}

/// Handle keyboard input
fn handle_key_event(app: &mut App, key: KeyEvent) {
    // Always-global keybindings (work even in modals)
    match (key.code, key.modifiers) {
        // Ctrl+C always quits immediately
        (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
            app.quit();
            return;
        }

        // Ctrl+S: Save config (global)
        (KeyCode::Char('s'), KeyModifiers::CONTROL) => {
            if app.config_dirty {
                if let Err(e) = app.save_config() {
                    app.set_status(format!("Failed to save config: {e}"));
                }
            }
            return;
        }

        _ => {}
    }

    // If a modal is active, pass most keys to screen-specific handlers
    // (except always-global shortcuts handled above)
    if is_modal_active(app) {
        // Handle escape and help toggle at global level
        match key.code {
            KeyCode::Esc => {
                // Priority: quit confirmation > help overlay > modal/status
                if app.confirm_quit {
                    app.cancel_quit();
                } else if app.show_help {
                    app.show_help = false;
                } else {
                    // Let screen handler close modal
                    handle_screen_specific_input(app, key);
                }
                return;
            }
            KeyCode::Char('?') => {
                app.show_help = !app.show_help;
                return;
            }
            _ => {
                // All other keys go to screen-specific handlers when modal is active
                handle_screen_specific_input(app, key);
                return;
            }
        }
    }

    // Normal global keybindings (only when no modal is active)
    match (key.code, key.modifiers) {
        // Quit: q
        (KeyCode::Char('q'), KeyModifiers::NONE) => {
            if app.confirm_quit {
                // User already confirmed, actually quit
                app.confirm_quit_action();
            } else {
                // First quit request - check for unsaved changes
                app.request_quit();
            }
        }

        // Tab: next screen
        (KeyCode::Tab, KeyModifiers::NONE) => {
            app.next_screen();
        }

        // Shift+Tab: previous screen
        (KeyCode::BackTab, KeyModifiers::SHIFT) => {
            app.prev_screen();
        }

        // Direct screen navigation shortcuts
        (KeyCode::Char('d'), KeyModifiers::NONE) => {
            app.goto_screen(Screen::Dashboard);
        }
        (KeyCode::Char('s'), KeyModifiers::NONE) => {
            app.goto_screen(Screen::Sinks);
        }
        (KeyCode::Char('r'), KeyModifiers::NONE) => {
            app.goto_screen(Screen::Rules);
        }
        (KeyCode::Char('t'), KeyModifiers::NONE) => {
            app.goto_screen(Screen::Settings);
        }

        // Escape: cancel quit confirmation, close help, or clear status message
        (KeyCode::Esc, KeyModifiers::NONE) => {
            if app.confirm_quit {
                app.cancel_quit();
            } else if app.show_help {
                app.show_help = false;
            } else {
                app.clear_status();
            }
        }

        // ?: Toggle help overlay
        (KeyCode::Char('?'), KeyModifiers::NONE) | (KeyCode::Char('?'), KeyModifiers::SHIFT) => {
            app.show_help = !app.show_help;
        }

        // Screen-specific input
        _ => {
            handle_screen_specific_input(app, key);
        }
    }
}

/// Handle screen-specific keyboard input
fn handle_screen_specific_input(app: &mut App, key: KeyEvent) {
    match app.current_screen {
        Screen::Dashboard => handle_dashboard_input(app, key),
        Screen::Settings => handle_settings_input(app, key),
        Screen::Sinks => handle_sinks_input(app, key),
        Screen::Rules => handle_rules_input(app, key),
    }
}

/// Handle dashboard screen input
fn handle_dashboard_input(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Up => {
            app.dashboard_screen.select_previous();
        }
        KeyCode::Down => {
            app.dashboard_screen.select_next();
        }
        KeyCode::Enter => {
            // Send the daemon action to the background worker if available
            let action = match app.dashboard_screen.selected_action {
                0 => DaemonAction::Start,
                1 => DaemonAction::Stop,
                2 => DaemonAction::Restart,
                _ => return,
            };
            if let Some(tx) = &app.bg_cmd_tx {
                let _ = tx.try_send(crate::tui::app::BgCommand::DaemonAction(action));
                app.set_status("Daemon action requested".to_string());
            } else {
                // Fallback: queue as pending (old behaviour)
                app.pending_daemon_action = Some(action);
            }
        }
        _ => {}
    }
}

/// Handle settings screen input
fn handle_settings_input(app: &mut App, key: KeyEvent) {
    // If editing log level dropdown
    if app.settings_screen.editing_log_level {
        match key.code {
            KeyCode::Up => {
                if app.settings_screen.log_level_index > 0 {
                    app.settings_screen.log_level_index -= 1;
                }
            }
            KeyCode::Down => {
                if app.settings_screen.log_level_index < 4 {
                    app.settings_screen.log_level_index += 1;
                }
            }
            KeyCode::Enter => {
                // Apply the selected log level
                let log_levels = ["error", "warn", "info", "debug", "trace"];
                app.config.settings.log_level =
                    log_levels[app.settings_screen.log_level_index].to_string();
                app.settings_screen.editing_log_level = false;
                app.mark_dirty();
            }
            KeyCode::Esc => {
                // Cancel editing
                app.settings_screen.editing_log_level = false;
            }
            _ => {}
        }
        return;
    }

    // Normal settings navigation
    match key.code {
        KeyCode::Up => {
            app.settings_screen.select_previous();
        }
        KeyCode::Down => {
            app.settings_screen.select_next();
        }
        KeyCode::Enter | KeyCode::Char(' ') => {
            if app.settings_screen.toggle_current(&mut app.config.settings) {
                app.mark_dirty();
            }
        }
        _ => {}
    }
}

/// Handle sinks screen input
fn handle_sinks_input(app: &mut App, key: KeyEvent) {
    match app.sinks_screen.mode {
        SinksMode::List => {
            match key.code {
                KeyCode::Up => {
                    app.sinks_screen.select_previous(app.config.sinks.len());
                }
                KeyCode::Down => {
                    app.sinks_screen.select_next(app.config.sinks.len());
                }
                KeyCode::Char('a') => {
                    app.sinks_screen.start_add();
                }
                KeyCode::Char('e') => {
                    app.sinks_screen.start_edit(&app.config.sinks);
                }
                KeyCode::Char('x') => {
                    // 'x' for delete (avoids conflict with Dashboard shortcut 'd')
                    if !app.config.sinks.is_empty() {
                        app.sinks_screen.start_delete();
                    }
                }
                KeyCode::Char(' ') => {
                    // Toggle default status
                    let idx = app.sinks_screen.selected;
                    if idx < app.config.sinks.len() {
                        // Clear all defaults first
                        for sink in &mut app.config.sinks {
                            sink.default = false;
                        }
                        // Set selected as default
                        app.config.sinks[idx].default = true;
                        app.mark_dirty();
                    }
                }
                _ => {}
            }
        }
        SinksMode::AddEdit => {
            handle_sink_editor_input(app, key);
        }
        SinksMode::Delete => {
            match key.code {
                KeyCode::Enter => {
                    // Confirm deletion
                    let idx = app.sinks_screen.selected;
                    if idx < app.config.sinks.len() {
                        let was_default = app.config.sinks[idx].default;
                        app.config.sinks.remove(idx);

                        // If we deleted the default and there are sinks left, make first one default
                        if was_default && !app.config.sinks.is_empty() {
                            app.config.sinks[0].default = true;
                        }

                        // Adjust selection
                        if app.sinks_screen.selected >= app.config.sinks.len()
                            && !app.config.sinks.is_empty()
                        {
                            app.sinks_screen.selected = app.config.sinks.len() - 1;
                        }

                        app.mark_dirty();
                        app.set_status("Sink deleted".to_string());
                    }
                    app.sinks_screen.cancel();
                }
                KeyCode::Esc => {
                    app.sinks_screen.cancel();
                }
                _ => {}
            }
        }
    }
}

/// Handle sink editor input (add/edit modal)
fn handle_sink_editor_input(app: &mut App, key: KeyEvent) {
    use crate::tui::editor_helpers::{insert_char_at, remove_char_at, remove_char_before};

    match key.code {
        KeyCode::Tab => {
            app.sinks_screen.editor.next_field();
        }
        KeyCode::BackTab => {
            app.sinks_screen.editor.prev_field();
        }
        KeyCode::Char(c) => {
            // Type into focused field (insert at cursor)
            match app.sinks_screen.editor.focused_field {
                0 => {
                    let cur = app.sinks_screen.editor.cursor_name;
                    let new_cur = insert_char_at(&mut app.sinks_screen.editor.name, c, cur);
                    app.sinks_screen.editor.cursor_name = new_cur;
                }
                1 => {
                    let cur = app.sinks_screen.editor.cursor_desc;
                    let new_cur = insert_char_at(&mut app.sinks_screen.editor.desc, c, cur);
                    app.sinks_screen.editor.cursor_desc = new_cur;
                }
                2 => {
                    let cur = app.sinks_screen.editor.cursor_icon;
                    let new_cur = insert_char_at(&mut app.sinks_screen.editor.icon, c, cur);
                    app.sinks_screen.editor.cursor_icon = new_cur;
                }
                3 => {
                    // Toggle default checkbox with space
                    if c == ' ' {
                        app.sinks_screen.editor.default = !app.sinks_screen.editor.default;
                    }
                }
                _ => {}
            }
        }
        KeyCode::Left => match app.sinks_screen.editor.focused_field {
            0 => {
                app.sinks_screen.editor.cursor_name =
                    app.sinks_screen.editor.cursor_name.saturating_sub(1)
            }
            1 => {
                app.sinks_screen.editor.cursor_desc =
                    app.sinks_screen.editor.cursor_desc.saturating_sub(1)
            }
            2 => {
                app.sinks_screen.editor.cursor_icon =
                    app.sinks_screen.editor.cursor_icon.saturating_sub(1)
            }
            _ => {}
        },
        KeyCode::Right => match app.sinks_screen.editor.focused_field {
            0 => {
                let len = app.sinks_screen.editor.name.chars().count();
                app.sinks_screen.editor.cursor_name =
                    usize::min(len, app.sinks_screen.editor.cursor_name + 1);
            }
            1 => {
                let len = app.sinks_screen.editor.desc.chars().count();
                app.sinks_screen.editor.cursor_desc =
                    usize::min(len, app.sinks_screen.editor.cursor_desc + 1);
            }
            2 => {
                let len = app.sinks_screen.editor.icon.chars().count();
                app.sinks_screen.editor.cursor_icon =
                    usize::min(len, app.sinks_screen.editor.cursor_icon + 1);
            }
            _ => {}
        },
        KeyCode::Home => match app.sinks_screen.editor.focused_field {
            0 => app.sinks_screen.editor.cursor_name = 0,
            1 => app.sinks_screen.editor.cursor_desc = 0,
            2 => app.sinks_screen.editor.cursor_icon = 0,
            _ => {}
        },
        KeyCode::End => match app.sinks_screen.editor.focused_field {
            0 => app.sinks_screen.editor.cursor_name = app.sinks_screen.editor.name.chars().count(),
            1 => app.sinks_screen.editor.cursor_desc = app.sinks_screen.editor.desc.chars().count(),
            2 => app.sinks_screen.editor.cursor_icon = app.sinks_screen.editor.icon.chars().count(),
            _ => {}
        },
        KeyCode::Backspace => match app.sinks_screen.editor.focused_field {
            0 => {
                let cur = app.sinks_screen.editor.cursor_name;
                let new_cur = remove_char_before(&mut app.sinks_screen.editor.name, cur);
                app.sinks_screen.editor.cursor_name = new_cur;
            }
            1 => {
                let cur = app.sinks_screen.editor.cursor_desc;
                let new_cur = remove_char_before(&mut app.sinks_screen.editor.desc, cur);
                app.sinks_screen.editor.cursor_desc = new_cur;
            }
            2 => {
                let cur = app.sinks_screen.editor.cursor_icon;
                let new_cur = remove_char_before(&mut app.sinks_screen.editor.icon, cur);
                app.sinks_screen.editor.cursor_icon = new_cur;
            }
            _ => {}
        },
        KeyCode::Delete => match app.sinks_screen.editor.focused_field {
            0 => {
                let cur = app.sinks_screen.editor.cursor_name;
                let new_cur = remove_char_at(&mut app.sinks_screen.editor.name, cur);
                app.sinks_screen.editor.cursor_name = new_cur;
            }
            1 => {
                let cur = app.sinks_screen.editor.cursor_desc;
                let new_cur = remove_char_at(&mut app.sinks_screen.editor.desc, cur);
                app.sinks_screen.editor.cursor_desc = new_cur;
            }
            2 => {
                let cur = app.sinks_screen.editor.cursor_icon;
                let new_cur = remove_char_at(&mut app.sinks_screen.editor.icon, cur);
                app.sinks_screen.editor.cursor_icon = new_cur;
            }
            _ => {}
        },
        KeyCode::Enter => {
            // Save the sink
            if app.sinks_screen.editor.name.is_empty() || app.sinks_screen.editor.desc.is_empty() {
                app.set_status("Name and Description are required".to_string());
                return;
            }

            let new_sink = SinkConfig {
                name: app.sinks_screen.editor.name.clone(),
                desc: app.sinks_screen.editor.desc.clone(),
                icon: if app.sinks_screen.editor.icon.is_empty() {
                    None
                } else {
                    Some(app.sinks_screen.editor.icon.clone())
                },
                default: app.sinks_screen.editor.default,
            };

            if let Some(idx) = app.sinks_screen.editing_index {
                // Editing existing
                app.config.sinks[idx] = new_sink;
                app.set_status("Sink updated".to_string());
            } else {
                // Adding new
                // If this is marked as default, clear other defaults
                if new_sink.default {
                    for sink in &mut app.config.sinks {
                        sink.default = false;
                    }
                }
                app.config.sinks.push(new_sink);
                app.set_status("Sink added".to_string());
            }

            app.mark_dirty();
            app.sinks_screen.cancel();
        }
        KeyCode::Esc => {
            app.sinks_screen.cancel();
        }
        _ => {}
    }
}

/// Handle rules screen input
fn handle_rules_input(app: &mut App, key: KeyEvent) {
    match app.rules_screen.mode {
        RulesMode::List => {
            match key.code {
                KeyCode::Up => {
                    app.rules_screen.select_previous(app.config.rules.len());
                }
                KeyCode::Down => {
                    app.rules_screen.select_next(app.config.rules.len());
                }
                KeyCode::Char('a') => {
                    app.rules_screen.start_add();
                }
                KeyCode::Char('e') => {
                    app.rules_screen.start_edit(&app.config.rules);
                }
                KeyCode::Char('x') => {
                    // 'x' for delete (avoids conflict with Dashboard shortcut 'd')
                    if !app.config.rules.is_empty() {
                        app.rules_screen.start_delete();
                    }
                }
                _ => {}
            }
        }
        RulesMode::AddEdit => {
            handle_rule_editor_input(app, key);
        }
        RulesMode::Delete => match key.code {
            KeyCode::Enter => {
                let idx = app.rules_screen.selected;
                if idx < app.config.rules.len() {
                    app.config.rules.remove(idx);

                    if app.rules_screen.selected >= app.config.rules.len()
                        && !app.config.rules.is_empty()
                    {
                        app.rules_screen.selected = app.config.rules.len() - 1;
                    }

                    app.mark_dirty();
                    app.set_status("Rule deleted".to_string());
                }
                app.rules_screen.cancel();
            }
            KeyCode::Esc => {
                app.rules_screen.cancel();
            }
            _ => {}
        },
        RulesMode::SelectSink => match key.code {
            KeyCode::Up => {
                if app.rules_screen.editor.sink_dropdown_index > 0 {
                    app.rules_screen.editor.sink_dropdown_index -= 1;
                }
            }
            KeyCode::Down => {
                if app.rules_screen.editor.sink_dropdown_index < app.config.sinks.len() - 1 {
                    app.rules_screen.editor.sink_dropdown_index += 1;
                }
            }
            KeyCode::Enter => {
                let idx = app.rules_screen.editor.sink_dropdown_index;
                if idx < app.config.sinks.len() {
                    app.rules_screen.editor.sink_ref = app.config.sinks[idx].desc.clone();
                }
                app.rules_screen.mode = RulesMode::AddEdit;
            }
            KeyCode::Esc => {
                app.rules_screen.mode = RulesMode::AddEdit;
            }
            _ => {}
        },
    }
}

/// Handle rule editor input (add/edit modal)
fn handle_rule_editor_input(app: &mut App, key: KeyEvent) {
    // Helper functions to operate on strings by character index
    fn insert_char_at(s: &mut String, ch: char, idx: usize) -> usize {
        let mut new = String::with_capacity(s.len() + ch.len_utf8());
        for (i, c) in s.chars().enumerate() {
            if i == idx {
                new.push(ch);
            }
            new.push(c);
        }
        if idx >= s.chars().count() {
            new.push(ch);
        }
        *s = new;
        // return new cursor position (after inserted char)
        usize::min(idx + 1, s.chars().count())
    }

    fn remove_char_before(s: &mut String, idx: usize) -> usize {
        if idx == 0 || s.is_empty() {
            return 0;
        }
        let mut new = String::with_capacity(s.len());
        let mut removed = false;
        for (i, c) in s.chars().enumerate() {
            if i == idx - 1 && !removed {
                removed = true;
                continue;
            }
            new.push(c);
        }
        *s = new;
        idx.saturating_sub(1)
    }

    fn remove_char_at(s: &mut String, idx: usize) -> usize {
        if s.is_empty() || idx >= s.chars().count() {
            return s.chars().count();
        }
        let mut new = String::with_capacity(s.len());
        for (i, c) in s.chars().enumerate() {
            if i == idx {
                continue;
            }
            new.push(c);
        }
        *s = new;
        idx
    }

    match key.code {
        KeyCode::Tab => app.rules_screen.editor.next_field(),
        KeyCode::BackTab => app.rules_screen.editor.prev_field(),
        KeyCode::Char(c) => {
            match app.rules_screen.editor.focused_field {
                0 => {
                    // insert at cursor_app
                    let cur = app.rules_screen.editor.cursor_app;
                    let new_cur =
                        insert_char_at(&mut app.rules_screen.editor.app_id_pattern, c, cur);
                    app.rules_screen.editor.cursor_app = new_cur;

                    // invalidate/compile cache
                    let pat = app.rules_screen.editor.app_id_pattern.clone();
                    if app.rules_screen.editor.compiled_app_id_for.as_ref() != Some(&pat) {
                        app.rules_screen.editor.compiled_app_id_for = Some(pat.clone());
                        app.rules_screen.editor.compiled_app_id = Regex::new(&pat).ok();
                    }

                    // Request live-preview
                    if let Some(tx) = &app.bg_cmd_tx {
                        let compiled_app = app
                            .rules_screen
                            .editor
                            .compiled_app_id
                            .as_ref()
                            .map(|r| std::sync::Arc::new(r.clone()));
                        let compiled_title = if app.rules_screen.editor.title_pattern.is_empty() {
                            None
                        } else {
                            app.rules_screen
                                .editor
                                .compiled_title
                                .as_ref()
                                .map(|r| std::sync::Arc::new(r.clone()))
                        };
                        let _ = tx.try_send(crate::tui::app::BgCommand::PreviewRequest {
                            app_pattern: pat.clone(),
                            title_pattern: if app.rules_screen.editor.title_pattern.is_empty() {
                                None
                            } else {
                                Some(app.rules_screen.editor.title_pattern.clone())
                            },
                            compiled_app,
                            compiled_title,
                        });
                    }
                }
                1 => {
                    let cur = app.rules_screen.editor.cursor_title;
                    let new_cur =
                        insert_char_at(&mut app.rules_screen.editor.title_pattern, c, cur);
                    app.rules_screen.editor.cursor_title = new_cur;

                    let pat = app.rules_screen.editor.title_pattern.clone();
                    if app.rules_screen.editor.compiled_title_for.as_ref() != Some(&pat) {
                        app.rules_screen.editor.compiled_title_for = Some(pat.clone());
                        app.rules_screen.editor.compiled_title = Regex::new(&pat).ok();
                    }

                    if let Some(tx) = &app.bg_cmd_tx {
                        let compiled_app = app
                            .rules_screen
                            .editor
                            .compiled_app_id
                            .as_ref()
                            .map(|r| std::sync::Arc::new(r.clone()));
                        let compiled_title = if pat.is_empty() {
                            None
                        } else {
                            app.rules_screen
                                .editor
                                .compiled_title
                                .as_ref()
                                .map(|r| std::sync::Arc::new(r.clone()))
                        };
                        let _ = tx.try_send(crate::tui::app::BgCommand::PreviewRequest {
                            app_pattern: app.rules_screen.editor.app_id_pattern.clone(),
                            title_pattern: if pat.is_empty() {
                                None
                            } else {
                                Some(pat.clone())
                            },
                            compiled_app,
                            compiled_title,
                        });
                    }
                }
                2 => {
                    // Sink field - don't type
                }
                3 => {
                    let cur = app.rules_screen.editor.cursor_desc;
                    let new_cur = insert_char_at(&mut app.rules_screen.editor.desc, c, cur);
                    app.rules_screen.editor.cursor_desc = new_cur;
                }
                4 => {
                    if c == ' ' {
                        app.rules_screen.editor.notify = match app.rules_screen.editor.notify {
                            None => Some(true),
                            Some(true) => Some(false),
                            Some(false) => None,
                        };
                    }
                }
                _ => {}
            }
        }
        KeyCode::Left => match app.rules_screen.editor.focused_field {
            0 => {
                app.rules_screen.editor.cursor_app =
                    app.rules_screen.editor.cursor_app.saturating_sub(1)
            }
            1 => {
                app.rules_screen.editor.cursor_title =
                    app.rules_screen.editor.cursor_title.saturating_sub(1)
            }
            3 => {
                app.rules_screen.editor.cursor_desc =
                    app.rules_screen.editor.cursor_desc.saturating_sub(1)
            }
            _ => {}
        },
        KeyCode::Right => match app.rules_screen.editor.focused_field {
            0 => {
                let len = app.rules_screen.editor.app_id_pattern.chars().count();
                app.rules_screen.editor.cursor_app =
                    usize::min(len, app.rules_screen.editor.cursor_app + 1);
            }
            1 => {
                let len = app.rules_screen.editor.title_pattern.chars().count();
                app.rules_screen.editor.cursor_title =
                    usize::min(len, app.rules_screen.editor.cursor_title + 1);
            }
            3 => {
                let len = app.rules_screen.editor.desc.chars().count();
                app.rules_screen.editor.cursor_desc =
                    usize::min(len, app.rules_screen.editor.cursor_desc + 1);
            }
            _ => {}
        },
        KeyCode::Home => match app.rules_screen.editor.focused_field {
            0 => app.rules_screen.editor.cursor_app = 0,
            1 => app.rules_screen.editor.cursor_title = 0,
            3 => app.rules_screen.editor.cursor_desc = 0,
            _ => {}
        },
        KeyCode::End => match app.rules_screen.editor.focused_field {
            0 => {
                app.rules_screen.editor.cursor_app =
                    app.rules_screen.editor.app_id_pattern.chars().count()
            }
            1 => {
                app.rules_screen.editor.cursor_title =
                    app.rules_screen.editor.title_pattern.chars().count()
            }
            3 => app.rules_screen.editor.cursor_desc = app.rules_screen.editor.desc.chars().count(),
            _ => {}
        },
        KeyCode::Backspace => match app.rules_screen.editor.focused_field {
            0 => {
                let cur = app.rules_screen.editor.cursor_app;
                let new_cur = remove_char_before(&mut app.rules_screen.editor.app_id_pattern, cur);
                app.rules_screen.editor.cursor_app = new_cur;
                let pat = app.rules_screen.editor.app_id_pattern.clone();
                if app.rules_screen.editor.compiled_app_id_for.as_ref() != Some(&pat) {
                    app.rules_screen.editor.compiled_app_id_for = Some(pat.clone());
                    app.rules_screen.editor.compiled_app_id = Regex::new(&pat).ok();
                }

                if let Some(tx) = &app.bg_cmd_tx {
                    let compiled_app = app
                        .rules_screen
                        .editor
                        .compiled_app_id
                        .as_ref()
                        .map(|r| std::sync::Arc::new(r.clone()));
                    let compiled_title = if app.rules_screen.editor.title_pattern.is_empty() {
                        None
                    } else {
                        app.rules_screen
                            .editor
                            .compiled_title
                            .as_ref()
                            .map(|r| std::sync::Arc::new(r.clone()))
                    };
                    let _ = tx.try_send(crate::tui::app::BgCommand::PreviewRequest {
                        app_pattern: pat.clone(),
                        title_pattern: if app.rules_screen.editor.title_pattern.is_empty() {
                            None
                        } else {
                            Some(app.rules_screen.editor.title_pattern.clone())
                        },
                        compiled_app,
                        compiled_title,
                    });
                }
            }
            1 => {
                let cur = app.rules_screen.editor.cursor_title;
                let new_cur = remove_char_before(&mut app.rules_screen.editor.title_pattern, cur);
                app.rules_screen.editor.cursor_title = new_cur;
                let pat = app.rules_screen.editor.title_pattern.clone();
                if app.rules_screen.editor.compiled_title_for.as_ref() != Some(&pat) {
                    app.rules_screen.editor.compiled_title_for = Some(pat.clone());
                    app.rules_screen.editor.compiled_title = Regex::new(&pat).ok();
                }

                if let Some(tx) = &app.bg_cmd_tx {
                    let compiled_app = app
                        .rules_screen
                        .editor
                        .compiled_app_id
                        .as_ref()
                        .map(|r| std::sync::Arc::new(r.clone()));
                    let compiled_title = if pat.is_empty() {
                        None
                    } else {
                        app.rules_screen
                            .editor
                            .compiled_title
                            .as_ref()
                            .map(|r| std::sync::Arc::new(r.clone()))
                    };
                    let _ = tx.try_send(crate::tui::app::BgCommand::PreviewRequest {
                        app_pattern: app.rules_screen.editor.app_id_pattern.clone(),
                        title_pattern: if pat.is_empty() {
                            None
                        } else {
                            Some(pat.clone())
                        },
                        compiled_app,
                        compiled_title,
                    });
                }
            }
            3 => {
                let cur = app.rules_screen.editor.cursor_desc;
                let new_cur = remove_char_before(&mut app.rules_screen.editor.desc, cur);
                app.rules_screen.editor.cursor_desc = new_cur;
            }
            _ => {}
        },
        KeyCode::Delete => match app.rules_screen.editor.focused_field {
            0 => {
                let cur = app.rules_screen.editor.cursor_app;
                let new_cur = remove_char_at(&mut app.rules_screen.editor.app_id_pattern, cur);
                app.rules_screen.editor.cursor_app = new_cur;
                let pat = app.rules_screen.editor.app_id_pattern.clone();
                if app.rules_screen.editor.compiled_app_id_for.as_ref() != Some(&pat) {
                    app.rules_screen.editor.compiled_app_id_for = Some(pat.clone());
                    app.rules_screen.editor.compiled_app_id = Regex::new(&pat).ok();
                }

                if let Some(tx) = &app.bg_cmd_tx {
                    let compiled_app = app
                        .rules_screen
                        .editor
                        .compiled_app_id
                        .as_ref()
                        .map(|r| std::sync::Arc::new(r.clone()));
                    let compiled_title = if app.rules_screen.editor.title_pattern.is_empty() {
                        None
                    } else {
                        app.rules_screen
                            .editor
                            .compiled_title
                            .as_ref()
                            .map(|r| std::sync::Arc::new(r.clone()))
                    };
                    let _ = tx.try_send(crate::tui::app::BgCommand::PreviewRequest {
                        app_pattern: pat.clone(),
                        title_pattern: if app.rules_screen.editor.title_pattern.is_empty() {
                            None
                        } else {
                            Some(app.rules_screen.editor.title_pattern.clone())
                        },
                        compiled_app,
                        compiled_title,
                    });
                }
            }
            1 => {
                let cur = app.rules_screen.editor.cursor_title;
                let new_cur = remove_char_at(&mut app.rules_screen.editor.title_pattern, cur);
                app.rules_screen.editor.cursor_title = new_cur;
                let pat = app.rules_screen.editor.title_pattern.clone();
                if app.rules_screen.editor.compiled_title_for.as_ref() != Some(&pat) {
                    app.rules_screen.editor.compiled_title_for = Some(pat.clone());
                    app.rules_screen.editor.compiled_title = Regex::new(&pat).ok();
                }

                if let Some(tx) = &app.bg_cmd_tx {
                    let compiled_app = app
                        .rules_screen
                        .editor
                        .compiled_app_id
                        .as_ref()
                        .map(|r| std::sync::Arc::new(r.clone()));
                    let compiled_title = if pat.is_empty() {
                        None
                    } else {
                        app.rules_screen
                            .editor
                            .compiled_title
                            .as_ref()
                            .map(|r| std::sync::Arc::new(r.clone()))
                    };
                    let _ = tx.try_send(crate::tui::app::BgCommand::PreviewRequest {
                        app_pattern: app.rules_screen.editor.app_id_pattern.clone(),
                        title_pattern: if pat.is_empty() {
                            None
                        } else {
                            Some(pat.clone())
                        },
                        compiled_app,
                        compiled_title,
                    });
                }
            }
            3 => {
                let cur = app.rules_screen.editor.cursor_desc;
                let new_cur = remove_char_at(&mut app.rules_screen.editor.desc, cur);
                app.rules_screen.editor.cursor_desc = new_cur;
            }
            _ => {}
        },
        KeyCode::Enter => {
            // If on sink field, open selector
            if app.rules_screen.editor.focused_field == 2 {
                app.rules_screen.open_sink_selector();
                return;
            }

            // Otherwise, save the rule
            if app.rules_screen.editor.app_id_pattern.is_empty() {
                app.set_status("App ID pattern is required".to_string());
                return;
            }

            if app.rules_screen.editor.sink_ref.is_empty() {
                app.set_status("Target sink is required".to_string());
                return;
            }

            // Validate regexes
            let app_id_regex = match Regex::new(&app.rules_screen.editor.app_id_pattern) {
                Ok(r) => r,
                Err(e) => {
                    app.set_status(format!("Invalid app_id regex: {e}"));
                    return;
                }
            };

            let title_regex = if app.rules_screen.editor.title_pattern.is_empty() {
                None
            } else {
                match Regex::new(&app.rules_screen.editor.title_pattern) {
                    Ok(r) => Some(r),
                    Err(e) => {
                        app.set_status(format!("Invalid title regex: {e}"));
                        return;
                    }
                }
            };

            let new_rule = Rule {
                app_id_regex,
                title_regex,
                sink_ref: app.rules_screen.editor.sink_ref.clone(),
                desc: if app.rules_screen.editor.desc.is_empty() {
                    None
                } else {
                    Some(app.rules_screen.editor.desc.clone())
                },
                notify: app.rules_screen.editor.notify,
                app_id_pattern: app.rules_screen.editor.app_id_pattern.clone(),
                title_pattern: if app.rules_screen.editor.title_pattern.is_empty() {
                    None
                } else {
                    Some(app.rules_screen.editor.title_pattern.clone())
                },
            };

            if let Some(idx) = app.rules_screen.editing_index {
                app.config.rules[idx] = new_rule;
                app.set_status("Rule updated".to_string());
            } else {
                app.config.rules.push(new_rule);
                app.set_status("Rule added".to_string());
            }

            app.mark_dirty();
            app.rules_screen.cancel();
        }
        KeyCode::Esc => {
            app.rules_screen.cancel();
        }
        _ => {}
    }
}

/// Handle mouse input
fn handle_mouse_event(_app: &mut App, mouse: MouseEvent) {
    match mouse.kind {
        MouseEventKind::Down(_button) => {
            // Mouse click handling will be added in Phase 3-4
            // For now, we just acknowledge the event
        }
        MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
            // Scroll handling for lists will be added in Phase 3-4
        }
        _ => {}
    }
}
