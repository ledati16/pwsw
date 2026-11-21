//! Sink activation worker
//!
//! One task owns the [`SinkSwitcher`] and runs activations one at a time in
//! `spawn_blocking`, so the daemon's main loop never waits on `PipeWire`. Requests go
//! through a `watch` channel: while an activation runs, newer requests overwrite older
//! ones and only the latest is run next ("latest wins").

use std::sync::Arc;

use color_eyre::eyre::Result;
use tokio::sync::{mpsc, watch};
use tracing::debug;

use crate::pipewire::PipeWire;

/// Something that can make a configured sink the default
pub trait SinkSwitcher: Send + Sync + 'static {
    /// Activate `sink_name` (blocking), returning the node name made default
    ///
    /// # Errors
    /// Returns an error if the sink cannot be activated.
    fn activate(&self, sink_name: &str) -> Result<String>;
}

/// The real switcher: [`PipeWire::activate_sink`]
pub struct PipeWireSwitcher;

impl SinkSwitcher for PipeWireSwitcher {
    fn activate(&self, sink_name: &str) -> Result<String> {
        PipeWire::activate_sink(sink_name)
    }
}

/// An activation request
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Job {
    /// Increases with every submitted job
    pub generation: u64,
    /// Configured sink node name
    pub sink: String,
}

/// The outcome of a job the worker ran
#[derive(Debug)]
pub(crate) struct JobResult {
    pub generation: u64,
    pub sink: String,
    /// The node made default, or the error (formatted with its causes)
    pub outcome: std::result::Result<String, String>,
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

async fn run<S: SinkSwitcher>(
    switcher: Arc<S>,
    mut jobs: watch::Receiver<Option<Job>>,
    results: mpsc::Sender<JobResult>,
) {
    while jobs.changed().await.is_ok() {
        let Some(job) = jobs.borrow_and_update().clone() else {
            continue;
        };
        debug!("Activating {} (job {})", job.sink, job.generation);

        let switcher = Arc::clone(&switcher);
        let sink = job.sink.clone();
        let outcome = match tokio::task::spawn_blocking(move || switcher.activate(&sink)).await {
            Ok(Ok(node)) => Ok(node),
            Ok(Err(e)) => Err(format!("{e:#}")),
            Err(e) => Err(format!("activation task failed: {e}")),
        };

        let result = JobResult {
            generation: job.generation,
            sink: job.sink,
            outcome,
        };
        if results.send(result).await.is_err() {
            break;
        }
    }
}
