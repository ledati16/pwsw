use crate::tui::input::simulate_key_event;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

fn make_app_sinks() -> crate::tui::app::App {
    // Ensure tests use a temporary XDG_CONFIG_HOME so config loading doesn't touch the real config
    let guard = crate::test_utils::XdgTemp::new();
    // Create a dummy config file so Config::load() doesn't try to create a default one (which is blocked in test mode)
    let config_path = crate::config::Config::get_config_path().unwrap();
    std::fs::create_dir_all(config_path.parent().unwrap()).unwrap();
    std::fs::write(&config_path, "# dummy config\n[[sinks]]\nname=\"s1\"\ndesc=\"d1\"\ndefault=true\n").unwrap();
    
    let config = crate::config::Config::load().expect("Config::load failed");
    let mut app = crate::tui::app::App::with_config(config);
    app.current_screen = crate::tui::app::Screen::Sinks;
    // drop guard so caller's environment is restored after app created
    drop(guard);
    app
}

#[test]
fn sinks_editor_input_wiring() {
    let mut app = make_app_sinks();
    app.sinks_screen.start_add();
    // Start with empty
    app.sinks_screen.editor.name =
        crate::tui::editor_state::EditorState::from_string(String::new());
    app.sinks_screen.editor.focused_field = 0;

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
    let _guard = crate::test_utils::XdgTemp::new();
    let config_path = crate::config::Config::get_config_path().unwrap();
    std::fs::create_dir_all(config_path.parent().unwrap()).unwrap();
    std::fs::write(&config_path, "# dummy config\n[[sinks]]\nname=\"s1\"\ndesc=\"d1\"\ndefault=true\n").unwrap();

    let config = crate::config::Config::load().expect("Config::load failed");
    let mut app = crate::tui::app::App::with_config(config);
    app.current_screen = crate::tui::app::Screen::Rules;
    app.rules_screen.start_add();
    app.rules_screen.editor.app_id_pattern =
        crate::tui::editor_state::EditorState::from_string("foo".to_string());
    app.rules_screen.editor.focused_field = 0;

    // Type 'd'
    let ke = KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE);
    simulate_key_event(&mut app, ke);
    assert_eq!(app.rules_screen.editor.app_id_pattern.value(), "food");
}
