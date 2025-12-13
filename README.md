# PWSW - PipeWire Switcher

**Automatic audio output switching for Wayland + PipeWire**

https://github.com/user-attachments/assets/1c8d7018-de76-43f5-b8bb-7fcaccb38de6

## What is PWSW?

PWSW is a daemon that automatically switches your PipeWire audio output based on which window is focused. Launch a game? Audio goes to your speakers. Open Discord? Audio switches to your headset. Close the window? It switches back automatically.

It uses standard Wayland protocols to monitor windows and PipeWire native tools (`pw-dump`, `pw-metadata`, `pw-cli`) for audio control—no compositor-specific hacks required.

## Features

- **Automatic sink switching** - Configure rules to switch audio based on window app_id/title (regex supported)
- **Priority modes** - Choose temporal (most recent window) or index-based (explicit rule ordering) priority
- **Profile switching support** - Automatically switch device profiles when needed (e.g., analog ↔ digital)
- **Desktop notifications** - Optional notifications for manual and rule-triggered switches
- **IPC daemon** - Background daemon with Unix socket for CLI control
- **Zero compositor-specific code** - Uses standard Wayland protocols (works with Sway, Hyprland, Niri, River, KDE Plasma, and more)
- **JSON output** - All commands support `--json` for scripting/status bars

## Supported Compositors

### ✅ Fully Supported (via wlr-foreign-toplevel-management)
Sway • Hyprland • Niri • River • Wayfire • labwc • dwl • hikari

### ✅ Experimental (via plasma-window-management)  
KDE Plasma/KWin

### ❌ Not Supported
GNOME/Mutter (protocol not exposed)

**Why these?** PWSW uses standard Wayland protocols instead of compositor-specific IPC, providing broad compatibility with less code.

## Quick Start

### 1. Install Dependencies
```bash
# Arch Linux
sudo pacman -S pipewire pipewire-pulse

# Fedora
sudo dnf install pipewire pipewire-utils

# Ubuntu/Debian
sudo apt install pipewire pipewire-bin
```

### 2. Build and Install
```bash
# Clone repository
git clone https://github.com/ledati16/pwsw.git
cd pwsw

# Build release binary
cargo build --release

# Install to ~/.cargo/bin (ensure it's in your PATH)
cargo install --path .
```

### 3. Configure and Run
```bash
# Discover your audio outputs
pwsw list-sinks

# Edit the generated config
pwsw validate  # Creates default config at ~/.config/pwsw/config.toml
$EDITOR ~/.config/pwsw/config.toml

# Start the daemon
pwsw daemon
```

That's it! Your audio will now switch automatically based on active windows.

## Usage

### Daemon Commands

Start the daemon to enable automatic audio switching:

```bash
pwsw daemon              # Run in background (detached)
pwsw daemon --foreground # Run in foreground with logs
pwsw                     # Alias for 'pwsw daemon'
```

### IPC Commands (require running daemon)

Communicate with the daemon via Unix socket:

```bash
pwsw status              # Show daemon status, current sink, active windows
pwsw status --json       # JSON output for scripting

pwsw reload              # Reload configuration (requires daemon restart for changes)
pwsw shutdown            # Gracefully stop the daemon

pwsw list-windows        # Show all windows tracked by daemon
pwsw list-windows --json # JSON output

pwsw test-rule "^mpv$"   # Test regex pattern against tracked windows
```

### Local Commands (no daemon needed)

These work without a running daemon:

```bash
pwsw list-sinks          # Discover available audio outputs
pwsw list-sinks --json   # JSON output with icons (for status bars)

pwsw validate            # Validate config and show parsed settings
```

## Configuration

Config location: `~/.config/pwsw/config.toml`

### Settings Section

```toml
[settings]
default_on_startup = true  # Switch to default sink when daemon starts
set_smart_toggle = true    # set-sink toggles back to default if already active
notify_manual = true       # Show notifications for manual sink changes
notify_rules = true        # Show notifications for rule-triggered switches
match_by_index = false     # Priority mode (see below)
log_level = "info"         # error, warn, info, debug, trace
```

#### Priority Modes: `match_by_index`

Controls how PWSW prioritizes multiple matching windows:

- **`false` (default)** - Temporal priority: most recently opened window wins
- **`true`** - Index priority: lower rule index (first in config) wins

**Example:**
```toml
match_by_index = true

[[rules]]  # Priority 1 (highest)
app_id = "^mpv$"
sink = "Headphones"

[[rules]]  # Priority 2
app_id = "^firefox$"
sink = "Speakers"
```

With `match_by_index = true`:
- Open Firefox → Speakers
- Open MPV → Headphones (MPV rule wins due to lower index)
- Close MPV → back to Speakers

With `match_by_index = false` (default):
- Open Firefox → Speakers
- Open MPV → Headphones (most recent window wins)
- Close MPV → back to Speakers

### Sinks Section

Define your audio outputs:

```toml
[[sinks]]
name = "alsa_output.pci-0000_0c_00.4.iec958-stereo"  # PipeWire node name
desc = "Optical Out"                                  # Human-readable name
default = true                                        # Fallback sink
# icon = "audio-card"  # Optional: override auto-detected icon

[[sinks]]
name = "alsa_output.pci-0000_0c_00.4.analog-stereo"
desc = "Headphones"
# Icons auto-detected from description keywords:
# "HDMI" → video-display, "headphone" → audio-headphones,
# "speaker" → audio-speakers, "analog" → audio-card
```

**Finding sink names:** Run `pwsw list-sinks` to see all available outputs.

### Rules Section

Define window matching rules:

```toml
[[rules]]
app_id = "^steam$"                       # Regex pattern for window app_id
title = "^Steam Big Picture Mode$"      # Optional: regex for window title
sink = "Optical Out"                     # Reference by: desc, name, or position (1, 2, ...)
desc = "Steam Big Picture"               # Optional: custom name for notifications
# notify = false                         # Optional: override notify_rules for this rule
```

**Finding app_id/title:**
```bash
pwsw list-windows           # While daemon is running
pwsw test-rule ".*"         # Test patterns (shows all windows)

# Compositor-specific tools:
swaymsg -t get_tree         # Sway/River/wlroots compositors
hyprctl clients             # Hyprland
niri msg windows            # Niri
# KDE Plasma: use KDE window inspector
```

**Regex pattern examples:**
```toml
app_id = "firefox"          # Matches anywhere in app_id
app_id = "^firefox$"        # Exact match only
app_id = "^(mpv|vlc)$"      # Matches mpv OR vlc
app_id = "(?i)discord"      # Case insensitive
app_id = ".*"               # Matches any window (useful with title-only matching)
```

**Title-only matching:**
```toml
[[rules]]
app_id = ".*"               # Match any app_id
title = "YouTube"           # Only match windows with "YouTube" in title
sink = "Speakers"
```

### Full Example Config

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

## Advanced Topics

### Profile Switching

Some audio devices require switching profiles to access different outputs (e.g., analog vs digital on the same card). PWSW handles this automatically.

**How it works:**
1. If a sink isn't currently active, PWSW checks if it requires a profile switch
2. Uses `pw-cli` to switch the device profile
3. Waits for the new sink node to appear (with retries)
4. Sets it as default with `pw-metadata`

**Example:**
```toml
[[sinks]]
name = "alsa_output.pci-0000_0c_00.4.analog-stereo"
desc = "Headphones"  # Requires switching to analog profile

[[sinks]]
name = "alsa_output.pci-0000_0c_00.4.iec958-stereo"
desc = "Optical Out"  # Requires switching to digital profile
```

PWSW automatically detects these and switches profiles as needed.

### IPC Socket Location

The daemon listens on a Unix socket (permissions: `0o600`):
- **Primary:** `$XDG_RUNTIME_DIR/pwsw.sock` (usually `/run/user/1000/pwsw.sock`)
- **Fallback:** `/tmp/pwsw-$USER.sock`

Stale sockets are automatically cleaned up on daemon start (500ms health check timeout).

### Logging

Control log verbosity with the `log_level` setting:
```toml
log_level = "debug"  # error < warn < info < debug < trace
```

Run daemon in foreground to see logs:
```bash
pwsw daemon --foreground
```

## Building from Source

### Prerequisites

- **Rust toolchain** (stable, 1.70+)
  ```bash
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
  rustup default stable
  ```

- **PipeWire and tools** (`pw-dump`, `pw-metadata`, `pw-cli`)
- **Supported Wayland compositor** (see above)
- **Notification daemon** (optional, for desktop notifications)

### Build Commands

```bash
# Debug build (with debug symbols)
cargo build

# Release build (optimized)
cargo build --release

# Check without building
cargo check

# Run tests
cargo test

# Install to ~/.cargo/bin/
cargo install --path .

# Format code
cargo fmt

# Lint with clippy
cargo clippy --all-targets
```

Binary location:
- Debug: `target/debug/pwsw`
- Release: `target/release/pwsw`

## Appendix: LLM-Generated Code Notice

> ⚠️ **Important Disclosure**

This project was **entirely generated by large language models** (Claude Sonnet/Opus 4.5) and "vibe coded" by someone without Rust experience.

**Key points:**
- Code is **100% LLM-generated**, not written by an experienced developer
- **No peer review** by anyone with Rust experience
- While there's no malicious code (and it works as intended), use with caution
- [Discussions](https://github.com/ledati16/pwsw/discussions) are open for community review and feedback
- **Do not package for Linux distributions** without peer review by a Rust developer
- This is a personal tool that works for the author, not a production-grade project

**However:** The code has undergone comprehensive cleanup:
- 154 clippy warnings → 8 acceptable pedantic warnings
- Security review and fixes applied
- Proper error handling and documentation added
- See [CLAUDE.md](CLAUDE.md) for code quality standards

**If you want to maintain/improve this project**, please fork and rename it (and link back here).

This is similar in spirit to [Belphemur/SoundSwitch](https://github.com/Belphemur/SoundSwitch) but for Wayland + PipeWire.

---

**License:** See [LICENSE](LICENSE)  
**Discussions:** https://github.com/ledati16/pwsw/discussions
