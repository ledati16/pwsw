//! Pure resolution: which sinks and sources can be reached by a profile switch, and
//! what to do to make a configured node name the default.
//!
//! # Identity
//!
//! Configs store node names. For an ALSA card running the `ACP` profile set, the sink
//! node of output mapping `M` on card `alsa_card.<S>` is named `alsa_output.<S>.<M>`
//! by default, where `M` is the part of a profile name after `output:` up to `+`
//! (`output:analog-stereo+input:analog-stereo` → `analog-stereo`); the source node of
//! input mapping `I` is `alsa_input.<S>.<I>`. That name is only used to find
//! *candidates*: the device must exist and must have an available profile providing
//! the mapping. After a switch the new node is recognised by `device.id` plus
//! `device.profile.name` (or `card.profile.device`), never by its name, because
//! `WirePlumber` rules may rename nodes.
//!
//! # Profile choice
//!
//! A sink's switch keeps the card's current input mapping when possible (a microphone
//! in use survives); a source's switch must keep the card's current output mappings
//! (pwsw never takes the audio output away to get a microphone). When one job wants a
//! sink and a source on the same card ([`plan_joint`]), one profile providing both is
//! chosen; if none exists the sink wins and the source is reported unreachable. While
//! sources are configured, the daemon's sink-only jobs take the source answer along, so
//! a sink switch on the microphone's card ([`sink_switch_touches`]) is planned that
//! way too and a dropped microphone is reported rather than silently lost.
//!
//! # Not supported
//!
//! `UCM` cards (profiles such as `HiFi`) and `pro-audio` have no `output:`/`input:`
//! mapping in their profile names, so there is no stable way to predict their node
//! names; their nodes can be used while active but are never reached by switching.

use std::collections::HashSet;

use anyhow::Result;
use tracing::debug;

use super::ProfileSink;
use super::model::{Availability, Device, Node, Profile, Snapshot};
use crate::config::Endpoint;

const CARD_PREFIX: &str = "alsa_card.";

/// `alsa_output.` / `alsa_input.`
const fn alsa_prefix(kind: Endpoint) -> &'static str {
    match kind {
        Endpoint::Sink => "alsa_output.",
        Endpoint::Source => "alsa_input.",
    }
}

/// Bluetooth nodes: they exist only while the device is connected
const fn bluez_prefix(kind: Endpoint) -> &'static str {
    match kind {
        Endpoint::Sink => "bluez_output.",
        Endpoint::Source => "bluez_input.",
    }
}

/// `output` / `input`, as in profile names
const fn mapping_word(kind: Endpoint) -> &'static str {
    match kind {
        Endpoint::Sink => "output",
        Endpoint::Source => "input",
    }
}

/// What to do to make a configured sink (source) the default.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Plan {
    /// The node exists: only set it as default (using this, its actual name).
    SetDefault { node_name: String },
    /// Switch a card's profile, wait for the node, then set it as default.
    SwitchProfile(ProfileSwitch),
}

/// A planned profile switch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProfileSwitch {
    pub(crate) device_id: u32,
    pub(crate) device_name: String,
    pub(crate) profile_index: u32,
    pub(crate) profile_name: String,
    /// Which node the switch is for (a sink or a source)
    pub(crate) kind: Endpoint,
    /// The output (input) mapping whose node we wait for.
    pub(crate) mapping: String,
    /// `card.profile.devices` of the chosen profile's `kind` side.
    pub(crate) card_devices: Vec<u32>,
}

impl ProfileSwitch {
    fn new(device: &Device, profile: &Profile, kind: Endpoint, mapping: &str) -> Self {
        Self {
            device_id: device.id,
            device_name: device.name.clone(),
            profile_index: profile.index,
            profile_name: profile.name.clone(),
            kind,
            mapping: mapping.to_string(),
            card_devices: profile.card_devices(kind).to_vec(),
        }
    }

    /// Whether `node` is the node this switch produces.
    ///
    /// Matches on `device.profile.name`; `card.profile.device` is only consulted for
    /// nodes that don't report a mapping, so a profile with several outputs can never
    /// match the wrong one.
    pub(crate) fn produced(&self, node: &Node) -> bool {
        node.device_id == Some(self.device_id)
            && match node.profile_name.as_deref() {
                Some(mapping) => mapping == self.mapping,
                None => node
                    .card_profile_device
                    .is_some_and(|d| self.card_devices.contains(&d)),
            }
    }
}

/// `alsa_card.<suffix>` → `<suffix>`.
fn card_suffix(device: &Device) -> Option<&str> {
    device.name.strip_prefix(CARD_PREFIX)
}

/// Default node name of `kind` mapping `mapping` on a card with suffix `suffix`.
fn predicted_node_name(kind: Endpoint, suffix: &str, mapping: &str) -> String {
    format!("{}{suffix}.{mapping}", alsa_prefix(kind))
}

/// A profile that may be switched to for its `kind` side.
fn offerable(profile: &Profile, kind: Endpoint) -> bool {
    profile.available != Availability::No && !profile.mappings(kind).is_empty()
}

/// The existing node of `kind` mapping `mapping` on `device`, if any.
fn existing_node<'a>(
    snapshot: &'a Snapshot,
    kind: Endpoint,
    device_id: u32,
    mapping: &str,
) -> Option<&'a Node> {
    snapshot
        .nodes(kind)
        .iter()
        .find(|n| n.device_id == Some(device_id) && n.profile_name.as_deref() == Some(mapping))
}

/// The device's active profile, if reported.
fn active_profile(device: &Device) -> Option<&Profile> {
    device
        .active_profile
        .and_then(|idx| device.profiles.iter().find(|p| p.index == idx))
}

/// Choose the profile to switch to for `kind` mapping `mapping` on `device`.
///
/// `partner` is a mapping of the *other* kind the profile must provide as well (a
/// joint switch: the job's sink and source on one card). Without one:
///
/// - for a sink: keep the current profile's input mapping
///   (`output:<mapping>+input:<current input>`); otherwise any profile with an input
///   (so the card's microphone survives); otherwise output-only.
/// - for a source: the profile must keep every output mapping of the current profile
///   (never take the audio output away for a microphone); a card without a current
///   output prefers profiles without outputs.
///
/// Ties go to the higher `priority`, then the lower index. Unavailable profiles are
/// never chosen.
///
/// A sink's duplex profile wins even when the current profile is output-only, i.e. a
/// switch may bring an input back that was off. That is deliberate: losing a
/// microphone silently is worse than having an unused one available, and an
/// output-only profile is more often a leftover (e.g. of an earlier switch that dropped
/// the input) than a choice.
pub(crate) fn choose_profile<'a>(
    device: &'a Device,
    kind: Endpoint,
    mapping: &str,
    partner: Option<&str>,
) -> Option<&'a Profile> {
    let other = match kind {
        Endpoint::Sink => Endpoint::Source,
        Endpoint::Source => Endpoint::Sink,
    };
    let current = active_profile(device);
    let current_input = current.and_then(Profile::input_mapping);
    let current_outputs: Vec<&str> = current.map_or_default(|p| p.output_mappings().collect());
    let eligible = |p: &&Profile| {
        offerable(p, kind)
            && p.provides(kind, mapping)
            && match (partner, kind) {
                (Some(m), _) => p.provides(other, m),
                (None, Endpoint::Sink) => true,
                (None, Endpoint::Source) => current_outputs.iter().all(|m| p.provides_output(m)),
            }
    };
    let rank = |p: &Profile| match (partner, kind) {
        (None, Endpoint::Sink) => {
            let keeps_input = current_input.is_some() && p.input_mapping() == current_input;
            (keeps_input, p.has_input, p.priority)
        }
        (None, Endpoint::Source) => {
            let quiet = current_outputs.is_empty() && p.output_mappings().next().is_none();
            (quiet, false, p.priority)
        }
        (Some(_), _) => (false, false, p.priority),
    };
    device
        .profiles
        .iter()
        .filter(eligible)
        .max_by(|a, b| rank(a).cmp(&rank(b)).then(b.index.cmp(&a.index)))
}

/// Nodes of `kind` reachable by switching a card profile: one entry per distinct
/// mapping per ALSA card that has no node yet, with the profile [`choose_profile`]
/// would use.
///
/// Never lists `off`, `pro-audio` or `UCM` profiles, nor profiles marked unavailable;
/// sinks never come from input-only profiles (and sources never from output-only ones).
pub(crate) fn list_profile_nodes(
    snapshot: &Snapshot,
    kind: Endpoint,
    active_names: &HashSet<&str>,
) -> Vec<ProfileSink> {
    let mut out = Vec::new();
    for device in &snapshot.devices {
        let Some(suffix) = card_suffix(device) else {
            continue;
        };
        let mut seen: HashSet<&str> = HashSet::new();
        let mut any_acp = false;
        for profile in &device.profiles {
            for mapping in profile.mappings(kind) {
                any_acp = true;
                if !offerable(profile, kind) || !seen.insert(mapping) {
                    continue;
                }
                let predicted_name = predicted_node_name(kind, suffix, mapping);
                if existing_node(snapshot, kind, device.id, mapping).is_some()
                    || active_names.contains(predicted_name.as_str())
                {
                    continue;
                }
                let Some(chosen) = choose_profile(device, kind, mapping, None) else {
                    continue;
                };
                // Describe the mapping itself, not the duplex combination, when possible.
                let own_name = format!("{}:{mapping}", mapping_word(kind));
                let description = device
                    .profiles
                    .iter()
                    .find(|p| p.name == own_name)
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
        if !any_acp && device.profiles.iter().any(|p| p.name != "off") {
            debug!(
                "Card {} has no ACP {} profiles (UCM or pro-audio only); profile switching not \
                 offered",
                device.name,
                mapping_word(kind)
            );
        }
    }
    out
}

/// Sinks reachable by switching a card profile (see [`list_profile_nodes`]).
pub(crate) fn list_profile_sinks(
    snapshot: &Snapshot,
    active_names: &HashSet<&str>,
) -> Vec<ProfileSink> {
    list_profile_nodes(snapshot, Endpoint::Sink, active_names)
}

/// Resolve a configured sink node name into an activation plan.
///
/// # Errors
/// See [`resolve_endpoint`].
#[cfg(test)]
pub(crate) fn resolve(snapshot: &Snapshot, target: &str) -> Result<Plan> {
    resolve_endpoint(snapshot, Endpoint::Sink, target)
}

/// The card a `kind` target belongs to by its predicted name (longest suffix wins),
/// with the mapping the name asks for
fn card_for<'a, 'b>(
    snapshot: &'a Snapshot,
    kind: Endpoint,
    target: &'b str,
) -> Option<(&'a Device, &'b str)> {
    snapshot
        .devices
        .iter()
        .filter_map(|d| {
            let suffix = card_suffix(d)?;
            let mapping = target
                .strip_prefix(alsa_prefix(kind))?
                .strip_prefix(suffix)?
                .strip_prefix('.')?;
            (!mapping.is_empty()).then_some((d, suffix.len(), mapping))
        })
        .max_by_key(|(_, len, _)| *len)
        .map(|(d, _, mapping)| (d, mapping))
}

/// Resolve a configured sink or source node name into an activation plan.
///
/// # Errors
/// Returns an error if no node has that name and it cannot be reached by switching
/// an available profile of a matching ALSA card. The message says why: a Bluetooth
/// device that is not connected, an ALSA name no card matches, a source whose card
/// cannot provide it without dropping its current output, or any other name that is
/// simply not present.
pub(crate) fn resolve_endpoint(snapshot: &Snapshot, kind: Endpoint, target: &str) -> Result<Plan> {
    let title = kind.title();
    if snapshot.node_by_name(kind, target).is_some() {
        return Ok(Plan::SetDefault {
            node_name: target.to_string(),
        });
    }

    let Some((device, mapping)) = card_for(snapshot, kind, target) else {
        if target.starts_with(bluez_prefix(kind)) {
            anyhow::bail!("{title} '{target}' is not present (Bluetooth device disconnected?)");
        }
        if target.starts_with(alsa_prefix(kind)) {
            anyhow::bail!(
                "{title} '{target}' not found: no such node exists and no ALSA card matches the \
                 name (check `pwsw list-sinks`)"
            );
        }
        anyhow::bail!(
            "{title} '{target}' is not present and not reachable by a profile switch (check \
             `pwsw list-sinks`)"
        );
    };

    // Already provided by the current profile under another (renamed) node name.
    if let Some(node) = existing_node(snapshot, kind, device.id, mapping) {
        debug!("{title} '{}' is active as '{}'", target, node.name);
        return Ok(Plan::SetDefault {
            node_name: node.name.clone(),
        });
    }

    if let Some(profile) = choose_profile(device, kind, mapping, None) {
        return Ok(Plan::SwitchProfile(ProfileSwitch::new(
            device, profile, kind, mapping,
        )));
    }

    let word = mapping_word(kind);
    let providing: Vec<&Profile> = device
        .profiles
        .iter()
        .filter(|p| p.provides(kind, mapping))
        .collect();
    if providing.iter().any(|p| p.available != Availability::No) {
        // Only possible for sources: every available profile with this input drops an
        // output the card plays to now
        let outputs: Vec<&str> =
            active_profile(device).map_or_default(|p| p.output_mappings().collect());
        anyhow::bail!(
            "{title} '{target}' is not reachable: no available profile of {} provides input \
             '{mapping}' while keeping its current output '{}'",
            device.name,
            outputs.join("+")
        );
    }
    if !providing.is_empty() {
        anyhow::bail!(
            "{title} '{target}' is not available: every profile of {} providing {word} \
             '{mapping}' is marked unavailable (is something plugged into that port?)",
            device.name
        );
    }
    anyhow::bail!(
        "{title} '{target}' not found: card {} has no profile with {word} '{mapping}' (check \
         `pwsw list-sinks`)",
        device.name
    )
}

/// The card and mapping a resolved plan acts on (`None` for nodes not on a card
/// pwsw knows, e.g. Bluetooth or virtual nodes)
fn plan_card(snapshot: &Snapshot, kind: Endpoint, plan: &Plan) -> Option<(u32, String)> {
    match plan {
        Plan::SwitchProfile(sw) => Some((sw.device_id, sw.mapping.clone())),
        Plan::SetDefault { node_name } => {
            let node = snapshot.node_by_name(kind, node_name)?;
            Some((node.device_id?, node.profile_name.clone()?))
        }
    }
}

/// A source target's card and input mapping: of its node, else by its predicted name
/// (its own resolution may fail only because it would drop an output a sink's switch
/// replaces anyway)
fn source_card(snapshot: &Snapshot, target: &str) -> Option<(u32, String)> {
    match snapshot.node_by_name(Endpoint::Source, target) {
        Some(node) => node.device_id.zip(node.profile_name.clone()),
        None => card_for(snapshot, Endpoint::Source, target)
            .map(|(d, mapping)| (d.id, mapping.to_string())),
    }
}

/// Whether `plan`'s sink switches the profile of a card one of `sources` is on (so the
/// switch decides whether that microphone survives)
pub(crate) fn sink_switch_touches(
    snapshot: &Snapshot,
    plan: &JointPlan,
    sources: &[String],
) -> bool {
    let Some((_, Plan::SwitchProfile(sw))) = &plan.sink else {
        return false;
    };
    sources.iter().any(|target| {
        source_card(snapshot, target).is_some_and(|(device, _)| device == sw.device_id)
    })
}

/// How the job's source is reached
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SourceStep {
    /// On its own (set the default, or switch its card)
    Plan(Plan),
    /// By the sink's profile switch on the same card (a joint switch): wait for this
    /// node after the sink's switch, then set it as default
    WithSinkSwitch(ProfileSwitch),
}

/// Where the job's sink is, as far as planning its source cares
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SinkSide {
    pub(crate) device_id: u32,
    pub(crate) mapping: String,
}

/// Both halves of a job resolved against one snapshot
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct JointPlan {
    /// The sink candidate to activate (index into the sink candidates) and how
    pub(crate) sink: Option<(usize, Plan)>,
    /// The source candidate to activate and how
    pub(crate) source: Option<(usize, SourceStep)>,
    /// Sink candidates skipped before the chosen one (or all), with why
    pub(crate) sink_skipped: Vec<(String, String)>,
    /// Source candidates skipped before the chosen one (or all), with why
    pub(crate) source_skipped: Vec<(String, String)>,
}

/// Resolve a job's sink and source candidates against one snapshot.
///
/// The sink is chosen exactly as on its own (the first reachable candidate). Then the
/// first source candidate that fits with it is chosen:
///
/// - the sink's card is switched and the source is on the same card: one profile must
///   provide the sink's output *and* the source's input; it replaces the sink's
///   profile (the card is switched once) and the source waits for that switch. If no
///   profile has both, the sink wins and this source is skipped as unreachable.
/// - the source's card must be switched and the sink is on it: the profile must keep
///   the sink's output, else this source is skipped.
/// - otherwise (different cards, Bluetooth, virtual nodes) the two are independent.
///
/// `sink_side` describes a sink activated earlier in the job (no sink candidates are
/// passed then), and `frozen` the cards already switched in this job: a source that
/// would need another switch of such a card is skipped (a job never switches a card
/// twice for its two halves).
pub(crate) fn plan_joint(
    snapshot: &Snapshot,
    sinks: &[String],
    sources: &[String],
    sink_side: Option<&SinkSide>,
    frozen: &HashSet<u32>,
) -> JointPlan {
    let mut plan = JointPlan {
        sink: None,
        source: None,
        sink_skipped: Vec::new(),
        source_skipped: Vec::new(),
    };
    for (index, target) in sinks.iter().enumerate() {
        match resolve_endpoint(snapshot, Endpoint::Sink, target) {
            Ok(p) => {
                plan.sink = Some((index, p));
                break;
            }
            Err(e) => plan.sink_skipped.push((target.clone(), format!("{e:#}"))),
        }
    }

    // The sink's card: planned now, or activated earlier
    let planned_side = plan.sink.as_ref().and_then(|(_, p)| {
        plan_card(snapshot, Endpoint::Sink, p)
            .map(|(device_id, mapping)| SinkSide { device_id, mapping })
    });
    let side = planned_side.as_ref().or(sink_side);
    let sink_switch = match &plan.sink {
        Some((_, Plan::SwitchProfile(sw))) => Some(sw.clone()),
        _ => None,
    };

    for (index, target) in sources.iter().enumerate() {
        let skip = |why: String| (target.clone(), why);
        let device = |id: u32| snapshot.devices.iter().find(|d| d.id == id);
        let card = source_card(snapshot, target);

        // The sink's switch is on this source's card: one profile for both
        if let (Some(sw), Some((device_id, mapping))) = (&sink_switch, &card)
            && sw.device_id == *device_id
        {
            let Some(card_device) = device(*device_id) else {
                plan.source_skipped
                    .push(skip(format!("card {device_id} vanished")));
                continue;
            };
            let Some(profile) =
                choose_profile(card_device, Endpoint::Sink, &sw.mapping, Some(mapping))
            else {
                plan.source_skipped.push(skip(format!(
                    "not reachable together with the sink: no available profile of {} \
                     provides output '{}' and input '{mapping}' (the sink wins)",
                    card_device.name, sw.mapping
                )));
                continue;
            };
            plan.sink = plan.sink.take().map(|(i, _)| {
                (
                    i,
                    Plan::SwitchProfile(ProfileSwitch::new(
                        card_device,
                        profile,
                        Endpoint::Sink,
                        &sw.mapping,
                    )),
                )
            });
            plan.source = Some((
                index,
                SourceStep::WithSinkSwitch(ProfileSwitch::new(
                    card_device,
                    profile,
                    Endpoint::Source,
                    mapping,
                )),
            ));
            return plan;
        }

        let own = match resolve_endpoint(snapshot, Endpoint::Source, target) {
            Ok(p) => p,
            Err(e) => {
                plan.source_skipped.push(skip(format!("{e:#}")));
                continue;
            }
        };
        let Plan::SwitchProfile(source_switch) = &own else {
            plan.source = Some((index, SourceStep::Plan(own)));
            return plan;
        };
        if frozen.contains(&source_switch.device_id) {
            plan.source_skipped.push(skip(format!(
                "needs another profile switch of {}, which this job already switched",
                source_switch.device_name
            )));
            continue;
        }
        // The source's switch is on the sink's card: it must keep the sink's output
        if let Some(side) = side
            && side.device_id == source_switch.device_id
        {
            let chosen = device(side.device_id).and_then(|d| {
                choose_profile(
                    d,
                    Endpoint::Source,
                    &source_switch.mapping,
                    Some(&side.mapping),
                )
                .map(|p| ProfileSwitch::new(d, p, Endpoint::Source, &source_switch.mapping))
            });
            let Some(sw) = chosen else {
                plan.source_skipped.push(skip(format!(
                    "not reachable without dropping the sink: no available profile of {} \
                     provides input '{}' and output '{}'",
                    source_switch.device_name, source_switch.mapping, side.mapping
                )));
                continue;
            };
            plan.source = Some((index, SourceStep::Plan(Plan::SwitchProfile(sw))));
            return plan;
        }
        plan.source = Some((index, SourceStep::Plan(own)));
        return plan;
    }
    plan
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
        let sw = switch(
            resolve(&snap(REAL), "alsa_output.pci-0000_0c_00.4.analog-stereo")
                .expect("target resolves"),
        );
        assert_eq!(sw.device_id, 43);
        assert_eq!(sw.profile_index, 1);
        assert_eq!(sw.profile_name, "output:analog-stereo+input:analog-stereo");
        assert_eq!(sw.mapping, "analog-stereo");
        assert_eq!(sw.card_devices, vec![5]);
    }

    #[test]
    fn test_resolve_keeps_current_input_over_priority() {
        // `+input:analog-mic` has the higher priority, but the card currently records
        // from `analog-stereo`, so that input is kept.
        let sw = switch(
            resolve(&snap(SYNTH), "alsa_output.pci-0000_00_1f.3.iec958-stereo")
                .expect("target resolves"),
        );
        assert_eq!(sw.profile_name, "output:iec958-stereo+input:analog-stereo");
    }

    #[test]
    fn test_resolve_usb_keeps_mono_fallback_input() {
        let sw = switch(
            resolve(
                &snap(SYNTH),
                "alsa_output.usb-Example_USB_Headset-00.analog-stereo",
            )
            .expect("target resolves"),
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
            .expect("target resolves"),
        );
        assert_eq!(sw.profile_name, "output:analog-surround-51");
    }

    #[test]
    fn test_resolve_all_unavailable_is_clear_error() {
        let err = resolve(&snap(SYNTH), "alsa_output.pci-0000_00_1f.3.hdmi-stereo")
            .expect_err("`resolve` fails");
        assert!(format!("{err}").contains("unavailable"), "{err}");
    }

    #[test]
    fn test_resolve_active_sink_sets_default_only() {
        assert_eq!(
            resolve(&snap(REAL), "alsa_output.pci-0000_0c_00.4.iec958-stereo")
                .expect("target resolves"),
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
            resolve(&s, "alsa_output.pci-0000_00_1f.3.analog-stereo").expect("target resolves"),
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
            let err = resolve(&s, name).expect_err("`resolve` fails");
            assert!(format!("{err}").contains("not found"), "{name}: {err}");
        }
    }

    /// Names no ALSA card could produce get an error that does not talk about ALSA
    #[test]
    fn test_resolve_missing_non_alsa_sinks_say_not_present() {
        let s = snap(REAL);
        let err = resolve(&s, "bluez_output.00_11_22_33_44_55.1")
            .expect_err("`resolve` fails")
            .to_string();
        assert_eq!(
            err,
            "Sink 'bluez_output.00_11_22_33_44_55.1' is not present (Bluetooth device \
             disconnected?)"
        );
        let err = resolve(&s, "my_renamed_speakers")
            .expect_err("`resolve` fails")
            .to_string();
        assert!(
            err.contains("is not present and not reachable by a profile switch"),
            "{err}"
        );
        assert!(!err.contains("ALSA"), "{err}");
    }

    #[test]
    fn test_resolve_never_offers_ucm_switch() {
        let err = resolve(&snap(SYNTH), "alsa_output.platform-example_ucm.HiFi")
            .expect_err("`resolve` fails");
        assert!(format!("{err}").contains("not found"), "{err}");
    }

    #[test]
    fn test_produced_matches_by_card_profile_device() {
        let sw = switch(
            resolve(&snap(REAL), "alsa_output.pci-0000_0c_00.4.analog-stereo")
                .expect("target resolves"),
        );
        let node = Node {
            id: 99,
            name: "custom".to_string(),
            description: String::new(),
            device_id: Some(43),
            profile_name: None,
            card_profile_device: Some(5),
        };
        assert!(sw.produced(&node));
        assert!(!sw.produced(&Node {
            device_id: Some(42),
            ..node.clone()
        }));
        assert!(!sw.produced(&Node {
            card_profile_device: Some(11),
            ..node
        }));
    }

    /// A profile with several outputs: a node reporting the *other* mapping is not
    /// the one the switch waits for, even though its card device belongs to the
    /// profile; a node without a mapping falls back to the card device
    #[test]
    fn test_produced_with_several_outputs_matches_only_the_wanted_mapping() {
        let sw = ProfileSwitch {
            device_id: 7,
            device_name: "alsa_card.pci-test".to_string(),
            profile_index: 2,
            profile_name: "output:analog-stereo+output:hdmi-stereo".to_string(),
            kind: Endpoint::Sink,
            mapping: "analog-stereo".to_string(),
            card_devices: vec![3, 4],
        };
        let node = |profile_name: Option<&str>, card_device: u32| Node {
            id: 99,
            name: "node".to_string(),
            description: String::new(),
            device_id: Some(7),
            profile_name: profile_name.map(String::from),
            card_profile_device: Some(card_device),
        };
        assert!(sw.produced(&node(Some("analog-stereo"), 3)));
        assert!(
            !sw.produced(&node(Some("hdmi-stereo"), 4)),
            "the profile's other output is not the wanted node"
        );
        assert!(
            sw.produced(&node(Some("analog-stereo"), 4)),
            "a reported mapping wins over the card device"
        );
        assert!(
            sw.produced(&node(None, 4)),
            "no mapping: card device decides"
        );
        assert!(!sw.produced(&node(None, 9)));
    }

    fn profile(index: u32, name: &str, priority: i64) -> Profile {
        Profile {
            index,
            name: name.to_string(),
            description: name.to_string(),
            available: Availability::Yes,
            priority,
            sink_card_devices: vec![0],
            source_card_devices: Vec::new(),
            has_input: name.contains("input:"),
        }
    }

    /// Profiles that rank the same (same priority, both keep or both lack an input)
    /// go to the lower index, wherever they sit in the list
    #[test]
    fn test_choose_profile_tie_goes_to_the_lower_index() {
        let device = Device {
            id: 7,
            name: "alsa_card.pci-test".to_string(),
            description: "Test card".to_string(),
            // The lower index comes first: `max_by` alone would pick the last one
            profiles: vec![
                profile(2, "output:analog-stereo", 100),
                profile(5, "output:analog-stereo+output:hdmi-stereo", 100),
                profile(9, "output:hdmi-stereo", 100),
            ],
            active_profile: Some(9),
        };
        let chosen = choose_profile(&device, Endpoint::Sink, "analog-stereo", None)
            .expect("two profiles provide the output");
        assert_eq!(chosen.index, 2);

        // Listed the other way round, still the lower index
        let mut reversed = device.clone();
        reversed.profiles.reverse();
        let chosen = choose_profile(&reversed, Endpoint::Sink, "analog-stereo", None)
            .expect("two profiles provide the output");
        assert_eq!(chosen.index, 2);

        // A higher priority beats a lower index
        let mut preferred = device;
        preferred.profiles[1].priority = 101;
        let chosen = choose_profile(&preferred, Endpoint::Sink, "analog-stereo", None)
            .expect("two profiles provide the output");
        assert_eq!(chosen.index, 5);
    }

    const IN_REAL: &str = "alsa_input.pci-0000_0c_00.4.analog-stereo";
    const OUT_REAL: &str = "alsa_output.pci-0000_0c_00.4.analog-stereo";

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|n| (*n).to_string()).collect()
    }

    fn joint(snapshot: &Snapshot, sinks: &[&str], sources: &[&str]) -> JointPlan {
        plan_joint(
            snapshot,
            &names(sinks),
            &names(sources),
            None,
            &HashSet::new(),
        )
    }

    #[test]
    fn test_predicted_input_names_and_listing() {
        assert_eq!(
            predicted_node_name(Endpoint::Source, "pci-0000_0c_00.4", "analog-stereo"),
            IN_REAL
        );
        // The onboard card has no input node; its input is reachable by a switch that
        // keeps the digital output it plays to now
        let s = snap(REAL);
        let listed: Vec<(String, String)> =
            list_profile_nodes(&s, Endpoint::Source, &HashSet::new())
                .into_iter()
                .map(|p| (p.predicted_name, p.profile_name))
                .collect();
        assert_eq!(listed, vec![(
            IN_REAL.to_string(),
            "output:iec958-stereo+input:analog-stereo".to_string()
        )]);
        // Synthetic: both inputs exist already; `analog-mic` would drop the output
        assert!(list_profile_nodes(&snap(SYNTH), Endpoint::Source, &HashSet::new()).is_empty());
    }

    #[test]
    fn test_resolve_source_keeps_the_current_output() {
        let sw = switch(
            resolve_endpoint(&snap(REAL), Endpoint::Source, IN_REAL).expect("target resolves"),
        );
        assert_eq!(sw.kind, Endpoint::Source);
        assert_eq!(sw.profile_name, "output:iec958-stereo+input:analog-stereo");
        assert_eq!(sw.mapping, "analog-stereo");
        assert_eq!(sw.card_devices, vec![0]);

        // Existing sources only set the default; a missing one says why
        let s = snap(SYNTH);
        assert_eq!(
            resolve_endpoint(
                &s,
                Endpoint::Source,
                "alsa_input.pci-0000_00_1f.3.analog-stereo"
            )
            .expect("target resolves"),
            Plan::SetDefault {
                node_name: "alsa_input.pci-0000_00_1f.3.analog-stereo".to_string()
            }
        );
        let err = resolve_endpoint(
            &s,
            Endpoint::Source,
            "alsa_input.pci-0000_00_1f.3.analog-mic",
        )
        .expect_err("`resolve_endpoint` fails")
        .to_string();
        assert!(
            err.contains("while keeping its current output 'analog-stereo'"),
            "{err}"
        );
        let err = resolve_endpoint(&s, Endpoint::Source, "bluez_input.00_11_22_33_44_55.0")
            .expect_err("`resolve_endpoint` fails")
            .to_string();
        assert!(err.starts_with("Source '"), "{err}");
        assert!(err.contains("Bluetooth device disconnected?"), "{err}");
        // Monitors are not sources
        let err = resolve_endpoint(
            &s,
            Endpoint::Source,
            "alsa_output.pci-0000_00_1f.3.analog-stereo.monitor",
        )
        .expect_err("`resolve_endpoint` fails")
        .to_string();
        assert!(err.contains("not present"), "{err}");
    }

    #[test]
    fn test_joint_plan_same_card_uses_one_duplex_profile() {
        // Real dump: card on `output:iec958-stereo`; the job wants analog output and
        // analog input → one switch to the duplex profile
        let plan = joint(&snap(REAL), &[OUT_REAL], &[IN_REAL]);
        let (sink_index, sink_plan) = plan.sink.expect("sink planned");
        assert_eq!(sink_index, 0);
        let sw = switch(sink_plan);
        assert_eq!(sw.profile_name, "output:analog-stereo+input:analog-stereo");
        let Some((0, SourceStep::WithSinkSwitch(source_sw))) = plan.source else {
            panic!("source rides on the sink's switch: {:?}", plan.source);
        };
        assert_eq!(source_sw.profile_index, sw.profile_index);
        assert_eq!(source_sw.kind, Endpoint::Source);
        assert_eq!(source_sw.mapping, "analog-stereo");
    }

    #[test]
    fn test_joint_plan_prefers_the_profile_with_both_mappings() {
        // Synthetic PCI card: digital output with the `analog-mic` input exists only
        // as profile 4 (the sink alone would keep the current `analog-stereo` input)
        let s = snap(SYNTH);
        let alone = switch(
            resolve(&s, "alsa_output.pci-0000_00_1f.3.iec958-stereo").expect("target resolves"),
        );
        assert_eq!(alone.profile_index, 3);
        let plan = joint(&s, &["alsa_output.pci-0000_00_1f.3.iec958-stereo"], &[
            "alsa_input.pci-0000_00_1f.3.analog-mic",
        ]);
        let sw = switch(plan.sink.expect("sink").1);
        assert_eq!(sw.profile_name, "output:iec958-stereo+input:analog-mic");
        assert!(matches!(
            plan.source,
            Some((0, SourceStep::WithSinkSwitch(_)))
        ));
    }

    #[test]
    fn test_joint_plan_impossible_combination_sink_wins() {
        // Surround 5.1 with an input is unavailable: the sink still switches (to the
        // output-only profile) and the source is reported unreachable
        let s = snap(SYNTH);
        let plan = joint(&s, &["alsa_output.pci-0000_00_1f.3.analog-surround-51"], &[
            "alsa_input.pci-0000_00_1f.3.analog-stereo",
        ]);
        let sw = switch(plan.sink.expect("the sink wins").1);
        assert_eq!(sw.profile_name, "output:analog-surround-51");
        assert_eq!(plan.source, None);
        let [(target, why)] = plan.source_skipped.as_slice() else {
            panic!("one skipped source: {:?}", plan.source_skipped);
        };
        assert_eq!(target, "alsa_input.pci-0000_00_1f.3.analog-stereo");
        assert!(
            why.contains("not reachable together with the sink"),
            "{why}"
        );

        // A fallback source on another card is still used
        let plan = joint(&s, &["alsa_output.pci-0000_00_1f.3.analog-surround-51"], &[
            "alsa_input.pci-0000_00_1f.3.analog-stereo",
            "alsa_input.usb-Example_USB_Headset-00.mono-fallback",
        ]);
        assert_eq!(
            plan.source,
            Some((
                1,
                SourceStep::Plan(Plan::SetDefault {
                    node_name: "alsa_input.usb-Example_USB_Headset-00.mono-fallback".to_string()
                })
            ))
        );
    }

    #[test]
    fn test_joint_plan_different_cards_are_independent() {
        // USB headset sink needs a switch (card on input-only); the PCI mic exists
        let s = snap(SYNTH);
        let plan = joint(
            &s,
            &["alsa_output.usb-Example_USB_Headset-00.analog-stereo"],
            &["alsa_input.pci-0000_00_1f.3.analog-stereo"],
        );
        let sw = switch(plan.sink.expect("sink").1);
        assert_eq!(sw.device_id, 60);
        assert_eq!(sw.profile_name, "output:analog-stereo+input:mono-fallback");
        assert_eq!(
            plan.source,
            Some((
                0,
                SourceStep::Plan(Plan::SetDefault {
                    node_name: "alsa_input.pci-0000_00_1f.3.analog-stereo".to_string()
                })
            ))
        );
    }

    #[test]
    fn test_source_switch_on_the_sinks_card_must_keep_the_sink() {
        // The sink stays on the existing analog output; the analog-mic input exists
        // only with the digital output, so it is unreachable for this job
        let s = snap(SYNTH);
        let plan = joint(&s, &["alsa_output.pci-0000_00_1f.3.analog-stereo"], &[
            "alsa_input.pci-0000_00_1f.3.analog-mic",
        ]);
        assert!(matches!(plan.sink, Some((0, Plan::SetDefault { .. }))));
        assert_eq!(plan.source, None);
        assert_eq!(plan.source_skipped.len(), 1);

        // A card this job already switched is never switched again for the source
        let real = snap(REAL);
        let frozen: HashSet<u32> = [43].into_iter().collect();
        let plan = plan_joint(&real, &[], &names(&[IN_REAL]), None, &frozen);
        assert_eq!(plan.source, None);
        assert!(
            plan.source_skipped[0].1.contains("already switched"),
            "{:?}",
            plan.source_skipped
        );
    }
}
