#[cfg(test)]
mod tests {
    use crate::tui::editor_state::SimpleEditor;

    #[test]
    fn simple_editor_insert_and_delete_unicode() {
        let mut e = SimpleEditor::from_string("a🚀b".to_string()); // chars: a,🚀,b
        // cursor initialized at end (3)
        assert_eq!(e.cursor, 3);

        // move left twice
        e.move_left();
        e.move_left();
        assert_eq!(e.cursor, 1);

        // insert 'X' after 'a' -> aX🚀b
        e.insert('X');
        assert_eq!(e.value, "aX🚀b");
        // cursor should have advanced by 1 (from 1 to 2)
        assert_eq!(e.cursor, 2);

        // backspace (remove before) removes 'X'
        e.remove_before();
        assert_eq!(e.value, "a🚀b");
        assert_eq!(e.cursor, 1);

        // delete at cursor (remove 🚀)
        e.remove_at();
        assert_eq!(e.value, "ab");
        assert_eq!(e.cursor, 1);
    }

    #[test]
    fn move_home_end_and_clamp() {
        let mut e = SimpleEditor::from_string("hello".to_string());
        e.move_home();
        assert_eq!(e.cursor, 0);
        e.move_end();
        assert_eq!(e.cursor, 5);
        // set cursor beyond and clamp
        e.cursor = 100;
        e.clamp();
        assert_eq!(e.cursor, 5);
    }
}
