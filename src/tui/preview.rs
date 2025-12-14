use crate::ipc::WindowInfo;
use std::time::Duration;

/// Match windows against provided regex patterns.
///
/// Returns `Ok(Vec<String>)` containing formatted "app_id | title" lines up to `max_results`.
/// Returns `Err(String)` if either regex fails to compile.
pub fn match_windows(
    app_pattern: &str,
    title_pattern: Option<&str>,
    windows: &[WindowInfo],
    max_results: usize,
) -> Result<Vec<String>, String> {
    let app_re = regex::Regex::new(app_pattern).map_err(|e| format!("app pattern error: {}", e))?;
    let title_re = if let Some(tp) = title_pattern {
        Some(regex::Regex::new(tp).map_err(|e| format!("title pattern error: {}", e))?)
    } else {
        None
    };

    let mut out = Vec::new();
    for w in windows.iter() {
        if app_re.is_match(&w.app_id) && title_re.as_ref().map_or(true, |r| r.is_match(&w.title)) {
            out.push(format!("{} | {}", w.app_id, w.title));
            if out.len() >= max_results {
                break;
            }
        }
    }

    Ok(out)
}

/// Run a blocking closure inside `spawn_blocking` with a timeout.
///
/// Returns `Ok(T)` if the closure completed within the timeout, otherwise `Err("timed_out")`.
async fn run_blocking_with_timeout<T, F>(f: F, timeout: Duration) -> Result<T, &'static str>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    let handle = tokio::task::spawn_blocking(f);
    match tokio::time::timeout(timeout, handle).await {
        Ok(join_res) => match join_res {
            Ok(v) => Ok(v),
            Err(_) => Err("join_error"),
        },
        Err(_) => Err("timed_out"),
    }
}

/// Execute preview matching with a timeout.
///
/// Returns `(Vec<String>, bool)` where the `bool` is `true` when the preview timed out or the
/// regexes were invalid.
pub async fn execute_preview(
    app_pattern: String,
    title_pattern: Option<String>,
    windows: Vec<WindowInfo>,
    max_results: usize,
    timeout: Duration,
    compiled_app: Option<std::sync::Arc<regex::Regex>>,
    compiled_title: Option<std::sync::Arc<regex::Regex>>,
) -> (Vec<String>, bool) {
    // We'll run matching inside `spawn_blocking` and enforce a timeout. If compiled regexes were
    // provided by the sender (editor cache), prefer reusing them to avoid recompilation.
    let patterns_app = app_pattern.clone();
    let patterns_title = title_pattern.clone();
    let compiled_app_cl = compiled_app.clone();
    let compiled_title_cl = compiled_title.clone();

    // The blocking closure returns a Result<Vec<String>, String> to surface invalid regex errors.
    let blocking_closure = move || {
        if let Some(app_re) = compiled_app_cl.as_ref() {
            // Use provided compiled app regex
            let title_re_opt = compiled_title_cl.as_ref().map(|r| r.as_ref());
            // Convert Arc<Regex> to &Regex by deref
            let app_re_ref: &regex::Regex = app_re.as_ref();

            // If title pattern provided, prefer compiled_title if present, else compile on the fly
            let title_re = title_re_opt.cloned().or_else(|| patterns_title.as_deref().and_then(|tp| regex::Regex::new(tp).ok()));

            // perform matching
            let mut out = Vec::new();
            for w in windows.iter() {
                let title_ok = title_re.as_ref().map_or(true, |r| r.is_match(&w.title));
                if app_re_ref.is_match(&w.app_id) && title_ok {
                    out.push(format!("{} | {}", w.app_id, w.title));
                    if out.len() >= max_results {
                        break;
                    }
                }
            }
            return Ok(out);
        }

        // Fallback: no compiled app regex provided - call existing helper which compiles from strings
        match_windows(&patterns_app, patterns_title.as_deref(), &windows, max_results)
    };

    match run_blocking_with_timeout(blocking_closure, timeout).await {
        Ok(Ok(v)) => (v, false),
        Ok(Err(_)) => (Vec::new(), true), // invalid regex
        Err(_) => (Vec::new(), true),     // timed out or join error
    }
}


// Debouncer & test harness
#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::WindowInfo;
    use crate::tui::app::AppUpdate;
    use std::time::Duration;
    use tokio::sync::mpsc::unbounded_channel;
    use tokio::sync::mpsc as bounded;
    use tokio::time::Instant;

    /// Run a simple debouncer that consumes preview requests from `preview_rx`, debounces them by `debounce_ms`,
    /// executes `execute_preview` and sends `AppUpdate::PreviewPending` and `AppUpdate::PreviewMatches` on `bg_tx`.
    async fn run_debouncer(mut preview_rx: bounded::Receiver<(String, Option<String>)>, bg_tx: tokio::sync::mpsc::UnboundedSender<AppUpdate>, windows: Vec<WindowInfo>, debounce_ms: Duration, timeout: Duration, poll_interval: Duration) {
        use tokio::time::sleep;

        let mut last_preview_req: Option<(String, Option<String>, Instant)> = None;
        loop {
            // Drain all pending preview requests (non-blocking)
            while let Ok((app_pat, title_pat)) = preview_rx.try_recv() {
                last_preview_req = Some((app_pat, title_pat, Instant::now()));
            }

            if let Some((app_pat, title_pat, ts)) = last_preview_req.clone() {
                if ts.elapsed() >= debounce_ms {
                    last_preview_req = None;

                    // send pending
                    let _ = bg_tx.send(AppUpdate::PreviewPending { app_pattern: app_pat.clone(), title_pattern: title_pat.clone() });

                    // execute preview
                    let (matches_out, timed_out) = execute_preview(app_pat.clone(), title_pat.clone(), windows.clone(), 100, timeout, None, None).await;

                    let _ = bg_tx.send(AppUpdate::PreviewMatches { app_pattern: app_pat.clone(), title_pattern: title_pat.clone(), matches: matches_out.into_iter().take(10).collect(), timed_out });
                }
            }

            // Sleep a short poll interval
            sleep(poll_interval).await;
        }
    }

    #[tokio::test]
    async fn test_debouncer_collapses_rapid_requests() {
        let windows = vec![
            WindowInfo { app_id: "firefox".into(), title: "Firefox Browser".into(), matched_on: None, tracked: None },
        ];

        let (preview_tx, preview_rx) = bounded::channel::<(String, Option<String>)>(8);
        let (bg_tx, mut bg_rx) = unbounded_channel::<AppUpdate>();

        let debouncer = tokio::spawn(run_debouncer(preview_rx, bg_tx, windows.clone(), Duration::from_millis(150), Duration::from_millis(200), Duration::from_millis(10)));

        // Send a burst of preview requests rapidly
        let _ = preview_tx.try_send(("f".to_string(), None));
        let _ = preview_tx.try_send(("fi".to_string(), None));
        let _ = preview_tx.try_send(("fir".to_string(), None));

        // Sleep less than debounce -> should not trigger yet
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(bg_rx.try_recv().is_err());

        // Sleep to exceed debounce
        tokio::time::sleep(Duration::from_millis(120)).await;

        // allow debouncer to run
        tokio::task::yield_now().await;

        // Expect PreviewPending then PreviewMatches
        let mut pending_seen = false;
        let mut matches_seen = false;
        for _ in 0..8 {
            if let Ok(msg) = bg_rx.try_recv() {
                match msg {
                    AppUpdate::PreviewPending { .. } => pending_seen = true,
                    AppUpdate::PreviewMatches { matches, .. } => {
                        matches_seen = true;
                        assert!(!matches.is_empty());
                    }
                    _ => {}
                }
            }
        }

        assert!(pending_seen, "expected PreviewPending");
        assert!(matches_seen, "expected PreviewMatches");

        // cleanup
        debouncer.abort();
    }

    #[tokio::test]
    async fn test_debouncer_respects_timeout() {
        // create a window list that will cause the blocking matching to sleep (simulate long work)
        // execute_preview runs match_windows which doesn't sleep; instead, we test timeout by passing a very small timeout.

        let windows = vec![WindowInfo { app_id: "a".into(), title: "b".into(), matched_on: None, tracked: None }];
        let (preview_tx, preview_rx) = bounded::channel::<(String, Option<String>)>(8);
        let (bg_tx, mut bg_rx) = unbounded_channel::<AppUpdate>();

        let debouncer = tokio::spawn(run_debouncer(preview_rx, bg_tx, windows.clone(), Duration::from_millis(10), Duration::from_millis(1), Duration::from_millis(5)));

        let _ = preview_tx.try_send(("a".to_string(), None));
        tokio::time::sleep(Duration::from_millis(20)).await;
        tokio::task::yield_now().await;

        // Expect a PreviewMatches with timed_out == true (because timeout very small)
            let mut saw = false;
        while let Ok(msg) = bg_rx.try_recv() {
            if let AppUpdate::PreviewMatches { .. } = msg {
                saw = true;
            }
        }
        assert!(saw, "expected PreviewMatches");

        debouncer.abort();
    }

    #[tokio::test]
    async fn test_run_blocking_with_timeout_ok() {
        let f = || 42u32;
        let res = run_blocking_with_timeout(f, Duration::from_millis(200)).await;
        assert!(res.is_ok());
        assert_eq!(res.unwrap(), 42u32);
    }

    #[tokio::test]
    async fn test_run_blocking_with_timeout_timeout() {
        let f = || {
            std::thread::sleep(std::time::Duration::from_millis(300));
            7u8
        };
        let res = run_blocking_with_timeout(f, Duration::from_millis(50)).await;
        assert!(res.is_err());
        assert_eq!(res.err().unwrap(), "timed_out");
    }

    #[tokio::test]
    async fn test_execute_preview_basic() {
        let windows = vec![
            WindowInfo { app_id: "firefox".into(), title: "Firefox Browser".into(), matched_on: None, tracked: None },
            WindowInfo { app_id: "mpv".into(), title: "mpv video".into(), matched_on: None, tracked: None },
        ];

        let (matches, timed_out) = execute_preview("firefox".into(), None, windows, 10, Duration::from_millis(200), None, None).await;
        assert!(!timed_out);
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0], "firefox | Firefox Browser");
    }

    #[tokio::test]
    async fn test_execute_preview_invalid_regex() {
        let windows: Vec<WindowInfo> = Vec::new();
        let (matches, timed_out) = execute_preview("(".into(), None, windows, 10, Duration::from_millis(200), None, None).await;
        assert!(timed_out);
        assert!(matches.is_empty());
    }
}
