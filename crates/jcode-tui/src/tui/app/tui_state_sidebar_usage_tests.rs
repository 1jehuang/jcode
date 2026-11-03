use super::{
    WidgetProviderKind, WidgetRouteInfo, remote_oauth_widget_usage,
    remote_oauth_widget_usage_for_route,
};
use crate::tui::info_widget::{AuthMethod, UsageProvider};
use crate::usage::AccountUsageSnapshot;

#[test]
fn startup_usage_slot_waits_for_account_binding_even_with_cached_quotas() {
    for runtime_pending in [false, true] {
        for usage_received in [false, true] {
            assert!(!super::startup_usage_slot_pending(
                false,
                runtime_pending,
                usage_received
            ));
            assert_eq!(
                super::startup_usage_slot_pending(true, runtime_pending, usage_received),
                runtime_pending || !usage_received,
            );
        }
    }
}

fn snapshot() -> AccountUsageSnapshot {
    AccountUsageSnapshot {
        label: "solosystems".into(),
        email: None,
        exhausted: false,
        primary_label: Some("5-hour".into()),
        five_hour_ratio: Some(1.0),
        secondary_label: Some("7-day".into()),
        seven_day_ratio: Some(0.0),
        resets_at: Some("2099-01-01T00:00:00Z".into()),
        error: None,
    }
}

fn providers() -> [(UsageProvider, &'static str); 2] {
    [
        (UsageProvider::Anthropic, "claude-oauth"),
        (UsageProvider::OpenAI, "openai-oauth"),
    ]
}

#[test]
fn remote_sidebar_usage_uses_daemon_label_without_saved_route() {
    for (provider, method) in providers() {
        let stale_route = format!("{method}@stale-personal-account");
        for route in [None, Some(stale_route.as_str())] {
            let info = remote_oauth_widget_usage_for_route(
                provider,
                route,
                Some("solosystems"),
                false,
                None,
                |label| {
                    assert_eq!(label, "solosystems");
                    Some(snapshot())
                },
            );
            assert_eq!(info.provider, provider);
            assert!(info.available);
            assert_eq!(info.five_hour, 1.0);
        }
    }
}

#[test]
fn remote_sidebar_usage_uses_bound_label_not_global_account() {
    for (provider, method) in providers() {
        let route = format!("{method}@solosystems");
        let info = remote_oauth_widget_usage_for_route(
            provider,
            Some(&route),
            None,
            false,
            Some(42.0),
            |label| {
                assert_eq!(label, "solosystems");
                Some(snapshot())
            },
        );
        assert_eq!(info.provider, provider);
        assert!(info.available);
        assert_eq!(info.primary_limit_label.as_deref(), Some("5-hour"));
        assert_eq!(info.five_hour, 1.0);
        assert_eq!(info.seven_day, 0.0);
        assert_eq!(info.output_tps, Some(42.0));
        assert!(info.five_hour_resets_at.is_some());
        assert!(info.seven_day_resets_at.is_none());
    }
}

#[test]
fn remote_sidebar_usage_without_bound_account_never_looks_up_global_usage() {
    for (provider, method) in providers() {
        for route in [
            None,
            Some(method),
            Some("anthropic-api-key"),
            Some("openai-api-key"),
        ] {
            for label in [None, Some(""), Some("bad\nlabel")] {
                let info = remote_oauth_widget_usage_for_route(
                    provider,
                    route,
                    label,
                    false,
                    Some(42.0),
                    |_| panic!("unbound route must not look up another account"),
                );
                assert_eq!(info.provider, provider);
                assert!(!info.available);
                assert!(!info.limit_reached);
                assert_eq!(info.output_tps, Some(42.0));
                assert!(info.primary_limit_label.is_none());
                assert!(info.secondary_limit_label.is_none());
                assert!(info.five_hour_resets_at.is_none());
                assert!(info.seven_day_resets_at.is_none());
            }
        }
    }
}

#[test]
fn remote_sidebar_usage_missing_daemon_account_does_not_retry_saved_route() {
    for (provider, method) in providers() {
        let route = format!("{method}@stale-personal-account");
        let info = remote_oauth_widget_usage_for_route(
            provider,
            Some(&route),
            Some("missing-daemon-account"),
            false,
            None,
            |label| {
                assert_eq!(label, "missing-daemon-account");
                None
            },
        );
        assert!(!info.available);
        assert!(!info.limit_reached);
        assert!(info.primary_limit_label.is_none());
        assert!(info.secondary_limit_label.is_none());
    }
}

#[test]
fn remote_sidebar_usage_preserves_weekly_only_and_unavailable_snapshots() {
    for (provider, _) in providers() {
        let mut usage = snapshot();
        usage.five_hour_ratio = None;
        usage.primary_label = None;
        let info = remote_oauth_widget_usage(provider, Some(&usage), false, None);
        assert!(info.available);
        assert!(info.primary_limit_label.is_none());
        assert_eq!(info.secondary_limit_label.as_deref(), Some("7-day"));
        assert_eq!(info.seven_day, 0.0);
        assert_eq!(info.seven_day_resets_at, usage.resets_at);
        for error in ["HTTP 429", "HTTP 401", "connection failed"] {
            usage.error = Some(error.into());
            let info = remote_oauth_widget_usage(provider, Some(&usage), false, None);
            assert!(!info.available);
            assert!(
                !info.limit_reached,
                "a fetch failure is not a quota rejection"
            );
        }
        let missing = remote_oauth_widget_usage(provider, None, true, None);
        assert!(!missing.available);
        assert!(missing.limit_reached);
    }
}

#[test]
fn remote_sidebar_usage_does_not_invent_missing_windows_or_resets() {
    for (provider, _) in providers() {
        let mut usage = snapshot();
        usage.five_hour_ratio = None;
        usage.seven_day_ratio = None;
        let info = remote_oauth_widget_usage(provider, Some(&usage), false, None);
        assert!(!info.available);
        assert!(info.primary_limit_label.is_none());
        assert!(info.secondary_limit_label.is_none());
        assert!(info.five_hour_resets_at.is_none());
        assert!(info.seven_day_resets_at.is_none());
    }
}

#[test]
fn remote_anthropic_widget_missing_usage_never_falls_back_to_local_getter() {
    let mut app = super::super::tests::create_test_app();
    let route = WidgetRouteInfo {
        provider: WidgetProviderKind::Anthropic,
        is_remote: true,
    };
    for label in [None, Some("remote-anthropic-missing-snapshot-test")] {
        app.remote_provider_oauth_account_label = label.map(str::to_string);
        app.session.route_api_method = Some("claude-oauth".into());
        let info = app
            .widget_usage_info(route, AuthMethod::AnthropicOAuth)
            .unwrap();
        assert_eq!(info.provider, UsageProvider::Anthropic);
        assert!(!info.available);
        assert!(!info.limit_reached);
        assert!(info.primary_limit_label.is_none());
        assert!(info.secondary_limit_label.is_none());
        assert!(info.five_hour_resets_at.is_none());
        assert!(info.seven_day_resets_at.is_none());
    }
    assert!(app.widget_usage_info(route, AuthMethod::Unknown).is_none());
    let api = app
        .widget_usage_info(route, AuthMethod::AnthropicApiKey)
        .unwrap();
    assert_eq!(api.provider, UsageProvider::CostBased);
}
