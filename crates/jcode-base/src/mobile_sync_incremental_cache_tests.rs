use super::*;

fn legacy(session: &Session, progress: &SessionProgress) -> (Vec<WireMessage>, u64) {
    let mut messages = wire_messages(session, 0);
    let fingerprint = wire_fingerprint(&messages, session.messages.len());
    if progress.wire_version == WIRE_VERSION
        && progress.uploaded <= session.messages.len()
        && progress.fingerprint == Some(wire_fingerprint(&messages, progress.uploaded))
    {
        messages.retain(|message| message.seq >= progress.uploaded);
    }
    (messages, fingerprint)
}

fn text(text: &str) -> ContentBlock {
    ContentBlock::Text {
        text: text.into(),
        cache_control: None,
    }
}
fn fixture() -> Session {
    let mut session = Session::create_with_id("prefix-cache".into(), None, None);
    session.add_message(Role::User, vec![text("question \"🦀\n")]);
    session.add_message(Role::Assistant, vec![
        text("working"),
        ContentBlock::ToolUse { id: "call".into(), name: "bash".into(), input: serde_json::json!({"command":"echo hello", "nested":[null, true, -0.0, {"v":"é"}]}), thought_signature: None },
    ]);
    session.add_message(
        Role::User,
        vec![ContentBlock::ToolResult {
            tool_use_id: "call".into(),
            content: "result\n".repeat(2048),
            is_error: Some(false),
        }],
    );
    session.add_message(
        Role::User,
        vec![text("<system-reminder>hidden</system-reminder>")],
    );
    session
}
fn progress(session: &Session) -> SessionProgress {
    SessionProgress {
        wire_version: WIRE_VERSION,
        uploaded: session.messages.len(),
        fingerprint: Some(legacy(session, &SessionProgress::default()).1),
        ..Default::default()
    }
}
fn check(cache: &mut PrefixCache, session: &Session, progress: &SessionProgress) {
    assert_eq!(cache.messages(session, progress), legacy(session, progress));
}

#[test]
fn prefix_cache_matches_legacy_after_reload_append_edits_and_truncation() {
    let mut session = fixture();
    let mut cache = PrefixCache::default();
    check(&mut cache, &session, &SessionProgress::default());
    let uploaded = progress(&session);
    check(&mut cache, &session, &uploaded);
    // Mobile sync reloads a new allocation from disk on each changed pass.
    session = serde_json::from_slice(&serde_json::to_vec(&session).unwrap()).unwrap();
    session.custom_title = Some("metadata-only".into());
    check(&mut cache, &session, &uploaded);
    session.add_message(Role::Assistant, vec![text("tail")]);
    check(&mut cache, &session, &uploaded);
    let uploaded = progress(&session);
    check(&mut cache, &session, &uploaded);
    session.messages[0].content = vec![text("rewritten")].into();
    check(&mut cache, &session, &uploaded);
    let uploaded = progress(&session);
    if let ContentBlock::ToolUse { input, .. } = &mut session.messages[1].content[1] {
        *input = serde_json::json!({"command":"echo changed", "nested":[0.0]});
    }
    check(&mut cache, &session, &uploaded);
    let uploaded = progress(&session);
    session.messages[2].timestamp = chrono::DateTime::from_timestamp(123, 0);
    session.messages[2].display_role = Some(crate::session::StoredDisplayRole::System);
    check(&mut cache, &session, &uploaded);
    session.truncate_messages(1);
    check(&mut cache, &session, &uploaded);
    check(
        &mut cache,
        &session,
        &SessionProgress {
            wire_version: WIRE_VERSION - 1,
            ..uploaded.clone()
        },
    );
    check(
        &mut cache,
        &session,
        &SessionProgress {
            fingerprint: None,
            ..uploaded
        },
    );
}

#[test]
fn external_markdown_images_bypass_prefix_cache_including_missing_files() {
    let mut session = fixture();
    session.add_message(
        Role::Assistant,
        vec![text("![future](file:///not-present.png)")],
    );
    let mut cache = PrefixCache::default();
    check(&mut cache, &session, &SessionProgress::default());
    assert!(cache.0.is_empty());
    check(&mut cache, &session, &progress(&session));
    assert!(cache.0.is_empty());
    session.messages.pop();
    session.add_message(
        Role::Assistant,
        vec![ContentBlock::ToolUse {
            id: "external-summary".into(),
            name: "bash".into(),
            input: serde_json::json!({"command": "![future](file:///not-present.png)"}),
            thought_signature: None,
        }],
    );
    check(&mut cache, &session, &SessionProgress::default());
    assert!(cache.0.is_empty());
}

#[test]
fn every_wire_metadata_and_tool_field_invalidates_the_prefix() {
    for field in 0..8 {
        let mut session = fixture();
        let mut cache = PrefixCache::default();
        cache.messages(&session, &SessionProgress::default());
        let uploaded = progress(&session);
        match field {
            0 => session.messages[0].role = Role::Assistant,
            1 => session.messages[0].timestamp = chrono::DateTime::from_timestamp(1, 123),
            2 => session.messages[0].display_role = Some(crate::session::StoredDisplayRole::System),
            3 | 4 => {
                if let ContentBlock::ToolUse { id, name, .. } = &mut session.messages[1].content[1]
                {
                    if field == 3 {
                        *id = "new-id".into();
                    } else {
                        *name = "new-tool".into();
                    }
                }
            }
            5..=7 => {
                if let ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                    is_error,
                } = &mut session.messages[2].content[0]
                {
                    match field {
                        5 => *tool_use_id = "new-result-id".into(),
                        6 => *content = "different result".into(),
                        _ => *is_error = Some(true),
                    }
                }
            }
            _ => unreachable!(),
        }
        check(&mut cache, &session, &uploaded);
    }
}

#[test]
fn source_validation_distinguishes_json_spelling_and_inline_images() {
    let mut a = SourceHasher::default();
    let mut b = SourceHasher::default();
    a.json(&serde_json::json!(-0.0));
    b.json(&serde_json::json!(0.0));
    assert_ne!(a.digest(), b.digest());
    let mut session = fixture();
    session.add_message(
        Role::User,
        vec![ContentBlock::Image {
            media_type: "image/png".into(),
            data: "invalid-a".into(),
        }],
    );
    let mut first = SourceHasher::default();
    assert!(first.extend(&session.messages, 0));
    if let ContentBlock::Image { data, .. } = &mut session.messages.last_mut().unwrap().content[0] {
        *data = "invalid-b".into();
    }
    let mut second = SourceHasher::default();
    assert!(second.extend(&session.messages, 0));
    assert_ne!(first.digest(), second.digest());
    let mut cache = PrefixCache::default();
    check(&mut cache, &session, &SessionProgress::default());
    check(&mut cache, &session, &progress(&session));
}

#[test]
fn failed_upload_progress_never_reuses_an_unuploaded_prefix() {
    let mut session = fixture();
    let mut cache = PrefixCache::default();
    check(&mut cache, &session, &SessionProgress::default());
    let uploaded = progress(&session);
    session.add_message(Role::Assistant, vec![text("first unuploaded tail")]);
    check(&mut cache, &session, &uploaded);
    // A failed HTTP request leaves persisted progress behind the cache.
    // Repeated retries and further appends must still include that tail.
    check(&mut cache, &session, &uploaded);
    session.add_message(Role::Assistant, vec![text("second unuploaded tail")]);
    check(&mut cache, &session, &uploaded);
    let mut stale = uploaded.clone();
    stale.fingerprint = Some(uploaded.fingerprint.unwrap().wrapping_add(1));
    check(&mut cache, &session, &stale);
}

#[test]
fn real_inline_image_replacement_invalidates_cached_prefix() {
    use base64::Engine;
    let image = |pixel: u8| {
        let mut bytes = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
            2,
            2,
            image::Rgb([pixel, 0, 0]),
        ))
        .write_to(&mut bytes, image::ImageFormat::Png)
        .unwrap();
        ContentBlock::Image {
            media_type: "image/png".into(),
            data: base64::engine::general_purpose::STANDARD.encode(bytes.into_inner()),
        }
    };
    let mut session = fixture();
    session.add_message(Role::User, vec![image(0)]);
    let mut cache = PrefixCache::default();
    check(&mut cache, &session, &SessionProgress::default());
    let uploaded = progress(&session);
    check(&mut cache, &session, &uploaded);
    session.messages.last_mut().unwrap().content = vec![image(255)].into();
    assert_ne!(progress(&session).fingerprint, uploaded.fingerprint);
    check(&mut cache, &session, &uploaded);
}

#[test]
fn bounded_validation_preserves_truncated_results_and_rejects_expensive_sources() {
    let mut session = fixture();
    let content = "🦀".repeat(MAX_TOOL_RESULT_CHARS + 128);
    session.add_message(
        Role::User,
        vec![ContentBlock::ToolResult {
            tool_use_id: "long-result".into(),
            content,
            is_error: None,
        }],
    );
    let mut cache = PrefixCache::default();
    check(&mut cache, &session, &SessionProgress::default());
    let uploaded = progress(&session);
    if let ContentBlock::ToolResult { content, .. } =
        &mut session.messages.last_mut().unwrap().content[0]
    {
        content.push_str("invisible tail");
    }
    check(&mut cache, &session, &uploaded);
    if let ContentBlock::ToolResult { content, .. } =
        &mut session.messages.last_mut().unwrap().content[0]
    {
        *content = "🦀".repeat(MAX_TOOL_RESULT_CHARS);
    }
    check(&mut cache, &session, &uploaded);
    session.add_message(
        Role::Assistant,
        vec![text(&"x".repeat(MAX_TEXT_CHARS * 4 + 1))],
    );
    check(&mut cache, &session, &SessionProgress::default());
    assert!(cache.0.is_empty());
    let mut source = SourceHasher::default();
    assert!(!source.json(&serde_json::json!("x".repeat(MAX_TOOL_INPUT_BYTES * 4))));
    assert!(!source.json(&serde_json::json!(vec![0; MAX_TOOL_INPUT_BYTES])));
}

#[test]
fn cache_retains_only_bounded_session_metadata() {
    let mut cache = PrefixCache::default();
    for n in 0..MAX_SESSIONS + 10 {
        let mut session = fixture();
        session.id = format!("session-{n}");
        check(&mut cache, &session, &SessionProgress::default());
        assert!(cache.0.len() <= MAX_SESSIONS);
    }
    assert!(cache.0.iter().all(|entry| entry.id != "session-0"));
    let mut session = fixture();
    session.id = "x".repeat(129);
    check(&mut cache, &session, &SessionProgress::default());
    assert!(cache.0.iter().all(|entry| entry.id != session.id));
}

#[test]
#[ignore = "large truncated input regression benchmark"]
fn benchmark_large_truncated_prefix() {
    use std::{hint::black_box, time::Instant};
    for mode in 0..4 {
        let mut session = fixture();
        for n in 0..8 {
            session.add_message(
                Role::Assistant,
                if mode == 1 {
                    vec![
                        ContentBlock::ToolUse {
                            id: format!("large-{n}"),
                            name: "bash".into(),
                            input: serde_json::json!({"command": "x".repeat(1024 * 1024)}),
                            thought_signature: None,
                        },
                        ContentBlock::ToolResult {
                            tool_use_id: format!("large-{n}"),
                            content: "o".repeat(1024 * 1024),
                            is_error: None,
                        },
                    ]
                } else if mode == 2 {
                    vec![ContentBlock::ToolResult {
                        tool_use_id: format!("large-{n}"),
                        content: "o".repeat(1024 * 1024),
                        is_error: None,
                    }]
                } else if mode == 3 {
                    vec![ContentBlock::ToolUse {
                        id: format!("array-{n}"),
                        name: "bash".into(),
                        input: serde_json::json!({"data": vec![0; 65536]}),
                        thought_signature: None,
                    }]
                } else {
                    vec![text(&"p".repeat(1024 * 1024))]
                },
            );
        }
        let mut cache = PrefixCache::default();
        check(&mut cache, &session, &SessionProgress::default());
        let uploaded = progress(&session);
        session.add_message(Role::Assistant, vec![text("tail")]);
        let start = Instant::now();
        let old = black_box(legacy(&session, &uploaded));
        let baseline = start.elapsed();
        let start = Instant::now();
        let new = black_box(cache.messages(&session, &uploaded));
        let cached = start.elapsed();
        assert_eq!(new, old);
        eprintln!(
            "MOBILE_LARGE mode={mode} old={baseline:?} cached={cached:?} ratio={:.2}x",
            baseline.as_secs_f64() / cached.as_secs_f64()
        );
    }
}

#[test]
#[ignore = "paired unchanged-profile mobile prefix conversion benchmark"]
fn benchmark_validated_mobile_prefix() {
    use std::{hint::black_box, time::Instant};
    for structured in [false, true] {
        let mut session = fixture();
        for n in 0..128 {
            let blocks = if structured {
                vec![
                    ContentBlock::ToolUse {
                        id: format!("call-{n}"),
                        name: "bash".into(),
                        input: serde_json::json!({"command": "x🦀\n\"".repeat(1024)}),
                        thought_signature: None,
                    },
                    ContentBlock::ToolResult {
                        tool_use_id: format!("call-{n}"),
                        content: "output\n".repeat(1024),
                        is_error: None,
                    },
                ]
            } else {
                vec![text(&"text 🦀\n\"".repeat(128))]
            };
            session.add_message(Role::Assistant, blocks);
        }
        let mut cache = PrefixCache::default();
        check(&mut cache, &session, &SessionProgress::default());
        let uploaded = progress(&session);
        session.add_message(Role::Assistant, vec![text("new tail")]);
        for round in 0..4 {
            // Refill with exactly the successfully uploaded prefix, as a real
            // append does. Cache state never advances past stale progress.
            let mut prefix = session.clone();
            prefix.messages.pop();
            cache.messages(&prefix, &SessionProgress::default());
            let start = Instant::now();
            let old = black_box(legacy(black_box(&session), black_box(&uploaded)));
            let baseline = start.elapsed();
            let start = Instant::now();
            let new = black_box(cache.messages(black_box(&session), black_box(&uploaded)));
            let cached = start.elapsed();
            assert_eq!(new, old);
            eprintln!(
                "MOBILE_PREFIX structured={structured} round={round} old={baseline:?} cached={cached:?} ratio={:.2}x",
                baseline.as_secs_f64() / cached.as_secs_f64()
            );
        }
    }
}
