use super::*;
use crate::session_env::{remove_session_env, set_session_env};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    sync::oneshot,
};

fn env(values: &[(&str, &str)]) -> HashMap<String, String> {
    values
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

#[test]
fn effective_proxy_layering_aliases_empty_fallback_and_cgi() {
    let daemon = env(&[
        ("HTTPS_PROXY", "http://daemon"),
        ("ALL_PROXY", "http://all"),
        ("NO_PROXY", "daemon.test"),
    ]);
    let resolve = |session: &HashMap<String, String>| {
        ProxyConfig::resolve(session, |k| daemon.get(k).cloned())
    };
    assert!(resolve(&HashMap::new()).is_none());
    assert!(resolve(&env(&[("UNRELATED", "secret")])).is_none());
    let config = resolve(&env(&[("NO_PROXY", "session.test")])).unwrap();
    assert_eq!(config.http.as_deref(), Some("http://all"));
    assert_eq!(config.https.as_deref(), Some("http://daemon"));
    assert_eq!(config.no.as_deref(), Some("session.test"));
    // Exact-key layering means an inherited uppercase alias still wins.
    let config = resolve(&env(&[("https_proxy", "http://lower")])).unwrap();
    assert_eq!(config.https.as_deref(), Some("http://daemon"));
    let config = resolve(&env(&[
        ("HTTPS_PROXY", ""),
        ("https_proxy", "http://lower"),
    ]))
    .unwrap();
    assert_eq!(config.https.as_deref(), Some("http://all"));
    let config = ProxyConfig::resolve(&env(&[("https_proxy", "http://lower")]), |_| None).unwrap();
    assert_eq!(config.https.as_deref(), Some("http://lower"));
    let config = ProxyConfig::resolve(
        &env(&[
            ("HTTPS_PROXY", "http://user:secret@host"),
            ("REQUEST_METHOD", "GET"),
        ]),
        |_| None,
    )
    .unwrap();
    assert!(config.http.is_none() && config.https.is_none());
    let config = resolve(&env(&[("HTTPS_PROXY", "http://user:secret@host")])).unwrap();
    assert!(!format!("{config:?}").contains("secret"));
    let invalid = ProxyConfig {
        http: Some("http://user:secret@[invalid".into()),
        https: None,
        no: None,
    };
    let error = invalid.apply(builder(Profile::Shared)).err().unwrap();
    assert!(!format!("{error:#?}").contains("secret"));
    let unsupported = ProxyConfig {
        http: Some("ftp://user:secret@host".into()),
        https: None,
        no: None,
    };
    assert_eq!(
        unsupported
            .apply(builder(Profile::Shared))
            .err()
            .unwrap()
            .to_string(),
        "Unsupported session HTTP proxy protocol"
    );
}

async fn server(
    body: &'static str,
) -> (
    String,
    oneshot::Receiver<String>,
    tokio::task::JoinHandle<()>,
) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (tx, rx) = oneshot::channel();
    let task = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        loop {
            let mut bytes = [0; 4096];
            let count = stream.read(&mut bytes).await.unwrap();
            if count == 0 {
                break;
            }
            request.extend_from_slice(&bytes[..count]);
            if request.windows(4).any(|w| w == b"\r\n\r\n") {
                break;
            }
        }
        let request = String::from_utf8_lossy(&request).into_owned();
        let connect = request.starts_with("CONNECT ");
        tx.send(request).unwrap();
        if !connect {
            stream
                .write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
        }
    });
    (format!("http://{address}"), rx, task)
}

fn context(session: &str) -> crate::tool::ToolContext {
    crate::tool::ToolContext {
        session_id: session.into(),
        message_id: "proxy-test".into(),
        tool_call_id: "proxy-test".into(),
        working_dir: None,
        stdin_request_tx: None,
        graceful_shutdown_signal: None,
        execution_mode: jcode_tool_core::ToolExecutionMode::Direct,
    }
}

#[tokio::test]
async fn concurrent_https_fetches_keep_session_proxy_credentials_across_rounds() {
    use crate::tool::Tool;
    use base64::Engine;

    // A CONNECT capture proxy needs no DNS, TLS certificate, or external service.
    // Refusing the tunnel also checks that transport errors redact credentials.
    let a = "session-http-concurrent-a";
    let b = "session-http-concurrent-b";
    for _ in 0..3 {
        let (proxy_a, request_a, task_a) = server("").await;
        let (proxy_b, request_b, task_b) = server("").await;
        for (session, proxy, user) in [(a, proxy_a, "alice"), (b, proxy_b, "bob")] {
            set_session_env(
                session,
                env(&[
                    (
                        "HTTPS_PROXY",
                        &proxy.replacen("http://", &format!("http://{user}:secret@"), 1),
                    ),
                    ("NO_PROXY", ""),
                ]),
            )
            .unwrap();
        }
        let tool = crate::tool::webfetch::WebFetchTool::new();
        let (result_a, result_b) = tokio::time::timeout(std::time::Duration::from_secs(10), async {
            tokio::join!(
                tool.execute(
                    serde_json::json!({"url":"https://alice.example.test/"}),
                    context(a),
                ),
                tool.execute(
                    serde_json::json!({"url":"https://bob.example.test/"}),
                    context(b),
                ),
            )
        })
        .await
        .expect("both session proxy requests must finish");
        for (result, request, user, other) in [
            (result_a, request_a, "alice", "bob"),
            (result_b, request_b, "bob", "alice"),
        ] {
            let error = format!("{:#?}", result.unwrap_err());
            assert!(!error.contains("secret"));
            let request = request.await.unwrap().to_lowercase();
            assert!(request.starts_with(&format!("connect {user}.example.test:443")));
            let authorization = |user| {
                format!(
                    "proxy-authorization: basic {}",
                    base64::engine::general_purpose::STANDARD.encode(format!("{user}:secret"))
                )
                .to_lowercase()
            };
            assert!(request.contains(&authorization(user)));
            assert!(!request.contains(&authorization(other)));
        }
        task_a.await.unwrap();
        task_b.await.unwrap();
    }
    remove_session_env(a);
    remove_session_env(b);
}

#[tokio::test]
async fn no_proxy_only_retains_daemon_proxy_for_other_hosts() {
    let (proxy, proxy_rx, proxy_task) = server("proxied").await;
    let (origin, origin_rx, origin_task) = server("direct").await;
    let config = ProxyConfig::resolve(&env(&[("NO_PROXY", "127.0.0.1")]), |key| {
        (key == "HTTP_PROXY").then(|| proxy.clone())
    })
    .unwrap();
    let client = config
        .apply(builder(Profile::Shared))
        .unwrap()
        .build()
        .unwrap();
    assert_eq!(
        client
            .get("http://remote.example.test/path")
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap(),
        "proxied"
    );
    assert_eq!(
        client
            .get(origin)
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap(),
        "direct"
    );
    assert!(
        proxy_rx
            .await
            .unwrap()
            .starts_with("GET http://remote.example.test/path")
    );
    assert!(origin_rx.await.unwrap().starts_with("GET / HTTP/1.1"));
    proxy_task.await.unwrap();
    origin_task.await.unwrap();
}

#[tokio::test]
async fn replacing_env_during_fetch_preserves_redirect_proxy_snapshot() {
    use crate::tool::Tool;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (started_tx, started_rx) = oneshot::channel();
    let (resume_tx, resume_rx) = oneshot::channel();
    let proxy_task = tokio::spawn(async move {
        let mut started_tx = Some(started_tx);
        let mut resume_rx = Some(resume_rx);
        for index in 0..2 {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            loop {
                let mut bytes = [0; 4096];
                let count = stream.read(&mut bytes).await.unwrap();
                assert!(count > 0);
                request.extend_from_slice(&bytes[..count]);
                if request.windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
            }
            let request = String::from_utf8(request).unwrap();
            assert!(request.to_lowercase().contains("proxy-authorization:"));
            if index == 0 {
                assert!(request.contains("http://first.example.test/"));
                started_tx.take().unwrap().send(()).unwrap();
                resume_rx.take().unwrap().await.unwrap();
                stream.write_all(b"HTTP/1.1 302 Found\r\nLocation: http://first.example.test/final\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await.unwrap();
            } else {
                assert!(request.contains("http://first.example.test/final"));
                stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 8\r\nConnection: close\r\n\r\noriginal").await.unwrap();
            }
        }
    });
    let sid = "session-http-inflight-redirect";
    set_session_env(
        sid,
        env(&[
            ("HTTP_PROXY", &format!("http://original:secret@{address}")),
            ("NO_PROXY", ""),
        ]),
    )
    .unwrap();
    let fetch = tokio::spawn(async move {
        crate::tool::webfetch::WebFetchTool::new()
            .execute(
                serde_json::json!({"url":"http://first.example.test/"}),
                context(sid),
            )
            .await
    });
    started_rx.await.unwrap();
    let (replacement, replacement_rx, replacement_task) = server("replacement").await;
    set_session_env(sid, env(&[("HTTP_PROXY", &replacement), ("NO_PROXY", "")])).unwrap();
    resume_tx.send(()).unwrap();
    assert!(fetch.await.unwrap().unwrap().output.contains("original"));
    let output = crate::tool::webfetch::WebFetchTool::new()
        .execute(
            serde_json::json!({"url":"http://next.example.test/"}),
            context(sid),
        )
        .await
        .unwrap();
    assert!(output.output.contains("replacement"));
    assert!(
        replacement_rx
            .await
            .unwrap()
            .contains("http://next.example.test/")
    );
    proxy_task.await.unwrap();
    replacement_task.await.unwrap();
    remove_session_env(sid);
}

#[tokio::test]
async fn tool_proxy_credentials_snapshot_bypass_cache_and_cleanup() {
    use crate::tool::Tool;
    use base64::Engine;
    let (a, a_rx, a_task) = server("from-a").await;
    let (b, b_rx, b_task) = server("from-b").await;
    let proxy_env = |proxy: &str, user: &str| {
        env(&[
            (
                "HTTP_PROXY",
                &proxy.replacen("http://", &format!("http://{user}:secret@"), 1),
            ),
            (
                "HTTPS_PROXY",
                &proxy.replacen("http://", &format!("http://{user}:secret@"), 1),
            ),
            ("ALL_PROXY", ""),
            ("NO_PROXY", ""),
        ])
    };
    let a_env = proxy_env(&a, "alice");
    let b_env = proxy_env(&b, "bob");
    let sid = "session-http-proxy-a";
    let other = "session-http-proxy-b";
    set_session_env(sid, a_env.clone()).unwrap();
    set_session_env(other, b_env.clone()).unwrap();
    let snapshot = client(sid, Profile::Shared).unwrap();
    // Replacing the registry cannot change an already-selected client.
    set_session_env(sid, b_env.clone()).unwrap();
    assert_eq!(
        snapshot
            .get("http://unresolvable.example.test/snapshot")
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap(),
        "from-a"
    );
    let tool = crate::tool::webfetch::WebFetchTool::new();
    let output = tool
        .execute(
            serde_json::json!({"url":"http://unresolvable.example.test/fetch", "format":"text"}),
            context(sid),
        )
        .await
        .unwrap();
    assert!(output.output.contains("from-b"));
    let a_request = a_rx.await.unwrap().to_lowercase();
    let b_request = b_rx.await.unwrap().to_lowercase();
    assert!(
        a_request.contains(
            &format!(
                "proxy-authorization: basic {}",
                base64::engine::general_purpose::STANDARD.encode("alice:secret")
            )
            .to_lowercase()
        )
    );
    assert!(
        b_request.contains(
            &format!(
                "proxy-authorization: basic {}",
                base64::engine::general_purpose::STANDARD.encode("bob:secret")
            )
            .to_lowercase()
        )
    );
    a_task.await.unwrap();
    b_task.await.unwrap();

    let (connect_proxy, rx, task) = server("").await;
    set_session_env(sid, proxy_env(&connect_proxy, "connect-user")).unwrap();
    let error = tool
        .execute(
            serde_json::json!({"url":"https://unresolvable.example.test/connect"}),
            context(sid),
        )
        .await
        .unwrap_err();
    assert!(!format!("{error:#?}").contains("connect-user"));
    let request = rx.await.unwrap();
    assert!(request.starts_with("CONNECT unresolvable.example.test:443"));
    assert!(
        request.to_lowercase().contains(
            &format!(
                "proxy-authorization: basic {}",
                base64::engine::general_purpose::STANDARD.encode("connect-user:secret")
            )
            .to_lowercase()
        )
    );
    task.await.unwrap();

    let (origin, rx, task) = server("direct").await;
    let mut bypass = proxy_env("http://127.0.0.1:1", "unused");
    bypass.insert("NO_PROXY".into(), "127.0.0.1".into());
    set_session_env(sid, bypass).unwrap();
    let output = tool
        .execute(serde_json::json!({"url":origin}), context(sid))
        .await
        .unwrap();
    assert!(output.output.contains("direct"));
    assert!(
        !rx.await
            .unwrap()
            .to_lowercase()
            .contains("proxy-authorization")
    );
    task.await.unwrap();

    let shared = proxy_env("http://127.0.0.1:19999", "cache-user");
    let config = ProxyConfig::resolve(&shared, |k| std::env::var(k).ok()).unwrap();
    for i in 0..40 {
        let session = format!("session-http-shared-{i}");
        set_session_env(&session, shared.clone()).unwrap();
        client(&session, Profile::Shared).unwrap();
    }
    assert_eq!(
        CACHE
            .lock()
            .unwrap()
            .iter()
            .filter(|e| e.config == config && e.profile == Profile::Shared)
            .count(),
        1
    );
    for i in 0..40 {
        remove_session_env(&format!("session-http-shared-{i}"));
    }
    assert!(!CACHE.lock().unwrap().iter().any(|e| e.config == config));
    for i in 0..40 {
        let session = format!("session-http-bounded-{i}");
        set_session_env(
            &session,
            proxy_env(&format!("http://127.0.0.1:{}", 20000 + i), "bounded"),
        )
        .unwrap();
        client(&session, Profile::Shared).unwrap();
        assert!(CACHE.lock().unwrap().len() <= CAPACITY);
    }
    for i in 0..40 {
        remove_session_env(&format!("session-http-bounded-{i}"));
    }
    set_session_env(sid, HashMap::new()).unwrap();
    assert!(ProxyConfig::resolve(&crate::session_env::session_env(sid), |_| None).is_none());
    remove_session_env(other);
}
