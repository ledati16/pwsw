# Changelog

All notable user-visible changes to PWSW are documented here.
The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the
project uses [Semantic Versioning](https://semver.org/) (pre-1.0: minor versions may
contain breaking changes).

## [0.7.0] - 2026-11-20

A full overhaul focused on correctness, safety and predictable behaviour, plus
microphone switching, fallback sinks, "audio follows the screen" rules, pause/resume,
a live status stream for bars and `pwsw doctor`. After this release PWSW is in
maintenance mode.

**Before upgrading**, note the behaviour changes (details under **Changed**):

- Some configs that 0.6.0 accepted are now rejected (unknown keys, empty or numeric
  descriptions, ambiguous references); run `pwsw validate` after upgrading.
- The daemon switches less eagerly: only when the rules' answer changes, and at
  start-up only with `default_on_startup = true`.
- No command except `pwsw init` and `pwsw tui` creates a config file any more.
- `status --json` has a new schema.
- Restart the daemon after upgrading (`systemctl --user restart pwsw`): the IPC
  protocol changed.

### Changed

- **The daemon switches only when the rules' answer changes** (a matching window opens,
  closes, changes title or moves to another monitor, or a config change alters the
  rules). A sink you choose in pavucontrol, or one WirePlumber picks, is no longer
  undone by the next unrelated window event. A manual `set-sink`/`next-sink`/`prev-sink`
  holds the same way.
- **`default_on_startup = false` is now honoured**: the daemon no longer switches on the
  first window event after start-up. With `true` it switches to the sink the rules want
  (the default sink if nothing matches). Either way the daemon first waits (up to 2 s)
  for the compositor to list the windows already open and decides from all of them, so
  start-up switches at most once instead of once per existing window. Window events
  that queue up are applied together, so a window opening on a monitor an `output` rule
  names switches once, straight to that rule's sink.
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
  could silently undo; that write is gone.
- **Profile switches are saved in WirePlumber** (so it keeps the profile), only
  available profiles are used, and a profile that keeps the card's microphone is
  preferred.
- **`next-sink`/`prev-sink` skip sinks that cannot be activated right now** (e.g. an
  HDMI output whose display is off) and start from the real PipeWire default.
  `set-sink` still tries the sink you asked for and reports why it failed (a
  disconnected Bluetooth device says so). A failed manual switch leaves the audio where
  it was.
- Manual switches go through the daemon when it is running, so its rules see your
  choice; without a daemon they switch PipeWire directly.
- **`status --json` has a new schema** and always prints exactly one JSON document with
  exit status 0: `current_sink`, `current_source`, `sources_configured`, `daemon`
  (`running`, `error`, `version`, `protocol`, `uptime_secs`, `daemon_sink`, `paused`,
  `backend`, ...), `pipewire` (`current_sink`, `current_source`, `error`) and `config`
  (`path`, `error`). Fields that cannot be determined are `null`, never missing. See
  `man pwsw` (STATUS JSON).
- **The IPC protocol is versioned** (now version 2). `pwsw status` reports a daemon
  that speaks another version, and a daemon that does not understand a newer client's
  request answers with a restart hint.
- **IPC socket fallback moved** from `/tmp/pwsw-<uid>.sock` to
  `/tmp/pwsw-<uid>/pwsw.sock` (used only when `XDG_RUNTIME_DIR` is unset, empty or not
  absolute). The directory must be ours with mode `0700`.
- **The PID file (`pwsw.pid`) is replaced by a lock file** (`pwsw.lock`, next to the
  socket), held with `flock` so a crashed daemon never blocks the next start.
- Errors are printed as one readable `error: ...` line (exit status 1; usage errors
  exit 2). Set `RUST_BACKTRACE=1` for the full report.
- Rule regexes are no longer checked against a list of "dangerous" constructs (Rust's
  regex engine is linear-time); only the compiled size is limited. The TUI editor,
  `test-rule` and config loading accept exactly the same patterns.
- The background daemon (`pwsw daemon`) runs in its own process group, so it survives
  the terminal or TUI that started it; if start-up fails, the error and the last log
  lines are printed.
- systemd unit: part of `graphical-session.target`, `Type=notify` with the current sink
  (and microphone) in `systemctl --user status`, `Wants=` instead of binding to
  `pipewire.service` (a PipeWire restart no longer stops pwsw), at most 5 restarts per
  minute, and sandboxed.
- Rule log lines name the window: `Rule matched: window 12 'kitty' "~/src" → Headphones`
  (title shortened), so lines for several windows of one app can be told apart.
- **TUI redesign**: a one-row header and footer (status messages and key hints from
  the real key bindings), forms and dialogs sized to their content, a theme that uses
  the terminal's own text colour (readable on light themes) and honours `--color never`
  and `NO_COLOR`, a Sinks table with a *State* column (playing, present, via profile,
  missing), and a "terminal too small" notice below 60×16. It is also much lighter: it
  no longer runs `pw-dump` while drawing, sleeps while idle, and shows rule previews
  immediately.
- Building requires **Rust 1.100** (was 1.94); the toolchain is pinned in
  `rust-toolchain.toml`.

### Added

- **Microphone (source) switching**: an optional `[[sources]]` list (same keys as
  `[[sinks]]`, exactly one `default`) and a rule key `source = "Headset Mic"` (or a
  fallback list, like `sink`) make the default input follow the rules too. The
  microphone is an independent dimension: the highest-priority matching window *whose
  rule has a `source`* decides it, else the default source, so a sink-only rule never
  moves the mic. Fallbacks, upgrades, retries, manual choice, pause and the settle delay
  work for it exactly as for sinks. When one switch needs a sink and a source on the
  same card, the card is switched once to a profile providing both (if none exists the
  sink wins and the source is reported unreachable); a sink-only switch of the managed
  microphone's card is planned the same way (and a microphone it drops is reported as
  unknown rather than still claimed); a source-only switch always keeps the card's
  current output. Without `[[sources]]` pwsw never touches the default source. New
  `pwsw set-source <ref>`; `pwsw list-sinks` gains an *INPUT DEVICES / SOURCES* section
  (and `*_sources` JSON fields); `status` shows the input; `list-windows` and
  `test-rule` show a rule's sources; the systemd status reads `Sink: X · Mic: Y`. In
  the TUI `i` on the Sinks screen switches to a mic list that works like the sink
  list, the rule editor has an optional **Mics** field picked like **Sinks**, the rules
  table gains a *Mic* column and the Dashboard shows the current mic.
- **Fallback sinks per rule**: `sink = ["BT Headphones", "Headphones"]` uses the first
  sink that can be activated right now, and the next one if activating it fails. When a
  fallback was used, the next window event or reload switches up once an earlier sink
  is available again (no polling). Status, the systemd status line, notifications and
  logs mark a fallback as such; `list-windows` and `test-rule` show a rule's fallbacks;
  `status --json` has `daemon.daemon_sink_fallback`. A plain string still works and is
  what single-sink rules are saved as. In the TUI the rule editor's **Sinks** field
  picks the sinks and their order (`Space` adds/removes, `Shift+↑/↓` reorders), and the
  rules table and details show them as `BT Headphones → Headphones`.
- **Audio follows the screen (`output` rules)**: a rule key `output = "HDMI-A-1"` (a
  regex, like `app_id`) matches the connector names of the monitors a window is on,
  while it is not minimized; `app_id` is now optional when `output` is set (a rule
  needs at least one of them). Moving a window onto a matching monitor counts as fresh
  activity, like a title change that starts matching. Monitor information comes only
  from `wlr-foreign-toplevel-management`, so with `output` rules the daemon prefers it
  over `ext`; on an ext-only compositor these rules never match and the daemon warns.
  Adding the first `output` rules to a running daemon that uses `ext` needs a restart
  (it logs a warning). `list-windows` shows each window's monitors
  (`outputs`/`minimized` in the JSON), `status` names the compositor protocol
  (`daemon.backend`, `daemon.monitor_info` in `status --json`), `validate` notes the
  `wlr` requirement, `pwsw test-rule --output REGEX` tests an `output` condition
  against open windows (alone or with an `app_id`/title pattern) and `pwsw doctor`
  reports whether monitor rules are available. In the TUI the rule editor has an
  **Output** field (App ID may then stay empty, shown as *any app*; the monitors seen
  are listed and the live preview matches like the daemon), the rules table shows such
  rules as `any app on HDMI-A-1`, and the Windows view has a *Monitor* column.
- **Pause / resume**: `pwsw pause`, `pwsw resume` and `pwsw toggle-pause` stop and
  restart rule-driven switching. While paused the daemon keeps tracking windows and
  reloading the config, and manual switches still work; resuming switches at once if
  the rules want another sink than the current one. The pause is not kept across
  daemon restarts. `pwsw status` shows it, `status --json` has `daemon.paused`, the
  systemd status line ends in `(paused)`, and with `notify_manual` pausing and resuming
  are notified. In the TUI `p` pauses and resumes (on any screen), the header shows
  `⏸ paused` and the Dashboard's daemon card `● running · paused`.
- **`switch_delay_ms` setting** (default 0, up to 5000): a rule-driven switch happens
  only if the rules still want the same sink after this delay, so a splash screen or a
  flickering window title no longer switches back and forth. Manual switches, retries,
  the start-up switch and `resume` are not delayed. The TUI's Settings screen edits it
  as **Switch Delay** (Enter, type the milliseconds, Enter).
- **Live status stream for status bars and shells**: `pwsw status --watch --json`
  prints one JSON line now and on every change of the output, microphone, deciding rule
  or pause state (pushed by the daemon, no polling), with the conventional
  `text`/`tooltip`/`class`/`alt` fields (`class`: `active`, `fallback`, `paused`,
  `stopped`) plus structured `sink`, `source`, `paused`, `daemon`, `version`,
  `protocol` and `managed_by` fields; the schema is documented as stable in `man pwsw`
  (STATUS STREAM). The deciding rule is named by its `desc` (else its position) and the
  window's `app_id`, never by the window title, so browser tab changes neither print
  new lines nor put page titles into the bar. It keeps running while the daemon is
  down (one `stopped` line, then reconnects with backoff) and exits quietly on
  `SIGINT`/`SIGTERM` or a closed pipe. Without `--json` it prints
  `Headphones · Mic: Headset Mic · paused`. The daemon serves up to 16 followers and
  drops one that stops reading. A Noctalia plugin is in `contrib/noctalia/`; the README
  shows Waybar, Quickshell and eww setups.
- **`pwsw doctor`**: one read-only command that checks the Wayland session and the
  compositor's window protocols, the PipeWire tools, version and WirePlumber, the
  config, whether each configured sink and source can be activated now (and by which
  profile switch; sources in their own *Mics* section, only when configured), the
  daemon (version, protocol, systemd or direct), the systemd unit and the last log
  warning, printing ✓/⚠/✗ lines with a one-line fix for each problem. Exits 1 if
  anything is broken.
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
- TUI: unsaved-changes protection on quit (`q` or `Ctrl+C`) and when cancelling an
  edit, a "changed on disk" dialog, bracketed paste into text fields, scrollbars, and
  help generated from the real key bindings.
- TUI: `n` in the Dashboard's Windows view makes a rule for the selected window (its app
  ID escaped and anchored, its title shown to copy from), or opens the rule that already
  matches it. The window list now has a selection (`↑/↓`, `PgUp/PgDn`, `Home/End`).

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
- A panic of the background daemon was lost; it is now recorded in `daemon.log`.
- The TUI could save configs that then failed to load; every save is validated and
  checked to load back identically.
- TUI: edits made while a save was running were marked as saved; quitting during a save
  could lose it; inverted page keys and clipped indicators in the Windows view; the log
  view now stays at the newest line; a log tailer that froze on invalid UTF-8 or re-read
  the file every second; panics on non-ASCII text and very narrow terminals; forms
  whose fields collapsed on 80×24 terminals.
- TUI daemon control: Start reports the real result, Stop/Restart act on the daemon the
  way it was actually started (systemd or direct).
- `list-sinks` warns when the config cannot be loaded instead of silently hiding the
  configured column.
- Notification bodies no longer show literal `&amp;` on servers without markup support.
- CLI output no longer panics on a closed pipe or stderr.

### Removed

- The `default.audio.sink` metadata write, the PID file and the `/tmp/pwsw-<uid>.sock`
  socket fallback (replaced as described under **Changed**).
- Debug builds no longer append to a log at a fixed `/tmp` path, and the
  `PWSW_ALLOW_CONFIG_WRITE` switch is gone.

### Security

- Window titles and `app_id`s are treated as untrusted: control characters are escaped
  and long titles truncated in logs, CLI output and the status stream (no forged log
  lines), and notification text cannot inject markup. Notification icons from
  `app_id`s must be plain icon names.
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
  `tui-logger` dependency chain was dropped, and `color-eyre` (with its backtrace and
  span-trace crates) was replaced by `anyhow`.
