use super::*;

/// Total per-request timeout for model catalog fetches. The shared HTTP
/// client only sets a connect timeout, so without this a hung catalog request
/// keeps the scope's refresh marked in-flight and the picker stays stale.
const CATALOG_REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// HTTP status carried inside model catalog fetch errors so callers can
/// distinguish auth rejections (401/403) from transient failures and recover
/// by force-refreshing OAuth tokens, mirroring the chat request path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelCatalogHttpStatus(pub u16);

impl std::fmt::Display for ModelCatalogHttpStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "HTTP status {}", self.0)
    }
}

impl std::error::Error for ModelCatalogHttpStatus {}

fn catalog_status_error(status: reqwest::StatusCode, context: String) -> anyhow::Error {
    anyhow::Error::new(ModelCatalogHttpStatus(status.as_u16())).context(context)
}

#[derive(Debug, Clone, Default)]
pub struct OpenAIModelCatalog {
    pub available_models: Vec<String>,
    pub context_limits: HashMap<String, usize>,
    /// Ordered reasoning-effort values advertised by each Codex model.
    pub reasoning_efforts: HashMap<String, Vec<String>>,
}

#[derive(Debug, Clone, Default)]
pub struct AnthropicModelCatalog {
    pub available_models: Vec<String>,
    pub context_limits: HashMap<String, usize>,
}

pub(crate) fn parse_anthropic_model_catalog(data: &serde_json::Value) -> AnthropicModelCatalog {
    let models = data
        .get("data")
        .and_then(|value| value.as_array())
        .or_else(|| data.as_array());

    let mut available: HashSet<String> = HashSet::new();
    let mut limits: HashMap<String, usize> = HashMap::new();

    for model in models.into_iter().flatten() {
        let Some(id) = model.get("id").and_then(|value| value.as_str()) else {
            continue;
        };

        let normalized = normalize_model_id(id);
        if normalized.is_empty() {
            continue;
        }

        available.insert(normalized.clone());

        if let Some(limit) = model
            .get("max_input_tokens")
            .and_then(|value| value.as_u64())
        {
            limits.insert(normalized, limit as usize);
        }
    }

    let mut available_models: Vec<String> = available.into_iter().collect();
    available_models.sort();

    AnthropicModelCatalog {
        available_models,
        context_limits: limits,
    }
}

pub(crate) fn parse_openai_model_catalog(data: &serde_json::Value) -> OpenAIModelCatalog {
    let models = data
        .get("models")
        .and_then(|m| m.as_array())
        .or_else(|| {
            data.get("data")
                .and_then(|d| d.get("models"))
                .and_then(|m| m.as_array())
        })
        .or_else(|| data.get("data").and_then(|d| d.as_array()))
        .or_else(|| data.as_array());

    let mut available: HashSet<String> = HashSet::new();
    let mut limits: HashMap<String, usize> = HashMap::new();
    let mut reasoning_efforts: HashMap<String, Vec<String>> = HashMap::new();

    for model in models.into_iter().flatten() {
        let Some(slug) = model
            .get("slug")
            .or_else(|| model.get("id"))
            .or_else(|| model.get("model"))
            .and_then(|s| s.as_str())
        else {
            continue;
        };

        let slug = normalize_model_id(slug);
        if slug.is_empty() {
            continue;
        }

        available.insert(slug.clone());

        if let Some(ctx) = model
            .get("context_window")
            .or_else(|| model.get("context_length"))
            .and_then(|c| c.as_u64())
        {
            limits.insert(slug.clone(), ctx as usize);
        }

        if let Some(values) = model
            .get("supported_reasoning_levels")
            .or_else(|| model.get("supported_reasoning_efforts"))
            .and_then(|value| value.as_array())
        {
            let mut efforts = Vec::new();
            for value in values {
                let raw = value.as_str().or_else(|| {
                    value
                        .get("reasoning_effort")
                        .or_else(|| value.get("effort"))
                        .or_else(|| value.get("value"))
                        .and_then(|value| value.as_str())
                });
                let Some(effort) = raw.and_then(jcode_provider_core::canonical_reasoning_effort)
                else {
                    continue;
                };
                if !efforts.iter().any(|existing| existing == effort) {
                    efforts.push(effort.to_string());
                }
            }
            if !efforts.is_empty() {
                reasoning_efforts.insert(slug, efforts);
            }
        }
    }

    let mut available_models: Vec<String> = available.into_iter().collect();
    available_models.sort();

    OpenAIModelCatalog {
        available_models,
        context_limits: limits,
        reasoning_efforts,
    }
}

/// Fetch model availability and context windows from the Codex backend API.
pub async fn fetch_openai_model_catalog(access_token: &str) -> Result<OpenAIModelCatalog> {
    note_openai_model_catalog_refresh_attempt();

    let client = shared_http_client();
    let resp = client
        .get("https://chatgpt.com/backend-api/codex/models?client_version=1.0.0")
        .header("Authorization", format!("Bearer {}", access_token))
        .timeout(CATALOG_REQUEST_TIMEOUT)
        .send()
        .await?;

    if !resp.status().is_success() {
        return Err(catalog_status_error(
            resp.status(),
            format!("Failed to fetch model context limits: {}", resp.status()),
        ));
    }

    let data: serde_json::Value = resp.json().await?;
    Ok(parse_openai_model_catalog(&data))
}

pub async fn fetch_anthropic_model_catalog(api_key: &str) -> Result<AnthropicModelCatalog> {
    fetch_anthropic_model_catalog_with_request(|client, after_id| {
        let mut req = client
            .get("https://api.anthropic.com/v1/models")
            .header("x-api-key", api_key)
            .header("anthropic-version", "2023-06-01")
            .query(&[("limit", "1000")]);

        if let Some(after) = after_id {
            req = req.query(&[("after_id", after)]);
        }

        req
    })
    .await
}

pub async fn fetch_anthropic_model_catalog_oauth(
    access_token: &str,
) -> Result<AnthropicModelCatalog> {
    fetch_anthropic_model_catalog_with_request(|client, after_id| {
        let mut req = crate::provider::anthropic::apply_oauth_attribution_headers(
            client
                .get("https://api.anthropic.com/v1/models")
                .header("Authorization", format!("Bearer {}", access_token))
                .header(
                    "User-Agent",
                    crate::provider::anthropic::CLAUDE_CLI_USER_AGENT,
                )
                .header("anthropic-version", "2023-06-01")
                .header(
                    "anthropic-beta",
                    crate::provider::anthropic::OAUTH_BETA_HEADERS,
                )
                .query(&[("limit", "1000")]),
            &crate::provider::anthropic::new_oauth_request_id(),
        );

        if let Some(after) = after_id {
            req = req.query(&[("after_id", after)]);
        }

        req
    })
    .await
}

async fn fetch_anthropic_model_catalog_with_request<F>(
    mut build_request: F,
) -> Result<AnthropicModelCatalog>
where
    F: FnMut(&reqwest::Client, Option<&str>) -> reqwest::RequestBuilder,
{
    let client = shared_http_client();
    let mut available = HashSet::new();
    let mut limits = HashMap::new();
    let mut after_id: Option<String> = None;

    loop {
        let resp = build_request(&client, after_id.as_deref())
            .timeout(CATALOG_REQUEST_TIMEOUT)
            .send()
            .await?;
        if !resp.status().is_success() {
            anyhow::bail!("Failed to fetch Anthropic model catalog: {}", resp.status());
        }

        let data: serde_json::Value = resp.json().await?;
        let page = parse_anthropic_model_catalog(&data);
        available.extend(page.available_models);
        limits.extend(page.context_limits);

        let has_more = data
            .get("has_more")
            .and_then(|value| value.as_bool())
            .unwrap_or(false);
        if !has_more {
            break;
        }

        let Some(next_after) = data
            .get("last_id")
            .and_then(|value| value.as_str())
            .map(|value| value.to_string())
        else {
            break;
        };

        after_id = Some(next_after);
    }

    let mut available_models: Vec<String> = available.into_iter().collect();
    available_models.sort();
    Ok(AnthropicModelCatalog {
        available_models,
        context_limits: limits,
    })
}

/// Fetch model availability from the OpenAI platform API using an API key.
///
/// The ChatGPT/Codex backend catalog endpoint only accepts ChatGPT OAuth
/// bearer tokens. OpenAI platform API keys return 401 there, so API-key
/// sessions must use the public platform models endpoint. That endpoint does
/// not currently expose context windows, so callers keep any built-in/cached
/// limits and only update account model availability.
pub async fn fetch_openai_api_key_model_catalog(api_key: &str) -> Result<OpenAIModelCatalog> {
    note_openai_model_catalog_refresh_attempt();

    let client = shared_http_client();
    // Honor the same API-base override as the Responses request path so a
    // custom/proxied endpoint is probed for models instead of the real
    // api.openai.com (issue #343).
    let models_url = format!(
        "{}/models",
        crate::provider::openai::resolve_api_base().trim_end_matches('/')
    );
    let resp = client
        .get(&models_url)
        .header("Authorization", format!("Bearer {}", api_key))
        .timeout(CATALOG_REQUEST_TIMEOUT)
        .send()
        .await?;

    if !resp.status().is_success() {
        return Err(catalog_status_error(
            resp.status(),
            format!(
                "Failed to fetch OpenAI platform model catalog: {}",
                resp.status()
            ),
        ));
    }

    let data: serde_json::Value = resp.json().await?;
    let mut available_models: Vec<String> = data
        .get("data")
        .and_then(|value| value.as_array())
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.get("id").and_then(|id| id.as_str()))
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(ToString::to_string)
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    available_models.sort();

    Ok(OpenAIModelCatalog {
        available_models,
        context_limits: HashMap::new(),
        reasoning_efforts: HashMap::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn openai_catalog_parses_string_and_object_reasoning_efforts() {
        let catalog = parse_openai_model_catalog(&serde_json::json!({
            "models": [
                {
                    "slug": "gpt-5.6",
                    "supported_reasoning_levels": [
                        { "reasoning_effort": "minimal" },
                        { "reasoning_effort": "high" },
                        { "reasoning_effort": "max" }
                    ]
                },
                {
                    "slug": "gpt-5.4",
                    "supported_reasoning_efforts": ["none", "low", "xhigh"]
                }
            ]
        }));

        assert_eq!(
            catalog.reasoning_efforts.get("gpt-5.6"),
            Some(&vec![
                "minimal".to_string(),
                "high".to_string(),
                "max".to_string()
            ])
        );
        assert_eq!(
            catalog.reasoning_efforts.get("gpt-5.4"),
            Some(&vec![
                "none".to_string(),
                "low".to_string(),
                "xhigh".to_string()
            ])
        );
    }
}

/// Fetch context window sizes from the Codex backend API.
/// Returns a map of model slug -> context_window tokens.
pub async fn fetch_openai_context_limits(access_token: &str) -> Result<HashMap<String, usize>> {
    Ok(fetch_openai_model_catalog(access_token)
        .await?
        .context_limits)
}

#[cfg(test)]
mod status_error_tests {
    use super::*;

    #[test]
    fn catalog_status_error_round_trips_through_anyhow_downcast() {
        let err = catalog_status_error(
            reqwest::StatusCode::UNAUTHORIZED,
            "Failed to fetch model context limits: 401 Unauthorized".to_string(),
        );
        let status = err
            .downcast_ref::<ModelCatalogHttpStatus>()
            .expect("status must survive the context wrapper");
        assert_eq!(status.0, 401);
        // The human-readable context must stay the outermost message.
        assert!(
            err.to_string()
                .contains("Failed to fetch model context limits")
        );
    }
}
