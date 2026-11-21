# Changelog

All notable user-visible changes to PWSW are documented here.
The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the
project uses [Semantic Versioning](https://semver.org/) (pre-1.0: minor versions may
contain breaking changes).

## [0.7.0] - 2026-11-20

A full overhaul focused on correctness, safety and predictable behaviour. Please read
**Changed** before upgrading: some configs that 0.6.0 accepted are now rejected, and
the daemon switches less eagerly than before.

### Changed

- **The daemon switches only when the rules' answer changes** (a matching window opens,
  closes or changes title, or the config changes the rules). A sink you choose in
  pavucontrol, or one WirePlumber picks, is no longer undone by the next unrelated
  window event. A manual `set-sink`/`next-sink`/`prev-sink` holds the same way.
- **`default_on_startup = false` is now honoured**: the daemon no longer switches on the
  first window event after start-up. With `true` it switches to the sink the rules want
  (the default sink if nothing matches).
- **Unknown config keys are errors.** A typo such as `defualt = true` used to be ignored;
  now loading fails with the key, line and file. Also rejected: empty sink names,
  descriptions or rule sink references, numeric sink descriptions, and a string that is
  one sink's `name` and another sink's `desc`.
- **Positional sink references are deprecated.** `sink = 2` (integers are accepted again,
  as documented) and `sink = "2"` still work but log a warning, because they silently
  point at a different sink when sinks are reordered by hand. Use the sink's `desc`. The
  TUI always writes `desc` and keeps rules pointing at the same sink when you rename,
  reorder or delete sinks.
- **Only `pwsw init` and `pwsw tui` create a config file.** Other commands (including the
  daemon and `validate`) report a missing file instead of writing a default one.
- **The default sink is set as `default.configured.audio.sink`**, like
  `wpctl set-default`. 0.6.0 wrote `default.audio.sink`, which WirePlumber recomputes and
  could silently undo.
- **Profile switches are saved in WirePlumber** (so it keeps the profile), only
  available profiles are used, and the profile that keeps the card's current microphone
  is preferred.
- **`next-sink`/`prev-sink` skip sinks that cannot be activated right now** (e.g. an
  HDMI output whose display is off) and start from the real PipeWire default.
  `set-sink` still tries the sink you asked for and reports why it failed. A failed
  manual switch leaves the audio where it was.
- Manual switches go through the daemon when it is running, so its rules see your
  choice; without a daemon they switch PipeWire directly.
- **`status --json` has a new schema** and always prints exactly one JSON document with
  exit status 0: `current_sink`, `daemon` (`running`, `error`, `version`, `protocol`,
  `uptime_secs`, ...), `pipewire` (`current_sink`, `error`) and `config` (`path`,
  `error`). Fields that cannot be determined are `null`, never missing. See `man pwsw`.
- **IPC socket fallback moved** from `/tmp/pwsw-<uid>.sock` to
  `/tmp/pwsw-<uid>/pwsw.sock` (used only when `XDG_RUNTIME_DIR` is unset, empty or not
  absolute). The directory must be ours with mode `0700`.
- **The PID file is replaced by a lock file** (`pwsw.lock`, next to the socket), held
  with `flock` so a crashed daemon never blocks the next start.
- Errors are printed as one readable `error: ...` line (exit status 1; usage errors
  exit 2). Set `RUST_BACKTRACE=1` for the full report.
- Rule regexes are no longer checked against a list of "dangerous" constructs (Rust's
  regex engine is linear-time); only the compiled size is limited. The TUI editor,
  `test-rule` and config loading accept exactly the same patterns.
- The background daemon (`pwsw daemon`) runs in its own process group, so it survives
  the terminal or TUI that started it; if start-up fails, the error and the last log
  lines are printed.
- systemd unit: part of `graphical-session.target`, `Type=notify` with the current sink
  in `systemctl --user status`, `Wants=` instead of binding to `pipewire.service` (a
  PipeWire restart no longer stops pwsw), at most 5 restarts per minute, and sandboxed.
- The TUI is much lighter: it no longer runs `pw-dump` while drawing, sleeps while idle,
  and shows rule previews immediately.
- Building requires **Rust 1.100** (was 1.94); the toolchain is pinned in
  `rust-toolchain.toml`.

### Added

- `pwsw init [--force]`: write the commented default config (`--force` keeps the old
  file as `config.toml.bak`).
- `--config PATH` and the `PWSW_CONFIG` environment variable to use another config file.
- `--color auto|always|never`; `NO_COLOR` and `CLICOLOR_FORCE` are honoured, and piped
  output contains no escape sequences.
- `pwsw completions <bash|zsh|fish|elvish|powershell>`; the PKGBUILD installs bash, zsh
  and fish completions.
- Comment-preserving saves: the TUI keeps your comments and formatting, keeps a
  `config.toml.bak`, keeps a symlinked config a symlink, and asks before overwriting a
  file that changed on disk since it was loaded.
- Hot reload follows symlinked configs (stow, chezmoi) and `SIGHUP` reloads; `log_level`
  changes apply without a restart.
- `pwsw --version` includes the git commit (`0.7.0+abc1234`, `-dirty` for local changes).
- IPC protocol version: `pwsw status` reports a daemon that needs a restart after an
  upgrade, and an old daemon answers unknown requests with a restart hint.
- TUI: unsaved-changes protection on quit (`q` or `Ctrl+C`), a "changed on disk" dialog,
  bracketed paste into text fields, scrollbars, and help generated from the real key
  bindings.

### Fixed

- Profile-switchable sinks were predicted wrongly for most duplex profiles (non-existent
  node names) and input-only profiles were offered as outputs.
- Switching to an output on the same card could remove the card's microphone.
- Activation no longer depends on node names after a profile switch (WirePlumber renames
  no longer break it).
- A hung PipeWire tool can no longer block pwsw: every `pw-dump`/`pw-cli`/`pw-metadata`
  call has a timeout, and the wait for a new sink is capped at 5 s.
- The daemon keeps answering IPC, signals and window events during a slow profile switch,
  and a failed rule-driven switch is retried (after 2 s and 5 s) instead of leaving the
  audio on the wrong sink while `status` claimed success.
- `match_by_index` was ignored when a window opened; priority after a reload is now
  deterministic.
- Closed windows leaked Wayland objects for the daemon's lifetime; a compositor that
  finishes or disconnects now makes the daemon exit with an error (so systemd restarts
  it) instead of running deaf; a slow daemon can no longer get disconnected by the
  compositor; spurious change events on focus are gone.
- Config reload dropped editors' final write; it is now debounced reliably.
- The TUI could save configs that then failed to load; every save is validated and
  checked to load back identically.
- TUI: edits made while a save was running were marked as saved; quitting during a save
  could lose it; inverted page keys and clipped indicators in the Windows view; the log
  view now stays at the newest line; a log tailer that froze on invalid UTF-8 or re-read
  the file every second; panics on non-ASCII text and very narrow terminals.
- TUI daemon control: Start reports the real result, Stop/Restart act on the daemon the
  way it was actually started (systemd or direct).
- `list-sinks` warns when the config cannot be loaded instead of silently hiding the
  configured column.
- Notification bodies no longer show literal `&amp;` on servers without markup support.
- CLI output no longer panics on a closed pipe or stderr.

### Removed

- The `default.audio.sink` metadata write (replaced by `default.configured.audio.sink`).
- The PID file (`pwsw.pid`) and the `/tmp/pwsw-<uid>.sock` socket fallback.
- Debug builds no longer append to a log at a fixed `/tmp` path, and the
  `PWSW_ALLOW_CONFIG_WRITE` switch is gone.
- Advice to restart the daemon after config changes (they apply automatically).

### Security

- Window titles and `app_id`s are treated as untrusted: control characters are escaped
  and long titles truncated in logs (no forged log lines), and notification text cannot
  inject markup. Notification icons from `app_id`s must be plain icon names.
- IPC: both client and daemon check that the peer runs as the same user; the socket is
  never reachable by other users, even briefly; stale-socket cleanup only removes a
  socket nobody listens on and only the exact file it checked.
- An empty or relative `XDG_RUNTIME_DIR` no longer puts the socket in the current
  directory.
- Config files are always written atomically with mode `0600` (`init` and first-run
  creation used `0644` before).
- The systemd unit is sandboxed (`ProtectSystem=strict`, `NoNewPrivileges`,
  `MemoryDenyWriteExecute`, `PrivateTmp`, `UMask=0077`, ...).
- Dependencies updated, clearing all `cargo audit` advisories; the unmaintained
  `tui-logger` dependency chain was dropped.
