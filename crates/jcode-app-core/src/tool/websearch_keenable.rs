//! Keenable search API. Works without a key on the public tier; an optional
//! key from the configured environment variable switches to the keyed endpoint.
//! Keep credentials out of URLs and diagnostics.
use super::SearchResult;
use anyhow::{Result, anyhow, ensure};
use serde_json::{Value, json};

const PUBLIC_ENDPOINT: &str = "https://api.keenable.ai/v1/search/public";
const KEYED_ENDPOINT: &str = "https://api.keenable.ai/v1/search";
const APP_TITLE: &str = "jcode";
const MAX_RESULTS: usize = 20;

fn request(
    client: &reqwest::Client,
    endpoints: (&str, &str),
    query: &str,
    count: usize,
    key_env: &str,
    env: impl FnOnce(&str) -> Option<String>,
) -> Result<reqwest::Request> {
    let key = if key_env.trim().is_empty() {
        None
    } else {
        env(key_env).filter(|key| !key.trim().is_empty())
    };
    let (public, keyed) = endpoints;
    let builder = match key {
        Some(key) => {
            let mut value = reqwest::header::HeaderValue::from_str(key.trim())
                .map_err(|_| anyhow!("Keenable search API key is not a valid HTTP header value"))?;
            value.set_sensitive(true);
            client.post(keyed).header("X-API-Key", value)
        }
        None => client.post(public).header("X-Keenable-Title", APP_TITLE),
    };
    builder
        .header(reqwest::header::ACCEPT, "application/json")
        .json(&json!({"query": query, "max_results": count.clamp(1, MAX_RESULTS)}))
        .build()
        .map_err(|_| anyhow!("Keenable search request construction failed"))
}

pub(super) async fn search(
    client: &reqwest::Client,
    query: &str,
    count: usize,
    key_env: &str,
) -> Result<Vec<SearchResult>> {
    if count == 0 {
        return Ok(Vec::new());
    }
    let request = request(
        client,
        (PUBLIC_ENDPOINT, KEYED_ENDPOINT),
        query,
        count,
        key_env,
        |name| std::env::var(name).ok(),
    )?;
    execute(client, request, count).await
}

async fn execute(
    client: &reqwest::Client,
    request: reqwest::Request,
    count: usize,
) -> Result<Vec<SearchResult>> {
    let response = client
        .execute(request)
        .await
        .map_err(|_| anyhow!("Keenable search request failed"))?;
    let status = response.status();
    ensure!(
        status != reqwest::StatusCode::TOO_MANY_REQUESTS,
        "Keenable search is rate limited (HTTP 429); set an API key in websearch.keenable_api_key_env for higher limits"
    );
    ensure!(
        status.is_success(),
        "Keenable search failed with HTTP status {status}"
    );
    let body: Value = response
        .json()
        .await
        .map_err(|_| anyhow!("Keenable search response is not valid JSON"))?;
    parse_results(&body, count)
}

fn parse_results(body: &Value, count: usize) -> Result<Vec<SearchResult>> {
    let rows = body
        .get("results")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("Keenable search response is missing results"))?;
    let text = |row: &Value, key: &str| {
        row.get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    };
    Ok(rows
        .iter()
        .filter_map(|row| {
            let url = text(row, "url")?;
            if !(url.starts_with("https://") || url.starts_with("http://")) {
                return None;
            }
            Some(SearchResult {
                title: text(row, "title").unwrap_or_else(|| url.clone()),
                snippet: text(row, "snippet")
                    .or_else(|| text(row, "description"))
                    .unwrap_or_default(),
                url,
            })
        })
        .take(count)
        .collect())
}

#[cfg(test)]
#[path = "websearch_keenable_tests.rs"]
mod tests;
