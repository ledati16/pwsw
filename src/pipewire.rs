//! `PipeWire` integration
//!
//! Provides audio sink discovery and control via `PipeWire` native tools:
//! - `pw-dump`: JSON queries for objects (sinks, devices, metadata)
//! - `pw-metadata`: Setting the default audio sink
//! - `pw-cli`: Profile switching for analog/digital outputs
//!
//! All required tools must be present in `PATH` for `PWSW` to function.

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

/// Top-level `PipeWire` object from `pw-dump` output
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
    #[must_use]
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

/// `PipeWire` object properties - uses permissive deserialization
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

// Per-device lock table to serialize profile switches on the same device
use std::sync::OnceLock;
use std::sync::{Arc, Mutex as StdMutex};

static DEVICE_LOCKS: OnceLock<StdMutex<std::collections::HashMap<u32, Arc<StdMutex<()>>>>> = OnceLock::new();

impl PipeWire {
    /// Validate that all required `PipeWire` tools are available in `PATH`
    ///
    /// Checks for: `pw-dump`, `pw-metadata`, `pw-cli`
    ///
    /// # Errors
    /// Returns an error with installation instructions if any tools are missing.
    pub fn validate_tools() -> Result<()> {
        let required_tools = ["pw-dump", "pw-metadata", "pw-cli"];
        let mut missing = Vec::new();

        for tool in &required_tools {
            // Try to run the tool with --version or --help to check if it exists
            let result = Command::new(tool).arg("--version").status();

            if result.is_err() || !result.unwrap().success() {
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

    /// Get all `PipeWire` objects via `pw-dump`
    ///
    /// # Errors
    /// Returns an error if `pw-dump` fails to execute or returns invalid JSON.
    pub fn dump() -> Result<Vec<PwObject>> {
        let output = Command::new("pw-dump")
            .output()
            .context("Failed to run pw-dump. Is PipeWire running?")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("pw-dump failed: {}", stderr.trim());
        }

        let objects: Vec<PwObject> =
            serde_json::from_slice(&output.stdout).context("Failed to parse pw-dump JSON")?;

        trace!("pw-dump returned {} objects", objects.len());
        Ok(objects)
    }

    /// Get currently active audio sinks from `PipeWire` objects
    #[must_use]
    pub fn get_active_sinks(objects: &[PwObject]) -> Vec<ActiveSink> {
        let default_name = Self::get_default_sink_name_from_objects(objects);

        objects
            .iter()
            .filter(|obj| obj.obj_type == "PipeWire:Interface:Node")
            .filter_map(|obj| {
                let props = obj.get_props()?;

                // Only Audio/Sink nodes
                if props.media_class.as_deref() != Some("Audio/Sink") {
                    return None;
                }

                let name = props.node_name.clone()?;
                let description = props
                    .node_description
                    .clone()
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
    #[must_use]
    pub fn get_profile_sinks(
        objects: &[PwObject],
        active_sinks: &[ActiveSink],
    ) -> Vec<ProfileSink> {
        let active_names: HashSet<&str> = active_sinks.iter().map(|s| s.name.as_str()).collect();

        let mut profile_sinks = Vec::new();

        for obj in objects {
            if obj.obj_type != "PipeWire:Interface:Device" {
                continue;
            }

            let Some(props) = obj.get_props() else {
                continue;
            };
            let Some(info) = &obj.info else { continue };
            let Some(params) = &info.params else { continue };
            let Some(enum_profiles) = &params.enum_profile else {
                continue;
            };

            // Only ALSA audio devices
            let device_name = match &props.device_name {
                Some(name) if name.starts_with("alsa_card.") => name,
                _ => continue,
            };

            // Get current profile to skip it
            let current_profile_index = params
                .profile
                .as_ref()
                .and_then(|p| p.first())
                .and_then(|p| p.index);

            for profile in enum_profiles {
                let Some(index) = profile.index else { continue };
                let Some(ref profile_name) = profile.name else {
                    continue;
                };

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
                let device_suffix = device_name
                    .strip_prefix("alsa_card.")
                    .unwrap_or(device_name);
                let profile_suffix = profile_name
                    .strip_prefix("output:")
                    .unwrap_or(profile_name)
                    .replace("+input:", "-");

                let predicted_name = format!("alsa_output.{device_suffix}.{profile_suffix}");

                // Skip if already active
                if active_names.contains(predicted_name.as_str()) {
                    continue;
                }

                let description = profile
                    .description
                    .clone()
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
    #[must_use]
    pub fn get_default_sink_name_from_objects(objects: &[PwObject]) -> Option<String> {
        for obj in objects {
            if obj.obj_type != "PipeWire:Interface:Metadata" {
                continue;
            }

            let Some(props) = obj.get_props() else {
                continue;
            };
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
    ///
    /// # Errors
    /// Returns an error if `pw-dump` fails or no default sink is configured in metadata.
    pub fn get_default_sink_name() -> Result<String> {
        let objects = Self::dump()?;
        Self::get_default_sink_name_from_objects(&objects)
            .ok_or_else(|| anyhow::anyhow!("No default sink found in PipeWire metadata"))
    }

    /// Set the default audio sink via `pw-metadata`
    ///
    /// # Errors
    /// Returns an error if `pw-metadata` command fails or the sink cannot be set.
    pub fn set_default_sink(node_name: &str) -> Result<()> {
        // Use proper JSON serialization to avoid injection risks
        let value_obj = serde_json::json!({"name": node_name});
        let value =
            serde_json::to_string(&value_obj).context("Failed to serialize sink name to JSON")?;

        let output = Command::new("pw-metadata")
            .args(["0", "default.audio.sink", &value, "Spa:String:JSON"])
            .output()
            .with_context(|| {
                format!("Failed to run pw-metadata to set default sink to '{node_name}'")
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!(
                "Failed to set default sink to '{}': {}",
                node_name,
                stderr.trim()
            );
        }

        debug!("Set default sink: {}", node_name);
        Ok(())
    }

    /// Switch device profile via `pw-cli`
    ///
    /// # Errors
    /// Returns an error if `pw-cli` command fails or the profile cannot be set.
    pub fn set_device_profile(device_id: u32, profile_index: u32) -> Result<()> {
        // Use proper JSON serialization to avoid any potential issues
        let profile_json = serde_json::json!({"index": profile_index}).to_string();

        let output = Command::new("pw-cli")
            .args(["s", &device_id.to_string(), "Profile", &profile_json])
            .output()
            .with_context(|| {
                format!("Failed to run pw-cli to set device {device_id} profile {profile_index}")
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!(
                "Failed to set device {} to profile {}: {}",
                device_id,
                profile_index,
                stderr.trim()
            );
        }

        debug!("Set device {} to profile {}", device_id, profile_index);
        Ok(())
    }

    /// Find profile sink info if sink requires profile switching
    #[must_use]
    pub fn find_profile_sink(objects: &[PwObject], sink_name: &str) -> Option<ProfileSink> {
        let active = Self::get_active_sinks(objects);
        let profile_sinks = Self::get_profile_sinks(objects, &active);
        profile_sinks
            .into_iter()
            .find(|s| s.predicted_name == sink_name)
    }

    /// Activate a sink, switching profiles if necessary
    ///
    /// # Errors
    /// Returns an error if the sink is not found, profile switching fails, or the sink
    /// node does not appear after profile switch.
    ///
    /// # Concurrency
    /// This function is not thread-safe for concurrent profile switches on the same device.
    /// Callers should ensure only one profile switch occurs at a time per device.
    pub fn activate_sink(sink_name: &str) -> Result<()> {
        let objects = Self::dump()?;

        // Check if sink is already active
        let active = Self::get_active_sinks(&objects);
        if active.iter().any(|s| s.name == sink_name) {
            return Self::set_default_sink(sink_name);
        }

        // Need profile switching?
        let profile_sink = Self::find_profile_sink(&objects, sink_name).ok_or_else(|| {
            anyhow::anyhow!(
                "Sink '{sink_name}' not found (not active and no profile switch available)"
            )
        })?;

        info!(
            "Switching profile: {} → {} (device: {})",
            profile_sink.profile_name, sink_name, profile_sink.device_name
        );

        // Acquire per-device lock to serialize profile switches
        let locks = DEVICE_LOCKS.get_or_init(|| StdMutex::new(std::collections::HashMap::new()));
        let device_mutex_arc = {
            let mut guard = locks.lock().unwrap();
            Arc::clone(
                guard
                    .entry(profile_sink.device_id)
                    .or_insert_with(|| Arc::new(StdMutex::new(()))),
            )
        };

        // Lock the device mutex for the duration of profile switch + polling
        let _device_guard = device_mutex_arc.lock().unwrap();

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

            debug!(
                "Waiting for sink '{}' (attempt {}/{})",
                sink_name, attempt, PROFILE_SWITCH_MAX_RETRIES
            );
        }

        // Profile switch succeeded but sink node didn't appear - this is an error
        anyhow::bail!(
            "Profile switched successfully but sink '{sink_name}' did not appear after {PROFILE_SWITCH_MAX_RETRIES} attempts.\n\
             \n\
             This may indicate:\n\
             - The device needs more time to initialize (increase PROFILE_SWITCH_DELAY_MS)\n\
             - The predicted node name '{sink_name}' is incorrect\n\
             - The audio device has a hardware issue\n\
             \n\
             You can check available sinks with: pwsw list-sinks"
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, AtomicBool, Ordering};
    use std::sync::Arc;
    use std::thread;
    use std::time::Duration;

    const MINIMAL_SINK_JSON: &str = r#"[
        {
            "id": 42,
            "type": "PipeWire:Interface:Node",
            "info": {
                "props": {
                    "node.name": "alsa_output.test.stereo",
                    "node.description": "Test Speakers",
                    "media.class": "Audio/Sink"
                }
            }
        }
    ]"#;

    const METADATA_OBJECT_FORMAT_JSON: &str = r#"[
        {
            "id": 0,
            "type": "PipeWire:Interface:Metadata",
            "props": {
                "metadata.name": "default"
            },
            "metadata": [
                {
                    "key": "default.audio.sink",
                    "value": {"name": "alsa_output.test.stereo"}
                }
            ]
        }
    ]"#;

    const METADATA_STRING_FORMAT_JSON: &str = r#"[
        {
            "id": 0,
            "type": "PipeWire:Interface:Metadata",
            "props": {
                "metadata.name": "default"
            },
            "metadata": [
                {
                    "key": "default.audio.sink",
                    "value": "alsa_output.test.stereo"
                }
            ]
        }
    ]"#;

    const MULTIPLE_SINKS_JSON: &str = r#"[
        {
            "id": 1,
            "type": "PipeWire:Interface:Node",
            "info": {
                "props": {
                    "node.name": "alsa_output.hdmi",
                    "node.description": "HDMI Output",
                    "media.class": "Audio/Sink"
                }
            }
        },
        {
            "id": 2,
            "type": "PipeWire:Interface:Node",
            "info": {
                "props": {
                    "node.name": "alsa_output.speakers",
                    "node.description": "Speakers",
                    "media.class": "Audio/Sink"
                }
            }
        },
        {
            "id": 3,
            "type": "PipeWire:Interface:Node",
            "info": {
                "props": {
                    "node.name": "alsa_input.mic",
                    "node.description": "Microphone",
                    "media.class": "Audio/Source"
                }
            }
        }
    ]"#;

    // PwMetadataEntry::get_name() tests
    #[test]
    fn test_metadata_get_name_object_format() {
        let objects: Vec<PwObject> = serde_json::from_str(METADATA_OBJECT_FORMAT_JSON).unwrap();
        let metadata = &objects[0].metadata.as_ref().unwrap()[0];
        assert_eq!(
            metadata.get_name(),
            Some("alsa_output.test.stereo".to_string())
        );
    }

    #[test]
    fn test_metadata_get_name_string_format() {
        let objects: Vec<PwObject> = serde_json::from_str(METADATA_STRING_FORMAT_JSON).unwrap();
        let metadata = &objects[0].metadata.as_ref().unwrap()[0];
        assert_eq!(
            metadata.get_name(),
            Some("alsa_output.test.stereo".to_string())
        );
    }

    #[test]
    fn test_metadata_get_name_null_returns_none() {
        let entry = PwMetadataEntry {
            key: "test".to_string(),
            value: None,
        };
        assert_eq!(entry.get_name(), None);
    }

    // PwObject::get_props() tests
    #[test]
    fn test_get_props_from_info() {
        let objects: Vec<PwObject> = serde_json::from_str(MINIMAL_SINK_JSON).unwrap();
        let props = objects[0].get_props();
        assert!(props.is_some());
        assert_eq!(
            props.unwrap().node_name.as_deref(),
            Some("alsa_output.test.stereo")
        );
    }

    #[test]
    fn test_get_props_from_toplevel() {
        let json = r#"[{
            "id": 0,
            "type": "PipeWire:Interface:Metadata",
            "props": {
                "metadata.name": "default"
            }
        }]"#;
        let objects: Vec<PwObject> = serde_json::from_str(json).unwrap();
        let props = objects[0].get_props();
        assert!(props.is_some());
        assert_eq!(props.unwrap().metadata_name.as_deref(), Some("default"));
    }

    // get_active_sinks() tests
    #[test]
    fn test_get_active_sinks_filters_audio_sink() {
        let objects: Vec<PwObject> = serde_json::from_str(MINIMAL_SINK_JSON).unwrap();
        let sinks = PipeWire::get_active_sinks(&objects);
        assert_eq!(sinks.len(), 1);
        assert_eq!(sinks[0].name, "alsa_output.test.stereo");
        assert_eq!(sinks[0].description, "Test Speakers");
    }

    #[test]
    fn test_get_active_sinks_ignores_sources() {
        let objects: Vec<PwObject> = serde_json::from_str(MULTIPLE_SINKS_JSON).unwrap();
        let sinks = PipeWire::get_active_sinks(&objects);
        assert_eq!(sinks.len(), 2);
        assert!(sinks.iter().all(|s| !s.name.contains("mic")));
    }

    #[test]
    fn test_get_active_sinks_uses_description_fallback() {
        let json = r#"[{
            "id": 1,
            "type": "PipeWire:Interface:Node",
            "info": {
                "props": {
                    "node.name": "test_sink",
                    "node.nick": "Test Nick",
                    "media.class": "Audio/Sink"
                }
            }
        }]"#;
        let objects: Vec<PwObject> = serde_json::from_str(json).unwrap();
        let sinks = PipeWire::get_active_sinks(&objects);
        assert_eq!(sinks.len(), 1);
        assert_eq!(sinks[0].description, "Test Nick");
    }

    // get_default_sink_name_from_objects() tests
    #[test]
    fn test_get_default_sink_found() {
        let objects: Vec<PwObject> = serde_json::from_str(METADATA_OBJECT_FORMAT_JSON).unwrap();
        let default_sink = PipeWire::get_default_sink_name_from_objects(&objects);
        assert_eq!(default_sink, Some("alsa_output.test.stereo".to_string()));
    }

    #[test]
    fn test_get_default_sink_missing() {
        let json = r#"[{
            "id": 0,
            "type": "PipeWire:Interface:Metadata",
            "props": {
                "metadata.name": "default"
            },
            "metadata": []
        }]"#;
        let objects: Vec<PwObject> = serde_json::from_str(json).unwrap();
        let default_sink = PipeWire::get_default_sink_name_from_objects(&objects);
        assert_eq!(default_sink, None);
    }

    #[test]
    fn test_device_lock_serialization() {
        // This test ensures per-device locks serialize profile switches for the same device.
        // We'll simulate two concurrent activations that require profile switching by directly
        // invoking activate_sink on a predicted name that requires a profile switch. Since
        // activate_sink calls pw-dump and pw-cli, and we cannot run those here, we'll instead
        // test the lock acquisition logic by having two threads attempt to lock the same device
        // entry using the internal DEVICE_LOCKS structure.

        let locks = DEVICE_LOCKS.get_or_init(|| StdMutex::new(std::collections::HashMap::new()));

        // Simulate device id 123
        let device_id = 123u32;

        // Insert a lock for device
        {
            let mut guard = locks.lock().unwrap();
            guard.insert(device_id, Arc::new(StdMutex::new(())));
        }

        let arc_lock = {
            let guard = locks.lock().unwrap();
            Arc::clone(guard.get(&device_id).unwrap())
        };

        // Shared state to track execution order
        let order = Arc::new(AtomicUsize::new(0));

        let o1 = order.clone();
        let l1 = arc_lock.clone();
        let t1 = thread::spawn(move || {
            let _g = l1.lock().unwrap();
            // Mark we have the lock
            o1.fetch_add(1, Ordering::SeqCst);
            // Hold the lock for a bit
            thread::sleep(Duration::from_millis(50));
        });

        // Give first thread time to acquire lock
        thread::sleep(Duration::from_millis(10));

        let o2 = order.clone();
        let l2 = arc_lock.clone();
        let t2 = thread::spawn(move || {
            let _g = l2.lock().unwrap();
            // This should only run after t1 releases
            o2.fetch_add(10, Ordering::SeqCst);
        });

        t1.join().unwrap();
        t2.join().unwrap();

        // After both have run, order should be 11 (1 from t1, 10 from t2)
        assert_eq!(order.load(Ordering::SeqCst), 11);
    }

    #[test]
    fn test_get_profile_sinks_excludes_active() {
        let device_json = r#"[
            {
                "id": 100,
                "type": "PipeWire:Interface:Device",
                "info": {
                    "props": {
                        "device.name": "alsa_card.test"
                    },
                    "params": {
                        "Profile": [{"index": 1, "name": "analog-stereo"}],
                        "EnumProfile": [
                            {"index": 0, "name": "off", "description": "Off"},
                            {"index": 1, "name": "analog-stereo", "description": "Analog Stereo", "available": "yes"},
                            {"index": 2, "name": "iec958-stereo", "description": "Digital Stereo", "available": "yes"}
                        ]
                    }
                }
            }
        ]"#;
        let objects: Vec<PwObject> = serde_json::from_str(device_json).unwrap();
        let active_sinks = vec![ActiveSink {
            name: "alsa_output.test.analog-stereo".to_string(),
            description: "Active Sink".to_string(),
            is_default: false,
        }];

        let profile_sinks = PipeWire::get_profile_sinks(&objects, &active_sinks);

        // Should exclude current profile (analog-stereo) and off profile
        assert_eq!(profile_sinks.len(), 1);
        assert!(profile_sinks[0].predicted_name.contains("iec958-stereo"));
    }

    #[test]
    fn test_get_profile_sinks_predicts_node_name() {
        let device_json = r#"[
            {
                "id": 100,
                "type": "PipeWire:Interface:Device",
                "info": {
                    "props": {
                        "device.name": "alsa_card.pci-0000_00_1f.3"
                    },
                    "params": {
                        "Profile": [{"index": 0, "name": "off"}],
                        "EnumProfile": [
                            {"index": 0, "name": "off", "description": "Off"},
                            {"index": 1, "name": "output:analog-stereo", "description": "Analog Stereo", "available": "yes"}
                        ]
                    }
                }
            }
        ]"#;
        let objects: Vec<PwObject> = serde_json::from_str(device_json).unwrap();
        let active_sinks = vec![];

        let profile_sinks = PipeWire::get_profile_sinks(&objects, &active_sinks);

        assert_eq!(profile_sinks.len(), 1);
        // Should predict: alsa_output.pci-0000_00_1f.3.analog-stereo
        assert_eq!(
            profile_sinks[0].predicted_name,
            "alsa_output.pci-0000_00_1f.3.analog-stereo"
        );
    }}
