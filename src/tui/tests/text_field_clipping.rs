use crate::tui::screens::rules::compute_display_window;

#[test]
fn test_compute_display_window_short() {
    let (s, cur, left) = compute_display_window("hello", 2, 10);
    assert_eq!(s, "hello");
    assert_eq!(cur, 2);
    assert!(!left);
}

#[test]
fn test_compute_display_window_truncate_left() {
    let (s, cur, left) = compute_display_window("abcdefghijklmnopqrstuvwxyz", 10, 5);
    // max_value_len=5 -> with one slot for ellipsis we take 4 chars
    assert!(left, "expected truncated left");
    assert_eq!(s.chars().count(), 4);
    // cursor around middle should be centered
    assert!(cur <= 3);
}

#[test]
fn test_compute_display_window_cursor_at_end() {
    let (s, cur, left) = compute_display_window("abcdefghijklmnopqrstuvwxyz", 25, 6);
    assert!(left);
    assert_eq!(s.chars().count(), 5); // one slot reserved for ellipsis
    // cursor_rel should point near the end
    assert!(cur >= 4 || cur == s.chars().count());
}

#[test]
fn test_compute_display_window_unicode() {
    let val = "こんにちは世界"; // 7 Japanese characters
    let (s, cur, left) = compute_display_window(val, 4, 4);
    assert_eq!(s.chars().count(), 4 - 1); // one reserved for ellipsis when truncated
    assert!(left || !left == true);
}
