//! `PipeWire` integration
//!
//! Provides audio sink and source discovery and control via `PipeWire` native tools:
//! - `pw-dump`: JSON snapshot of devices, nodes and metadata
//! - `pw-metadata`: setting the configured default sink (`default.configured.audio.sink`)
//!   and source (`default.configured.audio.source`)
//! - `pw-cli`: profile switching for ALSA cards (saved, so `WirePlumber` keeps it)
//!
//! Supports both ALSA and Bluetooth audio sinks and sources. ALSA cards support
//! profile switching (e.g., analog ↔ digital); Bluetooth nodes are listed while
//! connected. A job that wants a sink and a source on the same card switches it once.
//!
//! Layout:
//! - `model`: typed [`Snapshot`] of the graph
//! - `parse`: `pw-dump` JSON → [`Snapshot`] (pure)
//! - `resolve`: which sinks a profile switch can reach, and how (pure)
//! - `activate`: resolve + switch + wait + set default, over an `AudioBackend`
//! - `cli`: the only code that spawns processes (deadlines, output caps, exit checks)
//!
//! All required tools must be present in `PATH` for `PWSW` to function.

use std::collections::HashSet;
use std::sync::{Mutex, PoisonError};

use anyhow::Result;
use serde::Serialize;
use tracing::{debug, info};

mod activate;
mod cli;
mod model;
mod parse;
mod resolve;

use activate::{Request, SystemClock, WaitPolicy};
use cli::CliBackend;
pub use model::Snapshot;

use crate::config::Endpoint;

// ============================================================================
// Sink Discovery Types
// ============================================================================

/// A sink (or source) currently available in `PipeWire`
#[derive(Debug, Clone)]
pub struct ActiveSink {
    pub name: String,
    pub description: String,
    pub is_default: bool,
}

/// A sink (or source) that requires profile switching to become available
#[derive(Debug)]
pub struct ProfileSink {
    /// Predicted node name (based on device name + profile)
    pub predicted_name: String,
    /// Description from profile
    pub description: String,
    /// Device ID that owns this profile
    pub device_id: u32,
    /// Device name
    pub device_name: String,
    /// Profile index to switch to
    pub profile_index: u32,
    /// Profile name
    pub profile_name: String,
}

/// How a configured sink can be made the default right now (see
/// [`PipeWire::reachability`])
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reachability {
    /// The sink node exists, under this node name (it can differ from the configured
    /// one when the node was found by device identity)
    Present { node_name: String },
    /// Switching the card `device_name` to profile `profile_name` provides it
    ProfileSwitch {
        device_name: String,
        profile_name: String,
    },
}

/// What a `pw-dump` says about the `PipeWire` server and its clients
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GraphInfo {
    /// The server's version (core object), e.g. `1.2.7`
    pub server_version: Option<String>,
    /// `application.name` of the connected session manager client, if it is
    /// `WirePlumber`
    pub session_manager: Option<String>,
}

// ============================================================================
// JSON Output Structures (for --list-sinks --json)
// ============================================================================

#[derive(Debug, Serialize)]
pub struct ListSinksJson {
    pub active_sinks: Vec<ActiveSinkJson>,
    pub profile_sinks: Vec<ProfileSinkJson>,
    pub configured_sinks: Vec<ConfiguredSinkJson>,
    pub current_default: Option<String>,
    /// Input devices (`Audio/Source` nodes, monitors excluded), same shape as sinks
    pub active_sources: Vec<ActiveSinkJson>,
    /// Sources reachable by a profile switch that keeps the card's output
    pub profile_sources: Vec<ProfileSinkJson>,
    /// `[[sources]]` from the config
    pub configured_sources: Vec<ConfiguredSinkJson>,
    /// The default source as the user sees it
    pub current_default_source: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ActiveSinkJson {
    pub name: String,
    pub description: String,
    pub is_default: bool,
    pub configured: bool,
}

#[derive(Debug, Serialize)]
pub struct ProfileSinkJson {
    pub predicted_name: String,
    pub description: String,
    pub device_name: String,
    pub profile_name: String,
    pub profile_index: u32,
}

#[derive(Debug, Serialize)]
pub struct ConfiguredSinkJson {
    pub index: usize,
    pub name: String,
    pub desc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    pub is_default_config: bool,
    pub status: String,
}

// ============================================================================
// PipeWire Interface
// ============================================================================

/// `PipeWire` interface for audio control
pub struct PipeWire;

/// What one activation half did: the index of the candidate made default and its
/// node name, `None` when an upgrade check (or a [`JobHalf::keep`] half) changed
/// nothing
pub type Activated = Option<(usize, String)>;

/// One half (sink or source) of an activation job ([`PipeWire::activate_joint`])
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JobHalf<'a> {
    /// Configured node names, in priority order
    pub candidates: &'a [String],
    /// Candidate `k` is the default already: only a reachable earlier one is worth a
    /// switch (an upgrade check)
    pub upgrade_from: Option<usize>,
    /// Source halves only: ride along with the sink half so its profile switch keeps
    /// the microphone. Acted on only when the sink's planned switch is on the card of
    /// one of `candidates` (then planned jointly: one profile for both, or this half
    /// fails when none provides both); otherwise nothing is touched (`Ok(None)`).
    pub keep: bool,
}

impl<'a> JobHalf<'a> {
    /// A plain half: activate the first reachable of `candidates`
    #[must_use]
    pub const fn new(candidates: &'a [String]) -> Self {
        Self {
            candidates,
            upgrade_from: None,
            keep: false,
        }
    }
}

/// Serialises all sink and source activations in this process, so each one takes its snapshot
/// and makes its decision with no other switch in flight. Guards no data, so a
/// poisoned lock (a panicking activation) is simply taken over.
static ACTIVATION_LOCK: Mutex<()> = Mutex::new(());

impl PipeWire {
    /// Validate that all required `PipeWire` tools are available in `PATH`
    ///
    /// Checks for: `pw-dump`, `pw-metadata`, `pw-cli` (each run with `--version`
    /// under a deadline).
    ///
    /// # Errors
    /// Returns an error with installation instructions if any tools are missing.
    pub fn validate_tools() -> Result<()> {
        let missing: Vec<&str> = ["pw-dump", "pw-metadata", "pw-cli"]
            .into_iter()
            .filter(|tool| !cli::tool_works(tool))
            .collect();

        if !missing.is_empty() {
            anyhow::bail!(
                "Missing required PipeWire tools: {}\n\
                 \n\
                 Please install the PipeWire utilities package for your distribution:\n\
                 - Arch/Manjaro: pacman -S pipewire-tools\n\
                 - Fedora: dnf install pipewire-utils\n\
                 - Debian/Ubuntu: apt install pipewire-bin\n\
                 - openSUSE: zypper install pipewire-tools",
                missing.join(", ")
            );
        }

        Ok(())
    }

    /// Take a snapshot of the `PipeWire` graph via `pw-dump`
    ///
    /// # Errors
    /// Returns an error if `pw-dump` cannot be run, times out, fails, or returns
    /// invalid `JSON`.
    pub fn dump() -> Result<Snapshot> {
        activate::AudioBackend::snapshot(&CliBackend::SYSTEM)
    }

    /// Like [`Self::dump`], also returning what the dump says about the server and
    /// its clients (one `pw-dump`)
    ///
    /// # Errors
    /// Returns an error if `pw-dump` cannot be run, times out, fails, or returns
    /// invalid `JSON`.
    pub fn dump_with_info() -> Result<(Snapshot, GraphInfo)> {
        parse::parse_dump_with_info(&cli::dump_bytes()?)
    }

    /// The output of `tool --version` (run under the command deadline)
    ///
    /// # Errors
    /// Returns an error if the tool cannot be run, does not finish in time, or fails.
    pub fn tool_version(tool: &str) -> Result<String> {
        cli::tool_version(tool)
    }

    /// Get currently active audio sinks from a snapshot
    #[must_use]
    pub fn get_active_sinks(snapshot: &Snapshot) -> Vec<ActiveSink> {
        Self::get_active(snapshot, Endpoint::Sink)
    }

    /// Get currently active sinks or sources (monitors excluded) from a snapshot
    #[must_use]
    pub fn get_active(snapshot: &Snapshot, kind: Endpoint) -> Vec<ActiveSink> {
        let default_name = snapshot.current_default_of(kind);
        snapshot
            .nodes(kind)
            .iter()
            .map(|node| ActiveSink {
                name: node.name.clone(),
                description: node.description.clone(),
                is_default: default_name == Some(node.name.as_str()),
            })
            .collect()
    }

    /// Get sinks available through profile switching
    ///
    /// One entry per output of an ALSA card that has no sink node yet, carrying the
    /// predicted node name and the profile activation would switch to. Bluetooth
    /// sinks appear in [`Self::get_active_sinks`] while connected; `UCM` and
    /// `pro-audio` profiles are not offered.
    #[must_use]
    pub fn get_profile_sinks(snapshot: &Snapshot, active_sinks: &[ActiveSink]) -> Vec<ProfileSink> {
        let active_names: HashSet<&str> = active_sinks.iter().map(|s| s.name.as_str()).collect();
        resolve::list_profile_sinks(snapshot, &active_names)
    }

    /// Get sources (microphones) available through profile switching: one entry per
    /// input of an ALSA card that has no source node yet, with the profile activation
    /// would switch to (keeping the card's current output)
    #[must_use]
    pub fn get_profile_sources(
        snapshot: &Snapshot,
        active_sources: &[ActiveSink],
    ) -> Vec<ProfileSink> {
        let active_names: HashSet<&str> = active_sources.iter().map(|s| s.name.as_str()).collect();
        resolve::list_profile_nodes(snapshot, Endpoint::Source, &active_names)
    }

    /// The current default sink and source as the user sees them (one `pw-dump`;
    /// see [`Snapshot::current_default`])
    ///
    /// # Errors
    /// Returns an error if `pw-dump` fails.
    pub fn get_defaults() -> Result<(Option<String>, Option<String>)> {
        let snapshot = Self::dump()?;
        Ok((
            snapshot
                .current_default_of(Endpoint::Sink)
                .map(String::from),
            snapshot
                .current_default_of(Endpoint::Source)
                .map(String::from),
        ))
    }

    /// Check whether a configured sink could be made the default right now
    ///
    /// Decided by the same resolver activation uses: the node exists, or an available
    /// profile of a matching ALSA card provides it. Nothing is changed.
    ///
    /// # Errors
    /// Returns why the sink cannot be activated (not found, or every profile
    /// providing it is marked unavailable).
    pub fn check_reachable(snapshot: &Snapshot, sink_name: &str) -> Result<()> {
        Self::check_reachable_endpoint(snapshot, Endpoint::Sink, sink_name)
    }

    /// Like [`Self::check_reachable`], saying how the sink would be activated
    ///
    /// # Errors
    /// Returns why the sink cannot be activated (see [`Self::check_reachable`]).
    pub fn reachability(snapshot: &Snapshot, sink_name: &str) -> Result<Reachability> {
        Self::reachability_of(snapshot, Endpoint::Sink, sink_name)
    }

    /// Like [`Self::reachability`], for a configured sink or source
    ///
    /// # Errors
    /// Returns why the node cannot be activated (see [`Self::check_reachable`]).
    pub fn reachability_of(
        snapshot: &Snapshot,
        kind: Endpoint,
        name: &str,
    ) -> Result<Reachability> {
        Ok(match resolve::resolve_endpoint(snapshot, kind, name)? {
            resolve::Plan::SetDefault { node_name } => Reachability::Present { node_name },
            resolve::Plan::SwitchProfile(switch) => Reachability::ProfileSwitch {
                device_name: switch.device_name,
                profile_name: switch.profile_name,
            },
        })
    }

    /// Check whether a configured sink or source could be made the default right now
    /// (see [`Self::check_reachable`])
    ///
    /// # Errors
    /// Returns why the node cannot be activated.
    pub fn check_reachable_endpoint(snapshot: &Snapshot, kind: Endpoint, name: &str) -> Result<()> {
        resolve::resolve_endpoint(snapshot, kind, name).map(|_| ())
    }

    /// Activate a sink or source by its configured node name, switching profiles if
    /// necessary
    ///
    /// Activations are serialised process-wide; the snapshot and the decision are
    /// taken while holding the lock. After a profile switch, waits (bounded by
    /// `PROFILE_SWITCH_DELAY_MS` × `PROFILE_SWITCH_MAX_RETRIES`, at most 5 s) for the
    /// new node, then sets it as the configured default.
    ///
    /// Returns the node name that was made the default (it can differ from `name`
    /// when the node was resolved by device identity).
    ///
    /// # Errors
    /// Returns an error if the node is not found, a `PipeWire` tool fails or times
    /// out, or the node does not appear after the profile switch.
    pub fn activate(kind: Endpoint, name: &str) -> Result<String> {
        let candidates = [name.to_string()];
        let request = Some(JobHalf::new(&candidates));
        let (sink, source) = match kind {
            Endpoint::Sink => Self::activate_joint(request, None),
            Endpoint::Source => Self::activate_joint(None, request),
        };
        sink.or(source)
            .unwrap_or_else(|| Err(anyhow::anyhow!("Internal error: no result")))?
            .map(|(_, node_name)| node_name)
            .ok_or_else(|| anyhow::anyhow!("Internal error: nothing activated"))
    }

    /// Activate a job: the first reachable of the sink candidates and the first
    /// reachable of the source candidates (configured node names in priority order;
    /// `None` leaves that half alone), resolved together against one snapshot
    ///
    /// Each half falls through to its next reachable candidate if an activation
    /// fails. When both halves need the same card, the card is switched once, to a
    /// profile providing both; if none does, the sink wins and the source half
    /// reports the source as unreachable. With `upgrade_from: Some(k)` (candidate `k`
    /// is that half's current default), the half does nothing unless an earlier
    /// candidate is reachable now. A source half with `keep` does nothing unless the
    /// sink's profile switch is on the card of one of its candidates (see
    /// [`JobHalf::keep`]).
    ///
    /// Returns each half's result: the index of the activated candidate and the node
    /// made default, `Ok(None)` when an upgrade check found nothing better, or why no
    /// candidate could be activated (see [`PipeWire::activate`] for the causes).
    #[must_use]
    pub fn activate_joint(
        sink: Option<JobHalf<'_>>,
        source: Option<JobHalf<'_>>,
    ) -> (Option<Result<Activated>>, Option<Result<Activated>>) {
        fn request(half: JobHalf<'_>) -> Request<'_> {
            Request {
                candidates: half.candidates,
                upgrade_from: half.upgrade_from,
                keep: half.keep,
            }
        }
        let _guard = ACTIVATION_LOCK
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let outcome = activate::activate_joint(
            &CliBackend::SYSTEM,
            &SystemClock,
            WaitPolicy::from_env(),
            sink.map(request),
            source.map(request),
        );
        let report = |kind: Endpoint, result: Option<Result<Option<activate::Chosen>>>| {
            result.map(|r| {
                r.map(|chosen| {
                    chosen.map(|chosen| {
                        let activation = chosen.activation;
                        if let Some(switch) = &activation.profile_switch {
                            info!(
                                "Switched {} to profile {} and set default {} {}",
                                switch.device_name,
                                switch.profile_name,
                                kind.noun(),
                                activation.node_name
                            );
                        } else {
                            debug!("Set default {} {}", kind.noun(), activation.node_name);
                        }
                        (chosen.index, activation.node_name)
                    })
                })
            })
        };
        (
            report(Endpoint::Sink, outcome.sink),
            report(Endpoint::Source, outcome.source),
        )
    }
}

/// Parse `pw-dump` output (for tests outside this module)
#[cfg(test)]
pub(crate) fn parse_snapshot(json: &[u8]) -> Result<Snapshot> {
    parse::parse_dump(json)
}
