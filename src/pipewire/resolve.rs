//! Pure sink resolution: which sinks can be reached by a profile switch, and what
//! to do to make a configured node name the default.
//!
//! # Identity
//!
//! Configs store node names. For an ALSA card running the `ACP` profile set, the sink
//! node of output mapping `M` on card `alsa_card.<S>` is named `alsa_output.<S>.<M>`
//! by default, where `M` is the part of a profile name after `output:` up to `+`
//! (`output:analog-stereo+input:analog-stereo` → `analog-stereo`). That name is only
//! used to find *candidates*: the device must exist and must have an available profile
//! providing `M`. After a switch the new node is recognised by `device.id` plus
//! `device.profile.name` (or `card.profile.device`), never by its name, because
//! `WirePlumber` rules may rename nodes.
//!
//! # Not supported
//!
//! `UCM` cards (profiles such as `HiFi`) and `pro-audio` have no `output:` mapping in
//! their profile names, so there is no stable way to predict their node names; their
//! sinks can be used while active but are never offered via profile switching.

use std::collections::HashSet;

use color_eyre::eyre::{self, Result};
use tracing::debug;

use super::ProfileSink;
use super::model::{Availability, Device, Profile, SinkNode, Snapshot};

const CARD_PREFIX: &str = "alsa_card.";
const OUTPUT_PREFIX: &str = "alsa_output.";
/// Bluetooth sinks: they exist only while the device is connected
const BLUEZ_OUTPUT_PREFIX: &str = "bluez_output.";

/// What to do to make a configured sink the default.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Plan {
    /// The sink exists: only set it as default (using this, its actual name).
    SetDefault { node_name: String },
    /// Switch a card's profile, wait for the sink node, then set it as default.
    SwitchProfile(ProfileSwitch),
}

/// A planned profile switch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProfileSwitch {
    pub(crate) device_id: u32,
    pub(crate) device_name: String,
    pub(crate) profile_index: u32,
    pub(crate) profile_name: String,
    /// The output mapping whose sink node we wait for.
    pub(crate) mapping: String,
    /// `card.profile.devices` of the chosen profile's sink side.
    pub(crate) sink_card_devices: Vec<u32>,
}

impl ProfileSwitch {
    /// Whether `node` is the sink this switch produces.
    ///
    /// Matches on `device.profile.name`; `card.profile.device` is only consulted for
    /// nodes that don't report a mapping, so a profile with several outputs can never
    /// match the wrong one.
    pub(crate) fn produced(&self, node: &SinkNode) -> bool {
        node.device_id == Some(self.device_id)
            && match node.profile_name.as_deref() {
                Some(mapping) => mapping == self.mapping,
                None => node
                    .card_profile_device
                    .is_some_and(|d| self.sink_card_devices.contains(&d)),
            }
    }
}

/// `alsa_card.<suffix>` → `<suffix>`.
fn card_suffix(device: &Device) -> Option<&str> {
    device.name.strip_prefix(CARD_PREFIX)
}

/// Default node name of output mapping `mapping` on a card with suffix `suffix`.
fn predicted_node_name(suffix: &str, mapping: &str) -> String {
    format!("{OUTPUT_PREFIX}{suffix}.{mapping}")
}

/// A profile that may be switched to for its outputs.
fn offerable(profile: &Profile) -> bool {
    profile.available != Availability::No && profile.output_mappings().next().is_some()
}

/// The existing sink node of `mapping` on `device`, if any.
fn existing_node<'a>(
    snapshot: &'a Snapshot,
    device_id: u32,
    mapping: &str,
) -> Option<&'a SinkNode> {
    snapshot
        .sinks
        .iter()
        .find(|n| n.device_id == Some(device_id) && n.profile_name.as_deref() == Some(mapping))
}

/// Choose the profile to switch to for `mapping` on `device`.
///
/// Preference: keep the current profile's input mapping
/// (`output:<mapping>+input:<current input>`); otherwise any profile with an input
/// (so the card's microphone survives); otherwise output-only. Ties go to the higher
/// `priority`, then the lower index. Unavailable profiles are never chosen.
///
/// A duplex profile wins even when the current profile is output-only, i.e. a switch
/// may bring an input back that was off. That is deliberate: losing a microphone
/// silently is worse than having an unused one available, and an output-only profile
/// is more often a leftover (e.g. of an earlier switch that dropped the input) than a
/// choice.
pub(crate) fn choose_profile<'a>(device: &'a Device, mapping: &str) -> Option<&'a Profile> {
    let current_input = device
        .active_profile
        .and_then(|idx| device.profiles.iter().find(|p| p.index == idx))
        .and_then(Profile::input_mapping);
    let rank = |p: &Profile| {
        let keeps_input = current_input.is_some() && p.input_mapping() == current_input;
        (keeps_input, p.has_input, p.priority)
    };
    device
        .profiles
        .iter()
        .filter(|p| offerable(p) && p.provides_output(mapping))
        .max_by(|a, b| rank(a).cmp(&rank(b)).then(b.index.cmp(&a.index)))
}

/// Sinks reachable by switching a card profile: one entry per distinct output mapping
/// per ALSA card that has no sink node yet, with the profile [`choose_profile`] would use.
///
/// Never lists input-only, `off`, `pro-audio` or `UCM` profiles, nor profiles marked
/// unavailable.
pub(crate) fn list_profile_sinks(
    snapshot: &Snapshot,
    active_names: &HashSet<&str>,
) -> Vec<ProfileSink> {
    let mut out = Vec::new();
    for device in &snapshot.devices {
        let Some(suffix) = card_suffix(device) else {
            continue;
        };
        let mut seen: HashSet<&str> = HashSet::new();
        let mut any_acp_output = false;
        for profile in &device.profiles {
            for mapping in profile.output_mappings() {
                any_acp_output = true;
                if !offerable(profile) || !seen.insert(mapping) {
                    continue;
                }
                let predicted_name = predicted_node_name(suffix, mapping);
                if existing_node(snapshot, device.id, mapping).is_some()
                    || active_names.contains(predicted_name.as_str())
                {
                    continue;
                }
                let Some(chosen) = choose_profile(device, mapping) else {
                    continue;
                };
                // Describe the output itself, not the duplex combination, when possible.
                let description = device
                    .profiles
                    .iter()
                    .find(|p| p.name == format!("output:{mapping}"))
                    .unwrap_or(chosen)
                    .description
                    .clone();
                out.push(ProfileSink {
                    predicted_name,
                    description,
                    device_id: device.id,
                    device_name: device.name.clone(),
                    profile_index: chosen.index,
                    profile_name: chosen.name.clone(),
                });
            }
        }
        if !any_acp_output && device.profiles.iter().any(|p| p.name != "off") {
            debug!(
                "Card {} has no ACP output profiles (UCM or pro-audio only); profile switching not offered",
                device.name
            );
        }
    }
    out
}

/// Resolve a configured sink node name into an activation plan.
///
/// # Errors
/// Returns an error if no node has that name and it cannot be reached by switching
/// an available profile of a matching ALSA card. The message says why: a Bluetooth
/// sink that is not connected, an ALSA name no card matches, or any other name that
/// is simply not present.
pub(crate) fn resolve(snapshot: &Snapshot, target: &str) -> Result<Plan> {
    if snapshot.sink_by_name(target).is_some() {
        return Ok(Plan::SetDefault {
            node_name: target.to_string(),
        });
    }

    // Find the card whose predicted prefix the name carries (longest suffix wins).
    let candidate = snapshot
        .devices
        .iter()
        .filter_map(|d| {
            let suffix = card_suffix(d)?;
            let mapping = target
                .strip_prefix(OUTPUT_PREFIX)?
                .strip_prefix(suffix)?
                .strip_prefix('.')?;
            (!mapping.is_empty()).then_some((d, suffix.len(), mapping))
        })
        .max_by_key(|(_, len, _)| *len);
    let Some((device, _, mapping)) = candidate else {
        if target.starts_with(BLUEZ_OUTPUT_PREFIX) {
            eyre::bail!("Sink '{target}' is not present (Bluetooth device disconnected?)");
        }
        if target.starts_with(OUTPUT_PREFIX) {
            eyre::bail!(
                "Sink '{target}' not found: no such node exists and no ALSA card matches the \
                 name (check `pwsw list-sinks`)"
            );
        }
        eyre::bail!(
            "Sink '{target}' is not present and not reachable by a profile switch (check \
             `pwsw list-sinks`)"
        );
    };

    // Already provided by the current profile under another (renamed) node name.
    if let Some(node) = existing_node(snapshot, device.id, mapping) {
        debug!("Sink '{}' is active as '{}'", target, node.name);
        return Ok(Plan::SetDefault {
            node_name: node.name.clone(),
        });
    }

    if let Some(profile) = choose_profile(device, mapping) {
        return Ok(Plan::SwitchProfile(ProfileSwitch {
            device_id: device.id,
            device_name: device.name.clone(),
            profile_index: profile.index,
            profile_name: profile.name.clone(),
            mapping: mapping.to_string(),
            sink_card_devices: profile.sink_card_devices.clone(),
        }));
    }

    if device.profiles.iter().any(|p| p.provides_output(mapping)) {
        eyre::bail!(
            "Sink '{target}' is not available: every profile of {} providing output '{mapping}' \
             is marked unavailable (is something plugged into that port?)",
            device.name
        );
    }
    eyre::bail!(
        "Sink '{target}' not found: card {} has no profile with output '{mapping}' \
         (check `pwsw list-sinks`)",
        device.name
    )
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;
    use crate::pipewire::parse::parse_dump;

    const REAL: &str = include_str!("../../tests/fixtures/pwdump-alsa-profiles.json");
    const SYNTH: &str = include_str!("../../tests/fixtures/pwdump-synthetic.json");

    fn snap(json: &str) -> Snapshot {
        parse_dump(json.as_bytes()).expect("fixture parses")
    }

    fn listing(snapshot: &Snapshot) -> Vec<(String, String)> {
        let active: HashSet<&str> = snapshot.sinks.iter().map(|s| s.name.as_str()).collect();
        list_profile_sinks(snapshot, &active)
            .into_iter()
            .map(|p| (p.predicted_name, p.profile_name))
            .collect()
    }

    fn switch(plan: Plan) -> ProfileSwitch {
        match plan {
            Plan::SwitchProfile(sw) => sw,
            Plan::SetDefault { node_name } => panic!("expected a switch, got default {node_name}"),
        }
    }

    #[test]
    fn test_list_real_fixture_exact_set() {
        let p = "alsa_output.pci-0000_0c_00.4";
        let duplex = |m: &str| format!("output:{m}+input:analog-stereo");
        let expected: Vec<(String, String)> = [
            "analog-stereo",
            "analog-surround-21",
            "analog-surround-41",
            "analog-surround-51",
            "analog-surround-40",
            "analog-surround-50",
            "iec958-ac3-surround-51",
        ]
        .iter()
        .map(|m| (format!("{p}.{m}"), duplex(m)))
        .collect();
        // HDMI card: every output unavailable; iec958-stereo is already active;
        // no input-only, pro-audio or doubled `analog-stereo-analog-stereo` entries.
        assert_eq!(listing(&snap(REAL)), expected);
    }

    #[test]
    fn test_list_uses_output_description() {
        let s = snap(REAL);
        let active: HashSet<&str> = s.sinks.iter().map(|n| n.name.as_str()).collect();
        let first = &list_profile_sinks(&s, &active)[0];
        assert_eq!(first.description, "Analog Stereo Output");
        assert_eq!(first.device_id, 43);
        assert_eq!(first.profile_index, 1);
    }

    #[test]
    fn test_list_synthetic_skips_ucm_bluetooth_unavailable() {
        assert_eq!(listing(&snap(SYNTH)), vec![
            (
                "alsa_output.usb-Example_USB_Headset-00.analog-stereo".to_string(),
                "output:analog-stereo+input:mono-fallback".to_string()
            ),
            (
                "alsa_output.pci-0000_00_1f.3.iec958-stereo".to_string(),
                "output:iec958-stereo+input:analog-stereo".to_string()
            ),
            (
                "alsa_output.pci-0000_00_1f.3.analog-surround-51".to_string(),
                "output:analog-surround-51".to_string()
            ),
        ]);
    }

    #[test]
    fn test_resolve_restores_microphone_from_output_only_profile() {
        // The card sits on an output-only profile (microphone gone): the duplex
        // profile wins over `output:analog-stereo`, bringing the microphone back.
        let sw =
            switch(resolve(&snap(REAL), "alsa_output.pci-0000_0c_00.4.analog-stereo").unwrap());
        assert_eq!(sw.device_id, 43);
        assert_eq!(sw.profile_index, 1);
        assert_eq!(sw.profile_name, "output:analog-stereo+input:analog-stereo");
        assert_eq!(sw.mapping, "analog-stereo");
        assert_eq!(sw.sink_card_devices, vec![5]);
    }

    #[test]
    fn test_resolve_keeps_current_input_over_priority() {
        // `+input:analog-mic` has the higher priority, but the card currently records
        // from `analog-stereo`, so that input is kept.
        let sw =
            switch(resolve(&snap(SYNTH), "alsa_output.pci-0000_00_1f.3.iec958-stereo").unwrap());
        assert_eq!(sw.profile_name, "output:iec958-stereo+input:analog-stereo");
    }

    #[test]
    fn test_resolve_usb_keeps_mono_fallback_input() {
        let sw = switch(
            resolve(
                &snap(SYNTH),
                "alsa_output.usb-Example_USB_Headset-00.analog-stereo",
            )
            .unwrap(),
        );
        assert_eq!(sw.device_id, 60);
        assert_eq!(sw.profile_name, "output:analog-stereo+input:mono-fallback");
    }

    #[test]
    fn test_resolve_skips_unavailable_profile() {
        let sw = switch(
            resolve(
                &snap(SYNTH),
                "alsa_output.pci-0000_00_1f.3.analog-surround-51",
            )
            .unwrap(),
        );
        assert_eq!(sw.profile_name, "output:analog-surround-51");
    }

    #[test]
    fn test_resolve_all_unavailable_is_clear_error() {
        let err = resolve(&snap(SYNTH), "alsa_output.pci-0000_00_1f.3.hdmi-stereo").unwrap_err();
        assert!(format!("{err}").contains("unavailable"), "{err}");
    }

    #[test]
    fn test_resolve_active_sink_sets_default_only() {
        assert_eq!(
            resolve(&snap(REAL), "alsa_output.pci-0000_0c_00.4.iec958-stereo").unwrap(),
            Plan::SetDefault {
                node_name: "alsa_output.pci-0000_0c_00.4.iec958-stereo".to_string()
            }
        );
    }

    #[test]
    fn test_resolve_renamed_active_node_by_identity() {
        let mut s = snap(SYNTH);
        s.sinks[0].name = "renamed_by_wireplumber".to_string();
        assert_eq!(
            resolve(&s, "alsa_output.pci-0000_00_1f.3.analog-stereo").unwrap(),
            Plan::SetDefault {
                node_name: "renamed_by_wireplumber".to_string()
            }
        );
    }

    #[test]
    fn test_resolve_unknown_names_are_errors() {
        let s = snap(REAL);
        for name in [
            "alsa_output.usb-nope.analog-stereo",
            "alsa_output.pci-0000_0c_00.4.analog-stereo-analog-stereo",
            "alsa_output.pci-0000_0c_00.4.",
        ] {
            let err = resolve(&s, name).unwrap_err();
            assert!(format!("{err}").contains("not found"), "{name}: {err}");
        }
    }

    /// Names no ALSA card could produce get an error that does not talk about ALSA
    #[test]
    fn test_resolve_missing_non_alsa_sinks_say_not_present() {
        let s = snap(REAL);
        let err = resolve(&s, "bluez_output.00_11_22_33_44_55.1")
            .unwrap_err()
            .to_string();
        assert_eq!(
            err,
            "Sink 'bluez_output.00_11_22_33_44_55.1' is not present (Bluetooth device \
             disconnected?)"
        );
        let err = resolve(&s, "my_renamed_speakers").unwrap_err().to_string();
        assert!(
            err.contains("is not present and not reachable by a profile switch"),
            "{err}"
        );
        assert!(!err.contains("ALSA"), "{err}");
    }

    #[test]
    fn test_resolve_never_offers_ucm_switch() {
        let err = resolve(&snap(SYNTH), "alsa_output.platform-example_ucm.HiFi").unwrap_err();
        assert!(format!("{err}").contains("not found"), "{err}");
    }

    #[test]
    fn test_produced_matches_by_card_profile_device() {
        let sw =
            switch(resolve(&snap(REAL), "alsa_output.pci-0000_0c_00.4.analog-stereo").unwrap());
        let node = SinkNode {
            id: 99,
            name: "custom".to_string(),
            description: String::new(),
            device_id: Some(43),
            profile_name: None,
            card_profile_device: Some(5),
        };
        assert!(sw.produced(&node));
        assert!(!sw.produced(&SinkNode {
            device_id: Some(42),
            ..node.clone()
        }));
        assert!(!sw.produced(&SinkNode {
            card_profile_device: Some(11),
            ..node
        }));
    }
}
