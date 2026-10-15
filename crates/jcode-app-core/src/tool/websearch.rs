use super::{Tool, ToolContext, ToolOutput};
use crate::config::{CustomWebSearchConfig, CustomWebSearchMethod, WebSearchEngine};
use anyhow::Result;
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
        jcode_message_types::provider_native::LOCAL_WEBSEARCH_DESCRIPTION
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
                    "enum": ["duckduckgo", "bing", "searxng", "custom"],
                    "description": "Engine. Defaults to configured engine; custom uses websearch.custom."
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
        let engines = local_engine_order(
            params.engine.unwrap_or(config.websearch.engine),
            &config.websearch.fallback_engines,
        );

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
                "No results found for: {}\n\n\
                 If results are consistently empty on this machine, the default \
                 DuckDuckGo/Bing engines may be blocked here by TLS fingerprinting \
                 or IP reputation (common on Linux/servers). Workaround:\n\
                 - Point at a SearXNG instance: set `websearch.searxng_url` (or \
                 JCODE_SEARXNG_URL) and use engine \"searxng\".",
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
            WebSearchEngine::Searxng => self.search_searxng(query, num_results).await,
            WebSearchEngine::Custom => {
                let config = crate::config::config();
                let custom = config.websearch.custom.as_ref().ok_or_else(|| {
                    anyhow::anyhow!("Custom search requires [websearch.custom] configuration")
                })?;
                search_custom(custom, query, num_results).await
            }
            // Provider-native search never reaches the local tool: engine order
            // filters it out. Kept for exhaustiveness.
            WebSearchEngine::Native => Ok(Vec::new()),
        }
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

        let body = response.text().await?;
        let results = parse_ddg_results(&body, num_results);
        if results.is_empty()
            && let Some(reason) = detect_anti_bot_page(&body)
        {
            return Err(anyhow::anyhow!(
                "DuckDuckGo served an anti-bot challenge page ({reason}) instead of \
                 results. This is commonly caused by TLS fingerprinting or IP \
                 reputation on Linux. Falling back to another engine if configured."
            ));
        }

        Ok(results)
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

        let body = response.text().await?;
        let results = parse_bing_html_results(&body, num_results);
        if results.is_empty()
            && let Some(reason) = detect_anti_bot_page(&body)
        {
            return Err(anyhow::anyhow!(
                "Bing served an anti-bot challenge page ({reason}) instead of results."
            ));
        }

        Ok(results)
    }

    /// Query a user-configured SearXNG instance via its JSON API. SearXNG is a
    /// self-hostable metasearch engine; because the request goes to an instance
    /// the user controls (or a public one they trust), it sidesteps the TLS
    /// fingerprinting / IP-reputation blocks that DuckDuckGo and Bing apply to
    /// scraped requests on some hosts (see issue #270).
    async fn search_searxng(&self, query: &str, num_results: usize) -> Result<Vec<SearchResult>> {
        let config = crate::config::config();
        let base = config
            .websearch
            .searxng_url
            .as_deref()
            .filter(|u| !u.trim().is_empty())
            .map(|u| u.to_string())
            .or_else(|| {
                match std::env::var(&config.websearch.searxng_url_env) {
                    Ok(url) => Some(url).filter(|u| !u.trim().is_empty()),
                    Err(std::env::VarError::NotPresent) => None,
                    Err(std::env::VarError::NotUnicode(_)) => {
                        // Report the configuration problem, not its potentially secret bytes.
                        crate::logging::warn(
                            "SearXNG URL environment variable is not valid Unicode",
                        );
                        None
                    }
                }
            })
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "SearXNG engine selected but no instance URL configured. Set \
                     `websearch.searxng_url` in your config or the {} environment \
                     variable to a SearXNG base URL (e.g. https://searx.example.org).",
                    config.websearch.searxng_url_env
                )
            })?;

        let endpoint = format!("{}/search", base.trim_end_matches('/'));
        let response = self
            .client
            .get(&endpoint)
            .query(&[("q", query), ("format", "json")])
            .header(
                reqwest::header::USER_AGENT,
                "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36",
            )
            .header(reqwest::header::ACCEPT, "application/json")
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(anyhow::anyhow!(
                "SearXNG search failed with status {} (endpoint: {endpoint}). \
                 Ensure the instance has the JSON format enabled in its settings.",
                response.status()
            ));
        }

        let parsed: SearxngResponse = response.json().await.map_err(|err| {
            anyhow::anyhow!(
                "SearXNG returned a non-JSON response ({err}). The instance may have \
                 the JSON format disabled; enable `formats: [html, json]` in its settings."
            )
        })?;

        Ok(parse_searxng_results(parsed, num_results))
    }
}

fn validate_custom(config: &CustomWebSearchConfig) -> Result<reqwest::Url> {
    let url = reqwest::Url::parse(&config.endpoint)
        .map_err(|_| anyhow::anyhow!("Custom search endpoint must be an absolute HTTP(S) URL"))?;
    anyhow::ensure!(
        matches!(url.scheme(), "http" | "https") && url.host_str().is_some(),
        "Custom search endpoint must be an absolute HTTP(S) URL"
    );
    anyhow::ensure!(
        url.username().is_empty() && url.password().is_none(),
        "Custom search endpoint must not contain userinfo; use headers or header_env"
    );
    anyhow::ensure!(
        url.fragment().is_none(),
        "Custom search endpoint must not contain a fragment"
    );
    anyhow::ensure!(
        !config.query_param.trim().is_empty(),
        "Custom search query_param must not be empty"
    );
    if let Some(count) = &config.count_param {
        anyhow::ensure!(
            !count.trim().is_empty() && count != &config.query_param,
            "Custom search count_param must be nonempty and different from query_param"
        );
    }
    anyhow::ensure!(
        config.body.is_object(),
        "Custom search body must be a JSON object"
    );
    for (name, pointer) in [
        ("results_pointer", &config.results_pointer),
        ("title_pointer", &config.title_pointer),
        ("url_pointer", &config.url_pointer),
        ("snippet_pointer", &config.snippet_pointer),
    ] {
        let mut chars = pointer.chars();
        let mut valid = pointer.is_empty() || pointer.starts_with('/');
        while let Some(c) = chars.next() {
            if c == '~' && !matches!(chars.next(), Some('0' | '1')) {
                valid = false;
            }
        }
        anyhow::ensure!(valid, "Custom search {name} must be a valid JSON pointer");
    }
    Ok(url)
}

fn custom_request(
    client: &reqwest::Client,
    config: &CustomWebSearchConfig,
    query: &str,
    count: usize,
    env: impl Fn(&str) -> Option<String>,
) -> Result<reqwest::Request> {
    use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
    let mut url = validate_custom(config)?;
    let mut params = config.params.clone();
    let mut body = config.body.clone();
    match config.method {
        CustomWebSearchMethod::Get => {
            params.insert(config.query_param.clone(), query.into());
            if let Some(name) = &config.count_param {
                params.insert(name.clone(), count.to_string());
            }
        }
        CustomWebSearchMethod::Post => {
            body[&config.query_param] = json!(query);
            if let Some(name) = &config.count_param {
                body[name] = json!(count);
            }
        }
    }
    // Configured parameters replace endpoint query keys, rather than producing
    // ambiguous duplicate keys. Preserve other endpoint parameters.
    let existing: Vec<(String, String)> = url
        .query_pairs()
        .filter(|(k, _)| !params.contains_key(k.as_ref()))
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    url.set_query(None);
    if !existing.is_empty() || !params.is_empty() {
        url.query_pairs_mut()
            .extend_pairs(existing)
            .extend_pairs(params);
    }
    let mut headers = HeaderMap::new();
    for (name, value) in &config.headers {
        let name = HeaderName::from_bytes(name.as_bytes()).map_err(|_| {
            anyhow::anyhow!("Custom search headers contains an invalid header name")
        })?;
        let mut value = HeaderValue::from_str(value).map_err(|_| {
            anyhow::anyhow!("Custom search headers contains an invalid header value")
        })?;
        value.set_sensitive(true);
        headers.insert(name, value);
    }
    for (name, variable) in &config.header_env {
        let name = HeaderName::from_bytes(name.as_bytes()).map_err(|_| {
            anyhow::anyhow!("Custom search header_env contains an invalid header name")
        })?;
        anyhow::ensure!(
            !variable.is_empty() && !variable.contains(['=', '\0']),
            "Custom search header_env contains an invalid environment variable name"
        );
        let value = env(variable)
            .filter(|v| !v.trim().is_empty())
            .ok_or_else(|| {
                anyhow::anyhow!("Custom search header_env variable {variable} is missing or empty")
            })?;
        let mut value = HeaderValue::from_str(&value).map_err(|_| {
            anyhow::anyhow!("Custom search header_env contains an invalid header value")
        })?;
        value.set_sensitive(true);
        headers.insert(name, value);
    }
    let builder = match config.method {
        CustomWebSearchMethod::Get => client.get(url),
        CustomWebSearchMethod::Post => client.post(url).json(&body),
    };
    builder
        .headers(headers)
        .build()
        .map_err(|e| anyhow::anyhow!("Custom search request failed: {}", e.without_url()))
}

async fn search_custom(
    config: &CustomWebSearchConfig,
    query: &str,
    count: usize,
) -> Result<Vec<SearchResult>> {
    // Never forward user-configured credentials to redirect destinations.
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|_| anyhow::anyhow!("Custom search HTTP client initialization failed"))?;
    let request = custom_request(&client, config, query, count, |name| {
        match std::env::var(name) {
            Ok(value) => Some(value),
            Err(error) => {
                // The request builder returns the missing-credential error to the
                // caller. Record why resolution failed without exposing credentials.
                let reason = match error {
                    std::env::VarError::NotPresent => "not set",
                    std::env::VarError::NotUnicode(_) => "not valid Unicode",
                };
                crate::logging::warn(&format!(
                    "Custom search credential variable {name} is {reason}"
                ));
                None
            }
        }
    })?;
    let response = client
        .execute(request)
        .await
        .map_err(|e| anyhow::anyhow!("Custom search request failed: {}", e.without_url()))?;
    anyhow::ensure!(
        response.status().is_success(),
        "Custom search failed with HTTP status {}",
        response.status()
    );
    let body: Value = response.json().await.map_err(|e| {
        anyhow::anyhow!(
            "Custom search response is not valid JSON: {}",
            e.without_url()
        )
    })?;
    parse_custom_results(&body, config, count)
}

fn parse_custom_results(
    body: &Value,
    config: &CustomWebSearchConfig,
    count: usize,
) -> Result<Vec<SearchResult>> {
    let items = body
        .pointer(&config.results_pointer)
        .and_then(Value::as_array)
        .ok_or_else(|| {
            anyhow::anyhow!("Custom search results_pointer must resolve to a JSON array")
        })?;
    Ok(items
        .iter()
        .filter_map(|item| {
            let url = item.pointer(&config.url_pointer)?.as_str()?.trim();
            if url.is_empty() {
                return None;
            }
            let title = item
                .pointer(&config.title_pointer)
                .and_then(Value::as_str)
                .filter(|s| !s.trim().is_empty())
                .unwrap_or(url);
            let snippet = item
                .pointer(&config.snippet_pointer)
                .and_then(Value::as_str)
                .unwrap_or_default();
            Some(SearchResult {
                title: title.into(),
                url: url.into(),
                snippet: snippet.into(),
            })
        })
        .take(count)
        .collect())
}

/// Map a parsed SearXNG JSON response to `SearchResult`s, dropping entries with
/// empty URLs and capping to `num_results`.
fn parse_searxng_results(response: SearxngResponse, num_results: usize) -> Vec<SearchResult> {
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
            snippet: r.content.unwrap_or_default(),
        })
        .collect()
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
struct SearxngResponse {
    #[serde(default)]
    results: Vec<SearxngResult>,
}

#[derive(Deserialize)]
struct SearxngResult {
    #[serde(default)]
    title: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    content: Option<String>,
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

/// Detect whether an HTML body is an anti-bot/captcha challenge rather than a
/// real results page. DuckDuckGo (and similar) serve these with HTTP 200, so a
/// successful status plus zero parsed results is ambiguous without this check.
///
/// Returns a short human-readable reason when a challenge page is detected.
fn detect_anti_bot_page(html: &str) -> Option<&'static str> {
    let lowered = html.to_ascii_lowercase();
    const MARKERS: &[(&str, &str)] = &[
        ("anomaly-modal", "anomaly challenge"),
        ("anomaly.js", "anomaly challenge"),
        ("dpn=1", "anomaly challenge"),
        ("captcha", "captcha"),
        ("g-recaptcha", "recaptcha"),
        ("are you a robot", "bot check"),
        ("unusual traffic", "bot check"),
        ("verify you are human", "human verification"),
        ("challenge-platform", "cloudflare challenge"),
        ("cf-challenge", "cloudflare challenge"),
    ];
    for (needle, reason) in MARKERS {
        if lowered.contains(needle) {
            return Some(reason);
        }
    }
    None
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

/// Engines the local tool tries, in order. `native` is provider-side and never
/// runs locally: when it is preferred (e.g. the active provider has no server
/// search), the local fallbacks run, defaulting to DuckDuckGo then Bing.
fn local_engine_order(
    preferred: WebSearchEngine,
    fallbacks: &[WebSearchEngine],
) -> Vec<WebSearchEngine> {
    let mut engines: Vec<WebSearchEngine> = std::iter::once(preferred)
        .chain(fallbacks.iter().copied())
        .filter(|engine| engine.is_local())
        .collect();
    if engines.is_empty() {
        engines = vec![WebSearchEngine::Duckduckgo, WebSearchEngine::Bing];
    }
    let mut seen = std::collections::HashSet::new();
    engines.retain(|engine| seen.insert(*engine));
    engines
}

#[cfg(test)]
#[path = "websearch_tests.rs"]
mod tests;
