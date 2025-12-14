//! Input handling for keyboard events

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use std::time::Duration;

use super::app::{App, DaemonAction, Screen};
use super::screens::rules::RulesMode;
use super::screens::sinks::SinksMode;
use crate::config::{Rule, SinkConfig};
use regex::Regex;


/// Poll timeout for event checking (non-blocking). Set to 0 to avoid blocking in the tick loop.
const POLL_TIMEOUT: Duration = Duration::from_millis(0);
/// Maximum number of input events to process per tick. Prevents long blocking when the terminal
/// emits a large flood of input events.
const MAX_EVENTS_PER_TICK: usize = 64;

/// Handle keyboard input events
///
/// # Errors
/// Returns an error if event polling fails.
pub fn handle_events(app: &mut App) -> Result<()> {
    // Process up to MAX_EVENTS_PER_TICK pending events to avoid blocking the UI for too long.
    for _ in 0..MAX_EVENTS_PER_TICK {
        if !event::poll(POLL_TIMEOUT)? {
            break;
        }
        match event::read()? {
            Event::Key(key_event) => { handle_key_event(app, key_event); app.dirty = true; }
            Event::Mouse(_) => {
                // Mouse events are intentionally ignored in the keyboard-first TUI.
            }
            Event::Resize(_, _) => {
                // Ratatui handles resize automatically, but mark dirty so UI redraws at new size
                app.dirty = true;
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
                    app.sinks_screen.editor.name.insert(c);
                }
                1 => {
                    app.sinks_screen.editor.desc.insert(c);
                }
                2 => {
                    app.sinks_screen.editor.icon.insert(c);
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
            0 => app.sinks_screen.editor.name.move_left(),
            1 => app.sinks_screen.editor.desc.move_left(),
            2 => app.sinks_screen.editor.icon.move_left(),
            _ => {}
        },
        KeyCode::Right => match app.sinks_screen.editor.focused_field {
            0 => app.sinks_screen.editor.name.move_right(),
            1 => app.sinks_screen.editor.desc.move_right(),
            2 => app.sinks_screen.editor.icon.move_right(),
            _ => {}
        },
        KeyCode::Home => match app.sinks_screen.editor.focused_field {
            0 => app.sinks_screen.editor.name.move_home(),
            1 => app.sinks_screen.editor.desc.move_home(),
            2 => app.sinks_screen.editor.icon.move_home(),
            _ => {}
        },
        KeyCode::End => match app.sinks_screen.editor.focused_field {
            0 => app.sinks_screen.editor.name.move_end(),
            1 => app.sinks_screen.editor.desc.move_end(),
            2 => app.sinks_screen.editor.icon.move_end(),
            _ => {}
        },
        KeyCode::Backspace => match app.sinks_screen.editor.focused_field {
            0 => {
                app.sinks_screen.editor.name.remove_before();
            }
            1 => {
                app.sinks_screen.editor.desc.remove_before();
            }
            2 => {
                app.sinks_screen.editor.icon.remove_before();
            }
            _ => {}
        },
        KeyCode::Delete => match app.sinks_screen.editor.focused_field {
            0 => {
                app.sinks_screen.editor.name.remove_at();
            }
            1 => {
                app.sinks_screen.editor.desc.remove_at();
            }
            2 => {
                app.sinks_screen.editor.icon.remove_at();
            }
            _ => {}
        },
        KeyCode::Enter => {
            // Save the sink
            if app.sinks_screen.editor.name.value.is_empty()
                || app.sinks_screen.editor.desc.value.is_empty()
            {
                app.set_status("Name and Description are required".to_string());
                return;
            }

            let new_sink = SinkConfig {
                name: app.sinks_screen.editor.name.value.clone(),
                desc: app.sinks_screen.editor.desc.value.clone(),
                icon: if app.sinks_screen.editor.icon.value.is_empty() {
                    None
                } else {
                    Some(app.sinks_screen.editor.icon.value.clone())
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

    match key.code {
        KeyCode::Tab => app.rules_screen.editor.next_field(),
        KeyCode::BackTab => app.rules_screen.editor.prev_field(),
        KeyCode::Char(c) => {
            match app.rules_screen.editor.focused_field {
                0 => {
                    // insert at cursor_app
                    app.rules_screen.editor.app_id_pattern.insert(c);

                    // Ensure compiled caches are updated eagerly
                    app.rules_screen.editor.ensure_compiled();

                    // Request live-preview
                    if let Some(tx) = &app.bg_cmd_tx {
                        let compiled_app = app.rules_screen.editor.compiled_app_id.clone();
                        let compiled_title = app.rules_screen.editor.compiled_title.clone();
                        let _ = tx.try_send(crate::tui::app::BgCommand::PreviewRequest {
                            app_pattern: app.rules_screen.editor.app_id_pattern.value.clone(),
                            title_pattern: if app.rules_screen.editor.title_pattern.value.is_empty() {
                                None
                            } else {
                                Some(app.rules_screen.editor.title_pattern.value.clone())
                            },
                            compiled_app,
                            compiled_title,
                        });
                    }
                }
                1 => {
                    app.rules_screen.editor.title_pattern.insert(c);

                    // Ensure compiled caches are updated eagerly
                    app.rules_screen.editor.ensure_compiled();

                    if let Some(tx) = &app.bg_cmd_tx {
                        let compiled_app = app
                            .rules_screen
                            .editor
                            .compiled_app_id
                            .clone();
                        let compiled_title = app
                            .rules_screen
                            .editor
                            .compiled_title
                            .clone();
                        let _ = tx.try_send(crate::tui::app::BgCommand::PreviewRequest {
                            app_pattern: app.rules_screen.editor.app_id_pattern.value.clone(),
                            title_pattern: if app.rules_screen.editor.title_pattern.value.is_empty() {
                                None
                            } else {
                                Some(app.rules_screen.editor.title_pattern.value.clone())
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
                    app.rules_screen.editor.desc.insert(c);
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
            0 => app.rules_screen.editor.app_id_pattern.move_left(),
            1 => app.rules_screen.editor.title_pattern.move_left(),
            3 => app.rules_screen.editor.desc.move_left(),
            _ => {}
        },
        KeyCode::Right => match app.rules_screen.editor.focused_field {
            0 => app.rules_screen.editor.app_id_pattern.move_right(),
            1 => app.rules_screen.editor.title_pattern.move_right(),
            3 => app.rules_screen.editor.desc.move_right(),
            _ => {}
        },
        KeyCode::Home => match app.rules_screen.editor.focused_field {
            0 => app.rules_screen.editor.app_id_pattern.move_home(),
            1 => app.rules_screen.editor.title_pattern.move_home(),
            3 => app.rules_screen.editor.desc.move_home(),
            _ => {}
        },
        KeyCode::End => match app.rules_screen.editor.focused_field {
            0 => app.rules_screen.editor.app_id_pattern.move_end(),
            1 => app.rules_screen.editor.title_pattern.move_end(),
            3 => app.rules_screen.editor.desc.move_end(),
            _ => {}
        },
        KeyCode::Backspace => match app.rules_screen.editor.focused_field {
                0 => {
                app.rules_screen.editor.app_id_pattern.remove_before();

                // Ensure compiled caches updated eagerly
                app.rules_screen.editor.ensure_compiled();

                if let Some(tx) = &app.bg_cmd_tx {
let compiled_app = app.rules_screen.editor.compiled_app_id.clone();
                        let compiled_title = app.rules_screen.editor.compiled_title.clone();
                    let _ = tx.try_send(crate::tui::app::BgCommand::PreviewRequest {
                        app_pattern: app.rules_screen.editor.app_id_pattern.value.clone(),
                        title_pattern: if app.rules_screen.editor.title_pattern.value.is_empty() {
                            None
                        } else {
                            Some(app.rules_screen.editor.title_pattern.value.clone())
                        },
                        compiled_app,
                        compiled_title,
                    });
                }
            }
            1 => {
                app.rules_screen.editor.title_pattern.remove_before();
                let pat = app.rules_screen.editor.title_pattern.value.clone();
                if app.rules_screen.editor.compiled_title_for.as_ref() != Some(&pat) {
                    app.rules_screen.editor.compiled_title_for = Some(pat.clone());
                    app.rules_screen.editor.compiled_title = Regex::new(&pat).ok().map(std::sync::Arc::new);
                }

                if let Some(tx) = &app.bg_cmd_tx {
                    let compiled_app = app
                        .rules_screen
                        .editor
                        .compiled_app_id
                        .clone();
                    let compiled_title = if pat.is_empty() {
                        None
                    } else {
                        app.rules_screen
                            .editor
                            .compiled_title
                            .clone()
                    };
                    let _ = tx.try_send(crate::tui::app::BgCommand::PreviewRequest {
                        app_pattern: app.rules_screen.editor.app_id_pattern.value.clone(),
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
                app.rules_screen.editor.desc.remove_before();
            }
            _ => {}
        },
        KeyCode::Delete => match app.rules_screen.editor.focused_field {
            0 => {
                app.rules_screen.editor.app_id_pattern.remove_at();
                let pat = app.rules_screen.editor.app_id_pattern.value.clone();
                if app.rules_screen.editor.compiled_app_id_for.as_ref() != Some(&pat) {
                    app.rules_screen.editor.compiled_app_id_for = Some(pat.clone());
                    app.rules_screen.editor.compiled_app_id = Regex::new(&pat).ok().map(std::sync::Arc::new);
                }

                if let Some(tx) = &app.bg_cmd_tx {
let compiled_app = app.rules_screen.editor.compiled_app_id.clone();
                    let compiled_title = if pat.is_empty() { None } else { app.rules_screen.editor.compiled_title.clone() };
                    let _ = tx.try_send(crate::tui::app::BgCommand::PreviewRequest {
                        app_pattern: pat.clone(),
                        title_pattern: if app.rules_screen.editor.title_pattern.value.is_empty() {
                            None
                        } else {
                            Some(app.rules_screen.editor.title_pattern.value.clone())
                        },
                        compiled_app,
                        compiled_title,
                    });
                }
            }
            1 => {
                app.rules_screen.editor.title_pattern.remove_at();
                let pat = app.rules_screen.editor.title_pattern.value.clone();
                if app.rules_screen.editor.compiled_title_for.as_ref() != Some(&pat) {
                    app.rules_screen.editor.compiled_title_for = Some(pat.clone());
                    app.rules_screen.editor.compiled_title = Regex::new(&pat).ok().map(std::sync::Arc::new);
                }

                if let Some(tx) = &app.bg_cmd_tx {
                    let compiled_app = app
                        .rules_screen
                        .editor
                        .compiled_app_id
                        .clone();
                    let compiled_title = if pat.is_empty() {
                        None
                    } else {
                        app.rules_screen
                            .editor
                            .compiled_title
                            .clone()
                    };
                    let _ = tx.try_send(crate::tui::app::BgCommand::PreviewRequest {
                        app_pattern: app.rules_screen.editor.app_id_pattern.value.clone(),
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
                app.rules_screen.editor.desc.remove_at();
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
            if app.rules_screen.editor.app_id_pattern.value.is_empty() {
                app.set_status("App ID pattern is required".to_string());
                return;
            }

            if app.rules_screen.editor.sink_ref.is_empty() {
                app.set_status("Target sink is required".to_string());
                return;
            }

            // Validate regexes
            let app_id_regex = match Regex::new(&app.rules_screen.editor.app_id_pattern.value) {
                Ok(r) => r,
                Err(e) => {
                    app.set_status(format!("Invalid app_id regex: {e}"));
                    return;
                }
            };

            let title_regex = if app.rules_screen.editor.title_pattern.value.is_empty() {
                None
            } else {
                match Regex::new(&app.rules_screen.editor.title_pattern.value) {
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
                desc: if app.rules_screen.editor.desc.value.is_empty() {
                    None
                } else {
                    Some(app.rules_screen.editor.desc.value.clone())
                },
                notify: app.rules_screen.editor.notify,
                app_id_pattern: app.rules_screen.editor.app_id_pattern.value.clone(),
                title_pattern: if app.rules_screen.editor.title_pattern.value.is_empty() {
                    None
                } else {
                    Some(app.rules_screen.editor.title_pattern.value.clone())
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
