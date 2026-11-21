//! Where a configured sink (or source) stands in one `PipeWire` snapshot; shared by
//! `pwsw list-sinks` and the TUI, so both use the same words.

use super::{ActiveSink, ProfileSink};

/// Where a configured sink (or source) stands in a snapshot
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeState {
    /// The node exists and is the current default (audio plays, or is recorded, here)
    Current,
    /// The node exists
    Present,
    /// The node appears after a device profile switch
    ViaProfile,
    /// Not found
    Missing,
}

impl NodeState {
    /// State of the node `name` among the `active` nodes and those `profiles` create
    #[must_use]
    pub fn of(name: &str, active: &[ActiveSink], profiles: &[ProfileSink]) -> Self {
        match active.iter().find(|s| s.name == name) {
            Some(s) if s.is_default => Self::Current,
            Some(_) => Self::Present,
            None if profiles.iter().any(|p| p.predicted_name == name) => Self::ViaProfile,
            None => Self::Missing,
        }
    }

    /// The word for it in human-readable output (`current`, `present`, `via profile`,
    /// `missing`)
    #[must_use]
    pub const fn word(self) -> &'static str {
        match self {
            Self::Current => "current",
            Self::Present => "present",
            Self::ViaProfile => "via profile",
            Self::Missing => "missing",
        }
    }

    /// The `status` value of `list-sinks --json` (a contract: `active`,
    /// `requires_profile_switch`, `not_found`)
    #[must_use]
    pub const fn json_status(self) -> &'static str {
        match self {
            Self::Current | Self::Present => "active",
            Self::ViaProfile => "requires_profile_switch",
            Self::Missing => "not_found",
        }
    }

    /// Whether the node can be used right now without a profile switch
    #[must_use]
    pub const fn is_present(self) -> bool {
        matches!(self, Self::Current | Self::Present)
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    fn active(name: &str, is_default: bool) -> ActiveSink {
        ActiveSink {
            name: name.to_string(),
            description: String::new(),
            is_default,
        }
    }

    #[test]
    fn test_node_state_of_and_json() {
        let active = [active("a", true), active("b", false)];
        let profiles = [ProfileSink {
            predicted_name: "c".to_string(),
            description: String::new(),
            device_id: 1,
            device_name: "dev".to_string(),
            profile_index: 1,
            profile_name: "p".to_string(),
        }];
        let state = |name| NodeState::of(name, &active, &profiles);
        assert_eq!(state("a"), NodeState::Current);
        assert_eq!(state("b"), NodeState::Present);
        assert_eq!(state("c"), NodeState::ViaProfile);
        assert_eq!(state("d"), NodeState::Missing);
        // The JSON values are a contract and never change
        let json: Vec<&str> = ["a", "b", "c", "d"]
            .into_iter()
            .map(|n| state(n).json_status())
            .collect();
        assert_eq!(json, [
            "active",
            "active",
            "requires_profile_switch",
            "not_found"
        ]);
        assert!(state("a").is_present() && state("b").is_present());
        assert!(!state("c").is_present() && !state("d").is_present());
    }
}
