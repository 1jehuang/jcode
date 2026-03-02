use super::{Tool, ToolContext, ToolOutput};
use anyhow::Result;
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};

/// Web search using DuckDuckGo HTML (no API key required)
pub struct WebSearchTool {
    client: reqwest::Client,
}

impl WebSearchTool {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::builder()
                .user_agent("Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36")
                .build()
                .unwrap_or_default(),
        }
    }
}

#[derive(Deserialize)]
struct WebSearchInput {
    query: String,
    #[serde(default)]
    num_results: Option<usize>,
}

#[derive(Debug)]
struct SearchResult {
    title: String,
    url: String,
    snippet: String,
}

#[async_trait]
impl Tool for WebSearchTool {
    fn name(&self) -> &str {
        "websearch"
    }

    fn description(&self) -> &str {
        "Search the web using DuckDuckGo. Returns a list of search results with titles, URLs, and snippets. \
         Useful for finding current information, documentation, or resources."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["query"],
            "properties": {
                "query": {
                    "type": "string",
                    "description": "The search query"
                },
                "num_results": {
                    "type": "integer",
                    "description": "Number of results to return (default: 8, max: 20)"
                }
            }
        })
    }

    async fn execute(&self, input: Value, _ctx: ToolContext) -> Result<ToolOutput> {
        let params: WebSearchInput = serde_json::from_value(input)?;
        let num_results = params.num_results.unwrap_or(8).min(20);

        // Use DuckDuckGo HTML search
        let url = format!(
            "https://html.duckduckgo.com/html/?q={}",
            urlencoding::encode(&params.query)
        );

        let response = self.client.get(&url).send().await?;

        if !response.status().is_success() {
            return Err(anyhow::anyhow!(
                "Search failed with status: {}",
                response.status()
            ));
        }

        let html = response.text().await?;
        let results = parse_ddg_results(&html, num_results);

        if results.is_empty() {
            return Ok(ToolOutput::new(format!(
                "No results found for: {}",
                params.query
            )));
        }

        // Format results
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

mod search_regex {
    use regex::Regex;
    use std::sync::OnceLock;

    macro_rules! static_regex {
        ($name:ident, $pat:expr) => {
            pub fn $name() -> &'static Regex {
                static RE: OnceLock<Regex> = OnceLock::new();
                RE.get_or_init(|| Regex::new($pat).unwrap())
            }
        };
    }

    static_regex!(result_link, r#"<a[^>]*class="result__a"[^>]*href="([^"]*)"[^>]*>([^<]*)</a>"#);
    static_regex!(result_snippet, r#"<a[^>]*class="result__snippet"[^>]*>([^<]*(?:<[^>]*>[^<]*)*)</a>"#);
    static_regex!(tag, r"<[^>]+>");
}

fn parse_ddg_results(html: &str, max_results: usize) -> Vec<SearchResult> {
    let mut results = Vec::new();

    let links: Vec<_> = search_regex::result_link().captures_iter(html).collect();
    let snippets: Vec<_> = search_regex::result_snippet().captures_iter(html).collect();

    for (i, link_cap) in links.iter().enumerate() {
        if results.len() >= max_results {
            break;
        }

        let url = decode_ddg_url(&link_cap[1]);
        let title = html_decode(&link_cap[2]);

        if !url.starts_with("http") || url.contains("duckduckgo.com") {
            continue;
        }

        let snippet = if i < snippets.len() {
            let raw = &snippets[i][1];
            html_decode(&search_regex::tag().replace_all(raw, ""))
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
