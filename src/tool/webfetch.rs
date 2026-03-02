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
            return Err(anyhow::anyhow!("URL must start with http:// or https://"));
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

mod html_regex {
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

    static_regex!(script, r"(?is)<script[^>]*>.*?</script>");
    static_regex!(style, r"(?is)<style[^>]*>.*?</style>");
    static_regex!(tag, r"<[^>]+>");
    static_regex!(whitespace, r"\n\s*\n\s*\n");
    static_regex!(link, r#"(?i)<a[^>]*href=["']([^"']+)["'][^>]*>([^<]*)</a>"#);
    static_regex!(strong, r"(?i)<(?:strong|b)>([^<]*)</(?:strong|b)>");
    static_regex!(em, r"(?i)<(?:em|i)>([^<]*)</(?:em|i)>");
    static_regex!(code, r"(?i)<code>([^<]*)</code>");
    static_regex!(pre_code, r"(?is)<pre[^>]*><code[^>]*>(.+?)</code></pre>");
    static_regex!(li, r"(?i)<li[^>]*>");

    static H_OPEN: OnceLock<[Regex; 6]> = OnceLock::new();
    static H_CLOSE: OnceLock<[Regex; 6]> = OnceLock::new();

    pub fn h_open() -> &'static [Regex; 6] {
        H_OPEN.get_or_init(|| {
            std::array::from_fn(|i| {
                Regex::new(&format!(r"(?i)<h{}[^>]*>", i + 1)).unwrap()
            })
        })
    }

    pub fn h_close() -> &'static [Regex; 6] {
        H_CLOSE.get_or_init(|| {
            std::array::from_fn(|i| {
                Regex::new(&format!(r"(?i)</h{}>", i + 1)).unwrap()
            })
        })
    }
}

fn html_to_text(html: &str) -> String {
    let mut text = html.to_string();

    text = html_regex::script().replace_all(&text, "").to_string();
    text = html_regex::style().replace_all(&text, "").to_string();

    text = text.replace("<br>", "\n");
    text = text.replace("<br/>", "\n");
    text = text.replace("<br />", "\n");
    text = text.replace("</p>", "\n\n");
    text = text.replace("</div>", "\n");
    text = text.replace("</li>", "\n");
    text = text.replace("</tr>", "\n");

    text = html_regex::tag().replace_all(&text, "").to_string();

    text = text.replace("&nbsp;", " ");
    text = text.replace("&lt;", "<");
    text = text.replace("&gt;", ">");
    text = text.replace("&amp;", "&");
    text = text.replace("&quot;", "\"");
    text = text.replace("&#39;", "'");

    text = html_regex::whitespace().replace_all(&text, "\n\n").to_string();

    text.trim().to_string()
}

fn html_to_markdown(html: &str) -> String {
    let mut md = html.to_string();

    md = html_regex::script().replace_all(&md, "").to_string();
    md = html_regex::style().replace_all(&md, "").to_string();

    let h_open = html_regex::h_open();
    let h_close = html_regex::h_close();
    for i in 0..6 {
        let prefix = "#".repeat(i + 1);
        md = h_open[i]
            .replace_all(&md, &format!("\n{} ", prefix))
            .to_string();
        md = h_close[i].replace_all(&md, "\n").to_string();
    }

    md = html_regex::link().replace_all(&md, "[$2]($1)").to_string();
    md = html_regex::strong().replace_all(&md, "**$1**").to_string();
    md = html_regex::em().replace_all(&md, "*$1*").to_string();
    md = html_regex::code().replace_all(&md, "`$1`").to_string();
    md = html_regex::pre_code().replace_all(&md, "\n```\n$1\n```\n").to_string();
    md = html_regex::li().replace_all(&md, "\n- ").to_string();

    md = md.replace("<br>", "\n");
    md = md.replace("<br/>", "\n");
    md = md.replace("<br />", "\n");
    md = md.replace("</p>", "\n\n");

    md = html_regex::tag().replace_all(&md, "").to_string();

    md = md.replace("&nbsp;", " ");
    md = md.replace("&lt;", "<");
    md = md.replace("&gt;", ">");
    md = md.replace("&amp;", "&");
    md = md.replace("&quot;", "\"");
    md = md.replace("&#39;", "'");

    md = html_regex::whitespace().replace_all(&md, "\n\n").to_string();

    md.trim().to_string()
}
