TEMP TUI REFACTOR PLAN

Purpose
- This temporary file documents the TUI refactor plan and tracks the three implementation phases.
- It's intended to be kept in the repo temporarily while the work progresses, then removed when complete.

Overview
- Goal: Improve the TUI (ratatui + crossterm) for robustness, responsiveness, and consistency while aligning with project standards in `CLAUDE.md`.
- Approach: Implement the work in three phases. Each phase builds on the previous and targets high-impact issues first.

Phase 1 — Safety & Responsiveness (completed)
- Make terminal initialization and cleanup panic-safe and reliable.
  - Added a panic hook that restores terminal state (disable raw mode, leave alternate screen, show cursor) on panic.
- Decouple blocking/background work from the render loop.
  - Spawned a background updater task that polls daemon state, window list, and `PipeWire` sink snapshot on an interval.
  - Uses a tokio `mpsc` channel to send updates into the UI main loop.
- Make daemon control actions asynchronous and non-blocking for the UI.
  - When user issues Start/Stop/Restart, background task executes action and sends result back via channel.
- Introduced a cached `active_sinks` snapshot in `App` for the UI to consume (avoid calling `PipeWire::dump()` from render path).
- Implemented dirty-driven redraw and time-based spinner so UI only redraws on state changes or animation frames.
- Minimal, surgical changes kept behavior stable and tests passing.

Phase 2 — Regex & Render Optimizations (completed)
- Done:
  - Cached compiled `Regex` objects in `RuleEditor` (`compiled_app_id`, `compiled_title` and `*_for` markers). (see `src/tui/screens/rules.rs`)
  - Moved expensive preview matching off the render path into background worker / debouncer; introduced `execute_preview` which supports optional compiled regex caches and runs matching inside `spawn_blocking` with a timeout. (see `src/tui/preview.rs`)
  - Preview forwarder/debouncer collapses rapid preview updates and throttles execution; tests present for forwarder and debouncer. (see `src/tui/mod.rs` and tests in `src/tui/tests`)
  - `compute_display_window` and grapheme-aware textfield helpers are implemented and well-tested (see `src/tui/textfield.rs` and tests).
  - Render code was optimized in places to avoid large temporary String allocations (building `Span` vectors, etc.), and live preview rendering uses cached compiled regexes when available.
  - The mouse experiment was implemented and then removed; the codebase now intentionally omits pointer interactions (mouse code removed from `src/tui/input.rs`).

- Deferred / moved to Phase 3:
  - Further reduce allocations during render: low-ROI micro-optimizations (e.g., tiny string->Span tweaks, Vec reuse) deferred until we have profiling evidence.
  - Ensure compiled-regex compilation occurs eagerly everywhere: largely implemented via `RuleEditor::ensure_compiled()`; remaining `Regex::new` calls are intentional (validation, blocking preview executor) and acceptable for now.
  - Consider replacing per-tick `handle_events` poll with a blocking input thread: non-trivial design change with behavioral risk; defer to Phase 3 if we choose to rework input handling.
  - Add microbenchmarks/runtime instrumentation: we added debug slow-frame logging; deeper instrumentation or CSV logging is deferred unless profiling shows need.

- Work completed in this session:
  - Replaced multiple `format!` allocations in hot render paths with span-based rendering (rules, textfield, help).
  - Added cached padded display strings for the Settings screen (`SettingsScreen.padded_names`) to restore fixed-width alignment without per-frame allocations.
  - Completed a `Regex::new` audit in `src/tui` and updated editor input paths to call `RuleEditor::ensure_compiled()` eagerly on edits/removals so the background preview can reuse cached `Arc<Regex>` instead of compiling on render.
  - Updated the rule save/validate path to prefer using cached compiled regexes when available, falling back to explicit compilation on the explicit save action.
  - Implemented cached, padded sink descriptions in `SinksScreen::display_descs` with `update_display_descs(&[SinkConfig])` to restore alignment without per-frame `format!` allocations. This cache is initialized at `App::new()` and updated whenever sinks are added/edited/deleted or when defaults change.
  - Updated `render_list` in `src/tui/screens/rules.rs` to use a small per-render lookup of padded sink display strings (derived from sinks) so rule sink columns are aligned without per-row formatting allocations.
  - Moved preview match string construction out of the render path into the blocking preview executor (`src/tui/preview.rs`) and replaced `format!` allocations there with direct `String` building.
  - Replaced header tab/title `format!` calls with precomputed `String` builders to avoid format allocations per-render.
  - Replaced per-render Title allocation in rules delete modal with Span-based Line to avoid one `String` allocation per render.
  - Replaced some `to_string()`/`clone()` usages in render paths with `as_str()` references where possible.
  - Ran `cargo test` (all tests passed) and `cargo clippy --all-targets -- -D warnings` (passed).
  - Added debug-only `terminal.draw()` timing instrumentation to log slow frames (>15ms) to stderr in debug builds.
  - Enhanced slow-frame logs to include run-relative timestamp, current screen name, preview pending flag, and window count for easier correlation.

These changes reduce per-frame heap allocations and ensure regex compilation happens during edit events or explicit saves rather than silently during rendering.

Phase 3 — UX, Accessibility & Polishing (pending)
- Improve keyboard editing behavior: left/right, Home/End, delete word, selection where reasonable.
- Add TUI theme mapping and a `--tui-no-color` or `PWSW_TUI_MONOCHROME=1` option for accessibility/term compatibility.
- Add tests for critical helpers and expand documentation comments per `CLAUDE.md`.
- Run full `cargo clippy --all-targets` and `cargo test`, resolve warnings and errors.

Implementation notes
- Changes have been done in small, focused commits (local only). Nothing was pushed without explicit approval.
- The temporary `TEMP_TUI_REFACTOR.md` is kept in the repo while work progresses and will be removed when complete.

Progress
- Phase 1: completed.
- Phase 2: substantially started and several key items implemented (regex caching, background preview matching, timeouts, textfield helpers). Several optimization items remain.
- Phase 3: pending.

Current suggestions to finish Phase 2 (next actions)
1. Audit render hotspots for allocations; add reuse or early exits to avoid work when data unchanged.
2. Make compiled-regex compilation occur eagerly on editor changes so fallback render path can reuse caches rather than compile during render.
3. Add a microbenchmark/test or an instrumentation log around `terminal.draw()` to identify any expensive frames.
4. Optionally move input reading to a blocking thread and forward events to the async loop.

Recent micro-step
- Action A completed: added explicit `RuleEditor::ensure_compiled()` calls across editor mutation paths so compiled regex caches are updated eagerly and included when forwarder sends `PreviewRequest` to background worker.
- Action B completed: swept remaining render allocations and reduced clones/formatting in hot paths.
- Action C completed: added richer debug logs for slow frames including run-relative timestamp, screen name, preview pending, and window count.

If you want, I can now perform another interactive run and analyze the `tui_stderr.log` for correlation and targeted fixes.



Recent micro-step: word-nav helpers and tests
- Implemented grapheme-aware word navigation helpers and SimpleEditor wrappers in `src/tui/editor_helpers.rs` and `src/tui/editor_state.rs`.
- Wired Ctrl+Left/Ctrl+Right/Ctrl+Backspace in `src/tui/input.rs` for Sinks and Rules editors.
- Added unit tests `src/tui/tests/editor_word_nav_tests.rs` (basic + multibyte) and expanded edge-case tests in follow-up commit.
- Ran `cargo test`, `cargo clippy --all-targets -- -D warnings`, and `cargo fmt` successfully after these changes.

- Next small steps:
  1. Add integration tests that simulate key events to validate end-to-end input wiring for editors. (Added `src/tui/tests/input_integration_tests.rs` and `pub(crate) simulate_key_event` wrapper in `src/tui/input.rs`.)
  2. Add accessibility/theme toggle and a short docs entry.
  3. Run a lightweight `terminal.draw()` timing run to validate no regressions in render latency.

Phase 3 UX Polish - Recent Completion (2025-12-14)
- Implemented consistent focus indicators using border highlighting instead of left-bar approach
- Standardized modal sizes using constants (SMALL/MEDIUM/LARGE/DROPDOWN/HELP) across all screens
- Unified keybind notation in block titles to `[key]action` format for consistency
- Replaced sink selector in rules editor with button-like widget for better discoverability
- Standardized in-modal help text using `modal_help_line()` helper to avoid allocations
- Fixed help screen spacing with fixed-width key column (format! acceptable - help renders on-demand)
- Applied border-based focus styling to checkboxes/toggles for visual consistency
- Updated layout constraints from `Length(2)` to `Length(3)` for bordered fields
- All changes preserve Phase 2 performance optimizations (no `format!` in hot render paths)
- Ran `cargo test` (59 tests passed), `cargo clippy --all-targets` (clean)
- Created reusable helpers in `src/tui/widgets.rs`: `modal_size`, `focus_border_style()`, `render_selector_button()`, `modal_help_line()`

Files modified:
- `src/tui/widgets.rs`: Added UX helper functions and modal size constants
- `src/tui/textfield.rs`: Switched from left-bar to border-based focus indicator
- `src/tui/screens/sinks.rs`: Updated modal sizes, keybinds, help text, checkbox focus, constraints
- `src/tui/screens/rules.rs`: Updated modal sizes, keybinds, sink selector button, help text, notify toggle focus, constraints
- `src/tui/screens/settings.rs`: Updated modal size, keybind notation
- `src/tui/screens/dashboard.rs`: Updated keybind notation
- `src/tui/screens/help.rs`: Fixed spacing with fixed-width padding

Phase 3 UX Polish - Additional Improvements (2025-12-14)
- Fixed help overlay background bleed-through using `Clear` widget
- Added spacing between sink description and status indicator in sinks list
- Implemented colored boolean toggles in settings: green ✓ for enabled, red ✗ for disabled
- Styled [unsaved] indicator in yellow bold for better visibility
- Fixed dashboard render order issue (block now renders before content using Margin)
- Implemented full sink selector modal for adding sinks:
  - Shows both active sinks and profile sinks (requiring profile switching)
  - Smart text truncation: descriptions truncate from start, node names from end (shows distinguishing suffix)
  - Manual navigation with ↑/↓ keys, Enter to select, Esc to cancel
  - Populates both name and description fields when sink selected
  - Context-sensitive help hint: "Tip: Press Enter on Node Name to select from available sinks"
- Updated App state to store full sink data (`active_sink_list`, `profile_sink_list`)
- Modified background worker to fetch both active and profile sinks via `SinksData` message
- All changes maintain Phase 2 performance optimizations (no allocations in hot paths)
- Ran `cargo test` (all passed), `cargo clippy --all-targets` (clean)

Files modified:
- `src/tui/screens/help.rs`: Added `Clear` widget import and usage
- `src/tui/screens/sinks.rs`: Added spacing, sink selector modal with smart truncation, context help
- `src/tui/screens/settings.rs`: Colored boolean toggles (green/red)
- `src/tui/screens/dashboard.rs`: Fixed render order using `Margin`
- `src/tui/mod.rs`: Styled [unsaved] indicator, updated background worker for full sink data
- `src/tui/app.rs`: Added `active_sink_list`, `profile_sink_list`, `SinksData` variant
- `src/tui/input.rs`: Added SelectSink mode handling and Enter-to-select on name field

Phase 3 UX Polish - TUI Revamp & Scrolling (2025-12-15)
- Revamped Dashboard, Sinks, and Rules screens with modern, grid-based layouts and Table widgets.
- Replaced raw text lists with `Table` widgets in Sinks and Rules screens for better column alignment and readability.
- Added `Scrollbar` widgets to Sinks, Rules, Settings lists, and Sink Selector dropdowns.
- Implemented proper scrolling logic using `TableState` and `ListState` in App/Screens structs to maintain state across renders.
- Added "smart" modal resizing: Help text in add/edit modals is hidden if window height is too small (<20/25 rows).
- Fixed Sink Selector dropdown selection logic:
  - Removed duplicate `> ` visual indicators to fix double-selection glitch.
  - Implemented correct `visual_index` calculation to account for headers/spacers in the list, ensuring the highlighted row matches logical selection.
- Fixed Help Screen scrolling:
  - Rewired input handling to manipulate `TableState` offset directly for true viewport scrolling (instead of selection-based).
  - Exposed `get_help_row_count` to clamp scrolling correctly.
  - Removed confusing selection highlight in Help screen (pure view mode).
- Verified with `cargo test` and `cargo clippy`.

Files modified:
- `src/tui/screens/dashboard.rs`, `sinks.rs`, `rules.rs`, `settings.rs`, `help.rs`: UI layout & widget updates.
- `src/tui/app.rs`: State struct updates.
- `src/tui/input.rs`: Input handling logic updates.
- `src/tui/mod.rs`: Render function signature updates.

Next steps:
- Accessibility/theme toggle.
- Final polish and documentation.

-- End of TEMP_TUI_REFACTOR.md