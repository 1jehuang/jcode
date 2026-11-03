//! Retained-child regressions assert what the executable actually read, rather
//! than inferring history delivery from an echoed assistant response.

use super::*;
use jcode_message_types::{ContentBlock, Role};
use jcode_provider_core::EventStream;
use std::sync::Arc;

fn user_inputs(fake: &Fake) -> Vec<Value> {
    std::fs::read_to_string(fake.log.with_extension("log.stdin"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .filter(|entry| entry["message"]["type"] == "user")
        .collect()
}

fn input_text(entry: &Value) -> String {
    entry["message"]["message"]["content"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|block| block["text"].as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

struct OtherProvider;

#[tokio::test]
async fn text_turn_streams_once_and_reuses_the_child() {
    let fake = Fake::new();
    let provider = fake.provider();
    let events = run_turn(&provider, &[Message::user("text please")], None).await;
    assert_eq!(text(&events), "Hello from fake claude");
    assert!(
        events
            .iter()
            .any(|e| matches!(e, StreamEvent::SessionId(_)))
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e, StreamEvent::TokenUsage { .. }))
    );
    assert!(errors(&events).is_empty(), "{events:?}");
    let limits = provider.rate_limit_snapshot().expect("rate limits stored");
    assert_eq!(limits.five_hour.unwrap().utilization, Some(0.25));
    let events = run_turn(
        &provider,
        &[
            Message::user("text please"),
            Message::assistant_text("Hello from fake claude"),
            Message::user("text again"),
        ],
        None,
    )
    .await;
    assert_eq!(text(&events), "Hello from fake claude");
    assert_eq!(fake.launches().len(), 1);
    assert_eq!(input_text(&user_inputs(&fake)[1]), "text again");
    assert_eq!(
        provider.available_models_display(),
        vec!["claude-opus-5-5".to_string(), "claude-sonnet-5".to_string()]
    );
    provider.shutdown().await;
}

#[tokio::test]
async fn history_from_another_provider_is_seeded_once() {
    let fake = Fake::new();
    let provider = fake.provider();
    let history = [
        Message::user("what is 2+2"),
        Message::assistant_text("4"),
        Message::user("echo-prompt"),
    ];
    let events = run_turn(&provider, &history, None).await;
    let sent = text(&events);
    assert!(sent.contains("previous_conversation"), "{sent}");
    assert!(sent.contains("User: what is 2+2"), "{sent}");
    assert!(sent.contains("Assistant: 4"), "{sent}");
    let mut more = history.to_vec();
    more.push(Message::assistant_text(&sent));
    more.push(Message::user("echo-prompt"));
    let events = run_turn(&provider, &more, None).await;
    assert!(!text(&events).contains("previous_conversation"));
    assert_eq!(input_text(&user_inputs(&fake)[1]), "echo-prompt");
    provider.shutdown().await;
}

#[tokio::test]
async fn changing_dynamic_context_does_not_reseed_normal_continuation() {
    let fake = Fake::new();
    let provider = fake.provider();
    let mut history = vec![Message::user("text first")];
    let first = provider
        .complete_split(&history, &tools(), SYSTEM, "old clock", None)
        .await
        .unwrap()
        .collect::<Vec<_>>()
        .await;
    let first = first
        .into_iter()
        .collect::<anyhow::Result<Vec<_>>>()
        .unwrap();
    history.push(Message::assistant_text(&text(&first)));
    history.push(Message::user("text second"));
    let second = provider
        .complete_split(&history, &tools(), SYSTEM, "new clock", None)
        .await
        .unwrap()
        .collect::<Vec<_>>()
        .await;
    let second = second
        .into_iter()
        .collect::<anyhow::Result<Vec<_>>>()
        .unwrap();
    assert!(errors(&first).is_empty() && errors(&second).is_empty());
    assert_eq!(fake.launches().len(), 1);
    let inputs = user_inputs(&fake);
    let sent = input_text(&inputs[1]);
    assert!(sent.starts_with("text second"));
    assert!(sent.contains("new clock"));
    assert!(!sent.contains("old clock"));
    assert!(!sent.contains("previous_conversation"));
    provider.shutdown().await;
}

#[async_trait::async_trait]
impl Provider for OtherProvider {
    async fn complete(
        &self,
        _messages: &[Message],
        _tools: &[ToolDefinition],
        _system: &str,
        _resume: Option<&str>,
    ) -> anyhow::Result<EventStream> {
        Ok(Box::pin(futures::stream::iter(vec![
            Ok(StreamEvent::TextDelta(
                "OTHER_PROVIDER_RESPONSE_1819".into(),
            )),
            Ok(StreamEvent::MessageEnd {
                stop_reason: Some("end_turn".into()),
            }),
        ])))
    }

    fn name(&self) -> &str {
        "other-test-provider"
    }

    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(Self)
    }
}

async fn selected_turn(provider: &dyn Provider, history: &[Message]) -> Vec<StreamEvent> {
    let stream = provider
        .complete(history, &tools(), SYSTEM, None)
        .await
        .unwrap();
    let events = tokio::time::timeout(Duration::from_secs(30), stream.collect::<Vec<_>>())
        .await
        .expect("selected provider finished")
        .into_iter()
        .collect::<anyhow::Result<Vec<_>>>()
        .unwrap();
    assert!(errors(&events).is_empty(), "{events:?}");
    assert!(
        events
            .iter()
            .any(|event| matches!(event, StreamEvent::MessageEnd { .. }))
    );
    events
}

#[tokio::test]
async fn retained_seeded_child_receives_provider_switch_gap_once() {
    let fake = Fake::new();
    let provider = fake.provider();
    let other = OtherProvider;
    let mut selected: &dyn Provider = &provider;
    let mut history = vec![
        Message::user("OLD_SEED_1819"),
        Message::assistant_text("old seed response"),
        Message::user("text first Claude turn"),
    ];
    let first = selected_turn(selected, &history).await;
    history.push(Message::assistant_text(&text(&first)));
    history.push(Message::user("text normal continuation"));
    let second = selected_turn(selected, &history).await;
    history.push(Message::assistant_text(&text(&second)));

    selected = &other;
    history.push(Message::user("INTERVENING_USER_INSTRUCTION_1819"));
    let foreign = selected_turn(selected, &history).await;
    assert_eq!(text(&foreign), "OTHER_PROVIDER_RESPONSE_1819");
    history.push(Message::assistant_text(&text(&foreign)));
    assert_eq!(
        user_inputs(&fake).len(),
        2,
        "other provider must not use Claude stdin"
    );

    selected = &provider;
    history.push(Message::user("text back on Claude"));
    let returned = selected_turn(selected, &history).await;
    history.push(Message::assistant_text(&text(&returned)));
    history.push(Message::user("text after return"));
    selected_turn(selected, &history).await;

    let inputs = user_inputs(&fake);
    assert_eq!(inputs.len(), 4);
    assert_eq!(
        fake.launches().len(),
        1,
        "provider switch retains the child"
    );
    assert!(inputs.iter().all(|entry| entry["pid"] == inputs[0]["pid"]));
    assert!(input_text(&inputs[0]).contains("OLD_SEED_1819"));
    assert_eq!(input_text(&inputs[1]), "text normal continuation");
    let back = input_text(&inputs[2]);
    for marker in [
        "INTERVENING_USER_INSTRUCTION_1819",
        "OTHER_PROVIDER_RESPONSE_1819",
    ] {
        assert_eq!(
            back.matches(marker).count(),
            1,
            "missing/duplicated marker: {back}"
        );
    }
    assert!(
        !back.contains("OLD_SEED_1819"),
        "must send the gap, not the whole history"
    );
    assert!(
        !back.contains("Hello from fake claude"),
        "native response was already seen"
    );
    assert!(back.ends_with("text back on Claude"));
    assert_eq!(input_text(&inputs[3]), "text after return");
    provider.shutdown().await;
}

#[tokio::test]
async fn fresh_child_receives_the_same_provider_switch_markers() {
    let fake = Fake::new();
    let provider = fake.provider();
    let events = run_turn(
        &provider,
        &[
            Message::user("INTERVENING_USER_INSTRUCTION_1819"),
            Message::assistant_text("OTHER_PROVIDER_RESPONSE_1819"),
            Message::user("text fresh control"),
        ],
        None,
    )
    .await;
    assert!(errors(&events).is_empty(), "{events:?}");
    let inputs = user_inputs(&fake);
    let sent = input_text(&inputs[0]);
    assert!(sent.contains("INTERVENING_USER_INSTRUCTION_1819"));
    assert!(sent.contains("OTHER_PROVIDER_RESPONSE_1819"));
    provider.shutdown().await;
}

#[tokio::test]
async fn changed_prefix_starts_fresh_without_resuming_the_old_branch() {
    let fake = Fake::new();
    let provider = fake.provider();
    let first = run_turn(
        &provider,
        &[Message::user("text ORIGINAL_BRANCH_1819")],
        None,
    )
    .await;
    let events = run_turn(
        &provider,
        &[
            Message::user("EDITED_BRANCH_1819"),
            Message::assistant_text(&text(&first)),
            Message::user("text after edit"),
        ],
        None,
    )
    .await;
    assert!(errors(&events).is_empty(), "{events:?}");
    let launches = fake.launches();
    assert_eq!(launches.len(), 2);
    assert!(
        !launches[1]["argv"]
            .as_array()
            .unwrap()
            .iter()
            .any(|arg| arg.as_str().unwrap().starts_with("--resume="))
    );
    let inputs = user_inputs(&fake);
    assert_ne!(inputs[0]["pid"], inputs[1]["pid"]);
    let sent = input_text(&inputs[1]);
    assert!(sent.contains("EDITED_BRANCH_1819"));
    assert!(!sent.contains("ORIGINAL_BRANCH_1819"));
    provider.shutdown().await;
}

#[tokio::test]
async fn replaced_native_response_is_not_mistaken_for_a_seen_assistant_turn() {
    let fake = Fake::new();
    let provider = fake.provider();
    run_turn(&provider, &[Message::user("text")], None).await;
    run_turn(
        &provider,
        &[
            Message::user("text"),
            Message::assistant_text("OTHER_PROVIDER_REPLACED_NATIVE_RESPONSE_1819"),
            Message::user("text next"),
        ],
        None,
    )
    .await;
    assert_eq!(fake.launches().len(), 2);
    assert!(
        input_text(&user_inputs(&fake)[1]).contains("OTHER_PROVIDER_REPLACED_NATIVE_RESPONSE_1819")
    );
    provider.shutdown().await;
}

#[tokio::test]
async fn native_tool_result_continuation_sends_only_continue() {
    let fake = Fake::new();
    let provider = fake.provider();
    let first = run_turn(&provider, &[Message::user("tool")], None).await;
    assert!(errors(&first).is_empty(), "{first:?}");
    let mut blocks = vec![ContentBlock::Text {
        text: text(&first),
        cache_control: None,
    }];
    let mut tool = None;
    let mut result_blocks = Vec::new();
    for event in &first {
        match event {
            StreamEvent::ToolUseStart { id, name } => tool = Some((id.clone(), name.clone())),
            StreamEvent::ToolInputDelta(input) => {
                let (id, name) = tool.take().unwrap();
                blocks.push(ContentBlock::ToolUse {
                    id,
                    name,
                    input: serde_json::from_str(input).unwrap(),
                    thought_signature: None,
                });
            }
            StreamEvent::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => {
                result_blocks.push(ContentBlock::ToolResult {
                    tool_use_id: tool_use_id.clone(),
                    content: content.clone(),
                    is_error: Some(*is_error),
                });
            }
            _ => {}
        }
    }
    assert!(
        !result_blocks.is_empty(),
        "control must include an actual native tool result"
    );
    let history = [
        Message::user("tool"),
        Message {
            role: Role::Assistant,
            content: blocks,
            timestamp: None,
            tool_duration_ms: None,
        },
        Message {
            role: Role::User,
            content: result_blocks,
            timestamp: None,
            tool_duration_ms: None,
        },
    ];
    let continued = run_turn(&provider, &history, None).await;
    assert!(errors(&continued).is_empty(), "{continued:?}");
    let inputs = user_inputs(&fake);
    assert_eq!(inputs.len(), 2);
    assert_eq!(inputs[0]["pid"], inputs[1]["pid"]);
    assert_eq!(input_text(&inputs[1]), "Continue.");
    assert_eq!(fake.launches().len(), 1);
    provider.shutdown().await;
}
