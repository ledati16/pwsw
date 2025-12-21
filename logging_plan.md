# Logging & Error Handling Modernization Plan

**Status:** In Progress
**Estimated Time:** 20-30 minutes
**Date:** 2025-12-21

## Executive Summary

Migrate from `anyhow` to `color-eyre` (eyre) for error handling and enable `tracing` integration with `tui-logger` to fix currently broken TUI logging. This modernizes error handling, improves developer experience, and ensures TUI logs actually display traced events.

## Current State Analysis

### Dependencies (Cargo.toml)

**Error Handling:**
- ✅ `anyhow = "1"` - Used everywhere (14 files)
- ✅ `color-eyre = "0.6.5"` - Only used for panic handler installation
- ⚠️ **Problem:** Redundant - both serve same purpose

**Logging:**
- ✅ `tracing = "0.1"` - Used throughout codebase (9 files)
- ✅ `tracing-subscriber = "0.3"` - Used for initialization
- ✅ `tui-logger = "0.17.4"` - TUI log widget
- ✅ `log = "0.4.29"` - Legacy facade (not used directly)
- ⚠️ **Problem:** tui-logger is configured for `log` facade, but code uses `tracing` macros
- 🚨 **Critical Bug:** TUI logs don't display because tracing events aren't bridged to tui-logger

### Current Usage Patterns

**Files using anyhow (14 total):**
1. `src/bin/pwsw.rs` - `use anyhow::Result;`
2. `src/ipc.rs` - `use anyhow::{Context, Result};` + 3× `anyhow::bail!()`
3. `src/tui/daemon_control.rs` - `use anyhow::{Context, Result};` + 2× `anyhow::bail!()`
4. `src/daemon.rs` - `use anyhow::{Context, Result};`
5. `src/tui/app.rs` - `use anyhow::Result;`
6. `src/config.rs` - `use anyhow::{Context, Result};` + 1× `anyhow::Error::new()`
7. `src/state.rs` - `use anyhow::Result;`
8. `src/pipewire.rs` - `use anyhow::{Context, Result};` + 18× `anyhow::bail!()`
9. `src/notification.rs` - `use anyhow::{Context, Result};`
10. `src/compositor/wlr_toplevel.rs` - `use anyhow::{Context, Result};`
11. `src/tui/mod.rs` - `use anyhow::{Context, Result};`
12. `src/compositor/mod.rs` - `use anyhow::{Context, Result};`
13. `src/tui/log_tailer.rs` - `use anyhow::{Context, Result};`
14. `src/commands.rs` - `use anyhow::Result;`

**Macro usage:**
- 39× `anyhow::bail!()` calls across 3 files
- 1× `anyhow::Error::new()` in config.rs

---

## Why color-eyre is More Modern

**1. Better Error Display** - Colored, structured output with source locations
**2. Enhanced Context Methods** - `.note()`, `.suggestion()`, `.warning()`
**3. Better Panic Handler** - Backtraces with source code snippets
**4. Source Code Display** - Shows actual source lines where errors occurred
**5. Active Development** - Community standard for modern Rust applications

---

## Migration Plan

### Phase 1: Update Cargo.toml (2 min)

Remove `anyhow`, update `tui-logger` with tracing support, remove `log`.

### Phase 2: Update Import Statements (10 min)

Replace `use anyhow::` with `use color_eyre::eyre::` in 14 files.

### Phase 3: Update bail! Macros (5 min)

Find-replace `anyhow::bail!` → `eyre::bail!` (39 occurrences).

### Phase 4: Update Error Type (1 min)

Change `anyhow::Error::new` → `eyre::Error::new` in config.rs.

### Phase 5: Fix TUI Logging (5 min)

Replace `tui_logger::init_logger()` with `TuiTracingSubscriberLayer`.

### Phase 6: Testing & Verification (7 min)

Build, test, clippy, manual TUI/CLI testing.

### Phase 7: Commit (2 min)

Commit with descriptive message.

---

## Implementation Checklist

See logging_plan.md for detailed step-by-step instructions with checkboxes.

Total estimated time: **32 minutes**
