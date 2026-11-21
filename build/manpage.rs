//! Text fixes around `mandown` for the man pages, shared by `build.rs` and its test
//! (`tests/manpage.rs`). Pure string functions: no I/O, no dependencies.
//!
//! `mandown` 1.1 turns every non-ASCII character into an ASCII look-alike, keeps the
//! backticks of inline code, writes a definition term on the line right after the
//! preceding paragraph, and nests the term's own bold inside a bold that its closing
//! `\fP` does not undo. [`protect_unicode`] runs on the Markdown before conversion,
//! [`postprocess`] on the roff after it.

use std::fmt::Write as _;

/// Placeholder prefix for a protected non-ASCII character: `PWSWU` + hex code + `Z`
/// (alphanumeric, so `mandown`'s escaping leaves it alone)
const TOKEN_PREFIX: &str = "PWSWU";
const TOKEN_END: char = 'Z';

/// ASCII stand-ins for terminals without the glyph (`groff -Tascii`), declared with
/// `.fchar` so UTF-8 output keeps the real character
const FALLBACKS: &[(char, &str)] = &[
    ('\u{2191}', "Up"),
    ('\u{2193}', "Down"),
    ('\u{2190}', "Left"),
    ('\u{2192}', "Right"),
    ('\u{00B7}', "\\-"),
    ('\u{2713}', "OK"),
    ('\u{26A0}', "!"),
    ('\u{2717}', "x"),
    ('\u{23F8}', "||"),
    ('\u{2014}', "\\-\\-"),
    ('\u{00D7}', "x"),
];

/// Replace every non-ASCII character with an ASCII placeholder that survives
/// `mandown::convert`; [`postprocess`] turns it into a groff `\[uXXXX]` escape
#[must_use]
pub fn protect_unicode(markdown: &str) -> String {
    let mut out = String::with_capacity(markdown.len());
    for ch in markdown.chars() {
        if ch.is_ascii() {
            out.push(ch);
        } else {
            let _ = write!(out, "{TOKEN_PREFIX}{:04X}{TOKEN_END}", u32::from(ch));
        }
    }
    out
}

/// Non-ASCII characters in `markdown` without an ASCII stand-in in [`FALLBACKS`]
/// (`build.rs` refuses them, so a page never loses a character on an ASCII terminal)
#[must_use]
pub fn unicode_without_fallback(markdown: &str) -> Vec<char> {
    let mut missing = Vec::new();
    for ch in markdown.chars() {
        if !ch.is_ascii() && !FALLBACKS.iter().any(|(c, _)| *c == ch) && !missing.contains(&ch) {
            missing.push(ch);
        }
    }
    missing
}

/// Fix `mandown`'s roff output for page `title`(`section`) of version `version`
///
/// * `.TH` gets an empty date (reproducible builds) and *pwsw VERSION* as source,
///   followed by `.fchar` fallbacks for the protected characters the page uses.
/// * Protected characters become `\[uXXXX]`.
/// * Inline code loses its literal backticks.
/// * A definition term (a bold line followed by `.RS`, not opening a paragraph)
///   starts its own paragraph, ends in roman, and its definition follows directly.
#[must_use]
pub fn postprocess(roff: &str, title: &str, section: u8, version: &str) -> String {
    let roff = roff.replace("`\\f[CR]", "\\f[CR]").replace("\\fP`", "\\fP");
    let (roff, used) = restore_unicode(&roff);

    let th = format!(".TH \"{title}\" {section}");
    let lines: Vec<&str> = roff.lines().collect();
    let mut out = String::with_capacity(roff.len() + 256);
    // 1: the term was written, 2: its `.RS` too (a `.PP` right after it is dropped)
    let mut after_term = 0u8;
    for (i, line) in lines.iter().enumerate() {
        let prev = i.checked_sub(1).map_or("", |p| lines[p]);
        let next = lines.get(i + 1).copied().unwrap_or("");

        match after_term {
            1 => after_term = 2,
            2 => {
                after_term = 0;
                if *line == ".PP" {
                    continue;
                }
            }
            _ => {}
        }
        if *line == th {
            let _ = writeln!(out, "{th} \"\" \"{title} {version}\"");
            for (ch, fallback) in FALLBACKS {
                if used.contains(ch) {
                    let code = u32::from(*ch);
                    let _ = writeln!(out, ".if \\n(.g .fchar \\[u{code:04X}] {fallback}");
                }
            }
        } else if is_term(prev, line, next) {
            out.push_str(".PP\n");
            out.push_str(&line[..line.len() - "\\fP".len()]);
            out.push_str("\\fR\n");
            after_term = 1;
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

/// A definition term: bold from start to end, followed by the definition's `.RS`, and
/// not the first line of a paragraph (a bold paragraph before a list looks the same)
fn is_term(prev: &str, line: &str, next: &str) -> bool {
    line.starts_with("\\fB") && line.ends_with("\\fP") && next == ".RS" && prev != ".PP"
}

/// Turn the placeholders of [`protect_unicode`] into `\[uXXXX]`; also returns the
/// characters found
fn restore_unicode(roff: &str) -> (String, Vec<char>) {
    let mut out = String::with_capacity(roff.len());
    let mut used = Vec::new();
    let mut rest = roff;
    while let Some(start) = rest.find(TOKEN_PREFIX) {
        out.push_str(&rest[..start]);
        let after = &rest[start + TOKEN_PREFIX.len()..];
        let hex_len = after.bytes().take_while(u8::is_ascii_hexdigit).count();
        let decoded = (hex_len >= 4 && after[hex_len..].starts_with(TOKEN_END))
            .then(|| u32::from_str_radix(&after[..hex_len], 16).ok())
            .flatten()
            .and_then(char::from_u32);
        if let Some(ch) = decoded {
            let _ = write!(out, "\\[u{:04X}]", u32::from(ch));
            if !used.contains(&ch) {
                used.push(ch);
            }
            rest = &after[hex_len + 1..];
        } else {
            out.push_str(TOKEN_PREFIX);
            rest = after;
        }
    }
    out.push_str(rest);
    (out, used)
}
