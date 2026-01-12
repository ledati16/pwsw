//! Screen rendering modules

pub mod dashboard;
pub mod help;
pub mod rules;
pub mod settings;
pub mod sinks;

pub(crate) use dashboard::{
    DashboardRenderContext, DashboardScreen, DashboardView, render_dashboard,
};
pub(crate) use help::render_help;
pub(crate) use rules::{RulesRenderContext, RulesScreen, render_rules};
pub(crate) use settings::{SettingsScreen, render_settings};
pub(crate) use sinks::{SinksRenderContext, SinksScreen, render_sinks};
