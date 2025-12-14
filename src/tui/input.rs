//! Input handling for keyboard and mouse events

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind, MouseButton};
use std::time::Duration;

use super::app::{App, DaemonAction, Screen};
use super::screens::rules::RulesMode;
use super::screens::sinks::SinksMode;
use crate::config::{Rule, SinkConfig};
use regex::Regex;
use unicode_segmentation::UnicodeSegmentation;

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

                    // invalidate/compile cache
                    let pat = app.rules_screen.editor.app_id_pattern.value.clone();
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
                        let compiled_title =
                            if app.rules_screen.editor.title_pattern.value.is_empty() {
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
                            title_pattern: if app.rules_screen.editor.title_pattern.value.is_empty()
                            {
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

                    let pat = app.rules_screen.editor.title_pattern.value.clone();
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
                let pat = app.rules_screen.editor.app_id_pattern.value.clone();
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
                    let compiled_title = if app.rules_screen.editor.title_pattern.value.is_empty() {
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
                    app.rules_screen.editor.compiled_app_id = Regex::new(&pat).ok();
                }

                if let Some(tx) = &app.bg_cmd_tx {
                    let compiled_app = app
                        .rules_screen
                        .editor
                        .compiled_app_id
                        .as_ref()
                        .map(|r| std::sync::Arc::new(r.clone()));
                    let compiled_title = if app.rules_screen.editor.title_pattern.value.is_empty() {
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

/// Handle mouse input
fn handle_mouse_event(app: &mut App, mouse: MouseEvent) {
    use crate::tui::textfield::compute_display_window;
    use crate::tui::widgets::centered_rect;
    use crossterm::terminal::size as terminal_size;

    // Optional debug: show raw mouse events when PWSW_TUI_DEBUG_MOUSE is set
    if std::env::var("PWSW_TUI_DEBUG_MOUSE").is_ok() {
        app.set_status(format!("Mouse event: {:?}", mouse));
    }

    match mouse.kind {
        MouseEventKind::Down(button) => {
            // Only handle left-button presses for click-to-cursor mapping
            if button != MouseButton::Left {
                return;
            }
            // Translate mouse (column,row) into terminal Rect coords
            let (cols, rows) = match terminal_size() {
                Ok((c, r)) => (c as i16, r as i16),
                Err(_) => return,
            };
            let term_rect = ratatui::layout::Rect::new(0, 0, cols as u16, rows as u16);

            // Helper: compute click mapping for a text field
            let click_set_cursor = |area: ratatui::layout::Rect,
                                    label: &str,
                                    value: &str,
                                    cursor: usize,
                                    click_x: i16,
                                    click_y: i16|
             -> Option<usize> {
                // area is widget rect; label occupies label.len()+1 cols at left
                let label_len = label.len() + 1; // space after label
                let area_x = area.x as i16;
                let area_y = area.y as i16;
                if click_y < area_y || click_y >= area_y + area.height as i16 {
                    return None;
                }
                // click column relative to area
                let rel_col = click_x - area_x;
                if rel_col < 0 {
                    return None;
                }
                let area_width = area.width as usize;
                let mut max_value_len = (area_width as isize - label_len as isize) as usize;
                if max_value_len == 0 {
                    return Some(0);
                }
                // Reserve one char for cursor as render_text_field does
                if max_value_len > 0 {
                    if max_value_len > 1 {
                        max_value_len -= 1;
                    } else {
                        max_value_len = 0;
                    }
                }

                // click position relative to value area (not including label)
                let click_in_value = rel_col as usize - label_len;
                if (rel_col as usize) < label_len {
                    return None;
                }

                // Use compute_display_window with current cursor to reproduce displayed substring and start
                let (display_substr, _cursor_rel, truncated_left, start) =
                    compute_display_window(value, cursor, max_value_len);

                // When truncated_left is true, render shows an ellipsis occupying one column at start
                let mut rel = click_in_value;
                if truncated_left {
                    if rel == 0 {
                        // clicked the ellipsis - move to start
                        return Some(start);
                    } else {
                        rel = rel.saturating_sub(1);
                    }
                }

                let disp_len = display_substr.graphemes(true).count();
                let char_pos = if rel >= disp_len { disp_len } else { rel };
                Some(start + char_pos)
            };

            let mx = mouse.column as i16;
            let my = mouse.row as i16;

            match app.current_screen {
                Screen::Sinks => {
                    if app.sinks_screen.mode == SinksMode::AddEdit {
                        // popup area matches render_sinks centered_rect(70,60)
                        let popup = centered_rect(70, 60, term_rect);
                        let chunks = ratatui::layout::Layout::default()
                            .direction(ratatui::layout::Direction::Vertical)
                            .margin(2)
                            .constraints([
                                ratatui::layout::Constraint::Length(3), // Name
                                ratatui::layout::Constraint::Length(3), // Desc
                                ratatui::layout::Constraint::Length(3), // Icon
                                ratatui::layout::Constraint::Length(3), // Default
                                ratatui::layout::Constraint::Min(0),
                            ])
                            .split(popup);

                        // Figure out which field chunk was clicked
                        for (i, chunk) in chunks.iter().take(3).enumerate() {
                            if mx >= chunk.x as i16
                                && mx < (chunk.x + chunk.width) as i16
                                && my >= chunk.y as i16
                                && my < (chunk.y + chunk.height) as i16
                            {
                                let editor = &mut app.sinks_screen.editor;
                                match i {
                                    0 => {
                                        if let Some(newc) = click_set_cursor(
                                            *chunk,
                                            "Node Name:",
                                            &editor.name.value,
                                            editor.name.cursor,
                                            mx,
                                            my,
                                        ) {
                                            editor.name.cursor = newc;
                                            editor.focused_field = 0;
                                        }
                                    }
                                    1 => {
                                        if let Some(newc) = click_set_cursor(
                                            *chunk,
                                            "Description:",
                                            &editor.desc.value,
                                            editor.desc.cursor,
                                            mx,
                                            my,
                                        ) {
                                            editor.desc.cursor = newc;
                                            editor.focused_field = 1;
                                        }
                                    }
                                    2 => {
                                        if let Some(newc) = click_set_cursor(
                                            *chunk,
                                            "Icon (optional):",
                                            &editor.icon.value,
                                            editor.icon.cursor,
                                            mx,
                                            my,
                                        ) {
                                            editor.icon.cursor = newc;
                                            editor.focused_field = 2;
                                        }
                                    }
                                    _ => {}
                                }
                            }
                        }
                    }
                }
                Screen::Rules => {
                    if app.rules_screen.mode == RulesMode::AddEdit {
                        // popup area matches render_rules centered_rect(80,85)
                        let popup = centered_rect(80, 85, term_rect);
                        let chunks = ratatui::layout::Layout::default()
                            .direction(ratatui::layout::Direction::Vertical)
                            .margin(2)
                            .constraints([
                                ratatui::layout::Constraint::Length(3), // App ID
                                ratatui::layout::Constraint::Length(3), // Title
                                ratatui::layout::Constraint::Length(3), // Sink
                                ratatui::layout::Constraint::Length(3), // Desc
                                ratatui::layout::Constraint::Length(3), // Notify
                                ratatui::layout::Constraint::Min(5),    // Preview
                                ratatui::layout::Constraint::Length(3), // Help
                            ])
                            .split(popup);

                        for (i, chunk) in chunks.iter().enumerate() {
                            if mx >= chunk.x as i16
                                && mx < (chunk.x + chunk.width) as i16
                                && my >= chunk.y as i16
                                && my < (chunk.y + chunk.height) as i16
                            {
                                let editor = &mut app.rules_screen.editor;
                                match i {
                                    0 => {
                                        if let Some(newc) = click_set_cursor(
                                            *chunk,
                                            "App ID Pattern (regex):",
                                            &editor.app_id_pattern.value,
                                            editor.app_id_pattern.cursor,
                                            mx,
                                            my,
                                        ) {
                                            editor.app_id_pattern.cursor = newc;
                                            editor.focused_field = 0;
                                        }
                                    }
                                    1 => {
                                        if let Some(newc) = click_set_cursor(
                                            *chunk,
                                            "Title Pattern (optional regex):",
                                            &editor.title_pattern.value,
                                            editor.title_pattern.cursor,
                                            mx,
                                            my,
                                        ) {
                                            editor.title_pattern.cursor = newc;
                                            editor.focused_field = 1;
                                        }
                                    }
                                    3 => {
                                        if let Some(newc) = click_set_cursor(
                                            *chunk,
                                            "Description (optional):",
                                            &editor.desc.value,
                                            editor.desc.cursor,
                                            mx,
                                            my,
                                        ) {
                                            editor.desc.cursor = newc;
                                            editor.focused_field = 3;
                                        }
                                    }
                                    _ => {}
                                }
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
            // Scroll handling for lists will be added in Phase 3-4
        }
        _ => {}
    }
}
