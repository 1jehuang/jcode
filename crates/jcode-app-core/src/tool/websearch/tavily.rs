use super::{SearchResult, WebSearchTool, resolve_api_key};
use anyhow::Result;
use serde::Deserialize;
use serde_json::json;

impl WebSearchTool {
    /// Query the Tavily search API (JSON API designed for LLM agents).
    pub(super) async fn search_tavily(
        &self,
        query: &str,
        num_results: usize,
    ) -> Result<Vec<SearchResult>> {
        let config = crate::config::config();
        let api_key = resolve_api_key(
            config.websearch.tavily_api_key.as_deref(),
            &config.websearch.tavily_api_key_env,
        )
        .ok_or_else(|| {
            anyhow::anyhow!(
                "Tavily engine selected but no API key configured. Set \
                 `websearch.tavily_api_key` in your config or the {} environment variable.",
                config.websearch.tavily_api_key_env
            )
        })?;

        let response = self
            .client
            .post("https://api.tavily.com/search")
            .header(reqwest::header::AUTHORIZATION, format!("Bearer {api_key}"))
            .header(reqwest::header::ACCEPT, "application/json")
            .json(&json!({
                "query": query,
                "max_results": num_results,
            }))
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(anyhow::anyhow!(
                "Tavily search failed with status {}. Check the configured API key.",
                response.status()
            ));
        }

        let parsed: TavilyResponse = response.json().await?;
        Ok(parse_tavily_results(parsed, num_results))
    }
}

#[derive(Deserialize)]
pub(super) struct TavilyResponse {
    #[serde(default)]
    results: Vec<TavilyResult>,
}

#[derive(Deserialize)]
struct TavilyResult {
    #[serde(default)]
    title: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    content: Option<String>,
}

/// Map a parsed Tavily JSON response to `SearchResult`s, dropping entries with
/// empty URLs and capping to `num_results`.
pub(super) fn parse_tavily_results(
    response: TavilyResponse,
    num_results: usize,
) -> Vec<SearchResult> {
    response
        .results
        .into_iter()
        .filter(|r| !r.url.trim().is_empty())
        .take(num_results)
        .map(|r| SearchResult {
            title: if r.title.trim().is_empty() {
                r.url.clone()
            } else {
                r.title
            },
            url: r.url,
            snippet: r.content.unwrap_or_else(String::new),
        })
        .collect()
}
