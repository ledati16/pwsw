//! UTF-8-safe editor string helpers

/// Insert `ch` at character index `idx` in `s`. Returns new cursor position (after inserted char).
pub(crate) fn insert_char_at(s: &mut String, ch: char, idx: usize) -> usize {
    let mut new = String::with_capacity(s.len() + ch.len_utf8());
    let mut inserted = false;
    for (i, c) in s.chars().enumerate() {
        if i == idx {
            new.push(ch);
            inserted = true;
        }
        new.push(c);
    }
    if !inserted {
        new.push(ch);
    }
    *s = new;
    usize::min(idx + 1, s.chars().count())
}

/// Remove the character before `idx` (like Backspace) and return new cursor position.
pub(crate) fn remove_char_before(s: &mut String, idx: usize) -> usize {
    if idx == 0 || s.is_empty() {
        return 0;
    }
    let mut new = String::with_capacity(s.len());
    let mut removed = false;
    for (i, c) in s.chars().enumerate() {
        if i == idx - 1 && !removed {
            removed = true;
            continue;
        }
        new.push(c);
    }
    *s = new;
    idx.saturating_sub(1)
}

/// Remove the character at `idx` (like Delete) and return new cursor position.
pub(crate) fn remove_char_at(s: &mut String, idx: usize) -> usize {
    if s.is_empty() || idx >= s.chars().count() {
        return s.chars().count();
    }
    let mut new = String::with_capacity(s.len());
    for (i, c) in s.chars().enumerate() {
        if i == idx {
            continue;
        }
        new.push(c);
    }
    *s = new;
    idx
}
