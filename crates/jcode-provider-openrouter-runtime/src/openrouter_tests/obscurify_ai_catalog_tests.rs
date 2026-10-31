//! Obscurify.ai catalog behaviour: discovery, forced refresh, last-good
//! preservation and endpoint scoping, against the shape of its `/v1/models`.
use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

const PROBE_FIRST: &str = "futurelab/catalog-probe-first";
const PROBE_SECOND: &str = "futurelab/catalog-probe-second";
const PROBE_OTHER_ENDPOINT: &str = "futurelab/catalog-probe-other-endpoint";
const OK: &str = "200 OK";

fn catalog_body(model: &str) -> String {
    serde_json::json!({
        "object": "list",
        "data": [{
            "id": model,
            "object": "model",
            "owned_by": "futurelab",
            "name": "Catalog probe",
            "pricing": {
                "prompt": "0.0000001",
                "completion": "0.0000002",
                "image": "0",
                "request": "0"
            },
            "context_length": 524_288,
            "decisions": false,
            "tools": true
        }]
    })
    .to_string()
}

/// Serves one scripted response per connection, then closes the listener so
/// any further request fails at the network layer.
fn spawn_scripted_models_server(
    responses: Vec<(&'static str, String)>,
) -> (String, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake catalog server");
    let addr = listener.local_addr().expect("fake catalog addr");
    let requests = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&requests);

    std::thread::spawn(move || {
        for (status, body) in responses {
            let (mut stream, _) = listener.accept().expect("accept catalog request");
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .expect("set read timeout");
            let mut request = vec![0u8; 8192];
            let _ = stream.read(&mut request);
            counter.fetch_add(1, Ordering::SeqCst);
            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream
                .write_all(response.as_bytes())
                .expect("write catalog response");
        }
    });

    (format!("http://{addr}/v1"), requests)
}

fn obscurify_ai_provider(api_base: &str) -> OpenRouterProvider {
    let profile = jcode_base::provider_catalog::resolve_openai_compatible_profile(
        jcode_base::provider_catalog::OBSCURIFY_AI_PROFILE,
    );
    OpenRouterProvider {
        api_base: api_base.to_string(),
        model: Arc::new(RwLock::new(
            profile.default_model.expect("profile default model"),
        )),
        profile_id: Some(profile.id),
        conversation_id: new_conversation_id(),
        ..make_custom_compatible_provider()
    }
}

fn disk_model_ids() -> Vec<String> {
    jcode_provider_openrouter::load_disk_cache_entry_for_namespace("obscurify-ai")
        .map(|cache| cache.models.into_iter().map(|model| model.id).collect())
        .unwrap_or_default()
}

#[test]
fn obscurify_ai_catalog_refreshes_past_fresh_cache_and_keeps_last_good_per_endpoint() {
    let _lock = ENV_LOCK.lock();
    let temp = TempDir::new().expect("create temp home");
    let _home = EnvVarGuard::set("JCODE_HOME", temp.path());
    let _namespace = EnvVarGuard::set("JCODE_OPENROUTER_CACHE_NAMESPACE", "obscurify-ai");
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");

    let (api_base, requests) = spawn_scripted_models_server(vec![
        (OK, catalog_body(PROBE_FIRST)),
        (OK, catalog_body(PROBE_SECOND)),
        (
            "500 Internal Server Error",
            r#"{"error":"down"}"#.to_string(),
        ),
        ("401 Unauthorized", r#"{"error":"bad key"}"#.to_string()),
        (OK, "<html>not a catalog</html>".to_string()),
    ]);
    let provider = obscurify_ai_provider(&api_base);

    // Live discovery: a model absent from every fallback list becomes selectable.
    let discovered = rt
        .block_on(provider.refresh_models())
        .expect("discover live catalog");
    assert_eq!(discovered[0].id, PROBE_FIRST);
    assert_eq!(discovered[0].context_length, Some(524_288));
    assert!(
        provider
            .available_models_display()
            .iter()
            .any(|model| model == PROBE_FIRST)
    );
    assert_eq!(disk_model_ids(), [PROBE_FIRST]);
    assert_eq!(requests.load(Ordering::SeqCst), 1);

    // A new instance on the same endpoint is served from the fresh disk cache.
    let restarted = obscurify_ai_provider(&api_base);
    let cached = rt
        .block_on(restarted.fetch_models())
        .expect("read fresh disk cache");
    assert_eq!(cached[0].id, PROBE_FIRST);
    assert_eq!(
        requests.load(Ordering::SeqCst),
        1,
        "fresh cache needs no request"
    );

    // Explicit refresh bypasses that fresh cache.
    let refreshed = rt
        .block_on(restarted.refresh_models())
        .expect("forced refresh");
    assert_eq!(refreshed[0].id, PROBE_SECOND);
    assert_eq!(
        requests.load(Ordering::SeqCst),
        2,
        "refresh must hit the network"
    );
    assert_eq!(disk_model_ids(), [PROBE_SECOND]);

    // HTTP, auth, malformed-body and network failures all keep the last good catalog.
    for (failure, expected_requests) in [
        ("http 500", 3),
        ("http 401", 4),
        ("malformed body", 5),
        ("network", 5),
    ] {
        assert!(
            rt.block_on(restarted.refresh_models()).is_err(),
            "{failure} must surface as a refresh error"
        );
        assert_eq!(
            requests.load(Ordering::SeqCst),
            expected_requests,
            "{failure}"
        );
        assert_eq!(
            disk_model_ids(),
            [PROBE_SECOND],
            "{failure} changed the disk cache"
        );
        let served = rt
            .block_on(restarted.fetch_models())
            .expect("serve last good catalog");
        assert_eq!(
            served[0].id, PROBE_SECOND,
            "{failure} changed the served catalog"
        );
        assert!(
            restarted
                .available_models_display()
                .iter()
                .any(|model| model == PROBE_SECOND),
            "{failure} removed the model from the picker"
        );
    }

    // The same cache namespace on another endpoint must not reuse this catalog.
    let (other_base, other_requests) =
        spawn_scripted_models_server(vec![(OK, catalog_body(PROBE_OTHER_ENDPOINT))]);
    let other = obscurify_ai_provider(&other_base);
    assert!(other.load_usable_model_disk_cache_entry().is_none());
    let other_models = rt
        .block_on(other.fetch_models())
        .expect("fetch other endpoint catalog");
    assert_eq!(other_models[0].id, PROBE_OTHER_ENDPOINT);
    assert_eq!(other_requests.load(Ordering::SeqCst), 1);
}
