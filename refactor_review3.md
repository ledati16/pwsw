Refactor Review 3 — Final Implementation Plan

Purpose

This final plan reconciles:
- The deep-dive findings (refactor_review2.md),
- The refactor oversight list (refactor_review1.md), and
- The current repository state (compare to upstream `origin/revamp`) and refactor roadmap (refactor_review_final.md).

It prioritizes safety, correctness, and maintainability and includes concrete file-level actions, tests, and commit guidance. Follow the "Work Plan & Ordering" section when implementing; make small commits, run the test/lint cycle after each change, and do NOT push until you get explicit approval.

Executive summary of additions since refactor_review2

- Many TUI refactors (async event loop, input, editor improvements, cached regexes) are already implemented (see recent revamp commits). Update tasks to avoid duplicating completed work and to focus on missing/remaining items.
- The daemon now watches the config directory for hot-reload (src/daemon.rs); this must be hardened to watch only the actual config file (filter events) and to debounce better.
- State already tracks windows by id (State.active_windows / all_windows), but IPC responses (daemon::handle_ipc_request) still build lookup maps by (app_id, title) → fix to include and use window ids so ListWindows/TestRule are correct for duplicate titles.
- Several local working-tree changes are present (uncommitted). Consolidate and commit these changes in small, focused commits as part of executing the plan.

Priority-rank tasks (Absolutely / Highly / Light / Optional)

Absolutely recommend (Fix immediately, low-risk/high-impact)

1) Make sink activation non-blocking and async-safe (src/state.rs:process_event:219, switch_audio at src/state.rs:97)
   - Make State::process_event async: `pub async fn process_event(&mut self, event: WindowEvent) -> Result<()>`.
   - Update daemon::run to `.await` the call: `if let Some(event) = result { if let Err(e) = state.process_event(event).await { ... } }` (daemon.rs: ~211).
   - Inside State, perform blocking PipeWire work using `tokio::task::spawn_blocking` (or a dedicated blocking worker). Specifically, call PipeWire::activate_sink inside spawn_blocking and await the JoinHandle.
   - Update switch_audio/switch_to_target to await activation result and to only update state.current_sink_name on success.
   - Tests: convert relevant unit tests to async #[tokio::test], and add a test that simulates a slow activation (using a test-only stub or feature) to verify other events are processed while activation runs.
   - Files: src/state.rs:process_event (line ~219), src/daemon.rs: where process_event is called (line ~211).

2) Add per-device serialization for profile switching (src/pipewire.rs: activate_sink at ~line 499)
   - Add a static lock table keyed by device_id: `static DEVICE_LOCKS: OnceLock<Mutex<HashMap<u32, Arc<std::sync::Mutex<()>>>>>` (or similar). Use std::sync types so it's usable in spawn_blocking.
   - In activate_sink (before calling set_device_profile and polling), acquire the device mutex for the device id and hold it for the whole profile switch + polling + set_default work. This prevents concurrent profile switches on the same device.
   - Tests: add a unit test simulating concurrent profile switch requests for same device and verify serialization (test-only stub for set_device_profile / sleep inside a critical section).
   - Files: src/pipewire.rs: find_profile_sink (line ~82), activate_sink (line ~499).

3) IPC window identity correctness (src/daemon.rs: handle_ipc_request, src/ipc.rs: WindowInfo)
   - Add `id: Option<u64>` to ipc::WindowInfo (safe backward-compatible JSON extension).
   - Change State::get_all_windows() to return Vec<(u64, String, String)> (id, app_id, title), and use ids when building IPC responses.
   - Fix daemon::handle_ipc_request ListWindows code to use ids for building the tracked map and WindowInfo; avoid relying on (app_id, title) as unique key (this was causing false positives when multiple windows share app_id/title).
   - Update related CLI output where necessary (commands::list-windows) and add tests that create two windows with same app_id/title but distinct ids and ensure tracked/untacked status is correct.
   - Files: src/ipc.rs WindowInfo (line ~76), src/daemon.rs (handle_ipc_request at ~line 299), src/state.rs (get_all_windows at ~line 52).

4) Harden stale socket cleanup (src/ipc.rs: cleanup_stale_socket, lines ~46–82)
   - Before removing the socket file, verify it's a socket and owned by the current user: use metadata.file_type().is_socket() (Unix) and metadata.uid() == users::get_current_uid() (std::os::unix::fs::MetadataExt::uid())
   - If checks fail, warn and do not remove; add a unit test that creates a non-socket file and a socket and asserts behavior.
   - Files: src/ipc.rs: cleanup_stale_socket (line ~46)..

5) Validate PipeWire tool existence robustly (src/pipewire.rs: validate_tools at line ~204)
   - Check process exit status: prefer `Command::new(tool).arg("--version").status()` and check `.success()`, not only `Err` from spawn.
   - Optionally use `which` crate for cleaner behavior.
   - Tests: Add unit tests to ensure validate_tools returns helpful errors when tools are missing (mocking via PATH manipulation is acceptable in tests).

Highly recommend (Important, next-priority)

6) Filter config file watcher to actual config path (src/daemon.rs: watcher block added at ~line 173)
   - Current code watches the config directory and reacts to any create/modify event. Replace with a filter that checks event.paths contains the config file (Config::get_config_path()) before sending reload notification.
   - Add stronger debouncing: ignore repeated events for a short window (channel with capacity 1 is OK; improve by using tokio::time::sleep debounce or coalesce multiple events into one reload attempt).
   - Tests: Unit test for watcher filter using notify and temporary file operations.

7) Implement atomic config saves (src/config.rs: save at ~line 220)
   - Write to a temp file in the same directory, set file mode to 0o600 (Unix), then fs::rename to final path (atomic on POSIX). Use tempfile crate or write to path.with_extension("tmp") and rename.
   - Tests: Unit test verifying save() writes correctly and is atomic semantics (ensure not partial file left if write fails).

8) Fix TUI debug timing bug and clippy pedantic issues (src/tui/mod.rs lines ~392–412)
   - Replace incorrect debug timing calc with `elapsed.as_millis()` and remove strange `duration_since` call.
   - Run `cargo clippy --all-targets -- -W clippy::pedantic` and fix all warnings (observe the 7 acceptable warnings per GEMINI.md; do not introduce new ones).

Lightly recommend (Polish)

9) Remove dead TUI methods & fields (src/tui/app.rs)
   - Remove execute_pending_daemon_action, update_daemon_state, and pending_daemon_action if they are unused (they are flagged as #[allow(dead_code)] currently). Confirm no dependent code uses them.
   - Tests: Run UI integration tests to ensure no regression.

10) Sink selector consolidation & widget improvements (src/tui/screens/sinks.rs and src/tui/screens/rules.rs)
    - Consolidate duplicated logic for render_sink_selector into a shared widget in src/tui/widgets.rs.
    - Replace duplicated arrow/viewport calculations with shared helper.
    - Tests: Visual behavior unchanged (unit tests for arrow calculation exist already—expand where necessary).

11) Consider moving ThrobberState ownership into RulesScreen (src/tui/app.rs: throbber_state present)
    - Small refactor for encapsulation and easier testing. Low-risk.

Optional / Nice-to-have

12) Expose PROFILE_SWITCH parameters to config (either settings or env vars)
    - Let users increase PROFILE_SWITCH_DELAY_MS or PROFILE_SWITCH_MAX_RETRIES via configuration or env var when devices take longer to instantiate.
    - Implement sensible bounds and document behavior in README and CLAUDE.md.

13) Remove unused dependency tui-popup
    - Verify it is unused; if so remove it from Cargo.toml to keep deps minimal.

14) Rename SimpleEditor → EditorState (cosmetic)
    - Low-priority; big rename touches many files—optional if it improves API clarity.

15) Add integration tests for IPC and socket behavior (unix-only)
    - Create integration tests under tests/ that spawn the daemon in background and exercise IPC endpoints, socket cleanup, and health-check semantics.

Cross-cutting considerations & compatibility

- Conformance to project policies (GEMINI.md): Ensure that every public function returning Result has a doc comment with `# Errors`, add `# Panics` where `expect()`/`unwrap()` is present, and fix clippy pedantic warnings before merging.
- IPC compatibility: Adding `id: Option<u64>` to WindowInfo is backward-compatible (consumers ignoring unknown fields will be fine). Document change in release notes.
- Tests: Many unit tests in TUI and core components already exist—update tests that depend on changed signatures (process_event) to be async (#[tokio::test]).
- Atomic writes: Be careful with perms; create files owned by current user and set permissions to 0o600.

Work plan & ordering (with estimates)

Phase A — Safety & correctness (Day 1–2)
- Implement non-blocking activation + tests (4–8h)
- Per-device profile serialization + tests (2–4h)
- Harden cleanup_stale_socket + tests (1–2h)
- Fix validate_tools (0.5–1h)

Phase B — IPC correctness & config hot-reload (Day 2)
- Add id to WindowInfo, update ListWindows/TestRule response generation + tests (1–3h)
- Filter config watcher to specific path and debounce properly + tests (1–2h)
- Implement atomic config writes (1–2h)

Phase C — TUI cleanup and clippy passes (Day 3)
- Remove dead TUI code & minor refactors (0.5–1h)
- Consolidate sink selector widget and minor TUI refactors (1–3h)
- Fix debug timing bug and address remaining clippy pedantic warnings (1–2h)

Phase D — Tests/CI/Docs (Day 3–4)
- Add integration tests (1–3h)
- Update CLAUDE.md/GEMINI.md/refactor_review_final.md to reflect completed items (0.5–1h)
- Update CI to run pedantic clippy and test suite (0.5–1h)

Commit & PR guidance

- Make small, focussed commits and run the test/lint cycle before each commit.
- Suggested commit sequence (one commit per bullet):
  - refactor(state): make process_event async and run sink activation in spawn_blocking
  - fix(pipewire): add per-device locks for profile switching
  - fix(ipc): include window id in WindowInfo and use id-based matching
  - fix(ipc): harden cleanup_stale_socket to check filetype and ownership
  - fix(pipewire): validate_tools checks exit status
  - fix(daemon): config watcher filters only config file
  - fix(config): atomic save via temp file + set 0o600
  - refactor(tui): remove dead methods and unused fields
  - feat(tui): consolidate sink selector widget
  - chore: clippy pedantic fixes and test updates
- Run before committing: cargo fmt; cargo test; cargo clippy --all-targets -- -W clippy::pedantic
- DO NOT push to remote until you receive explicit approval. When ready to open a PR, update refactor_review_final.md to mark completed checkboxes and include a short PR description referencing this review file.

Testing checklist

- Unit tests: run `cargo test` and update tests as needed (convert to async where signatures changed).
- Clippy: run `cargo clippy --all-targets -- -W clippy::pedantic` and fix all but the 7 acceptable warnings noted in GEMINI.md.
- Manual checks: Start daemon (`cargo run -- daemon --foreground`) and exercise `pwsw list-windows`, `pwsw list-sinks`, `pwsw test-rule`, and TUI (`cargo run -- tui`). Verify config hot-reload and socket cleanup behaviors manually.

Final notes / risks

- Changing State::process_event signature to async will require careful test updates and may slightly change the daemon execution model—tests must be updated before committing.
- Profile switching paths are inherently brittle due to device differences; adding logging and optional config knobs for delays/retries will make support easier.
- IPC changes (adding id fields) are backwards-compatible but MUST be documented for consumers.

Next step (recommendation)

Start with Phase A (non-blocking activation + per-device lock + tests). I can implement Phase A now, run the test and clippy cycles, and create the commits locally for review. I will not push or open a PR without your explicit approval.

Do you want me to begin with Phase A now?