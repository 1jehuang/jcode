use super::*;
use jcode_session_types::StoredMessage;

// The pre-cache implementation remains the output oracle, including storage's
// backup recovery behavior rather than a permissive top-level enabled parser.
fn uncached_mobile_state() -> MobileSyncState {
    let path = state_path().unwrap();
    if !path.exists() {
        return MobileSyncState::default();
    }
    storage::read_json(&path).unwrap_or_default()
}

#[test]
fn generated_images_are_bounded_private_thumbnails() {
    use base64::Engine;
    let _lock = storage::lock_test_env();
    let home = tempfile::tempdir().unwrap();
    let _env = TestEnv::set(&[("JCODE_HOME", home.path().as_os_str())]);
    let root = home.path().join("generated-images");
    std::fs::create_dir(&root).unwrap();
    let image_path = root.join("generated.png");
    image::RgbImage::from_pixel(1500, 1200, image::Rgb([30, 80, 200]))
        .save(&image_path)
        .unwrap();
    let markdown = format!("![Generated image]({})", image_path.display());
    let images = generated_images(&format!("{markdown}\n{markdown}"));
    assert_eq!(
        images.len(),
        1,
        "duplicate references must not duplicate bytes"
    );
    assert_eq!(images[0].media_type, "image/jpeg");
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(&images[0].data)
        .unwrap();
    assert!(bytes.len() <= 256 * 1024);
    let decoded = image::load_from_memory(&bytes).unwrap();
    assert!(decoded.width() <= 1024 && decoded.height() <= 1024);
    let outside = home.path().join("private.png");
    std::fs::copy(&image_path, &outside).unwrap();
    assert!(generated_images(&format!("![private]({})", outside.display())).is_empty());
    assert!(generated_images(&format!("![private]({}/../private.png)", root.display())).is_empty());
    #[cfg(unix)]
    {
        let link = root.join("escape.png");
        std::os::unix::fs::symlink(&outside, &link).unwrap();
        assert!(generated_images(&format!("![escape]({})", link.display())).is_empty());
    }
    assert!(generated_images(&format!("[not an image]({})", image_path.display())).is_empty());
    let mut session = Session::create_with_id("generated-test".into(), None, None);
    session.add_message(Role::User, vec![text_block(markdown.clone())]);
    session.add_message(Role::Assistant, vec![text_block(markdown)]);
    let messages = wire_messages(&session, 0);
    assert!(messages[0].images.is_empty());
    assert_eq!(messages[1].images.len(), 1);
}

fn attachment_block() -> ContentBlock {
    use base64::Engine;
    let mut bytes = std::io::Cursor::new(Vec::new());
    image::RgbImage::from_pixel(1400, 1100, image::Rgb([10, 80, 170]))
        .write_to(&mut bytes, image::ImageFormat::Png)
        .unwrap();
    ContentBlock::Image {
        media_type: "image/png".into(),
        data: base64::engine::general_purpose::STANDARD.encode(bytes.into_inner()),
    }
}

#[test]
fn gif_and_webp_attachments_are_decoded_into_bounded_wire_thumbnails() {
    use base64::Engine;
    for (format, media_type) in [
        (image::ImageFormat::Gif, "image/gif"),
        (image::ImageFormat::WebP, "image/webp"),
    ] {
        let mut encoded = std::io::Cursor::new(Vec::new());
        image::RgbaImage::from_pixel(1400, 1100, image::Rgba([10, 80, 170, 255]))
            .write_to(&mut encoded, format)
            .unwrap();
        let encoded = encoded.into_inner();
        assert_eq!(image::guess_format(&encoded).unwrap(), format);
        let mut session = Session::create_with_id("encoded-attachments-test".into(), None, None);
        session.add_message(
            Role::User,
            vec![ContentBlock::Image {
                media_type: media_type.into(),
                data: base64::engine::general_purpose::STANDARD.encode(encoded),
            }],
        );
        let messages = wire_messages(&session, 0);
        assert_eq!(messages.len(), 1, "{media_type} image-only row disappeared");
        assert_eq!(messages[0].images.len(), 1, "{media_type} was discarded");
        let thumbnail = &messages[0].images[0];
        assert_eq!(thumbnail.media_type, "image/jpeg");
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(&thumbnail.data)
            .unwrap();
        assert!(bytes.len() <= 256 * 1024);
        assert_eq!(
            image::guess_format(&bytes).unwrap(),
            image::ImageFormat::Jpeg
        );
        let decoded = image::load_from_memory(&bytes).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (1024, 805));
    }
}

#[test]
fn visible_user_tool_and_image_only_attachments_survive_wire_serialization() {
    use base64::Engine;
    let mut session = Session::create_with_id("attachments-test".into(), None, None);
    let image = attachment_block();
    session.add_message(
        Role::User,
        vec![image.clone(), text_block("My screenshot".into())],
    );
    session.add_message(Role::User, vec![image.clone()]);
    // This is the real tool_output_to_content_blocks persistence format.
    session.add_message(
        Role::User,
        vec![
            ContentBlock::ToolResult {
                tool_use_id: "read-photo".into(),
                content: "Loaded image".into(),
                is_error: None,
            },
            image.clone(),
        ],
    );
    session.add_message(Role::Assistant, vec![image]);
    let messages = wire_messages(&session, 0);
    assert_eq!(messages.len(), 4);
    assert_eq!(
        messages.iter().map(|m| m.role).collect::<Vec<_>>(),
        ["user", "user", "tool", "assistant"]
    );
    assert_eq!(messages[1].text, "");
    assert_eq!(messages[2].tool_results[0].tool_use_id, "read-photo");
    for (seq, message) in messages.iter().enumerate() {
        assert_eq!(message.seq, seq);
        assert_eq!(message.images.len(), 1);
        assert_eq!(message.images[0].media_type, "image/jpeg");
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(&message.images[0].data)
            .unwrap();
        assert!(bytes.len() <= 256 * 1024);
        let decoded = image::load_from_memory(&bytes).unwrap();
        assert!(decoded.width() <= 1024 && decoded.height() <= 1024);
        let json = serde_json::to_value(message).unwrap();
        assert_eq!(json["images"][0]["media_type"], "image/jpeg");
    }
    assert_eq!(wire_messages(&session, 2).len(), 2);
}

#[test]
fn hidden_visual_context_and_invalid_attachments_are_not_uploaded() {
    let mut session = Session::create_with_id("hidden-attachments".into(), None, None);
    session.add_message(
        Role::User,
        vec![
            text_block(
                "  <system-reminder>\nGenerated image visual context</system-reminder>".into(),
            ),
            attachment_block(),
        ],
    );
    session.add_message(
        Role::User,
        vec![ContentBlock::Image {
            media_type: "image/png".into(),
            data: "not base64".into(),
        }],
    );
    session.add_message(
        Role::User,
        vec![ContentBlock::Image {
            media_type: "image/png".into(),
            data: "aGVsbG8=".into(),
        }],
    );
    assert!(wire_messages(&session, 0).is_empty());
    let mut blocks = vec![attachment_block(); 5];
    assert_eq!(attached_images(&blocks).len(), 4);
    if let ContentBlock::Image { media_type, .. } = &mut blocks[0] {
        *media_type = "application/octet-stream".into();
    }
    assert!(attached_images(&blocks[..1]).is_empty());
}

#[test]
fn attachment_wire_upgrade_replays_previously_uploaded_sessions() {
    let mut session = Session::create_with_id("attachment-upgrade".into(), None, None);
    session.add_message(
        Role::User,
        vec![attachment_block(), text_block("Photo".into())],
    );
    let current = wire_messages(&session, 0);
    let progress = SessionProgress {
        wire_version: WIRE_VERSION - 1,
        uploaded: 1,
        fingerprint: Some(wire_fingerprint(&current, 1)),
        ..SessionProgress::default()
    };
    let (replayed, _) = incremental_messages(&session, &progress);
    assert_eq!(replayed, current);
    let progress = SessionProgress {
        wire_version: WIRE_VERSION,
        ..progress
    };
    assert!(incremental_messages(&session, &progress).0.is_empty());
}

#[test]
fn image_upload_chunks_obey_byte_budget() {
    let message = WireMessage {
        seq: 0,
        role: "assistant",
        text: "x".repeat(100_000),
        display_text: None,
        tool_calls: Vec::new(),
        tool_results: Vec::new(),
        images: vec![WireImage {
            media_type: "image/jpeg".into(),
            data: "A".repeat(1_398_104),
        }],
        created_at: None,
    };
    let messages = vec![message; 10];
    let chunks = message_chunks(&messages);
    assert!(chunks.len() > 1);
    assert_eq!(chunks.iter().map(|c| c.len()).sum::<usize>(), 10);
    for chunk in chunks {
        assert!(serde_json::to_vec(chunk).unwrap().len() < 4 * 1024 * 1024);
    }
}

#[test]
fn parsed_state_reuse_matches_old_output_and_reads_current_bytes() {
    let _lock = storage::lock_test_env();
    let home = tempfile::tempdir().unwrap();
    let _env = TestEnv::set(&[("JCODE_HOME", home.path().as_os_str())]);
    let path = state_path().unwrap();
    let valid = r#"{"enabled":true,"generation":7,"sessions":{"one":{"uploaded":3,"mtime_ms":9}}}"#;
    std::fs::write(&path, valid).unwrap();
    let first = load_state_snapshot();
    assert!(Arc::ptr_eq(&first, &load_state_snapshot()));
    assert_eq!(*first, uncached_mobile_state());
    let mut owned = load_state();
    owned.sessions.clear();
    assert_eq!(load_state().sessions.len(), 1);

    // Equal byte length and identical mtime still require fresh authorization.
    let before = std::fs::metadata(&path).unwrap().modified().unwrap();
    std::fs::write(&path, valid.replace("\"generation\":7", "\"generation\":8")).unwrap();
    std::fs::File::open(&path)
        .unwrap()
        .set_modified(before)
        .unwrap();
    assert_eq!(load_state(), uncached_mobile_state());
    assert!(!sync_still_enabled(7));
    assert!(sync_still_enabled(8));
    std::fs::write(&path, valid.replace("true", "null")).unwrap();
    std::fs::File::open(&path)
        .unwrap()
        .set_modified(before)
        .unwrap();
    assert_eq!(load_state(), uncached_mobile_state());
    assert!(!is_enabled());
    for contents in [
        r#"{"enabled":true,"generation":7,"sessions":{"bad":{"uploaded":"oops","mtime_ms":9}}}"#,
        r#"{"enabled":true,"sessions":{"bad":{"uploaded":3}}}"#,
        r#"{"enabled":true,"sessions":[]}"#,
        "{",
    ] {
        std::fs::write(&path, contents).unwrap();
        assert_eq!(load_state(), uncached_mobile_state());
        assert!(!is_enabled());
        assert!(!sync_still_enabled(7));
    }
    std::fs::write(&path, valid).unwrap();
    assert!(sync_still_enabled(7));
    save_state(&MobileSyncState::default()).unwrap();
    assert!(!sync_still_enabled(7));
    save_state(&MobileSyncState {
        enabled: true,
        ..Default::default()
    })
    .unwrap();
    assert!(is_enabled());
    assert!(!sync_still_enabled(7));
    assert!(sync_still_enabled(9));
    std::fs::remove_file(&path).unwrap();
    assert_eq!(load_state(), uncached_mobile_state());
    assert!(!is_enabled());
}

#[test]
fn parsed_state_reuse_scopes_path_caps_retention_and_preserves_backup_recovery() {
    let _lock = storage::lock_test_env();
    let home = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    let _env = TestEnv::set(&[("JCODE_HOME", home.path().as_os_str())]);
    let path = state_path().unwrap();
    let valid = r#"{"enabled":true,"generation":4}"#;
    std::fs::write(&path, valid).unwrap();
    let first = load_state_snapshot();
    {
        let _other_env = TestEnv::set(&[("JCODE_HOME", other.path().as_os_str())]);
        std::fs::write(state_path().unwrap(), valid).unwrap();
        let second = load_state_snapshot();
        assert_eq!(*first, *second);
        assert!(!Arc::ptr_eq(&first, &second));
        assert_eq!(
            PARSED_STATE.lock().unwrap().as_ref().unwrap().path,
            state_path().unwrap()
        );
    }
    let oversized = format!("{}{}", valid, " ".repeat(MAX_CACHED_STATE_BYTES));
    std::fs::write(&path, oversized).unwrap();
    assert_eq!(load_state(), uncached_mobile_state());
    assert!(PARSED_STATE.lock().unwrap().is_none());
    std::fs::write(path.with_extension("bak"), valid).unwrap();
    std::fs::write(&path, r#"{"enabled":true,"sessions":{"bad":null}}"#).unwrap();
    assert_eq!(load_state().generation, 4);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), valid);
    assert_eq!(load_state(), uncached_mobile_state());
    std::fs::write(path.with_extension("bak"), "{").unwrap();
    std::fs::write(&path, "{").unwrap();
    assert_eq!(load_state(), uncached_mobile_state());
    assert!(!is_enabled());
}

#[test]
#[ignore = "paired warmed filesystem benchmark, run with --ignored --nocapture"]
fn benchmark_exact_state_reuse() {
    let _lock = storage::lock_test_env();
    let home = tempfile::tempdir().unwrap();
    let _env = TestEnv::set(&[("JCODE_HOME", home.path().as_os_str())]);
    for count in [0, 32, 1300] {
        let mut state = MobileSyncState {
            enabled: true,
            generation: 42,
            ..Default::default()
        };
        for i in 0..count {
            state.sessions.insert(
                format!("session-{i:04}"),
                SessionProgress {
                    wire_version: WIRE_VERSION,
                    uploaded: 200,
                    mtime_ms: 123456789,
                    revision: Some([123456789, 10000, 123456790, 1000]),
                    fingerprint: Some(123456789),
                },
            );
        }
        std::fs::write(
            state_path().unwrap(),
            serde_json::to_vec_pretty(&state).unwrap(),
        )
        .unwrap();
        assert_eq!(load_state(), uncached_mobile_state());
        for _ in 0..20 {
            std::hint::black_box(uncached_mobile_state());
            std::hint::black_box(load_state_snapshot());
        }
        let mut old = Vec::new();
        let mut cached = Vec::new();
        for round in 0..12 {
            for optimized in if round % 2 == 0 {
                [false, true]
            } else {
                [true, false]
            } {
                let started = Instant::now();
                for _ in 0..200 {
                    if optimized {
                        std::hint::black_box(sync_still_enabled(42));
                    } else {
                        let state = uncached_mobile_state();
                        std::hint::black_box(state.enabled && state.generation == 42);
                    }
                }
                if optimized {
                    cached.push(started.elapsed().as_nanos());
                } else {
                    old.push(started.elapsed().as_nanos());
                }
            }
        }
        old.sort_unstable();
        cached.sort_unstable();
        eprintln!(
            "exact state auth sessions={count} iterations=200 paired_rounds=12 old_median_ns={} cached_median_ns={} old_samples_ns={old:?} cached_samples_ns={cached:?}",
            old[6], cached[6]
        );
    }
}

struct TestEnv(Vec<(&'static str, Option<std::ffi::OsString>)>);

impl TestEnv {
    fn set(values: &[(&'static str, &std::ffi::OsStr)]) -> Self {
        Self(
            values
                .iter()
                .map(|(name, value)| {
                    let previous = std::env::var_os(name);
                    crate::env::set_var(name, value);
                    (*name, previous)
                })
                .collect(),
        )
    }
}

impl Drop for TestEnv {
    fn drop(&mut self) {
        for (name, value) in &self.0 {
            if let Some(value) = value {
                crate::env::set_var(name, value);
            } else {
                crate::env::remove_var(name);
            }
        }
    }
}

async fn mock_listing(status: &str, body: &str) -> (String, tokio::task::JoinHandle<String>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/", listener.local_addr().unwrap());
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let task = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        let mut buffer = [0; 1024];
        while !request.windows(4).any(|window| window == b"\r\n\r\n") {
            let count = socket.read(&mut buffer).await.unwrap();
            assert_ne!(count, 0);
            request.extend_from_slice(&buffer[..count]);
        }
        socket.write_all(response.as_bytes()).await.unwrap();
        String::from_utf8(request).unwrap()
    });
    (base, task)
}

#[tokio::test]
async fn cloud_session_listing_rejects_http_and_schema_errors() {
    for (status, body) in [
        ("401 Unauthorized", "{\"sessions\":[]}"),
        ("200 OK", "not json"),
        ("200 OK", "{}"),
        ("200 OK", "{\"sessions\":[{\"source\":\"cloud\"}]}"),
        ("200 OK", "{\"sessions\":[{\"session_id\":\"s1\"}]}"),
    ] {
        let (base, request) = mock_listing(status, body).await;
        assert!(
            list_cloud_sessions_with_endpoint(&reqwest::Client::new(), &base, "key")
                .await
                .is_err()
        );
        request.await.unwrap();
    }
    let (base, request) = mock_listing("200 OK", "{\"sessions\":[]}").await;
    assert!(
        list_cloud_sessions_with_endpoint(&reqwest::Client::new(), &base, "key")
            .await
            .unwrap()
            .is_empty()
    );
    request.await.unwrap();
}

fn msg(role: Role, content: Vec<ContentBlock>) -> StoredMessage {
    StoredMessage {
        id: "m".into(),
        role,
        content: content.into(),
        display_role: None,
        timestamp: None,
        tool_duration_ms: None,
        token_usage: None,
    }
}

#[test]
fn wire_messages_keep_visible_text_and_skip_hidden() {
    let mut session = Session::create_with_id("s1".into(), None, None);
    session.messages = vec![
        msg(
            Role::User,
            vec![ContentBlock::Text {
                text: "hi".into(),
                cache_control: None,
            }],
        ),
        msg(
            Role::Assistant,
            vec![ContentBlock::Reasoning {
                text: "secret".into(),
            }],
        ),
        msg(
            Role::Assistant,
            vec![
                ContentBlock::Text {
                    text: "running".into(),
                    cache_control: None,
                },
                ContentBlock::ToolUse {
                    id: "t".into(),
                    name: "bash".into(),
                    input: serde_json::json!({"command": "ls"}),
                    thought_signature: None,
                },
            ],
        ),
        msg(
            Role::User,
            vec![ContentBlock::ToolResult {
                tool_use_id: "t".into(),
                content: "x".repeat(MAX_TOOL_RESULT_CHARS + 10),
                is_error: None,
            }],
        ),
    ];
    let wire = wire_messages(&session, 0);
    assert_eq!(
        wire.iter().map(|m| m.seq).collect::<Vec<_>>(),
        vec![0, 2, 3]
    );
    assert_eq!(wire[0].role, "user");
    assert_eq!(wire[1].text, "running\n\n🔧 bash: ls");
    assert_eq!(wire[2].role, "tool");
    assert!(wire[2].text.ends_with("(truncated)"));
    assert!(!wire.iter().any(|m| m.text.contains("secret")));
    assert_eq!(wire_messages(&session, 3).len(), 1);
}

#[test]
fn session_ids_are_validated() {
    assert!(valid_session_id("session_abc-123.x"));
    assert!(!valid_session_id("bad id"));
    assert!(!valid_session_id(""));
}

#[test]
fn structured_wire_matches_mobile_cloud_card_fixture_and_hides_reasoning() {
    let mut session = Session::create_with_id("tool-fixture".into(), None, None);
    let call = |id: &str| ContentBlock::ToolUse {
        id: id.into(),
        name: "bash".into(),
        input: serde_json::json!({"command":"ls"}),
        thought_signature: Some("private signature".into()),
    };
    session.messages = vec![
        msg(Role::User, vec![text_block("go".into())]),
        msg(
            Role::Assistant,
            vec![ContentBlock::Reasoning {
                text: "private thinking".into(),
            }],
        ),
        msg(
            Role::Assistant,
            vec![
                text_block("on it".into()),
                call("a"),
                call("b"),
                text_block("<system-reminder>private reminder</system-reminder>".into()),
            ],
        ),
        msg(
            Role::User,
            vec![
                ContentBlock::ToolResult {
                    tool_use_id: "a".into(),
                    content: "x".into(),
                    is_error: None,
                },
                ContentBlock::ToolResult {
                    tool_use_id: "b".into(),
                    content: "y".into(),
                    is_error: Some(false),
                },
            ],
        ),
        msg(Role::Assistant, vec![text_block("**done**".into())]),
    ];
    let actual = serde_json::to_value(wire_messages(&session, 0)).unwrap();
    // Same fixture as mobile's structured_sync_matches_cloud_history_tool_phases.
    let expected = serde_json::json!([
        {"seq":0,"role":"user","text":"go"},
        {"seq":2,"role":"assistant","text":"on it\n\n🔧 bash: ls\n\n🔧 bash: ls",
         "display_text":"on it","tool_calls":[
             {"id":"a","name":"bash","input":{"command":"ls"}},
             {"id":"b","name":"bash","input":{"command":"ls"}}]},
        {"seq":3,"role":"tool","text":"x\n\ny","display_text":"","tool_results":[
            {"tool_use_id":"a","content":"x","is_error":false},
            {"tool_use_id":"b","content":"y","is_error":false}]},
        {"seq":4,"role":"assistant","text":"**done**"}
    ]);
    assert_eq!(actual, expected);
    assert!(!actual.to_string().contains("private"));
    session.messages[3].content = vec![ContentBlock::ToolResult {
        tool_use_id: "a".into(),
        content: "failed".into(),
        is_error: Some(true),
    }]
    .into();
    assert!(wire_messages(&session, 3)[0].tool_results[0].is_error);
}

#[test]
fn structured_wire_bounds_inputs_counts_ids_and_total_bytes() {
    let ordinary = serde_json::json!({"path":"界🦀/file","limit":7});
    assert_eq!(bounded_tool_input(&ordinary), ordinary);
    let huge = serde_json::json!({"command":"界🦀".repeat(MAX_TOOL_INPUT_BYTES)});
    let preview = bounded_tool_input(&huge);
    assert!(preview.as_str().unwrap().ends_with("(truncated)"));
    assert!(serde_json::to_vec(&preview).unwrap().len() < 2 * MAX_TOOL_INPUT_BYTES + 128);
    let call = |id: String, input: serde_json::Value| ContentBlock::ToolUse {
        id,
        name: "bash".into(),
        input,
        thought_signature: None,
    };
    let mut session = Session::create_with_id("tool-bounds".into(), None, None);
    session.messages = vec![msg(Role::Assistant, vec![call("a".into(), huge)])];
    let wire = wire_messages(&session, 0);
    assert_eq!(wire[0].tool_calls[0].input, preview);
    for blocks in [
        vec![call(
            "x".repeat(MAX_TOOL_ID_BYTES + 1),
            serde_json::Value::Null,
        )],
        (0..=MAX_STRUCTURED_TOOLS)
            .map(|i| call(i.to_string(), serde_json::Value::Null))
            .collect(),
        (0..MAX_STRUCTURED_TOOLS)
            .map(|i| ContentBlock::ToolResult {
                tool_use_id: i.to_string(),
                content: "🦀".repeat(MAX_TOOL_RESULT_CHARS + 10),
                is_error: None,
            })
            .collect(),
    ] {
        session.messages[0].content = blocks.into();
        let wire = wire_messages(&session, 0);
        assert_eq!(
            wire[0].display_text, None,
            "oversized metadata must retain legacy summaries"
        );
        assert!(wire[0].tool_calls.is_empty() && wire[0].tool_results.is_empty());
        assert!(!wire[0].text.is_empty());
    }
    session.messages[0].content = vec![ContentBlock::ToolResult {
        tool_use_id: "a".into(),
        content: "🦀".repeat(MAX_TOOL_RESULT_CHARS + 10),
        is_error: Some(true),
    }]
    .into();
    let wire = wire_messages(&session, 0);
    assert!(wire[0].tool_results[0].content.ends_with("(truncated)"));
    assert!(wire[0].tool_results[0].content.chars().count() < MAX_TOOL_RESULT_CHARS + 32);
}

#[test]
fn structured_wire_epoch_invalidates_image_only_progress_once() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("snapshot.json");
    std::fs::write(&path, b"{}").unwrap();
    let current = session_revision(&path);
    let mut image_only = current;
    image_only[1] ^= 1_u64 << 62;
    assert_ne!(
        current, image_only,
        "old unchanged progress must reupload tools"
    );
    assert_eq!(current[1], 2 ^ (3_u64 << 62));
    assert_eq!(
        session_revision(&path),
        current,
        "new epoch is stable after upload"
    );
}

#[tokio::test]
async fn structured_wire_version_replays_unchanged_snapshots_once() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let _lock = storage::lock_test_env();
    let home = tempfile::tempdir().unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let _env = TestEnv::set(&[
        ("JCODE_HOME", home.path().as_os_str()),
        ("JCODE_API_KEY", std::ffi::OsStr::new("test-key")),
        ("JCODE_API_BASE", std::ffi::OsStr::new(&base)),
    ]);
    let mut session = Session::create_with_id("schema-replay".into(), None, None);
    session.messages = vec![msg(
        Role::Assistant,
        vec![ContentBlock::ToolUse {
            id: "call-read".into(),
            name: "read".into(),
            input: serde_json::json!({"path":"README.md"}),
            thought_signature: None,
        }],
    )];
    session.save().unwrap();
    DIRTY_SESSIONS.lock().unwrap().clear();
    let path = home.path().join("sessions/schema-replay.json");
    let revision = session_revision(&path);
    // Even an identical fingerprint must not suppress a schema upgrade replay.
    let fingerprint = wire_fingerprint(&wire_messages(&session, 0), 1);
    let progress: SessionProgress = serde_json::from_value(serde_json::json!({
        "uploaded": 1, "mtime_ms": session_mtime_ms(&path),
        "revision": revision, "fingerprint": fingerprint,
    }))
    .unwrap();
    assert_eq!(progress.wire_version, 0);
    save_state(&MobileSyncState {
        enabled: true,
        ..Default::default()
    })
    .unwrap();
    commit_progress(
        load_state().generation,
        &BTreeMap::from([(session.id.clone(), progress)]),
    )
    .unwrap();
    assert_eq!(load_state().sessions[&session.id].revision, Some(revision));
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut bytes = Vec::new();
        let mut buffer = [0; 4096];
        let body = loop {
            let count = socket.read(&mut buffer).await.unwrap();
            assert_ne!(count, 0);
            bytes.extend_from_slice(&buffer[..count]);
            if let Some(index) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                let start = index + 4;
                let header = String::from_utf8_lossy(&bytes[..index]);
                let length: usize = header
                    .lines()
                    .find_map(|line| {
                        line.to_ascii_lowercase()
                            .strip_prefix("content-length:")
                            .map(|value| value.trim().parse().unwrap())
                    })
                    .unwrap();
                if bytes.len() >= start + length {
                    break serde_json::from_slice::<serde_json::Value>(
                        &bytes[start..start + length],
                    )
                    .unwrap();
                }
            }
        };
        assert_eq!(body["messages"][0]["seq"], 0);
        assert_eq!(
            body["messages"][0]["tool_calls"][0]["input"]["path"],
            "README.md"
        );
        socket
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}")
            .await
            .unwrap();
        listener
    });
    let client = reqwest::Client::new();
    let report = tokio::time::timeout(Duration::from_secs(5), sync_once(&client))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(report.messages_uploaded, 1);
    assert_eq!(report.sessions_uploaded, 1);
    let listener = server.await.unwrap();
    let state = load_state();
    assert_eq!(state.sessions[&session.id].wire_version, WIRE_VERSION);
    assert_eq!(state.sessions[&session.id].revision, Some(revision));
    assert_eq!(session_revision(&path), revision);
    assert_eq!(sync_once(&client).await.unwrap(), SyncReport::default());
    assert!(
        tokio::time::timeout(Duration::from_millis(50), listener.accept())
            .await
            .is_err()
    );
    DIRTY_SESSIONS.lock().unwrap().clear();
}

#[test]
fn structured_input_changes_invalidate_synced_fingerprint_without_changing_legacy_summary() {
    let mut session = Session::create_with_id("tool-hash".into(), None, None);
    let call = |limit: usize| ContentBlock::ToolUse {
        id: "a".into(),
        name: "read".into(),
        input: serde_json::json!({"intent":"inspect file","limit":limit}),
        thought_signature: None,
    };
    session.messages = vec![msg(Role::Assistant, vec![call(10)])];
    let before = wire_messages(&session, 0);
    session.messages[0].content = vec![call(20)].into();
    let after = wire_messages(&session, 0);
    assert_eq!(before[0].text, after[0].text);
    assert_ne!(wire_fingerprint(&before, 1), wire_fingerprint(&after, 1));
}

// Deliberately retain the original full-copy conversion as an independent
// semantic oracle and a baseline for the ignored representative benchmark.
fn old_wire_messages(session: &Session, start: usize) -> Vec<WireMessage> {
    fn old_truncate(text: &str, max: usize) -> String {
        if text.chars().count() <= max {
            return text.to_string();
        }
        let mut out: String = text.chars().take(max).collect();
        out.push_str("\n… (truncated)");
        out
    }
    session
        .messages
        .iter()
        .enumerate()
        .skip(start)
        .filter_map(|(seq, message)| {
            let mut parts = Vec::new();
            let mut is_tool_result = false;
            for block in &message.content {
                match block {
                    ContentBlock::Text { text, .. }
                        if !text.trim().is_empty()
                            && !text.trim_start().starts_with("<system-reminder>") =>
                    {
                        parts.push(text.clone())
                    }
                    ContentBlock::ToolUse { name, input, .. } => {
                        let detail = input
                            .get("intent")
                            .or_else(|| input.get("command"))
                            .or_else(|| input.get("file_path"))
                            .or_else(|| input.get("query"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        parts.push(if detail.is_empty() {
                            format!("🔧 {name}")
                        } else {
                            format!("🔧 {name}: {}", old_truncate(detail, 200))
                        });
                    }
                    ContentBlock::ToolResult { content, .. } => {
                        is_tool_result = true;
                        parts.push(old_truncate(content, MAX_TOOL_RESULT_CHARS));
                    }
                    _ => {}
                }
            }
            if parts.is_empty() {
                return None;
            }
            let role = if is_tool_result {
                "tool"
            } else if message.display_role.is_some() {
                "system"
            } else {
                match message.role {
                    Role::User => "user",
                    Role::Assistant => "assistant",
                }
            };
            Some(WireMessage {
                seq,
                role,
                images: Vec::new(),
                display_text: None,
                tool_calls: Vec::new(),
                tool_results: Vec::new(),
                text: old_truncate(&parts.join("\n\n"), MAX_TEXT_CHARS),
                created_at: message.timestamp.map(|t| t.to_rfc3339()),
            })
        })
        .collect()
}

fn text_block(text: String) -> ContentBlock {
    ContentBlock::Text {
        text,
        cache_control: None,
    }
}

fn result_block(content: String) -> ContentBlock {
    ContentBlock::ToolResult {
        tool_use_id: "t".into(),
        content,
        is_error: None,
    }
}

#[test]
fn bounded_wire_conversion_matches_original_unicode_and_boundaries() {
    let mut session = Session::create_with_id("bounded-wire".into(), None, None);
    for unit in ["a", "é", "界", "😀", "e\u{301}"] {
        for size in [
            0,
            1,
            199,
            200,
            201,
            MAX_TOOL_RESULT_CHARS - 1,
            MAX_TOOL_RESULT_CHARS,
            MAX_TOOL_RESULT_CHARS + 1,
            MAX_TEXT_CHARS - 2,
            MAX_TEXT_CHARS - 1,
            MAX_TEXT_CHARS,
            MAX_TEXT_CHARS + 1,
        ] {
            let value = unit.repeat(size);
            session
                .messages
                .push(msg(Role::User, vec![text_block(value.clone())]));
            session
                .messages
                .push(msg(Role::Assistant, vec![result_block(value.clone())]));
            session.messages.push(msg(
                Role::Assistant,
                vec![
                    text_block(value.clone()),
                    result_block(String::new()),
                    text_block("tail".into()),
                    result_block(value.clone()),
                    ContentBlock::ToolUse {
                        id: "t".into(),
                        name: "bash".into(),
                        input: serde_json::json!({"intent": value}),
                        thought_signature: None,
                    },
                ],
            ));
        }
    }
    session.messages.push(msg(
        Role::User,
        vec![
            text_block(" \n\u{2003}<system-reminder>private".into()),
            text_block(" \n\u{2003}".into()),
            ContentBlock::Reasoning {
                text: "private".into(),
            },
        ],
    ));
    session
        .messages
        .push(msg(Role::User, vec![result_block(String::new())]));
    session.messages.push(msg(
        Role::User,
        vec![result_block(String::new()), result_block(String::new())],
    ));
    for name in [String::new(), "😀".repeat(MAX_TEXT_CHARS + 1)] {
        for input in [
            serde_json::json!({}),
            serde_json::json!({"intent": ""}),
            serde_json::json!({"intent": null, "command": "not-used"}),
            serde_json::json!({"command": "界".repeat(201)}),
            serde_json::json!({"file_path": "file"}),
            serde_json::json!({"query": "query"}),
        ] {
            session.messages.push(msg(
                Role::Assistant,
                vec![ContentBlock::ToolUse {
                    id: "t".into(),
                    name: name.clone(),
                    input,
                    thought_signature: None,
                }],
            ));
        }
    }
    session.messages[3].display_role = Some(crate::session::StoredDisplayRole::System);
    session.messages[6].display_role = Some(crate::session::StoredDisplayRole::BackgroundTask);
    session.messages[1].timestamp = Some(chrono::Utc::now());
    for start in [0, 1, 3, session.messages.len(), session.messages.len() + 1] {
        let expected = old_wire_messages(&session, start);
        let mut actual = wire_messages(&session, start);
        // New metadata is additive. The independent oracle still verifies every
        // legacy text byte, role, stored sequence and timestamp unchanged.
        for message in &mut actual {
            message.display_text = None;
            message.tool_calls.clear();
            message.tool_results.clear();
        }
        assert_eq!(actual, expected, "start={start}");
        for end in [0, 1, session.messages.len() / 2, session.messages.len()] {
            assert_eq!(
                wire_fingerprint(&actual, end),
                wire_fingerprint(&expected, end)
            );
        }
    }
    for max in [0, 1, 2, 3, 200] {
        for value in ["", "a", "é😀界", "e\u{301}", "x\n"] {
            let expected = if value.chars().count() <= max {
                value.to_owned()
            } else {
                format!(
                    "{}\n… (truncated)",
                    value.chars().take(max).collect::<String>()
                )
            };
            assert_eq!(truncate(value, max), expected);
        }
    }
}

#[test]
fn bounded_text_matches_join_then_truncate() {
    let parts = ["", "a", "é", "界😀", "e\u{301}", "\n", "   "];
    for max in 0..12 {
        for a in parts {
            for b in parts {
                for c in parts {
                    let joined = [a, b, c].join("\n\n");
                    let expected = if joined.chars().count() <= max {
                        joined.clone()
                    } else {
                        format!(
                            "{}\n… (truncated)",
                            joined.chars().take(max).collect::<String>()
                        )
                    };
                    let mut actual = BoundedText::new(max);
                    for (index, part) in [a, b, c].iter().enumerate() {
                        if index != 0 {
                            actual.push("\n\n");
                        }
                        actual.push(part);
                    }
                    assert_eq!(actual.text, expected, "max={max}, parts={:?}", [a, b, c]);
                }
            }
        }
    }
}

#[test]
#[ignore = "comparative benchmark, run with --ignored --nocapture"]
fn benchmark_bounded_wire_conversion() {
    use std::hint::black_box;
    use std::time::Instant;
    for (label, size, blocks) in [
        ("typical", 1_000, 4),
        ("long-ascii", 2_000_000, 4),
        ("long-unicode", 1_000_000, 8),
    ] {
        let mut session = Session::create_with_id("wire-bench".into(), None, None);
        let unit = if label == "long-unicode" {
            "界😀é"
        } else {
            "a"
        };
        for _ in 0..8 {
            let mut content = (0..blocks)
                .map(|_| text_block(unit.repeat(size)))
                .collect::<Vec<_>>();
            content.push(result_block("tool output".repeat(1_000)));
            session.messages.push(msg(Role::Assistant, content));
        }
        assert_eq!(wire_messages(&session, 0), old_wire_messages(&session, 0));
        let iterations = if label == "typical" { 100 } else { 5 };
        let old_start = Instant::now();
        for _ in 0..iterations {
            black_box(old_wire_messages(black_box(&session), 0));
        }
        let old = old_start.elapsed();
        let new_start = Instant::now();
        for _ in 0..iterations {
            black_box(wire_messages(black_box(&session), 0));
        }
        let new = new_start.elapsed();
        eprintln!(
            "mobile wire {label}: iterations={iterations} old={old:?} bounded={new:?} ratio={:.2}x",
            old.as_secs_f64() / new.as_secs_f64()
        );
    }
}

#[test]
fn incremental_updates_replay_rewritten_and_shrunk_prefixes() {
    let mut session = Session::create_with_id("rewrite".into(), None, None);
    for text in ["first", "second"] {
        session.add_message(
            Role::User,
            vec![ContentBlock::Text {
                text: text.into(),
                cache_control: None,
            }],
        );
    }
    let (initial, fingerprint) = incremental_messages(&session, &SessionProgress::default());
    assert_eq!(initial.len(), 2);
    let progress = SessionProgress {
        wire_version: WIRE_VERSION,
        uploaded: 2,
        fingerprint: Some(fingerprint),
        ..Default::default()
    };
    assert!(incremental_messages(&session, &progress).0.is_empty());
    session.add_message(
        Role::Assistant,
        vec![ContentBlock::Text {
            text: "appended".into(),
            cache_control: None,
        }],
    );
    let appended = incremental_messages(&session, &progress).0;
    assert_eq!(appended.len(), 1);
    assert_eq!(appended[0].seq, 2);
    session.messages[0].content = vec![ContentBlock::Text {
        text: "rewritten".into(),
        cache_control: None,
    }]
    .into();
    let rewritten = incremental_messages(&session, &progress).0;
    assert_eq!(rewritten.len(), 3);
    assert_eq!(rewritten[0].text, "rewritten");
    session.truncate_messages(1);
    assert_eq!(incremental_messages(&session, &progress).0.len(), 1);
}

#[test]
fn phone_message_opt_out_survives_sync_saves_and_disable() {
    let _lock = storage::lock_test_env();
    let home = tempfile::tempdir().unwrap();
    let _env = TestEnv::set(&[("JCODE_HOME", home.path().as_os_str())]);
    save_state(&MobileSyncState {
        enabled: true,
        ..Default::default()
    })
    .unwrap();
    assert!(!uncached_mobile_state().phone_messages_off);
    let stale = uncached_mobile_state();

    assert!(set_phone_messages_off(true).unwrap().phone_messages_off);
    // A writer holding an older read must not turn messages back on.
    save_state(&stale).unwrap();
    assert!(uncached_mobile_state().phone_messages_off);
    assert!(uncached_mobile_state().enabled);
    save_state(&MobileSyncState::default()).unwrap();
    assert!(uncached_mobile_state().phone_messages_off);

    assert!(!set_phone_messages_off(false).unwrap().phone_messages_off);
    assert!(!uncached_mobile_state().phone_messages_off);
}

#[path = "mobile_sync_tests_additional_tests.rs"]
mod additional_benchmark_unchanged_reconciliation;
