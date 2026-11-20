//! Typed snapshot of the `PipeWire` graph, as far as `pwsw` cares about it.
//!
//! Built from `pw-dump` output by [`super::parse`]; consumed by the pure
//! resolution logic in [`super::resolve`]. Nothing here talks to `PipeWire`.

/// A point-in-time view of the audio devices, sink nodes and default-sink metadata.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Snapshot {
    /// Audio devices (cards) with their profile lists.
    pub(crate) devices: Vec<Device>,
    /// Nodes whose `media.class` is `Audio/Sink`.
    pub(crate) sinks: Vec<SinkNode>,
    /// Default-sink keys from the `default` metadata object.
    pub(crate) defaults: Defaults,
}

/// An audio device (`PipeWire:Interface:Device`), e.g. an ALSA card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Device {
    /// Global object id (what `pw-cli set-param` takes).
    pub(crate) id: u32,
    /// `device.name`, e.g. `alsa_card.pci-0000_0c_00.4`.
    pub(crate) name: String,
    /// `device.description` (falls back to the name).
    pub(crate) description: String,
    /// Entries of the `EnumProfile` param.
    pub(crate) profiles: Vec<Profile>,
    /// Index of the active profile (`Profile` param), if reported.
    pub(crate) active_profile: Option<u32>,
}

/// Availability of a profile as reported by `PipeWire`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Availability {
    Yes,
    No,
    Unknown,
}

/// One entry of a device's `EnumProfile` param.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Profile {
    /// Profile index (what `pw-cli set-param ... Profile` takes).
    pub(crate) index: u32,
    /// Profile name, e.g. `output:analog-stereo+input:analog-stereo`.
    pub(crate) name: String,
    /// Human readable description (falls back to the name).
    pub(crate) description: String,
    pub(crate) available: Availability,
    pub(crate) priority: i64,
    /// `card.profile.devices` of the `Audio/Sink` class entries: the card-internal
    /// device numbers that become sink nodes (`card.profile.device` on the node).
    pub(crate) sink_card_devices: Vec<u32>,
    /// Whether the profile provides a capture (`Audio/Source`) side.
    pub(crate) has_input: bool,
}

impl Profile {
    /// Output mapping names of an `ACP` profile name: the parts after `output:`.
    ///
    /// `output:analog-stereo+input:analog-stereo` → `["analog-stereo"]`.
    /// Input-only, `off`, `pro-audio` and `UCM` (`HiFi`…) profiles yield nothing.
    pub(crate) fn output_mappings(&self) -> impl Iterator<Item = &str> {
        self.name
            .split('+')
            .filter_map(|part| part.strip_prefix("output:"))
    }

    /// Input mapping name (the part after `input:`), if any.
    pub(crate) fn input_mapping(&self) -> Option<&str> {
        self.name
            .split('+')
            .find_map(|part| part.strip_prefix("input:"))
    }

    /// Whether this profile provides the given output mapping.
    pub(crate) fn provides_output(&self, mapping: &str) -> bool {
        self.output_mappings().any(|m| m == mapping)
    }
}

/// An `Audio/Sink` node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SinkNode {
    /// Global object id.
    pub(crate) id: u32,
    /// `node.name`.
    pub(crate) name: String,
    /// `node.description`, else `node.nick`, else the name.
    pub(crate) description: String,
    /// `device.id`: the owning device's global id.
    pub(crate) device_id: Option<u32>,
    /// `device.profile.name`: the output mapping that created the node (`ACP` cards).
    pub(crate) profile_name: Option<String>,
    /// `card.profile.device`: the card-internal device number.
    pub(crate) card_profile_device: Option<u32>,
}

/// Default-sink metadata.
///
/// `configured` (`default.configured.audio.sink`) is the user's choice, written by
/// `wpctl set-default`, `pactl` and `pwsw`. `effective` (`default.audio.sink`) is what
/// `WirePlumber` derived from it (it falls back to another sink when the configured
/// one does not exist) and recomputes on every rescan.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Defaults {
    pub(crate) configured: Option<String>,
    pub(crate) effective: Option<String>,
}

impl Snapshot {
    /// Find a sink node by `node.name`.
    pub(crate) fn sink_by_name(&self, name: &str) -> Option<&SinkNode> {
        self.sinks.iter().find(|s| s.name == name)
    }

    /// Find a device by global id.
    #[cfg(test)]
    pub(crate) fn device(&self, id: u32) -> Option<&Device> {
        self.devices.iter().find(|d| d.id == id)
    }

    /// `default.configured.audio.sink`: the user's chosen default, if set.
    #[must_use]
    pub fn configured_default(&self) -> Option<&str> {
        self.defaults.configured.as_deref()
    }

    /// `default.audio.sink`: the default `WirePlumber` currently routes to, if set.
    #[must_use]
    pub fn effective_default(&self) -> Option<&str> {
        self.defaults.effective.as_deref()
    }

    /// The default sink as the user sees it.
    ///
    /// Prefers the configured key when it names an existing sink; otherwise the
    /// effective key (what is actually playing), otherwise whatever is configured.
    #[must_use]
    pub fn current_default(&self) -> Option<&str> {
        let configured = self.defaults.configured.as_deref();
        if let Some(name) = configured
            && self.sink_by_name(name).is_some()
        {
            return Some(name);
        }
        self.defaults.effective.as_deref().or(configured)
    }
}
