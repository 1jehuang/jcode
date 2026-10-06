use super::{SearchResult, WebSearchTool, resolve_api_key};
use anyhow::Result;
use serde::Deserialize;
use serde_json::json;

impl WebSearchTool {
    /// Query the Exa search API (neural search designed for LLM agents).
    pub(super) async fn search_exa(
        &self,
        query: &str,
        num_results: usize,
    ) -> Result<Vec<SearchResult>> {
        let config = crate::config::config();
        let api_key = resolve_api_key(
            config.websearch.exa_api_key.as_deref(),
            &config.websearch.exa_api_key_env,
        )
        .ok_or_else(|| {
            anyhow::anyhow!(
                "Exa engine selected but no API key configured. Set \
                 `websearch.exa_api_key` in your config or the {} environment variable.",
                config.websearch.exa_api_key_env
            )
        })?;

        let response = self
            .client
            .post("https://api.exa.ai/search")
            .header("x-api-key", api_key)
            .header(reqwest::header::ACCEPT, "application/json")
            .json(&json!({
                "query": query,
                "numResults": num_results,
                "contents": { "text": { "maxCharacters": 500 } },
            }))
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(anyhow::anyhow!(
                "Exa search failed with status {}. Check the configured API key.",
                response.status()
            ));
        }

        let parsed: ExaResponse = response.json().await?;
        Ok(parse_exa_results(parsed, num_results))
    }
}

#[derive(Deserialize)]
pub(super) struct ExaResponse {
    #[serde(default)]
    results: Vec<ExaResult>,
}

#[derive(Deserialize)]
struct ExaResult {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    url: String,
    #[serde(default)]
    text: Option<String>,
}

/// Map a parsed Exa JSON response to `SearchResult`s, dropping entries with
/// empty URLs and capping to `num_results`.
pub(super) fn parse_exa_results(response: ExaResponse, num_results: usize) -> Vec<SearchResult> {
    response
        .results
        .into_iter()
        .filter(|r| !r.url.trim().is_empty())
        .take(num_results)
        .map(|r| SearchResult {
            title: match r.title {
                Some(t) if !t.trim().is_empty() => t,
                _ => r.url.clone(),
            },
            url: r.url,
            snippet: r.text.unwrap_or_else(String::new),
        })
        .collect()
}
