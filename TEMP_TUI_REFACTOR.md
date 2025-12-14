TEMP TUI REFACTOR PLAN

Purpose
- This temporary file documents the TUI refactor plan and tracks the three implementation phases.
- It's intended to be kept in the repo temporarily while the work progresses, then removed when complete.

Overview
- Goal: Improve the TUI (ratatui + crossterm) for robustness, responsiveness, and consistency while aligning with project standards in `CLAUDE.md`.
- Approach: Implement the work in three phases. Each phase builds on the previous and targets high-impact issues first.

Phase 1 — Safety & Responsiveness (current focus)
- Make terminal initialization and cleanup panic-safe and reliable.
  - Add a panic hook that restores terminal state (disable raw mode, leave alternate screen, show cursor) on panic.
- Decouple blocking/background work from the render loop.
  - Spawn a background updater task that polls daemon state, window list, and PipeWire sink snapshot on an interval.
  - Use a tokio `mpsc` channel to send updates into the UI main loop.
- Make daemon control actions asynchronous and non-blocking for the UI.
  - When user issues Start/Stop/Restart, spawn a background task and send result back via channel.
- Introduce a cached `active_sinks` snapshot in `App` for the UI to consume (avoid calling `PipeWire::dump()` from render path).
- Minimal, surgical changes to achieve the above without changing major UI behavior.

Phase 2 — Regex & Render Optimizations
- Cache compiled `Regex` objects in `RuleEditor` to avoid re-compiling on each render.
- Move heavy matching (live-preview) off the render path if necessary (background worker) and add timeouts/limits to protect UI from pathological regex or large window counts.
- Reduce allocations during render: only rebuild List/Line structures when underlying data changes or when area size changes.
- Improve input field rendering to use terminal cursor APIs rather than appending a block glyph. Add clipping and left/right editing support.

Phase 3 — UX, Accessibility & Polishing
- Improve keyboard editing behavior: left/right, Home/End, delete word, selection where reasonable.
- Add mouse support and hit testing for clickable widgets (popups, lists).
- Add TUI theme mapping and a `--tui-no-color` or `PWSW_TUI_MONOCHROME=1` option for accessibility/term compatibility.
- Add tests for critical helpers (e.g., `centered_rect`, selection clamping) and expand documentation comments to satisfy `CLAUDE.md`.
- Run full `cargo clippy --all-targets` and `cargo test`, resolve new warnings and errors.

Implementation notes
- Changes will be done in small, focused commits (local only). I will not push to remote without explicit approval.
- The temporary `TEMP_TUI_REFACTOR.md` will be updated as work progresses and removed at the end.

Progress
- Phase 1: completed (panic hook + panic-safe cleanup, background updater, non-blocking daemon actions, active_sinks snapshot).
- Phase 2/3: pending until Phase 1 stabilizes.

Contact
- If you'd like a different order of priorities (e.g., editing UX first), tell me and I'll adjust.

-- End of TEMP_TUI_REFACTOR.md
