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

**status**
:   Query and display the current daemon status, active sink, and tracked windows.

**tui**
:   Launch the interactive Terminal User Interface for configuration and monitoring.

**list-sinks** [*--json*]
:   List all active and profile-switchable PipeWire sinks.

**list-windows** [*--json*]
:   List all currently open windows known to the compositor.

**test-rule** *PATTERN*
:   Test a regex pattern against current windows to see what would match. See **pwsw**(5) for details on regex syntax.

**validate**
:   Validate the configuration file syntax and sink references.

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
The TUI supports mouse interaction and the following keybindings:

**Global**
:   `1-4`: Switch tabs (Dashboard, Sinks, Rules, Settings)
:   `?` or `F1`: Show context-aware help
:   `Ctrl+S`: Save configuration
:   `q` or `Ctrl+C`: Quit

**Navigation**
:   `Arrow Keys`: Navigate lists
:   `Shift+Up/Down`: Reorder items (Sinks/Rules)
:   `Enter`: Select / Edit / Save
:   `Space`: Toggle / Set Default
:   `a` / `e` / `x`: Add / Edit / Delete items

## OPTIONS
**-h**, **--help**
:   Print help information.

**-V**, **--version**
:   Print version information.

## ENVIRONMENT
**WAYLAND_DISPLAY**
:   The name of the Wayland display to connect to.

**XDG_RUNTIME_DIR**
:   Directory for the IPC socket (`pwsw.sock`) and PID file.

**PROFILE_SWITCH_DELAY_MS**
:   Delay between retries when waiting for a sink to appear after a profile switch (default: 150).

**PROFILE_SWITCH_MAX_RETRIES**
:   Maximum number of retries for profile switching (default: 5).

## FILES
*~/.config/pwsw/config.toml*
:   The configuration file. See **pwsw**(5) for details.

*~/.local/share/pwsw/daemon.log*
:   Log file for the background daemon.

## BUGS
See <https://github.com/ledati16/pwsw/issues>

## SEE ALSO
**pwsw**(5), **pw-dump**(1), **pw-metadata**(1), **pw-cli**(1)
