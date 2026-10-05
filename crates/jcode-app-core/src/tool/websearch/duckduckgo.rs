use super::{
    SearchResult, WebSearchTool,
    html::{detect_anti_bot_page, html_decode, search_regex},
};
use anyhow::Result;

impl WebSearchTool {
    pub(super) async fn search_duckduckgo(
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
}

pub(super) fn parse_ddg_results(html: &str, max_results: usize) -> Vec<SearchResult> {
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

pub(super) fn decode_ddg_url(url: &str) -> String {
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
