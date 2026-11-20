use std::time::Duration;

use ratatui::text::{Line, Span};

use crate::config::compile_rule_regex;
use crate::ipc::WindowInfo;

/// Match windows against provided regex patterns.
///
/// Returns `Ok(Vec<String>)` containing formatted "`app_id` | title" lines up to `max_results`.
/// Returns `Err(String)` if either regex fails to compile.
pub fn match_windows(
    app_pattern: &str,
    title_pattern: Option<&str>,
    windows: &[WindowInfo],
    max_results: usize,
) -> Result<Vec<String>, String> {
    let app_re = compile_rule_regex(app_pattern).map_err(|e| format!("app pattern error: {e}"))?;
    let title_re = if let Some(tp) = title_pattern {
        Some(compile_rule_regex(tp).map_err(|e| format!("title pattern error: {e}"))?)
    } else {
        None
    };

    let mut out = Vec::new();
    for w in windows {
        if app_re.is_match(&w.app_id) && title_re.as_ref().is_none_or(|r| r.is_match(&w.title)) {
            {
                let mut s = String::with_capacity(w.app_id.len() + 3 + w.title.len());
                s.push_str(&w.app_id);
                s.push_str(" | ");
                s.push_str(&w.title);
                out.push(s);
            }
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
/// Runs the regex matching operation in `spawn_blocking` with a timeout to prevent
/// catastrophic backtracking from freezing the UI. If the operation exceeds the timeout,
/// matching is aborted and an empty result is returned.
///
/// Returns `(Vec<String>, bool, Option<String>)` where:
/// - First element: Vector of matching window strings (up to `max_results`)
/// - Second element: `true` if the operation timed out, `false` on success or regex error
/// - Third element: `Some(error_message)` if regex was invalid, `None` otherwise
///
/// When timeout occurs, returned vector will be empty and timeout flag will be `true`.
/// When regex is invalid, returned vector will be empty and error message will be `Some(...)`.
pub async fn execute_preview(
    app_pattern: String,
    title_pattern: Option<String>,
    windows: Vec<WindowInfo>,
    max_results: usize,
    timeout: Duration,
    compiled_app: Option<std::sync::Arc<regex::Regex>>,
    compiled_title: Option<std::sync::Arc<regex::Regex>>,
) -> (Vec<String>, bool, Option<String>) {
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
            let title_re_opt = compiled_title_cl.as_ref().map(AsRef::as_ref);
            // Convert Arc<Regex> to &Regex by deref
            let app_re_ref: &regex::Regex = app_re.as_ref();

            // If title pattern provided, prefer compiled_title if present, else compile on the fly
            let title_re = title_re_opt.cloned().or_else(|| {
                patterns_title
                    .as_deref()
                    .and_then(|tp| compile_rule_regex(tp).ok())
            });

            // perform matching
            let mut out = Vec::new();
            for w in &windows {
                let title_ok = title_re.as_ref().is_none_or(|r| r.is_match(&w.title));
                if app_re_ref.is_match(&w.app_id) && title_ok {
                    let mut s = String::with_capacity(w.app_id.len() + 3 + w.title.len());
                    s.push_str(&w.app_id);
                    s.push_str(" | ");
                    s.push_str(&w.title);
                    out.push(s);
                    if out.len() >= max_results {
                        break;
                    }
                }
            }

            return Ok(out);
        }

        // Fallback: no compiled app regex provided - call existing helper which compiles from strings
        match_windows(
            &patterns_app,
            patterns_title.as_deref(),
            &windows,
            max_results,
        )
    };

    run_preview_with_timeout(blocking_closure, timeout).await
}

/// Run a preview matching closure with a timeout and map the outcome to the
/// `(matches, timed_out, regex_error)` triple returned by [`execute_preview`].
///
/// Split out of [`execute_preview`] so tests can drive the timeout path with a
/// closure that deterministically blocks past the deadline.
async fn run_preview_with_timeout<F>(f: F, timeout: Duration) -> (Vec<String>, bool, Option<String>)
where
    F: FnOnce() -> Result<Vec<String>, String> + Send + 'static,
{
    match run_blocking_with_timeout(f, timeout).await {
        Ok(Ok(v)) => (v, false, None),              // Success
        Ok(Err(e)) => (Vec::new(), false, Some(e)), // Invalid regex
        Err(_) => (Vec::new(), true, None),         // Timeout or join error
    }
}

/// Build ratatui `Line` preview text from string matches
///
/// Returns formatted `Line` values up to `max_results`. This helper is intended to be used by
/// renderers to convert `execute_preview`/`match_windows` output into `ratatui::text::Line` for
/// direct rendering.
pub fn build_preview_lines_from_strings(matches: &[String]) -> Vec<Line<'static>> {
    use ratatui::style::Style;

    use crate::style::colors;

    matches
        .iter()
        .map(|s| {
            Line::from(vec![
                Span::raw("  "),
                Span::styled("✓ ", Style::default().fg(colors::UI_SUCCESS)),
                Span::raw(s.clone()),
            ])
        })
        .collect()
}

/// Match windows using already-compiled regex references.
///
/// Returns a tuple `(Vec<String>, usize)` where the `Vec` contains up to `max_results`
/// formatted "`app_id` | title" strings, and the `usize` is the total number of matches found.
pub fn match_windows_with_compiled_count(
    app_re: Option<&regex::Regex>,
    title_re: Option<&regex::Regex>,
    windows: &[WindowInfo],
    max_results: usize,
) -> (Vec<String>, usize) {
    let mut out = Vec::new();
    let mut total = 0usize;
    for w in windows {
        let app_ok = app_re.is_none_or(|r| r.is_match(&w.app_id));
        let title_ok = title_re.is_none_or(|r| r.is_match(&w.title));
        if app_ok && title_ok {
            total += 1;
            if out.len() < max_results {
                let mut s = String::with_capacity(w.app_id.len() + 3 + w.title.len());
                s.push_str(&w.app_id);
                s.push_str(" | ");
                s.push_str(&w.title);
                out.push(s);
            }
        }
    }
    (out, total)
}

// Debouncer & test harness
#[cfg(test)]
mod tests {
    // `std::fmt::Write` not needed here; keep tests minimal
    use std::time::Duration;

    use tokio::sync::mpsc as bounded;
    use tokio::sync::mpsc::unbounded_channel;
    use tokio::time::Instant;

    use super::*;
    use crate::ipc::WindowInfo;
    use crate::tui::app::AppUpdate;

    /// Run a simple debouncer that consumes preview requests from `preview_rx`, debounces them by `debounce_ms`,
    /// runs `preview` (normally a call to `execute_preview`) and sends `AppUpdate::PreviewPending` and
    /// `AppUpdate::PreviewMatches` on `bg_tx`.
    ///
    /// Note: this debouncer is test-local; the production one lives in `src/tui/mod.rs`.
    async fn run_debouncer<F, Fut>(
        mut preview_rx: bounded::Receiver<(String, Option<String>)>,
        bg_tx: tokio::sync::mpsc::UnboundedSender<AppUpdate>,
        debounce_ms: Duration,
        poll_interval: Duration,
        mut preview: F,
    ) where
        F: FnMut(String, Option<String>) -> Fut,
        Fut: Future<Output = (Vec<String>, bool, Option<String>)>,
    {
        use tokio::time::sleep;

        let mut last_preview_req: Option<(String, Option<String>, Instant)> = None;
        loop {
            // Drain all pending preview requests (non-blocking)
            while let Ok((app_pat, title_pat)) = preview_rx.try_recv() {
                last_preview_req = Some((app_pat, title_pat, Instant::now()));
            }

            if let Some((app_pat, title_pat, ts)) = last_preview_req.clone()
                && ts.elapsed() >= debounce_ms
            {
                last_preview_req = None;

                // send pending
                let _ = bg_tx.send(AppUpdate::PreviewPending {
                    app_pattern: app_pat.clone(),
                    title_pattern: title_pat.clone(),
                });

                // execute preview
                let (matches_out, timed_out, regex_error) =
                    preview(app_pat.clone(), title_pat.clone()).await;

                let _ = bg_tx.send(AppUpdate::PreviewMatches {
                    app_pattern: app_pat.clone(),
                    title_pattern: title_pat.clone(),
                    matches: matches_out.into_iter().take(10).collect(),
                    timed_out,
                    regex_error,
                });
            }

            // Sleep a short poll interval
            sleep(poll_interval).await;
        }
    }

    #[tokio::test]
    async fn test_debouncer_collapses_rapid_requests() {
        let windows = vec![WindowInfo {
            id: None,
            app_id: "firefox".into(),
            title: "Firefox Browser".into(),
            matched_on: None,
            tracked: None,
        }];

        let (preview_tx, preview_rx) = bounded::channel::<(String, Option<String>)>(8);
        let (bg_tx, mut bg_rx) = unbounded_channel::<AppUpdate>();

        let debouncer = tokio::spawn(run_debouncer(
            preview_rx,
            bg_tx,
            Duration::from_millis(150),
            Duration::from_millis(10),
            move |app, title| {
                execute_preview(
                    app,
                    title,
                    windows.clone(),
                    100,
                    Duration::from_millis(200),
                    None,
                    None,
                )
            },
        ));

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
                        assert!(!matches.is_empty(), "expected at least one match");
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
        // The preview closure blocks on a channel until the test releases it, so the
        // 10ms preview timeout is guaranteed to fire first. Waiting on `recv()` (bounded
        // by a generous guard) instead of fixed sleeps removes scheduler-dependent timing.
        let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
        let release_rx = std::sync::Arc::new(std::sync::Mutex::new(release_rx));

        let (preview_tx, preview_rx) = bounded::channel::<(String, Option<String>)>(8);
        let (bg_tx, mut bg_rx) = unbounded_channel::<AppUpdate>();

        let debouncer = tokio::spawn(run_debouncer(
            preview_rx,
            bg_tx,
            Duration::from_millis(10),
            Duration::from_millis(5),
            move |_app, _title| {
                let release_rx = std::sync::Arc::clone(&release_rx);
                run_preview_with_timeout(
                    move || {
                        // Blocks until `release_tx` is dropped by the test.
                        let _ = release_rx.lock().map(|rx| rx.recv());
                        Ok(vec!["should never be reported".to_string()])
                    },
                    Duration::from_millis(10),
                )
            },
        ));

        preview_tx
            .try_send(("a".to_string(), None))
            .expect("preview channel has capacity");

        let (matches, timed_out, regex_error) =
            tokio::time::timeout(Duration::from_secs(5), async {
                loop {
                    match bg_rx.recv().await {
                        Some(AppUpdate::PreviewMatches {
                            matches,
                            timed_out,
                            regex_error,
                            ..
                        }) => break (matches, timed_out, regex_error),
                        Some(_) => {}
                        None => panic!("debouncer closed the update channel"),
                    }
                }
            })
            .await
            .expect("debouncer should emit PreviewMatches");

        // Unblock the blocking-pool thread before asserting so runtime shutdown is prompt.
        drop(release_tx);
        debouncer.abort();

        assert!(timed_out, "expected the preview to time out");
        assert_eq!(matches, Vec::<String>::new());
        assert_eq!(regex_error, None);
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
            WindowInfo {
                id: None,
                app_id: "firefox".into(),
                title: "Firefox Browser".into(),
                matched_on: None,
                tracked: None,
            },
            WindowInfo {
                id: None,
                app_id: "mpv".into(),
                title: "mpv video".into(),
                matched_on: None,
                tracked: None,
            },
        ];

        let (matches, timed_out, regex_error) = execute_preview(
            "firefox".into(),
            None,
            windows,
            10,
            Duration::from_millis(200),
            None,
            None,
        )
        .await;
        assert!(!timed_out);
        assert!(regex_error.is_none());
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0], "firefox | Firefox Browser");
    }

    #[tokio::test]
    async fn test_execute_preview_invalid_regex() {
        let windows: Vec<WindowInfo> = Vec::new();
        let (matches, timed_out, regex_error) = execute_preview(
            "(".into(),
            None,
            windows,
            10,
            Duration::from_millis(200),
            None,
            None,
        )
        .await;
        assert!(!timed_out); // Should not be timeout
        assert!(regex_error.is_some()); // Should have regex error
        assert!(matches.is_empty(), "expected no matches, got {matches:?}");
    }

    #[test]
    fn test_build_preview_lines_from_strings_simple() {
        let inputs = vec![
            "firefox | Firefox Browser".to_string(),
            "mpv | mpv video".to_string(),
        ];
        let lines = build_preview_lines_from_strings(&inputs);
        // We expect two lines with a leading checkmark span and the original string as raw span in second position
        assert_eq!(lines.len(), 2);
        // Convert first line to string by concatenating spans for assertion
        let first = lines[0]
            .spans
            .iter()
            .map(|sp| sp.content.clone())
            .collect::<Vec<_>>()
            .join("");
        assert!(first.contains("✓"));
        assert!(first.contains("firefox | Firefox Browser"));
    }
}
