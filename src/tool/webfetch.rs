use super::{Tool, ToolContext, ToolOutput};
use anyhow::Result;
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};
use std::time::Duration;

const MAX_SIZE: usize = 5 * 1024 * 1024; // 5MB
const DEFAULT_TIMEOUT: u64 = 30;
const MAX_TIMEOUT: u64 = 120;

pub struct WebFetchTool {
    client: reqwest::Client,
}

impl WebFetchTool {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::builder()
                .user_agent("Mozilla/5.0 (compatible; JCode/1.0)")
                .build()
                .unwrap_or_default(),
        }
    }
}

#[derive(Deserialize)]
struct WebFetchInput {
    url: String,
    #[serde(default)]
    format: Option<String>,
    #[serde(default)]
    timeout: Option<u64>,
}

#[async_trait]
impl Tool for WebFetchTool {
    fn name(&self) -> &str {
        "webfetch"
    }

    fn description(&self) -> &str {
        "Fetch content from a URL. Returns the page content as text, markdown, or HTML. \
         Useful for reading documentation, API responses, or web pages."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["url"],
            "properties": {
                "url": {
                    "type": "string",
                    "description": "The URL to fetch (must start with http:// or https://)"
                },
                "format": {
                    "type": "string",
                    "enum": ["text", "markdown", "html"],
                    "description": "Output format (default: markdown)"
                },
                "timeout": {
                    "type": "integer",
                    "description": "Timeout in seconds (default: 30, max: 120)"
                }
            }
        })
    }

    async fn execute(&self, input: Value, _ctx: ToolContext) -> Result<ToolOutput> {
        let params: WebFetchInput = serde_json::from_value(input)?;

        // Validate URL
        if !params.url.starts_with("http://") && !params.url.starts_with("https://") {
            return Err(anyhow::anyhow!(
                "URL must start with http:// or https://"
            ));
        }

        let timeout = params.timeout.unwrap_or(DEFAULT_TIMEOUT).min(MAX_TIMEOUT);
        let format = params.format.as_deref().unwrap_or("markdown");

        let response = self
            .client
            .get(&params.url)
            .timeout(Duration::from_secs(timeout))
            .send()
            .await?;

        let status = response.status();
        if !status.is_success() {
            return Err(anyhow::anyhow!("HTTP error: {}", status));
        }

        // Check content length
        if let Some(len) = response.content_length() {
            if len as usize > MAX_SIZE {
                return Err(anyhow::anyhow!(
                    "Response too large: {} bytes (max {} bytes)",
                    len,
                    MAX_SIZE
                ));
            }
        }

        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();

        let body = response.text().await?;

        // Truncate if too large
        let body = if body.len() > MAX_SIZE {
            format!(
                "{}...\n\n(truncated, showing first {} bytes)",
                &body[..MAX_SIZE],
                MAX_SIZE
            )
        } else {
            body
        };

        // Format output
        let output = match format {
            "html" => body,
            "text" => html_to_text(&body),
            "markdown" | _ => {
                if content_type.contains("text/html") {
                    html_to_markdown(&body)
                } else {
                    body
                }
            }
        };

        Ok(ToolOutput::new(format!(
            "Fetched {} ({} bytes)\n\n{}",
            params.url,
            output.len(),
            output
        )))
    }
}

fn html_to_text(html: &str) -> String {
    // Simple HTML to text conversion
    let mut text = html.to_string();

    // Remove script and style tags
    let script_re = regex::Regex::new(r"(?is)<script[^>]*>.*?</script>").unwrap();
    let style_re = regex::Regex::new(r"(?is)<style[^>]*>.*?</style>").unwrap();
    text = script_re.replace_all(&text, "").to_string();
    text = style_re.replace_all(&text, "").to_string();

    // Replace common elements
    text = text.replace("<br>", "\n");
    text = text.replace("<br/>", "\n");
    text = text.replace("<br />", "\n");
    text = text.replace("</p>", "\n\n");
    text = text.replace("</div>", "\n");
    text = text.replace("</li>", "\n");
    text = text.replace("</tr>", "\n");

    // Remove all remaining tags
    let tag_re = regex::Regex::new(r"<[^>]+>").unwrap();
    text = tag_re.replace_all(&text, "").to_string();

    // Decode common HTML entities
    text = text.replace("&nbsp;", " ");
    text = text.replace("&lt;", "<");
    text = text.replace("&gt;", ">");
    text = text.replace("&amp;", "&");
    text = text.replace("&quot;", "\"");
    text = text.replace("&#39;", "'");

    // Clean up whitespace
    let whitespace_re = regex::Regex::new(r"\n\s*\n\s*\n").unwrap();
    text = whitespace_re.replace_all(&text, "\n\n").to_string();

    text.trim().to_string()
}

fn html_to_markdown(html: &str) -> String {
    // Simple HTML to Markdown conversion
    let mut md = html.to_string();

    // Remove script and style tags
    let script_re = regex::Regex::new(r"(?is)<script[^>]*>.*?</script>").unwrap();
    let style_re = regex::Regex::new(r"(?is)<style[^>]*>.*?</style>").unwrap();
    md = script_re.replace_all(&md, "").to_string();
    md = style_re.replace_all(&md, "").to_string();

    // Convert headers
    for i in 1..=6 {
        let h_open = regex::Regex::new(&format!(r"(?i)<h{}[^>]*>", i)).unwrap();
        let h_close = regex::Regex::new(&format!(r"(?i)</h{}>", i)).unwrap();
        let prefix = "#".repeat(i);
        md = h_open.replace_all(&md, &format!("\n{} ", prefix)).to_string();
        md = h_close.replace_all(&md, "\n").to_string();
    }

    // Convert links
    let link_re = regex::Regex::new(r#"(?i)<a[^>]*href=["']([^"']+)["'][^>]*>([^<]*)</a>"#).unwrap();
    md = link_re.replace_all(&md, "[$2]($1)").to_string();

    // Convert bold/strong
    let strong_re = regex::Regex::new(r"(?i)<(?:strong|b)>([^<]*)</(?:strong|b)>").unwrap();
    md = strong_re.replace_all(&md, "**$1**").to_string();

    // Convert italic/em
    let em_re = regex::Regex::new(r"(?i)<(?:em|i)>([^<]*)</(?:em|i)>").unwrap();
    md = em_re.replace_all(&md, "*$1*").to_string();

    // Convert code
    let code_re = regex::Regex::new(r"(?i)<code>([^<]*)</code>").unwrap();
    md = code_re.replace_all(&md, "`$1`").to_string();

    // Convert pre/code blocks
    let pre_re = regex::Regex::new(r"(?is)<pre[^>]*><code[^>]*>(.+?)</code></pre>").unwrap();
    md = pre_re.replace_all(&md, "\n```\n$1\n```\n").to_string();

    // Convert lists
    let li_re = regex::Regex::new(r"(?i)<li[^>]*>").unwrap();
    md = li_re.replace_all(&md, "\n- ").to_string();

    // Convert paragraphs and breaks
    md = md.replace("<br>", "\n");
    md = md.replace("<br/>", "\n");
    md = md.replace("<br />", "\n");
    md = md.replace("</p>", "\n\n");

    // Remove remaining tags
    let tag_re = regex::Regex::new(r"<[^>]+>").unwrap();
    md = tag_re.replace_all(&md, "").to_string();

    // Decode HTML entities
    md = md.replace("&nbsp;", " ");
    md = md.replace("&lt;", "<");
    md = md.replace("&gt;", ">");
    md = md.replace("&amp;", "&");
    md = md.replace("&quot;", "\"");
    md = md.replace("&#39;", "'");

    // Clean up whitespace
    let whitespace_re = regex::Regex::new(r"\n\s*\n\s*\n").unwrap();
    md = whitespace_re.replace_all(&md, "\n\n").to_string();

    md.trim().to_string()
}
