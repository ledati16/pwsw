#[cfg(test)]
mod tests {
    use crate::tui::textfield::compute_display_window;

    #[test]
    fn no_truncation_when_short() {
        let (display, cursor_rel, truncated, _start) = compute_display_window("hello", 2, 10);
        assert_eq!(display, "hello");
        assert_eq!(cursor_rel, 2);
        assert!(!truncated);
    }

    #[test]
    fn truncation_left_with_ellipsis_reserved() {
        // value longer than max_value_len, cursor near end -> truncate left
        let val = "abcdefghijklmnopqrstuvwxyz";
        let max = 10usize;
        let cursor = 20usize; // near the end
        let (display, cursor_rel, truncated, _start) = compute_display_window(val, cursor, max);
        // When truncated left, one slot reserved for ellipsis, so display.len() should be max-1
        assert_eq!(display.chars().count(), max - 1);
        assert!(truncated);
        // Cursor relative should be within displayed range
        assert!(cursor_rel <= display.chars().count());
    }

    #[test]
    fn cursor_centering_behavior() {
        // With odd/even lengths ensure cursor is centered roughly
        let val = "0123456789ABCDEFGH"; // length 18
        let max = 7usize; // small window
        // Cursor in middle
        let cursor = 9usize;
        let (_display, cursor_rel, _truncated, _start) = compute_display_window(val, cursor, max);
        // For max=7, half=3 -> start=cursor-3=6, so cursor_rel should be 3
        assert_eq!(cursor_rel, 3);
    }

    #[test]
    fn unicode_handling_and_boundaries() {
        let val = "a🚀🌟bçd"; // chars: a,🚀,🌟,b,ç,d
        // cursor at various positions
        for cursor in 0..=6usize {
            let (display, cursor_rel, truncated, _start) = compute_display_window(val, cursor, 3);
            // display length shouldn't exceed 3 (or 2 when truncated left reserving ellipsis)
            assert!(display.chars().count() <= 3);
            // cursor_rel in 0..=display.len()
            assert!(cursor_rel <= display.chars().count());
            // truncated is boolean
            let _ = truncated;
        }
    }
}
