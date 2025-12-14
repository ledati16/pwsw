#[cfg(test)]
mod tests {
    use crate::tui::textfield::compute_display_window;

    #[test]
    fn short_string_no_truncate() {
        let (disp, cur, trunc) = compute_display_window("hello", 2, 10);
        assert_eq!(disp, "hello");
        assert_eq!(cur, 2);
        assert!(!trunc);
    }

    #[test]
    fn truncate_left_and_center_cursor() {
        let (disp, cur, trunc) = compute_display_window("abcdefghijklmno", 8, 5);
        assert!(trunc);
        // display length should be <= max
        assert!(disp.chars().count() <= 5);
        // cursor position should be within displayed length bounds
        assert!(cur <= disp.chars().count());
    }
}
