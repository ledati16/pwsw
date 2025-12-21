# Rust Version & Edition Modernization Plan

**Status:** Planning
**Target:** Rust 1.92 (latest stable) + Edition 2024
**Current:** Rust 1.74 + Edition 2021
**Date:** 2025-12-20

## Executive Summary

Upgrading from Rust 1.74 (December 2023) to Rust 1.92 (current stable) provides significant benefits for code quality, maintainability, and developer experience. Since PWSW is an **application** (not a library), we have maximum flexibility with MSRV - users either compile from source with their Rust toolchain or use pre-built binaries.

**Key Benefits:**
- Async closures for cleaner TUI event loops and IPC handlers
- `let_chains` to simplify nested pattern matching in daemon and TUI
- `#[expect]` lint for self-documenting technical debt (19 instances)
- `[lints]` table in Cargo.toml for centralized lint management
- LazyCell/LazyLock for cleaner static initialization
- Future-proofing for ratatui 0.30 (requires MSRV 1.86)

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

### Rust 1.87-1.92 (May-December 2025)
**Key Features:**
- ✅ Incremental improvements to standard library
- ✅ Performance optimizations
- ✅ Tooling improvements

**Relevance:** General ecosystem maturity, latest stable baseline.

**Sources:** [Rust Releases](https://releases.rs/), [Rust Blog](https://blog.rust-lang.org/releases/)

---

## Cargo.toml [lints] Table (Post-1.74 Feature)

One of the most impactful improvements is the `[lints]` table, which allows centralizing lint configuration instead of scattering `#[allow]` attributes across files.

**Current State (19 scattered allows across 8 files):**
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
# Justified suppressions with inline rationale
struct_excessive_bools = "allow"  # Config structs have independent flags, not state machines
too_many_lines = "allow"          # Event loops and screens are cohesive units
cast_possible_truncation = "allow" # IPC message length (u32), TUI scroll indicators (safe)
# ... etc
```

**Benefits:**
- ✅ Single source of truth for project lint policy
- ✅ Easier to audit and update
- ✅ Better visibility into technical debt
- ✅ Can add inline comments explaining rationale

**Sources:** Cargo Book, [lints] table documentation (introduced post-1.74)

---

## Current Codebase Impact Analysis

### Files with #[allow] Attributes (19 total)

1. **src/config.rs:**
   - `struct_excessive_bools` (3×) - Settings, SettingsFile, Config

2. **src/daemon.rs:**
   - `too_many_lines` (2×) - run_event_loop, main daemon logic

3. **src/tui/mod.rs:**
   - `too_many_lines` (2×) - TUI event loop, rendering

4. **src/tui/app.rs:**
   - `struct_excessive_bools` (1×) - App state
   - `too_many_lines` (1×) - Input handling

5. **src/tui/screens/*.rs:**
   - `too_many_lines` (9×) - Dashboard, rules, sinks, settings screens
   - `match_same_arms` (1×) - Rules editor input

6. **src/ipc.rs:**
   - `cast_possible_truncation` (1×) - IPC message length (u32)

7. **src/tui/screens/help.rs:**
   - `cast_possible_truncation` (2×) - TUI scroll arrow text length
   - `too_many_lines` (1×) - build_help_rows

8. **src/compositor/wlr_toplevel.rs:**
   - `needless_pass_by_value` (1×) - Wayland Connection must be moved
   - `items_after_statements` (2×) - Constants scoped in spawn blocks

9. **build.rs (generated):**
   - `needless_raw_string_hashes` (1×) - built_info crate
   - `doc_markdown` (1×) - built_info crate

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
