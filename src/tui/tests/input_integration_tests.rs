#[cfg(test)]
mod tests {
    use super::super::input::simulate_key_event;
    use crate::tui::app::App;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn make_app_sinks() -> crate::tui::app::App {
        let mut app = crate::tui::app::App::new().expect("App::new failed");
        app.current_screen = crate::tui::app::Screen::Sinks;
        app
    }

    #[test]
    fn sinks_editor_ctrl_word_nav_and_delete() {
        let mut app = make_app_sinks();
        app.sinks_screen.start_add();
        app.sinks_screen.editor.name = crate::tui::editor_state::SimpleEditor::from_string("one two".to_string());
        app.sinks_screen.editor.focused_field = 0;

        let ke = KeyEvent::new(KeyCode::Left, KeyModifiers::CONTROL);
        simulate_key_event(&mut app, ke);
        assert_eq!(app.sinks_screen.editor.name.cursor, 4);

        let ke2 = KeyEvent::new(KeyCode::Backspace, KeyModifiers::CONTROL);
        simulate_key_event(&mut app, ke2);
        assert_eq!(app.sinks_screen.editor.name.value, "one ");
    }

    #[test]
    fn rules_editor_ctrl_word_nav_and_preview_request() {
        let mut app = crate::tui::app::App::new().expect("App::new failed");
        app.current_screen = crate::tui::app::Screen::Rules;
        app.rules_screen.start_add();
        app.rules_screen.editor.app_id_pattern = crate::tui::editor_state::SimpleEditor::from_string("foo bar".to_string());
        app.rules_screen.editor.focused_field = 0;

        let ke = KeyEvent::new(KeyCode::Left, KeyModifiers::CONTROL);
        simulate_key_event(&mut app, ke);
        assert_eq!(app.rules_screen.editor.app_id_pattern.cursor, 4);

        let ke2 = KeyEvent::new(KeyCode::Backspace, KeyModifiers::CONTROL);
        simulate_key_event(&mut app, ke2);
        assert_eq!(app.rules_screen.editor.app_id_pattern.value, "foo ");
    }
}
