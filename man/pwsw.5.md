# pwsw(5)

## NAME
pwsw.toml - PWSW configuration file format

## DESCRIPTION
PWSW uses a TOML configuration file located at *~/.config/pwsw/config.toml*. The file defines global settings, audio sinks, and window matching rules.

## SETTINGS
The **[settings]** section controls global daemon behavior.

**default_on_startup** (boolean)
:   Whether to switch to the default sink when the daemon starts.

**set_smart_toggle** (boolean)
:   If true, calling `set-sink` on a sink that is already active will toggle back to the default sink.

**notify_manual** (boolean)
:   Show desktop notifications for manual sink switches and daemon events.

**notify_rules** (boolean)
:   Show desktop notifications for automatic rule-based switches.

**match_by_index** (boolean)
:   If true, the first matching rule in the list wins (priority by position). If false, the most recently focused window wins (priority by time).

**log_level** (string)
:   Verbosity of logging. Options: `error`, `warn`, `info`, `debug`, `trace`.

## SINKS
The **[[sinks]]** list defines the audio outputs PWSW should manage.

**name** (string)
:   The PipeWire node name (e.g., `alsa_output.pci-0000_00_1f.3.analog-stereo`).

**desc** (string)
:   A friendly description for the sink used in rules and notifications.

**default** (boolean)
:   Whether this is the fallback sink. Exactly one sink must be marked as default.

**icon** (string, optional)
:   The name of the icon to use in notifications.

## RULES
The **[[rules]]** list defines window-to-sink mappings.

**app_id** (string, regex)
:   Regex pattern matching the window's application ID.

**title** (string, regex, optional)
:   Regex pattern matching the window title.

**sink** (string or integer)
:   Reference to a sink by its `desc`, `name`, or 1-indexed position in the sinks list.

**desc** (string, optional)
:   A custom label for this rule used in notifications.

**notify** (boolean, optional)
:   Override the global `notify_rules` setting for this specific rule.

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
