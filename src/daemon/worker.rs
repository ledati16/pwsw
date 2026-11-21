//! Sink activation worker
//!
//! One task owns the [`SinkSwitcher`] and runs activations one at a time in
//! `spawn_blocking`, so the daemon's main loop never waits on `PipeWire`. Requests go
//! through a `watch` channel: while an activation runs, newer requests overwrite older
//! ones and only the latest is run next ("latest wins").
//!
//! Manual requests are decided here too, not on the loop: the worker probes the audio
//! server (the real default sink, which sinks can be activated) and lets
//! [`manual::plan`] pick the sink before activating it.

use std::sync::Arc;

use color_eyre::eyre::Result;
use tokio::sync::{mpsc, watch};
use tracing::debug;

use crate::config::Config;
use crate::manual::{self, ManualSwitch, ManualTarget, Probe};
use crate::pipewire::PipeWire;

/// Something that can make a configured sink the default
pub trait SinkSwitcher: Send + Sync + 'static {
    /// Activate `sink_name` (blocking), returning the node name made default
    ///
    /// # Errors
    /// Returns an error if the sink cannot be activated.
    fn activate(&self, sink_name: &str) -> Result<String>;

    /// Look at the audio server for a manual request (blocking): the real default
    /// sink and which of `config`'s sinks can be activated right now
    ///
    /// # Errors
    /// Returns an error if the audio server cannot be queried.
    fn probe(&self, config: &Config) -> Result<Probe>;
}

/// The real switcher: [`PipeWire::activate_sink`], probing with one `pw-dump`
pub struct PipeWireSwitcher;

impl SinkSwitcher for PipeWireSwitcher {
    fn activate(&self, sink_name: &str) -> Result<String> {
        PipeWire::activate_sink(sink_name)
    }

    fn probe(&self, config: &Config) -> Result<Probe> {
        Ok(Probe::from_snapshot(&PipeWire::dump()?, config))
    }
}

/// What a job should activate
#[derive(Debug, Clone)]
pub(crate) enum Target {
    /// This configured sink (rule-driven switches)
    Sink(String),
    /// Wherever a manual request leads, decided by [`manual::plan`] against a fresh
    /// probe of the audio server
    Manual {
        request: ManualSwitch,
        config: Arc<Config>,
    },
}

/// An activation request
#[derive(Debug, Clone)]
pub(crate) struct Job {
    /// Increases with every submitted job
    pub generation: u64,
    pub target: Target,
}

/// How a job ended
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// The sink is now the default, as this node
    Activated(String),
    /// Nothing needed doing (manual request); the reply message
    Done(String),
    /// The error (formatted with its causes)
    Failed(String),
}

/// The outcome of a job the worker ran
#[derive(Debug)]
pub(crate) struct JobResult {
    pub generation: u64,
    /// The configured sink activated or tried; `None` if the job never chose one
    pub sink: Option<String>,
    /// The real default sink the probe saw before acting (manual jobs only)
    pub observed: Option<String>,
    pub outcome: Outcome,
}

/// Capacity of the result channel back to the main loop
const RESULT_CHANNEL_CAPACITY: usize = 8;

/// Start the worker task
///
/// Returns the job sender (send `Some(job)`; the worker runs the latest value) and the
/// result receiver. The worker stops when the job sender is dropped.
pub(crate) fn spawn<S: SinkSwitcher>(
    switcher: Arc<S>,
) -> (watch::Sender<Option<Job>>, mpsc::Receiver<JobResult>) {
    let (job_tx, job_rx) = watch::channel(None);
    let (result_tx, result_rx) = mpsc::channel(RESULT_CHANNEL_CAPACITY);
    tokio::spawn(run(switcher, job_rx, result_tx));
    (job_tx, result_rx)
}

/// Run one job (blocking): pick the sink if needed, then activate it
fn execute<S: SinkSwitcher>(switcher: &S, generation: u64, target: Target) -> JobResult {
    let (sink, observed) = match target {
        Target::Sink(sink) => (sink, None),
        Target::Manual { request, config } => {
            let probe = match switcher.probe(&config) {
                Ok(probe) => probe,
                Err(e) => {
                    return JobResult {
                        generation,
                        sink: None,
                        observed: None,
                        outcome: Outcome::Failed(format!(
                            "Cannot read the current audio state: {e:#}"
                        )),
                    };
                }
            };
            let observed = probe.current.clone();
            match manual::plan(&config, &request, &probe) {
                Ok(ManualTarget::Switch(sink)) => (sink.name.clone(), observed),
                Ok(ManualTarget::Done(message)) => {
                    return JobResult {
                        generation,
                        sink: None,
                        observed,
                        outcome: Outcome::Done(message),
                    };
                }
                Err(message) => {
                    return JobResult {
                        generation,
                        sink: None,
                        observed,
                        outcome: Outcome::Failed(message),
                    };
                }
            }
        }
    };
    let outcome = match switcher.activate(&sink) {
        Ok(node) => Outcome::Activated(node),
        Err(e) => Outcome::Failed(format!("{e:#}")),
    };
    JobResult {
        generation,
        sink: Some(sink),
        observed,
        outcome,
    }
}

async fn run<S: SinkSwitcher>(
    switcher: Arc<S>,
    mut jobs: watch::Receiver<Option<Job>>,
    results: mpsc::Sender<JobResult>,
) {
    while jobs.changed().await.is_ok() {
        let Some(job) = jobs.borrow_and_update().clone() else {
            continue;
        };
        match &job.target {
            Target::Sink(sink) => debug!("Activating {sink} (job {})", job.generation),
            Target::Manual { request, .. } => {
                debug!("Manual request {request:?} (job {})", job.generation);
            }
        }

        let switcher = Arc::clone(&switcher);
        let generation = job.generation;
        let fallback_sink = match &job.target {
            Target::Sink(sink) => Some(sink.clone()),
            Target::Manual { .. } => None,
        };
        let result = tokio::task::spawn_blocking(move || {
            execute(switcher.as_ref(), job.generation, job.target)
        })
        .await
        .unwrap_or_else(|e| JobResult {
            generation,
            sink: fallback_sink,
            observed: None,
            outcome: Outcome::Failed(format!("activation task failed: {e}")),
        });
        if results.send(result).await.is_err() {
            break;
        }
    }
}
