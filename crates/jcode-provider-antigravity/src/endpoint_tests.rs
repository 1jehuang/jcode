use super::*;
use std::sync::Mutex;

// Serialize env-var mutation across tests in this module: `std::env` is
// process-global, and cargo runs tests in this crate on multiple threads.
static ENV_LOCK: Mutex<()> = Mutex::new(());

#[test]
fn antigravity_endpoint_defaults_to_daily_host() {
    let _guard = ENV_LOCK.lock().unwrap();
    unsafe {
        std::env::remove_var(ENDPOINT_ENV);
    }
    assert_eq!(antigravity_endpoint(), DEFAULT_ENDPOINT);
    assert_eq!(
        fetch_models_api_url(),
        format!("{DEFAULT_ENDPOINT}/v1internal:fetchAvailableModels")
    );
    assert_eq!(
        generate_content_api_url(),
        format!("{DEFAULT_ENDPOINT}/v1internal:generateContent")
    );
}

#[test]
fn antigravity_endpoint_honors_env_override_and_trims_trailing_slash() {
    let _guard = ENV_LOCK.lock().unwrap();
    unsafe {
        std::env::set_var(ENDPOINT_ENV, " https://example.googleapis.com/ ");
    }
    assert_eq!(antigravity_endpoint(), "https://example.googleapis.com");
    assert_eq!(
        fetch_models_api_url(),
        "https://example.googleapis.com/v1internal:fetchAvailableModels"
    );
    unsafe {
        std::env::remove_var(ENDPOINT_ENV);
    }
}

#[test]
fn antigravity_endpoint_ignores_blank_env_override() {
    let _guard = ENV_LOCK.lock().unwrap();
    unsafe {
        std::env::set_var(ENDPOINT_ENV, "   ");
    }
    assert_eq!(antigravity_endpoint(), DEFAULT_ENDPOINT);
    unsafe {
        std::env::remove_var(ENDPOINT_ENV);
    }
}

#[test]
fn antigravity_endpoint_falls_back_on_scheme_less_override() {
    let _guard = ENV_LOCK.lock().unwrap();
    unsafe {
        std::env::set_var(ENDPOINT_ENV, "example.googleapis.com");
    }
    assert_eq!(antigravity_endpoint(), DEFAULT_ENDPOINT);
    unsafe {
        std::env::remove_var(ENDPOINT_ENV);
    }
}

#[test]
fn antigravity_endpoint_falls_back_on_override_with_query_string() {
    let _guard = ENV_LOCK.lock().unwrap();
    unsafe {
        std::env::set_var(
            ENDPOINT_ENV,
            "https://example.googleapis.com?not=a-base-url",
        );
    }
    assert_eq!(antigravity_endpoint(), DEFAULT_ENDPOINT);
    unsafe {
        std::env::remove_var(ENDPOINT_ENV);
    }
}

#[test]
fn antigravity_endpoint_falls_back_on_override_with_fragment() {
    let _guard = ENV_LOCK.lock().unwrap();
    unsafe {
        std::env::set_var(ENDPOINT_ENV, "https://example.googleapis.com#frag");
    }
    assert_eq!(antigravity_endpoint(), DEFAULT_ENDPOINT);
    unsafe {
        std::env::remove_var(ENDPOINT_ENV);
    }
}

#[test]
fn antigravity_endpoint_accepts_valid_http_override() {
    let _guard = ENV_LOCK.lock().unwrap();
    unsafe {
        std::env::set_var(ENDPOINT_ENV, "http://localhost:8080");
    }
    assert_eq!(antigravity_endpoint(), "http://localhost:8080");
    unsafe {
        std::env::remove_var(ENDPOINT_ENV);
    }
}

fn sample_catalog(endpoint: Option<&str>) -> PersistedCatalog {
    PersistedCatalog {
        models: vec![CatalogModel {
            id: "gemini-3-flash".to_string(),
            display_name: None,
            reset_time: None,
            tag_title: None,
            model_provider: None,
            max_tokens: None,
            max_output_tokens: None,
            recommended: false,
            available: true,
            remaining_fraction_milli: None,
        }],
        fetched_at_rfc3339: "2026-01-01T00:00:00Z".to_string(),
        default_model_id: None,
        endpoint: endpoint.map(str::to_string),
        cli_version: Some(ANTIGRAVITY_VERSION.to_string()),
    }
}

#[test]
fn catalog_with_no_recorded_endpoint_matches_any_current_endpoint() {
    let _guard = ENV_LOCK.lock().unwrap();
    unsafe {
        std::env::remove_var(ENDPOINT_ENV);
    }
    assert!(catalog_matches_current_endpoint(&sample_catalog(None)));
}

#[test]
fn catalog_stamped_with_current_endpoint_matches() {
    let _guard = ENV_LOCK.lock().unwrap();
    unsafe {
        std::env::remove_var(ENDPOINT_ENV);
    }
    assert!(catalog_matches_current_endpoint(&sample_catalog(Some(
        DEFAULT_ENDPOINT
    ))));
}

#[test]
fn catalog_stamped_with_stale_endpoint_does_not_match_after_override() {
    let _guard = ENV_LOCK.lock().unwrap();
    unsafe {
        std::env::set_var(ENDPOINT_ENV, "https://example.googleapis.com");
    }
    // Cached from the old default before the override was set.
    assert!(!catalog_matches_current_endpoint(&sample_catalog(Some(
        DEFAULT_ENDPOINT
    ))));
    unsafe {
        std::env::remove_var(ENDPOINT_ENV);
    }
}

struct VersionEnvGuard {
    _lock: std::sync::MutexGuard<'static, ()>,
    prev: Option<std::ffi::OsString>,
}

impl VersionEnvGuard {
    fn acquire() -> Self {
        let lock = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let prev = std::env::var_os(VERSION_ENV);
        Self { _lock: lock, prev }
    }
}

impl Drop for VersionEnvGuard {
    fn drop(&mut self) {
        unsafe {
            if let Some(prev) = self.prev.as_ref() {
                std::env::set_var(VERSION_ENV, prev);
            } else {
                std::env::remove_var(VERSION_ENV);
            }
        }
    }
}

#[test]
fn resolve_version_honors_env_then_detected_cli_then_fallback() {
    assert_eq!(
        resolve_antigravity_version_with(Some("antigravity/cli/1.6.2"), Some("1.5.5")),
        "1.6.2"
    );
    assert_eq!(
        resolve_antigravity_version_with(Some("  cli/1.4.0  "), Some("1.5.5")),
        "1.4.0"
    );
    assert_eq!(
        resolve_antigravity_version_with(Some("   "), Some("1.6.0-rc.1")),
        "1.6.0-rc.1"
    );
    assert_eq!(
        resolve_antigravity_version_with(Some("not-a-semver\r\nBad: 1"), Some("1.5.5")),
        "1.5.5"
    );
    assert_eq!(
        resolve_antigravity_version_with(None, Some("garbage")),
        ANTIGRAVITY_VERSION
    );
    assert_eq!(
        resolve_antigravity_version_with(None, None),
        ANTIGRAVITY_VERSION
    );
}

#[test]
fn parse_agy_version_output_extracts_valid_semver_and_rejects_garbage() {
    assert_eq!(parse_agy_version_output("1.5.5\n"), Some("1.5.5".into()));
    assert_eq!(
        parse_agy_version_output("\n  agy version 1.6.0 (2026-11-14)\n"),
        Some("1.6.0".into())
    );
    assert_eq!(
        parse_agy_version_output("antigravity/cli/1.5.5 darwin/arm64\n"),
        Some("1.5.5".into())
    );
    assert_eq!(
        parse_agy_version_output("v2.0.1-beta.2\nDate: 2026-11-14\n"),
        Some("2.0.1-beta.2".into())
    );
    for invalid in [
        "",
        "   \n\t",
        "error: command failed",
        "1",
        "1.5.5; echo injected",
        "1.5.5-",
        "1..5",
    ] {
        assert_eq!(
            parse_agy_version_output(invalid),
            None,
            "invalid output {invalid:?} must be rejected"
        );
    }
}

#[cfg(unix)]
#[test]
fn probe_agy_version_command_handles_success_failure_missing_and_timeout() {
    assert_eq!(
        probe_agy_version_command(
            "/bin/sh",
            &["-c", "printf '1.7.3\\n'"],
            Duration::from_millis(500),
        ),
        Some("1.7.3".to_string())
    );
    assert_eq!(
        probe_agy_version_command(
            "/bin/sh",
            &["-c", "printf '1.7.3\\n'; exit 1"],
            Duration::from_millis(500),
        ),
        None
    );
    assert_eq!(
        probe_agy_version_command(
            "/nonexistent/jcode-missing-agy-binary",
            &["--version"],
            Duration::from_millis(200),
        ),
        None
    );
    let start = Instant::now();
    assert_eq!(
        probe_agy_version_command("/bin/sh", &["-c", "sleep 2"], Duration::from_millis(80)),
        None
    );
    assert!(
        start.elapsed() < Duration::from_millis(1000),
        "timed-out agy probe must be killed promptly"
    );
}

#[test]
fn antigravity_user_agent_normalizes_env_overrides_and_falls_back_cleanly() {
    let _guard = VersionEnvGuard::acquire();
    unsafe {
        std::env::remove_var(VERSION_ENV);
    }
    let expected_default = detected_agy_version().unwrap_or(ANTIGRAVITY_VERSION);
    assert!(is_valid_antigravity_semver(expected_default));
    assert_eq!(antigravity_version(), expected_default);
    assert_eq!(
        antigravity_user_agent(),
        format!(
            "antigravity/cli/{expected_default} {}",
            antigravity_platform_suffix()
        )
    );

    for blank in [
        "",
        "   ",
        "\t\n",
        "cli/",
        "  cli/   ",
        "antigravity/cli/",
        "bad\r\nheader",
    ] {
        unsafe {
            std::env::set_var(VERSION_ENV, blank);
        }
        assert_eq!(
            antigravity_version(),
            expected_default,
            "invalid override {blank:?} must fall back cleanly"
        );
    }

    for (expected_ver, raw_variants) in [
        (
            "1.4.0",
            [
                "1.4.0",
                "  1.4.0  ",
                "cli/1.4.0",
                "antigravity/cli/1.4.0",
                "  antigravity/cli/1.4.0  ",
            ],
        ),
        (
            "1.5.5",
            [
                "1.5.5",
                "v1.5.5",
                "cli/1.5.5",
                "antigravity/cli/1.5.5",
                "  cli/1.5.5  ",
            ],
        ),
    ] {
        let expected_ua = format!(
            "antigravity/cli/{expected_ver} {}",
            antigravity_platform_suffix()
        );
        for raw in raw_variants {
            unsafe {
                std::env::set_var(VERSION_ENV, raw);
            }
            assert_eq!(antigravity_version(), expected_ver);
            assert_eq!(antigravity_user_agent(), expected_ua);
        }
    }
}

#[test]
fn catalog_cli_version_and_legacy_non_cli_checks_invalidate_stale_caches() {
    let _guard = VersionEnvGuard::acquire();
    unsafe {
        std::env::set_var(VERSION_ENV, "1.5.5");
    }
    let mut catalog = sample_catalog(Some(DEFAULT_ENDPOINT));
    catalog.cli_version = Some("1.5.5".to_string());
    assert!(catalog_matches_current_cli_version(&catalog));

    catalog.cli_version = Some("1.4.2".to_string());
    assert!(!catalog_matches_current_cli_version(&catalog));

    catalog.cli_version = None;
    assert!(!catalog_matches_current_cli_version(&catalog));

    // Legacy non-CLI marker without Argon/Carbon models is rejected.
    catalog.models[0].id = "gemini-3.8-flash-tiered".to_string();
    assert!(!catalog_has_cli_models(&catalog));

    let mut carbon_model = catalog.models[0].clone();
    carbon_model.id = "gemini-4-carbon-medium".to_string();
    catalog.models.push(carbon_model);
    assert!(catalog_has_cli_models(&catalog));
}

#[test]
fn model_aliases_and_exclusivity_match_ascii_case_insensitively() {
    for (input, expected) in [
        ("CARBON-LOW", "gemini-4-carbon-low"),
        ("Carbon", "gemini-4-carbon-medium"),
        ("Gemini-4-Argon", "gemini-4-argon-medium"),
        ("ARGON-HIGH", "gemini-4-argon-high"),
        ("GEMINI-3.8-FLASH", "gemini-3.8-flash-medium"),
        ("GEMINI-3.1-PRO-HIGH", "gemini-pro-agent"),
        ("GEMINI-4-CARBON-HIGH", "gemini-4-carbon-high"),
        ("GPT-OSS-120B-MEDIUM", "gpt-oss-120b-medium"),
    ] {
        assert_eq!(remap_unsupported_model(input), expected, "input={input}");
        assert!(is_known_model(input), "input={input}");
        assert!(is_antigravity_exclusive_model(input), "input={input}");
    }

    for (shared, canonical) in [
        ("CLAUDE-SONNET-4-6", "claude-sonnet-4-6"),
        ("Gemini-3-Flash", "gemini-3-flash"),
    ] {
        assert_eq!(remap_unsupported_model(shared), canonical);
        assert!(is_known_model(shared));
        assert!(!is_antigravity_exclusive_model(shared));
    }

    assert_eq!(
        remap_unsupported_model("Gemini-5-Xenon-Ultra"),
        "Gemini-5-Xenon-Ultra"
    );
    assert!(!is_known_model("Gemini-5-Xenon-Ultra"));
    assert!(!is_antigravity_exclusive_model("Gemini-5-Xenon-Ultra"));
}
