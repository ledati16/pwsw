//! Rendering tests: draw the real UI into a `TestBackend` from in-memory state.
//!
//! Every screen and every dialog is drawn at 80x24, 120x40 and 50x15; the
//! assertions check concrete things a user sees (field values, dialog options,
//! whole key hints, state markers) rather than exact layouts.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::style::Color;

use crate::config::Endpoint;
use crate::ipc::{TrackedInfo, WindowInfo};
use crate::pipewire::{ActiveSink, ProfileSink};
use crate::test_utils::fixtures::{
    make_config, make_rule, make_sink, make_sink_with_icon, with_output, with_sources,
};
use crate::tui::app::{App, AppUpdate, Screen, StatusKind};
use crate::tui::input::simulate_key_event;
use crate::tui::keymap::{BINDINGS, RESUME_HINT};
use crate::tui::screens::list::ListMode;
use crate::tui::screens::rules::RuleField;
use crate::tui::screens::settings::SettingItem;
use crate::tui::screens::sinks::{SinkField, selector_names};
use crate::tui::{render_ui, theme};

/// The sizes every state is drawn at
const SIZES: [(u16, u16); 3] = [(80, 24), (120, 40), (50, 15)];

const BUILTIN: &str = "alsa_output.pci-0000_00_1f.3.analog-stereo";
const BT_HEADPHONES: &str = "bluez_output.00_11_22_33_44_55.1";
const HDMI: &str = "alsa_output.pci-0000_01_00.1.hdmi-stereo";
const USB: &str = "alsa_output.usb-Old_DAC-00.analog-stereo";
const BT_DESC: &str = "Sony WH-1000XM4 (Bluetooth A2DP Sink)";

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
            fallback_in_use: None,
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
            make_sink(BT_HEADPHONES, BT_DESC, false),
            make_sink_with_icon(HDMI, "HDMI / DisplayPort", false, "video-display"),
            make_sink(USB, "USB DAC", false),
        ],
        vec![
            make_rule("firefox", None, BT_DESC),
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
                name: BT_HEADPHONES.to_string(),
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
        sources: Vec::new(),
        profile_sources: Vec::new(),
    });
    app.handle_update(AppUpdate::DaemonState {
        monitor_info: Some(false),
        running: true,
        windows: vec![
            window("firefox", "Rust Book — Mozilla Firefox", Some(BT_DESC)),
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
        format!("2026-11-20T10:03:12.004Z  INFO pwsw::state: Rule matched: 'firefox' → {BT_DESC}"),
        "2026-11-20T10:03:13.900Z DEBUG pwsw::state: Window opened: app_id='kitty' title='~'"
            .to_string(),
    ]));
    app
}

const HEADSET_MIC: &str = "alsa_input.usb-Logitech_Headset-00.mono-fallback";
const WEBCAM_MIC: &str = "alsa_input.usb-Webcam_C920-02.analog-stereo";
const BUILTIN_MIC: &str = "alsa_input.pci-0000_00_1f.3.analog-stereo";

/// The fixture with three mics (in use, present, via profile), an output-only
/// rule (4), a rule with an output and two mics (5), and windows on monitors
fn with_mics_and_monitors(app: &mut App) {
    app.config.sources = vec![
        make_sink(HEADSET_MIC, "Headset Mic", true),
        make_sink(WEBCAM_MIC, "Webcam Mic", false),
        make_sink(BUILTIN_MIC, "Built-in Mic", false),
    ];
    app.config.rules.push(with_output(
        make_rule("", None, "HDMI / DisplayPort"),
        "^HDMI-A-1$",
    ));
    app.config.rules.push(with_output(
        with_sources(
            make_rule("^discord$", None, "Built-in Audio Analog Stereo"),
            &["Headset Mic", "Webcam Mic"],
        ),
        "^DP-",
    ));
    let sinks = app.active_sink_list.clone();
    app.handle_update(AppUpdate::SinksData {
        active: sinks,
        profiles: Vec::new(),
        sources: vec![
            ActiveSink {
                name: HEADSET_MIC.to_string(),
                description: "Logitech Headset Mono".to_string(),
                is_default: true,
            },
            ActiveSink {
                name: WEBCAM_MIC.to_string(),
                description: "HD Pro Webcam C920".to_string(),
                is_default: false,
            },
        ],
        profile_sources: vec![ProfileSink {
            predicted_name: BUILTIN_MIC.to_string(),
            description: "Built-in Audio Analog Stereo".to_string(),
            device_name: "alsa_card.pci-0000_00_1f.3".to_string(),
            device_id: 41,
            profile_name: "output:analog-stereo+input:analog-stereo".to_string(),
            profile_index: 3,
        }],
    });
    let placed = |mut w: WindowInfo, outputs: &[&str], minimized: bool| {
        w.outputs = outputs.iter().map(|o| (*o).to_string()).collect();
        w.minimized = minimized;
        w
    };
    app.handle_update(AppUpdate::DaemonState {
        monitor_info: Some(false),
        running: true,
        windows: vec![
            placed(
                window("firefox", "Rust Book — Mozilla Firefox", Some(BT_DESC)),
                &["DP-2"],
                false,
            ),
            placed(
                window("mpv", "big_buck_bunny.mkv", Some("HDMI / DisplayPort")),
                &["HDMI-A-1"],
                false,
            ),
            placed(
                window("discord", "#general", Some("Built-in Audio Analog Stereo")),
                &["DP-2", "HDMI-A-1"],
                false,
            ),
            placed(window("kitty", "~/src/pwsw", None), &["DP-2"], true),
            window("org.gnome.Nautilus", "Downloads", None),
        ],
        unit_installed: true,
        service_enabled: Some(true),
        paused: false,
    });
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
        ("mics", |app| {
            with_mics_and_monitors(app);
            app.current_screen = Screen::Sinks;
            app.endpoint_view = Endpoint::Source;
        }),
        ("mic add", |app| {
            with_mics_and_monitors(app);
            app.current_screen = Screen::Sinks;
            app.endpoint_view = Endpoint::Source;
            app.sources_screen.start_add();
        }),
        ("mic node picker", |app| {
            with_mics_and_monitors(app);
            app.current_screen = Screen::Sinks;
            app.endpoint_view = Endpoint::Source;
            app.sources_screen.start_add();
            let names = selector_names(&app.active_source_list, &app.profile_source_list)
                .into_iter()
                .map(str::to_string)
                .collect::<Vec<_>>();
            let names: Vec<&str> = names.iter().map(String::as_str).collect();
            app.sources_screen.open_selector(&names);
        }),
        ("mic details", |app| {
            with_mics_and_monitors(app);
            app.current_screen = Screen::Sinks;
            app.endpoint_view = Endpoint::Source;
            app.sources_screen.start_inspect();
        }),
        ("mic delete", |app| {
            with_mics_and_monitors(app);
            app.current_screen = Screen::Sinks;
            app.endpoint_view = Endpoint::Source;
            app.sources_screen.selected = 2;
            app.sources_screen.start_delete();
        }),
        ("no mics", |app| {
            app.current_screen = Screen::Sinks;
            app.endpoint_view = Endpoint::Source;
        }),
        ("rules with mics and outputs", |app| {
            with_mics_and_monitors(app);
            app.current_screen = Screen::Rules;
        }),
        ("rule edit with mics and output", |app| {
            with_mics_and_monitors(app);
            app.current_screen = Screen::Rules;
            app.rules_screen.selected = 4;
            app.rules_screen.start_edit(&app.config.rules);
            app.rules_screen.editor.focused_field = RuleField::Output;
        }),
        ("rule mic picker", |app| {
            with_mics_and_monitors(app);
            app.current_screen = Screen::Rules;
            app.rules_screen.selected = 4;
            app.rules_screen.start_edit(&app.config.rules);
            app.rules_screen
                .open_picker(Endpoint::Source, &app.config.sources);
        }),
        ("rule details output only", |app| {
            with_mics_and_monitors(app);
            app.current_screen = Screen::Rules;
            app.rules_screen.selected = 3;
            app.rules_screen.start_inspect();
        }),
        ("rule details with mics", |app| {
            with_mics_and_monitors(app);
            app.current_screen = Screen::Rules;
            app.rules_screen.selected = 4;
            app.rules_screen.start_inspect();
        }),
        ("dashboard with mics", |app| with_mics_and_monitors(app)),
        ("dashboard windows on monitors", |app| {
            with_mics_and_monitors(app);
            app.dashboard_screen.toggle_view();
        }),
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

/// Every "keys label" hint the keymap defines (the pause key also as "resume")
fn all_hints() -> Vec<String> {
    let mut hints: Vec<String> = BINDINGS
        .iter()
        .filter_map(|b| b.hint)
        .map(|h| format!("{} {}", h.keys, h.label))
        .collect();
    hints.push(format!("p {}", RESUME_HINT.0));
    hints
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

/// A status that fills the row is clipped with a visible ellipsis: the hints'
/// trailing space at the right edge does not draw over it
#[test]
fn test_footer_clips_a_long_status_with_an_ellipsis() {
    for width in [60, 80] {
        let mut app = state("sinks");
        app.set_status(
            StatusKind::Warning,
            format!(
                "Used by rule 3 ({}): retarget or delete it first",
                "x".repeat(80)
            ),
        );
        let rows = draw_rows(&mut app, width, 24);
        let last = rows.last().expect("footer");
        assert!(last.contains("⚠ Used by rule 3 (xxx"), "{last:?}");
        assert!(last.trim_end().ends_with('…'), "{width}: {last:?}");
    }
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
    assert!(rows[node].contains(BT_HEADPHONES), "{screen}");
    assert!(rows[node].contains("▌Node"), "focused marker: {screen}");
    assert!(rows[node].contains("␣ pick"), "{screen}");
    assert!(rows[row_with("Description")].contains(BT_DESC), "{screen}");
    assert!(
        rows[row_with("Icon")].contains("(auto: audio-headphones)"),
        "{screen}"
    );
    assert!(
        rows[row_with("Default")].contains("[ ] used when no rule matches"),
        "{screen}"
    );
    assert!(screen.contains("Enter apply · Esc cancel"), "{screen}");

    // The Node field shows the cursor (just after the name)
    let cursor = terminal.backend().cursor_position();
    assert!(terminal.backend().cursor_visible(), "cursor shown");
    assert_eq!(usize::from(cursor.y), node, "{cursor:?}");
    let name_end = rows[node].find(BT_HEADPHONES).expect("name") + BT_HEADPHONES.len();
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
        ("Sinks", BT_DESC),
        ("Description", "optional · shown in notifications"),
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
        assert!(row("Sony WH-1000XM4").contains("▶ current"), "{screen}");
        let builtin = row("Built-in Audio");
        assert!(builtin.contains("present"), "{screen}");
        assert!(builtin.contains("★ default"), "{screen}");
        assert!(
            row("HDMI / DisplayPort").contains("via profile"),
            "{screen}"
        );
        assert!(row("USB DAC").contains("missing"), "{screen}");
        assert_eq!(screen.matches("★ default").count(), 1, "{screen}");
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
    assert!(screen.contains("╭ Current "), "{screen}");
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
        monitor_info: Some(false),
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

/// Not an assertion: prints the 80x24 screens for review. Ignored so the suite
/// does not count it as a test (`cargo test print_80x24_screens -- --ignored --nocapture`)
#[test]
#[ignore = "review aid, asserts nothing: cargo test print_80x24_screens -- --ignored --nocapture"]
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
        sources: Vec::new(),
        profile_sources: Vec::new(),
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
        sources: Vec::new(),
        profile_sources: Vec::new(),
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
        monitor_info: Some(false),
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
    assert_eq!(app.config.sinks[1].desc, BT_DESC);

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
        monitor_info: Some(false),
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
        monitor_info: Some(false),
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
    assert!(
        screen.contains("Delay before a rule-driven switch"),
        "{screen}"
    );
    assert!(
        screen.contains("Default: 0 (switch immediately)"),
        "{screen}"
    );
}

// ============================================================================
// Mics (sources) and Output rules
// ============================================================================

/// The row containing `text` (panics with the screen if none)
fn row_with<'a>(rows: &'a [String], text: &str) -> &'a str {
    rows.iter()
        .find(|r| r.contains(text))
        .unwrap_or_else(|| panic!("no row with {text:?}:\n{}", rows.join("\n")))
}

#[test]
fn test_mics_list_marks_states_at_every_size() {
    for (width, height) in [(80, 24), (120, 40), (60, 16)] {
        let rows = draw_rows(&mut state("mics"), width, height);
        let screen = rows.join("\n");
        assert!(
            screen.contains("Mics · 3 configured · 2 present"),
            "{screen}"
        );
        assert!(
            row_with(&rows, "i sinks").contains('╭'),
            "switch hint in the title"
        );
        let headset = row_with(&rows, "Headset Mic");
        assert!(headset.contains("▶ current"), "{screen}");
        assert!(headset.contains("★ default"), "{screen}");
        assert!(
            row_with(&rows, "Webcam Mic").contains("present"),
            "{screen}"
        );
        assert!(
            row_with(&rows, "Built-in Mic").contains("via profile"),
            "{screen}"
        );
        assert!(
            rows[height as usize - 1].contains("i sinks/mics"),
            "{screen}"
        );
        // The sinks are not shown in the mic list
        assert!(!screen.contains("Sony"), "{screen}");
    }
    // The sinks list offers the mics the same way
    let rows = draw_rows(&mut state("sinks"), 80, 24);
    assert!(row_with(&rows, "i mics").contains('╭'));
}

#[test]
fn test_empty_mics_list_explains_what_mics_are_for() {
    let rows = draw_rows(&mut state("no mics"), 80, 24);
    let screen = rows.join("\n");
    assert!(screen.contains("No mics yet"), "{screen}");
    assert!(screen.contains("never touches the microphone"), "{screen}");
    assert!(
        screen.contains("to pick an input from PipeWire"),
        "{screen}"
    );
}

#[test]
fn test_mic_dialogs_name_mics() {
    let screen = draw_rows(&mut state("mic add"), 80, 24).join("\n");
    assert!(screen.contains("Add Mic"), "{screen}");
    assert!(
        screen.contains("used when no rule chooses a mic"),
        "{screen}"
    );
    let screen = draw_rows(&mut state("mic node picker"), 80, 24).join("\n");
    assert!(screen.contains("Inputs in PipeWire"), "{screen}");
    assert!(screen.contains("HD Pro Webcam C920"), "{screen}");
    assert!(screen.contains("After a profile switch"), "{screen}");
    let screen = draw_rows(&mut state("mic details"), 80, 24).join("\n");
    assert!(screen.contains("Mic Details"), "{screen}");
    assert!(screen.contains("▶ current"), "{screen}");
    let screen = draw_rows(&mut state("mic delete"), 80, 24).join("\n");
    assert!(screen.contains("Delete this mic?"), "{screen}");
    assert!(screen.contains("Built-in Mic"), "{screen}");
}

#[test]
fn test_rule_form_shows_output_and_mics_at_80x24() {
    let mut app = state("rule edit with mics and output");
    let rows = draw_rows(&mut app, 80, 24);
    let screen = rows.join("\n");
    assert!(row_with(&rows, "▌Output").contains("^DP-"), "{screen}");
    assert!(row_with(&rows, "▌Output").contains('✓'), "{screen}");
    assert!(
        row_with(&rows, "Mics ").contains("Headset Mic → Webcam Mic"),
        "{screen}"
    );
    assert!(
        row_with(&rows, "Monitors: ").contains("DP-2, HDMI-A-1"),
        "{screen}"
    );
    // discord is on DP-2: the preview matches it and shows its monitors
    assert!(screen.contains("Matching windows (1)"), "{screen}");
    assert!(
        screen.contains("discord | #general · DP-2,HDMI-A-1"),
        "{screen}"
    );

    // An invalid Output is marked and explained
    app.rules_screen.editor.output_pattern = tui_input::Input::new("(".to_string());
    let screen = draw_rows(&mut app, 80, 24).join("\n");
    assert!(screen.contains("✗ invalid"), "{screen}");
    assert!(screen.contains("Invalid output regex"), "{screen}");

    // Without an App ID the field reads "any app" while Output is set
    app.rules_screen.editor.output_pattern = tui_input::Input::new("^DP-".to_string());
    app.rules_screen.editor.app_id_pattern = tui_input::Input::default();
    let rows = draw_rows(&mut app, 80, 24);
    assert!(row_with(&rows, "App ID ").contains("any app"));
    // firefox and discord are on DP-2; kitty is minimized there
    assert!(rows.join("\n").contains("Matching windows (2)"));
}

#[test]
fn test_rule_form_without_mics_says_to_configure_them() {
    let rows = draw_rows(&mut state("rule edit"), 80, 24);
    assert!(
        row_with(&rows, "Mics ").contains("configure mics first"),
        "{}",
        rows.join("\n")
    );
    assert!(row_with(&rows, "Output ").contains("any monitor"));
    // The fixture's daemon is on ext: monitors are not tracked, and it says why
    assert!(row_with(&rows, "Monitors: ").contains("none (daemon on ext; see pwsw doctor)"));
}

#[test]
fn test_rule_form_monitors_hint_on_wlr_without_monitors() {
    let mut app = state("rule edit");
    app.daemon_monitor_info = Some(true);
    let rows = draw_rows(&mut app, 80, 24);
    assert!(
        row_with(&rows, "Monitors: ").contains("none reported yet"),
        "{}",
        rows.join("\n")
    );
}

/// The daemon runs but its status could not be read: the form does not claim it is
/// on ext; the ext wording fits the narrowest layout
#[test]
fn test_rule_form_monitors_hint_when_the_status_is_unknown() {
    let mut app = state("rule edit");
    app.daemon_monitor_info = None;
    let rows = draw_rows(&mut app, 80, 24);
    let row = row_with(&rows, "Monitors: ");
    assert!(row.contains("Monitors: none reported "), "{row}");
    assert!(!row.contains("ext"), "{row}");

    app.daemon_monitor_info = Some(false);
    let rows = draw_rows(&mut app, 60, 24);
    let row = row_with(&rows, "Monitors: ");
    assert!(
        row.contains("none (daemon on ext; see pwsw doctor)"),
        "{}",
        rows.join("\n")
    );
}

#[test]
fn test_rule_mic_picker_numbers_the_chosen_mics() {
    let rows = draw_rows(&mut state("rule mic picker"), 80, 24);
    let screen = rows.join("\n");
    assert!(screen.contains("Choose Mics (in order)"), "{screen}");
    let line_of = |text: &str| {
        rows.iter()
            .position(|r| r.contains(text) && r.contains("alsa_input"))
            .unwrap_or_else(|| panic!("{text}\n{screen}"))
    };
    assert!(
        [
            line_of("1 Headset Mic"),
            line_of("2 Webcam Mic"),
            line_of("  Built-in Mic")
        ]
        .is_sorted(),
        "{screen}"
    );
}

#[test]
fn test_rules_table_shows_output_rules_and_mics() {
    for (width, height) in [(80, 24), (120, 40)] {
        let rows = draw_rows(&mut state("rules with mics and outputs"), width, height);
        let screen = rows.join("\n");
        let header = row_with(&rows, "Window");
        assert!(header.contains("Mic"), "{screen}");
        // The output-only rule: no empty App ID
        let output_only = row_with(&rows, "│  4   ");
        assert!(output_only.contains("any app on HDMI-A-1"), "{screen}");
        let discord = row_with(&rows, "^discord$");
        assert!(discord.contains("on ^DP-"), "{screen}");
        assert!(discord.contains("Headset"), "{screen}");
        assert_eq!(header.contains("Description"), width >= 100, "{screen}");
        assert!(row_with(&rows, "firefox").contains('—'), "no mic: {screen}");
    }
    // Without mics in any rule there is no Mic column
    let rows = draw_rows(&mut state("rules"), 80, 24);
    assert!(!row_with(&rows, "Window").contains("Mic"));
}

#[test]
fn test_rule_details_list_every_condition_and_target() {
    let screen = draw_rows(&mut state("rule details output only"), 80, 24).join("\n");
    for text in [
        "App ID       any app",
        "Output       ^HDMI-A-1$",
        "Sink         HDMI / DisplayPort",
        "Mic          none · the mic is left alone",
    ] {
        assert!(screen.contains(text), "{text}\n{screen}");
    }
    let screen = draw_rows(&mut state("rule details with mics"), 80, 24).join("\n");
    for text in [
        "App ID       ^discord$",
        "Output       ^DP-",
        "Mics         Headset Mic",
        "→ Webcam Mic",
    ] {
        assert!(screen.contains(text), "{text}\n{screen}");
    }
}

#[test]
fn test_windows_view_shows_monitors_and_drops_them_when_narrow() {
    let rows = draw_rows(&mut state("dashboard windows on monitors"), 120, 40);
    let screen = rows.join("\n");
    assert!(row_with(&rows, "App ID").contains("Monitor"), "{screen}");
    assert!(row_with(&rows, "mpv").contains("HDMI-A-1"), "{screen}");
    assert!(
        row_with(&rows, "discord").contains("DP-2,HDMI-A-1"),
        "{screen}"
    );
    assert!(row_with(&rows, "kitty").contains("DP-2 (min)"), "{screen}");
    assert!(row_with(&rows, "Nautilus").contains('—'), "{screen}");

    let rows = draw_rows(&mut state("dashboard windows on monitors"), 80, 24);
    assert!(row_with(&rows, "App ID").contains("Monitor"));
    let rows = draw_rows(&mut state("dashboard windows on monitors"), 60, 16);
    let header = row_with(&rows, "App ID");
    assert!(
        !header.contains("Monitor"),
        "dropped first:\n{}",
        rows.join("\n")
    );
    assert!(header.contains("Sink"));
}

#[test]
fn test_dashboard_shows_the_current_mic_when_mics_are_configured() {
    let rows = draw_rows(&mut state("dashboard with mics"), 80, 24);
    let screen = rows.join("\n");
    assert!(row_with(&rows, "Mic Headset Mic").contains('│'), "{screen}");
    assert!(screen.contains("4 sinks · 3 mics · 5 rules"), "{screen}");
    // Without mics the card shows the sink's node name instead
    let screen = draw_rows(&mut state("dashboard"), 80, 24).join("\n");
    assert!(!screen.contains("Mic "), "{screen}");
}

/// Not an assertion: prints the 80x24 mic and output screens for review. Ignored
/// like [`test_print_80x24_screens`] (`cargo test print_80x24_mic -- --ignored --nocapture`)
#[test]
#[ignore = "review aid, asserts nothing: cargo test print_80x24_mic -- --ignored --nocapture"]
fn test_print_80x24_mic_screens() {
    for name in [
        "mics",
        "no mics",
        "rule edit with mics and output",
        "rules with mics and outputs",
        "dashboard windows on monitors",
        "dashboard with mics",
    ] {
        let rows = draw_rows(&mut state(name), 80, 24);
        println!("--- {name} ---\n{}", rows.join("\n"));
    }
    let rows = draw_rows(&mut state("rule edit with mics and output"), 60, 16);
    println!(
        "--- rule edit with mics and output (60x16) ---\n{}",
        rows.join("\n")
    );
}

/// The footer of a dialog names the same keys as the dialog: delete or close,
/// never a generic "ok"
#[test]
fn test_dialog_footers_match_their_dialogs() {
    for (name, wanted) in [
        ("sink delete", &["Enter delete", "Esc cancel"][..]),
        ("rule delete", &["Enter delete", "Esc cancel"]),
        ("sink details", &["Enter close"]),
        ("rule details", &["Enter close"]),
    ] {
        let rows = draw_rows(&mut state(name), 80, 24);
        let footer = rows.last().expect("footer");
        for hint in wanted {
            assert!(footer.contains(hint), "{name}: {hint:?} in {footer:?}");
        }
        assert!(!footer.contains("Enter ok"), "{name}: {footer:?}");
    }
}

/// A dialog clears a one-cell margin around itself: no text of the screen behind
/// it touches its border (seen as `Monitors: none╰────╯m)` before)
#[test]
fn test_dialogs_keep_a_blank_margin() {
    for (name, title) in [
        ("conflict", "Config Changed on Disk"),
        ("sink delete", "Delete Sink"),
        ("rule details", "Rule Details"),
        ("settings switch delay", "Switch Delay"),
    ] {
        for (width, height) in [(80, 24), (60, 16)] {
            let rows = draw_rows(&mut state(name), width, height);
            let grid: Vec<Vec<char>> = rows.iter().map(|r| r.chars().collect()).collect();
            let screen = rows.join("\n");
            // The dialog's top-left corner, found by its title
            let corner: Vec<char> = format!("╭ {title} ").chars().collect();
            let (top, left) = grid
                .iter()
                .enumerate()
                .find_map(|(y, row)| {
                    row.windows(corner.len())
                        .position(|w| w == corner.as_slice())
                        .map(|x| (y, x))
                })
                .expect("the dialog");
            let right = left
                + grid[top][left..]
                    .iter()
                    .position(|&c| c == '╮')
                    .expect("top-right corner");
            let bottom = (top..grid.len())
                .find(|&y| grid[y][left] == '╰')
                .expect("bottom-left corner");
            let last_col = usize::from(width) - 1;
            for (y, row) in grid.iter().enumerate().take(bottom + 1).skip(top) {
                for x in [left.checked_sub(1), Some(right + 1)].into_iter().flatten() {
                    // The outermost columns are panel borders, kept unless covered
                    if x == 0 || x >= last_col {
                        continue;
                    }
                    assert_eq!(
                        row[x], ' ',
                        "{name} at {width}x{height}: ({x}, {y}) next to the dialog\n{screen}"
                    );
                }
            }
            for y in [top.checked_sub(1), Some(bottom + 1)].into_iter().flatten() {
                // Header and footer are drawn over the margin
                if y == 0 || y + 1 >= grid.len() {
                    continue;
                }
                let from = left.saturating_sub(1).max(1);
                let to = (right + 1).min(last_col - 1);
                assert!(
                    grid[y][from..=to].iter().all(|&c| c == ' '),
                    "{name} at {width}x{height}: row {y} touches the dialog\n{screen}"
                );
            }
        }
    }
}

/// The pause key's footer hint follows the daemon: hidden while it is stopped,
/// "resume" while paused, and kept at 80x24 even with unsaved changes
#[test]
fn test_footer_pause_hint_follows_the_daemon() {
    let footer = |app: &mut App| draw_rows(app, 80, 24).pop().expect("footer");

    let mut app = state("dashboard");
    app.daemon_running = true;
    assert!(footer(&mut app).contains("p pause"), "running");

    app.daemon_running = false;
    let stopped = footer(&mut app);
    assert!(
        !stopped.contains("p pause") && !stopped.contains("p resume"),
        "{stopped}"
    );

    let mut app = state("dashboard paused");
    app.daemon_running = true;
    let paused = footer(&mut app);
    assert!(app.config_dirty, "unsaved changes too");
    assert!(paused.contains("p resume"), "{paused}");
    assert!(paused.contains("Ctrl+S save"), "{paused}");
    assert!(!paused.contains("p pause"), "{paused}");
}

/// Each About text starts with its own words, not the summary line repeated
#[test]
fn test_settings_about_does_not_repeat_the_summary() {
    for (index, item) in SettingItem::all().iter().enumerate() {
        let detail = item.detailed_description();
        let first = detail.lines().next().unwrap_or_default();
        assert_ne!(first.trim_end_matches('.'), item.description(), "{item:?}");
        let mut app = state("settings");
        app.settings_screen.selected = index;
        let screen = draw_rows(&mut app, 120, 40).join("\n");
        assert_eq!(
            screen.matches(item.description()).count(),
            1,
            "{item:?} summary shown once\n{screen}"
        );
        // Heading markers are never shown
        assert!(!screen.contains("# "), "{item:?}\n{screen}");
    }
}

/// Whether `c` must never reach a cell: control characters and the Unicode line
/// separators and bidirectional controls
fn is_unsafe_in_cell(c: char) -> bool {
    c.is_control()
        || matches!(
            c,
            '\u{2028}' | '\u{2029}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}'
        )
}

/// The fixture with escapes and bidi overrides in a configured sink, a rule, the
/// `PipeWire` node descriptions and names, and window titles and `app_id`s
fn hostile(app: &mut App) {
    const EVIL: &str = "USB\u{202e}CAD\u{1b}[2J";
    app.config.sinks[3].desc = EVIL.to_string();
    app.config.sinks[3].name = "alsa_output.usb-\u{202e}Old\u{1b}]0;x\u{7}.analog-stereo".into();
    app.config
        .rules
        .push(make_rule("ev\u{202e}il", Some("ti\u{202e}tle"), EVIL));
    let mut active = app.active_sink_list.clone();
    for sink in &mut active {
        sink.description = format!("{}\u{202e}\u{1b}[31m", sink.description);
    }
    active.push(ActiveSink {
        name: "bluez_output.\u{202e}\u{1b}[2J.1".to_string(),
        description: "Phones\u{202e}\n\u{1b}[2J".to_string(),
        is_default: false,
    });
    let profiles = app
        .profile_sink_list
        .iter()
        .map(|p| ProfileSink {
            predicted_name: p.predicted_name.clone(),
            description: format!("{}\u{202e}", p.description),
            device_name: p.device_name.clone(),
            device_id: p.device_id,
            profile_name: p.profile_name.clone(),
            profile_index: p.profile_index,
        })
        .collect();
    app.handle_update(AppUpdate::SinksData {
        active,
        profiles,
        sources: Vec::new(),
        profile_sources: Vec::new(),
    });
    app.handle_update(AppUpdate::DaemonState {
        monitor_info: Some(false),
        running: true,
        windows: vec![
            window("firefox", "Evil\u{202e}eltit\u{1b}]0;x\u{7}", Some(BT_DESC)),
            window("ev\u{202e}il", "ti\u{202e}tle\nforged", Some(EVIL)),
            window("org.\u{202e}kde\u{1b}[2J", "\u{2066}x\u{2069}", None),
        ],
        unit_installed: true,
        service_enabled: Some(true),
        paused: false,
    });
}

#[test]
fn test_untrusted_text_never_puts_control_or_bidi_characters_into_cells() {
    let edit_hostile_sink: Setup = |app| {
        app.current_screen = Screen::Sinks;
        app.sinks_screen.selected = 3;
        app.sinks_screen.start_edit(&app.config.sinks);
    };
    for (name, setup) in states()
        .into_iter()
        .chain([("hostile sink edit", edit_hostile_sink)])
    {
        let mut app = fixture();
        hostile(&mut app);
        setup(&mut app);
        let terminal = terminal(&mut app, 140, 40);
        let screen = rows_of(&terminal).join("\n");
        for cell in terminal.backend().buffer().content() {
            assert!(
                !cell.symbol().chars().any(is_unsafe_in_cell),
                "{name}: cell {:?} reached the screen:\n{screen}",
                cell.symbol()
            );
        }
    }
}
