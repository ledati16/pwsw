Refactor Review 2 — Implementation Plan (Revised)

Summary

This document captures the findings from the deep-dive review and defines a prioritized, actionable plan to implement fixes, refactors, and tests. It has been revised after comparing with `refactor_review1.md` and the repository governance (`GEMINI.md`). The plan includes the original issues (blocking PipeWire calls, profile-switch safety, IPC hardening, clippy fixes) plus additional items surfaced by the refactor review: dead UI code removal, config watcher filter, sink selector consolidation, Throbber state encapsulation, editor rename, InputWidget, atomic config writes, and dependency pruning.

Goals

- Make audio activation non-blocking and safe for the Tokio runtime
- Serialize profile switches per-device (prevent races)
- Harden IPC (stale-socket checks, ownership), and correct ListWindows identity handling
- Fix clippy pedantic warnings and TUI refactor oversights
- Implement UI improvements (shared selector widget, EditorState rename, InputWidget, Throbber relocation)
- Add tests and CI enforcement (pedantic clippy, docs requirements)

Phases & Tasks (revised)

Phase 1 — Non-blocking activation (Absolutely recommend)
- Make State::process_event async and await spawn_blocking for PipeWire::activate_sink
- Add tests ensuring worker responsiveness while a slow activation proceeds
- Update daemon::run calls to await process_event

Phase 2 — Per-device serialization (Absolutely recommend)
- Add a static device lock map (OnceLock<HashMap<u32, Arc<Mutex<()>>>>)
- Acquire per-device lock during profile switch (set_device_profile + polling)
- Add unit tests to assert serialization behavior

Phase 3 — IPC & ListWindows correctness + Stale socket hardening (Absolutely recommend)
- Add window id to IPC WindowInfo (optional field) and return ids via State::get_all_windows
- Use ids to map tracked windows in daemon::handle_ipc_request (avoid app_id/title collisions)
- Harden cleanup_stale_socket: confirm path is socket and owner UID == current user before removal (Unix-only checks using MetadataExt)
- Add unit tests for both changes

Phase 4 — Apply refactor_review1 items & TUI cleanup (Highly recommend)
- Remove dead TUI code: execute_pending_daemon_action, update_daemon_state, and pending_daemon_action from App (src/tui/app.rs)
- Fix config watcher: watch only the actual config file (compare event.paths to config_path) rather than any file in the directory (src/daemon.rs)
- Consolidate sink selector rendering into widgets::render_sink_selector (remove duplication from rules.rs & sinks.rs)
- Move ThrobberState from App into RulesScreen struct; update preview usage accordingly
- Rename SimpleEditor → EditorState across TUI (src/tui/editor_state.rs, tests, usages)
- Implement a proper InputWidget (InputWidget<'a>) in src/tui/widgets.rs that implements ratatui::widgets::Widget and replace render_input
- Add Input handling unit tests (simulate key events and editor behavior)
- Implement atomic config writes in Config::save (write tmp file + rename)
- Remove unused dependency tui-popup from Cargo.toml; review tui-logger usage and either keep (if used by TUI) or document/decide to remove

Phase 5 — Clippy, docs, tests, CI (Highly / Lightly recommend)
- Fix clippy pedantic warnings (address the few flagged locations: needless-range-loop, unused variables, dead_code)
- Fix TUI debug timing bug (replace erroneous duration calc with start.elapsed())
- Ensure every public Result-returning function has doc comments (# Errors) per GEMINI.md
- Add/expand tests for: input handling, socket cleanup, ListWindows with duplicate titles, profile-switch concurrency, atomic config writes
- Add CI check to run: cargo fmt, cargo test, cargo clippy --all-targets -- -W clippy::pedantic

Optional / Nice to have
- Per-device metrics for profile switching success/fail/latency
- Small integration harness to simulate slow profile switches
- Revisit tui-logger: fully integrate or remove to avoid zombie dependency

Cross-cutting constraints and decisions (from GEMINI.md)
- All changes must pass pedantic clippy (except the 7 acceptable warnings listed in GEMINI.md)
- Document all public Result-returning functions and any panics
- Avoid expect()/panic!() except documented defensive checks
- Use proper JSON construction for external commands (already in repo; keep it)
- Socket perms must remain 0o600; ensure any created sockets/files follow these rules

Small tweaks added from refactor_review1.md
- Immediately remove `execute_pending_daemon_action`, `update_daemon_state`, and pending_daemon_action field from TUI App (dead code)
- Change config hot-reload watcher to only trigger when config file matches Config::get_config_path() (fix false reloads)
- Swap duplicated sink selector code into common widget (also consolidates arrow/wrapping logic used in help/rules/sinks)
- Move ThrobberState to RulesScreen and make preview animation local to that screen
- Rename SimpleEditor → EditorState and update uses/tests
- Add InputWidget that contains cursor/scroll logic that used to live in render_input
- Implement atomic write in Config::save (write tempfile + rename)

Work Plan, ordering & estimates (updated)
1. Remove dead TUI code; fix config watcher filter, and implement unit tests for watcher (1–2h)
2. Non-blocking activation (process_event async, spawn_blocking) + tests (4–8h)
3. Per-device profile serialization + tests (2–4h)
4. IPC changes (add ids) + stale-socket hardening + tests (3–4h)
5. TUI refactors: shared selector widget, Throbber relocation, rename EditorState, InputWidget + tests (3–6h)
6. Clippy pedantic fixes, docs, CI updates (2–3h)
7. Optional/metrics/integration harness depending on time

Implementation rules
- Make small commits per task with conventional commit messages (see GEMINI.md)
- DO NOT push or open PRs without explicit confirmation
- Run cargo fmt, cargo test, cargo clippy --all-targets -- -W clippy::pedantic before creating each commit
- Update refactor_review_final.md checkboxes for completed substeps when done

Follow-ups
- Decide about tui-logger (remove or finish integration); I recommend keeping it for TUI feature and removing tui-popup.
- If you want, I can start implementing the top-priority items now (begin with dead-code + config watcher), create local commits, and run the test/lint cycle. I will not push until you say "push it".

Would you like me to start with the dead-code + config watcher changes or jump straight to making activation non-blocking?