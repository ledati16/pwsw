# PWSW - PipeWire Switcher

**Automatic audio output switching for Wayland + PipeWire**

https://github.com/user-attachments/assets/1c8d7018-de76-43f5-b8bb-7fcaccb38de6

## What is PWSW?

PWSW automatically switches your PipeWire audio output based on active windows. Launch a game? Audio goes to speakers. Open Discord? Switches to headset. Close the window? Switches back.

Uses standard Wayland protocols for window monitoring and PipeWire native tools for audio control.

## Features

- **Automatic sink switching** based on window app_id/title patterns (regex)
- **Priority modes** - temporal (recent window) or index-based (rule order)
- **Profile switching** - handles analog/digital device profile changes
- **Desktop notifications** - optional alerts for manual and automatic switches
- **IPC daemon** - background service with Unix socket control
- **JSON output** - for scripting and status bar integration
- **Compositor agnostic** - uses standard Wayland protocols

## Supported Compositors

### ✅ Fully Supported
**wlr-foreign-toplevel-management** (via [wlroots](https://gitlab.freedesktop.org/wlroots/wlroots) or [Smithay](https://github.com/Smithay/smithay)):  
Sway • Hyprland • Niri • River • Wayfire • labwc • dwl • hikari • Cosmic

### ✅ Experimental
**plasma-window-management:** KDE Plasma/KWin

### ❌ Not Supported
**GNOME/Mutter** (protocol not exposed)

> **Note:** wlr-foreign-toplevel-management is a standard protocol implemented by compositors built on **wlroots** (C library: Sway, Hyprland, River, etc.) or **Smithay** (Rust library: Niri, Cosmic). PWSW works with any compositor that implements the protocol.

## Quick Start

```bash
# 1. Install dependencies (Arch example)
sudo pacman -S pipewire pipewire-pulse rust cargo

# 2. Build and install
cargo install --path .

# 3. Discover audio outputs
pwsw list-sinks

# 4. Edit config (auto-created on first run)
pwsw validate
$EDITOR ~/.config/pwsw/config.toml

# 5. Start daemon
pwsw daemon
```

## Usage

### Daemon

```bash
pwsw daemon               # Start in background
pwsw daemon --foreground  # Start with logs to stderr
```

### Commands

```bash
# Status and monitoring
pwsw                    # Show current status (default command)
pwsw status             # Same as above (supports --json)
pwsw list-windows       # Show tracked windows (requires daemon, supports --json)

# Daemon control
pwsw shutdown           # Stop daemon gracefully

# Testing and validation
pwsw test-rule "^mpv$"  # Test regex against tracked windows (requires daemon)
pwsw validate           # Check config syntax (local, no daemon needed)
pwsw list-sinks         # List audio outputs (local, supports --json)
```

## Configuration

**Location:** `~/.config/pwsw/config.toml`

### Settings

```toml
[settings]
default_on_startup = true   # Switch to default sink on daemon start
set_smart_toggle = true     # set-sink toggles back to default if already active
notify_manual = true        # Notifications for manual switches
notify_rules = true         # Notifications for rule-triggered switches
match_by_index = false      # false: recent window wins | true: first rule wins
log_level = "info"          # error, warn, info, debug, trace
```

### Sinks

```toml
[[sinks]]
name = "alsa_output.pci-0000_0c_00.4.iec958-stereo"  # PipeWire node name
desc = "Optical Out"                                  # Human-readable label
default = true                                        # Fallback sink
# icon = "audio-card"                                 # Optional icon override

[[sinks]]
name = "alsa_output.pci-0000_0c_00.4.analog-stereo"
desc = "Headphones"
# Auto-detected icons: HDMI→video-display, headphone→audio-headphones,
#                      speaker→audio-speakers, analog→audio-card
```

**Find sink names:** `pwsw list-sinks`

### Rules

```toml
[[rules]]
app_id = "^steam$"                      # Regex for window app_id
title = "^Steam Big Picture Mode$"     # Optional: regex for window title
sink = "Optical Out"                    # Reference by desc, name, or position (1, 2, ...)
desc = "Steam Big Picture"              # Optional: custom notification label
# notify = false                        # Optional: override notify_rules

[[rules]]
app_id = "^mpv$"
sink = 2  # Position reference
```

**Find app_id/title:**
```bash
pwsw list-windows    # Requires daemon running
pwsw test-rule ".*"  # Show all windows with pattern matching

# Compositor tools:
swaymsg -t get_tree  # Sway/River/wlroots
hyprctl clients      # Hyprland
niri msg windows     # Niri
```

**Regex examples:**
```toml
app_id = "firefox"        # Substring match
app_id = "^firefox$"      # Exact match
app_id = "^(mpv|vlc)$"    # Multiple options
app_id = "(?i)discord"    # Case insensitive
app_id = ".*"             # Any (useful with title-only matching)
```

**Title-only matching:**
```toml
[[rules]]
app_id = ".*"       # Match any app
title = "YouTube"   # Filter by title
sink = "Speakers"
```

### Complete Example

```toml
[settings]
default_on_startup = true
set_smart_toggle = true
notify_manual = true
notify_rules = true
match_by_index = false
log_level = "info"

[[sinks]]
name = "alsa_output.pci-0000_0c_00.4.iec958-stereo"
desc = "Optical Out"
default = true

[[sinks]]
name = "alsa_output.pci-0000_0c_00.4.analog-stereo"
desc = "Headphones"

[[rules]]
app_id = "^steam$"
title = "^Steam Big Picture Mode$"
sink = "Optical Out"
desc = "Steam Big Picture"

[[rules]]
app_id = "^mpv$"
sink = "Headphones"
```

## Advanced

### Profile Switching

PWSW automatically switches device profiles when needed (e.g., analog ↔ digital on same card):
1. Detects if sink requires profile switch
2. Uses `pw-cli` to switch profile
3. Waits for new sink node (with retries)
4. Sets as default with `pw-metadata`

### Priority Modes

**`match_by_index = false`** (default): Most recent window wins  
**`match_by_index = true`**: First matching rule wins

Example with `match_by_index = true`:
```toml
[[rules]]              # Index 0 - highest priority
app_id = "^mpv$"
sink = "Headphones"

[[rules]]              # Index 1 - lower priority
app_id = "^firefox$"
sink = "Speakers"
```
Opening Firefox then MPV → Headphones (MPV always wins regardless of order)

### IPC Socket

- **Location:** `$XDG_RUNTIME_DIR/pwsw.sock` or `/tmp/pwsw-$USER.sock`
- **Permissions:** `0o600` (user-only)
- Stale sockets auto-cleaned on daemon start

### Logging

```bash
# Set in config
log_level = "debug"  # error < warn < info < debug < trace

# View logs
pwsw daemon --foreground
```

## Building

### Prerequisites
```bash
# Rust toolchain
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# PipeWire (Arch example)
sudo pacman -S pipewire pipewire-pulse

# Required: pw-dump, pw-metadata, pw-cli (usually bundled with PipeWire)
```

### Build Commands
```bash
cargo build --release   # Optimized build
cargo install --path .  # Install to ~/.cargo/bin/
cargo check             # Fast syntax check
cargo test              # Run tests
cargo clippy            # Lint
```

**Binary location:** `target/release/pwsw`

## Appendix: LLM-Generated Code

> ⚠️ **Important Disclosure**

This project was **entirely generated by LLMs** (Claude Sonnet/Opus 4.5) by someone without Rust experience.

**Key facts:**
- **100% LLM-generated**, no peer review by Rust developers
- Works as intended with no malicious code, but use with caution
- Personal tool, not production-grade software
- **Do not package for distributions** without peer review

**Code quality:**
- Comprehensive cleanup: 154 clippy warnings → 8 acceptable
- Security review and fixes applied
- See [CLAUDE.md](CLAUDE.md) for standards

[Discussions](https://github.com/ledati16/pwsw/discussions) open for community review.

**Fork and rename** if you want to maintain/improve this project (link back appreciated).

Similar to [Belphemur/SoundSwitch](https://github.com/Belphemur/SoundSwitch) but for Wayland + PipeWire.

---

**License:** [LICENSE](LICENSE) • **Discussions:** [GitHub](https://github.com/ledati16/pwsw/discussions)
