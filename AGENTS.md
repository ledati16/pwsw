## Agents guide

IMPORTANT: Read GEMINI.md before using agents. If you haven't already, please read GEMINI.md first.

This file mirrors important guidance from GEMINI.md for agents and contributors.

- Active refactoring plan: See `refactor_review_final.md` for current tasks and progress.
- Before beginning work: Read `refactor_review_final.md` to understand the current phase and objectives.
- After completing a step: Mark it as checked `[x]` in `refactor_review_final.md`.

Development workflow (copy of GEMINI.md core points):
- Format code: `cargo fmt`
- Check tests: `cargo test`
- Check clippy: `cargo clippy --all-targets -- -W clippy::pedantic`

Git Push Policy:
- Push only with explicit user approval.
- When creating commits: make local commits, inform user, and wait for push approval.

Notes:
- This is a local helper file for agent guidance; do not add sensitive info here.
