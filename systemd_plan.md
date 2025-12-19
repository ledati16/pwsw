# systemd Integration Improvement Plan

This document outlines planned improvements to the systemd integration added in recent commits.

## Current Implementation Overview

The current systemd integration (added in commits 3557710, 3e770dc, d47c7fe) provides dual-mode daemon management:

### Architecture

**Components:**
- `src/daemon_manager.rs` - Manager type detection and enum definition
- `src/tui/daemon_control.rs` - Start/stop/restart/enable/disable operations
- `contrib/systemd/pwsw.service` - systemd user service unit file
- `contrib/systemd/README.md` - Installation and usage documentation

**Detection Logic:**
```rust
// Current: Check if pwsw.service exists
Command::new("systemctl")
    .args(["--user", "cat", "pwsw.service"])
    .status()
    .is_ok_and(|status| status.success())
```

**Service Type:**
```ini
[Service]
Type=simple  # Current setting
ExecStart=%h/.cargo/bin/pwsw daemon --foreground
Restart=on-failure
RestartSec=5s
```

**Operation Modes:**
1. **Systemd mode** - Uses `systemctl --user` commands
2. **Direct mode** - Process spawning and IPC fallback

### What Works Well

✅ **Simple and maintainable** - Using `systemctl` commands is standard practice
✅ **Graceful fallback** - Direct mode ensures functionality without systemd
✅ **Conditional UI** - TUI shows Enable/Disable only when relevant
✅ **IPC-based detection** - Daemon detects once, clients query via IPC
✅ **Mode-specific behavior** - Operations adapt based on manager type

### Issues and Limitations

⚠️ **Race condition in startup**
- `Type=simple` means systemd considers service "started" immediately after fork
- 200ms sleep in `start()` tries to work around this
- Clients may try to connect before IPC socket is bound

⚠️ **Detection accuracy**
- Checking if `pwsw.service` exists doesn't detect actual systemd supervision
- Service could be installed but daemon started manually (would misdetect as Systemd mode)

⚠️ **No health monitoring**
- systemd doesn't know if daemon is healthy or hung
- No watchdog support

⚠️ **Not using systemd "native" APIs**
- `Type=notify` is best practice for daemons
- Missing sd-notify protocol integration

## Recommended Improvements

### Priority 1: High (Should Implement)

#### 1.1 Add Type=notify with sd-notify Protocol

**Rationale:**
- Eliminates startup race condition
- systemd knows when daemon is actually ready (not just started)
- Standard practice for production daemons
- Removes need for arbitrary 200ms sleep

**Implementation:**

**Add dependency to `Cargo.toml`:**
```toml
[dependencies]
# systemd notification protocol
sd-notify = "0.4"
```

**Update `contrib/systemd/pwsw.service`:**
```ini
[Service]
Type=notify          # Changed from simple
NotifyAccess=main    # Allow daemon to send notifications
ExecStart=%h/.cargo/bin/pwsw daemon --foreground
Restart=on-failure
RestartSec=5s
```

**In `src/daemon.rs` after full initialization (after IPC socket bound):**

Location: After line ~240 (after `ipc::bind_socket()` succeeds and before main event loop)

```rust
// Notify systemd that daemon is ready
#[cfg(unix)]
{
    if let Ok(true) = sd_notify::booted() {
        if let Err(e) = sd_notify::notify(true, &[sd_notify::NotifyState::Ready]) {
            warn!("Failed to notify systemd: {}", e);
        } else {
            info!("Notified systemd that daemon is ready");
        }
    }
}

info!("Daemon initialization complete, entering event loop");
```

**Remove sleep from `src/tui/daemon_control.rs`:**

```rust
// Before (line 22-25):
if output.status.success() {
    // Wait a moment for daemon to start
    tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
    Ok("Daemon started via systemd".to_string())
}

// After:
if output.status.success() {
    // With Type=notify, systemd waits for ready signal
    Ok("Daemon started via systemd".to_string())
}
```

**Benefits:**
- ✅ No race conditions (systemd waits for ready signal)
- ✅ Faster perceived startup (no artificial delay)
- ✅ More reliable (eliminates timing assumptions)
- ✅ Best practice for systemd services

**Testing:**
```bash
# After changes
systemctl --user restart pwsw.service
systemctl --user status pwsw.service
# Should show "active (running)" immediately when actually ready
# Not "active (running)" while still initializing

# Logs should show:
journalctl --user -u pwsw.service -f
# "Notified systemd that daemon is ready"
```

#### 1.2 Improve Detection with INVOCATION_ID

**Rationale:**
- `$INVOCATION_ID` is set by systemd for all supervised processes
- More accurate than checking if service file exists
- Detects actual systemd supervision, not just service installation

**Implementation:**

**Update `src/daemon_manager.rs`:**

```rust
impl DaemonManager {
    /// Detect which daemon manager is in use
    ///
    /// Checks if running under systemd supervision by examining the `INVOCATION_ID`
    /// environment variable (set by systemd for all supervised processes).
    /// Falls back to checking if `pwsw.service` exists for compatibility.
    ///
    /// This should be called once at daemon startup to determine how the daemon
    /// was started. The TUI queries this information via IPC rather than detecting
    /// independently.
    #[must_use]
    pub fn detect() -> Self {
        // Method 1: Check if running under systemd supervision (most reliable)
        // systemd sets INVOCATION_ID for all supervised processes
        if std::env::var("INVOCATION_ID").is_ok() {
            return Self::Systemd;
        }

        // Method 2: Fallback - check if service is installed
        // This handles detection from TUI/CLI when daemon isn't running yet
        if Self::check_systemd_available() {
            Self::Systemd
        } else {
            Self::Direct
        }
    }

    /// Check if systemd user service is available and `pwsw.service` exists
    fn check_systemd_available() -> bool {
        Command::new("systemctl")
            .args(["--user", "cat", "pwsw.service"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    }
}
```

**Benefits:**
- ✅ Accurate detection (checks actual supervision, not just file existence)
- ✅ Handles edge case: service installed but daemon started manually
- ✅ Backward compatible (fallback to old method still works)

**Testing scenarios:**
```bash
# Scenario 1: Daemon started via systemd
systemctl --user start pwsw.service
# Should detect Systemd mode via $INVOCATION_ID

# Scenario 2: Service installed but daemon started manually
pwsw daemon --foreground
# Should detect Direct mode (no $INVOCATION_ID)

# Scenario 3: No service installed
# Should detect Direct mode (fallback fails)
```

### Priority 2: Medium (Consider)

#### 2.1 Add Watchdog Support (Optional)

**Rationale:**
- systemd can detect if daemon hangs or becomes unresponsive
- Automatic restart on health check failure
- Production-grade monitoring

**Implementation:**

**Update `contrib/systemd/pwsw.service`:**
```ini
[Service]
Type=notify
NotifyAccess=main
WatchdogSec=30s      # Expect health check every 30 seconds
ExecStart=%h/.cargo/bin/pwsw daemon --foreground
Restart=on-failure
RestartSec=5s
```

**In `src/daemon.rs` main event loop:**

```rust
// Add to tokio::select! branches (around line 250)
tokio::select! {
    // ... existing branches ...

    // Systemd watchdog ping (every 15 seconds, half of WatchdogSec)
    _ = tokio::time::sleep(tokio::time::Duration::from_secs(15)) => {
        #[cfg(unix)]
        if let Ok(true) = sd_notify::booted() {
            let _ = sd_notify::notify(false, &[sd_notify::NotifyState::Watchdog]);
        }
        continue;
    }
}
```

**Benefits:**
- ✅ Detects hung daemon (e.g., deadlock in event loop)
- ✅ Automatic recovery via restart
- ✅ Production-grade monitoring

**Concerns:**
- ⚠️ Adds complexity to event loop
- ⚠️ May be overkill for single-user desktop daemon
- ⚠️ Current daemon is simple enough that hangs are unlikely

**Recommendation:** Defer unless user requests it or production issues arise.

#### 2.2 Document Why systemctl is Used

**Rationale:**
- Some developers question calling systemctl from code
- Standard practice (Docker, kubectl, many others do this)
- Worth documenting the decision

**Implementation:**

**Add to `src/daemon_manager.rs` module documentation:**

```rust
//! Daemon manager type detection
//!
//! Determines whether the daemon is running under systemd supervision or directly.
//!
//! ## Design Decision: Why Use systemctl Commands?
//!
//! This module uses `systemctl` commands (via `Command::new("systemctl")`) rather
//! than D-Bus APIs for systemd interaction. This is intentional and follows common
//! practice:
//!
//! **Why systemctl is appropriate:**
//! - Simple, readable, maintainable code
//! - Stable interface (systemctl is a stable API)
//! - Standard practice (used by Docker, kubectl, many production tools)
//! - No heavy dependencies (D-Bus would add ~30+ crates)
//! - Operations are user-triggered (not high-frequency)
//!
//! **When D-Bus would be better:**
//! - High-frequency operations (systemctl has fork/exec overhead)
//! - Need structured responses (systemctl uses exit codes)
//! - Complex queries (D-Bus has richer API)
//!
//! For this use case (infrequent user actions in TUI), systemctl is the right choice.
```

**Add to `contrib/systemd/README.md`:**

```markdown
## Design Notes

### Why systemctl Commands?

This project uses `systemctl` commands rather than D-Bus APIs for systemd
interaction. This is a deliberate design choice:

- **Simple and maintainable** - Easy to understand and debug
- **Standard practice** - Many production tools do this (Docker, kubectl, etc.)
- **Appropriate for use case** - User-triggered actions, not high-frequency
- **No heavy dependencies** - Avoids adding D-Bus client libraries

For more details, see the module documentation in `src/daemon_manager.rs`.
```

**Benefits:**
- ✅ Documents design rationale
- ✅ Prevents future "why not use D-Bus?" questions
- ✅ Helps contributors understand tradeoffs

### Priority 3: Low (Skip for Now)

#### 3.1 D-Bus API (Not Recommended)

**Rationale:**
- Native systemd API
- Structured responses
- Slightly faster (no shell overhead)

**Why skip:**
- ❌ Adds significant complexity (`zbus` crate + ~30 dependencies)
- ❌ Marginal benefit for infrequent user actions
- ❌ Harder to maintain
- ❌ systemctl is stable and sufficient

**Decision:** Keep using systemctl. D-Bus is overkill for this use case.

#### 3.2 Socket Activation (Not Recommended)

**Concept:** systemd creates IPC socket and passes it to daemon.

**Why skip:**
- ❌ More complex setup (requires `pwsw.socket` unit file)
- ❌ Complicates Direct mode fallback
- ❌ Current stale socket cleanup works well
- ❌ Marginal benefit (startup is already fast)

**Decision:** Current approach is fine. Socket activation adds complexity without clear benefit.

## Implementation Plan

### Phase 1: Type=notify Support

**Files to modify:**
1. `Cargo.toml` - Add `sd-notify = "0.4"` dependency
2. `contrib/systemd/pwsw.service` - Change `Type=simple` to `Type=notify`, add `NotifyAccess=main`
3. `src/daemon.rs` - Add sd-notify call after initialization
4. `src/tui/daemon_control.rs` - Remove 200ms sleep from `start()`

**Verification:**
```bash
cargo build --release
systemctl --user daemon-reload
systemctl --user restart pwsw.service
systemctl --user status pwsw.service  # Should show active immediately when ready
journalctl --user -u pwsw.service -f  # Check for "Notified systemd" log
```

**Tests:**
- Run full test suite (no code logic changes, just initialization)
- Manual: Start via systemd, verify TUI connects immediately
- Manual: Check journalctl logs for ready notification

### Phase 2: Improved Detection

**Files to modify:**
1. `src/daemon_manager.rs` - Update `detect()` to check `$INVOCATION_ID` first

**Verification:**
```bash
cargo build --release

# Test 1: Via systemd
systemctl --user restart pwsw.service
pwsw status  # Should report Systemd mode

# Test 2: Manual start with service installed
systemctl --user stop pwsw.service
pwsw daemon --foreground &
pwsw status  # Should report Direct mode

# Test 3: No service installed
rm ~/.config/systemd/user/pwsw.service
systemctl --user daemon-reload
pwsw daemon --foreground &
pwsw status  # Should report Direct mode
```

**Tests:**
- Run full test suite
- Manual: Test all three scenarios above
- TUI: Verify Enable/Disable buttons appear correctly

### Phase 3: Documentation (Optional)

**Files to modify:**
1. `src/daemon_manager.rs` - Add module documentation explaining systemctl choice
2. `contrib/systemd/README.md` - Add "Design Notes" section
3. `CLAUDE.md` - Add systemd integration notes if relevant

**Verification:**
- Review documentation for clarity
- Check that rationale is clear and concise

## Open Questions

**Q: Should we support other init systems (runit, OpenRC, etc.)?**

A: Not now. systemd covers the vast majority of modern Linux desktops. Direct mode
provides fallback for all other cases. Adding more init systems increases complexity
without clear user benefit.

**Q: Should watchdog support be implemented?**

A: Defer unless requested. The daemon is simple and unlikely to hang. Watchdog adds
complexity to the event loop for marginal benefit in a single-user desktop tool.

**Q: What about Windows/macOS service management?**

A: Out of scope. This is a Wayland-specific tool (Linux only). Windows/macOS don't
have Wayland compositors with the required protocols.

## Success Criteria

Phase 1 (Type=notify) is successful when:
- ✅ No startup race conditions observed
- ✅ `systemctl status` shows "active" only when daemon is ready
- ✅ TUI can connect immediately after `systemctl start`
- ✅ Logs show "Notified systemd that daemon is ready"
- ✅ All existing tests pass

Phase 2 (Detection) is successful when:
- ✅ Service started via systemd → detects Systemd mode
- ✅ Service installed but daemon manual → detects Direct mode
- ✅ No service installed → detects Direct mode
- ✅ TUI shows Enable/Disable buttons only in Systemd mode

## References

- systemd sd-notify protocol: https://www.freedesktop.org/software/systemd/man/sd_notify.html
- Type=notify best practices: https://www.freedesktop.org/software/systemd/man/systemd.service.html#Type=
- sd-notify crate: https://docs.rs/sd-notify/
- INVOCATION_ID: https://www.freedesktop.org/software/systemd/man/systemd.exec.html#%24INVOCATION_ID

## Changelog

- **2025-12-19**: Initial plan created
  - Phase 1: Type=notify support (high priority)
  - Phase 2: Improved detection with INVOCATION_ID (high priority)
  - Phase 3: Documentation (optional)
  - Deferred: Watchdog, D-Bus API, socket activation
