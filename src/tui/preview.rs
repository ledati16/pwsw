//! Live preview for the rule editor: which tracked windows the patterns match.
//!
//! The patterns are matched exactly as the daemon matches a rule
//! ([`Rule::matches`]: `app_id`, title, and monitors of a window that is not
//! minimized). Matching is synchronous. `regex` guarantees linear-time matching, the window
//! list is small, and the result is recomputed only when the pattern text or the
//! window list changes — never per frame. Compiled patterns are cached by their
//! text so typing back and forth does not recompile.

use std::collections::HashMap;

use ratatui::text::{Line, Span};
use regex::Regex;

use crate::config::{Rule, compile_rule_regex};
use crate::ipc::WindowInfo;

/// How many matching windows the preview lists (the rest are counted).
pub(crate) const MAX_SHOWN: usize = 5;

/// Upper bound on cached patterns; typing produces one per keystroke, so the
/// cache is simply cleared when it grows past this.
const MAX_CACHED_PATTERNS: usize = 256;

/// Compiled rule patterns keyed by their text (compile errors are cached too).
#[derive(Default)]
struct RegexCache {
    entries: HashMap<String, Result<Regex, String>>,
    compilations: usize,
}

impl RegexCache {
    fn get(&mut self, pattern: &str) -> Result<Regex, String> {
        if let Some(entry) = self.entries.get(pattern) {
            return entry.clone();
        }
        if self.entries.len() >= MAX_CACHED_PATTERNS {
            self.entries.clear();
        }
        self.compilations += 1;
        let entry = compile_rule_regex(pattern).map_err(|e| e.to_string());
        self.entries.insert(pattern.to_string(), entry.clone());
        entry
    }
}

/// The patterns of the rule editor (an empty one is not set)
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Patterns<'a> {
    pub(crate) app_id: &'a str,
    pub(crate) title: &'a str,
    pub(crate) output: &'a str,
}

/// What the preview shows for the current patterns.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct PreviewView {
    /// Compile error of the `app_id` pattern
    pub(crate) app_error: Option<String>,
    /// Compile error of the title pattern
    pub(crate) title_error: Option<String>,
    /// Compile error of the output pattern
    pub(crate) output_error: Option<String>,
    /// First [`MAX_SHOWN`] matches as "`app_id` | title" (with " · monitors" when
    /// an output pattern is set)
    pub(crate) shown: Vec<String>,
    /// Total number of matching windows
    pub(crate) total: usize,
}

/// Inputs the current [`PreviewView`] was computed from.
#[derive(PartialEq, Eq)]
struct PreviewKey {
    app_pattern: String,
    title_pattern: String,
    output_pattern: String,
    windows_generation: u64,
}

/// Live preview state: the regex cache and the last computed result.
#[derive(Default)]
pub(crate) struct LivePreview {
    cache: RegexCache,
    key: Option<PreviewKey>,
    view: PreviewView,
}

impl LivePreview {
    /// Recompute the preview if the patterns or the window list changed since
    /// the last call. Returns whether it was recomputed.
    ///
    /// `windows_generation` must change whenever `windows` changes content.
    /// An empty pattern is a condition the rule does not set; with neither an
    /// `app_id` nor an output pattern nothing matches (the rule is incomplete).
    pub(crate) fn refresh(
        &mut self,
        patterns: &Patterns,
        windows: &[WindowInfo],
        windows_generation: u64,
    ) -> bool {
        if self.key.as_ref().is_some_and(|k| {
            k.windows_generation == windows_generation
                && k.app_pattern == patterns.app_id
                && k.title_pattern == patterns.title
                && k.output_pattern == patterns.output
        }) {
            return false;
        }

        let mut compile = |pattern: &str| {
            if pattern.is_empty() {
                Ok(None)
            } else {
                self.cache.get(pattern).map(Some)
            }
        };
        let app_re = compile(patterns.app_id);
        let title_re = compile(patterns.title);
        let output_re = compile(patterns.output);

        self.view = match (app_re, title_re, output_re) {
            (Ok(None), _, Ok(None)) => PreviewView::default(),
            (Ok(app), Ok(title), Ok(output)) => {
                // A rule without `app_id` holds the empty pattern (never consulted)
                match app.map_or_else(|| self.cache.get(""), Ok) {
                    Ok(app_regex) => {
                        let shows_outputs = output.is_some();
                        let rule = preview_rule(app_regex, patterns, title, output);
                        let (shown, total) =
                            match_windows(&rule, shows_outputs, windows, MAX_SHOWN);
                        PreviewView {
                            shown,
                            total,
                            ..PreviewView::default()
                        }
                    }
                    Err(e) => PreviewView {
                        app_error: Some(e),
                        ..PreviewView::default()
                    },
                }
            }
            (app, title, output) => PreviewView {
                app_error: app.err(),
                title_error: title.err(),
                output_error: output.err(),
                ..PreviewView::default()
            },
        };
        self.key = Some(PreviewKey {
            app_pattern: patterns.app_id.to_string(),
            title_pattern: patterns.title.to_string(),
            output_pattern: patterns.output.to_string(),
            windows_generation,
        });
        true
    }

    /// The last computed preview.
    pub(crate) const fn view(&self) -> &PreviewView {
        &self.view
    }

    /// How many patterns were compiled so far (cache misses).
    #[cfg(test)]
    pub(crate) const fn compilations(&self) -> usize {
        self.cache.compilations
    }
}

/// A rule with the given conditions (no targets), to match windows with
/// [`Rule::matches`] exactly as the daemon does
fn preview_rule(
    app_id_regex: Regex,
    patterns: &Patterns,
    title_regex: Option<Regex>,
    output_regex: Option<Regex>,
) -> Rule {
    let set = |pattern: &str| (!pattern.is_empty()).then(|| pattern.to_string());
    Rule {
        app_id_regex,
        title_regex,
        output_regex,
        sink_ref: String::new(),
        fallback_refs: Vec::new(),
        source_refs: Vec::new(),
        desc: None,
        notify: None,
        app_id_pattern: patterns.app_id.to_string(),
        title_pattern: set(patterns.title),
        output_pattern: set(patterns.output),
    }
}

/// Index of the first rule (in config order) that matches `window`, the rule
/// the daemon would apply to it.
pub(crate) fn first_matching_rule(rules: &[Rule], window: &WindowInfo) -> Option<usize> {
    // Same matching as the daemon (app_id, title and output conditions)
    rules.iter().position(|rule| {
        rule.matches(
            &window.app_id,
            &window.title,
            &window.outputs,
            window.minimized,
        )
    })
}

/// Match windows against `rule`.
///
/// Returns up to `max_shown` formatted "`app_id` | title" strings (followed by
/// " · " and the window's monitors with `show_outputs`) and the total number of
/// matches.
fn match_windows(
    rule: &Rule,
    show_outputs: bool,
    windows: &[WindowInfo],
    max_shown: usize,
) -> (Vec<String>, usize) {
    let mut shown = Vec::new();
    let mut total = 0usize;
    for w in windows {
        if rule.matches(&w.app_id, &w.title, &w.outputs, w.minimized) {
            total += 1;
            if shown.len() < max_shown {
                let mut line = format!("{} | {}", w.app_id, w.title);
                if show_outputs {
                    line.push_str(" · ");
                    line.push_str(&w.outputs.join(","));
                }
                shown.push(line);
            }
        }
    }
    (shown, total)
}

/// Build preview lines (a check mark and the match) for rendering.
pub(crate) fn build_preview_lines(matches: &[String]) -> Vec<Line<'_>> {
    matches
        .iter()
        .map(|s| {
            Line::from(vec![
                Span::styled("✓ ", crate::tui::theme::success()),
                Span::raw(crate::sanitize::for_display(s)),
            ])
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    fn window(app_id: &str, title: &str) -> WindowInfo {
        WindowInfo {
            id: None,
            app_id: app_id.into(),
            title: title.into(),
            outputs: Vec::new(),
            minimized: false,
            matched_on: None,
            tracked: None,
        }
    }

    fn pat<'a>(app_id: &'a str, title: &'a str) -> Patterns<'a> {
        Patterns {
            app_id,
            title,
            output: "",
        }
    }

    fn windows() -> Vec<WindowInfo> {
        vec![
            window("firefox", "Firefox Browser"),
            window("mpv", "mpv video"),
            window("firefox", "Private Browsing"),
        ]
    }

    #[test]
    fn test_matches_app_and_title() {
        let mut preview = LivePreview::default();
        preview.refresh(&pat("firefox", ""), &windows(), 0);
        assert_eq!(preview.view().total, 2);
        assert_eq!(preview.view().shown[0], "firefox | Firefox Browser");

        preview.refresh(&pat("firefox", "Private"), &windows(), 0);
        assert_eq!(preview.view().shown, vec![
            "firefox | Private Browsing".to_string()
        ]);
        assert_eq!(preview.view().total, 1);
    }

    #[test]
    fn test_recomputes_only_on_pattern_or_window_change() {
        let wins = windows();
        let mut preview = LivePreview::default();

        assert!(preview.refresh(&pat("fire", ""), &wins, 0));
        assert_eq!(preview.compilations(), 1);

        // Same inputs (e.g. every frame): nothing recomputed or compiled
        for _ in 0..10 {
            assert!(!preview.refresh(&pat("fire", ""), &wins, 0));
        }
        assert_eq!(preview.compilations(), 1);

        // Pattern change: recompute + one compilation
        assert!(preview.refresh(&pat("firef", ""), &wins, 0));
        assert_eq!(preview.compilations(), 2);

        // Going back to an earlier pattern hits the cache
        assert!(preview.refresh(&pat("fire", ""), &wins, 0));
        assert_eq!(preview.compilations(), 2);

        // Window list change: recompute, no compilation
        assert!(preview.refresh(&pat("fire", ""), &wins[..1], 1));
        assert_eq!(preview.view().total, 1);
        assert_eq!(preview.compilations(), 2);
    }

    #[test]
    fn test_invalid_title_regex_is_reported() {
        let mut preview = LivePreview::default();
        preview.refresh(&pat("firefox", "(unclosed"), &windows(), 0);
        let view = preview.view();
        assert!(view.app_error.is_none());
        assert!(view.title_error.is_some(), "title error surfaced");
        assert_eq!(view.total, 0, "no matches shown for an invalid title");
    }

    #[test]
    fn test_invalid_app_regex_is_reported() {
        let mut preview = LivePreview::default();
        preview.refresh(&pat("(", ""), &windows(), 0);
        assert!(preview.view().app_error.is_some());
        assert!(preview.view().title_error.is_none());
    }

    #[test]
    fn test_empty_app_pattern_matches_nothing() {
        let mut preview = LivePreview::default();
        preview.refresh(&pat("", ""), &windows(), 0);
        assert_eq!(preview.view(), &PreviewView::default());
        assert_eq!(preview.compilations(), 0);
    }

    #[test]
    fn test_shown_is_capped_total_is_not() {
        let wins: Vec<_> = (0..12).map(|i| window("app", &format!("w{i}"))).collect();
        let mut preview = LivePreview::default();
        preview.refresh(&pat("app", ""), &wins, 0);
        assert_eq!(preview.view().shown.len(), MAX_SHOWN);
        assert_eq!(preview.view().total, 12);
    }

    #[test]
    fn test_first_matching_rule_uses_rule_order_and_titles() {
        use crate::test_utils::fixtures::make_rule;

        let rules = vec![
            make_rule("^firefox$", Some("Private"), "s"),
            make_rule("fire", None, "s"),
        ];
        let wins = windows();
        assert_eq!(first_matching_rule(&rules, &wins[0]), Some(1));
        assert_eq!(first_matching_rule(&rules, &wins[2]), Some(0));
        assert_eq!(first_matching_rule(&rules, &wins[1]), None);
    }

    #[test]
    fn test_build_preview_lines() {
        let inputs = vec!["firefox | Firefox Browser".to_string()];
        let lines = build_preview_lines(&inputs);
        assert_eq!(lines.len(), 1);
        let text: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text, "✓ firefox | Firefox Browser");
    }
}

#[cfg(test)]
mod output_rule_tests {
    use super::*;
    use crate::test_utils::fixtures::{make_rule, with_output};

    fn window_on(app_id: &str, outputs: &[&str], minimized: bool) -> WindowInfo {
        WindowInfo {
            id: None,
            app_id: app_id.into(),
            title: String::new(),
            outputs: outputs.iter().map(|o| (*o).to_string()).collect(),
            minimized,
            matched_on: None,
            tracked: None,
        }
    }

    /// The preview matches like the daemon: an output pattern needs the window on a
    /// matching monitor and not minimized, with or without an app ID
    #[test]
    fn test_preview_matches_outputs_like_the_daemon() {
        let windows = vec![
            window_on("mpv", &["HDMI-A-1"], false),
            window_on("mpv", &["DP-1"], false),
            window_on("firefox", &["HDMI-A-1", "DP-1"], false),
            window_on("kitty", &["HDMI-A-1"], true),
            window_on("foot", &[], false),
        ];
        let mut preview = LivePreview::default();
        let output_only = Patterns {
            app_id: "",
            title: "",
            output: "^HDMI-",
        };
        preview.refresh(&output_only, &windows, 0);
        assert_eq!(preview.view().total, 2);
        assert_eq!(preview.view().shown, [
            "mpv |  · HDMI-A-1",
            "firefox |  · HDMI-A-1,DP-1"
        ]);
        let both = Patterns {
            app_id: "^mpv$",
            ..output_only
        };
        preview.refresh(&both, &windows, 0);
        assert_eq!(preview.view().total, 1);
        let bad = Patterns {
            output: "(",
            ..both
        };
        preview.refresh(&bad, &windows, 0);
        assert!(preview.view().output_error.is_some());
        assert_eq!(preview.view().total, 0);
        // The window list changing its monitors only (same generation bump the app does)
        let moved = vec![window_on("mpv", &["DP-1"], false)];
        preview.refresh(&both, &moved, 1);
        assert_eq!(preview.view().total, 0);
    }

    #[test]
    fn test_output_only_rule_matches_only_windows_on_that_output() {
        let rules = vec![with_output(make_rule("", None, "TV"), "^HDMI-A-1$")];
        assert_eq!(
            first_matching_rule(&rules, &window_on("mpv", &["HDMI-A-1"], false)),
            Some(0)
        );
        assert_eq!(
            first_matching_rule(&rules, &window_on("mpv", &["DP-1"], false)),
            None
        );
        assert_eq!(
            first_matching_rule(&rules, &window_on("mpv", &["HDMI-A-1"], true)),
            None
        );
        assert_eq!(
            first_matching_rule(&rules, &window_on("mpv", &[], false)),
            None
        );
    }
}
