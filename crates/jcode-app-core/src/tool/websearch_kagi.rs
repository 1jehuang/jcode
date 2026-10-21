//! Kagi's v1 Search API. Keep credentials out of URLs, diagnostics, and redirects.
use super::SearchResult;
use anyhow::{Result, anyhow, ensure};
use serde_json::{Value, json};

const ENDPOINT: &str = "https://kagi.com/api/v1/search";

fn client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|_| anyhow!("Kagi search HTTP client initialization failed"))
}

fn request(
    client: &reqwest::Client,
    endpoint: &str,
    query: &str,
    count: usize,
    key_env: &str,
    env: impl FnOnce(&str) -> Option<String>,
) -> Result<reqwest::Request> {
    ensure!(
        !key_env.trim().is_empty(),
        "Kagi search API key environment variable name is empty"
    );
    let key = env(key_env)
        .filter(|key| !key.trim().is_empty())
        .ok_or_else(|| anyhow!("Kagi search requires an API key in the configured environment variable (websearch.kagi_api_key_env)"))?;
    let mut auth = reqwest::header::HeaderValue::from_str(&format!("Bearer {}", key.trim()))
        .map_err(|_| anyhow!("Kagi search API key is not a valid HTTP header value"))?;
    auth.set_sensitive(true);
    client
        .post(endpoint)
        .header(reqwest::header::AUTHORIZATION, auth)
        .header(reqwest::header::ACCEPT, "application/json")
        .json(&json!({"query": query, "workflow": "search", "limit": count.clamp(1, 20)}))
        .build()
        .map_err(|_| anyhow!("Kagi search request construction failed"))
}

pub(super) async fn search(query: &str, count: usize, key_env: &str) -> Result<Vec<SearchResult>> {
    if count == 0 {
        return Ok(Vec::new());
    }
    let client = client()?;
    let request = request(&client, ENDPOINT, query, count, key_env, |name| {
        std::env::var(name).ok()
    })?;
    execute(&client, request, count).await
}

async fn execute(
    client: &reqwest::Client,
    request: reqwest::Request,
    count: usize,
) -> Result<Vec<SearchResult>> {
    // No automatic retries: every successful API request may incur a charge.
    let response = client
        .execute(request)
        .await
        .map_err(|_| anyhow!("Kagi search request failed"))?;
    ensure!(
        response.status().is_success(),
        "Kagi search failed with HTTP status {}",
        response.status()
    );
    let body: Value = response
        .json()
        .await
        .map_err(|_| anyhow!("Kagi search response is not valid JSON"))?;
    parse_results(&body, count)
}

fn parse_results(body: &Value, count: usize) -> Result<Vec<SearchResult>> {
    let rows = body
        .pointer("/data/search")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("Kagi search response is missing data.search results"))?;
    Ok(rows
        .iter()
        .filter_map(|row| {
            let url = row.get("url")?.as_str()?.trim();
            if url.is_empty() {
                return None;
            }
            Some(SearchResult {
                url: url.to_owned(),
                title: row
                    .get("title")
                    .and_then(Value::as_str)
                    .filter(|s| !s.trim().is_empty())
                    .unwrap_or(url)
                    .to_owned(),
                snippet: row
                    .get("snippet")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
            })
        })
        .take(count)
        .collect())
}

#[cfg(test)]
#[path = "websearch_kagi_tests.rs"]
mod tests;
