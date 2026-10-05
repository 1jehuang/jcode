use super::bing::BingApiResponse;
use super::bing::{parse_bing_api_results, parse_bing_html_results};
use super::duckduckgo::parse_ddg_results;
use super::html::detect_anti_bot_page;
use super::searxng::{SearxngResponse, parse_searxng_results};
use super::*;

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

#[test]
fn extra_headers_reject_invalid_names_and_values() {
    let mut headers = std::collections::HashMap::new();
    headers.insert("bad header".to_string(), "x".to_string());
    let request = reqwest::Client::new().get("http://127.0.0.1/");
    assert!(apply_extra_headers(request, &headers).is_err());

    let mut headers = std::collections::HashMap::new();
    headers.insert("x-ok".to_string(), "bad\nvalue".to_string());
    let request = reqwest::Client::new().get("http://127.0.0.1/");
    assert!(apply_extra_headers(request, &headers).is_err());
}

#[tokio::test]
async fn extra_headers_are_sent_on_the_wire() {
    use std::io::{Read, Write};

    // Minimal one-shot HTTP server that captures the raw request headers.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .unwrap();
        let mut raw = Vec::new();
        let mut buf = [0u8; 4096];
        while !raw.windows(4).any(|w| w == b"\r\n\r\n") {
            match stream.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => raw.extend_from_slice(&buf[..n]),
            }
        }
        let body = "{\"results\":[]}";
        let response = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        stream.write_all(response.as_bytes()).unwrap();
        String::from_utf8_lossy(&raw).to_string()
    });

    let mut headers = std::collections::HashMap::new();
    headers.insert("Authorization".to_string(), "Bearer test-token".to_string());
    let request = apply_extra_headers(
        reqwest::Client::new()
            .get(format!("http://{addr}/search"))
            .query(&[("q", "test"), ("format", "json")]),
        &headers,
    )
    .unwrap();
    let response = request.send().await.unwrap();
    assert!(response.status().is_success());

    let raw = server.join().unwrap().to_lowercase();
    assert!(
        raw.contains("authorization: bearer test-token"),
        "configured header missing from request:\n{raw}"
    );
}

#[test]
fn searxng_insecure_remote_http_detection() {
    use super::searxng::is_insecure_remote_http;

    // Plaintext HTTP to a remote host is insecure for credentials.
    assert!(is_insecure_remote_http("http://searx.example.org"));
    assert!(is_insecure_remote_http("http://192.168.1.10:8080"));
    // Loopback plaintext HTTP stays allowed (local instances).
    assert!(!is_insecure_remote_http("http://127.0.0.1:8080"));
    assert!(!is_insecure_remote_http("http://localhost:8080"));
    assert!(!is_insecure_remote_http("http://[::1]:8080"));
    assert!(!is_insecure_remote_http("http://searx.localhost"));
    // HTTPS is fine anywhere.
    assert!(!is_insecure_remote_http("https://searx.example.org"));
}
