//! Opt-in acceptance test of the real public provider and installed browser bridge.
//! Sends only tiny diagnostics in owned Temporary Chats. Never runs by default.
use futures::StreamExt;
use jcode_message_types::{ContentBlock, Message, Role, StreamEvent};
use jcode_provider_core::Provider;
use jcode_provider_openai_runtime::OpenAIProvider;

async fn response(provider: &OpenAIProvider, model: &str, effort: &str) -> anyhow::Result<()> {
    provider.set_model(model)?;
    provider.set_reasoning_effort(effort)?;
    assert_eq!(provider.transport().as_deref(), Some("browser"));
    let messages = vec![Message {
        role: Role::User,
        content: vec![ContentBlock::Text {
            text: "Reply exactly JCODE_BRIDGE_OK.".to_string(),
            cache_control: None,
        }],
        timestamp: None,
        tool_duration_ms: None,
    }];
    println!(
        "JCODE_PROGRESS {{\"message\":\"Testing {model} at {effort} through the actual web provider\"}}"
    );
    let mut stream = provider.complete(
        &messages,
        &[],
        "This is a browser bridge diagnostic. Reply exactly JCODE_BRIDGE_OK and nothing else. Do not call tools.",
        None,
    ).await?;
    let mut text = String::new();
    let mut browser = false;
    let mut finished = false;
    while let Some(event) = stream.next().await {
        match event? {
            StreamEvent::ConnectionType { connection } => {
                assert_eq!(connection, "browser/chatgpt-web");
                browser = true;
            }
            StreamEvent::TextDelta(delta) => text.push_str(&delta),
            StreamEvent::MessageEnd { .. } => finished = true,
            StreamEvent::StatusDetail { detail } => println!(
                "JCODE_PROGRESS {{\"message\":{}}}",
                serde_json::to_string(&detail)?
            ),
            StreamEvent::ToolUseStart { .. } => {
                anyhow::bail!("Diagnostic unexpectedly requested a tool")
            }
            _ => {}
        }
    }
    assert!(
        browser && finished,
        "Public stream did not complete through the browser route"
    );
    assert_eq!(
        text.trim(),
        "JCODE_BRIDGE_OK",
        "Unexpected temporary diagnostic response for {model}"
    );
    println!(
        "JCODE_CHECKPOINT {{\"message\":\"{model} {effort}: actual response, selection checks, restoration, and owned-tab cleanup passed\"}}"
    );
    Ok(())
}

#[tokio::test]
#[ignore = "requires JCODE_CHATGPT_WEB_LIVE=1, an existing connected logged-in bridge, and permission to send tiny temporary diagnostics"]
async fn chatgpt_web_live_public_provider_all_available_models() -> anyhow::Result<()> {
    anyhow::ensure!(
        std::env::var("JCODE_CHATGPT_WEB_LIVE").as_deref() == Ok("1"),
        "Explicit live-test authorization is required"
    );
    let provider = OpenAIProvider::new_browser_only();
    provider.prefetch_models().await?;
    let models = jcode_base::provider::chatgpt_web::cached_models()
        .into_iter()
        .filter(|model| model.available)
        .collect::<Vec<_>>();
    anyhow::ensure!(
        !models.is_empty(),
        "No available ChatGPT models were discovered"
    );
    for model in &models {
        assert!(
            provider
                .available_models_for_switching()
                .contains(&model.model)
        );
        // Efforts are model-dependent, so select after changing model.
        provider.set_model(&model.model)?;
        let effort = if provider.available_efforts().contains(&"medium") {
            "medium"
        } else if provider.available_efforts().contains(&"none") {
            "none"
        } else {
            anyhow::bail!(
                "No bounded diagnostic effort is available for {}",
                model.model
            )
        };
        response(&provider, &model.model, effort).await?;
    }
    if std::env::var("JCODE_CHATGPT_WEB_TEST_MAX").as_deref() == Ok("1") {
        if let Some(model) = models.iter().find(|model| {
            jcode_base::provider::chatgpt_web::cached_reasoning_options(&model.model)
                .iter()
                .any(|effort| effort.available && effort.value == "max")
        }) {
            response(&provider, &model.model, "max").await?;
        }
    }
    assert!(
        provider.set_model("gpt-5.6-sol").is_err(),
        "Browser-only provider must not silently use API/OAuth models"
    );
    Ok(())
}

/// Opt-in: an agentic turn must come back as a jcode tool call, not text or an
/// error. JCODE_CHATGPT_WEB_LIVE_MODEL / _EFFORT pick the route (default the
/// first discovered model at max Power, the slowest and most verbose case).
#[tokio::test]
#[ignore = "requires JCODE_CHATGPT_WEB_LIVE=1, an existing connected logged-in bridge, and permission to send tiny temporary diagnostics"]
async fn chatgpt_web_live_agentic_turn_returns_a_tool_call() -> anyhow::Result<()> {
    anyhow::ensure!(
        std::env::var("JCODE_CHATGPT_WEB_LIVE").as_deref() == Ok("1"),
        "Explicit live-test authorization is required"
    );
    let provider = OpenAIProvider::new_browser_only();
    provider.prefetch_models().await?;
    let model = match std::env::var("JCODE_CHATGPT_WEB_LIVE_MODEL") {
        Ok(model) => model,
        Err(_) => jcode_base::provider::chatgpt_web::cached_models()
            .into_iter()
            .find(|model| model.available)
            .map(|model| model.model)
            .ok_or_else(|| anyhow::anyhow!("No available ChatGPT models were discovered"))?,
    };
    let effort = std::env::var("JCODE_CHATGPT_WEB_LIVE_EFFORT").unwrap_or_else(|_| "max".into());
    provider.set_model(&model)?;
    provider.set_reasoning_effort(&effort)?;
    let tools = vec![jcode_message_types::ToolDefinition {
        name: "read".to_string(),
        description: "Read a text file from the local workspace.".to_string(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "file_path": {"type": "string"},
                "intent": {"type": "string"}
            },
            "required": ["file_path", "intent"]
        }),
        defer_loading: false,
    }];
    let messages = vec![Message {
        role: Role::User,
        content: vec![ContentBlock::Text {
            text: "What does the file notes/JCODE_LIVE.txt say? You have not read it yet."
                .to_string(),
            cache_control: None,
        }],
        timestamp: None,
        tool_duration_ms: None,
    }];
    println!("JCODE_PROGRESS {{\"message\":\"Agentic turn on {model} at {effort}\"}}");
    let mut stream = provider
        .complete(
            &messages,
            &tools,
            "You are a coding agent. Use tools to inspect files before answering.",
            None,
        )
        .await?;
    let (mut tool, mut input, mut text) = (None, String::new(), String::new());
    while let Some(event) = stream.next().await {
        match event? {
            StreamEvent::ToolUseStart { name, .. } => tool = Some(name),
            StreamEvent::ToolInputDelta(delta) => input.push_str(&delta),
            StreamEvent::TextDelta(delta) => text.push_str(&delta),
            StreamEvent::StatusDetail { detail } => println!(
                "JCODE_PROGRESS {{\"message\":{}}}",
                serde_json::to_string(&detail)?
            ),
            _ => {}
        }
    }
    assert_eq!(
        tool.as_deref(),
        Some("read"),
        "expected a read tool call, got text: {text}"
    );
    assert!(input.contains("JCODE_LIVE.txt"), "tool input: {input}");
    println!(
        "JCODE_CHECKPOINT {{\"message\":\"{model} {effort}: tool call read {input} (lead-in text: {:?})\"}}",
        text.trim()
    );
    Ok(())
}
