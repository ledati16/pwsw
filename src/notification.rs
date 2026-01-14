//! Desktop notifications
//!
//! Handles sending notifications via notify-rust and icon detection
//! using `FreeDesktop` standard icon names.

use color_eyre::eyre::{Context, Result};
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
        || desc_lower.contains("earbuds")
        || desc_lower.contains("earphone")
        || desc_lower.contains("airpods")
        || name_lower.contains("bluez")
    {
        "audio-headphones".to_string()
    } else {
        // Default for speakers, optical, digital, etc.
        "audio-speakers".to_string()
    }
}

/// Convert `app_id` to icon name
///
/// Handles reverse-DNS style `app_ids` by extracting the meaningful component.
/// For example: `"org.mozilla.firefox"` → `"firefox"`, `"org.telegram.desktop"` → `"telegram"`.
#[must_use]
pub fn get_app_icon(app_id: &str) -> String {
    // Generic suffixes that aren't useful as icon names
    const GENERIC_SUFFIXES: &[&str] = &[
        "desktop",
        "client",
        "app",
        "application",
        "gui",
        "gtk",
        "gtk3",
        "gtk4",
        "qt",
        "qt5",
        "qt6",
    ];

    if app_id.is_empty() {
        return String::new();
    }

    // Split by dots and work backwards to find a meaningful name
    let parts: Vec<&str> = app_id.split('.').collect();

    for part in parts.iter().rev() {
        let lower = part.to_lowercase();
        // Skip empty parts and generic suffixes
        if !lower.is_empty() && !GENERIC_SUFFIXES.contains(&lower.as_str()) {
            return lower;
        }
    }

    // Fallback: return the whole app_id lowercased
    app_id.to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::fixtures::{make_sink, make_sink_with_icon};

    #[test]
    fn test_get_sink_icon_custom_override() {
        let sink = make_sink_with_icon("test.sink", "Test Speakers", false, "custom-icon");
        assert_eq!(get_sink_icon(&sink), "custom-icon");
    }

    #[test]
    fn test_get_sink_icon_hdmi_detection() {
        let sink = make_sink("test.hdmi", "HDMI Output", false);
        assert_eq!(get_sink_icon(&sink), "video-display");

        let sink2 = make_sink("test.sink", "Test TV", false);
        assert_eq!(get_sink_icon(&sink2), "video-display");

        let sink3 = make_sink("alsa.hdmi.stereo", "Test", false);
        assert_eq!(get_sink_icon(&sink3), "video-display");
    }

    #[test]
    fn test_get_sink_icon_headphone_detection() {
        let sink = make_sink("test.headphones", "Headphones", false);
        assert_eq!(get_sink_icon(&sink), "audio-headphones");

        let sink2 = make_sink("test.bt", "Bluetooth Headset", false);
        assert_eq!(get_sink_icon(&sink2), "audio-headphones");

        let sink3 = make_sink("bluez.sink", "Test", false);
        assert_eq!(get_sink_icon(&sink3), "audio-headphones");

        // New patterns
        let sink4 = make_sink("test.sink", "Galaxy Buds Earbuds", false);
        assert_eq!(get_sink_icon(&sink4), "audio-headphones");

        let sink5 = make_sink("test.sink", "Earphone Jack", false);
        assert_eq!(get_sink_icon(&sink5), "audio-headphones");

        let sink6 = make_sink("test.sink", "AirPods Pro", false);
        assert_eq!(get_sink_icon(&sink6), "audio-headphones");
    }

    #[test]
    fn test_get_sink_icon_default_speakers() {
        let sink = make_sink("test.analog", "Analog Stereo", false);
        assert_eq!(get_sink_icon(&sink), "audio-speakers");

        let sink2 = make_sink("test.digital", "Digital Output", false);
        assert_eq!(get_sink_icon(&sink2), "audio-speakers");
    }

    #[test]
    fn test_get_app_icon_reverse_dns() {
        // Extracts last meaningful segment from reverse-DNS app_ids
        assert_eq!(get_app_icon("org.mozilla.firefox"), "firefox");
        assert_eq!(get_app_icon("org.gnome.Nautilus"), "nautilus");
        assert_eq!(get_app_icon("org.mozilla.Thunderbird"), "thunderbird");
        assert_eq!(get_app_icon("org.videolan.VLC"), "vlc");
        assert_eq!(get_app_icon("com.discordapp.Discord"), "discord");
        assert_eq!(get_app_icon("io.mpv.Mpv"), "mpv");
    }

    #[test]
    fn test_get_app_icon_skips_generic_suffixes() {
        // Skips generic suffixes like "desktop", "client", "app"
        assert_eq!(get_app_icon("org.telegram.desktop"), "telegram");
        assert_eq!(get_app_icon("com.spotify.Client"), "spotify");
        assert_eq!(get_app_icon("org.example.App"), "example");
        assert_eq!(get_app_icon("com.example.Application"), "example");
        assert_eq!(get_app_icon("org.kde.something.qt5"), "something");
    }

    #[test]
    fn test_get_app_icon_simple_names() {
        // Simple app_ids without dots pass through (lowercased)
        assert_eq!(get_app_icon("mpv"), "mpv");
        assert_eq!(get_app_icon("steam"), "steam");
        assert_eq!(get_app_icon("Firefox"), "firefox");
        assert_eq!(get_app_icon("Steam"), "steam");
    }

    #[test]
    fn test_get_app_icon_edge_cases() {
        // Empty string
        assert_eq!(get_app_icon(""), "");

        // All generic suffixes - falls back to full string lowercased
        assert_eq!(get_app_icon("desktop"), "desktop");

        // Two-part with generic suffix
        assert_eq!(get_app_icon("myapp.desktop"), "myapp");
    }
}
