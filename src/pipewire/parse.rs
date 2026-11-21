//! `pw-dump` JSON → [`Snapshot`]. Pure; no process execution.
//!
//! Parsing is permissive per object: an object that lacks what `pwsw` needs is
//! skipped (logged at `trace`), so one odd node cannot hide every other sink.

use color_eyre::eyre::{Context, Result};
use serde_json::{Map, Value};
use tracing::trace;

use super::GraphInfo;
use super::model::{Availability, Defaults, Device, Profile, SinkNode, Snapshot};

const TYPE_DEVICE: &str = "PipeWire:Interface:Device";
const TYPE_NODE: &str = "PipeWire:Interface:Node";
const TYPE_METADATA: &str = "PipeWire:Interface:Metadata";
const TYPE_CORE: &str = "PipeWire:Interface:Core";
const TYPE_CLIENT: &str = "PipeWire:Interface:Client";

const KEY_CONFIGURED_SINK: &str = "default.configured.audio.sink";
const KEY_EFFECTIVE_SINK: &str = "default.audio.sink";

/// Parse raw `pw-dump` output.
///
/// # Errors
/// Returns an error if the input is not a `JSON` array.
pub(crate) fn parse_dump(bytes: &[u8]) -> Result<Snapshot> {
    let objects: Vec<Value> =
        serde_json::from_slice(bytes).context("Failed to parse pw-dump JSON")?;
    trace!("pw-dump returned {} objects", objects.len());
    Ok(snapshot_from_objects(&objects))
}

/// Parse raw `pw-dump` output into a snapshot plus what it says about the server
/// and its clients.
///
/// # Errors
/// Returns an error if the input is not a `JSON` array.
pub(crate) fn parse_dump_with_info(bytes: &[u8]) -> Result<(Snapshot, GraphInfo)> {
    let objects: Vec<Value> =
        serde_json::from_slice(bytes).context("Failed to parse pw-dump JSON")?;
    Ok((snapshot_from_objects(&objects), graph_info(&objects)))
}

/// The server version (from the core object) and the session manager client (the
/// first client whose `application.name` contains `WirePlumber`).
fn graph_info(objects: &[Value]) -> GraphInfo {
    let mut info = GraphInfo::default();
    for obj in objects {
        match obj.get("type").and_then(Value::as_str) {
            Some(TYPE_CORE) if info.server_version.is_none() => {
                info.server_version = obj
                    .get("info")
                    .and_then(|i| i.get("version"))
                    .and_then(Value::as_str)
                    .map(String::from);
            }
            Some(TYPE_CLIENT) if info.session_manager.is_none() => {
                info.session_manager = props(obj)
                    .and_then(|p| prop_str(p, "application.name"))
                    .filter(|name| name.to_lowercase().contains("wireplumber"))
                    .map(String::from);
            }
            _ => {}
        }
    }
    info
}

/// Build a snapshot from already-decoded `pw-dump` objects.
pub(crate) fn snapshot_from_objects(objects: &[Value]) -> Snapshot {
    let mut snapshot = Snapshot::default();
    for obj in objects {
        let Some(kind) = obj.get("type").and_then(Value::as_str) else {
            continue;
        };
        match kind {
            TYPE_DEVICE => {
                if let Some(device) = parse_device(obj) {
                    snapshot.devices.push(device);
                }
            }
            TYPE_NODE => {
                if let Some(sink) = parse_sink_node(obj) {
                    snapshot.sinks.push(sink);
                }
            }
            TYPE_METADATA => parse_default_metadata(obj, &mut snapshot.defaults),
            _ => {}
        }
    }
    snapshot
}

fn object_id(obj: &Value) -> Option<u32> {
    obj.get("id")
        .and_then(Value::as_u64)
        .and_then(|id| u32::try_from(id).ok())
}

/// `info.props`, or top-level `props` (metadata objects keep them there).
fn props(obj: &Value) -> Option<&Map<String, Value>> {
    obj.get("info")
        .and_then(|i| i.get("props"))
        .and_then(Value::as_object)
        .or_else(|| obj.get("props").and_then(Value::as_object))
}

fn prop_str<'a>(props: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    props.get(key).and_then(Value::as_str)
}

/// A numeric prop; `PipeWire` normally emits numbers but older versions used strings.
fn prop_u32(props: &Map<String, Value>, key: &str) -> Option<u32> {
    as_u32(props.get(key)?)
}

fn as_u32(value: &Value) -> Option<u32> {
    match value {
        Value::Number(n) => n.as_u64().and_then(|v| u32::try_from(v).ok()),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

fn param<'a>(obj: &'a Value, name: &str) -> Option<&'a Vec<Value>> {
    obj.get("info")?.get("params")?.get(name)?.as_array()
}

fn parse_device(obj: &Value) -> Option<Device> {
    let id = object_id(obj)?;
    let props = props(obj)?;
    let name = prop_str(props, "device.name")?.to_string();
    let description = prop_str(props, "device.description")
        .or_else(|| prop_str(props, "device.nick"))
        .unwrap_or(&name)
        .to_string();
    let profiles = param(obj, "EnumProfile")
        .map_or_default(|list| list.iter().filter_map(parse_profile).collect());
    let active_profile = param(obj, "Profile")
        .and_then(|list| list.first())
        .and_then(|p| p.get("index"))
        .and_then(as_u32);
    Some(Device {
        id,
        name,
        description,
        profiles,
        active_profile,
    })
}

fn parse_profile(value: &Value) -> Option<Profile> {
    let index = as_u32(value.get("index")?)?;
    let name = value.get("name")?.as_str()?.to_string();
    let description = value
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or(&name)
        .to_string();
    let available = match value.get("available").and_then(Value::as_str) {
        Some("no") => Availability::No,
        Some("yes") => Availability::Yes,
        _ => Availability::Unknown,
    };
    let priority = value.get("priority").and_then(Value::as_i64).unwrap_or(0);
    let (sink_card_devices, has_source_class) = parse_profile_classes(value.get("classes"));
    let has_input = has_source_class || name.split('+').any(|p| p.starts_with("input:"));
    Some(Profile {
        index,
        name,
        description,
        available,
        priority,
        sink_card_devices,
        has_input,
    })
}

/// Parse `EnumProfile.classes`:
/// `[count, ["Audio/Sink", n, "card.profile.devices", [ids…]], ["Audio/Source", …]]`.
///
/// Returns the `card.profile.devices` of the `Audio/Sink` entries and whether an
/// `Audio/Source` entry exists.
fn parse_profile_classes(classes: Option<&Value>) -> (Vec<u32>, bool) {
    let mut sink_devices = Vec::new();
    let mut has_source = false;
    let Some(entries) = classes.and_then(Value::as_array) else {
        return (sink_devices, has_source);
    };
    for entry in entries.iter().filter_map(Value::as_array) {
        let Some(class) = entry.first().and_then(Value::as_str) else {
            continue;
        };
        match class {
            "Audio/Source" => has_source = true,
            "Audio/Sink" => {
                // Key/value pairs follow the class name and its node count.
                let devices = entry
                    .get(2..)
                    .unwrap_or_default()
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .find(|[key, _]| key.as_str() == Some("card.profile.devices"))
                    .and_then(|[_, value]| value.as_array())
                    .map_or_default(|ids| ids.iter().filter_map(as_u32).collect::<Vec<_>>());
                sink_devices.extend(devices);
            }
            _ => {}
        }
    }
    (sink_devices, has_source)
}

fn parse_sink_node(obj: &Value) -> Option<SinkNode> {
    let id = object_id(obj)?;
    let props = props(obj)?;
    if prop_str(props, "media.class") != Some("Audio/Sink") {
        return None;
    }
    let Some(name) = prop_str(props, "node.name") else {
        trace!("Skipping Audio/Sink node {id} without node.name");
        return None;
    };
    let description = prop_str(props, "node.description")
        .or_else(|| prop_str(props, "node.nick"))
        .unwrap_or(name)
        .to_string();
    Some(SinkNode {
        id,
        name: name.to_string(),
        description,
        device_id: prop_u32(props, "device.id"),
        profile_name: prop_str(props, "device.profile.name").map(String::from),
        card_profile_device: prop_u32(props, "card.profile.device"),
    })
}

fn parse_default_metadata(obj: &Value, defaults: &mut Defaults) {
    let Some(props) = props(obj) else { return };
    if prop_str(props, "metadata.name") != Some("default") {
        return;
    }
    let Some(entries) = obj.get("metadata").and_then(Value::as_array) else {
        return;
    };
    for entry in entries {
        // Only global (subject 0) entries; a missing subject is treated as global.
        if entry.get("subject").and_then(Value::as_u64).unwrap_or(0) != 0 {
            continue;
        }
        let slot = match entry.get("key").and_then(Value::as_str) {
            Some(KEY_CONFIGURED_SINK) => &mut defaults.configured,
            Some(KEY_EFFECTIVE_SINK) => &mut defaults.effective,
            _ => continue,
        };
        *slot = entry.get("value").and_then(metadata_name);
    }
}

/// Extract the node name from a metadata value: `{"name": "…"}`, a `JSON`
/// string containing that object, or a plain string.
fn metadata_name(value: &Value) -> Option<String> {
    match value {
        Value::Object(map) => map.get("name").and_then(Value::as_str).map(String::from),
        Value::String(s) => match serde_json::from_str::<Value>(s) {
            Ok(Value::Object(map)) => map.get("name").and_then(Value::as_str).map(String::from),
            _ => Some(s.clone()),
        },
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;
    use serde_json::json;

    use super::*;

    const REAL_FIXTURE: &str = include_str!("../../tests/fixtures/pwdump-alsa-profiles.json");

    #[test]
    fn test_parse_real_fixture_devices_and_sinks() {
        let snap = parse_dump(REAL_FIXTURE.as_bytes()).expect("fixture parses");

        let names: Vec<&str> = snap.devices.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, [
            "alsa_card.pci-0000_0a_00.1",
            "alsa_card.pci-0000_0c_00.4"
        ]);

        let card = snap.device(43).expect("device 43");
        assert_eq!(card.active_profile, Some(4));
        assert_eq!(card.profiles.len(), 19);
        let duplex = &card.profiles[1];
        assert_eq!(duplex.name, "output:analog-stereo+input:analog-stereo");
        assert_eq!(duplex.available, Availability::Yes);
        assert_eq!(duplex.priority, 6565);
        assert_eq!(duplex.sink_card_devices, vec![5]);
        assert!(duplex.has_input);
        let out_only = &card.profiles[2];
        assert_eq!(out_only.name, "output:analog-stereo");
        assert!(!out_only.has_input);
        let input_only = &card.profiles[17];
        assert!(input_only.has_input);
        assert_eq!(input_only.sink_card_devices, Vec::<u32>::new());
        assert_eq!(input_only.output_mappings().count(), 0);

        assert_eq!(snap.sinks.len(), 1);
        let sink = &snap.sinks[0];
        assert_eq!(sink.name, "alsa_output.pci-0000_0c_00.4.iec958-stereo");
        assert_eq!(sink.device_id, Some(43));
        assert_eq!(sink.profile_name.as_deref(), Some("iec958-stereo"));
        assert_eq!(sink.card_profile_device, Some(11));
    }

    #[test]
    fn test_parse_real_fixture_defaults_prefers_existing_sink() {
        let snap = parse_dump(REAL_FIXTURE.as_bytes()).expect("fixture parses");
        assert_eq!(
            snap.configured_default(),
            Some("alsa_output.pci-0000_0c_00.4.analog-stereo")
        );
        assert_eq!(
            snap.effective_default(),
            Some("alsa_output.pci-0000_0c_00.4.iec958-stereo")
        );
        // The configured sink does not exist (output-only digital profile is active),
        // so the effective default is what is really playing.
        assert_eq!(
            snap.current_default(),
            Some("alsa_output.pci-0000_0c_00.4.iec958-stereo")
        );
    }

    #[test]
    fn test_current_default_prefers_configured_when_present() {
        let objects = vec![
            json!({"id": 5, "type": TYPE_NODE, "info": {"props": {
                "node.name": "a", "media.class": "Audio/Sink"}}}),
            json!({"id": 6, "type": TYPE_NODE, "info": {"props": {
                "node.name": "b", "media.class": "Audio/Sink"}}}),
            json!({"id": 0, "type": TYPE_METADATA, "props": {"metadata.name": "default"},
            "metadata": [
                {"subject": 0, "key": KEY_EFFECTIVE_SINK, "value": {"name": "a"}},
                {"subject": 0, "key": KEY_CONFIGURED_SINK, "value": {"name": "b"}}
            ]}),
        ];
        let snap = snapshot_from_objects(&objects);
        assert_eq!(snap.current_default(), Some("b"));
    }

    #[test]
    fn test_metadata_value_formats() {
        assert_eq!(metadata_name(&json!({"name": "x"})).as_deref(), Some("x"));
        assert_eq!(
            metadata_name(&json!("{\"name\":\"y\"}")).as_deref(),
            Some("y")
        );
        assert_eq!(metadata_name(&json!("plain")).as_deref(), Some("plain"));
        assert_eq!(metadata_name(&json!(null)), None);
    }

    #[test]
    fn test_sink_node_filters_and_description_fallback() {
        let objects = vec![
            json!({"id": 1, "type": TYPE_NODE, "info": {"props": {
                "node.name": "mic", "media.class": "Audio/Source"}}}),
            json!({"id": 2, "type": TYPE_NODE, "info": {"props": {
                "node.name": "nick_only", "node.nick": "Nick", "media.class": "Audio/Sink",
                "device.id": "7"}}}),
            json!({"id": 3, "type": TYPE_NODE, "info": {"props": {"media.class": "Audio/Sink"}}}),
            json!({"type": TYPE_NODE}),
        ];
        let snap = snapshot_from_objects(&objects);
        assert_eq!(snap.sinks.len(), 1);
        assert_eq!(snap.sinks[0].description, "Nick");
        assert_eq!(snap.sinks[0].device_id, Some(7));
    }

    #[test]
    fn test_parse_rejects_non_array() {
        assert!(parse_dump(b"{}").is_err());
        assert!(parse_dump(b"not json").is_err());
    }

    #[test]
    fn test_graph_info_reads_core_version_and_wireplumber() {
        let objects = json!([
            { "id": 0, "type": TYPE_CORE, "info": { "version": "1.2.7", "name": "pipewire-0" } },
            { "id": 33, "type": TYPE_CLIENT,
              "info": { "props": { "application.name": "pw-dump" } } },
            { "id": 34, "type": TYPE_CLIENT,
              "info": { "props": { "application.name": "WirePlumber" } } },
        ]);
        let bytes = serde_json::to_vec(&objects).expect("serialize");
        let (snapshot, info) = parse_dump_with_info(&bytes).expect("parses");
        assert_eq!(snapshot, Snapshot::default());
        assert_eq!(info, GraphInfo {
            server_version: Some("1.2.7".to_string()),
            session_manager: Some("WirePlumber".to_string()),
        });
        // The fixtures have neither
        let (_, info) = parse_dump_with_info(REAL_FIXTURE.as_bytes()).expect("fixture parses");
        assert_eq!(info, GraphInfo::default());
    }
}
