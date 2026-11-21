//! Sanitising untrusted text before it reaches logs, notifications, the terminal or
//! the `TUI`
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

    let mut out = String::with_capacity(input.len().min(max_chars.saturating_mul(4)) + 8);
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

/// Prepare untrusted text (window titles, `PipeWire` node names and descriptions,
/// configured strings) for a `TUI` cell: unsafe characters (see `is_unsafe_char`)
/// that are whitespace (newline, tab, line separator) become a space, the others
/// (escape, bidirectional overrides, ...) are dropped. Display only: never apply it
/// to a value used for matching or written to the configuration.
///
/// Returns the input unchanged (borrowed) when nothing needed doing.
#[must_use]
pub fn for_display(input: &str) -> Cow<'_, str> {
    if !input.chars().any(is_unsafe_char) {
        return Cow::Borrowed(input);
    }
    Cow::Owned(
        input
            .chars()
            .filter_map(|c| match c {
                c if !is_unsafe_char(c) => Some(c),
                c if c.is_whitespace() => Some(' '),
                _ => None,
            })
            .collect(),
    )
}

/// Fold a multi-line message (an error chain, a `TOML` parse error with its source
/// excerpt) into one log line: lines trimmed and joined with `; `, empty lines and
/// pure decoration (`|`, `^^^`) dropped, a line repeating the previous one dropped
#[must_use]
pub fn one_line(text: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for line in text.lines().map(str::trim) {
        let decoration = line.chars().all(|c| matches!(c, '|' | '^' | '~' | ' '));
        if decoration || parts.last() == Some(&line) {
            continue;
        }
        parts.push(line);
    }
    parts.join("; ")
}

/// An error and its causes as one log line (like `{e:#}`), leaving out a cause whose
/// text the message before it already ends with (some errors repeat their source in
/// their own message: `Io error: Broken pipe: Broken pipe`)
#[must_use]
pub fn error_line(error: &anyhow::Error) -> String {
    let mut text = String::new();
    for cause in error.chain() {
        let cause = cause.to_string();
        if text.is_empty() {
            text = cause;
        } else if !text.ends_with(&cause) {
            text.push_str(": ");
            text.push_str(&cause);
        }
    }
    one_line(&text)
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

    #[test]
    fn test_error_line_drops_repeated_causes() {
        #[derive(Debug)]
        struct Repeats(std::io::Error);
        impl std::fmt::Display for Repeats {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "Io error: {}", self.0)
            }
        }
        impl std::error::Error for Repeats {
            fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
                Some(&self.0)
            }
        }
        let io = std::io::Error::new(std::io::ErrorKind::BrokenPipe, "Broken pipe");
        let error = anyhow::Error::new(Repeats(io)).context("Lost the Wayland connection");
        assert_eq!(
            error_line(&error),
            "Lost the Wayland connection: Io error: Broken pipe"
        );
        let plain = anyhow::anyhow!("line one\nline two").context("outer");
        assert_eq!(error_line(&plain), "outer: line one; line two");
    }

    #[test]
    fn test_one_line_folds_multi_line_messages() {
        let toml = "TOML parse error at line 3, column 5\n  |\n3 | foo\n  |     ^\nexpected `=`\n";
        assert_eq!(
            one_line(toml),
            "TOML parse error at line 3, column 5; 3 | foo; expected `=`"
        );
        assert_eq!(
            one_line("Compositor lost: Broken pipe (os error 32)\nBroken pipe (os error 32)"),
            "Compositor lost: Broken pipe (os error 32); Broken pipe (os error 32)",
            "only a line repeating the previous one is dropped"
        );
        assert_eq!(one_line("a\n\nb\nb\n"), "a; b");
        assert_eq!(one_line("single"), "single");
    }

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
    fn test_sanitize_line_without_limit() {
        assert_eq!(sanitize_line("a\nb", usize::MAX), "a\\nb");
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

    #[test_case("Headphones", "Headphones" ; "plain")]
    #[test_case("abc\u{202e}fed", "abcfed" ; "bidi override dropped")]
    #[test_case("\u{2066}x\u{2069}", "x" ; "bidi isolates dropped")]
    #[test_case("x\u{1b}[2Jy", "x[2Jy" ; "escape dropped")]
    #[test_case("a\nb\tc\u{2028}d", "a b c d" ; "line breaks and tabs become spaces")]
    #[test_case("音楽 🎵 ·", "音楽 🎵 ·" ; "unicode kept")]
    fn test_for_display(input: &str, expected: &str) {
        assert_eq!(for_display(input), expected);
    }

    #[test]
    fn test_for_display_borrows_when_clean() {
        assert!(matches!(for_display("ok"), Cow::Borrowed("ok")));
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
