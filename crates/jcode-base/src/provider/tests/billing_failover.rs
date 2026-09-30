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
        account_failover: Default::default(),
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
fn billing_402_in_countdown_mode_offers_sibling_without_resending() {
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
        // The turn must not reach prof-b before the user had the countdown to
        // cancel it: the provider only offers the switch.
        let err = first.expect_err("countdown mode must not resend on its own");
        let prompt = crate::provider::parse_failover_prompt_message(&err.to_string())
            .unwrap_or_else(|| panic!("expected a failover prompt, got: {err:#}"));
        assert_eq!(prompt.to_provider, "prof-b:glm-5.3");
        assert!(prompt.reason.to_ascii_lowercase().contains("credit"));
        assert_eq!(a_hits.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(
            b_hits.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "prof-b must get zero requests before the countdown ends"
        );
        assert_eq!(
            ProviderRegistry::new(&provider).active_compatible_profile_id(),
            Some("prof-a".to_string()),
            "the provider must not switch profile by itself"
        );

        // The TUI countdown switches to the offered target and resends.
        provider
            .set_model(&prompt.to_provider)
            .expect("countdown switch to profile B");
        let second = rt.block_on(async {
            let stream = provider.complete(&messages, &[], "", None).await?;
            drain_text(stream).await
        });
        assert_eq!(second.expect("resend on B"), "hello from B");
        assert_eq!(b_hits.load(std::sync::atomic::Ordering::SeqCst), 1);

        // A later turn that explicitly goes back to A offers B again without
        // touching A while A is marked out of credit.
        provider
            .set_model("prof-a:glm-5.3")
            .expect("reselect profile A");
        let third = rt.block_on(async {
            let stream = provider.complete(&messages, &[], "", None).await?;
            drain_text(stream).await
        });
        let err = third.expect_err("A is still out of credit");
        assert!(crate::provider::parse_failover_prompt_message(&err.to_string()).is_some());
        assert_eq!(
            a_hits.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "profile A is marked unavailable and must not be retried"
        );
        crate::provider::models::clear_provider_unavailable_for_account("openai-compatible:prof-a");
    });
}

fn write_three_profile_config(a_base: &str, b_base: &str, c_base: &str) {
    let jcode_home = std::env::var_os("JCODE_HOME").expect("test JCODE_HOME should be set");
    let profile = |name: &str, base: &str| {
        format!(
            r#"
[providers.{name}]
type = "openai-compatible"
base_url = "{base}"
auth = "none"
model_catalog = false
default_model = "glm-5.3"

[[providers.{name}.models]]
id = "glm-5.3"
"#
        )
    };
    std::fs::write(
        std::path::PathBuf::from(jcode_home).join("config.toml"),
        format!(
            "[provider]\ncross_provider_failover = \"countdown\"\n{}{}{}",
            profile("prof-a", a_base),
            profile("prof-b", b_base),
            profile("prof-c", c_base)
        ),
    )
    .expect("write test config.toml");
    crate::config::invalidate_config_cache();
}

#[test]
fn failing_sibling_offers_the_next_sibling_instead_of_ending_failover() {
    for (status, body) in [
        // The runtime retries 429 and 5xx itself; `Retry-After: 0` keeps
        // those retries instant.
        (
            "429 Too Many Requests\r\nRetry-After: 0",
            r#"{"error":{"message":"rate limited"}}"#,
        ),
        ("401 Unauthorized", r#"{"error":{"message":"bad key"}}"#),
        (
            "500 Internal Server Error\r\nRetry-After: 0",
            r#"{"error":{"message":"boom"}}"#,
        ),
    ] {
        with_clean_provider_test_env(|| {
            let (a_base, _a_hits) = spawn_canned_chat_server(
                "402 Payment Required",
                "application/json",
                BILLING_402_BODY.to_string(),
            );
            let (b_base, b_hits) =
                spawn_canned_chat_server(status, "application/json", body.to_string());
            let (c_base, c_hits) =
                spawn_canned_chat_server("200 OK", "text/event-stream", b_answer_body());
            write_three_profile_config(&a_base, &b_base, &c_base);

            let provider = billing_failover_test_provider();
            provider.set_model("prof-a:glm-5.3").expect("select A");
            let rt = enter_test_runtime();
            let messages = billing_test_messages();
            let run = |provider: &MultiProvider| {
                rt.block_on(async {
                    let stream = provider.complete(&messages, &[], "", None).await?;
                    drain_text(stream).await
                })
            };

            let err = run(&provider).expect_err("A is out of credit");
            let prompt = crate::provider::parse_failover_prompt_message(&err.to_string())
                .expect("prompt for B");
            assert_eq!(prompt.to_provider, "prof-b:glm-5.3");

            provider.set_model(&prompt.to_provider).expect("switch to B");
            let err = run(&provider).expect_err("B fails before any output");
            let prompt = crate::provider::parse_failover_prompt_message(&err.to_string())
                .unwrap_or_else(|| {
                    panic!("B answered {status}: expected an offer for prof-c, got: {err:#}")
                });
            assert_eq!(prompt.to_provider, "prof-c:glm-5.3", "{status}");
            assert!(b_hits.load(std::sync::atomic::Ordering::SeqCst) >= 1);
            assert_eq!(c_hits.load(std::sync::atomic::Ordering::SeqCst), 0);
            let notices = provider.drain_startup_notices().join("\n");
            assert!(
                !notices.contains("Switched"),
                "a failing sibling must not be announced as a success: {notices}"
            );

            provider.set_model(&prompt.to_provider).expect("switch to C");
            assert_eq!(run(&provider).expect("C answers"), "hello from B");
            assert_eq!(c_hits.load(std::sync::atomic::Ordering::SeqCst), 1);
            for profile in ["prof-a", "prof-b", "prof-c"] {
                crate::provider::models::clear_provider_unavailable_for_account(&format!(
                    "openai-compatible:{profile}"
                ));
            }
        });
    }
}

#[test]
fn every_sibling_failing_surfaces_the_last_error() {
    with_clean_provider_test_env(|| {
        let (a_base, _) = spawn_canned_chat_server(
            "402 Payment Required",
            "application/json",
            BILLING_402_BODY.to_string(),
        );
        let (b_base, _) = spawn_canned_chat_server(
            "401 Unauthorized",
            "application/json",
            r#"{"error":{"message":"bad key on b"}}"#.to_string(),
        );
        write_two_profile_config(&a_base, &b_base, "countdown");
        let provider = billing_failover_test_provider();
        provider.set_model("prof-a:glm-5.3").expect("select A");
        let rt = enter_test_runtime();
        let messages = billing_test_messages();
        let err = rt
            .block_on(async {
                let stream = provider.complete(&messages, &[], "", None).await?;
                drain_text(stream).await
            })
            .expect_err("A is out of credit");
        let prompt =
            crate::provider::parse_failover_prompt_message(&err.to_string()).expect("prompt");
        provider.set_model(&prompt.to_provider).expect("switch to B");
        let err = rt
            .block_on(async {
                let stream = provider.complete(&messages, &[], "", None).await?;
                drain_text(stream).await
            })
            .expect_err("B fails and nothing is left");
        let text = format!("{err:#}");
        assert!(crate::provider::parse_failover_prompt_message(&text).is_none());
        assert!(text.contains("401") && text.contains("bad key on b"), "{text}");
        crate::provider::models::clear_provider_unavailable_for_account("openai-compatible:prof-a");
        crate::provider::models::clear_provider_unavailable_for_account("openai-compatible:prof-b");
    });
}

#[test]
fn billing_error_in_http_200_sse_event_offers_sibling() {
    with_clean_provider_test_env(|| {
        let sse_error = "data: {\"error\":{\"message\":\"Insufficient credits\",\"code\":402}}\n\ndata: [DONE]\n\n".to_string();
        let (a_base, a_hits) = spawn_canned_chat_server("200 OK", "text/event-stream", sse_error);
        let (b_base, b_hits) =
            spawn_canned_chat_server("200 OK", "text/event-stream", b_answer_body());
        write_two_profile_config(&a_base, &b_base, "countdown");
        let provider = billing_failover_test_provider();
        provider.set_model("prof-a:glm-5.3").expect("select A");
        let rt = enter_test_runtime();
        let messages = billing_test_messages();
        let err = rt
            .block_on(async {
                let stream = provider.complete(&messages, &[], "", None).await?;
                drain_text(stream).await
            })
            .expect_err("A reports no credit in an SSE error event");
        let prompt = crate::provider::parse_failover_prompt_message(&err.to_string())
            .unwrap_or_else(|| panic!("expected an offer for prof-b, got: {err:#}"));
        assert_eq!(prompt.to_provider, "prof-b:glm-5.3");
        assert_eq!(a_hits.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(b_hits.load(std::sync::atomic::Ordering::SeqCst), 0);
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

