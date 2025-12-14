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

Phase 2 — Regex & Render Optimizations (in progress)
- Done:
  - Cached compiled `Regex` objects in `RuleEditor` (`compiled_app_id`, `compiled_title` and `*_for` markers). (see `src/tui/screens/rules.rs`)
  - Moved expensive preview matching off the render path into background worker / debouncer; introduced `execute_preview` which supports optional compiled regex caches and runs matching inside `spawn_blocking` with a timeout. (see `src/tui/preview.rs`)
  - Preview forwarder/debouncer collapses rapid preview updates and throttles execution; tests present for forwarder and debouncer. (see `src/tui/mod.rs` and tests in `src/tui/tests`)
  - `compute_display_window` and grapheme-aware textfield helpers are implemented and well-tested (see `src/tui/textfield.rs` and tests).
  - Render code was optimized in places to avoid large temporary String allocations (building `Span` vectors, etc.), and live preview rendering uses cached compiled regexes when available.
  - The mouse experiment was implemented and then removed; the codebase now intentionally omits pointer interactions (mouse code removed from `src/tui/input.rs`).

- Remaining / suggested for Phase 2:
  - Further reduce allocations during render: audit hot render paths (`render_rules`, `render_sinks`) and avoid reallocating `Vec`/`String` on every draw when data unchanged. Use small object reuse patterns where helpful.
  - Ensure all render-time regex work reuses compiled caches when possible (some fallback code still compiles per-render when cache missing; consider moving compilation earlier on edit events to ensure compiled regex is available for preview fallback).
  - Consider replacing per-tick `handle_events` poll with a blocking input thread to decouple input immediately from the tick cadence (optional; current approach is acceptable with dirty redraws).
  - Add a couple of microbenchmarks or runtime instrumentation to spot expensive renders while interacting.

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

If you want, I can start with action (1) and scan the render functions to propose minimal, surgical optimizations. Otherwise tell me which Phase 2 item you'd like me to pick up next.

-- End of TEMP_TUI_REFACTOR.md
