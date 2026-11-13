use super::*;

const ENDPOINTS: (&str, &str) = (
    "https://public.test/v1/search/public",
    "https://keyed.test/v1/search",
);

// Tests talk to a local server, so ignore HTTP_PROXY/ALL_PROXY from the environment.
fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap()
}

fn body(req: &reqwest::Request) -> Value {
    serde_json::from_slice(req.body().unwrap().as_bytes().unwrap()).unwrap()
}

#[test]
fn keenable_request_is_keyless_without_a_key() {
    let client = client();
    let req = request(&client, ENDPOINTS, "rust & c++", 3, "TEST_KEY", |name| {
        assert_eq!(name, "TEST_KEY");
        None
    })
    .unwrap();
    assert_eq!(req.method(), reqwest::Method::POST);
    assert_eq!(req.url().as_str(), ENDPOINTS.0);
    assert_eq!(req.headers()["x-keenable-title"], "jcode");
    assert!(req.headers().get("x-api-key").is_none());
    assert!(req.headers().get("authorization").is_none());
    assert_eq!(body(&req), json!({"query": "rust & c++", "max_results": 3}));

    let blank = request(&client, ENDPOINTS, "q", 3, "TEST_KEY", |_| {
        Some("  ".into())
    })
    .unwrap();
    assert_eq!(blank.url().as_str(), ENDPOINTS.0);
    let unset = request(&client, ENDPOINTS, "q", 3, "", |_| {
        panic!("must not read environment")
    })
    .unwrap();
    assert_eq!(unset.url().as_str(), ENDPOINTS.0);
}

#[test]
fn keenable_request_uses_sensitive_key_header_when_configured() {
    let client = client();
    let req = request(&client, ENDPOINTS, "q", 100, "TEST_KEY", |_| {
        Some(" test-secret ".into())
    })
    .unwrap();
    assert_eq!(req.url().as_str(), ENDPOINTS.1);
    assert_eq!(req.headers()["x-api-key"], "test-secret");
    assert!(req.headers()["x-api-key"].is_sensitive());
    assert!(!format!("{req:?}").contains("test-secret"));
    assert!(!req.url().as_str().contains("test-secret"));
    assert_eq!(body(&req)["max_results"], 20);

    let error = request(&client, ENDPOINTS, "private-query", 2, "TEST_KEY", |_| {
        Some("sentinel\ninvalid".into())
    })
    .unwrap_err()
    .to_string();
    assert!(error.contains("Keenable search"));
    assert!(!error.contains("sentinel"));
    assert!(!error.contains("private-query"));
}

#[tokio::test]
async fn keenable_transport_errors_do_not_expose_request_data() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);
    let client = client();
    let public = format!("http://{address}/credential-sentinel");
    let req = request(
        &client,
        (&public, &public),
        "private-query",
        2,
        "TEST_KEY",
        |_| Some("test-secret".into()),
    )
    .unwrap();
    let error = format!("{:#}", execute(&client, req, 2).await.unwrap_err());
    assert_eq!(error, "Keenable search request failed");
}

#[test]
fn keenable_maps_results_and_skips_unusable_rows() {
    let body = json!({"query": "q", "results": [
        {"url": ""}, {"url": null}, {"url": "file:///etc/passwd", "title": "File"},
        {"url": "https://one.test", "title": "One", "snippet": "First", "description": "ignored"},
        {"url": "https://two.test", "description": "Second"},
        {"url": "https://three.test"}
    ]});
    let results = parse_results(&body, 2).unwrap();
    assert_eq!(results.len(), 2);
    assert_eq!(results[0].title, "One");
    assert_eq!(results[0].url, "https://one.test");
    assert_eq!(results[0].snippet, "First");
    assert_eq!(results[1].title, "https://two.test");
    assert_eq!(results[1].snippet, "Second");
    assert!(
        parse_results(&json!({"results": []}), 5)
            .unwrap()
            .is_empty()
    );
    assert!(parse_results(&json!({"data": []}), 5).is_err());
}

#[tokio::test]
async fn keenable_local_http_success_errors_and_redirect_safety() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    for (status, body) in [
        (
            200,
            r#"{"results":[{"url":"https://rust.test","title":"Rust","snippet":"JSON"}]}"#,
        ),
        (200, "credential-sentinel"),
        (200, r#"{"error":"credential-sentinel"}"#),
        (401, "credential-sentinel"),
        (429, "credential-sentinel"),
        (500, "credential-sentinel"),
        (302, "credential-sentinel"),
    ] {
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
                if let Some(end) = received.windows(4).position(|w| w == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&received[..end]).to_lowercase();
                    let length: usize = headers
                        .lines()
                        .find_map(|line| line.strip_prefix("content-length: "))
                        .unwrap()
                        .parse()
                        .unwrap();
                    if received.len() >= end + 4 + length {
                        break;
                    }
                }
            }
            let request = String::from_utf8(received).unwrap();
            assert!(request.starts_with("POST /keyed HTTP/1.1"));
            assert!(request.to_lowercase().contains("x-api-key: test-secret"));
            let response = format!(
                "HTTP/1.1 {status} Test\r\nLocation: http://{address}/leak\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).await.unwrap();
            drop(stream);
            // Neither redirects nor failures should trigger a second request.
            assert!(
                tokio::time::timeout(std::time::Duration::from_millis(100), listener.accept())
                    .await
                    .is_err()
            );
        });
        let client = client();
        let public = format!("http://{address}/public");
        let keyed = format!("http://{address}/keyed");
        let req = request(
            &client,
            (&public, &keyed),
            "private-query",
            2,
            "TEST_KEY",
            |_| Some("test-secret".into()),
        )
        .unwrap();
        let result = execute(&client, req, 2).await;
        if status == 200 && body.starts_with("{\"results\"") {
            let results = result.unwrap();
            assert_eq!(results[0].title, "Rust");
            assert_eq!(results[0].url, "https://rust.test");
            assert_eq!(results[0].snippet, "JSON");
        } else {
            let error = format!("{:#}", result.unwrap_err());
            assert!(!error.contains("credential-sentinel"));
            assert!(!error.contains("test-secret"));
            assert!(!error.contains("private-query"));
            if status != 200 {
                assert!(error.contains(&status.to_string()));
            }
        }
        server.await.unwrap();
    }
}
