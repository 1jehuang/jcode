use super::*;

#[test]
#[ignore = "isolated CPU/allocator comparison, run serially with --nocapture"]
fn export_sharing_cpu_allocator_probe() {
    fn live() -> u64 {
        crate::process_memory::allocator_info()
            .stats
            .and_then(|stats| stats.allocated_bytes)
            .expect("allocator statistics required for isolated probe")
    }
    for dirty in [false, true] {
        let mut session = Session::create_with_id("export-probe".into(), None, None);
        for _ in 0..16 {
            let mut text = "safe payload ".repeat(8192);
            if dirty {
                text.push_str(" OPENROUTER_API_KEY=sk-or-v1-abcdefghijklmnopqrstuvwxyz0123456789");
            }
            session.add_message(
                Role::User,
                vec![ContentBlock::Text {
                    text,
                    cache_control: None,
                }],
            );
        }
        session.add_message(
            Role::User,
            vec![ContentBlock::Image {
                media_type: "image/png".into(),
                data: "a".repeat(2 * 1024 * 1024),
            }],
        );
        // Warm the regexes and allocator paths before measuring retained bytes.
        drop(session.redacted_for_export());
        let baseline = live();
        let started = std::time::Instant::now();
        let legacy: Vec<_> = (0..8)
            .map(|_| {
                let mut exported = session.clone();
                for message in &mut exported.messages {
                    legacy_redact(&mut message.content);
                }
                exported
            })
            .collect();
        let legacy_elapsed = started.elapsed();
        std::hint::black_box(&legacy);
        let legacy_growth = live().saturating_sub(baseline);
        drop(legacy);
        let baseline = live();
        let started = std::time::Instant::now();
        let shared: Vec<_> = (0..8).map(|_| session.redacted_for_export()).collect();
        let shared_elapsed = started.elapsed();
        std::hint::black_box(&shared);
        let shared_growth = live().saturating_sub(baseline);
        assert!(shared.iter().all(|exported| {
            exported.messages[16]
                .content
                .shares_storage_with(&session.messages[16].content)
        }));
        println!(
            "export_probe dirty={dirty} snapshots=8 text_messages=16 text_bytes_each=106496 opaque_image_bytes=2097152 legacy_elapsed_us={} shared_elapsed_us={} legacy_allocator_growth={legacy_growth} shared_allocator_growth={shared_growth}; isolated probe, not production RSS",
            legacy_elapsed.as_micros(),
            shared_elapsed.as_micros()
        );
    }
}

// The pre-optimization eager pass, retained independently as an output oracle.
fn legacy_redact(blocks: &mut [ContentBlock]) {
    for block in blocks {
        match block {
            ContentBlock::Text { text, .. }
            | ContentBlock::Reasoning { text }
            | ContentBlock::ReasoningTrace { text } => *text = crate::message::redact_secrets(text),
            ContentBlock::AnthropicThinking { thinking, .. } => {
                *thinking = crate::message::redact_secrets(thinking);
            }
            ContentBlock::OpenAIReasoning { summary, .. } => {
                for text in summary {
                    *text = crate::message::redact_secrets(text);
                }
            }
            ContentBlock::ToolResult { content, .. } => {
                *content = crate::message::redact_secrets(content)
            }
            ContentBlock::ToolUse { input, .. } => redact_json_value(input),
            ContentBlock::ProviderNative { item, .. } => redact_json_value(item),
            ContentBlock::Image { .. }
            | ContentBlock::OpenAICompaction { .. }
            | ContentBlock::ToolReference { .. } => {}
        }
    }
}

fn blocks(text: &str, json: serde_json::Value) -> Vec<ContentBlock> {
    vec![
        ContentBlock::Text {
            text: text.into(),
            cache_control: None,
        },
        ContentBlock::Reasoning { text: text.into() },
        ContentBlock::ReasoningTrace { text: text.into() },
        ContentBlock::AnthropicThinking {
            thinking: text.into(),
            signature: text.into(),
        },
        ContentBlock::OpenAIReasoning {
            id: "id".into(),
            summary: vec![text.into()],
            encrypted_content: Some(text.into()),
            status: None,
        },
        ContentBlock::ToolUse {
            id: "id".into(),
            name: "tool".into(),
            input: json.clone(),
            thought_signature: Some(text.into()),
        },
        ContentBlock::ToolResult {
            tool_use_id: "id".into(),
            content: text.into(),
            is_error: None,
        },
        ContentBlock::Image {
            media_type: "image/png".into(),
            data: text.into(),
        },
        ContentBlock::OpenAICompaction {
            encrypted_content: text.into(),
        },
        ContentBlock::ToolReference {
            tool_use_id: "id".into(),
            tool_name: text.into(),
        },
        ContentBlock::ProviderNative {
            provider: "openai".into(),
            item: json,
        },
    ]
}

#[test]
fn export_sharing_matches_eager_redaction_for_every_variant_and_json_value() {
    let secret = "OPENROUTER_API_KEY=sk-or-v1-abcdefghijklmnopqrstuvwxyz0123456789";
    for text in ["safe unchanged content", "[REDACTED_SECRET]", secret] {
        for value in [
            serde_json::Value::Null,
            serde_json::json!(false),
            serde_json::json!(42),
            serde_json::json!("short secret"),
            serde_json::json!([{"token": "nested"}]),
            serde_json::json!({"nested": secret}),
        ] {
            for json in [
                value.clone(),
                serde_json::json!({"API-key": value, "safe": [text], "password": "[REDACTED_SECRET]"}),
            ] {
                // Test each variant independently, so a changed sibling cannot
                // hide a false negative in the immutable inspection pass.
                for block in blocks(text, json) {
                    let mut expected = vec![block.clone()];
                    legacy_redact(&mut expected);
                    let changed = serde_json::to_vec(&expected).unwrap()
                        != serde_json::to_vec(&vec![block.clone()]).unwrap();
                    let mut session = Session::create_with_id("export-sharing".into(), None, None);
                    session.add_message(Role::User, vec![block]);
                    let original = serde_json::to_vec(&session.messages).unwrap();
                    let exported = session.redacted_for_export();
                    assert_eq!(
                        serde_json::to_vec(&exported.messages[0].content).unwrap(),
                        serde_json::to_vec(&expected).unwrap()
                    );
                    assert_eq!(
                        exported.messages[0]
                            .content
                            .shares_storage_with(&session.messages[0].content),
                        !changed
                    );
                    assert_eq!(serde_json::to_vec(&session.messages).unwrap(), original);
                }
            }
        }
    }
}

#[test]
fn export_sharing_large_safe_payloads_stay_shared_and_mutations_are_isolated() {
    let mut session = Session::create_with_id("export-large".into(), None, None);
    session.add_message(
        Role::User,
        blocks(
            &"safe payload ".repeat(128 * 1024),
            serde_json::json!({"token": "[REDACTED_SECRET]", "data": "safe"}),
        ),
    );
    let mut exported = session.redacted_for_export();
    let sibling = exported.clone();
    assert!(
        exported.messages[0]
            .content
            .shares_storage_with(&session.messages[0].content)
    );
    if let ContentBlock::Text { text, .. } = &mut exported.messages[0].content[0] {
        text.push_str(" edited");
    }
    assert!(
        !exported.messages[0]
            .content
            .shares_storage_with(&session.messages[0].content)
    );
    assert!(
        sibling.messages[0]
            .content
            .shares_storage_with(&session.messages[0].content)
    );
    if let ContentBlock::Image { data, .. } = &mut session.messages[0].content[7] {
        data.clear();
    }
    assert!(
        !sibling.messages[0]
            .content
            .shares_storage_with(&session.messages[0].content)
    );
    assert!(
        matches!(&sibling.messages[0].content[7], ContentBlock::Image { data, .. } if !data.is_empty())
    );
}
