//! Centralized editor helpers using `tui-input`
//!
//! This module provides a lightweight wrapper around `tui-input::Input` to provide
//! consistent text input handling across all TUI editor widgets (rules, sinks, settings).
//! The wrapper adds convenient constructors and value accessors while exposing the
//! underlying `Input` for event handling.

use tui_input::Input;

/// Wrapper around `tui-input::Input` providing consistent text editing interface
///
/// This struct wraps the `tui-input` crate's `Input` type to provide a stable API
/// for the TUI's text input needs. It supports creation from empty state or existing
/// strings, and exposes the underlying `Input` for event handling in input widgets.
#[derive(Debug, Clone, Default)]
pub(crate) struct EditorState {
    pub(crate) input: Input,
}

impl EditorState {
    /// Create an empty editor with cursor at position 0
    ///
    /// Equivalent to `EditorState::default()` but provided for ergonomics.
    #[must_use]
    pub(crate) fn new() -> Self {
        Self {
            input: Input::default(),
        }
    }

    /// Create from an existing string and place cursor at the end
    ///
    /// Used when editing existing configuration values (rules, sinks).
    #[must_use]
    pub(crate) fn from_string(s: String) -> Self {
        Self {
            input: Input::new(s),
        }
    }

    /// Get the current text value
    pub(crate) fn value(&self) -> &str {
        self.input.value()
    }

    /// Set the text value (replacing current input state)
    pub(crate) fn set_value(&mut self, s: String) {
        self.input = Input::new(s);
    }
}
