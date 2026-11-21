use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::tui::input::simulate_key_event;

fn make_app_sinks() -> crate::tui::app::App {
    let temp_dir = tempfile::tempdir().expect("failed to create tempdir");
    let config_path = temp_dir.path().join("config.toml");
    std::fs::write(
        &config_path,
        "[[sinks]]\nname=\"s1\"\ndesc=\"d1\"\ndefault=true\n",
    )
    .unwrap();

    let config = crate::config::Config::load_from_path(&config_path).expect("Config::load failed");
    let mut app = crate::tui::app::App::with_config(config);
    app.current_screen = crate::tui::app::Screen::Sinks;
    app
}

#[test]
fn sinks_editor_input_wiring() {
    let mut app = make_app_sinks();
    app.sinks_screen.start_add();
    // Start with empty
    app.sinks_screen.editor.name = tui_input::Input::new(String::new());
    app.sinks_screen.editor.focused_field = crate::tui::screens::sinks::SinkField::Name;

    // Type 'a'
    let ke = KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE);
    simulate_key_event(&mut app, ke);
    assert_eq!(app.sinks_screen.editor.name.value(), "a");

    // Backspace
    let ke2 = KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE);
    simulate_key_event(&mut app, ke2);
    assert_eq!(app.sinks_screen.editor.name.value(), "");
}

#[test]
fn rules_editor_input_wiring() {
    let temp_dir = tempfile::tempdir().expect("failed to create tempdir");
    let config_path = temp_dir.path().join("config.toml");
    std::fs::write(
        &config_path,
        "[[sinks]]\nname=\"s1\"\ndesc=\"d1\"\ndefault=true\n",
    )
    .unwrap();

    let config = crate::config::Config::load_from_path(&config_path).expect("Config::load failed");
    let mut app = crate::tui::app::App::with_config(config);
    app.current_screen = crate::tui::app::Screen::Rules;
    app.rules_screen.start_add();
    app.rules_screen.editor.app_id_pattern = tui_input::Input::new("foo".to_string());
    app.rules_screen.editor.focused_field = crate::tui::screens::rules::RuleField::AppId;

    // Type 'd'
    let ke = KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE);
    simulate_key_event(&mut app, ke);
    assert_eq!(app.rules_screen.editor.app_id_pattern.value(), "food");
}

#[test]
fn sinks_delete_refuses_sink_used_by_rule() {
    let temp_dir = tempfile::tempdir().expect("failed to create tempdir");
    let config_path = temp_dir.path().join("config.toml");
    std::fs::write(
        &config_path,
        "[[sinks]]\nname=\"s1\"\ndesc=\"d1\"\ndefault=true\n\n[[sinks]]\nname=\"s2\"\ndesc=\"d2\"\n\n\
         [[rules]]\napp_id=\"foo\"\nsink=\"d2\"\n",
    )
    .unwrap();

    let config = crate::config::Config::load_from_path(&config_path).expect("Config::load failed");
    let mut app = crate::tui::app::App::with_config(config);
    app.current_screen = crate::tui::app::Screen::Sinks;
    app.sinks_screen.selected = 1;
    app.sinks_screen.start_delete();
    simulate_key_event(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    assert_eq!(
        app.config.sinks.len(),
        2,
        "referenced sink must not be removed"
    );
    let status = app.status_message().expect("refusal is reported");
    assert!(status.contains("Cannot remove sink 'd2'"), "{status}");
}
