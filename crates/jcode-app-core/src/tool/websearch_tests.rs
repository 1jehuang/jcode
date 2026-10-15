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
    let body: Value = serde_json::from_slice(request.body().unwrap().as_bytes().unwrap()).unwrap();
    assert_eq!(body, json!({"search": "new", "options": {"safe": true}}));
    config.count_param = Some("limit".into());
    config.params.insert("static".into(), "yes".into());
    let request = custom_request(&reqwest::Client::new(), &config, "new", 4, |_| None).unwrap();
    let body: Value = serde_json::from_slice(request.body().unwrap().as_bytes().unwrap()).unwrap();
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
                request.starts_with("GET /search?token=secret&count=2&q=rust+%26+json HTTP/1.1")
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
                    tokio::time::timeout(std::time::Duration::from_millis(200), listener.accept())
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
