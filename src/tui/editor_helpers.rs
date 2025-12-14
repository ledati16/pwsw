//! UTF-8-safe editor string helpers (grapheme-aware)

use unicode_segmentation::UnicodeSegmentation;

/// Insert `ch` at grapheme cluster index `idx` in `s`. Returns new cursor position (after inserted grapheme).
pub(crate) fn insert_char_at(s: &mut String, ch: char, idx: usize) -> usize {
    let mut new = String::with_capacity(s.len() + ch.len_utf8());
    let mut inserted = false;
    for (i, g) in s.graphemes(true).enumerate() {
        if i == idx {
            new.push(ch);
            inserted = true;
        }
        new.push_str(g);
    }
    if !inserted {
        new.push(ch);
    }
    *s = new;
    usize::min(idx + 1, s.graphemes(true).count())
}

/// Remove the grapheme cluster before `idx` (like Backspace) and return new cursor position.
pub(crate) fn remove_char_before(s: &mut String, idx: usize) -> usize {
    if idx == 0 || s.is_empty() {
        return 0;
    }
    let mut new = String::with_capacity(s.len());
    let mut removed = false;
    for (i, g) in s.graphemes(true).enumerate() {
        if i == idx - 1 && !removed {
            removed = true;
            continue;
        }
        new.push_str(g);
    }
    *s = new;
    idx.saturating_sub(1)
}

/// Remove the grapheme cluster at `idx` (like Delete) and return new cursor position.
pub(crate) fn remove_char_at(s: &mut String, idx: usize) -> usize {
    if s.is_empty() || idx >= s.graphemes(true).count() {
        return s.graphemes(true).count();
    }
    let mut new = String::with_capacity(s.len());
    for (i, g) in s.graphemes(true).enumerate() {
        if i == idx {
            continue;
        }
        new.push_str(g);
    }
    *s = new;
    idx
}
