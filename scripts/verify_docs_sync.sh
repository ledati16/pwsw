#!/usr/bin/env bash
# Verification script to ensure CLAUDE.md and AGENTS.md are kept in sync on key policies.
set -euo pipefail
root_dir="$(dirname "${BASH_SOURCE[0]}")/.."
claude="$root_dir/CLAUDE.md"
agents="$root_dir/AGENTS.md"

# Phrases that MUST exist in CLAUDE.md
claude_required=(
  "cargo clippy --all-targets"
  "cargo clippy --all-targets -- -W clippy::pedantic"
  "TEMP_TUI_REFACTOR.md"
  "0o600"
  "NEVER push changes to remote without explicit user approval"
)

# Phrases that MUST exist in AGENTS.md (and thus reflect CLAUDE.md policy)
agents_required=(
  "CLAUDE.md"
  "cargo clippy --all-targets"
  "cargo clippy --all-targets -- -W clippy::pedantic"
  "TEMP_TUI_REFACTOR.md"
  "0o600"
  "explicit approval"
)

missing_claude=()
for phrase in "${claude_required[@]}"; do
  if ! grep -Fq -- "$phrase" "$claude"; then
    missing_claude+=("$phrase")
  fi
done

missing_agents=()
for phrase in "${agents_required[@]}"; do
  if ! grep -Fq -- "$phrase" "$agents"; then
    missing_agents+=("$phrase")
  fi
done

if [ ${#missing_claude[@]} -ne 0 ] || [ ${#missing_agents[@]} -ne 0 ]; then
  echo "Docs sync check failed." >&2
  if [ ${#missing_claude[@]} -ne 0 ]; then
    echo " Missing from CLAUDE.md:" >&2
    for m in "${missing_claude[@]}"; do
      echo "  - $m" >&2
    done
  fi
  if [ ${#missing_agents[@]} -ne 0 ]; then
    echo " Missing from AGENTS.md:" >&2
    for m in "${missing_agents[@]}"; do
      echo "  - $m" >&2
    done
  fi
  exit 2
fi

echo "Docs sync check passed: CLAUDE.md and AGENTS.md contain required policy phrases."