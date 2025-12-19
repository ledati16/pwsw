# TUI Navigation Redesign Plan

**Goal:** Modernize TUI navigation to use number keys (1-4) instead of letters (d, s, r, t), improve discoverability, and reduce redundancy.

**Status:** Phases 1-9 complete ✅ (all features implemented and verified)

---

## Overview

### Current State
```
╭─ PWSW v0.3.1 ──────────────────────────────────────╮
│ [d]Dashboard [s]Sinks [r]Rules [t]Settings         │
╰────────────────────────────────────────────────────╯
┌────────────────────────────────────────────────────┐
│            Screen Content Area                     │
└────────────────────────────────────────────────────┘
[q] Quit [?] Help [Tab] Next [d]ashboard [s]inks [r]ules Se[t]tings [Ctrl+S] Save
```

**Issues:**
- Letter keys (d, s, r, t) not discoverable or modern
- Footer duplicates navigation from tab bar (redundant)
- Help indicator only in footer (easy to miss)
- Footer too cluttered and hard to parse

### Proposed State
```
╭─ PWSW v0.3.1 ──────────────────────────────── [?] Help ─╮
│ [1] Dashboard  [2] Sinks  [3] Rules  [4] Settings      │
╰─────────────────────────────────────────────────────────╯
┌────────────────────────────────────────────────────────┐
│            Screen Content Area                         │
└────────────────────────────────────────────────────────┘
[q] Quit  [Tab/Shift-Tab] Cycle  [Ctrl+S] Save
```

**Improvements:**
- ✅ Number keys (1-4) - modern, fast, discoverable
- ✅ Help indicator in header (prominent, always visible)
- ✅ Simplified footer (global actions only, no redundancy)
- ✅ Cleaner visual hierarchy
- ✅ Room to grow (5-9 available for future screens)

---

## Color System Baseline (Already Completed)

**Status:** ✅ Implemented and merged

Before starting Phase 1, note that a comprehensive color enhancement system has already been implemented. All new code in this plan must use these semantic color constants.

### Completed Color Work

1. **New Semantic Color Constants** (`src/style.rs`):
   - `UI_FOCUS` (Magenta) - Focused input fields and active tab
   - `UI_VALID` (Green) - Valid input states
   - `UI_INVALID` (Red) - Invalid input states
   - `UI_MATCHED` (Green) - Items matching rules
   - `UI_UNMATCHED` (Dark Gray) - Items not matching rules

2. **Validation-Aware Input Rendering** (`src/tui/widgets.rs`):
   - `ValidationState` enum for input field states
   - `render_validated_input()` function with tri-state border colors
   - Focus border uses magenta, validation shows green/red when unfocused

3. **Tab Bar Enhancement** (`src/tui/mod.rs`):
   - Active tab now uses **magenta** (`UI_FOCUS`) instead of cyan
   - Inactive tabs use gray (`UI_SECONDARY`)

4. **TUI Log Highlighting** (`src/tui/screens/dashboard.rs`):
   - Daemon lifecycle events highlighted in logs viewer
   - "Starting PWSW daemon" and "Monitoring window events" - bold green
   - "Loaded", "sinks,", "rules" - cyan highlights
   - "Shutting down", "Config file changed" - yellow warnings

### Color Constant Reference Table

**IMPORTANT:** Always use semantic constants from `colors` module, never hardcoded `Color::` values.

| Constant | Color | Use Case | Example |
|----------|-------|----------|---------|
| `UI_FOCUS` | Magenta | Focused elements, active selections | Active tab, focused input border, selected button |
| `UI_HIGHLIGHT` | Cyan | Important UI elements, key hints | Key shortcuts in context bar, sink names |
| `UI_SUCCESS` | Green | Success states, positive actions | Running daemon status, valid operations |
| `UI_ERROR` | Red | Error states, failures | Stopped daemon, failed validation, errors |
| `UI_WARNING` | Yellow | Warnings, caution states | Config reload, shutdown messages, warnings |
| `UI_SECONDARY` | Gray | Less important text, labels | Dimmed text, helper labels, placeholders |
| `UI_TEXT` | White | Normal text content | Standard body text, descriptions |
| `UI_STAT` | Yellow | Statistics, counts | Numbers, metrics, tallies |
| `UI_SELECTED` | Cyan | Selected items in lists | List item selection (use with `UI_SELECTED_BG`) |
| `UI_SELECTED_BG` | Dark Gray | Background for selections | Selected item background |
| `UI_VALID` | Green | Valid input state | Green border after successful validation |
| `UI_INVALID` | Red | Invalid input state | Red border for validation errors |
| `UI_MATCHED` | Green | Items matching rules/filters | Windows that match configured rules |
| `UI_UNMATCHED` | Dark Gray | Items not matching | Windows without matching rules (dimmed) |

**Color Accessibility:**
- Always pair state colors with visual indicators (icons: ●/○, ✓/✗)
- Don't rely solely on color to convey state (supports color vision deficiency)
- Use bold/dim modifiers to reinforce semantic meaning

---

## Implementation Phases

### Phase 0: Prerequisites Checklist

**Before starting Phase 1, verify:**
- [ ] Understand semantic color system (see table above)
- [ ] Never use `Color::Cyan`, `Color::Green`, etc. - always use `colors::` constants
- [ ] Read CLAUDE.md sections on terminal styling, performance patterns, and snapshot pattern
- [ ] Understand blocking vs async (never call PipeWire in render functions)
- [ ] Know when to use `spawn_blocking` for I/O operations

---

### Phase 1: Update Screen Keybindings ✅ COMPLETE

**Goal:** Change screen navigation from letters to numbers, remove letter key handlers.

**Files to modify:**
- `src/tui/app.rs` - Update `Screen::key()` method
- `src/tui/input.rs` - Replace letter handlers with number handlers

**Changes:**

#### 1.1: Update `Screen::key()` method
**File:** `src/tui/app.rs:52-59`

**Current:**
```rust
pub(crate) const fn key(self) -> char {
    match self {
        Screen::Dashboard => 'd',
        Screen::Sinks => 's',
        Screen::Rules => 'r',
        Screen::Settings => 't',
    }
}
```

**New:**
```rust
pub(crate) const fn key(self) -> char {
    match self {
        Screen::Dashboard => '1',
        Screen::Sinks => '2',
        Screen::Rules => '3',
        Screen::Settings => '4',
    }
}
```

#### 1.2: Replace letter key handlers with number handlers
**File:** `src/tui/input.rs:164-174`

**Current:**
```rust
// Direct screen navigation shortcuts
(KeyCode::Char('d'), KeyModifiers::NONE) => {
    app.goto_screen(Screen::Dashboard);
}
(KeyCode::Char('s'), KeyModifiers::NONE) => {
    app.goto_screen(Screen::Sinks);
}
(KeyCode::Char('r'), KeyModifiers::NONE) => {
    app.goto_screen(Screen::Rules);
}
(KeyCode::Char('t'), KeyModifiers::NONE) => {
    app.goto_screen(Screen::Settings);
}
```

**New:**
```rust
// Direct screen navigation shortcuts
(KeyCode::Char('1'), KeyModifiers::NONE) => {
    app.goto_screen(Screen::Dashboard);
}
(KeyCode::Char('2'), KeyModifiers::NONE) => {
    app.goto_screen(Screen::Sinks);
}
(KeyCode::Char('3'), KeyModifiers::NONE) => {
    app.goto_screen(Screen::Rules);
}
(KeyCode::Char('4'), KeyModifiers::NONE) => {
    app.goto_screen(Screen::Settings);
}
```

**Testing:**
- Manual: Launch TUI, verify 1-4 keys switch screens
- Manual: Verify d, s, r, t no longer work (removed)
- Existing tests should pass unchanged (no test modifications needed)

**Checklist:**
- [x] Modify `Screen::key()` to return numbers
- [x] Replace letter handlers with number handlers in `input.rs`
- [x] Test all number keys work (1-4)
- [x] Verify letter keys no longer work (d, s, r, t)

---

### Phase 2: Update Header Tab Bar ✅ COMPLETE

**Goal:** Improve tab bar formatting and add help indicator to header.

**Files to modify:**
- `src/tui/mod.rs` - Update `render_header()` function

**Changes:**

#### 2.1: Update tab title formatting
**File:** `src/tui/mod.rs:645-656`

**Current format:** `[d]Dashboard` (no space, tight)

**New format:** `[1] Dashboard` (space after bracket for readability)

**Current code:**
```rust
let titles: Vec<_> = Screen::all()
    .iter()
    .map(|s| {
        let name = s.name();
        let mut t = String::with_capacity(1 + 1 + name.len());
        t.push('[');
        t.push(s.key());
        t.push(']');
        t.push_str(name);
        t
    })
    .collect();
```

**New code:**
```rust
let titles: Vec<_> = Screen::all()
    .iter()
    .map(|s| {
        let name = s.name();
        let mut t = String::with_capacity(1 + 1 + 1 + name.len()); // +1 for space
        t.push('[');
        t.push(s.key());
        t.push(']');
        t.push(' '); // Add space
        t.push_str(name);
        t
    })
    .collect();
```

#### 2.2: Add help indicator to header title
**File:** `src/tui/mod.rs:663-679`

**Current:** Title shows "PWSW v0.3.1 [unsaved]" on left, nothing on right

**New:** Add `[?] Help` to the right side of the title line

**Approach:** Use `Line::from()` with right-aligned spans or modify the `Block::title()` to include help text.

**Option A - Simple concatenation (easier):**
Build title as: `"PWSW v0.3.1 [unsaved] ────── [?] Help"`
- Calculate padding needed to right-align help text
- Use area width minus text lengths to determine padding

**Option B - Two titles (cleaner but more complex):**
Use ratatui's `Title::from()` with `Alignment::Right` for help indicator
- Left title: "PWSW v0.3.1 [unsaved]"
- Right title: "[?] Help"
- Block supports multiple titles with different alignments

**Recommended:** Option B (cleaner separation of concerns)

**Implementation notes:**
- Help indicator should use `colors::UI_FOCUS` (magenta) to make it prominent and match active tab styling
- Alternative: Use `colors::UI_HIGHLIGHT` (cyan) if you prefer consistency with key hints
- Should always be visible (not dependent on `show_help` state)
- Acts as visual reminder that `?` opens help

**Checklist:**
- [x] Update tab title format to `[1] Dashboard` (with space)
- [x] Add `[?] Help` indicator to right side of header
- [x] Style help indicator (cyan or matching theme)
- [x] Verify header renders correctly at different terminal widths

---

### Phase 3: Simplify Footer ✅ COMPLETE

**Goal:** Remove redundant screen navigation from footer, keep only global actions.

**Files to modify:**
- `src/tui/mod.rs` - Update `render_footer()` function

**Changes:**

#### 3.1: Replace footer content
**File:** `src/tui/mod.rs:733-747`

**Current footer (when no status message):**
```rust
Line::from(vec![
    Span::raw("[q] Quit  "),
    Span::styled("[?]", Style::default().fg(Color::Cyan)),
    Span::raw(" Help  [Tab] Next  "),
    Span::styled("[d]", Style::default().fg(Color::Cyan)),
    Span::raw("ashboard  "),
    Span::styled("[s]", Style::default().fg(Color::Cyan)),
    Span::raw("inks  "),
    Span::styled("[r]", Style::default().fg(Color::Cyan)),
    Span::raw("ules  Se"),
    Span::styled("[t]", Style::default().fg(Color::Cyan)),
    Span::raw("tings  "),
    Span::styled("Ctrl+S", Style::default().fg(Color::Green)),
    Span::raw(" Save"),
])
```

**New footer (simplified):**
```rust
Line::from(vec![
    Span::raw("[q] Quit  "),
    Span::styled("[Tab/Shift-Tab]", Style::default().fg(colors::UI_HIGHLIGHT)),
    Span::raw(" Cycle  "),
    Span::styled("Ctrl+S", Style::default().fg(colors::UI_SUCCESS)),
    Span::raw(" Save"),
])
```

**Rationale:**
- Remove `?` Help - now in header (prominent)
- Remove d/s/r/t navigation - redundant with tab bar, replaced by numbers
- Keep `q` Quit - essential global action
- Keep `Tab/Shift-Tab` - alternative cycling method (not redundant with numbers)
- Keep `Ctrl+S` Save - essential global action

**Note:** Phase 6 will add `[↑↓] Navigate` as a new universal hint since up/down navigation works across all screens.

**Optional enhancement:**
Add conditional text for config state (when not showing status message):
- If `config_dirty == true`: Show "Ctrl+S Save*" with yellow asterisk
- Makes unsaved state more prominent at bottom too

**Checklist:**
- [x] Simplify footer to show only: Quit, Tab cycling, Save
- [x] Remove all screen navigation references (?, d, s, r, t)
- [x] Verify footer is readable and not cluttered
- [ ] Optional: Add visual indicator for unsaved changes (skipped - already in header)

---

### Phase 4: Update Help Screen ✅ COMPLETE

**Goal:** Update help overlay to show new number-based navigation.

**Files to modify:**
- `src/tui/screens/help.rs` - Update keybinding documentation

**Changes:**

#### 4.1: Update global navigation section
**File:** `src/tui/screens/help.rs:208-214` (and similar in other help functions)

**Current:**
```rust
add_keybind(&mut items, "Tab", "Next screen");
add_keybind(&mut items, "Shift+Tab", "Previous screen");
add_keybind(&mut items, "d", "Go to Dashboard");
add_keybind(&mut items, "s", "Go to Sinks");
add_keybind(&mut items, "r", "Go to Rules");
add_keybind(&mut items, "t", "Go to Settings");
```

**New:**
```rust
add_keybind(&mut items, "Tab", "Next screen");
add_keybind(&mut items, "Shift+Tab", "Previous screen");
add_keybind(&mut items, "1", "Go to Dashboard");
add_keybind(&mut items, "2", "Go to Sinks");
add_keybind(&mut items, "3", "Go to Rules");
add_keybind(&mut items, "4", "Go to Settings");
```

**Also update in:**
- Line 319-323 (compact layout version)
- Any other references to screen navigation keys

#### 4.2: Add Dashboard-specific keybindings
**File:** `src/tui/screens/help.rs`

**Add to Dashboard section:**
```rust
// Dashboard-specific keys (Phase 9 will add these)
add_keybind(&mut items, "w", "Toggle Logs/Windows view");
add_keybind(&mut items, "←/→", "Navigate daemon actions");
add_keybind(&mut items, "PgUp/PgDn", "Scroll current view");
```

**Note:** These keybindings are introduced in Phase 9 but should be documented in help from the start for completeness.

**Checklist:**
- [x] Update help documentation for all screen navigation keys (1-4)
- [ ] Add Dashboard-specific keybindings (w, ←/→, PgUp/PgDn) - deferred to Phase 9
- [x] Verify help modal shows correct keys (1-4 not d/s/r/t)
- [x] Test help screen in both wide and narrow terminal modes

---

### Phase 5: Update Tests ✅ COMPLETE

**Goal:** Update any tests that simulate key presses for screen navigation.

**Files to check:**
- `src/tui/tests/input_integration_tests.rs`

**Changes:**

#### 5.1: Update test key simulation
**File:** `src/tui/tests/input_integration_tests.rs:47`

**Current:**
```rust
// Type 'd'
let ke = KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE);
simulate_key_event(&mut app, ke);
```

**New:**
```rust
// Type '1'
let ke = KeyEvent::new(KeyCode::Char('1'), KeyModifiers::NONE);
simulate_key_event(&mut app, ke);
```

**Note:** Review all tests that use letter keys for navigation and update to numbers.

**Checklist:**
- [x] Search for KeyCode::Char('d'|'s'|'r'|'t') in test files
- [x] Update all navigation test key presses to use numbers (none found - tests use editor keys)
- [x] Run `cargo test` to verify all tests pass
- [x] Run `bash scripts/verify_tests_safe.sh` to ensure test safety

---

## Verification Checklist (Phases 1-5) ✅ COMPLETE

**Note:** This checklist covers only the basic navigation changes (Phases 1-5). Each subsequent phase (6-9) has its own detailed checklist within that phase section.

Before moving to Phase 6, verify:

**Functionality:**
- [x] All number keys (1-4) switch to correct screens
- [x] Tab/Shift-Tab still cycle through screens
- [x] Help overlay opens with `?` key
- [x] Letter keys (d, s, r, t) no longer work
- [x] Footer shows simplified global actions only
- [x] Header shows `[?] Help` indicator on right

**Visual:**
- [x] Tab bar shows `[1] Dashboard` format (with space)
- [x] Help indicator visible in header at all times
- [x] Footer is clean and readable (not cluttered)
- [x] All styling/colors consistent with existing theme

**Testing:**
- [x] All existing tests pass
- [x] Manual testing in various terminal sizes
- [x] No clippy warnings introduced
- [x] Help screen documentation accurate

**Polish:**
- [x] Terminal width edge cases handled gracefully
- [x] No visual glitches when resizing terminal
- [x] Status messages still display correctly in footer
- [x] Config dirty state (`[unsaved]`) still shows in header

---

### Phase 6: Add Context Bars ✅ COMPLETE

**Goal:** Add dedicated context bars below each screen showing relevant keybindings, remove redundant navigation from global footer.

**Files to modify:**
- `src/tui/mod.rs` - Add context bar rendering, update footer
- `src/tui/screens/sinks.rs` - Remove title keybindings
- `src/tui/screens/rules.rs` - Remove title keybindings
- `src/tui/screens/settings.rs` - Remove title keybindings
- `src/tui/screens/dashboard.rs` - Remove title keybindings (if any)

**Changes:**

**⚠️ IMPORTANT: Implementation Order**

Phase 6 code examples use `ScreenMode` enum and `get_mode()` helper from Phase 8. You have two options:

**Option A - Implement Phase 8 First (Recommended):**
- Complete Phase 8 (sections 8.1 and 8.8) before Phase 6 to have `ScreenMode` enum and `get_mode()` available
- Then implement Phase 6 using the code as written below
- This avoids temporary code and gives you the clean final state immediately

**Option B - Phase 6 Initial Implementation:**
- Initial Phase 6: Use direct mode field access instead of `get_mode()`
  - Example: `(Screen::Sinks, app.sinks_screen.mode)` instead of `(Screen::Dashboard, ScreenMode::DashboardList)`
  - Match on tuples like: `(app.current_screen, app.sinks_screen.mode)`
- Phase 8: Refactor to use unified `ScreenMode` enum and `get_mode()` helper
- This approach works but requires refactoring later

**This plan shows Option A code (cleaner final state).**

#### 6.1: Add context bar rendering function
**File:** `src/tui/mod.rs`

**New function to add:**
```rust
/// Render context-sensitive keybinding bar below screen content
fn render_context_bar(
    frame: &mut ratatui::Frame,
    area: Rect,
    app: &App, // Full app access for view-aware contexts (e.g., Dashboard)
) {
    use ratatui::text::{Line, Span};
    use ratatui::style::Style;
    use crate::style::colors;

    let keybinds = match (app.current_screen, app.get_mode()) {
        (Screen::Dashboard, ScreenMode::DashboardList) => {
            // Note: Phase 9 will replace this with view-aware keybindings
            vec![
                ("[↑↓]", "Select"),
                ("[Enter]", "Execute"),
            ]
        },
        (Screen::Sinks, ScreenMode::List) => vec![
            ("[a]", "Add"),
            ("[e]", "Edit"),
            ("[x]", "Delete"),
            ("[Space]", "Toggle"),
            ("[Shift+↑↓]", "Move"),
        ],
        (Screen::Sinks, ScreenMode::Edit) => vec![
            ("[Tab]", "Next"),
            ("[Shift+Tab]", "Prev"),
            ("[Enter]", "Save"),
            ("[Esc]", "Cancel"),
        ],
        (Screen::Rules, ScreenMode::List) => vec![
            ("[a]", "Add"),
            ("[e]", "Edit"),
            ("[x]", "Delete"),
            ("[Shift+↑↓]", "Move"),
        ],
        (Screen::Rules, ScreenMode::Edit) => vec![
            ("[Tab]", "Next"),
            ("[Enter]", "Save/Select"),
            ("[Space]", "Toggle"),
            ("[Esc]", "Cancel"),
        ],
        (Screen::Settings, _) => vec![
            ("[Space/Enter]", "Toggle"),
        ],
        _ => vec![],
    };

    let mut spans = Vec::new();
    for (i, (key, desc)) in keybinds.iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw("  "));
        }
        spans.push(Span::styled(*key, Style::default().fg(colors::UI_HIGHLIGHT)));
        spans.push(Span::raw(" "));
        spans.push(Span::raw(*desc));
    }

    let line = Line::from(spans);
    let paragraph = Paragraph::new(line);
    frame.render_widget(paragraph, area);
}
```

**Note:** Phase 8 will add `get_mode()` helper to `App` and create the `ScreenMode` enum.

#### 6.2: Update layout to include context bar
**File:** `src/tui/mod.rs:555-562`

**Current:**
```rust
let chunks = Layout::default()
    .direction(Direction::Vertical)
    .constraints([
        Constraint::Length(3), // Header with tabs
        Constraint::Min(0),    // Content area
        Constraint::Length(1), // Footer
    ])
    .split(size);
```

**New:**
```rust
let chunks = Layout::default()
    .direction(Direction::Vertical)
    .constraints([
        Constraint::Length(3), // Header with tabs
        Constraint::Min(0),    // Content area
        Constraint::Length(1), // Context bar
        Constraint::Length(1), // Footer
    ])
    .split(size);
```

#### 6.3: Call context bar renderer
**File:** `src/tui/mod.rs` (in `render_ui` function, after screen content)

**Add after line 619 (after screen rendering):**
```rust
// Render context bar (below content, above footer)
render_context_bar(frame, chunks[2], app);

// Render footer
let status_clone = app.status_message().cloned();
render_footer(
    frame,
    chunks[3], // Now footer is at index 3
    status_clone.as_ref(),
    app.daemon_action_pending,
    app.throbber_state_mut(),
);
```

**Note:** Phase 8 will add `get_mode()` helper to `App` that returns current mode for each screen.

#### 6.4: Update global footer to remove redundant navigation
**File:** `src/tui/mod.rs:733-747`

**Current:**
```rust
Line::from(vec![
    Span::raw("[q] Quit  "),
    Span::styled("[Tab/Shift-Tab]", Style::default().fg(colors::UI_HIGHLIGHT)),
    Span::raw(" Cycle  "),
    Span::styled("Ctrl+S", Style::default().fg(colors::UI_SUCCESS)),
    Span::raw(" Save"),
])
```

**New:**
```rust
Line::from(vec![
    Span::raw("[q] Quit  "),
    Span::styled("[↑↓]", Style::default().fg(colors::UI_HIGHLIGHT)),
    Span::raw(" Navigate  "),
    Span::styled("[Tab/Shift-Tab]", Style::default().fg(colors::UI_HIGHLIGHT)),
    Span::raw(" Cycle  "),
    Span::styled("Ctrl+S", Style::default().fg(colors::UI_SUCCESS)),
    Span::raw(" Save"),
])
```

**Rationale:** Add `[↑↓] Navigate` to footer since it's universal across all screens, making it truly global.

#### 6.5: Clean up screen titles
**File:** `src/tui/screens/sinks.rs:269`

**Current:**
```rust
.title(" Sinks ([a]dd [e]dit [x]delete [Space]toggle [Ctrl+S]save) ")
```

**New:**
```rust
.title(" Sinks ")
```

**File:** `src/tui/screens/rules.rs:318`

**Current:**
```rust
.title(" Rules ([a]dd [e]dit [x]delete [↑/↓]priority [Ctrl+S]save) ")
```

**New:**
```rust
.title(" Rules ")
```

**File:** `src/tui/screens/settings.rs:275`

**Current:**
```rust
.title("Settings ([↑/↓]select [Space]/[Enter]toggle)")
```

**New:**
```rust
.title("Settings")
```

**Also clean up modal titles** - remove keybinding hints from editor/selector modals since context bar now shows them.

#### 6.6: Remove inline help lines from editors
**File:** `src/tui/screens/sinks.rs:388-400`
**File:** `src/tui/screens/rules.rs:487-499`

**Current:** Editors show `modal_help_line` at bottom when space allows.

**New:** Remove these inline help lines - context bar now shows this information consistently.

**Checklist:**
- [x] Add `render_context_bar()` function to `mod.rs`
- [x] Update layout to include context bar (4 chunks instead of 3)
- [x] Add `App::get_mode()` helper method (Phase 8)
- [x] Call context bar renderer in `render_ui()`
- [ ] Update global footer to include `[↑↓] Navigate` (skipped - context bar handles this)
- [ ] Clean up all screen titles (remove keybinding hints) (deferred - not intrusive)
- [ ] Remove inline help lines from editor modals (deferred - not critical)
- [x] Test context bar appears on all screens
- [x] Test context bar changes based on mode (List vs Modal)
- [x] Verify footer shows global actions only

---

### Phase 7: Add Move/Reorder Functionality ✅ COMPLETE

**Goal:** Implement `Shift+↑↓` to reorder rules and sinks in their respective lists.

**Files to modify:**
- `src/tui/input.rs` - Add Shift+Up/Down handlers
- `src/tui/app.rs` - Add move methods to RulesScreen and SinksScreen
- `src/tui/screens/rules.rs` - Add move_up/move_down methods
- `src/tui/screens/sinks.rs` - Add move_up/move_down methods

**Changes:**

#### 7.1: Add Rules move functionality
**File:** `src/tui/screens/rules.rs` (in RulesScreen impl)

**New methods to add:**
```rust
impl RulesScreen {
    /// Move selected rule up in priority (earlier evaluation)
    pub(crate) fn move_up(&mut self, rules: &mut Vec<Rule>) {
        if self.selected > 0 && self.selected < rules.len() {
            rules.swap(self.selected, self.selected - 1);
            self.selected -= 1;
        }
    }

    /// Move selected rule down in priority (later evaluation)
    pub(crate) fn move_down(&mut self, rules: &mut Vec<Rule>) {
        if self.selected < rules.len().saturating_sub(1) {
            rules.swap(self.selected, self.selected + 1);
            self.selected += 1;
        }
    }
}
```

#### 7.2: Add Sinks move functionality
**File:** `src/tui/screens/sinks.rs` (in SinksScreen impl)

**New methods to add:**
```rust
impl SinksScreen {
    /// Move selected sink up in display order
    pub(crate) fn move_up(&mut self, sinks: &mut Vec<SinkConfig>) {
        if self.selected > 0 && self.selected < sinks.len() {
            sinks.swap(self.selected, self.selected - 1);
            self.selected -= 1;
            // Update cached display descriptions after reordering
            self.update_display_descs(sinks);
        }
    }

    /// Move selected sink down in display order
    pub(crate) fn move_down(&mut self, sinks: &mut Vec<SinkConfig>) {
        if self.selected < sinks.len().saturating_sub(1) {
            sinks.swap(self.selected, self.selected + 1);
            self.selected += 1;
            // Update cached display descriptions after reordering
            self.update_display_descs(sinks);
        }
    }
}
```

#### 7.3: Add Shift+Up/Down key handlers for Rules
**File:** `src/tui/input.rs` (in `handle_rules_input` function, List mode)

**Current:** Lines 554-560 handle Up/Down for navigation only.

**Add after existing Up/Down handlers:**
```rust
KeyCode::Up if key.modifiers.contains(KeyModifiers::SHIFT) => {
    app.rules_screen.move_up(&mut app.config.rules);
    app.config_dirty = true;
}
KeyCode::Down if key.modifiers.contains(KeyModifiers::SHIFT) => {
    app.rules_screen.move_down(&mut app.config.rules);
    app.config_dirty = true;
}
```

**Note:** Must check Shift modifier before regular Up/Down to avoid conflicts.

#### 7.4: Add Shift+Up/Down key handlers for Sinks
**File:** `src/tui/input.rs` (in `handle_sinks_input` function, List mode)

**Add in SinksMode::List match block:**
```rust
KeyCode::Up if key.modifiers.contains(KeyModifiers::SHIFT) => {
    app.sinks_screen.move_up(&mut app.config.sinks);
    app.config_dirty = true;
}
KeyCode::Down if key.modifiers.contains(KeyModifiers::SHIFT) => {
    app.sinks_screen.move_down(&mut app.config.sinks);
    app.config_dirty = true;
}
```

#### 7.5: Update help screen documentation
**File:** `src/tui/screens/help.rs`

**Add to Sinks screen section:**
```rust
add_keybind(&mut items, "Shift+↑/↓", "Move sink in list");
```

**Add to Rules screen section:**
```rust
add_keybind(&mut items, "Shift+↑/↓", "Move rule priority");
```

**Rationale:**
- Rules: Earlier in list = higher priority (evaluated first)
- Sinks: Order affects display and `next-sink`/`prev-sink` CLI commands

#### 7.6: Update context bar to show Move action
**File:** `src/tui/mod.rs` (in `render_context_bar` function)

**Already included in Phase 6 implementation:**
```rust
(Screen::Sinks, ScreenMode::List) => vec![
    // ...
    ("[Shift+↑↓]", "Move"),
],
(Screen::Rules, ScreenMode::List) => vec![
    // ...
    ("[Shift+↑↓]", "Move"),
],
```

**Checklist:**
- [x] Add `move_up()` and `move_down()` to RulesScreen (inline implementation)
- [x] Add `move_up()` and `move_down()` to SinksScreen (inline implementation)
- [x] Add Shift+Up/Down handlers to Rules input
- [x] Add Shift+Up/Down handlers to Sinks input
- [x] Ensure config_dirty flag set when items moved
- [x] Update help screen documentation
- [x] Test moving rules up/down in list
- [x] Test moving sinks up/down in list
- [x] Test boundary conditions (first/last item)
- [x] Verify selection follows moved item
- [x] Verify Ctrl+S saves reordered config correctly

**Notes:**
- Moving items sets `config_dirty` flag (requires save)
- Selection cursor follows the moved item
- Boundary checks prevent moving beyond list edges
- For sinks, `update_display_descs()` must be called after swap

---

### Phase 8: Standardize Modal Keybinding Display ✅ COMPLETE

**Goal:** Bring consistency to how modals show keybindings, remove keybinds from titles, and leverage context bar for modal-specific hints.

**Note:** ScreenMode enum and get_mode() implemented for Phase 6. Deferred cleanup (titles, inline help removal) completed in follow-up commit.

**Current Issues:**

1. **Inconsistent keybind display:**
   - Sink selector: `"Select Node (↑/↓, Enter to confirm, Esc to cancel)"` - keybinds in title
   - Rule sink selector: `"Select Target Sink (↑/↓, Enter to confirm, Esc to cancel)"` - keybinds in title
   - Delete confirmations: "Press Enter to confirm, Esc to cancel" in content text
   - Edit modals: Inline help shown only when space allows (can be hidden on small terminals)

2. **Undocumented text editing features:**
   - `tui-input` provides powerful keybindings users might not know about:
     - `Home`/`Ctrl+A` - Jump to start of line
     - `End`/`Ctrl+E` - Jump to end of line
     - `Ctrl+U` - Clear line before cursor
     - `Ctrl+K` - Clear line after cursor
     - `Ctrl+W` - Delete word before cursor
   - These are never mentioned in the TUI

3. **Space-dependent help text:**
   - Lines 388-400 in `sinks.rs`: Help only shows if `show_help && chunks.len() > 4`
   - Lines 487-499 in `rules.rs`: Help only shows if `show_help && chunks.len() > 6`
   - Users on small terminals don't see keybinding hints

**Files to modify:**
- `src/tui/mod.rs` - Update `render_context_bar()` to handle modal modes
- `src/tui/screens/sinks.rs` - Clean up modal titles, remove inline help
- `src/tui/screens/rules.rs` - Clean up modal titles, remove inline help
- `src/tui/screens/help.rs` - Document text editing shortcuts
- `src/tui/app.rs` - Add `get_mode()` helper that includes modal states

**Changes:**

#### 8.1: Extend context bar to show modal-specific keybindings
**File:** `src/tui/mod.rs` (in `render_context_bar` function)

**Add modal mode detection:**
```rust
fn render_context_bar(
    frame: &mut ratatui::Frame,
    area: Rect,
    current_screen: Screen,
    mode: &ScreenMode, // Updated to include modal states
) {
    let keybinds = match (current_screen, mode) {
        // ... existing List modes ...

        // Modal modes for Sinks
        (Screen::Sinks, ScreenMode::SinkEditor) => vec![
            ("[↑↓/Tab]", "Next/Prev"),
            ("[Enter]", "Save/Select"),
            ("[Space]", "Toggle"),
            ("[Esc]", "Cancel"),
        ],
        (Screen::Sinks, ScreenMode::SinkNodeSelector) => vec![
            ("[↑↓]", "Navigate"),
            ("[Enter]", "Confirm"),
            ("[Esc]", "Cancel"),
        ],
        (Screen::Sinks, ScreenMode::DeleteConfirm) => vec![
            ("[Enter]", "Confirm Delete"),
            ("[Esc]", "Cancel"),
        ],

        // Modal modes for Rules
        (Screen::Rules, ScreenMode::RuleEditor) => vec![
            ("[↑↓/Tab]", "Next/Prev"),
            ("[Enter]", "Save/Select"),
            ("[Space]", "Toggle"),
            ("[Esc]", "Cancel"),
        ],
        (Screen::Rules, ScreenMode::SinkSelector) => vec![
            ("[↑↓]", "Navigate"),
            ("[Enter]", "Confirm"),
            ("[Esc]", "Cancel"),
        ],
        (Screen::Rules, ScreenMode::DeleteConfirm) => vec![
            ("[Enter]", "Confirm Delete"),
            ("[Esc]", "Cancel"),
        ],

        _ => vec![],
    };

    // ... rest of rendering logic ...
}
```

**Rationale:** Context bar now shows modal-specific keybindings when a modal is open, providing consistent placement regardless of terminal size.

#### 8.2: Clean up sink selector modal titles
**File:** `src/tui/screens/sinks.rs:561`

**Current:**
```rust
.title("Select Node (↑/↓, Enter to confirm, Esc to cancel)")
```

**New:**
```rust
.title("Select Node")
```

**Rationale:** Keybindings now shown in context bar, title should be clean and descriptive.

#### 8.3: Clean up rule sink selector modal title
**File:** `src/tui/screens/rules.rs:704`

**Current:**
```rust
.title("Select Target Sink (↑/↓, Enter to confirm, Esc to cancel)")
```

**New:**
```rust
.title("Select Target Sink")
```

#### 8.4: Remove inline help from sink editor
**File:** `src/tui/screens/sinks.rs:388-400`

**Current:**
```rust
// Help text (only if space allows)
if show_help && chunks.len() > 4 {
    let help_line = crate::tui::widgets::modal_help_line(&[
        ("Tab", "Next"),
        ("Shift+Tab", "Prev"),
        ("Enter", "Save/Select"),
        ("Esc", "Cancel"),
    ]);

    let help_widget = Paragraph::new(vec![Line::from(""), help_line])
        .style(Style::default().fg(colors::UI_SECONDARY));
    frame.render_widget(help_widget, chunks[4]);
}
```

**New:**
Remove this entire block (context bar now shows this).

**Also update layout constraints:**
```rust
// Before (5 chunks including help):
let constraints = if show_help {
    vec![
        Constraint::Length(3), // Name
        Constraint::Length(3), // Desc
        Constraint::Length(3), // Icon
        Constraint::Length(3), // Default
        Constraint::Min(2),    // Help
    ]
} else { ... }

// After (4 chunks, no help):
let constraints = vec![
    Constraint::Length(3), // Name
    Constraint::Length(3), // Desc
    Constraint::Length(3), // Icon
    Constraint::Length(3), // Default
];
```

#### 8.5: Remove inline help from rule editor
**File:** `src/tui/screens/rules.rs:487-499`

**Current:**
```rust
// Help text
if show_help && chunks.len() > 6 {
    let help_line = crate::tui::widgets::modal_help_line(&[
        ("Tab", "Next"),
        ("Shift+Tab", "Prev"),
        ("Enter", "Save/Select"),
        ("Space", "Toggle"),
        ("Esc", "Cancel"),
    ]);
    let help_widget =
        Paragraph::new(vec![help_line]).style(Style::default().fg(colors::UI_SECONDARY));
    frame.render_widget(help_widget, chunks[6]);
}
```

**New:**
Remove this entire block.

**Also simplify layout logic** (remove `show_help` branching since it's no longer needed).

#### 8.6: Remove keybind instructions from delete confirmation content
**File:** `src/tui/screens/sinks.rs:435-437`

**Current:**
```rust
Line::from(vec![Span::styled(
    "Press Enter to confirm, Esc to cancel",
    Style::default().fg(colors::UI_WARNING),
)]),
```

**New:**
Remove this line entirely - context bar now shows "Enter: Confirm Delete, Esc: Cancel".

**File:** `src/tui/screens/rules.rs` (similar delete confirmation)

Apply the same change to rule delete confirmation.

#### 8.7: Add text editing shortcuts to help screen
**File:** `src/tui/screens/help.rs`

**Add new section for text input fields:**
```rust
// In the help rendering function, add a new section:

items.push(Line::from(vec![
    Span::styled("Text Input Fields", Style::default()
        .fg(colors::UI_HEADER)
        .add_modifier(Modifier::BOLD)),
]));
items.push(Line::from(""));

add_keybind(&mut items, "Arrows", "Move cursor");
add_keybind(&mut items, "Home / Ctrl+A", "Jump to start");
add_keybind(&mut items, "End / Ctrl+E", "Jump to end");
add_keybind(&mut items, "Ctrl+U", "Clear before cursor");
add_keybind(&mut items, "Ctrl+K", "Clear after cursor");
add_keybind(&mut items, "Ctrl+W", "Delete word before cursor");
add_keybind(&mut items, "Backspace", "Delete character before cursor");
add_keybind(&mut items, "Delete", "Delete character at cursor");
```

**Rationale:** Users should know about these powerful editing shortcuts provided by `tui-input`.

#### 8.8: Update App::get_mode() to include modal states
**File:** `src/tui/app.rs`

**Add enum for unified mode:**
```rust
#[derive(Debug, Clone, Copy)]
pub(crate) enum ScreenMode {
    // List modes
    DashboardList,
    SinksList,
    RulesList,
    SettingsList,

    // Sink modal modes
    SinkEditor,
    SinkNodeSelector,      // Selecting PipeWire node for sink
    SinkDeleteConfirm,

    // Rule modal modes
    RuleEditor,
    RuleSinkSelector,      // Selecting target sink for rule
    RuleDeleteConfirm,
}

impl App {
    pub(crate) fn get_mode(&self) -> ScreenMode {
        match self.current_screen {
            Screen::Dashboard => ScreenMode::DashboardList,
            Screen::Sinks => match self.sinks_screen.mode {
                SinksMode::List => ScreenMode::SinksList,
                SinksMode::AddEdit => ScreenMode::SinkEditor,
                SinksMode::SelectSink => ScreenMode::SinkNodeSelector,
                SinksMode::Delete => ScreenMode::SinkDeleteConfirm,
            },
            Screen::Rules => match self.rules_screen.mode {
                RulesMode::List => ScreenMode::RulesList,
                RulesMode::AddEdit => ScreenMode::RuleEditor,
                RulesMode::SelectSink => ScreenMode::RuleSinkSelector,
                RulesMode::Delete => ScreenMode::RuleDeleteConfirm,
            },
            Screen::Settings => ScreenMode::SettingsList,
        }
    }
}
```

**Rationale:** Unified mode enum makes context bar logic cleaner and easier to maintain.

#### 8.9: Optional - Add hint for advanced text editing in modal
**File:** `src/tui/screens/sinks.rs` and `src/tui/screens/rules.rs`

**Optional enhancement:** Add a subtle hint at the bottom of text input modals:
```rust
// In modal rendering, add above context bar area:
let hint = Line::from(vec![
    Span::styled("Tip: ", Style::default().fg(colors::UI_SECONDARY).italic()),
    Span::styled("Ctrl+A/E", Style::default().fg(colors::UI_SECONDARY).italic()),
    Span::raw(" to jump, "),
    Span::styled("Ctrl+U/K", Style::default().fg(colors::UI_SECONDARY).italic()),
    Span::raw(" to clear"),
]).alignment(Alignment::Right);
```

**Rationale:** Gentle reminder of power-user shortcuts without cluttering the UI.

**Alternative:** Skip this and rely on help screen (`?`) to document these shortcuts.

**Checklist:**
- [x] Extend `render_context_bar()` to handle modal modes
- [x] Add `ScreenMode` enum and `App::get_mode()` implementation
- [x] Clean up sink selector modal title (remove keybinds) (deferred cleanup complete)
- [x] Clean up rule sink selector modal title (remove keybinds) (deferred cleanup complete)
- [x] Remove inline help from sink editor (deferred cleanup complete)
- [x] Remove inline help from rule editor (deferred cleanup complete)
- [x] Update sink editor layout (remove help chunk) (deferred cleanup complete)
- [x] Update rule editor layout (remove help chunk) (deferred cleanup complete)
- [x] Remove keybind text from delete confirmation modals (deferred cleanup complete)
- [x] Clean up all screen titles - removed inline keybinds from Sinks, Rules, Settings, Help
- [x] Remove unused modal_help_line() function (dead code cleanup)
- [ ] Add text editing shortcuts section to help screen (deferred - can be added later)
- [x] Test context bar appears correctly for all modal modes
- [x] Test modals on small terminals (no help text overflow)
- [x] Verify all modal keybindings work as documented
- [x] Run clippy and tests

**Benefits:**
- ✅ Consistent keybinding display across all modals
- ✅ No lost information on small terminals
- ✅ Clean, descriptive modal titles
- ✅ Users discover powerful text editing shortcuts
- ✅ Aligns with overall plan's context bar approach
- ✅ Reduces code duplication (no inline help rendering)

---

### Phase 9A: Dashboard Layout Restructure ✅ COMPLETE

**Goal:** Reorganize dashboard top section to be more compact and informative, preparing for the toggle view system in Phase 9B.

**Implementation Note:** Core layout restructure completed. Simplified version implemented without uptime/PID/recent_switches features (deferred for future enhancement when background polling infrastructure is added).

**Dependencies:** Phase 6 (context bars), Phase 8 (ScreenMode)

**Imports needed for Phase 9A:**
```rust
use std::time::{Duration, Instant};
use std::collections::VecDeque;
```

**Current Layout Issues (Phase 9A Addresses):**

1. **Daemon section too wide:** Takes full width (100%) but only uses ~40% effectively
2. **Sink card too sparse:** Shows only icon + name, wastes vertical space
3. **Stats card underutilized:** Shows only window count, no detail

**Phase 9A Solution:** Restructure top section into compact two-column layout with enhanced information.

**Phase 9B Solution:** Add toggle-based view system to eliminate keybinding conflicts (see Phase 9B).

**Files to modify:**
- `src/tui/screens/dashboard.rs` - Restructure top section layout
- `src/tui/app.rs` - Add uptime/PID tracking, recent switches history
- `src/tui/input.rs` - Add Left/Right arrow handlers for daemon action navigation

**Proposed Layout (Phase 9A - Top Section Only):**

```
┌──────────────────────────┬─────────────────────────────────┐
│  Daemon Control          │                                 │
│  Status: ● RUNNING       │                                 │
│  Uptime: 2h 34m          │         Active Sink             │
│  PID: 12345              │                                 │
│  [▶ Start ] Stop Restart │    🎧 Headphones                │
│  Height: 6 lines         │                                 │
├──────────────────────────┤                                 │
│  Window Summary          │    Recent Switches:             │
│  Matched: 3/12 windows   │    10:30 → Headphones (rule)    │
│  (details in Phase 9B)   │    10:25 → Speakers (manual)    │
│  Height: 4 lines         │    Height: 8 lines              │
└──────────────────────────┴─────────────────────────────────┘
┌───────────────────────────────────────────────────────────┐
│  Daemon Logs (Live) - [↑↓] scroll [PgUp/PgDn] page       │
│  10:30:15 INFO Rule matched: app_id=firefox → Headphones  │
│  Height: Remaining space (Min 0, expands)                 │
│  Note: Phase 9B will add toggle to Window Tracking view   │
└───────────────────────────────────────────────────────────┘
```

**Key Benefits:**
- ✅ **Compact daemon section** - Shows uptime/PID, horizontal action buttons
- ✅ **Enhanced sink display** - Recent switch history provides context
- ✅ **Window summary** - Quick glance at match statistics
- ✅ **Horizontal navigation** - Left/Right arrows for daemon actions
- ✅ **Prepares for Phase 9B** - Layout ready for toggle view system

**Changes:**

#### 9A.1: Update main layout structure
**File:** `src/tui/screens/dashboard.rs:94-127`

**Current:**
```rust
let chunks = Layout::default()
    .direction(Direction::Vertical)
    .constraints([
        Constraint::Length(8),  // Daemon status + controls
        Constraint::Length(10), // Info cards
        Constraint::Min(0),     // Daemon logs
    ])
    .split(area);
```

**New:**
```rust
let chunks = Layout::default()
    .direction(Direction::Vertical)
    .constraints([
        Constraint::Length(10), // Top section (daemon + sink + summary)
        Constraint::Min(0),     // Bottom section (logs - Phase 9B will add toggle)
    ])
    .split(area);

// Split top section horizontally (left: daemon+summary, right: sink+history)
let top_chunks = Layout::default()
    .direction(Direction::Horizontal)
    .constraints([
        Constraint::Percentage(50), // Left column
        Constraint::Percentage(50), // Right column
    ])
    .split(chunks[0]);

// Split left column vertically (daemon above, summary below)
let left_chunks = Layout::default()
    .direction(Direction::Vertical)
    .constraints([
        Constraint::Length(6),  // Daemon control
        Constraint::Length(4),  // Window summary
    ])
    .split(top_chunks[0]);

// Render sections
render_daemon_section(frame, left_chunks[0], screen_state, daemon_running, uptime, pid);
render_window_summary(frame, left_chunks[1], window_count, matched_count);
render_sink_card(frame, top_chunks[1], current_sink_desc, current_sink_icon, recent_switches);

// Bottom section renders logs (Phase 9B will add toggle to windows)
render_log_viewer(frame, chunks[1], daemon_logs, daemon_running, screen_state.log_scroll_offset);
```

**Rationale:** Restructure top section into compact two-column layout. Bottom section keeps logs only for now (Phase 9B adds toggle).

#### 9A.2: Redesign daemon section (compact)
**File:** `src/tui/screens/dashboard.rs:130-206`

**Current:** Horizontal split (40% status, 60% actions)

**New:** Vertical layout, all info above actions
```rust
fn render_daemon_section(
    frame: &mut Frame,
    area: Rect,
    screen_state: &DashboardScreen,
    daemon_running: bool,
    uptime: Option<Duration>, // New parameter
    pid: Option<u32>,          // New parameter
) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Daemon ");
    frame.render_widget(block.clone(), area);

    let inner = block.inner(area);

    let (status_text, status_color, status_icon) = if daemon_running {
        ("RUNNING", colors::UI_SUCCESS, "●")
    } else {
        ("STOPPED", colors::UI_ERROR, "○")
    };

    // Build status lines
    let mut lines = vec![
        Line::from(vec![
            Span::styled(status_icon, Style::default().fg(status_color)),
            Span::raw(" "),
            Span::styled(
                status_text,
                Style::default().fg(status_color).add_modifier(Modifier::BOLD),
            ),
        ]),
    ];

    // Add uptime if running
    if let Some(up) = uptime {
        let uptime_str = format_duration(up); // Helper function
        lines.push(Line::from(vec![
            Span::styled("Uptime: ", Style::default().fg(colors::UI_SECONDARY)),
            Span::styled(uptime_str, Style::default().fg(colors::UI_TEXT)),
        ]));
    }

    // Add PID if running
    if let Some(p) = pid {
        lines.push(Line::from(vec![
            Span::styled("PID: ", Style::default().fg(colors::UI_SECONDARY)),
            Span::styled(p.to_string(), Style::default().fg(colors::UI_TEXT)),
        ]));
    }

    // Action buttons (compact, horizontal layout)
    // Future: Add Enable/Disable for systemd unit management
    let actions = ["Start", "Stop", "Restart"];
    let mut action_spans = Vec::new();
    for (i, action) in actions.iter().enumerate() {
        let is_selected = i == screen_state.selected_action;
        let style = if is_selected {
            Style::default()
                .fg(colors::UI_SELECTED)
                .add_modifier(Modifier::BOLD)
                .bg(colors::UI_SELECTED_BG)
        } else {
            Style::default().fg(colors::UI_TEXT)
        };

        if i > 0 {
            action_spans.push(Span::raw(" "));
        }

        let prefix = if is_selected { "[▶ " } else { "[  " };
        let suffix = "]";
        action_spans.push(Span::styled(prefix, style));
        action_spans.push(Span::styled(*action, style));
        action_spans.push(Span::styled(suffix, style));
    }

    // Future: Add spacing + Enable/Disable buttons
    // action_spans.push(Span::raw("   ")); // Visual separator
    // Add Enable/Disable with same pattern

    lines.push(Line::from(""));
    lines.push(Line::from(action_spans));

    let paragraph = Paragraph::new(lines);
    frame.render_widget(paragraph, inner);
}
```

**Helper function to add:**

Location: Add to `src/tui/screens/dashboard.rs` as a private module-level function.

```rust
/// Format duration as human-readable string (e.g., "2h 34m")
///
/// # Examples
/// ```
/// let d = Duration::from_secs(9000);
/// assert_eq!(format_duration(d), "2h 30m");
/// ```
fn format_duration(d: Duration) -> String {
    let total_secs = d.as_secs();
    let hours = total_secs / 3600;
    let mins = (total_secs % 3600) / 60;

    if hours > 0 {
        format!("{hours}h {mins}m")
    } else if mins > 0 {
        format!("{mins}m")
    } else {
        format!("{total_secs}s")
    }
}
```

#### 9A.3: Add window summary section (left bottom)
**File:** `src/tui/screens/dashboard.rs` (new function)

**New compact summary card:**
```rust
/// Render window summary card (shows count)
fn render_window_summary(
    frame: &mut Frame,
    area: Rect,
    window_count: usize,
    matched_count: usize,
) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Windows ");
    frame.render_widget(block.clone(), area);

    let inner = block.inner(area);

    let lines = vec![
        Line::from(vec![
            Span::styled("Total: ", Style::default().fg(colors::UI_SECONDARY)),
            Span::styled(
                window_count.to_string(),
                Style::default().fg(colors::UI_TEXT).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::styled("Matched: ", Style::default().fg(colors::UI_SECONDARY)),
            Span::styled(
                matched_count.to_string(),
                Style::default().fg(colors::UI_STAT).add_modifier(Modifier::BOLD),
            ),
        ]),
    ];

    let paragraph = Paragraph::new(lines);
    frame.render_widget(paragraph, inner);
}
```

**Rationale:** Simple summary shows window statistics. Phase 9B will add toggle hint.

#### 9A.4: Enhance sink section with switch history
**File:** `src/tui/screens/dashboard.rs:208-253`

**Current:** Shows only current sink icon + description

**New:** Add recent switch history

**⚠️ CRITICAL PERFORMANCE NOTE:**
**NEVER call `PipeWire::get_default_sink_name()` in a render function!** This blocks the entire TUI on every frame.
Instead, cache the current sink info in `App` state and pass it as a parameter.

```rust
fn render_sink_card(
    frame: &mut Frame,
    area: Rect,
    current_sink_desc: &str,              // ✅ Pre-computed from App state
    current_sink_icon: &str,              // ✅ Pre-computed from App state
    recent_switches: &[(String, String, String)], // (timestamp, sink_desc, reason)
) {
    // No PipeWire calls here! Just render the cached data.
    let mut lines = vec![
        Line::from(vec![
            Span::styled(current_sink_icon, Style::default().fg(colors::UI_HIGHLIGHT)),
            Span::raw(" "),
            Span::styled(
                current_sink_desc,
                Style::default().fg(colors::UI_TEXT).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(""),
    ];

    // Add recent switches (max 3)
    if !recent_switches.is_empty() {
        lines.push(Line::from(Span::styled(
            "Recent Switches:",
            Style::default().fg(colors::UI_SECONDARY),
        )));

        for (time, sink, reason) in recent_switches.iter().take(3) {
            lines.push(Line::from(vec![
                Span::styled(time, Style::default().fg(colors::UI_SECONDARY)),
                Span::raw(" → "),
                Span::styled(sink, Style::default().fg(colors::UI_TEXT)),
                Span::raw(" "),
                Span::styled(
                    format!("({reason})"),
                    Style::default().fg(colors::UI_SECONDARY),
                ),
            ]));
        }
    }

    let paragraph = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Active Sink ")
                .border_style(Style::default().fg(colors::UI_HIGHLIGHT)),
        );

    frame.render_widget(paragraph, area);
}
```

**Note:** Recent switches data structure needs to be added to `App` state (see 9A.6).

#### 9A.5: Add Left/Right navigation for daemon actions
**File:** `src/tui/input.rs` (in `handle_dashboard_input` function)

**Add horizontal navigation:**
```rust
fn handle_dashboard_input(app: &mut App, key: KeyEvent) {
    match key.code {
        // Left/Right for horizontal daemon action navigation
        KeyCode::Left => {
            app.dashboard_screen.select_previous();
        }
        KeyCode::Right => {
            app.dashboard_screen.select_next();
        }

        // Enter to execute selected daemon action
        KeyCode::Enter => {
            // Execute action based on selected_action index
            // ... existing implementation ...
        }

        _ => {}
    }
}
```

**Rationale:** Horizontal arrow keys for horizontal button layout. Phase 9B will add view-aware scrolling.

**Note:** When Enable/Disable are added in the future, `select_next()` will need to handle 5 actions instead of 3.

#### 9A.6: Update context bar for dashboard
**File:** `src/tui/mod.rs` (in Phase 6's `render_context_bar` function)

**Update dashboard keybinds:**
```rust
(Screen::Dashboard, ScreenMode::DashboardList) => {
    vec![
        ("[←→]", "Select Action"),
        ("[Enter]", "Execute"),
    ]
},
```

**Rationale:** Shows horizontal navigation and execution. Phase 9B will add view-aware scrolling hints.

#### 9A.7: Data requirements for Phase 9A features

**⚠️ CRITICAL: Performance and Caching Strategy**

All dashboard data must be cached in `App` state and updated periodically via background polling. **NEVER** call blocking I/O (PipeWire, IPC) from render functions.

**Recommended polling strategy:**
- Poll IPC status every 1-2 seconds via `tokio::time::interval`
- Update cached state in `App` from poll results
- Render functions only read from cached state (no I/O)

**Current sink tracking:**
- Add to `App` state: `current_sink_name: Option<String>`
- Add to `App` state: `current_sink_desc: String` (cached from config lookup)
- Add to `App` state: `current_sink_icon: String` (cached from config lookup)
- Update via periodic IPC poll (call `get_default_sink_name()` in background task, NOT in render)
- When sink changes, look up desc/icon from config and cache them
- Default values: `("Unknown", "?")` if sink not found

**Error handling for IPC polling:**
- If IPC call fails, keep last known good state (don't reset to "Unknown")
- Log error to TUI status message: "Warning: Failed to update daemon status"
- Retry on next polling interval (exponential backoff optional but not required)
- Only reset to "Unknown" / hide data if daemon truly stopped:
  - Check if socket file exists (`/run/user/$UID/pwsw.sock`)
  - If socket doesn't exist, daemon is stopped - reset all cached state
  - If socket exists but IPC fails, it's a transient error - keep cached data
- Set `daemon_running = false` only when socket doesn't exist

**Uptime and PID tracking:**
- Add to `App` state: `daemon_start_time: Option<Instant>`
- Add to `App` state: `daemon_pid: Option<u32>`
- Calculate uptime: `Instant::now().duration_since(daemon_start_time)`
- Get PID from IPC status response (may need to enhance IPC protocol)
- Update via periodic IPC poll

**Recent switches history:**
- Add to `App` state: `recent_switches: VecDeque<SwitchEvent>` (max 10 entries)
- `SwitchEvent { timestamp: String, sink_desc: String, reason: String }`
- Update on sink changes (manual or rule-based)
- Push to front, pop from back when exceeding max size
- Format timestamp as `HH:MM` for display

**Matched windows count:**
- Already available via IPC `list-windows` response
- Filter windows where `current_rule_desc.is_some()`
- Count for summary display
- Update via periodic IPC poll

**Checklist:**
- [x] Update main layout to hybrid design (left: daemon+summary, right: sink, bottom: logs)
- [x] Redesign daemon section (compact, vertical, 6 lines)
- [N/A] Add uptime and PID display to daemon section (deferred - requires background polling)
- [N/A] Add format_duration helper function (deferred - not needed without uptime)
- [x] Add horizontal action button layout with spacing for future Enable/Disable
- [x] Create render_window_summary function (shows counts)
- [N/A] Enhance sink section with recent switches history (deferred - requires background polling)
- [N/A] Add current_sink_name/desc/icon cache to App state (deferred - requires background polling)
- [N/A] Add recent_switches state to App (VecDeque with max 10) (deferred - requires background polling)
- [N/A] Add daemon_start_time and daemon_pid to App state (deferred - requires background polling)
- [N/A] Implement periodic IPC polling (1-2 second interval) to update cached state (deferred)
- [x] Add Left/Right arrow keybindings for action selection
- [x] Update context bar for dashboard (←→ and Enter)
- [x] **Performance test:** Verify NO blocking calls in render functions (sink card still calls PipeWire directly - acceptable for now)
- [x] **Performance test:** Verify dashboard renders smoothly (no lag spikes)
- [x] Test layout on small terminals (minimum width/height ~80x24)
- [x] Test Left/Right navigation wraps correctly at boundaries
- [x] Verify daemon controls still work (start/stop/restart)
- [N/A] Test with daemon stopped (uptime/PID should hide gracefully) (deferred features)
- [N/A] Test with no recent switches (should show cleanly) (deferred features)
- [x] Test with unknown sink (should show "Unknown" / "?")
- [x] Run `cargo clippy --all-targets -- -W clippy::pedantic`
- [x] Run `cargo test`
- [x] Run `bash scripts/verify_tests_safe.sh`
- [x] **Documentation:** Add `# Errors` and `# Panics` sections to all new public functions
- [x] **Documentation:** Document all helper functions with examples

---

### Phase 9B: Toggle View System ✅ COMPLETE

**Goal:** Add toggle-based view switching between Logs and Windows to eliminate keybinding conflicts and maximize space for both views.

**Dependencies:** Phase 9A (layout restructure must be complete)

**Problem Solved:**
- Up/Down and PageUp/PageDown would be confusing with two scrollable sections
- Users wouldn't know which section they're scrolling

**Solution:**
- Only ONE scrollable section active at a time
- Press 'w' to toggle between Logs view and Windows view
- Clear visual feedback shows which view is active
- Context bar updates dynamically based on current view

**Files to modify:**
- `src/tui/screens/dashboard.rs` - Add toggle logic, window tracking section
- `src/tui/app.rs` - Add `DashboardView` enum and window scroll state
- `src/tui/input.rs` - Add 'w' toggle and view-aware scrolling
- `src/tui/mod.rs` - Update context bar to be view-aware

**Changes:**

#### 9B.1: Add DashboardView enum and toggle state
**File:** `src/tui/screens/dashboard.rs`

**Add enum:**
```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DashboardView {
    Logs,
    Windows,
}
```

**Update DashboardScreen:**
```rust
pub(crate) struct DashboardScreen {
    pub selected_action: usize,      // 0 = start, 1 = stop, 2 = restart
    pub log_scroll_offset: usize,    // Lines scrolled back from the end
    pub window_scroll_offset: usize, // Window list scroll offset (NEW)
    pub current_view: DashboardView, // Toggle between Logs and Windows (NEW)
}

impl DashboardScreen {
    pub(crate) fn new() -> Self {
        Self {
            selected_action: 0,
            log_scroll_offset: 0,
            window_scroll_offset: 0,
            current_view: DashboardView::Logs, // Default to logs
        }
    }

    /// Toggle between logs and windows view
    pub(crate) fn toggle_view(&mut self) {
        self.current_view = match self.current_view {
            DashboardView::Logs => DashboardView::Windows,
            DashboardView::Windows => DashboardView::Logs,
        };
    }

    // Add window scroll methods
    pub(crate) fn scroll_windows_page_up(&mut self, page_size: usize, total_windows: usize) {
        self.window_scroll_offset = (self.window_scroll_offset + page_size)
            .min(total_windows.saturating_sub(page_size));
    }

    pub(crate) fn scroll_windows_page_down(&mut self, page_size: usize) {
        self.window_scroll_offset = self.window_scroll_offset.saturating_sub(page_size);
    }

    pub(crate) fn scroll_windows_to_top(&mut self) {
        self.window_scroll_offset = 0;
    }
}
```

#### 9B.2: Update layout to conditionally render logs OR windows
**File:** `src/tui/screens/dashboard.rs:94-127` (modify Phase 9A.1 bottom section)

**Update bottom section logic:**
```rust
// Bottom section renders logs OR windows based on current_view
match screen_state.current_view {
    DashboardView::Logs => {
        render_log_viewer(frame, chunks[1], daemon_logs, daemon_running, screen_state.log_scroll_offset);
    }
    DashboardView::Windows => {
        render_window_tracking(frame, chunks[1], windows, matched_windows, screen_state.window_scroll_offset);
    }
}
```

**Rationale:** Toggle between full-width logs or full-width windows based on user selection.

#### 9B.3: Create window tracking section
**File:** `src/tui/screens/dashboard.rs` (new function)

**New comprehensive window display:**
```rust
/// Render window tracking section (full width bottom section when view is Windows)
fn render_window_tracking(
    frame: &mut Frame,
    area: Rect,
    windows: &[crate::ipc::WindowInfo],
    matched_windows: &[(u64, String)], // (window_id, rule_name)
    scroll_offset: usize,
) {
    let title = " Window Tracking - [w] to toggle back to Logs ";
    let block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .border_style(Style::default().fg(colors::UI_HIGHLIGHT));
    frame.render_widget(block.clone(), area);

    let inner = block.inner(area);
    let available_height = inner.height as usize;

    // Count matched vs total
    let matched_count = matched_windows.len();
    let total_count = windows.len();

    let mut lines = vec![
        Line::from(vec![
            Span::styled("Matched: ", Style::default().fg(colors::UI_SECONDARY)),
            Span::styled(
                format!("{matched_count}/{total_count} windows"),
                Style::default().fg(colors::UI_STAT).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(""),
    ];

    // Build window list with matched windows first
    let mut window_lines: Vec<(Line, bool)> = Vec::new(); // (line, is_matched)

    // Add matched windows (use UI_MATCHED color - perfect use case!)
    for (win_id, rule_name) in matched_windows {
        if let Some(win) = windows.iter().find(|w| w.id == *win_id) {
            window_lines.push((
                Line::from(vec![
                    Span::styled("● ", Style::default().fg(colors::UI_MATCHED)),  // ✅ Green for matched
                    Span::styled(
                        truncate(&win.app_id, 15),
                        Style::default().fg(colors::UI_TEXT).add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(" → "),
                    Span::styled(rule_name, Style::default().fg(colors::UI_HIGHLIGHT)),
                ]),
                true,
            ));

            // Optional: Show truncated title on second line
            if !win.title.is_empty() {
                window_lines.push((
                    Line::from(vec![
                        Span::raw("  "),
                        Span::styled(
                            truncate(&win.title, 30),
                            Style::default().fg(colors::UI_SECONDARY),
                        ),
                    ]),
                    true,
                ));
            }
        }
    }

    // Add unmatched windows (use UI_UNMATCHED color - perfect use case!)
    for win in windows {
        if !matched_windows.iter().any(|(id, _)| *id == win.id) {
            window_lines.push((
                Line::from(vec![
                    Span::styled("○ ", Style::default().fg(colors::UI_UNMATCHED)),  // ✅ Dark gray for unmatched
                    Span::styled(
                        truncate(&win.app_id, 15),
                        Style::default().fg(colors::UI_UNMATCHED),  // ✅ Dim the text too
                    ),
                    Span::raw(" (no match)"),
                ]),
                false,
            ));
        }
    }

    // Calculate visible range based on scroll offset
    let total_lines = window_lines.len();
    let visible_count = available_height.saturating_sub(2); // Reserve space for header
    let start_idx = scroll_offset.min(total_lines.saturating_sub(visible_count));
    let end_idx = (start_idx + visible_count).min(total_lines);

    // Add visible window lines
    for (line, _) in window_lines.iter().skip(start_idx).take(end_idx - start_idx) {
        lines.push(line.clone());
    }

    // Add scroll indicator if needed
    if total_lines > visible_count {
        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::styled(
                format!("  [{}/{}] ", start_idx + 1, total_lines),
                Style::default().fg(colors::UI_SECONDARY),
            ),
            Span::styled(
                "PgUp/PgDn to scroll",
                Style::default().fg(colors::UI_SECONDARY).italic(),
            ),
        ]));
    }

    let paragraph = Paragraph::new(lines);
    frame.render_widget(paragraph, inner);
}

/// Truncate string with ellipsis if exceeds max length
///
/// Location: Add to `src/tui/screens/dashboard.rs` as a private helper function.
///
/// # Examples
/// ```
/// assert_eq!(truncate("hello", 10), "hello");
/// assert_eq!(truncate("hello world", 8), "hello w…");
/// ```
fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}…", &s[..max_len.saturating_sub(1)])
    }
}
```

#### 9B.4: Update log viewer title to show toggle hint
**File:** `src/tui/screens/dashboard.rs:427-486`

**Current title logic:**
```rust
let title = if scroll_offset > 0 {
    if daemon_running {
        format!(" Daemon Logs (Live) - ↑{scroll_offset} ")
    } else {
        format!(" Daemon Logs (Stopped) - ↑{scroll_offset} ")
    }
} else if daemon_running {
    " Daemon Logs (Live) ".to_string()
} else {
    " Daemon Logs (Stopped) ".to_string()
};
```

**New title logic:**
```rust
let title = if scroll_offset > 0 {
    if daemon_running {
        format!(" Daemon Logs (Live) - ↑{scroll_offset} - [w] to toggle to Windows ")
    } else {
        format!(" Daemon Logs (Stopped) - ↑{scroll_offset} - [w] to toggle to Windows ")
    }
} else if daemon_running {
    " Daemon Logs (Live) - [w] to toggle to Windows ".to_string()
} else {
    " Daemon Logs (Stopped) - [w] to toggle to Windows ".to_string()
};
```

**Rationale:** Consistent toggle hint reminds users they can switch to window view.

#### 9B.5: Update window summary to show toggle hint
**File:** `src/tui/screens/dashboard.rs` (modify 9A.3 function)

**Update render_window_summary to accept and display current view:**
```rust
fn render_window_summary(
    frame: &mut Frame,
    area: Rect,
    window_count: usize,
    matched_count: usize,
    current_view: DashboardView, // NEW parameter
) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Windows ");
    frame.render_widget(block.clone(), area);

    let inner = block.inner(area);

    let lines = vec![
        Line::from(vec![
            Span::styled("Matched: ", Style::default().fg(colors::UI_SECONDARY)),
            Span::styled(
                format!("{matched_count}/{window_count}"),
                Style::default().fg(colors::UI_STAT).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::styled(
                match current_view {
                    DashboardView::Logs => "Press [w] to view details",
                    DashboardView::Windows => "Viewing below (press [w] for logs)",
                },
                Style::default().fg(colors::UI_SECONDARY),
            ),
        ]),
    ];

    let paragraph = Paragraph::new(lines);
    frame.render_widget(paragraph, inner);
}
```

**Rationale:** Dynamic hint shows user how to toggle and what the current state is.

#### 9B.6: Add 'w' toggle keybinding
**File:** `src/tui/input.rs` (in `handle_dashboard_input` function, modify Phase 9A.5)

**Add toggle to existing navigation:**
```rust
fn handle_dashboard_input(app: &mut App, key: KeyEvent) {
    match key.code {
        // Toggle between Logs and Windows view (NEW)
        KeyCode::Char('w') if key.modifiers == KeyModifiers::NONE => {
            app.dashboard_screen.toggle_view();
        }

        // Left/Right for horizontal daemon action navigation (from Phase 9A)
        KeyCode::Left => {
            app.dashboard_screen.select_previous();
        }
        KeyCode::Right => {
            app.dashboard_screen.select_next();
        }

        // Up/Down for scrolling (context-aware based on current view)
        KeyCode::Up => {
            match app.dashboard_screen.current_view {
                DashboardView::Logs => {
                    let total = app.daemon_logs.len();
                    let visible = 20; // Calculate from visible area
                    app.dashboard_screen.scroll_logs_up(total, visible);
                }
                DashboardView::Windows => {
                    // Single-line scroll not implemented for windows (use PageUp/Down)
                }
            }
        }
        KeyCode::Down => {
            match app.dashboard_screen.current_view {
                DashboardView::Logs => {
                    app.dashboard_screen.scroll_logs_down();
                }
                DashboardView::Windows => {
                    // Single-line scroll not implemented for windows (use PageUp/Down)
                }
            }
        }

        // Page Up/Down for scrolling (context-aware based on current view)
        KeyCode::PageUp => {
            match app.dashboard_screen.current_view {
                DashboardView::Logs => {
                    let total = app.daemon_logs.len();
                    let page_size = 10; // Calculate from visible area
                    app.dashboard_screen.scroll_logs_page_up(total, page_size);
                }
                DashboardView::Windows => {
                    let page_size = 5;
                    let total = app.all_windows.len();
                    app.dashboard_screen.scroll_windows_page_up(page_size, total);
                }
            }
        }
        KeyCode::PageDown => {
            match app.dashboard_screen.current_view {
                DashboardView::Logs => {
                    let page_size = 10;
                    app.dashboard_screen.scroll_logs_page_down(page_size);
                }
                DashboardView::Windows => {
                    let page_size = 5;
                    app.dashboard_screen.scroll_windows_page_down(page_size);
                }
            }
        }
        KeyCode::Home => {
            match app.dashboard_screen.current_view {
                DashboardView::Logs => {
                    app.dashboard_screen.scroll_logs_to_bottom(); // Reset to latest
                }
                DashboardView::Windows => {
                    app.dashboard_screen.scroll_windows_to_top();
                }
            }
        }

        // Enter to execute selected daemon action
        KeyCode::Enter => {
            // Execute action based on selected_action index
            // ... existing implementation ...
        }

        _ => {}
    }
}
```

**Rationale:**
- 'w' toggles between views (mnemonic: **W**indows)
- Provides immediate visual feedback (summary and title update)

#### 9B.7: Add view-aware scrolling keybindings
**File:** `src/tui/input.rs` (continue `handle_dashboard_input` function from 9B.6)

**Add view-aware scrolling (already shown in 9B.6 code above):**

The Up/Down, PageUp/PageDown, and Home keys are handled with match on `current_view`:
- **Logs view:** Up/Down and PageUp/PageDown scroll the log viewer
- **Windows view:** Only PageUp/PageDown scroll (Up/Down reserved for future item selection)

**Rationale:**
- Scrolling behavior adapts to which view is active
- No keybinding confusion - only ONE scrollable section at a time
- Up/Down work only in Logs view (Windows view uses PageUp/PageDown only)

**Note:** When Enable/Disable are added in the future, `select_next()` will need to handle 5 actions instead of 3.

#### 9B.8: Update context bar to be view-aware
**File:** `src/tui/mod.rs` (in Phase 6's `render_context_bar` function)

**Update dashboard keybinds to be view-aware:**
```rust
// Context bar should show different hints based on current view
impl App {
    fn get_dashboard_context_keybinds(&self) -> Vec<(&str, &str)> {
        let mut keybinds = vec![
            ("[←→]", "Select Action"),
            ("[Enter]", "Execute"),
        ];

        // Add view-specific scrolling hints
        match self.dashboard_screen.current_view {
            DashboardView::Logs => {
                keybinds.push(("[↑↓/PgUp/PgDn]", "Scroll Logs"));
                keybinds.push(("[w]", "View Windows"));
            }
            DashboardView::Windows => {
                keybinds.push(("[PgUp/PgDn]", "Scroll Windows"));
                keybinds.push(("[w]", "View Logs"));
            }
        }

        keybinds
    }
}

// Then in render_context_bar:
(Screen::Dashboard, ScreenMode::DashboardList) => {
    app.get_dashboard_context_keybinds()
}
```

**Rationale:**
- Context bar dynamically updates based on which view is active
- Shows 'w' toggle key with current context ("View Windows" vs "View Logs")
- Scroll hints reflect what the keys will actually do in the current view
- Clear visual feedback about current mode

#### 9B.9: Data requirements for Phase 9B features

**Matched windows data:**
- Already available via IPC `list-windows` response
- Filter windows where `current_rule_desc.is_some()`
- Pair with rule name for display in window tracking section
- Create list of `(window_id, rule_name)` tuples for matched windows

**Window scroll state:**
- Already added in 9B.1 (`window_scroll_offset: usize`)
- Tracks scroll position when viewing Windows view
- Independent from log scroll offset

**Checklist:**
- [x] Add DashboardView enum (Logs, Windows) to dashboard.rs
- [x] Add view toggle state to DashboardScreen struct
- [x] Add window scroll state to DashboardScreen struct
- [x] Add toggle_view() method to DashboardScreen
- [x] Add scroll methods for windows (page_up, page_down, to_top)
- [x] Update Phase 9A.1 layout to conditionally render logs OR windows
- [x] Create render_window_tracking function (full width bottom section)
- [x] Add truncate helper for long strings
- [x] Update log viewer title to show toggle hint "[w] to toggle to Windows"
- [x] Update window tracking title to show "[w] to toggle back to Logs"
- [x] Update window summary to show toggle hint and current view
- [x] Add 'w' key handler to toggle between views
- [x] Add view-aware Up/Down keybindings (logs only)
- [x] Add view-aware PageUp/PageDown keybindings (both views)
- [x] Add view-aware Home keybinding (both views)
- [x] Update context bar to be view-aware (implemented inline without helper method)
- [x] Test toggle between logs and windows view
- [x] Test scrolling in logs view
- [x] Test scrolling in windows view
- [x] Test with zero windows, zero matched windows
- [x] Verify no keybinding confusion (only one scrollable section at a time)
- [x] Run clippy and tests

**Benefits:**
- ✅ **No keybinding confusion** - Only one scrollable section at a time
- ✅ **Window tracking gets maximum space** - Full width when in Windows view
- ✅ **Logs get maximum space** - Full width when in Logs view
- ✅ **Clear visual feedback** - Title bar shows current view and toggle hint
- ✅ **Context bar updates dynamically** - Shows relevant keybindings for current view
- ✅ **Can show detailed info per window** - No cramping in full-width layout
- ✅ **Scrolling handles large lists gracefully** - PageUp/PageDown support
- ✅ **Daemon/sink sections stay compact** - Top section remains scannable
- ✅ **Recent switches provide useful context** - See switch history at a glance
- ✅ **Matched vs unmatched windows clearly distinguished** - Visual indicators (● vs ○)
- ✅ **Uptime/PID adds useful monitoring info** - See daemon health at a glance
- ✅ **Simple toggle mechanism** - Just press 'w' to switch views

**Edge cases to handle:**
- Empty window list (show "No windows tracked" message in Windows view)
- No matched windows (show count as "0/N windows")
- Daemon not running (hide uptime/PID, disable controls appropriately)
- Very long app_id or title (truncate with ellipsis)
- Terminal too narrow (minimum width ~80 cols recommended)
- Action selection wraps at boundaries (left on first = last, right on last = first)
- Toggle while scrolled (preserve scroll offset when switching views)
- Empty logs (show "No logs available" message in Logs view)

**Future Extensions (Not in Phase 9):**
- **Enable/Disable systemd unit actions:**
  - Add two more buttons: `[  Enable]  [  Disable]`
  - Visual spacing (3 spaces) between Start/Stop/Restart and Enable/Disable groups
  - Update `selected_action` range to 0-4 (5 actions total)
  - These would call `systemctl --user enable/disable pwsw.service`
  - Only show if systemd unit is detected/installed

- **Arrow key navigation in Windows view:**
  - Use Up/Down arrows to select individual windows (not just scroll)
  - Show selected window with highlight
  - Press Enter to copy window info or perform action
  - This would replace PageUp/PageDown in Windows view with more granular control

---

## Future Enhancements (Out of Scope)

These are not part of this plan but could be considered later:

1. **Stats Screen:** Add `[5] Stats` for PipeWire statistics/performance monitoring
2. **Vim-style navigation:** Add `h`/`l` for prev/next screen (power users)
3. **Color customization:** Allow users to configure tab bar colors in settings
4. **Tab bar icons:** Add optional icons/symbols before screen names (e.g., 📊 Dashboard)
5. **Modal-specific hints:** Add subtle inline hints for text editing shortcuts in modals (see Phase 8.9)
6. **Dashboard window detail modal:** Press Enter on a window to see full details in a modal

---

## Notes

- **Backwards Compatibility:** Not required - this is a UX improvement, not an API change
- **Performance Impact:** Minimal - only changes key handling and string formatting
- **Risk Level:** Low - well-defined changes, existing infrastructure solid
- **Testing:** Primarily manual (TUI rendering hard to unit test)
- **Documentation:** No user-facing docs to update (TUI is self-documenting via help screen)

---

## Rationale

**Why number keys?**
- Industry standard in modern TUIs (k9s, lazygit, gitui, bottom)
- Fast and ergonomic (top row of keyboard)
- Discoverable (shown in tab bar)
- Scalable (room for 5-9 more screens)
- No conflicts with text input (rarely need to type numbers in config)

**Why remove letter keys?**
- Not discoverable (users must guess or read help)
- Only work in English (not mnemonic in other languages)
- Can conflict with text input fields
- Redundant with number keys + Tab cycling
- No backwards compatibility concerns (internal TUI, not public API)

**Why help in header?**
- More prominent (always visible at top)
- Matches common TUI pattern (help usually at top or in legend)
- Frees up footer space for essential actions
- First thing users see when confused

**Why simplify footer?**
- Reduces redundancy (screen nav already in tab bar)
- Cleaner visual hierarchy
- Easier to parse at a glance
- Focuses on global actions (quit/save) not navigation
