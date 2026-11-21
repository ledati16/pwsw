//! App state tests: saving, quitting, pasting and the "changed on disk" dialog.
//!
//! Configs live in temporary directories (`load_from_path`/`save_to`); nothing
//! touches the default config path.

use std::path::PathBuf;

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use pretty_assertions::assert_eq;
use tempfile::TempDir;
use tokio::sync::mpsc;

use crate::config::Config;
use crate::test_utils::fixtures::{make_config, make_rule, make_sink};
use crate::tui::app::{App, AppUpdate, BgCommand, SaveOutcome, Screen};
use crate::tui::input::{handle_event, simulate_key_event};
use crate::tui::screens::rules::{RuleField, RulesMode};
use crate::tui::screens::sinks::{SinkField, SinksMode};

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn ctrl(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
}

/// An app editing a real config file in `dir`.
fn file_app(dir: &TempDir) -> (App, PathBuf) {
    let path = dir.path().join("config.toml");
    make_config(
        vec![
            make_sink("s1", "Sink One", true),
            make_sink("s2", "Sink Two", false),
        ],
        vec![make_rule("firefox", None, "Sink One")],
    )
    .save_to(&path)
    .expect("write config");
    let config = Config::load_from_path(&path).expect("load config");
    (App::new(config, path.clone()), path)
}

/// Make an edit through the UI state (toggles a setting).
fn edit(app: &mut App) {
    app.config.settings.notify_rules.toggle();
    app.mark_dirty();
}

#[test]
fn test_edit_after_ctrl_s_stays_dirty() {
    let (tx, mut rx) = mpsc::channel(4);
    let mut app = App::with_config(make_config(vec![make_sink("s1", "d1", true)], vec![]));
    app.bg_cmd_tx = Some(tx);
    app.config_path = Some(PathBuf::from("unused"));

    edit(&mut app);
    simulate_key_event(&mut app, ctrl('s'));
    let Ok(BgCommand::SaveConfig { config, .. }) = rx.try_recv() else {
        panic!("save queued");
    };
    assert!(app.save_pending);

    // Edited again while the save runs
    app.config.settings.match_by_index.toggle();
    app.mark_dirty();

    app.handle_update(AppUpdate::ConfigSaved {
        saved: config,
        outcome: SaveOutcome::Saved,
    });
    assert!(!app.save_pending);
    assert!(app.config_dirty, "the later edit is not on disk");
    let status = app.status_message().expect("status");
    assert!(status.contains("not saved yet"), "{status}");

    // Quitting now still asks
    simulate_key_event(&mut app, key(KeyCode::Char('q')));
    assert!(!app.should_quit);
    assert!(app.confirm_quit);
}

#[test]
fn test_save_of_current_state_marks_clean() {
    let dir = TempDir::new().expect("tempdir");
    let (mut app, path) = file_app(&dir);
    edit(&mut app);
    simulate_key_event(&mut app, ctrl('s'));
    assert!(!app.config_dirty);
    let on_disk = Config::load_from_path(&path).expect("reload");
    assert_eq!(on_disk, app.config);
}

#[test]
fn test_quit_confirmation_is_reset_by_other_keys() {
    let mut app = App::with_config(make_config(vec![make_sink("s1", "d1", true)], vec![]));
    edit(&mut app);

    simulate_key_event(&mut app, key(KeyCode::Char('q')));
    assert!(app.confirm_quit);

    // Switching screens dismisses the prompt (and still switches)
    simulate_key_event(&mut app, key(KeyCode::Char('2')));
    assert!(!app.confirm_quit);
    assert_eq!(app.current_screen, Screen::Sinks);

    // A later single q asks again instead of quitting
    simulate_key_event(&mut app, key(KeyCode::Char('q')));
    assert!(!app.should_quit);
    assert!(app.confirm_quit);

    // Esc only dismisses the prompt
    simulate_key_event(&mut app, key(KeyCode::Esc));
    assert!(!app.confirm_quit);
    assert!(!app.should_quit);
    assert_eq!(app.status_message(), None);

    // q, q quits
    simulate_key_event(&mut app, key(KeyCode::Char('q')));
    simulate_key_event(&mut app, key(KeyCode::Char('q')));
    assert!(app.should_quit);
}

#[test]
fn test_quit_after_changes_were_saved_needs_no_confirmation() {
    let dir = TempDir::new().expect("tempdir");
    let (mut app, _path) = file_app(&dir);
    edit(&mut app);
    simulate_key_event(&mut app, key(KeyCode::Char('q')));
    assert!(app.confirm_quit);
    simulate_key_event(&mut app, ctrl('s')); // dismisses the prompt, saves
    assert!(!app.confirm_quit);
    assert!(!app.config_dirty);
    simulate_key_event(&mut app, key(KeyCode::Char('q')));
    assert!(app.should_quit);
}

#[test]
fn test_ctrl_c_asks_before_discarding_changes() {
    let mut app = App::with_config(make_config(vec![make_sink("s1", "d1", true)], vec![]));
    simulate_key_event(&mut app, ctrl('c'));
    assert!(app.should_quit, "nothing to lose: quits at once");

    let mut app = App::with_config(make_config(vec![make_sink("s1", "d1", true)], vec![]));
    app.current_screen = Screen::Sinks;
    edit(&mut app);
    app.sinks_screen.start_add();

    // In an editor: Ctrl+C prompts, Esc dismisses the prompt but keeps the editor
    simulate_key_event(&mut app, ctrl('c'));
    assert!(!app.should_quit);
    assert!(app.confirm_quit);
    simulate_key_event(&mut app, key(KeyCode::Esc));
    assert!(!app.confirm_quit);
    assert_eq!(app.sinks_screen.mode, SinksMode::AddEdit);

    // q while the prompt is up in an editor is text, not a confirmation
    simulate_key_event(&mut app, ctrl('c'));
    simulate_key_event(&mut app, key(KeyCode::Char('q')));
    assert!(!app.should_quit);
    assert_eq!(app.sinks_screen.editor.name.value(), "q");

    simulate_key_event(&mut app, ctrl('c'));
    simulate_key_event(&mut app, ctrl('c'));
    assert!(app.should_quit);
}

#[test]
fn test_paste_goes_into_focused_field_without_control_chars() {
    let mut app = App::with_config(make_config(vec![make_sink("s1", "d1", true)], vec![]));
    app.current_screen = Screen::Rules;
    app.rules_screen.start_add();
    app.rules_screen.editor.focused_field = RuleField::Title;

    handle_event(
        &mut app,
        &Event::Paste("Fire\nfox\tq\r\u{1b}[x".to_string()),
    );
    assert_eq!(app.rules_screen.editor.title_pattern.value(), "Firefoxq[x");
    assert_eq!(
        app.rules_screen.mode,
        RulesMode::AddEdit,
        "newline did not save"
    );
    assert!(!app.should_quit);
    assert_eq!(app.config.rules.len(), 0);
}

#[test]
fn test_paste_without_focused_field_is_ignored() {
    let mut app = App::with_config(make_config(vec![make_sink("s1", "d1", true)], vec![
        make_rule("a", None, "d1"),
    ]));
    app.current_screen = Screen::Rules;
    let before = app.config.clone();

    // List view: the paste must not act as key presses (x deletes, q quits)
    handle_event(&mut app, &Event::Paste("xq\n".to_string()));
    assert_eq!(app.rules_screen.mode, RulesMode::List);
    assert!(!app.should_quit);
    assert_eq!(app.config, before);

    // Editor focused on a non-text field
    app.current_screen = Screen::Sinks;
    app.sinks_screen.start_add();
    app.sinks_screen.editor.focused_field = SinkField::Default;
    handle_event(&mut app, &Event::Paste("abc".to_string()));
    assert_eq!(app.sinks_screen.editor.name.value(), "");
    assert_eq!(app.sinks_screen.editor.desc.value(), "");
    assert_eq!(app.sinks_screen.editor.icon.value(), "");
}

/// Write a different (valid) config to `path` behind the app's back.
fn change_on_disk(path: &std::path::Path) -> Config {
    let external = make_config(vec![make_sink("ext", "External Sink", true)], vec![]);
    external.save_to(path).expect("external write");
    Config::load_from_path(path).expect("reload external")
}

/// App with an unsaved edit whose save was refused because the file changed.
fn conflicted(dir: &TempDir) -> (App, PathBuf, Config) {
    let (mut app, path) = file_app(dir);
    edit(&mut app);
    let external = change_on_disk(&path);
    simulate_key_event(&mut app, ctrl('s'));
    assert!(app.save_conflict, "save refused");
    assert!(app.config_dirty);
    (app, path, external)
}

#[test]
fn test_conflict_overwrite_saves_tui_config() {
    let dir = TempDir::new().expect("tempdir");
    let (mut app, path, _) = conflicted(&dir);

    // Other keys do nothing while the dialog is open
    simulate_key_event(&mut app, key(KeyCode::Char('2')));
    assert_eq!(app.current_screen, Screen::Dashboard);
    assert!(app.save_conflict);

    simulate_key_event(&mut app, key(KeyCode::Char('o')));
    assert!(!app.save_conflict);
    assert!(!app.config_dirty);
    assert_eq!(Config::load_from_path(&path).expect("load"), app.config);
}

#[test]
fn test_conflict_reload_takes_disk_config() {
    let dir = TempDir::new().expect("tempdir");
    let (mut app, path, external) = conflicted(&dir);
    app.sinks_screen.selected = 1;

    simulate_key_event(&mut app, key(KeyCode::Char('r')));
    assert!(!app.save_conflict);
    assert!(!app.config_dirty);
    assert_eq!(app.config, external);
    assert_eq!(app.sinks_screen.selected, 0, "editor state reset");
    // The reloaded config saves without a new conflict
    edit(&mut app);
    simulate_key_event(&mut app, ctrl('s'));
    assert!(!app.save_conflict);
    assert_eq!(Config::load_from_path(&path).expect("load"), app.config);
}

#[test]
fn test_conflict_cancel_keeps_edits_and_file() {
    let dir = TempDir::new().expect("tempdir");
    let (mut app, path, external) = conflicted(&dir);

    simulate_key_event(&mut app, key(KeyCode::Esc));
    assert!(!app.save_conflict);
    assert!(app.config_dirty);
    assert_eq!(Config::load_from_path(&path).expect("load"), external);
}

#[test]
fn test_enable_disable_offered_when_unit_installed_even_if_daemon_runs_directly() {
    let mut app = App::with_config(make_config(vec![], vec![]));
    app.handle_update(AppUpdate::DaemonState {
        running: true,
        windows: Vec::new(),
        unit_installed: true,
        service_enabled: Some(false),
    });
    assert_eq!(app.dashboard_screen.max_action_index, 4);

    app.handle_update(AppUpdate::DaemonState {
        running: true,
        windows: Vec::new(),
        unit_installed: false,
        service_enabled: None,
    });
    assert_eq!(app.dashboard_screen.max_action_index, 2);
}

#[test]
fn test_rule_sink_dropdown_accepts_enter_and_survives_shrunk_list() {
    let mut app = App::with_config(make_config(
        vec![make_sink("s1", "d1", true), make_sink("s2", "d2", false)],
        vec![],
    ));
    app.current_screen = Screen::Rules;
    app.rules_screen.start_add();
    app.rules_screen.editor.focused_field = RuleField::Sink;
    simulate_key_event(&mut app, key(KeyCode::Char(' ')));
    assert_eq!(app.rules_screen.mode, RulesMode::SelectSink);
    simulate_key_event(&mut app, key(KeyCode::Down));
    simulate_key_event(&mut app, key(KeyCode::Enter));
    assert_eq!(app.rules_screen.mode, RulesMode::AddEdit);
    assert_eq!(app.rules_screen.editor.sink_ref, "d2");
}
