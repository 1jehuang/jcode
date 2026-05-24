pub use jcode_core::util::*;

/// Read an HTTP error body without hiding failures behind an empty string.
///
/// This is useful after a non-success status when the response is about to be
/// converted into an error. If reading the body itself fails, the returned text
/// preserves that failure so callers can include it in their error message.
pub async fn http_error_body(response: reqwest::Response, context: &str) -> String {
    match response.text().await {
        Ok(body) => body,
        Err(err) => format!("<failed to read {context} response body: {err}>"),
    }
}

/// Format an anyhow error including its full cause chain.
///
/// This preserves actionable upstream details such as HTTP status/body instead of
/// only showing the outermost context message.
pub fn format_error_chain(err: &anyhow::Error) -> String {
    let mut parts = Vec::new();
    for cause in err.chain() {
        let text = cause.to_string();
        let trimmed = text.trim();
        if trimmed.is_empty() {
            continue;
        }
        if parts.last().is_some_and(|prev: &String| prev == trimmed) {
            continue;
        }
        parts.push(trimmed.to_string());
    }

    match parts.len() {
        0 => "unknown error".to_string(),
        1 => parts.remove(0),
        _ => parts.join(": "),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_error_chain_includes_nested_causes() {
        let err =
            anyhow::anyhow!("HTTP 400: invalid argument").context("Gemini generateContent failed");
        assert_eq!(
            format_error_chain(&err),
            "Gemini generateContent failed: HTTP 400: invalid argument"
        );
    }

    #[test]
    fn test_format_error_chain_deduplicates_repeated_messages() {
        let err = anyhow::anyhow!("same").context("same");
        assert_eq!(format_error_chain(&err), "same");
    }
}
