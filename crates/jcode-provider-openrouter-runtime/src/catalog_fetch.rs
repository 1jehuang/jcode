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
                Self::AuthorizationBearer { token, .. } => {
                    return Ok(
                        jcode_base::provider_catalog::apply_openai_compatible_catalog_auth(
                            req, api_base, token,
                        ),
                    );
                }
                Self::HeaderValue {
                    header_name, value, ..
                } if header_name.as_str() == "x-api-key" => {
                    return Ok(
                        jcode_base::provider_catalog::apply_openai_compatible_catalog_auth(
                            req, api_base, value,
                        ),
                    );
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
    // Native Muse subscription lives on the official api.meta.ai base but its
    // catalog is GET /muse-code/models (not /v1/models). Only this namespace
    // and only when the api base matches the pinned official base is eligible,
    // so no other profile infers a non-/models entitlement from catalog alone.
    let url = if cache_namespace.as_deref() == Some("muse-subscription")
        && api_base.trim_end_matches('/') == "https://api.meta.ai/v1"
    {
        // Drop the trailing /v1: https://api.meta.ai + /muse-code/models
        let base_no_v1 = api_base
            .trim_end_matches('/')
            .strip_suffix("/v1")
            .unwrap_or(api_base.trim_end_matches('/'));
        format!("{}/muse-code/models", base_no_v1)
    } else {
        format!("{}/models", api_base)
    };
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
    let mut models = parse_openai_compatible_models_response(&raw_body).with_context(|| {
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
