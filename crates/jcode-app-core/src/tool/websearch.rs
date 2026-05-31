use super::{Tool, ToolContext, ToolOutput};
use crate::config::WebSearchEngine;
use anyhow::{Context, Result};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};

/// Web search using DuckDuckGo or Bing (HTML scraping, with optional Bing API)
pub struct WebSearchTool {
    client: reqwest::Client,
}

impl WebSearchTool {
    pub fn new() -> Self {
        Self {
            client: crate::provider::shared_http_client(),
        }
    }
}

#[derive(Deserialize)]
struct WebSearchInput {
    query: String,
    #[serde(default)]
    num_results: Option<usize>,
    #[serde(default)]
    engine: Option<WebSearchEngine>,
    #[serde(default)]
    bing_market: Option<String>,
}

#[derive(Debug)]
struct SearchResult {
    title: String,
    url: String,
    snippet: String,
}

#[derive(Clone, Copy)]
struct BingSearchOptions<'a> {
    market: &'a str,
    configured_api_key: Option<&'a str>,
    api_key_env: &'a str,
}

#[async_trait]
impl Tool for WebSearchTool {
    fn name(&self) -> &str {
        "websearch"
    }

    fn description(&self) -> &str {
        "Search the web."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["query"],
            "properties": {
                "intent": super::intent_schema_property(),
                "query": {
                    "type": "string",
                    "description": "Search query."
                },
                "num_results": {
                    "type": "integer",
                    "description": "Max results."
                },
                "engine": {
                    "type": "string",
                    "enum": ["duckduckgo", "bing"],
                    "description": "Search engine. Defaults to duckduckgo. Bing uses JCODE_BING_API_KEY when set, otherwise Bing HTML scraping."
                },
                "bing_market": {
                    "type": "string",
                    "description": "Optional Bing market, e.g. en-US or zh-CN. Defaults to JCODE_BING_MARKET or en-US."
                }
            }
        })
    }

    async fn execute(&self, input: Value, _ctx: ToolContext) -> Result<ToolOutput> {
        let params: WebSearchInput = serde_json::from_value(input)?;
        let num_results = params.num_results.unwrap_or(8).min(20);

        let config = crate::config::config();
        let mut engines = Vec::new();
        engines.push(params.engine.unwrap_or(config.websearch.engine));
        engines.extend(config.websearch.fallback_engines.iter().copied());
        engines.dedup();

        let market = params
            .bing_market
            .as_deref()
            .unwrap_or(&config.websearch.bing_market);
        let mut last_error = None;
        let mut results = Vec::new();
        for (index, engine) in engines.into_iter().enumerate() {
            let allow_bing_api = index == 0;
            match self
                .search_with_engine(
                    engine,
                    &params.query,
                    num_results,
                    BingSearchOptions {
                        market,
                        configured_api_key: config.websearch.bing_api_key.as_deref(),
                        api_key_env: &config.websearch.bing_api_key_env,
                    },
                    allow_bing_api,
                )
                .await
            {
                Ok(found) => {
                    if !found.is_empty() {
                        results = found;
                        break;
                    }
                }
                Err(err) => last_error = Some(err),
            }
        }

        if results.is_empty()
            && let Some(err) = last_error
        {
            return Err(err);
        }

        if results.is_empty() {
            return Ok(ToolOutput::new(format!(
                "No results found for: {}",
                params.query
            )));
        }

        let mut output = format!("Search results for: {}\n\n", params.query);

        for (i, result) in results.iter().enumerate() {
            output.push_str(&format!(
                "{}. **{}**\n   {}\n   {}\n\n",
                i + 1,
                result.title,
                result.url,
                result.snippet
            ));
        }

        Ok(ToolOutput::new(output))
    }
}

impl WebSearchTool {
    async fn search_with_engine(
        &self,
        engine: WebSearchEngine,
        query: &str,
        num_results: usize,
        bing: BingSearchOptions<'_>,
        allow_bing_api: bool,
    ) -> Result<Vec<SearchResult>> {
        match engine {
            WebSearchEngine::Duckduckgo => self.search_duckduckgo(query, num_results).await,
            WebSearchEngine::Bing => {
                self.search_bing(query, num_results, bing, allow_bing_api)
                    .await
            }
            WebSearchEngine::Exa => self.search_exa(query, num_results).await,
        }
    }

    /// Issue #79: Exa.ai semantic search.
    ///
    /// Endpoint: POST https://api.exa.ai/search
    /// Auth:     x-api-key header (read from EXA_API_KEY env)
    /// Body:     `{ "query": ..., "numResults": N, "useAutoprompt": true }`
    ///
    /// Returns `Err` (with helpful message) if EXA_API_KEY is unset, so the
    /// caller's fallback chain can move on to DuckDuckGo / Bing.
    async fn search_exa(&self, query: &str, num_results: usize) -> Result<Vec<SearchResult>> {
        let api_key = std::env::var("EXA_API_KEY").map_err(|_| {
            anyhow::anyhow!(
                "Exa search requires EXA_API_KEY env var. Get one at https://dashboard.exa.ai (free tier: 1k queries/month)."
            )
        })?;
        let api_key = api_key.trim().to_string();
        if api_key.is_empty() {
            anyhow::bail!("EXA_API_KEY is set but empty.");
        }

        let body = serde_json::json!({
            "query": query,
            "numResults": num_results,
            "useAutoprompt": true,
            "type": "auto",
        });

        let response = self
            .client
            .post("https://api.exa.ai/search")
            .header("x-api-key", api_key)
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await
            .context("exa: POST /search")?;

        let status = response.status();
        let text = response.text().await.context("exa: read body")?;
        if !status.is_success() {
            anyhow::bail!("Exa search failed ({}): {}", status, text);
        }

        let json: serde_json::Value =
            serde_json::from_str(&text).context("exa: parse response JSON")?;
        let results = json
            .get("results")
            .and_then(|r| r.as_array())
            .ok_or_else(|| anyhow::anyhow!("exa: response missing 'results' array"))?;

        let mut out = Vec::with_capacity(results.len());
        for item in results {
            let title = item
                .get("title")
                .and_then(|v| v.as_str())
                .unwrap_or("(untitled)")
                .to_string();
            let url = item
                .get("url")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let snippet = item
                .get("text")
                .and_then(|v| v.as_str())
                .or_else(|| item.get("snippet").and_then(|v| v.as_str()))
                .unwrap_or("")
                .to_string();
            if !url.is_empty() {
                out.push(SearchResult {
                    title,
                    url,
                    snippet,
                });
            }
        }
        Ok(out)
    }

    async fn search_duckduckgo(
        &self,
        query: &str,
        num_results: usize,
    ) -> Result<Vec<SearchResult>> {
        // DuckDuckGo's HTML endpoint now serves an anti-bot "anomaly" challenge
        // (HTTP 202, no results) for plain GET requests. Submitting the query as
        // a POST form, the same way the real HTML page does, still returns the
        // standard results markup with a 200.
        let response = self
            .client
            .post("https://html.duckduckgo.com/html/")
            .header(
                reqwest::header::USER_AGENT,
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36",
            )
            .header(reqwest::header::ACCEPT, "text/html,application/xhtml+xml")
            .header(
                reqwest::header::CONTENT_TYPE,
                "application/x-www-form-urlencoded",
            )
            .form(&[("q", query), ("kl", "us-en")])
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(anyhow::anyhow!(
                "Search failed with status: {}",
                response.status()
            ));
        }

        Ok(parse_ddg_results(&response.text().await?, num_results))
    }

    async fn search_bing(
        &self,
        query: &str,
        num_results: usize,
        options: BingSearchOptions<'_>,
        allow_api: bool,
    ) -> Result<Vec<SearchResult>> {
        if allow_api {
            if let Some(api_key) = options
                .configured_api_key
                .filter(|key| !key.trim().is_empty())
            {
                return self
                    .search_bing_api(query, num_results, options.market, api_key)
                    .await;
            }
            if let Ok(api_key) = std::env::var(options.api_key_env)
                && !api_key.trim().is_empty()
            {
                return self
                    .search_bing_api(query, num_results, options.market, &api_key)
                    .await;
            }
        }

        self.search_bing_html(query, num_results, options.market)
            .await
    }

    async fn search_bing_api(
        &self,
        query: &str,
        num_results: usize,
        market: &str,
        api_key: &str,
    ) -> Result<Vec<SearchResult>> {
        let response = self
            .client
            .get("https://api.bing.microsoft.com/v7.0/search")
            .query(&[
                ("q", query),
                ("count", &num_results.to_string()),
                ("mkt", market),
            ])
            .header("Ocp-Apim-Subscription-Key", api_key)
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(anyhow::anyhow!(
                "Bing API search failed with status: {}",
                response.status()
            ));
        }

        Ok(parse_bing_api_results(response.json().await?, num_results))
    }

    async fn search_bing_html(
        &self,
        query: &str,
        num_results: usize,
        market: &str,
    ) -> Result<Vec<SearchResult>> {
        let url = format!(
            "https://www.bing.com/search?q={}&mkt={}",
            urlencoding::encode(query),
            urlencoding::encode(market)
        );

        let response = self
            .client
            .get(&url)
            .header(
                reqwest::header::USER_AGENT,
                "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36",
            )
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(anyhow::anyhow!(
                "Bing search failed with status: {}",
                response.status()
            ));
        }

        Ok(parse_bing_html_results(
            &response.text().await?,
            num_results,
        ))
    }
}

mod search_regex {
    use regex::Regex;
    use std::sync::OnceLock;

    fn compile_regex(pattern: &str, label: &str) -> Option<Regex> {
        match Regex::new(pattern) {
            Ok(regex) => Some(regex),
            Err(err) => {
                crate::logging::warn(&format!(
                    "websearch: failed to compile static regex {label}: {}",
                    err
                ));
                None
            }
        }
    }

    macro_rules! static_regex {
        ($name:ident, $pat:expr_2021) => {
            pub fn $name() -> Option<&'static Regex> {
                static RE: OnceLock<Option<Regex>> = OnceLock::new();
                RE.get_or_init(|| compile_regex($pat, stringify!($name)))
                    .as_ref()
            }
        };
    }

    static_regex!(
        result_link,
        r#"(?s)<a[^>]*class="result__a"[^>]*href="([^"]*)"[^>]*>(.*?)</a>"#
    );
    static_regex!(
        result_snippet,
        r#"(?s)<a[^>]*class="result__snippet"[^>]*>(.*?)</a>"#
    );
    static_regex!(tag, r"<[^>]+>");
    static_regex!(
        bing_result_block,
        r#"(?s)<li[^>]*class="[^"]*\bb_algo\b[^"]*"[^>]*>(.*?)</li>"#
    );
    static_regex!(
        bing_link,
        r#"(?s)<h2[^>]*>\s*<a[^>]*href="([^"]+)"[^>]*>(.*?)</a>\s*</h2>"#
    );
    static_regex!(
        bing_caption,
        r#"(?s)<div[^>]*class="[^"]*\bb_caption\b[^"]*"[^>]*>.*?<p[^>]*>(.*?)</p>"#
    );
}

#[derive(Deserialize)]
struct BingApiResponse {
    #[serde(rename = "webPages")]
    web_pages: Option<BingWebPages>,
}

#[derive(Deserialize)]
struct BingWebPages {
    value: Vec<BingWebPage>,
}

#[derive(Deserialize)]
struct BingWebPage {
    name: String,
    url: String,
    #[serde(default)]
    snippet: String,
}

fn parse_bing_api_results(response: BingApiResponse, max_results: usize) -> Vec<SearchResult> {
    response
        .web_pages
        .map(|pages| {
            pages
                .value
                .into_iter()
                .take(max_results)
                .map(|page| SearchResult {
                    title: page.name,
                    url: page.url,
                    snippet: page.snippet,
                })
                .collect()
        })
        .unwrap_or_default()
}

fn parse_bing_html_results(html: &str, max_results: usize) -> Vec<SearchResult> {
    let mut results = Vec::new();
    let (Some(block_re), Some(link_re), Some(caption_re), Some(tag_re)) = (
        search_regex::bing_result_block(),
        search_regex::bing_link(),
        search_regex::bing_caption(),
        search_regex::tag(),
    ) else {
        return results;
    };

    for block in block_re.captures_iter(html) {
        if results.len() >= max_results {
            break;
        }
        let Some(link) = link_re.captures(&block[1]) else {
            continue;
        };
        let url = html_decode(&link[1]);
        if !url.starts_with("http") || url.contains("bing.com") {
            continue;
        }
        let title = html_decode(&tag_re.replace_all(&link[2], ""));
        let snippet = caption_re
            .captures(&block[1])
            .map(|cap| html_decode(&tag_re.replace_all(&cap[1], "")))
            .unwrap_or_default();
        results.push(SearchResult {
            title,
            url,
            snippet,
        });
    }

    results
}

fn parse_ddg_results(html: &str, max_results: usize) -> Vec<SearchResult> {
    let mut results = Vec::new();

    let (Some(result_link), Some(result_snippet), Some(tag)) = (
        search_regex::result_link(),
        search_regex::result_snippet(),
        search_regex::tag(),
    ) else {
        return results;
    };

    let links: Vec<_> = result_link.captures_iter(html).collect();
    let snippets: Vec<_> = result_snippet.captures_iter(html).collect();

    for (i, link_cap) in links.iter().enumerate() {
        if results.len() >= max_results {
            break;
        }

        let url = decode_ddg_url(&link_cap[1]);
        let title = html_decode(&tag.replace_all(&link_cap[2], ""));

        if !url.starts_with("http") || url.contains("duckduckgo.com") {
            continue;
        }

        let snippet = if i < snippets.len() {
            let raw = &snippets[i][1];
            html_decode(&tag.replace_all(raw, ""))
        } else {
            String::new()
        };

        results.push(SearchResult {
            title,
            url,
            snippet,
        });
    }

    results
}

fn decode_ddg_url(url: &str) -> String {
    // DDG wraps URLs like //duckduckgo.com/l/?uddg=ACTUAL_URL&...
    if let Some(uddg_start) = url.find("uddg=") {
        let start = uddg_start + 5;
        let end = url[start..]
            .find('&')
            .map(|i| start + i)
            .unwrap_or(url.len());
        let encoded = &url[start..end];
        urlencoding::decode(encoded)
            .map(|s| s.to_string())
            .unwrap_or_else(|_| encoded.to_string())
    } else {
        url.to_string()
    }
}

fn html_decode(s: &str) -> String {
    s.replace("&nbsp;", " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&#x27;", "'")
        .replace("&apos;", "'")
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_bing_html_results() {
        let html = r#"
            <li class="b_algo">
              <h2><a href="https://example.com/rust">Rust &amp; Cargo</a></h2>
              <div class="b_caption"><p>A <strong>systems</strong> language.</p></div>
            </li>
            <li class="b_algo"><h2><a href="https://www.bing.com/aclk">ad</a></h2></li>
            <li class="b_algo">
              <h2><a href="https://example.org/jcode">Jcode</a></h2>
              <div class="b_caption"><p>Agentic coding.</p></div>
            </li>
        "#;

        let results = parse_bing_html_results(html, 10);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].title, "Rust & Cargo");
        assert_eq!(results[0].url, "https://example.com/rust");
        assert_eq!(results[0].snippet, "A systems language.");
        assert_eq!(results[1].title, "Jcode");
    }

    #[test]
    fn parses_bing_api_results() {
        let response: BingApiResponse = serde_json::from_value(json!({
            "webPages": {
                "value": [
                    {"name": "One", "url": "https://one.test", "snippet": "first"},
                    {"name": "Two", "url": "https://two.test", "snippet": "second"}
                ]
            }
        }))
        .unwrap();

        let results = parse_bing_api_results(response, 1);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "One");
        assert_eq!(results[0].url, "https://one.test");
    }

    #[test]
    fn parses_ddg_html_results() {
        // Mirrors the markup html.duckduckgo.com returns for the POST form,
        // where titles and snippets contain inline <b> highlight tags.
        let html = r#"
            <div class="result results_links results_links_deep web-result">
              <a class="result__a" href="https://rust-lang.org/"><b>Rust</b> Language</a>
              <a class="result__snippet" href="https://rust-lang.org/">A <b>systems</b> programming language.</a>
            </div>
            <div class="result results_links results_links_deep web-result">
              <a class="result__a" href="https://en.wikipedia.org/wiki/Rust">Rust on Wikipedia</a>
              <a class="result__snippet" href="https://en.wikipedia.org/wiki/Rust">Encyclopedia <b>entry</b>.</a>
            </div>
        "#;

        let results = parse_ddg_results(html, 10);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].title, "Rust Language");
        assert_eq!(results[0].url, "https://rust-lang.org/");
        assert_eq!(results[0].snippet, "A systems programming language.");
        assert_eq!(results[1].url, "https://en.wikipedia.org/wiki/Rust");
        assert_eq!(results[1].snippet, "Encyclopedia entry.");
    }

    #[test]
    fn websearch_engine_accepts_aliases() {
        assert_eq!(
            WebSearchEngine::parse("ddg"),
            Some(WebSearchEngine::Duckduckgo)
        );
        assert_eq!(WebSearchEngine::parse("bing"), Some(WebSearchEngine::Bing));
        assert_eq!(WebSearchEngine::parse("exa"), Some(WebSearchEngine::Exa));
        assert_eq!(WebSearchEngine::parse("exa.ai"), Some(WebSearchEngine::Exa));
        assert_eq!(WebSearchEngine::parse("exa-ai"), Some(WebSearchEngine::Exa));
        assert_eq!(WebSearchEngine::parse("EXA"), Some(WebSearchEngine::Exa));
        assert_eq!(WebSearchEngine::parse("google"), None);
    }

    #[test]
    fn exa_engine_as_str_is_lowercase() {
        assert_eq!(WebSearchEngine::Exa.as_str(), "exa");
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn exa_search_errors_when_api_key_unset() {
        let _lock = crate::storage::lock_test_env();
        let prev = std::env::var_os("EXA_API_KEY");
        crate::env::remove_var("EXA_API_KEY");

        let tool = WebSearchTool::new();
        let res = tool.search_exa("rust async best practices", 5).await;
        assert!(res.is_err(), "must error when EXA_API_KEY missing");
        let msg = res.unwrap_err().to_string();
        assert!(
            msg.contains("EXA_API_KEY"),
            "error must mention EXA_API_KEY: {msg}"
        );

        if let Some(p) = prev {
            crate::env::set_var("EXA_API_KEY", p);
        }
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn exa_search_errors_when_api_key_blank() {
        let _lock = crate::storage::lock_test_env();
        let prev = std::env::var_os("EXA_API_KEY");
        crate::env::set_var("EXA_API_KEY", "   ");

        let tool = WebSearchTool::new();
        let res = tool.search_exa("rust", 1).await;
        assert!(res.is_err(), "blank key should fail validation");

        if let Some(p) = prev {
            crate::env::set_var("EXA_API_KEY", p);
        } else {
            crate::env::remove_var("EXA_API_KEY");
        }
    }
}
