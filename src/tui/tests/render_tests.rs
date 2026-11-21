//! Rendering tests: draw the real UI into a `TestBackend` from in-memory state.

use ratatui::Terminal;
use ratatui::backend::TestBackend;

use crate::pipewire::ActiveSink;
use crate::test_utils::fixtures::{make_config, make_sink, make_sink_with_icon};
use crate::tui::app::{App, AppUpdate, Screen};
use crate::tui::editor_state::EditorState;
use crate::tui::render_ui;

/// Draw `app` like the event loop does and return the screen as text.
fn draw(app: &mut App) -> String {
    let mut terminal = Terminal::new(TestBackend::new(140, 40)).expect("test terminal");
    app.refresh_preview();
    terminal
        .draw(|frame| render_ui(frame, app))
        .expect("draw succeeds");
    let buffer = terminal.backend().buffer();
    let width = usize::from(buffer.area.width);
    buffer
        .content()
        .chunks(width)
        .map(|row| {
            row.iter()
                .map(ratatui::buffer::Cell::symbol)
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn sinks_data(name: &str, description: &str) -> AppUpdate {
    AppUpdate::SinksData {
        active: vec![
            ActiveSink {
                name: "alsa_output.other".to_string(),
                description: "Other Output".to_string(),
                is_default: false,
            },
            ActiveSink {
                name: name.to_string(),
                description: description.to_string(),
                is_default: true,
            },
        ],
        profiles: Vec::new(),
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
    app.handle_update(sinks_data("alsa_output.test", "PipeWire Name"));

    let screen = draw(&mut app);
    assert!(screen.contains("Test Headphones"), "{screen}");
    assert!(screen.contains("alsa_output.test"), "{screen}");
}

#[test]
fn test_dashboard_shows_unconfigured_default_sink_by_pipewire_name() {
    let mut app = App::with_config(make_config(vec![], vec![]));
    app.handle_update(sinks_data("alsa_output.usb", "USB Interface"));

    let screen = draw(&mut app);
    assert!(screen.contains("USB Interface"), "{screen}");
}

#[test]
fn test_dashboard_without_snapshot_shows_unknown() {
    let mut app = App::with_config(make_config(vec![], vec![]));
    let screen = draw(&mut app);
    assert!(screen.contains("Unknown Sink"), "{screen}");

    app.handle_update(AppUpdate::PipeWireUnavailable);
    let screen = draw(&mut app);
    assert!(screen.contains("PipeWire unavailable"), "{screen}");
}

#[test]
fn test_rule_editor_shows_invalid_title_regex() {
    let mut app = App::with_config(make_config(vec![make_sink("s1", "d1", true)], vec![]));
    app.current_screen = Screen::Rules;
    app.rules_screen.start_add();
    app.rules_screen.editor.app_id_pattern = EditorState::from_string("firefox".to_string());
    app.rules_screen.editor.title_pattern = EditorState::from_string("(oops".to_string());

    let screen = draw(&mut app);
    assert!(screen.contains("Invalid title regex"), "{screen}");
}

#[test]
fn test_rule_editor_preview_follows_window_updates() {
    let mut app = App::with_config(make_config(vec![make_sink("s1", "d1", true)], vec![]));
    app.current_screen = Screen::Rules;
    app.rules_screen.start_add();
    app.rules_screen.editor.app_id_pattern = EditorState::from_string("^mpv$".to_string());

    let window = |app_id: &str, title: &str| crate::ipc::WindowInfo {
        id: None,
        app_id: app_id.to_string(),
        title: title.to_string(),
        matched_on: None,
        tracked: None,
    };
    let state = |windows| AppUpdate::DaemonState {
        running: true,
        windows,
        daemon_manager: None,
        service_enabled: None,
    };

    app.handle_update(state(vec![window("firefox", "Browser")]));
    let screen = draw(&mut app);
    assert!(screen.contains("No matching windows"), "{screen}");

    app.handle_update(state(vec![
        window("firefox", "Browser"),
        window("mpv", "Some Video"),
    ]));
    let screen = draw(&mut app);
    assert!(screen.contains("mpv | Some Video"), "{screen}");
}
