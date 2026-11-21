//! Window-tracking state for the daemon
//!
//! A pure reducer: [`State::apply`] folds compositor [`WindowEvent`]s into the set of
//! open windows and their rule matches, and [`State::desired`] answers which sinks the
//! rules want right now, in priority order (a rule may list fallbacks).
//! [`State::desired_for`] answers the same for sources (microphones), independently:
//! the highest-priority matching window *whose rule has a `source`* decides it, else
//! the default source; a rule without `source` never affects the microphone. Nothing here
//! talks to `PipeWire` or sends notifications: which candidate can actually be
//! activated is decided by the daemon's activation worker, and the daemon reconciles
//! the desired candidates with what it last activated (see `daemon`).
//!
//! Ordering is deterministic: every window carries a sequence number from a monotonic
//! counter, assigned when it opens and renewed when it starts matching a rule, or a
//! different rule than before (after a title change, or a move to a monitor an
//! `output` rule names), so the "most recent"
//! window is never decided by clock resolution or `HashMap` order.
//!
//! Rules match a window's `app_id`, title and, for `output` rules, the monitors it is on
//! ([`WindowEvent::Placement`], reported only by the `wlr` backend): see
//! [`Rule::matches`].

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;
use tracing::{debug, info};

use crate::compositor::WindowEvent;
use crate::config::{Config, Endpoint, Rule, SinkConfig, SourceConfig};
use crate::sanitize::{for_log, sanitize_line};

/// Maximum characters of a window title in rule log lines (they also carry the
/// `app_id` and the targets, so a full [`crate::sanitize::MAX_LOG_CHARS`] title is too
/// long)
const LOG_TITLE_CHARS: usize = 60;

/// Window-tracking state for daemon mode
///
/// Invariant (checked by [`State::new`] and [`State::reconfigure`]): the config has at
/// least one sink, a default sink, and every sink reference of every rule resolves;
/// with sources configured, a default source and every source reference resolves.
pub struct State {
    config: Arc<Config>,
    /// Index into `config.sinks` of the default sink
    default_sink: usize,
    /// For each rule (same index), the indices into `config.sinks` of its candidates
    /// in priority order (never empty)
    rule_sinks: Vec<Vec<usize>>,
    /// Index into `config.sources` of the default source; `None` when no sources are
    /// configured (pwsw then never touches the default source)
    default_source: Option<usize>,
    /// For each rule, the indices into `config.sources` of its source candidates
    /// (empty for a rule without `source`)
    rule_sources: Vec<Vec<usize>>,
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
    /// Connector names of the monitors the window is on (from `Opened`, then
    /// `Placement`; always empty with the `ext` backend)
    outputs: Vec<String>,
    /// The compositor's minimized flag (from `Opened`, then `Placement`)
    minimized: bool,
    /// Priority ordering: assigned on open, renewed when the window starts matching a
    /// rule (or a different rule than before)
    seq: u64,
    /// The rule this window currently matches, if any
    matched: Option<Match>,
    /// The rule this window last left (stopped matching, or moved from to another
    /// rule), as logged at info: coming back to it is logged at debug only, so a title
    /// that flickers logs one info line per flicker, not two
    last_rule: Option<usize>,
}

/// A window's current rule match
#[derive(Debug, Clone)]
struct Match {
    rule_index: usize,
    /// What triggered the match: the rule's `desc`, else the title when it matched
    trigger: String,
}

/// The sinks (or sources) the rules currently want, and the window that decided it
#[derive(Debug, Clone)]
pub struct Desired<'a> {
    /// Candidates in priority order, never empty: the winning rule's sink (source) and
    /// its fallbacks, or just the default when no window decides. The first one that
    /// can be activated is the one to use; that is for the activation worker to find.
    pub candidates: Vec<&'a SinkConfig>,
    /// The matching window that won, or `None` when falling back to the default
    pub window: Option<Winner<'a>>,
}

impl<'a> Desired<'a> {
    /// The first choice
    #[must_use]
    pub fn first(&self) -> &'a SinkConfig {
        self.candidates[0]
    }

    /// The candidates' node names, in priority order
    #[must_use]
    pub fn names(&self) -> Vec<String> {
        self.candidates.iter().map(|s| s.name.clone()).collect()
    }
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
    /// Connector names of the monitors the window is on (empty when unknown)
    pub outputs: &'a [String],
    /// Whether the window is minimized (`false` when unknown)
    pub minimized: bool,
    /// The sinks this window's rule points at in priority order (its sink, then its
    /// fallbacks); empty if it matches no rule
    pub sinks: Vec<&'a SinkConfig>,
    /// The sources this window's rule points at in priority order; empty if it
    /// matches no rule or its rule has no `source`
    pub sources: Vec<&'a SourceConfig>,
}

/// The config indexed for the reducer (see [`index_config`])
struct Index {
    default_sink: usize,
    rule_sinks: Vec<Vec<usize>>,
    default_source: Option<usize>,
    rule_sources: Vec<Vec<usize>>,
}

/// Check the state invariant and index the config
fn index_config(config: &Config) -> Result<Index> {
    if config.sinks.is_empty() {
        anyhow::bail!("No sinks configured");
    }
    let default_sink = config
        .sinks
        .iter()
        .position(|s| s.default)
        .ok_or_else(|| anyhow::anyhow!("No default sink configured"))?;
    let rule_sinks = config
        .rules
        .iter()
        .map(|rule| {
            config.resolve_rule_sinks(rule).ok_or_else(|| {
                anyhow::anyhow!(
                    "Rule '{}' references an unknown sink ({})",
                    rule.label(),
                    rule.sink_refs().collect::<Vec<_>>().join(", ")
                )
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let default_source = if config.sources.is_empty() {
        None
    } else {
        Some(
            config
                .sources
                .iter()
                .position(|s| s.default)
                .ok_or_else(|| anyhow::anyhow!("No default source configured"))?,
        )
    };
    let rule_sources = config
        .rules
        .iter()
        .map(|rule| {
            if !rule.source_refs.is_empty() && default_source.is_none() {
                anyhow::bail!(
                    "Rule '{}' sets a source but no sources are configured",
                    rule.label()
                );
            }
            config
                .resolve_rule_refs(Endpoint::Source, rule)
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "Rule '{}' references an unknown source ({})",
                        rule.label(),
                        rule.source_refs.join(", ")
                    )
                })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(Index {
        default_sink,
        rule_sinks,
        default_source,
        rule_sources,
    })
}

impl State {
    /// Create the state for a config, with no windows tracked
    ///
    /// # Errors
    /// Returns an error if the config has no sinks, no default sink, or a rule that
    /// does not resolve to a sink (config validation normally prevents all three).
    pub fn new(config: Arc<Config>) -> Result<Self> {
        let index = index_config(&config)?;
        Ok(Self {
            config,
            default_sink: index.default_sink,
            rule_sinks: index.rule_sinks,
            default_source: index.default_source,
            rule_sources: index.rule_sources,
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
        let index = index_config(&config)?;
        let before = self.match_sinks();
        self.config = config;
        self.default_sink = index.default_sink;
        self.rule_sinks = index.rule_sinks;
        self.default_source = index.default_source;
        self.rule_sources = index.rule_sources;
        self.rematch(&before);
        Ok(())
    }

    /// For each matching window, its trigger and the names of the sinks it points at
    /// (then its sources)
    fn match_sinks(&self) -> HashMap<u64, (String, Vec<String>)> {
        self.windows
            .iter()
            .filter_map(|(id, w)| {
                let m = w.matched.as_ref()?;
                Some((*id, (m.trigger.clone(), self.target_names(m.rule_index))))
            })
            .collect()
    }

    /// A rule's sink names, then its source names (to notice a retargeted rule)
    fn target_names(&self, rule_index: usize) -> Vec<String> {
        self.rule_sinks(rule_index)
            .chain(self.rule_sources(rule_index))
            .map(|s| s.name.clone())
            .collect()
    }

    /// Re-match every window against the current config, logging what changed since
    /// `before` (from [`Self::match_sinks`]); windows are visited oldest first, so the
    /// log reads the same every time
    fn rematch(&mut self, before: &HashMap<u64, (String, Vec<String>)>) {
        debug!("Re-evaluating all windows against current rules");
        let mut order: Vec<(u64, u64)> = self.windows.iter().map(|(id, w)| (w.seq, *id)).collect();
        order.sort_unstable();
        for (_, id) in order {
            let Some(window) = self.windows.get(&id) else {
                continue;
            };
            let matched = match_window(&self.config, window);
            match (before.get(&id), &matched) {
                (None, Some(m)) => info!(
                    "Rule matched: {} → {}",
                    window_label(id, window),
                    self.targets_desc(m.rule_index)
                ),
                (Some((trigger, _)), None) => info!(
                    "Rule unmatched: {} no longer matches (was: '{}')",
                    window_label(id, window),
                    for_log(trigger)
                ),
                (Some((_, old_targets)), Some(m))
                    if self.target_names(m.rule_index) != *old_targets =>
                {
                    info!(
                        "Rule matched: {} → {}",
                        window_label(id, window),
                        self.targets_desc(m.rule_index)
                    );
                }
                _ => {}
            }
            if let Some(window) = self.windows.get_mut(&id) {
                window.matched = matched;
            }
        }
    }

    /// Find the first rule matching `app_id` and `title` (on no monitor), with its index
    #[cfg(test)]
    fn find_matching_rule(&self, app_id: &str, title: &str) -> Option<(usize, &Rule)> {
        find_rule(&self.config, app_id, title, &[], false)
    }

    /// Fold one compositor event into the tracked windows (no I/O)
    ///
    /// Returns the window that stopped deciding anything, if the event closed a
    /// matching window or made one stop matching.
    pub fn apply(&mut self, event: WindowEvent) -> Option<Departure> {
        match event {
            WindowEvent::Opened {
                id,
                app_id,
                title,
                outputs,
                minimized,
            } => self.upsert(id, app_id, title, Some((outputs, minimized))),
            WindowEvent::Changed { id, app_id, title } => self.upsert(id, app_id, title, None),
            WindowEvent::Placement {
                id,
                outputs,
                minimized,
            } => self.place(id, outputs, minimized),
            WindowEvent::Closed { id } => self.close(id),
            // Only marks a point in the stream (see `daemon::event_loop`)
            WindowEvent::InitialSync => None,
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

    /// The candidate sinks of a rule, in priority order (the default sink for an
    /// index out of range, which the invariant rules out)
    fn rule_sinks(&self, rule_index: usize) -> impl Iterator<Item = &SinkConfig> {
        let indices = self
            .rule_sinks
            .get(rule_index)
            .map_or(std::slice::from_ref(&self.default_sink), Vec::as_slice);
        indices.iter().map(|&i| &self.config.sinks[i])
    }

    /// The source candidates of a rule, in priority order (empty without `source`)
    fn rule_sources(&self, rule_index: usize) -> impl Iterator<Item = &SourceConfig> {
        self.rule_sources
            .get(rule_index)
            .map_or(&[][..], Vec::as_slice)
            .iter()
            .map(|&i| &self.config.sources[i])
    }

    /// A rule's candidates for the log: `Desc`, or `Desc (fallback: Other, Third)`,
    /// then `, mic: Desc (fallback: …)` when the rule has a source
    fn targets_desc(&self, rule_index: usize) -> String {
        fn chain<'a>(mut descs: impl Iterator<Item = &'a str>) -> Option<String> {
            let first = descs.next()?.to_string();
            let rest: Vec<&str> = descs.collect();
            Some(if rest.is_empty() {
                first
            } else {
                format!("{first} (fallback: {})", rest.join(", "))
            })
        }
        let sinks = chain(self.rule_sinks(rule_index).map(|s| s.desc.as_str())).unwrap_or_default();
        match chain(self.rule_sources(rule_index).map(|s| s.desc.as_str())) {
            Some(sources) => format!("{sinks}, mic: {sources}"),
            None => sinks,
        }
    }

    /// A window opened (with its initial placement) or changed `app_id`/title
    fn upsert(
        &mut self,
        id: u64,
        app_id: String,
        title: String,
        placement: Option<(Vec<String>, bool)>,
    ) -> Option<Departure> {
        debug!(
            "Window: id={}, app_id='{}', title='{}'{}",
            id,
            for_log(&app_id),
            for_log(&title),
            placement
                .as_ref()
                .map_or_else(String::new, |(outputs, minimized)| format!(
                    ", outputs=[{}], minimized={minimized}",
                    for_log(&outputs.join(", "))
                ))
        );
        if self.windows.contains_key(&id) {
            return self.refresh(id, |w| {
                w.app_id = app_id;
                w.title = title;
                if let Some((outputs, minimized)) = placement {
                    w.outputs = outputs;
                    w.minimized = minimized;
                }
            });
        }
        let seq = self.take_seq();
        let (outputs, minimized) = placement.unwrap_or_default();
        let mut window = Window {
            app_id,
            title,
            outputs,
            minimized,
            seq,
            matched: None,
            last_rule: None,
        };
        window.matched = match_window(&self.config, &window);
        if let Some(m) = &window.matched {
            info!(
                "Rule matched: {} → {}",
                window_label(id, &window),
                self.targets_desc(m.rule_index)
            );
        }
        self.windows.insert(id, window);
        None
    }

    /// A window's monitors or minimized state changed (`wlr` only)
    fn place(&mut self, id: u64, outputs: Vec<String>, minimized: bool) -> Option<Departure> {
        debug!(
            "Window placement: id={}, outputs=[{}], minimized={}",
            id,
            for_log(&outputs.join(", ")),
            minimized
        );
        if !self.windows.contains_key(&id) {
            // The compositor layer never sends this before `Opened`
            debug!("Placement for unknown window {id}; ignored");
            return None;
        }
        self.refresh(id, |w| {
            w.outputs = outputs;
            w.minimized = minimized;
        })
    }

    /// Apply `change` to the known window `id` and re-match it: starting to match a
    /// rule (also a different rule than before, e.g. after a title change) counts as
    /// the most recent activity (a new sequence number); keeping its rule keeps the
    /// trigger text from when the rule first matched
    fn refresh(&mut self, id: u64, change: impl FnOnce(&mut Window)) -> Option<Departure> {
        let window = self.windows.get_mut(&id)?;
        let previous = window.matched.take();
        let before = window.clone();
        change(window);
        let window = self.windows.get(&id)?;
        let matched = match_window(&self.config, window);

        let mut renew = false;
        let mut left = None;
        let departure = match (&previous, &matched) {
            // Back to the rule it just left (e.g. a flickering title): leaving was
            // logged, and a switch it causes is logged by the loop
            (None, Some(m)) if window.last_rule == Some(m.rule_index) => {
                debug!(
                    "Rule matched again: {} → {}",
                    window_label(id, window),
                    self.targets_desc(m.rule_index)
                );
                renew = true;
                None
            }
            (None, Some(m)) => {
                info!(
                    "Rule matched: {} → {}",
                    window_label(id, window),
                    self.targets_desc(m.rule_index)
                );
                renew = true;
                None
            }
            (Some(old), None) => {
                info!(
                    "Rule unmatched: {} no longer matches (was: '{}'{})",
                    window_label(id, window),
                    for_log(&old.trigger),
                    what_changed(&before, window)
                );
                left = Some(old.rule_index);
                Some(Departure {
                    trigger: old.trigger.clone(),
                    notify: self.rule_notify(old.rule_index),
                    closed: false,
                })
            }
            (Some(old), Some(new)) if old.rule_index != new.rule_index => {
                if window.last_rule == Some(new.rule_index) {
                    debug!(
                        "Rule matched again: {} → {}",
                        window_label(id, window),
                        self.targets_desc(new.rule_index)
                    );
                } else {
                    info!(
                        "Rule matched: {} → {} (another rule than before, → {}{})",
                        window_label(id, window),
                        self.targets_desc(new.rule_index),
                        self.targets_desc(old.rule_index),
                        what_changed(&before, window)
                    );
                    left = Some(old.rule_index);
                }
                renew = true;
                None
            }
            _ => None,
        };

        let seq = renew.then(|| self.take_seq());
        if let Some(w) = self.windows.get_mut(&id) {
            if let Some(seq) = seq {
                w.seq = seq;
            }
            if left.is_some() {
                w.last_rule = left;
            }
            w.matched = match (previous, matched) {
                (Some(old), Some(new)) if old.rule_index == new.rule_index => Some(old),
                (_, new) => new,
            };
        }
        departure
    }

    fn close(&mut self, id: u64) -> Option<Departure> {
        let mut window = self.windows.remove(&id)?;
        let matched = window.matched.take()?;
        // The trigger is the rule's `desc`, or the title when the rule matched: named
        // only when it adds something to the label
        let was = if matched.trigger == window.title {
            String::new()
        } else {
            format!(" (was: '{}')", for_log(&matched.trigger))
        };
        info!("Tracked window closed: {}{was}", window_label(id, &window));
        Some(Departure {
            notify: self.rule_notify(matched.rule_index),
            trigger: matched.trigger,
            closed: true,
        })
    }

    /// The sinks the rules want right now (in priority order), and the window that
    /// decided them
    ///
    /// Priority follows `match_by_index`:
    /// - `false`: the most recently opened (or most recently matching) window wins
    /// - `true`: the window whose rule has the lowest index wins; most recent breaks ties
    ///
    /// Falls back to the default sink (as the only candidate) when no window matches.
    /// Which candidate is reachable is not decided here.
    #[must_use]
    pub fn desired(&self) -> Desired<'_> {
        self.decide(|rule_index| self.rule_sinks(rule_index).collect())
            .unwrap_or_else(|| Desired {
                candidates: vec![self.default_sink()],
                window: None,
            })
    }

    /// What the rules want for `kind`: [`State::desired`] for sinks; for sources the
    /// same priority over the matching windows whose rule has a `source`, else the
    /// default source, and `None` when no sources are configured
    #[must_use]
    pub fn desired_for(&self, kind: Endpoint) -> Option<Desired<'_>> {
        match kind {
            Endpoint::Sink => Some(self.desired()),
            Endpoint::Source => {
                let default = self.default_source?;
                Some(
                    self.decide(|rule_index| self.rule_sources(rule_index).collect())
                        .unwrap_or_else(|| Desired {
                            candidates: vec![&self.config.sources[default]],
                            window: None,
                        }),
                )
            }
        }
    }

    /// The winning window among those whose rule has candidates (`targets` of its
    /// rule index, empty for "this rule has no say"), by the priority mode
    fn decide<'a>(&'a self, targets: impl Fn(usize) -> Vec<&'a SinkConfig>) -> Option<Desired<'a>> {
        let candidates = self.windows.iter().filter_map(|(id, w)| {
            let m = w.matched.as_ref()?;
            let targets = targets(m.rule_index);
            (!targets.is_empty()).then_some((*id, w, m, targets))
        });
        let winner = if self.config.settings.match_by_index {
            candidates.min_by_key(|(_, w, m, _)| (m.rule_index, std::cmp::Reverse(w.seq)))
        } else {
            candidates.max_by_key(|(_, w, ..)| w.seq)
        };

        winner.map(|(id, w, m, targets)| Desired {
            candidates: targets,
            window: Some(Winner {
                id,
                app_id: &w.app_id,
                title: &w.title,
                rule_index: m.rule_index,
                trigger: &m.trigger,
                notify: self.rule_notify(m.rule_index),
            }),
        })
    }

    /// The rules' first choice right now (see [`State::desired`])
    #[must_use]
    pub fn desired_sink(&self) -> &SinkConfig {
        self.desired().first()
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

    /// Every open window, sorted by id, with the sinks its rule points at (if any)
    #[must_use]
    pub fn windows(&self) -> Vec<WindowView<'_>> {
        let mut views: Vec<WindowView<'_>> = self
            .windows
            .iter()
            .map(|(id, w)| WindowView {
                id: *id,
                app_id: &w.app_id,
                title: &w.title,
                outputs: &w.outputs,
                minimized: w.minimized,
                sinks: w
                    .matched
                    .as_ref()
                    .map_or_else(Vec::new, |m| self.rule_sinks(m.rule_index).collect()),
                sources: w
                    .matched
                    .as_ref()
                    .map_or_else(Vec::new, |m| self.rule_sources(m.rule_index).collect()),
            })
            .collect();
        views.sort_by_key(|v| v.id);
        views
    }
}

fn find_rule<'a>(
    config: &'a Config,
    app_id: &str,
    title: &str,
    outputs: &[String],
    minimized: bool,
) -> Option<(usize, &'a Rule)> {
    config
        .rules
        .iter()
        .enumerate()
        .find(|(_, rule)| rule.matches(app_id, title, outputs, minimized))
}

fn match_window(config: &Config, window: &Window) -> Option<Match> {
    find_rule(
        config,
        &window.app_id,
        &window.title,
        &window.outputs,
        window.minimized,
    )
    .map(|(rule_index, rule)| Match {
        rule_index,
        trigger: rule.desc.clone().unwrap_or_else(|| window.title.clone()),
    })
}

/// What changed between two states of a window, for a rule log line (`; title
/// changed`, `; now on DP-1`, `; now minimized`); empty when nothing it shows did
fn what_changed(before: &Window, now: &Window) -> String {
    let mut parts = Vec::new();
    if before.app_id != now.app_id {
        parts.push("app_id changed".to_string());
    }
    if before.title != now.title {
        parts.push("title changed".to_string());
    }
    if before.outputs != now.outputs {
        parts.push(if now.outputs.is_empty() {
            "now on no monitor".to_string()
        } else {
            format!("now on {}", for_log(&now.outputs.join(", ")))
        });
    }
    if before.minimized != now.minimized {
        parts.push(
            if now.minimized {
                "now minimized"
            } else {
                "now restored"
            }
            .to_string(),
        );
    }
    parts.iter().fold(String::new(), |mut out, p| {
        out.push_str("; ");
        out.push_str(p);
        out
    })
}

/// A window for rule log lines: `window 12 'kitty' "~/src"`, so lines about several
/// windows of one app can be told apart (untrusted text sanitised, title shortened)
fn window_label(id: u64, window: &Window) -> String {
    format!(
        "window {id} '{}' \"{}\"",
        for_log(&window.app_id),
        sanitize_line(&window.title, LOG_TITLE_CHARS)
    )
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;
    use test_case::test_case;

    use super::*;
    use crate::test_utils::fixtures::{
        make_config, make_rule, make_rule_with_fallbacks, make_sink, with_output, with_sources,
    };

    fn opened(id: u64, app_id: &str, title: &str) -> WindowEvent {
        WindowEvent::Opened {
            id,
            app_id: app_id.to_string(),
            title: title.to_string(),
            outputs: Vec::new(),
            minimized: false,
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

    /// Why [`State::new`] refuses `config`
    fn new_error(config: Config) -> String {
        match State::new(Arc::new(config)) {
            Ok(_) => panic!("config accepted"),
            Err(e) => e.to_string(),
        }
    }

    #[test]
    fn test_new_rejects_empty_sinks() {
        assert_eq!(
            new_error(make_config(vec![], vec![])),
            "No sinks configured"
        );
    }

    #[test]
    fn test_new_rejects_missing_default() {
        let config = make_config(vec![make_sink("a", "A", false)], vec![]);
        assert_eq!(new_error(config), "No default sink configured");
    }

    #[test]
    fn test_new_rejects_unresolvable_rule() {
        let config = make_config(vec![make_sink("a", "A", true)], vec![make_rule(
            "x", None, "nope",
        )]);
        let err = new_error(config);
        assert!(err.ends_with("references an unknown sink (nope)"), "{err}");
    }

    #[test]
    fn test_no_windows_desires_default() {
        let s = state(three_sink_config(false));
        let desired = s.desired();
        assert_eq!(desired.first().name, "speakers");
        assert!(desired.window.is_none());
    }

    #[test]
    fn test_open_matching_window_desires_its_sink() {
        let mut s = state(three_sink_config(false));
        assert_eq!(s.apply(opened(1, "firefox", "Browser")), None);
        let desired = s.desired();
        assert_eq!(desired.first().name, "headphones");
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
        assert_eq!(desired.first().name, "speakers");
        assert_eq!(desired.window.map(|w| w.rule_index), Some(0));
        assert_eq!(desired.window.map(|w| w.title), Some("Music"));
        assert_eq!(
            desired.window.map(|w| w.trigger),
            Some("Music"),
            "a different rule takes the trigger from the window as it is now"
        );
    }

    /// B4: a window that starts matching a *different* rule (its title changed) is the
    /// most recent activity, ahead of a window that started matching before that
    #[test]
    fn test_moving_to_another_rule_renews_priority() {
        let config = make_config(
            vec![
                make_sink("speakers", "Speakers", true),
                make_sink("hdmi", "HDMI", false),
                make_sink("headphones", "Headphones", false),
                make_sink("usb", "USB", false),
            ],
            vec![
                make_rule("firefox", Some("YouTube"), "hdmi"),
                make_rule("firefox", None, "headphones"),
                make_rule("^steam$", None, "usb"),
            ],
        );
        let mut s = state(config);
        s.apply(opened(1, "firefox", "New Tab"));
        s.apply(opened(2, "steam", "Steam"));
        assert_eq!(s.desired_sink().name, "usb");
        assert_eq!(s.apply(changed(1, "firefox", "Cats - YouTube")), None);
        let desired = s.desired();
        assert_eq!(desired.first().name, "hdmi", "the moved window wins");
        assert_eq!(desired.window.map(|w| w.id), Some(1));

        // Back to its first rule: again the most recent
        s.apply(changed(2, "steam", "Steam - Library"));
        s.apply(changed(1, "firefox", "New Tab"));
        assert_eq!(s.desired_sink().name, "headphones");
    }

    /// While a window keeps matching the same rule, title changes neither renew its
    /// priority nor replace the trigger it first matched with
    #[test]
    fn test_trigger_kept_from_first_match_while_the_same_rule_holds() {
        let config = make_config(
            vec![
                make_sink("speakers", "Speakers", true),
                make_sink("hdmi", "HDMI", false),
                make_sink("headphones", "Headphones", false),
            ],
            vec![
                make_rule("firefox", Some("YouTube"), "hdmi"),
                make_rule("^steam$", None, "headphones"),
            ],
        );
        let mut s = state(config);
        s.apply(opened(1, "firefox", "YouTube - First"));
        assert_eq!(s.apply(changed(1, "firefox", "YouTube - Second")), None);
        let desired = s.desired();
        assert_eq!(desired.window.map(|w| w.title), Some("YouTube - Second"));
        assert_eq!(desired.window.map(|w| w.trigger), Some("YouTube - First"));

        // A newer matching window stays ahead of the retitled one
        s.apply(opened(2, "steam", "Steam"));
        s.apply(changed(1, "firefox", "YouTube - Third"));
        assert_eq!(s.desired().window.map(|w| w.id), Some(2));
        s.apply(WindowEvent::Closed { id: 2 });
        assert_eq!(
            s.desired().window.map(|w| w.trigger),
            Some("YouTube - First")
        );

        // Leaving the rule reports the trigger it first matched with
        assert_eq!(
            s.apply(changed(1, "firefox", "Search")),
            Some(Departure {
                trigger: "YouTube - First".to_string(),
                notify: true,
                closed: false,
            })
        );
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
    fn test_reconfigure_is_deterministic_with_many_windows() {
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

    /// Capture what `f` logs at `INFO` and above (this thread only), without `ANSI`
    fn capture_info_logs(f: impl FnOnce()) -> String {
        #[derive(Clone, Default)]
        struct Buffer(Arc<std::sync::Mutex<Vec<u8>>>);
        impl std::io::Write for Buffer {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                self.0.lock().expect("lock").extend_from_slice(buf);
                Ok(buf.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let buffer = Buffer::default();
        let writer = buffer.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(move || writer.clone())
            .with_ansi(false)
            .with_max_level(tracing::Level::INFO)
            .finish();
        tracing::subscriber::with_default(subscriber, f);
        let bytes = buffer.0.lock().expect("lock").clone();
        String::from_utf8(bytes).expect("utf-8 logs")
    }

    /// A reload that makes an open window match (or stop matching, or point at another
    /// sink) says so, like a window event would; untrusted text is sanitised
    #[test]
    fn test_reconfigure_logs_matches_and_unmatches() {
        let sinks = vec![
            make_sink("speakers", "Speakers", true),
            make_sink("headphones", "Headphones", false),
            make_sink("hdmi", "HDMI", false),
        ];
        let mut s = state(make_config(sinks.clone(), vec![
            make_rule("^steam$", None, "hdmi"),
            make_rule("^mpv$", None, "hdmi"),
        ]));
        s.apply(opened(1, "steam", "Steam"));
        s.apply(opened(2, "mpv", "video"));
        s.apply(opened(3, "fire\nfox", "Browser"));

        let config = make_config(sinks, vec![
            make_rule("^mpv$", None, "headphones"),
            make_rule("^fire", None, "headphones"),
        ]);
        let logs = capture_info_logs(|| {
            s.reconfigure(Arc::new(config.clone()))
                .expect("valid config");
        });
        let lines: Vec<&str> = logs
            .lines()
            .filter_map(|l| l.find("Rule ").map(|i| &l[i..]))
            .collect();
        assert_eq!(lines, vec![
            "Rule unmatched: window 1 'steam' \"Steam\" no longer matches (was: 'Steam')",
            "Rule matched: window 2 'mpv' \"video\" → Headphones",
            "Rule matched: window 3 'fire\\nfox' \"Browser\" → Headphones",
        ]);

        // Nothing changed: nothing logged
        let logs = capture_info_logs(|| {
            s.reconfigure(Arc::new(config)).expect("valid config");
        });
        assert!(!logs.contains("Rule "), "{logs}");
    }

    /// Rule lines for several windows of one app name each window and its (sanitised,
    /// shortened) title, so they can be told apart
    #[test]
    fn test_rule_log_lines_identify_the_window() {
        let sinks = vec![make_sink("headphones", "Headphones", true)];
        let mut s = state(make_config(sinks, vec![make_rule(
            "^kitty$",
            None,
            "headphones",
        )]));
        let long_title = format!("~/src\n{}", "x".repeat(100));
        let logs = capture_info_logs(|| {
            s.apply(opened(7, "kitty", "~/src"));
            s.apply(opened(8, "kitty", &long_title));
        });
        let shortened = format!("~/src\\n{}…", "x".repeat(LOG_TITLE_CHARS - 6));
        let lines: Vec<&str> = logs
            .lines()
            .filter_map(|l| l.find("Rule ").map(|i| &l[i..]))
            .collect();
        assert_eq!(lines, vec![
            "Rule matched: window 7 'kitty' \"~/src\" → Headphones".to_string(),
            format!("Rule matched: window 8 'kitty' \"{shortened}\" → Headphones"),
        ]);
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
        let err = s
            .reconfigure(Arc::new(make_config(vec![], vec![])))
            .expect_err("no sinks");
        assert_eq!(err.to_string(), "No sinks configured");
        assert_eq!(s.desired_sink().name, "headphones");
    }

    #[test]
    fn test_windows_view_sorted_with_sinks() {
        let mut s = state(three_sink_config(false));
        s.apply(opened(5, "firefox", "B"));
        s.apply(opened(2, "zsh", "A"));
        let views = s.windows();
        assert_eq!(views.iter().map(|v| v.id).collect::<Vec<_>>(), vec![2, 5]);
        assert_eq!(views[0].sinks.len(), 0, "unmatched");
        assert_eq!(
            views[1]
                .sinks
                .iter()
                .map(|k| k.name.as_str())
                .collect::<Vec<_>>(),
            ["headphones"]
        );
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

    /// `three_sink_config` with firefox → [headphones, hdmi] (headphones first)
    fn fallback_config(match_by_index: bool) -> Config {
        let mut config = three_sink_config(match_by_index);
        config.rules[1] = make_rule_with_fallbacks("firefox", None, &["Headphones", "hdmi"]);
        config
    }

    fn names(desired: &Desired<'_>) -> Vec<String> {
        desired.names()
    }

    #[test]
    fn test_desired_is_the_winning_rules_candidate_list() {
        let mut s = state(fallback_config(false));
        assert_eq!(names(&s.desired()), ["speakers"], "default alone");
        s.apply(opened(1, "firefox", "Docs"));
        let desired = s.desired();
        assert_eq!(names(&desired), ["headphones", "hdmi"]);
        assert_eq!(desired.first().name, "headphones");
        assert_eq!(desired.window.map(|w| w.id), Some(1));
        // A single-sink rule wins: its one sink is the whole list
        s.apply(opened(2, "steam", "Library"));
        assert_eq!(names(&s.desired()), ["hdmi"]);
        s.apply(WindowEvent::Closed { id: 2 });
        assert_eq!(names(&s.desired()), ["headphones", "hdmi"]);
        s.apply(WindowEvent::Closed { id: 1 });
        assert_eq!(names(&s.desired()), ["speakers"]);
    }

    #[test]
    fn test_windows_view_lists_fallbacks_and_reconfigure_follows_them() {
        let mut s = state(three_sink_config(false));
        s.apply(opened(1, "firefox", "Docs"));
        assert_eq!(names(&s.desired()), ["headphones"]);
        // (Captured: a log call made without a subscriber can race other tests'
        // capture of the same call site)
        let logs = capture_info_logs(|| {
            s.reconfigure(Arc::new(fallback_config(false)))
                .expect("valid config");
        });
        assert!(
            logs.contains(
                "Rule matched: window 1 'firefox' \"Docs\" → Headphones (fallback: HDMI)"
            ),
            "{logs}"
        );
        assert_eq!(names(&s.desired()), ["headphones", "hdmi"]);
        let views = s.windows();
        let sinks: Vec<&str> = views[0].sinks.iter().map(|k| k.name.as_str()).collect();
        assert_eq!(sinks, ["headphones", "hdmi"]);
    }

    /// Sinks Speakers (default), Headset; sources Desk Mic (default), Headset Mic;
    /// discord → Headset + Headset Mic, firefox → Speakers (no source),
    /// zoom → Speakers + [Headset Mic, Desk Mic]
    fn source_config(match_by_index: bool) -> Config {
        let mut config = make_config(
            vec![
                make_sink("speakers", "Speakers", true),
                make_sink("headset", "Headset", false),
            ],
            vec![
                with_sources(make_rule("^discord$", None, "headset"), &["Headset Mic"]),
                make_rule("^firefox$", None, "speakers"),
                with_sources(make_rule("^zoom$", None, "speakers"), &[
                    "Headset Mic",
                    "Desk Mic",
                ]),
            ],
        )
        .with_sources(vec![
            make_sink("mic.desk", "Desk Mic", true),
            make_sink("mic.headset", "Headset Mic", false),
        ]);
        config.settings.match_by_index = match_by_index;
        config
    }

    fn source_names(s: &State) -> Vec<String> {
        s.desired_for(Endpoint::Source)
            .expect("sources configured")
            .names()
    }

    #[test]
    fn test_no_sources_configured_means_no_source_answer() {
        let mut s = state(three_sink_config(false));
        assert!(s.desired_for(Endpoint::Source).is_none());
        s.apply(opened(1, "firefox", "x"));
        assert!(s.desired_for(Endpoint::Source).is_none());
        assert_eq!(
            s.desired_for(Endpoint::Sink).map(|d| d.names()),
            Some(vec!["headphones".to_string()])
        );
        assert_eq!(s.windows()[0].sources, Vec::<&SinkConfig>::new());
    }

    #[test]
    fn test_sink_and_source_are_independent_dimensions() {
        let mut s = state(source_config(false));
        assert_eq!(source_names(&s), ["mic.desk"], "default source");

        s.apply(opened(1, "discord", "Voice"));
        assert_eq!(names(&s.desired()), ["headset"]);
        assert_eq!(source_names(&s), ["mic.headset"]);

        // Firefox wins the sink but has no say about the microphone
        s.apply(opened(2, "firefox", "Video"));
        assert_eq!(names(&s.desired()), ["speakers"]);
        assert_eq!(source_names(&s), ["mic.headset"]);
        let source = s.desired_for(Endpoint::Source).expect("sources");
        assert_eq!(
            source.window.map(|w| w.id),
            Some(1),
            "Discord decides the mic"
        );

        // Zoom decides both, with a fallback microphone
        s.apply(opened(3, "zoom", "Meeting"));
        assert_eq!(source_names(&s), ["mic.headset", "mic.desk"]);
        s.apply(WindowEvent::Closed { id: 3 });
        assert_eq!(source_names(&s), ["mic.headset"]);

        // Closing Discord: no rule with a source left → default source
        s.apply(WindowEvent::Closed { id: 1 });
        assert_eq!(source_names(&s), ["mic.desk"]);
        assert!(
            s.desired_for(Endpoint::Source)
                .expect("sources")
                .window
                .is_none()
        );
        assert_eq!(names(&s.desired()), ["speakers"]);
    }

    #[test]
    fn test_source_priority_follows_match_by_index() {
        let mut s = state(source_config(true));
        s.apply(opened(1, "zoom", "Meeting"));
        s.apply(opened(2, "discord", "Voice"));
        // Rule 0 (discord) outranks rule 2 (zoom) for both dimensions
        assert_eq!(names(&s.desired()), ["headset"]);
        assert_eq!(source_names(&s), ["mic.headset"]);
        // Firefox (rule 1) outranks zoom for the sink only; discord still decides both
        s.apply(WindowEvent::Closed { id: 2 });
        s.apply(opened(3, "firefox", "x"));
        assert_eq!(names(&s.desired()), ["speakers"]);
        assert_eq!(s.desired().window.map(|w| w.id), Some(3));
        assert_eq!(source_names(&s), ["mic.headset", "mic.desk"]);
        assert_eq!(
            s.desired_for(Endpoint::Source)
                .and_then(|d| d.window)
                .map(|w| w.id),
            Some(1)
        );
    }

    #[test]
    fn test_windows_view_lists_sources_and_reconfigure_logs_them() {
        let mut s = state(source_config(false));
        s.apply(opened(1, "zoom", "Meeting"));
        let views = s.windows();
        let sources: Vec<&str> = views[0].sources.iter().map(|k| k.desc.as_str()).collect();
        assert_eq!(sources, ["Headset Mic", "Desk Mic"]);

        // Retargeting only the rule's source is a change worth logging
        let mut config = source_config(false);
        config.rules[2].source_refs = vec!["Desk Mic".to_string()];
        let logs = capture_info_logs(|| {
            s.reconfigure(Arc::new(config)).expect("valid config");
        });
        assert!(
            logs.contains("Rule matched: window 1 'zoom' \"Meeting\" → Speakers, mic: Desk Mic"),
            "{logs}"
        );
        assert_eq!(source_names(&s), ["mic.desk"]);
    }

    #[test]
    fn test_state_rejects_source_rule_without_sources() {
        let config = make_config(vec![make_sink("a", "A", true)], vec![with_sources(
            make_rule("x", None, "a"),
            &["Mic"],
        )]);
        let err = new_error(config);
        assert!(
            err.ends_with("sets a source but no sources are configured"),
            "{err}"
        );
    }

    // ------------------------------------------------------------------------
    // Monitor (`output`) rules
    // ------------------------------------------------------------------------

    fn placed(id: u64, outputs: &[&str], minimized: bool) -> WindowEvent {
        WindowEvent::Placement {
            id,
            outputs: outputs.iter().map(|o| (*o).to_string()).collect(),
            minimized,
        }
    }

    /// Default speakers; any window on `HDMI-A-1` → TV; firefox → headphones
    fn monitor_config(match_by_index: bool) -> Config {
        let mut config = make_config(
            vec![
                make_sink("speakers", "Speakers", true),
                make_sink("tv", "TV", false),
                make_sink("headphones", "Headphones", false),
            ],
            vec![
                with_output(make_rule("", None, "TV"), "^HDMI-A-1$"),
                make_rule("firefox", None, "Headphones"),
            ],
        );
        config.settings.match_by_index = match_by_index;
        config
    }

    /// A window that opens on a monitor matches its output rule straight away (the
    /// initial placement comes with `Opened`), not first a non-output rule
    #[test]
    fn test_opened_with_placement_matches_output_rule_at_once() {
        let mut s = state(monitor_config(false));
        s.apply(WindowEvent::Opened {
            id: 1,
            app_id: "firefox".into(),
            title: "Video".into(),
            outputs: vec!["HDMI-A-1".into()],
            minimized: false,
        });
        assert_eq!(s.desired_sink().name, "tv");
        assert_eq!(s.windows()[0].outputs, ["HDMI-A-1"]);

        // Opening minimized there: the output rule does not apply
        s.apply(WindowEvent::Opened {
            id: 2,
            app_id: "firefox".into(),
            title: "Hidden".into(),
            outputs: vec!["HDMI-A-1".into()],
            minimized: true,
        });
        assert!(s.windows()[1].minimized);
        assert_eq!(s.apply(WindowEvent::InitialSync), None);
    }

    #[test]
    fn test_output_rule_matches_window_on_that_monitor() {
        let mut s = state(monitor_config(false));
        s.apply(opened(1, "mpv", "video"));
        assert_eq!(s.desired_sink().name, "speakers", "placement not known yet");
        assert_eq!(s.apply(placed(1, &["HDMI-A-1"], false)), None);
        let desired = s.desired();
        assert_eq!(desired.first().name, "tv");
        assert_eq!(desired.window.map(|w| w.trigger), Some("video"));
        let views = s.windows();
        assert_eq!(views[0].outputs, ["HDMI-A-1"]);
        assert!(!views[0].minimized);
    }

    /// Moving to another monitor stops matching (back to the default); moving back
    /// matches again as fresh activity, winning over a window that matched since
    #[test]
    fn test_moving_between_monitors_rematches_with_a_new_sequence() {
        let mut s = state(monitor_config(false));
        s.apply(opened(1, "mpv", "video"));
        s.apply(placed(1, &["HDMI-A-1"], false));
        assert_eq!(s.desired_sink().name, "tv");

        let departure = s.apply(placed(1, &["DP-1"], false));
        assert_eq!(
            departure,
            Some(Departure {
                trigger: "video".to_string(),
                notify: true,
                closed: false,
            })
        );
        assert_eq!(s.desired_sink().name, "speakers");

        s.apply(opened(2, "firefox", "Browser"));
        assert_eq!(s.desired_sink().name, "headphones");
        assert_eq!(s.apply(placed(1, &["HDMI-A-1"], false)), None);
        assert_eq!(s.desired_sink().name, "tv", "newly matching wins");
    }

    /// A window spanning both monitors matches; moving between two matching monitors
    /// keeps its place in the priority order
    #[test]
    fn test_spanning_and_moving_between_matching_monitors_keep_sequence() {
        let mut config = monitor_config(false);
        config.rules[0] = with_output(make_rule("", None, "TV"), "^HDMI-");
        let mut s = state(config);
        s.apply(opened(1, "mpv", "video"));
        s.apply(placed(1, &["DP-1", "HDMI-A-1"], false));
        assert_eq!(s.desired_sink().name, "tv", "spans both: matches");
        s.apply(opened(2, "firefox", "Browser"));
        assert_eq!(s.desired_sink().name, "headphones");

        // Still on a matching monitor: no fresh activity, firefox stays newer
        s.apply(placed(1, &["HDMI-A-2"], false));
        s.apply(placed(1, &["HDMI-A-1"], false));
        assert_eq!(s.desired_sink().name, "headphones");
        s.apply(WindowEvent::Closed { id: 2 });
        assert_eq!(s.desired_sink().name, "tv");
    }

    #[test]
    fn test_minimized_window_does_not_match_output_rule() {
        let mut s = state(monitor_config(false));
        s.apply(opened(1, "mpv", "video"));
        s.apply(placed(1, &["HDMI-A-1"], true));
        assert_eq!(s.desired_sink().name, "speakers");
        assert!(s.windows()[0].minimized);

        s.apply(placed(1, &["HDMI-A-1"], false));
        assert_eq!(s.desired_sink().name, "tv", "restored");
        assert!(
            s.apply(placed(1, &["HDMI-A-1"], true)).is_some(),
            "minimized again"
        );
        assert_eq!(s.desired_sink().name, "speakers");
    }

    /// `app_id` and `output` together: both must match
    #[test]
    fn test_app_id_and_output_combined() {
        let config = make_config(
            vec![
                make_sink("speakers", "Speakers", true),
                make_sink("tv", "TV", false),
            ],
            vec![with_output(make_rule("^mpv$", None, "TV"), "^HDMI-")],
        );
        let mut s = state(config);
        s.apply(opened(1, "firefox", "Browser"));
        s.apply(placed(1, &["HDMI-A-1"], false));
        assert_eq!(s.desired_sink().name, "speakers", "wrong app_id");
        s.apply(opened(2, "mpv", "video"));
        assert_eq!(s.desired_sink().name, "speakers", "no placement yet");
        s.apply(placed(2, &["DP-1"], false));
        assert_eq!(s.desired_sink().name, "speakers", "wrong monitor");
        s.apply(placed(2, &["HDMI-A-1"], false));
        assert_eq!(s.desired_sink().name, "tv");
        assert_eq!(s.tracked_count(), 1);
    }

    /// The `ext` backend never sends `Placement`: output rules never match its windows,
    /// whatever their `app_id`; other rules still do
    #[test]
    fn test_windows_without_placement_never_match_output_rules() {
        let mut config = monitor_config(false);
        config.rules[0] = with_output(make_rule(".*", None, "TV"), ".*");
        let mut s = state(config);
        s.apply(opened(1, "mpv", "video"));
        s.apply(changed(1, "mpv", "video 2"));
        assert_eq!(s.tracked_count(), 0);
        s.apply(opened(2, "firefox", "Browser"));
        assert_eq!(s.desired_sink().name, "headphones");
        assert_eq!(s.windows()[0].outputs, Vec::<String>::new());
    }

    /// With `match_by_index`, an output rule ranks by its index like any other rule
    #[test]
    fn test_output_rule_priority_by_index() {
        let mut s = state(monitor_config(true));
        s.apply(opened(1, "mpv", "video"));
        s.apply(placed(1, &["HDMI-A-1"], false));
        s.apply(opened(2, "firefox", "Browser"));
        assert_eq!(s.desired_sink().name, "tv", "rule 1 beats rule 2");
    }

    /// A placement for a window the reducer never saw open is ignored
    #[test]
    fn test_placement_for_unknown_window_is_ignored() {
        let mut s = state(monitor_config(false));
        assert_eq!(s.apply(placed(7, &["HDMI-A-1"], false)), None);
        assert_eq!(s.windows().len(), 0);
    }

    /// A reload that adds an output rule matches windows already on that monitor
    #[test]
    fn test_reconfigure_with_output_rule_uses_known_placement() {
        let sinks = vec![
            make_sink("speakers", "Speakers", true),
            make_sink("tv", "TV", false),
        ];
        let mut s = state(make_config(sinks.clone(), vec![]));
        s.apply(opened(1, "mpv", "video"));
        s.apply(placed(1, &["HDMI-A-1"], false));
        assert_eq!(s.desired_sink().name, "speakers");
        s.reconfigure(Arc::new(make_config(sinks, vec![with_output(
            make_rule("", None, "TV"),
            "HDMI",
        )])))
        .expect("valid config");
        assert_eq!(s.desired_sink().name, "tv");
    }
}
