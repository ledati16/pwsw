# pwsw(1)

## NAME
pwsw - PipeWire Switcher daemon and client

## SYNOPSIS
**pwsw** [*OPTIONS*] [*COMMAND*]

## DESCRIPTION
**pwsw** is a daemon that automatically switches PipeWire audio sinks based on active windows in Wayland compositors. It monitors window events using standard protocols and uses PipeWire native tools for audio control.

It can also be used as a command-line client to query the daemon status, list windows, and manually switch sinks.

## COMMANDS
**daemon** [*OPTIONS*]
:   Start the PWSW daemon.

    **--foreground**
    :   Run in the foreground (useful for systemd or debugging).

**status** [*--json*]
:   Query and display the current daemon status, active sink, and tracked windows.

**tui**
:   Launch the interactive Terminal User Interface for configuration and monitoring.

**list-sinks** [*--json*]
:   List all active and profile-switchable PipeWire sinks.

**list-windows** [*--json*]
:   List all currently open windows known to the compositor.

**test-rule** *PATTERN* [*--json*]
:   Test a regex pattern against current windows to see what would match. See **pwsw**(5) for details on regex syntax.

**validate**
:   Validate the configuration file syntax and sink references.

**set-sink** *SINK*
:   Set audio output by description, node name, or 1-indexed position (e.g., "1", "2"). If `set_smart_toggle` is enabled in config and the target sink is already active, toggles back to the default sink.

**next-sink**
:   Cycle to the next configured sink (wraps around).

**prev-sink**
:   Cycle to the previous configured sink (wraps around).

**shutdown**
:   Gracefully stop the running daemon.

## COMPATIBILITY
**pwsw** relies on standard Wayland protocols to monitor windows.

**Supported:**
*   **ext-foreign-toplevel-list-v1**: The official Wayland standard.
*   **wlr-foreign-toplevel-management**: Used by Sway, Hyprland, River, Wayfire, and others.

**Not Supported:**
*   **GNOME / Mutter**: Does not expose window management protocols.
*   **KDE Plasma 6**: Removed protocol support (pending standard implementation).

## TUI KEYS
The TUI supports mouse interaction and the following keybindings. Press `?` or `F1` within the TUI for comprehensive context-sensitive help.

**Global**
:   `Tab` / `Shift+Tab`: Next / Previous screen
:   `1-4`: Jump to screen (Dashboard, Sinks, Rules, Settings)
:   `?` or `F1`: Toggle context-aware help overlay
:   `Ctrl+S`: Save configuration
:   `Esc`: Clear status message / Cancel action
:   `q` or `Ctrl+C`: Quit

**Lists (Sinks/Rules)**
:   `↑/↓`: Navigate items
:   `Shift+↑/↓`: Reorder items
:   `a` / `e` / `x`: Add / Edit / Delete
:   `Enter`: Inspect selected item
:   `Space`: Set default (Sinks) / Quick actions

**Editors**
:   `Tab` / `Shift+Tab`: Switch between fields
:   `Space`: Open selector dropdown / Select from dropdown
:   `Enter`: Save changes
:   `Esc`: Cancel editing / Close dropdown

**Dashboard**
:   `←/→`: Navigate daemon actions (Start/Stop/Restart/Enable/Disable)
:   `Enter`: Execute selected action
:   `w`: Toggle between Logs and Windows view
:   `↑/↓` / `PageUp/PageDown`: Scroll logs

## OPTIONS
**--config** *PATH*
:   Use *PATH* as the configuration file instead of the default location. Takes precedence over **PWSW_CONFIG**. Accepted before or after the command; when **pwsw** starts the daemon itself (background mode or the TUI), the same path is passed on. A default configuration is created at *PATH* if it does not exist.

**-h**, **--help**
:   Print help information.

**-V**, **--version**
:   Print version information.

## ENVIRONMENT
**PWSW_CONFIG**
:   Path to the configuration file to use when **--config** is not given. Ignored when empty.

**XDG_CONFIG_HOME**
:   Base directory for the default configuration file (*$XDG_CONFIG_HOME/pwsw/config.toml*, normally *~/.config/pwsw/config.toml*).

**WAYLAND_DISPLAY**
:   The name of the Wayland display to connect to.

**XDG_RUNTIME_DIR**
:   Directory for the IPC socket (`pwsw.sock`) and PID file.

**PROFILE_SWITCH_DELAY_MS**
:   Interval in milliseconds between checks for the new sink after a profile switch (default: 150, clamped to 10-1000).

**PROFILE_SWITCH_MAX_RETRIES**
:   Number of checks for the new sink after a profile switch (default: 10). The total wait (interval × checks) is capped at 5 seconds.

## FILES
*~/.config/pwsw/config.toml*
:   The default configuration file (override with **--config** or **PWSW_CONFIG**). See **pwsw**(5) for details.

*~/.local/share/pwsw/daemon.log*
:   Log file for the background daemon.

## BUGS
See <https://github.com/ledati16/pwsw/issues>

## SEE ALSO
**pwsw**(5), **pw-dump**(1), **pw-metadata**(1), **pw-cli**(1)
