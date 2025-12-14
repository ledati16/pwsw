#[cfg(test)]
mod tests {
    use crate::tui::editor_helpers::{insert_char_at, remove_char_before, remove_char_at};

    #[test]
    fn insert_unicode_middle() {
        let mut s = String::from("a600b"); // includes emoji as surrogate pair in display
        // Insert 'X' at position 1 (after 'a')
        let pos = insert_char_at(&mut s, 'X', 1);
        assert_eq!(s, "aX600b");
        assert_eq!(pos, 2);
    }

    #[test]
    fn remove_before_unicode_boundary() {
        let mut s = String::from("hé😊lo");
        // remove char before index 3 (the emoji)
        let new_pos = remove_char_before(&mut s, 3);
        assert!(s.graphemes(true).count() <= 4);
        assert_eq!(new_pos, 2);
    }

    #[test]
    fn remove_at_out_of_bounds() {
        let mut s = String::from("abc");
        let pos = remove_char_at(&mut s, 10);
        assert_eq!(pos, 3);
        assert_eq!(s, "abc");
    }
}
