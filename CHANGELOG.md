# Changelog

## Unreleased

- tui: remove mouse support
  - Removed terminal mouse capture and all mouse handling code from the TUI.
  - Simplifies input handling and avoids fragile terminal-specific mouse bugs.
  - Keeps grapheme-aware text editing and keyboard-first UX.

