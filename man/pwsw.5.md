# pwsw(5)

## NAME
pwsw.toml - PWSW configuration file format

## DESCRIPTION
PWSW uses a TOML configuration file located at *~/.config/pwsw/config.toml*. The file defines global settings, audio sinks, and window matching rules.

Create it with **pwsw init** (a commented template) or **pwsw tui**; no other command creates it. Unknown keys are an error: a typo such as `defualt = true` is reported with the key name, its line, and the file path.

When the TUI saves the file, comments and formatting are preserved (comments above or beside an entry move with it, and are removed with it), the previous version is copied to *config.toml.bak* next to the file, a symlinked file stays a symlink (its target is replaced), and the save is refused if the file was changed on disk since it was loaded.

## SETTINGS
The **[settings]** section controls global daemon behavior.

**default_on_startup** (boolean, default: false)
:   Whether to switch to the default sink when the daemon starts. If false, the current sink stays until the rules want a different sink (e.g. a matching window opens; windows already open when the daemon starts count as opening).

**set_smart_toggle** (boolean)
:   If true, calling `set-sink` on a sink that is already the PipeWire default will toggle back to the default sink.
    
    *Example:* If "Headphones" are active, `pwsw set-sink "Headphones"` switches back to the default sink (e.g., "Speakers").

**notify_manual** (boolean)
:   Show desktop notifications for manual sink switches and daemon events.

**notify_rules** (boolean)
:   Show desktop notifications for automatic rule-based switches.

**match_by_index** (boolean)
:   If true, the first matching rule in the list wins (priority by position). If false, the most recently opened (or started matching) window wins (priority by time).
    
    *Tip:* Rules can be reordered in the TUI using `Shift+Up/Down` to adjust priority.

**log_level** (string)
:   Verbosity of logging. Options: `error`, `warn`, `info`, `debug`, `trace`.

## SINKS
The **[[sinks]]** list defines the audio outputs PWSW should manage.

**name** (string)
:   The PipeWire node name (e.g., `alsa_output.pci-0000_00_1f.3.analog-stereo`). Must be non-empty and unique.

**desc** (string)
:   A friendly description for the sink used in rules and notifications. Must be non-empty, unique, not a number, and not equal to another sink's `name`.

**default** (boolean)
:   Whether this is the fallback sink. When sinks are configured, exactly one must be marked as default. The first sink added via TUI is automatically set as default.

**icon** (string, optional)
:   The name of the icon to use in notifications.

## RULES
The **[[rules]]** list defines window-to-sink mappings.

**app_id** (string, regex)
:   Regex pattern matching the window's application ID.

**title** (string, regex, optional)
:   Regex pattern matching the window title.

**sink** (string or integer)
:   Reference to a sink by its `desc` (recommended; the TUI always writes this) or its `name`. A 1-indexed position in the sinks list (`sink = 2` or `sink = "2"`) is still accepted but deprecated: it logs a warning on load and points at a different sink if the list is reordered by hand. Renaming, reordering, or deleting sinks in the TUI keeps every rule pointing at the same sink, and a sink that rules still use cannot be deleted.

**desc** (string, optional)
:   A custom label for this rule used in notifications.

**notify** (boolean, optional)
:   Override the global `notify_rules` setting for this specific rule.

## REGEX SYNTAX
PWSW uses the Rust **regex** crate, which prioritizes safety and performance.

### Flavor
The syntax is mostly compatible with Perl-style regular expressions but does **not** support features like lookaround or backreferences to ensure linear-time matching. Because matching is always linear-time, any valid pattern is allowed (including nested repetition such as `^(.+)+-beta$`); the only limit is on the compiled size of a pattern (1 MiB), which rejects things like `\w{100000}`. Config loading, the TUI rule editor, and **pwsw test-rule** apply the same limit.

### Common Patterns
**Substring match** (default)
:   `firefox` - matches any window containing "firefox" in its `app_id` or title.

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

[[rules]]
app_id = "discord"
sink = "Headset"

[[rules]]
app_id = "firefox"
title = "YouTube"
sink = "Speakers"
```

## SEE ALSO
**pwsw**(1)
