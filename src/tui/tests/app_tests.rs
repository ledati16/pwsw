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
        monitor_info: Some(false),
        running: true,
        windows: Vec::new(),
        unit_installed: true,
        service_enabled: Some(false),
        paused: false,
    });
    assert_eq!(app.dashboard_screen.max_action_index, 4);

    app.handle_update(AppUpdate::DaemonState {
        monitor_info: Some(false),
        running: true,
        windows: Vec::new(),
        unit_installed: false,
        service_enabled: None,
        paused: false,
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
    app.rules_screen.editor.focused_field = RuleField::Sinks;
    simulate_key_event(&mut app, key(KeyCode::Char(' ')));
    assert_eq!(app.rules_screen.mode, ListMode::SelectSink);
    simulate_key_event(&mut app, key(KeyCode::Down));
    simulate_key_event(&mut app, key(KeyCode::Enter));
    assert_eq!(app.rules_screen.mode, ListMode::AddEdit);
    assert_eq!(app.rules_screen.editor.sink_refs, ["d2"]);
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
        outputs: Vec::new(),
        minimized: false,
        matched_on: None,
        tracked: matched.then(|| crate::ipc::TrackedInfo {
            sink_name: "s1".to_string(),
            sink_desc: "Sink One".to_string(),
            fallbacks: Vec::new(),
            sources: Vec::new(),
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
        monitor_info: Some(false),
        running: true,
        windows,
        unit_installed: false,
        service_enabled: None,
        paused: false,
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
        assert!(editor.sink_refs.is_empty(), "the user chooses the sink");
        assert_eq!(editor.focused_field, RuleField::Sinks);
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
    assert_eq!(app.rules_screen.editor.sink_refs, ["Sink One"]);
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
        monitor_info: Some(false),
        running: true,
        windows: vec![
            window(3, "mpv", "video", false),
            window(2, "firefox", "Browser", true),
            window(1, "kitty", "~", false),
        ],
        unit_installed: false,
        service_enabled: None,
        paused: false,
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

/// The daemon's state as the status poll reports it
fn daemon_state(running: bool, paused: bool) -> AppUpdate {
    AppUpdate::DaemonState {
        monitor_info: Some(false),
        running,
        windows: Vec::new(),
        unit_installed: false,
        service_enabled: None,
        paused,
    }
}

#[test]
fn test_pause_key_sends_toggle_and_shows_the_answer() {
    let (tx, mut rx) = mpsc::channel(4);
    let mut app = App::with_config(make_config(vec![make_sink("s1", "d1", true)], vec![]));
    app.bg_cmd_tx = Some(tx);
    app.handle_update(daemon_state(true, false));

    for screen in [Screen::Dashboard, Screen::Settings] {
        app.current_screen = screen;
        simulate_key_event(&mut app, key(KeyCode::Char('p')));
        assert!(
            matches!(rx.try_recv(), Ok(BgCommand::TogglePause)),
            "{screen:?}: toggle sent"
        );
        assert!(app.pause_pending);
        // Nothing is shown as paused before the daemon answered
        let expected = screen == Screen::Settings;
        assert_eq!(app.daemon_paused, expected, "{screen:?}");

        // A second press while the request runs is refused
        simulate_key_event(&mut app, key(KeyCode::Char('p')));
        assert!(rx.try_recv().is_err(), "{screen:?}: not sent twice");
        assert_eq!(
            app.status_message(),
            Some("Already changing the pause state")
        );

        app.handle_update(AppUpdate::PauseToggled(Ok(!expected)));
        assert!(!app.pause_pending);
        assert_eq!(app.daemon_paused, !expected);
        let status = app.status_message().expect("status");
        if expected {
            assert_eq!(status, "Resumed rule-driven switching");
        } else {
            assert!(
                status.starts_with("Paused rule-driven switching"),
                "{status}"
            );
        }
    }

    // The next poll agrees; a failed request changes nothing and says why
    app.handle_update(daemon_state(true, false));
    simulate_key_event(&mut app, key(KeyCode::Char('p')));
    assert!(matches!(rx.try_recv(), Ok(BgCommand::TogglePause)));
    app.handle_update(AppUpdate::PauseToggled(Err(
        "Failed to pause or resume: timeout".to_string(),
    )));
    assert!(!app.pause_pending);
    assert!(!app.daemon_paused);
    assert_eq!(app.status_kind(), Some(crate::tui::app::StatusKind::Error));
}

#[test]
fn test_pause_key_refused_while_daemon_stopped() {
    let (tx, mut rx) = mpsc::channel(4);
    let mut app = App::with_config(make_config(vec![make_sink("s1", "d1", true)], vec![]));
    app.bg_cmd_tx = Some(tx);
    app.handle_update(daemon_state(false, false));

    simulate_key_event(&mut app, key(KeyCode::Char('p')));
    assert!(rx.try_recv().is_err(), "nothing sent");
    assert!(!app.pause_pending);
    let status = app.status_message().expect("status");
    assert!(status.starts_with("Daemon not running"), "{status}");
}

#[test]
fn test_pause_state_follows_the_status_poll() {
    let mut app = App::with_config(make_config(vec![make_sink("s1", "d1", true)], vec![]));
    app.handle_update(daemon_state(true, true));
    assert!(app.daemon_paused);
    // A daemon that stopped is not paused (a restarted one starts unpaused)
    app.handle_update(daemon_state(false, true));
    assert!(!app.daemon_paused);
    app.handle_update(daemon_state(true, false));
    assert!(!app.daemon_paused);
}

#[test]
fn test_pause_key_is_typed_into_text_fields() {
    let (tx, mut rx) = mpsc::channel(4);
    let mut app = App::with_config(make_config(vec![make_sink("s1", "d1", true)], vec![]));
    app.bg_cmd_tx = Some(tx);
    app.handle_update(daemon_state(true, false));
    app.current_screen = Screen::Rules;
    app.rules_screen.start_add();
    simulate_key_event(&mut app, key(KeyCode::Char('p')));
    assert!(rx.try_recv().is_err(), "typing p does not pause");
    assert_eq!(app.rules_screen.editor.app_id_pattern.value(), "p");
}

/// Open the switch delay editor on the Settings screen and type `text` over
/// the current value
fn type_delay(app: &mut App, text: &str) {
    use crate::tui::screens::settings::SettingItem;

    app.current_screen = Screen::Settings;
    app.settings_screen.selected = SettingItem::SwitchDelay.index();
    simulate_key_event(app, key(KeyCode::Enter));
    assert!(app.settings_screen.delay_input.is_some(), "editor opened");
    simulate_key_event(app, ctrl('u'));
    for c in text.chars() {
        simulate_key_event(app, key(KeyCode::Char(c)));
    }
}

#[test]
fn test_switch_delay_edit_applies_and_marks_dirty() {
    let mut app = App::with_config(make_config(vec![make_sink("s1", "d1", true)], vec![]));
    type_delay(&mut app, "3x00");
    assert_eq!(
        app.settings_screen
            .delay_input
            .as_ref()
            .map(tui_input::Input::value),
        Some("300"),
        "only digits are typed"
    );
    simulate_key_event(&mut app, key(KeyCode::Enter));
    assert!(app.settings_screen.delay_input.is_none(), "editor closed");
    assert_eq!(app.config.settings.switch_delay_ms, 300);
    assert!(app.config_dirty);

    // Esc keeps the old value
    type_delay(&mut app, "40");
    simulate_key_event(&mut app, key(KeyCode::Esc));
    assert!(app.settings_screen.delay_input.is_none());
    assert_eq!(app.config.settings.switch_delay_ms, 300);
}

#[test]
fn test_switch_delay_out_of_range_is_refused_with_a_message() {
    let mut app = App::with_config(make_config(vec![make_sink("s1", "d1", true)], vec![]));
    for text in ["5001", "99999999999", ""] {
        type_delay(&mut app, text);
        simulate_key_event(&mut app, key(KeyCode::Enter));
        assert!(
            app.settings_screen.delay_input.is_some(),
            "{text:?}: editor stays open"
        );
        let status = app.status_message().expect("status");
        assert!(
            status.contains("between 0 and 5000 ms"),
            "{text:?}: {status}"
        );
        assert_eq!(app.config.settings.switch_delay_ms, 0);
        assert!(!app.config_dirty);
        simulate_key_event(&mut app, key(KeyCode::Esc));
    }

    // The limit itself is accepted, and the saved config loads again
    type_delay(&mut app, "5000");
    simulate_key_event(&mut app, key(KeyCode::Enter));
    assert_eq!(app.config.settings.switch_delay_ms, 5000);

    let dir = TempDir::new().expect("tempdir");
    let path = dir.path().join("config.toml");
    app.config.save_to(&path).expect("valid config saves");
    assert_eq!(
        Config::load_from_path(&path)
            .expect("load")
            .settings
            .switch_delay_ms,
        5000
    );
}

// ============================================================================
// Mics (sources) and Output rules
// ============================================================================

fn shift(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::SHIFT)
}

/// Type `text` into the focused field, key by key
fn type_text(app: &mut App, text: &str) {
    for c in text.chars() {
        simulate_key_event(app, key(KeyCode::Char(c)));
    }
}

fn press_all(app: &mut App, keys: &[KeyEvent]) {
    for &k in keys {
        simulate_key_event(app, k);
    }
}

fn source_snapshot(sources: &[(&str, &str, bool)]) -> AppUpdate {
    AppUpdate::SinksData {
        active: Vec::new(),
        profiles: Vec::new(),
        sources: sources
            .iter()
            .map(|&(name, desc, is_default)| crate::pipewire::ActiveSink {
                name: name.to_string(),
                description: desc.to_string(),
                is_default,
            })
            .collect(),
        profile_sources: Vec::new(),
    }
}

/// Adding, picking, reordering, making the fallback, editing and deleting mics on
/// the Sinks screen's mic list, a rule choosing one, then saving: the file holds
/// exactly what the TUI shows.
#[test]
fn test_mics_are_managed_through_the_sinks_screen_and_saved() {
    let dir = TempDir::new().expect("tempdir");
    let (mut app, path) = file_app(&dir);
    app.handle_update(source_snapshot(&[("alsa_input.b", "Mic B", false)]));
    press_all(&mut app, &[
        key(KeyCode::Char('2')),
        key(KeyCode::Char('i')),
    ]);
    assert_eq!(app.endpoint_view, crate::config::Endpoint::Source);

    // Typed in
    simulate_key_event(&mut app, key(KeyCode::Char('a')));
    type_text(&mut app, "alsa_input.a");
    simulate_key_event(&mut app, key(KeyCode::Down));
    type_text(&mut app, "Mic A");
    simulate_key_event(&mut app, key(KeyCode::Enter));
    assert_eq!(app.sources_screen.mode, ListMode::List);
    assert_eq!(app.config.sources.len(), 1);
    assert!(
        app.config.sources[0].default,
        "the first mic is the default"
    );
    assert!(app.config.sinks.len() == 2, "sinks untouched");

    // Picked from PipeWire's inputs
    press_all(&mut app, &[
        key(KeyCode::Char('a')),
        key(KeyCode::Char(' ')),
        key(KeyCode::Enter),
    ]);
    assert_eq!(app.sources_screen.editor.name.value(), "alsa_input.b");
    simulate_key_event(&mut app, key(KeyCode::Enter));
    assert_eq!(app.config.sources[1].desc, "Mic B");

    // Reorder and make Mic B the fallback
    press_all(&mut app, &[
        key(KeyCode::Down),
        shift(KeyCode::Up),
        key(KeyCode::Char(' ')),
    ]);
    assert_eq!(app.sources_screen.selected, 0);
    assert_eq!(app.config.sources[0].desc, "Mic B");
    assert!(app.config.sources[0].default && !app.config.sources[1].default);

    // A rule chooses Mic B (Rules → edit → Mics → pick → apply → save)
    press_all(&mut app, &[
        key(KeyCode::Char('3')),
        key(KeyCode::Char('e')),
    ]);
    while app.rules_screen.editor.focused_field != RuleField::Mics {
        simulate_key_event(&mut app, key(KeyCode::Down));
    }
    press_all(&mut app, &[
        key(KeyCode::Char(' ')),
        key(KeyCode::Char(' ')),
        key(KeyCode::Enter),
        key(KeyCode::Enter),
    ]);
    assert_eq!(app.rules_screen.mode, ListMode::List);
    assert_eq!(app.config.rules[0].source_refs, ["Mic B"]);
    assert_eq!(app.config.rules[0].sink_ref, "Sink One", "sink kept");

    // Renaming Mic B keeps the rule pointing at it
    press_all(&mut app, &[
        key(KeyCode::Char('2')),
        key(KeyCode::Char('e')),
        key(KeyCode::Down),
        ctrl('u'),
    ]);
    type_text(&mut app, "Headset");
    simulate_key_event(&mut app, key(KeyCode::Enter));
    assert_eq!(app.config.sources[0].desc, "Headset");
    assert_eq!(app.config.rules[0].source_refs, ["Headset"]);

    // A mic a rule uses cannot be deleted (said at once, no question asked); the
    // other one can
    simulate_key_event(&mut app, key(KeyCode::Char('x')));
    assert_eq!(app.endpoint_screen().mode, ListMode::List, "no dialog");
    let status = app.status_message().expect("says why");
    assert!(
        status.starts_with("Used by rule 1 (")
            && status.ends_with("): retarget or delete it first"),
        "{status}"
    );
    assert_eq!(app.config.sources.len(), 2, "refused");
    press_all(&mut app, &[
        key(KeyCode::Down),
        key(KeyCode::Char('x')),
        key(KeyCode::Enter),
    ]);
    assert_eq!(app.config.sources.len(), 1);
    assert_eq!(app.config.sources[0].desc, "Headset");

    // i goes back to the sinks, whose list is unchanged
    simulate_key_event(&mut app, key(KeyCode::Char('i')));
    assert_eq!(app.endpoint_view, crate::config::Endpoint::Sink);

    simulate_key_event(&mut app, ctrl('s'));
    assert!(!app.config_dirty, "{:?}", app.status_message());
    let on_disk = Config::load_from_path(&path).expect("reload");
    assert_eq!(on_disk, app.config);
    assert_eq!(on_disk.rules[0].source_refs, ["Headset"]);
}

/// An output-only rule is made, saved and edited through the form: App ID may
/// stay empty only while Output is set
#[test]
fn test_rule_form_edits_output_and_app_id_is_optional_only_with_it() {
    let dir = TempDir::new().expect("tempdir");
    let (mut app, path) = file_app(&dir);
    press_all(&mut app, &[
        key(KeyCode::Char('3')),
        key(KeyCode::Char('a')),
    ]);
    let focus = |app: &mut App, field: RuleField| {
        while app.rules_screen.editor.focused_field != field {
            simulate_key_event(app, key(KeyCode::Down));
        }
    };
    // Sinks chosen but no App ID and no Output: refused, the form stays open
    focus(&mut app, RuleField::Sinks);
    press_all(&mut app, &[key(KeyCode::Char(' ')), key(KeyCode::Enter)]);
    simulate_key_event(&mut app, key(KeyCode::Enter));
    assert_eq!(app.rules_screen.mode, ListMode::AddEdit);
    assert_eq!(
        app.status_message(),
        Some(crate::tui::screens::rules::APP_ID_REQUIRED)
    );

    // With an Output it saves
    press_all(&mut app, &[key(KeyCode::Up)]);
    assert_eq!(app.rules_screen.editor.focused_field, RuleField::Output);
    type_text(&mut app, "^HDMI-A-1$");
    simulate_key_event(&mut app, key(KeyCode::Enter));
    assert_eq!(app.rules_screen.mode, ListMode::List);
    let rule = &app.config.rules[1];
    assert!(!rule.has_app_id());
    assert_eq!(rule.output_pattern.as_deref(), Some("^HDMI-A-1$"));
    assert_eq!(rule.sink_ref, "Sink One");

    // Editing keeps it; clearing Output again needs an App ID
    press_all(&mut app, &[key(KeyCode::Down), key(KeyCode::Char('e'))]);
    assert_eq!(app.rules_screen.editor.output_pattern.value(), "^HDMI-A-1$");
    focus(&mut app, RuleField::Output);
    simulate_key_event(&mut app, ctrl('u'));
    simulate_key_event(&mut app, key(KeyCode::Enter));
    assert_eq!(app.rules_screen.mode, ListMode::AddEdit, "refused");
    type_text(&mut app, "^DP-");
    simulate_key_event(&mut app, key(KeyCode::Enter));
    assert_eq!(app.config.rules[1].output_pattern.as_deref(), Some("^DP-"));

    simulate_key_event(&mut app, ctrl('s'));
    let on_disk = Config::load_from_path(&path).expect("reload");
    assert_eq!(on_disk, app.config);
    assert!(!on_disk.rules[1].has_app_id());
}

/// Without configured mics the Mics field only says so
#[test]
fn test_mics_field_without_mics_says_to_configure_them() {
    let dir = TempDir::new().expect("tempdir");
    let (mut app, _path) = file_app(&dir);
    press_all(&mut app, &[
        key(KeyCode::Char('3')),
        key(KeyCode::Char('e')),
    ]);
    while app.rules_screen.editor.focused_field != RuleField::Mics {
        simulate_key_event(&mut app, key(KeyCode::Down));
    }
    simulate_key_event(&mut app, key(KeyCode::Char(' ')));
    assert_eq!(app.rules_screen.mode, ListMode::AddEdit, "no picker");
    assert!(
        app.status_message().is_some_and(|m| m.contains("No mics")),
        "{:?}",
        app.status_message()
    );
}

/// The live preview matches the Output pattern like the daemon (monitors, not
/// minimized) and follows windows moving between monitors
#[test]
fn test_rule_preview_uses_output_matching() {
    let on = |id: u64, app_id: &str, outputs: &[&str], minimized: bool| {
        let mut w = window(id, app_id, "t", false);
        w.outputs = outputs.iter().map(|o| (*o).to_string()).collect();
        w.minimized = minimized;
        w
    };
    let mut app = windows_app(two_sinks(), vec![], vec![
        on(1, "mpv", &["HDMI-A-1"], false),
        on(2, "firefox", &["DP-2"], false),
        on(3, "kitty", &["HDMI-A-1"], true),
    ]);
    app.goto_screen(Screen::Rules);
    app.rules_screen.start_add();
    app.rules_screen.editor.output_pattern = tui_input::Input::new("^HDMI-".to_string());
    app.refresh_preview();
    assert_eq!(app.preview.view().total, 1, "{:?}", app.preview.view());
    assert_eq!(app.preview.view().shown, ["mpv | t · HDMI-A-1"]);

    // firefox moves onto the HDMI monitor: the preview follows
    app.handle_update(AppUpdate::DaemonState {
        monitor_info: Some(false),
        running: true,
        windows: vec![
            on(1, "mpv", &["HDMI-A-1"], false),
            on(2, "firefox", &["HDMI-A-1"], false),
            on(3, "kitty", &["HDMI-A-1"], true),
        ],
        unit_installed: false,
        service_enabled: None,
        paused: false,
    });
    app.refresh_preview();
    assert_eq!(app.preview.view().total, 2);

    // With an App ID too, both must match
    app.rules_screen.editor.app_id_pattern = tui_input::Input::new("^firefox$".to_string());
    app.refresh_preview();
    assert_eq!(app.preview.view().total, 1);
}

/// Typing in the sink editor reaches its focused field
#[test]
fn test_sink_editor_receives_typed_keys() {
    let mut app = App::with_config(make_config(vec![make_sink("s1", "d1", true)], vec![]));
    app.current_screen = Screen::Sinks;
    app.sinks_screen.start_add();
    app.sinks_screen.editor.name = tui_input::Input::new(String::new());
    app.sinks_screen.editor.focused_field = SinkField::Name;

    simulate_key_event(&mut app, key(KeyCode::Char('a')));
    assert_eq!(app.sinks_screen.editor.name.value(), "a");
    simulate_key_event(&mut app, key(KeyCode::Backspace));
    assert_eq!(app.sinks_screen.editor.name.value(), "");
}

/// Typing in the rule editor appends to its focused field
#[test]
fn test_rule_editor_receives_typed_keys() {
    let mut app = App::with_config(make_config(vec![make_sink("s1", "d1", true)], vec![]));
    app.current_screen = Screen::Rules;
    app.rules_screen.start_add();
    app.rules_screen.editor.app_id_pattern = tui_input::Input::new("foo".to_string());
    app.rules_screen.editor.focused_field = RuleField::AppId;

    simulate_key_event(&mut app, key(KeyCode::Char('d')));
    assert_eq!(app.rules_screen.editor.app_id_pattern.value(), "food");
}

/// Confirming the deletion of a sink a rule uses is refused and reported
#[test]
fn test_sink_delete_refuses_sink_used_by_rule() {
    let mut app = App::with_config(make_config(
        vec![make_sink("s1", "d1", true), make_sink("s2", "d2", false)],
        vec![make_rule("foo", None, "d2")],
    ));
    app.current_screen = Screen::Sinks;
    app.sinks_screen.selected = 1;
    app.sinks_screen.start_delete();
    simulate_key_event(&mut app, key(KeyCode::Enter));

    assert_eq!(
        app.config.sinks.len(),
        2,
        "referenced sink must not be removed"
    );
    let status = app.status_message().expect("refusal is reported");
    assert!(status.contains("Cannot remove sink 'd2'"), "{status}");
}

// ============================================================================
// First run, quitting with s, daemon start checks
// ============================================================================

/// `s` at the unsaved-changes prompt saves, then quits once the file is written
#[test]
fn test_quit_prompt_s_saves_then_quits() {
    let dir = TempDir::new().expect("tempdir");
    let (mut app, path) = file_app(&dir);
    edit(&mut app);
    simulate_key_event(&mut app, key(KeyCode::Char('q')));
    assert!(app.confirm_quit);
    let prompt = app.status_message().expect("prompt");
    assert!(prompt.contains("s save and quit"), "{prompt}");

    simulate_key_event(&mut app, key(KeyCode::Char('s')));
    assert!(app.should_quit, "saved (no worker: at once) and quit");
    let saved = Config::load_from_path(&path).expect("reload");
    assert_eq!(
        saved.settings.notify_rules, app.config.settings.notify_rules,
        "the edit is on disk"
    );
}

/// With a form open, `s` and `q` are typed into it rather than answering the
/// unsaved-changes prompt: the prompt then names only Ctrl+C
#[test]
fn test_quit_prompt_in_a_form_names_only_ctrl_c() {
    let dir = TempDir::new().expect("tempdir");
    let (mut app, _) = file_app(&dir);
    edit(&mut app);
    app.current_screen = Screen::Rules;
    simulate_key_event(&mut app, key(KeyCode::Char('e')));
    assert_eq!(app.rules_screen.mode, ListMode::AddEdit);
    simulate_key_event(&mut app, ctrl('c'));
    assert!(app.confirm_quit);
    assert_eq!(
        app.status_message(),
        Some("Unsaved changes: Ctrl+C again quits without saving · other keys stay")
    );
    // `s` goes to the form (and dismisses the prompt), it does not save and quit
    simulate_key_event(&mut app, key(KeyCode::Char('s')));
    assert!(!app.should_quit);
    assert!(!app.confirm_quit);
    assert_eq!(app.rules_screen.editor.app_id_pattern.value(), "firefoxs");

    simulate_key_event(&mut app, ctrl('c'));
    simulate_key_event(&mut app, ctrl('c'));
    assert!(app.should_quit, "Ctrl+C twice quits");

    // On a list the prompt offers s and q
    let (mut app, _) = file_app(&dir);
    edit(&mut app);
    simulate_key_event(&mut app, ctrl('c'));
    assert_eq!(
        app.status_message(),
        Some("Unsaved changes: s save and quit · q/Ctrl+C quit unsaved · other keys stay")
    );
}

/// With a background worker the quit waits for the save's result
#[test]
fn test_quit_prompt_s_waits_for_the_save() {
    let (tx, mut rx) = mpsc::channel(4);
    let mut app = App::with_config(make_config(vec![make_sink("s1", "d1", true)], vec![]));
    app.bg_cmd_tx = Some(tx);
    app.config_path = Some(PathBuf::from("unused"));
    edit(&mut app);
    simulate_key_event(&mut app, key(KeyCode::Char('q')));
    simulate_key_event(&mut app, key(KeyCode::Char('s')));
    let Ok(BgCommand::SaveConfig { config, .. }) = rx.try_recv() else {
        panic!("save queued");
    };
    assert!(!app.should_quit, "not before the file is written");
    assert!(app.is_quitting_after_save());
    app.handle_update(AppUpdate::ConfigSaved {
        saved: config,
        outcome: SaveOutcome::Saved,
    });
    assert!(app.should_quit);
}

/// A config without sinks opens on the Sinks screen with the next step in the
/// footer (naming the file when it was just created)
#[test]
fn test_first_run_opens_on_sinks_with_guidance() {
    let mut app = App::new(make_config(vec![], vec![]), PathBuf::from("/c/config.toml"));
    app.first_run_guidance(true);
    assert_eq!(app.current_screen, Screen::Sinks);
    let status = app.status_message().expect("guidance");
    assert!(status.starts_with("Created /c/config.toml."), "{status}");
    assert!(
        status.contains("Press a, then Space to pick an output"),
        "{status}"
    );

    let mut app = App::new(make_config(vec![], vec![]), PathBuf::from("/c/config.toml"));
    app.first_run_guidance(false);
    assert_eq!(app.current_screen, Screen::Sinks);
    let status = app.status_message().expect("guidance");
    assert!(status.starts_with("No sinks yet"), "{status}");

    // The path in the footer is short
    assert_eq!(
        crate::tui::app::home_relative(
            std::path::Path::new("/home/u/.config/pwsw/config.toml"),
            Some(std::ffi::OsStr::new("/home/u"))
        ),
        "~/.config/pwsw/config.toml"
    );
    assert_eq!(
        crate::tui::app::home_relative(std::path::Path::new("/etc/pwsw.toml"), None),
        "/etc/pwsw.toml"
    );

    // With sinks nothing changes
    let mut app = App::new(
        make_config(vec![make_sink("s1", "d1", true)], vec![]),
        PathBuf::from("/c/config.toml"),
    );
    app.first_run_guidance(true);
    assert_eq!(app.current_screen, Screen::Dashboard);
    assert_eq!(app.status_message(), None);
}

/// Adding the first sink (or mic) opens the form with Default already ticked;
/// Esc closes it without asking (nothing was changed)
#[test]
fn test_first_entry_form_is_prefilled_as_default() {
    let mut app = App::with_config(make_config(vec![], vec![]));
    app.current_screen = Screen::Sinks;
    simulate_key_event(&mut app, key(KeyCode::Char('a')));
    assert_eq!(app.sinks_screen.mode, ListMode::AddEdit);
    assert!(app.sinks_screen.editor.default, "first sink is the default");
    simulate_key_event(&mut app, key(KeyCode::Esc));
    assert_eq!(app.sinks_screen.mode, ListMode::List, "not modified");

    // With entries already there, a new one is not the default
    let mut app = App::with_config(make_config(vec![make_sink("s1", "d1", true)], vec![]));
    app.current_screen = Screen::Sinks;
    simulate_key_event(&mut app, key(KeyCode::Char('a')));
    assert!(!app.sinks_screen.editor.default);

    // Mics: the first one too
    simulate_key_event(&mut app, key(KeyCode::Esc));
    simulate_key_event(&mut app, key(KeyCode::Char('i')));
    simulate_key_event(&mut app, key(KeyCode::Char('a')));
    assert!(
        app.sources_screen.editor.default,
        "first mic is the default"
    );
}

/// Applying an edit says that Ctrl+S still has to write it
#[test]
fn test_applied_edits_remind_to_save() {
    let dir = TempDir::new().expect("tempdir");
    let (mut app, _) = file_app(&dir);
    app.current_screen = Screen::Rules;
    simulate_key_event(&mut app, key(KeyCode::Char('e')));
    simulate_key_event(&mut app, key(KeyCode::Char('$')));
    simulate_key_event(&mut app, key(KeyCode::Enter));
    assert_eq!(app.config.rules[0].app_id_pattern, "firefox$");
    assert_eq!(app.status_message(), Some("Rule updated · Ctrl+S to save"));

    app.current_screen = Screen::Sinks;
    simulate_key_event(&mut app, key(KeyCode::Char('a')));
    type_text(&mut app, "alsa_output.new");
    simulate_key_event(&mut app, key(KeyCode::Down));
    type_text(&mut app, "New Sink");
    simulate_key_event(&mut app, key(KeyCode::Enter));
    assert_eq!(app.config.sinks.len(), 3);
    assert_eq!(app.status_message(), Some("Sink added · Ctrl+S to save"));
}

/// An applied edit that leaves the config as saved does not ask for Ctrl+S (which
/// would answer "No unsaved changes")
#[test]
fn test_unchanged_edit_does_not_remind_to_save() {
    let dir = TempDir::new().expect("tempdir");
    let (mut app, _) = file_app(&dir);
    app.current_screen = Screen::Sinks;
    simulate_key_event(&mut app, key(KeyCode::Char('e')));
    simulate_key_event(&mut app, key(KeyCode::Enter));
    assert!(!app.config_dirty);
    assert_eq!(app.status_message(), Some("Sink updated"));

    // Undoing an edit by hand: the config equals the saved one again
    app.current_screen = Screen::Rules;
    simulate_key_event(&mut app, key(KeyCode::Char('e')));
    simulate_key_event(&mut app, key(KeyCode::Char('$')));
    simulate_key_event(&mut app, key(KeyCode::Enter));
    assert_eq!(app.status_message(), Some("Rule updated · Ctrl+S to save"));
    simulate_key_event(&mut app, key(KeyCode::Char('e')));
    simulate_key_event(&mut app, key(KeyCode::Backspace));
    simulate_key_event(&mut app, key(KeyCode::Enter));
    assert!(!app.config_dirty);
    assert_eq!(app.status_message(), Some("Rule updated"));
}

/// Start and Restart are refused when they cannot work: no sinks, or unsaved
/// changes the daemon would not see (nothing is saved for the user)
#[test]
fn test_dashboard_start_checks_sinks_and_unsaved_changes() {
    let (tx, mut rx) = mpsc::channel(4);
    let mut app = App::with_config(make_config(vec![], vec![]));
    app.bg_cmd_tx = Some(tx);
    app.config_path = Some(PathBuf::from("unused"));
    app.dashboard_screen.selected_action = 0; // Start
    simulate_key_event(&mut app, key(KeyCode::Enter));
    assert!(rx.try_recv().is_err(), "not sent");
    assert_eq!(app.status_message(), Some("Add a sink first (2 Sinks → a)"));

    app.config.sinks.push(make_sink("s1", "d1", true));
    app.mark_dirty();
    assert!(app.config_dirty);
    app.dashboard_screen.selected_action = 2; // Restart
    simulate_key_event(&mut app, key(KeyCode::Enter));
    assert!(rx.try_recv().is_err(), "not sent");
    let status = app.status_message().expect("why");
    assert!(
        status.starts_with("Unsaved changes: Ctrl+S first"),
        "{status}"
    );
    assert!(app.config_dirty, "not saved behind the user's back");

    // Stop is never refused for these reasons
    app.dashboard_screen.selected_action = 1;
    simulate_key_event(&mut app, key(KeyCode::Enter));
    assert!(matches!(
        rx.try_recv(),
        Ok(BgCommand::DaemonAction(crate::tui::app::DaemonAction::Stop))
    ));
}
