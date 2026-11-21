//! Window-tracking state for the daemon
//!
//! A pure reducer: [`State::apply`] folds compositor [`WindowEvent`]s into the set of
//! open windows and their rule matches, and [`State::desired`] answers which sink the
//! rules want right now. Nothing here talks to `PipeWire` or sends notifications; the
//! daemon reconciles the desired sink with what it last activated (see `daemon`).
//!
//! Ordering is deterministic: every window carries a sequence number from a monotonic
//! counter, assigned when it opens and renewed when it starts matching a rule, so the
//! "most recent" window is never decided by clock resolution or `HashMap` order.

use std::collections::HashMap;
use std::sync::Arc;

use color_eyre::eyre::{self, Result};
use tracing::{debug, info};

use crate::compositor::WindowEvent;
use crate::config::{Config, Rule, SinkConfig};
use crate::sanitize::for_log;

/// Window-tracking state for daemon mode
///
/// Invariant (checked by [`State::new`] and [`State::reconfigure`]): the config has at
/// least one sink, a default sink, and every rule resolves to a sink.
pub struct State {
    config: Arc<Config>,
    /// Index into `config.sinks` of the default sink
    default_sink: usize,
    /// For each rule (same index), the index into `config.sinks` it resolves to
    rule_sinks: Vec<usize>,
    /// Every open window the compositor told us about, matched or not
    windows: HashMap<u64, Window>,
    /// Next sequence number to hand out (monotonic, never reused)
    next_seq: u64,
}

/// An open window
#[derive(Debug, Clone)]
struct Window {
    app_id: String,
    title: String,
    /// Priority ordering: assigned on open, renewed when the window starts matching
    seq: u64,
    /// The rule this window currently matches, if any
    matched: Option<Match>,
}

/// A window's current rule match
#[derive(Debug, Clone)]
struct Match {
    rule_index: usize,
    /// What triggered the match: the rule's `desc`, else the title when it matched
    trigger: String,
}

/// The sink the rules currently want, and the window that decided it
#[derive(Debug, Clone, Copy)]
pub struct Desired<'a> {
    /// Target sink (the default sink when no window matches)
    pub sink: &'a SinkConfig,
    /// The matching window that won, or `None` when falling back to the default
    pub window: Option<Winner<'a>>,
}

/// The matching window that determines the desired sink
#[derive(Debug, Clone, Copy)]
pub struct Winner<'a> {
    pub id: u64,
    pub app_id: &'a str,
    pub title: &'a str,
    pub rule_index: usize,
    /// Rule `desc`, else the window title at the time it matched
    pub trigger: &'a str,
    /// Whether this window's rule wants notifications (rule `notify`, else `notify_rules`)
    pub notify: bool,
}

/// A window that stopped deciding anything: it closed, or no longer matches a rule
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Departure {
    /// The departed window's trigger description
    pub trigger: String,
    /// Whether its rule wanted notifications
    pub notify: bool,
    /// `true` if the window closed, `false` if it stopped matching
    pub closed: bool,
}

/// A window as seen by `IPC` queries
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowView<'a> {
    pub id: u64,
    pub app_id: &'a str,
    pub title: &'a str,
    /// The sink this window's rule points at, if it matches one
    pub sink: Option<&'a SinkConfig>,
}

/// Check the state invariant and index the config: (default sink, sink of each rule)
fn index_config(config: &Config) -> Result<(usize, Vec<usize>)> {
    if config.sinks.is_empty() {
        eyre::bail!("No sinks configured");
    }
    let default_sink = config
        .sinks
        .iter()
        .position(|s| s.default)
        .ok_or_else(|| eyre::eyre!("No default sink configured"))?;
    let rule_sinks = config
        .rules
        .iter()
        .map(|rule| {
            config
                .resolve_sink(&rule.sink_ref)
                .and_then(|sink| config.sinks.iter().position(|s| std::ptr::eq(s, sink)))
                .ok_or_else(|| {
                    eyre::eyre!(
                        "Rule '{}' references unknown sink '{}'",
                        rule.app_id_pattern,
                        rule.sink_ref
                    )
                })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok((default_sink, rule_sinks))
}

impl State {
    /// Create the state for a config, with no windows tracked
    ///
    /// # Errors
    /// Returns an error if the config has no sinks, no default sink, or a rule that
    /// does not resolve to a sink (config validation normally prevents all three).
    pub fn new(config: Arc<Config>) -> Result<Self> {
        let (default_sink, rule_sinks) = index_config(&config)?;
        Ok(Self {
            config,
            default_sink,
            rule_sinks,
            windows: HashMap::new(),
            next_seq: 0,
        })
    }

    /// The active configuration
    #[must_use]
    pub fn config(&self) -> &Arc<Config> {
        &self.config
    }

    /// Replace the configuration and re-evaluate every window against the new rules
    ///
    /// Windows keep their sequence numbers, so the outcome does not depend on the
    /// order windows are visited in.
    ///
    /// # Errors
    /// Returns an error (and keeps the current config) if the new config breaks the
    /// invariant described on [`State`].
    pub fn reconfigure(&mut self, config: Arc<Config>) -> Result<()> {
        let (default_sink, rule_sinks) = index_config(&config)?;
        self.config = config;
        self.default_sink = default_sink;
        self.rule_sinks = rule_sinks;
        self.reevaluate();
        Ok(())
    }

    /// Recompute every window's rule match in one pass, keeping sequence numbers
    pub fn reevaluate(&mut self) {
        debug!("Re-evaluating all windows against current rules");
        let config = Arc::clone(&self.config);
        for window in self.windows.values_mut() {
            window.matched = match_window(&config, &window.app_id, &window.title);
        }
    }

    /// Find the first rule matching `app_id` and `title`, with its index
    #[must_use]
    pub fn find_matching_rule(&self, app_id: &str, title: &str) -> Option<(usize, &Rule)> {
        find_rule(&self.config, app_id, title)
    }

    /// Fold one compositor event into the tracked windows (no I/O)
    ///
    /// Returns the window that stopped deciding anything, if the event closed a
    /// matching window or made one stop matching.
    pub fn apply(&mut self, event: WindowEvent) -> Option<Departure> {
        match event {
            WindowEvent::Opened { id, app_id, title }
            | WindowEvent::Changed { id, app_id, title } => self.upsert(id, app_id, title),
            WindowEvent::Closed { id } => self.close(id),
        }
    }

    fn take_seq(&mut self) -> u64 {
        let seq = self.next_seq;
        self.next_seq += 1;
        seq
    }

    fn rule_notify(&self, rule_index: usize) -> bool {
        self.config
            .rules
            .get(rule_index)
            .and_then(|r| r.notify)
            .unwrap_or(self.config.settings.notify_rules)
    }

    fn rule_sink(&self, rule_index: usize) -> &SinkConfig {
        let sink_index = self
            .rule_sinks
            .get(rule_index)
            .copied()
            .unwrap_or(self.default_sink);
        &self.config.sinks[sink_index]
    }

    fn upsert(&mut self, id: u64, app_id: String, title: String) -> Option<Departure> {
        debug!(
            "Window: id={}, app_id='{}', title='{}'",
            id,
            for_log(&app_id),
            for_log(&title)
        );
        let matched = match_window(&self.config, &app_id, &title);

        let Some(previous) = self.windows.get(&id).map(|w| w.matched.clone()) else {
            if let Some(m) = &matched {
                info!(
                    "Rule matched: '{}' → {}",
                    for_log(&app_id),
                    self.rule_sink(m.rule_index).desc
                );
            }
            let seq = self.take_seq();
            self.windows.insert(id, Window {
                app_id,
                title,
                seq,
                matched,
            });
            return None;
        };

        let departure = match (&previous, &matched) {
            (None, Some(m)) => {
                info!(
                    "Rule matched: '{}' → {}",
                    for_log(&app_id),
                    self.rule_sink(m.rule_index).desc
                );
                // Starting to match counts as the most recent activity
                let seq = self.take_seq();
                if let Some(w) = self.windows.get_mut(&id) {
                    w.seq = seq;
                }
                None
            }
            (Some(old), None) => {
                info!(
                    "Rule unmatched: window {} no longer matches (was: '{}', now: app_id='{}' title='{}')",
                    id,
                    for_log(&old.trigger),
                    for_log(&app_id),
                    for_log(&title)
                );
                Some(Departure {
                    trigger: old.trigger.clone(),
                    notify: self.rule_notify(old.rule_index),
                    closed: false,
                })
            }
            (Some(old), Some(new)) if old.rule_index != new.rule_index => {
                debug!(
                    "Rule changed for window {}: rule {} → rule {}",
                    id, old.rule_index, new.rule_index
                );
                None
            }
            _ => None,
        };

        if let Some(w) = self.windows.get_mut(&id) {
            w.app_id = app_id;
            w.title = title;
            // Keep the trigger text from when the rule first matched
            w.matched = match (previous, matched) {
                (Some(old), Some(new)) if old.rule_index == new.rule_index => Some(old),
                (_, new) => new,
            };
        }
        departure
    }

    fn close(&mut self, id: u64) -> Option<Departure> {
        let window = self.windows.remove(&id)?;
        let matched = window.matched?;
        info!(
            "Tracked window closed: {} (was: {})",
            id,
            for_log(&matched.trigger)
        );
        Some(Departure {
            notify: self.rule_notify(matched.rule_index),
            trigger: matched.trigger,
            closed: true,
        })
    }

    /// The sink the rules want right now, and the window that decided it
    ///
    /// Priority follows `match_by_index`:
    /// - `false`: the most recently opened (or most recently matching) window wins
    /// - `true`: the window whose rule has the lowest index wins; most recent breaks ties
    ///
    /// Falls back to the default sink when no window matches.
    #[must_use]
    pub fn desired(&self) -> Desired<'_> {
        let candidates = self
            .windows
            .iter()
            .filter_map(|(id, w)| w.matched.as_ref().map(|m| (*id, w, m)));
        let winner = if self.config.settings.match_by_index {
            candidates.min_by_key(|(_, w, m)| (m.rule_index, std::cmp::Reverse(w.seq)))
        } else {
            candidates.max_by_key(|(_, w, _)| w.seq)
        };

        match winner {
            Some((id, w, m)) => Desired {
                sink: self.rule_sink(m.rule_index),
                window: Some(Winner {
                    id,
                    app_id: &w.app_id,
                    title: &w.title,
                    rule_index: m.rule_index,
                    trigger: &m.trigger,
                    notify: self.rule_notify(m.rule_index),
                }),
            },
            None => Desired {
                sink: self.default_sink(),
                window: None,
            },
        }
    }

    /// The sink the rules want right now (see [`State::desired`])
    #[must_use]
    pub fn desired_sink(&self) -> &SinkConfig {
        self.desired().sink
    }

    /// The configured default sink
    #[must_use]
    pub fn default_sink(&self) -> &SinkConfig {
        &self.config.sinks[self.default_sink]
    }

    /// Number of windows currently matching a rule
    #[must_use]
    pub fn tracked_count(&self) -> usize {
        self.windows
            .values()
            .filter(|w| w.matched.is_some())
            .count()
    }

    /// Every open window, sorted by id, with the sink its rule points at (if any)
    #[must_use]
    pub fn windows(&self) -> Vec<WindowView<'_>> {
        let mut views: Vec<WindowView<'_>> = self
            .windows
            .iter()
            .map(|(id, w)| WindowView {
                id: *id,
                app_id: &w.app_id,
                title: &w.title,
                sink: w.matched.as_ref().map(|m| self.rule_sink(m.rule_index)),
            })
            .collect();
        views.sort_by_key(|v| v.id);
        views
    }
}

fn find_rule<'a>(config: &'a Config, app_id: &str, title: &str) -> Option<(usize, &'a Rule)> {
    config.rules.iter().enumerate().find(|(_, rule)| {
        rule.app_id_regex.is_match(app_id)
            && rule.title_regex.as_ref().is_none_or(|r| r.is_match(title))
    })
}

fn match_window(config: &Config, app_id: &str, title: &str) -> Option<Match> {
    find_rule(config, app_id, title).map(|(rule_index, rule)| Match {
        rule_index,
        trigger: rule.desc.clone().unwrap_or_else(|| title.to_string()),
    })
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;
    use test_case::test_case;

    use super::*;
    use crate::test_utils::fixtures::{make_config, make_rule, make_sink};

    fn opened(id: u64, app_id: &str, title: &str) -> WindowEvent {
        WindowEvent::Opened {
            id,
            app_id: app_id.to_string(),
            title: title.to_string(),
        }
    }

    fn changed(id: u64, app_id: &str, title: &str) -> WindowEvent {
        WindowEvent::Changed {
            id,
            app_id: app_id.to_string(),
            title: title.to_string(),
        }
    }

    /// Default sink "speakers", rule 0 steam → hdmi, rule 1 firefox → headphones
    fn three_sink_config(match_by_index: bool) -> Config {
        let mut config = make_config(
            vec![
                make_sink("speakers", "Speakers", true),
                make_sink("hdmi", "HDMI", false),
                make_sink("headphones", "Headphones", false),
            ],
            vec![
                make_rule("^steam$", None, "hdmi"),
                make_rule("firefox", None, "headphones"),
            ],
        );
        config.settings.match_by_index = match_by_index;
        config
    }

    fn state(config: Config) -> State {
        State::new(Arc::new(config)).expect("valid test config")
    }

    #[test_case("firefox", None, "firefox", "Any Title", true ; "matches app_id only")]
    #[test_case("steam", Some("Big Picture"), "steam", "Steam Big Picture Mode", true ; "matches app_id and title")]
    #[test_case("firefox", None, "chrome", "Any Title", false ; "no match wrong app_id")]
    #[test_case("steam", Some("Big Picture"), "steam", "Steam Library", false ; "no match wrong title")]
    #[test_case("firefox", None, "org.mozilla.firefox", "Title", true ; "regex partial match")]
    #[test_case("^steam$", None, "steamapp", "Title", false ; "regex anchored no match")]
    #[test_case("^steam$", None, "steam", "Title", true ; "regex anchored exact match")]
    fn test_find_matching_rule(
        rule_app_id: &str,
        rule_title: Option<&str>,
        test_app_id: &str,
        test_title: &str,
        should_match: bool,
    ) {
        let s = state(make_config(vec![make_sink("sink1", "Sink 1", true)], vec![
            make_rule(rule_app_id, rule_title, "sink1"),
        ]));
        assert_eq!(
            s.find_matching_rule(test_app_id, test_title).is_some(),
            should_match
        );
    }

    #[test]
    fn test_find_matching_rule_returns_first_match() {
        let s = state(make_config(vec![make_sink("sink1", "Sink 1", true)], vec![
            make_rule("firefox", None, "sink1"),
            make_rule("fire.*", None, "sink1"),
        ]));
        let (idx, _) = s.find_matching_rule("firefox", "Title").expect("matches");
        assert_eq!(idx, 0);
    }

    #[test]
    fn test_new_rejects_empty_sinks() {
        assert!(State::new(Arc::new(make_config(vec![], vec![]))).is_err());
    }

    #[test]
    fn test_new_rejects_missing_default() {
        let config = make_config(vec![make_sink("a", "A", false)], vec![]);
        assert!(State::new(Arc::new(config)).is_err());
    }

    #[test]
    fn test_new_rejects_unresolvable_rule() {
        let config = make_config(vec![make_sink("a", "A", true)], vec![make_rule(
            "x", None, "nope",
        )]);
        assert!(State::new(Arc::new(config)).is_err());
    }

    #[test]
    fn test_no_windows_desires_default() {
        let s = state(three_sink_config(false));
        let desired = s.desired();
        assert_eq!(desired.sink.name, "speakers");
        assert!(desired.window.is_none());
    }

    #[test]
    fn test_open_matching_window_desires_its_sink() {
        let mut s = state(three_sink_config(false));
        assert_eq!(s.apply(opened(1, "firefox", "Browser")), None);
        let desired = s.desired();
        assert_eq!(desired.sink.name, "headphones");
        assert_eq!(desired.window.map(|w| w.id), Some(1));
        assert_eq!(desired.window.map(|w| w.trigger), Some("Browser"));
    }

    #[test]
    fn test_most_recent_wins_and_close_falls_back_in_order() {
        let mut s = state(three_sink_config(false));
        s.apply(opened(1, "steam", "Steam"));
        s.apply(opened(2, "firefox", "Browser"));
        assert_eq!(s.desired_sink().name, "headphones");

        // Closing the newest falls back to the previous matching window
        let departure = s.apply(WindowEvent::Closed { id: 2 });
        assert_eq!(
            departure,
            Some(Departure {
                trigger: "Browser".to_string(),
                notify: true,
                closed: true,
            })
        );
        assert_eq!(s.desired_sink().name, "hdmi");

        // Closing the last falls back to the default
        s.apply(WindowEvent::Closed { id: 1 });
        assert_eq!(s.desired_sink().name, "speakers");
    }

    #[test]
    fn test_match_by_index_applies_on_open() {
        // Regression: the new-window path used to switch straight to the new rule's sink
        let mut s = state(three_sink_config(true));
        s.apply(opened(1, "steam", "Steam"));
        s.apply(opened(2, "firefox", "Browser"));
        assert_eq!(
            s.desired_sink().name,
            "hdmi",
            "rule 0 (steam) outranks rule 1"
        );
        assert_eq!(s.desired().window.map(|w| w.id), Some(1));
    }

    #[test]
    fn test_match_by_index_tiebreak_is_most_recent() {
        let mut s = state(three_sink_config(true));
        s.apply(opened(1, "firefox", "A"));
        s.apply(opened(2, "firefox", "B"));
        assert_eq!(s.desired().window.map(|w| w.id), Some(2));
    }

    #[test]
    fn test_title_change_that_stops_matching() {
        let mut config = make_config(
            vec![
                make_sink("speakers", "Speakers", true),
                make_sink("hdmi", "HDMI", false),
            ],
            vec![make_rule("firefox", Some("YouTube"), "hdmi")],
        );
        config.rules[0].notify = Some(false);
        let mut s = state(config);

        s.apply(opened(1, "firefox", "YouTube - Video"));
        assert_eq!(s.desired_sink().name, "hdmi");

        let departure = s.apply(changed(1, "firefox", "Search"));
        assert_eq!(
            departure,
            Some(Departure {
                trigger: "YouTube - Video".to_string(),
                notify: false,
                closed: false,
            })
        );
        assert_eq!(s.desired_sink().name, "speakers");
        assert_eq!(s.tracked_count(), 0);
        assert_eq!(s.windows().len(), 1, "window stays known while open");
    }

    #[test]
    fn test_window_that_starts_matching_becomes_most_recent() {
        let mut s = state(three_sink_config(false));
        s.apply(opened(1, "firefox-ish", "x"));
        s.apply(opened(2, "other", "x"));
        s.apply(opened(3, "steam", "Steam"));
        assert_eq!(s.desired_sink().name, "hdmi");
        // Window 2 starts matching (app_id change) after steam opened
        s.apply(changed(2, "firefox", "x"));
        assert_eq!(s.desired().window.map(|w| w.id), Some(2));
    }

    #[test]
    fn test_rule_change_on_title_updates_sink() {
        let config = make_config(
            vec![
                make_sink("speakers", "Speakers", true),
                make_sink("headphones", "Headphones", false),
            ],
            vec![
                make_rule("kitty", Some("Music"), "speakers"),
                make_rule("kitty", None, "headphones"),
            ],
        );
        let mut s = state(config);
        s.apply(opened(1, "kitty", "Shell"));
        assert_eq!(s.desired_sink().name, "headphones");
        assert_eq!(s.apply(changed(1, "kitty", "Music")), None);
        let desired = s.desired();
        assert_eq!(desired.sink.name, "speakers");
        assert_eq!(desired.window.map(|w| w.rule_index), Some(0));
        assert_eq!(desired.window.map(|w| w.title), Some("Music"));
    }

    #[test]
    fn test_events_for_unknown_ids_are_tolerated() {
        // The compositor may cancel an undelivered Opened together with its Closed,
        // so Closed/Changed can name ids the reducer never saw
        let mut s = state(three_sink_config(false));
        s.apply(opened(1, "steam", "Steam"));
        assert_eq!(s.apply(WindowEvent::Closed { id: 42 }), None);
        assert_eq!(s.desired().window.map(|w| w.id), Some(1));

        // Changed for an unknown id: tracked like a fresh window, no departure
        assert_eq!(s.apply(changed(43, "zsh", "shell")), None);
        assert_eq!(s.desired().window.map(|w| w.id), Some(1));
        assert_eq!(s.windows().len(), 2);
        assert_eq!(s.apply(WindowEvent::Closed { id: 43 }), None);
        assert_eq!(s.desired_sink().name, "hdmi");
    }

    #[test]
    fn test_close_of_unmatched_window_is_not_a_departure() {
        let mut s = state(three_sink_config(false));
        s.apply(opened(1, "terminal", "zsh"));
        assert_eq!(s.apply(WindowEvent::Closed { id: 1 }), None);
        assert_eq!(s.windows(), vec![]);
    }

    #[test]
    fn test_reevaluate_is_deterministic_with_many_windows() {
        // Rules match nothing at first; reconfigure so many windows match at once. The
        // winner must not depend on HashMap iteration order (repeat to shake it out).
        let base = make_config(
            vec![
                make_sink("speakers", "Speakers", true),
                make_sink("hdmi", "HDMI", false),
                make_sink("headphones", "Headphones", false),
            ],
            vec![],
        );
        for _ in 0..20 {
            let mut s = state(base.clone());
            for id in [7_u64, 3, 11, 5, 9] {
                s.apply(opened(id, &format!("app{id}"), "t"));
            }
            assert_eq!(s.desired_sink().name, "speakers");

            let mut config = base.clone();
            config.rules = vec![
                make_rule("app(3|11)$", None, "hdmi"),
                make_rule("app", None, "headphones"),
            ];
            s.reconfigure(Arc::new(config.clone()))
                .expect("valid config");
            // Most recently opened is 9 (rule 1 → headphones)
            assert_eq!(s.desired().window.map(|w| w.id), Some(9));
            assert_eq!(s.desired_sink().name, "headphones");

            config.settings.match_by_index = true;
            s.reconfigure(Arc::new(config)).expect("valid config");
            // Rule 0 wins; of windows 3 and 11, 11 opened later
            assert_eq!(s.desired().window.map(|w| w.id), Some(11));
            assert_eq!(s.desired_sink().name, "hdmi");
        }
    }

    #[test]
    fn test_reconfigure_changing_default_with_no_windows() {
        let mut s = state(three_sink_config(false));
        let mut config = three_sink_config(false);
        config.sinks[0].default = false;
        config.sinks[1].default = true;
        s.reconfigure(Arc::new(config)).expect("valid config");
        assert_eq!(s.desired_sink().name, "hdmi");
    }

    #[test]
    fn test_reconfigure_rejects_invalid_and_keeps_old() {
        let mut s = state(three_sink_config(false));
        s.apply(opened(1, "firefox", "Browser"));
        assert!(
            s.reconfigure(Arc::new(make_config(vec![], vec![])))
                .is_err()
        );
        assert_eq!(s.desired_sink().name, "headphones");
    }

    #[test]
    fn test_windows_view_sorted_with_sinks() {
        let mut s = state(three_sink_config(false));
        s.apply(opened(5, "firefox", "B"));
        s.apply(opened(2, "zsh", "A"));
        let views = s.windows();
        assert_eq!(views.iter().map(|v| v.id).collect::<Vec<_>>(), vec![2, 5]);
        assert!(views[0].sink.is_none());
        assert_eq!(views[1].sink.map(|k| k.name.as_str()), Some("headphones"));
        assert_eq!(s.tracked_count(), 1);
    }

    #[test]
    fn test_rule_desc_is_trigger_and_rule_notify_used() {
        let mut config = three_sink_config(false);
        config.rules[0].desc = Some("Steam Big Picture".to_string());
        config.rules[0].notify = Some(false);
        let mut s = state(config);
        s.apply(opened(1, "steam", "whatever"));
        let w = s.desired().window.expect("winner");
        assert_eq!(w.trigger, "Steam Big Picture");
        assert!(!w.notify);
    }
}
