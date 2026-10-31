//! Regression tests for #1680: computer-use screenshots must not accumulate
//! without bound in the persisted transcript.

use super::*;
use anyhow::{Result, anyhow};

const SCREENSHOT_CHARS: usize = 64 * 1024;

fn add_screenshot(session: &mut Session, index: usize) {
    session.add_message(
        Role::User,
        vec![
            ContentBlock::ToolResult {
                tool_use_id: format!("call_{index}"),
                content: format!("Captured main display #{index}"),
                is_error: None,
            },
            ContentBlock::Image {
                media_type: "image/png".to_string(),
                data: "A".repeat(SCREENSHOT_CHARS),
            },
        ],
    );
}

fn inline_image_count(messages: &[StoredMessage]) -> usize {
    messages
        .iter()
        .flat_map(|message| message.content.iter())
        .filter(|block| matches!(block, ContentBlock::Image { .. }))
        .count()
}

#[test]
fn computer_use_screenshots_stay_bounded_in_saved_transcript() -> Result<()> {
    let _env_lock = lock_env();
    let temp_home = tempfile::Builder::new()
        .prefix("jcode-tool-image-decay-")
        .tempdir()
        .map_err(|e| anyhow!(e))?;
    let _home = EnvVarGuard::set("JCODE_HOME", temp_home.path().as_os_str());

    let session_id = "session_tool_image_decay";
    let mut session = Session::create_with_id(session_id.to_string(), None, None);
    let screenshots = 100;
    for index in 0..screenshots {
        add_screenshot(&mut session, index);
        session.save()?;
        assert!(
            inline_image_count(&session.messages)
                <= jcode_compaction_core::TOOL_IMAGE_DECAY_TRIGGER_COUNT
        );
    }

    // Every tool_result survives so tool_use pairing stays valid.
    assert_eq!(session.messages.len(), screenshots);
    // The newest screenshot is still inline and reaches the provider.
    let provider_messages = session.provider_messages();
    let last = provider_messages.last().expect("provider message");
    assert!(
        last.content
            .iter()
            .any(|block| matches!(block, ContentBlock::Image { .. }))
    );

    let loaded = Session::load(session_id)?;
    assert_eq!(loaded.messages.len(), screenshots);
    let inline = inline_image_count(&loaded.messages);
    assert!(inline >= jcode_compaction_core::TOOL_IMAGE_DECAY_KEEP_COUNT);
    assert!(inline <= jcode_compaction_core::TOOL_IMAGE_DECAY_TRIGGER_COUNT);

    let snapshot_bytes = std::fs::metadata(session_path(session_id)?)?.len() as usize;
    let journal_bytes = std::fs::metadata(session_journal_path(session_id)?)
        .map(|meta| meta.len() as usize)
        .unwrap_or(0);
    let on_disk = snapshot_bytes + journal_bytes;
    assert!(
        on_disk < (jcode_compaction_core::TOOL_IMAGE_DECAY_TRIGGER_COUNT + 2) * SCREENSHOT_CHARS,
        "transcript grew to {on_disk} bytes for {screenshots} screenshots"
    );
    Ok(())
}

#[test]
fn loading_legacy_transcript_decays_old_screenshots() -> Result<()> {
    let _env_lock = lock_env();
    let temp_home = tempfile::Builder::new()
        .prefix("jcode-tool-image-legacy-")
        .tempdir()
        .map_err(|e| anyhow!(e))?;
    let _home = EnvVarGuard::set("JCODE_HOME", temp_home.path().as_os_str());

    // Simulate a transcript written before decay existed by appending the raw
    // messages without going through the decaying append path.
    let session_id = "session_tool_image_legacy";
    let mut session = Session::create_with_id(session_id.to_string(), None, None);
    let screenshots = 30;
    for index in 0..screenshots {
        add_screenshot(&mut session, index);
    }
    let mut legacy = Session::create_with_id(session_id.to_string(), None, None);
    legacy.messages = session.messages.clone();
    for message in &mut legacy.messages {
        if let [ContentBlock::ToolResult { .. }, block] = message.content.as_mut_slice() {
            *block = ContentBlock::Image {
                media_type: "image/png".to_string(),
                data: "A".repeat(SCREENSHOT_CHARS),
            };
        }
    }
    assert_eq!(inline_image_count(&legacy.messages), screenshots);
    std::fs::create_dir_all(session_path(session_id)?.parent().unwrap())?;
    crate::storage::write_json(&session_path(session_id)?, &legacy)?;

    let loaded = Session::load(session_id)?;
    assert_eq!(loaded.messages.len(), screenshots);
    assert_eq!(
        inline_image_count(&loaded.messages),
        jcode_compaction_core::TOOL_IMAGE_DECAY_KEEP_COUNT
    );
    Ok(())
}

#[test]
fn loading_legacy_transcript_downscales_large_png_in_memory_only() -> Result<()> {
    use base64::Engine as _;

    let _env_lock = lock_env();
    let temp_home = tempfile::Builder::new()
        .prefix("jcode-session-image-downscale-")
        .tempdir()
        .map_err(|e| anyhow!(e))?;
    let _home = EnvVarGuard::set("JCODE_HOME", temp_home.path().as_os_str());

    let session_id = "session_large_image_load";
    let pixels = image::RgbaImage::from_pixel(
        crate::image_normalize::SESSION_IMAGE_MAX_EDGE + 128,
        900,
        image::Rgba([9, 8, 7, 255]),
    );
    let mut png = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(pixels)
        .write_to(&mut png, image::ImageFormat::Png)
        .unwrap();
    let original_data = base64::engine::general_purpose::STANDARD.encode(png.into_inner());

    let mut fresh = Session::create_with_id("session_new_image_ingest".to_string(), None, None);
    fresh.add_message(
        Role::User,
        vec![ContentBlock::Image {
            media_type: "image/png".into(),
            data: original_data.clone(),
        }],
    );
    let ContentBlock::Image { data, .. } = &fresh.messages[0].content[0] else {
        panic!("new screenshot image remains in the transcript")
    };
    assert_ne!(data, &original_data);

    let mut legacy = Session::create_with_id(session_id.to_string(), None, None);
    legacy.messages = vec![StoredMessage {
        id: "screenshot".into(),
        role: Role::User,
        content: vec![ContentBlock::Image {
            media_type: "image/png".into(),
            data: original_data.clone(),
        }],
        display_role: None,
        timestamp: None,
        tool_duration_ms: None,
        token_usage: None,
    }];
    std::fs::create_dir_all(session_path(session_id)?.parent().unwrap())?;
    crate::storage::write_json(&session_path(session_id)?, &legacy)?;
    let disk_before = std::fs::read(session_path(session_id)?)?;

    let loaded = Session::load(session_id)?;
    assert_eq!(loaded.messages.len(), 1);
    let ContentBlock::Image { data, .. } = &loaded.messages[0].content[0] else {
        panic!("screenshot image remains in the transcript")
    };
    assert_ne!(data, &original_data);
    let resized = base64::engine::general_purpose::STANDARD
        .decode(data)
        .unwrap();
    let decoded = image::load_from_memory(&resized).unwrap();
    let dimensions = (decoded.width(), decoded.height());
    assert_eq!(
        dimensions,
        (crate::image_normalize::SESSION_IMAGE_MAX_EDGE, 832)
    );
    assert_eq!(std::fs::read(session_path(session_id)?)?, disk_before);
    Ok(())
}
