## Refactoring Protocol

**Active Refactoring Plan:**
We are currently executing a comprehensive refactoring plan detailed in `refactor_review_final.md`.
- **Read First:** Before performing any task, read `refactor_review_final.md` to understand the current phase and objectives.
- **Update Constantly:** After completing a step, mark it as checked `[x]` in `refactor_review_final.md`. If you change the approach, update the plan text.
- **Goal:** Modernize the codebase using ecosystem crates (`tui-input`, `notify`, `color-eyre`) to reduce boilerplate and fragility while preserving safety and testability.

## Project Overview

PWSW (PipeWire Switcher) is a Rust daemon that automatically switches PipeWire audio sinks based on active windows in Wayland compositors. It uses the wlr-foreign-toplevel-management protocol to monitor window events and PipeWire native tools (pw-dump, pw-metadata, pw-cli) for audio control.

**Safety & Provenance Note:**
- This codebase has undergone extensive automated and manual refactors. Treat changes with care: make small, reversible commits and run the verification cycle described below before committing.

## Development & Agent Guidelines (New / Updated)

These are practical, actionable rules we've learned while refactoring. Follow them for every repo edit.

- Read `AGENTS.md` and `refactor_review_final.md` before acting.
- Use the `scripts/verify_tests_safe.sh` script to validate tests are safe (they must not touch the real user config). Run it after substantial changes.
- Run the validation cycle before committing: `cargo fmt && cargo test && cargo clippy --all-targets -- -W clippy::pedantic && bash scripts/verify_tests_safe.sh`.
- When using automated agents (or running scripted edits), always:
  - Provide a short preamble describing the immediate actions.
  - Create a concise todo plan (use the repository's `todowrite` convention if available).
  - Keep each change small and self-contained; prefer many tiny commits to one big one.
- Use the repository helper scripts: `scripts/install_git_hook.sh` (for git hooks) and `scripts/verify_tests_safe.sh` (safety checks).
- Do not push to remote without explicit human approval. Make local commits, inform the user, and wait for a push approval.

## Code Quality Standards (Updated)

All new code must adhere to these standards. These have been tightened slightly to reflect lessons learned.

### Formatting & Tests
- Run `cargo fmt` before committing.
- Unit tests must be fast and deterministic. Tests that interact with external resources must be sandboxed or mocked.
- Tests must default to an isolated XDG_CONFIG_HOME (see `src/test_utils.rs` helpers / `XdgTemp` pattern) so they never modify the real `~/.config/pwsw`.

### Clippy Compliance
- Run pedantic clippy frequently:

```bash
cargo clippy --all-targets -- -W clippy::pedantic
```

- There are a small number of historically accepted pedantic warnings documented in `refactor_review_final.md`. Re-evaluate these on a per-refactor basis — prefer fixing a warning over adding another `#[allow(...)]`.

### Documentation
- Every public function returning `Result` must include an `# Errors` section describing possible failures.
- Any use of `expect()`/`unwrap()` must be justified and documented under `# Panics`.
- Use backticks for technical terms (e.g., `PipeWire`, `app_id`, `XDG_RUNTIME_DIR`).

### Error Handling & Security
- Prefer `anyhow::Context` for richer error messages in application code.
- Avoid `expect()` in production paths; reserve for truly unreachable conditions and document them.
- External commands must be executed safely (no shell interpolation) and their exit status validated.
- IPC sockets and files created by the daemon must use user-only permissions (0o600) and be validated before removal.

## TUI & Concurrency Patterns (New guidance)

During the recent TUI refactor we encountered borrow-checker conflicts when trying to make fields private (notably `throbber_state`). From that we derived these patterns:

- Prefer snapshotting read-only data before taking mutable borrows. Example pattern:
  - Read/clonesmall pieces of data you need from the app (strings, flags, small vectors) into local variables.
  - Drop or let those immutable references go out of scope, then take `&mut` borrows for render state.
  - This avoids overlapping borrows and is safer than adding interior mutability just to bypass borrow checks.
- Avoid introducing `RefCell`/`Mutex` inside the main UI state purely to silence borrow errors; prefer small API or render-scope refactors.
- When refactoring UI rendering functions, try to extract pure logic (matching, line building) into `fn`s that accept owned or immutable references — these are easy to unit test.

## Build & Run Commands (unchanged)

Development:

```bash
cargo build
cargo build --release
cargo check
cargo test
cargo clippy
cargo fmt
```

Installation:

```bash
cargo install --path .
```

Running:

```bash
cargo run -- daemon --foreground
cargo run -- daemon
cargo run -- status
```

## Architecture & Design Notes (minor updates)

- Keep blocking PipeWire or other long-running calls inside `tokio::task::spawn_blocking` to avoid stalling the runtime.
- When profile-switching, serialize switches per-device (use a per-device mutex in `pipewire.rs`) to avoid races; prefer `std::sync` primitives usable from `spawn_blocking`.
- IPC handlers should clone only needed small snapshots of state; avoid holding long-lived locks while servicing a client.

## Testing Checklist (strongly enforced)

Before committing any change:

- `cargo fmt`
- `cargo test` (unit tests)
- `cargo clippy --all-targets -- -W clippy::pedantic` (fix warnings or justify them in the refactor notes)
- `bash scripts/verify_tests_safe.sh` (ensure no accidental modification of real user config)

Manual scenarios to exercise when relevant:
- Start daemon (`cargo run -- daemon --foreground`), run `pwsw list-windows`, `pwsw list-sinks`, `pwsw test-rule`.
- For PipeWire changes, test on a machine with PipeWire available or use unit tests that parse sample JSON dumps.

## Git & Commit Guidelines (unchanged but reinforced)

- Use conventional commits: `feat:`, `fix:`, `refactor:`, `docs:`, `chore:` etc.
- Include the rationale in the commit message body (why not only what).
- Create commits locally and wait for explicit approval before pushing.
- Use `scripts/install_git_hook.sh` to set up hooks that enforce tests and formatting where appropriate.

## Miscellaneous

- If you introduce a new public API, update `refactor_review_final.md` and add a short rationale in the PR description.
- If a refactor temporarily adds `#[allow(...)]`, include a TODO comment explaining why and link to an issue or `refactor_review_final.md` entry.

-- End of GEMINI guidance --
