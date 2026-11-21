//! Live preview for the rule editor: which tracked windows the patterns match.
//!
//! Matching is synchronous. `regex` guarantees linear-time matching, the window
//! list is small, and the result is recomputed only when the pattern text or the
//! window list changes — never per frame. Compiled patterns are cached by their
//! text so typing back and forth does not recompile.

use std::collections::HashMap;

use ratatui::style::Style;
use ratatui::text::{Line, Span};
use regex::Regex;

use crate::config::compile_rule_regex;
use crate::ipc::WindowInfo;
use crate::style::colors;

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

/// What the preview shows for the current patterns.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct PreviewView {
    /// Compile error of the `app_id` pattern
    pub(crate) app_error: Option<String>,
    /// Compile error of the title pattern
    pub(crate) title_error: Option<String>,
    /// First [`MAX_SHOWN`] matches as "`app_id` | title"
    pub(crate) shown: Vec<String>,
    /// Total number of matching windows
    pub(crate) total: usize,
}

/// Inputs the current [`PreviewView`] was computed from.
#[derive(PartialEq, Eq)]
struct PreviewKey {
    app_pattern: String,
    title_pattern: String,
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
    /// An empty `title_pattern` means "any title".
    pub(crate) fn refresh(
        &mut self,
        app_pattern: &str,
        title_pattern: &str,
        windows: &[WindowInfo],
        windows_generation: u64,
    ) -> bool {
        if self.key.as_ref().is_some_and(|k| {
            k.windows_generation == windows_generation
                && k.app_pattern == app_pattern
                && k.title_pattern == title_pattern
        }) {
            return false;
        }

        let app_re = if app_pattern.is_empty() {
            Ok(None)
        } else {
            self.cache.get(app_pattern).map(Some)
        };
        let title_re = if title_pattern.is_empty() {
            Ok(None)
        } else {
            self.cache.get(title_pattern).map(Some)
        };

        self.view = match (app_re, title_re) {
            (Ok(Some(app)), Ok(title)) => {
                let (shown, total) = match_windows(&app, title.as_ref(), windows, MAX_SHOWN);
                PreviewView {
                    shown,
                    total,
                    ..PreviewView::default()
                }
            }
            (app, title) => PreviewView {
                app_error: app.err(),
                title_error: title.err(),
                ..PreviewView::default()
            },
        };
        self.key = Some(PreviewKey {
            app_pattern: app_pattern.to_string(),
            title_pattern: title_pattern.to_string(),
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

/// Match windows against compiled patterns.
///
/// Returns up to `max_shown` formatted "`app_id` | title" strings and the total
/// number of matches.
fn match_windows(
    app_re: &Regex,
    title_re: Option<&Regex>,
    windows: &[WindowInfo],
    max_shown: usize,
) -> (Vec<String>, usize) {
    let mut shown = Vec::new();
    let mut total = 0usize;
    for w in windows {
        if app_re.is_match(&w.app_id) && title_re.is_none_or(|r| r.is_match(&w.title)) {
            total += 1;
            if shown.len() < max_shown {
                shown.push(format!("{} | {}", w.app_id, w.title));
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
                Span::raw("  "),
                Span::styled("✓ ", Style::default().fg(colors::UI_SUCCESS)),
                Span::raw(s.as_str()),
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
            matched_on: None,
            tracked: None,
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
        preview.refresh("firefox", "", &windows(), 0);
        assert_eq!(preview.view().total, 2);
        assert_eq!(preview.view().shown[0], "firefox | Firefox Browser");

        preview.refresh("firefox", "Private", &windows(), 0);
        assert_eq!(preview.view().shown, vec![
            "firefox | Private Browsing".to_string()
        ]);
        assert_eq!(preview.view().total, 1);
    }

    #[test]
    fn test_recomputes_only_on_pattern_or_window_change() {
        let wins = windows();
        let mut preview = LivePreview::default();

        assert!(preview.refresh("fire", "", &wins, 0));
        assert_eq!(preview.compilations(), 1);

        // Same inputs (e.g. every frame): nothing recomputed or compiled
        for _ in 0..10 {
            assert!(!preview.refresh("fire", "", &wins, 0));
        }
        assert_eq!(preview.compilations(), 1);

        // Pattern change: recompute + one compilation
        assert!(preview.refresh("firef", "", &wins, 0));
        assert_eq!(preview.compilations(), 2);

        // Going back to an earlier pattern hits the cache
        assert!(preview.refresh("fire", "", &wins, 0));
        assert_eq!(preview.compilations(), 2);

        // Window list change: recompute, no compilation
        assert!(preview.refresh("fire", "", &wins[..1], 1));
        assert_eq!(preview.view().total, 1);
        assert_eq!(preview.compilations(), 2);
    }

    #[test]
    fn test_invalid_title_regex_is_reported() {
        let mut preview = LivePreview::default();
        preview.refresh("firefox", "(unclosed", &windows(), 0);
        let view = preview.view();
        assert!(view.app_error.is_none());
        assert!(view.title_error.is_some(), "title error surfaced");
        assert_eq!(view.total, 0, "no matches shown for an invalid title");
    }

    #[test]
    fn test_invalid_app_regex_is_reported() {
        let mut preview = LivePreview::default();
        preview.refresh("(", "", &windows(), 0);
        assert!(preview.view().app_error.is_some());
        assert!(preview.view().title_error.is_none());
    }

    #[test]
    fn test_empty_app_pattern_matches_nothing() {
        let mut preview = LivePreview::default();
        preview.refresh("", "", &windows(), 0);
        assert_eq!(preview.view(), &PreviewView::default());
        assert_eq!(preview.compilations(), 0);
    }

    #[test]
    fn test_shown_is_capped_total_is_not() {
        let wins: Vec<_> = (0..12).map(|i| window("app", &format!("w{i}"))).collect();
        let mut preview = LivePreview::default();
        preview.refresh("app", "", &wins, 0);
        assert_eq!(preview.view().shown.len(), MAX_SHOWN);
        assert_eq!(preview.view().total, 12);
    }

    #[test]
    fn test_build_preview_lines() {
        let inputs = vec!["firefox | Firefox Browser".to_string()];
        let lines = build_preview_lines(&inputs);
        assert_eq!(lines.len(), 1);
        let text: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text, "  ✓ firefox | Firefox Browser");
    }
}
