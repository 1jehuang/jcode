use super::{Tool, ToolContext, ToolOutput};
use anyhow::{Context, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

const EXA_SEARCH_URL: &str = "https://api.exa.ai/search";
const EXA_INTEGRATION_HEADER: &str = "x-exa-integration";
const EXA_INTEGRATION_NAME: &str = "jcode";
const EXA_API_KEY_ENV: &str = "EXA_API_KEY";

const DEFAULT_NUM_RESULTS: usize = 8;
const MAX_NUM_RESULTS: usize = 25;
const DEFAULT_TEXT_MAX_CHARS: usize = 1000;

/// AI-powered web search via the Exa API. Requires `EXA_API_KEY`.
pub struct ExaSearchTool {
    client: reqwest::Client,
}

impl ExaSearchTool {
    pub fn new() -> Self {
        Self {
            client: crate::provider::shared_http_client(),
        }
    }
}

#[derive(Deserialize)]
struct ExaSearchInput {
    query: String,
    #[serde(default)]
    num_results: Option<usize>,
    #[serde(default, rename = "type")]
    search_type: Option<String>,
    #[serde(default)]
    category: Option<String>,
    #[serde(default)]
    include_domains: Option<Vec<String>>,
    #[serde(default)]
    exclude_domains: Option<Vec<String>>,
    #[serde(default)]
    start_published_date: Option<String>,
    #[serde(default)]
    end_published_date: Option<String>,
    #[serde(default)]
    user_location: Option<String>,
    #[serde(default)]
    contents: Option<ContentsInput>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct ContentsInput {
    /// Full page text. `true` enables with defaults; an object lets you cap length.
    text: Option<TextOption>,
    /// Highlight snippets. `true` enables with defaults.
    highlights: Option<bool>,
    /// LLM-generated summary, optionally guided by a query.
    summary: Option<SummaryOption>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum TextOption {
    Enabled(bool),
    Detailed { max_characters: Option<usize> },
}

#[derive(Deserialize)]
#[serde(untagged)]
enum SummaryOption {
    Enabled(bool),
    Guided { query: Option<String> },
}

#[derive(Debug, Deserialize)]
struct ExaResponse {
    #[serde(default)]
    results: Vec<ExaResult>,
}

#[derive(Debug, Deserialize)]
struct ExaResult {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    author: Option<String>,
    #[serde(default, rename = "publishedDate")]
    published_date: Option<String>,
    #[serde(default)]
    score: Option<f64>,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    highlights: Option<Vec<String>>,
    #[serde(default)]
    summary: Option<String>,
}

impl ExaResult {
    /// Pick the best available snippet: summary > highlights > truncated text.
    fn snippet(&self) -> Option<String> {
        if let Some(s) = self.summary.as_ref().filter(|s| !s.trim().is_empty()) {
            return Some(s.trim().to_string());
        }
        if let Some(highlights) = self.highlights.as_ref() {
            let joined: String = highlights
                .iter()
                .map(|h| h.trim())
                .filter(|h| !h.is_empty())
                .collect::<Vec<_>>()
                .join(" … ");
            if !joined.is_empty() {
                return Some(joined);
            }
        }
        if let Some(text) = self.text.as_ref().filter(|t| !t.trim().is_empty()) {
            return Some(truncate_chars(text.trim(), 400));
        }
        None
    }
}

fn truncate_chars(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    let head: String = s.chars().take(max_chars).collect();
    format!("{head}…")
}

#[derive(Serialize)]
struct ExaRequestBody<'a> {
    query: &'a str,
    #[serde(rename = "numResults")]
    num_results: usize,
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    search_type: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    category: Option<&'a str>,
    #[serde(rename = "includeDomains", skip_serializing_if = "Option::is_none")]
    include_domains: Option<&'a [String]>,
    #[serde(rename = "excludeDomains", skip_serializing_if = "Option::is_none")]
    exclude_domains: Option<&'a [String]>,
    #[serde(
        rename = "startPublishedDate",
        skip_serializing_if = "Option::is_none"
    )]
    start_published_date: Option<&'a str>,
    #[serde(rename = "endPublishedDate", skip_serializing_if = "Option::is_none")]
    end_published_date: Option<&'a str>,
    #[serde(rename = "userLocation", skip_serializing_if = "Option::is_none")]
    user_location: Option<&'a str>,
    #[serde(skip_serializing_if = "Value::is_null")]
    contents: Value,
}

fn build_contents_value(contents: &Option<ContentsInput>) -> Value {
    let Some(c) = contents else {
        // Default: highlights + a small text excerpt is a useful baseline.
        return json!({
            "highlights": true,
            "text": { "maxCharacters": DEFAULT_TEXT_MAX_CHARS }
        });
    };

    let mut obj = serde_json::Map::new();

    match &c.text {
        Some(TextOption::Enabled(true)) => {
            obj.insert(
                "text".into(),
                json!({ "maxCharacters": DEFAULT_TEXT_MAX_CHARS }),
            );
        }
        Some(TextOption::Detailed {
            max_characters: Some(max),
        }) => {
            obj.insert("text".into(), json!({ "maxCharacters": max }));
        }
        Some(TextOption::Detailed {
            max_characters: None,
        }) => {
            obj.insert("text".into(), Value::Bool(true));
        }
        Some(TextOption::Enabled(false)) | None => {}
    }

    if matches!(c.highlights, Some(true)) {
        obj.insert("highlights".into(), Value::Bool(true));
    }

    match &c.summary {
        Some(SummaryOption::Enabled(true)) => {
            obj.insert("summary".into(), Value::Bool(true));
        }
        Some(SummaryOption::Guided { query: Some(q) }) if !q.is_empty() => {
            obj.insert("summary".into(), json!({ "query": q }));
        }
        Some(SummaryOption::Guided { query: _ }) => {
            obj.insert("summary".into(), Value::Bool(true));
        }
        Some(SummaryOption::Enabled(false)) | None => {}
    }

    if obj.is_empty() {
        return json!({
            "highlights": true,
            "text": { "maxCharacters": DEFAULT_TEXT_MAX_CHARS }
        });
    }
    Value::Object(obj)
}

#[async_trait]
impl Tool for ExaSearchTool {
    fn name(&self) -> &str {
        "exa_search"
    }

    fn description(&self) -> &str {
        "AI-powered web search via Exa. Returns ranked URLs with optional highlights, full text, and summaries. Requires EXA_API_KEY."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["query"],
            "properties": {
                "intent": super::intent_schema_property(),
                "query": {
                    "type": "string",
                    "description": "Search query. Natural language works well for neural/auto search."
                },
                "num_results": {
                    "type": "integer",
                    "description": "Max results (1-25, default 8)."
                },
                "type": {
                    "type": "string",
                    "enum": ["auto", "neural", "fast", "deep-lite", "deep", "deep-reasoning", "instant"],
                    "description": "Search type. 'auto' picks the best mode."
                },
                "category": {
                    "type": "string",
                    "enum": ["company", "research paper", "news", "personal site", "financial report", "people"],
                    "description": "Restrict results to a category."
                },
                "include_domains": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Only return results from these domains."
                },
                "exclude_domains": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Skip results from these domains."
                },
                "start_published_date": {
                    "type": "string",
                    "description": "ISO 8601 date; only include results published on/after."
                },
                "end_published_date": {
                    "type": "string",
                    "description": "ISO 8601 date; only include results published on/before."
                },
                "user_location": {
                    "type": "string",
                    "description": "Two-letter ISO country code (e.g. 'US')."
                },
                "contents": {
                    "type": "object",
                    "description": "Per-result content to retrieve.",
                    "properties": {
                        "text": {
                            "description": "Full page text. true, or { max_characters }.",
                            "oneOf": [
                                { "type": "boolean" },
                                {
                                    "type": "object",
                                    "properties": {
                                        "max_characters": { "type": "integer" }
                                    }
                                }
                            ]
                        },
                        "highlights": {
                            "type": "boolean",
                            "description": "Return relevance-ranked snippet highlights."
                        },
                        "summary": {
                            "description": "LLM summary. true, or { query } to steer it.",
                            "oneOf": [
                                { "type": "boolean" },
                                {
                                    "type": "object",
                                    "properties": {
                                        "query": { "type": "string" }
                                    }
                                }
                            ]
                        }
                    }
                }
            }
        })
    }

    async fn execute(&self, input: Value, _ctx: ToolContext) -> Result<ToolOutput> {
        let params: ExaSearchInput = serde_json::from_value(input)?;

        let api_key = std::env::var(EXA_API_KEY_ENV).map_err(|_| {
            anyhow::anyhow!(
                "EXA_API_KEY environment variable is not set. Get a key at https://exa.ai and export EXA_API_KEY."
            )
        })?;

        let num_results = params
            .num_results
            .unwrap_or(DEFAULT_NUM_RESULTS)
            .clamp(1, MAX_NUM_RESULTS);

        let body = ExaRequestBody {
            query: &params.query,
            num_results,
            search_type: params.search_type.as_deref(),
            category: params.category.as_deref(),
            include_domains: params.include_domains.as_deref(),
            exclude_domains: params.exclude_domains.as_deref(),
            start_published_date: params.start_published_date.as_deref(),
            end_published_date: params.end_published_date.as_deref(),
            user_location: params.user_location.as_deref(),
            contents: build_contents_value(&params.contents),
        };

        let response = self
            .client
            .post(EXA_SEARCH_URL)
            .header("x-api-key", api_key)
            .header(EXA_INTEGRATION_HEADER, EXA_INTEGRATION_NAME)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .json(&body)
            .send()
            .await
            .context("failed to reach Exa API")?;

        let status = response.status();
        let body_text = response.text().await.context("failed to read Exa response")?;

        if !status.is_success() {
            return Err(anyhow::anyhow!(
                "Exa search failed (HTTP {}): {}",
                status,
                body_text
            ));
        }

        let parsed: ExaResponse = serde_json::from_str(&body_text)
            .with_context(|| format!("failed to parse Exa response: {body_text}"))?;

        Ok(ToolOutput::new(format_results(&params.query, &parsed)))
    }
}

fn format_results(query: &str, response: &ExaResponse) -> String {
    if response.results.is_empty() {
        return format!("No Exa results for: {query}");
    }

    let mut out = format!("Exa results for: {query}\n\n");
    for (i, r) in response.results.iter().enumerate() {
        let title = r.title.as_deref().unwrap_or("(no title)");
        let url = r.url.as_deref().unwrap_or("");
        out.push_str(&format!("{}. **{}**\n   {}\n", i + 1, title, url));

        let mut meta = Vec::new();
        if let Some(date) = &r.published_date {
            meta.push(format!("published: {date}"));
        }
        if let Some(author) = &r.author {
            meta.push(format!("author: {author}"));
        }
        if let Some(score) = r.score {
            meta.push(format!("score: {score:.3}"));
        }
        if !meta.is_empty() {
            out.push_str(&format!("   ({})\n", meta.join(" · ")));
        }

        if let Some(snippet) = r.snippet() {
            out.push_str(&format!("   {snippet}\n"));
        }
        out.push('\n');
    }
    out
}

#[cfg(test)]
#[path = "exa_search_tests.rs"]
mod exa_search_tests;
