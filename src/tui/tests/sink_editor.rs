#[cfg(test)]
mod tests {
    use crate::tui::editor_helpers::{insert_char_at, remove_char_before, remove_char_at};

    #[test]
    fn cursor_insert_remove_unicode() {
        let mut s = String::from("héllo");
        let mut cursor = s.chars().count();
        // insert '!' at end
        cursor = insert_char_at(&mut s, '!', cursor);
        assert_eq!(s, "héllo!");
        // move left and backspace
        cursor = remove_char_before(&mut s, cursor);
        assert_eq!(s, "héllo");
        // remove at 1
        let pos = remove_char_at(&mut s, 1);
        assert_eq!(s, "hllo");
        assert_eq!(pos, 1);
    }
}
