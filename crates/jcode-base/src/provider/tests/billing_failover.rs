// A 402 (out of credit) from one OpenAI-compatible profile must resend the
// turn to another configured profile that serves the same model id, and later
// turns must go straight to that profile.

const BILLING_402_BODY: &str =
    r#"{"error":{"message":"Insufficient credits","type":"insufficient_quota","code":402}}"#;

/// Fake OpenAI-compatible endpoint that answers every request with the same
/// canned HTTP response and counts the requests it served.
fn spawn_canned_chat_server(
    status_line: &'static str,
    content_type: &'static str,
    body: String,
) -> (String, Arc<std::sync::atomic::AtomicUsize>) {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind fake server");
    let addr = listener.local_addr().expect("fake server addr");
    let hits = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let hits_thread = Arc::clone(&hits);
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(2)));
            let mut request = Vec::new();
            let mut buf = [0u8; 16384];
            // Read headers plus the declared body so the client never sees a
            // reset before it finished writing.
            loop {
                let n = stream.read(&mut buf).unwrap_or(0);
                if n == 0 {
                    break;
                }
                request.extend_from_slice(&buf[..n]);
                let text = String::from_utf8_lossy(&request);
                if let Some(split) = text.find("\r\n\r\n") {
                    let len = text[..split]
                        .lines()
                        .find_map(|line| {
                            let lower = line.to_ascii_lowercase();
                            lower
                                .strip_prefix("content-length:")
                                .and_then(|v| v.trim().parse::<usize>().ok())
                        })
                        .unwrap_or(0);
                    if request.len() >= split + 4 + len {
                        break;
                    }
                }
            }
            if !String::from_utf8_lossy(&request).starts_with("POST ") {
                let _ = stream.write_all(
                    b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                );
                continue;
            }
            hits_thread.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let response = format!(
                "HTTP/1.1 {status_line}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(response.as_bytes());
        }
    });
    (format!("http://{addr}/v1"), hits)
}

fn billing_failover_test_provider() -> MultiProvider {
    MultiProvider {
        anthropic: RwLock::new(None),
        openai: RwLock::new(None),
        copilot_api: RwLock::new(None),
        antigravity: RwLock::new(None),
        gemini: RwLock::new(None),
        cursor: RwLock::new(None),
        bedrock: RwLock::new(None),
        openrouter: RwLock::new(None),
        openai_compatible_profiles: RwLock::new(std::collections::HashMap::new()),
        active_openai_compatible_profile: RwLock::new(None),
        active: RwLock::new(ActiveProvider::OpenRouter),
        startup_notices: RwLock::new(Vec::new()),
        initial_provider: None,
        routes_memo: std::sync::Mutex::new(None),
        post_auth_refreshes_pending: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
    }
}

fn write_two_profile_config(a_base: &str, b_base: &str, failover_mode: &str) {
    let jcode_home = std::env::var_os("JCODE_HOME").expect("test JCODE_HOME should be set");
    std::fs::write(
        std::path::PathBuf::from(jcode_home).join("config.toml"),
        format!(
            r#"
[provider]
cross_provider_failover = "{failover_mode}"

[providers.prof-a]
type = "openai-compatible"
base_url = "{a_base}"
auth = "none"
model_catalog = false
default_model = "glm-5.3"

[[providers.prof-a.models]]
id = "glm-5.3"

[providers.prof-b]
type = "openai-compatible"
base_url = "{b_base}"
auth = "none"
model_catalog = false
default_model = "glm-5.3"

[[providers.prof-b.models]]
id = "glm-5.3"
"#
        ),
    )
    .expect("write test config.toml");
    crate::config::invalidate_config_cache();
}

fn billing_test_messages() -> Vec<crate::message::Message> {
    vec![crate::message::Message {
        role: crate::message::Role::User,
        content: vec![crate::message::ContentBlock::Text {
            text: "hello".to_string(),
            cache_control: None,
        }],
        timestamp: None,
        tool_duration_ms: None,
    }]
}

/// Drain a completion stream; returns the assistant text or the first error.
async fn drain_text(mut stream: EventStream) -> anyhow::Result<String> {
    let mut text = String::new();
    while let Some(event) = futures::StreamExt::next(&mut stream).await {
        if let crate::message::StreamEvent::TextDelta(delta) = event? {
            text.push_str(&delta);
        }
    }
    Ok(text)
}

fn b_answer_body() -> String {
    "data: {\"id\":\"x\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"hello from B\"}}]}\n\ndata: {\"id\":\"x\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n".to_string()
}

#[test]
fn billing_402_on_compatible_profile_resends_to_sibling_serving_same_model() {
    with_clean_provider_test_env(|| {
        let (a_base, a_hits) = spawn_canned_chat_server(
            "402 Payment Required",
            "application/json",
            BILLING_402_BODY.to_string(),
        );
        let (b_base, b_hits) =
            spawn_canned_chat_server("200 OK", "text/event-stream", b_answer_body());
        write_two_profile_config(&a_base, &b_base, "countdown");

        let provider = billing_failover_test_provider();
        provider
            .set_model("prof-a:glm-5.3")
            .expect("select profile A");

        let rt = enter_test_runtime();
        let messages = billing_test_messages();
        let first = rt.block_on(async {
            let stream = provider.complete(&messages, &[], "", None).await?;
            drain_text(stream).await
        });
        let text = first.expect("402 on profile A should be resent to profile B");
        assert_eq!(text, "hello from B");
        assert_eq!(a_hits.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(b_hits.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(
            ProviderRegistry::new(&provider).active_compatible_profile_id(),
            Some("prof-b".to_string())
        );
        assert_eq!(provider.model(), "glm-5.3");

        let notices = provider.drain_startup_notices().join("\n");
        assert!(
            notices.contains("prof-a") && notices.contains("prof-b"),
            "notice must name both profiles: {notices}"
        );
        assert!(
            notices.to_ascii_lowercase().contains("credit"),
            "notice must say why in plain words: {notices}"
        );
        assert!(
            notices.contains("Switched this session to prof-b")
                && !notices.contains("next few minutes"),
            "notice must say the session moved and not promise a return: {notices}"
        );

        // A later turn that explicitly goes back to A skips it while A is
        // marked out of credit.
        provider
            .set_model("prof-a:glm-5.3")
            .expect("reselect profile A");
        let second = rt.block_on(async {
            let stream = provider.complete(&messages, &[], "", None).await?;
            drain_text(stream).await
        });
        assert_eq!(second.expect("second turn"), "hello from B");
        assert_eq!(
            a_hits.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "profile A is marked unavailable and must not be retried"
        );
        assert_eq!(b_hits.load(std::sync::atomic::Ordering::SeqCst), 2);
        crate::provider::models::clear_provider_unavailable_for_account("openai-compatible:prof-a");
    });
}

#[test]
fn billing_402_in_manual_mode_offers_sibling_without_resending() {
    with_clean_provider_test_env(|| {
        let (a_base, a_hits) = spawn_canned_chat_server(
            "402 Payment Required",
            "application/json",
            BILLING_402_BODY.to_string(),
        );
        let (b_base, b_hits) =
            spawn_canned_chat_server("200 OK", "text/event-stream", b_answer_body());
        write_two_profile_config(&a_base, &b_base, "manual");

        let provider = billing_failover_test_provider();
        provider
            .set_model("prof-a:glm-5.3")
            .expect("select profile A");

        let rt = enter_test_runtime();
        let messages = billing_test_messages();
        let result = rt.block_on(async {
            let stream = provider.complete(&messages, &[], "", None).await?;
            drain_text(stream).await
        });
        let err = result.expect_err("manual mode must not resend on its own");
        let prompt = crate::provider::parse_failover_prompt_message(&err.to_string())
            .unwrap_or_else(|| panic!("expected a failover prompt, got: {err:#}"));
        assert!(prompt.from_label.contains("prof-a"), "{prompt:?}");
        assert!(prompt.to_label.contains("prof-b"), "{prompt:?}");
        assert_eq!(prompt.to_provider, "prof-b:glm-5.3");
        assert!(prompt.reason.to_ascii_lowercase().contains("credit"));
        assert_eq!(a_hits.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(b_hits.load(std::sync::atomic::Ordering::SeqCst), 0);
        crate::provider::models::clear_provider_unavailable_for_account("openai-compatible:prof-a");
    });
}

#[test]
fn billing_402_without_sibling_keeps_original_error() {
    with_clean_provider_test_env(|| {
        let (a_base, a_hits) = spawn_canned_chat_server(
            "402 Payment Required",
            "application/json",
            BILLING_402_BODY.to_string(),
        );
        let jcode_home = std::env::var_os("JCODE_HOME").expect("JCODE_HOME");
        std::fs::write(
            std::path::PathBuf::from(jcode_home).join("config.toml"),
            format!(
                r#"
[providers.prof-a]
type = "openai-compatible"
base_url = "{a_base}"
auth = "none"
model_catalog = false
default_model = "glm-5.3"

[[providers.prof-a.models]]
id = "glm-5.3"
"#
            ),
        )
        .expect("write config");
        crate::config::invalidate_config_cache();

        let provider = billing_failover_test_provider();
        provider
            .set_model("prof-a:glm-5.3")
            .expect("select profile A");
        let rt = enter_test_runtime();
        let messages = billing_test_messages();
        let result = rt.block_on(async {
            let stream = provider.complete(&messages, &[], "", None).await?;
            drain_text(stream).await
        });
        let err = format!("{:#}", result.expect_err("no sibling: keep the error"));
        assert!(err.contains("402"), "original error kept: {err}");
        assert!(crate::provider::parse_failover_prompt_message(&err).is_none());
        assert_eq!(a_hits.load(std::sync::atomic::Ordering::SeqCst), 1);
        crate::provider::models::clear_provider_unavailable_for_account("openai-compatible:prof-a");
    });
}
