# NAME
pwsw - PWSW configuration file (config.toml)

# DESCRIPTION
PWSW uses a TOML configuration file, by default *$XDG_CONFIG_HOME/pwsw/config.toml* (normally *~/.config/pwsw/config.toml*). **pwsw --config** *PATH* or the **PWSW_CONFIG** environment variable select a different file. The file defines global settings, audio sinks, optionally audio sources (microphones), and window matching rules.

Create it with **pwsw init** (a commented template) or **pwsw tui**; no other command creates it. Check it with **pwsw validate**. Unknown keys are an error: a typo such as `defualt = true` is reported with the key name, its line, and the file path.

When the TUI saves the file, comments and formatting are preserved (comments above or beside an entry move with it, and are removed with it), the previous version is copied to *config.toml.bak* next to the file, a symlinked file stays a symlink (its target is replaced), and if the file was changed on disk since it was loaded, the TUI asks whether to overwrite it or reload it. Saved files have mode 0600.

# SETTINGS
The **[settings]** section controls global daemon behavior. The section and every key in it are optional.

**default_on_startup** (boolean, default: false)
:   Whether to switch, when the daemon starts, to the sink (and, with **[[sources]]**, the source) the rules want: the default sink if no open window matches. Either way the daemon first waits (at most 2 seconds) for the compositor to list the windows already open and decides from all of them at once, so start-up switches at most once. If false, nothing is switched at start-up: the current sink stays until the rules' answer changes (e.g. a matching window opens after start-up). The start-up switch shows no notification, even with *notify_rules* (it would come with every login).

**set_smart_toggle** (boolean, default: true)
:   If true, calling `set-sink` on a sink that is already the PipeWire default will toggle back to the default sink (`set-source` likewise for sources).

    *Example:* If "Headphones" are active, `pwsw set-sink "Headphones"` switches back to the default sink (e.g., "Speakers"). Without it, asking for the active sink does nothing.

**notify_manual** (boolean, default: true)
:   Show desktop notifications for manual switches (**set-sink**, **next-sink**, **prev-sink**, **set-source**) and daemon events: start and stop, configuration reloaded or rejected, switching paused or resumed.

**notify_rules** (boolean, default: true)
:   Show desktop notifications for rule-driven switches (sink and mic). A rule's own *notify* key overrides it.

**match_by_index** (boolean, default: false)
:   Which window decides the sink (and the source) when several match (see **PRIORITY**). If false, the most recently opened (or most recently matching) window wins. If true, the window matching the earliest rule in the list wins.

    *Tip:* Rules can be reordered in the TUI using `Shift+Up/Down` to adjust priority.

**log_level** (string, default: "info")
:   Verbosity of the daemon log. One of `error`, `warn`, `info`, `debug`, `trace`. Applied on reload without a restart; the **RUST_LOG** environment variable overrides it.

**switch_delay_ms** (integer, default: 0)
:   Settle delay for rule-driven switches, in milliseconds (0 to 5000). With 0 the daemon switches as soon as the sink its rules want changes. With a positive value it waits this long and switches only if the rules still want the same sink then (sources wait the same way, each dimension on its own: a sink change never delays or cancels a source switch, and vice versa): another change restarts the wait, and a change back to the current sink cancels the switch. Use it when a window matches only for a moment, such as a splash screen or a launcher, or when an application's title flickers between values that match different rules. Manual switches, retries of failed switches, the start-up switch (*default_on_startup*) and **pwsw resume** are never delayed.

# SINKS
The **[[sinks]]** list defines the audio outputs PWSW should manage. An empty list is a valid file (so **pwsw tui** can start from it), but the daemon refuses to start without sinks. When sinks are configured, exactly one must have `default = true`.

**name** (string, required)
:   The PipeWire node name (e.g., `alsa_output.pci-0000_00_1f.3.analog-stereo`), as shown by **pwsw list-sinks**. Must be non-empty and unique. A sink that is not active yet but can be reached by switching its card's profile is listed there too, with the node name it will have. How pwsw reaches such a sink is described in **PROFILE SWITCHING**. A Bluetooth sink (`bluez_output.*`) exists only while its device is connected.

**desc** (string, required)
:   A friendly description for the sink used in rules and notifications. Must be non-empty, unique, not a number, and not equal to another sink's `name`.

**default** (boolean, default: false)
:   Whether this is the default sink, used when no window matches a rule. The first sink added in the TUI becomes the default automatically.

**icon** (string, optional)
:   The icon name to use in notifications. Without it, the icon is chosen from the description and node name: `video-display` for HDMI/TV/display, `audio-headphones` for headphones, headsets and Bluetooth, otherwise `audio-speakers`.

# SOURCES
The optional **[[sources]]** list defines the audio inputs (microphones) PWSW may switch, with the same keys as **[[sinks]]**: **name** (the `Audio/Source` node name, e.g. `alsa_input.pci-0000_00_1f.3.analog-stereo` or `bluez_input.*`, as shown in the *MICS (SOURCES)* section of **pwsw list-sinks**; monitors of sinks are not sources), **desc**, **default** and **icon** (default icon: `audio-input-microphone`). The same uniqueness rules apply within the list (a source may share a description with a sink). When sources are configured, exactly one must have `default = true`: it is used when no open window's rule asks for a source.

Without any **[[sources]]** (the default), pwsw never touches the default source; existing configurations keep working unchanged.

A source that is not present yet can be reached by switching its ALSA card's profile (see **PROFILE SWITCHING**); such a switch always keeps the card's current output. Renaming, reordering and deleting sources in the TUI keeps rule references pointing at the same source, and a source a rule uses cannot be deleted.

# RULES
The **[[rules]]** list defines window-to-sink mappings. Rules require at least one sink. A rule matches a window when every condition it sets matches (**app_id**, **title**, **output**); each rule needs at least one of **app_id** and **output**. Each window is matched by the first rule in the list that matches it.

**app_id** (string, regex, required unless **output** is set)
:   Regex pattern matching the window's application ID. Must not be empty; use `.*` to match every application. A rule without **app_id** (it then has **output**) matches any application.

**title** (string, regex, optional)
:   Regex pattern matching the window title. If omitted, any title matches. Must not be empty (leave it out instead).

**output** (string, regex, optional)
:   Regex pattern matching the *connector name* of a monitor (`HDMI-A-1`, `DP-2`, `eDP-1`; see **pwsw list-windows**, or **swaymsg -t get_outputs**, **hyprctl monitors**, **niri msg outputs**): "audio follows the screen". The condition matches when the window is on a monitor whose name matches (a window spanning two monitors matches if either does) and the window is not minimized. Must not be empty; use `.*` for "on any monitor".

    Only compositors offering **wlr-foreign-toplevel-management** (e.g. Sway, Hyprland) report which monitor a window is on; **ext-foreign-toplevel-list** does not. When the configuration has rules with **output**, the daemon prefers **wlr** at start-up. On a compositor that offers only **ext**, rules with **output** never match (the daemon logs a warning, and **pwsw doctor** says so); the other rules work as usual. Moving a window onto a matching monitor counts as fresh activity for **PRIORITY**, like a title change that starts matching; moving it between two matching monitors does not.

**sink** (string, integer, or array of them, required)
:   Reference to a sink by its `desc` (recommended; the TUI always writes this) or its `name`. A 1-indexed position in the sinks list (`sink = 2` or `sink = "2"`) is still accepted but deprecated: it logs a warning on load and points at a different sink if the list is reordered by hand. A reference that names no sink is an error. Renaming, reordering, or deleting sinks in the TUI keeps every rule pointing at the same sink, and a sink that rules still use cannot be deleted.

    An array lists *fallback sinks* in priority order: `sink = ["BT Headphones", "Headphones"]`. When the rule decides the output, the first sink that can be activated right now wins (it exists, or a profile switch can provide it); if activating it fails, the next one that can be activated is tried. Every entry must name a configured sink, a sink may appear only once per rule, and the array must not be empty (a one-entry array is fine). Saves write a rule with one sink as a plain string and one with several as an array. A sink that is any entry of a rule, first or not, cannot be deleted.

    When the daemon had to settle for a fallback, it checks again on the next window event or configuration reload (at most every 2 seconds) and switches up if an earlier sink of the list can be activated by then; it does not poll in between. A manual switch is never undone by this check, and neither is a sink chosen outside pwsw (e.g. in **pavucontrol**) since the fallback was applied: the check sees that the default is no longer the fallback and leaves it alone until the rules' answer changes. Status output, notifications and logs name the sink actually used and mark it *(fallback)*.

**source** (string, integer, or array of them, optional)
:   The microphone this rule wants, referenced like **sink** (by `desc`, `name`, or a deprecated position) against **[[sources]]**; an array lists fallback sources in priority order, with the same rules as fallback sinks. Allowed only when sources are configured. A rule without **source** never affects the microphone: see **PRIORITY**.

**desc** (string, optional)
:   A custom label for this rule, used in notifications and in the status stream (**pwsw**(1), *STATUS STREAM*), which otherwise names the rule by its position (*#N*).

**notify** (boolean, optional)
:   Override the global `notify_rules` setting for this rule. It also applies when the sink falls back because this rule's window closed or stopped matching.

# PRIORITY
The daemon tracks every open window and which rule it matches. The sink it wants is decided as follows:

*   With `match_by_index = false`: the window that most recently opened, or most recently started matching a rule, or a different rule than before (e.g. after its title changed, or after it was moved onto, or restored on, a monitor an **output** rule names), wins.
*   With `match_by_index = true`: the window whose rule comes first in the list wins; among windows of the same rule, the most recent one.
*   When no window matches any rule: the default sink.

The answer is the winning rule's list of sinks (see **sink** above); which of them is used is decided when switching. The daemon switches only when this answer *changes*. A sink selected elsewhere (e.g. with **pavucontrol**) or with **pwsw set-sink** stays until then.

The source is an independent second answer: the same priority applies, but only among windows whose rule has a **source**; when none matches, the default source. So with Discord (sink *Headset*, source *Headset Mic*) open and then Firefox (sink *Speakers*, no source), the output follows Firefox and the microphone stays with Discord. Retries, fallbacks and upgrades, manual choices (**pwsw set-source**), pausing and the settle delay work for the source exactly as for the sink, each on its own.

# PROFILE SWITCHING
A sink or source that is not present right now, but that its ALSA card could provide in another profile, is reached by switching the card's profile (**pwsw list-sinks** lists such nodes with the name they will have). pwsw predicts the default ALSA node names, `alsa_output.<card>.<output>` and `alsa_input.<card>.<input>`, from the card's profiles; after the switch it recognises the new node by its device and profile, so a node WirePlumber renames is still found, but a configured name that WirePlumber rules changed can only be activated while that node exists. Only profiles marked available are used; UCM (`HiFi`) and Pro Audio profiles are never switched to, so their nodes work only while already active.

*   For a sink, pwsw picks a profile providing that output that also keeps the card's current input (or provides one), so a microphone is never dropped silently.
*   For a source, the profile must keep the card's current output: pwsw never takes the audio output away to get a microphone.
*   When one switch needs both a sink and a source on the same card, the card's profile is switched once, to a profile that provides the sink's output *and* the source's input (e.g. `output:analog-stereo+input:analog-stereo`). If no such profile exists, the sink wins: its profile is used and that source is reported as unreachable (a fallback source on another card is still used). With **[[sources]]** configured, a sink switch on the card of the microphone pwsw manages is planned the same way. Sinks and sources on different cards are switched independently.

The profile is set with **pw-cli** and saved (`save: true`), so WirePlumber keeps it. pwsw then waits for the new node (by default up to 3 s; see *PROFILE_SWITCH_DELAY_MS* in **pwsw**(1), never more than 5 s) and makes it the default. If it does not appear in time, the card is switched back to its previous profile, so a failed switch leaves the audio where it was (the error says *switched it back to its previous profile ...*; if the previous profile is no longer available, the card is left on the new one and the error says that instead). A failed rule-driven switch is retried after 2 s and 5 s (see **pwsw**(1)); the retries switch the card again and wait up to 5 s for the node, so a card too slow for the first attempt usually makes it then. For a device that needs more than 3 s, raise *PROFILE_SWITCH_DELAY_MS* so first attempts and manual switches wait longer too. Defaults are written to `default.configured.audio.sink` and `default.configured.audio.source`, as **wpctl set-default** does, so WirePlumber keeps them.

# HOT RELOAD
The running daemon applies changes automatically shortly after the file is saved, and on **SIGHUP**. If the file is a symlink (e.g. managed by **stow** or **chezmoi**), changes to its target are seen too, also after the symlink is pointed elsewhere. A file that fails validation, or that has no sinks, is rejected and the previous configuration stays in effect (the error is logged and, with *notify_manual*, shown as a notification). Restarting the daemon is not needed for configuration changes, with one exception: a daemon started without rules with **output** uses the **ext** protocol when the compositor offers it, which reports no monitors. Adding the first **output** rules then logs a warning (and the reload notification says so) and they do not match until the daemon is restarted (**systemctl --user restart pwsw**, or **pwsw shutdown** then **pwsw daemon** when it was not started by systemd); removing them later needs no restart.

# REGEX SYNTAX
PWSW uses the Rust **regex** crate, which prioritizes safety and performance.

## Flavor
The syntax is mostly compatible with Perl-style regular expressions but does **not** support features like lookaround or backreferences to ensure linear-time matching. Because matching is always linear-time, any valid pattern is allowed (including nested repetition such as `^(.+)+-beta$`); the only limit is on the compiled size of a pattern (1 MiB), which rejects things like `\w{100000}`. Config loading, the TUI rule editor, and **pwsw test-rule** apply the same limit.

## Common Patterns
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

# EXAMPLES
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

[[sinks]]
name = "alsa_output.pci-0000_01_00.1.hdmi-stereo"
desc = "TV"

[[sources]]
name = "alsa_input.usb-Logitech_G_Pro-00.mono-fallback"
desc = "Headset Mic"

[[sources]]
name = "alsa_input.pci-0000_00_1f.3.analog-stereo"
desc = "Desk Mic"
default = true          # when no rule asks for a source

[[rules]]
app_id = "discord"
# Bluetooth when connected, else the headset
sink = ["BT Headphones", "Headset"]
source = "Headset Mic"

[[rules]]
app_id = "firefox"
title = "YouTube"
sink = "Speakers"
notify = false

# Audio follows the screen: anything on the TV plays on the TV
[[rules]]
output = "^HDMI-A-1$"
sink = "TV"

# Or only the video player, on any HDMI monitor
[[rules]]
app_id = "^mpv$"
output = "^HDMI-"
sink = "TV"
```

# FILES
*$XDG_CONFIG_HOME/pwsw/config.toml*
:   The default configuration file.

*config.toml.bak*
:   The previous version (*\<name\>.bak* for a file given with **--config**), written next to the file by **pwsw tui** saves and **pwsw init --force**.

# SEE ALSO
**pwsw**(1)
