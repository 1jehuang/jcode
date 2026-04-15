use super::*;

#[derive(Debug, Clone, Default)]
pub struct OpenAIModelCatalog {
    pub available_models: Vec<String>,
    pub context_limits: HashMap<String, usize>,
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
            limits.insert(slug, ctx as usize);
        }
    }

    let mut available_models: Vec<String> = available.into_iter().collect();
    available_models.sort();

    OpenAIModelCatalog {
        available_models,
        context_limits: limits,
    }
}

/// Fetch model availability and context windows from the Codex backend API.
pub async fn fetch_openai_model_catalog(access_token: &str) -> Result<OpenAIModelCatalog> {
    note_openai_model_catalog_refresh_attempt();

    let client = shared_http_client();
    let resp = client
        .get("https://chatgpt.com/backend-api/codex/models?client_version=1.0.0")
        .header("Authorization", format!("Bearer {}", access_token))
        .send()
        .await?;

    if !resp.status().is_success() {
        anyhow::bail!("Failed to fetch model context limits: {}", resp.status());
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
        let mut req = client
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
            .query(&[("limit", "1000")]);

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
        let resp = build_request(&client, after_id.as_deref()).send().await?;
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

/// Fetch context window sizes from the Codex backend API.
/// Returns a map of model slug -> context_window tokens.
pub async fn fetch_openai_context_limits(access_token: &str) -> Result<HashMap<String, usize>> {
    Ok(fetch_openai_model_catalog(access_token)
        .await?
        .context_limits)
}
