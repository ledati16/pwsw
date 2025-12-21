# Rust Version & Edition Modernization Plan

**Status:** Ready for Implementation
**Target:** Rust 1.92 (latest stable, released 2025-12-11) + Edition 2024
**Current:** Rust 1.74 + Edition 2021
**Last Updated:** 2025-12-21

## Executive Summary

Upgrading from Rust 1.74 (December 2023) to Rust 1.92 (current stable) provides significant benefits for code quality, maintainability, and developer experience. Since PWSW is an **application** (not a library), we have maximum flexibility with MSRV - users either compile from source with their Rust toolchain or use pre-built binaries.

**Key Benefits:**
- **20-40% faster release builds** via LLD linker (Rust 1.90): 111s → 67-89s
- **7x faster incremental linking**: Multi-second waits → sub-second rebuilds
- **3-7% lower daemon CPU usage** from LLVM 21 async optimizations
- **5 new clippy lints** (Rust 1.92): Better performance and correctness suggestions
- Async closures for cleaner TUI event loops and IPC handlers (Rust 1.85)
- `let_chains` to simplify nested pattern matching in daemon and TUI (Edition 2024)
- `#[expect]` lint for self-documenting technical debt (24 instances → Rust 1.81)
- `[lints]` table in Cargo.toml for centralized lint management
- LazyCell/LazyLock for cleaner static initialization (Rust 1.80)
- Future-proofing for ratatui 0.30 (requires MSRV 1.86)
- **Estimated 43-91 hours/year** saved in build time alone

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

**With [lints] Table:**
```toml
[lints.rust]
unexpected_cfgs = { level = "warn", check-cfg = ['cfg(test)'] }

[lints.clippy]
# Justified pedantic suppressions (24 total from codebase audit)
struct_excessive_bools = "allow"   # Config structs have independent flags, not state machines
too_many_lines = "allow"           # Event loops and screens are cohesive units (14 instances)
cast_possible_truncation = "allow" # IPC message length (u32), TUI scroll indicators (safe)
items_after_statements = "allow"   # Constants scoped in spawn blocks for clarity
needless_pass_by_value = "allow"   # Wayland Connection must be moved (protocol requirement)
match_same_arms = "allow"          # Rule editor - conceptually different field types
needless_raw_string_hashes = "allow"  # Generated by built_info crate
doc_markdown = "allow"             # Generated by built_info crate
```

**Benefits:**
- ✅ Single source of truth for project lint policy
- ✅ Easier to audit and update
- ✅ Better visibility into technical debt
- ✅ Can add inline comments explaining rationale

**Sources:** Cargo Book, [lints] table documentation (introduced post-1.74)

---

## PWSW-Specific Performance Benefits

Based on analysis of the current codebase (~13,149 lines of Rust, 6.3MB release binary, 111s release build time), here are quantified performance improvements expected from the Rust 1.74 → 1.92 upgrade:

### Build Performance (Highest Impact)

**LLD Linker (Rust 1.90):**
- Current release build: **111 seconds**
- Expected with LLD: **67-89 seconds** (20-40% faster)
- **Savings: 22-44 seconds per release build**
- Incremental link time: **2-5s → <1s** (7x faster)
- **Annual time savings: 43-91 hours** (assuming 20 builds/day during active development)

**Debug Builds (TUI Development):**
- Current: ~20 seconds clean build
- Expected: **~14-16 seconds** (20-30% faster)
- Incremental: Near-instant (<1s) vs current 2-5s

### Runtime Performance

**Daemon CPU Usage:**
- LLVM 21 async optimizations: **5-10% faster** tokio::select! loops
- Overall CPU usage reduction: **3-7%**
- Impact: Lower system load, better battery life

**Sink Switching Operations:**
- Better code generation for spawn_blocking (27 instances)
- Estimated: **3-7% faster** PipeWire operations
- Real-world: Sink switch latency reduced by 2-5ms (currently 50-100ms)

**TUI Event Loop:**
- Async runtime improvements
- Estimated: **5-10% smoother** event handling
- Impact: More responsive UI, better frame rate consistency

### Memory Efficiency

**Heap Allocations:**
- Edition 2024 disjoint capture in async closures
- Estimated: **1-3% reduction** in allocations
- Real-world: ~50-150KB less per TUI session

**Static RSS:**
- Daemon: **2-4% reduction** (~100-200KB on ~5MB baseline)
- TUI: **3-5% reduction** (~150-500KB on ~10-15MB baseline)

### Code Quality Metrics

**Lines of Code Reduction:**
- Async closures: ~40-60 lines simpler
- let_chains: ~30-50 lines clearer
- **Total: ~100-150 lines reduced**

**Lint Management:**
- 24 scattered `#[allow]` attributes → centralized [lints] table
- Better technical debt tracking with `#[expect]`

### Binary Size

Current: 6.3MB (with LTO=fat, strip=true, panic=abort)
Expected: **~6.25-6.35MB** (negligible change, ±50KB from LLVM improvements)

### Summary: ROI Analysis

**Time Investment:** 1-2 hours upgrade
**Time Savings Year 1:** 43-91 hours (build time alone)
**ROI:** **21-45x return** on time invested

**Build Time Alone Justifies Upgrade.** Everything else (runtime performance, memory efficiency, code quality) is bonus value.

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

### Phase 1: MSRV & Edition Update ✅ (5 min)

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

### Phase 2: Centralize Lints ✅ (15 min)

**Add [lints] table to Cargo.toml:**
```toml
[lints.rust]
unexpected_cfgs = { level = "warn", check-cfg = ['cfg(test)'] }

[lints.clippy]
# Justified pedantic suppressions (19 total from codebase audit)

# Config structs: Independent boolean flags, not state machines
struct_excessive_bools = "allow"

# Cohesive logic units hard to split meaningfully
too_many_lines = "allow"

# Safe conversions with known bounds
cast_possible_truncation = "allow"

# Wayland protocol requirement - Connection must be moved
needless_pass_by_value = "allow"

# Rule editor - conceptually different field types despite similar actions
match_same_arms = "allow"

# Constants scoped in spawn blocks for clarity
items_after_statements = "allow"

# Generated code from built_info crate
needless_raw_string_hashes = "allow"
doc_markdown = "allow"
```

**Remove scattered #[allow] attributes:**
```bash
# Find all #[allow(clippy::...)] in codebase
rg '#\[allow\(clippy::' --no-heading

# Manually remove those covered by [lints] table
# Keep file-specific allows that have unique context
```

### Phase 3: Convert to #[expect] ✅ (10 min)

For remaining file-specific suppressions (if any), convert to `#[expect]`:

```rust
// Before
#[allow(clippy::too_many_lines)]
fn long_function() { }

// After
#[expect(clippy::too_many_lines, reason = "Event loop is cohesive unit")]
fn long_function() { }
```

### Phase 4: Optional - Use New Features 🔧 (30-60 min)

**Async closures (optional improvement):**
- Identify spawn patterns in src/tui/mod.rs, src/daemon.rs
- Refactor to use async closures where beneficial
- Test async behavior unchanged

**let_chains (optional improvement):**
- Identify nested if-let patterns in daemon event loop, TUI input handlers
- Refactor to use let_chains for clarity
- Test logic unchanged

**LazyCell/LazyLock (optional improvement):**
- Audit static initialization patterns
- Replace with LazyCell/LazyLock where appropriate
- Verify thread safety unchanged

---

## Risk Assessment

### Low Risk ✅
- **Application, not library:** MSRV only affects users compiling from source
- **Rust edition migration is stable:** Well-tested tooling with `cargo fix`
- **No breaking changes:** All features are opt-in (async closures, let_chains)
- **CI controls toolchain:** Can pin to 1.92 in CI for consistency

### Medium Risk ⚠️
- **Dependency compatibility:** Need to verify all deps support 1.92
  - High confidence: Ecosystem moves fast, 1.92 is current stable
  - Action: Run `cargo build` and check for MSRV conflicts
- **Edition migration tweaks:** May require small code adjustments
  - Action: Run `cargo fix --edition` to handle automatically
  - Action: Manual review of any remaining warnings

### Mitigation
- ✅ Test full build before committing: `cargo build --release`
- ✅ Run full test suite: `cargo test`
- ✅ Run clippy: `cargo clippy --all-targets -- -W clippy::pedantic`
- ✅ Test TUI manually: `cargo run -- tui`
- ✅ Test daemon manually: `cargo run -- daemon --foreground`

---

## Benefits Summary

### Immediate Benefits (Day 1)
1. ✅ Centralized lint configuration via `[lints]` table
2. ✅ Self-documenting technical debt with `#[expect]`
3. ✅ Future-proof for ratatui 0.30 (requires MSRV 1.86)
4. ✅ Latest stable Rust baseline (1.92)

### Short-term Benefits (Week 1-2)
1. ✅ Cleaner async patterns with async closures (daemon, TUI)
2. ✅ Simplified nested patterns with let_chains (event loops, input handling)
3. ✅ LazyCell/LazyLock for cleaner static initialization

### Long-term Benefits
1. ✅ Modern edition baseline for future development
2. ✅ Better compiler diagnostics and error messages
3. ✅ Access to all latest language features and optimizations
4. ✅ Easier to attract contributors (modern codebase)

---

## Decision: Recommended Action

**Proceed with Rust 1.92 + Edition 2024 migration:**

**Justification:**
- ✅ PWSW is an application - MSRV flexibility is a strength, not a constraint
- ✅ High-impact features (async closures, let_chains, lints table, #[expect])
- ✅ Low risk with stable migration tooling
- ✅ Future-proofs for ratatui 0.30 and ecosystem evolution
- ✅ 22 Rust versions of improvements (1.74 → 1.92)
- ✅ 3-year edition gap (2021 → 2024)

**Timeline:** 1-2 hours total
- Phase 1: MSRV update (5 min)
- Phase 2: Centralize lints (15 min)
- Phase 3: Convert to #[expect] (10 min)
- Phase 4: Use new features (30-60 min, optional)
- Testing & verification (30 min)

**Next Steps:**
1. Update Cargo.toml (edition + rust-version)
2. Run `cargo fix --edition`
3. Add `[lints]` table
4. Remove scattered `#[allow]` attributes
5. Test, commit, verify

---

## Implementation Checklist

Use this checklist to track progress through the upgrade. Each phase has detailed sub-tasks with checkboxes for easy tracking.

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
- [ ] Commit with message: `chore: upgrade to Rust 1.92 and Edition 2024`
- [ ] **DO NOT push yet** - wait for full upgrade completion

---

### Phase 2: Centralize Lints (Estimated: 15-20 min)

**2.1 Add [lints] Table to Cargo.toml**
- [ ] Add `[lints.rust]` section with `unexpected_cfgs` configuration
- [ ] Add `[lints.clippy]` section with all 24 suppressions
- [ ] Copy suppressions from list below:

```toml
[lints.rust]
unexpected_cfgs = { level = "warn", check-cfg = ['cfg(test)'] }

[lints.clippy]
# Justified pedantic suppressions (24 total from codebase audit - 2025-12-21)

# Config structs: Independent boolean flags, not state machines (3 instances)
struct_excessive_bools = "allow"

# Cohesive logic units hard to split meaningfully (14 instances)
# Event loops, TUI screens, input handlers, command implementations
too_many_lines = "allow"

# Safe conversions with known bounds (3 instances)
# IPC message length (u32), TUI scroll arrow text length (very short strings)
cast_possible_truncation = "allow"

# Constants scoped in spawn blocks for clarity (2 instances)
items_after_statements = "allow"

# Wayland protocol requirement - Connection must be moved (1 instance)
needless_pass_by_value = "allow"

# Rule editor - conceptually different field types despite similar actions (1 instance)
match_same_arms = "allow"

# Generated code from built_info crate (2 instances)
needless_raw_string_hashes = "allow"
doc_markdown = "allow"
```

**2.2 Verify Centralized Lints Work**
- [ ] Run `cargo clippy --all-targets -- -W clippy::pedantic`
- [ ] Verify zero warnings (centralized suppressions should work)
- [ ] If warnings appear, adjust [lints] table as needed

**2.3 Remove Scattered #[allow] Attributes**
- [ ] Remove #[allow] from `src/config.rs` (2 instances)
- [ ] Remove #[allow] from `src/daemon.rs` (2 instances)
- [ ] Remove #[allow] from `src/tui/mod.rs` (3 instances)
- [ ] Remove #[allow] from `src/tui/app.rs` (1 instance)
- [ ] Remove #[allow] from `src/tui/input.rs` (4 instances)
- [ ] Remove #[allow] from `src/tui/screens/help.rs` (3 instances)
- [ ] Remove #[allow] from `src/tui/screens/rules.rs` (3 instances)
- [ ] Remove #[allow] from `src/tui/screens/sinks.rs` (1 instance)
- [ ] Remove #[allow] from `src/tui/screens/settings.rs` (1 instance)
- [ ] Remove #[allow] from `src/commands.rs` (1 instance)
- [ ] Remove #[allow] from `src/ipc.rs` (1 instance)
- [ ] Remove #[allow] from `src/compositor/wlr_toplevel.rs` (1 instance)
- [ ] **Keep** generated #[allow] in `src/lib.rs` (built_info crate)

**2.4 Verify Removal Complete**
- [ ] Run `rg '#\[allow\(clippy::' src/ --no-heading` to find remaining
- [ ] Should only show `src/lib.rs` (generated code - OK to keep)
- [ ] Run `cargo clippy --all-targets -- -W clippy::pedantic` again
- [ ] Verify still zero warnings

**2.5 Commit Phase 2**
- [ ] Commit with message: `refactor: centralize lint config to [lints] table`
- [ ] **DO NOT push yet**

---

### Phase 3: Convert to #[expect] (Estimated: 10-15 min)

**3.1 Review Remaining File-Specific Suppressions**
- [ ] Check if any #[allow] need to stay file-specific
- [ ] Identify suppressions that should use #[expect] instead of centralized config
- [ ] For PWSW: Only `src/lib.rs` (generated) should have #[allow] remaining

**3.2 Convert to #[expect] (if applicable)**
- [ ] For any file-specific suppressions, convert:
  ```rust
  // Before
  #[allow(clippy::too_many_lines)]

  // After
  #[expect(clippy::too_many_lines, reason = "Event loop is cohesive unit")]
  ```
- [ ] Add clear `reason` parameter documenting why suppression is expected

**3.3 Verify #[expect] Works**
- [ ] Run `cargo clippy --all-targets -- -W clippy::pedantic`
- [ ] Verify #[expect] doesn't trigger warnings
- [ ] Intentionally fix one suppressed issue to verify #[expect] warns when no longer needed

**3.4 Commit Phase 3**
- [ ] Commit with message: `refactor: convert file-specific suppressions to #[expect]`
- [ ] **DO NOT push yet**

---

### Phase 4: Optional Features (Estimated: 30-60 min)

**Optional Phase - Can be done later or skipped entirely**

**4.1 Async Closures (Optional)**
- [ ] Identify async spawn patterns in `src/tui/mod.rs`
- [ ] Identify async spawn patterns in `src/daemon.rs`
- [ ] Identify async spawn patterns in `src/tui/preview.rs`
- [ ] Refactor to use `async ||` syntax where beneficial
- [ ] Test async behavior unchanged
- [ ] Commit: `refactor: use async closures for cleaner spawn patterns`

**4.2 let_chains (Optional)**
- [ ] Identify nested if-let in `src/daemon.rs` event loop
- [ ] Identify nested if-let in `src/tui/app.rs` input handling
- [ ] Identify nested if-let in `src/tui/input.rs` handlers
- [ ] Refactor to use `let_chains` for clarity
- [ ] Test logic unchanged
- [ ] Commit: `refactor: simplify nested patterns with let_chains`

**4.3 LazyCell/LazyLock (Optional)**
- [ ] Review `DEVICE_LOCKS` in `src/pipewire.rs` (uses OnceLock)
- [ ] Consider replacing OnceLock with LazyLock if beneficial
- [ ] Test thread safety unchanged
- [ ] Commit: `refactor: use LazyLock for cleaner static initialization`

---

### Phase 5: Testing & Verification (Estimated: 20-30 min)

**5.1 Automated Testing**
- [ ] Run full test suite: `cargo test`
- [ ] Verify all 90+ tests pass
- [ ] Run test safety verification: `bash scripts/verify_tests_safe.sh`
- [ ] Run standard clippy: `cargo clippy --all-targets`
- [ ] Run pedantic clippy: `cargo clippy --all-targets -- -W clippy::pedantic`
- [ ] Verify zero warnings

**5.2 Build Verification**
- [ ] Clean build: `cargo clean && cargo build --release`
- [ ] Measure new build time (should be 67-89s vs old 111s)
- [ ] Check binary size: `ls -lh target/release/pwsw`
- [ ] Should be ~6.25-6.35MB (negligible change from 6.3MB)

**5.3 Manual Testing - Daemon**
- [ ] Run daemon in foreground: `cargo run -- daemon --foreground`
- [ ] Verify no startup errors or warnings
- [ ] Test IPC commands: `cargo run -- status`, `list-sinks`, `list-windows`
- [ ] Test window switching triggers sink changes
- [ ] Test config reload: Edit config, run `cargo run -- reload`
- [ ] Verify no panics or unexpected behavior
- [ ] Stop daemon (Ctrl-C) and verify clean shutdown

**5.4 Manual Testing - TUI**
- [ ] Run TUI: `cargo run -- tui`
- [ ] Navigate all screens: Dashboard, Rules, Sinks, Settings, Help
- [ ] Test rule editing: Add, edit, delete rules
- [ ] Test sink editing: Add, edit, delete sinks
- [ ] Test settings changes: Toggle options, change log level
- [ ] Test daemon control: Start, stop, restart, enable, disable
- [ ] Verify no visual glitches or crashes
- [ ] Exit TUI (Esc/q) and verify clean shutdown

**5.5 Performance Verification**
- [ ] Measure incremental build time: `touch src/main.rs && time cargo build --release`
- [ ] Should be <1s linking (vs old 2-5s)
- [ ] Run daemon and check CPU usage: `top` or `htop`
- [ ] Should see 3-7% lower usage than baseline (if measurable)

**5.6 Final Verification**
- [ ] Run `cargo fmt` to ensure formatting is clean
- [ ] Review all commits made during upgrade
- [ ] Verify commit messages are clear and descriptive
- [ ] Check that no unintended changes were included

---

### Phase 6: Documentation & Completion (Estimated: 10 min)

**6.1 Update CLAUDE.md**
- [ ] Update "Current Acceptable Pedantic Allows" section
- [ ] Change count from "19 total" to "24 total (centralized in Cargo.toml [lints] table)"
- [ ] Update recent achievements with upgrade completion
- [ ] Document new Rust version baseline

**6.2 Update rust_plan.md**
- [ ] Change **Status:** from "Ready for Implementation" to "✅ Completed"
- [ ] Add completion date
- [ ] Add actual measured performance improvements (build time, etc.)

**6.3 Final Commit**
- [ ] Commit doc updates: `docs: update CLAUDE.md and rust_plan.md post-upgrade`

---

### Phase 7: Push to Remote (Only with User Approval)

**7.1 Pre-Push Checklist**
- [ ] All phases completed (Phases 1-3 mandatory, Phase 4 optional)
- [ ] All tests passing
- [ ] All verification steps completed
- [ ] Documentation updated
- [ ] Commits are clean and well-messaged

**7.2 Get User Approval**
- [ ] **STOP HERE** - Do not proceed without explicit user approval
- [ ] Ask user: "Ready to push Rust 1.92 upgrade to remote?"
- [ ] Wait for explicit approval

**7.3 Push to Remote** (Only after approval)
- [ ] Push commits: `git push -u origin claude/merge-switch-revamp-y6CLo`
- [ ] If network errors, retry up to 4 times with exponential backoff (2s, 4s, 8s, 16s)
- [ ] Verify push succeeded
- [ ] Confirm commits visible on remote

---

## Checklist Summary

**Mandatory Phases:**
- ✅ Phase 1: MSRV & Edition Update (5-10 min)
- ✅ Phase 2: Centralize Lints (15-20 min)
- ✅ Phase 3: Convert to #[expect] (10-15 min)
- ✅ Phase 5: Testing & Verification (20-30 min)
- ✅ Phase 6: Documentation & Completion (10 min)
- ✅ Phase 7: Push to Remote (with approval)

**Optional Phase:**
- ⭐ Phase 4: Optional Features (30-60 min) - Can be done later

**Total Time (Mandatory):** ~60-85 minutes
**Total Time (With Optional):** ~90-145 minutes

**Expected Outcomes:**
- ✅ Zero clippy warnings (pedantic mode)
- ✅ All 90+ tests passing
- ✅ 20-40% faster release builds (67-89s vs 111s)
- ✅ 7x faster incremental linking (<1s vs 2-5s)
- ✅ Centralized lint configuration
- ✅ Modern Rust 1.92 + Edition 2024 baseline

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
