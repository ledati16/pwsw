#[cfg(test)]
mod tests {
    use crate::tui::textfield::compute_display_window;
    use unicode_segmentation::UnicodeSegmentation;

    /// Helper: replicate the click->cursor mapping logic used by the TUI input handler.
    fn map_click_to_cursor(value: &str, cursor: usize, max_value_len: usize, click_in_value: usize) -> usize {
        let (display_substr, _cursor_rel, truncated_left, start) =
            compute_display_window(value, cursor, max_value_len);
        let mut rel = click_in_value;
        if truncated_left {
            if rel == 0 {
                return start;
            } else {
                rel = rel.saturating_sub(1);
            }
        }
        let disp_len = display_substr.graphemes(true).count();
        let char_pos = if rel >= disp_len { disp_len } else { rel };
        start + char_pos
    }

    #[test]
    fn click_no_truncation_positions() {
        let val = "hello";
        let max = 10usize;
        // full display, start == 0
        let (display, _cur, truncated, start) = compute_display_window(val, 0, max);
        assert!(!truncated);
        assert_eq!(start, 0);
        assert_eq!(display, "hello");

        // clicking at positions 0..=len should map to 0..=len
        for click in 0..=display.graphemes(true).count() {
            let mapped = map_click_to_cursor(val, 0, max, click);
            assert_eq!(mapped, click);
        }
    }

    #[test]
    fn click_left_truncation_ellipsis_and_positions() {
        // choose ascii for clarity
        let val = "abcdef"; // graphemes: a,b,c,d,e,f
        // small display width to force left truncation
        let max = 3usize;
        // choose a cursor that forces start > 0
        let cursor = 4usize;
        let (display, _cur, truncated, start) = compute_display_window(val, cursor, max);
        assert!(truncated);
        // clicking the ellipsis (click_in_value == 0) should map to start
        let mapped_ellipsis = map_click_to_cursor(val, cursor, max, 0);
        assert_eq!(mapped_ellipsis, start);

        // clicking first visible grapheme (click_in_value == 1) -> start + 0
        let mapped_first = map_click_to_cursor(val, cursor, max, 1);
        assert_eq!(mapped_first, start + 0);

        // clicking beyond displayed length should clamp to end of display
        let beyond = 10usize;
        let mapped_beyond = map_click_to_cursor(val, cursor, max, beyond);
        let disp_len = display.graphemes(true).count();
        assert_eq!(mapped_beyond, start + disp_len);
    }

    #[test]
    fn click_unicode_grapheme_positions() {
        // include multi-codepoint graphemes (emoji)
        let val = "a🚀🌟bçd"; // graphemes: a,🚀,🌟,b,ç,d
        let max = 3usize; // small window
        let cursor = 2usize; // between 🚀 and 🌟
        let (display, _cur, truncated, start) = compute_display_window(val, cursor, max);
        // ensure we got a display substring
        let disp_g = display.graphemes(true).collect::<Vec<&str>>();
        assert!(disp_g.len() <= max);

        // Test clicking each visible grapheme maps to start + index (accounting for ellipsis)
        for i in 0..=disp_g.len() {
            // when truncated, clicking index 0 is ellipsis
            let click_in_value = if truncated { i + 1 } else { i };
            let mapped = map_click_to_cursor(val, cursor, max, click_in_value);
            let expected = if truncated {
                // clicking first clickable position maps to start + (i - 1)
                if i == 0 { start } else { start + (i - 1) }
            } else {
                start + i
            };
            assert_eq!(mapped, expected);
        }
    }
}
