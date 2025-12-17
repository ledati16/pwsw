#!/usr/bin/env bash
set -euo pipefail

USAGE="Usage: $0 [install|uninstall|print]"
if [ "$#" -ne 1 ]; then
  echo "$USAGE"
  exit 1
fi

ACTION=$1
REPO_ROOT=$(git rev-parse --show-toplevel)
HOOK_DIR="$REPO_ROOT/.git/hooks"
HOOK_FILE="$HOOK_DIR/pre-push"

MARKER="# pwsw-managed-hook"

install_hook() {
  mkdir -p "$HOOK_DIR"

  FORCE=0
if [ "${2-}" = "--force" ] || [ "${2-}" = "-f" ]; then
    FORCE=1
fi

if [ -f "$HOOK_FILE" ] && ! grep -qF "$MARKER" "$HOOK_FILE"; then
    if [ "$FORCE" -ne 1 ]; then
      echo "Existing pre-push hook at $HOOK_FILE appears to be user-managed. Use --force to overwrite." >&2
      exit 1
    else
      echo "Overwriting existing user-managed pre-push hook due to --force" >&2
    fi
fi

  cat > "$HOOK_FILE" <<'HOOK'
#!/usr/bin/env bash
set -euo pipefail
# pwsw-managed-hook
# pre-push hook to verify tests run sandboxed locally
# This hook resolves the repository root at runtime and runs the verification script.

REPO_ROOT=$(git rev-parse --show-toplevel)
SCRIPT_PATH="${REPO_ROOT}/scripts/verify_tests_safe.sh"

if [ ! -x "$SCRIPT_PATH" ]; then
  echo "verify_tests_safe.sh not found or not executable at $SCRIPT_PATH" >&2
  exit 1
fi

# Run verification; non-zero exit aborts the push
"$SCRIPT_PATH"
HOOK

  chmod +x "$HOOK_FILE"
  echo "Installed pre-push hook at $HOOK_FILE"
}

uninstall_hook() {
  if [ -f "$HOOK_FILE" ]; then
    if grep -qF "$MARKER" "$HOOK_FILE"; then
      rm "$HOOK_FILE"
      echo "Removed pwsw-managed pre-push hook"
    else
      echo "Pre-push hook at $HOOK_FILE is not managed by pwsw; leaving intact" >&2
      exit 1
    fi
  else
    echo "No pre-push hook to remove"
  fi
}

print_hook() {
  if [ -f "$HOOK_FILE" ]; then
    echo "Current pre-push hook contents:";
    sed -n '1,200p' "$HOOK_FILE"
  else
    echo "No pre-push hook installed"
  fi
}

case "$ACTION" in
  install) install_hook ;;
  uninstall) uninstall_hook ;;
  print) print_hook ;;
  *) echo "$USAGE"; exit 1 ;;
esac
