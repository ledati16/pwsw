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

/// Move cursor to the start of the previous word (grapheme index). Words are delimited by
/// whitespace. Returns new cursor index.
pub(crate) fn move_cursor_word_left(idx: usize, s: &str) -> usize {
    if idx == 0 {
        return 0;
    }
    let g: Vec<&str> = s.graphemes(true).collect();
    let mut i = idx;
    // Skip any whitespace immediately to the left
    while i > 0 && g[i - 1].trim().is_empty() {
        i -= 1;
    }
    // Then skip non-whitespace to find word boundary
    while i > 0 && !g[i - 1].trim().is_empty() {
        i -= 1;
    }
    i
}

/// Move cursor to the end of the next word (grapheme index). Returns new cursor index.
pub(crate) fn move_cursor_word_right(idx: usize, s: &str) -> usize {
    let g: Vec<&str> = s.graphemes(true).collect();
    let len = g.len();
    if idx >= len {
        return len;
    }
    let mut i = idx;
    // Skip whitespace to the right
    while i < len && g[i].trim().is_empty() {
        i += 1;
    }
    // Then skip non-whitespace
    while i < len && !g[i].trim().is_empty() {
        i += 1;
    }
    i
}

/// Remove word before `idx` (Ctrl+Backspace behavior). Returns new cursor index.
pub(crate) fn remove_word_before(s: &mut String, idx: usize) -> usize {
    if idx == 0 || s.is_empty() {
        return 0;
    }
    let g: Vec<&str> = s.graphemes(true).collect();
    let len = g.len();

    // If the cursor is at the start of a word (i.e. the grapheme at `idx` is non-whitespace),
    // prefer removing the word to the right (the word under the cursor). Otherwise, remove
    // the previous word (traditional Ctrl+Backspace behavior).
    let (start, end) = if idx < len && !g[idx].trim().is_empty() {
        let end = move_cursor_word_right(idx, s);
        (idx, end)
    } else {
        let start = move_cursor_word_left(idx, s);
        (start, idx)
    };

    // Rebuild string without graphemes in [start, end)
    let mut new = String::with_capacity(s.len());
    for (i, gr) in g.iter().enumerate() {
        if i >= start && i < end {
            continue;
        }
        new.push_str(gr);
    }
    *s = new;
    start
}
