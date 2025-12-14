//! Centralized editor helpers that operate on (String, cursor) pairs.

use crate::tui::editor_helpers as helpers;

/// Insert `ch` at the given cursor position in `s` and update `cursor`.
/// Returns the new cursor position (after the inserted char).
pub fn insert_char(s: &mut String, cursor: &mut usize, ch: char) -> usize {
    let new_cur = helpers::insert_char_at(s, ch, *cursor);
    *cursor = new_cur;
    new_cur
}

/// Remove the character before the cursor (Backspace semantics).
/// Updates `cursor` and returns the new cursor position.
pub fn remove_char_before(s: &mut String, cursor: &mut usize) -> usize {
    let new_cur = helpers::remove_char_before(s, *cursor);
    *cursor = new_cur;
    new_cur
}

/// Remove the character at the cursor (Delete semantics).
/// Updates `cursor` and returns the new cursor position.
pub fn remove_char_at(s: &mut String, cursor: &mut usize) -> usize {
    let new_cur = helpers::remove_char_at(s, *cursor);
    *cursor = new_cur;
    new_cur
}

/// Move cursor left by one (saturating at 0).
pub fn move_left(cursor: &mut usize) {
    *cursor = cursor.saturating_sub(1);
}

/// Move cursor right by one, clamped to string length.
pub fn move_right(cursor: &mut usize, s: &str) {
    let len = s.chars().count();
    *cursor = usize::min(len, *cursor + 1);
}

/// Move cursor to the start of the string.
pub fn move_home(cursor: &mut usize) {
    *cursor = 0;
}

/// Move cursor to the end of the string.
pub fn move_end(cursor: &mut usize, s: &str) {
    *cursor = s.chars().count();
}

/// Ensure cursor is within valid bounds for the string.
#[allow(dead_code)]
pub fn clamp_cursor(cursor: &mut usize, s: &str) {
    let len = s.chars().count();
    if *cursor > len {
        *cursor = len;
    }
}

/// SimpleEditor bundles a `String` and a `cursor` (char index) together
/// and exposes convenient editing methods that delegate to the editor helpers.
///
/// This keeps editor state compact and avoids duplicating cursor logic.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SimpleEditor {
    pub value: String,
    pub cursor: usize,
}

impl SimpleEditor {
    /// Create an empty editor with cursor at 0.
    pub fn new() -> Self {
        Self {
            value: String::new(),
            cursor: 0,
        }
    }

    /// Create from an existing string and place cursor at the end.
    pub fn from_string(s: String) -> Self {
        let cursor = s.chars().count();
        Self { value: s, cursor }
    }

    /// Insert a character at the current cursor position.
    /// Returns the new cursor position.
    pub fn insert(&mut self, ch: char) -> usize {
        let new = insert_char(&mut self.value, &mut self.cursor, ch);
        self.cursor = new;
        new
    }

    /// Remove the character before the cursor (backspace behavior).
    /// Returns the new cursor position.
    pub fn remove_before(&mut self) -> usize {
        let new = remove_char_before(&mut self.value, &mut self.cursor);
        self.cursor = new;
        new
    }

    /// Remove the character at the cursor (delete behavior).
    /// Returns the new cursor position.
    pub fn remove_at(&mut self) -> usize {
        let new = remove_char_at(&mut self.value, &mut self.cursor);
        self.cursor = new;
        new
    }

    /// Move the cursor left by one char.
    pub fn move_left(&mut self) {
        move_left(&mut self.cursor);
    }

    /// Move the cursor right by one char.
    pub fn move_right(&mut self) {
        move_right(&mut self.cursor, self.value.as_str());
    }

    /// Move cursor to the start.
    pub fn move_home(&mut self) {
        move_home(&mut self.cursor);
    }

    /// Move cursor to the end.
    pub fn move_end(&mut self) {
        move_end(&mut self.cursor, self.value.as_str());
    }

    /// Clamp cursor to valid range for current value.
    #[allow(dead_code)]
    pub fn clamp(&mut self) {
        if self.cursor > self.value.chars().count() {
            self.cursor = self.value.chars().count();
        }
    }
}
