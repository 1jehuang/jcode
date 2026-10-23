use super::*;

#[test]
fn kagi_request_uses_v1_post_and_sensitive_bearer_header() {
    let client = client().unwrap();
    let request = request(&client, ENDPOINT, "rust & c++", 3, "TEST_KEY", |name| {
        assert_eq!(name, "TEST_KEY");
        Some("test-secret".into())
    })
    .unwrap();
    assert_eq!(request.method(), reqwest::Method::POST);
    assert_eq!(request.url().as_str(), ENDPOINT);
    assert_eq!(request.headers()["authorization"], "Bearer test-secret");
    assert!(request.headers()["authorization"].is_sensitive());
    assert!(!format!("{request:?}").contains("test-secret"));
    let body: Value = serde_json::from_slice(request.body().unwrap().as_bytes().unwrap()).unwrap();
    assert_eq!(
        body,
        json!({"query":"rust & c++", "workflow":"search", "limit":3})
    );
}

#[test]
fn kagi_request_caps_result_limit() {
    let client = client().unwrap();
    let request = request(&client, ENDPOINT, "q", 100, "TEST_KEY", |_| {
        Some("fake".into())
    })
    .unwrap();
    let body: Value = serde_json::from_slice(request.body().unwrap().as_bytes().unwrap()).unwrap();
    assert_eq!(body["limit"], 20);
}

#[tokio::test]
async fn kagi_transport_errors_do_not_expose_request_data() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);
    let client = client().unwrap();
    let request = request(
        &client,
        &format!("http://{address}/credential-sentinel"),
        "private-query",
        2,
        "TEST_KEY",
        |_| Some("test-secret".into()),
    )
    .unwrap();
    let error = format!("{:#}", execute(&client, request, 2).await.unwrap_err());
    assert_eq!(error, "Kagi search request failed");
}

#[test]
fn kagi_credentials_fail_closed_without_leaking_values() {
    let client = client().unwrap();
    for key in [None, Some(" ".into()), Some("sentinel\ninvalid".into())] {
        let error = request(&client, ENDPOINT, "private-query", 2, "TEST_KEY", |_| key)
            .unwrap_err()
            .to_string();
        assert!(error.contains("Kagi search"));
        assert!(!error.contains("sentinel"));
        assert!(!error.contains("private-query"));
    }
    assert!(
        request(&client, ENDPOINT, "q", 2, "", |_| panic!(
            "must not read environment"
        ))
        .is_err()
    );
}

#[test]
fn kagi_maps_v1_results_and_limits_valid_rows() {
    let body = json!({"data":{"search":[
        {"url":""}, {"url":null}, {"url":"https://one.test", "title":"One", "snippet":"First"},
        {"url":"https://two.test"}, {"url":"https://three.test"}
    ], "image":[{"url":"https://ignored.test"}]}});
    let results = parse_results(&body, 2).unwrap();
    assert_eq!(results.len(), 2);
    assert_eq!(results[0].title, "One");
    assert_eq!(results[0].snippet, "First");
    assert_eq!(results[1].title, "https://two.test");
    assert_eq!(results[1].snippet, "");
    assert!(parse_results(&body, 0).unwrap().is_empty());
    assert!(
        parse_results(&json!({"data":{"search":[]}}), 2)
            .unwrap()
            .is_empty()
    );
    for invalid in [
        json!({}),
        json!({"data":[]}),
        json!({"data":{"search":null}}),
        json!({"error":"sentinel"}),
    ] {
        let error = parse_results(&invalid, 2).unwrap_err().to_string();
        assert!(!error.contains("sentinel"));
    }
}

#[tokio::test]
async fn kagi_zero_results_never_resolves_credentials_or_sends_request() {
    assert!(search("q", 0, "").await.unwrap().is_empty());
}

#[tokio::test]
async fn kagi_local_http_success_errors_and_redirect_safety() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    for (status, body) in [
        (
            200,
            r#"{"data":{"search":[{"url":"https://rust.test","title":"Rust","snippet":"JSON"}]}}"#,
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
            assert!(request.starts_with("POST /search HTTP/1.1"));
            assert!(
                request
                    .to_lowercase()
                    .contains("authorization: bearer test-secret")
            );
            let response = format!(
                "HTTP/1.1 {status} Test\r\nLocation: http://{address}/leak\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).await.unwrap();
            drop(stream);
            // Neither redirects nor failures should trigger an extra paid request.
            assert!(
                tokio::time::timeout(std::time::Duration::from_millis(100), listener.accept())
                    .await
                    .is_err()
            );
        });
        let client = client().unwrap();
        let request = request(
            &client,
            &format!("http://{address}/search"),
            "private-query",
            2,
            "TEST_KEY",
            |_| Some("test-secret".into()),
        )
        .unwrap();
        let result = execute(&client, request, 2).await;
        if status == 200 && body.starts_with("{\"data\"") {
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
