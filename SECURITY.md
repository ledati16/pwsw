# Security policy

## Supported versions

Only the latest release (and the current `main` branch) receives security fixes.

## Reporting a vulnerability

Please report privately through GitHub: on the repository's **Security** tab choose
**Report a vulnerability** (private vulnerability reporting). Do not post a security
problem publicly (for example in Discussions). Include the pwsw version
(`pwsw --version`), what an attacker needs (another local user? a malicious window
title? a crafted config?), and steps to reproduce if you have them.

This is a hobby project with a single maintainer; expect a reply within a few weeks.
Fixes are released as a new version and noted under "Security" in the CHANGELOG.

## Scope

In scope, for example:

- **IPC socket** (`$XDG_RUNTIME_DIR/pwsw.sock` or `/tmp/pwsw-<uid>/`): access by another
  user, peer-credential bypass, crashes or hangs from malformed or oversized messages.
- **Config handling**: unsafe writes or permissions, following symlinks into files pwsw
  should not touch, deleting files it does not own.
- **Untrusted window titles and `app_id`s**: injection into logs, terminal output, the
  TUI, the `status --watch` stream or desktop notifications; regex denial of service from window input.
- **Command execution**: anything that lets input reach `pw-dump`, `pw-cli` or
  `pw-metadata` arguments beyond the intended JSON values, or run other programs.
- Vulnerable dependencies that pwsw actually reaches (`cargo audit` runs weekly in CI).

Out of scope: attacks that already require running code as your user, and limitations of
the compositor protocols or PipeWire itself.
