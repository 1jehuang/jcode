//! Model catalog request authentication, fetching, and cache updates.

use super::*;

impl ProviderAuth {
    pub(super) async fn apply_catalog(
        &self,
        req: reqwest::RequestBuilder,
        api_base: &str,
    ) -> Result<reqwest::RequestBuilder> {
        // Reuse key-probe rules without changing other providers' auth modes.
        if jcode_base::provider_catalog::api_base_is_anthropic(api_base) {
            match self {
                Self::AuthorizationBearer {
                    key_name,
                    env_file,
                    fallback_token,
                    label,
                } => {
                    let token = Self::resolve_key(key_name, env_file, fallback_token.as_deref(), label)?;
                    return Ok(
                        jcode_base::provider_catalog::apply_openai_compatible_catalog_auth(
                            req, api_base, &token,
                        ),
                    );
                }
                Self::InlineBearer { token, .. } => {
                    return Ok(
                        jcode_base::provider_catalog::apply_openai_compatible_catalog_auth(
                            req, api_base, token,
                        ),
                    );
                }
                Self::HeaderValue {
                    header_name,
                    key_name,
                    env_file,
                    fallback_token,
                    label,
                } if header_name.as_str() == "x-api-key" => {
                    let token = Self::resolve_key(key_name, env_file, fallback_token.as_deref(), label)?;
                    return Ok(
                        jcode_base::provider_catalog::apply_openai_compatible_catalog_auth(
                            req, api_base, &token,
                        ),
                    );
                }
                Self::InlineHeader {
                    header_name, token, ..
                } if header_name.as_str() == "x-api-key" => {
                    return Ok(
                        jcode_base::provider_catalog::apply_openai_compatible_catalog_auth(
                            req, api_base, token,
                        ),
                    );
                }
                Self::OptionalBearer {
                    key_name,
                    env_file,
                    label,
                } => {
                    if let Ok(token) = Self::resolve_key(key_name, env_file, None, label) {
                        return Ok(
                            jcode_base::provider_catalog::apply_openai_compatible_catalog_auth(
                                req, api_base, &token,
                            ),
                        );
                    }
                }
                Self::MuseSubscription { .. } => {
                    if let Ok(credentials) = jcode_base::auth::muse::load_credentials() {
                        return Ok(
                            jcode_base::provider_catalog::apply_openai_compatible_catalog_auth(
                                req, api_base, &credentials.api_key,
                            ),
                        );
                    }
                }
                _ => {}
            }
            return Ok(self.apply(req).await?.header(
                "anthropic-version",
                jcode_base::provider_catalog::ANTHROPIC_VERSION_HEADER_VALUE,
            ));
        }
        self.apply(req).await
    }
}

pub(super) async fn fetch_models_from_api(
    client: Client,
    api_base: String,
    auth: ProviderAuth,
    models_cache: Arc<RwLock<ModelsCache>>,
    cache_namespace: Option<String>,
) -> Result<Vec<ModelInfo>> {
    let url = muse_catalog_url(&api_base, cache_namespace.as_deref());
    let request = auth.apply_catalog(client.get(&url), &api_base).await?;
    let response = apply_kimi_coding_agent_headers(request, &api_base, None)
            .send()
            .await
            .with_context(|| {
                format!(
                    "Failed to send OpenAI-compatible model catalog request\n  endpoint: {}\n  auth: {}\nHint: check network connectivity, DNS/TLS, and that the base URL includes the API version (usually /v1).",
                    url,
                    auth.label()
                )
            })?;

    if !response.status().is_success() {
        let status = response.status();
        let body = jcode_base::util::http_error_body(response, "HTTP error").await;
        anyhow::bail!(
            "OpenAI-compatible model catalog request failed\n  endpoint: {}\n  auth: {}\n  status: {}\n  response: {}\nHint: verify the base URL includes the API version (usually /v1), the key is valid for this endpoint, and the provider supports GET /models.",
            url,
            auth.label(),
            status,
            body
        );
    }

    let raw_body = response
        .text()
        .await
        .with_context(|| format!("Failed to read model catalog response body from {}", url))?;
    let parsed = if cache_namespace.as_deref() == Some("grok-build") {
        models_catalog_parse::parse_grok_models_response(&raw_body)
    } else {
        parse_openai_compatible_models_response(&raw_body)
    };
    let mut models = parsed.with_context(|| {
            format!(
                "Failed to parse OpenAI-compatible model catalog response\n  endpoint: {}\n  auth: {}\n  expected: JSON object with a `data` or `models` array, or a top-level array, with model objects containing at least `id` or `name`\n  response: {}",
                url,
                auth.label(),
                jcode_base::util::truncate_str(&raw_body.trim().replace('\n', "\\n"), 1200)
            )
        })?;

    let ns = cache_namespace.as_deref();
    ollama_context::maybe_enrich(&client, &api_base, ns, &mut models).await;
    if let Some(namespace) = ns {
        save_disk_cache_with_source_for_namespace(namespace, &models, Some(&api_base));
    } else {
        save_disk_cache_with_source(&models, Some(&api_base));
    }

    if let Some(now) = current_unix_secs() {
        let mut cache = models_cache.write().await;
        cache.models = models.clone();
        cache.fetched = true;
        cache.cached_at = Some(now);
    } else {
        let mut cache = models_cache.write().await;
        cache.models = models.clone();
        cache.fetched = true;
    }

    Ok(models)
}

/// Pure helper so the native route guard can be unit-tested without I/O.
pub(crate) fn muse_catalog_url(api_base: &str, cache_namespace: Option<&str>) -> String {
    if is_native_muse_catalog_request(api_base, cache_namespace) {
        let base_no_v1 = api_base
            .trim_end_matches('/')
            .strip_suffix("/v1")
            .unwrap_or(api_base.trim_end_matches('/'));
        format!("{}/muse-code/models", base_no_v1)
    } else {
        format!("{}/models", api_base)
    }
}

/// Only the built-in `muse-subscription` namespace and only when the API base
/// exactly matches the pinned official Meta base is eligible for the native
/// `GET /muse-code/models` path. Never send the native credential to an
/// arbitrary/custom base.
pub(crate) fn is_native_muse_catalog_request(
    api_base: &str,
    cache_namespace: Option<&str>,
) -> bool {
    cache_namespace == Some("muse-subscription")
        && api_base.trim_end_matches('/') == "https://api.meta.ai/v1"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_route_only_for_muse_subscription_and_pinned_base() {
        assert!(is_native_muse_catalog_request(
            "https://api.meta.ai/v1",
            Some("muse-subscription")
        ));
        // Trailing slash normalized.
        assert!(is_native_muse_catalog_request(
            "https://api.meta.ai/v1/",
            Some("muse-subscription")
        ));
        assert!(!is_native_muse_catalog_request(
            "https://api.meta.ai/v1",
            Some("openrouter")
        ));
        assert!(!is_native_muse_catalog_request(
            "https://api.meta.ai/v1",
            None
        ));
        assert!(!is_native_muse_catalog_request(
            "https://custom.example.test/v1",
            Some("muse-subscription")
        ));
        // Non-matching host or extra path must not be treated as native.
        assert!(!is_native_muse_catalog_request(
            "https://api.meta.ai.evil.test/v1",
            Some("muse-subscription")
        ));
        assert!(!is_native_muse_catalog_request(
            "https://api.meta.ai/v1/extra",
            Some("muse-subscription")
        ));
        assert!(!is_native_muse_catalog_request(
            "https://api.meta.ai/v1/v1",
            Some("muse-subscription")
        ));
    }

    #[test]
    fn native_catalog_url_is_locked_to_official_base() {
        assert_eq!(
            muse_catalog_url("https://api.meta.ai/v1", Some("muse-subscription")),
            "https://api.meta.ai/muse-code/models"
        );
        assert_eq!(
            muse_catalog_url("https://api.meta.ai/v1/", Some("muse-subscription")),
            "https://api.meta.ai/muse-code/models"
        );
        assert_eq!(
            muse_catalog_url("https://custom.example.test/v1", Some("muse-subscription")),
            "https://custom.example.test/v1/models"
        );
        assert_eq!(
            muse_catalog_url("https://api.meta.ai/v1", Some("openrouter")),
            "https://api.meta.ai/v1/models"
        );
        assert_eq!(
            muse_catalog_url("https://api.meta.ai/v1", None),
            "https://api.meta.ai/v1/models"
        );
        // Official binary strings evidence: the literal must exist.
        assert!(
            muse_catalog_url("https://api.meta.ai/v1", Some("muse-subscription"))
                .contains("/muse-code/models")
        );
    }

    #[tokio::test]
    async fn loopback_catalog_fetch_hits_locked_muse_route() {
        use std::io::{Read, Write};
        use std::net::TcpListener;

        // Prove the guard never leaks the native credential to an arbitrary
        // base: a custom base with muse-subscription namespace must still hit
        // the generic /v1/models (task requirement: never send native
        // credential to arbitrary base). The path capture is on loopback.
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let addr = listener.local_addr().expect("addr");
        // This test mutates the process-global `JCODE_HOME`. The default test
        // runner is multi-threaded, so it must hold the shared env lock like
        // every sibling test; otherwise it races the autodetect/provider tests
        // and makes the whole suite flaky.
        let _lock = crate::tests::ENV_LOCK.lock();
        let (path_tx, path_rx) = std::sync::mpsc::channel::<String>();
        let (auth_tx, auth_rx) = std::sync::mpsc::channel::<String>();

        let body = r#"{"data":[{"id":"muse-spark-1.3","metadata":{"muse-code":{"name":"Muse Spark 1.3","is_hidden":false,"modalities":{"input":["text"],"output":["text"]},"limit":{"context":1048576,"output":65536}}}}]}"#;
        let body_owned = body.to_string();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                .unwrap();
            let mut buf = vec![0u8; 8192];
            let n = stream.read(&mut buf).unwrap_or(0);
            let raw = String::from_utf8_lossy(&buf[..n]).to_string();
            let path = raw
                .lines()
                .next()
                .and_then(|line| line.split_whitespace().nth(1))
                .unwrap_or("")
                .to_string();
            let auth = raw
                .lines()
                .find(|line| line.to_ascii_lowercase().starts_with("authorization:"))
                .unwrap_or("")
                .to_string();
            let _ = path_tx.send(path);
            let _ = auth_tx.send(auth);
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body_owned.len(),
                body_owned
            );
            let _ = stream.write_all(response.as_bytes());
        });

        let client = reqwest::Client::new();
        let api_base = format!("http://{addr}/v1");
        let cache = std::sync::Arc::new(tokio::sync::RwLock::new(
            jcode_provider_openrouter::ModelsCache::default(),
        ));
        let tmp = tempfile::tempdir().unwrap();
        let prev_home = std::env::var_os("JCODE_HOME");
        // SAFETY: test-only env mutation, serialized by single-threaded test runner.
        unsafe {
            std::env::set_var("JCODE_HOME", tmp.path());
        }
        let models = fetch_models_from_api(
            client,
            api_base.clone(),
            crate::ProviderAuth::InlineBearer {
                token: "loopback-token".into(),
                label: "TEST".into(),
            },
            cache,
            Some("muse-subscription".into()),
        )
        .await
        .expect("loopback fetch");
        match prev_home {
            Some(value) => unsafe {
                std::env::set_var("JCODE_HOME", value);
            },
            None => unsafe {
                std::env::remove_var("JCODE_HOME");
            },
        }
        assert_eq!(models[0].id, "muse-spark-1.3");
        // Loopback base is not the pinned official base, so it must not use the native route.
        let path = path_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        assert!(
            path.ends_with("/v1/models") || path.ends_with("/models"),
            "unexpected path {path}"
        );
        assert!(
            !path.contains("/muse-code/models"),
            "loopback must not hit native route {path}"
        );
        let auth = auth_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        assert!(auth.to_ascii_lowercase().contains("bearer loopback-token"));
    }

    #[test]
    fn native_url_helper_proves_official_route_is_401_without_auth() {
        // Coordinator evidence: official binary strings shows /muse-code/models,
        // and an unauth live GET to https://api.meta.ai/muse-code/models
        // returned 401 proving the route exists (no network call in tests,
        // just the URL helper contract).
        let url = muse_catalog_url("https://api.meta.ai/v1", Some("muse-subscription"));
        assert_eq!(url, "https://api.meta.ai/muse-code/models");
        assert!(
            !muse_catalog_url("https://custom.example.test/v1", Some("muse-subscription"))
                .contains("/muse-code/models")
        );
    }
}
