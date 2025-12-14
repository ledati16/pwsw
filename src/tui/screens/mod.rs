//! Screen rendering modules

pub mod dashboard;
pub mod help;
pub mod rules;
pub mod settings;
pub mod sinks;

pub use dashboard::{render_dashboard, DashboardScreen};
pub use help::render_help;
pub use rules::{render_rules, RulesScreen};
pub use settings::{render_settings, SettingsScreen};
pub use sinks::{render_sinks, SinksScreen};
