#[cfg(test)]
mod tests {
    use crate::tui::editor_state::SimpleEditor;

    #[test]
    fn move_word_left_basic() {
        let mut ed = SimpleEditor::from_string("hello world".to_string());
        // cursor starts at end
        assert_eq!(ed.value.graphemes(true).count(), 11);
        ed.move_word_left();
        // should land at start of "world" (index 6)
        assert_eq!(ed.cursor, 6);
    }

    #[test]
    fn move_word_right_basic() {
        let mut ed = SimpleEditor::from_string("hello world".to_string());
        // place cursor at start
        ed.cursor = 0;
        ed.move_word_right();
        // should land at end of first word (index 5)
        assert_eq!(ed.cursor, 5);
    }

    #[test]
    fn remove_word_before_multibyte() {
        let mut ed = SimpleEditor::from_string("a 🚀 b".to_string());
        // cursor at end
        assert_eq!(ed.value.graphemes(true).count(), 5);
        ed.remove_word_before();
        // Removing previous word should remove the 'b' grapheme only
        assert_eq!(ed.value, "a 🚀 ");
        // Cursor should move to the start index where the word was removed
        assert_eq!(ed.cursor, 4);
    }

    // Edge cases added below
    #[test]
    fn move_word_left_multiple_spaces() {
        let mut ed = SimpleEditor::from_string("one   two".to_string());
        // cursor at end
        ed.move_word_left();
        // should land at start of "two" (index 4)
        assert_eq!(ed.cursor, 4);
        ed.move_word_left();
        // should land at start of "one" (index 0)
        assert_eq!(ed.cursor, 0);
    }

    #[test]
    fn move_word_right_leading_spaces() {
        let mut ed = SimpleEditor::from_string("   foo bar".to_string());
        ed.cursor = 0;
        ed.move_word_right();
        // skip leading spaces, land at end of "foo" which is index 4
        assert_eq!(ed.cursor, 4);
    }

    #[test]
    fn remove_word_before_at_start() {
        let mut ed = SimpleEditor::from_string("abc".to_string());
        ed.cursor = 0;
        ed.remove_word_before();
        assert_eq!(ed.value, "abc");
        assert_eq!(ed.cursor, 0);
    }

    #[test]
    fn remove_word_before_with_tabs_and_spaces() {
        let mut ed = SimpleEditor::from_string("a\t \t b".to_string());
        // count graphemes
        let total = ed.value.graphemes(true).count();
        ed.remove_word_before();
        // should remove the 'b' only
        assert!(ed.value.contains("a"));
        assert!(ed.cursor < total);
    }
}
