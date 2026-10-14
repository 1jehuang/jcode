use super::*;

fn window(id: u64, app_id: &str, ts: Option<(u64, u32)>) -> NiriWindow {
    NiriWindow {
        id,
        app_id: Some(app_id.to_string()),
        focus_timestamp: ts.map(|(secs, nanos)| NiriTimestamp { secs, nanos }),
    }
}

#[test]
fn normalize_desktop_entry_strips_suffix_and_adds_stem() {
    let stems = normalize_desktop_entry_to_stems("firefox.desktop");
    assert_eq!(stems, vec!["firefox".to_string()]);

    let stems = normalize_desktop_entry_to_stems("org.mozilla.firefox.desktop");
    assert_eq!(
        stems,
        vec!["org.mozilla.firefox".to_string(), "firefox".to_string()]
    );
}

#[test]
fn app_id_matches_is_case_insensitive_and_guards_short_ids() {
    let stems = vec!["firefox".to_string()];
    assert!(app_id_matches(Some("firefox"), &stems));
    assert!(app_id_matches(Some("Firefox"), &stems));
    assert!(!app_id_matches(Some("kitty"), &stems));
    assert!(!app_id_matches(None, &stems));
    // Very short ids should never match to avoid accidental substring hits.
    assert!(!app_id_matches(Some("fi"), &["fi".to_string()]));
}

#[test]
fn select_window_prefers_newly_created_browser_window() {
    let windows = vec![
        window(10, "kitty", Some((100, 0))),
        window(11, "firefox", Some((50, 0))),
        window(20, "firefox", Some((40, 0))),
    ];
    let stems = vec!["firefox".to_string()];
    let pre_ids: std::collections::HashSet<u64> = [11].into_iter().collect();
    // Window 20 is the new firefox window (not in pre_ids), so it wins even
    // though window 11 was focused more recently.
    assert_eq!(select_window_to_focus(&windows, &stems, &pre_ids), Some(20));
}

#[test]
fn select_window_falls_back_to_most_recently_focused() {
    let windows = vec![
        window(11, "firefox", Some((50, 0))),
        window(20, "firefox", Some((90, 0))),
    ];
    let stems = vec!["firefox".to_string()];
    // Both windows already existed: raise the most recently focused one (20),
    // which is where browsers add a new tab.
    let pre_ids: std::collections::HashSet<u64> = [11, 20].into_iter().collect();
    assert_eq!(select_window_to_focus(&windows, &stems, &pre_ids), Some(20));
}

#[test]
fn select_window_returns_none_without_browser_windows() {
    let windows = vec![window(10, "kitty", Some((100, 0)))];
    let stems = vec!["firefox".to_string()];
    let pre_ids = std::collections::HashSet::new();
    assert_eq!(select_window_to_focus(&windows, &stems, &pre_ids), None);
}

#[test]
fn target_parse_distinguishes_urls_and_paths() {
    assert!(matches!(
        Target::parse("https://example.com").unwrap(),
        Target::Url(ref u) if u == "https://example.com"
    ));
    let dir = std::env::temp_dir();
    assert!(matches!(
        Target::parse(dir.to_str().unwrap()).unwrap(),
        Target::Local {
            kind: LocalTargetKind::Directory,
            ..
        }
    ));
    assert!(Target::parse("/definitely/not/here/xyz").is_err());
}
