//! Desktop notifications
//!
//! Handles sending notifications via notify-rust and icon detection
//! using `FreeDesktop` standard icon names.

use anyhow::{Context, Result};
use notify_rust::Notification;

use crate::config::SinkConfig;

/// Send a desktop notification
///
/// # Errors
/// Returns an error if the notification cannot be sent (e.g., no notification daemon running).
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

/// Determine icon for a sink (custom or auto-detected using `FreeDesktop` standard names)
#[must_use]
pub fn get_sink_icon(sink: &SinkConfig) -> String {
    // Custom icon takes priority
    if let Some(ref icon) = sink.icon {
        return icon.clone();
    }

    // Auto-detect from sink description and name
    let desc_lower = sink.desc.to_lowercase();
    let name_lower = sink.name.to_lowercase();

    if desc_lower.contains("hdmi")
        || desc_lower.contains("tv")
        || desc_lower.contains("display")
        || name_lower.contains("hdmi")
    {
        "video-display".to_string()
    } else if desc_lower.contains("headphone")
        || desc_lower.contains("headset")
        || desc_lower.contains("bluetooth")
        || name_lower.contains("bluez")
    {
        "audio-headphones".to_string()
    } else {
        // Default for speakers, optical, digital, etc.
        "audio-speakers".to_string()
    }
}

/// Convert `app_id` to icon name (handles common `app_id` formats)
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
