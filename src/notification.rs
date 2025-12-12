//! Desktop notifications
//!
//! Handles sending notifications via notify-rust and icon detection
//! using FreeDesktop standard icon names.

use anyhow::{Context, Result};
use notify_rust::Notification;

use crate::config::SinkConfig;

/// Send a desktop notification
pub fn send_notification(summary: &str, body: &str, icon: Option<&str>) -> Result<()> {
    // Use provided icon, or fall back to generic audio icon
    let icon = icon.unwrap_or("audio-card");

    Notification::new()
        .summary(summary)
        .body(body)
        .appname("PWSW")
        .icon(icon)
        .timeout(3000)
        .show()
        .context("Failed to show notification")?;

    Ok(())
}

/// Determine icon for a sink (custom or auto-detected using FreeDesktop standard names)
#[must_use]
pub fn get_sink_icon(sink: &SinkConfig) -> String {
    // Custom icon takes priority
    if let Some(ref icon) = sink.icon {
        return icon.clone();
    }
    get_sink_icon_auto(sink)
}

/// Get auto-detected sink icon using FreeDesktop standard names
/// Used for notifications when status_bar_icons is enabled
#[must_use]
pub fn get_sink_icon_auto(sink: &SinkConfig) -> String {
    let desc_lower = sink.desc.to_lowercase();
    let name_lower = sink.name.to_lowercase();

    if desc_lower.contains("hdmi") || desc_lower.contains("tv") || desc_lower.contains("display")
        || name_lower.contains("hdmi") {
        "video-display".to_string()
    } else if desc_lower.contains("headphone") || desc_lower.contains("headset")
        || desc_lower.contains("bluetooth") || name_lower.contains("bluez") {
        "audio-headphones".to_string()
    } else {
        // Default for speakers, optical, digital, etc.
        "audio-speakers".to_string()
    }
}

/// Get sink icon for notifications (respects status_bar_icons setting)
#[must_use]
pub fn get_notification_sink_icon(sink: &SinkConfig, status_bar_icons: bool) -> String {
    if status_bar_icons {
        // Custom icons only for status bar; use auto-detected for notifications
        get_sink_icon_auto(sink)
    } else {
        // Use custom icon everywhere
        get_sink_icon(sink)
    }
}

/// Convert app_id to icon name (handles common app_id formats)
#[must_use]
pub fn get_app_icon(app_id: &str) -> String {
    // Handle common app_id patterns that don't directly match icon names
    match app_id {
        "org.mozilla.firefox" => "firefox".to_string(),
        "org.mozilla.Thunderbird" => "thunderbird".to_string(),
        "org.gnome.Nautilus" => "nautilus".to_string(),
        "org.telegram.desktop" => "telegram".to_string(),
        // Most app_ids can be used directly as icon names
        _ => app_id.to_string(),
    }
}
