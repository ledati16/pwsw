#!/usr/bin/env bash
# Prove that the test suite never touches the user's real `pwsw` files.
#
# 1. Snapshot the real config/data dirs (`~/.config/pwsw`, `$XDG_CONFIG_HOME/pwsw`,
#    `$XDG_DATA_HOME/pwsw`): every entry's path, type and content checksum.
# 2. Run `cargo test` with HOME, every XDG dir and TMPDIR pointed into a fresh sandbox,
#    and with session variables (Wayland, D-Bus, PipeWire, PWSW_CONFIG) removed.
# 3. Fail if
#    (a) the real snapshot changed, or anything under it got a newer mtime, or
#    (b) anything appeared at the sandbox's *default* pwsw locations, meaning some test
#        resolved the default config/data path instead of using an explicit temp path.
#
# Environment knobs:
#   PWSW_TMP        base dir for the sandbox (default: `mktemp -d` in the system temp dir)
#   PWSW_REAL_HOME  home whose pwsw files must stay untouched (default: $HOME)
#   CARGO_HOME / RUSTUP_HOME are preserved (defaulting to the real home) so the
#   toolchain keeps working after HOME moves.
# Extra arguments are passed to `cargo test`.
set -euo pipefail

REAL_HOME="${PWSW_REAL_HOME:-$HOME}"
REAL_CONFIG_HOME="${XDG_CONFIG_HOME:-$REAL_HOME/.config}"
REAL_DATA_HOME="${XDG_DATA_HOME:-$REAL_HOME/.local/share}"
if [ -n "${PWSW_REAL_HOME:-}" ]; then
  # An explicit real home wins over the caller's XDG dirs
  REAL_CONFIG_HOME="$REAL_HOME/.config"
  REAL_DATA_HOME="$REAL_HOME/.local/share"
fi
WATCHED=()
for candidate in \
  "$REAL_HOME/.config/pwsw" \
  "$REAL_CONFIG_HOME/pwsw" \
  "$REAL_DATA_HOME/pwsw"; do
  case " ${WATCHED[*]-} " in
    *" $candidate "*) ;; # already watched
    *) WATCHED+=("$candidate") ;;
  esac
done

if ! command -v cargo >/dev/null 2>&1; then
  echo "cargo not found in PATH" >&2
  exit 1
fi

if [ -n "${PWSW_TMP:-}" ]; then
  mkdir -p "$PWSW_TMP"
  SANDBOX=$(mktemp -d "$PWSW_TMP/verify-tests.XXXXXX")
else
  SANDBOX=$(mktemp -d)
fi
cleanup() { rm -rf "$SANDBOX"; }
trap cleanup EXIT

checksum() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | cut -d' ' -f1
  else
    shasum -a 256 "$1" | cut -d' ' -f1
  fi
}

# snapshot <out-file>: path, type and checksum of every entry under the watched dirs
snapshot() {
  local out="$1" dir entry
  : > "$out"
  for dir in "${WATCHED[@]}"; do
    if [ ! -e "$dir" ]; then
      printf '%s\tabsent\n' "$dir" >> "$out"
      continue
    fi
    find "$dir" -print | LC_ALL=C sort | while IFS= read -r entry; do
      if [ -L "$entry" ]; then
        printf '%s\tlink\t%s\n' "$entry" "$(readlink "$entry")"
      elif [ -f "$entry" ]; then
        printf '%s\tfile\t%s\n' "$entry" "$(checksum "$entry")"
      else
        printf '%s\tother\n' "$entry"
      fi
    done >> "$out"
  done
}

STATE="$SANDBOX/state-files"
mkdir -p "$STATE"
snapshot "$STATE/before"
# mtime marker: anything under the watched dirs newer than this was touched
touch "$STATE/marker"

for sub in home config data state cache runtime tmp; do
  mkdir -p "$SANDBOX/$sub"
done
chmod 700 "$SANDBOX/runtime"

echo "Running cargo test in sandbox $SANDBOX"
set +e
(
  export CARGO_HOME="${CARGO_HOME:-$REAL_HOME/.cargo}"
  export RUSTUP_HOME="${RUSTUP_HOME:-$REAL_HOME/.rustup}"
  export HOME="$SANDBOX/home"
  export XDG_CONFIG_HOME="$SANDBOX/config"
  export XDG_DATA_HOME="$SANDBOX/data"
  export XDG_STATE_HOME="$SANDBOX/state"
  export XDG_CACHE_HOME="$SANDBOX/cache"
  export XDG_RUNTIME_DIR="$SANDBOX/runtime"
  export TMPDIR="$SANDBOX/tmp"
  unset WAYLAND_DISPLAY DBUS_SESSION_BUS_ADDRESS PIPEWIRE_REMOTE PWSW_CONFIG
  cargo test "$@"
)
test_status=$?
set -e

failed=0

if [ "$test_status" -ne 0 ]; then
  echo "ERROR: cargo test failed (exit $test_status)" >&2
  failed=1
fi

# (a) real files unchanged
snapshot "$STATE/after"
if ! cmp -s "$STATE/before" "$STATE/after"; then
  echo "ERROR: tests modified the real pwsw files:" >&2
  diff -u "$STATE/before" "$STATE/after" >&2 || true
  failed=1
fi
for dir in "${WATCHED[@]}"; do
  if [ -e "$dir" ]; then
    touched=$(find "$dir" -newer "$STATE/marker" -print)
    if [ -n "$touched" ]; then
      echo "ERROR: tests touched files under $dir:" >&2
      printf '%s\n' "$touched" >&2
      failed=1
    fi
  fi
done

# (b) no test resolved the default locations inside the sandbox
for path in \
  "$SANDBOX/config/pwsw" \
  "$SANDBOX/home/.config/pwsw" \
  "$SANDBOX/data/pwsw" \
  "$SANDBOX/home/.local/share/pwsw" \
  "$SANDBOX/runtime/pwsw.sock"; do
  if [ -e "$path" ]; then
    echo "ERROR: a test used the default path: $path exists" >&2
    find "$path" -print >&2 || true
    failed=1
  fi
done

if [ "$failed" -ne 0 ]; then
  exit 1
fi

echo "OK: tests passed, left the real pwsw files untouched and never used the default paths"
