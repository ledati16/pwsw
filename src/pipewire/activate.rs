//! Activation: resolve configured node names against a fresh snapshot, switch the
//! card profile if needed, wait for the node, set it as default.
//!
//! A job has up to two halves, the sink and the source (microphone), each a list of
//! candidates in priority order (a rule with fallbacks). [`activate_joint`] resolves
//! both against one snapshot ([`resolve::plan_joint`]): when both need the same card,
//! that card is switched once, to a profile providing both. Each half activates the
//! first reachable candidate, moving on to the next reachable one if that activation
//! fails.
//!
//! Everything goes through [`AudioBackend`] and [`Clock`], so the whole flow is
//! unit-tested without `PipeWire` and without real sleeps.

use std::collections::HashSet;
use std::fmt::Write as _;
use std::time::{Duration, Instant};

use anyhow::Result;
use tracing::{debug, info, warn};

use super::model::Snapshot;
use super::resolve::{self, Plan, PreviousProfile, ProfileSwitch, SinkSide, SourceStep};
use crate::config::Endpoint;
use crate::sanitize::{for_log, one_line};

/// The operations activation needs from an audio server.
pub(crate) trait AudioBackend {
    /// Take a fresh snapshot of devices, sinks, sources and defaults.
    fn snapshot(&self) -> Result<Snapshot>;
    /// Set the configured default sink (or source) by node name.
    fn set_default(&self, kind: Endpoint, node_name: &str) -> Result<()>;
    /// Switch a device to a profile (persistently).
    fn set_profile(&self, device_id: u32, profile_index: u32) -> Result<()>;
}

/// Time source for the post-switch wait (injectable for tests).
pub(crate) trait Clock {
    fn now(&self) -> Instant;
    fn sleep(&self, duration: Duration);
}

/// The real clock.
pub(crate) struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Instant {
        Instant::now()
    }

    fn sleep(&self, duration: Duration) {
        std::thread::sleep(duration);
    }
}

/// Default poll interval while waiting for a sink after a profile switch.
/// Override with `PROFILE_SWITCH_DELAY_MS`.
const DEFAULT_POLL_INTERVAL_MS: u64 = 150;
/// Default number of polls (3 s in all with the default interval). Override with
/// `PROFILE_SWITCH_MAX_RETRIES`.
const DEFAULT_MAX_POLLS: u64 = 20;
/// Bounds for `PROFILE_SWITCH_DELAY_MS`.
const MIN_POLL_INTERVAL_MS: u64 = 10;
const MAX_POLL_INTERVAL_MS: u64 = 1_000;
/// Hard cap on the total sleep (interval × polls) of one activation's post-switch
/// wait. It caps the sleeps only: each poll's `pw-dump` can add up to its own timeout
/// (`PW_DUMP_TIMEOUT`).
///
/// This does not bound how long an `IPC` client waits: a manual switch queued behind
/// an in-flight activation waits for both. The daemon's guard is its 8 s "switch in
/// progress" reply (`SWITCH_REPLY_TIMEOUT`), below the client's 10 s read timeout.
const MAX_TOTAL_WAIT_MS: u64 = 5_000;

/// How long to wait for the sink node after a profile switch, and how often to look.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct WaitPolicy {
    pub(crate) poll_interval: Duration,
    pub(crate) timeout: Duration,
}

impl WaitPolicy {
    /// From `PROFILE_SWITCH_DELAY_MS` / `PROFILE_SWITCH_MAX_RETRIES`.
    pub(crate) fn from_env() -> Self {
        Self::from_values(
            std::env::var("PROFILE_SWITCH_DELAY_MS").ok().as_deref(),
            std::env::var("PROFILE_SWITCH_MAX_RETRIES").ok().as_deref(),
        )
    }

    /// Parse the two env values. Invalid or absent values use the defaults; the
    /// interval is clamped to 10–1000 ms and the total wait to 5 s.
    fn from_values(delay_ms: Option<&str>, max_retries: Option<&str>) -> Self {
        let interval = delay_ms
            .and_then(|v| v.trim().parse::<u64>().ok())
            .unwrap_or(DEFAULT_POLL_INTERVAL_MS)
            .clamp(MIN_POLL_INTERVAL_MS, MAX_POLL_INTERVAL_MS);
        let polls = max_retries
            .and_then(|v| v.trim().parse::<u64>().ok())
            .unwrap_or(DEFAULT_MAX_POLLS)
            .max(1);
        let total = interval.saturating_mul(polls).min(MAX_TOTAL_WAIT_MS);
        Self {
            poll_interval: Duration::from_millis(interval),
            timeout: Duration::from_millis(total),
        }
    }
}

/// What an activation did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Activation {
    /// The node that is now the configured default (its actual name).
    pub(crate) node_name: String,
    /// The profile switch performed first, if any.
    pub(crate) profile_switch: Option<ProfileSwitch>,
}

/// Which candidate an activation chose, and what it did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Chosen {
    /// Index of the activated candidate (0 = the first choice).
    pub(crate) index: usize,
    pub(crate) activation: Activation,
}

/// One half of a job: configured node names in priority order, and with
/// `upgrade_from: Some(k)` the note that candidate `k` is the default already (only a
/// reachable earlier candidate is worth a switch)
#[derive(Debug, Clone, Copy)]
pub(crate) struct Request<'a> {
    pub(crate) candidates: &'a [String],
    pub(crate) upgrade_from: Option<usize>,
    /// A source half that only rides along with the sink half, so a profile switch for
    /// the sink keeps the microphone: it is acted on (planned jointly, as any source
    /// half) only when the sink's planned switch is on the card of one of its
    /// candidates, and left alone (`Ok(None)`) otherwise
    pub(crate) keep: bool,
    /// A profile switch whose node does not appear in time is switched back to the
    /// card's previous profile, because no further attempt follows (a manual switch,
    /// the last retry of a rule switch, an upgrade check). Without it the card is
    /// left on the new profile, so a slow card can finish while the retry that
    /// follows waits, and that retry finds the node; the error is then a
    /// [`CardLeft`].
    pub(crate) switch_back: bool,
}

/// Why an upgrade check left its half alone although the configured default is not
/// the fallback it was told is applied (it was changed outside pwsw)
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Elsewhere {
    /// The default is something else: as the user sees it now (its node name)
    Other(String),
    /// The default is this higher-priority candidate (an index before the applied
    /// fallback's): chosen outside pwsw, it serves the list as well
    Candidate(usize),
}

/// The error of a half whose failed activation left a card on a profile it switched
/// to (the node did not appear in time and the card was not switched back): the
/// default before the job may not play any more
#[derive(Debug)]
pub(crate) struct CardLeft(String);

impl std::fmt::Display for CardLeft {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for CardLeft {}

#[cfg(test)]
impl CardLeft {
    /// For fakes of the activation in other modules' tests
    pub(crate) fn new(message: &str) -> Self {
        Self(message.to_string())
    }
}

/// Cards an attempt left on the profile it switched to (the node had not appeared;
/// see [`Request::switch_back`]), each with that profile and the one the card had
/// before the first such attempt
///
/// Kept across jobs (the caller holds it), so the attempts that follow do not switch
/// the card again but wait for the node once more, and the last one switches it back
/// to the profile from before the first attempt, not to the one it was left on. An
/// entry is dropped once a node on that card is activated, the card is switched back,
/// or its profile is no longer the one it was left on.
#[derive(Debug, Default)]
pub(crate) struct LeftCards(Vec<LeftCard>);

#[derive(Debug, Clone)]
struct LeftCard {
    device_id: u32,
    /// The profile the card was left on
    on: u32,
    /// The profile it had before
    before: PreviousProfile,
}

impl LeftCards {
    /// No cards left switched
    pub(crate) const fn new() -> Self {
        Self(Vec::new())
    }

    /// The entry of `device_id`, if the card is still on the profile it was left on
    /// (`current`: its profile now)
    fn get(&self, device_id: u32, current: Option<u32>) -> Option<&LeftCard> {
        self.0
            .iter()
            .find(|c| c.device_id == device_id && Some(c.on) == current)
    }

    fn remove(&mut self, device_id: u32) {
        self.0.retain(|c| c.device_id != device_id);
    }

    /// Forget cards whose profile changed since (switched by someone else)
    fn prune(&mut self, snapshot: &Snapshot) {
        self.0.retain(|c| {
            snapshot
                .devices
                .iter()
                .any(|d| d.id == c.device_id && d.active_profile == Some(c.on))
        });
    }
}

/// What a job did for each half it was given (`None`: the half was not asked for)
///
/// Each result is the chosen candidate, `Ok(None)` when nothing was touched (an
/// upgrade check found nothing better or was skipped, a `keep` half was not
/// affected), or the error.
#[derive(Debug, Default)]
pub(crate) struct JointOutcome {
    pub(crate) sink: Option<Result<Option<Chosen>>>,
    pub(crate) source: Option<Result<Option<Chosen>>>,
    /// Per [`Endpoint::index`]: an upgrade check that was skipped because the
    /// configured default is not the candidate it was told is applied (it was changed
    /// outside pwsw), and what the default is now
    pub(crate) elsewhere: [Option<Elsewhere>; 2],
}

/// Cards a job switched: every switch it attempted, and those of them it switched
/// back after the node did not appear (or whose switch changed nothing); and the
/// cards left switched by this and earlier jobs
#[derive(Debug, Default)]
struct Cards {
    switched: HashSet<u32>,
    restored: HashSet<u32>,
    left: LeftCards,
}

/// Make `target` (a configured node name) the default sink.
///
/// The snapshot and the decision are taken here, so a caller that serialises calls
/// (one activation at a time) never acts on a stale view.
///
/// # Errors
/// Returns an error if the snapshot fails, the target cannot be resolved, the profile
/// switch fails, the sink node does not appear within `policy.timeout`, or setting
/// the default fails.
#[cfg(test)]
pub(crate) fn activate(
    backend: &dyn AudioBackend,
    clock: &dyn Clock,
    policy: WaitPolicy,
    target: &str,
) -> Result<Activation> {
    let chosen = activate_first(backend, clock, policy, &[target.to_string()], None)?;
    chosen
        .map(|c| c.activation)
        .ok_or_else(|| anyhow::anyhow!("Internal error: nothing activated"))
}

/// Make the first reachable of `candidates` (configured sink node names, in priority
/// order) the default sink: [`activate_joint`] with only a sink half.
///
/// # Errors
/// Returns an error if `candidates` is empty, a snapshot fails, or no candidate could
/// be activated (for a single candidate, its own error; otherwise one naming every
/// candidate and why it failed).
#[cfg(test)]
pub(crate) fn activate_first(
    backend: &dyn AudioBackend,
    clock: &dyn Clock,
    policy: WaitPolicy,
    candidates: &[String],
    upgrade_from: Option<usize>,
) -> Result<Option<Chosen>> {
    let request = Request {
        candidates,
        upgrade_from,
        keep: false,
        switch_back: true,
    };
    activate_joint(
        backend,
        clock,
        policy,
        Some(request),
        None,
        &mut LeftCards::new(),
    )
    .sink
    .unwrap_or_else(|| Err(anyhow::anyhow!("Internal error: no sink result")))
}

/// Activate a job's sink half and source half against one fresh snapshot.
///
/// Each half's candidates are resolved in order and the first reachable one is
/// activated ([`resolve::plan_joint`] decides how the two fit together: one profile
/// switch when they share a card, the sink winning when no profile serves both). The
/// sink half runs first. If an activation fails (e.g. the profile switch did not
/// produce the node), the remaining candidates of that half are resolved against a
/// new snapshot (the failed switch may have changed the graph) and the next reachable
/// one is tried; a source half that can no longer follow the plan is planned again
/// the same way, never switching a card this job already switched.
///
/// With `upgrade_from: Some(k)` a half does nothing (`Ok(None)`) unless a candidate
/// before `k` is reachable now; if one is, it proceeds as above over its whole list.
/// It does nothing either when the configured default is not candidate `k` (someone
/// chose another one since), and reports what is the default instead
/// ([`JointOutcome::elsewhere`]: one of the candidates before `k`, or something else).
///
/// A profile switch whose node does not appear in time is switched back with
/// [`Request::switch_back`], and left for the retry that follows without it
/// (recorded in `left`, which the caller keeps from one job to the next).
///
/// A source half with `keep` does nothing (`Ok(None)`) unless the sink's planned
/// profile switch is on the card of one of its candidates; then it is planned with the
/// sink like any source half (one profile for both, or the source reported as
/// unreachable when none provides both: the sink's switch drops it). If the sink's
/// switch was undone (its node did not appear and the card was switched back), or no
/// snapshot could be taken at all, the `keep` half is `Ok(None)` as well: the card is
/// as it was.
pub(crate) fn activate_joint(
    backend: &dyn AudioBackend,
    clock: &dyn Clock,
    policy: WaitPolicy,
    sink: Option<Request<'_>>,
    source: Option<Request<'_>>,
    left: &mut LeftCards,
) -> JointOutcome {
    let mut outcome = JointOutcome::default();
    let mut halves = [sink, source];
    for (kind, half) in Endpoint::ALL.into_iter().zip(&mut halves) {
        if half.is_some_and(|r| r.candidates.is_empty()) {
            *half = None;
            *outcome.slot(kind) = Some(Err(anyhow::anyhow!("No {} to activate", kind.noun())));
        }
    }
    if halves.iter().all(Option::is_none) {
        return outcome;
    }

    let snapshot = match backend.snapshot() {
        Ok(snapshot) => snapshot,
        Err(e) => {
            let message = format!("{e:#}");
            for (kind, half) in Endpoint::ALL.into_iter().zip(&halves) {
                if let Some(request) = half {
                    // Nothing was switched, so a microphone riding along is untouched
                    *outcome.slot(kind) = Some(if request.keep {
                        Ok(None)
                    } else {
                        Err(anyhow::anyhow!("{message}"))
                    });
                }
            }
            return outcome;
        }
    };

    // An upgrade check leaves its half alone when the default is no longer the
    // fallback it would improve on (chosen outside pwsw since), or nothing better is
    // reachable
    for (kind, half) in Endpoint::ALL.into_iter().zip(&mut halves) {
        let Some(Request {
            candidates,
            upgrade_from: Some(current),
            ..
        }) = *half
        else {
            continue;
        };
        if let Some(now) = default_elsewhere(&snapshot, kind, candidates.get(current)) {
            debug!(
                "Not upgrading the {}: the default is {}, not the fallback pwsw set",
                kind.noun(),
                for_log(&now)
            );
            // A higher-priority candidate chosen outside pwsw serves the list too
            let configured = snapshot.defaults(kind).configured.as_deref();
            let candidate = configured.and_then(|configured| {
                candidates[..current.min(candidates.len())]
                    .iter()
                    .position(|c| is_default(&snapshot, kind, c, configured))
            });
            outcome.elsewhere[kind.index()] =
                Some(candidate.map_or(Elsewhere::Other(now), Elsewhere::Candidate));
        } else if candidates[..current.min(candidates.len())]
            .iter()
            .any(|c| resolve::resolve_endpoint(&snapshot, kind, c).is_ok())
        {
            continue;
        }
        *half = None;
        *outcome.slot(kind) = Some(Ok(None));
    }
    let [sink, mut source] = halves;
    let sinks = sink.map_or(&[][..], |r| r.candidates);
    let sources = source.map_or(&[][..], |r| r.candidates);
    let plan = resolve::plan_joint(&snapshot, sinks, sources, None, &HashSet::new());
    // A source riding along matters only if the sink's switch is on its card
    if source.is_some_and(|r| r.keep) && !resolve::sink_switch_touches(&snapshot, &plan, sources) {
        debug!("The sink's activation leaves the microphone's card alone");
        source = None;
        outcome.source = Some(Ok(None));
    }

    // Cards switched in this job (attempted or done), those switched back, and the
    // cards left switched (still on the profile they were left on)
    left.prune(&snapshot);
    let mut cards = Cards {
        left: std::mem::take(left),
        ..Cards::default()
    };
    let mut sink_as_planned = true;
    let mut sink_side = None;
    if let Some(request) = sink {
        let (result, as_planned) = run_sink(
            backend,
            clock,
            policy,
            request,
            plan.sink.clone(),
            plan.sink_skipped.clone(),
            &mut cards,
        );
        sink_as_planned = as_planned;
        if let Ok(Some(chosen)) = &result {
            sink_side = activated_side(&snapshot, &chosen.activation);
            if let Some(side) = &sink_side {
                // The card serves a node now: nothing to go back to
                cards.left.remove(side.device_id);
            }
        }
        outcome.sink = Some(result);
    }

    // A riding source whose card the sink's activation ended up not changing (its
    // switch failed and was undone, or it went to a candidate elsewhere) is as it was
    if source.is_some_and(|r| r.keep)
        && !sink_as_planned
        && !keep_affected(&snapshot, sources, sink_side.as_ref(), &cards)
    {
        debug!("The sink's activation left the microphone's card as it was");
        source = None;
        outcome.source = Some(Ok(None));
    }

    if let Some(request) = source {
        let planned = if sink_as_planned {
            Ok((plan.source, plan.source_skipped))
        } else {
            // The sink went another way than planned: plan the source against what
            // the graph looks like now
            backend
                .snapshot()
                .map(|now| {
                    if let Some(side) = &sink_side {
                        debug!(
                            "Planning the source again (sink on device {})",
                            side.device_id
                        );
                    }
                    let again = resolve::plan_joint(
                        &now,
                        &[],
                        sources,
                        sink_side.as_ref(),
                        &cards.switched,
                    );
                    (again.source, again.source_skipped)
                })
                .map_err(|e| anyhow::anyhow!("after the sink's activation: {e:#}"))
        };
        let result = match planned {
            Ok(planned) => run_source(
                backend,
                clock,
                policy,
                request,
                planned,
                sink_side.as_ref(),
                &mut cards,
            ),
            Err(e) => Err(e),
        };
        if let Ok(Some(chosen)) = &result {
            let device = match &chosen.activation.profile_switch {
                Some(switch) => Some(switch.device_id),
                None => snapshot
                    .node_by_name(Endpoint::Source, &chosen.activation.node_name)
                    .and_then(|node| node.device_id),
            };
            if let Some(device) = device {
                cards.left.remove(device);
            }
        }
        outcome.source = Some(result);
    }
    *left = cards.left;
    outcome
}

/// Whether a riding source may have been changed by the sink's activation: its final
/// profile switch is on one of the sources' cards, or a switch of such a card was
/// not undone
fn keep_affected(
    snapshot: &Snapshot,
    sources: &[String],
    sink_side: Option<&SinkSide>,
    cards: &Cards,
) -> bool {
    let switched_last = sink_side.is_some_and(|side| {
        cards.switched.contains(&side.device_id)
            && resolve::on_source_card(snapshot, sources, side.device_id)
    });
    switched_last
        || cards
            .switched
            .difference(&cards.restored)
            .any(|&device| resolve::on_source_card(snapshot, sources, device))
}

/// The default of `kind` as the user sees it, when the configured default is not
/// `applied` (a configured node name, by name or as the node it resolves to): it was
/// changed outside pwsw. `None` when it is `applied`, or nothing is configured.
fn default_elsewhere(
    snapshot: &Snapshot,
    kind: Endpoint,
    applied: Option<&String>,
) -> Option<String> {
    let configured = snapshot.defaults(kind).configured.as_deref()?;
    if is_default(snapshot, kind, applied?, configured) {
        return None;
    }
    Some(
        snapshot
            .current_default_of(kind)
            .unwrap_or(configured)
            .to_string(),
    )
}

/// Whether the configured node name `name` is the configured default `configured`:
/// by name, or as the node it resolves to (a node renamed by `WirePlumber`,
/// recognised by device and profile)
fn is_default(snapshot: &Snapshot, kind: Endpoint, name: &str, configured: &str) -> bool {
    name == configured
        || matches!(
            resolve::resolve_endpoint(snapshot, kind, name),
            Ok(Plan::SetDefault { node_name }) if node_name == configured
        )
}

impl JointOutcome {
    const fn slot(&mut self, kind: Endpoint) -> &mut Option<Result<Option<Chosen>>> {
        match kind {
            Endpoint::Sink => &mut self.sink,
            Endpoint::Source => &mut self.source,
        }
    }
}

/// Where an activated sink lives (for planning the source around it)
fn activated_side(snapshot: &Snapshot, activation: &Activation) -> Option<SinkSide> {
    if let Some(sw) = &activation.profile_switch {
        return Some(SinkSide {
            device_id: sw.device_id,
            mapping: sw.mapping.clone(),
        });
    }
    let node = snapshot.node_by_name(Endpoint::Sink, &activation.node_name)?;
    Some(SinkSide {
        device_id: node.device_id?,
        mapping: node.profile_name.clone()?,
    })
}

/// The error for a half none of whose candidates could be activated (one line: it
/// goes to the log, notifications and `IPC` replies)
fn none_activated(failures: &[(String, String)]) -> anyhow::Error {
    if let [(_, only)] = failures {
        return anyhow::anyhow!("{only}");
    }
    let mut message = String::from("every candidate failed:");
    for (i, (target, e)) in failures.iter().enumerate() {
        let sep = if i == 0 { " " } else { "; " };
        let _ = write!(message, "{sep}'{target}': {e}");
    }
    anyhow::anyhow!(message)
}

/// The first candidate from `start` on that resolves against `snapshot`, recording
/// why the others were skipped
fn first_reachable(
    snapshot: &Snapshot,
    kind: Endpoint,
    candidates: &[String],
    start: usize,
    failures: &mut Vec<(String, String)>,
) -> Option<(usize, Plan)> {
    for (index, target) in candidates.iter().enumerate().skip(start) {
        match resolve::resolve_endpoint(snapshot, kind, target) {
            Ok(plan) => return Some((index, plan)),
            Err(e) => {
                debug!("Skipping '{target}': {e:#}");
                failures.push((target.clone(), format!("{e:#}")));
            }
        }
    }
    None
}

/// A half's error, as a [`CardLeft`] when one of its attempts left a card on the
/// profile it switched to (`left`)
fn card_left_if(error: anyhow::Error, left: bool) -> anyhow::Error {
    if left {
        anyhow::Error::new(CardLeft(one_line(&format!("{error:#}"))))
    } else {
        error
    }
}

/// Run the sink half from its planned candidate; returns the result and whether it
/// went as planned (the planned candidate activated)
fn run_sink(
    backend: &dyn AudioBackend,
    clock: &dyn Clock,
    policy: WaitPolicy,
    request: Request<'_>,
    planned: Option<(usize, Plan)>,
    mut failures: Vec<(String, String)>,
    cards: &mut Cards,
) -> (Result<Option<Chosen>>, bool) {
    let candidates = request.candidates;
    let mut next = planned;
    let mut as_planned = true;
    // An attempt left a card on the profile it switched to
    let mut left = false;
    loop {
        let Some((index, plan)) = next.take() else {
            return (Err(card_left_if(none_activated(&failures), left)), false);
        };
        let target = &candidates[index];
        if let Plan::SwitchProfile(sw) = &plan {
            cards.switched.insert(sw.device_id);
        }
        let attempt = Attempt {
            kind: Endpoint::Sink,
            target,
            switch_back: request.switch_back,
        };
        match execute(backend, clock, policy, plan, attempt, cards) {
            Ok(activation) => return (Ok(Some(Chosen { index, activation })), as_planned),
            Err(e) => {
                as_planned = false;
                left |= e.downcast_ref::<CardLeft>().is_some();
                let text = one_line(&format!("{e:#}"));
                let more = index + 1 < candidates.len();
                if more {
                    warn!("Activating '{target}' failed, trying the next sink: {text}");
                }
                failures.push((target.clone(), text));
                if !more {
                    continue;
                }
                match backend.snapshot() {
                    Ok(snapshot) => {
                        next = first_reachable(
                            &snapshot,
                            Endpoint::Sink,
                            candidates,
                            index + 1,
                            &mut failures,
                        );
                    }
                    Err(e) => {
                        let error = anyhow::anyhow!(
                            "after activating '{target}' failed: {}",
                            one_line(&format!("{e:#}"))
                        );
                        return (Err(card_left_if(error, left)), false);
                    }
                }
            }
        }
    }
}

/// The source half's first candidate to try (index and how), and the candidates
/// skipped before it with why
type PlannedSource = (Option<(usize, SourceStep)>, Vec<(String, String)>);

/// Run the source half from its planned candidate (see [`activate_joint`])
fn run_source(
    backend: &dyn AudioBackend,
    clock: &dyn Clock,
    policy: WaitPolicy,
    request: Request<'_>,
    (planned, mut failures): PlannedSource,
    sink_side: Option<&SinkSide>,
    cards: &mut Cards,
) -> Result<Option<Chosen>> {
    let candidates = request.candidates;
    let mut next = planned;
    let mut left = false;
    loop {
        let Some((index, step)) = next.take() else {
            return Err(card_left_if(none_activated(&failures), left));
        };
        let target = &candidates[index];
        let result = match step {
            SourceStep::WithSinkSwitch(switch) => {
                // The sink's half switched the card already (and succeeded: the
                // switch is not undone for the source)
                wait_for_node(backend, clock, policy, &switch)
                    .map_err(|timeout| {
                        anyhow::anyhow!("{}", timeout.message(&switch, target, policy, None))
                    })
                    .and_then(|node_name| {
                        backend.set_default(Endpoint::Source, &node_name)?;
                        Ok(Activation {
                            node_name,
                            profile_switch: Some(switch),
                        })
                    })
            }
            SourceStep::Plan(plan) => {
                if let Plan::SwitchProfile(sw) = &plan {
                    cards.switched.insert(sw.device_id);
                }
                let attempt = Attempt {
                    kind: Endpoint::Source,
                    target,
                    switch_back: request.switch_back,
                };
                execute(backend, clock, policy, plan, attempt, cards)
            }
        };
        match result {
            Ok(activation) => return Ok(Some(Chosen { index, activation })),
            Err(e) => {
                left |= e.downcast_ref::<CardLeft>().is_some();
                let text = one_line(&format!("{e:#}"));
                let rest = index + 1;
                if rest < candidates.len() {
                    warn!("Activating '{target}' failed, trying the next source: {text}");
                }
                failures.push((target.clone(), text));
                if rest >= candidates.len() {
                    continue;
                }
                let snapshot = backend.snapshot().map_err(|e| {
                    let error = anyhow::anyhow!(
                        "after activating '{target}' failed: {}",
                        one_line(&format!("{e:#}"))
                    );
                    card_left_if(error, left)
                })?;
                let again = resolve::plan_joint(
                    &snapshot,
                    &[],
                    &candidates[rest..],
                    sink_side,
                    &cards.switched,
                );
                failures.extend(again.source_skipped);
                next = again.source.map(|(i, step)| (rest + i, step));
            }
        }
    }
}

/// One activation attempt: the configured node name it is for, and whether a timed-out
/// profile switch for it is switched back ([`Request::switch_back`])
#[derive(Debug, Clone, Copy)]
struct Attempt<'a> {
    kind: Endpoint,
    target: &'a str,
    switch_back: bool,
}

/// Carry out a resolved plan for an attempt (a sink or a source).
///
/// A profile switch whose node does not appear in time is undone with
/// `switch_back` (the card is switched back to its previous profile, recorded in
/// `cards`), so the last failed attempt leaves the audio where it was; otherwise,
/// or when switching back is not possible, the card is left on the new profile and
/// the error is a [`CardLeft`] that says so.
fn execute(
    backend: &dyn AudioBackend,
    clock: &dyn Clock,
    policy: WaitPolicy,
    plan: Plan,
    attempt: Attempt<'_>,
    cards: &mut Cards,
) -> Result<Activation> {
    let Attempt { kind, target, .. } = attempt;
    let switch = match plan {
        Plan::SetDefault { node_name } => {
            backend.set_default(kind, &node_name)?;
            return Ok(Activation {
                node_name,
                profile_switch: None,
            });
        }
        Plan::SwitchProfile(switch) => switch,
    };

    // A card an earlier attempt left on this very profile is not switched again: its
    // node may still appear, and switching back goes to the profile from before.
    // Switched to another profile, it is done with.
    let current = switch.previous.as_ref().map(|p| p.index);
    let earlier = cards
        .left
        .get(switch.device_id, current)
        .filter(|_| current == Some(switch.profile_index))
        .cloned();
    let mut switch = switch;
    if let Some(earlier) = &earlier {
        switch.previous = Some(earlier.before.clone());
    } else {
        cards.left.remove(switch.device_id);
    }
    if earlier.is_some() {
        debug!(
            "{} is still on profile {} from an earlier attempt; waiting for its {} again",
            for_log(&switch.device_name),
            for_log(&switch.profile_name),
            switch.kind.noun()
        );
    } else {
        debug!(
            "Switching {} to profile {} ({}) for '{}'",
            for_log(&switch.device_name),
            switch.profile_index,
            for_log(&switch.profile_name),
            target
        );
        backend.set_profile(switch.device_id, switch.profile_index)?;
    }

    let node_name = match wait_for_node(backend, clock, policy, &switch) {
        Ok(node_name) => node_name,
        Err(timeout) => {
            let (undone, left) = after_timeout(backend, &switch, attempt, policy, cards);
            let message = timeout.message(&switch, target, policy, Some(&undone));
            if left {
                return Err(anyhow::Error::new(CardLeft(message)));
            }
            anyhow::bail!("{message}");
        }
    };
    cards.left.remove(switch.device_id);
    if node_name != target {
        info!(
            "{} '{}' appeared as '{}'",
            kind.title(),
            target,
            for_log(&node_name)
        );
    }
    backend.set_default(kind, &node_name)?;
    Ok(Activation {
        node_name,
        profile_switch: Some(switch),
    })
}

/// What to do about the card of a timed-out `switch`: switch it back to its previous
/// profile when the attempt says so (best effort), else leave it for the retry
///
/// Returns what happened, for the error message, and whether the card was left on
/// the new profile.
fn after_timeout(
    backend: &dyn AudioBackend,
    switch: &ProfileSwitch,
    attempt: Attempt<'_>,
    policy: WaitPolicy,
    cards: &mut Cards,
) -> (String, bool) {
    let new = for_log(&switch.profile_name);
    let Some(previous) = &switch.previous else {
        return (
            format!("its previous profile is unknown, so the card stays on '{new}'"),
            true,
        );
    };
    if previous.index == switch.profile_index {
        // Nothing changed on the card: it is as it was
        cards.restored.insert(switch.device_id);
        return (
            "the card was already on that profile before, so there was nothing to switch back"
                .to_string(),
            false,
        );
    }
    let old = for_log(&previous.name);
    // Whatever happens now, an older entry for the card is replaced or done with
    cards.left.remove(switch.device_id);
    if !attempt.switch_back {
        cards.left.0.push(LeftCard {
            device_id: switch.device_id,
            on: switch.profile_index,
            before: previous.clone(),
        });
        debug!(
            "Leaving {} on profile {new} for the next attempt: the {} for '{}' did not appear \
             within {} ms",
            for_log(&switch.device_name),
            switch.kind.noun(),
            attempt.target,
            policy.timeout.as_millis()
        );
        return (
            format!(
                "left the card on that profile, since pwsw tries again (the {} may still \
                 appear)",
                switch.kind.noun()
            ),
            true,
        );
    }
    if !previous.available {
        return (
            format!("its previous profile '{old}' is not available, so the card stays on '{new}'"),
            true,
        );
    }
    match backend.set_profile(switch.device_id, previous.index) {
        Ok(()) => {
            cards.restored.insert(switch.device_id);
            debug!(
                "Switched {} back to profile {old}: the {} for '{}' did not appear within {} ms",
                for_log(&switch.device_name),
                switch.kind.noun(),
                attempt.target,
                policy.timeout.as_millis()
            );
            (
                format!("switched it back to its previous profile '{old}'"),
                false,
            )
        }
        Err(e) => (
            format!(
                "switching it back to profile '{old}' failed ({}), so the card stays on '{new}'",
                one_line(&format!("{e:#}"))
            ),
            true,
        ),
    }
}

/// The node of a profile switch did not appear in time
#[derive(Debug)]
struct WaitTimeout {
    /// The last `pw-dump` error while waiting, if any (folded into one line)
    last_error: Option<String>,
}

impl WaitTimeout {
    /// The error message (one line), with what was done about the switch (`undone`)
    fn message(
        &self,
        switch: &ProfileSwitch,
        target: &str,
        policy: WaitPolicy,
        undone: Option<&str>,
    ) -> String {
        let undone = undone.map_or_else(String::new, |u| format!("; {u}"));
        let cause = self
            .last_error
            .as_ref()
            .map_or_else(String::new, |e| format!(" (last pw-dump error: {e})"));
        format!(
            "Switched {} to profile '{}' but its {} for '{}' did not appear within {} ms{undone}. \
             The device may need more time (raise PROFILE_SWITCH_DELAY_MS / \
             PROFILE_SWITCH_MAX_RETRIES; the wait is capped at {} ms), or the port may have no \
             hardware behind it (check `pwsw list-sinks`){cause}",
            for_log(&switch.device_name),
            for_log(&switch.profile_name),
            switch.kind.noun(),
            target,
            policy.timeout.as_millis(),
            MAX_TOTAL_WAIT_MS,
        )
    }
}

/// Poll snapshots until the switch's node appears or the deadline passes.
fn wait_for_node(
    backend: &dyn AudioBackend,
    clock: &dyn Clock,
    policy: WaitPolicy,
    switch: &ProfileSwitch,
) -> Result<String, WaitTimeout> {
    let start = clock.now();
    let mut polls = 0u32;
    let mut last_error = None;
    loop {
        let elapsed = clock.now().saturating_duration_since(start);
        if elapsed >= policy.timeout {
            break;
        }
        clock.sleep(
            policy
                .poll_interval
                .min(policy.timeout.saturating_sub(elapsed)),
        );
        polls += 1;
        match backend.snapshot() {
            Ok(snapshot) => {
                if let Some(node) = snapshot
                    .nodes(switch.kind)
                    .iter()
                    .find(|n| switch.produced(n))
                {
                    return Ok(node.name.clone());
                }
                debug!(
                    "Waiting for the {} of {} (poll {polls})",
                    switch.kind.noun(),
                    for_log(&switch.mapping)
                );
            }
            Err(e) => {
                let error = one_line(&format!("{e:#}"));
                debug!(
                    "pw-dump failed while waiting for {}: {error}",
                    for_log(&switch.mapping)
                );
                last_error = Some(error);
            }
        }
    }
    Err(WaitTimeout { last_error })
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};

    use pretty_assertions::assert_eq;
    use test_case::test_case;

    use super::*;
    use crate::pipewire::model::Node;
    use crate::pipewire::parse::parse_dump;

    const REAL: &str = include_str!("../../tests/fixtures/pwdump-alsa-profiles.json");

    #[derive(Debug, Clone, PartialEq, Eq)]
    enum Call {
        Snapshot,
        SetDefault(String),
        SetSource(String),
        SetProfile(u32, u32),
    }

    /// In-memory backend: a profile switch removes the device's sinks and sources and
    /// makes the new profile's sink and source appear after `appear_after` further
    /// snapshots.
    struct FakeBackend {
        state: RefCell<Snapshot>,
        appear_after: Option<usize>,
        rename: Option<&'static str>,
        /// A switch to this output mapping never produces its sink
        never_appears: Option<&'static str>,
        /// A switch to this input mapping never produces its source
        never_appears_source: Option<&'static str>,
        pending: RefCell<Vec<(Endpoint, Node, usize)>>,
        calls: RefCell<Vec<Call>>,
        /// Fail this many snapshots from now on (like a `pw-dump` that timed out)
        failing_snapshots: Cell<usize>,
        /// The error of a failing snapshot
        snapshot_error: &'static str,
        /// Switching a device to this profile index fails
        failing_profile: Option<u32>,
        /// The error of a failing profile switch
        profile_error: &'static str,
    }

    impl FakeBackend {
        fn new(appear_after: Option<usize>) -> Self {
            Self::with_fixture(REAL, appear_after)
        }

        fn with_fixture(json: &str, appear_after: Option<usize>) -> Self {
            Self {
                state: RefCell::new(parse_dump(json.as_bytes()).expect("fixture parses")),
                appear_after,
                rename: None,
                never_appears: None,
                never_appears_source: None,
                pending: RefCell::new(Vec::new()),
                calls: RefCell::new(Vec::new()),
                failing_snapshots: Cell::new(0),
                snapshot_error: "`pw-dump` did not finish within 5000 ms and was killed",
                failing_profile: None,
                profile_error: "pw-cli: set-param failed",
            }
        }

        /// Set the configured default, as `pavucontrol` would
        fn set_configured(&self, kind: Endpoint, name: &str) {
            let mut state = self.state.borrow_mut();
            let defaults = match kind {
                Endpoint::Sink => &mut state.defaults,
                Endpoint::Source => &mut state.source_defaults,
            };
            defaults.configured = Some(name.to_string());
        }

        fn calls(&self) -> Vec<Call> {
            self.calls.borrow().clone()
        }

        fn switches(&self) -> Vec<Call> {
            self.calls()
                .into_iter()
                .filter(|c| matches!(c, Call::SetProfile(..)))
                .collect()
        }
    }

    impl AudioBackend for FakeBackend {
        fn snapshot(&self) -> Result<Snapshot> {
            self.calls.borrow_mut().push(Call::Snapshot);
            if self.failing_snapshots.get() > 0 {
                self.failing_snapshots.set(self.failing_snapshots.get() - 1);
                anyhow::bail!("{}", self.snapshot_error);
            }
            let mut pending = self.pending.borrow_mut();
            let mut state = self.state.borrow_mut();
            pending.retain_mut(|(kind, node, left)| {
                if *left > 0 {
                    *left -= 1;
                    return true;
                }
                match kind {
                    Endpoint::Sink => state.sinks.push(node.clone()),
                    Endpoint::Source => state.sources.push(node.clone()),
                }
                false
            });
            Ok(state.clone())
        }

        fn set_default(&self, kind: Endpoint, node_name: &str) -> Result<()> {
            let mut state = self.state.borrow_mut();
            let (call, defaults) = match kind {
                Endpoint::Sink => (Call::SetDefault(node_name.to_string()), &mut state.defaults),
                Endpoint::Source => (
                    Call::SetSource(node_name.to_string()),
                    &mut state.source_defaults,
                ),
            };
            self.calls.borrow_mut().push(call);
            defaults.configured = Some(node_name.to_string());
            Ok(())
        }

        fn set_profile(&self, device_id: u32, profile_index: u32) -> Result<()> {
            self.calls
                .borrow_mut()
                .push(Call::SetProfile(device_id, profile_index));
            if self.failing_profile == Some(profile_index) {
                anyhow::bail!("{}", self.profile_error);
            }
            let mut state = self.state.borrow_mut();
            state.sinks.retain(|s| s.device_id != Some(device_id));
            state.sources.retain(|s| s.device_id != Some(device_id));
            let device = state
                .devices
                .iter_mut()
                .find(|d| d.id == device_id)
                .expect("device exists");
            device.active_profile = Some(profile_index);
            let profile = device
                .profiles
                .iter()
                .find(|p| p.index == profile_index)
                .expect("profile exists");
            let suffix = device.name.strip_prefix("alsa_card.").expect("alsa card");
            let mut pending = self.pending.borrow_mut();
            pending.clear();
            let Some(n) = self.appear_after else {
                return Ok(());
            };
            if let Some(mapping) = profile.output_mappings().next()
                && self.never_appears != Some(mapping)
            {
                let node = Node {
                    id: 1000,
                    name: self
                        .rename
                        .map_or_else(|| format!("alsa_output.{suffix}.{mapping}"), String::from),
                    description: String::new(),
                    device_id: Some(device_id),
                    profile_name: Some(mapping.to_string()),
                    card_profile_device: profile.sink_card_devices.first().copied(),
                };
                pending.push((Endpoint::Sink, node, n));
            }
            if let Some(mapping) = profile.input_mapping()
                && self.never_appears_source != Some(mapping)
            {
                let node = Node {
                    id: 1001,
                    name: format!("alsa_input.{suffix}.{mapping}"),
                    description: String::new(),
                    device_id: Some(device_id),
                    profile_name: Some(mapping.to_string()),
                    card_profile_device: profile.source_card_devices.first().copied(),
                };
                pending.push((Endpoint::Source, node, n));
            }
            Ok(())
        }
    }

    /// Virtual clock: `sleep` advances time instantly.
    struct FakeClock {
        base: Instant,
        offset: Cell<Duration>,
    }

    impl FakeClock {
        fn new() -> Self {
            Self {
                base: Instant::now(),
                offset: Cell::new(Duration::ZERO),
            }
        }
    }

    impl Clock for FakeClock {
        fn now(&self) -> Instant {
            self.base + self.offset.get()
        }

        fn sleep(&self, duration: Duration) {
            self.offset.set(self.offset.get() + duration);
        }
    }

    /// [`activate_joint`] with nothing left switched by earlier jobs
    fn joint_fresh(
        backend: &dyn AudioBackend,
        clock: &dyn Clock,
        policy: WaitPolicy,
        sink: Option<Request<'_>>,
        source: Option<Request<'_>>,
    ) -> JointOutcome {
        activate_joint(backend, clock, policy, sink, source, &mut LeftCards::new())
    }

    const POLICY: WaitPolicy = WaitPolicy {
        poll_interval: Duration::from_millis(100),
        timeout: Duration::from_millis(1_000),
    };
    const ANALOG: &str = "alsa_output.pci-0000_0c_00.4.analog-stereo";

    #[test]
    fn test_activate_active_sink_only_sets_default() {
        let backend = FakeBackend::new(Some(0));
        let name = "alsa_output.pci-0000_0c_00.4.iec958-stereo";
        let act = activate(&backend, &FakeClock::new(), POLICY, name).expect("activates");
        assert_eq!(act.node_name, name);
        assert_eq!(act.profile_switch, None);
        assert_eq!(backend.calls(), vec![
            Call::Snapshot,
            Call::SetDefault(name.to_string())
        ]);
    }

    #[test]
    fn test_activate_switches_profile_then_sets_default() {
        let backend = FakeBackend::new(Some(2));
        let clock = FakeClock::new();
        let act = activate(&backend, &clock, POLICY, ANALOG).expect("activates");
        assert_eq!(act.node_name, ANALOG);
        assert_eq!(
            act.profile_switch.map(|s| s.profile_index),
            Some(1),
            "duplex profile keeps the microphone"
        );
        assert_eq!(backend.calls(), vec![
            Call::Snapshot,
            Call::SetProfile(43, 1),
            Call::Snapshot,
            Call::Snapshot,
            Call::Snapshot,
            Call::SetDefault(ANALOG.to_string()),
        ]);
        assert_eq!(clock.offset.get(), Duration::from_millis(300));
    }

    #[test]
    fn test_activate_uses_appearing_nodes_real_name() {
        let mut backend = FakeBackend::new(Some(0));
        backend.rename = Some("renamed.by.wireplumber");
        let act = activate(&backend, &FakeClock::new(), POLICY, ANALOG).expect("activates");
        assert_eq!(act.node_name, "renamed.by.wireplumber");
        assert_eq!(
            backend.calls().last(),
            Some(&Call::SetDefault("renamed.by.wireplumber".to_string()))
        );
    }

    #[test]
    fn test_activate_node_never_appears_errors_after_deadline() {
        let backend = FakeBackend::new(None);
        let clock = FakeClock::new();
        let err = activate(&backend, &clock, POLICY, ANALOG).expect_err("`activate` fails");
        assert!(
            format!("{err}").contains("did not appear within 1000 ms"),
            "{err}"
        );
        assert_eq!(clock.offset.get(), POLICY.timeout);
        assert!(
            !backend
                .calls()
                .iter()
                .any(|c| matches!(c, Call::SetDefault(_)))
        );
        // One initial snapshot plus one per 100 ms poll.
        let snapshots = backend
            .calls()
            .iter()
            .filter(|c| **c == Call::Snapshot)
            .count();
        assert_eq!(snapshots, 11);
    }

    #[test]
    fn test_activate_unknown_sink_touches_nothing() {
        let backend = FakeBackend::new(Some(0));
        let err =
            activate(&backend, &FakeClock::new(), POLICY, "nope").expect_err("`activate` fails");
        assert!(format!("{err}").contains("is not present"), "{err}");
        assert_eq!(backend.calls(), vec![Call::Snapshot]);
    }

    const IEC: &str = "alsa_output.pci-0000_0c_00.4.iec958-stereo";

    fn names(names: &[&str]) -> Vec<String> {
        names.iter().map(|n| (*n).to_string()).collect()
    }

    #[test]
    fn test_activate_first_skips_unreachable_candidates() {
        let backend = FakeBackend::new(Some(0));
        let chosen = activate_first(
            &backend,
            &FakeClock::new(),
            POLICY,
            &names(&["bluez_output.gone", IEC]),
            None,
        )
        .expect("activates")
        .expect("something chosen");
        assert_eq!(chosen.index, 1);
        assert_eq!(chosen.activation.node_name, IEC);
        assert_eq!(backend.calls(), vec![
            Call::Snapshot,
            Call::SetDefault(IEC.to_string())
        ]);
    }

    #[test]
    fn test_activate_first_prefers_the_first_reachable() {
        let backend = FakeBackend::new(Some(0));
        let chosen = activate_first(
            &backend,
            &FakeClock::new(),
            POLICY,
            &names(&[ANALOG, IEC]),
            None,
        )
        .expect("activates")
        .expect("something chosen");
        assert_eq!(chosen.index, 0, "a profile switch away is still reachable");
        assert_eq!(chosen.activation.node_name, ANALOG);
    }

    #[test]
    fn test_activate_first_falls_through_when_activation_fails() {
        let mut backend = FakeBackend::new(Some(0));
        backend.never_appears = Some("analog-stereo");
        let chosen = activate_first(
            &backend,
            &FakeClock::new(),
            POLICY,
            &names(&[ANALOG, IEC]),
            None,
        )
        .expect("the second candidate activates")
        .expect("something chosen");
        assert_eq!(chosen.index, 1);
        assert_eq!(chosen.activation.node_name, IEC);
        let calls = backend.calls();
        // The failed switch removed the digital sink, so the fresh snapshot taken after
        // the failure leads to a switch back to the digital profile
        let switches: Vec<_> = calls
            .iter()
            .filter(|c| matches!(c, Call::SetProfile(..)))
            .collect();
        assert_eq!(switches.len(), 2, "{calls:?}");
        assert_eq!(calls.last(), Some(&Call::SetDefault(IEC.to_string())));
    }

    #[test]
    fn test_activate_first_none_reachable_names_every_candidate() {
        let backend = FakeBackend::new(Some(0));
        let err = activate_first(
            &backend,
            &FakeClock::new(),
            POLICY,
            &names(&["bluez_output.a", "bluez_output.b"]),
            None,
        )
        .expect_err("`activate_first` fails");
        let text = format!("{err:#}");
        assert!(text.contains("every candidate failed"), "{text}");
        assert!(
            text.contains("'bluez_output.a'") && text.contains("'bluez_output.b'"),
            "{text}"
        );
        assert_eq!(backend.calls(), vec![Call::Snapshot]);
    }

    #[test]
    fn test_activate_first_upgrade_only_acts_for_a_better_candidate() {
        // Candidate 1 is the current default; candidate 0 is still unreachable
        let backend = FakeBackend::new(Some(0));
        backend.set_configured(Endpoint::Sink, IEC);
        let chosen = activate_first(
            &backend,
            &FakeClock::new(),
            POLICY,
            &names(&["bluez_output.gone", IEC]),
            Some(1),
        )
        .expect("no error");
        assert_eq!(chosen, None);
        assert_eq!(backend.calls(), vec![Call::Snapshot], "nothing touched");

        // Now candidate 0 is reachable: it is activated
        let backend = FakeBackend::new(Some(0));
        backend.set_configured(Endpoint::Sink, "bluez_output.gone");
        let chosen = activate_first(
            &backend,
            &FakeClock::new(),
            POLICY,
            &names(&[IEC, "bluez_output.gone"]),
            Some(1),
        )
        .expect("activates")
        .expect("upgraded");
        assert_eq!(chosen.index, 0);
    }

    const SYNTH: &str = include_str!("../../tests/fixtures/pwdump-synthetic.json");
    const IN_ANALOG: &str = "alsa_input.pci-0000_0c_00.4.analog-stereo";

    type HalfOutcome = Option<Result<Option<Chosen>>>;

    fn joint(
        backend: &FakeBackend,
        sinks: &[&str],
        sources: &[&str],
    ) -> (HalfOutcome, HalfOutcome) {
        fn half(c: &[String]) -> Option<Request<'_>> {
            (!c.is_empty()).then_some(Request {
                candidates: c,
                upgrade_from: None,
                keep: false,
                switch_back: true,
            })
        }
        let (sinks, sources) = (names(sinks), names(sources));
        let outcome = joint_fresh(
            backend,
            &FakeClock::new(),
            POLICY,
            half(&sinks),
            half(&sources),
        );
        (outcome.sink, outcome.source)
    }

    fn chosen(result: HalfOutcome) -> Chosen {
        result
            .expect("half was asked for")
            .expect("activated")
            .expect("something chosen")
    }

    #[test]
    fn test_joint_same_card_switches_once_to_a_duplex_profile() {
        let backend = FakeBackend::new(Some(1));
        let (sink, source) = joint(&backend, &[ANALOG], &[IN_ANALOG]);
        assert_eq!(chosen(sink).activation.node_name, ANALOG);
        let source = chosen(source);
        assert_eq!(source.activation.node_name, IN_ANALOG);
        assert_eq!(
            backend.switches(),
            vec![Call::SetProfile(43, 1)],
            "one switch"
        );
        let calls = backend.calls();
        assert_eq!(
            calls.first(),
            Some(&Call::Snapshot),
            "one snapshot decides both"
        );
        let tail: Vec<&Call> = calls
            .iter()
            .filter(|c| matches!(c, Call::SetDefault(_) | Call::SetSource(_)))
            .collect();
        assert_eq!(tail, [
            &Call::SetDefault(ANALOG.to_string()),
            &Call::SetSource(IN_ANALOG.to_string())
        ]);
    }

    #[test]
    fn test_joint_impossible_combination_activates_the_sink_and_reports_the_source() {
        let backend = FakeBackend::with_fixture(SYNTH, Some(0));
        let surround = "alsa_output.pci-0000_00_1f.3.analog-surround-51";
        let mic = "alsa_input.pci-0000_00_1f.3.analog-stereo";
        let (sink, source) = joint(&backend, &[surround], &[mic]);
        assert_eq!(chosen(sink).activation.node_name, surround);
        let err = source
            .expect("asked")
            .expect_err("the source fails")
            .to_string();
        assert!(
            err.contains("not reachable together with the sink"),
            "{err}"
        );
        assert_eq!(backend.switches(), vec![Call::SetProfile(70, 9)]);
        assert!(
            !backend
                .calls()
                .iter()
                .any(|c| matches!(c, Call::SetSource(_)))
        );
    }

    /// A sink job with the source lane's answer riding along (`keep`)
    fn joint_keep(
        backend: &FakeBackend,
        sinks: &[&str],
        sources: &[&str],
    ) -> (HalfOutcome, HalfOutcome) {
        let (sinks, sources) = (names(sinks), names(sources));
        let outcome = joint_fresh(
            backend,
            &FakeClock::new(),
            POLICY,
            Some(Request {
                candidates: &sinks,
                upgrade_from: None,
                keep: false,
                switch_back: true,
            }),
            Some(Request {
                candidates: &sources,
                upgrade_from: None,
                keep: true,
                switch_back: true,
            }),
        );
        (outcome.sink, outcome.source)
    }

    /// The managed microphone is on the card a sink-only job switches, and the sink's
    /// only profile has no input: the switch still happens (the sink wins), and the
    /// source half reports the microphone lost instead of claiming it is still there
    #[test]
    fn test_keep_reports_a_microphone_the_sink_switch_drops() {
        let backend = FakeBackend::with_fixture(SYNTH, Some(0));
        let surround = "alsa_output.pci-0000_00_1f.3.analog-surround-51";
        let mic = "alsa_input.pci-0000_00_1f.3.analog-stereo";
        let (sink, source) = joint_keep(&backend, &[surround], &[mic]);
        assert_eq!(chosen(sink).activation.node_name, surround);
        let err = source
            .expect("asked")
            .expect_err("the source fails")
            .to_string();
        assert!(
            err.contains("not reachable together with the sink"),
            "{err}"
        );
        assert_eq!(backend.switches(), vec![Call::SetProfile(70, 9)]);
    }

    /// The sink's switch is on the microphone's card and a profile keeps both: one
    /// switch to it, and the microphone is set again once it reappears
    #[test]
    fn test_keep_plans_the_microphone_with_the_sink_switch() {
        let backend = FakeBackend::new(Some(1));
        let (sink, source) = joint_keep(&backend, &[ANALOG], &[IN_ANALOG]);
        assert_eq!(chosen(sink).activation.node_name, ANALOG);
        assert_eq!(chosen(source).activation.node_name, IN_ANALOG);
        assert_eq!(backend.switches(), vec![Call::SetProfile(43, 1)]);
    }

    /// The sink is on another card (or needs no switch): the riding source is left
    /// alone, nothing is set for it
    #[test]
    fn test_keep_leaves_the_microphone_alone_when_its_card_is_not_switched() {
        let backend = FakeBackend::with_fixture(SYNTH, Some(0));
        let headset = "alsa_output.usb-Example_USB_Headset-00.analog-stereo";
        let mic = "alsa_input.pci-0000_00_1f.3.analog-stereo";
        let (sink, source) = joint_keep(&backend, &[headset], &[mic]);
        assert_eq!(chosen(sink).activation.node_name, headset);
        assert!(matches!(source, Some(Ok(None))), "{source:?}");
        assert!(
            !backend
                .calls()
                .iter()
                .any(|c| matches!(c, Call::SetSource(_)))
        );

        let (sink, source) = joint_keep(&backend, &[headset], &[mic]);
        assert!(chosen(sink).activation.profile_switch.is_none());
        assert!(matches!(source, Some(Ok(None))), "{source:?}");
    }

    #[test]
    fn test_joint_different_cards_are_activated_independently() {
        let backend = FakeBackend::with_fixture(SYNTH, Some(0));
        let headset = "alsa_output.usb-Example_USB_Headset-00.analog-stereo";
        let mic = "alsa_input.pci-0000_00_1f.3.analog-stereo";
        let (sink, source) = joint(&backend, &[headset], &[mic]);
        assert_eq!(chosen(sink).activation.node_name, headset);
        let source = chosen(source);
        assert_eq!(source.activation.node_name, mic);
        assert_eq!(source.activation.profile_switch, None);
        assert_eq!(backend.switches(), vec![Call::SetProfile(60, 1)]);
    }

    #[test]
    fn test_source_alone_switches_keeping_the_output() {
        let backend = FakeBackend::new(Some(0));
        let (sink, source) = joint(&backend, &[], &[IN_ANALOG]);
        assert!(sink.is_none(), "no sink half, no sink result");
        let source = chosen(source);
        assert_eq!(source.activation.node_name, IN_ANALOG);
        assert_eq!(backend.switches(), vec![Call::SetProfile(43, 3)]);
        assert!(
            !backend
                .calls()
                .iter()
                .any(|c| matches!(c, Call::SetDefault(_)))
        );
    }

    #[test]
    fn test_joint_source_that_never_appears_falls_through_without_another_switch() {
        // The duplex switch produces the sink but not the input: the source half
        // moves to its fallback, and never switches the card a second time
        let mut backend = FakeBackend::new(Some(0));
        backend.never_appears_source = Some("analog-stereo");
        let (sink, source) = joint(&backend, &[ANALOG], &[IN_ANALOG, "bluez_input.gone"]);
        assert_eq!(chosen(sink).activation.node_name, ANALOG);
        let err = format!(
            "{:#}",
            source.expect("asked").expect_err("the source fails")
        );
        assert!(err.contains("every candidate failed"), "{err}");
        assert!(err.contains("did not appear"), "{err}");
        assert_eq!(backend.switches(), vec![Call::SetProfile(43, 1)]);
    }

    #[test]
    fn test_joint_upgrade_checks_are_per_half() {
        // The sink half is an upgrade check with nothing better: it is left alone,
        // while the source half still activates
        let backend = FakeBackend::new(Some(0));
        let sinks = names(&["bluez_output.gone", IEC]);
        let sources = names(&[IN_ANALOG]);
        let outcome = joint_fresh(
            &backend,
            &FakeClock::new(),
            POLICY,
            Some(Request {
                candidates: &sinks,
                upgrade_from: Some(1),
                keep: false,
                switch_back: true,
            }),
            Some(Request {
                candidates: &sources,
                upgrade_from: None,
                keep: false,
                switch_back: true,
            }),
        );
        assert!(matches!(outcome.sink, Some(Ok(None))));
        assert_eq!(chosen(outcome.source).activation.node_name, IN_ANALOG);
        assert!(
            !backend
                .calls()
                .iter()
                .any(|c| matches!(c, Call::SetDefault(_)))
        );
    }

    /// B1: a `pw-dump` failure before anything was switched fails the sink half, but
    /// leaves a microphone riding along untouched (`Ok(None)`): nothing was switched,
    /// so the source is where it was and pwsw must not forget it
    #[test]
    fn test_keep_is_untouched_when_the_snapshot_fails() {
        let backend = FakeBackend::new(Some(0));
        backend.failing_snapshots.set(1);
        let (sink, source) = joint_keep(&backend, &[ANALOG], &[IN_ANALOG]);
        let err = sink.expect("asked").expect_err("the sink fails");
        assert!(format!("{err:#}").contains("pw-dump"), "{err:#}");
        assert!(matches!(source, Some(Ok(None))), "{source:?}");
        assert_eq!(backend.calls(), vec![Call::Snapshot], "nothing touched");

        // A plain source half (its own job) still reports the failure
        let backend = FakeBackend::new(Some(0));
        backend.failing_snapshots.set(1);
        let (_, source) = joint(&backend, &[ANALOG], &[IN_ANALOG]);
        assert!(matches!(source, Some(Err(_))), "{source:?}");
    }

    /// B2: an upgrade check whose fallback is no longer the configured default (the
    /// user picked another one in `pavucontrol`) touches nothing and reports what the
    /// default is now, even though a better candidate is reachable
    #[test]
    fn test_upgrade_is_skipped_when_the_default_was_changed_elsewhere() {
        let backend = FakeBackend::new(Some(0));
        // The fallback `ANALOG` was applied, the user chose the digital output since
        backend.set_configured(Endpoint::Sink, IEC);
        let candidates = names(&["bluez_output.back", ANALOG]);
        backend.state.borrow_mut().sinks.push(Node {
            id: 900,
            name: "bluez_output.back".to_string(),
            description: String::new(),
            device_id: None,
            profile_name: None,
            card_profile_device: None,
        });
        let outcome = joint_fresh(
            &backend,
            &FakeClock::new(),
            POLICY,
            Some(Request {
                candidates: &candidates,
                upgrade_from: Some(1),
                keep: false,
                switch_back: true,
            }),
            None,
        );
        assert!(matches!(outcome.sink, Some(Ok(None))), "{:?}", outcome.sink);
        assert_eq!(outcome.elsewhere, [
            Some(Elsewhere::Other(IEC.to_string())),
            None
        ]);
        assert_eq!(backend.calls(), vec![Call::Snapshot], "nothing touched");

        // The user chose the rule's own first choice: reported as that candidate
        backend.set_configured(Endpoint::Sink, "bluez_output.back");
        let outcome = joint_fresh(
            &backend,
            &FakeClock::new(),
            POLICY,
            Some(Request {
                candidates: &candidates,
                upgrade_from: Some(1),
                keep: false,
                switch_back: true,
            }),
            None,
        );
        assert!(matches!(outcome.sink, Some(Ok(None))), "{:?}", outcome.sink);
        assert_eq!(outcome.elsewhere, [Some(Elsewhere::Candidate(0)), None]);
        assert!(backend.switches().is_empty(), "nothing touched");

        // Still on the fallback (by name): the upgrade goes ahead
        backend.set_configured(Endpoint::Sink, ANALOG);
        let outcome = joint_fresh(
            &backend,
            &FakeClock::new(),
            POLICY,
            Some(Request {
                candidates: &candidates,
                upgrade_from: Some(1),
                keep: false,
                switch_back: true,
            }),
            None,
        );
        assert_eq!(outcome.elsewhere, [None, None]);
        assert_eq!(chosen(outcome.sink).index, 0);
    }

    /// B2: a fallback activated under another node name (renamed by `WirePlumber`) is
    /// still recognised as the configured default
    #[test]
    fn test_upgrade_recognises_a_renamed_fallback_as_the_default() {
        let backend = FakeBackend::new(Some(0));
        // The digital output is active as `IEC`; pretend `WirePlumber` renamed it
        backend.state.borrow_mut().sinks[0].name = "renamed.digital".to_string();
        backend.set_configured(Endpoint::Sink, "renamed.digital");
        let candidates = names(&["bluez_output.gone", IEC]);
        let outcome = joint_fresh(
            &backend,
            &FakeClock::new(),
            POLICY,
            Some(Request {
                candidates: &candidates,
                upgrade_from: Some(1),
                keep: false,
                switch_back: true,
            }),
            None,
        );
        assert!(matches!(outcome.sink, Some(Ok(None))), "{:?}", outcome.sink);
        assert_eq!(outcome.elsewhere, [None, None], "not changed elsewhere");
    }

    /// B3: the default wait (20 polls of 150 ms) covers a card that needs 2.5 s
    #[test]
    fn test_default_wait_covers_a_slow_card() {
        let policy = WaitPolicy::from_values(None, None);
        assert_eq!(policy.timeout, Duration::from_secs(3));
        // The node appears on the 17th poll (2.55 s)
        let backend = FakeBackend::new(Some(16));
        let act = activate(&backend, &FakeClock::new(), policy, ANALOG).expect("activates");
        assert_eq!(act.node_name, ANALOG);
    }

    /// B3: a switch whose node does not appear in time is undone: the card goes back
    /// to its previous profile (so the old node comes back), nothing is set as
    /// default, and the error (one line) says so
    #[test]
    fn test_timed_out_switch_is_switched_back() {
        let backend = FakeBackend::new(Some(50));
        let err = activate(&backend, &FakeClock::new(), POLICY, ANALOG).expect_err("times out");
        let text = format!("{err:#}");
        assert!(text.contains("did not appear within 1000 ms"), "{text}");
        assert!(
            text.contains("; switched it back to its previous profile 'output:iec958-stereo'"),
            "{text}"
        );
        assert!(!text.contains('\n'), "one line: {text:?}");
        assert!(
            err.downcast_ref::<CardLeft>().is_none(),
            "the card is as it was"
        );
        assert_eq!(
            backend.switches(),
            vec![Call::SetProfile(43, 1), Call::SetProfile(43, 4)],
            "switched to the duplex profile, then back to the digital one"
        );
        assert!(
            !backend
                .calls()
                .iter()
                .any(|c| matches!(c, Call::SetDefault(_))),
            "the configured default is left alone"
        );
    }

    /// B3: when the previous profile cannot be switched back to, the card is left and
    /// the error says so
    #[test_case(true, false, "is not available, so the card stays on" ; "previous unavailable")]
    #[test_case(false, true, "failed (pw-cli: set-param failed), so the card stays on" ; "switch back fails")]
    fn test_timed_out_switch_that_cannot_be_undone_says_so(
        unavailable: bool,
        fails: bool,
        expected: &str,
    ) {
        let mut backend = FakeBackend::new(Some(50));
        if unavailable {
            let mut state = backend.state.borrow_mut();
            let device = state.devices.iter_mut().find(|d| d.id == 43).expect("card");
            let previous = device
                .profiles
                .iter_mut()
                .find(|p| p.index == 4)
                .expect("active profile");
            previous.available = crate::pipewire::model::Availability::No;
        }
        if fails {
            backend.failing_profile = Some(4);
        }
        let err = activate(&backend, &FakeClock::new(), POLICY, ANALOG).expect_err("times out");
        let text = format!("{err:#}");
        assert!(text.contains(expected), "{text}");
        assert!(
            err.downcast_ref::<CardLeft>().is_some(),
            "the card is left switched"
        );
        let expected_switches = if fails {
            vec![Call::SetProfile(43, 1), Call::SetProfile(43, 4)]
        } else {
            vec![Call::SetProfile(43, 1)]
        };
        assert_eq!(backend.switches(), expected_switches);
    }

    /// B3/B1: the sink's switch on the microphone's card timed out and was undone:
    /// the microphone riding along is reported untouched (not lost), and no second
    /// switch is made for it
    #[test]
    fn test_keep_is_untouched_when_the_sink_switch_was_undone() {
        let backend = FakeBackend::new(Some(50));
        let (sink, source) = joint_keep(&backend, &[ANALOG], &[IN_ANALOG]);
        let err = sink.expect("asked").expect_err("the sink times out");
        assert!(format!("{err:#}").contains("switched it back"), "{err:#}");
        assert!(matches!(source, Some(Ok(None))), "{source:?}");
        assert_eq!(backend.switches(), vec![
            Call::SetProfile(43, 1),
            Call::SetProfile(43, 4)
        ]);
        assert!(
            !backend
                .calls()
                .iter()
                .any(|c| matches!(c, Call::SetSource(_)))
        );
    }

    /// A sink half over `candidates` with the given `switch_back`
    fn sink_half(candidates: &[String], switch_back: bool) -> Request<'_> {
        Request {
            candidates,
            upgrade_from: None,
            keep: false,
            switch_back,
        }
    }

    /// A rule attempt that a retry follows (`switch_back: false`) leaves a timed-out
    /// switch in place and says so ([`CardLeft`]); the retry finds the node the slow
    /// card produced meanwhile and only sets the default (no flapping)
    #[test]
    fn test_timed_out_switch_is_left_for_the_retry_that_follows() {
        // The node appears on the 11th snapshot after the switch: just after the 10
        // polls of the first attempt
        let backend = FakeBackend::new(Some(10));
        let candidates = names(&[ANALOG]);
        let outcome = joint_fresh(
            &backend,
            &FakeClock::new(),
            POLICY,
            Some(sink_half(&candidates, false)),
            None,
        );
        let err = outcome.sink.expect("asked").expect_err("times out");
        assert!(err.downcast_ref::<CardLeft>().is_some(), "{err:#}");
        let text = format!("{err:#}");
        assert!(
            text.contains(
                "did not appear within 1000 ms; left the card on that profile, since pwsw tries \
                 again (the sink may still appear)"
            ),
            "{text}"
        );
        assert!(!text.contains("switched it back"), "{text}");
        assert_eq!(
            backend.switches(),
            vec![Call::SetProfile(43, 1)],
            "not undone"
        );

        let outcome = joint_fresh(
            &backend,
            &FakeClock::new(),
            POLICY,
            Some(sink_half(&candidates, false)),
            None,
        );
        assert_eq!(chosen(outcome.sink).activation.node_name, ANALOG);
        assert_eq!(
            backend.switches(),
            vec![Call::SetProfile(43, 1)],
            "the retry switches nothing"
        );
        assert_eq!(
            backend.calls().last(),
            Some(&Call::SetDefault(ANALOG.to_string()))
        );
    }

    /// A node that never appears: the attempts that follow a left switch do not switch
    /// the card again but wait once more, and the last one switches it back to the
    /// profile from before the first attempt (not the one it was left on)
    #[test]
    fn test_left_card_is_waited_for_again_and_switched_back_at_the_end() {
        let mut backend = FakeBackend::new(Some(0));
        backend.never_appears = Some("analog-stereo");
        let candidates = names(&[ANALOG]);
        let mut left = LeftCards::new();
        let mut attempt = |switch_back| {
            activate_joint(
                &backend,
                &FakeClock::new(),
                POLICY,
                Some(sink_half(&candidates, switch_back)),
                None,
                &mut left,
            )
            .sink
            .expect("asked")
            .expect_err("never appears")
        };
        for _ in 0..2 {
            let err = attempt(false);
            assert!(err.downcast_ref::<CardLeft>().is_some(), "{err:#}");
        }
        let err = attempt(true);
        assert!(
            format!("{err:#}")
                .contains("switched it back to its previous profile 'output:iec958-stereo'"),
            "{err:#}"
        );
        assert!(err.downcast_ref::<CardLeft>().is_none(), "{err:#}");
        assert_eq!(
            backend.switches(),
            vec![Call::SetProfile(43, 1), Call::SetProfile(43, 4)],
            "switched once, back once"
        );
        assert!(left.0.is_empty(), "done with the card");
    }

    /// A card left switched and then wanted on another profile is simply switched
    /// (from the profile it is on), and forgotten as left
    #[test]
    fn test_left_card_wanted_elsewhere_is_switched_normally() {
        let mut backend = FakeBackend::new(Some(0));
        backend.never_appears = Some("analog-stereo");
        let mut left = LeftCards::new();
        let analog = names(&[ANALOG]);
        let outcome = activate_joint(
            &backend,
            &FakeClock::new(),
            POLICY,
            Some(sink_half(&analog, false)),
            None,
            &mut left,
        );
        assert!(outcome.sink.expect("asked").is_err());
        assert_eq!(left.0.len(), 1);
        let iec = names(&["alsa_output.pci-0000_0c_00.4.iec958-stereo"]);
        let outcome = activate_joint(
            &backend,
            &FakeClock::new(),
            POLICY,
            Some(sink_half(&iec, true)),
            None,
            &mut left,
        );
        let chosen = chosen(outcome.sink);
        let switch = chosen.activation.profile_switch.expect("a profile switch");
        assert_eq!(
            switch.previous.map(|p| p.index),
            Some(1),
            "from where it is"
        );
        // From the duplex profile it is on, the digital one that keeps the input
        assert_eq!(backend.switches(), vec![
            Call::SetProfile(43, 1),
            Call::SetProfile(43, 3)
        ]);
        assert!(left.0.is_empty());
    }

    /// The same slow card on the last attempt (`switch_back: true`) is switched back
    #[test]
    fn test_timed_out_switch_on_the_last_attempt_is_switched_back() {
        let backend = FakeBackend::new(Some(10));
        let candidates = names(&[ANALOG]);
        let outcome = joint_fresh(
            &backend,
            &FakeClock::new(),
            POLICY,
            Some(sink_half(&candidates, true)),
            None,
        );
        let err = outcome.sink.expect("asked").expect_err("times out");
        assert!(err.downcast_ref::<CardLeft>().is_none(), "{err:#}");
        assert_eq!(backend.switches(), vec![
            Call::SetProfile(43, 1),
            Call::SetProfile(43, 4)
        ]);
    }

    /// A riding microphone whose card the sink's switch was to the profile the card
    /// already had (its sink node was missing): the switch timed out with nothing to
    /// switch back, and the microphone counts as untouched, not lost
    #[test]
    fn test_keep_is_untouched_when_the_switch_was_to_the_cards_profile() {
        let backend = FakeBackend::new(Some(50));
        {
            let mut state = backend.state.borrow_mut();
            let card = state.devices.iter_mut().find(|d| d.id == 43).expect("card");
            card.active_profile = Some(1);
            state.sinks.retain(|s| s.device_id != Some(43));
        }
        let (sink, source) = joint_keep(&backend, &[ANALOG], &[IN_ANALOG]);
        let err = sink.expect("asked").expect_err("the sink times out");
        let text = format!("{err:#}");
        assert!(
            text.contains(
                "; the card was already on that profile before, so there was nothing to switch \
                 back"
            ),
            "{text}"
        );
        assert!(err.downcast_ref::<CardLeft>().is_none(), "{text}");
        assert!(matches!(source, Some(Ok(None))), "{source:?}");
        assert_eq!(backend.switches(), vec![Call::SetProfile(43, 1)]);
    }

    /// Fails every snapshot but the first (the planning one), with the backend's error
    struct FailAfterFirst<'a>(&'a FakeBackend, Cell<bool>);

    impl AudioBackend for FailAfterFirst<'_> {
        fn snapshot(&self) -> Result<Snapshot> {
            if self.1.replace(true) {
                self.0.failing_snapshots.set(1);
            }
            self.0.snapshot()
        }

        fn set_default(&self, kind: Endpoint, node_name: &str) -> Result<()> {
            self.0.set_default(kind, node_name)
        }

        fn set_profile(&self, device_id: u32, profile_index: u32) -> Result<()> {
            self.0.set_profile(device_id, profile_index)
        }
    }

    /// `pw-dump` errors of several lines (its stderr) while waiting, or when looking
    /// for the next candidate, end up folded into the half's one-line error
    #[test_case(&[ANALOG], "(last pw-dump error: `pw-dump` failed (exit status: 1): pw-dump: failed to connect; Host is down)" ; "while waiting")]
    #[test_case(&[ANALOG, "bluez_output.gone"], "failed: `pw-dump` failed (exit status: 1): pw-dump: failed to connect; Host is down" ; "after the attempt")]
    fn test_multi_line_pw_dump_errors_are_folded(sinks: &[&str], expected: &str) {
        let mut backend = FakeBackend::new(Some(50));
        backend.snapshot_error =
            "`pw-dump` failed (exit status: 1): pw-dump: failed to connect\nHost is down";
        let candidates = names(sinks);
        let mut outcome = None;
        let logs = capture_debug_logs(|| {
            outcome = Some(joint_fresh(
                &FailAfterFirst(&backend, Cell::new(false)),
                &FakeClock::new(),
                POLICY,
                Some(sink_half(&candidates, true)),
                None,
            ));
        });
        let outcome = outcome.expect("ran");
        let err = outcome.sink.expect("asked").expect_err("fails");
        let text = format!("{err:#}");
        assert!(!text.contains('\n'), "one line: {text:?}");
        assert!(text.contains(expected), "{text}");
        // Nor do the log lines on the way (the wait's debug lines, the warning before
        // the next candidate) break into a second line
        assert!(logs.contains("Host is down"), "{logs}");
        assert!(
            !logs.lines().any(|l| l.starts_with("Host is down")),
            "{logs}"
        );
    }

    /// A failure of several lines (a tool's stderr) is folded into the warning before
    /// the next candidate is tried
    #[test]
    fn test_fall_through_warning_is_one_line() {
        let mut backend = FakeBackend::new(Some(0));
        backend.failing_profile = Some(1);
        backend.profile_error =
            "`pw-cli` failed (exit status: 1): Error: \"set-param\"\nInvalid argument";
        let iec = "alsa_output.pci-0000_0c_00.4.iec958-stereo";
        let candidates = names(&[ANALOG, iec]);
        let mut outcome = None;
        let logs = capture_debug_logs(|| {
            outcome = Some(joint_fresh(
                &backend,
                &FakeClock::new(),
                POLICY,
                Some(sink_half(&candidates, true)),
                None,
            ));
        });
        assert_eq!(chosen(outcome.expect("ran").sink).index, 1);
        assert!(
            logs.contains("trying the next sink: `pw-cli` failed (exit status: 1): Error: \"set-param\"; Invalid argument"),
            "{logs}"
        );
        assert!(
            !logs.lines().any(|l| l.starts_with("Invalid argument")),
            "{logs}"
        );
    }

    /// Capture what `f` logs at `DEBUG` and above (this thread only), without `ANSI`
    fn capture_debug_logs(f: impl FnOnce()) -> String {
        #[derive(Clone, Default)]
        struct Buffer(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);
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
            .with_max_level(tracing::Level::DEBUG)
            .finish();
        tracing::subscriber::with_default(subscriber, f);
        let bytes = buffer.0.lock().expect("lock").clone();
        String::from_utf8(bytes).expect("utf-8 logs")
    }

    #[test_case(None, None, 150, 3_000 ; "unset")]
    #[test_case(Some("200"), Some("10"), 200, 2_000 ; "custom")]
    #[test_case(Some("1000"), Some("100"), 1_000, 5_000 ; "total_capped")]
    #[test_case(Some("999999999"), Some("999999999"), 1_000, 5_000 ; "extreme")]
    #[test_case(Some("x"), Some("-1"), 150, 3_000 ; "invalid")]
    #[test_case(Some("0"), Some("0"), 10, 10 ; "tiny")]
    fn test_wait_policy_from_values(
        delay: Option<&str>,
        retries: Option<&str>,
        interval_ms: u64,
        total_ms: u64,
    ) {
        assert_eq!(WaitPolicy::from_values(delay, retries), WaitPolicy {
            poll_interval: Duration::from_millis(interval_ms),
            timeout: Duration::from_millis(total_ms),
        });
    }
}
