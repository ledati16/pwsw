#!/usr/bin/env bash
# Quick verification script to ensure AGENTS.md and CLAUDE.md contain key policy phrases
set -euo pipefail
root_dir="$(dirname "${BASH_SOURCE[0]}")/.."
claude="$root_dir/CLAUDE.md"
agents="$root_dir/AGENTS.md"

required=(
  "cargo clippy --all-targets"
  "cargo clippy --all-targets -- -W clippy::pedantic"
  "TEMP_TUI_REFACTOR.md"
  "0o600"
  "NEVER push changes to remote without explicit user approval"
)

missing=()
for phrase in "${required[@]}"; do
  if ! grep -Fq "$phrase" "$claude" && ! grep -Fq "$phrase" "$agents"; then
    missing+=("$phrase")
  fi
done

if [ ${#missing[@]} -ne 0 ]; then
  echo "Docs sync check failed. The following required phrases are missing from both CLAUDE.md and AGENTS.md:" >&2
  for m in "${missing[@]}"; do
    echo " - $m" >&2
  done
  exit 2
fi

echo "Docs sync check passed."