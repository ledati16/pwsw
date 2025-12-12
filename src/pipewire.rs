//! PipeWire integration
//!
//! Provides audio sink discovery and control via PipeWire native tools:
//! - `pw-dump`: JSON queries for objects (sinks, devices, metadata)
//! - `pw-metadata`: Setting the default audio sink
//! - `pw-cli`: Profile switching for analog/digital outputs
//!
//! All required tools must be present in PATH for PWSW to function.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::process::Command;
use std::time::Duration;
use tracing::{debug, info, trace};

// ============================================================================
// Constants
// ============================================================================

/// Time to wait for a new sink node to appear after profile switch
const PROFILE_SWITCH_DELAY_MS: u64 = 150;

/// Maximum retries when waiting for sink after profile switch
const PROFILE_SWITCH_MAX_RETRIES: u32 = 5;

// ============================================================================
// PipeWire JSON Structures (from pw-dump)
// ============================================================================

/// Top-level PipeWire object from pw-dump output
#[derive(Debug, Deserialize)]
pub struct PwObject {
    pub id: u32,
    #[serde(rename = "type")]
    pub obj_type: String,
    #[serde(default)]
    pub info: Option<PwInfo>,
    #[serde(default)]
    pub props: Option<PwProps>,
    #[serde(default)]
    pub metadata: Option<Vec<PwMetadataEntry>>,
}

impl PwObject {
    /// Get props from either info.props or top-level props (metadata objects use top-level)
    pub fn get_props(&self) -> Option<&PwProps> {
        self.info
            .as_ref()
            .and_then(|i| i.props.as_ref())
            .or(self.props.as_ref())
    }
}

#[derive(Debug, Deserialize)]
pub struct PwInfo {
    #[serde(default)]
    pub props: Option<PwProps>,
    #[serde(default)]
    pub params: Option<PwParams>,
}

/// PipeWire object properties - uses permissive deserialization
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct PwProps {
    #[serde(rename = "node.name")]
    pub node_name: Option<String>,
    #[serde(rename = "node.description")]
    pub node_description: Option<String>,
    #[serde(rename = "node.nick")]
    pub node_nick: Option<String>,
    #[serde(rename = "media.class")]
    pub media_class: Option<String>,
    #[serde(rename = "metadata.name")]
    pub metadata_name: Option<String>,
    #[serde(rename = "device.name")]
    pub device_name: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct PwParams {
    #[serde(rename = "EnumProfile")]
    pub enum_profile: Option<Vec<PwProfile>>,
    #[serde(rename = "Profile")]
    pub profile: Option<Vec<PwProfile>>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct PwProfile {
    pub index: Option<u32>,
    pub name: Option<String>,
    pub description: Option<String>,
    pub available: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct PwMetadataEntry {
    pub key: String,
    #[serde(default)]
    pub value: Option<serde_json::Value>,
}

impl PwMetadataEntry {
    /// Extract sink name from metadata value (handles multiple formats)
    pub fn get_name(&self) -> Option<String> {
        let value = self.value.as_ref()?;
        // Try object with "name" field first
        if let Some(obj) = value.as_object() {
            if let Some(name_val) = obj.get("name") {
                return name_val.as_str().map(String::from);
            }
        }
        // Fall back to plain string
        value.as_str().map(String::from)
    }
}

// ============================================================================
// Sink Discovery Types
// ============================================================================

/// A sink currently available in PipeWire
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

/// JSON output for --get-sink --json
#[derive(Debug, Serialize)]
pub struct SinkInfoJson {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    pub icon: String,
}

// ============================================================================
// PipeWire Interface
// ============================================================================

/// PipeWire interface for audio control
pub struct PipeWire;

impl PipeWire {
    /// Validate that all required PipeWire tools are available in PATH
    ///
    /// Checks for: pw-dump, pw-metadata, pw-cli
    /// Returns an error with installation instructions if any are missing.
    pub fn validate_tools() -> Result<()> {
        let required_tools = ["pw-dump", "pw-metadata", "pw-cli"];
        let mut missing = Vec::new();

        for tool in &required_tools {
            // Try to run the tool with --version or --help to check if it exists
            let result = Command::new(tool)
                .arg("--version")
                .output();

            if result.is_err() {
                missing.push(*tool);
            }
        }

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

    /// Get all PipeWire objects via pw-dump
    pub fn dump() -> Result<Vec<PwObject>> {
        let output = Command::new("pw-dump")
            .output()
            .context("Failed to run pw-dump. Is PipeWire running?")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("pw-dump failed: {}", stderr.trim());
        }

        let objects: Vec<PwObject> = serde_json::from_slice(&output.stdout)
            .context("Failed to parse pw-dump JSON")?;

        trace!("pw-dump returned {} objects", objects.len());
        Ok(objects)
    }

    /// Get currently active audio sinks from PipeWire objects
    pub fn get_active_sinks(objects: &[PwObject]) -> Vec<ActiveSink> {
        let default_name = Self::get_default_sink_name_from_objects(objects);

        objects.iter()
            .filter(|obj| obj.obj_type == "PipeWire:Interface:Node")
            .filter_map(|obj| {
                let props = obj.get_props()?;

                // Only Audio/Sink nodes
                if props.media_class.as_deref() != Some("Audio/Sink") {
                    return None;
                }

                let name = props.node_name.clone()?;
                let description = props.node_description.clone()
                    .or_else(|| props.node_nick.clone())
                    .unwrap_or_else(|| name.clone());

                Some(ActiveSink {
                    name: name.clone(),
                    description,
                    is_default: default_name.as_ref() == Some(&name),
                })
            })
            .collect()
    }

    /// Get sinks available through profile switching
    pub fn get_profile_sinks(objects: &[PwObject], active_sinks: &[ActiveSink]) -> Vec<ProfileSink> {
        let active_names: HashSet<&str> = active_sinks.iter()
            .map(|s| s.name.as_str())
            .collect();

        let mut profile_sinks = Vec::new();

        for obj in objects {
            if obj.obj_type != "PipeWire:Interface:Device" {
                continue;
            }

            let Some(props) = obj.get_props() else { continue };
            let Some(info) = &obj.info else { continue };
            let Some(params) = &info.params else { continue };
            let Some(enum_profiles) = &params.enum_profile else { continue };

            // Only ALSA audio devices
            let device_name = match &props.device_name {
                Some(name) if name.starts_with("alsa_card.") => name,
                _ => continue,
            };

            // Get current profile to skip it
            let current_profile_index = params.profile.as_ref()
                .and_then(|p| p.first())
                .and_then(|p| p.index);

            for profile in enum_profiles {
                let Some(index) = profile.index else { continue };
                let Some(ref profile_name) = profile.name else { continue };

                // Skip "off" profile and currently active profile
                if profile_name == "off" || Some(index) == current_profile_index {
                    continue;
                }

                // Skip unavailable profiles
                if profile.available.as_deref() == Some("no") {
                    continue;
                }

                // Only output profiles (stereo, surround, etc.)
                let is_output = profile_name.contains("output:")
                    || profile_name.ends_with("-stereo")
                    || profile_name.ends_with("-surround-40")
                    || profile_name.ends_with("-surround-51")
                    || profile_name.ends_with("-surround-71");

                if !is_output {
                    continue;
                }

                // Predict node name: alsa_output.{device_suffix}.{profile_suffix}
                let device_suffix = device_name.strip_prefix("alsa_card.").unwrap_or(device_name);
                let profile_suffix = profile_name
                    .strip_prefix("output:")
                    .unwrap_or(profile_name)
                    .replace("+input:", "-");

                let predicted_name = format!("alsa_output.{}.{}", device_suffix, profile_suffix);

                // Skip if already active
                if active_names.contains(predicted_name.as_str()) {
                    continue;
                }

                let description = profile.description.clone()
                    .unwrap_or_else(|| profile_name.clone());

                profile_sinks.push(ProfileSink {
                    predicted_name,
                    description,
                    device_id: obj.id,
                    device_name: device_name.clone(),
                    profile_index: index,
                    profile_name: profile_name.clone(),
                });
            }
        }

        profile_sinks
    }

    /// Extract default sink name from metadata objects
    pub fn get_default_sink_name_from_objects(objects: &[PwObject]) -> Option<String> {
        for obj in objects {
            if obj.obj_type != "PipeWire:Interface:Metadata" {
                continue;
            }

            let Some(props) = obj.get_props() else { continue };
            if props.metadata_name.as_deref() != Some("default") {
                continue;
            }

            if let Some(metadata) = &obj.metadata {
                for entry in metadata {
                    if entry.key == "default.audio.sink" {
                        return entry.get_name();
                    }
                }
            }
        }
        None
    }

    /// Get current default sink name (fresh query)
    pub fn get_default_sink_name() -> Result<String> {
        let objects = Self::dump()?;
        Self::get_default_sink_name_from_objects(&objects)
            .ok_or_else(|| anyhow::anyhow!("No default sink found in PipeWire metadata"))
    }

    /// Set the default audio sink via pw-metadata
    pub fn set_default_sink(node_name: &str) -> Result<()> {
        // Use proper JSON serialization to avoid injection risks
        let value_obj = serde_json::json!({"name": node_name});
        let value = serde_json::to_string(&value_obj)
            .context("Failed to serialize sink name to JSON")?;

        let output = Command::new("pw-metadata")
            .args(["0", "default.audio.sink", &value, "Spa:String:JSON"])
            .output()
            .with_context(|| format!("Failed to run pw-metadata to set default sink to '{}'", node_name))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Failed to set default sink to '{}': {}", node_name, stderr.trim());
        }

        debug!("Set default sink: {}", node_name);
        Ok(())
    }

    /// Switch device profile via pw-cli
    pub fn set_device_profile(device_id: u32, profile_index: u32) -> Result<()> {
        let profile_json = format!("{{ index: {} }}", profile_index);

        let output = Command::new("pw-cli")
            .args(["s", &device_id.to_string(), "Profile", &profile_json])
            .output()
            .with_context(|| format!("Failed to run pw-cli to set device {} profile {}", device_id, profile_index))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Failed to set device {} to profile {}: {}", device_id, profile_index, stderr.trim());
        }

        debug!("Set device {} to profile {}", device_id, profile_index);
        Ok(())
    }

    /// Find profile sink info if sink requires profile switching
    pub fn find_profile_sink(objects: &[PwObject], sink_name: &str) -> Option<ProfileSink> {
        let active = Self::get_active_sinks(objects);
        let profile_sinks = Self::get_profile_sinks(objects, &active);
        profile_sinks.into_iter().find(|s| s.predicted_name == sink_name)
    }

    /// Activate a sink, switching profiles if necessary
    pub fn activate_sink(sink_name: &str) -> Result<()> {
        let objects = Self::dump()?;

        // Check if sink is already active
        let active = Self::get_active_sinks(&objects);
        if active.iter().any(|s| s.name == sink_name) {
            return Self::set_default_sink(sink_name);
        }

        // Need profile switching?
        let profile_sink = Self::find_profile_sink(&objects, sink_name)
            .ok_or_else(|| anyhow::anyhow!(
                "Sink '{}' not found (not active and no profile switch available)",
                sink_name
            ))?;

        info!(
            "Switching profile: {} → {} (device: {})",
            profile_sink.profile_name, sink_name, profile_sink.device_name
        );

        Self::set_device_profile(profile_sink.device_id, profile_sink.profile_index)?;

        // Wait for the new node to appear with retries
        for attempt in 1..=PROFILE_SWITCH_MAX_RETRIES {
            std::thread::sleep(Duration::from_millis(PROFILE_SWITCH_DELAY_MS));

            let objects = Self::dump()?;
            let active = Self::get_active_sinks(&objects);

            if active.iter().any(|s| s.name == sink_name) {
                Self::set_default_sink(sink_name)?;
                return Ok(());
            }

            debug!("Waiting for sink '{}' (attempt {}/{})", sink_name, attempt, PROFILE_SWITCH_MAX_RETRIES);
        }

        // Profile switch succeeded but sink node didn't appear - this is an error
        anyhow::bail!(
            "Profile switched successfully but sink '{}' did not appear after {} attempts.\n\
             \n\
             This may indicate:\n\
             - The device needs more time to initialize (increase PROFILE_SWITCH_DELAY_MS)\n\
             - The predicted node name '{}' is incorrect\n\
             - The audio device has a hardware issue\n\
             \n\
             You can check available sinks with: pwsw list-sinks",
            sink_name,
            PROFILE_SWITCH_MAX_RETRIES,
            sink_name
        )
    }
}
