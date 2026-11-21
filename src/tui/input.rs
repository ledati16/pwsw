//! Input handling for keyboard events

use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use tui_input::Input;
use tui_input::backend::crossterm::EventHandler;

use super::app::{App, BgCommand, DaemonAction, Screen, StatusKind};
use super::preview::first_matching_rule;
use super::screens::dashboard::DashboardView;
use super::screens::list::{FormEditor, ListMode, ListScreen};
use super::screens::rules::RuleField;
use super::screens::settings::LOG_LEVELS;
use super::screens::sinks::{SinkField, selector_names};

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
        Screen::Sinks => app.sinks_screen.mode != ListMode::List,
        Screen::Rules => app.rules_screen.mode != ListMode::List,
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
        let help = &mut app.help;
        match key.code {
            KeyCode::Esc | KeyCode::F(1) | KeyCode::Char('?' | 'q') => app.show_help = false,
            KeyCode::Up => help.up(1),
            KeyCode::Down => help.down(1),
            KeyCode::PageUp => help.up(help.page()),
            KeyCode::PageDown => help.down(help.page()),
            KeyCode::Home => help.offset = 0,
            KeyCode::End => help.scroll_to_end(),
            _ => {}
        }
        return;
    }

    // If a modal is active (other than help), pass most keys to screen-specific handlers
    if is_modal_active(app) {
        match key.code {
            // F1 opens help over a modal; so does ?, unless it is typed into a field
            KeyCode::F(1) => app.open_help(),
            KeyCode::Char('?') if !app.is_input_focused() => app.open_help(),
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

        // F1, or ? while no text field has focus: help overlay
        (KeyCode::F(1), _) => app.open_help(),
        (KeyCode::Char('?'), KeyModifiers::NONE | KeyModifiers::SHIFT)
            if !app.is_input_focused() =>
        {
            app.open_help();
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
        (KeyCode::PageUp, _) => dashboard.page_up(),
        (KeyCode::PageDown, _) => dashboard.page_down(),
        (KeyCode::Home, _) => dashboard.scroll_to_top(),
        (KeyCode::End, _) => dashboard.scroll_to_bottom(),

        // n in the window list: a rule for the selected window
        (KeyCode::Char('n'), KeyModifiers::NONE)
            if dashboard.current_view == DashboardView::Windows =>
        {
            new_rule_from_selected_window(app);
        }

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

/// Open the Rules screen's form for the window selected in the Dashboard's
/// window list: the rule that already matches it, or a new rule pre-filled
/// from it. Says why in the status line when there is no window to use.
fn new_rule_from_selected_window(app: &mut App) {
    if !app.daemon_running {
        app.set_status(
            StatusKind::Warning,
            "Daemon not running: start it to list windows",
        );
        return;
    }
    let Some(window) = app.dashboard_screen.selected_window(&app.windows).cloned() else {
        app.set_status(StatusKind::Warning, "No windows to make a rule for");
        return;
    };
    if app.config.sinks.is_empty() {
        app.set_status(StatusKind::Warning, "Add a sink first (2 Sinks → a)");
        return;
    }
    app.goto_screen(Screen::Rules);
    // The rule the daemon would apply, found the way the live preview matches
    if let Some(index) = first_matching_rule(&app.config.rules, &window) {
        app.rules_screen
            .start_edit_for_window(index, &app.config.rules, &window);
        app.set_status(
            StatusKind::Info,
            format!("This window already matches rule {}", index + 1),
        );
    } else {
        app.rules_screen
            .start_add_for_window(&window, &app.config.sinks);
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

/// What a key on the sink or rule list asks of the config
enum ListKey {
    /// Nothing more to do
    Done,
    /// Move item `from` to `to`
    Move { from: usize, to: usize },
    /// Remove item `index` (the delete dialog was confirmed)
    Delete(usize),
    /// Not a shared key: the screen handles it
    Unhandled,
}

/// The keys the sink and rule screens share: list navigation, add/edit/delete,
/// the delete and details dialogs.
fn list_keys<E: FormEditor>(
    screen: &mut ListScreen<E>,
    items: &[E::Item],
    key: KeyEvent,
) -> ListKey {
    let len = items.len();
    match (screen.mode, key.code, key.modifiers) {
        (ListMode::List, KeyCode::Up, KeyModifiers::NONE) => screen.select_previous(),
        (ListMode::List, KeyCode::Down, KeyModifiers::NONE) => screen.select_next(len),
        (ListMode::List, KeyCode::Up, KeyModifiers::SHIFT) if screen.selected > 0 => {
            if screen.selected < len {
                return ListKey::Move {
                    from: screen.selected,
                    to: screen.selected - 1,
                };
            }
        }
        (ListMode::List, KeyCode::Down, KeyModifiers::SHIFT) if screen.selected + 1 < len => {
            return ListKey::Move {
                from: screen.selected,
                to: screen.selected + 1,
            };
        }
        (ListMode::List, KeyCode::Char('a'), KeyModifiers::NONE) => screen.start_add(),
        (ListMode::List, KeyCode::Char('e'), KeyModifiers::NONE) => screen.start_edit(items),
        // 'x' for delete (avoids conflict with Dashboard shortcut 'd')
        (ListMode::List, KeyCode::Char('x'), KeyModifiers::NONE) if len > 0 => {
            screen.start_delete();
        }
        (ListMode::List, KeyCode::Enter, KeyModifiers::NONE) if len > 0 => screen.start_inspect(),
        (ListMode::Delete, KeyCode::Enter, _) => {
            screen.cancel();
            if screen.selected < len {
                return ListKey::Delete(screen.selected);
            }
        }
        (ListMode::Delete, KeyCode::Esc, _)
        | (ListMode::Inspect, KeyCode::Enter | KeyCode::Esc, _) => screen.cancel(),
        _ => return ListKey::Unhandled,
    }
    ListKey::Done
}

/// Report the result of moving the selected sink or rule `what` to `to`
fn moved(app: &mut App, result: color_eyre::Result<()>, what: &str, to: usize) {
    match result {
        Ok(()) => {
            match app.current_screen {
                Screen::Sinks => app.sinks_screen.selected = to,
                Screen::Rules => app.rules_screen.selected = to,
                Screen::Dashboard | Screen::Settings => {}
            }
            app.mark_dirty();
        }
        Err(e) => app.set_status(StatusKind::Error, format!("Cannot move {what}: {e}")),
    }
}

/// Report the result of a config change made from a form or dialog: `done`
/// on success, the reason (keeping the form open) on failure
fn changed(app: &mut App, result: color_eyre::Result<()>, done: &str) -> bool {
    match result {
        Ok(()) => {
            app.mark_dirty();
            app.set_status(StatusKind::Success, done);
            true
        }
        Err(e) => {
            app.set_status(StatusKind::Warning, format!("{e}"));
            false
        }
    }
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

/// The keys every form handles alike: the discard question, switching fields,
/// Esc and typing into the focused field. Space and Enter are left to the
/// form's own handler (returns `false`).
fn form_keys<E: FormEditor>(screen: &mut ListScreen<E>, key: KeyEvent) -> bool {
    if screen.confirm_discard {
        match discard_answer(key) {
            Some(true) => screen.cancel(),
            Some(false) => screen.confirm_discard = false,
            None => {}
        }
        return true;
    }
    match key.code {
        KeyCode::Up | KeyCode::BackTab => screen.editor.prev_field(),
        KeyCode::Down | KeyCode::Tab => screen.editor.next_field(),
        KeyCode::Esc => screen.close_editor(),
        KeyCode::Char(' ') | KeyCode::Enter => return false,
        // Selectors and toggles only respond to Space
        _ => type_key(&mut screen.editor, key),
    }
    true
}

/// Type `key` into the focused text field of `editor`, if any
fn type_key<E: FormEditor>(editor: &mut E, key: KeyEvent) {
    if let Some(input) = editor.focused_input_mut() {
        input.handle_event(&Event::Key(key));
    }
}

/// Handle sinks screen input
fn handle_sinks_input(app: &mut App, key: KeyEvent) {
    match list_keys(&mut app.sinks_screen, &app.config.sinks, key) {
        ListKey::Done => {}
        ListKey::Move { from, to } => {
            let result = app.config.move_sink(from, to);
            moved(app, result, "sink", to);
        }
        ListKey::Delete(index) => {
            // Refuses (with the referencing rules named) if any rule uses it
            let result = app.config.remove_sink(index);
            app.sinks_screen.clamp_selection(app.config.sinks.len());
            changed(app, result, "Sink deleted");
        }
        ListKey::Unhandled => match app.sinks_screen.mode {
            // Space: make the selected sink the fallback
            ListMode::List if key.code == KeyCode::Char(' ') && key.modifiers.is_empty() => {
                let idx = app.sinks_screen.selected;
                if idx < app.config.sinks.len() {
                    match app.config.set_default_sink(idx) {
                        Ok(()) => app.mark_dirty(),
                        Err(e) => app.set_status(
                            StatusKind::Error,
                            format!("Cannot make it the fallback: {e}"),
                        ),
                    }
                }
            }
            ListMode::AddEdit => handle_sink_editor_input(app, key),
            ListMode::SelectSink => match key.code {
                KeyCode::Up | KeyCode::Down => {
                    // The lists are replaced by every poll: move by node name
                    let names = selector_names(&app.active_sink_list, &app.profile_sink_list);
                    app.sinks_screen
                        .move_selector(&names, key.code == KeyCode::Down);
                }
                KeyCode::Char(' ') | KeyCode::Enter => pick_selected_node(app),
                KeyCode::Esc => app.sinks_screen.mode = ListMode::AddEdit,
                _ => {}
            },
            _ => {}
        },
    }
}

/// Fill the sink editor from the node highlighted in the node selector.
///
/// The node is looked up by name in the current lists, so a list refresh while
/// the selector was open cannot substitute a different node.
fn pick_selected_node(app: &mut App) {
    let Some(node) = app.sinks_screen.editor.selector_node.clone() else {
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
    app.sinks_screen.editor.name = Input::new(node);
    app.sinks_screen.editor.desc = Input::new(description);
    app.sinks_screen.mode = ListMode::AddEdit;
}

/// Handle sink editor input (add/edit modal)
fn handle_sink_editor_input(app: &mut App, key: KeyEvent) {
    if form_keys(&mut app.sinks_screen, key) {
        return;
    }
    let editor = &mut app.sinks_screen.editor;
    match (key.code, editor.focused_field) {
        (KeyCode::Char(' '), SinkField::Name) => {
            let names = selector_names(&app.active_sink_list, &app.profile_sink_list);
            app.sinks_screen.open_selector(&names);
        }
        (KeyCode::Char(' '), SinkField::Default) => editor.default.toggle(),
        (KeyCode::Char(' '), _) => type_key(editor, key),
        (KeyCode::Enter, _) => {
            let sink = match editor.to_sink() {
                Ok(sink) => sink,
                Err(e) => return app.set_status(StatusKind::Warning, e),
            };
            // The config methods keep the invariants: exactly one default (the first
            // sink added becomes it), unique names/descs, and rules following renames.
            let (result, done) = match app.sinks_screen.editing_index {
                Some(idx) => (app.config.update_sink(idx, sink), "Sink updated"),
                None => (app.config.add_sink(sink), "Sink added"),
            };
            if changed(app, result, done) {
                app.sinks_screen.cancel();
            }
        }
        _ => {}
    }
}

/// Handle rules screen input
fn handle_rules_input(app: &mut App, key: KeyEvent) {
    // Rules need a sink to send windows to
    if app.rules_screen.mode == ListMode::List
        && key.modifiers.is_empty()
        && matches!(key.code, KeyCode::Char('a' | 'e'))
        && app.config.sinks.is_empty()
    {
        app.set_status(StatusKind::Warning, "Add a sink first (2 Sinks → a)");
        return;
    }
    match list_keys(&mut app.rules_screen, &app.config.rules, key) {
        ListKey::Done => {}
        ListKey::Move { from, to } => {
            let result = app.config.move_rule(from, to);
            moved(app, result, "rule", to);
        }
        ListKey::Delete(index) => {
            let result = app.config.remove_rule(index);
            app.rules_screen.clamp_selection(app.config.rules.len());
            changed(app, result, "Rule deleted");
        }
        ListKey::Unhandled => match app.rules_screen.mode {
            ListMode::AddEdit => handle_rule_editor_input(app, key),
            ListMode::SelectSink => {
                let editor = &mut app.rules_screen.editor;
                match key.code {
                    KeyCode::Up => {
                        editor.sink_dropdown_index = editor.sink_dropdown_index.saturating_sub(1);
                    }
                    KeyCode::Down => {
                        if editor.sink_dropdown_index + 1 < app.config.sinks.len() {
                            editor.sink_dropdown_index += 1;
                        }
                    }
                    KeyCode::Char(' ') | KeyCode::Enter => {
                        if let Some(sink) = app.config.sinks.get(editor.sink_dropdown_index) {
                            editor.sink_ref.clone_from(&sink.desc);
                        }
                        app.rules_screen.mode = ListMode::AddEdit;
                    }
                    KeyCode::Esc => app.rules_screen.mode = ListMode::AddEdit,
                    _ => {}
                }
            }
            _ => {}
        },
    }
}

/// Handle rule editor input (add/edit modal)
fn handle_rule_editor_input(app: &mut App, key: KeyEvent) {
    if form_keys(&mut app.rules_screen, key) {
        return;
    }
    let editor = &mut app.rules_screen.editor;
    match (key.code, editor.focused_field) {
        (KeyCode::Char(' '), RuleField::Sink) => {
            app.rules_screen.open_sink_selector(&app.config.sinks);
        }
        (KeyCode::Char(' '), RuleField::Notify) => editor.cycle_notify(),
        (KeyCode::Char(' '), _) => type_key(editor, key),
        (KeyCode::Enter, _) => {
            let rule = match editor.to_rule() {
                Ok(rule) => rule,
                Err(e) => return app.set_status(StatusKind::Warning, e),
            };
            // Stored with a `desc` sink reference; refused if it would make the config invalid
            let (result, done) = match app.rules_screen.editing_index {
                Some(idx) => (app.config.update_rule(idx, rule), "Rule updated"),
                None => (app.config.add_rule(rule), "Rule added"),
            };
            if changed(app, result, done) {
                app.rules_screen.cancel();
            }
        }
        _ => {}
    }
}
