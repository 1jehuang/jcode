use super::antigravity_tests::TestEnvGuard;
use super::*;
use chrono::Utc;
use jcode_provider_antigravity::{
    FetchAvailableModelsResponse, parse_fetch_available_models_response,
};
use jcode_provider_core::Provider;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn test_catalog_entry(id: &str, available: bool) -> CatalogModel {
    CatalogModel {
        id: id.to_string(),
        display_name: None,
        reset_time: None,
        tag_title: None,
        model_provider: None,
        max_tokens: None,
        max_output_tokens: None,
        recommended: true,
        available,
        remaining_fraction_milli: Some(if available { 1000 } else { 0 }),
    }
}

#[test]
fn load_persisted_catalog_invalidates_unstamped_mismatched_and_legacy_non_cli_caches() {
    let _env = TestEnvGuard::acquire();
    jcode_base::env::set_var("JCODE_ANTIGRAVITY_VERSION", "1.5.5");

    let path = jcode_base::provider::antigravity::persisted_catalog_path().expect("catalog path");

    // 1. Unstamped legacy cache (`cli_version: None`) must be invalidated.
    jcode_base::storage::write_json(
        &path,
        &PersistedCatalog {
            models: vec![test_catalog_entry("gemini-4-argon-low", true)],
            fetched_at_rfc3339: Utc::now().to_rfc3339(),
            default_model_id: Some("gemini-4-argon-low".to_string()),
            endpoint: None,
            cli_version: None,
        },
    )
    .expect("write unstamped catalog");
    assert!(jcode_base::provider::antigravity::load_persisted_catalog().is_none());

    // 2. Version-mismatched cache (`cli_version: Some("1.3.5")`) must be invalidated
    // so upgrading `agy` triggers a fresh catalog fetch for newly unlocked models.
    jcode_base::storage::write_json(
        &path,
        &PersistedCatalog {
            models: vec![test_catalog_entry("gemini-4-argon-low", true)],
            fetched_at_rfc3339: Utc::now().to_rfc3339(),
            default_model_id: Some("gemini-4-argon-low".to_string()),
            endpoint: None,
            cli_version: Some("1.3.5".to_string()),
        },
    )
    .expect("write mismatched-version catalog");
    assert!(jcode_base::provider::antigravity::load_persisted_catalog().is_none());

    // 3. Legacy non-CLI catalog (`gemini-3.8-flash-tiered` without Carbon/Argon)
    // must be rejected even if stamped with the current CLI version.
    jcode_base::storage::write_json(
        &path,
        &PersistedCatalog {
            models: vec![test_catalog_entry("gemini-3.8-flash-tiered", true)],
            fetched_at_rfc3339: Utc::now().to_rfc3339(),
            default_model_id: None,
            endpoint: None,
            cli_version: Some("1.5.5".to_string()),
        },
    )
    .expect("write legacy tiered catalog");
    assert!(jcode_base::provider::antigravity::load_persisted_catalog().is_none());

    // 4. `persist_catalog` stamps `cli_version` automatically and loads cleanly.
    jcode_base::provider::antigravity::persist_catalog(&CatalogSnapshot {
        models: vec![
            test_catalog_entry("gemini-4-carbon-low", true),
            test_catalog_entry("gemini-4-argon-low", true),
        ],
        default_model_id: Some("gemini-4-carbon-low".to_string()),
    });
    let loaded = jcode_base::provider::antigravity::load_persisted_catalog()
        .expect("current CLI catalog must load");
    assert_eq!(loaded.cli_version.as_deref(), Some("1.5.5"));
    assert_eq!(
        loaded.default_model_id.as_deref(),
        Some("gemini-4-carbon-low")
    );
}

#[test]
fn future_catalog_model_absent_from_static_fallbacks_is_discovered_in_switching_and_routes() {
    assert!(
        !AVAILABLE_MODELS.contains(&"gemini-5-xenon-ultra"),
        "test fixture model must be absent from static AVAILABLE_MODELS"
    );

    let response: FetchAvailableModelsResponse = serde_json::from_value(serde_json::json!({
        "defaultAgentModelId": "gemini-5-xenon-ultra",
        "models": {
            "gemini-5-xenon-ultra": {
                "displayName": "Gemini 5 Xenon Ultra",
                "quotaInfo": { "remainingFraction": 0.9, "resetTime": "2026-12-01T00:00:00Z" },
                "recommended": true,
                "maxTokens": 2_000_000,
                "maxOutputTokens": 65_536,
                "modelProvider": "MODEL_PROVIDER_GOOGLE"
            }
        }
    }))
    .expect("parse future catalog response");

    let snapshot = parse_fetch_available_models_response(&response);
    let provider = AntigravityProvider::empty_for_tests();
    *provider.fetched_catalog.write().expect("catalog lock") = snapshot.models;
    *provider
        .backend_default_model
        .write()
        .expect("default lock") = snapshot.default_model_id;

    assert!(
        provider
            .available_models_for_switching()
            .contains(&"gemini-5-xenon-ultra".to_string())
    );
    assert!(
        provider
            .available_models_display()
            .contains(&"gemini-5-xenon-ultra".to_string())
    );
    assert_eq!(
        provider.resolve_model_for_request("default"),
        "gemini-5-xenon-ultra"
    );
    assert_eq!(provider.context_window(), 2_000_000);

    let routes = provider.model_routes();
    let future_route = routes
        .iter()
        .find(|route| route.model == "gemini-5-xenon-ultra")
        .expect("future catalog model route");
    assert!(future_route.available);
    assert!(future_route.detail.contains("Gemini 5 Xenon Ultra"));
}

#[test]
fn model_routes_marks_omitted_static_fallbacks_unverified_in_sparse_catalog_and_preserves_false() {
    let provider = AntigravityProvider::empty_for_tests();
    *provider.fetched_catalog.write().expect("catalog lock") = vec![
        CatalogModel {
            id: "gemini-3-flash".to_string(),
            display_name: Some("Gemini 3 Flash".to_string()),
            reset_time: None,
            tag_title: None,
            model_provider: Some("MODEL_PROVIDER_GOOGLE".to_string()),
            max_tokens: None,
            max_output_tokens: None,
            recommended: true,
            available: true,
            remaining_fraction_milli: Some(1000),
        },
        CatalogModel {
            id: "claude-sonnet-4-6".to_string(),
            display_name: Some("Claude Sonnet 4.6".to_string()),
            reset_time: Some("2026-10-15T00:00:00Z".to_string()),
            tag_title: None,
            model_provider: Some("MODEL_PROVIDER_ANTHROPIC".to_string()),
            max_tokens: None,
            max_output_tokens: None,
            recommended: false,
            available: false,
            remaining_fraction_milli: Some(0),
        },
        // Duplicate entry in fetched_catalog must be deduplicated, keeping first entry.
        CatalogModel {
            id: "claude-sonnet-4-6".to_string(),
            display_name: Some("Duplicate Claude Sonnet".to_string()),
            reset_time: None,
            tag_title: None,
            model_provider: None,
            max_tokens: None,
            max_output_tokens: None,
            recommended: false,
            available: true,
            remaining_fraction_milli: Some(1000),
        },
    ];

    let routes = provider.model_routes();

    let flash_routes: Vec<_> = routes
        .iter()
        .filter(|route| route.model == "gemini-3-flash")
        .collect();
    assert_eq!(flash_routes.len(), 1, "gemini-3-flash must not duplicate");
    assert!(flash_routes[0].available);
    assert!(flash_routes[0].detail.contains("Gemini 3 Flash"));

    let claude_routes: Vec<_> = routes
        .iter()
        .filter(|route| route.model == "claude-sonnet-4-6")
        .collect();
    assert_eq!(
        claude_routes.len(),
        1,
        "claude-sonnet-4-6 must be deduplicated across catalog and fallback list"
    );
    assert!(
        !claude_routes[0].available,
        "explicit available:false in fetched catalog must be preserved"
    );
    assert!(claude_routes[0].detail.contains("quota 0.0%"));

    // Static fallback models absent from the sparse fetched catalog must be
    // present as unverified fallbacks (available: false), not marked verified.
    for omitted in [
        "gemini-4-carbon-high",
        "gemini-4-carbon-medium",
        "gemini-4-carbon-low",
        "gemini-4-argon-high",
        "gemini-4-argon-medium",
        "gemini-4-argon-low",
        "gemini-3.8-flash-medium",
    ] {
        let matching: Vec<_> = routes
            .iter()
            .filter(|route| route.model == omitted)
            .collect();
        assert_eq!(matching.len(), 1, "{omitted} should appear once in routes");
        assert!(
            !matching[0].available,
            "omitted static fallback {omitted} must not be marked verified available when catalog is non-empty"
        );
        assert_eq!(matching[0].detail, "fallback catalog (unverified)");
    }
}

#[test]
fn model_routes_reconciles_alias_availability_by_canonical_request_id() {
    let provider = AntigravityProvider::empty_for_tests();
    *provider.fetched_catalog.write().expect("catalog lock") = vec![
        CatalogModel {
            id: "gemini-4-carbon".to_string(),
            display_name: Some("Gemini 4 Carbon".to_string()),
            reset_time: None,
            tag_title: None,
            model_provider: None,
            max_tokens: None,
            max_output_tokens: None,
            recommended: false,
            available: false,
            remaining_fraction_milli: Some(0),
        },
        CatalogModel {
            id: "gemini-4-carbon-medium".to_string(),
            display_name: Some("Gemini 4 Carbon Medium".to_string()),
            reset_time: None,
            tag_title: None,
            model_provider: None,
            max_tokens: None,
            max_output_tokens: None,
            recommended: false,
            available: true,
            remaining_fraction_milli: Some(1000),
        },
        test_catalog_entry("gemini-3-flash", true),
    ];

    let routes = provider.model_routes();
    let bare_carbon = routes
        .iter()
        .find(|route| route.model == "gemini-4-carbon")
        .expect("bare carbon route");
    let medium_carbon = routes
        .iter()
        .find(|route| route.model == "gemini-4-carbon-medium")
        .expect("medium carbon route");

    assert!(!bare_carbon.available);
    assert!(
        !medium_carbon.available,
        "canonical target must not be marked available when its alias is unavailable"
    );
    assert!(
        medium_carbon
            .detail
            .contains("canonical target 'gemini-4-carbon-medium' unavailable"),
        "expected canonical veto explanation in detail, got: {}",
        medium_carbon.detail
    );

    // Exact-ID duplicate policy is keep-first: a trailing `available: false`
    // duplicate of the exact same ID must not veto the first retained `true` row.
    *provider.fetched_catalog.write().expect("catalog lock") = vec![
        test_catalog_entry("gemini-3-flash", true),
        test_catalog_entry(" gemini-3-flash ", false),
    ];
    let deduped_routes = provider.model_routes();
    let flash = deduped_routes
        .iter()
        .find(|route| route.model == "gemini-3-flash")
        .expect("gemini-3-flash route");
    assert!(
        flash.available,
        "exact-ID deduplication is keep-first and must retain the first row's availability"
    );
}

#[test]
fn antigravity_context_window_resolves_carbon_and_argon_defaults_and_catalog_fallback() {
    let provider = AntigravityProvider::empty_for_tests();
    *provider.fetched_catalog.write().expect("catalog lock") = vec![CatalogModel {
        id: "chat_20706".to_string(),
        display_name: None,
        reset_time: None,
        tag_title: None,
        model_provider: None,
        max_tokens: Some(16_384),
        max_output_tokens: None,
        recommended: false,
        available: true,
        remaining_fraction_milli: Some(1000),
    }];
    *provider
        .backend_default_model
        .write()
        .expect("default lock") = Some("gemini-4-carbon".to_string());

    assert_eq!(
        provider.resolve_model_for_request("default"),
        "gemini-4-carbon-medium"
    );
    assert_eq!(provider.context_window(), 1_000_000);
    for model in [
        "gemini-4-carbon",
        "carbon-low",
        "gemini-4-carbon-high",
        "gemini-4-argon",
        "argon-low",
        "gemini-4-argon-high",
    ] {
        provider.set_model(model).unwrap();
        assert_eq!(provider.context_window(), 1_000_000, "model={model}");
    }
    provider.set_model("chat_20706").unwrap();
    assert_eq!(provider.context_window(), 16_384);
}

#[test]
fn resolve_model_for_request_skips_alias_when_canonical_target_is_unavailable() {
    // Gemini-preferred fallback branch: available `gemini-4-carbon` alias whose
    // canonical `gemini-4-carbon-medium` target is unavailable must be skipped
    // in favor of available `gemini-3-flash`.
    let provider = AntigravityProvider::empty_for_tests();
    *provider.fetched_catalog.write().expect("catalog lock") = vec![
        test_catalog_entry("gemini-4-carbon", true),
        test_catalog_entry("gemini-4-carbon-medium", false),
        test_catalog_entry("gemini-3-flash", true),
    ];
    *provider
        .backend_default_model
        .write()
        .expect("default lock") = Some("gemini-4-carbon".to_string());
    assert_eq!(
        provider.resolve_model_for_request("default"),
        "gemini-3-flash"
    );
    assert_eq!(provider.resolve_model_for_request(""), "gemini-3-flash");

    // General non-Gemini catalog fallback branch: when the only Gemini entries
    // map to an unavailable canonical target, fallback continues to the next
    // available non-Gemini catalog entry.
    *provider
        .backend_default_model
        .write()
        .expect("default lock") = None;
    *provider.fetched_catalog.write().expect("catalog lock") = vec![
        test_catalog_entry("gemini-4-argon", true),
        test_catalog_entry("gemini-4-argon-medium", false),
        test_catalog_entry("gpt-oss-120b-medium", true),
    ];
    assert_eq!(
        provider.resolve_model_for_request("default"),
        "gpt-oss-120b-medium"
    );
    assert_eq!(
        provider.resolve_model_for_request(""),
        "gpt-oss-120b-medium"
    );
}

struct MockCatalogServer {
    base_url: String,
    fetch_models_hits: Arc<AtomicUsize>,
    last_user_agent: Arc<Mutex<Option<String>>>,
    next_response: Arc<Mutex<(u16, String)>>,
    task: tokio::task::JoinHandle<()>,
}

impl MockCatalogServer {
    async fn start(initial_status: u16, initial_body: String) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind mock catalog server");
        let addr = listener.local_addr().expect("local addr");
        let fetch_models_hits = Arc::new(AtomicUsize::new(0));
        let last_user_agent = Arc::new(Mutex::new(None));
        let next_response = Arc::new(Mutex::new((initial_status, initial_body)));

        let hits_clone = Arc::clone(&fetch_models_hits);
        let ua_clone = Arc::clone(&last_user_agent);
        let resp_clone = Arc::clone(&next_response);
        let task = tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let hits = Arc::clone(&hits_clone);
                let ua = Arc::clone(&ua_clone);
                let resp = Arc::clone(&resp_clone);
                tokio::spawn(async move {
                    let mut buf = Vec::with_capacity(2048);
                    let mut chunk = [0_u8; 1024];
                    let header_end = loop {
                        match stream.read(&mut chunk).await {
                            Ok(0) | Err(_) => return,
                            Ok(n) => {
                                buf.extend_from_slice(&chunk[..n]);
                                if let Some(pos) =
                                    buf.windows(4).position(|window| window == b"\r\n\r\n")
                                {
                                    break pos + 4;
                                }
                            }
                        }
                    };

                    let header_text = String::from_utf8_lossy(&buf[..header_end]).into_owned();
                    let content_length = header_text
                        .lines()
                        .find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            name.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().ok())
                                .flatten()
                        })
                        .unwrap_or(0);
                    while buf.len() < header_end + content_length {
                        match stream.read(&mut chunk).await {
                            Ok(0) | Err(_) => break,
                            Ok(n) => buf.extend_from_slice(&chunk[..n]),
                        }
                    }

                    let request_line = header_text.lines().next().unwrap_or("");
                    let (status, body) = if request_line.contains("loadCodeAssist") {
                        (
                            200,
                            r#"{"cloudaicompanionProject":"mock-project"}"#.to_string(),
                        )
                    } else {
                        for line in header_text.lines() {
                            if let Some((name, value)) = line.split_once(':')
                                && name.eq_ignore_ascii_case("user-agent")
                            {
                                *ua.lock().expect("ua lock") = Some(value.trim().to_string());
                            }
                        }
                        hits.fetch_add(1, Ordering::SeqCst);
                        resp.lock().expect("response lock").clone()
                    };

                    let reason = if status == 200 {
                        "OK"
                    } else {
                        "Internal Server Error"
                    };
                    let raw = format!(
                        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = stream.write_all(raw.as_bytes()).await;
                    let _ = stream.shutdown().await;
                });
            }
        });

        Self {
            base_url: format!("http://{addr}"),
            fetch_models_hits,
            last_user_agent,
            next_response,
            task,
        }
    }

    fn set_response(&self, status: u16, body: impl Into<String>) {
        *self.next_response.lock().expect("response lock") = (status, body.into());
    }

    fn hits(&self) -> usize {
        self.fetch_models_hits.load(Ordering::SeqCst)
    }

    fn reset_hits(&self) {
        self.fetch_models_hits.store(0, Ordering::SeqCst);
    }
}

impl Drop for MockCatalogServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

#[test]
fn live_catalog_discovery_handles_future_models_fresh_cache_forced_refresh_and_failures() {
    let _env = TestEnvGuard::acquire();
    jcode_base::env::set_var("JCODE_ANTIGRAVITY_VERSION", "1.5.5");
    jcode_base::auth::antigravity::save_tokens(&jcode_base::auth::antigravity::AntigravityTokens {
        access_token: "test-antigravity-access-token".to_string(),
        refresh_token: "test-antigravity-refresh-token".to_string(),
        expires_at: Utc::now().timestamp_millis() + 3_600_000,
        email: Some("tester@example.com".to_string()),
        project_id: Some("test-project".to_string()),
    })
    .expect("save test antigravity tokens");

    assert!(
        !AVAILABLE_MODELS.contains(&"gemini-5-xenon-ultra")
            && !AVAILABLE_MODELS.contains(&"gemini-5-radon-pro")
    );

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("test runtime");

    runtime.block_on(async {
        let initial_payload = serde_json::json!({
            "defaultAgentModelId": "gemini-5-xenon-ultra",
            "commandModelIds": ["gemini-4-carbon-low"],
            "models": {
                "gemini-5-xenon-ultra": {
                    "displayName": "Gemini 5 Xenon Ultra",
                    "quotaInfo": { "remainingFraction": 0.95, "resetTime": "2026-12-01T00:00:00Z" },
                    "recommended": true,
                    "maxTokens": 2_000_000,
                    "maxOutputTokens": 65_536,
                    "modelProvider": "MODEL_PROVIDER_GOOGLE"
                },
                "gemini-4-carbon-low": {
                    "displayName": "Gemini 4 Carbon Low",
                    "quotaInfo": { "remainingFraction": 1.0 },
                    "recommended": false,
                    "maxTokens": 1_000_000,
                    "modelProvider": "MODEL_PROVIDER_GOOGLE"
                }
            }
        })
        .to_string();

        let server = MockCatalogServer::start(200, initial_payload).await;
        jcode_base::env::set_var("JCODE_ANTIGRAVITY_ENDPOINT", &server.base_url);

        // (a) Cold provider discovers future model over HTTP and exposes it in
        // switching lists and picker routes.
        let provider = AntigravityProvider::new();
        assert!(provider.model_capabilities_need_refresh());
        assert_eq!(server.hits(), 0);

        provider
            .prefetch_models()
            .await
            .expect("cold prefetch should succeed");
        assert_eq!(server.hits(), 1);
        assert!(!provider.model_capabilities_need_refresh());
        let observed_ua = server
            .last_user_agent
            .lock()
            .expect("ua lock")
            .clone()
            .expect("captured user-agent");
        assert!(
            observed_ua.starts_with("antigravity/cli/1.5.5 "),
            "unexpected User-Agent: {observed_ua}"
        );
        assert!(
            provider
                .available_models_for_switching()
                .contains(&"gemini-5-xenon-ultra".to_string())
        );
        assert_eq!(
            provider.resolve_model_for_request("default"),
            "gemini-5-xenon-ultra"
        );
        let xenon_route = provider
            .model_routes()
            .into_iter()
            .find(|route| route.model == "gemini-5-xenon-ultra")
            .expect("gemini-5-xenon-ultra route");
        assert!(xenon_route.available);
        assert!(xenon_route.detail.contains("Gemini 5 Xenon Ultra"));

        // (b) `prefetch_models` makes 0 HTTP requests while cache is fresh (both
        // warm in-memory, newly constructed from disk, and empty-memory seeded
        // from fresh disk cache), whereas `refresh_model_catalog` bypasses fresh
        // caches and clears a removed `defaultAgentModelId`.
        let updated_payload = serde_json::json!({
            "models": {
                "gemini-5-xenon-ultra": {
                    "displayName": "Gemini 5 Xenon Ultra",
                    "quotaInfo": { "remainingFraction": 0.80 },
                    "recommended": true,
                    "maxTokens": 2_000_000,
                    "modelProvider": "MODEL_PROVIDER_GOOGLE"
                },
                "gemini-5-radon-pro": {
                    "displayName": "Gemini 5 Radon Pro",
                    "quotaInfo": { "remainingFraction": 1.0 },
                    "recommended": true,
                    "maxTokens": 1_500_000,
                    "modelProvider": "MODEL_PROVIDER_GOOGLE"
                },
                "gemini-4-carbon-low": {
                    "displayName": "Gemini 4 Carbon Low",
                    "quotaInfo": { "remainingFraction": 1.0 },
                    "recommended": false,
                    "maxTokens": 1_000_000,
                    "modelProvider": "MODEL_PROVIDER_GOOGLE"
                }
            }
        })
        .to_string();
        server.set_response(200, updated_payload);
        server.reset_hits();

        provider
            .prefetch_models()
            .await
            .expect("warm prefetch should skip network");
        assert_eq!(
            server.hits(),
            0,
            "warm in-memory prefetch must do 0 HTTP requests"
        );
        assert!(
            !provider
                .available_models_for_switching()
                .contains(&"gemini-5-radon-pro".to_string())
        );

        let disk_seeded = AntigravityProvider::new();
        assert!(!disk_seeded.model_capabilities_need_refresh());
        disk_seeded
            .prefetch_models()
            .await
            .expect("disk-seeded prefetch should skip network");
        assert_eq!(
            server.hits(),
            0,
            "disk-seeded prefetch must do 0 HTTP requests"
        );

        let empty_mem = AntigravityProvider::empty_for_tests();
        empty_mem
            .prefetch_models()
            .await
            .expect("empty-memory prefetch should seed from fresh disk cache");
        assert_eq!(
            server.hits(),
            0,
            "fresh disk cache must satisfy empty-memory prefetch"
        );
        assert!(
            empty_mem
                .available_models_for_switching()
                .contains(&"gemini-5-xenon-ultra".to_string())
        );

        let summary = provider
            .refresh_model_catalog()
            .await
            .expect("explicit refresh must bypass fresh cache");
        assert!(server.hits() >= 1, "explicit refresh must hit mock server");
        assert!(summary.models_added >= 1);
        assert!(
            provider
                .available_models_for_switching()
                .contains(&"gemini-5-radon-pro".to_string())
        );
        assert_eq!(
            provider.backend_default_model(),
            None,
            "omitted defaultAgentModelId must clear stale backend_default_model"
        );

        // (c) HTTP 500, HTTP 200 `{}` (empty catalog), and malformed JSON on
        // explicit refresh all return `Err` and preserve the last-good in-memory
        // and persisted catalogs + picker routes.
        let expected_switching = provider.available_models_for_switching();
        let expected_routes = provider.model_routes();
        let expected_disk = jcode_base::provider::antigravity::load_persisted_catalog()
            .expect("persisted catalog after refresh");

        for (label, status, body) in [
            ("HTTP 500", 500_u16, r#"{"error":"backend outage"}"#),
            ("empty JSON object", 200_u16, "{}"),
            ("malformed JSON", 200_u16, "{not-valid-json"),
        ] {
            server.set_response(status, body);
            assert!(
                provider.refresh_model_catalog().await.is_err(),
                "{label} must fail explicit refresh"
            );
            assert_eq!(
                provider.available_models_for_switching(),
                expected_switching,
                "{label} must preserve switching models"
            );
            assert_eq!(
                provider.model_routes(),
                expected_routes,
                "{label} must preserve picker routes"
            );
            let disk_after = jcode_base::provider::antigravity::load_persisted_catalog()
                .expect("persisted catalog must survive failure");
            assert_eq!(
                disk_after.models, expected_disk.models,
                "{label} must preserve persisted disk catalog"
            );
        }
    });
}
