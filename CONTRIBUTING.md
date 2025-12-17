**Purpose**

- This repository enforces a safety-first developer workflow: tests must be hermetic and must not modify a developer's real config (`~/.config/pwsw`) or the shared write log (`/tmp/pwsw-config-write.log`).

**Prerequisites**

- Install Rust and Cargo (the repo requires `rust-version = "1.74"`).
- Ensure basic shell tooling available: `git`, `bash`, `mktemp`, `chmod`, `find`, `stat` (GNU or BSD), and `perl` (fallback used by the verifier).

**Quick Start**

- Clone and enter repo:
  - `git clone <repo-url> pwsw && cd pwsw`
- (Optional) Checkout the branch you want to work on:
  - `git checkout <branch-or-commit>`
- Run tests locally:
  - `cargo test --all --verbose`

**Verify tests are safe (recommended)**

- We provide a verification script that runs the test suite inside an isolated `XDG_CONFIG_HOME` and checks that no files under `~/.config/pwsw` nor `/tmp/pwsw-config-write.log` were modified.
- To run it locally (recommended before pushing):
  - `bash scripts/verify_tests_safe.sh`
- File reference: `scripts/verify_tests_safe.sh:1`

**Install the repo pre-push hook (optional)**

- The repository includes a helper to install a `pre-push` git hook that runs the verification script before pushes. The hook is non-destructive by default and will not overwrite an existing user-managed hook unless you pass `--force`.
- Install:
  - `bash scripts/install_git_hook.sh install`
- Install, forcing overwrite of an existing hook:
  - `bash scripts/install_git_hook.sh install --force`
- Uninstall (only removes the hook if it was installed by this script):
  - `bash scripts/install_git_hook.sh uninstall`
- Inspect currently installed hook:
  - `bash scripts/install_git_hook.sh print`
- File reference: `scripts/install_git_hook.sh:1`

**Formatting & linting**

- Format code:
  - `cargo fmt`
- Run clippy and treat warnings as errors:
  - `cargo clippy --all-targets -- -D warnings`

**Commit & Push policy (developer-facing)**

- Format, run tests, and run clippy before opening a PR.
  - Minimal checklist: `cargo fmt && cargo test --all --verbose && cargo clippy --all-targets -- -D warnings`.
- Follow the repo's guidance in `AGENTS.md` for push policy and pre-commit checks.
  - File reference: `AGENTS.md:1`

**CI**

- The repository CI runs tests, clippy, and the verification script.
  - File reference: `.github/workflows/ci.yml:1`
- If you change `scripts/verify_tests_safe.sh`, update the CI workflow accordingly.

**Safety notes**

- Tests are intentionally defensive: during `cargo test` the code refuses to write the real user config unless `PWSW_ALLOW_CONFIG_WRITE=1` is explicitly set. Do not set this variable unless you intentionally want to overwrite `~/.config/pwsw`.
- The verification script sets `XDG_CONFIG_HOME` to a temporary directory so tests operate on an isolated config directory.
- The git hook installer marks hooks it manages with `# pwsw-managed-hook` so it won't remove or overwrite unmarked hooks without `--force`.

**If you delete your working copy and reclone**

- Recreate the state with these commands:
  - `git clone <repo-url> pwsw && cd pwsw`
  - `rustup toolchain install stable` (if rustup is set up)
  - `cargo test --all --verbose`
  - `bash scripts/verify_tests_safe.sh`
  - `bash scripts/install_git_hook.sh install` (optional)

**Contact / Maintainers**

- If you need approval to push, or have questions about safety policies and CI, ask the maintainers listed in the project README or open a draft PR for feedback.

---

This file is intentionally short — if you want more details (example workflows, CI matrices, or a Windows/macOS checklist) tell me which sections to expand and I will update this file.