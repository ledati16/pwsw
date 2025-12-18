# TUI Navigation Redesign Plan

**Goal:** Modernize TUI navigation to use number keys (1-4) instead of letters (d, s, r, t), improve discoverability, and reduce redundancy.

**Status:** Planning phase - not yet implemented

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

## Implementation Phases

### Phase 1: Update Screen Keybindings

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
- [ ] Modify `Screen::key()` to return numbers
- [ ] Replace letter handlers with number handlers in `input.rs`
- [ ] Test all number keys work (1-4)
- [ ] Verify letter keys no longer work (d, s, r, t)

---

### Phase 2: Update Header Tab Bar

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
- Help indicator should be cyan/styled to match `?` key prominence
- Should always be visible (not dependent on `show_help` state)
- Acts as visual reminder that `?` opens help

**Checklist:**
- [ ] Update tab title format to `[1] Dashboard` (with space)
- [ ] Add `[?] Help` indicator to right side of header
- [ ] Style help indicator (cyan or matching theme)
- [ ] Verify header renders correctly at different terminal widths

---

### Phase 3: Simplify Footer

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
    Span::styled("[Tab/Shift-Tab]", Style::default().fg(Color::Cyan)),
    Span::raw(" Cycle  "),
    Span::styled("Ctrl+S", Style::default().fg(Color::Green)),
    Span::raw(" Save"),
])
```

**Rationale:**
- Remove `?` Help - now in header (prominent)
- Remove d/s/r/t navigation - redundant with tab bar, replaced by numbers
- Keep `q` Quit - essential global action
- Keep `Tab/Shift-Tab` - alternative cycling method (not redundant with numbers)
- Keep `Ctrl+S` Save - essential global action

**Optional enhancement:**
Add conditional text for config state (when not showing status message):
- If `config_dirty == true`: Show "Ctrl+S Save*" with yellow asterisk
- Makes unsaved state more prominent at bottom too

**Checklist:**
- [ ] Simplify footer to show only: Quit, Tab cycling, Save
- [ ] Remove all screen navigation references (?, d, s, r, t)
- [ ] Verify footer is readable and not cluttered
- [ ] Optional: Add visual indicator for unsaved changes

---

### Phase 4: Update Help Screen

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

**Checklist:**
- [ ] Update help documentation for all screen navigation keys
- [ ] Verify help modal shows correct keys (1-4 not d/s/r/t)
- [ ] Test help screen in both wide and narrow terminal modes

---

### Phase 5: Update Tests

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
- [ ] Search for KeyCode::Char('d'|'s'|'r'|'t') in test files
- [ ] Update all navigation test key presses to use numbers
- [ ] Run `cargo test` to verify all tests pass
- [ ] Run `bash scripts/verify_tests_safe.sh` to ensure test safety

---

## Verification Checklist

Before considering this plan complete, verify:

**Functionality:**
- [ ] All number keys (1-4) switch to correct screens
- [ ] Tab/Shift-Tab still cycle through screens
- [ ] Help overlay opens with `?` key
- [ ] Letter keys (d, s, r, t) no longer work
- [ ] Footer shows simplified global actions only
- [ ] Header shows `[?] Help` indicator on right

**Visual:**
- [ ] Tab bar shows `[1] Dashboard` format (with space)
- [ ] Help indicator visible in header at all times
- [ ] Footer is clean and readable (not cluttered)
- [ ] All styling/colors consistent with existing theme

**Testing:**
- [ ] All existing tests pass
- [ ] Manual testing in various terminal sizes
- [ ] No clippy warnings introduced
- [ ] Help screen documentation accurate

**Polish:**
- [ ] Terminal width edge cases handled gracefully
- [ ] No visual glitches when resizing terminal
- [ ] Status messages still display correctly in footer
- [ ] Config dirty state (`[unsaved]`) still shows in header

---

### Phase 6: Add Context Bars

**Goal:** Add dedicated context bars below each screen showing relevant keybindings, remove redundant navigation from global footer.

**Files to modify:**
- `src/tui/mod.rs` - Add context bar rendering, update footer
- `src/tui/screens/sinks.rs` - Remove title keybindings
- `src/tui/screens/rules.rs` - Remove title keybindings
- `src/tui/screens/settings.rs` - Remove title keybindings
- `src/tui/screens/dashboard.rs` - Remove title keybindings (if any)

**Changes:**

#### 6.1: Add context bar rendering function
**File:** `src/tui/mod.rs`

**New function to add:**
```rust
/// Render context-sensitive keybinding bar below screen content
fn render_context_bar(
    frame: &mut ratatui::Frame,
    area: Rect,
    current_screen: Screen,
    mode: &ScreenMode, // Enum representing current mode (List, Edit, Delete, etc.)
) {
    use ratatui::text::{Line, Span};
    use ratatui::style::{Style, Color};

    let keybinds = match (current_screen, mode) {
        (Screen::Dashboard, _) => vec![
            ("[↑↓]", "Select"),
            ("[Enter]", "Execute"),
        ],
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
        spans.push(Span::styled(*key, Style::default().fg(Color::Cyan)));
        spans.push(Span::raw(" "));
        spans.push(Span::raw(*desc));
    }

    let line = Line::from(spans);
    let paragraph = Paragraph::new(line);
    frame.render_widget(paragraph, area);
}
```

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
render_context_bar(frame, chunks[2], app.current_screen, &app.get_mode());

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

**Note:** Need to add `get_mode()` helper to `App` that returns current mode for each screen.

#### 6.4: Update global footer to remove redundant navigation
**File:** `src/tui/mod.rs:733-747`

**Current:**
```rust
Line::from(vec![
    Span::raw("[q] Quit  "),
    Span::styled("[Tab/Shift-Tab]", Style::default().fg(Color::Cyan)),
    Span::raw(" Cycle  "),
    Span::styled("Ctrl+S", Style::default().fg(Color::Green)),
    Span::raw(" Save"),
])
```

**New:**
```rust
Line::from(vec![
    Span::raw("[q] Quit  "),
    Span::styled("[↑↓]", Style::default().fg(Color::Cyan)),
    Span::raw(" Navigate  "),
    Span::styled("[Tab/Shift-Tab]", Style::default().fg(Color::Cyan)),
    Span::raw(" Cycle  "),
    Span::styled("Ctrl+S", Style::default().fg(Color::Green)),
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
- [ ] Add `render_context_bar()` function to `mod.rs`
- [ ] Update layout to include context bar (4 chunks instead of 3)
- [ ] Add `App::get_mode()` helper method
- [ ] Call context bar renderer in `render_ui()`
- [ ] Update global footer to include `[↑↓] Navigate`
- [ ] Clean up all screen titles (remove keybinding hints)
- [ ] Remove inline help lines from editor modals
- [ ] Test context bar appears on all screens
- [ ] Test context bar changes based on mode (List vs Edit)
- [ ] Verify footer shows global actions only

---

### Phase 7: Add Move/Reorder Functionality

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
- [ ] Add `move_up()` and `move_down()` to RulesScreen
- [ ] Add `move_up()` and `move_down()` to SinksScreen
- [ ] Add Shift+Up/Down handlers to Rules input
- [ ] Add Shift+Up/Down handlers to Sinks input
- [ ] Ensure config_dirty flag set when items moved
- [ ] Update help screen documentation
- [ ] Test moving rules up/down in list
- [ ] Test moving sinks up/down in list
- [ ] Test boundary conditions (first/last item)
- [ ] Verify selection follows moved item
- [ ] Verify Ctrl+S saves reordered config correctly

**Notes:**
- Moving items sets `config_dirty` flag (requires save)
- Selection cursor follows the moved item
- Boundary checks prevent moving beyond list edges
- For sinks, `update_display_descs()` must be called after swap

---

### Phase 8: Standardize Modal Keybinding Display

**Goal:** Bring consistency to how modals show keybindings, remove keybinds from titles, and leverage context bar for modal-specific hints.

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
        (Screen::Sinks, ScreenMode::SinkSelector) => vec![
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
    SinkSelector,
    SinkDeleteConfirm,

    // Rule modal modes
    RuleEditor,
    RuleSinkSelector,
    RuleDeleteConfirm,
}

impl App {
    pub(crate) fn get_mode(&self) -> ScreenMode {
        match self.current_screen {
            Screen::Dashboard => ScreenMode::DashboardList,
            Screen::Sinks => match self.sinks_screen.mode {
                SinksMode::List => ScreenMode::SinksList,
                SinksMode::AddEdit => ScreenMode::SinkEditor,
                SinksMode::SelectSink => ScreenMode::SinkSelector,
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
- [ ] Extend `render_context_bar()` to handle modal modes
- [ ] Add `ScreenMode` enum and `App::get_mode()` implementation
- [ ] Clean up sink selector modal title (remove keybinds)
- [ ] Clean up rule sink selector modal title (remove keybinds)
- [ ] Remove inline help from sink editor
- [ ] Remove inline help from rule editor
- [ ] Update sink editor layout (remove help chunk)
- [ ] Update rule editor layout (remove help chunk)
- [ ] Remove keybind text from delete confirmation modals
- [ ] Add text editing shortcuts section to help screen
- [ ] Test context bar appears correctly for all modal modes
- [ ] Test modals on small terminals (no help text overflow)
- [ ] Verify all modal keybindings work as documented
- [ ] Run clippy and tests

**Benefits:**
- ✅ Consistent keybinding display across all modals
- ✅ No lost information on small terminals
- ✅ Clean, descriptive modal titles
- ✅ Users discover powerful text editing shortcuts
- ✅ Aligns with overall plan's context bar approach
- ✅ Reduces code duplication (no inline help rendering)

---

### Phase 9: Dashboard Layout Redesign

**Goal:** Reorganize dashboard to maximize information density, giving window tracking the space it needs while keeping daemon/sink sections compact.

**Current Layout Issues:**

1. **Daemon section too wide:** Takes full width (100%) but only uses ~40% effectively
2. **Sink card too sparse:** Shows only icon + name, wastes vertical space
3. **Stats card underutilized:** Shows only window count, could show much more detail
4. **No scrolling:** Can't show all windows if list is long

**Files to modify:**
- `src/tui/screens/dashboard.rs` - Complete layout restructure
- `src/tui/app.rs` - Add window scroll state to `DashboardScreen`
- `src/tui/input.rs` - Add Page Up/Down handlers for dashboard

**Proposed Layout:**

```
┌──────────────────────────┬─────────────────────────────────┐
│  Daemon Control          │                                 │
│  Status: ● RUNNING       │                                 │
│  Uptime: 2h 34m          │                                 │
│  PID: 12345              │                                 │
│  [▶ Start ] Stop Restart │    Window Tracking              │
│  Height: 6 lines         │    (Scrollable w/ PgUp/PgDn)    │
├──────────────────────────┤                                 │
│  Active Sink             │    Matched: 3/12 windows        │
│  🎧 Headphones           │                                 │
│  Recent Switches:        │    ● firefox → Headphones       │
│  10:30 → Headphones (rule)│    ● mpv → Speakers            │
│  10:25 → Speakers (manual)│    ○ discord (no match)        │
│  Height: 8 lines         │                                 │
└──────────────────────────┴─────────────────────────────────┘
┌───────────────────────────────────────────────────────────┐
│  Daemon Logs (Live) - [↑↓] scroll [PgUp/PgDn] page       │
│  10:30:15 INFO Rule matched: app_id=firefox → Headphones  │
│  Height: Remaining space (Min 0, expands)                 │
└───────────────────────────────────────────────────────────┘
```

**Changes:**

#### 9.1: Update main layout structure
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
        Constraint::Length(14), // Top section (daemon + sink + windows)
        Constraint::Min(0),     // Daemon logs (expands)
    ])
    .split(area);

// Split top section horizontally (left: daemon+sink, right: windows)
let top_chunks = Layout::default()
    .direction(Direction::Horizontal)
    .constraints([
        Constraint::Percentage(50), // Left column
        Constraint::Percentage(50), // Right column (windows)
    ])
    .split(chunks[0]);

// Split left column vertically (daemon above, sink below)
let left_chunks = Layout::default()
    .direction(Direction::Vertical)
    .constraints([
        Constraint::Length(6),  // Daemon control
        Constraint::Length(8),  // Active sink + history
    ])
    .split(top_chunks[0]);
```

**Rationale:** Hybrid layout gives window tracking maximum vertical space while keeping daemon/sink compact.

#### 9.2: Redesign daemon section (compact)
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
```rust
/// Format duration as human-readable string (e.g., "2h 34m")
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

#### 9.3: Enhance sink section with switch history
**File:** `src/tui/screens/dashboard.rs:208-253`

**Current:** Shows only current sink icon + description

**New:** Add recent switch history
```rust
fn render_sink_card(
    frame: &mut Frame,
    area: Rect,
    config: &Config,
    recent_switches: &[(String, String, String)], // New param: (timestamp, sink_desc, reason)
) {
    let current_sink_name = crate::pipewire::PipeWire::get_default_sink_name().ok();

    let (sink_desc, sink_icon) = current_sink_name
        .as_ref()
        .and_then(|name| {
            config.sinks.iter().find(|s| &s.name == name).map(|s| {
                (
                    s.desc.clone(),
                    s.icon.clone().unwrap_or_else(|| "🔊".to_string()),
                )
            })
        })
        .unwrap_or(("Unknown".to_string(), "?".to_string()));

    let mut lines = vec![
        Line::from(vec![
            Span::styled(sink_icon, Style::default().fg(colors::UI_HIGHLIGHT)),
            Span::raw(" "),
            Span::styled(
                sink_desc,
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

**Note:** Recent switches data structure needs to be added to `App` state (TBD).

#### 9.4: Create new window tracking section
**File:** `src/tui/screens/dashboard.rs` (new function)

**New comprehensive window display:**
```rust
/// Render window tracking section (right side, full height)
fn render_window_tracking(
    frame: &mut Frame,
    area: Rect,
    windows: &[crate::ipc::WindowInfo],
    matched_windows: &[(u64, String)], // (window_id, rule_name)
    scroll_offset: usize,
    screen_state: &DashboardScreen,
) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Window Tracking ");
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

    // Add matched windows
    for (win_id, rule_name) in matched_windows {
        if let Some(win) = windows.iter().find(|w| w.id == *win_id) {
            window_lines.push((
                Line::from(vec![
                    Span::styled("● ", Style::default().fg(colors::UI_SUCCESS)),
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

    // Add unmatched windows
    for win in windows {
        if !matched_windows.iter().any(|(id, _)| *id == win.id) {
            window_lines.push((
                Line::from(vec![
                    Span::styled("○ ", Style::default().fg(colors::UI_SECONDARY)),
                    Span::styled(
                        truncate(&win.app_id, 15),
                        Style::default().fg(colors::UI_SECONDARY),
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

/// Truncate string with ellipsis if too long
fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}…", &s[..max_len.saturating_sub(1)])
    }
}
```

#### 9.5: Add window scroll state to DashboardScreen
**File:** `src/tui/screens/dashboard.rs:29-33`

**Current:**
```rust
pub(crate) struct DashboardScreen {
    pub selected_action: usize,   // 0 = start, 1 = stop, 2 = restart
    pub log_scroll_offset: usize, // Lines scrolled back from the end
}
```

**New:**
```rust
pub(crate) struct DashboardScreen {
    pub selected_action: usize,      // 0 = start, 1 = stop, 2 = restart
    pub log_scroll_offset: usize,    // Lines scrolled back from the end
    pub window_scroll_offset: usize, // Windows list scroll offset
}
```

**Add scroll methods:**
```rust
impl DashboardScreen {
    pub(crate) fn new() -> Self {
        Self {
            selected_action: 0,
            log_scroll_offset: 0,
            window_scroll_offset: 0,
        }
    }

    // ... existing methods ...

    /// Scroll windows up (page up)
    pub(crate) fn scroll_windows_page_up(&mut self, page_size: usize, total_windows: usize) {
        self.window_scroll_offset = (self.window_scroll_offset + page_size)
            .min(total_windows.saturating_sub(page_size));
    }

    /// Scroll windows down (page down)
    pub(crate) fn scroll_windows_page_down(&mut self, page_size: usize) {
        self.window_scroll_offset = self.window_scroll_offset.saturating_sub(page_size);
    }

    /// Reset window scroll to top
    pub(crate) fn scroll_windows_to_top(&mut self) {
        self.window_scroll_offset = 0;
    }
}
```

#### 9.6: Add navigation keybindings for dashboard
**File:** `src/tui/input.rs` (in `handle_dashboard_input` function)

**Add horizontal and page-based navigation:**
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

        // Page Up/Down for window list scrolling
        KeyCode::PageUp => {
            let page_size = 5; // Or calculate from visible area
            let total = app.all_windows.len();
            app.dashboard_screen.scroll_windows_page_up(page_size, total);
        }
        KeyCode::PageDown => {
            let page_size = 5;
            app.dashboard_screen.scroll_windows_page_down(page_size);
        }
        KeyCode::Home => {
            app.dashboard_screen.scroll_windows_to_top();
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

**Note:** Left/Right navigation works with the horizontal button layout. When Enable/Disable are added in the future, `select_next()` will need to handle 5 actions instead of 3.

#### 9.7: Update context bar for dashboard
**File:** `src/tui/mod.rs` (in Phase 6's `render_context_bar` function)

**Add dashboard keybinds:**
```rust
(Screen::Dashboard, ScreenMode::DashboardList) => vec![
    ("[←→]", "Select Action"),
    ("[Enter]", "Execute"),
    ("[PgUp/PgDn]", "Scroll Windows"),
],
```

**Rationale:** Left/Right arrows navigate the horizontal daemon action buttons. Page Up/Down scroll the window list.

#### 9.8: Data requirements for new features

**Uptime and PID tracking:**
- Add to `App` state: `daemon_start_time: Option<Instant>`
- Calculate uptime: `Instant::now() - daemon_start_time`
- Get PID from IPC status response (may need to enhance IPC protocol)

**Recent switches history:**
- Add to `App` state: `recent_switches: VecDeque<SwitchEvent>` (max 10 entries)
- `SwitchEvent { timestamp: String, sink_desc: String, reason: String }`
- Update on sink changes (manual or rule-based)

**Matched windows data:**
- Already available via IPC `list-windows` response
- Filter windows where `current_rule_desc.is_some()`
- Pair with rule name for display

**Checklist:**
- [ ] Update main layout to hybrid two-column design
- [ ] Redesign daemon section (compact, vertical)
- [ ] Add uptime and PID display to daemon section
- [ ] Add format_duration helper function
- [ ] Add horizontal action button layout with spacing for future Enable/Disable
- [ ] Enhance sink section with recent switches history
- [ ] Add recent_switches state to App
- [ ] Create render_window_tracking function
- [ ] Add window scroll state to DashboardScreen
- [ ] Add scroll methods (page_up, page_down, to_top)
- [ ] Add Left/Right arrow keybindings for action selection
- [ ] Add PageUp/PageDown/Home keybindings for window scrolling
- [ ] Update context bar for dashboard (show ←→ for actions, PgUp/PgDn for windows)
- [ ] Add truncate helper for long strings
- [ ] Test layout on small terminals (minimum width/height)
- [ ] Test window scrolling with many windows
- [ ] Test with zero windows, zero matched windows
- [ ] Test Left/Right navigation through action buttons
- [ ] Verify daemon controls still work (start/stop/restart)
- [ ] Run clippy and tests

**Benefits:**
- ✅ Window tracking gets maximum space (full right column)
- ✅ Can show detailed info per window without cramping
- ✅ Scrolling handles large window lists gracefully
- ✅ Daemon/sink sections stay compact and scannable
- ✅ Recent switches provide useful context
- ✅ Matched vs unmatched windows clearly distinguished
- ✅ Uptime/PID adds useful monitoring info

**Edge cases to handle:**
- Empty window list (show "No windows tracked" message)
- No matched windows (show count as "0/N windows")
- Daemon not running (hide uptime/PID, disable controls appropriately)
- Very long app_id or title (truncate with ellipsis)
- Terminal too narrow (minimum width ~80 cols recommended)
- Action selection wraps at boundaries (left on first = last, right on last = first)

**Future Extensions (Not in Phase 9):**
- **Enable/Disable systemd unit actions:**
  - Add two more buttons: `[  Enable]  [  Disable]`
  - Visual spacing (3 spaces) between Start/Stop/Restart and Enable/Disable groups
  - Update `selected_action` range to 0-4 (5 actions total)
  - These would call `systemctl --user enable/disable pwsw.service`
  - Only show if systemd unit is detected/installed

---

## Future Enhancements (Out of Scope)

These are not part of this plan but could be considered later:

1. **Logs Screen:** Add `[5] Logs` for daemon log viewer (currently only in Dashboard)
2. **Stats Screen:** Add `[6] Stats` for PipeWire statistics/performance monitoring
3. **Vim-style navigation:** Add `h`/`l` for prev/next screen (power users)
4. **Color customization:** Allow users to configure tab bar colors in settings
5. **Tab bar icons:** Add optional icons/symbols before screen names (e.g., 📊 Dashboard)
6. **Modal-specific hints:** Add subtle inline hints for text editing shortcuts in modals (see Phase 8.9)
7. **Dashboard window focus:** Arrow keys to select/highlight specific windows in the tracking section

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
