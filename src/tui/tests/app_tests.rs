//! App state tests: saving, quitting, pasting, the "changed on disk" dialog and
//! making a rule from a window.
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
use crate::tui::screens::list::ListMode;
use crate::tui::screens::rules::RuleField;
use crate::tui::screens::sinks::SinkField;

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
    assert_eq!(app.sinks_screen.mode, ListMode::AddEdit);

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
        ListMode::AddEdit,
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
    assert_eq!(app.rules_screen.mode, ListMode::List);
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
    assert_eq!(app.rules_screen.mode, ListMode::SelectSink);
    simulate_key_event(&mut app, key(KeyCode::Down));
    simulate_key_event(&mut app, key(KeyCode::Enter));
    assert_eq!(app.rules_screen.mode, ListMode::AddEdit);
    assert_eq!(app.rules_screen.editor.sink_ref, "d2");
}

/// An app with unsaved edits whose Ctrl+S save is in flight on a (fake) worker.
fn app_with_save_in_flight() -> (App, mpsc::Receiver<BgCommand>, Box<Config>) {
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
    (app, rx, config)
}

fn press(app: &mut App, key: KeyEvent) {
    handle_event(app, &Event::Key(key));
}

#[test]
fn test_quit_during_save_waits_for_it_then_exits() {
    for quit_key in [key(KeyCode::Char('q')), ctrl('c')] {
        let (mut app, _rx, saved) = app_with_save_in_flight();

        // Unsaved-changes prompt, then the confirmation: waits instead of exiting
        press(&mut app, quit_key);
        assert!(app.confirm_quit);
        press(&mut app, quit_key);
        assert!(!app.should_quit, "must not exit while the save runs");
        assert!(app.is_quitting_after_save());
        assert!(app.quit_save_deadline().is_some());
        assert!(app.status_message().expect("status").contains("Saving"));

        // Input is ignored while waiting
        press(&mut app, key(KeyCode::Char('2')));
        assert_eq!(app.current_screen, Screen::Dashboard);

        app.handle_update(AppUpdate::ConfigSaved {
            saved,
            outcome: SaveOutcome::Saved,
        });
        assert!(app.should_quit, "exits once the save is on disk");
    }
}

#[test]
fn test_quit_during_failed_save_stays_open_and_reports() {
    let (mut app, _rx, saved) = app_with_save_in_flight();
    press(&mut app, ctrl('c'));
    press(&mut app, ctrl('c'));
    assert!(app.is_quitting_after_save());

    app.handle_update(AppUpdate::ConfigSaved {
        saved,
        outcome: SaveOutcome::Failed("disk full".to_string()),
    });
    assert!(!app.should_quit);
    assert!(!app.is_quitting_after_save());
    assert!(app.config_dirty, "edits are still unsaved");
    let status = app.status_message().expect("status");
    assert!(
        status.contains("Not quitting") && status.contains("disk full"),
        "{status}"
    );
}

#[test]
fn test_quit_during_save_times_out_then_can_force() {
    let (mut app, _rx, _saved) = app_with_save_in_flight();
    press(&mut app, key(KeyCode::Char('q')));
    press(&mut app, key(KeyCode::Char('q')));
    assert!(app.is_quitting_after_save());

    // The event loop calls this at the deadline
    app.quit_save_wait_expired();
    assert!(!app.should_quit, "stays open rather than lose the save");
    assert!(!app.is_quitting_after_save());
    assert!(
        app.status_message()
            .expect("status")
            .contains("Save still running")
    );

    // A further confirmed quit exits without waiting again
    press(&mut app, key(KeyCode::Char('q')));
    press(&mut app, key(KeyCode::Char('q')));
    assert!(app.should_quit);
}

#[test]
fn test_esc_cancels_quit_waiting_for_save() {
    let (mut app, _rx, saved) = app_with_save_in_flight();
    press(&mut app, key(KeyCode::Char('q')));
    press(&mut app, key(KeyCode::Char('q')));
    press(&mut app, key(KeyCode::Esc));
    assert!(!app.is_quitting_after_save());

    app.handle_update(AppUpdate::ConfigSaved {
        saved,
        outcome: SaveOutcome::Saved,
    });
    assert!(!app.should_quit, "quit was cancelled");
}

// ============================================================================
// "New rule for this window" (n in the Dashboard's window list)
// ============================================================================

fn window(id: u64, app_id: &str, title: &str, matched: bool) -> crate::ipc::WindowInfo {
    crate::ipc::WindowInfo {
        id: Some(id),
        app_id: app_id.to_string(),
        title: title.to_string(),
        matched_on: None,
        tracked: matched.then(|| crate::ipc::TrackedInfo {
            sink_name: "s1".to_string(),
            sink_desc: "Sink One".to_string(),
            fallbacks: Vec::new(),
        }),
    }
}

/// The Dashboard's window list showing `windows`, with these sinks and rules
fn windows_app(
    sinks: Vec<crate::config::SinkConfig>,
    rules: Vec<crate::config::Rule>,
    windows: Vec<crate::ipc::WindowInfo>,
) -> App {
    let mut app = App::with_config(make_config(sinks, rules));
    app.handle_update(AppUpdate::DaemonState {
        running: true,
        windows,
        unit_installed: false,
        service_enabled: None,
    });
    app.dashboard_screen.toggle_view();
    app
}

fn two_sinks() -> Vec<crate::config::SinkConfig> {
    vec![
        make_sink("s1", "Sink One", true),
        make_sink("s2", "Sink Two", false),
    ]
}

#[test]
fn test_new_rule_from_window_escapes_and_anchors_app_id() {
    for (app_id, expected) in [
        ("org.mozilla.firefox", r"^org\.mozilla\.firefox$"),
        ("a+b", r"^a\+b$"),
    ] {
        let mut app = windows_app(
            two_sinks(),
            vec![make_rule("^kitty$", None, "Sink One")],
            vec![window(1, app_id, "Some Title", false)],
        );
        simulate_key_event(&mut app, key(KeyCode::Char('n')));

        assert_eq!(app.current_screen, Screen::Rules, "{app_id}");
        let screen = &app.rules_screen;
        assert_eq!(screen.mode, ListMode::AddEdit, "{app_id}");
        assert_eq!(screen.editing_index, None, "adds a rule: {app_id}");
        let editor = &screen.editor;
        assert_eq!(editor.app_id_pattern.value(), expected);
        assert_eq!(editor.title_pattern.value(), "", "any title");
        assert_eq!(editor.desc.value(), "");
        assert_eq!(editor.sink_ref, "", "the user chooses the sink");
        assert_eq!(editor.focused_field, RuleField::Sink);
        assert_eq!(editor.window_title.as_deref(), Some("Some Title"));

        // The pattern matches exactly this app ID
        let rule_re = regex::Regex::new(expected).expect("valid regex");
        assert!(rule_re.is_match(app_id));
        for other in ["orgXmozillaXfirefox", "aab", "x.a+b", "a+bc"] {
            assert!(!rule_re.is_match(other), "{expected} matches {other}");
        }
    }
}

#[test]
fn test_new_rule_from_window_saves_after_choosing_a_sink() {
    let mut app = windows_app(two_sinks(), vec![], vec![window(
        1,
        "org.mozilla.firefox",
        "Firefox",
        false,
    )]);
    simulate_key_event(&mut app, key(KeyCode::Char('n')));
    // Untouched: Esc closes without asking
    simulate_key_event(&mut app, key(KeyCode::Esc));
    assert_eq!(app.rules_screen.mode, ListMode::List);
    assert!(!app.rules_screen.confirm_discard);

    app.goto_screen(Screen::Dashboard);
    simulate_key_event(&mut app, key(KeyCode::Char('n')));
    simulate_key_event(&mut app, key(KeyCode::Char(' '))); // open the sink list
    simulate_key_event(&mut app, key(KeyCode::Down));
    simulate_key_event(&mut app, key(KeyCode::Enter)); // Sink Two
    simulate_key_event(&mut app, key(KeyCode::Enter)); // save
    assert_eq!(app.rules_screen.mode, ListMode::List);
    let rule = app.config.rules.first().expect("rule added");
    assert_eq!(rule.app_id_pattern, r"^org\.mozilla\.firefox$");
    assert_eq!(rule.title_pattern, None);
    assert_eq!(rule.sink_ref, "Sink Two");
    assert!(app.config_dirty);
}

#[test]
fn test_new_rule_from_window_with_one_sink_preselects_it() {
    let mut app = windows_app(vec![make_sink("s1", "Sink One", true)], vec![], vec![
        window(1, "mpv", "video.mkv", false),
    ]);
    simulate_key_event(&mut app, key(KeyCode::Char('n')));
    assert_eq!(app.rules_screen.mode, ListMode::AddEdit);
    assert_eq!(app.rules_screen.editor.sink_ref, "Sink One");
    simulate_key_event(&mut app, key(KeyCode::Enter));
    assert_eq!(app.config.rules.len(), 1, "saved as it is");
    assert_eq!(app.config.rules[0].app_id_pattern, "^mpv$");
}

#[test]
fn test_new_rule_from_window_opens_the_rule_that_already_matches() {
    let mut app = windows_app(
        two_sinks(),
        vec![
            make_rule("^kitty$", None, "Sink One"),
            make_rule("firefox", None, "Sink Two"),
            make_rule("fire", None, "Sink One"),
        ],
        vec![window(1, "org.mozilla.firefox", "Rust Book", true)],
    );
    simulate_key_event(&mut app, key(KeyCode::Char('n')));
    assert_eq!(app.current_screen, Screen::Rules);
    let screen = &app.rules_screen;
    assert_eq!(screen.mode, ListMode::AddEdit);
    assert_eq!(screen.editing_index, Some(1), "the first matching rule");
    assert_eq!(screen.editor.app_id_pattern.value(), "firefox");
    assert_eq!(screen.editor.window_title.as_deref(), Some("Rust Book"));
    assert_eq!(
        app.status_message(),
        Some("This window already matches rule 2")
    );
}

#[test]
fn test_new_rule_follows_the_window_selection() {
    // Matched windows are listed first: firefox, then kitty, then mpv
    let mut app = windows_app(
        two_sinks(),
        vec![make_rule("firefox", None, "Sink One")],
        vec![
            window(1, "kitty", "~", false),
            window(2, "firefox", "Browser", true),
            window(3, "mpv", "video", false),
        ],
    );
    simulate_key_event(&mut app, key(KeyCode::Down));
    simulate_key_event(&mut app, key(KeyCode::Down));
    // The list refreshes with the same windows: the selection stays on mpv
    app.handle_update(AppUpdate::DaemonState {
        running: true,
        windows: vec![
            window(3, "mpv", "video", false),
            window(2, "firefox", "Browser", true),
            window(1, "kitty", "~", false),
        ],
        unit_installed: false,
        service_enabled: None,
    });
    simulate_key_event(&mut app, key(KeyCode::Char('n')));
    assert_eq!(app.rules_screen.editor.app_id_pattern.value(), "^mpv$");
}

#[test]
fn test_new_rule_without_windows_only_says_why() {
    // Daemon running, no windows
    let mut app = windows_app(two_sinks(), vec![], vec![]);
    simulate_key_event(&mut app, key(KeyCode::Char('n')));
    assert_eq!(app.current_screen, Screen::Dashboard);
    assert_eq!(app.rules_screen.mode, ListMode::List);
    assert_eq!(app.status_message(), Some("No windows to make a rule for"));

    // Daemon stopped
    let mut app = windows_app(two_sinks(), vec![], vec![window(1, "mpv", "x", false)]);
    app.daemon_running = false;
    simulate_key_event(&mut app, key(KeyCode::Char('n')));
    assert_eq!(app.current_screen, Screen::Dashboard);
    assert_eq!(app.rules_screen.mode, ListMode::List);
    assert_eq!(
        app.status_message(),
        Some("Daemon not running: start it to list windows")
    );

    // No sinks to send the window to
    let mut app = windows_app(vec![], vec![], vec![window(1, "mpv", "x", false)]);
    simulate_key_event(&mut app, key(KeyCode::Char('n')));
    assert_eq!(app.current_screen, Screen::Dashboard);
    assert_eq!(app.status_message(), Some("Add a sink first (2 Sinks → a)"));

    // In the logs view n does nothing
    let mut app = windows_app(two_sinks(), vec![], vec![window(1, "mpv", "x", false)]);
    app.dashboard_screen.toggle_view();
    simulate_key_event(&mut app, key(KeyCode::Char('n')));
    assert_eq!(app.current_screen, Screen::Dashboard);
    assert_eq!(app.status_message(), None);
}
