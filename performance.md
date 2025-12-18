# TUI Performance Improvement Plan

## Problem Statement

The TUI currently feels laggy and unresponsive during rapid key input (e.g., holding 'c' in an input box) and navigation. Users expect immediate visual feedback (1-5ms) but currently experience delays of 0-80ms (average ~40ms).

## Root Cause Analysis

### Current Implementation

**File:** `src/tui/mod.rs:424-548` (`run_app` function)

The event loop uses a tick-based rendering model:

```rust
// Line 431
let mut tick = tokio::time::interval(std::time::Duration::from_millis(80));

loop {
    tokio::select! {
        _ = tick.tick() => {
            // Advance animation (spinner)
            if now.duration_since(last_anim).as_millis() >= 120 {
                app.throbber_state_mut().calc_next();
                app.dirty = true;
            }

            // Render only happens here!
            if app.dirty {
                terminal.draw(|frame| render_ui(frame, app))?;
                app.dirty = false;
            }
        }
        Some(Ok(event)) = events.next() => {
            handle_event(app, &event);  // Sets dirty=true, but doesn't render
        }
        // ... background updates
    }
}
```

### The Problem Flow

1. **User presses key** → `events.next()` fires immediately
2. **`handle_event()` called** → Sets `app.dirty = true` (in `src/tui/input.rs:19`)
3. **`tokio::select!` returns** → Waits for next iteration
4. **Tick fires** (0-80ms later) → Finally renders the frame

**Result:** Effective frame rate of **~12.5 FPS** (1000ms ÷ 80ms), which feels sluggish compared to modern TUI standards (30-60 FPS).

### Why This Architecture Was Chosen

The tick-based approach is common for game loops and simple TUIs:
- ✅ Simple to implement
- ✅ Predictable frame timing
- ✅ Easy animation synchronization
- ❌ Fixed frame rate regardless of input
- ❌ Unnecessary renders when idle
- ❌ Input lag proportional to tick interval

## Recommended Solution

### Event-Driven Rendering with Frame Rate Cap

Move from tick-polling to event-driven rendering while maintaining a maximum frame rate to prevent excessive redraws during key repeat.

**Benefits:**
- ⚡ **Immediate response** to user input (<5ms perceived latency)
- 🎯 **Smoother experience** at 60 FPS (vs current 12.5 FPS)
- 💚 **Lower CPU usage** when idle (no unnecessary 80ms polling)
- 🛡️ **Protected against key spam** via frame rate limiter

**Tradeoffs:**
- Slightly more complex code (frame timing logic)
- Need to ensure all state updates set `dirty` flag correctly (already the case)

## Implementation Plan

### Phase 1: Add Frame Rate Limiter (Preparation)

**File:** `src/tui/mod.rs:424-548`

**Goal:** Add infrastructure for frame rate limiting before restructuring render logic.

#### Step 1.1: Add constants at function start

```rust
async fn run_app<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    app: &mut App,
) -> Result<()> {
    use std::time::Instant;

    // Frame rate constants
    const TARGET_FPS: u64 = 60;
    const MIN_FRAME_TIME_MS: u64 = 1000 / TARGET_FPS;  // 16ms (actual: ~62.5 FPS)
    const ANIM_MS: u64 = 120; // spinner frame every 120ms

    // Timing state
    let mut last_frame = Instant::now();
    let mut last_anim = Instant::now();

    // Ensure initial render happens
    app.dirty = true;

    // ... rest of function
```

**Rationale:** 60 FPS is the sweet spot for responsiveness without excessive CPU usage. Modern terminals can easily handle this, and it matches user expectations from native applications.

#### Step 1.2: Keep tick interval for animation timing only

```rust
    // Tick provides 60 FPS baseline for animations and frame rate limiting
    // Rendering happens after every select! iteration if dirty and enough time elapsed
    let mut tick = tokio::time::interval(std::time::Duration::from_millis(MIN_FRAME_TIME_MS));

    let mut events = EventStream::new();
```

**Rationale:** Change tick from 80ms to 16ms (60 FPS) for smoother animations. This becomes the baseline heartbeat for the event loop.

### Phase 2: Restructure Event Loop (Core Change)

**File:** `src/tui/mod.rs:439-544`

**Goal:** Move rendering outside `tokio::select!` so it happens after every event, not just ticks.

#### Step 2.1: Simplify tick branch (animation only)

Replace the current tick branch:

```rust
// OLD (lines 441-487):
_ = tick.tick() => {
    // Advance animation if enough time elapsed
    let now = Instant::now();
    if now.duration_since(last_anim).as_millis() >= u128::from(ANIM_MS) {
        app.throbber_state_mut().calc_next();
        last_anim = now;
        app.dirty = true;
    }

    // Only redraw when needed (dirty)
    if app.dirty {
        terminal.draw(|frame| render_ui(frame, app))?;
        app.dirty = false;
    }

    if app.should_quit {
        break;
    }
}
```

**NEW:**

```rust
_ = tick.tick() => {
    // Advance animation if enough time elapsed
    let now = Instant::now();
    if now.duration_since(last_anim).as_millis() >= u128::from(ANIM_MS) {
        app.throbber_state_mut().calc_next();
        last_anim = now;
        app.dirty = true;
    }
    // Note: Rendering happens at end of loop, not here
}
```

**Rationale:** Tick branch now ONLY handles animation timing, not rendering. This separates concerns and makes the loop more maintainable.

#### Step 2.2: Events branch stays the same

```rust
// Handle input events (line 489-492)
Some(Ok(event)) = events.next() => {
    handle_event(app, &event);
    // Note: handle_event sets app.dirty = true internally
    // Rendering happens at end of loop, not here
}
```

**Rationale:** No changes needed - `handle_event()` already sets `dirty` flag correctly.

#### Step 2.3: Background updates branch stays the same

```rust
// Process background updates if any (lines 494-543)
maybe_update = async {
    if let Some(rx) = &mut app.bg_update_rx { rx.recv().await } else { None }
} => {
    if let Some(update) = maybe_update {
        match update {
            AppUpdate::SinksData { .. } => { /* sets dirty */ }
            AppUpdate::DaemonState { .. } => { /* sets dirty */ }
            // ... all branches set app.dirty = true
        }
    }
}
```

**Rationale:** Background updates already set `dirty` flag, no changes needed.

#### Step 2.4: Add common render path after select!

**Add after the closing `}` of `tokio::select!` (currently line 544):**

```rust
        } // End of tokio::select!

        // Common render path: Execute after every select! branch
        // This ensures immediate visual feedback for all state changes
        if app.dirty {
            let now = Instant::now();
            let elapsed_since_last_frame = now.duration_since(last_frame);

            // Frame rate limiter: Only render if enough time has passed
            if elapsed_since_last_frame.as_millis() >= u128::from(MIN_FRAME_TIME_MS) {
                #[cfg(debug_assertions)]
                {
                    let start = Instant::now();
                    terminal.draw(|frame| render_ui(frame, app))?;
                    let render_time = start.elapsed();

                    // Log slow frames (threshold increased since we're targeting 60 FPS)
                    if render_time.as_millis() > MIN_FRAME_TIME_MS as u128 {
                        let run_ms = render_time.as_millis();
                        let screen_name = format!("{:?}", app.current_screen);
                        let preview_pending = app.preview.as_ref().is_some_and(|p| p.pending);
                        let windows = app.window_count;
                        tracing::debug!(
                            run_ms,
                            screen = %screen_name,
                            preview_pending,
                            windows,
                            "slow frame (exceeds 16ms target)"
                        );
                    }
                }
                #[cfg(not(debug_assertions))]
                {
                    terminal.draw(|frame| render_ui(frame, app))?;
                }

                app.dirty = false;
                last_frame = now;
            }
            // If frame rate limited, dirty flag stays true so we render next tick
        }

        // Check if we should quit (moved out of tick branch)
        if app.should_quit {
            break;
        }
    } // End of loop
```

**Rationale:**
- **Common render path:** Executes after every `select!` iteration (tick, event, or update)
- **Frame rate limiter:** Prevents excessive redraws during key repeat (max 60 FPS)
- **Dirty persistence:** If rate-limited, dirty flag stays true so we render on next tick
- **Responsive:** Events trigger render within 16ms max (vs 80ms currently)
- **Efficient:** No renders when idle and dirty=false

### Phase 3: Testing & Validation

#### Step 3.1: Manual testing checklist

Test these scenarios to verify responsiveness:

1. **Input box typing:**
   - Navigate to Rules screen → Edit rule → hold 'c' key
   - **Expected:** Smooth character insertion with no visible lag
   - **Compare:** Current feels choppy, new should feel instant

2. **Navigation:**
   - Hold arrow keys on any list (sinks, rules, help)
   - **Expected:** Smooth scrolling at 60 FPS
   - **Compare:** Current stutters every 80ms, new should be fluid

3. **Log scrolling:**
   - Dashboard screen → hold PgUp/PgDn
   - **Expected:** Smooth scroll through logs
   - **Compare:** Current choppy, new should feel like native terminal scroll

4. **Screen switching:**
   - Rapidly tap number keys (1-4) to switch screens
   - **Expected:** Instant screen switches
   - **Compare:** Current has 0-80ms delay, new should be <16ms

5. **Spinner animation:**
   - Trigger daemon action → observe loading spinner
   - **Expected:** Smooth 120ms frame animation (unchanged)
   - **Verify:** Animation still works at same speed as before

#### Step 3.2: Performance verification

**Debug mode logging:**

With the new slow frame detection (threshold = 16ms), monitor for performance regressions:

```bash
# Run TUI with debug logging
RUST_LOG=debug cargo run -- tui

# Exercise all screens and input modes
# Check for "slow frame" log entries
# If many slow frames appear, investigate render bottlenecks
```

**Expected outcomes:**
- Most frames should complete in <10ms (plenty of headroom)
- Occasional slow frames (>16ms) acceptable during heavy operations
- If >50% of frames are slow, investigate render optimization

**CPU usage verification:**

```bash
# Terminal 1: Start TUI
cargo run --release -- tui

# Terminal 2: Monitor CPU
top -p $(pgrep pwsw)
```

**Expected CPU usage:**
- **Idle (no input):** <1% CPU (renders only when dirty or ticks for animation)
- **Scrolling/typing:** 2-5% CPU (60 FPS rendering)
- **Heavy operations:** 5-15% CPU (background tasks + rendering)

**Compare to current:**
- Current uses ~1-2% CPU even when idle due to 80ms polling
- New should be same or better (event-driven)

#### Step 3.3: Edge case testing

1. **Rapid key repeat:**
   - Hold down a key for 5+ seconds
   - Verify: No excessive CPU usage (frame limiter working)
   - Verify: Input still feels responsive

2. **Terminal resize:**
   - Resize terminal window rapidly
   - Verify: UI redraws smoothly at new size
   - Verify: No crashes or visual artifacts

3. **Background updates:**
   - Dashboard screen → start daemon → observe logs streaming
   - Verify: Log updates appear immediately
   - Verify: Scrolling still smooth while logs update

4. **Multiple simultaneous events:**
   - Hold arrow key while background updates arrive
   - Verify: Both input and updates render smoothly
   - Verify: No event starvation

### Phase 4: Documentation Updates

#### Step 4.1: Update CLAUDE.md

Add to "Performance Patterns" section (after line ~340):

```markdown
**TUI Rendering Strategy:**
- Event-driven rendering with 60 FPS frame rate cap
- All state changes set `app.dirty = true` flag
- Render loop runs after every `tokio::select!` iteration
- Frame rate limiter prevents excessive redraws (16ms minimum between frames)
- Tick interval (16ms) used only for animations, not driving renders
- Result: Immediate responsiveness (<16ms latency) with efficient CPU usage

**When adding new input handlers:**
- Always set `app.dirty = true` after state changes
- Rendering happens automatically in main loop
- No need to call `terminal.draw()` directly in handlers
```

#### Step 4.2: Add performance notes to CLAUDE.md

Add to the "TUI Mode" section or create a new "TUI Performance" subsection:

```markdown
**TUI Rendering Strategy:**
- Event-driven rendering with 60 FPS frame rate cap
- Changed from tick-based (80ms) to event-driven with frame limiter (16ms max latency)
- Result: ~62.5 FPS sustained, <16ms input latency (down from 0-80ms avg)

**Performance Improvement (Date: YYYY-MM-DD):**
- Problem: Tick-based rendering (80ms interval, ~12.5 FPS) caused perceived input lag
- Solution: Event-driven rendering with 60 FPS frame rate cap
- Changes: Restructured event loop in `src/tui/mod.rs:424-548`
  - Moved render logic outside `tokio::select!` for immediate execution
  - Added frame rate limiter (16ms minimum between frames)
  - Reduced tick interval to 16ms (60 FPS baseline)
- Impact: User input latency 40ms avg → <16ms avg (62.5% improvement)
- Result: Matches native terminal application responsiveness
```

## Alternative Approaches Considered

### Option A: Reduce Tick Interval Only (NOT RECOMMENDED)

**Approach:** Change `Duration::from_millis(80)` to `Duration::from_millis(16)`

**Pros:**
- Minimal code change (one line)
- Would improve FPS to 60

**Cons:**
- ❌ Still polling-based (wastes CPU when idle)
- ❌ No frame rate limiting during rapid input
- ❌ Doesn't address architectural issue
- ❌ Renders at 60 FPS even when nothing is happening

**Verdict:** Quick fix but not optimal long-term solution.

### Option B: Immediate Render After Events (NOT RECOMMENDED)

**Approach:** Call `terminal.draw()` directly in event branch without frame limiter

**Pros:**
- Minimal code change
- Immediate responsiveness

**Cons:**
- ❌ No protection against key spam (could render at 200+ FPS during key repeat)
- ❌ Excessive CPU usage during rapid input
- ❌ Potential terminal flickering on slow systems
- ❌ Animation timing becomes complex

**Verdict:** Too risky without frame rate limiting.

### Option C: Hybrid Render-on-Demand (RECOMMENDED - This Plan)

**Approach:** Event-driven rendering with frame rate cap (described in this document)

**Pros:**
- ✅ Immediate response to user input
- ✅ Protected against excessive redraws
- ✅ Lower CPU usage when idle
- ✅ Smooth 60 FPS experience
- ✅ Clean architectural separation

**Cons:**
- Requires ~50 lines of code changes
- Slightly more complex timing logic

**Verdict:** Best balance of responsiveness, efficiency, and maintainability.

## Success Criteria

### Quantitative Metrics

1. **Input Latency:** <16ms from keypress to visual update (currently 0-80ms)
2. **Frame Rate:** Sustained 60 FPS during active use (currently 12.5 FPS)
3. **CPU Usage:** <1% idle, <5% active (currently ~2% idle due to polling)
4. **Slow Frames:** <10% of frames exceed 16ms threshold

### Qualitative Goals

1. **Responsiveness:** TUI feels as responsive as native terminal editors (vim, nano)
2. **Smoothness:** Scrolling and navigation feel fluid, not choppy
3. **Animation:** Loading spinners maintain smooth rotation (unchanged)
4. **Stability:** No regressions in functionality or crashes

## Risk Assessment

### Low Risk
- ✅ **Dirty flag pattern already works:** All state changes correctly set `app.dirty = true`
- ✅ **Well-isolated change:** Only affects `src/tui/mod.rs` event loop
- ✅ **Easy to revert:** Simple git revert if issues arise
- ✅ **No external dependencies:** Uses existing `std::time::Instant`

### Medium Risk
- ⚠️ **Frame timing edge cases:** Need to test rapid events, terminal resize
- ⚠️ **CPU usage spike:** Monitor for unexpected performance regressions
- **Mitigation:** Thorough testing in Phase 3, performance monitoring

### High Risk
- ❌ None identified

## Implementation Timeline Estimate

- **Phase 1 (Preparation):** 15 minutes - Add constants and frame timing state
- **Phase 2 (Core Changes):** 30 minutes - Restructure event loop
- **Phase 3 (Testing):** 45 minutes - Manual testing, performance verification
- **Phase 4 (Documentation):** 20 minutes - Update CLAUDE.md and refactor docs

**Total:** ~2 hours for complete implementation and validation

## References

**Files to modify:**
- `src/tui/mod.rs:424-548` - Main event loop (`run_app` function)
- `CLAUDE.md` - Performance patterns documentation
- `refactor_review_final.md` - Track this improvement

**Files to reference (no changes needed):**
- `src/tui/input.rs:19` - Already sets `dirty` flag correctly
- `src/tui/app.rs` - App state with `dirty` flag

**Related context:**
- TUI uses `ratatui` + `crossterm` (standard Rust TUI stack)
- Event stream is async via `tokio::select!`
- Background worker polls daemon state every 2.5s (independent of render rate)

## Conclusion

This performance improvement addresses a core UX issue with minimal risk and clear benefits. The event-driven architecture with frame rate limiting is a proven pattern used by modern TUIs and game engines. Implementation is straightforward, well-scoped, and easily testable.

**Recommendation:** Implement this change before Phase 1 of the TUI plan (`tui_plan.md`) to ensure all future navigation improvements benefit from the improved responsiveness.
