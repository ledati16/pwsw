//! Activation worker
//!
//! One task owns the [`SinkSwitcher`] and runs activations one at a time in
//! `spawn_blocking`, so the daemon's main loop never waits on `PipeWire`. Requests go
//! through a `watch` channel holding one slot per [`Endpoint`] (sink, source): while
//! an activation runs, a newer request overwrites the older one in its slot and only
//! the latest is run next ("latest wins", per slot).
//!
//! Whatever is pending in both slots when the worker starts a run goes into one
//! *joint* activation: the switcher resolves the sink half and the source half against
//! one fresh snapshot, so a sink and a microphone on the same card cost one profile
//! switch. Each half's result comes back as its own [`JobResult`].
//!
//! Rule-driven jobs carry the rule's candidates in priority order; the switcher
//! activates the first reachable one (falling through to the next reachable one if
//! that activation fails), and the result says which candidate won.
//!
//! Manual requests are decided here too, not on the loop: the worker probes the audio
//! server (the real defaults, which sinks can be activated) and lets [`manual::plan`]
//! pick the target before activating it.

use std::sync::Arc;

use anyhow::Result;
use tokio::sync::{mpsc, watch};
use tracing::debug;

use crate::config::{Config, Endpoint};
use crate::manual::{self, ManualSwitch, ManualTarget, Probe};
use crate::pipewire::{Activated, CardLeft, PipeWire};

/// One half of an activation: candidates (configured node names, in priority order),
/// for an upgrade check the index of the candidate that is the default already, and
/// whether it only rides along with the sink half ([`Target::Keep`])
pub type Half<'a> = crate::pipewire::JobHalf<'a>;

/// What one half did (see [`Activated`])
pub type HalfResult = Result<Activated>;

/// Something that can make configured sinks and sources the defaults
pub trait SinkSwitcher: Send + Sync + 'static {
    /// Activate the first reachable sink and the first reachable source of the given
    /// halves (blocking), resolved together; a half that is `None` is left alone and
    /// gets `None` back
    ///
    /// Within a half, an activation that fails moves on to the next reachable
    /// candidate. With `upgrade_from: Some(k)` (candidate `k` is the default already),
    /// the half returns [`Activated::Unchanged`] without changing anything unless a
    /// candidate before `k` is reachable, and [`Activated::Elsewhere`] when the real
    /// default is not candidate `k` (chosen outside pwsw since). A source half with
    /// `keep` returns [`Activated::Unchanged`] unless
    /// the sink half's profile switch is on its card (see
    /// [`crate::pipewire::JobHalf::keep`]). A half's error means no candidate could be
    /// activated.
    fn activate(
        &self,
        sink: Option<Half<'_>>,
        source: Option<Half<'_>>,
    ) -> (Option<HalfResult>, Option<HalfResult>);

    /// Look at the audio server for a manual request (blocking): the real defaults
    /// and which of `config`'s sinks can be activated right now
    ///
    /// # Errors
    /// Returns an error if the audio server cannot be queried.
    fn probe(&self, config: &Config) -> Result<Probe>;
}

/// The real switcher: [`PipeWire::activate_joint`], probing with one `pw-dump`
pub struct PipeWireSwitcher;

impl SinkSwitcher for PipeWireSwitcher {
    fn activate(
        &self,
        sink: Option<Half<'_>>,
        source: Option<Half<'_>>,
    ) -> (Option<HalfResult>, Option<HalfResult>) {
        PipeWire::activate_joint(sink, source)
    }

    fn probe(&self, config: &Config) -> Result<Probe> {
        Ok(Probe::from_snapshot(&PipeWire::dump()?, config))
    }
}

/// What a job should activate
#[derive(Debug, Clone)]
pub(crate) enum Target {
    /// The first reachable of these configured sinks (sources), in priority order
    /// (rule-driven switches). With `upgrade_from: Some(k)`, candidate `k` is the
    /// default already and only a reachable candidate before it is worth a switch.
    Rule {
        candidates: Vec<String>,
        upgrade_from: Option<usize>,
        /// No further attempt follows if this one fails (the last retry, an upgrade
        /// check): a profile switch whose node does not appear in time is switched
        /// back. Otherwise the card is left on the new profile for the retry.
        final_attempt: bool,
    },
    /// Wherever a manual request leads, decided by [`manual::plan`] against a fresh
    /// probe of the audio server
    Manual {
        request: ManualSwitch,
        config: Arc<Config>,
    },
    /// Source slot only, riding along with a sink job: keep these sources (the source
    /// lane's current answer) working across the sink's profile switch. Touches
    /// nothing ([`Outcome::Unchanged`]) unless that switch is on their card.
    Keep { candidates: Vec<String> },
}

/// An activation request
#[derive(Debug, Clone)]
pub(crate) struct Job {
    /// Increases with every submitted job (one counter for both slots)
    pub generation: u64,
    /// The default the job changes (its slot)
    pub kind: Endpoint,
    pub target: Target,
}

/// The worker's input: the latest job per [`Endpoint`] (index [`Endpoint::index`])
pub(crate) type Pending = [Option<Job>; 2];

/// How a job ended
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// `tried[index]` is now the default, as this node
    Activated { index: usize, node_name: String },
    /// An upgrade check found no better candidate reachable; nothing changed
    Unchanged,
    /// An upgrade check found the default changed outside pwsw (this node is the
    /// default now); nothing changed
    Elsewhere(String),
    /// An upgrade check found `tried[index]` (a candidate before the applied
    /// fallback) is the default already, chosen outside pwsw; nothing changed
    Adopted { index: usize },
    /// Nothing needed doing (manual request); the reply message
    Done(String),
    /// The error (formatted with its causes)
    Failed(String),
}

/// The outcome of a job the worker ran
#[derive(Debug)]
pub(crate) struct JobResult {
    pub generation: u64,
    pub kind: Endpoint,
    /// The configured sinks (sources) the job chose among, in priority order (a
    /// manual job: the one it planned); empty if it never chose one
    pub tried: Vec<String>,
    /// The real default the probe saw before acting (manual jobs only)
    pub observed: Option<String>,
    /// A manual request whose target was the default already (smart toggle): the
    /// description of that target; the job switched to the default sink (source)
    pub toggled_from: Option<String>,
    pub outcome: Outcome,
    /// The job failed and left a card on a profile it switched to (the node had not
    /// appeared yet; see [`crate::pipewire::JobHalf::switch_back`]), so the default
    /// from before may not play any more
    pub card_left: bool,
    /// Another result of the same run follows (the other half of a joint
    /// activation), so the loop can publish the status once for the whole run
    pub more: bool,
}

/// Capacity of the result channel back to the main loop
const RESULT_CHANNEL_CAPACITY: usize = 8;

/// Start the worker task
///
/// Returns the job sender (put `Some(job)` into the job's slot; the worker runs the
/// latest value of each slot) and the result receiver. The worker stops when the job
/// sender is dropped.
pub(crate) fn spawn<S: SinkSwitcher>(
    switcher: Arc<S>,
) -> (watch::Sender<Pending>, mpsc::Receiver<JobResult>) {
    let (job_tx, job_rx) = watch::channel([None, None]);
    let (result_tx, result_rx) = mpsc::channel(RESULT_CHANNEL_CAPACITY);
    tokio::spawn(run(switcher, job_rx, result_tx));
    (job_tx, result_rx)
}

/// A job made ready to activate: its candidates, or already finished
enum Prepared {
    Activate {
        tried: Vec<String>,
        upgrade_from: Option<usize>,
        keep: bool,
        switch_back: bool,
        observed: Option<String>,
        toggled_from: Option<String>,
    },
    Finished(JobResult),
}

/// Decide what a job activates (probing for a manual request)
fn prepare<S: SinkSwitcher>(switcher: &S, job: Job, probe: &mut Option<Probe>) -> Prepared {
    let Job {
        generation,
        kind,
        target,
    } = job;
    let finished = |observed, outcome| {
        Prepared::Finished(JobResult {
            generation,
            kind,
            tried: Vec::new(),
            observed,
            toggled_from: None,
            outcome,
            card_left: false,
            more: false,
        })
    };
    match target {
        Target::Rule {
            candidates,
            upgrade_from,
            final_attempt,
        } => Prepared::Activate {
            tried: candidates,
            upgrade_from,
            keep: false,
            switch_back: final_attempt,
            observed: None,
            toggled_from: None,
        },
        // Nothing retries a microphone riding along
        Target::Keep { candidates } => Prepared::Activate {
            tried: candidates,
            upgrade_from: None,
            keep: true,
            switch_back: true,
            observed: None,
            toggled_from: None,
        },
        Target::Manual { request, config } => {
            let probed = match probe {
                Some(probe) => probe,
                None => match switcher.probe(&config) {
                    Ok(fresh) => probe.insert(fresh),
                    Err(e) => {
                        return finished(
                            None,
                            Outcome::Failed(format!("Cannot read the current audio state: {e:#}")),
                        );
                    }
                },
            };
            let observed = match kind {
                Endpoint::Sink => probed.current.clone(),
                Endpoint::Source => probed.current_source.clone(),
            };
            match manual::plan(&config, &request, probed) {
                // A failed manual switch is not retried: the card goes back at once
                Ok(ManualTarget::Switch(target)) => Prepared::Activate {
                    tried: vec![target.name.clone()],
                    upgrade_from: None,
                    keep: false,
                    switch_back: true,
                    observed,
                    toggled_from: None,
                },
                Ok(ManualTarget::ToggleBack { from, to }) => Prepared::Activate {
                    tried: vec![to.name.clone()],
                    upgrade_from: None,
                    keep: false,
                    switch_back: true,
                    observed,
                    toggled_from: Some(from.desc.clone()),
                },
                Ok(ManualTarget::Done(message)) => finished(observed, Outcome::Done(message)),
                Err(message) => finished(observed, Outcome::Failed(message)),
            }
        }
    }
}

/// The outcome of one activated half, and whether it left a card on a profile it
/// switched to ([`JobResult::card_left`])
fn outcome_of(result: Option<HalfResult>, tried: &[String]) -> (Outcome, bool) {
    if let Some(Err(e)) = &result
        && e.downcast_ref::<CardLeft>().is_some()
    {
        return (Outcome::Failed(format!("{e:#}")), true);
    }
    let outcome = match result {
        Some(Ok(Activated::Switched { index, node_name })) if index < tried.len() => {
            Outcome::Activated { index, node_name }
        }
        Some(Ok(Activated::Switched { index, node_name })) => Outcome::Failed(format!(
            "Internal error: activated candidate {index} ({node_name}) of {}",
            tried.len()
        )),
        Some(Ok(Activated::Unchanged)) => Outcome::Unchanged,
        Some(Ok(Activated::Elsewhere(current))) => Outcome::Elsewhere(current),
        Some(Ok(Activated::Adopted { index })) if index < tried.len() => Outcome::Adopted { index },
        Some(Ok(Activated::Adopted { index })) => Outcome::Failed(format!(
            "Internal error: adopted candidate {index} of {}",
            tried.len()
        )),
        Some(Err(e)) => Outcome::Failed(format!("{e:#}")),
        None => Outcome::Failed("Internal error: the activation returned nothing".to_string()),
    };
    (outcome, false)
}

/// A job to activate
struct Ready {
    generation: u64,
    tried: Vec<String>,
    upgrade_from: Option<usize>,
    keep: bool,
    switch_back: bool,
    observed: Option<String>,
    toggled_from: Option<String>,
}

/// Run the pending jobs (at most one per slot) as one joint activation (blocking)
fn execute<S: SinkSwitcher>(switcher: &S, jobs: Vec<Job>) -> Vec<JobResult> {
    fn half(slot: Option<&Ready>) -> Option<Half<'_>> {
        slot.map(|ready| Half {
            candidates: &ready.tried,
            upgrade_from: ready.upgrade_from,
            keep: ready.keep,
            switch_back: ready.switch_back,
        })
    }

    let mut probe = None;
    let mut results = Vec::new();
    let mut halves: [Option<Ready>; 2] = [None, None];
    for job in jobs {
        let (generation, kind) = (job.generation, job.kind);
        match prepare(switcher, job, &mut probe) {
            Prepared::Activate {
                tried,
                upgrade_from,
                keep,
                switch_back,
                observed,
                toggled_from,
            } => {
                halves[kind.index()] = Some(Ready {
                    generation,
                    tried,
                    upgrade_from,
                    keep,
                    switch_back,
                    observed,
                    toggled_from,
                });
            }
            Prepared::Finished(result) => results.push(result),
        }
    }
    // Nothing to activate, or only a source riding along with a sink job that
    // finished without activating anything (a manual request with nothing to do): the
    // source was not touched, so no snapshot is needed to say so
    if halves.iter().flatten().all(|ready| ready.keep) {
        for (kind, slot) in [Endpoint::Sink, Endpoint::Source].into_iter().zip(halves) {
            if let Some(Ready {
                generation,
                tried,
                observed,
                toggled_from,
                ..
            }) = slot
            {
                results.push(JobResult {
                    generation,
                    kind,
                    tried,
                    observed,
                    toggled_from,
                    outcome: Outcome::Unchanged,
                    card_left: false,
                    more: false,
                });
            }
        }
        return results;
    }
    let (sink, source) = switcher.activate(half(halves[0].as_ref()), half(halves[1].as_ref()));
    for (kind, result) in [(Endpoint::Sink, sink), (Endpoint::Source, source)] {
        if let Some(Ready {
            generation,
            tried,
            observed,
            toggled_from,
            ..
        }) = halves[kind.index()].take()
        {
            let (outcome, card_left) = outcome_of(result, &tried);
            results.push(JobResult {
                generation,
                kind,
                tried,
                observed,
                toggled_from,
                outcome,
                card_left,
                more: false,
            });
        }
    }
    results
}

async fn run<S: SinkSwitcher>(
    switcher: Arc<S>,
    mut jobs: watch::Receiver<Pending>,
    results: mpsc::Sender<JobResult>,
) {
    // The generation last run per slot: a slot not overwritten since is not rerun
    let mut done = [0_u64; 2];
    while jobs.changed().await.is_ok() {
        let pending = jobs.borrow_and_update().clone();
        let todo: Vec<Job> = pending
            .into_iter()
            .flatten()
            .filter(|job| job.generation > done[job.kind.index()])
            .collect();
        if todo.is_empty() {
            continue;
        }
        for job in &todo {
            done[job.kind.index()] = job.generation;
            match &job.target {
                Target::Rule {
                    candidates,
                    upgrade_from,
                    final_attempt,
                } => debug!(
                    "Activating the first reachable {} of [{}]{}{} (job {})",
                    job.kind.noun(),
                    candidates.join(", "),
                    upgrade_from.map_or_else(String::new, |k| format!(" before #{k}")),
                    if *final_attempt { ", last attempt" } else { "" },
                    job.generation
                ),
                Target::Manual { request, .. } => {
                    debug!("Manual request {request} (job {})", job.generation);
                }
                Target::Keep { candidates } => debug!(
                    "Keeping the {} [{}] across the sink's activation (job {})",
                    job.kind.noun(),
                    candidates.join(", "),
                    job.generation
                ),
            }
        }

        // If the blocking task dies, every job of the run fails
        let failed: Vec<(u64, Endpoint, Vec<String>)> = todo
            .iter()
            .map(|job| {
                let tried = match &job.target {
                    Target::Rule { candidates, .. } | Target::Keep { candidates } => {
                        candidates.clone()
                    }
                    Target::Manual { .. } => Vec::new(),
                };
                (job.generation, job.kind, tried)
            })
            .collect();
        let switcher = Arc::clone(&switcher);
        let finished = tokio::task::spawn_blocking(move || execute(switcher.as_ref(), todo))
            .await
            .unwrap_or_else(|e| {
                failed
                    .into_iter()
                    .map(|(generation, kind, tried)| JobResult {
                        generation,
                        kind,
                        tried,
                        observed: None,
                        toggled_from: None,
                        outcome: Outcome::Failed(format!("activation task failed: {e}")),
                        card_left: false,
                        more: false,
                    })
                    .collect()
            });
        let last = finished.len().saturating_sub(1);
        for (i, mut result) in finished.into_iter().enumerate() {
            result.more = i < last;
            if results.send(result).await.is_err() {
                return;
            }
        }
    }
}
