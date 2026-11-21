//! `PipeWire` integration
//!
//! Provides audio sink discovery and control via `PipeWire` native tools:
//! - `pw-dump`: JSON snapshot of devices, nodes and metadata
//! - `pw-metadata`: setting the configured default sink (`default.configured.audio.sink`)
//! - `pw-cli`: profile switching for ALSA cards (saved, so `WirePlumber` keeps it)
//!
//! Supports both ALSA and Bluetooth audio sinks. ALSA cards support profile
//! switching (e.g., analog ↔ digital); Bluetooth sinks are listed while connected.
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

use color_eyre::eyre::{self, Result};
use serde::Serialize;
use tracing::{debug, info};

mod activate;
mod cli;
mod model;
mod parse;
mod resolve;

use activate::{SystemClock, WaitPolicy};
use cli::CliBackend;
pub use model::Snapshot;

// ============================================================================
// Sink Discovery Types
// ============================================================================

/// A sink currently available in `PipeWire`
#[derive(Debug, Clone)]
pub struct ActiveSink {
    pub name: String,
    pub description: String,
    pub is_default: bool,
}

/// A sink that requires profile switching to become available
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

// ============================================================================
// JSON Output Structures (for --list-sinks --json)
// ============================================================================

#[derive(Debug, Serialize)]
pub struct ListSinksJson {
    pub active_sinks: Vec<ActiveSinkJson>,
    pub profile_sinks: Vec<ProfileSinkJson>,
    pub configured_sinks: Vec<ConfiguredSinkJson>,
    pub current_default: Option<String>,
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

/// Serialises all sink activations in this process, so each one takes its snapshot
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
            eyre::bail!(
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
        activate::AudioBackend::snapshot(&CliBackend)
    }

    /// Get currently active audio sinks from a snapshot
    #[must_use]
    pub fn get_active_sinks(snapshot: &Snapshot) -> Vec<ActiveSink> {
        let default_name = snapshot.current_default();
        snapshot
            .sinks
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

    /// Get the current default sink name (fresh query)
    ///
    /// Prefers `default.configured.audio.sink` when that sink exists, falling back
    /// to the effective `default.audio.sink`.
    ///
    /// # Errors
    /// Returns an error if `pw-dump` fails or no default sink is set in metadata.
    pub fn get_default_sink_name() -> Result<String> {
        let snapshot = Self::dump()?;
        snapshot
            .current_default()
            .map(String::from)
            .ok_or_else(|| eyre::eyre!("No default sink found in PipeWire metadata"))
    }

    /// Activate a sink by its configured node name, switching profiles if necessary
    ///
    /// Activations are serialised process-wide; the snapshot and the decision are
    /// taken while holding the lock. After a profile switch, waits (bounded by
    /// `PROFILE_SWITCH_DELAY_MS` × `PROFILE_SWITCH_MAX_RETRIES`, at most 5 s) for the
    /// new sink node, then sets it as the configured default.
    ///
    /// Returns the node name that was made the default (it can differ from
    /// `sink_name` when the sink was resolved by device identity).
    ///
    /// # Errors
    /// Returns an error if the sink is not found, a `PipeWire` tool fails or times
    /// out, or the sink node does not appear after the profile switch.
    pub fn activate_sink(sink_name: &str) -> Result<String> {
        let _guard = ACTIVATION_LOCK
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let activation =
            activate::activate(&CliBackend, &SystemClock, WaitPolicy::from_env(), sink_name)?;
        if let Some(switch) = &activation.profile_switch {
            info!(
                "Switched {} to profile {} and set default sink {}",
                switch.device_name, switch.profile_name, activation.node_name
            );
        } else {
            debug!("Set default sink {}", activation.node_name);
        }
        Ok(activation.node_name)
    }
}
