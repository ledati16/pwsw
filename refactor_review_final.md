Refactor Review — Final Implementation Plan (with progress checkboxes)

Purpose

This final plan reconciles:
- The deep-dive findings (refactor_review2.md),
- The refactor oversight list (refactor_review1.md), and
- The current repository state (compare to upstream `origin/revamp`) and refactor roadmap (refactor_review_final.md).

It prioritizes safety, correctness, and maintainability and includes concrete file-level actions, tests, and commit guidance. Follow the "Work Plan & Ordering" section when implementing; make small commits, run the test/lint cycle after each change, and do NOT push until you get explicit approval.

Executive summary of additions since refactor_review2

- Many TUI refactors (async event loop, input, editor improvements, cached regexes) are already implemented (see recent revamp commits). Update tasks to avoid duplicating completed work and to focus on missing/remaining items.
- The daemon now watches the config directory for hot-reload (src/daemon.rs); this must be hardened to watch only the actual config file (filter events) and to debounce better.
- State already tracks windows by id (State.active_windows / all_windows), and IPC responses now include ids for correctness.

Status summary (completed so far)

- [x] Phase A: non-blocking activation (spawn_blocking) implemented
- [x] Phase A: per-device profile serialization implemented
- [x] IPC: `WindowInfo.id: Option<u64>` added/used in ListWindows/TestRule
- [x] PipeWire: validate_tools now checks process exit status
- [x] GEMINI.md updated and copied to AGENTS.md

Priority-rank tasks (Absolutely / Highly / Light / Optional)

Absolutely recommend (Fix immediately, low-risk/high-impact)

- [x] 1) Make sink activation non-blocking and async-safe (src/state.rs:process_event:219, switch_audio at src/state.rs:97)
   - Make State::process_event async: `pub async fn process_event(&mut self, event: WindowEvent) -> Result<()>`.
   - Update daemon::run to `.await` the call: `if let Some(event) = result { if let Err(e) = state.process_event(event).await { ... } }` (daemon.rs: ~211).
   - Inside State, perform blocking PipeWire work using `tokio::task::spawn_blocking` (or a dedicated blocking worker). Specifically, call PipeWire::activate_sink inside spawn_blocking and await the JoinHandle.
   - Update switch_audio/switch_to_target to await activation result and to only update state.current_sink_name on success.
   - Tests: convert relevant unit tests to async #[tokio::test], and add a test that simulates a slow activation (using a test-only stub or feature) to verify other events are processed while activation runs. **(Slow activation test is pending)**
   - Files: src/state.rs:process_event (line ~219), src/daemon.rs: where process_event is called (line ~211).

- [x] 2) Add per-device serialization for profile switching (src/pipewire.rs: activate_sink at ~line 499)
   - Add a static lock table keyed by device_id: `static DEVICE_LOCKS: OnceLock<Mutex<HashMap<u32, Arc<std::sync::Mutex<()>>>>>` (or similar). Use std::sync types so it's usable in spawn_blocking.
   - In activate_sink (before calling set_device_profile and polling), acquire the device mutex for the device id and hold it for the whole profile switch + polling + set_default work. This prevents concurrent profile switches on the same device.
   - Tests: add a unit test simulating concurrent profile switch requests for same device and verify serialization (test-only stub for set_device_profile / sleep inside a critical section).
   - Files: src/pipewire.rs: find_profile_sink (line ~82), activate_sink (line ~499).

- [x] 3) IPC window identity correctness (src/daemon.rs: handle_ipc_request, src/ipc.rs: WindowInfo)
   - Add `id: Option<u64>` to ipc::WindowInfo (safe backward-compatible JSON extension).
   - Change State::get_all_windows() to return Vec<(u64, String, String)> (id, app_id, title), and use ids when building IPC responses.
   - Fix daemon::handle_ipc_request ListWindows code to use ids for building the tracked map and WindowInfo; avoid relying on (app_id, title) as unique key (this was causing false positives when multiple windows share app_id/title).
   - Update related CLI output where necessary (commands::list-windows) and add tests that create two windows with same app_id/title but distinct ids and ensure tracked/untacked status is correct.
   - Files: src/ipc.rs WindowInfo (line ~76), src/daemon.rs (handle_ipc_request at ~line 299), src/state.rs (get_all_windows at ~line 52).

- [x] 4) Harden stale socket cleanup (src/ipc.rs: cleanup_stale_socket, lines ~46–82)
   - Before removing the socket file, verify it's a socket and owned by the current user: use metadata.file_type().is_socket() (Unix) and metadata.uid() == users::get_current_uid() (std::os::unix::fs::MetadataExt::uid())
   - If checks fail, warn and do not remove; added unit tests that create a non-socket file, an active socket, and a stale socket and assert expected behavior.
   - Files: src/ipc.rs: cleanup_stale_socket (line ~46)..

- [x] 5) Validate PipeWire tool existence robustly (src/pipewire.rs: validate_tools at line ~204)
   - Check process exit status: prefer `Command::new(tool).arg("--version").status()` and check `.success()`, not only `Err` from spawn.
   - Optionally use `which` crate for cleaner behavior.
   - Tests: Add unit tests to ensure validate_tools returns helpful errors when tools are missing (mocking via PATH manipulation is acceptable in tests).

Highly recommend (Important, next-priority)

- [x] 6) Filter config file watcher to actual config path (src/daemon.rs: watcher block added at ~line 173)
   - Current code watches the config directory and reacts to any create/modify event. Replaced with a filter that checks event.paths contains the config file (Config::get_config_path()) before sending reload notification.
   - Added non-blocking `try_send` to avoid blocking the watcher and coalesce rapid events (basic debounce).
   - Tests: Unit test for watcher filter using notify and temporary file operations.

- [x] 7) Implement atomic config saves (src/config.rs: save at ~line 220)
   - Write to a temp file in the same directory, set file mode to 0o600 (Unix), then fs::rename to final path (atomic on POSIX). Use tempfile crate or write to path.with_extension("tmp") and rename.
   - Tests: Unit test verifying save() writes correctly and is atomic semantics (ensure not partial file left if write fails).

- [ ] 8) Fix TUI debug timing bug and clippy pedantic issues (src/tui/mod.rs lines ~392–412)
   - Replace incorrect debug timing calc with `elapsed.as_millis()` and remove strange `duration_since` call.
   - Run `cargo clippy --all-targets -- -W clippy::pedantic` and fix all warnings (observe the 7 acceptable warnings per GEMINI.md; do not introduce new ones).

Lightly recommend (Polish)

- [ ] 9) Remove dead TUI methods & fields (src/tui/app.rs)
   - Remove execute_pending_daemon_action, update_daemon_state, and pending_daemon_action if they are unused (they are flagged as #[allow(dead_code)] currently). Confirm no dependent code uses them.
   - Tests: Run UI integration tests to ensure no regression.

- [ ] 10) Sink selector consolidation & widget improvements (src/tui/screens/sinks.rs and src/tui/screens/rules.rs)
   - Consolidate duplicated logic for render_sink_selector into a shared widget in src/tui/widgets.rs.
   - Replace duplicated arrow/viewport calculations with shared helper.
   - Tests: Visual behavior unchanged (unit tests for arrow calculation exist already—expand where necessary).

- [ ] 11) Consider moving ThrobberState ownership into RulesScreen (src/tui/app.rs: throbber_state present)
   - Small refactor for encapsulation and easier testing. Low-risk.

Optional / Nice-to-have

- [ ] 12) Expose PROFILE_SWITCH parameters to config (either settings or env vars)
- [ ] 13) Remove unused dependency tui-popup
- [ ] 14) Rename SimpleEditor → EditorState (cosmetic)
- [ ] 15) Add integration tests for IPC and socket behavior (unix-only)

Cross-cutting considerations & compatibility

- Conformance to project policies (GEMINI.md): Ensure that every public function returning Result has a doc comment with `# Errors`, add `# Panics` where `expect()`/`unwrap()` is present, and fix clippy pedantic warnings before merging.
- IPC compatibility: Adding `id: Option<u64>` to WindowInfo is backward-compatible (consumers ignoring unknown fields will be fine). Document change in release notes.
- Tests: Many unit tests in TUI and core components already exist—update tests that depend on changed signatures (process_event) to be async (#[tokio::test]).
- Atomic writes: Be careful with perms; create files owned by current user and set permissions to 0o600.

Work plan & ordering (with estimates)

Phase A — Safety & correctness (Day 1–2)

- [x] Implement non-blocking activation + tests (4–8h)
- [x] Per-device profile serialization + tests (2–4h)
- [x] Harden cleanup_stale_socket + tests (1–2h)
- [x] Fix validate_tools (0.5–1h)

Phase B — IPC correctness & config hot-reload (Day 2)

- [x] Add id to WindowInfo, update ListWindows/TestRule response generation + tests (1–3h)
- [x] Filter config watcher to specific path and debounce properly + tests (1–2h)
- [x] Implement atomic config writes (1–2h)

Phase C — TUI cleanup and clippy passes (Day 3–5)

- [ ] Remove dead TUI code & minor refactors (0.5–1h)
- [ ] Consolidate sink selector widget and minor TUI refactors (1–3h)
- [ ] Fix debug timing bug and address remaining clippy pedantic warnings (1–2h)


Phase C.1 — Incremental TUI Refactor Plan (sub-phase)

Goal: iteratively remove pedantic clippy warnings and improve maintainability by extracting small, testable helpers and consolidating duplicated widgets, while keeping changes minimal and reversible. Follow the per-step verification (tests + clippy + optional manual UI check) and make one small commit per completed step.

Steps:

- [x] C.1.1 Identify targets and capture baseline
  - Action: run and save `cargo clippy --all-targets -- -W clippy::pedantic` output and `rg "clippy::" -n src/tui || true` to list flagged locations; baseline captured in `tmp/tui-clippy-baseline.txt`.
  - Verification: baseline saved in `tmp/tui-clippy-baseline.txt` (commit `b6e3d32`).

- [x] C.1.2 Extract pure logic from rendering (low-risk, high-value)
  - Targets: `render_live_preview` in `src/tui/screens/rules.rs` (extract preview-building/matching logic), `render_sink_selector` in `src/tui/screens/sinks.rs` (extract truncation and visual line calculations).
  - Action: created `match_windows_with_compiled_count` and `build_preview_lines_from_strings` in `src/tui/preview.rs`; added `truncate_desc`, `truncate_node_name`, and `compute_visual_line_counts` in `src/tui/widgets.rs`. Updated `render_live_preview` and `render_sink_selector` to use the helpers. Added unit tests.
  - Verification: unit tests added; `cargo test` passes; `cargo clippy --all-targets -- -W clippy::pedantic` passes for affected areas. Commit: `6efcdfa`.

- [x] C.1.3 Move items/declarations out of the middle of functions
  - Action: moved `use`/`const`/type alias declarations out of function bodies where Clippy flagged `items_after_statements`; preferred module-level aliases for widely used items (e.g., animation constants, type aliases).
  - Verification: ran `cargo clippy --all-targets -- -W clippy::pedantic` and verified there are no remaining `items_after_statements` warnings; see `tmp/tui-clippy-baseline.txt` for the prior baseline.

- [x] C.1.4 Consolidate sink selector rendering into a shared widget
  - Action: created `src/tui/widgets.rs` helpers `compute_has_above_below` and `render_scroll_arrows`; replaced duplicated arrow/viewport code in `src/tui/screens/sinks.rs` and `src/tui/screens/rules.rs` to use the shared helpers.
  - Verification: `cargo test` passes; `cargo clippy --all-targets -- -W clippy::pedantic` is clean; visual behavior preserved. Commit: `5f6508e`.

- [ ] C.1.5 Remove dead/unneeded code and simplify structs
  - Action: remove or consolidate flagged dead code (e.g., unused methods/fields in `src/tui/app.rs`) after confirming no callers exist (use `rg`/`git grep` to verify). Replace `#[allow(dead_code)]` with actual removal where safe.
  - Verification: code compiles and tests pass; small commit with a clear message listing removed symbols.

- [ ] C.1.6 Iterate pedantic Clippy fixes and documentation
  - Action: Re-run `cargo clippy --all-targets -- -W clippy::pedantic` after the above steps. For remaining warnings, prefer refactor or micro-fixes (merge match arms, remove unnecessary clones, add small helper functions) rather than adding new `#[allow(...)]` attributes. Update public API docs for `# Errors` and `# Panics` where needed.
  - Verification: clippy output reduced to the project's agreed allowable pedantic warnings (documented in GEMINI.md). All tests pass.

- [ ] C.1.7 Final consolidation and cleanup
  - Action: Remove temporary `#[allow(...)]` attributes added earlier where the underlying cause has been fixed. Ensure each remaining allow is justified in a code comment (link back to an issue or design note if necessary).
  - Verification: final `cargo clippy --all-targets -- -W clippy::pedantic` shows only the documented allowable warnings; update `refactor_review_final.md` to mark sub-steps complete.

Per-step commit guidance

- Make one commit per sub-step (C.1.2, C.1.3, etc.). Each commit message should summarize the "why" not only the "what" (e.g., `refactor(tui): extract preview building helper to reduce render size and enable unit tests`). Include the baseline clippy snippet in the first commit message as context.
- After each commit run: `cargo fmt && cargo test && cargo clippy --all-targets -- -W clippy::pedantic` and record the clippy delta in the commit body if non-trivial.

Safety and rollback

- Keep changes small and reversible. If a refactor risks UI regression, prefer to isolate changes to logic helpers with covered unit tests and leave rendering glue untouched until validated.
- Do not push until Phase C.1 is fully complete and reviewed. When ready, prepare a PR describing the incremental refactor steps and link to this file.

Markers & progress tracking

- Each sub-step above is a checkbox; mark it as completed (`[x]`) when its unit tests, clippy, and manual verification pass. Add brief notes under each completed item explaining the change and the commit SHA.


Phase D — Tests/CI/Docs (Day 3–4)

- [ ] Add integration tests (1–3h)
- [x] Update CLAUDE.md/GEMINI.md/refactor_review_final.md to reflect completed items (0.5–1h)
- [ ] Update CI to run pedantic clippy and test suite (0.5–1h)

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

Phase A has been implemented (non-blocking activation, per-device locks, IPC window-id, validate_tools).
Next: Focus on robustly adding a slow-activation test to assert spawn_blocking prevents blocking the tokio runtime. This will likely require a refined mocking strategy for PipeWire interactions in tests. Following that: harden stale socket cleanup, filter config watcher + debounce, implement atomic config saves, and run pedantic clippy and TUI fixes.