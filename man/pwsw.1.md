# pwsw(1)

## NAME
pwsw - PipeWire Switcher daemon and client

## SYNOPSIS
**pwsw** [*OPTIONS*] [*COMMAND*]

## DESCRIPTION
**pwsw** is a daemon that automatically switches PipeWire audio sinks based on active windows in Wayland compositors. It monitors window events using standard protocols and uses PipeWire native tools for audio control.

It can also be used as a command-line client to query the daemon status, list windows, and manually switch sinks.

Optionally it also switches the default *source* (microphone): with **[[sources]]** in the configuration, a rule may name a *source* as well, and the microphone follows the highest-priority matching window whose rule has one, independently of the sink (see **pwsw**(5)). Without **[[sources]]** pwsw never touches the default source.

The daemon switches when the sink its rules want *changes* (a matching window opens, closes or changes title, or a reload changes the rules). It does not fight other programs: a default sink changed elsewhere (e.g. with **pavucontrol**, or by WirePlumber) stays until the rules' answer changes again. A manual switch holds the same way. A failed rule-driven switch is retried after 2 s and 5 s, then once more on the next window event or reload. Under systemd the daemon reports the current sink as its status line (**systemctl --user status pwsw**), as *Sink: X · Mic: Y* when sources are configured. Sources follow the same rules (switch on a change of their answer, retries, fallbacks, manual choice, pause, settle delay), each dimension on its own.

## COMMANDS
**daemon** [*OPTIONS*]
:   Start the PWSW daemon.

    Without **--foreground**, starts the daemon as a detached background process (in its own process group, so it survives the terminal closing) and waits until it answers; on failure the error and the last log lines are printed.

    With **-f**/**--foreground** it runs in the foreground instead (useful for systemd or debugging); logs then also go to stderr.

**status** [*--json*] [*--watch*]
:   Show the current audio output (queried from PipeWire directly; with sources configured also the current input) and the daemon's status: uptime, version, whether rule-driven switching is paused, active rule, the microphone it applied (with sources configured) and number of tracked windows. Works without a running daemon; this is also what **pwsw** without a command does. A part that cannot be determined (configuration, PipeWire, daemon) is reported and the rest is still shown; the human-readable form then exits with status 1 and prints the problems to stderr as *error:* lines.

    With **--json**, a single JSON document is always printed (exit status 0), even when PipeWire or the daemon cannot be reached; see **STATUS JSON**.

    With **--watch**, follow the daemon's status instead: one line now and one on every change, for status bars and shells; with **--json** each line is a JSON object. It keeps running while the daemon is stopped; see **STATUS STREAM**.

**tui**
:   Launch the interactive Terminal User Interface for configuration and monitoring. On first run it creates the commented default configuration file (like **pwsw init**).

**list-sinks** [*--json*]
:   List all active and profile-switchable PipeWire sinks and the configured sinks' status, then the same for input devices in an *INPUT DEVICES / SOURCES* section (sources; monitors of sinks are not listed). Inputs reachable by a profile switch are those a switch can provide while keeping the card's current output. There is no separate **list-sources** command. With **--json**, the object has *active_sinks*, *profile_sinks*, *configured_sinks*, *current_default*, and the same for sources: *active_sources*, *profile_sources*, *configured_sources*, *current_default_source*.

**list-windows** [*--json*]
:   List all open windows the daemon sees, split into tracked (matching a rule, with the sink they selected and its fallback sinks, if any, and the rule's sources) and untracked ones. A *monitor* line shows the connector names of the monitors a window is on (`HDMI-A-1, DP-1`) and *(minimized)*, the names **output** rules match (see **pwsw**(5)); it is missing when the daemon does not know (with the **ext** protocol it never does). In the JSON output each window has *outputs* (an array, empty when unknown) and *minimized*, and a tracked window's *tracked.sources* lists the rule's sources (first choice first), omitted when the rule has none. Requires a running daemon.

**test-rule** [*PATTERN*] [*--output* *REGEX*] [*--json*]
:   Test a regex pattern against the *app_id* and title of every open window and show which matched, and on which field; for a window that currently matches a rule, also that rule's sinks (with fallbacks) and sources. With *--output*, a window must also be on a monitor whose connector name matches *REGEX* and not be minimized, exactly like a rule's **output** condition (so it never matches with the **ext** protocol, which reports no monitors); *PATTERN* may then be left out to test the output condition alone. Requires a running daemon. See **pwsw**(5) for details on regex syntax.

**validate**
:   Validate the configuration file syntax and sink references. Reports a missing file (it does not create one). When rules use **output**, it adds a note that they need a compositor with **wlr-foreign-toplevel-management** (it does not check the compositor; **pwsw doctor** does).

**doctor**
:   Check the whole setup and print a plain-language report: one line per check, marked **✓** (works), **·** (for information), **⚠** (works, but probably not as intended) or **✗** (broken), each problem followed by a one-line hint on how to fix it. It checks, in order: the Wayland session (*WAYLAND_DISPLAY*, the connection, which of *ext_foreign_toplevel_list_v1* and *zwlr_foreign_toplevel_manager_v1* the compositor advertises with their versions, and which one the daemon would use, which depends on whether the configuration has rules with **output**; then whether *monitor rules* are available, i.e. the compositor offers **wlr**, with a **⚠** when the configuration has such rules but the compositor offers only **ext**); the PipeWire tools (**pw-dump**, **pw-cli**, **pw-metadata**), the PipeWire version and whether WirePlumber is connected; the configuration file (which path is used and why, whether it exists and validates, deprecated positional sink references, the number of sinks and rules); each configured sink (present, available through a profile switch and which one, or why not), from a single **pw-dump**; the daemon (running, its version and IPC protocol compared with this **pwsw**, whether systemd manages it, its current sink, and a **⚠** when it runs on **ext** although the configuration now has rules with **output** and the compositor offers **wlr**: restart it); the systemd user unit (installed, enabled, active; skipped without **systemctl**); and the log file with its last warning or error. Colours follow **--color** and *NO_COLOR*.

    Read-only: it never changes the audio state, the configuration or any service, and creates no files. Exits with status 0 when no check is **✗**, 1 otherwise (warnings do not count).

**init** [*--force*]
:   Write the commented default configuration file. Refuses if a file already exists; with **--force** it is replaced and the old file kept as *config.toml.bak*. **pwsw tui** also creates this file on first run; no other command does.

**set-sink** *SINK*
:   Set audio output by description or node name (a 1-indexed position such as "2" also works, but is deprecated like positional references in the configuration). If `set_smart_toggle` is enabled in config and the target sink is already active, toggles back to the default sink; "already active" means the current PipeWire default. The sink is always attempted, and its error reported, even if it cannot be activated right now. A failed switch leaves the audio where it was. When the daemon runs, the switch goes through it (so its rules see the manual choice); otherwise PipeWire is switched directly.

**set-source** *SOURCE*
:   Set the default input (microphone) to a configured source, by description or node name (a deprecated position also works). Needs **[[sources]]** in the configuration. Like **set-sink**: with `set_smart_toggle`, asking for the source that is already the PipeWire default switches to the default source; the source is always attempted; a failed switch changes nothing; when the daemon runs the switch goes through it, and the choice holds until the rules' *source* answer changes (sink-only changes do not end it). A source on an ALSA card whose current profile lacks its input is reached by a profile switch that keeps the card's current output; if none exists, the error says so.

**next-sink**
:   Cycle to the next configured sink after the current PipeWire default (wraps around), skipping sinks that cannot be activated right now (e.g. an HDMI output whose display is off). If no other sink can be activated, says why and changes nothing. Through the daemon when it runs, like **set-sink**.

**prev-sink**
:   Like **next-sink**, in the other direction.

**pause**
:   Stop rule-driven switching. The daemon keeps tracking windows and applying configuration changes, and manual switches (**set-sink**, **next-sink**, **prev-sink**) still work, but no rule switches the sink (or the source) until **resume**. Pending retries and settle waits (*switch_delay_ms*) are dropped. The pause is kept in memory only: a restarted daemon starts unpaused. Requires a running daemon; under systemd the status line ends in *(paused)*.

**resume**
:   Resume rule-driven switching. This counts as a change of the rules' answer: if the sink (or source) the rules want now is not the current one, the daemon switches at once (without the settle delay), also over a manual choice. Requires a running daemon.

**toggle-pause**
:   **pause** when running, **resume** when paused (e.g. for a key binding). Requires a running daemon.

**shutdown**
:   Gracefully stop the running daemon.

**completions** *SHELL*
:   Print a completion script for *SHELL* (**bash**, **zsh**, **fish**, **elvish** or **powershell**) to stdout, e.g. **pwsw completions bash > ~/.local/share/bash-completion/completions/pwsw**. Packages install them for bash, zsh and fish.

## CONFIGURATION CHANGES
The daemon watches its configuration file and applies changes automatically when the file is saved (also on **SIGHUP**). If the file is a symlink (e.g. managed by **stow** or **chezmoi**), changes to its target are seen too, also after the symlink is pointed elsewhere. An invalid file is rejected and the previous configuration stays in effect; the error is logged and, with *notify_manual* enabled, shown as a notification. Restarting the daemon is only needed after upgrading **pwsw**: **pwsw status** reports a daemon that speaks a different IPC protocol, and a daemon that does not understand a newer client's request answers with its version and a hint to restart it.

## STATUS JSON
**pwsw status --json** prints one object. Fields that cannot be determined are *null*, never missing:

```
{
  "current_sink": { "name": "...", "description": "..." } | null,
  "current_source": { "name": "...", "description": "..." } | null,
  "sources_configured": true | false,
  "daemon": {
    "running": true | false,
    "error": null | "...",
    "version": "v0.7.0+abc1234" | null,
    "protocol": 2 | null,
    "uptime_secs": 3600 | null,
    "uptime_human": "1h" | null,
    "daemon_sink": "..." | null,
    "daemon_sink_fallback": true | false | null,
    "daemon_source": "..." | null,
    "daemon_source_fallback": true | false | null,
    "active_window": "..." | null,
    "tracked_windows": 2 | null,
    "paused": true | false | null,
    "backend": "ext" | "wlr" | null,
    "monitor_info": true | false | null
  },
  "pipewire": {
    "current_sink": { "name": "...", "description": "..." } | null,
    "current_source": { "name": "...", "description": "..." } | null,
    "error": null | "..."
  },
  "config": { "path": "..." | null, "error": null | "..." }
}
```

*current_sink* is the PipeWire default sink (its configured description if it is a configured sink, else its node name); it is the same as *pipewire.current_sink* and is *null* when PipeWire could not be queried (*pipewire.error* says why). *daemon.running* is false when no daemon answers on the socket; *daemon.error* is set when one runs but could not report its status, or speaks a different IPC protocol than this client (restart it). *daemon.daemon_sink* is the sink the daemon last applied (its configured description), or *null* while the daemon does not know the default sink (it could not query it at startup and has not switched since). *daemon.daemon_sink_fallback* is true when that sink is a fallback a rule settled for because its earlier sinks could not be activated (*null* together with *daemon_sink*); *daemon.active_window* then also ends in *(fallback)*. *daemon.paused* is true while rule-driven switching is paused (**pwsw pause**), *null* when no daemon status could be read. *current_source* (also *pipewire.current_source*) is the PipeWire default source the same way, *null* when there is none; *sources_configured* says whether the configuration has **[[sources]]**. *daemon.daemon_source* is the source the daemon last applied (configured description), *null* without sources configured or while unknown, and *daemon.daemon_source_fallback* is true when it is a fallback of the deciding rule. *daemon.backend* is the compositor protocol the daemon uses (**ext-foreign-toplevel-list** or **wlr-foreign-toplevel-management**) and *daemon.monitor_info* whether it reports which monitors windows are on, so rules with **output** can match; both are *null* when no daemon status could be read or the daemon predates them. *config.error* is set when the configuration file could not be loaded (descriptions then fall back to node names).

## STATUS STREAM
**pwsw status --watch** asks the daemon to report its status and every change of it (nothing is polled), and prints one line per update to stdout, flushed at once. Without **--json** the line reads *Headphones · Mic: Headset Mic · paused* (the microphone only with sources configured, *(fallback)* after a fallback, *paused* while paused) or *pwsw stopped*.

The command never exits because the daemon is gone: when it cannot connect, or the connection drops (the daemon stopped or is restarting), it prints one *stopped* line, retries with a backoff from 250 ms up to 5 s, and prints the fresh state once the daemon answers again. Identical lines are never printed twice in a row. It exits with status 0 on **SIGINT** or **SIGTERM**, or when nobody reads its output any more (a closed pipe, noticed within a second even while nothing changes). The daemon serves at most 16 followers at once and drops one that stops reading.

With **--json** every line is one JSON object. This schema is stable: fields are only ever added.

```
{"text":"Headphones",
 "tooltip":"Output: Headphones (alsa_output.usb-...)\nMic: Headset Mic\nRule: discord",
 "class":"active", "alt":"active",
 "sink":{"desc":"Headphones","node":"alsa_output.usb-...","fallback":false,
         "window":{"app_id":"discord","title":"...","rule":"discord"}},
 "source":{"desc":"Headset Mic","node":"alsa_input.usb-...","fallback":false,"window":{...}} | null,
 "paused":false, "daemon":"running",
 "version":"v0.7.0+abc1234", "protocol":2, "managed_by":"systemd" | "direct"}
```

(shown wrapped; each object is printed on a single line). *text*, *tooltip*, *class* and *alt* are the conventional status-bar fields. *class* and *alt* are *active*, *fallback* (the sink or the source is a fallback a rule settled for), *paused* (wins over *fallback*) or *stopped*. *text* is the description of the sink the daemon applied (*unknown* while it does not know the default sink). *tooltip* has an *Output:* line, a *Mic:* line with sources configured, *Rule:* (and *Mic rule:* when another window decides the microphone) naming the deciding rule (its *desc*, else the window title it matched), and *Paused: rules do not switch* while paused. *sink* and *source* are what the daemon last applied: *desc* (configured description, else the node name), *node*, *fallback*, and *window*, the window whose rule chose it (*null* for the default, a manual choice, or a default found at startup); window text is sanitised (control characters escaped, at most 200 characters). *sink* is *null* while the default sink is unknown; *source* is *null* without **[[sources]]** or while unknown.

When no daemon answers, the line is:

```
{"text":"pwsw stopped","tooltip":"pwsw daemon is not running","class":"stopped",
 "alt":"stopped","daemon":"stopped","sink":null,"source":null,"paused":false}
```

When a daemon answers but refuses the stream (too many followers, or an older daemon that does not know it: restart it), *daemon* is *error*, *text* is *pwsw error* and *tooltip* says why; *class* is *stopped*, and the command keeps retrying.

## COMPATIBILITY
**pwsw** relies on standard Wayland protocols to monitor windows.

**Supported:**
*   **ext-foreign-toplevel-list-v1**: The official Wayland standard.
*   **wlr-foreign-toplevel-management**: Used by Sway, Hyprland, River, Wayfire, and others.

Only **wlr-foreign-toplevel-management** reports which monitor a window is on, so rules with **output** (see **pwsw**(5)) need it; with such rules the daemon prefers it over **ext** when the compositor offers both.

**Not Supported:**
*   **GNOME / Mutter**: Does not expose window management protocols.
*   **KDE Plasma 6**: Removed protocol support (pending standard implementation).

## TUI KEYS
The most important keys are listed below. Press `?` or `F1` within the TUI for the complete, context-sensitive list (it is generated from the same table the key handling uses).

**Global**
:   `Tab` / `Shift+Tab`: Next / previous screen
:   `1-4`: Jump to screen (Dashboard, Sinks, Rules, Settings)
:   `Ctrl+S`: Save the configuration file
:   `q` or `Ctrl+C`: Quit (with unsaved changes: press again to confirm)
:   `p`: Pause / resume rule-driven switching (like **toggle-pause**; needs a running daemon). While paused the header shows *⏸ paused* on every screen and the Dashboard's daemon card says *running · paused*
:   `Esc`: Clear status message
:   `?` or `F1`: Toggle the help overlay

**Dashboard**
:   `←/→`: Select daemon action (Start/Stop/Restart; Enable/Disable when the systemd unit is installed)
:   `Enter`: Run the selected action
:   `w`: Toggle between Logs and Windows view
:   `↑/↓`, `PgUp/PgDn`, `Home/End`: Scroll the log (`End` returns to live view), or select a window in the Windows view
:   `n` (Windows view): New rule for the selected window, its app ID escaped and anchored so it matches that app ID only; the Sinks field is focused (preselected when there is only one sink) and the window's title is shown to copy from. If a rule already matches the window, that rule opens for editing instead

**Sinks and Rules lists**
:   `↑/↓`: Navigate
:   `Shift+↑/↓`: Move the selected item (rule order is the priority with *match_by_index*)
:   `a` / `e` / `x`: Add / edit / delete
:   `Enter`: Show details
:   `Space` (Sinks): Make the selected sink the fallback (used when no rule matches)

**Editors**
:   `↑/↓`, `Tab` / `Shift+Tab`: Switch field
:   `Space`: Open the node list (sink *Node*) or the sink picker (rule *Sinks*); toggle or cycle on/off fields
:   `Enter`: Apply the edit (written to the file with `Ctrl+S`)
:   `Esc`: Cancel editing / close a list

**Rule sink picker** (a rule's *Sinks*: the first is tried first, the rest are fallbacks in order)
:   `Space`: Add the highlighted sink at the end of the order, or remove it (the next one moves up)
:   `Shift+↑/↓`: Move the highlighted chosen sink earlier / later
:   `Enter`: Use the chosen sinks (with none chosen, the highlighted one); `Esc`: back without changes

**Settings**
:   `↑/↓`: Navigate
:   `Enter` / `Space`: Toggle the setting, open the log level list, or edit the switch delay (*switch_delay_ms*)
:   In the switch delay editor: type the milliseconds (digits only), `Enter` applies a value from 0 to 5000 (anything else is refused with the reason), `Esc` cancels
:   `PgUp/PgDn`: Scroll the description

**Text fields**
:   `←/→`, `Ctrl+←/→`, `Home/End`: Move the cursor
:   `Ctrl+W` / `Ctrl+Delete`: Delete the previous / next word
:   `Ctrl+U`: Clear the field; `Ctrl+K`: delete to the end

If the configuration file changed on disk since the TUI loaded it, saving asks whether to overwrite it or reload it from disk.

## OPTIONS
**--config** *PATH*
:   Use *PATH* as the configuration file instead of the default location. Takes precedence over **PWSW_CONFIG**. Accepted before or after the command; when **pwsw** starts the daemon itself (background mode or the TUI), the same path is passed on. Use **pwsw --config** *PATH* **init** to create a configuration there.

**--color** *WHEN*
:   When to colour output: **auto** (the default) colours a stream only if it is a terminal and **NO_COLOR** is unset or empty (**CLICOLOR_FORCE** forces colour on); **always** and **never** override all of that. Decided separately for stdout and stderr.

**-h**, **--help**
:   Print help information.

**-V**, **--version**
:   Print the version: the release, plus the git commit (and *-dirty* for uncommitted changes) when built from a git checkout, e.g. *pwsw 0.7.0+abc1234*.

## ENVIRONMENT
**PWSW_CONFIG**
:   Path to the configuration file to use when **--config** is not given. Ignored when empty.

**XDG_CONFIG_HOME**
:   Base directory for the default configuration file (*$XDG_CONFIG_HOME/pwsw/config.toml*, normally *~/.config/pwsw/config.toml*).

**WAYLAND_DISPLAY**
:   The name of the Wayland display to connect to.

**NO_COLOR**
:   When set and not empty, no colours or other styling (unless **--color always**).

**CLICOLOR_FORCE**
:   When set and not *0*, colour output even when it is not a terminal (unless **NO_COLOR** is set, or **--color never**).

**RUST_LOG**
:   When set, a **tracing** filter (e.g. *pwsw=debug*) that overrides *log_level* from the configuration, also across reloads.

**RUST_BACKTRACE**, **RUST_LIB_BACKTRACE**
:   When set (and not *0*), errors are printed as full reports with a backtrace instead of a one-line *error:* message.

**XDG_RUNTIME_DIR**
:   Directory for the IPC socket (`pwsw.sock`) and the single-instance lock file (`pwsw.lock`). If unset, empty or not an absolute path, both are placed in */tmp/pwsw-<uid>/* instead (see **FILES**).

**PROFILE_SWITCH_DELAY_MS**
:   Interval in milliseconds between checks for the new sink after a profile switch (default: 150, clamped to 10-1000).

**PROFILE_SWITCH_MAX_RETRIES**
:   Number of checks for the new sink after a profile switch (default: 10). The total wait (interval × checks) is capped at 5 seconds.

## FILES
*$XDG_CONFIG_HOME/pwsw/config.toml*
:   The default configuration file (normally *~/.config/pwsw/config.toml*; override with **--config** or **PWSW_CONFIG**). See **pwsw**(5) for details.

*~/.local/share/pwsw/daemon.log*
:   Daemon log file (rotated at 1 MB to *daemon.log.old*). Written in every mode; the background daemon started by **pwsw daemon** also appends its start-up errors here. Under systemd the same lines also go to the journal (**journalctl --user -u pwsw.service**).

*$XDG_RUNTIME_DIR/pwsw.sock*
:   IPC socket (mode 0600). Fallback: */tmp/pwsw-<uid>/pwsw.sock*, in a directory created with mode 0700; pwsw refuses to use that directory if it is a symlink, owned by another user, or has a different mode. Both sides check that the peer runs as the same user.

*$XDG_RUNTIME_DIR/pwsw.lock*
:   Single-instance lock (mode 0600), held with **flock**(2) while the daemon runs and released by the kernel when it exits, so a crashed daemon never blocks the next start. Lives next to the socket (fallback: */tmp/pwsw-<uid>/pwsw.lock*).

*contrib/pwsw.service*
:   systemd user unit (install to *~/.config/systemd/user/*). Part of *graphical-session.target*, restarts on failure, sandboxed.

## EXIT STATUS
**0**
:   Success.

**1**
:   An error (printed to stderr). For **status**, also when part of the status could not be determined (not with **--json**). For **doctor**, when a check failed (a **✗** line). **pwsw daemon** exits non-zero when it fails to start or stops on an error.

**2**
:   Invalid command-line usage.

## BUGS
See <https://github.com/ledati16/pwsw/issues>

## SEE ALSO
**pwsw**(5), **pw-dump**(1), **pw-metadata**(1), **pw-cli**(1)
