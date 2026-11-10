use super::*;
use crate::auth::{ActiveCredential, ProviderAuth};

fn text(lines: Vec<Line<'static>>) -> String {
    lines
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn remote_rows_mask_email_and_show_quota() {
    let account = crate::usage::AccountUsageSnapshot {
        additional_windows: Vec::new(),
        label: "work".into(),
        email: Some("jeremy@example.com".into()),
        exhausted: false,
        primary_label: Some("5-hour".into()),
        five_hour_ratio: Some(0.15),
        secondary_label: None,
        seven_day_ratio: None,
        resets_at: None,
        error: None,
    };
    let out = text(remote_inventory_lines(
        &[],
        &[account],
        "openai",
        Some("work"),
    ));
    assert!(out.contains("openai"), "{out}");
    assert!(out.contains("j***y@example.com"), "{out}");
    assert!(!out.contains("jeremy@"), "{out}");
    assert!(out.contains("85%"), "{out}");
}

#[test]
fn account_headers_render_all_usage_windows_and_fit_unicode_widths() {
    let mut usage = crate::usage::AccountUsageSnapshot {
        label: "work".into(),
        email: Some("界界@example.com".into()),
        exhausted: false,
        primary_label: Some("session".into()),
        five_hour_ratio: Some(0.15),
        secondary_label: Some("7-day".into()),
        seven_day_ratio: Some(0.42),
        additional_windows: vec![
            crate::protocol::AccountUsageWindow {
                label: "Fable 7d".into(),
                usage_ratio: 0.73,
                resets_at: None,
            },
            crate::protocol::AccountUsageWindow {
                label: "Spark 界".into(),
                usage_ratio: 0.91,
                resets_at: None,
            },
            crate::protocol::AccountUsageWindow {
                label: "future model / month".into(),
                usage_ratio: 0.32,
                resets_at: None,
            },
        ],
        resets_at: None,
        error: None,
    };
    for display_used in [false, true] {
        let rendered = text(vec![remaining_bar(&usage, display_used).unwrap()]);
        for label in [
            "session",
            "7d",
            "Fable 7d",
            "Spark 界",
            "future model / month",
        ] {
            assert!(rendered.contains(label), "{rendered}");
        }
        for percent in if display_used {
            [15, 42, 73, 91, 32]
        } else {
            [85, 58, 27, 9, 68]
        } {
            assert!(rendered.contains(&format!("{percent}%")), "{rendered}");
        }
    }
    for (anthropic, openai, provider) in [
        (vec![], vec![usage.clone()], "openai"),
        (vec![usage.clone()], vec![], "anthropic"),
    ] {
        let rows = remote_inventory_lines(&anthropic, &openai, provider, Some("work"));
        assert_eq!(rows.len(), 6, "identity plus one logical row per quota");
        for width in [0, 1, 2, 8, 24, 40, 80, 120, 240] {
            let fitted = fit_header_lines(rows.clone(), width);
            assert_eq!(fitted.len(), rows.len());
            for (source, result) in rows.iter().zip(&fitted) {
                assert!(result.width() <= usize::from(width.saturating_sub(1)));
                if width >= 8 && source.width() > usize::from(width.saturating_sub(1)) {
                    assert!(text(vec![result.clone()]).ends_with('…'));
                }
            }
            if width >= 40 {
                let rendered = text(fitted);
                for label in [
                    "session",
                    "7d",
                    "Fable 7d",
                    "Spark 界",
                    "future model / month",
                ] {
                    assert!(rendered.contains(label), "width={width}: {rendered}");
                }
            }
        }
    }
    // One malformed window must not hide the valid windows that follow it.
    usage.five_hour_ratio = Some(f32::NAN);
    usage.seven_day_ratio = None;
    let rendered = text(vec![remaining_bar(&usage, false).unwrap()]);
    assert!(!rendered.contains("session"));
    assert!(rendered.contains("Fable 7d"));
    assert!(rendered.contains("future model / month"));
}

fn auth() -> AuthStatus {
    AuthStatus {
        anthropic: ProviderAuth {
            state: AuthState::Available,
            has_oauth: true,
            oauth_state: AuthState::Available,
            has_api_key: true,
        },
        openai: AuthState::Available,
        openai_has_oauth: true,
        openai_oauth_state: AuthState::Available,
        openai_has_api_key: true,
        openrouter: AuthState::Available,
        ..Default::default()
    }
}

fn login(provider: &'static str, label: &str, email: &str) -> OAuthLogin {
    OAuthLogin {
        provider,
        label: label.into(),
        email: Some(email.into()),
        active: true,
    }
}

#[test]
fn router_inventory_follows_pool_order_within_priority_without_promoting_active() {
    let inventory = Inventory {
        logins: vec![
            login("claude", "a", "alice@example.com"),
            login("openai", "a", "anna@example.com"),
            login("openai", "b", "bob@example.com"),
        ],
        pool: AccountPool {
            members: vec![
                "openai:b".into(),
                "claude:a".into(),
                "openai-api".into(),
                "openai:a".into(),
            ],
            excluded: vec![],
            priorities: [("openai-api".into(), 1)].into_iter().collect(),
        },
        ..Default::default()
    };
    let lines = inventory_lines(
        &auth(),
        ActiveCredentialOverrides {
            openai: Some(ActiveCredential::OAuth),
            anthropic: Some(ActiveCredential::OAuth),
        },
        &inventory,
        "openai",
        Some("a"),
    );
    let rendered = text(lines.clone());
    assert_eq!(lines.len(), 4, "{rendered}");
    assert!(text(vec![lines[0].clone()]).contains("b***b@example.com"));
    assert!(text(vec![lines[1].clone()]).contains("a***e@example.com"));
    assert!(text(vec![lines[2].clone()]).contains("API key"));
    assert!(text(vec![lines[3].clone()]).contains("a***a@example.com"));
    assert_eq!(lines[3].spans[1].content, "○");
    assert_eq!(lines[0].spans[1].content, "○");
    assert_eq!(rendered.matches('●').count(), 1);
    assert_eq!(lines[3].spans[0].content, "> ");
    assert_eq!(rendered.matches('>').count(), 1);
    for (index, line) in lines.iter().enumerate() {
        let state = if index == 2 {
            AuthState::Available
        } else {
            AuthState::NotConfigured
        };
        assert_eq!(line.spans[1].style.fg, Some(auth_dot_color(state)));
    }
    for word in [
        "active",
        "standby",
        "fallback",
        "manual",
        "alice@example.com",
        "bob@example.com",
        "anthropic-api",
    ] {
        assert!(!rendered.contains(word), "{rendered}");
    }
}

#[test]
fn router_inventory_hides_excluded_and_manual_credentials_even_if_active() {
    let inventory = Inventory {
        logins: vec![
            login("openai", "excluded", "secret@example.com"),
            login("claude", "a", "alice@example.com"),
        ],
        pool: AccountPool {
            members: vec!["openrouter".into()],
            excluded: vec!["openai:excluded".into()],
            ..Default::default()
        },
        ..Default::default()
    };
    let rendered = text(inventory_lines(
        &auth(),
        ActiveCredentialOverrides {
            openai: Some(ActiveCredential::ApiKey),
            anthropic: None,
        },
        &inventory,
        "openai",
        Some("excluded"),
    ));
    assert!(rendered.contains("openrouter"));
    assert!(rendered.contains("API key"));
    assert!(rendered.contains("a***e@example.com"));
    assert!(!rendered.contains("openai"));
    assert!(!rendered.contains("s***t@example.com"));
    assert_eq!(rendered.matches("API key").count(), 1);
}

#[test]
fn router_inventory_default_subscriptions_and_expiry_are_account_scoped() {
    let mut status = auth();
    status.openai_oauth_state = AuthState::Expired;
    let inventory = Inventory {
        logins: vec![
            login("openai", "old", "old@example.com"),
            login("openai", "bound", "new@example.com"),
        ],
        expired: vec![("openai", "old".into())],
        ..Default::default()
    };
    let lines = inventory_lines(
        &status,
        ActiveCredentialOverrides {
            openai: Some(ActiveCredential::OAuth),
            anthropic: None,
        },
        &inventory,
        "openai",
        Some("bound"),
    );
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0].spans[1].content, "◐");
    assert_eq!(
        lines[0].spans[1].style.fg,
        Some(auth_dot_color(AuthState::Expired))
    );
    assert_eq!(lines[0].spans[0].content, "  ");
    assert_eq!(lines[1].spans[0].content, "> ");
    assert_eq!(lines[1].spans[1].content, "○");
    assert_eq!(
        lines[1].spans[1].style.fg,
        Some(auth_dot_color(AuthState::NotConfigured))
    );
    assert!(!text(lines).contains("API key"));
}

#[test]
fn router_inventory_selection_is_independent_of_status() {
    for state in [
        AuthState::Available,
        AuthState::Expired,
        AuthState::NotConfigured,
    ] {
        for selected in [false, true] {
            let row = detail(state, "credential".into(), selected);
            assert_eq!(row.spans[0].content, if selected { "> " } else { "  " });
            assert_eq!(row.spans[1].content, auth_dot_char(state));
            assert_eq!(row.spans[1].style.fg, Some(auth_dot_color(state)));
            assert_eq!(row.width(), 14);
        }
    }
}

#[test]
fn router_priorities_only_dim_original_identity_colors_and_fit_narrow_headers() {
    let usage = crate::usage::AccountUsageSnapshot {
        additional_windows: Vec::new(),
        label: "one".into(),
        email: None,
        exhausted: false,
        primary_label: Some("5h".into()),
        five_hour_ratio: Some(0.48),
        secondary_label: None,
        seven_day_ratio: None,
        resets_at: None,
        error: None,
    };
    for selected in [false, true] {
        let mut row = detail(
            AuthState::Available,
            "openai 界界@example.com".into(),
            selected,
        );
        row.spans
            .extend(remaining_bar(&usage, false).unwrap().spans);
        let high = priority_row(row.clone(), 20, &[20, 30, 90]);
        let low = priority_row(row.clone(), 90, &[20, 30, 90]);
        assert_eq!(low.spans[2].content, row.spans[2].content);
        assert_eq!(high.spans[2].style.fg, row.spans[2].style.fg);
        assert_eq!(low.spans[..2], row.spans[..2]);
        assert_eq!(low.spans[3..], row.spans[3..]);
        assert_ne!(high.spans[2].style.fg, low.spans[2].style.fg);
        assert_eq!(
            high.spans[2].style.fg,
            priority_row(row.clone(), 0, &[0, 1, 2]).spans[2].style.fg
        );
        assert_eq!(
            high.spans[2].style.fg,
            priority_row(row.clone(), 90, &[90]).spans[2].style.fg
        );
        for width in [0, 1, 4, 12, 24, 40, 80] {
            let fitted = fit_header_lines(vec![low.clone()], width);
            assert!(fitted[0].width() <= usize::from(width.saturating_sub(1)));
        }
    }
}

#[test]
fn router_inventory_usability_requires_account_scoped_evidence() {
    let mut usage = crate::usage::AccountUsageSnapshot {
        additional_windows: Vec::new(),
        label: "one".into(),
        email: None,
        exhausted: false,
        primary_label: Some("5h".into()),
        five_hour_ratio: Some(0.48),
        secondary_label: None,
        seven_day_ratio: None,
        resets_at: None,
        error: None,
    };
    assert_eq!(
        oauth_credential_status(false, None),
        CredentialStatus::Unknown
    );
    assert_eq!(
        oauth_credential_status(true, None),
        CredentialStatus::Attention
    );
    assert_eq!(
        oauth_credential_status(false, Some(&usage)),
        CredentialStatus::Available
    );
    usage.exhausted = true;
    assert_eq!(
        oauth_credential_status(false, Some(&usage)),
        CredentialStatus::Blocked
    );
    usage.exhausted = false;
    usage.five_hour_ratio = Some(1.0);
    assert_eq!(
        oauth_credential_status(false, Some(&usage)),
        CredentialStatus::Blocked
    );
    usage.five_hour_ratio = Some(0.48);
    usage.seven_day_ratio = Some(1.0);
    assert_eq!(
        oauth_credential_status(false, Some(&usage)),
        CredentialStatus::Blocked
    );
    usage.seven_day_ratio = None;
    for error in [
        "Usage API error (401 Unauthorized): invalid token",
        "API error (401 Unauthorized)",
        "HTTP 401",
    ] {
        usage.error = Some(error.into());
        assert_eq!(
            oauth_credential_status(false, Some(&usage)),
            CredentialStatus::Blocked,
            "{error}"
        );
    }
    // Probe failures cannot make old quota values look authoritative.
    usage.exhausted = true;
    for error in [
        "timeout",
        "API error (403 Forbidden)",
        "API error (429 Too Many Requests)",
        "API error (500): body mentions 401 Unauthorized",
        "HTTP 4010",
    ] {
        usage.error = Some(error.into());
        assert_eq!(
            oauth_credential_status(false, Some(&usage)),
            CredentialStatus::Unknown,
            "{error}"
        );
    }
    usage.error = None;
    usage.exhausted = false;
    for ratio in [None, Some(f32::NAN), Some(-0.1), Some(1.1)] {
        usage.five_hour_ratio = ratio;
        assert_eq!(
            oauth_credential_status(false, Some(&usage)),
            CredentialStatus::Unknown
        );
    }
    for (status, dot, color) in [
        (
            CredentialStatus::Available,
            "●",
            auth_dot_color(AuthState::Available),
        ),
        (
            CredentialStatus::Attention,
            "◐",
            auth_dot_color(AuthState::Expired),
        ),
        (CredentialStatus::Blocked, "●", rgb(255, 100, 100)),
        (
            CredentialStatus::Unknown,
            "○",
            auth_dot_color(AuthState::NotConfigured),
        ),
    ] {
        for selected in [false, true] {
            let row = detail(status, "credential".into(), selected);
            assert_eq!(row.spans[0].content, if selected { "> " } else { "  " });
            assert_eq!(row.spans[1].content, dot);
            assert_eq!(row.spans[1].style.fg, Some(color));
            assert_eq!(row.width(), 14);
        }
    }
}

#[test]
fn router_inventory_empty_and_unknown_identities_remain_private() {
    assert_eq!(masked_identity(None), "saved account");
    assert_eq!(
        masked_identity(Some("secret-not-an-email")),
        "saved account"
    );
    assert_eq!(masked_identity(Some("a@bad\ndomain")), "saved account");
    assert_eq!(masked_identity(Some("éx@example.com")), "é*@example.com");
    let rendered = text(inventory_lines(
        &AuthStatus::default(),
        ActiveCredentialOverrides::default(),
        &Inventory::default(),
        "",
        None,
    ));
    assert!(rendered.contains("No auto-routing accounts"));
    assert!(rendered.contains("/login"));
}

#[test]
fn router_inventory_missing_usage_has_explicit_safe_status() {
    assert_eq!(usage_status_label(None), "limits pending");
    let mut usage = crate::usage::AccountUsageSnapshot {
        additional_windows: Vec::new(),
        label: "test".into(),
        email: None,
        exhausted: false,
        primary_label: None,
        five_hour_ratio: None,
        secondary_label: None,
        seven_day_ratio: None,
        resets_at: None,
        error: None,
    };
    assert_eq!(usage_status_label(Some(&usage)), "limits not reported");
    for (error, expected) in [
        (
            "Usage temporarily rate limited (429)",
            "usage throttled, retrying soon",
        ),
        (
            "Couldn't fetch usage (429: lookup throttled)",
            "usage throttled, retrying soon",
        ),
        (
            "Usage authentication failed (401)",
            "usage fetch needs login",
        ),
        ("secret transport diagnostic", "usage fetch failed"),
    ] {
        usage.error = Some(error.into());
        assert_eq!(usage_status_label(Some(&usage)), expected);
        assert!(remaining_bar(&usage, false).is_none());
    }
    usage.error = Some("Usage authentication failed (401)".into());
    assert_eq!(
        oauth_credential_status(false, Some(&usage)),
        CredentialStatus::Blocked
    );
}

#[test]
fn banked_expiry_colors_cover_boundaries_and_unknown() {
    let now = chrono::DateTime::parse_from_rfc3339("2026-11-06T08:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    for (hours, expected) in [
        (-1, Color::Rgb(255, 100, 100)),
        (24, Color::Rgb(255, 100, 100)),
        (25, Color::Rgb(255, 180, 80)),
        (72, Color::Rgb(255, 180, 80)),
        (73, Color::Rgb(230, 210, 100)),
        (168, Color::Rgb(230, 210, 100)),
        (169, Color::Rgb(100, 210, 130)),
    ] {
        let expiry = (now + chrono::Duration::hours(hours)).to_rfc3339();
        assert_eq!(banked_expiry_color(Some(&expiry), now), expected);
    }
    for expiry in [None, Some("invalid")] {
        assert_eq!(banked_expiry_color(expiry, now), Color::Rgb(120, 190, 255));
    }
}

#[test]
fn banked_resets_suffix_shows_count_or_next_unlock() {
    let ready = crate::usage::AccountBankedResets {
        available: 2,
        next_available_at: None,
        expires_at: None,
    };
    assert_eq!(banked_resets_span(&ready).unwrap().content, "↻ 2 resets");
    let expiring = crate::usage::AccountBankedResets {
        available: 1,
        next_available_at: None,
        expires_at: Some("2099-01-01T00:00:00Z".into()),
    };
    assert!(
        banked_resets_span(&expiring)
            .unwrap()
            .content
            .starts_with("↻ 1 reset · exp ")
    );
    let spent = crate::usage::AccountBankedResets {
        available: 0,
        next_available_at: Some("2099-01-01T00:00:00Z".into()),
        expires_at: None,
    };
    assert!(
        banked_resets_span(&spent)
            .unwrap()
            .content
            .starts_with("↻ in ")
    );
    let none = crate::usage::AccountBankedResets {
        available: 0,
        next_available_at: None,
        expires_at: None,
    };
    assert!(banked_resets_span(&none).is_none());
}

#[test]
fn router_inventory_weekly_only_usage_shows_reported_window() {
    let mut usage = crate::usage::AccountUsageSnapshot {
        additional_windows: Vec::new(),
        label: "weekly-only".into(),
        email: None,
        exhausted: false,
        primary_label: None,
        five_hour_ratio: None,
        secondary_label: Some("7-day window".into()),
        seven_day_ratio: Some(0.0),
        resets_at: None,
        error: None,
    };
    let bar = remaining_bar(&usage, false).unwrap();
    assert_eq!(bar.spans[0].content, "    ");
    assert_eq!(bar.spans[1].content, "7d ");
    assert!(text(vec![bar]).contains("100% left"));
    usage.seven_day_ratio = Some(1.0);
    assert!(text(vec![remaining_bar(&usage, false).unwrap()]).contains("0% left"));
    for ratio in [None, Some(f32::NAN), Some(-0.1), Some(1.1)] {
        usage.seven_day_ratio = ratio;
        assert!(remaining_bar(&usage, false).is_none());
    }
    usage.seven_day_ratio = Some(0.0);
    usage.error = Some("Usage unavailable".into());
    let stale = text(vec![remaining_bar(&usage, false).unwrap()]);
    assert!(stale.contains("100% left"));
    assert!(stale.contains("7d stale"));
    assert!(stale.contains("usage fetch failed"));
}

#[test]
fn router_inventory_remaining_bar_rejects_unknown_and_invalid_usage() {
    let mut usage = crate::usage::AccountUsageSnapshot {
        additional_windows: Vec::new(),
        label: "one".into(),
        email: None,
        exhausted: false,
        primary_label: Some("5h".into()),
        five_hour_ratio: Some(0.48),
        secondary_label: None,
        seven_day_ratio: None,
        resets_at: None,
        error: None,
    };
    let bar = remaining_bar(&usage, false).unwrap();
    assert!(text(vec![bar.clone()]).contains("52% left"));
    assert!(text(vec![bar.clone()]).contains('▰'));
    assert!(text(vec![bar.clone()]).contains('▱'));
    assert!(!text(vec![bar.clone()]).contains('['));
    assert_eq!(bar.spans[0].content, "    ");
    assert_eq!(bar.spans[1].content, "5h ");
    let shared = crate::tui::info_widget::render_usage_limit_bar("5h", 48, 52, None, 22, false);
    assert_eq!(bar.spans[2..], shared.spans[1..]);
    assert!(text(vec![remaining_bar(&usage, true).unwrap()]).contains("48% used"));
    for ratio in [None, Some(f32::NAN), Some(-0.1), Some(1.1)] {
        usage.five_hour_ratio = ratio;
        assert!(remaining_bar(&usage, false).is_none());
    }
    usage.five_hour_ratio = Some(0.0);
    usage.error = Some("HTTP 429".into());
    let stale = text(vec![remaining_bar(&usage, false).unwrap()]);
    assert!(stale.contains("100% left"));
    assert!(stale.contains("5h stale"));
    assert!(stale.contains("usage throttled, retrying soon"));
    usage.error = Some("HTTP 429 (retry at 2099-01-01T00:00:00Z)".into());
    let stale = text(vec![remaining_bar(&usage, false).unwrap()]);
    assert!(stale.contains("usage throttled, retry in "), "{stale}");
    usage.error = Some("HTTP 429".into());
    assert_eq!(
        oauth_credential_status(false, Some(&usage)),
        CredentialStatus::Unknown
    );
}
