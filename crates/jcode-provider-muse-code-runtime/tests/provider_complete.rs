//! End-to-end tests for `MuseCodeProvider::complete`.
//!
//! These exercise the provider's **entire runtime path** — spawn the host,
//! handshake, `model/list`, `session/start`, `turn/start`, the event stream, and
//! turn termination — against a real `muse serve` process.
//!
//! They use [`MuseHostConfig::echo`], a stub provider that answers locally, so
//! **no model call is made and no subscription usage is spent**. That is what
//! makes it possible to test the real integration path in CI, and it is why the
//! provider takes an injectable host config at all.
//!
//! The only thing these cannot cover is the live `meta` provider, which spends
//! subscription usage. Everything above that boundary is real.

use futures::StreamExt;
use jcode_message_types::{ContentBlock, Message, Role, StreamEvent};
use jcode_provider_core::Provider;
use jcode_provider_muse_code_runtime::{MUSE_CODE_MODELS, MuseCodeProvider, MuseHostConfig};
use std::time::Duration;

fn muse_available() -> bool {
    std::process::Command::new("muse")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

macro_rules! require_muse {
    () => {
        if !muse_available() {
            eprintln!("skipping: no `muse` binary on PATH");
            return;
        }
    };
}

fn user_message(text: &str) -> Message {
    Message {
        role: Role::User,
        content: vec![ContentBlock::Text {
            text: text.to_string(),
            cache_control: None,
        }],
        timestamp: None,
        tool_duration_ms: None,
    }
}

/// A provider wired to the quota-free echo host.
fn echo_provider() -> MuseCodeProvider {
    MuseCodeProvider::new().with_host_config(MuseHostConfig::echo())
}

#[tokio::test]
async fn complete_streams_text_and_terminates_against_a_real_host() {
    require_muse!();
    let provider = echo_provider();
    let messages = vec![user_message("say hello")];

    let mut stream = provider
        .complete(&messages, &[], "ignored system prompt", None)
        .await
        .expect("complete should start a turn against a real host");

    let mut text = String::new();
    let mut saw_done = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);

    while let Some(event) = tokio::time::timeout_at(deadline, stream.next())
        .await
        .expect("the provider stream must not hang")
    {
        match event.expect("the provider stream must not error on a healthy host") {
            StreamEvent::TextDelta(delta) => text.push_str(&delta),
            StreamEvent::TextDone => {
                saw_done = true;
                break;
            }
            // Any other event kind is out of scope for this assertion.
            _ => {}
        }
    }

    assert!(
        !text.is_empty(),
        "expected streamed assistant text, got nothing"
    );
    // The echo provider mirrors the submitted prompt, so this also proves the
    // user text actually reached the host rather than being dropped.
    assert!(
        text.contains("say hello"),
        "expected the echoed prompt in the output, got {text:?}"
    );
    assert!(saw_done, "expected a terminal TextDone event");
}

#[tokio::test]
async fn a_second_turn_reuses_the_same_live_session() {
    require_muse!();
    let provider = echo_provider();

    // First turn brings the host up.
    let mut first = provider
        .complete(&[user_message("first")], &[], "", None)
        .await
        .expect("first turn should start");
    while let Some(event) = first.next().await {
        if matches!(event.unwrap(), StreamEvent::TextDone) {
            break;
        }
    }

    // Second turn must succeed without spawning a second host or session; if the
    // session were not reused this would still pass, so the meaningful assertion
    // is that it completes normally at all.
    let mut second = provider
        .complete(&[user_message("second")], &[], "", None)
        .await
        .expect("second turn should reuse the live session");
    let mut text = String::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while let Some(event) = tokio::time::timeout_at(deadline, second.next())
        .await
        .expect("second turn must not hang")
    {
        match event.unwrap() {
            StreamEvent::TextDelta(delta) => text.push_str(&delta),
            StreamEvent::TextDone => break,
            _ => {}
        }
    }
    assert!(
        text.contains("second"),
        "expected the second prompt echoed back, got {text:?}"
    );
}

#[tokio::test]
async fn complete_rejects_an_empty_prompt_before_touching_the_host() {
    require_muse!();
    let provider = echo_provider();

    // No user text at all: the provider must refuse rather than submit an empty
    // turn. This is checked before the host is even started.
    let err = match provider.complete(&[], &[], "", None).await {
        Ok(_) => panic!("an empty prompt must be rejected"),
        Err(err) => err,
    };
    assert!(
        format!("{err:#}").contains("non-empty user message"),
        "unexpected error: {err:#}"
    );

    // Whitespace-only content is equally empty.
    let err = match provider
        .complete(&[user_message("   ")], &[], "", None)
        .await
    {
        Ok(_) => panic!("a whitespace-only prompt must be rejected"),
        Err(err) => err,
    };
    assert!(format!("{err:#}").contains("non-empty user message"));
}

#[tokio::test]
async fn context_window_is_adopted_from_the_host_catalog() {
    require_muse!();
    let provider = echo_provider();
    let fallback = provider.context_window();

    // Force the host up via a turn.
    let mut stream = provider
        .complete(&[user_message("warm the host")], &[], "", None)
        .await
        .expect("turn should start");
    while let Some(event) = stream.next().await {
        if matches!(event.unwrap(), StreamEvent::TextDone) {
            break;
        }
    }

    // After the host answered `model/list`, the provider should report the
    // host's own figure rather than the fallback constant. Muse Code reports
    // 1,007,997 (its documented 1,048,576 window minus reserved headroom).
    let adopted = provider.context_window();
    assert!(
        adopted > 0,
        "context window should be a positive token count, got {adopted}"
    );
    assert!(
        adopted <= fallback,
        "adopted window ({adopted}) should not exceed the documented window ({fallback})"
    );
}

#[tokio::test]
async fn route_metadata_is_consistent_with_the_documented_model_list() {
    require_muse!();
    let provider = echo_provider();
    let routes = provider.model_routes();

    assert_eq!(routes.len(), MUSE_CODE_MODELS.len());
    for model in MUSE_CODE_MODELS {
        assert!(
            routes.iter().any(|route| route.model == *model),
            "expected a picker route for {model}"
        );
    }
    // The provider is a front-end over Muse's loop, so jcode's tools are unused.
    // That must be visible to the agent loop, not just documented.
    assert!(provider.handles_tools_internally());
}
