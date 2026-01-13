//! Input handling for keyboard events

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use tui_input::backend::crossterm::EventHandler;

use super::app::{App, DaemonAction, Screen};
use super::screens::rules::RulesMode;
use super::screens::sinks::SinksMode;
use crate::config::{Rule, SinkConfig};
use regex::Regex;

/// Handle a single input event and update app state
///
/// Sets `app.dirty = true` to trigger a redraw. Delegates to `handle_key_event` for
/// keyboard input and handles terminal resize events.
pub(crate) fn handle_event(app: &mut App, event: &Event) {
    if let Event::Key(key_event) = event {
        handle_key_event(app, *key_event);
        app.dirty = true;
    } else if let Event::Resize(_, _) = event {
        // Ratatui handles resize automatically, but mark dirty so UI redraws at new size
        app.dirty = true;
    }
}

#[cfg(test)]
pub(crate) fn simulate_key_event(app: &mut crate::tui::app::App, key: crossterm::event::KeyEvent) {
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
                if let Some(tx) = &app.bg_cmd_tx {
                    let _ = tx.try_send(crate::tui::app::BgCommand::SaveConfig(app.config.clone()));
                    // Don't clear config_dirty - wait for ConfigSaved result
                    app.set_status("Saving configuration...".to_string());
                } else {
                    // Fallback to blocking save if background worker not available
                    if let Err(e) = app.save_config() {
                        app.set_status(format!("Failed to save config: {e}"));
                    }
                }
            }
            return;
        }

        _ => {}
    }

    // Help overlay input (blocks everything else)
    if app.show_help {
        const HELP_PAGE_SIZE: usize = 15;
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
                let new_selected = current_selected.saturating_sub(HELP_PAGE_SIZE);
                app.help_scroll_state.select(Some(new_selected));

                let current_offset = app.help_scroll_state.offset();
                let new_offset = current_offset.saturating_sub(HELP_PAGE_SIZE);
                *app.help_scroll_state.offset_mut() = new_offset;
            }
            KeyCode::PageDown => {
                let row_count = crate::tui::screens::help::get_help_row_count(
                    app.current_screen,
                    &app.help_collapsed_sections,
                );
                let current_selected = app.help_scroll_state.selected().unwrap_or(0);
                let new_selected = current_selected
                    .saturating_add(HELP_PAGE_SIZE)
                    .min(row_count.saturating_sub(1));
                app.help_scroll_state.select(Some(new_selected));

                let max_offset = crate::tui::screens::help::get_help_max_offset(
                    app.current_screen,
                    &app.help_collapsed_sections,
                    app.help_viewport_height,
                );
                let current_offset = app.help_scroll_state.offset();
                let new_offset = current_offset.saturating_add(HELP_PAGE_SIZE).min(max_offset);
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
            KeyCode::Esc => {
                // Priority: quit confirmation > modal
                if app.confirm_quit {
                    app.cancel_quit();
                } else {
                    // Let screen handler close modal
                    handle_screen_specific_input(app, key);
                }
                return;
            }
            // F1: Toggle help overlay (global)
            KeyCode::F(1) => {
                app.show_help = true;
                app.reset_help_scroll();
                return;
            }
            // Help toggle is handled in global section below if no modal,
            // but if modal is active, we might want to allow it?
            // "Global shortcuts" say ? is global.
            // Let's allow ? to open help even over a modal, but ONLY if not in input
            KeyCode::Char('?') if !app.is_input_focused() => {
                app.show_help = true;
                app.reset_help_scroll();
                return;
            }
            _ => {
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

        // Escape: cancel quit confirmation or clear status message
        (KeyCode::Esc, KeyModifiers::NONE) => {
            if app.confirm_quit {
                app.cancel_quit();
            } else {
                app.clear_status();
            }
        }

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
    use super::screens::DashboardView;
    use crossterm::event::KeyModifiers;

    match (key.code, key.modifiers) {
        // Toggle between Logs and Windows view (Phase 9B)
        (KeyCode::Char('w'), KeyModifiers::NONE) => {
            app.dashboard_screen.toggle_view();
        }

        // Left/Right for horizontal daemon action navigation (Phase 9A)
        (KeyCode::Left, KeyModifiers::NONE) => {
            app.dashboard_screen.select_previous();
        }
        (KeyCode::Right, KeyModifiers::NONE) => {
            app.dashboard_screen.select_next();
        }

        // Up/Down for scrolling (view-aware)
        (KeyCode::Up, KeyModifiers::NONE) => match app.dashboard_screen.current_view {
            DashboardView::Logs => {
                let total = app.daemon_log_lines.len();
                let visible = 20;
                app.dashboard_screen.scroll_logs_up(total, visible);
            }
            DashboardView::Windows => {
                // Single-line scroll not implemented for windows (use PageUp/Down)
            }
        },
        (KeyCode::Down, KeyModifiers::NONE) => match app.dashboard_screen.current_view {
            DashboardView::Logs => {
                app.dashboard_screen.scroll_logs_down();
            }
            DashboardView::Windows => {
                // Single-line scroll not implemented for windows (use PageUp/Down)
            }
        },

        // PageUp/PageDown for scrolling (view-aware)
        (KeyCode::PageUp, _) => match app.dashboard_screen.current_view {
            DashboardView::Logs => {
                let total = app.daemon_log_lines.len();
                let visible = 10;
                app.dashboard_screen.scroll_logs_page_up(total, visible);
            }
            DashboardView::Windows => {
                let page_size = 5;
                let total = app.windows.len();
                app.dashboard_screen
                    .scroll_windows_page_up(page_size, total);
            }
        },
        (KeyCode::PageDown, _) => match app.dashboard_screen.current_view {
            DashboardView::Logs => {
                let visible = 10;
                app.dashboard_screen.scroll_logs_page_down(visible);
            }
            DashboardView::Windows => {
                let page_size = 5;
                app.dashboard_screen.scroll_windows_page_down(page_size);
            }
        },

        // Home: jump to top/bottom (view-aware)
        (KeyCode::Home, _) => match app.dashboard_screen.current_view {
            DashboardView::Logs => {
                app.dashboard_screen.scroll_logs_to_bottom(); // Reset to latest
            }
            DashboardView::Windows => {
                app.dashboard_screen.scroll_windows_to_top();
            }
        },

        // Enter: execute selected daemon action
        (KeyCode::Enter, KeyModifiers::NONE) => {
            // Send the daemon action to the background worker if available
            let action = match app.dashboard_screen.selected_action {
                0 => DaemonAction::Start,
                1 => DaemonAction::Stop,
                2 => DaemonAction::Restart,
                3 => DaemonAction::Enable,
                4 => DaemonAction::Disable,
                _ => return,
            };
            if let Some(tx) = &app.bg_cmd_tx {
                let _ = tx.try_send(crate::tui::app::BgCommand::DaemonAction(action));
                app.daemon_action_pending = true;
                app.set_status("Daemon action requested".to_string());
            } else {
                // No background worker available to handle daemon actions; show feedback
                app.set_status("Daemon action requested (no background worker)".to_string());
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
        KeyCode::PageUp => {
            app.settings_screen.scroll_desc_up();
        }
        KeyCode::PageDown => {
            app.settings_screen.scroll_desc_down();
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
                        app.config.sinks.swap(idx, idx - 1);
                        app.sinks_screen.selected = idx - 1;
                        app.sinks_screen.update_display_descs(&app.config.sinks);
                        app.mark_dirty();
                    }
                }
                // Shift+Down: Move sink down in list
                (KeyCode::Down, KeyModifiers::SHIFT) => {
                    let idx = app.sinks_screen.selected;
                    if idx + 1 < app.config.sinks.len() {
                        app.config.sinks.swap(idx, idx + 1);
                        app.sinks_screen.selected = idx + 1;
                        app.sinks_screen.update_display_descs(&app.config.sinks);
                        app.mark_dirty();
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
                        // Clear all defaults first
                        for sink in &mut app.config.sinks {
                            sink.default = false;
                        }
                        // Set selected as default
                        app.config.sinks[idx].default = true;
                        // Update caches
                        app.sinks_screen.update_display_descs(&app.config.sinks);
                        app.mark_dirty();
                    }
                }
                (KeyCode::Enter, KeyModifiers::NONE) => {
                    if !app.config.sinks.is_empty() {
                        app.sinks_screen.start_inspect();
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

                        // Update cached display descriptions
                        app.sinks_screen.update_display_descs(&app.config.sinks);

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
        SinksMode::SelectSink => {
            // Total number of selectable items (active + profile sinks, excluding headers)
            let total_items = app.active_sink_list.len() + app.profile_sink_list.len();

            match key.code {
                KeyCode::Up => {
                    if total_items > 0 && app.sinks_screen.sink_selector_index > 0 {
                        app.sinks_screen.sink_selector_index -= 1;
                    }
                }
                KeyCode::Down => {
                    if total_items > 0 && app.sinks_screen.sink_selector_index < total_items - 1 {
                        app.sinks_screen.sink_selector_index += 1;
                    }
                }
                KeyCode::Enter => {
                    // Select the chosen sink and populate editor fields
                    let idx = app.sinks_screen.sink_selector_index;

                    if idx < app.active_sink_list.len() {
                        // Active sink selected
                        let sink = &app.active_sink_list[idx];
                        app.sinks_screen.editor.name.set_value(sink.name.clone());
                        app.sinks_screen
                            .editor
                            .desc
                            .set_value(sink.description.clone());
                    } else {
                        // Profile sink selected
                        let profile_idx = idx - app.active_sink_list.len();
                        if profile_idx < app.profile_sink_list.len() {
                            let sink = &app.profile_sink_list[profile_idx];
                            app.sinks_screen
                                .editor
                                .name
                                .set_value(sink.predicted_name.clone());
                            app.sinks_screen
                                .editor
                                .desc
                                .set_value(sink.description.clone());
                        }
                    }

                    // Return to editor mode
                    app.sinks_screen.mode = SinksMode::AddEdit;
                }
                KeyCode::Esc => {
                    // Cancel and return to editor
                    app.sinks_screen.mode = SinksMode::AddEdit;
                }
                _ => {}
            }
        }
        SinksMode::Inspect => match key.code {
            KeyCode::Enter | KeyCode::Esc => {
                app.sinks_screen.cancel();
            }
            _ => {}
        },
    }
}

/// Handle sink editor input (add/edit modal)
fn handle_sink_editor_input(app: &mut App, key: KeyEvent) {
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

        // --- Actions (Save/Cancel) ---
        KeyCode::Enter => {
            // If on name field, open sink selector; otherwise save
            if app.sinks_screen.editor.focused_field == 0 {
                // Open sink selector modal
                app.sinks_screen.mode = SinksMode::SelectSink;
                app.sinks_screen.sink_selector_index = 0; // Reset to first item
                return;
            }

            // Save the sink
            if app.sinks_screen.editor.name.value().is_empty()
                || app.sinks_screen.editor.desc.value().is_empty()
            {
                app.set_status("Name and Description are required".to_string());
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

            // Update cached display descriptions
            app.sinks_screen.update_display_descs(&app.config.sinks);

            app.mark_dirty();
            app.sinks_screen.cancel();
        }
        KeyCode::Esc => {
            app.sinks_screen.cancel();
        }
        // --- Field Specific Input ---
        _ => {
            let event = Event::Key(key);
            match app.sinks_screen.editor.focused_field {
                0 => {
                    app.sinks_screen.editor.name.input.handle_event(&event);
                }
                1 => {
                    app.sinks_screen.editor.desc.input.handle_event(&event);
                }
                2 => {
                    app.sinks_screen.editor.icon.input.handle_event(&event);
                }
                3 => {
                    // Toggle default checkbox with space
                    if key.code == KeyCode::Char(' ') {
                        app.sinks_screen.editor.default = !app.sinks_screen.editor.default;
                    }
                }
                _ => {}
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
                        app.config.rules.swap(idx, idx - 1);
                        app.rules_screen.selected = idx - 1;
                        app.mark_dirty();
                    }
                }
                // Shift+Down: Move rule down in list
                (KeyCode::Down, KeyModifiers::SHIFT) => {
                    let idx = app.rules_screen.selected;
                    if idx + 1 < app.config.rules.len() {
                        app.config.rules.swap(idx, idx + 1);
                        app.rules_screen.selected = idx + 1;
                        app.mark_dirty();
                    }
                }
                (KeyCode::Char('a'), KeyModifiers::NONE) => {
                    app.rules_screen.start_add();
                }
                (KeyCode::Char('e'), KeyModifiers::NONE) => {
                    app.rules_screen.start_edit(&app.config.rules);
                }
                (KeyCode::Char('x'), KeyModifiers::NONE) => {
                    // 'x' for delete (avoids conflict with Dashboard shortcut 'd')
                    if !app.config.rules.is_empty() {
                        app.rules_screen.start_delete();
                    }
                }
                (KeyCode::Enter, KeyModifiers::NONE) => {
                    if !app.config.rules.is_empty() {
                        app.rules_screen.start_inspect();
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
    match key.code {
        // --- Navigation (Field Switching) ---
        KeyCode::Up | KeyCode::BackTab => app.rules_screen.editor.prev_field(),
        KeyCode::Down | KeyCode::Tab => app.rules_screen.editor.next_field(),

        // --- Actions (Save/Cancel) ---
        KeyCode::Enter => {
            // If on sink field, open selector
            if app.rules_screen.editor.focused_field == 2 {
                app.rules_screen.open_sink_selector();
                return;
            }

            // Otherwise, save the rule
            if app.rules_screen.editor.app_id_pattern.value().is_empty() {
                app.set_status("App ID pattern is required".to_string());
                return;
            }

            if app.rules_screen.editor.sink_ref.is_empty() {
                app.set_status("Target sink is required".to_string());
                return;
            }

            // Validate regexes
            // Validate regexes: prefer using compiled caches when available
            let app_id_regex = match app
                .rules_screen
                .editor
                .compiled_app_id
                .as_ref()
                .map(std::convert::AsRef::as_ref)
            {
                Some(r) => r.clone(),
                None => match Regex::new(app.rules_screen.editor.app_id_pattern.value()) {
                    Ok(r) => r,
                    Err(e) => {
                        app.set_status(format!("Invalid app_id regex: {e}"));
                        return;
                    }
                },
            };

            let title_regex = if app.rules_screen.editor.title_pattern.value().is_empty() {
                None
            } else if let Some(r) = app
                .rules_screen
                .editor
                .compiled_title
                .as_ref()
                .map(std::convert::AsRef::as_ref)
            {
                Some(r.clone())
            } else {
                match Regex::new(app.rules_screen.editor.title_pattern.value()) {
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
        // --- Field Specific Input ---
        _ => {
            // Forward other keys to inputs
            let event = Event::Key(key);
            let mut changed = false;

            match app.rules_screen.editor.focused_field {
                0 => {
                    // app_id
                    if app
                        .rules_screen
                        .editor
                        .app_id_pattern
                        .input
                        .handle_event(&event)
                        .is_some()
                    {
                        changed = true;
                    }
                }
                1 => {
                    // title
                    if app
                        .rules_screen
                        .editor
                        .title_pattern
                        .input
                        .handle_event(&event)
                        .is_some()
                    {
                        changed = true;
                    }
                }
                3 => {
                    // desc
                    app.rules_screen.editor.desc.input.handle_event(&event);
                }
                4 => {
                    // notify
                    if key.code == KeyCode::Char(' ') {
                        app.rules_screen.editor.notify = match app.rules_screen.editor.notify {
                            None => Some(true),
                            Some(true) => Some(false),
                            Some(false) => None,
                        };
                    }
                }
                _ => {}
            }

            // Trigger preview if patterns changed
            if changed {
                app.rules_screen.editor.ensure_compiled();

                if let Some(tx) = &app.preview_in_tx {
                    let compiled_app = app.rules_screen.editor.compiled_app_id.clone();
                    let compiled_title = app.rules_screen.editor.compiled_title.clone();

                    let app_pattern = app.rules_screen.editor.app_id_pattern.value().to_string();
                    let title_pattern = if app.rules_screen.editor.title_pattern.value().is_empty()
                    {
                        None
                    } else {
                        Some(app.rules_screen.editor.title_pattern.value().to_string())
                    };

                    let _ = tx.send((app_pattern, title_pattern, compiled_app, compiled_title));
                }
            }
        }
    }
}
