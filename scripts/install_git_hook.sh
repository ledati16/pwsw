#!/usr/bin/env bash
# Install (or remove) pwsw's git hooks.
#
#   bash scripts/install_git_hook.sh [--force] [install|uninstall|print]
#
# install (default)  write the pre-commit and pre-push hooks
# uninstall          remove them again (only hooks this script wrote)
# print              show the installed hooks
# --force, -f        overwrite hooks that were not written by this script
#
# pre-commit runs the fast checks: nightly `rustfmt --check` and clippy with
# `-D warnings`. pre-push runs the test suite through `scripts/verify_tests_safe.sh`,
# which runs `cargo test --locked` with HOME/XDG sandboxed and then proves the real
# `pwsw` config was never touched. Skip a hook once with `git commit --no-verify` /
# `git push --no-verify`.
#
# Hooks go to `git rev-parse --git-path hooks`, so they work from any worktree (all
# worktrees share one hooks directory) and honour `core.hooksPath`.
set -euo pipefail

# Keep in sync with the fmt job in .github/workflows/ci.yml
FMT_TOOLCHAIN="nightly-2026-11-14"
MARKER="# pwsw-managed-hook"
HOOKS=(pre-commit pre-push)

usage() {
  echo "Usage: $0 [--force] [install|uninstall|print]"
}

FORCE=0
ACTION=""
for arg in "$@"; do
  case "$arg" in
    -f | --force) FORCE=1 ;;
    -h | --help)
      usage
      exit 0
      ;;
    install | uninstall | print)
      if [ -n "$ACTION" ]; then
        usage >&2
        exit 1
      fi
      ACTION="$arg"
      ;;
    *)
      usage >&2
      exit 1
      ;;
  esac
done
ACTION="${ACTION:-install}"

HOOK_DIR=$(git rev-parse --path-format=absolute --git-path hooks)

is_managed() {
  grep -qF "$MARKER" "$1"
}

hook_body() {
  case "$1" in
    pre-commit)
      cat <<HOOK
#!/bin/sh
$MARKER (written by scripts/install_git_hook.sh; reinstall to update)
# Fast checks before every commit. They see the working tree, not just the index.
set -eu
cd "\$(git rev-parse --show-toplevel)"

echo "pre-commit: cargo +$FMT_TOOLCHAIN fmt --check"
if ! cargo +$FMT_TOOLCHAIN fmt --check; then
  echo "pre-commit: formatting differs; run 'cargo +$FMT_TOOLCHAIN fmt'" >&2
  exit 1
fi

echo "pre-commit: cargo clippy --all-targets --locked -- -D warnings"
cargo clippy --all-targets --locked -- -D warnings

echo "pre-commit: OK"
HOOK
      ;;
    pre-push)
      cat <<HOOK
#!/bin/sh
$MARKER (written by scripts/install_git_hook.sh; reinstall to update)
# Full test suite before every push: verify_tests_safe.sh runs 'cargo test --locked'
# in a sandboxed HOME/XDG and fails if any test touched the real pwsw files.
set -eu
cd "\$(git rev-parse --show-toplevel)"

echo "pre-push: scripts/verify_tests_safe.sh --locked (cargo test --locked, sandboxed)"
bash scripts/verify_tests_safe.sh --locked

echo "pre-push: OK"
HOOK
      ;;
  esac
}

install_hooks() {
  local name file
  mkdir -p "$HOOK_DIR"
  # Check everything first so a refusal leaves no hook half-installed
  for name in "${HOOKS[@]}"; do
    file="$HOOK_DIR/$name"
    if [ -e "$file" ] && ! is_managed "$file" && [ "$FORCE" -ne 1 ]; then
      echo "Refusing to overwrite existing $name hook at $file (not written by this script)." >&2
      echo "Re-run with --force to replace it." >&2
      exit 1
    fi
  done
  for name in "${HOOKS[@]}"; do
    file="$HOOK_DIR/$name"
    if [ -e "$file" ] && ! is_managed "$file"; then
      echo "Overwriting existing $name hook at $file (--force)"
    fi
    hook_body "$name" > "$file"
    chmod +x "$file"
    echo "Installed $name hook at $file"
  done
}

uninstall_hooks() {
  local name file status=0
  for name in "${HOOKS[@]}"; do
    file="$HOOK_DIR/$name"
    if [ ! -e "$file" ]; then
      echo "No $name hook installed"
    elif is_managed "$file"; then
      rm -- "$file"
      echo "Removed $name hook at $file"
    else
      echo "$name hook at $file was not written by this script; leaving it" >&2
      status=1
    fi
  done
  return "$status"
}

print_hooks() {
  local name file
  for name in "${HOOKS[@]}"; do
    file="$HOOK_DIR/$name"
    if [ -e "$file" ]; then
      echo "=== $file"
      cat -- "$file"
    else
      echo "=== $name: not installed"
    fi
  done
}

case "$ACTION" in
  install) install_hooks ;;
  uninstall) uninstall_hooks ;;
  print) print_hooks ;;
esac
