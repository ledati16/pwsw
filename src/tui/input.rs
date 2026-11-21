//! Input handling for keyboard events

use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use tui_input::backend::crossterm::EventHandler;

use super::app::{App, BgCommand, DaemonAction, Screen, StatusKind};
use super::screens::rules::{RuleField, RulesMode};
use super::screens::settings::LOG_LEVELS;
use super::screens::sinks::{SinkField, SinksMode, selector_names};
use crate::config::{Rule, SinkConfig, compile_rule_regex};

/// Handle a single input event and update app state
///
/// Key presses go to `handle_key_event`; a bracketed paste goes to the focused
/// text field only (never to key bindings); resizes trigger a redraw. While a quit
/// waits for an in-flight save, everything but Esc (cancel the quit) is ignored.
pub fn handle_event(app: &mut App, event: &Event) {
    // Quitting, waiting for an in-flight save: only Esc (cancel the quit) does anything
    if app.is_quitting_after_save() {
        match event {
            Event::Key(key) if key.kind != KeyEventKind::Release && key.code == KeyCode::Esc => {
                app.cancel_quit_after_save();
            }
            Event::Resize(..) => app.dirty = true,
            _ => {}
        }
        return;
    }
    match event {
        Event::Key(key_event) if key_event.kind != KeyEventKind::Release => {
            handle_key_event(app, *key_event);
            app.dirty = true;
        }
        Event::Paste(text) => app.paste(text),
        // Ratatui handles resize automatically, but mark dirty so UI redraws at new size
        Event::Resize(..) => app.dirty = true,
        _ => {}
    }
}

#[cfg(test)]
pub fn simulate_key_event(app: &mut crate::tui::app::App, key: crossterm::event::KeyEvent) {
    handle_key_event(app, key);
}

/// Check if any modal or editor is currently active
fn is_modal_active(app: &App) -> bool {
    // Check screen-specific modals
    match app.current_screen {
        Screen::Sinks => app.sinks_screen.mode != SinksMode::List,
        Screen::Rules => app.rules_screen.mode != RulesMode::List,
        Screen::Settings => app.settings_screen.editing_log_level,
        Screen::Dashboard => false,
    }
}

/// Handle keyboard input
// Input dispatch across all screens - cohesive routing logic
fn handle_key_event(app: &mut App, key: KeyEvent) {
    let ctrl_c = key.code == KeyCode::Char('c') && key.modifiers == KeyModifiers::CONTROL;

    // A warning or error stays until the user does something next
    app.dismiss_sticky_status();

    // A pending "unsaved changes" quit prompt: the very next key decides. q (where
    // it is a command, not text) or Ctrl+C quits; Esc only dismisses the prompt;
    // any other key dismisses it and then does its normal job.
    if app.confirm_quit {
        let q_command = key.code == KeyCode::Char('q')
            && key.modifiers == KeyModifiers::NONE
            && !is_modal_active(app)
            && !app.save_conflict;
        if ctrl_c || q_command {
            app.quit();
            return;
        }
        app.cancel_quit();
        if key.code == KeyCode::Esc {
            return;
        }
    }

    // Always-global keybindings (work even in modals)
    if ctrl_c {
        app.request_quit();
        return;
    }
    if key.code == KeyCode::Char('s') && key.modifiers == KeyModifiers::CONTROL {
        if app.config_dirty {
            // config_dirty stays until the save reports back
            app.request_save(false);
        } else {
            app.set_status(StatusKind::Info, "No unsaved changes");
        }
        return;
    }

    // "Changed on disk" dialog (blocks everything else)
    if app.save_conflict {
        match key.code {
            KeyCode::Char('o') => {
                app.save_conflict = false;
                app.request_save(true);
            }
            KeyCode::Char('r') => {
                app.save_conflict = false;
                app.request_reload();
            }
            KeyCode::Esc => {
                app.save_conflict = false;
                app.set_status(StatusKind::Info, "Not saved; your changes are kept");
            }
            _ => {}
        }
        return;
    }

    // Help overlay input (blocks everything else)
    if app.show_help {
        // One page = the help viewport measured at the last render
        let page = app.help_viewport_height.max(1);
        match key.code {
            KeyCode::Esc | KeyCode::F(1) | KeyCode::Char('?' | 'q') => {
                app.show_help = false;
            }
            KeyCode::Char(' ') => {
                // Toggle section at current selected row
                if let Some(selected_row) = app.help_scroll_state.selected()
                    && let Some(section_name) = crate::tui::screens::help::get_section_at_row(
                        app.current_screen,
                        &app.help_collapsed_sections,
                        selected_row,
                    )
                {
                    if app.help_collapsed_sections.contains(&section_name) {
                        app.help_collapsed_sections.remove(&section_name);
                    } else {
                        app.help_collapsed_sections.insert(section_name);
                    }
                    app.dirty = true;
                }
            }
            KeyCode::Up => {
                let current_selected = app.help_scroll_state.selected().unwrap_or(0);
                if let Some(prev_section) = crate::tui::screens::help::find_prev_section_header(
                    app.current_screen,
                    &app.help_collapsed_sections,
                    current_selected,
                ) {
                    app.help_scroll_state.select(Some(prev_section));

                    // Adjust scroll offset if cursor moves above viewport
                    let current_offset = app.help_scroll_state.offset();
                    if prev_section < current_offset {
                        *app.help_scroll_state.offset_mut() = prev_section;
                    }
                }
            }
            KeyCode::Down => {
                let current_selected = app.help_scroll_state.selected().unwrap_or(0);
                if let Some(next_section) = crate::tui::screens::help::find_next_section_header(
                    app.current_screen,
                    &app.help_collapsed_sections,
                    current_selected,
                ) {
                    app.help_scroll_state.select(Some(next_section));

                    // Adjust scroll offset if cursor moves below viewport
                    let current_offset = app.help_scroll_state.offset();
                    let viewport_bottom = current_offset + app.help_viewport_height;
                    if next_section >= viewport_bottom {
                        *app.help_scroll_state.offset_mut() = current_offset + 1;
                    }
                }
            }
            KeyCode::Home => {
                // Jump to first section header (row 0)
                app.reset_help_scroll();
            }
            KeyCode::End => {
                let row_count = crate::tui::screens::help::get_help_row_count(
                    app.current_screen,
                    &app.help_collapsed_sections,
                );

                // Find last section header by searching backwards from end
                let mut last_section = row_count.saturating_sub(1);
                for row in (0..row_count).rev() {
                    if crate::tui::screens::help::get_section_at_row(
                        app.current_screen,
                        &app.help_collapsed_sections,
                        row,
                    )
                    .is_some()
                    {
                        last_section = row;
                        break;
                    }
                }

                app.help_scroll_state.select(Some(last_section));

                let max_offset = crate::tui::screens::help::get_help_max_offset(
                    app.current_screen,
                    &app.help_collapsed_sections,
                    app.help_viewport_height,
                );
                *app.help_scroll_state.offset_mut() = max_offset;
            }
            KeyCode::PageUp => {
                let current_selected = app.help_scroll_state.selected().unwrap_or(0);
                let new_selected = current_selected.saturating_sub(page);
                app.help_scroll_state.select(Some(new_selected));

                let current_offset = app.help_scroll_state.offset();
                let new_offset = current_offset.saturating_sub(page);
                *app.help_scroll_state.offset_mut() = new_offset;
            }
            KeyCode::PageDown => {
                let row_count = crate::tui::screens::help::get_help_row_count(
                    app.current_screen,
                    &app.help_collapsed_sections,
                );
                let current_selected = app.help_scroll_state.selected().unwrap_or(0);
                let new_selected = current_selected
                    .saturating_add(page)
                    .min(row_count.saturating_sub(1));
                app.help_scroll_state.select(Some(new_selected));

                let max_offset = crate::tui::screens::help::get_help_max_offset(
                    app.current_screen,
                    &app.help_collapsed_sections,
                    app.help_viewport_height,
                );
                let current_offset = app.help_scroll_state.offset();
                let new_offset = current_offset.saturating_add(page).min(max_offset);
                *app.help_scroll_state.offset_mut() = new_offset;
            }
            _ => {}
        }
        app.dirty = true;
        return;
    }

    // If a modal is active (other than help), pass most keys to screen-specific handlers
    if is_modal_active(app) {
        match key.code {
            // F1: Toggle help overlay (global)
            KeyCode::F(1) => {
                app.show_help = true;
                app.reset_help_scroll();
            }
            // ? opens help over a modal too, unless it is being typed into a field
            KeyCode::Char('?') if !app.is_input_focused() => {
                app.show_help = true;
                app.reset_help_scroll();
            }
            _ => handle_screen_specific_input(app, key),
        }
        return;
    }

    // Normal global keybindings (only when no modal is active)
    match (key.code, key.modifiers) {
        (KeyCode::Char('q'), KeyModifiers::NONE) => app.request_quit(),

        // Tab: next screen
        (KeyCode::Tab, KeyModifiers::NONE) => {
            app.next_screen();
        }

        // Shift+Tab: previous screen
        (KeyCode::BackTab, KeyModifiers::SHIFT) => {
            app.prev_screen();
        }

        // Direct screen navigation shortcuts
        (KeyCode::Char('1'), KeyModifiers::NONE) => {
            app.goto_screen(Screen::Dashboard);
        }
        (KeyCode::Char('2'), KeyModifiers::NONE) => {
            app.goto_screen(Screen::Sinks);
        }
        (KeyCode::Char('3'), KeyModifiers::NONE) => {
            app.goto_screen(Screen::Rules);
        }
        (KeyCode::Char('4'), KeyModifiers::NONE) => {
            app.goto_screen(Screen::Settings);
        }

        // Escape: clear status message
        (KeyCode::Esc, KeyModifiers::NONE) => app.clear_status(),

        // F1: Always toggle help overlay (global)
        (KeyCode::F(1), _) => {
            app.show_help = true;
            app.reset_help_scroll();
        }

        // ?: Toggle help overlay (only if no input focused)
        (KeyCode::Char('?'), KeyModifiers::NONE | KeyModifiers::SHIFT)
            if !app.is_input_focused() =>
        {
            app.show_help = true;
            app.reset_help_scroll();
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
    let dashboard = &mut app.dashboard_screen;
    match (key.code, key.modifiers) {
        // Toggle between Logs and Windows view
        (KeyCode::Char('w'), KeyModifiers::NONE) => dashboard.toggle_view(),

        // Left/Right for horizontal daemon action navigation
        (KeyCode::Left, KeyModifiers::NONE) => dashboard.select_previous(),
        (KeyCode::Right, KeyModifiers::NONE) => dashboard.select_next(),

        // Scrolling (view-aware; page size and limits come from the last render)
        (KeyCode::Up, KeyModifiers::NONE) => dashboard.scroll_up(1),
        (KeyCode::Down, KeyModifiers::NONE) => dashboard.scroll_down(1),
        (KeyCode::PageUp, _) => dashboard.scroll_up(dashboard.page_size()),
        (KeyCode::PageDown, _) => dashboard.scroll_down(dashboard.page_size()),
        (KeyCode::Home, _) => dashboard.scroll_to_top(),
        (KeyCode::End, _) => dashboard.scroll_to_bottom(),

        // Enter: execute selected daemon action
        (KeyCode::Enter, KeyModifiers::NONE) => {
            let action = match dashboard.selected_action {
                0 => DaemonAction::Start,
                1 => DaemonAction::Stop,
                2 => DaemonAction::Restart,
                3 => DaemonAction::Enable,
                4 => DaemonAction::Disable,
                _ => return,
            };
            if app.daemon_action_pending {
                app.set_status(StatusKind::Warning, "A daemon action is already running");
            } else if app.send_command(BgCommand::DaemonAction(action)) {
                app.daemon_action_pending = true;
                app.set_status(StatusKind::Info, format!("{}…", action.progress()));
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
                if app.settings_screen.log_level_index + 1 < LOG_LEVELS.len() {
                    app.settings_screen.log_level_index += 1;
                }
            }
            KeyCode::Enter | KeyCode::Char(' ') => {
                // Apply the selected log level
                app.config.settings.log_level = LOG_LEVELS
                    .get(app.settings_screen.log_level_index)
                    .unwrap_or(&"info")
                    .to_string();
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
        KeyCode::PageUp => {
            app.settings_screen.scroll_desc_up();
        }
        KeyCode::PageDown => {
            app.settings_screen.scroll_desc_down();
        }
        KeyCode::Enter | KeyCode::Char(' ') => {
            let changed = app.settings_screen.toggle_current(&mut app.config.settings);
            if changed {
                app.mark_dirty();
            }
        }
        _ => {}
    }
}

/// Handle sinks screen input
// Sinks screen input handling - modal and list modes with many keybindings
fn handle_sinks_input(app: &mut App, key: KeyEvent) {
    match app.sinks_screen.mode {
        SinksMode::List => {
            match (key.code, key.modifiers) {
                (KeyCode::Up, KeyModifiers::NONE) => {
                    app.sinks_screen.select_previous(app.config.sinks.len());
                }
                (KeyCode::Down, KeyModifiers::NONE) => {
                    app.sinks_screen.select_next(app.config.sinks.len());
                }
                // Shift+Up: Move sink up in list
                (KeyCode::Up, KeyModifiers::SHIFT) => {
                    let idx = app.sinks_screen.selected;
                    if idx > 0 && idx < app.config.sinks.len() {
                        match app.config.move_sink(idx, idx - 1) {
                            Ok(()) => {
                                app.sinks_screen.selected = idx - 1;
                                app.mark_dirty();
                            }
                            Err(e) => {
                                app.set_status(StatusKind::Error, format!("Cannot move sink: {e}"));
                            }
                        }
                    }
                }
                // Shift+Down: Move sink down in list
                (KeyCode::Down, KeyModifiers::SHIFT) => {
                    let idx = app.sinks_screen.selected;
                    if idx + 1 < app.config.sinks.len() {
                        match app.config.move_sink(idx, idx + 1) {
                            Ok(()) => {
                                app.sinks_screen.selected = idx + 1;
                                app.mark_dirty();
                            }
                            Err(e) => {
                                app.set_status(StatusKind::Error, format!("Cannot move sink: {e}"));
                            }
                        }
                    }
                }
                (KeyCode::Char('a'), KeyModifiers::NONE) => {
                    app.sinks_screen.start_add();
                }
                (KeyCode::Char('e'), KeyModifiers::NONE) => {
                    app.sinks_screen.start_edit(&app.config.sinks);
                }
                (KeyCode::Char('x'), KeyModifiers::NONE) => {
                    // 'x' for delete (avoids conflict with Dashboard shortcut 'd')
                    if !app.config.sinks.is_empty() {
                        app.sinks_screen.start_delete();
                    }
                }
                (KeyCode::Char(' '), KeyModifiers::NONE) => {
                    // Toggle default status
                    let idx = app.sinks_screen.selected;
                    if idx < app.config.sinks.len() {
                        match app.config.set_default_sink(idx) {
                            Ok(()) => {
                                app.mark_dirty();
                            }
                            Err(e) => app.set_status(
                                StatusKind::Error,
                                format!("Cannot make it the fallback: {e}"),
                            ),
                        }
                    }
                }
                (KeyCode::Enter, KeyModifiers::NONE) if !app.config.sinks.is_empty() => {
                    app.sinks_screen.start_inspect();
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
                        // Refuses (with the referencing rules named) if any rule uses it
                        if let Err(e) = app.config.remove_sink(idx) {
                            app.set_status(StatusKind::Warning, format!("{e}"));
                            app.sinks_screen.cancel();
                            return;
                        }

                        app.sinks_screen.clamp_selection(app.config.sinks.len());

                        app.mark_dirty();
                        app.set_status(StatusKind::Success, "Sink deleted");
                    }
                    app.sinks_screen.cancel();
                }
                KeyCode::Esc => {
                    app.sinks_screen.cancel();
                }
                _ => {}
            }
        }
        SinksMode::SelectSink => match key.code {
            KeyCode::Up | KeyCode::Down => {
                // The lists are replaced by every poll: move by node name
                let names = selector_names(&app.active_sink_list, &app.profile_sink_list);
                app.sinks_screen
                    .move_selector(&names, key.code == KeyCode::Down);
            }
            KeyCode::Char(' ') | KeyCode::Enter => pick_selected_node(app),
            KeyCode::Esc => {
                // Cancel and return to editor
                app.sinks_screen.mode = SinksMode::AddEdit;
            }
            _ => {}
        },
        SinksMode::Inspect => match key.code {
            KeyCode::Enter | KeyCode::Esc => {
                app.sinks_screen.cancel();
            }
            _ => {}
        },
    }
}

/// Fill the sink editor from the node highlighted in the node selector.
///
/// The node is looked up by name in the current lists, so a list refresh while
/// the selector was open cannot substitute a different node.
fn pick_selected_node(app: &mut App) {
    let Some(node) = app.sinks_screen.selector_node.clone() else {
        app.set_status(StatusKind::Warning, "No node selected");
        return;
    };
    let found = app
        .active_sink_list
        .iter()
        .find(|s| s.name == node)
        .map(|s| s.description.clone())
        .or_else(|| {
            app.profile_sink_list
                .iter()
                .find(|s| s.predicted_name == node)
                .map(|s| s.description.clone())
        });
    let Some(description) = found else {
        app.set_status(
            StatusKind::Warning,
            format!("Node {node} is no longer available"),
        );
        return;
    };
    app.sinks_screen.editor.name.set_value(node);
    app.sinks_screen.editor.desc.set_value(description);
    app.sinks_screen.mode = SinksMode::AddEdit;
}

/// Answer to the "discard changes?" question: `Some(true)` discard, `Some(false)`
/// keep editing, `None` for any other key (ignored).
const fn discard_answer(key: KeyEvent) -> Option<bool> {
    match key.code {
        KeyCode::Char('y' | 'Y') => Some(true),
        KeyCode::Char('n' | 'N') | KeyCode::Esc => Some(false),
        _ => None,
    }
}

/// Handle sink editor input (add/edit modal)
fn handle_sink_editor_input(app: &mut App, key: KeyEvent) {
    if app.sinks_screen.confirm_discard {
        match discard_answer(key) {
            Some(true) => app.sinks_screen.cancel(),
            Some(false) => app.sinks_screen.confirm_discard = false,
            None => {}
        }
        return;
    }
    match key.code {
        // --- Navigation (Field Switching) ---
        KeyCode::Up => {
            // Move focus to previous field in the editor (arrow up behaves like Shift+Tab)
            app.sinks_screen.editor.prev_field();
        }
        KeyCode::Down => {
            // Move focus to next field in the editor (arrow down behaves like Tab)
            app.sinks_screen.editor.next_field();
        }
        KeyCode::Tab => {
            app.sinks_screen.editor.next_field();
        }
        KeyCode::BackTab => {
            app.sinks_screen.editor.prev_field();
        }

        // --- Field Actions (Space) ---
        KeyCode::Char(' ') => match app.sinks_screen.editor.focused_field {
            SinkField::Name => {
                // Open the node selector on the Name field
                let names = selector_names(&app.active_sink_list, &app.profile_sink_list);
                app.sinks_screen.open_selector(&names);
            }
            SinkField::Default => app.sinks_screen.editor.default.toggle(),
            SinkField::Desc | SinkField::Icon => {
                // Text fields: type a space
                if let Some(input) = app.sinks_screen.editor.focused_input_mut() {
                    input.handle_event(&Event::Key(key));
                }
            }
        },

        // --- Actions (Save/Cancel) ---
        KeyCode::Enter => {
            // Save the sink
            if app.sinks_screen.editor.name.value().is_empty()
                || app.sinks_screen.editor.desc.value().is_empty()
            {
                app.set_status(StatusKind::Warning, "Node and Description are required");
                return;
            }

            let new_sink = SinkConfig {
                name: app.sinks_screen.editor.name.value().to_string(),
                desc: app.sinks_screen.editor.desc.value().to_string(),
                icon: if app.sinks_screen.editor.icon.value().is_empty() {
                    None
                } else {
                    Some(app.sinks_screen.editor.icon.value().to_string())
                },
                default: app.sinks_screen.editor.default,
            };

            // The config methods keep the invariants: exactly one default (the first sink
            // added becomes it), unique names/descs, and rules following renames.
            let result = if let Some(idx) = app.sinks_screen.editing_index {
                app.config
                    .update_sink(idx, new_sink)
                    .map(|()| "Sink updated")
            } else {
                app.config.add_sink(new_sink).map(|()| "Sink added")
            };
            match result {
                Ok(msg) => app.set_status(StatusKind::Success, msg),
                Err(e) => {
                    // Keep the editor open so the user can fix the input
                    app.set_status(StatusKind::Warning, format!("{e}"));
                    return;
                }
            }

            app.mark_dirty();
            app.sinks_screen.cancel();
        }
        KeyCode::Esc => {
            if app.sinks_screen.editor.is_modified() {
                app.sinks_screen.confirm_discard = true;
            } else {
                app.sinks_screen.cancel();
            }
        }
        // --- Field Specific Input (the Default checkbox only responds to Space) ---
        _ => {
            if let Some(input) = app.sinks_screen.editor.focused_input_mut() {
                input.handle_event(&Event::Key(key));
            }
        }
    }
}

/// Handle rules screen input
fn handle_rules_input(app: &mut App, key: KeyEvent) {
    match app.rules_screen.mode {
        RulesMode::List => {
            match (key.code, key.modifiers) {
                (KeyCode::Up, KeyModifiers::NONE) => {
                    app.rules_screen.select_previous(app.config.rules.len());
                }
                (KeyCode::Down, KeyModifiers::NONE) => {
                    app.rules_screen.select_next(app.config.rules.len());
                }
                // Shift+Up: Move rule up in list
                (KeyCode::Up, KeyModifiers::SHIFT) => {
                    let idx = app.rules_screen.selected;
                    if idx > 0 && idx < app.config.rules.len() {
                        match app.config.move_rule(idx, idx - 1) {
                            Ok(()) => {
                                app.rules_screen.selected = idx - 1;
                                app.mark_dirty();
                            }
                            Err(e) => {
                                app.set_status(StatusKind::Error, format!("Cannot move rule: {e}"));
                            }
                        }
                    }
                }
                // Shift+Down: Move rule down in list
                (KeyCode::Down, KeyModifiers::SHIFT) => {
                    let idx = app.rules_screen.selected;
                    if idx + 1 < app.config.rules.len() {
                        match app.config.move_rule(idx, idx + 1) {
                            Ok(()) => {
                                app.rules_screen.selected = idx + 1;
                                app.mark_dirty();
                            }
                            Err(e) => {
                                app.set_status(StatusKind::Error, format!("Cannot move rule: {e}"));
                            }
                        }
                    }
                }
                (KeyCode::Char('a'), KeyModifiers::NONE) => {
                    // Rules require sinks to reference
                    if app.config.sinks.is_empty() {
                        app.set_status(StatusKind::Warning, "Add a sink first (2 Sinks → a)");
                        return;
                    }
                    app.rules_screen.start_add();
                }
                (KeyCode::Char('e'), KeyModifiers::NONE) => {
                    // Rules require sinks to reference
                    if app.config.sinks.is_empty() {
                        app.set_status(StatusKind::Warning, "Add a sink first (2 Sinks → a)");
                        return;
                    }
                    app.rules_screen.start_edit(&app.config.rules);
                }
                (KeyCode::Char('x'), KeyModifiers::NONE) => {
                    // 'x' for delete (avoids conflict with Dashboard shortcut 'd')
                    if !app.config.rules.is_empty() {
                        app.rules_screen.start_delete();
                    }
                }
                (KeyCode::Enter, KeyModifiers::NONE) if !app.config.rules.is_empty() => {
                    app.rules_screen.start_inspect();
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
                    if let Err(e) = app.config.remove_rule(idx) {
                        app.set_status(StatusKind::Warning, format!("{e}"));
                        app.rules_screen.cancel();
                        return;
                    }

                    app.rules_screen.clamp_selection(app.config.rules.len());

                    app.mark_dirty();
                    app.set_status(StatusKind::Success, "Rule deleted");
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
                // Use saturating_sub to prevent underflow when sinks is empty
                // (shouldn't happen due to guards, but defensive)
                let max_idx = app.config.sinks.len().saturating_sub(1);
                if app.rules_screen.editor.sink_dropdown_index < max_idx {
                    app.rules_screen.editor.sink_dropdown_index += 1;
                }
            }
            KeyCode::Char(' ') | KeyCode::Enter => {
                let idx = app.rules_screen.editor.sink_dropdown_index;
                if let Some(sink) = app.config.sinks.get(idx) {
                    app.rules_screen.editor.sink_ref.clone_from(&sink.desc);
                }
                app.rules_screen.mode = RulesMode::AddEdit;
            }
            KeyCode::Esc => {
                app.rules_screen.mode = RulesMode::AddEdit;
            }
            _ => {}
        },
        RulesMode::Inspect => match key.code {
            KeyCode::Enter | KeyCode::Esc => {
                app.rules_screen.cancel();
            }
            _ => {}
        },
    }
}

/// Handle rule editor input (add/edit modal)
// Match arms handle conceptually different field types despite similar-looking actions
fn handle_rule_editor_input(app: &mut App, key: KeyEvent) {
    if app.rules_screen.confirm_discard {
        match discard_answer(key) {
            Some(true) => app.rules_screen.cancel(),
            Some(false) => app.rules_screen.confirm_discard = false,
            None => {}
        }
        return;
    }
    match key.code {
        // --- Navigation (Field Switching) ---
        KeyCode::Up | KeyCode::BackTab => app.rules_screen.editor.prev_field(),
        KeyCode::Down | KeyCode::Tab => app.rules_screen.editor.next_field(),

        // --- Field Actions (Space) ---
        KeyCode::Char(' ') => match app.rules_screen.editor.focused_field {
            RuleField::Sink => app.rules_screen.open_sink_selector(&app.config.sinks),
            RuleField::Notify => {
                // Cycle notify setting: None -> Some(true) -> Some(false) -> None
                app.rules_screen.editor.notify = match app.rules_screen.editor.notify {
                    None => Some(true),
                    Some(true) => Some(false),
                    Some(false) => None,
                };
            }
            RuleField::AppId | RuleField::Title | RuleField::Desc => {
                // Text fields: type a space
                if let Some(input) = app.rules_screen.editor.focused_input_mut() {
                    input.handle_event(&Event::Key(key));
                }
            }
        },

        // --- Actions (Save/Cancel) ---
        KeyCode::Enter => {
            // Save the rule
            if app.rules_screen.editor.app_id_pattern.value().is_empty() {
                app.set_status(StatusKind::Warning, "App ID pattern is required");
                return;
            }

            if app.rules_screen.editor.sink_ref.is_empty() {
                app.set_status(StatusKind::Warning, "Target sink is required");
                return;
            }

            // Validate regexes with the same builder config loading uses
            let app_id_regex =
                match compile_rule_regex(app.rules_screen.editor.app_id_pattern.value()) {
                    Ok(r) => r,
                    Err(e) => {
                        app.set_status(StatusKind::Warning, format!("Invalid app_id regex: {e}"));
                        return;
                    }
                };

            let title_regex = if app.rules_screen.editor.title_pattern.value().is_empty() {
                None
            } else {
                match compile_rule_regex(app.rules_screen.editor.title_pattern.value()) {
                    Ok(r) => Some(r),
                    Err(e) => {
                        app.set_status(StatusKind::Warning, format!("Invalid title regex: {e}"));
                        return;
                    }
                }
            };

            let new_rule = Rule {
                app_id_regex,
                title_regex,
                sink_ref: app.rules_screen.editor.sink_ref.clone(),
                desc: if app.rules_screen.editor.desc.value().is_empty() {
                    None
                } else {
                    Some(app.rules_screen.editor.desc.value().to_string())
                },
                notify: app.rules_screen.editor.notify,
                app_id_pattern: app.rules_screen.editor.app_id_pattern.value().to_string(),
                title_pattern: if app.rules_screen.editor.title_pattern.value().is_empty() {
                    None
                } else {
                    Some(app.rules_screen.editor.title_pattern.value().to_string())
                },
            };

            // Stored with a `desc` sink reference; refused if it would make the config invalid
            let result = if let Some(idx) = app.rules_screen.editing_index {
                app.config
                    .update_rule(idx, new_rule)
                    .map(|()| "Rule updated")
            } else {
                app.config.add_rule(new_rule).map(|()| "Rule added")
            };
            match result {
                Ok(msg) => app.set_status(StatusKind::Success, msg),
                Err(e) => {
                    // Keep the editor open so the user can fix the input
                    app.set_status(StatusKind::Warning, format!("{e}"));
                    return;
                }
            }

            app.mark_dirty();
            app.rules_screen.cancel();
        }
        KeyCode::Esc => {
            if app.rules_screen.editor.is_modified() {
                app.rules_screen.confirm_discard = true;
            } else {
                app.rules_screen.cancel();
            }
        }
        // --- Field Specific Input (sink and notify only respond to Space) ---
        // (the live preview follows the pattern text by itself; see `App::refresh_preview`)
        _ => {
            if let Some(input) = app.rules_screen.editor.focused_input_mut() {
                input.handle_event(&Event::Key(key));
            }
        }
    }
}
