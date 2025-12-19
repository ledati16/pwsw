# Sink Card Enhancement Plan

## Current State

The Active Sink card on the dashboard right side (50% width, 10 lines height) is sparse:
- Only shows: icon + sink description + "Active Audio Output" label
- Wastes ~6 lines of vertical space
- No additional context or useful information

## Proposed Solution: Split into Two Cards (Option 1)

Split the right 50% vertically into two cards:
1. **Active Sink card** (6 lines) - Enhanced with technical details
2. **Statistics card** (4 lines) - Quick overview stats

### Visual Mockup

```
┌─ Daemon (6 lines) ─────────┬─ Active Sink (6 lines) ───┐
│ ● RUNNING (enabled)        │ 🔊 Built-in Audio         │
│                            │                           │
│ │ → Start │ Stop │ ...     │ Sample Rate: 48000 Hz     │
│                            │ Format: Float32LE         │
├─ Windows (4 lines) ────────┼─ Statistics (4 lines) ────┤
│ Matched: 3/10              │ Rules: 5 active           │
│ Press [w] to view details  │ Sinks: 3 available        │
└────────────────────────────┴───────────────────────────┘
```

## Implementation Plan

### 1. Update Layout in `render_dashboard`

**File:** `src/tui/screens/dashboard.rs`

**Current code (line 180-209):**
```rust
// Split top section horizontally (left: daemon+summary, right: sink)
let top_chunks = Layout::default()
    .direction(Direction::Horizontal)
    .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
    .split(chunks[0]);

// Split left column vertically (daemon above, summary below)
let left_chunks = Layout::default()
    .direction(Direction::Vertical)
    .constraints([
        Constraint::Length(6), // Daemon control
        Constraint::Length(4), // Window summary
    ])
    .split(top_chunks[0]);

// Render top section components
render_daemon_section(frame, left_chunks[0], ctx.screen_state, ctx.daemon_running);
render_window_summary(frame, left_chunks[1], ...);
render_sink_card(frame, top_chunks[1], ctx.config);
```

**New code:**
```rust
// Split top section horizontally (left: daemon+summary, right: sink+stats)
let top_chunks = Layout::default()
    .direction(Direction::Horizontal)
    .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
    .split(chunks[0]);

// Split left column vertically (daemon above, summary below)
let left_chunks = Layout::default()
    .direction(Direction::Vertical)
    .constraints([
        Constraint::Length(6), // Daemon control
        Constraint::Length(4), // Window summary
    ])
    .split(top_chunks[0]);

// Split right column vertically (sink above, stats below)
let right_chunks = Layout::default()
    .direction(Direction::Vertical)
    .constraints([
        Constraint::Length(6), // Active sink details
        Constraint::Length(4), // Statistics
    ])
    .split(top_chunks[1]);

// Render top section components
render_daemon_section(frame, left_chunks[0], ctx.screen_state, ctx.daemon_running);
render_window_summary(frame, left_chunks[1], ...);
render_sink_card(frame, right_chunks[0], ctx.config);
render_statistics_card(frame, right_chunks[1], ctx.config, matched_count, ctx.window_count);
```

### 2. Enhance `render_sink_card`

**File:** `src/tui/screens/dashboard.rs` (lines 401-447)

**Add technical details from PipeWire node info:**

```rust
fn render_sink_card(frame: &mut Frame, area: Rect, config: &Config) {
    let current_sink_name = crate::pipewire::PipeWire::get_default_sink_name().ok();

    // Get node details from pw-dump
    let (sink_desc, sink_icon, sample_rate, format) = current_sink_name
        .as_ref()
        .and_then(|name| {
            // Find sink in config
            let sink = config.sinks.iter().find(|s| &s.name == name)?;

            // Get technical details from PipeWire (new helper function needed)
            let node_info = crate::pipewire::PipeWire::get_sink_info(name).ok()?;

            Some((
                sink.desc.clone(),
                sink.icon.clone().unwrap_or_else(|| "🔊".to_string()),
                node_info.sample_rate,
                node_info.format,
            ))
        })
        .unwrap_or((
            "Unknown Sink".to_string(),
            "?".to_string(),
            "Unknown".to_string(),
            "Unknown".to_string(),
        ));

    let text = vec![
        Line::from(vec![
            Span::styled(sink_icon, Style::default().fg(colors::UI_HIGHLIGHT)),
            Span::raw(" "),
            Span::styled(
                sink_desc,
                Style::default()
                    .fg(colors::UI_TEXT)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("Sample Rate: ", Style::default().fg(colors::UI_SECONDARY)),
            Span::styled(sample_rate, Style::default().fg(colors::UI_TEXT)),
        ]),
        Line::from(vec![
            Span::styled("Format: ", Style::default().fg(colors::UI_SECONDARY)),
            Span::styled(format, Style::default().fg(colors::UI_TEXT)),
        ]),
    ];

    let paragraph = Paragraph::new(text)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Active Sink ")
                .border_style(Style::default().fg(colors::UI_HIGHLIGHT)),
        )
        .alignment(Alignment::Left); // Changed from Center to Left

    frame.render_widget(paragraph, area);
}
```

### 3. Create `render_statistics_card`

**File:** `src/tui/screens/dashboard.rs` (new function after `render_sink_card`)

```rust
/// Render statistics card showing quick overview
fn render_statistics_card(
    frame: &mut Frame,
    area: Rect,
    config: &Config,
    matched_count: usize,
    window_count: usize,
) {
    let rule_count = config.rules.len();
    let sink_count = config.sinks.len();

    let lines = vec![
        Line::from(vec![
            Span::styled("Rules: ", Style::default().fg(colors::UI_SECONDARY)),
            Span::styled(
                format!("{rule_count} active"),
                Style::default()
                    .fg(colors::UI_STAT)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::styled("Sinks: ", Style::default().fg(colors::UI_SECONDARY)),
            Span::styled(
                format!("{sink_count} available"),
                Style::default()
                    .fg(colors::UI_STAT)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
    ];

    let paragraph = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Overview ")
        );

    frame.render_widget(paragraph, area);
}
```

### 4. Add PipeWire Helper Function

**File:** `src/pipewire.rs` (new function)

**Purpose:** Extract sample rate and format from pw-dump node info

```rust
/// Node information for a sink
pub struct SinkInfo {
    pub sample_rate: String,
    pub format: String,
}

impl PipeWire {
    /// Get detailed information about a specific sink
    ///
    /// # Errors
    /// Returns error if pw-dump fails or node not found
    pub fn get_sink_info(node_name: &str) -> Result<SinkInfo> {
        let objects = Self::dump()?;

        // Find the node in pw-dump output
        let node = objects
            .iter()
            .filter(|obj| obj["type"] == "PipeWire:Interface:Node")
            .find(|obj| {
                obj["info"]["props"]["node.name"]
                    .as_str()
                    .map_or(false, |name| name == node_name)
            })
            .context("Sink node not found")?;

        // Extract audio parameters
        let params = &node["info"]["params"];

        // Look for Format param type
        let format_param = params["Format"]
            .as_array()
            .and_then(|arr| arr.first())
            .context("No Format param found")?;

        // Parse sample rate from Format:Audio
        let sample_rate = format_param["rate"]
            .as_u64()
            .map(|rate| format!("{rate} Hz"))
            .unwrap_or_else(|| "Unknown".to_string());

        // Parse format from Format:Audio
        let format = format_param["format"]
            .as_str()
            .unwrap_or("Unknown")
            .to_string();

        Ok(SinkInfo {
            sample_rate,
            format,
        })
    }
}
```

**Note:** This is a rough sketch. The actual pw-dump JSON structure may differ. Need to:
1. Run `pw-dump` and inspect actual JSON structure
2. Find where audio params are stored (might be in `info.params.EnumFormat` or similar)
3. Adjust parsing logic accordingly

### 5. Update `DashboardRenderContext`

**File:** `src/tui/screens/dashboard.rs` (lines 159-167)

**No changes needed** - context already has config and window counts.

## Alternative Enhancements (Future)

If PipeWire node info is hard to parse or unreliable, simplify the Active Sink card:

```rust
let text = vec![
    Line::from(vec![
        Span::styled(sink_icon, Style::default().fg(colors::UI_HIGHLIGHT)),
        Span::raw(" "),
        Span::styled(sink_desc, Style::default().fg(colors::UI_TEXT).add_modifier(Modifier::BOLD)),
    ]),
    Line::from(""),
    Line::from(vec![
        Span::styled("Node: ", Style::default().fg(colors::UI_SECONDARY)),
        Span::styled(
            truncate_node_name(node_name),
            Style::default().fg(colors::UI_TEXT),
        ),
    ]),
    Line::from(Span::styled("Active Audio Output", Style::default().fg(colors::UI_SECONDARY))),
];
```

Where `truncate_node_name` shortens long ALSA node names like:
- `alsa_output.pci-0000_00_1f.3.analog-stereo` → `alsa_output...analog-stereo`

## Testing Checklist

- [ ] Verify layout looks balanced on different terminal sizes
- [ ] Check sample rate/format parsing with different sinks (HDMI, analog, Bluetooth)
- [ ] Ensure statistics update when rules/sinks change
- [ ] Test with zero rules, zero windows, unknown sink
- [ ] Verify colors match semantic style guide

## Files to Modify

1. `src/tui/screens/dashboard.rs` - Layout split, sink card enhancement, new stats card
2. `src/pipewire.rs` - New `get_sink_info()` helper (if pursuing technical details)

## Estimated Complexity

- **Layout changes:** Low - simple vertical split
- **Stats card:** Low - just rendering existing data
- **PipeWire parsing:** Medium - requires inspecting pw-dump JSON structure and careful parsing

## Color Scheme

- **Labels:** `UI_SECONDARY` (gray) for "Sample Rate:", "Rules:", etc.
- **Values:** `UI_TEXT` (white) for normal text, `UI_STAT` (yellow + bold) for counts
- **Icon:** `UI_HIGHLIGHT` (cyan) for sink icon
- **Borders:** Default for stats card, `UI_HIGHLIGHT` (cyan) for active sink card
