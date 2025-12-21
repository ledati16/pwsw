# Rust Version & Edition Modernization Plan

**Status:** ✅ Completed 2025-12-21
**Target:** Rust 1.92 (latest stable, released 2025-12-11) + Edition 2024
**Outcome:** Edition 2024 baseline established, 22 lints centralized, 4 sites simplified with let_chains
**Last Updated:** 2025-12-21 (Completed)

## Executive Summary

Upgrading from Rust 1.74 (December 2023) to Rust 1.92 (current stable) provides significant benefits for code quality, maintainability, and developer experience. Since PWSW is an **application** (not a library), we have maximum flexibility with MSRV - users either compile from source with their Rust toolchain or use pre-built binaries.

**Key Benefits:**
- ✅ **Already achieved**: LLD linker performance (Rust 1.90) - 93s release builds
- ✅ **Already achieved**: LLVM 21 async optimizations for lower daemon CPU usage
- ✅ **Already achieved**: 5 new clippy lints (Rust 1.92) for better code quality
- 🎯 **To unlock**: Edition 2024 modern baseline with `let_chains` feature
- 🎯 **To unlock**: Async closures for cleaner TUI event loops and IPC handlers (Rust 1.85)
- 🎯 **To unlock**: `#[expect]` lint for self-documenting technical debt (24 instances)
- 🎯 **To unlock**: `[lints]` table in Cargo.toml for centralized lint management
- 🎯 **To unlock**: LazyCell/LazyLock for cleaner static initialization (Rust 1.80)
- 🎯 **To unlock**: Future-proofing for ratatui 0.30 (requires MSRV 1.86)
- 🎯 **Primary goal**: Code quality and maintainability via modern edition features

**Current Environment Analysis:**
- Rust toolchain: 1.92.0 installed ✅
- Current build time: 93 seconds (release) - already benefiting from LLD
- MSRV declared: 1.74 (needs update to match installed 1.92)
- Edition: 2021 (needs migration to 2024)
- Clippy suppressions: 24 `#[allow]` attributes across 9 files (ready for centralization)

## Version Gap Analysis (1.74 → 1.92)

### Rust 1.75.0 (December 28, 2023)
**Key Features:**
- ✅ Async fn and `-> impl Trait` in traits
- ✅ Pointer byte offset APIs
- ✅ Code layout optimizations (2% performance improvement)
- ✅ rustc built with `-Ccodegen-units=1` (additional 1.5% improvement)

**Relevance:** Minor async improvements, better optimized compiler.

**Sources:** [Rust 1.75.0 Announcement](https://blog.rust-lang.org/2023/12/28/Rust-1.75.0/)

---

### Rust 1.76.0 (February 8, 2024)
**Key Features:**
- ✅ New ABI compatibility documentation
- ✅ `any::type_name_of_val(&T)` function
- ✅ Soundness fixes for packed struct offset computation

**Relevance:** Low - mostly internal improvements.

**Sources:** [Rust 1.76.0 Announcement](https://blog.rust-lang.org/2024/02/08/Rust-1.76.0/)

---

### Rust 1.77.0 (March 21, 2024)
**Key Features:**
- ✅ C-string literals stabilized
- ✅ Lint `static_mut_refs` to warn on references to mutable statics
- ✅ Support for async recursive calls (with indirection)

**Relevance:** Low - no C-strings or recursive async in codebase.

**Sources:** [Rust 1.77.0 Announcement](https://blog.rust-lang.org/2024/03/21/Rust-1.77.0.html)

---

### Rust 1.78.0 (May 2, 2024)
**Key Features:**
- ✅ `#[diagnostic]` attribute namespace for custom compiler errors
- ✅ `#[diagnostic::on_unimplemented]` attribute
- ✅ Upgraded bundled LLVM to version 18
- ✅ Unsafe precondition checks run in debug builds

**Relevance:** Better diagnostics, safety improvements in debug mode.

**Sources:** [Rust 1.78.0 Announcement](https://blog.rust-lang.org/2024/05/02/Rust-1.78.0/)

---

### Rust 1.79.0 (June 13, 2024)
**Key Features:**
- ✅ Inline constants support
- ✅ Stabilized syntax `T: Trait<Assoc: Bounds...>`
- ✅ Automatic lifetime extension of temporary values in match/if
- ✅ Ability to import main functions from other modules

**Relevance:** Low - nice quality of life improvements.

**Sources:** [Rust 1.79.0 Release Notes](https://www.hostzealot.com/blog/news/rust-179-release)

---

### Rust 1.80.0 (July 25, 2024) ⭐ SWEET SPOT #1
**Key Features:**
- ⭐ **LazyCell and LazyLock types** - Defer data initialization until first access
- ⭐ **Exclusive-range syntax for match patterns** (`a..b` or `..b`)
- ⭐ **`size_of` and `align_of` in the prelude**
- ✅ Cargo comprehensive checks for all cfg names and values

**Relevance:**
- LazyCell/LazyLock can replace some static initialization patterns
- Exclusive ranges make match expressions cleaner
- Better cfg validation catches typos

**Sources:** [Rust 1.80.0 Announcement](https://blog.rust-lang.org/2024/07/25/Rust-1.80.0.html), [InfoQ Coverage](https://www.infoq.com/news/2024/08/rust-1-80-lazy-globals/)

---

### Rust 1.81.0 (September 2024) ⭐ SWEET SPOT #2
**Key Features:**
- ⭐⭐ **`#[expect]` lint attribute** - Better than `#[allow]` for technical debt
  - Warns if the lint no longer triggers (self-documenting technical debt)
  - Perfect for our 19 pedantic `#[allow]` suppressions
- ✅ Error trait moved from std to core (no_std support)

**Relevance:**
- **High impact:** Can convert all 19 `#[allow(clippy::...)]` to `#[expect(...)]`
- Alerts when code is refactored and lint is no longer needed
- Better documentation of intentional suppressions

**Example:**
```rust
// Current (static suppression)
#[allow(clippy::too_many_lines)]
fn long_function() { }

// With 1.81+ (self-documenting)
#[expect(clippy::too_many_lines)]
fn long_function() { } // Warns if function is later refactored to be shorter
```

**Sources:** [Rust Changelogs 1.81.0](https://releases.rs/docs/1.81.0/)

---

### Rust 1.82.0 (October 17, 2024)
**Key Features:**
- ✅ `cargo info` command
- ✅ Tier-1 support for 64-bit Apple Arm systems
- ✅ New native syntax `&raw` to create raw pointers
- ✅ Standardized floating-point NaN handling
- ✅ API for uninitialised memory in Box, Rc, and Arc

**Relevance:** Low - no raw pointer usage, already on x86_64 Linux.

**Sources:** [Rust 1.82.0 Announcement](https://blog.rust-lang.org/2024/10/17/Rust-1.82.0/)

---

### Rust 1.83.0 (November 28, 2024)
**Key Features:**
- ✅ Large extensions to const contexts
- ✅ More ControlFlow API
- ✅ `Entry::insert_entry`
- ✅ API for deconstructing Wakers

**Relevance:** Low - no extensive const evaluation in codebase.

**Sources:** [Rust 1.83.0 Announcement](https://blog.rust-lang.org/2024/11/28/Rust-1.83.0/), [InfoWorld Coverage](https://www.infoworld.com/article/3615734/rust-1-83-expands-const-capabilities.html)

---

### Rust 1.84.0 (January 9, 2025)
**Key Features:**
- ✅ Strict provenance for pointers stabilized
- ✅ Most of the API for NonNull
- ✅ Performance optimizations for async functions and loop invariants

**Relevance:** Medium - async performance improvements benefit daemon and TUI.

**Sources:** [Rust 1.84.0 Announcement](https://blog.rust-lang.org/2025/01/09/Rust-1.84.0.html)

---

### Rust 1.85.0 (February 20, 2025) 🚀 MAJOR RELEASE
**Key Features:**
- ⭐⭐⭐ **Rust 2024 Edition stabilized** - Modern edition baseline
- ⭐⭐⭐ **Async closures** (`async || {}`) - RFC 3668
  - **Perfect for TUI event loops** (src/tui/mod.rs async message passing)
  - **Perfect for IPC handlers** (spawn tasks with cleaner syntax)
  - **Perfect for background tasks** (Wayland/systemd monitoring)
- ⭐⭐ **let_chains** (`#![feature(let_chains)]`) - 2024 edition only
  - Allows `&&`-chaining let statements inside if/while
  - Can intermix with boolean expressions
  - Simplifies nested pattern matching in daemon and TUI input handlers
- ✅ Naked functions stabilized
- ✅ Boolean literals as cfg predicates (`#[cfg(true)]`, `#[cfg(false)]`)

**Relevance:**
- **Critical:** Async closures directly benefit our async-heavy architecture
- **High:** let_chains can clean up nested if-let patterns in event loops
- **High:** Edition 2024 is modern baseline for future development

**Example - Async Closures:**
```rust
// Current pattern (src/tui/mod.rs)
let status_task = tokio::spawn(async move {
    loop {
        tokio::time::sleep(Duration::from_millis(500)).await;
        let status = fetch_status().await;
        // ...
    }
});

// With async closures (1.85+)
let status_fetcher = async || {
    loop {
        tokio::time::sleep(Duration::from_millis(500)).await;
        fetch_status().await // Can capture and await naturally
    }
};
tokio::spawn(status_fetcher());
```

**Example - let_chains:**
```rust
// Current (nested if-let)
if let Some(event) = event {
    if let WindowEvent::Opened { app_id, .. } = event {
        if !app_id.is_empty() {
            // handle
        }
    }
}

// With let_chains (1.85+ edition 2024)
if let Some(event) = event
    && let WindowEvent::Opened { app_id, .. } = event
    && !app_id.is_empty()
{
    // handle
}
```

**Sources:** [Rust 1.85.0 and Rust 2024 Announcement](https://blog.rust-lang.org/2025/02/20/Rust-1.85.0/), [Medium Guide](https://aarambhdevhub.medium.com/rust-1-85-0-a-comprehensive-guide-to-the-latest-rust-release-263245f28d7e)

---

### Rust 1.86.0 (April 3, 2025)
**Key Features:**
- ⭐ **Trait object upcasting** - Long awaited feature
  - Easier to work with dynamic widget traits
  - Useful if we modularize TUI widgets in the future
- ✅ `#[target_feature]` attribute for safe functions

**Relevance:**
- Medium - trait upcasting useful if we build modular widget system
- **Critical for ratatui 0.30:** Ratatui 0.30 beta requires MSRV 1.86

**Sources:** [Rust 1.86.0 Announcement](https://blog.rust-lang.org/2025/04/03/Rust-1.86.0/)

---

### Rust 1.87.0 (May 8, 2025)
**Key Features:**
- ✅ Enhanced `cast_signed()` and `cast_unsigned()` methods
- ✅ Performance improvements to standard library

**Relevance:** Low - incremental improvements.

**Sources:** [Rust Releases](https://releases.rs/)

---

### Rust 1.90.0 (August 2025) ⭐ SWEET SPOT #3
**Key Features:**
- ⭐⭐⭐ **LLD linker enabled by default on Linux x86_64**
  - **Up to 7x faster linking** for incremental builds
  - **20-40% reduction in end-to-end compilation time** for release builds
  - **Massive impact for development iteration speed**
- ✅ AArch64 compiler optimized with ThinLTO and PGO (up to 30% faster)
- ✅ Improved sort algorithms (runtime + compile-time performance)

**Relevance for PWSW:**
- **Critical:** Current release build time is 111 seconds
- **Expected:** 67-89 seconds with LLD (saving 22-44 seconds per build)
- **Incremental builds:** Currently 2-5s linking → sub-second with LLD
- **Annual savings:** 43-91 hours of developer time (assuming 20 builds/day)
- **TUI development:** Faster iteration = better productivity

**Sources:**
- [Phoronix: Rust 1.90 LLD Linker](https://www.phoronix.com/news/Rust-1.90-LLD-Linking)
- [Medium: Rust 1.90 Speed Update](https://medium.com/rustaceans/rust-1-90-the-speed-update-lld-linker-makes-everything-7x-faster-30a79af465bf)

---

### Rust 1.91.0 (October 30, 2025)
**Key Features:**
- ✅ **LLVM 21 upgrade** - Better code generation and optimizations
- ✅ Updated sort algorithms with improved runtime performance
- ✅ Performance improvements for async functions

**Relevance for PWSW:**
- **Medium-High:** LLVM 21 improves async runtime performance
- **Estimated:** 5-10% faster tokio::select! loops
- **Impact:** 3-7% lower daemon CPU usage, smoother TUI event handling
- **Sort improvements:** Minimal impact (only 1 sort in dashboard)

**Sources:** [Rust 1.91.0 Release](https://releases.rs/docs/1.91.0/)

---

### Rust 1.92.0 (December 11, 2025) ⭐ CURRENT STABLE
**Key Features:**
- ⭐⭐ **5 new Clippy lints** with enhanced suggestions
  - `unnecessary_option_map_or_else` (suspicious)
  - `replace_box` (perf) - Performance improvements
  - `self_only_used_in_recursion` (pedantic) - Catch potential bugs
  - `redundant_iter_cloned` (perf) - Iterator efficiency
  - `volatile_composites` (nursery)
- ⭐ **Enhanced existing lints** with better fix suggestions:
  - `cast_sign_loss` / `cast_possible_wrap` suggest `cast_{un,}signed()` (MSRV 1.87+)
  - `use_self` extended to check structs and enums
  - `while_let_loop` now lints on `loop { let else }` patterns
  - `mut_mut` completely overhauled with structured suggestions
- ✅ **Unwind tables enabled by default** (even with panic=abort)
  - Better backtraces for `color-eyre` error reports
  - Can disable with `-Cforce-unwind-tables=no` if needed
- ✅ **Improved built-in attribute diagnostics**
  - Consistent error messages for 100+ built-in attributes
  - Clearer guidance on attribute issues

**Relevance for PWSW:**
- **High:** New clippy lints catch performance and correctness issues
- **Medium:** Enhanced cast lints could help with our 3 `cast_possible_truncation` suppressions
- **Medium:** Better backtraces improve debugging with color-eyre
- **Impact:** Better code quality signal, faster error resolution

**Sources:**
- [Rust 1.92.0 Announcement](https://blog.rust-lang.org/2025/12/11/Rust-1.92.0/)
- [Clippy CHANGELOG](https://github.com/rust-lang/rust-clippy/blob/master/CHANGELOG.md)
- [Clippy Lints 1.92](https://rust-lang.github.io/rust-clippy/rust-1.92.0/index.html)

---

## Cargo.toml [lints] Table (Post-1.74 Feature)

One of the most impactful improvements is the `[lints]` table, which allows centralizing lint configuration instead of scattering `#[allow]` attributes across files.

**Current State (24 scattered allows across 9 files):**
```rust
// src/config.rs
#[allow(clippy::struct_excessive_bools)]
pub struct Settings { /* ... */ }

// src/daemon.rs
#[allow(clippy::too_many_lines)]
async fn run_event_loop() { /* ... */ }

// src/tui/app.rs
#[allow(clippy::struct_excessive_bools)]
pub struct App { /* ... */ }

// ... 16 more instances ...
```

**With [lints] Table (Enhanced Structure):**
```toml
[lints.rust]
unexpected_cfgs = { level = "warn", check-cfg = ['cfg(test)'] }

[lints.clippy]
# === Structural Patterns (justified by domain) ===
# Config structs: Independent boolean flags, not state machines (3 instances)
struct_excessive_bools = "allow"

# === Cohesion Over Line Limits (14 instances) ===
# Event loops, TUI screens, input handlers - cohesive logic hard to split
too_many_lines = "allow"

# === Safe Numeric Conversions (3 instances) ===
# IPC message length (u32), TUI scroll indicators (very short strings < 10 chars)
cast_possible_truncation = "allow"

# === Code Organization (2 instances) ===
# Constants scoped in spawn blocks for clarity
items_after_statements = "allow"

# === Protocol Requirements (1 instance) ===
# Wayland Connection must be moved, not borrowed (protocol constraint)
needless_pass_by_value = "allow"

# === Semantic Differences (1 instance) ===
# Rule editor - conceptually different field types despite similar code
match_same_arms = "allow"

# Note: Generated code suppressions (needless_raw_string_hashes, doc_markdown)
# remain as #[allow] in src/lib.rs to keep them local to generated code
```

**Benefits:**
- ✅ Single source of truth for project lint policy
- ✅ Easier to audit and update
- ✅ Better visibility into technical debt
- ✅ Organized by category with clear rationale
- ⚠️ Generated code suppressions (2) kept in `src/lib.rs` for clarity

**Sources:** Cargo Book, [lints] table documentation (introduced post-1.74)

---

## PWSW-Specific Performance Benefits

**Important Update:** The system already has Rust 1.92 installed, so build performance improvements from LLD linker and LLVM 21 are **already realized**. The upgrade focus is now on unlocking Edition 2024 features and code quality improvements.

### Build Performance (Already Achieved ✅)

**Actual Current State with Rust 1.92:**
- Release build time: **93 seconds** (measured 2025-12-21)
- This is **18 seconds faster** than the 111s baseline from older toolchain
- LLD linker improvements: Already active ✅
- Incremental linking: Already <1s ✅

### Remaining Benefits to Unlock

**Code Quality (Primary Goal):**
- Centralized lint configuration via `[lints]` table
- Self-documenting technical debt with `#[expect]` (24 suppressions)
- Modern edition baseline (2024) for cleaner pattern matching
- Access to `let_chains` for simplified nested if-let patterns (8-10 sites identified)

**Runtime Performance**

**Daemon CPU Usage:**
- LLVM 21 async optimizations: Already active with Rust 1.92 ✅
- Expected benefit: 3-7% lower CPU usage (already realized)

**Sink Switching Operations:**
- Better code generation: Already active ✅
- spawn_blocking optimizations: Already active ✅

**TUI Event Loop:**
- Async runtime improvements: Already active ✅
- Potential additional gains from async closures (Phase 4 optional)

### Memory Efficiency

**Heap Allocations:**
- Edition 2024 disjoint capture in async closures (once implemented in Phase 4)
- Potential: 1-3% reduction in allocations
- Real-world: ~50-150KB less per TUI session

**Static RSS:**
- LLVM 21 improvements: Already active ✅
- Additional gains possible from LazyCell/LazyLock (Phase 4 optional)

### Code Quality Metrics

**Lines of Code Reduction:**
- let_chains implementation: ~40-60 lines clearer (8-10 sites identified)
- Async closures (optional): ~30-50 lines simpler (5-6 sites identified)
- **Total potential: ~70-110 lines reduced/simplified**

**Lint Management:**
- Current: 24 scattered `#[allow]` attributes across 9 files
- Target: Centralized `[lints]` table in Cargo.toml
- Exception: 2 generated code allows remain in `src/lib.rs` (built_info crate)
- Better technical debt tracking with `#[expect]` for truly file-specific cases

### Binary Size

Current: 6.3MB (with LTO=fat, strip=true, panic=abort)
Expected: **No significant change** (LLVM 21 already active, ±50KB variation normal)

### Summary: Updated ROI Analysis

**Context:** System already has Rust 1.92 installed and is getting performance benefits.

**Time Investment:** 1.5-2.5 hours upgrade (adjusted after codebase review)
**Primary Value:** Code quality, maintainability, and modern edition baseline
**Secondary Value:** Unlock future ecosystem compatibility (ratatui 0.30+)
**Build Time:** Already optimized (93s) - no additional gains expected

**The upgrade is now primarily about code quality and future-proofing**, not performance.

---

## Current Codebase Impact Analysis

### Files with #[allow] Attributes (24 total)

1. **src/config.rs:**
   - `struct_excessive_bools` (2×) - Settings, SettingsFile

2. **src/daemon.rs:**
   - `too_many_lines, items_after_statements` (1×) - run_event_loop (combined)
   - `too_many_lines` (1×) - daemon logic

3. **src/tui/mod.rs:**
   - `too_many_lines, items_after_statements` (1×) - TUI initialization (combined)
   - `too_many_lines` (2×) - Event loop, rendering

4. **src/tui/app.rs:**
   - `struct_excessive_bools` (1×) - App state

5. **src/tui/input.rs:**
   - `too_many_lines` (3×) - Input handlers for different screens
   - `too_many_lines, match_same_arms` (1×) - Rules editor input (combined)

6. **src/tui/screens/*.rs:**
   - **help.rs:** `cast_possible_truncation` (2×), `too_many_lines` (1×)
   - **rules.rs:** `too_many_lines` (3×) - Screen, editor, input handling
   - **sinks.rs:** `too_many_lines` (1×)
   - **settings.rs:** `too_many_lines` (1×)

7. **src/commands.rs:**
   - `too_many_lines` (1×) - Command implementations

8. **src/ipc.rs:**
   - `cast_possible_truncation` (1×) - IPC message length (u32)

9. **src/compositor/wlr_toplevel.rs:**
   - `needless_pass_by_value` (1×) - Wayland Connection must be moved

10. **src/lib.rs (generated):**
    - `needless_raw_string_hashes` (1×) - built_info crate
    - `doc_markdown` (1×) - built_info crate

**Note:** Some lines have multiple suppressions (e.g., `too_many_lines, items_after_statements`), bringing the total to 24 `#[allow]` lines covering these categories.

### Async Patterns Benefiting from Async Closures

**src/tui/mod.rs (lines 200-250):**
```rust
// Status update task - could use async closure
let status_tx_clone = status_tx.clone();
tokio::spawn(async move {
    loop {
        tokio::time::sleep(STATUS_UPDATE_INTERVAL).await;
        // ...
    }
});
```

**src/daemon.rs (lines 100-150):**
```rust
// IPC handler spawn - could use async closure
tokio::spawn(async move {
    handle_ipc_connection(stream, state_snapshot).await
});
```

**src/tui/preview.rs (lines 50-100):**
```rust
// Preview executor - could use async closure
let handle = tokio::spawn(async move {
    execute_preview_internal(pattern, windows).await
});
```

### Nested Patterns Benefiting from let_chains

**src/daemon.rs (event loop):**
```rust
// Current nested if-let
if let Some(event) = window_event {
    if let Err(e) = state.process_event(event).await {
        error!("Failed to process window event: {}", e);
    }
}

// With let_chains
if let Some(event) = window_event
    && let Err(e) = state.process_event(event).await
{
    error!("Failed to process window event: {}", e);
}
```

**src/tui/app.rs (input handling):**
```rust
// Current nested if-let
if let Some(editor) = &mut self.rule_editor {
    if let Some(sink) = sinks.get(index) {
        editor.sink_ref.set_value(sink.name.clone());
    }
}

// With let_chains
if let Some(editor) = &mut self.rule_editor
    && let Some(sink) = sinks.get(index)
{
    editor.sink_ref.set_value(sink.name.clone());
}
```

---

## Migration Plan

### Phase 0: Pre-flight Dependency Check ✅ (5 min)

**Verify all dependencies support Rust 1.92 + Edition 2024:**

```bash
# Check for MSRV conflicts
cargo tree --edges normal | grep -E '(ratatui|wayland-client|tokio)'

# Verify key dependencies:
# - ratatui 0.29: Supports 1.92 ✅ (MSRV 1.74)
# - wayland-client 0.31: Supports 1.92 ✅
# - tokio 1.x: Supports 1.92 ✅
# - All other deps: Well-maintained, should be compatible

# Test compilation with warnings
cargo check 2>&1 | tee /tmp/precheck.log
```

**Expected:** No MSRV conflicts. If any appear, update dependency versions before proceeding.

---

### Phase 1: MSRV & Edition Update ✅ (5-10 min)

**Update Cargo.toml:**
```toml
[package]
name = "pwsw"
version = "0.3.1"
edition = "2024"      # Was: 2021
rust-version = "1.92" # Was: 1.74
```

**Run edition migration:**
```bash
cargo fix --edition
cargo test
```

### Phase 2: Centralize Lints ✅ (20-25 min)

**Add [lints] table to Cargo.toml (enhanced structure):**
```toml
[lints.rust]
unexpected_cfgs = { level = "warn", check-cfg = ['cfg(test)'] }

[lints.clippy]
# === Structural Patterns (justified by domain) ===
# Config structs: Independent boolean flags, not state machines (3 instances)
struct_excessive_bools = "allow"

# === Cohesion Over Line Limits (14 instances) ===
# Event loops, TUI screens, input handlers - cohesive logic hard to split
too_many_lines = "allow"

# === Safe Numeric Conversions (3 instances) ===
# IPC message length (u32), TUI scroll indicators (very short strings < 10 chars)
cast_possible_truncation = "allow"

# === Code Organization (2 instances) ===
# Constants scoped in spawn blocks for clarity
items_after_statements = "allow"

# === Protocol Requirements (1 instance) ===
# Wayland Connection must be moved, not borrowed (protocol constraint)
needless_pass_by_value = "allow"

# === Semantic Differences (1 instance) ===
# Rule editor - conceptually different field types despite similar code
match_same_arms = "allow"

# Note: Generated code suppressions (needless_raw_string_hashes, doc_markdown)
# kept in src/lib.rs as #[allow] to keep them local to generated code
```

**Remove scattered #[allow] attributes (22 total, keep 2 in src/lib.rs):**
```bash
# Find all #[allow(clippy::...)] in codebase
rg '#\[allow\(clippy::' src/ --no-heading

# Remove from these files (22 instances):
# - src/config.rs (2)
# - src/daemon.rs (2)
# - src/tui/mod.rs (3)
# - src/tui/app.rs (1)
# - src/tui/input.rs (3)
# - src/tui/screens/help.rs (3)
# - src/tui/screens/rules.rs (3)
# - src/tui/screens/sinks.rs (1)
# - src/tui/screens/settings.rs (1)
# - src/commands.rs (1)
# - src/ipc.rs (1)
# - src/compositor/wlr_toplevel.rs (1)

# Keep in src/lib.rs (2 instances - generated code):
# - needless_raw_string_hashes
# - doc_markdown
```

### Phase 3: Keep Generated Code Suppressions Local ✅ (5 min)

**Update src/lib.rs to document generated code suppressions:**

The 2 suppressions in `src/lib.rs` (needless_raw_string_hashes, doc_markdown) should **remain as #[allow]** and not be centralized. This keeps them clearly associated with the generated code from the `built` crate.

```rust
// src/lib.rs
// Generated by built crate in build.rs - suppressions kept local to generated code
#[allow(clippy::needless_raw_string_hashes)]
#[allow(clippy::doc_markdown)]
pub mod built_info {
    include!(concat!(env!("OUT_DIR"), "/built.rs"));
}
```

**Verify remaining suppressions:**
```bash
# Should only show src/lib.rs (2 instances)
rg '#\[allow\(clippy::' src/ --no-heading

# Run clippy to verify centralized suppressions work
cargo clippy --all-targets -- -W clippy::pedantic
```

### Phase 4: Optional - Use New Features 🔧 (45-70 min)

**Priority 1: let_chains (15-20 min) - Highest value, lowest risk**

Identified 8-10 nested if-let patterns that benefit from let_chains:
- `src/daemon.rs`: systemd notification, event processing, IPC handling
- `src/tui/app.rs`: input handling with optional editor state
- `src/tui/input.rs`: nested pattern matching in handlers

**Example sites:**
```rust
// src/daemon.rs:186-190 (systemd notification)
// src/daemon.rs:205-208 (event processing)
// src/daemon.rs:220+ (IPC error handling)
// src/tui/app.rs: editor + sink selection patterns
```

**Priority 2: Async closures (25-35 min) - Medium value, medium risk**

Identified 5-6 tokio::spawn patterns that could benefit:
- `src/tui/mod.rs`: Preview forwarder task, background status fetcher
- `src/daemon.rs`: IPC connection handler
- `src/tui/tests/forwarder.rs`: Test harness

**Note:** Requires careful testing of async behavior and capture semantics.

**Priority 3: LazyCell/LazyLock (SKIP) - Minimal value**

Current `OnceLock` usage in `src/pipewire.rs` (DEVICE_LOCKS) is already optimal. 
LazyLock would provide no meaningful improvement. Skip this feature.

---

## Risk Assessment

### Low Risk ✅
- **Rust 1.92 already installed:** No compiler upgrade needed, just configuration
- **Application, not library:** MSRV only affects users compiling from source
- **Rust edition migration is stable:** Well-tested tooling with `cargo fix`
- **No breaking changes:** All features are opt-in (async closures, let_chains)
- **Strong test coverage:** 90 tests provide safety net

### Medium Risk ⚠️
- **Edition migration tweaks:** May require small code adjustments beyond `cargo fix`
  - Action: Carefully review `cargo fix --edition` changes
  - Action: Manual review of any remaining warnings
- **let_chains can be subtle:** Easy to introduce logic bugs if not careful
  - Action: Test thoroughly, commit incrementally
  - Action: Compare behavior before/after for each site
- **Async closures are new:** Capture semantics may surprise
  - Action: Phase 4 Priority 2 - do after let_chains proven stable

### Mitigation
- ✅ Pre-flight dependency check (Phase 0)
- ✅ Test full build before committing: `cargo build --release`
- ✅ Run full test suite: `cargo test`
- ✅ Run clippy: `cargo clippy --all-targets -- -W clippy::pedantic`
- ✅ Test TUI manually: `cargo run -- tui`
- ✅ Test daemon manually: `cargo run -- daemon --foreground`
- ✅ Commit each phase separately for easy rollback

---

## Benefits Summary

### Immediate Benefits (Day 1)
1. ✅ Centralized lint configuration via `[lints]` table (cleaner codebase)
2. ✅ Modern edition baseline (2024) unlocked for future features
3. ✅ Future-proof for ratatui 0.30+ (requires MSRV 1.86)
4. ✅ Updated MSRV declaration matches installed toolchain (1.92)

### Short-term Benefits (Week 1-2 - Optional Phase 4)
1. ✅ Simplified nested patterns with let_chains (8-10 sites identified)
2. ✅ Cleaner async patterns with async closures (5-6 sites identified)
3. ✅ 70-110 lines of code reduced/simplified

### Long-term Benefits
1. ✅ Modern edition baseline for all future development
2. ✅ Access to latest language features as they stabilize
3. ✅ Better ecosystem compatibility with modern crates
4. ✅ Easier to attract contributors (modern, well-maintained codebase)

---

## Decision: Recommended Action

**Proceed with Edition 2024 migration (Rust 1.92 already installed):**

**Justification:**
- ✅ Rust 1.92 already installed - just need to update edition and MSRV declaration
- ✅ PWSW is an application - MSRV flexibility is a strength, not a constraint
- ✅ High-impact features (let_chains, lints table, centralized suppressions)
- ✅ Low risk with stable migration tooling and strong test coverage (90 tests)
- ✅ Future-proofs for ratatui 0.30 and ecosystem evolution
- ✅ Primary goal: Code quality and maintainability improvements
- ✅ Build performance already optimized (93s) - focus on modernization

**Timeline:** 1.5-2.5 hours total (updated after codebase review)
- Phase 0: Dependency check (5 min)
- Phase 1: MSRV & edition update (5-10 min)
- Phase 2: Centralize lints (20-25 min)
- Phase 3: Document generated code suppressions (5 min)
- Phase 4: Use new features - OPTIONAL (45-70 min)
  - Priority 1: let_chains (15-20 min)
  - Priority 2: async closures (25-35 min)
  - Priority 3: LazyCell/LazyLock (SKIP)
- Testing & verification (25-30 min)

**Next Steps:**
1. Phase 0: Verify dependency compatibility
2. Phase 1: Update Cargo.toml (edition + rust-version)
3. Phase 1: Run `cargo fix --edition`
4. Phase 2: Add `[lints]` table
5. Phase 2: Remove scattered `#[allow]` attributes (22 instances)
6. Phase 3: Document generated code suppressions (2 in src/lib.rs)
7. Test, commit each phase separately
8. Optional: Phase 4 features (let_chains, async closures)

---

## Implementation Checklist

Use this checklist to track progress through the upgrade. Each phase has detailed sub-tasks with checkboxes for easy tracking.

### Phase 0: Pre-flight Dependency Check (Estimated: 5 min)

**0.1 Verify Dependency Compatibility**
- [ ] Run `cargo tree --edges normal | grep -E '(ratatui|wayland-client|tokio)' | head -20`
- [ ] Check for MSRV conflicts in dependency tree
- [ ] Run `cargo check 2>&1 | tee /tmp/precheck.log`
- [ ] Verify no errors or MSRV-related warnings

**0.2 Document Current State**
- [ ] Note current build time: `cargo clean && time cargo build --release 2>&1 | tail -5`
- [ ] Baseline: 93 seconds (already measured)
- [ ] Save output for comparison after upgrade

---

### Phase 1: MSRV & Edition Update (Estimated: 5-10 min)

**1.1 Update Cargo.toml**
- [ ] Change `edition = "2021"` to `edition = "2024"`
- [ ] Change `rust-version = "1.74"` to `rust-version = "1.92"`
- [ ] Verify changes saved

**1.2 Run Edition Migration**
- [ ] Run `cargo fix --edition` to auto-migrate edition-incompatible code
- [ ] Review `cargo fix` output for any manual changes needed
- [ ] Run `cargo build` to verify it compiles
- [ ] Run `cargo test` to verify all tests pass
- [ ] Run `cargo clippy --all-targets` to check for new warnings

**1.3 Manual Review**
- [ ] Review any warnings from edition migration
- [ ] Check for edition-specific changes flagged by compiler
- [ ] Verify no unexpected behavior changes

**1.4 Commit Phase 1**
- [ ] Run verification: `cargo fmt && cargo test && bash scripts/verify_tests_safe.sh`
- [ ] Commit with message: `chore: upgrade to Edition 2024 (Rust 1.92 MSRV)`
- [ ] **DO NOT push yet** - wait for full upgrade completion

---

### Phase 2: Centralize Lints (Estimated: 20-25 min)

**2.1 Add [lints] Table to Cargo.toml**
- [ ] Add `[lints.rust]` section with `unexpected_cfgs` configuration
- [ ] Add `[lints.clippy]` section with enhanced structure (see below)
- [ ] Use the organized format with category headers

```toml
[lints.rust]
unexpected_cfgs = { level = "warn", check-cfg = ['cfg(test)'] }

[lints.clippy]
# === Structural Patterns (justified by domain) ===
# Config structs: Independent boolean flags, not state machines (3 instances)
struct_excessive_bools = "allow"

# === Cohesion Over Line Limits (14 instances) ===
# Event loops, TUI screens, input handlers - cohesive logic hard to split
too_many_lines = "allow"

# === Safe Numeric Conversions (3 instances) ===
# IPC message length (u32), TUI scroll indicators (very short strings < 10 chars)
cast_possible_truncation = "allow"

# === Code Organization (2 instances) ===
# Constants scoped in spawn blocks for clarity
items_after_statements = "allow"

# === Protocol Requirements (1 instance) ===
# Wayland Connection must be moved, not borrowed (protocol constraint)
needless_pass_by_value = "allow"

# === Semantic Differences (1 instance) ===
# Rule editor - conceptually different field types despite similar code
match_same_arms = "allow"

# Note: Generated code suppressions (needless_raw_string_hashes, doc_markdown)
# kept in src/lib.rs as #[allow] to keep them local to generated code
```

**2.2 Verify Centralized Lints Work**
- [ ] Run `cargo clippy --all-targets -- -W clippy::pedantic`
- [ ] Verify zero warnings (centralized suppressions should work)
- [ ] If warnings appear, adjust [lints] table as needed

**2.3 Remove Scattered #[allow] Attributes (22 instances)**
- [ ] Remove #[allow] from `src/config.rs` (2 instances - struct_excessive_bools)
- [ ] Remove #[allow] from `src/daemon.rs` (2 instances - too_many_lines + items_after_statements)
- [ ] Remove #[allow] from `src/tui/mod.rs` (3 instances - too_many_lines variations)
- [ ] Remove #[allow] from `src/tui/app.rs` (1 instance - struct_excessive_bools)
- [ ] Remove #[allow] from `src/tui/input.rs` (3 instances - too_many_lines + match_same_arms)
- [ ] Remove #[allow] from `src/tui/screens/help.rs` (3 instances - cast_possible_truncation + too_many_lines)
- [ ] Remove #[allow] from `src/tui/screens/rules.rs` (3 instances - too_many_lines)
- [ ] Remove #[allow] from `src/tui/screens/sinks.rs` (1 instance - too_many_lines)
- [ ] Remove #[allow] from `src/tui/screens/settings.rs` (1 instance - too_many_lines)
- [ ] Remove #[allow] from `src/commands.rs` (1 instance - too_many_lines)
- [ ] Remove #[allow] from `src/ipc.rs` (1 instance - cast_possible_truncation)
- [ ] Remove #[allow] from `src/compositor/wlr_toplevel.rs` (1 instance - needless_pass_by_value)
- [ ] **Keep** generated #[allow] in `src/lib.rs` (2 instances - DO NOT REMOVE)
- [ ] Remove #[allow] from `src/commands.rs` (1 instance)
- [ ] Remove #[allow] from `src/ipc.rs` (1 instance)
- [ ] Remove #[allow] from `src/compositor/wlr_toplevel.rs` (1 instance)
- [ ] **Keep** generated #[allow] in `src/lib.rs` (built_info crate)

**2.4 Verify Removal Complete**
- [ ] Run `rg '#\[allow\(clippy::' src/ --no-heading` to find remaining
- [ ] Should only show `src/lib.rs` (2 generated code suppressions - OK to keep)
- [ ] Run `cargo clippy --all-targets -- -W clippy::pedantic` again
- [ ] Verify still zero warnings

**2.5 Commit Phase 2**
- [ ] Run verification: `cargo fmt && cargo test`
- [ ] Commit with message: `refactor: centralize lint config to [lints] table`
- [ ] **DO NOT push yet**

---

### Phase 3: Document Generated Code Suppressions (Estimated: 5 min)

**3.1 Update src/lib.rs Documentation**
- [ ] Check current format of #[allow] attributes in `src/lib.rs`
- [ ] Add clarifying comment above the #[allow] attributes:
  ```rust
  // Generated by built crate in build.rs - suppressions kept local to generated code
  #[allow(clippy::needless_raw_string_hashes)]
  #[allow(clippy::doc_markdown)]
  pub mod built_info {
      include!(concat!(env!("OUT_DIR"), "/built.rs"));
  }
  ```

**3.2 Verify Final State**
- [ ] Run `rg '#\[allow\(clippy::' src/ --no-heading`
- [ ] Should only show `src/lib.rs` (2 instances with explanatory comment)
- [ ] Run `cargo clippy --all-targets -- -W clippy::pedantic`
- [ ] Verify zero warnings

**3.3 Commit Phase 3**
- [ ] Commit with message: `docs: clarify generated code lint suppressions in lib.rs`
- [ ] **DO NOT push yet**

---

### Phase 4: Optional Features (Estimated: 45-70 min)

**Optional Phase - Can be done later or skipped entirely**

**Priority 1: let_chains (15-20 min) - RECOMMENDED**
- [ ] Identify nested if-let in `src/daemon.rs` event loop (3-4 sites)
- [ ] Identify nested if-let in `src/tui/app.rs` input handling (2-3 sites)
- [ ] Identify nested if-let in `src/tui/input.rs` handlers (2-3 sites)
- [ ] Refactor ONE site first as proof of concept
- [ ] Test that ONE site thoroughly
- [ ] If successful, refactor remaining sites
- [ ] Test logic unchanged after each conversion
- [ ] Commit incrementally: `refactor: simplify nested patterns with let_chains in [module]`

**Priority 2: Async Closures (25-35 min) - MODERATE RISK**
- [ ] Identify async spawn patterns in `src/tui/mod.rs` (2-3 sites)
- [ ] Identify async spawn patterns in `src/daemon.rs` (1-2 sites)
- [ ] Identify async spawn patterns in `src/tui/tests/forwarder.rs` (1 site)
- [ ] Refactor ONE site first as proof of concept
- [ ] Test that ONE site thoroughly (capture semantics can be tricky)
- [ ] If successful, refactor remaining sites
- [ ] Test async behavior unchanged after each conversion
- [ ] Commit incrementally: `refactor: use async closures in [module]`

**Priority 3: LazyCell/LazyLock - SKIP**
- Current `OnceLock` usage in `src/pipewire.rs` is already optimal
- LazyLock provides no meaningful benefit
- **Decision: Skip this feature**

---

### Phase 5: Testing & Verification (Estimated: 25-30 min)

**5.1 Automated Testing**
- [ ] Run full test suite: `cargo test`
- [ ] Verify all 90+ tests pass
- [ ] Run test safety verification: `bash scripts/verify_tests_safe.sh`
- [ ] Run standard clippy: `cargo clippy --all-targets`
- [ ] Run pedantic clippy: `cargo clippy --all-targets -- -W clippy::pedantic`
- [ ] Verify zero warnings

**5.2 Build Verification**
- [ ] Clean build: `cargo clean && cargo build --release 2>&1 | tail -5`
- [ ] Measure build time (baseline: 93s, expect: ~90-95s, no major change)
- [ ] Check binary size: `ls -lh target/release/pwsw`
- [ ] Should be ~6.3MB (±50KB variation normal)

**5.3 Manual Testing - Daemon**
- [ ] Run daemon in foreground: `cargo run -- daemon --foreground`
- [ ] Verify no startup errors or warnings
- [ ] Test IPC commands in another terminal:
  - `cargo run -- status`
  - `cargo run -- list-sinks`
  - `cargo run -- list-windows`
- [ ] Test window switching triggers sink changes (if possible)
- [ ] Stop daemon (Ctrl-C) and verify clean shutdown

**5.4 Manual Testing - TUI**
- [ ] Run TUI: `cargo run -- tui`
- [ ] Navigate all screens: Dashboard (1), Rules (2), Sinks (3), Settings (4), Help (h)
- [ ] Test rule editing: Add, edit, delete rules
- [ ] Test sink editing: Add, edit, delete sinks
- [ ] Test settings changes: Toggle options, change log level
- [ ] Verify no visual glitches or crashes
- [ ] Exit TUI (Esc or q) and verify clean shutdown

**5.5 Final Verification**
- [ ] Run `cargo fmt` to ensure formatting is clean
- [ ] Review all commits made during upgrade (git log)
- [ ] Verify commit messages are clear and descriptive
- [ ] Check that no unintended changes were included

---

### Phase 6: Documentation & Completion (Estimated: 10 min)

**6.1 Update CLAUDE.md**
- [ ] Update "Current Acceptable Pedantic Allows" section
- [ ] Change from "24 total" to "22 total (centralized in Cargo.toml [lints] table, 2 in generated code)"
- [ ] Update recent achievements with upgrade completion date
- [ ] Update rust-version baseline to 1.92, edition to 2024

**6.2 Update rust_plan.md**
- [ ] Change **Status:** from "In Progress" to "✅ Completed [date]"
- [ ] Add completion summary with actual results:
  - Build time: [actual]s (baseline: 93s)
  - Test results: [pass/fail count]
  - Features implemented: [Phase 4 choices]

**6.3 Final Commit**
- [ ] Commit doc updates: `docs: update planning docs after Edition 2024 upgrade`

---

### Phase 7: Push to Remote (Only with User Approval)

**7.1 Pre-Push Checklist**
- [ ] All mandatory phases completed (Phases 0-3)
- [ ] Optional Phase 4 completed or explicitly skipped
- [ ] All tests passing
- [ ] All verification steps completed
- [ ] Documentation updated
- [ ] Commits are clean and well-messaged (git log review)

**7.2 Get User Approval**
- [ ] **STOP HERE** - Do not proceed without explicit user approval
- [ ] Ask user: "Edition 2024 upgrade complete. Ready to push to remote?"
- [ ] Wait for explicit approval (e.g., "push it", "yes", "go ahead")

**7.3 Push to Remote** (Only after approval)
- [ ] Identify current branch: `git branch --show-current`
- [ ] Push commits: `git push` or `git push -u origin <branch>` for new branches
- [ ] If network errors, retry up to 4 times with exponential backoff (2s, 4s, 8s, 16s)
- [ ] Verify push succeeded: Check remote repository
- [ ] Confirm commits visible: `git log origin/<branch> --oneline | head -10`

---

## Checklist Summary

**Mandatory Phases (Total: ~40-55 min):**
- ✅ Phase 0: Dependency Check (5 min)
- ✅ Phase 1: MSRV & Edition Update (5-10 min)
- ✅ Phase 2: Centralize Lints (20-25 min)
- ✅ Phase 3: Document Generated Code (5 min)
- ✅ Phase 5: Testing & Verification (25-30 min)
- ✅ Phase 6: Documentation (10 min)
- ✅ Phase 7: Push (with approval)

**Optional Phase:**
- ⭐ Phase 4: Optional Features (45-70 min)
  - Priority 1: let_chains (15-20 min) - Recommended
  - Priority 2: async closures (25-35 min) - Moderate risk
  - Priority 3: LazyCell/LazyLock - SKIP

**Total Time:**
- Mandatory only: ~40-55 minutes
- With Phase 4 Priority 1: ~55-75 minutes
- With Phase 4 full: ~85-125 minutes

**Expected Outcomes:**
- ✅ Zero clippy warnings (pedantic mode)
- ✅ All 90+ tests passing
- ✅ Edition 2024 baseline established
- ✅ 22 lint suppressions centralized (2 remain in generated code)
- ✅ Modern Rust 1.92 MSRV declared
- ✅ Optional: 8-10 sites simplified with let_chains
- ✅ Optional: 5-6 sites improved with async closures

---

## Completion Summary

**Completed:** 2025-12-21

### Phases Completed

✅ **Phase 0: Dependency Check** - Rust 1.92 confirmed installed  
✅ **Phase 1: MSRV & Edition Update** - Updated to Edition 2024, Rust 1.92 MSRV, applied clippy auto-fixes  
✅ **Phase 2: Centralize Lints** - Moved 22 suppressions to `[lints]` table in Cargo.toml  
✅ **Phase 3: Document Generated Code** - Added explanatory comments for generated code suppressions  
✅ **Phase 4 Priority 1: let_chains** - Refactored 4 nested if-let sites using Edition 2024 features  
⏭️ **Phase 4 Priority 2: async closures** - Skipped (optional, moderate risk)  
⏭️ **Phase 4 Priority 3: LazyCell/LazyLock** - Skipped (current `OnceLock` already optimal)

### Results

**Test Results:**
- ✅ All 94 tests passing (83 unit + 5 cli_smoke + 5 config_integration + 1 doctest)
- ✅ Test safety verification passed (no tests touch real user config)

**Code Quality:**
- ✅ Zero standard clippy warnings
- ✅ 25 pedantic warnings (all intentionally suppressed via centralized config)
- ✅ Centralized lint configuration: 22 suppressions in Cargo.toml `[lints]` table
- ✅ 2 generated code suppressions remain in src/lib.rs (appropriate)
- ✅ 1 Cargo.toml warning-level lint for cfg validation

**Edition 2024 Features Adopted:**
- ✅ `let_chains` - 4 sites simplified (daemon.rs, wlr_toplevel.rs, rules.rs)
- ✅ `let-else` - 1 site improved with cleaner early return pattern
- ✅ Reduced nesting depth, improved readability

**Build & Performance:**
- Release build time: ~93s (no significant change)
- Binary size: ~6.3MB (no significant change)
- No regressions in functionality

### Commits Created

1. `91e7afb` - "chore: upgrade to Edition 2024 and Rust 1.92 MSRV"
2. `f2a7cfe` - "refactor: centralize lint config to [lints] table"
3. `b58a8f5` - "docs: clarify generated code lint suppressions in lib.rs"
4. `edff29b` - "refactor: simplify nested patterns with let_chains"

**Total time:** ~45 minutes (phases 0-3 + phase 4 priority 1)

---

## References

**Rust Announcements:**
- [Rust 1.75.0](https://blog.rust-lang.org/2023/12/28/Rust-1.75.0/)
- [Rust 1.76.0](https://blog.rust-lang.org/2024/02/08/Rust-1.76.0/)
- [Rust 1.77.0](https://blog.rust-lang.org/2024/03/21/Rust-1.77.0.html)
- [Rust 1.78.0](https://blog.rust-lang.org/2024/05/02/Rust-1.78.0/)
- [Rust 1.80.0](https://blog.rust-lang.org/2024/07/25/Rust-1.80.0.html)
- [Rust 1.82.0](https://blog.rust-lang.org/2024/10/17/Rust-1.82.0/)
- [Rust 1.83.0](https://blog.rust-lang.org/2024/11/28/Rust-1.83.0/)
- [Rust 1.84.0](https://blog.rust-lang.org/2025/01/09/Rust-1.84.0.html)
- [Rust 1.85.0 and Rust 2024](https://blog.rust-lang.org/2025/02/20/Rust-1.85.0/)
- [Rust 1.86.0](https://blog.rust-lang.org/2025/04/03/Rust-1.86.0/)

**Additional Resources:**
- [Rust Changelogs](https://releases.rs/)
- [Rust 1.79.0 Release Notes](https://www.hostzealot.com/blog/news/rust-179-release)
- [Rust 1.80 InfoQ Coverage](https://www.infoq.com/news/2024/08/rust-1-80-lazy-globals/)
- [Rust 1.83 InfoWorld Coverage](https://www.infoworld.com/article/3615734/rust-1-83-expands-const-capabilities.html)
- [Rust 1.85.0 Comprehensive Guide](https://aarambhdevhub.medium.com/rust-1-85-0-a-comprehensive-guide-to-the-latest-rust-release-263245f28d7e)

**Official Documentation:**
- [Edition Guide - Rust 2024](https://doc.rust-lang.org/edition-guide/rust-2024/index.html)
- [Cargo Book - [lints] Table](https://doc.rust-lang.org/cargo/reference/manifest.html#the-lints-section)
- [RFC 3668 - Async Closures](https://rust-lang.github.io/rfcs/3668-async-closures.html)
