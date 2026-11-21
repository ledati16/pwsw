//! `IPC` requests served by `handle_connection` against the real loop: rule tests,
//! window lists, manager info, and shutting down

use pretty_assertions::assert_eq;

use super::*;

/// `request` as one frame to a server in front of `commands`
async fn exchange(commands: &mpsc::Sender<Command>, request: &Request) -> Response {
    let body = serde_json::to_vec(request).expect("serialize");
    ipc_exchange(commands, &body, None).await
}

/// `(id, app_id, title, matched_on)` of each match
fn summary(response: Response) -> Vec<(u64, String, String, Option<String>)> {
    let Response::RuleMatches { matches, .. } = response else {
        panic!("expected rule matches, got {response:?}");
    };
    matches
        .into_iter()
        .map(|w| (w.id.expect("an id"), w.app_id, w.title, w.matched_on))
        .collect()
}

/// `test-rule PATTERN`: every window whose `app_id` or title matches, saying which,
/// with the sinks of the rule it matches now; nothing else
#[tokio::test(start_paused = true)]
async fn test_test_rule_reports_what_matched() {
    let h = start(base_config(), FakeSwitcher::default(), Some("speakers"));
    h.open_on(1, "firefox", &["DP-1"]).await; // title "firefox window"
    h.open(2, "foxit").await;
    h.event(WindowEvent::Changed {
        id: 2,
        app_id: "foxit".to_string(),
        title: "manual.pdf".to_string(),
    })
    .await;
    h.open(3, "zsh").await;
    h.event(WindowEvent::Changed {
        id: 3,
        app_id: "zsh".to_string(),
        title: "fox news".to_string(),
    })
    .await;
    h.open(4, "mpv").await;
    h.wait_windows(4).await;

    let response = exchange(&h.commands, &Request::TestRule {
        pattern: "fox".to_string(),
    })
    .await;
    let Response::RuleMatches { pattern, matches } = &response else {
        panic!("expected rule matches, got {response:?}");
    };
    assert_eq!(pattern, "fox");
    let firefox = &matches[0];
    assert_eq!(firefox.outputs, ["DP-1"], "placement is kept for --output");
    let tracked = firefox.tracked.as_ref().expect("firefox matches a rule");
    assert_eq!(tracked.sink_name, "headphones");
    assert!(matches[1].tracked.is_none(), "foxit matches no rule");
    assert_eq!(summary(response), vec![
        (
            1,
            "firefox".to_string(),
            "firefox window".to_string(),
            Some("both".to_string())
        ),
        (
            2,
            "foxit".to_string(),
            "manual.pdf".to_string(),
            Some("app_id".to_string())
        ),
        (
            3,
            "zsh".to_string(),
            "fox news".to_string(),
            Some("title".to_string())
        ),
    ]);

    let response = exchange(&h.commands, &Request::TestRule {
        pattern: "^nothing$".to_string(),
    })
    .await;
    assert_eq!(summary(response), vec![]);
    h.stop().await.expect("clean shutdown");
}

/// An invalid pattern is reported, not matched
#[tokio::test(start_paused = true)]
async fn test_test_rule_rejects_an_invalid_pattern() {
    let h = start(base_config(), FakeSwitcher::default(), Some("speakers"));
    let response = exchange(&h.commands, &Request::TestRule {
        pattern: "(unclosed".to_string(),
    })
    .await;
    let Response::Error { message } = response else {
        panic!("expected an error, got {response:?}");
    };
    assert!(message.starts_with("Invalid regex pattern: "), "{message}");
    h.stop().await.expect("clean shutdown");
}

/// `list-windows` (and `test-rule --output` without a pattern, which filters this
/// list by monitor): every window, by id, with its monitors and whether it is
/// minimized
#[tokio::test(start_paused = true)]
async fn test_list_windows_reports_placement() {
    let h = start(base_config(), FakeSwitcher::default(), Some("speakers"));
    h.open(2, "mpv").await;
    h.open_on(1, "firefox", &["HDMI-A-1", "DP-1"]).await;
    h.event(WindowEvent::Placement {
        id: 2,
        outputs: vec!["DP-1".to_string()],
        minimized: true,
    })
    .await;
    h.wait_for("mpv minimized", |s, _| {
        s.windows.iter().any(|w| w.minimized)
    })
    .await;

    let response = exchange(&h.commands, &Request::ListWindows).await;
    let Response::Windows { windows } = response else {
        panic!("expected windows, got {response:?}");
    };
    let placement: Vec<(Option<u64>, &str, &[String], bool)> = windows
        .iter()
        .map(|w| (w.id, w.app_id.as_str(), w.outputs.as_slice(), w.minimized))
        .collect();
    assert_eq!(placement, vec![
        (
            Some(1),
            "firefox",
            &["HDMI-A-1".to_string(), "DP-1".to_string()][..],
            false
        ),
        (Some(2), "mpv", &["DP-1".to_string()][..], true),
    ]);
    assert!(windows.iter().all(|w| w.matched_on.is_none()));
    h.stop().await.expect("clean shutdown");
}

/// `GetManagerInfo` answers from the server's facts
#[tokio::test(start_paused = true)]
async fn test_get_manager_info() {
    let h = start(base_config(), FakeSwitcher::default(), Some("speakers"));
    let response = exchange(&h.commands, &Request::GetManagerInfo).await;
    assert!(
        matches!(response, Response::ManagerInfo {
            daemon_manager: DaemonManager::Direct
        }),
        "{response:?}"
    );
    h.stop().await.expect("clean shutdown");
}

/// `Shutdown` is answered before the loop is told to stop; the loop then exits
/// cleanly, and every later request that needs it is told the daemon is shutting down
#[tokio::test(start_paused = true)]
async fn test_shutdown_request_then_shutting_down_replies() {
    let h = start(base_config(), FakeSwitcher::default(), Some("speakers"));
    let commands = h.commands.clone();
    let response = exchange(&commands, &Request::Shutdown).await;
    assert!(
        matches!(&response, Response::Ok { message } if message == "Daemon shutting down..."),
        "{response:?}"
    );
    within("loop to exit", h.task)
        .await
        .expect("loop task")
        .expect("clean exit");

    for request in [
        Request::Status,
        Request::ListWindows,
        Request::TestRule {
            pattern: "x".to_string(),
        },
        Request::SetSink {
            sink: "HDMI".to_string(),
        },
        Request::NextSink,
        Request::PrevSink,
        Request::SetSource {
            source: "Mic".to_string(),
        },
        Request::Pause,
        Request::Resume,
        Request::TogglePause,
        Request::Subscribe,
    ] {
        let response = exchange(&commands, &request).await;
        assert!(
            matches!(&response, Response::Error { message } if message == "Daemon is shutting down"),
            "{request:?}: {response:?}"
        );
    }
    // Needs no loop
    let response = exchange(&commands, &Request::GetManagerInfo).await;
    assert!(
        matches!(response, Response::ManagerInfo { .. }),
        "{response:?}"
    );
}
