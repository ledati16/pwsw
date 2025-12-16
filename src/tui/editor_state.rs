//! Centralized editor helpers using tui-input

use tui_input::Input;

/// `SimpleEditor` wraps `tui_input::Input` to provide a compatible interface
/// for the application state.
#[derive(Debug, Clone, Default)]
pub(crate) struct SimpleEditor {
    pub(crate) input: Input,
}

impl SimpleEditor {
    /// Create an empty editor with cursor at 0.
    pub(crate) fn new() -> Self {
        Self {
            input: Input::default(),
        }
    }

    /// Create from an existing string and place cursor at the end.
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
