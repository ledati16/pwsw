//! Every key the help documents must do something in its context.
//!
//! For each binding of [`crate::tui::keymap::BINDINGS`] and each key press it
//! lists, the test builds realistic states for the binding's context (different
//! selections, fields and scroll positions), feeds the key to the real input
//! handler and requires that at least one state changed.

use std::path::Path;

use crossterm::event::KeyEvent;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use tempfile::TempDir;
use tui_input::Input;

use crate::config::Config;
use crate::ipc::WindowInfo;
use crate::pipewire::{ActiveSink, ProfileSink};
use crate::test_utils::fixtures::{make_config, make_rule, make_sink};
use crate::tui::app::{App, AppUpdate, Screen, StatusKind};
use crate::tui::input::simulate_key_event;
use crate::tui::keymap::{BINDINGS, Context};
use crate::tui::render_ui;
use crate::tui::screens::dashboard::DashboardView;
use crate::tui::screens::list::{FormEditor, ListMode};
use crate::tui::screens::rules::RuleField;
use crate::tui::screens::sinks::{SinkField, selector_names};

fn draw(app: &mut App) {
    let mut terminal = Terminal::new(TestBackend::new(120, 40)).expect("test terminal");
    app.refresh_preview();
    terminal
        .draw(|frame| render_ui(frame, app))
        .expect("draw succeeds");
}

/// An app with plenty of everything, a config file in `dir`, unsaved changes
/// and a status message.
fn base(dir: &Path) -> App {
    let path = dir.join("config.toml");
    make_config(
        vec![
            make_sink("s1", "Sink One", true),
            make_sink("s2", "Sink Two", false),
            make_sink("s3", "Sink Three", false),
        ],
        vec![
            make_rule("a", None, "Sink One"),
            make_rule("b", Some("t"), "Sink Two"),
            make_rule("c", None, "Sink Three"),
        ],
    )
    .save_to(&path)
    .expect("write config");
    let config = Config::load_from_path(&path).expect("load config");
    let mut app = App::new(config, path);
    app.config.settings.notify_rules.toggle();
    app.mark_dirty();

    app.handle_update(AppUpdate::DaemonLogs(
        (0..200)
            .map(|i| format!("2025-01-01T00:00:00Z INFO line {i}"))
            .collect(),
    ));
    app.handle_update(AppUpdate::DaemonState {
        running: true,
        windows: (0..40)
            .map(|i| WindowInfo {
                id: Some(i),
                app_id: format!("app{i}"),
                title: format!("title {i}"),
                matched_on: None,
                tracked: None,
            })
            .collect(),
        unit_installed: true,
        service_enabled: Some(true),
    });
    let active = |name: &str| ActiveSink {
        name: name.to_string(),
        description: format!("{name} desc"),
        is_default: false,
    };
    app.handle_update(AppUpdate::SinksData {
        active: vec![active("n1"), active("n2"), active("n3")],
        profiles: vec![ProfileSink {
            predicted_name: "n4".to_string(),
            description: "n4 desc".to_string(),
            device_id: 1,
            device_name: "dev".to_string(),
            profile_index: 1,
            profile_name: "p".to_string(),
        }],
    });
    // An info message: it expires by time, so no key press clears it as a side effect
    app.set_status(StatusKind::Info, "status");
    app
}

type Setup = Box<dyn Fn(&mut App)>;

/// The Dashboard in `view`, at the top and in the middle of the lists
fn dashboard_setups(view: DashboardView) -> Vec<Setup> {
    [false, true]
        .into_iter()
        .map(|middle| -> Setup {
            Box::new(move |app: &mut App| {
                app.current_screen = Screen::Dashboard;
                app.dashboard_screen.current_view = view;
                draw(app); // records the scroll limits
                if middle {
                    app.dashboard_screen.log_scroll_offset = 5;
                    app.dashboard_screen.window_scroll_offset = 5;
                    app.dashboard_screen.window_selected = 7;
                    app.dashboard_screen.selected_action = 1;
                }
            })
        })
        .collect()
}

/// Realistic states in which bindings of `context` apply.
fn setups(context: Context) -> Vec<Setup> {
    let mut out: Vec<Setup> = Vec::new();
    match context {
        Context::Global => {
            for &screen in Screen::all() {
                out.push(Box::new(move |app: &mut App| app.current_screen = screen));
            }
        }
        Context::List(Screen::Dashboard) => {
            for view in [DashboardView::Logs, DashboardView::Windows] {
                out.extend(dashboard_setups(view));
            }
        }
        Context::DashboardView(view) => out.extend(dashboard_setups(view)),
        Context::List(screen) => {
            for selected in [0, 1] {
                out.push(Box::new(move |app: &mut App| {
                    app.current_screen = screen;
                    app.sinks_screen.selected = selected;
                    app.rules_screen.selected = selected;
                    app.settings_screen.selected = selected;
                    app.settings_screen.desc_scroll = u16::try_from(selected).unwrap_or(0);
                }));
            }
        }
        Context::Editor(Screen::Sinks) => {
            for field in [
                SinkField::Name,
                SinkField::Desc,
                SinkField::Icon,
                SinkField::Default,
            ] {
                out.push(Box::new(move |app: &mut App| {
                    app.current_screen = Screen::Sinks;
                    app.sinks_screen.selected = 1;
                    app.sinks_screen.start_edit(&app.config.sinks);
                    app.sinks_screen.editor.focused_field = field;
                }));
            }
        }
        Context::Editor(_) => {
            for field in [
                RuleField::AppId,
                RuleField::Title,
                RuleField::Sinks,
                RuleField::Desc,
                RuleField::Notify,
            ] {
                out.push(Box::new(move |app: &mut App| {
                    app.current_screen = Screen::Rules;
                    app.rules_screen.selected = 1;
                    app.rules_screen.start_edit(&app.config.rules);
                    app.rules_screen.editor.focused_field = field;
                }));
            }
        }
        Context::Dropdown(Screen::Sinks) => {
            for down in [false, true] {
                out.push(Box::new(move |app: &mut App| {
                    app.current_screen = Screen::Sinks;
                    app.sinks_screen.start_add();
                    let names = selector_names(&app.active_sink_list, &app.profile_sink_list);
                    app.sinks_screen.open_selector(&names);
                    if down {
                        app.sinks_screen.move_selector(&names, true);
                    }
                }));
            }
        }
        Context::Dropdown(Screen::Rules) => {
            // Nothing chosen yet, and two sinks chosen
            for chosen in [false, true] {
                for index in [0, 1] {
                    out.push(Box::new(move |app: &mut App| {
                        app.current_screen = Screen::Rules;
                        app.rules_screen.start_add();
                        app.rules_screen.open_sink_selector(&app.config.sinks);
                        if chosen {
                            app.rules_screen.editor.picked = vec![2, 0];
                        }
                        app.rules_screen.editor.sink_dropdown_index = index;
                    }));
                }
            }
        }
        Context::Dropdown(_) => {
            out.push(Box::new(|app: &mut App| {
                app.current_screen = Screen::Settings;
                app.settings_screen.selected = 5;
                app.settings_screen.editing_log_level = true;
                app.settings_screen.log_level_index = 2;
            }));
        }
        Context::Dialog(screen) => {
            for inspect in [false, true] {
                out.push(Box::new(move |app: &mut App| {
                    app.current_screen = screen;
                    app.sinks_screen.selected = 1;
                    app.rules_screen.selected = 1;
                    app.sinks_screen.mode = if inspect {
                        ListMode::Inspect
                    } else {
                        ListMode::Delete
                    };
                    app.rules_screen.mode = if inspect {
                        ListMode::Inspect
                    } else {
                        ListMode::Delete
                    };
                }));
            }
        }
        Context::Discard(screen) => {
            out.push(Box::new(move |app: &mut App| {
                app.current_screen = screen;
                app.sinks_screen.selected = 1;
                app.rules_screen.selected = 1;
                if screen == Screen::Sinks {
                    app.sinks_screen.start_edit(&app.config.sinks);
                    app.sinks_screen.editor.default.toggle();
                } else {
                    app.rules_screen.start_edit(&app.config.rules);
                    app.rules_screen.editor.notify = Some(true);
                }
                simulate_key_event(app, KeyEvent::from(crossterm::event::KeyCode::Esc));
                assert!(
                    app.sinks_screen.confirm_discard || app.rules_screen.confirm_discard,
                    "Esc with edits asks first"
                );
            }));
        }
        Context::Conflict => {
            for screen in [Screen::Dashboard, Screen::Rules] {
                out.push(Box::new(move |app: &mut App| {
                    app.current_screen = screen;
                    app.save_conflict = true;
                }));
            }
        }
        Context::TextInput => {
            let text = || Input::new("hello world".to_string()).with_cursor(5);
            for field in [SinkField::Name, SinkField::Desc, SinkField::Icon] {
                out.push(Box::new(move |app: &mut App| {
                    app.current_screen = Screen::Sinks;
                    app.sinks_screen.start_add();
                    app.sinks_screen.editor.focused_field = field;
                    if let Some(input) = app.sinks_screen.editor.focused_input_mut() {
                        *input = text();
                    }
                }));
            }
            for field in [RuleField::AppId, RuleField::Title, RuleField::Desc] {
                out.push(Box::new(move |app: &mut App| {
                    app.current_screen = Screen::Rules;
                    app.rules_screen.start_add();
                    app.rules_screen.editor.focused_field = field;
                    if let Some(input) = app.rules_screen.editor.focused_input_mut() {
                        *input = text();
                    }
                }));
            }
        }
        Context::Help => {
            for middle in [false, true] {
                out.push(Box::new(move |app: &mut App| {
                    app.current_screen = Screen::Rules;
                    app.open_help();
                    draw(app); // records the scroll limits
                    if middle {
                        app.help.offset = 4;
                    }
                }));
            }
        }
    }
    out
}

/// Everything a key press could observably change.
fn fingerprint(app: &App) -> String {
    format!(
        "{:?}|{}|{}|{}|{}|{}|{:?}|{:?}|{:?}|{:?}|{:?}|{:?}|{:?}|{:?}|{:?}",
        app.current_screen,
        app.should_quit,
        app.show_help,
        app.confirm_quit,
        app.config_dirty,
        app.save_conflict,
        app.status_message(),
        app.config.settings,
        app.config.sinks,
        app.config.rules,
        app.sinks_screen,
        app.rules_screen,
        app.settings_screen,
        app.dashboard_screen,
        app.help,
    )
}

#[test]
fn test_every_documented_key_does_something() {
    let dir = TempDir::new().expect("tempdir");
    let mut failures = Vec::new();
    for binding in BINDINGS {
        for &(code, modifiers) in binding.chords {
            let acted = setups(binding.context).iter().any(|setup| {
                let mut app = base(dir.path());
                setup(&mut app);
                let before = fingerprint(&app);
                simulate_key_event(&mut app, KeyEvent::new(code, modifiers));
                fingerprint(&app) != before
            });
            if !acted {
                failures.push(format!(
                    "{:?}: \"{}\" ({code:?} {modifiers:?}) does nothing",
                    binding.context, binding.keys
                ));
            }
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

#[test]
fn test_every_binding_lists_its_keys() {
    for binding in BINDINGS {
        assert!(
            !binding.chords.is_empty() || binding.keys == "Paste",
            "{} has no key presses to check",
            binding.keys
        );
        assert_ne!(binding.description, "");
    }
}

#[test]
fn test_footer_hints_come_from_bindings_and_fit() {
    use crate::tui::keymap::{Hint, footer_hints};

    let mut contexts: Vec<Context> = Vec::new();
    for binding in BINDINGS {
        if !contexts.contains(&binding.context) {
            contexts.push(binding.context);
        }
        if let Some(hint) = binding.hint {
            assert!(
                binding.keys.contains(hint.keys),
                "hint keys {:?} are not among {:?}",
                hint.keys,
                binding.keys
            );
            assert!(!hint.label.is_empty() && !hint.label.contains("  "));
        }
    }
    for context in contexts {
        if context == Context::TextInput {
            continue; // typed into, never the footer's context
        }
        let active = match context {
            Context::List(_) => vec![context, Context::Global],
            Context::DashboardView(_) => {
                vec![Context::List(Screen::Dashboard), context, Context::Global]
            }
            _ => vec![context],
        };
        for width in [40, 76, 200] {
            let hints = footer_hints(&active, true, width);
            let used: usize =
                hints.iter().map(Hint::width).sum::<usize>() + 2 * hints.len().saturating_sub(1);
            assert!(used <= width, "{context:?} at {width}: {used}");
            assert!(!hints.is_empty(), "{context:?} has footer hints");
            // The most important hint is always kept
            let best = BINDINGS
                .iter()
                .filter(|b| active.contains(&b.context))
                .filter_map(|b| b.hint)
                .min_by_key(|h| h.priority)
                .expect("a hint");
            assert!(hints.contains(&best), "{context:?} at {width}");
        }
        // Ctrl+S only when there is something to save
        let clean = footer_hints(&active, false, 200);
        assert!(clean.iter().all(|h| !h.only_unsaved), "{context:?}");
    }
}
