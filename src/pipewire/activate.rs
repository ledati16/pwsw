//! Sink activation: resolve a configured node name against a fresh snapshot, switch
//! the card profile if needed, wait for the sink node, set it as default.
//!
//! A rule with fallbacks hands over several candidates: [`activate_first`] resolves
//! them in order against one snapshot and activates the first reachable one, moving
//! on to the next reachable one if that activation fails.
//!
//! Everything goes through [`AudioBackend`] and [`Clock`], so the whole flow is
//! unit-tested without `PipeWire` and without real sleeps.

use std::fmt::Write as _;
use std::time::{Duration, Instant};

use color_eyre::eyre::{self, Context, Result};
use tracing::{debug, info, warn};

use super::model::Snapshot;
use super::resolve::{self, Plan, ProfileSwitch};

/// The operations activation needs from an audio server.
pub(crate) trait AudioBackend {
    /// Take a fresh snapshot of devices, sinks and defaults.
    fn snapshot(&self) -> Result<Snapshot>;
    /// Set the configured default sink by node name.
    fn set_default(&self, node_name: &str) -> Result<()>;
    /// Switch a device to a profile (persistently).
    fn set_profile(&self, device_id: u32, profile_index: u32) -> Result<()>;
}

/// Time source for the post-switch wait (injectable for tests).
pub(crate) trait Clock {
    fn now(&self) -> Instant;
    fn sleep(&self, duration: Duration);
}

/// The real clock.
pub(crate) struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Instant {
        Instant::now()
    }

    fn sleep(&self, duration: Duration) {
        std::thread::sleep(duration);
    }
}

/// Default poll interval while waiting for a sink after a profile switch.
/// Override with `PROFILE_SWITCH_DELAY_MS`.
const DEFAULT_POLL_INTERVAL_MS: u64 = 150;
/// Default number of polls. Override with `PROFILE_SWITCH_MAX_RETRIES`.
const DEFAULT_MAX_POLLS: u64 = 10;
/// Bounds for `PROFILE_SWITCH_DELAY_MS`.
const MIN_POLL_INTERVAL_MS: u64 = 10;
const MAX_POLL_INTERVAL_MS: u64 = 1_000;
/// Hard cap on the total wait (interval × polls) of one activation.
///
/// This does not bound how long an `IPC` client waits: a manual switch queued behind
/// an in-flight activation waits for both. The daemon's guard is its 8 s "switch in
/// progress" reply (`SWITCH_REPLY_TIMEOUT`), below the client's 10 s read timeout.
const MAX_TOTAL_WAIT_MS: u64 = 5_000;

/// How long to wait for the sink node after a profile switch, and how often to look.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct WaitPolicy {
    pub(crate) poll_interval: Duration,
    pub(crate) timeout: Duration,
}

impl WaitPolicy {
    /// From `PROFILE_SWITCH_DELAY_MS` / `PROFILE_SWITCH_MAX_RETRIES`.
    pub(crate) fn from_env() -> Self {
        Self::from_values(
            std::env::var("PROFILE_SWITCH_DELAY_MS").ok().as_deref(),
            std::env::var("PROFILE_SWITCH_MAX_RETRIES").ok().as_deref(),
        )
    }

    /// Parse the two env values. Invalid or absent values use the defaults; the
    /// interval is clamped to 10–1000 ms and the total wait to 5 s.
    fn from_values(delay_ms: Option<&str>, max_retries: Option<&str>) -> Self {
        let interval = delay_ms
            .and_then(|v| v.trim().parse::<u64>().ok())
            .unwrap_or(DEFAULT_POLL_INTERVAL_MS)
            .clamp(MIN_POLL_INTERVAL_MS, MAX_POLL_INTERVAL_MS);
        let polls = max_retries
            .and_then(|v| v.trim().parse::<u64>().ok())
            .unwrap_or(DEFAULT_MAX_POLLS)
            .max(1);
        let total = interval.saturating_mul(polls).min(MAX_TOTAL_WAIT_MS);
        Self {
            poll_interval: Duration::from_millis(interval),
            timeout: Duration::from_millis(total),
        }
    }
}

/// What an activation did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Activation {
    /// The node that is now the configured default (its actual name).
    pub(crate) node_name: String,
    /// The profile switch performed first, if any.
    pub(crate) profile_switch: Option<ProfileSwitch>,
}

/// Which candidate an activation chose, and what it did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Chosen {
    /// Index of the activated candidate (0 = the first choice).
    pub(crate) index: usize,
    pub(crate) activation: Activation,
}

/// Make `target` (a configured node name) the default sink.
///
/// The snapshot and the decision are taken here, so a caller that serialises calls
/// (one activation at a time) never acts on a stale view.
///
/// # Errors
/// Returns an error if the snapshot fails, the target cannot be resolved, the profile
/// switch fails, the sink node does not appear within `policy.timeout`, or setting
/// the default fails.
#[cfg(test)]
pub(crate) fn activate(
    backend: &dyn AudioBackend,
    clock: &dyn Clock,
    policy: WaitPolicy,
    target: &str,
) -> Result<Activation> {
    let chosen = activate_first(backend, clock, policy, &[target.to_string()], None)?;
    chosen
        .map(|c| c.activation)
        .ok_or_else(|| eyre::eyre!("Internal error: nothing activated"))
}

/// Make the first reachable of `candidates` (configured node names, in priority
/// order) the default sink.
///
/// Every candidate is resolved against one fresh snapshot, in order, and the first
/// one that resolves (present, or reachable by a profile switch) is activated. If
/// that activation fails (e.g. the profile switch did not produce the node), the
/// remaining candidates are resolved against a new snapshot (the failed switch may
/// have changed the graph) and the next reachable one is tried.
///
/// With `upgrade_from: Some(k)` the caller already has candidate `k` as the default:
/// nothing is done (`Ok(None)`) unless a candidate before `k` is reachable now; if
/// one is, activation proceeds as above over the whole list.
///
/// # Errors
/// Returns an error if `candidates` is empty, a snapshot fails, or no candidate could
/// be activated (for a single candidate, its own error; otherwise one naming every
/// candidate and why it failed).
pub(crate) fn activate_first(
    backend: &dyn AudioBackend,
    clock: &dyn Clock,
    policy: WaitPolicy,
    candidates: &[String],
    upgrade_from: Option<usize>,
) -> Result<Option<Chosen>> {
    if candidates.is_empty() {
        eyre::bail!("No sink to activate");
    }
    let mut snapshot = backend.snapshot()?;
    if let Some(current) = upgrade_from {
        let better = candidates[..current.min(candidates.len())]
            .iter()
            .any(|c| resolve::resolve(&snapshot, c).is_ok());
        if !better {
            return Ok(None);
        }
    }

    let mut failures: Vec<(&str, eyre::Report)> = Vec::new();
    for (index, target) in candidates.iter().enumerate() {
        let plan = match resolve::resolve(&snapshot, target) {
            Ok(plan) => plan,
            Err(e) => {
                debug!("Skipping '{target}': {e:#}");
                failures.push((target, e));
                continue;
            }
        };
        match execute(backend, clock, policy, plan, target) {
            Ok(activation) => return Ok(Some(Chosen { index, activation })),
            Err(e) => {
                if index + 1 < candidates.len() {
                    warn!("Activating '{target}' failed, trying the next sink: {e:#}");
                    snapshot = backend
                        .snapshot()
                        .wrap_err_with(|| format!("after activating '{target}' failed"))?;
                }
                failures.push((target, e));
            }
        }
    }

    if failures.len() == 1
        && let Some((_, e)) = failures.pop()
    {
        return Err(e);
    }
    let mut message = String::from("every candidate failed:");
    for (target, e) in &failures {
        let _ = write!(message, "\n  - '{target}': {e:#}");
    }
    Err(eyre::eyre!(message))
}

/// Carry out a resolved plan for `target`.
fn execute(
    backend: &dyn AudioBackend,
    clock: &dyn Clock,
    policy: WaitPolicy,
    plan: Plan,
    target: &str,
) -> Result<Activation> {
    let switch = match plan {
        Plan::SetDefault { node_name } => {
            backend.set_default(&node_name)?;
            return Ok(Activation {
                node_name,
                profile_switch: None,
            });
        }
        Plan::SwitchProfile(switch) => switch,
    };

    debug!(
        "Switching {} to profile {} ({}) for '{}'",
        switch.device_name, switch.profile_index, switch.profile_name, target
    );
    backend.set_profile(switch.device_id, switch.profile_index)?;

    let node_name = wait_for_node(backend, clock, policy, &switch, target)?;
    if node_name != target {
        info!("Sink '{}' appeared as '{}'", target, node_name);
    }
    backend.set_default(&node_name)?;
    Ok(Activation {
        node_name,
        profile_switch: Some(switch),
    })
}

/// Poll snapshots until the switch's sink node appears or the deadline passes.
fn wait_for_node(
    backend: &dyn AudioBackend,
    clock: &dyn Clock,
    policy: WaitPolicy,
    switch: &ProfileSwitch,
    target: &str,
) -> Result<String> {
    let start = clock.now();
    let mut polls = 0u32;
    let mut last_error = None;
    loop {
        let elapsed = clock.now().saturating_duration_since(start);
        if elapsed >= policy.timeout {
            break;
        }
        clock.sleep(
            policy
                .poll_interval
                .min(policy.timeout.saturating_sub(elapsed)),
        );
        polls += 1;
        match backend.snapshot() {
            Ok(snapshot) => {
                if let Some(node) = snapshot.sinks.iter().find(|n| switch.produced(n)) {
                    return Ok(node.name.clone());
                }
                debug!("Waiting for sink '{}' (poll {})", target, polls);
            }
            Err(e) => {
                debug!("pw-dump failed while waiting for '{}': {:#}", target, e);
                last_error = Some(e);
            }
        }
    }

    let cause = last_error.map_or_else(String::new, |e| format!("\nLast pw-dump error: {e:#}"));
    eyre::bail!(
        "Switched {} to profile '{}' but its sink for '{}' did not appear within {} ms.\n\
         The device may need more time (raise PROFILE_SWITCH_DELAY_MS / \
         PROFILE_SWITCH_MAX_RETRIES; total wait is capped at {} ms), or the port may \
         have no hardware behind it. Check `pwsw list-sinks`.{cause}",
        switch.device_name,
        switch.profile_name,
        target,
        policy.timeout.as_millis(),
        MAX_TOTAL_WAIT_MS,
    )
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};

    use pretty_assertions::assert_eq;

    use super::*;
    use crate::pipewire::model::SinkNode;
    use crate::pipewire::parse::parse_dump;

    const REAL: &str = include_str!("../../tests/fixtures/pwdump-alsa-profiles.json");

    #[derive(Debug, Clone, PartialEq, Eq)]
    enum Call {
        Snapshot,
        SetDefault(String),
        SetProfile(u32, u32),
    }

    /// In-memory backend: a profile switch removes the device's sinks and makes the
    /// new profile's sink appear after `appear_after` further snapshots.
    struct FakeBackend {
        state: RefCell<Snapshot>,
        appear_after: Option<usize>,
        rename: Option<&'static str>,
        /// A switch to this output mapping never produces its sink
        never_appears: Option<&'static str>,
        pending: RefCell<Option<(SinkNode, usize)>>,
        calls: RefCell<Vec<Call>>,
    }

    impl FakeBackend {
        fn new(appear_after: Option<usize>) -> Self {
            Self {
                state: RefCell::new(parse_dump(REAL.as_bytes()).expect("fixture parses")),
                appear_after,
                rename: None,
                never_appears: None,
                pending: RefCell::new(None),
                calls: RefCell::new(Vec::new()),
            }
        }

        fn calls(&self) -> Vec<Call> {
            self.calls.borrow().clone()
        }
    }

    impl AudioBackend for FakeBackend {
        fn snapshot(&self) -> Result<Snapshot> {
            self.calls.borrow_mut().push(Call::Snapshot);
            let mut pending = self.pending.borrow_mut();
            if let Some((node, left)) = pending.as_mut() {
                if *left == 0 {
                    self.state.borrow_mut().sinks.push(node.clone());
                    *pending = None;
                } else {
                    *left -= 1;
                }
            }
            Ok(self.state.borrow().clone())
        }

        fn set_default(&self, node_name: &str) -> Result<()> {
            self.calls
                .borrow_mut()
                .push(Call::SetDefault(node_name.to_string()));
            self.state.borrow_mut().defaults.configured = Some(node_name.to_string());
            Ok(())
        }

        fn set_profile(&self, device_id: u32, profile_index: u32) -> Result<()> {
            self.calls
                .borrow_mut()
                .push(Call::SetProfile(device_id, profile_index));
            let mut state = self.state.borrow_mut();
            state.sinks.retain(|s| s.device_id != Some(device_id));
            let device = state
                .devices
                .iter_mut()
                .find(|d| d.id == device_id)
                .expect("device exists");
            device.active_profile = Some(profile_index);
            let profile = device
                .profiles
                .iter()
                .find(|p| p.index == profile_index)
                .expect("profile exists");
            let mapping = profile.output_mappings().next().expect("output profile");
            let suffix = device.name.strip_prefix("alsa_card.").expect("alsa card");
            let node = SinkNode {
                id: 1000,
                name: self
                    .rename
                    .map_or_else(|| format!("alsa_output.{suffix}.{mapping}"), String::from),
                description: String::new(),
                device_id: Some(device_id),
                profile_name: Some(mapping.to_string()),
                card_profile_device: profile.sink_card_devices.first().copied(),
            };
            if let Some(n) = self.appear_after
                && self.never_appears != Some(mapping)
            {
                *self.pending.borrow_mut() = Some((node, n));
            }
            Ok(())
        }
    }

    /// Virtual clock: `sleep` advances time instantly.
    struct FakeClock {
        base: Instant,
        offset: Cell<Duration>,
    }

    impl FakeClock {
        fn new() -> Self {
            Self {
                base: Instant::now(),
                offset: Cell::new(Duration::ZERO),
            }
        }
    }

    impl Clock for FakeClock {
        fn now(&self) -> Instant {
            self.base + self.offset.get()
        }

        fn sleep(&self, duration: Duration) {
            self.offset.set(self.offset.get() + duration);
        }
    }

    const POLICY: WaitPolicy = WaitPolicy {
        poll_interval: Duration::from_millis(100),
        timeout: Duration::from_millis(1_000),
    };
    const ANALOG: &str = "alsa_output.pci-0000_0c_00.4.analog-stereo";

    #[test]
    fn test_activate_active_sink_only_sets_default() {
        let backend = FakeBackend::new(Some(0));
        let name = "alsa_output.pci-0000_0c_00.4.iec958-stereo";
        let act = activate(&backend, &FakeClock::new(), POLICY, name).expect("activates");
        assert_eq!(act.node_name, name);
        assert_eq!(act.profile_switch, None);
        assert_eq!(backend.calls(), vec![
            Call::Snapshot,
            Call::SetDefault(name.to_string())
        ]);
    }

    #[test]
    fn test_activate_switches_profile_then_sets_default() {
        let backend = FakeBackend::new(Some(2));
        let clock = FakeClock::new();
        let act = activate(&backend, &clock, POLICY, ANALOG).expect("activates");
        assert_eq!(act.node_name, ANALOG);
        assert_eq!(
            act.profile_switch.map(|s| s.profile_index),
            Some(1),
            "duplex profile keeps the microphone"
        );
        assert_eq!(backend.calls(), vec![
            Call::Snapshot,
            Call::SetProfile(43, 1),
            Call::Snapshot,
            Call::Snapshot,
            Call::Snapshot,
            Call::SetDefault(ANALOG.to_string()),
        ]);
        assert_eq!(clock.offset.get(), Duration::from_millis(300));
    }

    #[test]
    fn test_activate_uses_appearing_nodes_real_name() {
        let mut backend = FakeBackend::new(Some(0));
        backend.rename = Some("renamed.by.wireplumber");
        let act = activate(&backend, &FakeClock::new(), POLICY, ANALOG).expect("activates");
        assert_eq!(act.node_name, "renamed.by.wireplumber");
        assert_eq!(
            backend.calls().last(),
            Some(&Call::SetDefault("renamed.by.wireplumber".to_string()))
        );
    }

    #[test]
    fn test_activate_node_never_appears_errors_after_deadline() {
        let backend = FakeBackend::new(None);
        let clock = FakeClock::new();
        let err = activate(&backend, &clock, POLICY, ANALOG).unwrap_err();
        assert!(
            format!("{err}").contains("did not appear within 1000 ms"),
            "{err}"
        );
        assert_eq!(clock.offset.get(), POLICY.timeout);
        assert!(
            !backend
                .calls()
                .iter()
                .any(|c| matches!(c, Call::SetDefault(_)))
        );
        // One initial snapshot plus one per 100 ms poll.
        let snapshots = backend
            .calls()
            .iter()
            .filter(|c| **c == Call::Snapshot)
            .count();
        assert_eq!(snapshots, 11);
    }

    #[test]
    fn test_activate_unknown_sink_touches_nothing() {
        let backend = FakeBackend::new(Some(0));
        let err = activate(&backend, &FakeClock::new(), POLICY, "nope").unwrap_err();
        assert!(format!("{err}").contains("is not present"), "{err}");
        assert_eq!(backend.calls(), vec![Call::Snapshot]);
    }

    const IEC: &str = "alsa_output.pci-0000_0c_00.4.iec958-stereo";

    fn names(names: &[&str]) -> Vec<String> {
        names.iter().map(|n| (*n).to_string()).collect()
    }

    #[test]
    fn test_activate_first_skips_unreachable_candidates() {
        let backend = FakeBackend::new(Some(0));
        let chosen = activate_first(
            &backend,
            &FakeClock::new(),
            POLICY,
            &names(&["bluez_output.gone", IEC]),
            None,
        )
        .expect("activates")
        .expect("something chosen");
        assert_eq!(chosen.index, 1);
        assert_eq!(chosen.activation.node_name, IEC);
        assert_eq!(backend.calls(), vec![
            Call::Snapshot,
            Call::SetDefault(IEC.to_string())
        ]);
    }

    #[test]
    fn test_activate_first_prefers_the_first_reachable() {
        let backend = FakeBackend::new(Some(0));
        let chosen = activate_first(
            &backend,
            &FakeClock::new(),
            POLICY,
            &names(&[ANALOG, IEC]),
            None,
        )
        .expect("activates")
        .expect("something chosen");
        assert_eq!(chosen.index, 0, "a profile switch away is still reachable");
        assert_eq!(chosen.activation.node_name, ANALOG);
    }

    #[test]
    fn test_activate_first_falls_through_when_activation_fails() {
        let mut backend = FakeBackend::new(Some(0));
        backend.never_appears = Some("analog-stereo");
        let chosen = activate_first(
            &backend,
            &FakeClock::new(),
            POLICY,
            &names(&[ANALOG, IEC]),
            None,
        )
        .expect("the second candidate activates")
        .expect("something chosen");
        assert_eq!(chosen.index, 1);
        assert_eq!(chosen.activation.node_name, IEC);
        let calls = backend.calls();
        // The failed switch removed the digital sink, so the fresh snapshot taken after
        // the failure leads to a switch back to the digital profile
        let switches: Vec<_> = calls
            .iter()
            .filter(|c| matches!(c, Call::SetProfile(..)))
            .collect();
        assert_eq!(switches.len(), 2, "{calls:?}");
        assert_eq!(calls.last(), Some(&Call::SetDefault(IEC.to_string())));
    }

    #[test]
    fn test_activate_first_none_reachable_names_every_candidate() {
        let backend = FakeBackend::new(Some(0));
        let err = activate_first(
            &backend,
            &FakeClock::new(),
            POLICY,
            &names(&["bluez_output.a", "bluez_output.b"]),
            None,
        )
        .unwrap_err();
        let text = format!("{err:#}");
        assert!(text.contains("every candidate failed"), "{text}");
        assert!(
            text.contains("'bluez_output.a'") && text.contains("'bluez_output.b'"),
            "{text}"
        );
        assert_eq!(backend.calls(), vec![Call::Snapshot]);
    }

    #[test]
    fn test_activate_first_upgrade_only_acts_for_a_better_candidate() {
        // Candidate 1 is the current default; candidate 0 is still unreachable
        let backend = FakeBackend::new(Some(0));
        let chosen = activate_first(
            &backend,
            &FakeClock::new(),
            POLICY,
            &names(&["bluez_output.gone", IEC]),
            Some(1),
        )
        .expect("no error");
        assert_eq!(chosen, None);
        assert_eq!(backend.calls(), vec![Call::Snapshot], "nothing touched");

        // Now candidate 0 is reachable: it is activated
        let backend = FakeBackend::new(Some(0));
        let chosen = activate_first(
            &backend,
            &FakeClock::new(),
            POLICY,
            &names(&[IEC, "bluez_output.gone"]),
            Some(1),
        )
        .expect("activates")
        .expect("upgraded");
        assert_eq!(chosen.index, 0);
    }

    #[rstest::rstest]
    #[case::unset(None, None, 150, 1_500)]
    #[case::custom(Some("200"), Some("10"), 200, 2_000)]
    #[case::total_capped(Some("1000"), Some("100"), 1_000, 5_000)]
    #[case::extreme(Some("999999999"), Some("999999999"), 1_000, 5_000)]
    #[case::invalid(Some("x"), Some("-1"), 150, 1_500)]
    #[case::tiny(Some("0"), Some("0"), 10, 10)]
    fn test_wait_policy_from_values(
        #[case] delay: Option<&str>,
        #[case] retries: Option<&str>,
        #[case] interval_ms: u64,
        #[case] total_ms: u64,
    ) {
        assert_eq!(WaitPolicy::from_values(delay, retries), WaitPolicy {
            poll_interval: Duration::from_millis(interval_ms),
            timeout: Duration::from_millis(total_ms),
        });
    }
}
