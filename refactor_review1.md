
  1. Absolutely Recommend (Critical Cleanup & Fixes)

   * Remove Dead Code in `src/tui/app.rs`:
       * Issue: The methods execute_pending_daemon_action and update_daemon_state, along with the pending_daemon_action field, are remnants of the old synchronous architecture. The new async worker handles these tasks directly.
       * Action: Delete them to clean up the state struct.
   * Fix Config Watcher Filter (`src/daemon.rs`):
       * Issue: The current watcher triggers on any file event in the ~/.config/pwsw/ directory. Creating a unrelated file (e.g., backup.toml) triggers a reload.
       * Action: Filter events to match config.toml path specifically.
   * Remove Unused Dependencies (`Cargo.toml`):
       * Issue: tui-popup is included but unused (we used a custom implementation). tui-logger is included but the UI integration was skipped.
       * Action: Remove tui-popup. Remove tui-logger (and its init code in main.rs) to keep the build clean until the feature is implemented.

  2. Highly Recommend (Optimization & Refinement)

   * Refactor `render_sink_selector` to Shared Widget:
       * Issue: Complex rendering logic for the sink dropdown is duplicated in rules.rs and sinks.rs.
       * Action: Move this complex rendering logic into src/tui/widgets.rs as a reusable component.
   * Encapsulate `ThrobberState`:
       * Issue: ThrobberState lives in the global App struct but is only used by RulesScreen.
       * Action: Move it to RulesScreen to improve encapsulation.

  3. Lightly Recommend (Style & API)

   * Rename `SimpleEditor`:
       * Issue: The name is a legacy artifact.
       * Action: Rename to EditorState.
   * Simplify `src/tui/widgets.rs`:
       * Issue: render_input manually calculates offsets.
       * Action: Create a proper struct InputWidget<'a> that implements ratatui::widgets::Widget.

  Optional (Future Work)

   * Input Handling Tests: The new handle_event logic in input.rs is critical but untested. Unit tests would ensure stability.
   * Atomic Config Writes: Ensure Config::save() writes to a temp file and renames it.

  Refactor Oversight: The tui-logger UI implementation was in the plan but skipped. The dependency remains in Cargo.toml and main.rs, creating a "zombie" feature.
