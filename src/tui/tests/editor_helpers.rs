#[cfg(test)]
mod tests {
    use crate::tui::editor_helpers::{insert_char_at, remove_char_at, remove_char_before};

    #[test]
    fn insert_simple() {
        let mut s = String::from("abc");
        let pos = insert_char_at(&mut s, 'X', 1);
        assert_eq!(s, "aXbc");
        assert_eq!(pos, 2);
    }

    #[test]
    fn insert_at_end() {
        let mut s = String::from("hi");
        let pos = insert_char_at(&mut s, '!', 10);
        assert_eq!(s, "hi!");
        assert_eq!(pos, 3);
    }

    #[test]
    fn remove_before_start() {
        let mut s = String::from("");
        let pos = remove_char_before(&mut s, 0);
        assert_eq!(s, "");
        assert_eq!(pos, 0);
    }

    #[test]
    fn remove_before_middle() {
        let mut s = String::from("héllo"); // unicode
        let pos = remove_char_before(&mut s, 2);
        assert_eq!(s, "hllo");
        assert_eq!(pos, 1);
    }

    #[test]
    fn remove_at_middle() {
        let mut s = String::from("abcd");
        let pos = remove_char_at(&mut s, 1);
        assert_eq!(s, "acd");
        assert_eq!(pos, 1);
    }
}
