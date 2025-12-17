# Refactor Plan: PWSW Modernization

This document tracks the progress of the PWSW codebase refactoring. The goal is to modernize the architecture, reduce boilerplate, and improve maintainability by leveraging ecosystem crates (`tui-input`, `notify`, `serde_regex`, etc.).

## Progress Summary

- [x] Phase 1: Foundation & Safety
- [x] Phase 2: Daemon Modernization
- [x] Phase 3: TUI Infrastructure (Async & Input)
- [x] Phase 4: TUI Polish & Features

---

## Phase 1: Foundation & Safety
**Goal:** Set up the environment, dependencies, and safety nets before touching core logic.

- [x] **1.1 Add Dependencies**
    - `cargo add tui-input tui-popup throbber-widgets-tui tui-logger color-eyre notify serde_regex crossterm --features crossterm/event-stream`
    - Verified.
- [x] **1.2 Implement `color-eyre`**
    - Replaced custom panic hooks in `src/bin/pwsw.rs` and `src/tui/mod.rs` (wrapper).
- [x] **1.3 Setup `tui-logger`**
    - Initialized in `src/bin/pwsw.rs`.

## Phase 2: Daemon Modernization
**Goal:** Simplify configuration handling and enable hot-reloading.

- [x] **2.1 Simplify Config with `serde_regex`**
    - Refactored `src/config.rs`.
- [x] **2.2 Implement Config Hot-Reloading**
    - Added file watcher in `src/daemon.rs`.
    - Added `reload_config` to `State`.

## Phase 3: TUI Infrastructure (The Deep Dive)
**Goal:** Rewrite the TUI event loop to be async and replace the fragile manual text editor.

- [x] **3.1 Async Event Loop (`crossterm::EventStream`)**
    - Refactored `run_app` in `src/tui/mod.rs`.
- [x] **3.2 Integrate `tui-input` (Backend)**
    - Refactored `SimpleEditor` in `src/tui/editor_state.rs` to wrap `tui_input::Input`.
- [x] **3.3 Cleanup Old Input Code**
    - Deleted `src/tui/textfield.rs` and `src/tui/editor_helpers.rs`.
    - Removed `unicode-segmentation` dependency.
- [x] **3.4 Refactor Input Handling**
    - Rewrote `src/tui/input.rs` to use `tui-input` event handling.
    - Updated `rules.rs` and `sinks.rs` to use new editor API.

## Phase 4: TUI Polish & Features
**Goal:** Visually polish the UI.

- [x] **4.1 Modernize Widgets (`throbber-widgets-tui`)**
    - Updated `src/tui/screens/rules.rs` to use `Throbber` for the spinner.
    - Note: `tui-popup` integration deferred as `centered_modal` is sufficient.
- [ ] **4.2 Implement Logs Tab**
    - Skipped for this iteration to stabilize core refactor first.
- [x] **4.3 Final Code Review & Cleanup**
    - `cargo check` passes.
    - `cargo clippy` has minimal warnings.

---

## Conclusion
The codebase has been significantly modernized. The manual text input engine has been replaced with `tui-input`, the event loop is now async, and the daemon supports config hot-reloading.