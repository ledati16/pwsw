use crate::ipc::WindowInfo;
use crate::tui::windows_fingerprint;

#[test]
fn test_windows_fingerprint_deterministic_and_sensitive() {
    let a = WindowInfo {
        id: None,
        app_id: "firefox".into(),
        title: "Firefox".into(),
        matched_on: None,
        tracked: None,
    };
    let b = WindowInfo {
        id: None,
        app_id: "mpv".into(),
        title: "mpv video".into(),
        matched_on: None,
        tracked: None,
    };

    // Same order -> equal
    let v1 = vec![a.clone(), b.clone()];
    let v2 = vec![a.clone(), b.clone()];
    assert_eq!(windows_fingerprint(&v1), windows_fingerprint(&v2));

    // Different order -> likely different fingerprint
    let v3 = vec![b, a];
    assert_ne!(windows_fingerprint(&v1), windows_fingerprint(&v3));

    // Content change -> different
    let mut v4 = v1.clone();
    v4[0].title = "Different Title".into();
    assert_ne!(windows_fingerprint(&v1), windows_fingerprint(&v4));
}
