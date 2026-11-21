//! Sanitising untrusted text before it reaches logs, notifications or the `TUI`
//!
//! Window titles and `app_id`s are chosen by whatever program owns the window, so
//! they must be treated as untrusted. A title containing a newline can forge whole
//! lines in `daemon.log` (which the `TUI` then highlights as if the daemon wrote
//! them), terminal escape sequences can repaint a terminal, and `<`/`&` are
//! interpreted as Pango markup by notification daemons that advertise the
//! `body-markup` capability.

use std::borrow::Cow;
use std::fmt::Write as _;

/// Maximum number of characters of an untrusted string written to the log.
pub const MAX_LOG_CHARS: usize = 200;

/// Ellipsis appended when a string is truncated.
const ELLIPSIS: char = '…';

/// Whether `c` must not appear verbatim in a single log line or notification.
///
/// Covers C0/C1 control characters (newline, carriage return, escape, …), the
/// Unicode line/paragraph separators, and the bidirectional override/isolate
/// controls that can visually reorder the rest of a line.
fn is_unsafe_char(c: char) -> bool {
    c.is_control()
        || matches!(
            c,
            '\u{2028}' | '\u{2029}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}'
        )
}

/// Escape unsafe characters (see `is_unsafe_char`) as visible escapes such as
/// `\n` or `\u{1b}` and truncate to at most `max_chars` characters (plus an
/// ellipsis). Truncation always happens on a `char` boundary.
///
/// Returns the input unchanged (borrowed) when nothing needed doing.
#[must_use]
pub fn sanitize_line(input: &str, max_chars: usize) -> Cow<'_, str> {
    let needs_escape = input.chars().any(is_unsafe_char);
    // Byte length is an upper bound on the char count, so this is a cheap pre-check
    let needs_truncate = input.len() > max_chars && input.chars().count() > max_chars;
    if !needs_escape && !needs_truncate {
        return Cow::Borrowed(input);
    }

    let mut out = String::with_capacity(input.len().min(max_chars * 4) + 8);
    for (index, c) in input.chars().enumerate() {
        if index >= max_chars {
            out.push(ELLIPSIS);
            break;
        }
        if is_unsafe_char(c) {
            // `escape_debug` gives `\n`, `\r`, `\t` or `\u{…}` - always printable ASCII
            let _ = write!(out, "{}", c.escape_debug());
        } else {
            out.push(c);
        }
    }
    Cow::Owned(out)
}

/// Sanitise an untrusted string (window title, `app_id`, rule trigger text) for
/// a log line: escape control characters and truncate to [`MAX_LOG_CHARS`].
#[must_use]
pub fn for_log(input: &str) -> Cow<'_, str> {
    sanitize_line(input, MAX_LOG_CHARS)
}

/// Prepare untrusted text for a notification body: replace unsafe control
/// characters (see `is_unsafe_char`) with a space and, if `markup` is set,
/// escape the characters Pango markup interprets (`&`, `<`, `>`) so the text is
/// shown literally.
///
/// `markup` must reflect whether the notification server interprets body markup
/// (its `body-markup` capability): a server that does not would show the escapes
/// themselves (`Tom &amp; Jerry`).
#[must_use]
pub fn for_notification(input: &str, markup: bool) -> String {
    let mut out = String::with_capacity(input.len());
    for c in input.chars() {
        match c {
            '&' if markup => out.push_str("&amp;"),
            '<' if markup => out.push_str("&lt;"),
            '>' if markup => out.push_str("&gt;"),
            c if is_unsafe_char(c) => out.push(' '),
            c => out.push(c),
        }
    }
    out
}

/// Remove `ANSI` escape sequences (`CSI` such as `ESC [ 31 m`, `OSC` terminated by
/// `BEL` or `ESC \`, and two-byte escapes) from `input`, then drop any remaining
/// control characters other than newline and tab.
///
/// Used to show the stderr of a child `pwsw` process (whose error reports are
/// coloured) inside the `TUI` status line.
#[must_use]
pub fn strip_ansi(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            if !c.is_control() || c == '\n' || c == '\t' {
                out.push(c);
            }
            continue;
        }
        match chars.next() {
            // CSI: parameters/intermediates until a final byte in 0x40..=0x7E
            Some('[') => {
                for c in chars.by_ref() {
                    if ('\u{40}'..='\u{7e}').contains(&c) {
                        break;
                    }
                }
            }
            // OSC: until BEL or ST (`ESC \`)
            Some(']') => {
                while let Some(c) = chars.next() {
                    if c == '\u{7}' {
                        break;
                    }
                    if c == '\u{1b}' && chars.peek() == Some(&'\\') {
                        chars.next();
                        break;
                    }
                }
            }
            // Two-byte escape (or a lone trailing ESC): drop both
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;
    use test_case::test_case;

    use super::*;

    #[test_case("plain title", "plain title" ; "unchanged")]
    #[test_case("a\nb", "a\\nb" ; "newline escaped")]
    #[test_case("a\r\nINFO forged", "a\\r\\nINFO forged" ; "crlf escaped")]
    #[test_case("x\u{1b}[31mred", "x\\u{1b}[31mred" ; "escape escaped")]
    #[test_case("tab\there", "tab\\there" ; "tab escaped")]
    #[test_case("del\u{7f}", "del\\u{7f}" ; "del escaped")]
    #[test_case("c1\u{85}", "c1\\u{85}" ; "c1 next line escaped")]
    #[test_case("ls\u{2028}x", "ls\\u{2028}x" ; "line separator escaped")]
    #[test_case("bidi\u{202e}txt", "bidi\\u{202e}txt" ; "bidi override escaped")]
    fn test_sanitize_line_escapes(input: &str, expected: &str) {
        assert_eq!(sanitize_line(input, 1000), expected);
    }

    #[test]
    fn test_sanitize_line_borrows_when_clean() {
        assert!(matches!(sanitize_line("ok", 10), Cow::Borrowed("ok")));
    }

    #[test]
    fn test_sanitize_line_truncates_on_char_boundary() {
        // Multi-byte characters: emoji (4 bytes) and CJK (3 bytes)
        let input = "🎵🎶音楽プレーヤー";
        assert_eq!(sanitize_line(input, 3), "🎵🎶音…");
        assert_eq!(sanitize_line(input, 9), input, "exactly max chars");
        // A long CJK string whose byte length exceeds the limit but char count does not
        let cjk = "音".repeat(10);
        assert_eq!(sanitize_line(&cjk, 10), cjk);
    }

    #[test]
    fn test_for_log_caps_length() {
        let long = "x".repeat(MAX_LOG_CHARS + 50);
        let out = for_log(&long);
        assert_eq!(out.chars().count(), MAX_LOG_CHARS + 1);
        assert!(out.ends_with(ELLIPSIS));
    }

    #[test]
    fn test_sanitize_line_escape_counts_as_one_char() {
        // The newline becomes `\n` but counts as a single input character
        assert_eq!(sanitize_line("a\nbcd", 2), "a\\n…");
    }

    #[test_case("Firefox", "Firefox" ; "plain")]
    #[test_case("<b>bold</b>", "&lt;b&gt;bold&lt;/b&gt;" ; "tags")]
    #[test_case("Tom & Jerry", "Tom &amp; Jerry" ; "ampersand")]
    #[test_case("&amp;", "&amp;amp;" ; "already escaped is escaped again")]
    #[test_case("a\nb\u{1b}c", "a b c" ; "controls become spaces")]
    #[test_case("音楽 🎵", "音楽 🎵" ; "unicode kept")]
    fn test_for_notification(input: &str, expected: &str) {
        assert_eq!(for_notification(input, true), expected);
    }

    #[test_case("<b>bold</b>", "<b>bold</b>" ; "tags kept")]
    #[test_case("Tom & Jerry", "Tom & Jerry" ; "ampersand kept")]
    #[test_case("a\nb\u{1b}c\u{202e}d", "a b c d" ; "controls still become spaces")]
    fn test_for_notification_without_markup(input: &str, expected: &str) {
        assert_eq!(for_notification(input, false), expected);
    }

    #[test_case("plain", "plain" ; "plain")]
    #[test_case("\u{1b}[31mError:\u{1b}[0m boom", "Error: boom" ; "csi colours")]
    #[test_case("\u{1b}[1;38;5;196mX", "X" ; "csi with params")]
    #[test_case("\u{1b}]8;;http://x\u{7}link\u{1b}]8;;\u{1b}\\", "link" ; "osc hyperlink")]
    #[test_case("a\nb\tc\rd", "a\nb\tcd" ; "keeps newline and tab")]
    #[test_case("trailing\u{1b}", "trailing" ; "lone escape")]
    fn test_strip_ansi(input: &str, expected: &str) {
        assert_eq!(strip_ansi(input), expected);
    }
}
