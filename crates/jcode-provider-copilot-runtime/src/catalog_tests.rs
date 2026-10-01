use super::*;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

struct CatalogHome {
    previous: Option<std::ffi::OsString>,
    _directory: tempfile::TempDir,
    _lock: std::sync::MutexGuard<'static, ()>,
}

impl CatalogHome {
    fn new() -> Result<Self> {
        let lock = jcode_base::storage::lock_test_env();
        let directory = tempfile::tempdir()?;
        let previous = std::env::var_os("JCODE_HOME");
        jcode_base::env::set_var("JCODE_HOME", directory.path());
        Ok(Self {
            previous,
            _directory: directory,
            _lock: lock,
        })
    }
}

impl Drop for CatalogHome {
    fn drop(&mut self) {
        match self.previous.take() {
            Some(value) => jcode_base::env::set_var("JCODE_HOME", value),
            None => jcode_base::env::remove_var("JCODE_HOME"),
        }
    }
}

async fn read_headers(socket: &mut tokio::net::TcpStream) -> Result<Vec<u8>> {
    let mut request = Vec::new();
    let mut buffer = [0; 4096];
    loop {
        let count = socket.read(&mut buffer).await?;
        anyhow::ensure!(count != 0, "request closed before headers");
        request.extend_from_slice(&buffer[..count]);
        if request.windows(4).any(|part| part == b"\r\n\r\n") {
            return Ok(request);
        }
    }
}

async fn set_host(provider: &CopilotApiProvider, host: String) -> copilot_auth::CopilotApiToken {
    let token = copilot_auth::CopilotApiToken {
        token: "local-token".to_string(),
        expires_at: Utc::now().timestamp() + 3600,
        api_base: host,
    };
    *provider.bearer_token.write().await = Some(token.clone());
    token
}

async fn reply(
    socket: &mut tokio::net::TcpStream,
    status: &str,
    content_type: &str,
    body: &str,
) -> Result<()> {
    socket.write_all(format!(
        "HTTP/1.1 {status}\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}", body.len()
    ).as_bytes()).await?;
    Ok(())
}

async fn completion_text(provider: &CopilotApiProvider) -> Result<String> {
    use futures::StreamExt;
    let messages = vec![make_msg(
        Role::User,
        vec![ContentBlock::Text {
            text: "hello".to_string(),
            cache_control: None,
        }],
    )];
    let mut stream = provider.complete(&messages, &[], "system", None).await?;
    let mut text = String::new();
    while let Some(event) = stream.next().await {
        if let StreamEvent::TextDelta(delta) = event? {
            text.push_str(&delta);
        }
    }
    Ok(text)
}

async fn catalog_server(endpoint: &str) -> Result<(String, tokio::task::JoinHandle<Result<()>>)> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let host = format!("http://{}", listener.local_addr()?);
    let body = json!({"data": [{"id": "future-model", "model_picker_enabled": true,
        "supported_endpoints": [endpoint]}]})
    .to_string();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await?;
        let request = read_headers(&mut socket).await?;
        anyhow::ensure!(
            request.starts_with(b"GET /models HTTP/1.1\r\n"),
            "wrong catalog path"
        );
        socket.write_all(format!("HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}", body.len()).as_bytes()).await?;
        Ok(())
    });
    Ok((host, server))
}

#[tokio::test]
async fn stalled_catalog_bounds_completions_without_queuing_or_repeated_fetches() -> Result<()> {
    let mut provider = make_test_provider(Vec::new());
    provider.client = reqwest::Client::builder().no_proxy().build()?;
    provider.catalog.write().unwrap().source = CatalogSource::None;
    provider.set_model("gpt-5-mini")?;
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    set_host(&provider, format!("http://{}", listener.local_addr()?)).await;
    let provider = Arc::new(provider);
    let (catalog_started, catalog_observed) = tokio::sync::oneshot::channel();
    let fetches = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let count = fetches.clone();
    let server = tokio::spawn(async move {
        let mut catalog_started = Some(catalog_started);
        let mut stalls = tokio::task::JoinSet::new();
        for _ in 0..4 {
            let (mut socket, _) = listener.accept().await?;
            let request = read_headers(&mut socket).await?;
            if request.starts_with(b"GET /models HTTP/1.1\r\n") {
                count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                socket.write_all(b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 1024\r\n\r\n{").await?;
                if let Some(started) = catalog_started.take() {
                    started.send(()).unwrap();
                }
                stalls.spawn(async move {
                    let _socket = socket;
                    std::future::pending::<()>().await;
                });
            } else {
                anyhow::ensure!(
                    request.starts_with(b"POST /chat/completions HTTP/1.1\r\n"),
                    "wrong fallback route"
                );
                reply(&mut socket, "200 OK", "text/event-stream", "data: {\"choices\":[{\"delta\":{\"content\":\"FALLBACK_OK\"}}]}\n\ndata: [DONE]\n\n").await?;
            }
        }
        Ok::<_, anyhow::Error>(())
    });
    let first_provider = provider.clone();
    let first = tokio::spawn(async move { completion_text(&first_provider).await });
    catalog_observed.await?;
    // A turn arriving during discovery must use fallback without waiting for the lock.
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(1), completion_text(&provider)).await??,
        "FALLBACK_OK"
    );
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(4), first).await???,
        "FALLBACK_OK"
    );
    // The next turn must not retry the stalled catalog during its failure cooldown.
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(1), completion_text(&provider)).await??,
        "FALLBACK_OK"
    );
    server.await??;
    assert_eq!(fetches.load(std::sync::atomic::Ordering::SeqCst), 1);
    Ok(())
}

#[tokio::test]
async fn host_change_refreshes_live_routes() -> Result<()> {
    let _home = CatalogHome::new()?;
    let mut provider = make_test_provider(Vec::new());
    provider.client = reqwest::Client::builder().no_proxy().build()?;
    provider.catalog.write().unwrap().source = CatalogSource::None;
    let (first_host, first_server) = catalog_server("/responses").await?;
    let first_bearer = set_host(&provider, first_host).await;
    provider
        .ensure_model_catalog(&first_bearer, "future-model")
        .await;
    first_server.await??;
    assert!(provider.model_uses_responses_api("future-model", &first_bearer.api_base)?);
    let (second_host, second_server) = catalog_server("/chat/completions").await?;
    let second_bearer = set_host(&provider, second_host).await;
    assert!(
        provider
            .model_uses_responses_api("future-model", &second_bearer.api_base)
            .is_err()
    );
    provider
        .ensure_model_catalog(&second_bearer, "future-model")
        .await;
    second_server.await??;
    assert!(!provider.model_uses_responses_api("future-model", &second_bearer.api_base)?);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reader_cannot_drop_catalog_publication() -> Result<()> {
    let _home = CatalogHome::new()?;
    let mut provider = make_test_provider(vec!["old-model".to_string()]);
    provider.client = reqwest::Client::builder().no_proxy().build()?;
    let (host, server) = catalog_server("/responses").await?;
    set_host(&provider, host).await;
    let models = provider.catalog.clone();
    let (held_tx, held_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let reader = std::thread::spawn(move || {
        let _guard = models.read().unwrap();
        held_tx.send(()).unwrap();
        release_rx.recv().unwrap();
    });
    held_rx.await?;
    let release = tokio::spawn(async move {
        server.await??;
        // Hold a real catalog reader during publication, then allow it to finish.
        tokio::time::sleep(Duration::from_millis(100)).await;
        release_tx.send(())?;
        Ok::<_, anyhow::Error>(())
    });
    provider.detect_tier_and_set_default().await;
    release.await??;
    reader.join().unwrap();
    assert_eq!(provider.available_models_display(), vec!["future-model"]);
    let persisted: PersistedCatalog =
        jcode_base::storage::read_json(&CopilotApiProvider::persisted_catalog_path()?)?;
    assert_eq!(persisted.models, vec!["future-model"]);
    Ok(())
}

async fn read_completion(socket: &mut tokio::net::TcpStream, path: &str) -> Result<Value> {
    let mut request = read_headers(socket).await?;
    let boundary = request
        .windows(4)
        .position(|part| part == b"\r\n\r\n")
        .unwrap()
        + 4;
    let headers = std::str::from_utf8(&request[..boundary])?;
    anyhow::ensure!(
        headers.starts_with(&format!("POST {path} HTTP/1.1\r\n")),
        "wrong inference route: {headers}"
    );
    let length: usize = headers
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then_some(value.trim())
        })
        .ok_or_else(|| anyhow::anyhow!("missing request content-length"))?
        .parse()?;
    while request.len() < boundary + length {
        let mut buffer = [0; 4096];
        let count = socket.read(&mut buffer).await?;
        anyhow::ensure!(count != 0, "request body truncated");
        request.extend_from_slice(&buffer[..count]);
    }
    Ok(serde_json::from_slice(
        &request[boundary..boundary + length],
    )?)
}

#[tokio::test]
async fn retry_on_new_host_rebuilds_route_and_payload() -> Result<()> {
    let _home = CatalogHome::new()?;
    let mut provider = make_test_provider(Vec::new());
    provider.client = reqwest::Client::builder().no_proxy().build()?;
    provider.catalog.write().unwrap().source = CatalogSource::None;
    provider.set_model("future-model")?;
    let first = TcpListener::bind("127.0.0.1:0").await?;
    let second = TcpListener::bind("127.0.0.1:0").await?;
    let new_host = format!("http://{}", second.local_addr()?);
    set_host(&provider, format!("http://{}", first.local_addr()?)).await;
    let bearer = provider.bearer_token.clone();
    let old_server = tokio::spawn(async move {
        let (mut catalog, _) = first.accept().await?;
        read_headers(&mut catalog).await?;
        reply(&mut catalog, "200 OK", "application/json", r#"{"data":[{"id":"future-model","model_picker_enabled":true,"supported_endpoints":["/chat/completions"]}]}"#).await?;
        let (mut completion, _) = first.accept().await?;
        let body = read_completion(&mut completion, "/chat/completions").await?;
        assert_eq!(body["messages"][1]["content"], "hello");
        assert!(body.get("input").is_none());
        // A refreshed token changes hosts while this attempt is in flight.
        *bearer.write().await = Some(copilot_auth::CopilotApiToken {
            token: "new-host-token".to_string(),
            expires_at: Utc::now().timestamp() + 3600,
            api_base: new_host,
        });
        reply(
            &mut completion,
            "503 Service Unavailable",
            "application/json",
            "{}",
        )
        .await?;
        Ok::<_, anyhow::Error>(())
    });
    let new_server = tokio::spawn(async move {
        let (mut catalog, _) = second.accept().await?;
        let headers = read_headers(&mut catalog).await?;
        assert!(headers.starts_with(b"GET /models HTTP/1.1\r\n"));
        reply(&mut catalog, "200 OK", "application/json", r#"{"data":[{"id":"future-model","model_picker_enabled":true,"supported_endpoints":["/responses"]}]}"#).await?;
        let (mut completion, _) = second.accept().await?;
        let body = read_completion(&mut completion, "/responses").await?;
        assert_eq!(body["model"], "future-model");
        assert_eq!(body["instructions"], "system");
        assert_eq!(body["input"][0]["content"][0]["text"], "hello");
        assert!(body.get("messages").is_none());
        reply(&mut completion, "200 OK", "text/event-stream", "data: {\"type\":\"response.output_text.delta\",\"delta\":\"NEW_HOST_OK\"}\n\ndata: {\"type\":\"response.completed\",\"response\":{}}\n\n").await?;
        Ok::<_, anyhow::Error>(())
    });
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(10), completion_text(&provider)).await??,
        "NEW_HOST_OK"
    );
    old_server.await??;
    new_server.await??;
    Ok(())
}

#[tokio::test]
async fn failed_catalog_can_be_explicitly_refreshed_during_cooldown() -> Result<()> {
    let _home = CatalogHome::new()?;
    let mut provider = make_test_provider(Vec::new());
    provider.client = reqwest::Client::builder().no_proxy().build()?;
    provider.catalog.write().unwrap().source = CatalogSource::None;
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let bearer = set_host(&provider, format!("http://{}", listener.local_addr()?)).await;
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await?;
        read_headers(&mut socket).await?;
        reply(
            &mut socket,
            "503 Service Unavailable",
            "application/json",
            "{}",
        )
        .await?;
        let (mut socket, _) = listener.accept().await?;
        read_headers(&mut socket).await?;
        reply(&mut socket, "200 OK", "application/json", r#"{"data":[{"id":"future-model","model_picker_enabled":true,"supported_endpoints":["/responses"]}]}"#).await?;
        Ok::<_, anyhow::Error>(())
    });
    provider.ensure_model_catalog(&bearer, "future-model").await;
    provider.ensure_model_catalog(&bearer, "future-model").await;
    assert!(
        provider
            .model_uses_responses_api("future-model", &bearer.api_base)
            .is_err()
    );
    provider.detect_tier_and_set_default().await;
    server.await??;
    assert!(provider.model_uses_responses_api("future-model", &bearer.api_base)?);
    assert_eq!(provider.available_models_display(), vec!["future-model"]);
    Ok(())
}

#[tokio::test]
async fn concurrent_responses_only_turns_share_discovery_before_sending() -> Result<()> {
    let _home = CatalogHome::new()?;
    let mut provider = make_test_provider(Vec::new());
    provider.client = reqwest::Client::builder().no_proxy().build()?;
    provider.catalog.write().unwrap().source = CatalogSource::None;
    provider.set_model("gpt-6-luna")?;
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    set_host(&provider, format!("http://{}", listener.local_addr()?)).await;
    let provider = Arc::new(provider);
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut catalog, _) = listener.accept().await?;
        let headers = read_headers(&mut catalog).await?;
        assert!(headers.starts_with(b"GET /models HTTP/1.1\r\n"));
        started_tx.send(()).unwrap();
        release_rx.await?;
        reply(&mut catalog, "200 OK", "application/json", r#"{"data":[{"id":"gpt-6-luna","model_picker_enabled":true,"supported_endpoints":["/responses"]}]}"#).await?;
        for _ in 0..2 {
            let (mut socket, _) = listener.accept().await?;
            let body = read_completion(&mut socket, "/responses").await?;
            assert_eq!(body["model"], "gpt-6-luna");
            assert_eq!(body["input"][0]["content"][0]["text"], "hello");
            reply(&mut socket, "200 OK", "text/event-stream", "data: {\"type\":\"response.output_text.delta\",\"delta\":\"ROUTE_OK\"}\n\ndata: {\"type\":\"response.completed\",\"response\":{}}\n\n").await?;
        }
        Ok::<_, anyhow::Error>(())
    });
    let first_provider = provider.clone();
    let first = tokio::spawn(async move { completion_text(&first_provider).await });
    started_rx.await?;
    let messages = vec![make_msg(
        Role::User,
        vec![ContentBlock::Text {
            text: "hello".to_string(),
            cache_control: None,
        }],
    )];
    let mut second = Box::pin(provider.complete(&messages, &[], "system", None));
    assert!(
        futures::poll!(second.as_mut()).is_pending(),
        "unknown model bypassed the active refresh"
    );
    release_tx.send(()).unwrap();
    let mut stream = tokio::time::timeout(Duration::from_secs(4), second).await??;
    let mut text = String::new();
    use futures::StreamExt;
    while let Some(event) = stream.next().await {
        if let StreamEvent::TextDelta(delta) = event? {
            text.push_str(&delta);
        }
    }
    assert_eq!(text, "ROUTE_OK");
    assert_eq!(first.await??, "ROUTE_OK");
    server.await??;
    Ok(())
}

#[tokio::test]
async fn unknown_model_with_stalled_catalog_fails_without_inference_or_repeated_fetches()
-> Result<()> {
    let mut provider = make_test_provider(Vec::new());
    provider.client = reqwest::Client::builder().no_proxy().build()?;
    provider.catalog.write().unwrap().source = CatalogSource::None;
    provider.set_model("gpt-6-luna")?;
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    set_host(&provider, format!("http://{}", listener.local_addr()?)).await;
    let provider = Arc::new(provider);
    let first_provider = provider.clone();
    let first = tokio::spawn(async move { completion_text(&first_provider).await });
    let (mut stalled_catalog, _) = listener.accept().await?;
    let headers = read_headers(&mut stalled_catalog).await?;
    assert!(headers.starts_with(b"GET /models HTTP/1.1\r\n"));
    stalled_catalog
        .write_all(
            b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 1024\r\n\r\n{",
        )
        .await?;
    let concurrent =
        tokio::time::timeout(Duration::from_secs(3), completion_text(&provider)).await?;
    assert!(
        concurrent.is_err(),
        "unknown model was sent without endpoint metadata"
    );
    assert!(
        tokio::time::timeout(Duration::from_secs(3), first)
            .await??
            .is_err()
    );
    let repeated = tokio::time::timeout(Duration::from_secs(1), completion_text(&provider)).await?;
    assert!(repeated.is_err());
    // Neither an inference request nor a second catalog fetch reached the API host.
    assert!(
        tokio::time::timeout(Duration::from_millis(100), listener.accept())
            .await
            .is_err()
    );
    Ok(())
}
