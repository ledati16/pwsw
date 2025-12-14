use std::time::Duration;
use tokio::time::sleep;

use crate::tui::app::{BgCommand, DaemonAction};

/// Spawn a copy of the preview forwarder used by the TUI.
/// Returns the preview_in sender and a JoinHandle for the forwarder task.
fn spawn_forwarder() -> (
    tokio::sync::mpsc::UnboundedSender<(String, Option<String>, Option<std::sync::Arc<regex::Regex>>, Option<std::sync::Arc<regex::Regex>>)>,
    tokio::task::JoinHandle<()>,
    tokio::sync::mpsc::Sender<BgCommand>,
    tokio::sync::mpsc::Receiver<BgCommand>,
) {
    let (preview_in_tx, mut preview_in_rx) = tokio::sync::mpsc::unbounded_channel::<(String, Option<String>, Option<std::sync::Arc<regex::Regex>>, Option<std::sync::Arc<regex::Regex>>) >();
    let (cmd_tx, cmd_rx) = tokio::sync::mpsc::channel::<BgCommand>(1);
    let forward_cmd = cmd_tx.clone();

    let handle = tokio::spawn(async move {
        use tokio::time::Instant;
        let mut latest: Option<(String, Option<String>, Option<std::sync::Arc<regex::Regex>>, Option<std::sync::Arc<regex::Regex>>, Instant)> = None;
        loop {
            match preview_in_rx.recv().await {
                Some((app_pattern, title_pattern, compiled_app, compiled_title)) => {
                    latest = Some((app_pattern, title_pattern, compiled_app, compiled_title, Instant::now()));

                    for _ in 0..3 {
                        if let Some((ap, tp, ca, ct, _)) = latest.clone() {
                            if forward_cmd.try_send(BgCommand::PreviewRequest { app_pattern: ap.clone(), title_pattern: tp.clone(), compiled_app: ca.clone(), compiled_title: ct.clone() }).is_ok() {
                                latest = None;
                                break;
                            }
                        }
                        sleep(Duration::from_millis(20)).await;
                    }
                }
                None => break,
            }
        }
    });

    (preview_in_tx, handle, cmd_tx, cmd_rx)
}

#[tokio::test]
async fn forwarder_collapses_rapid_requests_and_forwards_latest_after_drain() {
    let (preview_in_tx, handle, cmd_tx, mut cmd_rx) = spawn_forwarder();

    // Pre-fill the bounded cmd channel so the forwarder cannot immediately send.
    cmd_tx.try_send(BgCommand::DaemonAction(DaemonAction::Start)).expect("pre-fill should succeed");

    // Send rapid preview requests
    let _ = preview_in_tx.send(("one".to_string(), None, None, None));
    let _ = preview_in_tx.send(("two".to_string(), None, None, None));
    let _ = preview_in_tx.send(("three".to_string(), None, None, None));

    // Give the forwarder time to attempt retries and collapse latest
    sleep(Duration::from_millis(150)).await;

    // Drain the pre-fill so there is now space in the channel
    let prefill = cmd_rx.recv().await.expect("expected prefill message");
    match prefill {
        BgCommand::DaemonAction(_) => {}
        _ => panic!("expected daemon action prefill")
    }

    // Now the forwarder should be able to deliver the latest preview request
    let forwarded = cmd_rx.recv().await.expect("expected forwarded PreviewRequest");
    match forwarded {
        BgCommand::PreviewRequest { app_pattern, title_pattern, compiled_app: _, compiled_title: _ } => {
            assert_eq!(app_pattern, "three");
            assert!(title_pattern.is_none());
        }
        other => panic!("unexpected command: {:?}", other),
    }

    // cleanup
    drop(preview_in_tx);
    handle.abort();
}

#[tokio::test]
async fn forwarder_sends_when_space_appears_quickly() {
    let (preview_in_tx, handle, cmd_tx, mut cmd_rx) = spawn_forwarder();

    // Pre-fill channel to block immediate sends
    cmd_tx.try_send(BgCommand::DaemonAction(DaemonAction::Start)).expect("pre-fill should succeed");

    // Send a single preview request
    let _ = preview_in_tx.send(("alpha".to_string(), None, None, None));

    // After a short delay, free the channel slot so the forwarder can send in its retries window
    sleep(Duration::from_millis(30)).await;
    // Drain the prefill now
    let _ = cmd_rx.recv().await.expect("expected prefill");

    // The forwarder had a few retries spaced 20ms apart; giving a short wait should let it send
    let forwarded = tokio::time::timeout(Duration::from_millis(200), cmd_rx.recv()).await.expect("expected forwarded command within timeout").expect("recv returned None");

    match forwarded {
        BgCommand::PreviewRequest { app_pattern, title_pattern, compiled_app: _, compiled_title: _ } => {
            assert_eq!(app_pattern, "alpha");
            assert!(title_pattern.is_none());
        }
        other => panic!("unexpected command: {:?}", other),
    }

    drop(preview_in_tx);
    handle.abort();
}
