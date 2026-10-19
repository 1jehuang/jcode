use super::*;
use async_trait::async_trait;
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Clone)]
struct CompactionCacheProvider {
    calls: Arc<AtomicUsize>,
    compaction_after_usage: bool,
    discard_compaction: bool,
}

#[async_trait]
impl Provider for CompactionCacheProvider {
    async fn complete(
        &self,
        _: &[Message],
        _: &[ToolDefinition],
        _: &str,
        _: Option<&str>,
    ) -> Result<crate::provider::EventStream> {
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        let mut events = vec![Ok(StreamEvent::TokenUsage {
            input_tokens: Some(if call == 0 { 216_517 } else { 204_393 }),
            output_tokens: Some(5),
            cache_read_input_tokens: Some(if call == 0 { 216_064 } else { 0 }),
            cache_creation_input_tokens: None,
        })];
        if call == 1 {
            let compaction = Ok(StreamEvent::Compaction {
                trigger: "openai_native_auto".into(),
                pre_tokens: None,
                openai_encrypted_content: Some("test-encrypted-compaction".into()),
            });
            if self.compaction_after_usage {
                events.push(compaction);
            } else {
                events.insert(0, compaction);
            }
            if self.discard_compaction {
                events.push(Ok(StreamEvent::RetryRollback { attempt: 1, max: 2 }));
                events.push(Ok(StreamEvent::TokenUsage {
                    input_tokens: Some(204_393),
                    output_tokens: Some(5),
                    cache_read_input_tokens: Some(0),
                    cache_creation_input_tokens: None,
                }));
            }
        }
        events.push(Ok(StreamEvent::TextDelta("answer".into())));
        events.push(Ok(StreamEvent::MessageEnd {
            stop_reason: Some("end_turn".into()),
        }));
        Ok(Box::pin(futures::stream::iter(events)))
    }

    fn name(&self) -> &str {
        "openai"
    }

    fn model(&self) -> String {
        "cache-compaction-test".into()
    }

    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(self.clone())
    }
}

#[tokio::test]
async fn native_compaction_cache_refresh_is_classified_before_compaction_is_announced() {
    let _home = crate::auth::test_sandbox::AuthTestSandbox::new().unwrap();
    for (compaction_after_usage, discard_compaction) in
        [(false, false), (true, false), (true, true)]
    {
        let provider: Arc<dyn Provider> = Arc::new(CompactionCacheProvider {
            calls: Arc::new(AtomicUsize::new(0)),
            compaction_after_usage,
            discard_compaction,
        });
        let registry = Registry::new(provider.clone()).await;
        let mut agent = Agent::new(provider, registry);
        agent.add_message(
            Role::User,
            vec![ContentBlock::Text {
                text: "warm cache".into(),
                cache_control: None,
            }],
        );
        let (tx, mut rx) = mpsc::unbounded_channel();
        agent.run_turn_streaming_mpsc(tx.clone()).await.unwrap();
        while rx.try_recv().is_ok() {}
        agent.add_message(
            Role::User,
            vec![ContentBlock::Text {
                text: "continue".into(),
                cache_control: None,
            }],
        );
        agent.run_turn_streaming_mpsc(tx).await.unwrap();
        let mut refresh_seen = false;
        let mut compaction_seen = false;
        while let Ok(event) = rx.try_recv() {
            match event {
                ServerEvent::KvCacheMiss {
                    reason,
                    harness_caused,
                    expected_tokens,
                    read_tokens,
                    documented_cause,
                    message,
                    ..
                } => {
                    assert_eq!(
                        reason,
                        if discard_compaction {
                            "zero_read"
                        } else {
                            "compaction"
                        }
                    );
                    assert!(!harness_caused);
                    assert_eq!(expected_tokens, 216_517);
                    assert_eq!(read_tokens, 0);
                    if discard_compaction {
                        assert_eq!(documented_cause, None);
                        assert!(message.starts_with("KV cache miss"));
                    } else {
                        assert_eq!(
                            documented_cause.as_deref(),
                            Some("provider_auto_compaction")
                        );
                        assert!(message.starts_with("KV cache refresh"));
                    }
                    refresh_seen = true;
                }
                ServerEvent::Compaction {
                    trigger,
                    pre_tokens,
                    post_tokens,
                    tokens_saved,
                    summary_chars,
                    ..
                } => {
                    assert_eq!(trigger, "openai_native_auto");
                    assert_eq!(
                        pre_tokens, None,
                        "response input is not a pre-compaction count"
                    );
                    assert_eq!(
                        post_tokens, None,
                        "opaque payload length is not a token count"
                    );
                    assert_eq!(tokens_saved, None);
                    assert_eq!(summary_chars, None);
                    assert!(
                        refresh_seen,
                        "usage classification precedes compaction application"
                    );
                    compaction_seen = true;
                }
                _ => {}
            }
        }
        assert!(
            refresh_seen,
            "completed request must classify its zero read"
        );
        assert_eq!(compaction_seen, !discard_compaction);
    }
}

/// Replays the 2026-10-19 incident: a 429 on account `claude-fox` failed the
/// turn over to `claude-otter` with the same model. The new account read 0
/// cached tokens, which used to be reported as an unexplained zero read.
#[derive(Clone)]
struct AccountFailoverProvider {
    calls: Arc<AtomicUsize>,
    account: Arc<std::sync::Mutex<String>>,
}

#[async_trait]
impl Provider for AccountFailoverProvider {
    async fn complete(
        &self,
        _: &[Message],
        _: &[ToolDefinition],
        _: &str,
        _: Option<&str>,
    ) -> Result<crate::provider::EventStream> {
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        let (read, write) = if call == 0 { (33_000, 96) } else { (0, 39_721) };
        Ok(Box::pin(futures::stream::iter(vec![
            Ok(StreamEvent::TokenUsage {
                input_tokens: Some(483),
                output_tokens: Some(5),
                cache_read_input_tokens: Some(read),
                cache_creation_input_tokens: Some(write),
            }),
            Ok(StreamEvent::TextDelta("answer".into())),
            Ok(StreamEvent::MessageEnd {
                stop_reason: Some("end_turn".into()),
            }),
        ])))
    }

    fn name(&self) -> &str {
        "claude"
    }

    fn model(&self) -> String {
        "claude-opus-5-5".into()
    }

    fn active_resolved_credential(&self) -> Option<jcode_provider_core::ResolvedCredential> {
        Some(jcode_provider_core::ResolvedCredential::Oauth)
    }

    fn oauth_account_label(&self) -> Option<String> {
        Some(self.account.lock().unwrap().clone())
    }

    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(self.clone())
    }
}

async fn run_account_turns(switch_account: bool) -> Vec<ServerEvent> {
    let account = Arc::new(std::sync::Mutex::new("claude-fox".to_string()));
    let provider: Arc<dyn Provider> = Arc::new(AccountFailoverProvider {
        calls: Arc::new(AtomicUsize::new(0)),
        account: account.clone(),
    });
    let registry = Registry::new(provider.clone()).await;
    let mut agent = Agent::new(provider, registry);
    let (tx, mut rx) = mpsc::unbounded_channel();
    for (index, text) in ["warm cache", "continue"].into_iter().enumerate() {
        if index == 1 && switch_account {
            // Same as an accepted failover route: rebind runtime and session.
            *account.lock().unwrap() = "claude-otter".into();
            agent.session.route_api_method = Some("claude-oauth@claude-otter".into());
        }
        agent.add_message(
            Role::User,
            vec![ContentBlock::Text {
                text: text.into(),
                cache_control: None,
            }],
        );
        agent.run_turn_streaming_mpsc(tx.clone()).await.unwrap();
    }
    let mut misses = Vec::new();
    while let Ok(event) = rx.try_recv() {
        if matches!(event, ServerEvent::KvCacheMiss { .. }) {
            misses.push(event);
        }
    }
    misses
}

#[tokio::test]
async fn account_failover_miss_names_both_accounts_with_readable_counts() {
    let _home = crate::auth::test_sandbox::AuthTestSandbox::new().unwrap();
    let misses = run_account_turns(true).await;
    assert_eq!(misses.len(), 1, "{misses:?}");
    let ServerEvent::KvCacheMiss {
        reason,
        harness_caused,
        missed_tokens,
        expected_tokens,
        read_tokens,
        message,
        ..
    } = &misses[0]
    else {
        unreachable!()
    };
    eprintln!("account failover notice: {message}");
    assert_eq!(reason, "account_switch");
    assert!(!harness_caused);
    assert_eq!(
        (*expected_tokens, *read_tokens, *missed_tokens),
        (33_096, 0, 33_096)
    );
    assert!(message.contains("[account_switch]"), "{message}");
    assert!(message.contains("claude-fox -> claude-otter"), "{message}");
    assert!(message.contains("~33.1K tokens resent"), "{message}");
    assert!(!message.contains("33096"), "raw count leaked: {message}");
}

#[tokio::test]
async fn same_account_zero_read_explains_what_was_ruled_out() {
    let _home = crate::auth::test_sandbox::AuthTestSandbox::new().unwrap();
    let misses = run_account_turns(false).await;
    assert_eq!(misses.len(), 1, "{misses:?}");
    let ServerEvent::KvCacheMiss {
        reason, message, ..
    } = &misses[0]
    else {
        unreachable!()
    };
    eprintln!("same-account zero read notice: {message}");
    assert_eq!(reason, "zero_read");
    assert!(
        message.contains("read 0 of the 33.1K-token prefix"),
        "{message}"
    );
    assert!(message.contains("after the previous call"), "{message}");
    assert!(
        message.contains("same provider, model, account"),
        "{message}"
    );
}
