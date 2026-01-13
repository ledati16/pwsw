//! Screen rendering modules

pub mod dashboard;
pub mod help;
pub mod rules;
pub mod settings;
pub mod sinks;

pub use dashboard::{DashboardRenderContext, DashboardScreen, DashboardView, render_dashboard};
pub use help::render_help;
pub use rules::{RulesRenderContext, RulesScreen, render_rules};
pub use settings::{SettingsScreen, render_settings};
pub use sinks::{SinksRenderContext, SinksScreen, render_sinks};
