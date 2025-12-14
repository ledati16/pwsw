# AGENTS.md

This file gives explicit instructions for automated agents and human contributors working on the repository, and must be kept in sync with `CLAUDE.md` (the authoritative project guidance).

Scope
- The scope of this file is the entire repository. Agents must follow these rules when touching any files, and must pay special attention when editing files under `src/tui/`.
- Before making changes that touch the TUI, consult `TEMP_TUI_REFACTOR.md` for the current plan and progress. `TEMP_TUI_REFACTOR.md` is the canonical progress log for the TUI refactor and must be updated for each minor step.

Quick checklist (required before committing any changes that touch source code)
- Run the unit test suite: `cargo test` (all tests must pass).
- Run Clippy: `cargo clippy --all-targets` (fix warnings). Then run pedantic checks: `cargo clippy --all-targets -- -W clippy::pedantic` and ensure only the acceptably documented pedantic warnings from `CLAUDE.md` are present.
- Format code: `cargo fmt`.
- Update `TEMP_TUI_REFACTOR.md` with a concise entry describing what changed and the next small steps for TUI-related work.
- Ensure public functions that return `Result` have `# Errors` doc sections per `CLAUDE.md` guidance.
- Avoid adding per-frame allocations or compiling regexes in render paths. Use cached `Arc<regex::Regex>` from editor state or from the background worker.
- Use precomputed padded-display patterns: `SinksScreen::update_display_descs()` and `SettingsScreen.padded_names` (or similar) when repeated rendering and alignment are required.
- Do not push to remote or open a pull request without explicit approval from the repository owner (unless the user explicitly asked). Follow the Git Push Policy in `CLAUDE.md`.

Behavioral rules for agents
- Keep changes small and focused; prefer surgical edits over large refactors.
- When modifying a TUI render function, include a brief explanation in the commit/PR or in `TEMP_TUI_REFACTOR.md` detailing why the change avoids allocations and how it was validated (tests, clippy, or manual check).
- If you add new caches (e.g., padded strings), ensure they are initialized in `App::new()` and updated in any code path that mutates the underlying data.
- Follow `CLAUDE.md` conventions for documentation, error handling, and modern Rust idioms.

Documentation & progress tracking
- `TEMP_TUI_REFACTOR.md` is the canonical progress log for the TUI refactor. Update it at every successful minor milestone (e.g., "added cached padded settings names", "removed Regex::new from render", "added preview debounce tests").
- Keep `TEMP_TUI_REFACTOR.md` short, factual and actionable: what was changed, validation performed, and the next step.
- Before committing any TUI-related edits, add a short entry to `TEMP_TUI_REFACTOR.md` describing the change.

Security & safety notes
- Do not construct shell commands with user input. Use `serde_json` and structured APIs for external tool invocations.
- When creating files or sockets, ensure user-only permissions where appropriate (refer to `CLAUDE.md` for socket permission guidance: `0o600`).

Commit / Push policy
- Use conventional commits per `CLAUDE.md`: `<type>: <subject>`, include a body, and append the generator/co-author lines only when appropriate.
- NEVER push changes to remote without explicit user approval. The normal flow is:
  1. Make a local commit.
  2. Notify the user of the commit and provide a short summary.
  3. Wait for explicit push approval (e.g., "push it").
  4. If approved, push with `git push` (or `git push -u origin <branch>` for a new branch).

Automation notes for agent implementers
- If you are an automated agent performing edits:
  - Follow the checks above before committing.
  - Always write a short, one-line `TEMP_TUI_REFACTOR.md` entry describing the edit and the next small step.
  - Do not open PRs or push without human approval.

Where AGENTS.md and CLAUDE.md must remain in sync
- `CLAUDE.md` is the comprehensive project policy. `AGENTS.md` is a concise operational checklist derived from it for agents and contributors.
- Whenever a policy in `CLAUDE.md` changes (e.g., new clippy rules, additional pedantic exceptions, or push policy changes), update `AGENTS.md` to reflect the change.
- Before making releases or merging large changes, cross-verify `AGENTS.md` and `CLAUDE.md` for consistency.

-- End of AGENTS.md
