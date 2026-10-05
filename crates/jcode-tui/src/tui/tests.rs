//! Tests for `#[path]`-attributed module `tests` of `mod.rs`.
//!
//! Extracted from the parent file: an inline `#[cfg(test)] mod` counts toward
//! the code-size budget, which only exempts `*_tests.rs` files.

use super::cache_expected_warm;

use super::{
    CacheTtlInfo, KvCacheProblemKind, connection_type_icon, cross_project_attach_notice,
    detect_kv_cache_problem, keyboard_enhancement_flags, resolve_subscribe_metadata,
    scheduled_notification_text,
};
use crate::ambient::AmbientStatus;
use crate::tui::info_widget::AmbientWidgetData;
use crossterm::event::KeyboardEnhancementFlags;

fn warm_cache_ttl() -> CacheTtlInfo {
    CacheTtlInfo {
        is_estimate: false,
        remaining_secs: 240,
        ttl_secs: 300,
        is_cold: false,
        cold_for_secs: 0,
        cached_tokens: Some(12_000),
    }
}

fn cold_cache_ttl() -> CacheTtlInfo {
    CacheTtlInfo {
        is_estimate: false,
        remaining_secs: 0,
        ttl_secs: 300,
        is_cold: true,
        cold_for_secs: 90,
        cached_tokens: Some(12_000),
    }
}

#[test]
fn cache_estimate_is_not_evidence_of_an_expected_warm_hit() {
    let mut timer = warm_cache_ttl();
    assert!(super::cache_expected_warm(Some(&timer)));
    timer.is_estimate = true;
    assert!(!super::cache_expected_warm(Some(&timer)));
}

#[test]
fn cross_project_attach_notice_names_both_projects() {
    let notice = cross_project_attach_notice(Some("/client/project"), Some("/server/project"))
        .expect("a session in another project must be reported");

    assert!(
        notice.contains("/client/project"),
        "the notice must name the project the user launched in: {notice}"
    );
    assert!(
        notice.contains("/server/project"),
        "the notice must name the project the session belongs to: {notice}"
    );
}

#[test]
fn cross_project_attach_notice_is_quiet_when_the_projects_agree() {
    // Same project, including a path that differs only by a trailing separator.
    assert_eq!(
        cross_project_attach_notice(Some("/repo"), Some("/repo")),
        None
    );
    assert_eq!(
        cross_project_attach_notice(Some("/repo/"), Some("/repo")),
        None
    );
    assert_eq!(
        cross_project_attach_notice(Some("/repo"), Some("/repo/")),
        None
    );
}

#[test]
fn cross_project_attach_notice_stays_quiet_when_either_side_is_unknown() {
    // A client that cannot name its own project, or a session that has not been
    // attributed to one, has no disagreement to report. Claiming a cross-project
    // attach here would be guessing, and a notice that fires for a normal session
    // is worse than no notice at all.
    assert_eq!(
        cross_project_attach_notice(None, Some("/server/project")),
        None
    );
    assert_eq!(
        cross_project_attach_notice(Some("/client/project"), None),
        None
    );
    assert_eq!(cross_project_attach_notice(None, None), None);
}

#[test]
fn cross_project_attach_notice_tolerates_unresolvable_paths() {
    // A directory that no longer exists cannot be canonicalized on either side, so
    // the comparison falls back to the literal strings. Two different literal
    // strings are still reported; equal ones are not.
    let gone = std::env::temp_dir().join("jcode-cross-project-notice-missing");
    let gone = gone.to_string_lossy().to_string();
    assert_eq!(cross_project_attach_notice(Some(&gone), Some(&gone)), None);
    assert!(cross_project_attach_notice(Some(&gone), Some("/elsewhere")).is_some());
}

#[test]
fn subscribe_metadata_prefers_remote_working_dir_override() {
    let local_dir = "/client/project";
    let (working_dir, selfdev) =
        resolve_subscribe_metadata(Some(local_dir), Some("/server/project"), false);

    assert_eq!(working_dir.as_deref(), Some("/server/project"));
    assert_eq!(selfdev, None);
}

#[test]
fn subscribe_metadata_uses_client_cwd_without_override() {
    let local_dir = "/client/project";
    let (working_dir, _selfdev) = resolve_subscribe_metadata(Some(local_dir), None, false);

    assert_eq!(working_dir.as_deref(), Some("/client/project"));
}

#[test]
fn format_compact_age_is_glanceable() {
    use super::format_compact_age;
    assert_eq!(format_compact_age(0), "0s");
    assert_eq!(format_compact_age(45), "45s");
    assert_eq!(format_compact_age(60), "1m");
    assert_eq!(format_compact_age(3_660), "1h 1m");
    assert_eq!(format_compact_age(7_200), "2h");
    assert_eq!(format_compact_age(90_000), "1d 1h");
    assert_eq!(format_compact_age(172_800), "2d");
}

#[test]
fn anthropic_cache_creation_on_turn_two_is_warmup_not_problem() {
    let ttl = warm_cache_ttl();
    assert_eq!(
        detect_kv_cache_problem(
            "anthropic",
            None,
            2,
            12_000,
            Some(0),
            Some(12_000),
            Some(&ttl)
        ),
        None
    );
}

#[test]
fn anthropic_cache_creation_without_read_on_warm_later_turn_is_problem() {
    let ttl = warm_cache_ttl();
    let problem = detect_kv_cache_problem(
        "anthropic",
        None,
        3,
        12_000,
        Some(0),
        Some(12_000),
        Some(&ttl),
    )
    .expect("expected explicit cache creation without read to warn");
    assert_eq!(problem.kind, KvCacheProblemKind::UnexpectedCacheCreation);
    assert_eq!(problem.affected_tokens, Some(12_000));
}

#[test]
fn cache_read_suppresses_cache_creation_warning() {
    let ttl = warm_cache_ttl();
    assert_eq!(
        detect_kv_cache_problem(
            "anthropic",
            None,
            3,
            12_000,
            Some(8_000),
            Some(4_000),
            Some(&ttl)
        ),
        None
    );
}

#[test]
fn cold_cache_suppresses_cache_warning() {
    let ttl = cold_cache_ttl();
    assert_eq!(
        detect_kv_cache_problem(
            "anthropic",
            None,
            3,
            12_000,
            Some(0),
            Some(12_000),
            Some(&ttl)
        ),
        None
    );
}

#[test]
fn openai_explicit_zero_cache_read_on_warm_cacheable_turn_is_problem() {
    let ttl = warm_cache_ttl();
    let problem = detect_kv_cache_problem("openai", None, 3, 8_000, Some(0), None, Some(&ttl))
        .expect("expected explicit zero cached tokens to warn");
    assert_eq!(problem.kind, KvCacheProblemKind::ExpectedCacheReadMissing);
    assert_eq!(problem.affected_tokens, Some(8_000));
}

#[test]
fn missing_cache_read_metric_is_not_a_warning() {
    let ttl = warm_cache_ttl();
    assert_eq!(
        detect_kv_cache_problem("openai", None, 3, 8_000, None, None, Some(&ttl)),
        None
    );
}

#[test]
fn read_only_warning_requires_cacheable_input_size() {
    let ttl = warm_cache_ttl();
    assert_eq!(
        detect_kv_cache_problem("openai", None, 3, 800, Some(0), None, Some(&ttl)),
        None
    );
}

#[test]
fn openrouter_zero_cache_read_requires_known_cache_capable_upstream() {
    let ttl = warm_cache_ttl();
    assert_eq!(
        detect_kv_cache_problem("openrouter", None, 3, 8_000, Some(0), None, Some(&ttl)),
        None
    );

    let problem = detect_kv_cache_problem(
        "openrouter",
        Some("OpenAI"),
        3,
        8_000,
        Some(0),
        None,
        Some(&ttl),
    )
    .expect("known OpenAI upstream should make explicit zero read actionable");
    assert_eq!(problem.kind, KvCacheProblemKind::ExpectedCacheReadMissing);
}

#[test]
fn unsupported_provider_zero_cache_read_does_not_warn_even_if_metric_present() {
    let ttl = warm_cache_ttl();
    assert_eq!(
        detect_kv_cache_problem("copilot", None, 3, 8_000, Some(0), None, Some(&ttl)),
        None
    );
}

#[test]
fn gemini_zero_cache_read_uses_conservative_minimum() {
    let ttl = warm_cache_ttl();
    assert_eq!(
        detect_kv_cache_problem("gemini", None, 3, 3_000, Some(0), None, Some(&ttl)),
        None
    );

    let problem = detect_kv_cache_problem("gemini", None, 3, 5_000, Some(0), None, Some(&ttl))
        .expect("large Gemini prompt with explicit zero cached content should warn");
    assert_eq!(problem.kind, KvCacheProblemKind::ExpectedCacheReadMissing);
}

#[test]
fn connection_type_icon_uses_protocol_specific_icons() {
    assert_eq!(connection_type_icon(Some("websocket")), Some("🔌"));
    assert_eq!(connection_type_icon(Some("wss")), Some("🔌"));
    assert_eq!(connection_type_icon(Some("https")), Some("🌐"));
    assert_eq!(connection_type_icon(Some("https/sse")), Some("🌐"));
    assert_eq!(connection_type_icon(Some("http")), Some("🌐"));
    assert_eq!(connection_type_icon(Some("unknown")), None);
    assert_eq!(connection_type_icon(None), None);
}

#[test]
fn connection_type_icons_avoid_vs16_sequences() {
    // macOS window/tab title fonts ignore the VS16 emoji-presentation
    // selector, so title icons must be single emoji-default codepoints.
    for connection in ["websocket", "wss", "https", "https/sse", "http"] {
        let icon = connection_type_icon(Some(connection)).unwrap();
        assert_eq!(
            icon.chars().count(),
            1,
            "connection icon for '{connection}' must be a single codepoint, got {icon:?}"
        );
        assert!(
            !icon.contains('\u{FE0F}'),
            "connection icon for '{connection}' must not need VS16, got {icon:?}"
        );
    }
}

#[test]
fn scheduled_notification_text_uses_session_reminder_count_only() {
    let info = AmbientWidgetData {
        show_widget: false,
        status: AmbientStatus::Disabled,
        queue_count: 88,
        next_queue_preview: Some("ambient backlog".to_string()),
        reminder_count: 2,
        next_reminder_preview: Some("follow up".to_string()),
        last_run_ago: None,
        last_summary: None,
        next_wake: Some("in 0s".to_string()),
        next_reminder_wake: Some("in 5m".to_string()),
        budget_percent: None,
    };

    assert_eq!(
        scheduled_notification_text(Some(&info)).as_deref(),
        Some("⏰ next scheduled task in 5m · 2 queued")
    );
}

#[test]
fn keyboard_enhancement_flags_avoid_report_all_keys_escape_mode() {
    let flags = keyboard_enhancement_flags();

    assert!(flags.contains(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES));
    assert!(flags.contains(KeyboardEnhancementFlags::REPORT_EVENT_TYPES));
    assert!(flags.contains(KeyboardEnhancementFlags::REPORT_ALTERNATE_KEYS));
    assert!(!flags.contains(KeyboardEnhancementFlags::REPORT_ALL_KEYS_AS_ESCAPE_CODES));
}
