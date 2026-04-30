use super::*;

#[test]
fn parses_full_response() {
    let body = r#"{
        "requestId": "req_1",
        "results": [
            {
                "id": "abc",
                "url": "https://example.com/a",
                "title": "Example A",
                "publishedDate": "2026-01-15T00:00:00Z",
                "author": "Jane Doe",
                "score": 0.87,
                "text": "full body text",
                "highlights": ["snippet one", "snippet two"],
                "summary": "concise summary"
            }
        ]
    }"#;

    let parsed: ExaResponse = serde_json::from_str(body).expect("parses");
    assert_eq!(parsed.results.len(), 1);
    let r = &parsed.results[0];
    assert_eq!(r.url.as_deref(), Some("https://example.com/a"));
    assert_eq!(r.title.as_deref(), Some("Example A"));
    assert_eq!(r.author.as_deref(), Some("Jane Doe"));
    assert_eq!(r.published_date.as_deref(), Some("2026-01-15T00:00:00Z"));
    assert!((r.score.unwrap() - 0.87).abs() < 1e-6);
}

#[test]
fn parses_response_with_missing_optional_fields() {
    // Only id/url/title — no text, highlights, summary, publishedDate, author, score.
    let body = r#"{
        "results": [
            { "id": "abc", "url": "https://example.com/a", "title": "Bare result" }
        ]
    }"#;

    let parsed: ExaResponse = serde_json::from_str(body).expect("parses");
    assert_eq!(parsed.results.len(), 1);
    let r = &parsed.results[0];
    assert!(r.text.is_none());
    assert!(r.highlights.is_none());
    assert!(r.summary.is_none());
    assert!(r.snippet().is_none());
}

#[test]
fn snippet_prefers_summary_over_highlights_and_text() {
    let r = ExaResult {
        title: None,
        url: None,
        author: None,
        published_date: None,
        score: None,
        text: Some("ignored body".into()),
        highlights: Some(vec!["ignored highlight".into()]),
        summary: Some("the summary".into()),
    };
    assert_eq!(r.snippet().as_deref(), Some("the summary"));
}

#[test]
fn snippet_falls_back_to_highlights_when_no_summary() {
    let r = ExaResult {
        title: None,
        url: None,
        author: None,
        published_date: None,
        score: None,
        text: Some("ignored body text".into()),
        highlights: Some(vec!["first hit".into(), "second hit".into()]),
        summary: None,
    };
    assert_eq!(r.snippet().as_deref(), Some("first hit … second hit"));
}

#[test]
fn snippet_falls_back_to_text_when_no_summary_or_highlights() {
    let r = ExaResult {
        title: None,
        url: None,
        author: None,
        published_date: None,
        score: None,
        text: Some("just body text".into()),
        highlights: None,
        summary: None,
    };
    assert_eq!(r.snippet().as_deref(), Some("just body text"));
}

#[test]
fn snippet_treats_empty_strings_as_missing() {
    let r = ExaResult {
        title: None,
        url: None,
        author: None,
        published_date: None,
        score: None,
        text: Some("   ".into()),
        highlights: Some(vec!["   ".into(), "".into()]),
        summary: Some("   ".into()),
    };
    assert!(r.snippet().is_none());
}

#[test]
fn snippet_truncates_long_text_fallback() {
    let long_text: String = "a".repeat(800);
    let r = ExaResult {
        title: None,
        url: None,
        author: None,
        published_date: None,
        score: None,
        text: Some(long_text),
        highlights: None,
        summary: None,
    };
    let snippet = r.snippet().unwrap();
    // truncate_chars caps at 400 + ellipsis
    assert!(snippet.ends_with('…'));
    assert_eq!(snippet.chars().count(), 401);
}

#[test]
fn default_contents_includes_highlights_and_text() {
    let v = build_contents_value(&None);
    assert_eq!(v["highlights"], serde_json::Value::Bool(true));
    assert!(v["text"]["maxCharacters"].is_number());
}

#[test]
fn explicit_text_max_characters_is_passed_through() {
    let input = ContentsInput {
        text: Some(TextOption::Detailed {
            max_characters: Some(2500),
        }),
        highlights: None,
        summary: None,
    };
    let v = build_contents_value(&Some(input));
    assert_eq!(v["text"]["maxCharacters"], 2500);
}

#[test]
fn summary_query_is_propagated() {
    let input = ContentsInput {
        text: None,
        highlights: None,
        summary: Some(SummaryOption::Guided {
            query: Some("main findings".into()),
        }),
    };
    let v = build_contents_value(&Some(input));
    assert_eq!(v["summary"]["query"], "main findings");
}

#[test]
fn summary_guided_with_empty_query_collapses_to_bool() {
    let input = ContentsInput {
        text: None,
        highlights: None,
        summary: Some(SummaryOption::Guided { query: None }),
    };
    let v = build_contents_value(&Some(input));
    assert_eq!(v["summary"], serde_json::Value::Bool(true));
}

#[test]
fn format_results_renders_empty_message() {
    let response = ExaResponse { results: vec![] };
    let rendered = format_results("foo", &response);
    assert!(rendered.contains("No Exa results"));
}

#[test]
fn format_results_renders_metadata_and_snippet() {
    let response = ExaResponse {
        results: vec![ExaResult {
            title: Some("Title".into()),
            url: Some("https://example.com".into()),
            author: Some("Author".into()),
            published_date: Some("2026-04-01".into()),
            score: Some(0.5),
            text: None,
            highlights: None,
            summary: Some("a summary".into()),
        }],
    };
    let rendered = format_results("q", &response);
    assert!(rendered.contains("Title"));
    assert!(rendered.contains("https://example.com"));
    assert!(rendered.contains("a summary"));
    assert!(rendered.contains("Author"));
    assert!(rendered.contains("2026-04-01"));
}

#[tokio::test]
async fn execute_errors_when_api_key_missing() {
    // Snapshot env so concurrent tests don't break each other; if a key is set,
    // skip rather than clobber it.
    if std::env::var("EXA_API_KEY").is_ok() {
        return;
    }

    let tool = ExaSearchTool::new();
    let ctx = ToolContext {
        session_id: "t".into(),
        message_id: "t".into(),
        tool_call_id: "t".into(),
        working_dir: None,
        stdin_request_tx: None,
        graceful_shutdown_signal: None,
        execution_mode: super::super::ToolExecutionMode::Direct,
    };
    let err = tool
        .execute(serde_json::json!({ "query": "anything" }), ctx)
        .await
        .expect_err("should fail without API key");
    let msg = format!("{err:#}");
    assert!(msg.contains("EXA_API_KEY"), "unexpected error: {msg}");
}
