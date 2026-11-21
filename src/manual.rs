//! Manual sink switching (`set-sink`, `next-sink`, `prev-sink`)
//!
//! Which sink a request leads to is decided against what `PipeWire` reports right now
//! (a [`Probe`]): the real default sink is the smart-toggle reference and the base for
//! cycling, and cycling skips configured sinks that cannot be activated at the moment
//! (e.g. a TV that is switched off). An explicit `set-sink` always tries its sink, so
//! its error says why it is unavailable.
//!
//! Pure, and shared by the daemon's activation worker and the CLI's local fallback
//! (no daemon running), so both behave the same.

use std::fmt::Write as _;

use tracing::info;

use crate::config::{Config, SinkConfig};
use crate::pipewire::{PipeWire, Snapshot};

/// Reply when there is nothing to cycle through
const ONE_SINK: &str = "Only one sink configured, nothing to cycle";

/// A manual switch request
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManualSwitch {
    /// `set-sink <ref>`: a 1-based position, description or node name
    Set(String),
    /// `next-sink`
    Next,
    /// `prev-sink`
    Prev,
}

/// What `PipeWire` says right now, as far as manual switching cares
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Probe {
    /// The real default sink's node name, if `PipeWire` has one
    pub current: Option<String>,
    /// Configured sinks (node names) that cannot be activated right now, with why
    pub unreachable: Vec<(String, String)>,
}

impl Probe {
    /// Probe a `PipeWire` snapshot for every sink in `config`
    ///
    /// A sink is reachable when the activation resolver finds a way to make it the
    /// default: its node exists, or an available card profile provides it.
    #[must_use]
    pub fn from_snapshot(snapshot: &Snapshot, config: &Config) -> Self {
        Self {
            current: snapshot.current_default().map(str::to_owned),
            unreachable: config
                .sinks
                .iter()
                .filter_map(|sink| {
                    PipeWire::check_reachable(snapshot, &sink.name)
                        .err()
                        .map(|e| (sink.name.clone(), format!("{e:#}")))
                })
                .collect(),
        }
    }

    /// Why `name` cannot be activated right now, or `None` if it can
    fn why_unreachable(&self, name: &str) -> Option<&str> {
        self.unreachable
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, why)| why.as_str())
    }
}

/// Where a manual request leads
#[derive(Debug, PartialEq, Eq)]
pub enum ManualTarget<'a> {
    /// Activate this sink
    Switch(&'a SinkConfig),
    /// Nothing to do; reply with this message
    Done(String),
}

/// The error for a `set-sink` reference that names no configured sink
fn unknown_sink(config: &Config, sink_ref: &str) -> String {
    let available: Vec<String> = config
        .sinks
        .iter()
        .enumerate()
        .map(|(i, s)| format!("{}. '{}'", i + 1, s.desc))
        .collect();
    format!(
        "Unknown sink '{sink_ref}'. Available: [{}]",
        available.join(", ")
    )
}

/// The answer to a request that needs no look at `PipeWire`, if it has one
///
/// `Some(Err(_))` for an unknown `set-sink` reference, `Some(Ok(message))` when there is
/// nothing to cycle through, `None` when [`plan`] needs a [`Probe`].
#[must_use]
pub fn answer_without_pipewire(
    config: &Config,
    request: &ManualSwitch,
) -> Option<Result<String, String>> {
    match request {
        ManualSwitch::Set(sink_ref) => config
            .resolve_sink(sink_ref)
            .is_none()
            .then(|| Err(unknown_sink(config, sink_ref))),
        ManualSwitch::Next | ManualSwitch::Prev => {
            (config.sinks.len() < 2).then(|| Ok(ONE_SINK.to_string()))
        }
    }
}

/// The sinks a cycle may move to from `current`, in the order to try them
///
/// Starts after `current` (after the default sink when `current` is not a configured
/// sink), wraps around, and leaves out `current` itself.
fn cycle_order<'a>(
    config: &'a Config,
    current: Option<&str>,
    forward: bool,
) -> Vec<&'a SinkConfig> {
    let count = config.sinks.len();
    let start = current
        .and_then(|name| config.sinks.iter().position(|s| s.name == name))
        .or_else(|| config.sinks.iter().position(|s| s.default))
        .unwrap_or(0);
    (1..=count)
        .map(|step| {
            if forward {
                (start + step) % count
            } else {
                (start + count - step) % count
            }
        })
        .map(|i| &config.sinks[i])
        .filter(|s| current != Some(s.name.as_str()))
        .collect()
}

/// Decide where a manual request leads, given what `PipeWire` reports
///
/// # Errors
/// Returns the message to report when the `set-sink` reference names no configured
/// sink, or when no other sink can be activated for a cycle.
pub fn plan<'a>(
    config: &'a Config,
    request: &ManualSwitch,
    probe: &Probe,
) -> Result<ManualTarget<'a>, String> {
    match request {
        ManualSwitch::Set(sink_ref) => {
            let target = config
                .resolve_sink(sink_ref)
                .ok_or_else(|| unknown_sink(config, sink_ref))?;
            if config.settings.set_smart_toggle
                && probe.current.as_deref() == Some(target.name.as_str())
                && let Some(default) = config.get_default_sink()
            {
                if default.name == target.name {
                    return Ok(ManualTarget::Done(format!("Already on: {}", default.desc)));
                }
                return Ok(ManualTarget::Switch(default));
            }
            Ok(ManualTarget::Switch(target))
        }
        ManualSwitch::Next | ManualSwitch::Prev => {
            if config.sinks.len() < 2 {
                return Ok(ManualTarget::Done(ONE_SINK.to_string()));
            }
            let candidates = cycle_order(
                config,
                probe.current.as_deref(),
                *request == ManualSwitch::Next,
            );
            for sink in &candidates {
                match probe.why_unreachable(&sink.name) {
                    None => return Ok(ManualTarget::Switch(sink)),
                    Some(why) => info!("Skipping {} (cannot be activated now): {why}", sink.desc),
                }
            }
            let mut message = "No other sink can be activated right now:".to_string();
            for sink in &candidates {
                let why = probe.why_unreachable(&sink.name).unwrap_or_default();
                let _ = write!(message, "\n  {}: {why}", sink.desc);
            }
            Err(message)
        }
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;
    use crate::test_utils::fixtures::{make_config, make_sink};

    /// Speakers (default), HDMI, Headphones, USB
    fn config() -> Config {
        make_config(
            vec![
                make_sink("speakers", "Speakers", true),
                make_sink("hdmi", "HDMI", false),
                make_sink("headphones", "Headphones", false),
                make_sink("usb", "USB", false),
            ],
            vec![],
        )
    }

    fn probe(current: Option<&str>, unreachable: &[&str]) -> Probe {
        Probe {
            current: current.map(str::to_owned),
            unreachable: unreachable
                .iter()
                .map(|n| ((*n).to_string(), format!("{n} is off")))
                .collect(),
        }
    }

    fn switch_to(result: Result<ManualTarget<'_>, String>) -> &str {
        match result {
            Ok(ManualTarget::Switch(sink)) => &sink.name,
            other => panic!("expected a switch, got {other:?}"),
        }
    }

    #[test]
    fn test_cycle_wraps_from_the_real_default() {
        let c = config();
        let next = |current| switch_to(plan(&c, &ManualSwitch::Next, &probe(current, &[])));
        let prev = |current| switch_to(plan(&c, &ManualSwitch::Prev, &probe(current, &[])));
        assert_eq!(next(Some("headphones")), "usb");
        assert_eq!(next(Some("usb")), "speakers");
        assert_eq!(prev(Some("speakers")), "usb");
        // Not a configured sink (or unknown): start from the default, which stays a
        // candidate because audio is not on it
        assert_eq!(next(Some("elsewhere")), "hdmi");
        assert_eq!(prev(None), "usb");
    }

    #[test]
    fn test_cycle_skips_unreachable_sinks() {
        let c = config();
        // HDMI is off: forwards from Speakers steps over it
        let p = probe(Some("speakers"), &["hdmi"]);
        assert_eq!(switch_to(plan(&c, &ManualSwitch::Next, &p)), "headphones");
        // Backwards from Headphones too; with Speakers also off, wrap to USB
        let p = probe(Some("headphones"), &["hdmi"]);
        assert_eq!(switch_to(plan(&c, &ManualSwitch::Prev, &p)), "speakers");
        let p = probe(Some("headphones"), &["hdmi", "speakers"]);
        assert_eq!(switch_to(plan(&c, &ManualSwitch::Prev, &p)), "usb");
    }

    #[test]
    fn test_cycle_with_nothing_reachable_is_a_clear_error() {
        let c = config();
        let p = probe(Some("headphones"), &["speakers", "hdmi", "usb"]);
        let err = plan(&c, &ManualSwitch::Next, &p).expect_err("nothing reachable");
        assert!(
            err.starts_with("No other sink can be activated right now:"),
            "{err}"
        );
        for line in [
            "USB: usb is off",
            "Speakers: speakers is off",
            "HDMI: hdmi is off",
        ] {
            assert!(err.contains(line), "{err}");
        }
        // The current sink being unreachable does not matter for leaving it
        let p = probe(Some("headphones"), &["headphones"]);
        assert_eq!(switch_to(plan(&c, &ManualSwitch::Next, &p)), "usb");
    }

    #[test]
    fn test_set_sink_tries_unreachable_sink_and_toggles_on_real_default() {
        let c = config();
        let set = |r: &str| ManualSwitch::Set(r.to_string());
        // Explicit set never skips
        assert_eq!(
            switch_to(plan(&c, &set("HDMI"), &probe(Some("speakers"), &["hdmi"]))),
            "hdmi"
        );
        // Smart toggle compares with the real default
        assert_eq!(
            switch_to(plan(&c, &set("HDMI"), &probe(Some("hdmi"), &[]))),
            "speakers"
        );
        assert_eq!(
            plan(&c, &set("Speakers"), &probe(Some("speakers"), &[])),
            Ok(ManualTarget::Done("Already on: Speakers".to_string()))
        );
        assert_eq!(
            switch_to(plan(&c, &set("2"), &probe(Some("speakers"), &[]))),
            "hdmi"
        );
        assert!(plan(&c, &set("nope"), &probe(None, &[])).is_err());

        let mut toggle_off = config();
        toggle_off.settings.set_smart_toggle = false;
        assert_eq!(
            switch_to(plan(&toggle_off, &set("HDMI"), &probe(Some("hdmi"), &[]))),
            "hdmi"
        );
    }

    #[test]
    fn test_answer_without_pipewire() {
        let c = config();
        assert_eq!(answer_without_pipewire(&c, &ManualSwitch::Next), None);
        assert_eq!(
            answer_without_pipewire(&c, &ManualSwitch::Set("USB".to_string())),
            None
        );
        assert!(matches!(
            answer_without_pipewire(&c, &ManualSwitch::Set("nope".to_string())),
            Some(Err(message)) if message.starts_with("Unknown sink 'nope'")
        ));
        let one = make_config(vec![make_sink("speakers", "Speakers", true)], vec![]);
        assert_eq!(
            answer_without_pipewire(&one, &ManualSwitch::Prev),
            Some(Ok(ONE_SINK.to_string()))
        );
    }

    /// Reachability from a real `pw-dump` shape: an output whose every profile is
    /// marked unavailable (HDMI with nothing plugged in) is skipped
    #[test]
    fn test_probe_from_snapshot_uses_the_resolver() {
        let snapshot = crate::pipewire::parse_snapshot(include_bytes!(
            "../tests/fixtures/pwdump-synthetic.json"
        ))
        .expect("fixture parses");
        let c = make_config(
            vec![
                make_sink(
                    "alsa_output.pci-0000_00_1f.3.analog-stereo",
                    "Speakers",
                    true,
                ),
                make_sink("alsa_output.pci-0000_00_1f.3.hdmi-stereo", "TV", false),
                make_sink(
                    "alsa_output.usb-Example_USB_Headset-00.analog-stereo",
                    "Headset",
                    false,
                ),
            ],
            vec![],
        );
        let p = Probe::from_snapshot(&snapshot, &c);
        assert_eq!(
            p.current.as_deref(),
            Some("alsa_output.pci-0000_00_1f.3.analog-stereo")
        );
        let unreachable: Vec<&str> = p.unreachable.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(unreachable, vec![
            "alsa_output.pci-0000_00_1f.3.hdmi-stereo"
        ]);
        assert!(p.unreachable[0].1.contains("marked unavailable"));
        // Speakers → next skips the TV and lands on the headset (a profile switch away)
        assert_eq!(
            switch_to(plan(&c, &ManualSwitch::Next, &p)),
            "alsa_output.usb-Example_USB_Headset-00.analog-stereo"
        );
    }
}
