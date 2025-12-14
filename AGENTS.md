# AGENTS.md

This file gives explicit instructions for automated agents and human contributors working on the repository, with a focus on the ongoing TUI refactor tracked in `TEMP_TUI_REFACTOR.md`.

Scope
- The scope of this file is the entire repository. Agents must follow these rules when touching any files, but **must** pay special attention when editing files under `src/tui/`.

Quick checklist (required before committing any changes that touch source code)
- Run the unit test suite: `cargo test` (all tests must pass).
- Run Clippy with warnings treated as errors: `cargo clippy --all-targets -- -D warnings` (must pass cleanly).
- Update `TEMP_TUI_REFACTOR.md` with a concise entry describing what changed and the next small steps. Each minor step in the TUI refactor must be recorded there.
- Ensure public functions that return `Result` have `# Errors` doc sections per repository `CLAUDE.md` guidance.
- Avoid adding per-frame allocations or compiling regexes in render paths. Use cached `Arc<regex::Regex>` from editor state or background worker.
- Use the `SinksScreen::update_display_descs()` and `SettingsScreen.padded_names` patterns for precomputed padded strings where consistent alignment or repeated rendering occurs.
- Do not push to remote or open a pull request without explicit approval from the repository owner (unless the user explicitly asked). Follow local commit/push policy in `CLAUDE.md`.

Behavioral rules for agents
- Keep changes small and focused; prefer surgical edits over large refactors.
- When modifying a TUI render function, include a brief explanation in the repo PR or in `TEMP_TUI_REFACTOR.md` why the change avoids allocations and how it was validated (tests/clippy/manual check).
- If you add new caches (e.g., padded strings), ensure they are initialized in `App::new()` and updated in any code path that mutates the underlying data.

Documentation & progress tracking
- `TEMP_TUI_REFACTOR.md` is the canonical progress log for the TUI refactor. Update it at every successful minor milestone (e.g., "added cached padded settings names", "removed Regex::new from render", "added preview debounce tests").
- Keep `TEMP_TUI_REFACTOR.md` short, factual and actionable: what was changed, validation performed, and the next step.

Security & safety notes
- Do not construct shell commands with user input. Use `serde_json` and structured APIs for external tool invocations.
- When creating files or sockets, ensure user-only permissions where appropriate.

If you are an agent executing automated edits: follow the checks above and explicitly write a short entry to `TEMP_TUI_REFACTOR.md` before committing.

-- End of AGENTS.md
