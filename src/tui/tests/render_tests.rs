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
        unit_installed: false,
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

/// Draw `app` into a `width`×`height` terminal and return the rows.
fn draw_rows(app: &mut App, width: u16, height: u16) -> Vec<String> {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("test terminal");
    app.refresh_preview();
    terminal
        .draw(|frame| render_ui(frame, app))
        .expect("draw succeeds");
    let buffer = terminal.backend().buffer();
    buffer
        .content()
        .chunks(usize::from(width))
        .map(|row| {
            row.iter()
                .map(ratatui::buffer::Cell::symbol)
                .collect::<String>()
        })
        .collect()
}

fn windows_app(count: u64) -> App {
    let mut app = App::with_config(make_config(vec![], vec![]));
    app.handle_update(AppUpdate::DaemonState {
        running: true,
        windows: (0..count)
            .map(|i| crate::ipc::WindowInfo {
                id: Some(i),
                app_id: format!("app{i:02}"),
                title: String::new(),
                matched_on: None,
                tracked: None,
            })
            .collect(),
        unit_installed: false,
        service_enabled: None,
    });
    app.dashboard_screen.toggle_view();
    app
}

#[test]
fn test_windows_view_scrolls_down_with_pagedown_and_shows_position() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    use crate::tui::input::simulate_key_event;

    let mut app = windows_app(40);
    let screen = draw(&mut app);
    assert!(screen.contains("app00"), "{screen}");
    assert!(screen.contains("[1-"), "indicator visible: {screen}");
    assert!(screen.contains("/40]"), "{screen}");
    let page = app.dashboard_screen.page_size();
    assert!(page > 1 && page < 40, "page from the viewport: {page}");

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

    // End shows the last window; PageUp goes back up
    simulate_key_event(&mut app, KeyEvent::new(KeyCode::End, KeyModifiers::NONE));
    let screen = draw(&mut app);
    assert!(screen.contains("app39"), "{screen}");
    assert!(screen.contains("-40/40]"), "{screen}");
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
    use ratatui::layout::Rect;

    use crate::tui::screens::settings::LOG_LEVELS;
    use crate::tui::widgets::{centered_modal, modal_size};

    let mut app = App::with_config(make_config(vec![], vec![]));
    app.current_screen = Screen::Settings;
    app.settings_screen.selected = 5; // Log Level
    app.settings_screen.editing_log_level = true;

    let (width, height) = (140, 40);
    let rows = draw_rows(&mut app, width, height);
    let popup = centered_modal(modal_size::DROPDOWN, Rect::new(0, 0, width, height));
    for y in popup.y + 1..popup.y + popup.height - 1 {
        let row: String = rows[usize::from(y)]
            .chars()
            .skip(usize::from(popup.x + 1))
            .take(usize::from(popup.width - 2))
            .collect();
        let content = row.replace('→', "");
        let content = content.trim();
        assert!(
            content.is_empty() || LOG_LEVELS.contains(&content),
            "row {y} shows background text: {row:?}\n{}",
            rows.join("\n")
        );
    }
}

#[test]
fn test_tiny_terminals_render_without_panic() {
    use crate::tui::screens::rules::RulesMode;
    use crate::tui::screens::sinks::SinksMode;

    for (width, height) in [(1, 1), (3, 10), (4, 20), (4, 3), (20, 4)] {
        for &screen in Screen::all() {
            let mut app =
                App::with_config(make_config(vec![make_sink("s1", "Sink One", true)], vec![]));
            app.handle_update(AppUpdate::DaemonLogs(vec![
                "2025-01-01T00:00:00Z INFO hello ünïcödé".to_string(),
            ]));
            app.current_screen = screen;
            draw_rows(&mut app, width, height);
            // With modals and overlays open
            app.sinks_screen.mode = SinksMode::Inspect;
            app.rules_screen.mode = RulesMode::AddEdit;
            app.settings_screen.editing_log_level = true;
            app.dashboard_screen.toggle_view();
            draw_rows(&mut app, width, height);
            app.show_help = true;
            app.save_conflict = true;
            draw_rows(&mut app, width, height);
        }
    }
}

#[test]
fn test_conflict_dialog_is_drawn() {
    let mut app = App::with_config(make_config(vec![], vec![]));
    app.save_conflict = true;
    let screen = draw(&mut app);
    assert!(screen.contains("Config Changed on Disk"), "{screen}");
    assert!(screen.contains("[o]"), "{screen}");
    assert!(screen.contains("Reload from disk"), "{screen}");
}

#[test]
fn test_dashboard_shows_enable_with_installed_unit() {
    let mut app = App::with_config(make_config(vec![], vec![]));
    app.handle_update(AppUpdate::DaemonState {
        running: true,
        windows: Vec::new(),
        unit_installed: true,
        service_enabled: Some(false),
    });
    let screen = draw(&mut app);
    assert!(screen.contains("ENABLE"), "{screen}");
    assert!(screen.contains("disabled"), "{screen}");
}
