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
                std::env::var(&config.websearch.searxng_url_env)
                    .ok()
                    .filter(|u| !u.trim().is_empty())
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
        std::env::var(name).ok()
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
mod tests {
    fn custom_config() -> CustomWebSearchConfig {
        CustomWebSearchConfig {
            endpoint: "https://example.test/search?keep=yes&q=old".into(),
            count_param: Some("count".into()),
            ..Default::default()
        }
    }

    #[test]
    fn custom_get_encodes_query_and_resolves_env_headers() {
        let mut config = custom_config();
        config.params.insert("format".into(), "json".into());
        config.headers.insert("authorization".into(), "old".into());
        config
            .header_env
            .insert("Authorization".into(), "SEARCH_TOKEN".into());
        let request = custom_request(&reqwest::Client::new(), &config, "rust & c++", 3, |name| {
            assert_eq!(name, "SEARCH_TOKEN");
            Some("Bearer secret".into())
        })
        .unwrap();
        assert_eq!(request.method(), reqwest::Method::GET);
        let pairs: std::collections::BTreeMap<_, _> = request.url().query_pairs().collect();
        assert_eq!(pairs["q"], "rust & c++");
        assert_eq!(pairs["count"], "3");
        assert_eq!(pairs["keep"], "yes");
        assert_eq!(
            request
                .url()
                .query_pairs()
                .filter(|(k, _)| k == "q")
                .count(),
            1
        );
        assert!(request.url().as_str().contains("%26"));
        assert_eq!(request.headers()["authorization"], "Bearer secret");
        assert!(request.headers()["authorization"].is_sensitive());
    }

    #[test]
    fn custom_post_merges_body_and_optional_count() {
        let mut config = custom_config();
        config.method = CustomWebSearchMethod::Post;
        config.query_param = "search".into();
        config.count_param = None;
        config.body = json!({"search": "old", "options": {"safe": true}});
        let request = custom_request(&reqwest::Client::new(), &config, "new", 4, |_| None).unwrap();
        assert_eq!(request.method(), reqwest::Method::POST);
        let body: Value =
            serde_json::from_slice(request.body().unwrap().as_bytes().unwrap()).unwrap();
        assert_eq!(body, json!({"search": "new", "options": {"safe": true}}));
        config.count_param = Some("limit".into());
        config.params.insert("static".into(), "yes".into());
        let request = custom_request(&reqwest::Client::new(), &config, "new", 4, |_| None).unwrap();
        let body: Value =
            serde_json::from_slice(request.body().unwrap().as_bytes().unwrap()).unwrap();
        assert_eq!(body["limit"], json!(4));
        assert!(
            request
                .url()
                .query_pairs()
                .any(|(k, v)| k == "static" && v == "yes")
        );
    }

    #[test]
    fn custom_maps_nested_and_escaped_pointers() {
        let mut config = custom_config();
        config.results_pointer = "/data/hits".into();
        config.url_pointer = "/link/href".into();
        config.title_pointer = "/a~1b/~0title".into();
        let body = json!({"data": {"hits": [
            {"link": {"href": ""}},
            {"link": {"href": "https://one.test"}, "a/b": {"~title": "One"}},
            {"link": {"href": "https://two.test"}, "snippet": "Two"}
        ]}});
        let results = parse_custom_results(&body, &config, 2).unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].title, "One");
        assert_eq!(results[1].title, "https://two.test");
        assert_eq!(results[1].snippet, "Two");
        assert_eq!(parse_custom_results(&body, &config, 0).unwrap().len(), 0);
        assert!(parse_custom_results(&json!({}), &config, 2).is_err());
        config.results_pointer.clear();
        config.url_pointer = "/url".into();
        let results = parse_custom_results(
            &json!([{ "url": 7 }, { "url": null }, { "url": " " }, { "url": "https://root.test" }]),
            &config,
            2,
        )
        .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].url, "https://root.test");
    }

    #[test]
    fn custom_validation_errors_do_not_expose_credentials() {
        let client = reqwest::Client::new();
        let mut config = custom_config();
        config.endpoint = "https://user:secret@example.test".into();
        let error = custom_request(&client, &config, "q", 1, |_| None)
            .unwrap_err()
            .to_string();
        assert!(error.contains("userinfo"));
        assert!(!error.contains("secret"));
        config = custom_config();
        config
            .headers
            .insert("authorization".into(), "secret\ninvalid".into());
        let error = custom_request(&client, &config, "q", 1, |_| None)
            .unwrap_err()
            .to_string();
        assert!(!error.contains("secret"));
        config.headers.clear();
        config
            .header_env
            .insert("authorization".into(), "SEARCH_TOKEN".into());
        assert!(custom_request(&client, &config, "q", 1, |_| None).is_err());
        config.header_env.clear();
        config.results_pointer = "/invalid~2escape".into();
        assert!(validate_custom(&config).is_err());
        config.results_pointer = "/results".into();
        config.body = json!([]);
        assert!(validate_custom(&config).is_err());
    }

    #[tokio::test]
    async fn custom_local_http_and_redirect_safety() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        for redirect in [false, true] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let server = tokio::spawn(async move {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut received = Vec::new();
                loop {
                    let mut buffer = [0; 1024];
                    let n = stream.read(&mut buffer).await.unwrap();
                    assert!(n > 0);
                    received.extend_from_slice(&buffer[..n]);
                    if received.windows(4).any(|w| w == b"\r\n\r\n") {
                        break;
                    }
                }
                let request = String::from_utf8(received).unwrap();
                assert!(
                    request
                        .starts_with("GET /search?token=secret&count=2&q=rust+%26+json HTTP/1.1")
                );
                assert!(
                    request
                        .to_ascii_lowercase()
                        .contains("authorization: bearer secret")
                );
                let body =
                    r#"{"results":[{"title":"Rust","url":"https://rust.test","snippet":"JSON"}]}"#;
                let response = if redirect {
                    format!(
                        "HTTP/1.1 302 Found\r\nLocation: http://{address}/leak\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    )
                } else {
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    )
                };
                stream.write_all(response.as_bytes()).await.unwrap();
                if redirect {
                    assert!(
                        tokio::time::timeout(
                            std::time::Duration::from_millis(200),
                            listener.accept()
                        )
                        .await
                        .is_err()
                    );
                }
            });
            let mut config = custom_config();
            config.endpoint = format!("http://{address}/search?token=secret");
            config
                .headers
                .insert("Authorization".into(), "Bearer secret".into());
            let result = search_custom(&config, "rust & json", 2).await;
            if redirect {
                let error = result.unwrap_err().to_string();
                assert!(error.contains("302"));
                assert!(!error.contains("secret"));
            } else {
                let results = result.unwrap();
                assert_eq!(results[0].title, "Rust");
                assert_eq!(results[0].snippet, "JSON");
            }
            server.await.unwrap();
        }
    }

    #[tokio::test]
    async fn custom_transport_and_json_errors_hide_url_secrets() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let mut config = custom_config();
        config.endpoint = format!("http://{address}/secret-path?token=credential-sentinel");
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut bytes = [0; 4096];
            assert!(stream.read(&mut bytes).await.unwrap() > 0);
            stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 19\r\nConnection: close\r\n\r\ncredential-sentinel").await.unwrap();
        });
        let error = format!(
            "{:#}",
            search_custom(&config, "private-query", 1)
                .await
                .unwrap_err()
        );
        assert!(!error.contains("credential-sentinel"));
        assert!(!error.contains("secret-path"));
        assert!(!error.contains("private-query"));
        server.await.unwrap();
        let error = format!(
            "{:#}",
            search_custom(&config, "private-query", 1)
                .await
                .unwrap_err()
        );
        assert!(!error.contains("credential-sentinel"));
        assert!(!error.contains("secret-path"));
        assert!(!error.contains("private-query"));
    }

    #[test]
    fn native_engine_falls_back_to_local_engines() {
        use super::local_engine_order;
        assert_eq!(
            local_engine_order(WebSearchEngine::Native, &[WebSearchEngine::Searxng]),
            vec![WebSearchEngine::Searxng]
        );
        assert_eq!(
            local_engine_order(WebSearchEngine::Native, &[WebSearchEngine::Native]),
            vec![WebSearchEngine::Duckduckgo, WebSearchEngine::Bing]
        );
        assert_eq!(
            local_engine_order(
                WebSearchEngine::Bing,
                &[WebSearchEngine::Duckduckgo, WebSearchEngine::Bing]
            ),
            vec![WebSearchEngine::Bing, WebSearchEngine::Duckduckgo]
        );
    }

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
        assert_eq!(WebSearchEngine::parse("google"), None);
    }

    #[test]
    fn detects_ddg_anomaly_challenge_page() {
        // Shape of the anti-bot challenge DDG serves (HTTP 200) instead of
        // results when a request is flagged (e.g. TLS fingerprint on Linux).
        let html = r#"<!DOCTYPE html><html><head>
            <script src="/dist/anomaly.js"></script></head>
            <body><div class="anomaly-modal__title">Unfortunately, bots use DuckDuckGo too.</div>
            </body></html>"#;
        assert_eq!(detect_anti_bot_page(html), Some("anomaly challenge"));
        // And it should parse to zero real results.
        assert!(parse_ddg_results(html, 10).is_empty());
    }

    #[test]
    fn detects_generic_captcha_page() {
        let html = r#"<html><body><div class="g-recaptcha"></div>
            Please verify you are human.</body></html>"#;
        assert!(detect_anti_bot_page(html).is_some());
    }

    #[test]
    fn real_results_are_not_flagged_as_anti_bot() {
        let html = r#"
            <div class="result results_links web-result">
              <a class="result__a" href="https://rust-lang.org/">Rust</a>
              <a class="result__snippet" href="https://rust-lang.org/">A language.</a>
            </div>
        "#;
        assert_eq!(detect_anti_bot_page(html), None);
        assert_eq!(parse_ddg_results(html, 10).len(), 1);
    }

    // Captured from a live DuckDuckGo request that was flagged on Linux (GH #270):
    // the HTML endpoint returns HTTP 202 with an "anomaly" challenge page and no
    // results. These fixtures pin the real-world shapes so the fix stays honest.
    #[test]
    fn real_captured_ddg_anomaly_fixture_is_detected() {
        let html = include_str!("testdata/ddg_anomaly.html");
        // The bug: this page parses to zero real results...
        assert!(
            parse_ddg_results(html, 10).is_empty(),
            "anomaly page should yield no results"
        );
        // ...but the fix now recognizes it as a challenge instead of a silent
        // "no results found".
        assert_eq!(detect_anti_bot_page(html), Some("anomaly challenge"));
    }

    #[test]
    fn real_captured_ddg_results_fixture_parses() {
        let html = include_str!("testdata/ddg_results.html");
        assert_eq!(detect_anti_bot_page(html), None);
        assert!(
            !parse_ddg_results(html, 10).is_empty(),
            "real results page should yield results"
        );
    }

    #[test]
    fn parses_searxng_json_results() {
        // Shape of a real SearXNG /search?format=json response (#270).
        let body = serde_json::json!({
            "query": "rust",
            "results": [
                {
                    "url": "https://www.rust-lang.org/",
                    "title": "Rust Programming Language",
                    "content": "A language empowering everyone."
                },
                {
                    "url": "https://doc.rust-lang.org/book/",
                    "title": "The Rust Book",
                    "content": "Learn Rust."
                },
                // Entry with empty url is dropped; missing content tolerated.
                { "url": "", "title": "junk" },
                { "url": "https://crates.io", "title": "" }
            ]
        });
        let parsed: SearxngResponse = serde_json::from_value(body).unwrap();
        let results = parse_searxng_results(parsed, 10);
        assert_eq!(results.len(), 3, "empty-url entry should be dropped");
        assert_eq!(results[0].url, "https://www.rust-lang.org/");
        assert_eq!(results[0].title, "Rust Programming Language");
        assert_eq!(results[0].snippet, "A language empowering everyone.");
        // Missing title falls back to the URL.
        assert_eq!(results[2].title, "https://crates.io");
        assert_eq!(results[2].snippet, "");
    }

    #[test]
    fn searxng_results_respect_limit() {
        let body = serde_json::json!({
            "results": (0..10)
                .map(|i| serde_json::json!({"url": format!("https://x/{i}"), "title": "t"}))
                .collect::<Vec<_>>()
        });
        let parsed: SearxngResponse = serde_json::from_value(body).unwrap();
        assert_eq!(parse_searxng_results(parsed, 3).len(), 3);
    }

    #[test]
    fn websearch_engine_parses_searxng_aliases() {
        assert_eq!(
            WebSearchEngine::parse("searxng"),
            Some(WebSearchEngine::Searxng)
        );
        assert_eq!(
            WebSearchEngine::parse("searx"),
            Some(WebSearchEngine::Searxng)
        );
        assert_eq!(WebSearchEngine::Searxng.as_str(), "searxng");
    }
}
