//! Model discovery regression tests for the built-in IO Intelligence profile.
//!
//! IO Intelligence serves `GET /v1/models` in the OpenAI-compatible list shape,
//! with the live context limit in `context_window` and the display label in
//! `name`. These tests use that response shape to check that a model which is
//! absent from every offline fallback array is discovered, exposed as a picker
//! route, picked up by an explicit refresh even while the cache is fresh, and
//! that a failed refresh keeps the last good catalog.

use super::*;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

const IONET_DEFAULT_MODEL: &str = "meta-llama/Llama-3.3-70B-Instruct";
const IONET_FUTURE_MODEL: &str = "meta-llama/Llama-5-Future-Instruct";

/// Catalog as served today: only the current default model.
const IONET_CATALOG_CURRENT: &str = r#"{
    "object": "list",
    "data": [
        {
            "id": "meta-llama/Llama-3.3-70B-Instruct",
            "name": "Meta: Llama 3.3 70B Instruct",
            "object": "model",
            "created": 1733762183,
            "owned_by": "io-intelligence",
            "root": null,
            "parent": null,
            "max_model_len": null,
            "max_tokens": null,
            "context_window": 128000,
            "supports_reasoning": false,
            "supports_tools": true,
            "supports_responses": false,
            "input_modalities": ["text"],
            "output_modalities": ["text"],
            "supports_prompt_cache": false,
            "input_token_price": 4.786e-07,
            "output_token_price": 1.0946e-06,
            "cache_read_token_price": 2.393e-07,
            "precision": "fp16",
            "supports_attestation": false,
            "min_access_tier": 2,
            "higher_tier_required": false,
            "permission": []
        }
    ]
}"#;

/// The same catalog after the provider ships a model Jcode has never heard of.
const IONET_CATALOG_WITH_FUTURE_MODEL: &str = r#"{
    "object": "list",
    "data": [
        {
            "id": "meta-llama/Llama-3.3-70B-Instruct",
            "name": "Meta: Llama 3.3 70B Instruct",
            "object": "model",
            "created": 1733762183,
            "owned_by": "io-intelligence",
            "max_model_len": null,
            "max_tokens": null,
            "context_window": 128000,
            "supports_reasoning": false,
            "supports_tools": true,
            "input_modalities": ["text"],
            "output_modalities": ["text"],
            "permission": []
        },
        {
            "id": "meta-llama/Llama-5-Future-Instruct",
            "name": "Meta: Llama 5 Future Instruct",
            "object": "model",
            "created": 1800000000,
            "owned_by": "io-intelligence",
            "max_model_len": null,
            "max_tokens": null,
            "context_window": 262144,
            "supports_reasoning": true,
            "supports_tools": true,
            "input_modalities": ["text"],
            "output_modalities": ["text"],
            "permission": []
        }
    ]
}"#;

/// One-shot `/models` server that answers its connections in order, each with
/// the given HTTP status and body, and reports every raw request it received.
fn spawn_sequential_models_server(
    responses: Vec<(u16, &'static str)>,
) -> (String, Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake IO Intelligence server");
    let addr = listener.local_addr().expect("fake IO Intelligence addr");
    let (request_tx, request_rx) = mpsc::channel();

    std::thread::spawn(move || {
        for (status, body) in responses {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
            let mut request = vec![0u8; 8192];
            let n = stream.read(&mut request).unwrap_or(0);
            let request = String::from_utf8_lossy(&request[..n]).into_owned();
            let _ = request_tx.send(request);

            let reason = match status {
                200 => "OK",
                500 => "Internal Server Error",
                _ => "Error",
            };
            let response = format!(
                "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(response.as_bytes());
        }
    });

    (format!("http://{addr}/v1"), request_rx)
}

fn ionet_test_provider(api_base: String) -> OpenRouterProvider {
    OpenRouterProvider {
        api_base,
        model: Arc::new(RwLock::new(IONET_DEFAULT_MODEL.to_string())),
        auth: Arc::new(|| {
            Ok(ProviderAuth::AuthorizationBearer {
                token: "test-ionet-catalog-token".to_string(),
                label: "IONET_API_KEY".to_string(),
            })
        }),
        supports_provider_features: false,
        supports_model_catalog: true,
        profile_id: Some(jcode_provider_metadata::IONET_PROFILE.id.to_string()),
        static_models: jcode_base::provider_catalog::openai_compatible_profile_static_models(
            jcode_provider_metadata::IONET_PROFILE,
        ),
        send_openrouter_headers: false,
        conversation_id: new_conversation_id(),
        ..make_custom_compatible_provider()
    }
}

fn block_on<F: std::future::Future>(future: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime")
        .block_on(future)
}

#[test]
fn ionet_future_model_is_absent_from_every_offline_fallback() {
    let static_models = jcode_base::provider_catalog::openai_compatible_profile_static_models(
        jcode_provider_metadata::IONET_PROFILE,
    );
    assert!(
        !static_models
            .iter()
            .any(|model| model == IONET_FUTURE_MODEL),
        "the discovery tests below must use a model no fallback array knows about: {static_models:?}"
    );
    assert_ne!(
        jcode_provider_metadata::IONET_PROFILE.default_model,
        Some(IONET_FUTURE_MODEL)
    );
}

#[test]
fn ionet_catalog_parses_native_response_shape() {
    let models = parse_openai_compatible_models_response(IONET_CATALOG_WITH_FUTURE_MODEL)
        .expect("IO Intelligence catalog response should parse");

    let current = models
        .iter()
        .find(|model| model.id == IONET_DEFAULT_MODEL)
        .expect("current default model should be listed");
    assert_eq!(current.name, "Meta: Llama 3.3 70B Instruct");
    assert_eq!(current.context_length, Some(128_000));
    assert_eq!(current.created, Some(1_733_762_183));

    let future = models
        .iter()
        .find(|model| model.id == IONET_FUTURE_MODEL)
        .expect("a model absent from every fallback array must still be discovered");
    assert_eq!(future.name, "Meta: Llama 5 Future Instruct");
    assert_eq!(
        future.context_length,
        Some(262_144),
        "IO Intelligence reports the live window in `context_window`"
    );
}

#[test]
fn ionet_live_catalog_discovers_future_model_and_explicit_refresh_bypasses_fresh_cache() {
    let _lock = ENV_LOCK.lock();
    let temp = TempDir::new().expect("create temp home");
    let _home = EnvVarGuard::set("HOME", temp.path());
    let _appdata = EnvVarGuard::set("APPDATA", temp.path().join("AppData").join("Roaming"));
    let _namespace = EnvVarGuard::set("JCODE_OPENROUTER_CACHE_NAMESPACE", "test-ionet-discovery");

    let (api_base, request_rx) = spawn_sequential_models_server(vec![
        (200, IONET_CATALOG_CURRENT),
        (200, IONET_CATALOG_WITH_FUTURE_MODEL),
    ]);
    let provider = ionet_test_provider(api_base.clone());

    assert!(
        !provider
            .available_models_display()
            .iter()
            .any(|model| model == IONET_FUTURE_MODEL),
        "the future model must not be known before the live catalog is fetched"
    );

    let fetched = block_on(provider.refresh_models()).expect("first IO Intelligence catalog fetch");
    assert_eq!(fetched.len(), 1);
    assert_eq!(fetched[0].id, IONET_DEFAULT_MODEL);

    let request = request_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("capture first catalog request");
    assert!(
        request.starts_with("GET /v1/models "),
        "unexpected catalog request: {request}"
    );
    assert!(
        request
            .to_ascii_lowercase()
            .contains("authorization: bearer test-ionet-catalog-token"),
        "catalog request should carry the IO Intelligence API key: {request}"
    );

    // The in-memory catalog is now fresh (fetched seconds ago). An explicit
    // refresh must still go to the network and pick up the newly shipped model.
    let refreshed = block_on(provider.refresh_models()).expect("explicit IO Intelligence refresh");
    request_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("explicit refresh must hit /v1/models despite a fresh cache");
    assert!(
        refreshed.iter().any(|model| model.id == IONET_FUTURE_MODEL),
        "explicit refresh should return the newly discovered model: {refreshed:?}"
    );

    let display = provider.available_models_display();
    assert!(display.iter().any(|model| model == IONET_DEFAULT_MODEL));
    assert!(
        display.iter().any(|model| model == IONET_FUTURE_MODEL),
        "discovered model should appear in the switching list: {display:?}"
    );

    let routes = provider.model_routes();
    let route = routes
        .iter()
        .find(|route| route.model == IONET_FUTURE_MODEL)
        .expect("discovered model should have a picker route");
    assert_eq!(route.provider, "IO Intelligence");
    assert_eq!(route.api_method, "openai-compatible:ionet");
    assert!(route.available);
    assert!(
        !route.detail.contains("fallback"),
        "a live catalog model must not be labeled as a static fallback: {route:?}"
    );

    provider
        .set_model(IONET_FUTURE_MODEL)
        .expect("discovered model should be selectable");
    assert_eq!(provider.model(), IONET_FUTURE_MODEL);
    assert_eq!(
        provider.context_window(),
        262_144,
        "context budget should come from the live catalog, not a static default"
    );

    // A new provider instance for the same endpoint reads the persisted catalog
    // without another network round trip.
    let fresh_provider = ionet_test_provider(api_base);
    fresh_provider
        .set_model(IONET_FUTURE_MODEL)
        .expect("discovered model should be selectable from the persisted catalog");
    assert_eq!(fresh_provider.context_window(), 262_144);
    assert!(
        fresh_provider
            .available_models_display()
            .iter()
            .any(|model| model == IONET_FUTURE_MODEL),
        "persisted catalog should expose the discovered model to a fresh instance"
    );
    assert!(
        request_rx.recv_timeout(Duration::from_millis(300)).is_err(),
        "reading the persisted catalog must not issue another /v1/models request"
    );
}

#[test]
fn ionet_failed_refresh_preserves_last_good_catalog() {
    let _lock = ENV_LOCK.lock();
    let temp = TempDir::new().expect("create temp home");
    let _home = EnvVarGuard::set("HOME", temp.path());
    let _appdata = EnvVarGuard::set("APPDATA", temp.path().join("AppData").join("Roaming"));
    let _namespace = EnvVarGuard::set("JCODE_OPENROUTER_CACHE_NAMESPACE", "test-ionet-last-good");

    let (api_base, request_rx) = spawn_sequential_models_server(vec![
        (200, IONET_CATALOG_WITH_FUTURE_MODEL),
        (500, r#"{"error":{"message":"upstream unavailable"}}"#),
        (200, "<html>maintenance page, not a catalog</html>"),
    ]);
    let provider = ionet_test_provider(api_base);

    block_on(provider.refresh_models()).expect("initial IO Intelligence catalog fetch");
    request_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("capture initial catalog request");
    let good_display = provider.available_models_display();
    assert!(good_display.iter().any(|model| model == IONET_FUTURE_MODEL));
    provider
        .set_model(IONET_FUTURE_MODEL)
        .expect("discovered model should be selectable");
    assert_eq!(provider.context_window(), 262_144);

    let http_error = block_on(provider.refresh_models());
    request_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("capture failing catalog request");
    assert!(
        http_error.is_err(),
        "an HTTP 500 from /v1/models must surface as a refresh error"
    );
    assert_eq!(
        provider.available_models_display(),
        good_display,
        "an HTTP failure must not drop the last good catalog"
    );
    assert_eq!(provider.context_window(), 262_144);

    let malformed = block_on(provider.refresh_models());
    request_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("capture malformed-response catalog request");
    assert!(
        malformed.is_err(),
        "a non-JSON body from /v1/models must surface as a refresh error"
    );
    assert_eq!(
        provider.available_models_display(),
        good_display,
        "a malformed response must not drop the last good catalog"
    );
    assert!(
        provider
            .model_routes()
            .iter()
            .any(|route| route.model == IONET_FUTURE_MODEL && route.available),
        "picker routes should keep serving the last good catalog after failures"
    );
}
