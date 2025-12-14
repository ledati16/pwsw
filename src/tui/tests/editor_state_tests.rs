#[cfg(test)]
mod tests {
    use crate::tui::editor_state as editor_state;

    #[test]
    fn insert_and_cursor_moves_unicode() {
        let mut s = "a🚀b".to_string(); // chars: ['a','🚀','b']
        let mut cursor = 2usize; // between '🚀' and 'b'
        // insert 'X' at cursor -> a🚀Xb
        let new_cur = editor_state::insert_char(&mut s, &mut cursor, 'X');
        assert_eq!(s, "a🚀Xb");
        assert_eq!(new_cur, 3);
        assert_eq!(cursor, 3);

        // move left
        editor_state::move_left(&mut cursor);
        assert_eq!(cursor, 2);

        // remove char before (should remove '🚀')
        let new_cur = editor_state::remove_char_before(&mut s, &mut cursor);
        assert_eq!(s, "aXb");
        assert_eq!(new_cur, 1);
        assert_eq!(cursor, 1);
    }

    #[test]
    fn delete_at_and_boundaries() {
        let mut s = "abçd".to_string(); // chars ['a','b','ç','d']
        let mut cursor = 2usize;
        // remove at cursor (delete 'ç')
        let new_cur = editor_state::remove_char_at(&mut s, &mut cursor);
        assert_eq!(s, "abd");
        assert_eq!(new_cur, 2);
        assert_eq!(cursor, 2);

        // move end
        editor_state::move_end(&mut cursor, &s);
        assert_eq!(cursor, s.chars().count());

        // move home
        editor_state::move_home(&mut cursor);
        assert_eq!(cursor, 0);
    }

    #[test]
    fn clamp_cursor_behaviour() {
        let mut s = "hello".to_string();
        let mut cursor = 100usize;
        editor_state::clamp_cursor(&mut cursor, &s);
        assert_eq!(cursor, s.chars().count());
    }
}
