#!/usr/bin/env bash
set -euo pipefail

# Run the test suite with an isolated XDG_CONFIG_HOME and verify no writes
# occurred to the real user config (~/.config/pwsw) or the config write log.

echo "Running sandboxed tests (isolated XDG_CONFIG_HOME)"

TMP_XDG=$(mktemp -d)
trap 'rm -rf "$TMP_XDG"' EXIT

HOME_PWSW="$HOME/.config/pwsw"

# Ensure cargo is available
if ! command -v cargo >/dev/null 2>&1; then
  echo "cargo not found in PATH" >&2
  exit 1
fi

# Portable stat helper: size_of <file>
size_of() {
  local f="$1"
  if stat --version >/dev/null 2>&1; then
    stat -c%s "$f"
    return
  fi
  # BSD/macOS stat
  if stat -f%z "$f" >/dev/null 2>&1; then
    stat -f%z "$f"
    return
  fi
  # Fallback
  wc -c < "$f" | awk '{print $1}'
}

# Portable listing of files under a dir into snapshot file
list_files_snapshot() {
  local dir="$1"; shift
  local out="$1"; shift
  if [ -d "$dir" ]; then
    # Prefer GNU find -printf when available
    if find "$dir" -type f -printf "%P\t%s\t%T@\n" >/dev/null 2>&1; then
      find "$dir" -type f -printf "%P\t%s\t%T@\n" | sort > "$out"
    else
      # BSD find: print file names then use stat for sizes
      find "$dir" -type f -print | while IFS= read -r f; do
        rel=${f#${dir}/}
        size=$(size_of "$f")
        mtime=$(perl -e 'print((stat shift)[9])' "$f" 2>/dev/null || stat -c%Y "$f" 2>/dev/null || echo 0)
        printf "%s\t%s\t%s\n" "$rel" "$size" "$mtime"
      done | sort > "$out"
    fi
  else
    echo "" > "$out"
  fi
}

# Snapshot of existing files under ~/.config/pwsw
BEFORE_SNAP=$(mktemp)
list_files_snapshot "$HOME_PWSW" "$BEFORE_SNAP"

# Snapshot size of the write log
LOG_PATH="/tmp/pwsw-config-write.log"
LOG_BEFORE_SIZE=0
if [ -f "$LOG_PATH" ]; then
  LOG_BEFORE_SIZE=$(size_of "$LOG_PATH")
fi

# Run tests with isolated XDG_CONFIG_HOME
export XDG_CONFIG_HOME="$TMP_XDG"

echo "XDG_CONFIG_HOME=$XDG_CONFIG_HOME"

# Capture tests output for debugging
TEST_OUTPUT="/tmp/pwsw-test-output-sandbox.txt"

if ! cargo test --all --verbose 2>&1 | tee "$TEST_OUTPUT"; then
  echo "cargo test failed — see $TEST_OUTPUT"
  exit 1
fi

# Compare ~/.config/pwsw before/after
AFTER_SNAP=$(mktemp)
list_files_snapshot "$HOME_PWSW" "$AFTER_SNAP"

if ! cmp -s "$BEFORE_SNAP" "$AFTER_SNAP"; then
  echo "ERROR: Tests modified files under $HOME_PWSW"
  echo "Current listing:"
  ls -la "$HOME_PWSW" || true
  echo "Diff (before -> after):"
  diff -u "$BEFORE_SNAP" "$AFTER_SNAP" || true
  exit 2
fi

# Check log growth
LOG_AFTER_SIZE=0
if [ -f "$LOG_PATH" ]; then
  LOG_AFTER_SIZE=$(size_of "$LOG_PATH")
fi

if [ "$LOG_AFTER_SIZE" -ne "$LOG_BEFORE_SIZE" ]; then
  echo "ERROR: $LOG_PATH changed during the test run"
  echo "Log contents (tail):"
  tail -n 200 "$LOG_PATH" || true
  exit 3
fi

echo "OK: Tests ran in sandbox and did not touch $HOME_PWSW or $LOG_PATH"
exit 0
