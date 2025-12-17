//! Integration tests for config loading, validation, and persistence
//!
//! These tests verify the full lifecycle of config operations through TOML
//! serialization/deserialization, rather than constructing Config structs directly.

use std::fs;
use tempfile::TempDir;

/// Helper to create a temporary config directory
fn setup_temp_config() -> (TempDir, std::path::PathBuf) {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let config_dir = temp_dir.path().join("pwsw");
    fs::create_dir_all(&config_dir).expect("Failed to create config dir");
    let config_path = config_dir.join("config.toml");
    (temp_dir, config_path)
}

#[test]
fn test_config_save_and_load_toml() {
    let (_temp, config_path) = setup_temp_config();

    // Write a valid TOML config file
    let toml_content = r#"
[settings]
default_on_startup = true
set_smart_toggle = true
notify_manual = true
notify_rules = true
match_by_index = false
log_level = "info"

[[sinks]]
name = "alsa_output.pci-0000_00_1f.3.analog-stereo"
desc = "Built-in Audio"
default = true

[[sinks]]
name = "alsa_output.usb-0000_01_00.0.analog-stereo"
desc = "USB Headphones"
default = false
icon = "audio-headphones"

[[rules]]
app_id = "firefox"
sink = "Built-in Audio"
desc = "Browser audio"

[[rules]]
app_id = "mpv"
title = ".*Music.*"
sink = "USB Headphones"
desc = "Music player"
notify = false
"#;

    fs::write(&config_path, toml_content).expect("Failed to write TOML");

    // Load the config
    let loaded = pwsw::config::Config::load_from_path(&config_path).expect("Failed to load config");

    // Verify settings
    assert!(loaded.settings.default_on_startup);
    assert!(loaded.settings.set_smart_toggle);
    assert!(loaded.settings.notify_manual);
    assert!(loaded.settings.notify_rules);
    assert!(!loaded.settings.match_by_index);
    assert_eq!(loaded.settings.log_level, "info");

    // Verify sinks
    assert_eq!(loaded.sinks.len(), 2);
    assert_eq!(loaded.sinks[0].desc, "Built-in Audio");
    assert!(loaded.sinks[0].default);
    assert_eq!(loaded.sinks[1].desc, "USB Headphones");
    assert!(!loaded.sinks[1].default);
    assert_eq!(loaded.sinks[1].icon, Some("audio-headphones".to_string()));

    // Verify rules
    assert_eq!(loaded.rules.len(), 2);
    assert_eq!(loaded.rules[0].app_id_pattern, "firefox");
    assert_eq!(loaded.rules[0].title_pattern, None);
    assert_eq!(loaded.rules[0].sink_ref, "Built-in Audio");
    assert_eq!(loaded.rules[1].app_id_pattern, "mpv");
    assert_eq!(
        loaded.rules[1].title_pattern,
        Some(".*Music.*".to_string())
    );
    assert_eq!(loaded.rules[1].notify, Some(false));
}

#[test]
fn test_config_validation_rejects_no_default_sink() {
    let (_temp, config_path) = setup_temp_config();

    // Config with no default sink (invalid)
    let invalid_toml = r#"
[settings]
default_on_startup = true
set_smart_toggle = true
notify_manual = true
notify_rules = true
match_by_index = false
log_level = "info"

[[sinks]]
name = "sink1"
desc = "Sink 1"
default = false

[[sinks]]
name = "sink2"
desc = "Sink 2"
default = false
"#;

    fs::write(&config_path, invalid_toml).expect("Failed to write TOML");

    // Load should fail validation
    let result = pwsw::config::Config::load_from_path(&config_path);
    assert!(
        result.is_err(),
        "Loading config with no default sink should fail"
    );
    let err_msg = format!("{:?}", result.unwrap_err());
    assert!(
        err_msg.contains("default") || err_msg.contains("sink"),
        "Error should mention missing default sink"
    );
}

#[test]
fn test_config_file_permissions() {
    let (_temp, config_path) = setup_temp_config();

    let toml_content = r#"
[settings]
default_on_startup = true
set_smart_toggle = true
notify_manual = true
notify_rules = true
match_by_index = false
log_level = "info"

[[sinks]]
name = "test_sink"
desc = "Test Sink"
default = true
"#;

    fs::write(&config_path, toml_content).expect("Failed to write TOML");

    // Load and save to trigger atomic write with proper permissions
    let config = pwsw::config::Config::load_from_path(&config_path).expect("Failed to load");
    config.save_to(&config_path).expect("Failed to save");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let metadata = fs::metadata(&config_path).expect("Failed to read metadata");
        let mode = metadata.permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "Config file should have 0o600 permissions after save");
    }

    // Verify config is still valid
    let loaded = pwsw::config::Config::load_from_path(&config_path).expect("Failed to load after save");
    assert_eq!(loaded.sinks.len(), 1);
    assert_eq!(loaded.sinks[0].desc, "Test Sink");
}

#[test]
fn test_config_duplicate_description_detection() {
    let (_temp, config_path) = setup_temp_config();

    // Config with duplicate sink descriptions
    let dup_desc_toml = r#"
[settings]
default_on_startup = true
set_smart_toggle = true
notify_manual = true
notify_rules = true
match_by_index = false
log_level = "info"

[[sinks]]
name = "sink1"
desc = "Same Description"
default = true

[[sinks]]
name = "sink2"
desc = "Same Description"
default = false
"#;

    fs::write(&config_path, dup_desc_toml).expect("Failed to write TOML");

    let result = pwsw::config::Config::load_from_path(&config_path);
    assert!(
        result.is_err(),
        "Duplicate sink descriptions should fail validation"
    );
}

#[test]
fn test_config_duplicate_name_detection() {
    let (_temp, config_path) = setup_temp_config();

    // Config with duplicate sink names
    let dup_name_toml = r#"
[settings]
default_on_startup = true
set_smart_toggle = true
notify_manual = true
notify_rules = true
match_by_index = false
log_level = "info"

[[sinks]]
name = "same_name"
desc = "Sink 1"
default = true

[[sinks]]
name = "same_name"
desc = "Sink 2"
default = false
"#;

    fs::write(&config_path, dup_name_toml).expect("Failed to write TOML");

    let result = pwsw::config::Config::load_from_path(&config_path);
    assert!(
        result.is_err(),
        "Duplicate sink names should fail validation"
    );
}
