# pwsw(5)

## NAME
pwsw.toml - PWSW configuration file format

## DESCRIPTION
PWSW uses a TOML configuration file, by default *$XDG_CONFIG_HOME/pwsw/config.toml* (normally *~/.config/pwsw/config.toml*). **pwsw --config** *PATH* or the **PWSW_CONFIG** environment variable select a different file. The file defines global settings, audio sinks, and window matching rules.

Create it with **pwsw init** (a commented template) or **pwsw tui**; no other command creates it. Check it with **pwsw validate**. Unknown keys are an error: a typo such as `defualt = true` is reported with the key name, its line, and the file path.

When the TUI saves the file, comments and formatting are preserved (comments above or beside an entry move with it, and are removed with it), the previous version is copied to *config.toml.bak* next to the file, a symlinked file stays a symlink (its target is replaced), and the save is refused if the file was changed on disk since it was loaded. Saved files have mode 0600.

## SETTINGS
The **[settings]** section controls global daemon behavior. The section and every key in it are optional.

**default_on_startup** (boolean, default: false)
:   Whether to switch when the daemon starts to the sink the rules want (the default sink if no open window matches). If false, the current sink stays until the rules want a different sink (e.g. a matching window opens; windows already open when the daemon starts count as opening).

**set_smart_toggle** (boolean, default: true)
:   If true, calling `set-sink` on a sink that is already the PipeWire default will toggle back to the default sink.

    *Example:* If "Headphones" are active, `pwsw set-sink "Headphones"` switches back to the default sink (e.g., "Speakers").

**notify_manual** (boolean, default: true)
:   Show desktop notifications for manual sink switches and daemon events (start, configuration reloaded or rejected).

**notify_rules** (boolean, default: true)
:   Show desktop notifications for automatic rule-based switches. A rule's own *notify* key overrides it.

**match_by_index** (boolean, default: false)
:   Which window decides the sink when several match (see **PRIORITY**). If false, the most recently opened (or most recently matching) window wins. If true, the window matching the earliest rule in the list wins.

    *Tip:* Rules can be reordered in the TUI using `Shift+Up/Down` to adjust priority.

**log_level** (string, default: "info")
:   Verbosity of the daemon log. One of `error`, `warn`, `info`, `debug`, `trace`. Applied on reload without a restart; the **RUST_LOG** environment variable overrides it.

## SINKS
The **[[sinks]]** list defines the audio outputs PWSW should manage. An empty list is a valid file (so **pwsw tui** can start from it), but the daemon refuses to start without sinks. When sinks are configured, exactly one must have `default = true`.

**name** (string, required)
:   The PipeWire node name (e.g., `alsa_output.pci-0000_00_1f.3.analog-stereo`), as shown by **pwsw list-sinks**. Must be non-empty and unique. A sink that is not active yet but can be reached by switching its card's profile is listed there too, with the node name it will have. To reach such a sink, pwsw switches to an available profile providing that output, preferring one that also keeps (or provides) the card's input, so a microphone is never dropped silently; it picks such a duplex profile even when the current profile has no input. Profile switching predicts the default ALSA node names (`alsa_output.<card>.<output>`): a name that WirePlumber rules changed can only be activated while that node exists. A Bluetooth sink (`bluez_output.*`) exists only while its device is connected.

**desc** (string, required)
:   A friendly description for the sink used in rules and notifications. Must be non-empty, unique, not a number, and not equal to another sink's `name`.

**default** (boolean, default: false)
:   Whether this is the fallback sink, used when no window matches a rule. The first sink added in the TUI becomes the default automatically.

**icon** (string, optional)
:   The icon name to use in notifications. Without it, the icon is chosen from the description and node name: `video-display` for HDMI/TV/display, `audio-headphones` for headphones, headsets and Bluetooth, otherwise `audio-speakers`.

## RULES
The **[[rules]]** list defines window-to-sink mappings. Rules require at least one sink. Each window is matched by the first rule in the list that matches it.

**app_id** (string, regex, required)
:   Regex pattern matching the window's application ID. Must not be empty; use `.*` to match every application.

**title** (string, regex, optional)
:   Regex pattern matching the window title. If omitted, any title matches.

**sink** (string, integer, or array of them, required)
:   Reference to a sink by its `desc` (recommended; the TUI always writes this) or its `name`. A 1-indexed position in the sinks list (`sink = 2` or `sink = "2"`) is still accepted but deprecated: it logs a warning on load and points at a different sink if the list is reordered by hand. A reference that names no sink is an error. Renaming, reordering, or deleting sinks in the TUI keeps every rule pointing at the same sink, and a sink that rules still use cannot be deleted.

    An array lists *fallback sinks* in priority order: `sink = ["BT Headphones", "Headphones"]`. When the rule decides the output, the first sink that can be activated right now wins (it exists, or a profile switch can provide it); if activating it fails, the next one that can be activated is tried. Every entry must name a configured sink, a sink may appear only once per rule, and the array must not be empty (a one-entry array is fine). Saves write a rule with one sink as a plain string and one with several as an array. A sink that is any entry of a rule, first or not, cannot be deleted.

    When the daemon had to settle for a fallback, it checks again on the next window event or configuration reload (at most every 2 seconds) and switches up if an earlier sink of the list can be activated by then; it does not poll in between. A manual switch is never undone by this check. Status output, notifications and logs name the sink actually used and mark it *(fallback)*.

**desc** (string, optional)
:   A custom label for this rule used in notifications.

**notify** (boolean, optional)
:   Override the global `notify_rules` setting for this rule. It also applies when the sink falls back because this rule's window closed or stopped matching.

## PRIORITY
The daemon tracks every open window and which rule it matches. The sink it wants is decided as follows:

*   With `match_by_index = false`: the window that most recently opened, or most recently started matching a rule (e.g. after its title changed), wins.
*   With `match_by_index = true`: the window whose rule comes first in the list wins; among windows of the same rule, the most recent one.
*   When no window matches any rule: the default sink.

The answer is the winning rule's list of sinks (see **sink** above); which of them is used is decided when switching. The daemon switches only when this answer *changes*. A sink selected elsewhere (e.g. with **pavucontrol**) or with **pwsw set-sink** stays until then.

## HOT RELOAD
The running daemon applies changes automatically shortly after the file is saved, and on **SIGHUP**. If the file is a symlink (e.g. managed by **stow** or **chezmoi**), changes to its target are seen too, also after the symlink is pointed elsewhere. A file that fails validation, or that has no sinks, is rejected and the previous configuration stays in effect (the error is logged and, with *notify_manual*, shown as a notification). Restarting the daemon is never needed for configuration changes.

## REGEX SYNTAX
PWSW uses the Rust **regex** crate, which prioritizes safety and performance.

### Flavor
The syntax is mostly compatible with Perl-style regular expressions but does **not** support features like lookaround or backreferences to ensure linear-time matching. Because matching is always linear-time, any valid pattern is allowed (including nested repetition such as `^(.+)+-beta$`); the only limit is on the compiled size of a pattern (1 MiB), which rejects things like `\w{100000}`. Config loading, the TUI rule editor, and **pwsw test-rule** apply the same limit.

### Common Patterns
**Substring match** (default)
:   `firefox` - matches any `app_id` (or title) containing "firefox".

**Anchored match**
:   `^steam$` - matches only if the string is exactly "steam".

**Case-insensitive**
:   `(?i)discord` - matches "Discord", "DISCORD", etc.

**Alternation**
:   `^(mpv|vlc)$` - matches exactly "mpv" or "vlc".

**Catch-all**
:   `.*` - matches any string.

## EXAMPLES
```toml
[settings]
log_level = "info"
match_by_index = false

[[sinks]]
name = "alsa_output.pci-0000_00_1f.3.analog-stereo"
desc = "Speakers"
default = true

[[sinks]]
name = "alsa_output.usb-Logitech_G_Pro-00.analog-stereo"
desc = "Headset"

[[sinks]]
name = "bluez_output.40_ED_98_1C_1D_08.1"
desc = "BT Headphones"

[[rules]]
app_id = "discord"
sink = ["BT Headphones", "Headset"]  # Bluetooth when connected, else the headset

[[rules]]
app_id = "firefox"
title = "YouTube"
sink = "Speakers"
notify = false
```

## FILES
*$XDG_CONFIG_HOME/pwsw/config.toml*
:   The default configuration file.

*config.toml.bak*
:   The previous version, written next to the file by **pwsw tui** saves and **pwsw init --force**.

## SEE ALSO
**pwsw**(1)
