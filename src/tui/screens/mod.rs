//! Screen rendering modules

pub mod dashboard;
pub mod help;
pub mod rules;
pub mod settings;
pub mod sinks;

pub(crate) use dashboard::{render_dashboard, DashboardScreen};
pub(crate) use help::render_help;
pub(crate) use rules::{render_rules, RulesRenderContext, RulesScreen};
pub(crate) use settings::{render_settings, SettingsScreen};
pub(crate) use sinks::{render_sinks, SinksScreen};
