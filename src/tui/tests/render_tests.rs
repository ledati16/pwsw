//! Rendering tests: draw the real UI into a `TestBackend` from in-memory state.
//!
//! Every screen and every dialog is drawn at 80x24, 120x40 and 50x15; the
//! assertions check concrete things a user sees (field values, dialog options,
//! whole key hints, state markers) rather than exact layouts.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::style::Color;

use crate::ipc::{TrackedInfo, WindowInfo};
use crate::pipewire::{ActiveSink, ProfileSink};
use crate::test_utils::fixtures::{make_config, make_rule, make_sink, make_sink_with_icon};
use crate::tui::app::{App, AppUpdate, Screen, StatusKind};
use crate::tui::input::simulate_key_event;
use crate::tui::keymap::BINDINGS;
use crate::tui::screens::list::ListMode;
use crate::tui::screens::settings::SettingItem;
use crate::tui::screens::sinks::{SinkField, selector_names};
use crate::tui::{render_ui, theme};

/// The sizes every state is drawn at
const SIZES: [(u16, u16); 3] = [(80, 24), (120, 40), (50, 15)];

const BUILTIN: &str = "alsa_output.pci-0000_00_1f.3.analog-stereo";
const SONY: &str = "bluez_output.40_ED_98_1C_1D_08.1";
const HDMI: &str = "alsa_output.pci-0000_01_00.1.hdmi-stereo";
const USB: &str = "alsa_output.usb-Old_DAC-00.analog-stereo";
const SONY_DESC: &str = "Sony WH-1000XM4 (Bluetooth A2DP Sink)";

/// Draw `app` into a `width`×`height` terminal like the event loop does.
fn terminal(app: &mut App, width: u16, height: u16) -> Terminal<TestBackend> {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("test terminal");
    app.refresh_preview();
    terminal
        .draw(|frame| render_ui(frame, app))
        .expect("draw succeeds");
    terminal
}

/// The screen as rows of text
fn rows_of(terminal: &Terminal<TestBackend>) -> Vec<String> {
    let buffer = terminal.backend().buffer();
    buffer
        .content()
        .chunks(usize::from(buffer.area.width))
        .map(|row| {
            row.iter()
                .map(ratatui::buffer::Cell::symbol)
                .collect::<String>()
        })
        .collect()
}

fn draw_rows(app: &mut App, width: u16, height: u16) -> Vec<String> {
    rows_of(&terminal(app, width, height))
}

/// Draw at 140x40 and return the screen as one string
fn draw(app: &mut App) -> String {
    draw_rows(app, 140, 40).join("\n")
}

fn window(app_id: &str, title: &str, sink: Option<&str>) -> WindowInfo {
    WindowInfo {
        id: None,
        app_id: app_id.to_string(),
        title: title.to_string(),
        outputs: Vec::new(),
        minimized: false,
        matched_on: None,
        tracked: sink.map(|s| TrackedInfo {
            sink_name: String::new(),
            sink_desc: s.to_string(),
            fallbacks: Vec::new(),
            sources: Vec::new(),
        }),
    }
}

/// A realistic setup: four sinks in every state (playing, present, reachable by
/// profile switch, missing), rules, windows, logs and an installed unit.
fn fixture() -> App {
    let mut app = App::with_config(make_config(
        vec![
            make_sink_with_icon(
                BUILTIN,
                "Built-in Audio Analog Stereo",
                true,
                "audio-speakers",
            ),
            make_sink(SONY, SONY_DESC, false),
            make_sink_with_icon(HDMI, "HDMI / DisplayPort", false, "video-display"),
            make_sink(USB, "USB DAC", false),
        ],
        vec![
            make_rule("firefox", None, SONY_DESC),
            make_rule("^mpv$", None, "HDMI / DisplayPort"),
            make_rule("discord", Some("(?i)call"), "Built-in Audio Analog Stereo"),
        ],
    ));
    app.handle_update(AppUpdate::SinksData {
        active: vec![
            ActiveSink {
                name: BUILTIN.to_string(),
                description: "Built-in Audio Analog Stereo".to_string(),
                is_default: false,
            },
            ActiveSink {
                name: SONY.to_string(),
                description: "Sony WH-1000XM4".to_string(),
                is_default: true,
            },
        ],
        profiles: vec![ProfileSink {
            predicted_name: HDMI.to_string(),
            description: "HDMI / DisplayPort".to_string(),
            device_name: "alsa_card.pci-0000_01_00.1".to_string(),
            device_id: 40,
            profile_name: "output:hdmi-stereo".to_string(),
            profile_index: 5,
        }],
    });
    app.handle_update(AppUpdate::DaemonState {
        running: true,
        windows: vec![
            window("firefox", "Rust Book — Mozilla Firefox", Some(SONY_DESC)),
            window("mpv", "big_buck_bunny.mkv", Some("HDMI / DisplayPort")),
            window("kitty", "~/src/pwsw", None),
            window("org.gnome.Nautilus", "Downloads", None),
        ],
        unit_installed: true,
        service_enabled: Some(true),
        paused: false,
    });
    app.handle_update(AppUpdate::DaemonLogs(vec![
        "2026-11-20T10:03:10.120Z  INFO pwsw::daemon: Starting PWSW daemon v0.7.0".to_string(),
        "2026-11-20T10:03:11.400Z  WARN pwsw::daemon: Could not query default sink: timeout"
            .to_string(),
        format!(
            "2026-11-20T10:03:12.004Z  INFO pwsw::state: Rule matched: 'firefox' → {SONY_DESC}"
        ),
        "2026-11-20T10:03:13.900Z DEBUG pwsw::state: Window opened: app_id='kitty' title='~'"
            .to_string(),
    ]));
    app
}

type Setup = fn(&mut App);

/// Every screen and every dialog, by name
fn states() -> Vec<(&'static str, Setup)> {
    vec![
        ("dashboard", |_| {}),
        ("dashboard paused", |app| {
            app.daemon_paused = true;
            app.config.settings.notify_rules.toggle();
            app.mark_dirty();
        }),
        ("dashboard windows", |app| {
            app.dashboard_screen.toggle_view();
        }),
        ("sinks", |app| app.current_screen = Screen::Sinks),
        ("sink add", |app| {
            app.current_screen = Screen::Sinks;
            app.sinks_screen.start_add();
        }),
        ("sink edit", |app| {
            app.current_screen = Screen::Sinks;
            app.sinks_screen.selected = 1;
            app.sinks_screen.start_edit(&app.config.sinks);
        }),
        ("sink node picker", |app| {
            app.current_screen = Screen::Sinks;
            app.sinks_screen.start_add();
            let names = selector_names(&app.active_sink_list, &app.profile_sink_list);
            app.sinks_screen.open_selector(&names);
        }),
        ("sink delete", |app| {
            app.current_screen = Screen::Sinks;
            app.sinks_screen.selected = 3;
            app.sinks_screen.start_delete();
        }),
        ("sink details", |app| {
            app.current_screen = Screen::Sinks;
            app.sinks_screen.start_inspect();
        }),
        ("sink discard", |app| {
            app.current_screen = Screen::Sinks;
            app.sinks_screen.start_edit(&app.config.sinks);
            app.sinks_screen.editor.default.toggle();
            app.sinks_screen.confirm_discard = true;
        }),
        ("rules", |app| app.current_screen = Screen::Rules),
        ("rule add", |app| {
            app.current_screen = Screen::Rules;
            app.rules_screen.start_add();
        }),
        ("rule edit", |app| {
            app.current_screen = Screen::Rules;
            app.rules_screen.start_edit(&app.config.rules);
        }),
        ("rule from window", |app| {
            // Nautilus: no rule matches it (matched windows are listed first)
            app.dashboard_screen.toggle_view();
            app.dashboard_screen.window_selected = 3;
            simulate_key_event(app, KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE));
        }),
        ("rule sink picker", |app| {
            app.current_screen = Screen::Rules;
            app.rules_screen.start_add();
            app.rules_screen.open_sink_selector(&app.config.sinks);
        }),
        ("rule delete", |app| {
            app.current_screen = Screen::Rules;
            app.rules_screen.start_delete();
        }),
        ("rule details", |app| {
            app.current_screen = Screen::Rules;
            app.rules_screen.start_inspect();
        }),
        ("rule discard", |app| {
            app.current_screen = Screen::Rules;
            app.rules_screen.start_edit(&app.config.rules);
            app.rules_screen.confirm_discard = true;
        }),
        ("settings", |app| app.current_screen = Screen::Settings),
        ("settings log level", |app| {
            app.current_screen = Screen::Settings;
            app.settings_screen.selected = SettingItem::LogLevel.index();
            app.settings_screen.editing_log_level = true;
        }),
        ("settings switch delay", |app| {
            app.current_screen = Screen::Settings;
            app.config.settings.switch_delay_ms = 300;
            app.settings_screen.selected = SettingItem::SwitchDelay.index();
            app.settings_screen.toggle_current(&mut app.config.settings);
        }),
        ("help", |app| {
            app.current_screen = Screen::Rules;
            app.show_help = true;
        }),
        ("conflict", |app| app.save_conflict = true),
        ("no sinks", |app| {
            app.config.rules.clear();
            app.config.sinks.clear();
            app.current_screen = Screen::Sinks;
        }),
        ("no rules", |app| {
            app.config.rules.clear();
            app.current_screen = Screen::Rules;
        }),
        ("no rules, no sinks", |app| {
            app.config.rules.clear();
            app.config.sinks.clear();
            app.current_screen = Screen::Rules;
        }),
    ]
}

fn state(name: &str) -> App {
    let (_, setup) = states()
        .into_iter()
        .find(|(n, _)| *n == name)
        .expect("known state");
    let mut app = fixture();
    setup(&mut app);
    app
}

/// Every "keys label" hint the keymap defines
fn all_hints() -> Vec<String> {
    BINDINGS
        .iter()
        .filter_map(|b| b.hint)
        .map(|h| format!("{} {}", h.keys, h.label))
        .collect()
}

/// Assert the footer text (after any status) is made only of whole hints
fn assert_whole_hints(hints_text: &str, context: &str) {
    let known = all_hints();
    for piece in hints_text
        .split("  ")
        .map(str::trim)
        .filter(|p| !p.is_empty())
    {
        assert!(
            known.iter().any(|h| h == piece),
            "{context}: footer piece {piece:?} is not a whole hint"
        );
    }
}

#[test]
fn test_every_state_renders_at_every_size() {
    for (name, setup) in states() {
        for (width, height) in SIZES {
            let mut app = fixture();
            setup(&mut app);
            let rows = draw_rows(&mut app, width, height);
            let screen = rows.join("\n");
            let label = format!("{name} at {width}x{height}");
            if width < 60 || height < 16 {
                assert!(
                    screen.contains("Terminal too small — resize to at least 60x16"),
                    "{label}:\n{screen}"
                );
                continue;
            }
            // Header: exactly one row, then the content's top border
            assert!(rows[0].contains("1 Dashboard"), "{label}:\n{screen}");
            assert!(rows[0].contains("4 Settings"), "{label}:\n{screen}");
            assert!(!rows[1].contains("Dashboard"), "{label}:\n{screen}");
            // Footer: exactly one row below the content's bottom border
            let last = &rows[rows.len() - 1];
            assert!(
                rows[rows.len() - 2].trim_start().starts_with('╰'),
                "{label}:\n{screen}"
            );
            assert!(
                !last.contains('─') && !last.contains('╰'),
                "{label}:\n{screen}"
            );
            assert_whole_hints(last, &label);
            assert!(
                !last.trim().is_empty(),
                "{label}: footer shows hints\n{screen}"
            );
        }
    }
}

#[test]
fn test_footer_keeps_status_and_whole_hints() {
    for (width, height) in [(60, 16), (80, 24), (120, 40)] {
        for (name, setup) in states() {
            let mut app = fixture();
            setup(&mut app);
            app.set_status(StatusKind::Success, "Configuration saved");
            let rows = draw_rows(&mut app, width, height);
            let last = &rows[rows.len() - 1];
            let (_, after) = last
                .split_once("✓ Configuration saved")
                .unwrap_or_else(|| panic!("{name}: status shown: {last:?}"));
            assert_whole_hints(after, name);
        }
    }

    // Errors get ✗, and the most important hints survive at 80 columns
    let mut app = state("sinks");
    app.set_status(StatusKind::Error, "Failed to save config: disk full");
    let rows = draw_rows(&mut app, 80, 24);
    let last = &rows[23];
    assert!(
        last.contains("✗ Failed to save config: disk full"),
        "{last}"
    );
    assert!(last.contains("a add"), "{last}");
    assert!(last.contains("q quit"), "{last}");
}

#[test]
fn test_footer_drops_hints_by_priority() {
    let mut app = state("sinks");
    let wide = draw_rows(&mut app, 120, 40);
    let narrow = draw_rows(&mut app, 60, 16);
    let (wide, narrow) = (&wide[39], &narrow[15]);
    for hint in ["a add", "e edit", "q quit"] {
        assert!(narrow.contains(hint), "{hint} kept: {narrow}");
    }
    assert!(wide.contains("Enter details"), "{wide}");
    assert!(
        !narrow.contains("Enter details"),
        "least important dropped: {narrow}"
    );

    // Ctrl+S is offered only with unsaved changes
    assert!(!wide.contains("Ctrl+S save"), "{wide}");
    app.config.settings.notify_rules.toggle();
    app.mark_dirty();
    let rows = draw_rows(&mut app, 80, 24);
    assert!(rows[23].contains("Ctrl+S save"), "{}", rows[23]);
    assert!(
        rows[0].contains("● unsaved"),
        "marker in the header: {}",
        rows[0]
    );
}

#[test]
fn test_status_expires_or_waits_for_a_key() {
    let mut app = fixture();
    app.set_status(StatusKind::Success, "Sink added");
    assert!(app.status_deadline().is_some(), "success expires");
    app.set_status(StatusKind::Error, "Failed");
    assert!(app.status_deadline().is_none(), "errors do not expire");
    assert_eq!(app.status_kind(), Some(StatusKind::Error));
    simulate_key_event(&mut app, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    assert_eq!(
        app.status_message(),
        None,
        "the next key dismisses an error"
    );
}

#[test]
fn test_sink_form_shows_every_field_at_80x24() {
    let mut app = state("sink edit");
    let terminal = terminal(&mut app, 80, 24);
    let rows = rows_of(&terminal);
    let screen = rows.join("\n");
    let form_top = rows
        .iter()
        .position(|r| r.contains("╭ Edit Sink "))
        .expect("form drawn");
    let row_with = |label: &str| {
        (form_top..rows.len())
            .find(|&y| rows[y].contains(&format!("{label} ")))
            .unwrap_or_else(|| panic!("{label} row:\n{screen}"))
    };
    let node = row_with("Node");
    assert!(rows[node].contains(SONY), "{screen}");
    assert!(rows[node].contains("▌Node"), "focused marker: {screen}");
    assert!(rows[node].contains("␣ pick"), "{screen}");
    assert!(
        rows[row_with("Description")].contains(SONY_DESC),
        "{screen}"
    );
    assert!(
        rows[row_with("Icon")].contains("(auto: audio-headphones)"),
        "{screen}"
    );
    assert!(
        rows[row_with("Fallback")].contains("[ ] used when no rule matches"),
        "{screen}"
    );
    assert!(screen.contains("Enter save · Esc cancel"), "{screen}");

    // The Node field shows the cursor (just after the name)
    let cursor = terminal.backend().cursor_position();
    assert!(terminal.backend().cursor_visible(), "cursor shown");
    assert_eq!(usize::from(cursor.y), node, "{cursor:?}");
    let name_end = rows[node].find(SONY).expect("name") + SONY.len();
    let cursor_x = rows[node][..name_end].chars().count();
    assert_eq!(usize::from(cursor.x), cursor_x, "{cursor:?}\n{screen}");
}

#[test]
fn test_sink_form_cursor_follows_focus() {
    let mut app = state("sink add");
    app.sinks_screen.editor.focused_field = SinkField::Desc;
    let term = terminal(&mut app, 80, 24);
    let rows = rows_of(&term);
    let cursor = term.backend().cursor_position();
    assert!(term.backend().cursor_visible());
    assert!(
        rows[usize::from(cursor.y)].contains("▌Description"),
        "{rows:#?}"
    );

    // The checkbox has no cursor
    app.sinks_screen.editor.focused_field = SinkField::Default;
    let term = terminal(&mut app, 80, 24);
    assert!(!term.backend().cursor_visible());
}

#[test]
fn test_rule_form_shows_every_field_at_80x24() {
    let mut app = state("rule edit");
    let rows = draw_rows(&mut app, 80, 24);
    let screen = rows.join("\n");
    for (label, value) in [
        ("App ID", "firefox"),
        ("Title", "any title"),
        ("Sinks", SONY_DESC),
        ("Description", "optional note"),
        ("Notify", "default (from Settings)"),
    ] {
        assert!(
            rows.iter()
                .any(|r| r.contains(&format!("{label} ")) && r.contains(value)),
            "{label}: {value}\n{screen}"
        );
    }
    assert!(screen.contains("Matching windows (1)"), "{screen}");
    assert!(screen.contains("firefox | Rust Book"), "{screen}");
}

/// The fixture with the first rule (firefox) sending to Sony, then HDMI, then USB
fn three_sink_rule() -> App {
    let mut app = fixture();
    app.config.rules[0].fallback_refs = vec!["HDMI / DisplayPort".to_string(), USB.to_string()];
    app
}

#[test]
fn test_rule_form_shows_three_sinks_in_order_at_80x24() {
    let mut app = three_sink_rule();
    app.current_screen = Screen::Rules;
    app.rules_screen.start_edit(&app.config.rules);
    app.rules_screen.editor.focused_field = crate::tui::screens::rules::RuleField::Sinks;
    let rows = draw_rows(&mut app, 80, 24);
    let screen = rows.join("\n");
    let row = rows
        .iter()
        .find(|r| r.contains("▌Sinks "))
        .unwrap_or_else(|| panic!("focused Sinks row\n{screen}"));
    // Cut to the dialog, first choice first
    assert!(
        row.contains("Sony WH-1000XM4 (Bluetooth A2DP Sink) → HDMI"),
        "{row}"
    );
    assert!(row.contains("␣ choose"), "{row}");
    assert!(rows.iter().all(|r| r.chars().count() == 80));
    assert!(rows[23].contains("Space choose"), "{}", rows[23]);

    // The picker numbers the chosen sinks in order, the others after them
    simulate_key_event(
        &mut app,
        KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE),
    );
    let rows = draw_rows(&mut app, 80, 24);
    let screen = rows.join("\n");
    assert!(screen.contains("Choose Sinks (in order)"), "{screen}");
    // (the rules table behind it names the sinks too: look at picker rows only)
    let line_of = |text: &str| {
        rows.iter()
            .position(|r| r.contains(text) && r.contains("_output"))
            .unwrap_or_else(|| panic!("{text}\n{screen}"))
    };
    let lines = [
        line_of("1 Sony"),
        line_of("2 HDMI"),
        line_of("3 USB DAC"),
        line_of("    Built-in Audio"),
    ];
    assert!(lines.is_sorted(), "{lines:?}\n{screen}");
    assert!(rows[23].contains("Shift+↑/↓ order"), "{}", rows[23]);

    // Shift+↓ on the first choice swaps it with HDMI; Enter applies
    simulate_key_event(&mut app, KeyEvent::new(KeyCode::Down, KeyModifiers::SHIFT));
    simulate_key_event(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    let rows = draw_rows(&mut app, 80, 24);
    assert!(
        rows.iter()
            .any(|r| r.contains("Sinks ") && r.contains("HDMI / DisplayPort → Sony")),
        "{}",
        rows.join("\n")
    );
}

#[test]
fn test_rules_table_and_details_show_the_sink_order() {
    let mut app = three_sink_rule();
    app.current_screen = Screen::Rules;
    let screen = draw(&mut app);
    assert!(
        screen.contains("Sony WH-1000XM4 (Bluetooth A2DP Sink) → HD…"),
        "{screen}"
    );
    app.rules_screen.start_inspect();
    let screen = draw(&mut app);
    for line in [
        "Sinks        Sony WH-1000XM4 (Bluetooth A2DP Sink)",
        "             → HDMI / DisplayPort",
        "             → USB DAC",
    ] {
        assert!(screen.contains(line), "{line}\n{screen}");
    }
    app.rules_screen.start_delete();
    let screen = draw(&mut app);
    assert!(screen.contains("→ USB DAC"), "{screen}");
}

#[test]
fn test_rule_form_from_window_at_80x24() {
    let mut app = state("rule from window");
    let rows = draw_rows(&mut app, 80, 24);
    let screen = rows.join("\n");
    assert!(screen.contains("╭ Add Rule "), "{screen}");
    for (label, value) in [
        ("App ID", r"^org\.gnome\.Nautilus$"),
        ("Title", "any title"),
        ("Sinks", "none chosen"),
    ] {
        assert!(
            rows.iter()
                .any(|r| r.contains(&format!("{label} ")) && r.contains(value)),
            "{label}: {value}\n{screen}"
        );
    }
    assert!(screen.contains("Current title: Downloads"), "{screen}");
    assert!(screen.contains("Matching windows (1)"), "{screen}");
    assert!(
        screen.contains("org.gnome.Nautilus | Downloads"),
        "{screen}"
    );
    assert!(rows[23].contains("Space choose"), "{}", rows[23]);

    // A long title is cut to the dialog
    let mut app = state("rule from window");
    app.rules_screen.editor.window_title = Some("y".repeat(300));
    let rows = draw_rows(&mut app, 80, 24);
    let hint = rows
        .iter()
        .find(|r| r.contains("Current title: "))
        .expect("hint row");
    assert!(hint.contains("y…"), "{hint}");
    assert!(rows.iter().all(|r| r.chars().count() == 80));

    // A window that a rule already matches opens that rule
    let mut app = fixture();
    app.dashboard_screen.toggle_view();
    app.dashboard_screen.window_selected = 1; // mpv
    simulate_key_event(
        &mut app,
        KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE),
    );
    let rows = draw_rows(&mut app, 80, 24);
    let screen = rows.join("\n");
    assert!(screen.contains("╭ Edit Rule "), "{screen}");
    assert!(
        rows[23].contains("This window already matches rule 2"),
        "{}",
        rows[23]
    );
}

#[test]
fn test_windows_view_highlights_selection_and_offers_new_rule() {
    let mut app = state("dashboard windows");
    simulate_key_event(&mut app, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    let terminal = terminal(&mut app, 80, 24);
    let rows = rows_of(&terminal);
    let screen = rows.join("\n");
    assert!(rows[23].contains("n new rule"), "{}", rows[23]);
    let y = rows
        .iter()
        .position(|r| r.contains("mpv"))
        .expect("mpv row");
    let x = rows[y].find("mpv").expect("mpv");
    let x = rows[y][..x].chars().count();
    let cell =
        &terminal.backend().buffer()[(u16::try_from(x).expect("x"), u16::try_from(y).expect("y"))];
    assert!(
        cell.modifier.contains(ratatui::style::Modifier::REVERSED),
        "selected row highlighted:\n{screen}"
    );
    // The logs view does not offer it
    let rows = draw_rows(&mut state("dashboard"), 80, 24);
    assert!(!rows[23].contains("new rule"), "{}", rows[23]);
}

#[test]
fn test_rule_form_fits_the_smallest_terminal() {
    let mut app = state("rule edit");
    let rows = draw_rows(&mut app, 60, 16);
    let screen = rows.join("\n");
    for label in ["App ID", "Title", "Sinks", "Description", "Notify"] {
        assert!(screen.contains(&format!("{label} ")), "{label}\n{screen}");
    }
}

#[test]
fn test_dialogs_show_their_options_at_80x24() {
    for (name, options) in [
        (
            "sink delete",
            &["Delete this sink?", "USB DAC", "Enter delete", "Esc cancel"][..],
        ),
        ("rule delete", &[
            "Delete this rule?",
            "firefox",
            "Enter delete",
            "Esc cancel",
        ]),
        ("sink details", &[
            "Built-in Audio",
            "audio-speakers",
            "Enter close",
        ]),
        ("rule details", &[
            "firefox",
            "default (from Settings)",
            "Enter close",
        ]),
        ("sink discard", &[
            "Discard changes?",
            "y discard",
            "n keep editing",
        ]),
        ("rule discard", &[
            "Discard changes?",
            "y discard",
            "n keep editing",
        ]),
        ("conflict", &[
            "Config Changed on Disk",
            "Overwrite the file",
            "Reload from disk",
            "Cancel (keep editing",
        ]),
        ("settings log level", &[
            "Log Level",
            "error",
            "trace",
            "Enter apply",
        ]),
        ("sink node picker", &[
            "Sinks in PipeWire",
            "After a profile switch",
            "Sony WH-1000XM4",
        ]),
        ("rule sink picker", &[
            "Choose Sinks (in order)",
            "USB DAC",
            "Space pick",
            "Enter done",
        ]),
    ] {
        let mut app = state(name);
        let screen = draw_rows(&mut app, 80, 24).join("\n");
        for option in options {
            assert!(screen.contains(option), "{name}: {option}\n{screen}");
        }
    }
}

#[test]
fn test_sinks_table_marks_states_and_never_prints_icon_names() {
    for (width, height) in [(80, 24), (120, 40)] {
        let mut app = state("sinks");
        let rows = draw_rows(&mut app, width, height);
        let screen = rows.join("\n");
        let row = |text: &str| {
            rows.iter()
                .find(|r| r.contains(text))
                .unwrap_or_else(|| panic!("{text}:\n{screen}"))
        };
        assert!(row("Sony WH-1000XM4").contains("▶ playing"), "{screen}");
        let builtin = row("Built-in Audio");
        assert!(builtin.contains("present"), "{screen}");
        assert!(builtin.contains("★ fallback"), "{screen}");
        assert!(
            row("HDMI / DisplayPort").contains("via profile"),
            "{screen}"
        );
        assert!(row("USB DAC").contains("missing"), "{screen}");
        assert_eq!(screen.matches("★ fallback").count(), 1, "{screen}");
        for icon in ["audio-speakers", "video-display", "audio-headphones"] {
            assert!(
                !screen.contains(icon),
                "icon name {icon} printed:\n{screen}"
            );
        }
        assert!(
            screen.contains("Description") && screen.contains("State"),
            "{screen}"
        );
        assert!(screen.contains("4 configured · 2 present"), "{screen}");
    }
}

#[test]
fn test_empty_states() {
    let screen = draw_rows(&mut state("no sinks"), 80, 24).join("\n");
    assert!(screen.contains("No sinks yet"), "{screen}");
    assert!(
        screen.contains("Press a, then Space to pick an output from PipeWire"),
        "{screen}"
    );

    let screen = draw_rows(&mut state("no rules"), 80, 24).join("\n");
    assert!(
        screen.contains("No rules yet — press a to send a window's audio to a sink"),
        "{screen}"
    );
    assert!(
        screen.contains("or pick a window in Dashboard → w → n"),
        "{screen}"
    );

    let screen = draw_rows(&mut state("no rules, no sinks"), 80, 24).join("\n");
    assert!(
        screen.contains("Add a sink first (2 Sinks → a)"),
        "{screen}"
    );
}

#[test]
fn test_dashboard_summary_and_logs() {
    let rows = draw_rows(&mut fixture(), 80, 24);
    let screen = rows.join("\n");
    assert!(screen.contains("Daemon ● running · enabled"), "{screen}");
    assert!(screen.contains("2/4 windows matched"), "{screen}");
    assert!(screen.contains("Playing on"), "{screen}");
    assert!(screen.contains("Sony WH-1000XM4"), "{screen}");
    assert!(screen.contains("4 sinks · 2 present · 3 rules"), "{screen}");
    assert!(screen.contains("Logs · live · UTC"), "{screen}");
    // Levels are padded, so messages start in the same column
    let column = |text: &str| {
        let row = rows.iter().find(|r| r.contains(text)).expect(text);
        row.chars().position(|c| c == 'p').expect("message")
    };
    assert_eq!(column("Starting PWSW"), column("Window opened"), "{screen}");
    assert_eq!(
        column("Starting PWSW"),
        column("Could not query"),
        "{screen}"
    );
}

#[test]
fn test_dashboard_preselects_the_sensible_action() {
    let mut app = App::with_config(make_config(vec![], vec![]));
    assert_eq!(
        app.dashboard_screen.selected_action, 0,
        "start when stopped"
    );
    let state = |running| AppUpdate::DaemonState {
        running,
        windows: Vec::new(),
        unit_installed: true,
        service_enabled: Some(true),
        paused: false,
    };
    app.handle_update(state(true));
    assert_eq!(
        app.dashboard_screen.selected_action, 2,
        "restart when running"
    );
    app.dashboard_screen.selected_action = 1; // the user picks stop
    app.handle_update(state(true));
    assert_eq!(app.dashboard_screen.selected_action, 1, "a choice is kept");
    app.handle_update(state(false));
    assert_eq!(
        app.dashboard_screen.selected_action, 0,
        "start once stopped"
    );
}

#[test]
fn test_windows_table_uses_the_width() {
    let mut app = state("dashboard windows");
    let rows = draw_rows(&mut app, 120, 40);
    let screen = rows.join("\n");
    assert!(screen.contains("Windows · 2/4 matched"), "{screen}");
    let firefox = rows
        .iter()
        .find(|r| r.contains("firefox"))
        .expect("firefox");
    assert!(firefox.contains("Rust Book — Mozilla Firefox"), "{screen}");
    assert!(firefox.contains("→ Sony WH-1000XM4"), "{screen}");
    // Matched windows first
    let pos = |t: &str| rows.iter().position(|r| r.contains(t)).expect(t);
    assert!(pos("mpv") < pos("kitty"), "{screen}");
}

#[test]
fn test_without_colors_no_cell_is_colored() {
    let colored = |app: &mut App| {
        let terminal = terminal(app, 80, 24);
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .any(|c| c.fg != Color::Reset || c.bg != Color::Reset)
    };
    assert!(colored(&mut fixture()), "colours are used by default");
    theme::without_colors(|| {
        for (name, setup) in states() {
            let mut app = fixture();
            setup(&mut app);
            app.set_status(StatusKind::Error, "Something failed");
            assert!(
                !colored(&mut app),
                "{name} has coloured cells without colour"
            );
        }
    });
}

#[test]
fn test_text_uses_the_terminal_colors() {
    // Body text (form values) and dialogs keep the terminal's own colours
    let mut app = state("sink edit");
    let terminal = terminal(&mut app, 80, 24);
    let buffer = terminal.backend().buffer();
    let rows = rows_of(&terminal);
    let y = rows
        .iter()
        .position(|r| r.contains("Description ") && r.contains("Sony"))
        .expect("description row");
    let x = rows[y].chars().position(|c| c == 'S').expect("value");
    let cell = &buffer[(u16::try_from(x).expect("x"), u16::try_from(y).expect("y"))];
    assert_eq!((cell.fg, cell.bg), (Color::Reset, Color::Reset));
    assert!(
        buffer.content().iter().all(|c| c.bg == Color::Reset),
        "no painted backgrounds"
    );
}

/// Not an assertion: prints the 80x24 screens for review
/// (`cargo test print_80x24 -- --nocapture`)
#[test]
fn test_print_80x24_screens() {
    for name in [
        "dashboard",
        "dashboard paused",
        "settings",
        "settings switch delay",
        "dashboard windows",
        "sinks",
        "sink edit",
        "rule edit",
        "rule from window",
        "no rules",
    ] {
        let rows = draw_rows(&mut state(name), 80, 24);
        println!("--- {name} ---\n{}", rows.join("\n"));
    }
}

#[test]
fn test_dashboard_shows_default_sink_from_snapshot() {
    let mut app = App::with_config(make_config(
        vec![
            make_sink("alsa_output.other", "Other Speakers", false),
            make_sink_with_icon("alsa_output.test", "Test Headphones", true, "H"),
        ],
        vec![],
    ));
    app.handle_update(AppUpdate::SinksData {
        active: vec![ActiveSink {
            name: "alsa_output.test".to_string(),
            description: "PipeWire Name".to_string(),
            is_default: true,
        }],
        profiles: Vec::new(),
    });
    let screen = draw(&mut app);
    assert!(screen.contains("Test Headphones"), "{screen}");
    assert!(screen.contains("alsa_output.test"), "{screen}");
}

#[test]
fn test_dashboard_shows_unconfigured_default_sink_by_pipewire_name() {
    let mut app = App::with_config(make_config(vec![], vec![]));
    app.handle_update(AppUpdate::SinksData {
        active: vec![ActiveSink {
            name: "alsa_output.usb".to_string(),
            description: "USB Interface".to_string(),
            is_default: true,
        }],
        profiles: Vec::new(),
    });
    let screen = draw(&mut app);
    assert!(
        screen.contains("USB Interface (not configured)"),
        "{screen}"
    );
}

#[test]
fn test_dashboard_without_snapshot_shows_unknown() {
    let mut app = App::with_config(make_config(vec![], vec![]));
    let screen = draw(&mut app);
    assert!(screen.contains("No default sink"), "{screen}");

    app.handle_update(AppUpdate::PipeWireUnavailable);
    let screen = draw(&mut app);
    assert!(screen.contains("PipeWire unavailable"), "{screen}");
}

#[test]
fn test_rule_editor_shows_invalid_title_regex() {
    let mut app = App::with_config(make_config(vec![make_sink("s1", "d1", true)], vec![]));
    app.current_screen = Screen::Rules;
    app.rules_screen.start_add();
    app.rules_screen.editor.app_id_pattern = tui_input::Input::new("firefox".to_string());
    app.rules_screen.editor.title_pattern = tui_input::Input::new("(oops".to_string());

    let screen = draw(&mut app);
    assert!(screen.contains("Invalid title regex"), "{screen}");
    assert!(screen.contains("✗ invalid"), "{screen}");
}

#[test]
fn test_rule_editor_preview_follows_window_updates() {
    let mut app = App::with_config(make_config(vec![make_sink("s1", "d1", true)], vec![]));
    app.current_screen = Screen::Rules;
    app.rules_screen.start_add();
    app.rules_screen.editor.app_id_pattern = tui_input::Input::new("^mpv$".to_string());

    let state = |windows| AppUpdate::DaemonState {
        running: true,
        windows,
        unit_installed: false,
        service_enabled: None,
        paused: false,
    };

    app.handle_update(state(vec![window("firefox", "Browser", None)]));
    let screen = draw(&mut app);
    assert!(screen.contains("No matching windows"), "{screen}");

    app.handle_update(state(vec![
        window("firefox", "Browser", None),
        window("mpv", "Some Video", None),
    ]));
    let screen = draw(&mut app);
    assert!(screen.contains("mpv | Some Video"), "{screen}");
}

#[test]
fn test_esc_with_edits_asks_before_discarding() {
    let esc = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
    let key = |c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE);

    // Unchanged: Esc closes at once
    let mut app = state("sink edit");
    simulate_key_event(&mut app, esc);
    assert_eq!(app.sinks_screen.mode, ListMode::List);

    // Changed: asks; n keeps editing (and the edit), y discards
    let mut app = state("sink edit");
    app.sinks_screen.editor.focused_field = SinkField::Desc;
    simulate_key_event(&mut app, key('!'));
    simulate_key_event(&mut app, esc);
    assert!(app.sinks_screen.confirm_discard);
    assert_eq!(app.sinks_screen.mode, ListMode::AddEdit);
    simulate_key_event(&mut app, key('x'));
    assert!(app.sinks_screen.confirm_discard, "other keys are ignored");
    simulate_key_event(&mut app, key('n'));
    assert!(!app.sinks_screen.confirm_discard);
    assert!(app.sinks_screen.editor.desc.value().ends_with('!'));
    simulate_key_event(&mut app, esc);
    simulate_key_event(&mut app, key('y'));
    assert_eq!(app.sinks_screen.mode, ListMode::List);
    assert_eq!(app.config.sinks[1].desc, SONY_DESC);

    let mut app = state("rule add");
    simulate_key_event(&mut app, key('f'));
    simulate_key_event(&mut app, esc);
    assert!(app.rules_screen.confirm_discard);
    simulate_key_event(&mut app, esc);
    assert!(!app.rules_screen.confirm_discard, "Esc keeps editing");
    assert_eq!(app.rules_screen.mode, ListMode::AddEdit);
}

fn windows_app(count: u64) -> App {
    let mut app = App::with_config(make_config(vec![], vec![]));
    app.handle_update(AppUpdate::DaemonState {
        running: true,
        windows: (0..count)
            .map(|i| WindowInfo {
                id: Some(i),
                app_id: format!("app{i:02}"),
                title: String::new(),
                outputs: Vec::new(),
                minimized: false,
                matched_on: None,
                tracked: None,
            })
            .collect(),
        unit_installed: false,
        service_enabled: None,
        paused: false,
    });
    app.dashboard_screen.toggle_view();
    app
}

#[test]
fn test_windows_view_scrolls_down_with_pagedown_and_shows_position() {
    let mut app = windows_app(100);
    let screen = draw(&mut app);
    assert!(screen.contains("app00"), "{screen}");
    assert!(screen.contains("[1-"), "indicator visible: {screen}");
    assert!(screen.contains("/100]"), "{screen}");
    let page = app.dashboard_screen.page_size();
    assert!(page > 1 && page < 50, "page from the viewport: {page}");

    simulate_key_event(
        &mut app,
        KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE),
    );
    let screen = draw(&mut app);
    assert!(
        !screen.contains("app00"),
        "first page scrolled away: {screen}"
    );
    assert!(screen.contains(&format!("app{page:02}")), "{screen}");
    assert!(screen.contains(&format!("[{}-", page + 1)), "{screen}");

    // End shows the last window; Home goes back up
    simulate_key_event(&mut app, KeyEvent::new(KeyCode::End, KeyModifiers::NONE));
    let screen = draw(&mut app);
    assert!(screen.contains("app99"), "{screen}");
    assert!(screen.contains("-100/100]"), "{screen}");
    simulate_key_event(&mut app, KeyEvent::new(KeyCode::Home, KeyModifiers::NONE));
    let screen = draw(&mut app);
    assert!(screen.contains("app00"), "{screen}");
}

#[test]
fn test_windows_view_offset_clamped_when_list_shrinks() {
    let mut app = windows_app(40);
    draw(&mut app);
    app.dashboard_screen.window_scroll_offset = 35;
    let mut shrunk = windows_app(3);
    std::mem::swap(&mut app.windows, &mut shrunk.windows);
    let screen = draw(&mut app);
    assert!(screen.contains("app00"), "{screen}");
    assert_eq!(app.dashboard_screen.window_scroll_offset, 0);
}

#[test]
fn test_log_view_keeps_newest_line_visible_with_wrapped_lines() {
    let mut app = App::with_config(make_config(vec![], vec![]));
    let long = "x".repeat(400);
    let mut lines: Vec<String> = (0..50)
        .map(|i| format!("2025-01-01T00:00:00Z INFO {long} {i}"))
        .collect();
    lines.push("2025-01-01T00:00:01Z INFO newest-marker".to_string());
    app.handle_update(AppUpdate::DaemonLogs(lines));

    let screen = draw(&mut app);
    assert!(screen.contains("newest-marker"), "{screen}");
}

#[test]
fn test_log_view_renders_non_ascii_lines() {
    let mut app = App::with_config(make_config(vec![], vec![]));
    app.handle_update(AppUpdate::DaemonLogs(vec![
        "2025-12-17Tääääääääää INFO grüße".to_string(),
        "日本T語語語語語語語語語.語 WARN 窓".to_string(),
        "\u{fffd}\u{fffd} broken".to_string(),
    ]));
    let screen = draw(&mut app);
    assert!(screen.contains("grüße"), "{screen}");
    assert!(screen.contains("broken"), "{screen}");
}

#[test]
fn test_settings_log_level_dropdown_hides_background() {
    use crate::tui::screens::settings::LOG_LEVELS;

    let mut app = state("settings log level");
    let rows = draw_rows(&mut app, 80, 24);
    let screen = rows.join("\n");
    let top = rows
        .iter()
        .position(|r| r.contains("╭ Log Level "))
        .unwrap_or_else(|| panic!("dropdown drawn:\n{screen}"));
    let chars = |row: &str| row.chars().collect::<Vec<_>>();
    let top_row = chars(&rows[top]);
    let left = rows[top][..rows[top].find("╭ Log Level ").expect("title")]
        .chars()
        .count();
    let right = left
        + 1
        + top_row[left + 1..]
            .iter()
            .position(|c| *c == '╮' || *c == '╭')
            .expect("corner");
    let mut seen = Vec::new();
    for row in &rows[top + 1..] {
        let row = chars(row);
        if row[left] == '╰' {
            break;
        }
        let inside: String = row[left + 1..right].iter().collect();
        let content = inside.replace('▌', "");
        let content = content.trim();
        assert!(
            LOG_LEVELS.contains(&content),
            "background shows through: {inside:?}\n{screen}"
        );
        seen.push(content.to_string());
    }
    assert_eq!(seen, LOG_LEVELS, "every level visible");
}

#[test]
fn test_tiny_terminals_render_without_panic() {
    for (width, height) in [
        (1, 1),
        (3, 10),
        (4, 20),
        (4, 3),
        (20, 4),
        (60, 16),
        (61, 17),
    ] {
        for (_, setup) in states() {
            let mut app = fixture();
            setup(&mut app);
            draw_rows(&mut app, width, height);
            app.show_help = true;
            app.save_conflict = true;
            draw_rows(&mut app, width, height);
        }
    }
    // Long values everywhere
    let long_desc = "日本".repeat(100);
    let mut app = App::with_config(make_config(
        vec![make_sink(&"n".repeat(300), &long_desc, true)],
        vec![make_rule(
            &"a".repeat(300),
            Some(&"t".repeat(300)),
            &long_desc,
        )],
    ));
    for screen in Screen::all() {
        app.current_screen = *screen;
        draw_rows(&mut app, 60, 16);
    }
    app.current_screen = Screen::Sinks;
    app.sinks_screen.start_edit(&app.config.sinks);
    draw_rows(&mut app, 60, 16);
}

#[test]
fn test_dashboard_shows_enable_with_installed_unit() {
    let mut app = App::with_config(make_config(vec![], vec![]));
    app.handle_update(AppUpdate::DaemonState {
        running: true,
        windows: Vec::new(),
        unit_installed: true,
        service_enabled: Some(false),
        paused: false,
    });
    let screen = draw(&mut app);
    assert!(screen.contains("Enable"), "{screen}");
    assert!(screen.contains("disabled"), "{screen}");
}

#[test]
fn test_paused_daemon_is_marked_on_every_screen() {
    for (width, height) in [(80, 24), (120, 40), (60, 16)] {
        for &screen in Screen::all() {
            let mut app = state("dashboard paused");
            app.current_screen = screen;
            let rows = draw_rows(&mut app, width, height);
            let label = format!("{screen:?} at {width}x{height}");
            let screen_text = rows.join("\n");
            assert!(rows[0].contains('⏸'), "{label}:\n{screen_text}");
            if width >= 80 {
                assert!(
                    rows[0].contains("⏸ paused  ● unsaved"),
                    "{label}:\n{screen_text}"
                );
            }
            assert!(rows[0].contains("4 Settings"), "{label}:\n{screen_text}");
            if screen == Screen::Dashboard {
                assert!(
                    rows[1].contains("Daemon ● running · paused"),
                    "{label}:\n{screen_text}"
                );
            }
        }
    }
    // Not paused: no marker anywhere
    let rows = draw_rows(&mut fixture(), 80, 24);
    assert!(!rows[0].contains('⏸'), "{}", rows.join("\n"));
    assert!(
        rows[1].contains("Daemon ● running ·"),
        "{}",
        rows.join("\n")
    );
    assert!(!rows[1].contains("paused"), "{}", rows.join("\n"));
}

#[test]
fn test_paused_marker_uses_the_warning_colour() {
    let mut app = state("dashboard paused");
    let terminal = terminal(&mut app, 80, 24);
    let rows = rows_of(&terminal);
    let x = rows[0].chars().position(|c| c == '⏸').expect("marker");
    let cell = &terminal.backend().buffer()[(u16::try_from(x).expect("x"), 0)];
    assert_eq!(cell.style().fg, theme::warning().fg);
}

#[test]
fn test_settings_show_the_switch_delay_and_its_editor() {
    let mut app = state("settings");
    app.config.settings.switch_delay_ms = 300;
    let screen = draw_rows(&mut app, 80, 24).join("\n");
    assert!(screen.contains("Switch Delay"), "{screen}");
    assert!(screen.contains("300 ms"), "{screen}");

    let mut app = state("settings");
    let screen = draw_rows(&mut app, 80, 24).join("\n");
    assert!(screen.contains("0 ms (immediate)"), "{screen}");

    let mut app = state("settings switch delay");
    let rows = draw_rows(&mut app, 80, 24);
    let screen = rows.join("\n");
    assert!(screen.contains("╭ Switch Delay "), "{screen}");
    assert!(
        rows.iter()
            .any(|r| r.contains("Milliseconds") && r.contains("300")),
        "{screen}"
    );
    assert!(
        screen.contains("0 to 5000; 0 = switch immediately"),
        "{screen}"
    );
    assert!(screen.contains("Enter apply · Esc cancel"), "{screen}");

    // Selected (editor closed): the description explains it without scrolling
    let mut app = state("settings switch delay");
    app.settings_screen.delay_input = None;
    let screen = draw_rows(&mut app, 80, 24).join("\n");
    assert!(screen.contains("Wait this long for the rules"), "{screen}");
    assert!(
        screen.contains("Default: 0 (switch immediately)"),
        "{screen}"
    );
}
