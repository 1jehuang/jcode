use super::*;
use crate::tool::{ToolContext, ToolExecutionMode};
use serde_json::json;

fn make_ctx(working_dir: std::path::PathBuf) -> ToolContext {
    ToolContext {
        session_id: "test-session".to_string(),
        message_id: "test-message".to_string(),
        tool_call_id: "test-call".to_string(),
        working_dir: Some(working_dir),
        stdin_request_tx: None,
        graceful_shutdown_signal: None,
        execution_mode: ToolExecutionMode::Direct,
    }
}

#[test]
fn normalize_read_range_supports_start_and_end_lines() {
    let params: ReadInput = serde_json::from_value(json!({
        "file_path": "src/lib.rs",
        "start_line": 10,
        "end_line": 20
    }))
    .expect("deserialize params");

    let range = normalize_read_range(&params).expect("normalize range");
    assert_eq!(
        range,
        NormalizedReadRange {
            offset: 9,
            limit: 11,
            style: ReadRangeStyle::StartEnd,
        }
    );
}

#[test]
fn normalize_read_range_supports_start_line_and_limit() {
    let params: ReadInput = serde_json::from_value(json!({
        "file_path": "src/lib.rs",
        "start_line": 10,
        "limit": 20
    }))
    .expect("deserialize params");

    let range = normalize_read_range(&params).expect("start_line + limit should work");
    assert_eq!(
        range,
        NormalizedReadRange {
            offset: 9,
            limit: 20,
            style: ReadRangeStyle::StartEnd,
        }
    );
}

#[test]
fn normalize_read_range_prefers_end_line_over_limit() {
    let params: ReadInput = serde_json::from_value(json!({
        "file_path": "src/lib.rs",
        "start_line": 10,
        "end_line": 20,
        "limit": 999
    }))
    .expect("deserialize params");

    let range = normalize_read_range(&params).expect("end_line should take precedence");
    assert_eq!(
        range,
        NormalizedReadRange {
            offset: 9,
            limit: 11,
            style: ReadRangeStyle::StartEnd,
        }
    );
}

#[test]
fn normalize_read_range_rejects_start_line_and_offset() {
    let params: ReadInput = serde_json::from_value(json!({
        "file_path": "src/lib.rs",
        "start_line": 10,
        "offset": 20
    }))
    .expect("deserialize params");

    let err = normalize_read_range(&params).expect_err("mixed range styles should fail");
    assert!(
        err.to_string().contains("Use either start_line/end_line")
            || err.to_string().contains("not both"),
        "unexpected error: {err}"
    );
}

#[test]
fn normalize_read_range_accepts_matching_start_line_and_offset() {
    let params: ReadInput = serde_json::from_value(json!({
        "file_path": "src/lib.rs",
        "start_line": 10,
        "offset": 9,
        "limit": 20
    }))
    .expect("deserialize params");

    let range = normalize_read_range(&params).expect("matching range styles should work");
    assert_eq!(
        range,
        NormalizedReadRange {
            offset: 9,
            limit: 20,
            style: ReadRangeStyle::StartEnd,
        }
    );
}

#[test]
fn normalize_read_range_accepts_end_line_with_zero_offset() {
    let params: ReadInput = serde_json::from_value(json!({
        "file_path": "src/lib.rs",
        "end_line": 20,
        "offset": 0
    }))
    .expect("deserialize params");

    let range = normalize_read_range(&params).expect("redundant zero offset should work");
    assert_eq!(
        range,
        NormalizedReadRange {
            offset: 0,
            limit: 20,
            style: ReadRangeStyle::StartEnd,
        }
    );
}

#[test]
fn normalize_read_range_rejects_invalid_end_before_start() {
    let params: ReadInput = serde_json::from_value(json!({
        "file_path": "src/lib.rs",
        "start_line": 20,
        "end_line": 10
    }))
    .expect("deserialize params");

    let err = normalize_read_range(&params).expect_err("invalid range should fail");
    assert!(
        err.to_string()
            .contains("greater than or equal to start_line"),
        "unexpected error: {err}"
    );
}

#[test]
fn read_tool_schema_avoids_openai_incompatible_combinators() {
    let schema = ReadTool::new().parameters_schema();

    assert_eq!(schema.get("type"), Some(&json!("object")));
    assert!(schema.get("allOf").is_none());
    assert!(schema.get("not").is_none());
}

#[test]
fn read_tool_schema_advertises_only_canonical_public_fields() {
    let schema = ReadTool::new().parameters_schema();
    let properties = schema["properties"]
        .as_object()
        .expect("read schema properties should be an object");

    assert!(properties.contains_key("file_path"));
    assert!(properties.contains_key("start_line"));
    assert!(properties.contains_key("limit"));
    assert!(!properties.contains_key("end_line"));
    assert!(!properties.contains_key("offset"));
}

#[test]
fn read_tool_description_advertises_supported_file_types() {
    let tool = ReadTool::new();
    let description = tool.description().to_lowercase();
    assert!(description.contains("text"), "description={description}");
    assert!(description.contains("image"), "description={description}");
    assert!(description.contains("pdf"), "description={description}");

    let schema = tool.parameters_schema();
    let file_path_description = schema["properties"]["file_path"]["description"]
        .as_str()
        .expect("file_path should have a description");
    assert_eq!(file_path_description, "Path to a file.");
}

#[tokio::test]
async fn read_tool_supports_start_line_and_end_line() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("sample.txt");
    std::fs::write(&path, "one\ntwo\nthree\nfour\nfive\n").expect("write sample file");

    let tool = ReadTool::new();
    let output = tool
        .execute(
            json!({
                "file_path": "sample.txt",
                "start_line": 2,
                "end_line": 4
            }),
            make_ctx(temp.path().to_path_buf()),
        )
        .await
        .expect("read execution should succeed");

    assert!(
        output.output.contains("2\ttwo"),
        "output={:?}",
        output.output
    );
    assert!(
        output.output.contains("3\tthree"),
        "output={:?}",
        output.output
    );
    assert!(
        output.output.contains("4\tfour"),
        "output={:?}",
        output.output
    );
    assert!(
        !output.output.contains("1\tone"),
        "output={:?}",
        output.output
    );
    assert!(
        !output.output.contains("5\tfive"),
        "output={:?}",
        output.output
    );
}

#[tokio::test]
async fn read_tool_continuation_hint_matches_start_line_style() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("sample.txt");
    std::fs::write(&path, "one\ntwo\nthree\nfour\nfive\n").expect("write sample file");

    let tool = ReadTool::new();
    let output = tool
        .execute(
            json!({
                "file_path": "sample.txt",
                "start_line": 2,
                "end_line": 3
            }),
            make_ctx(temp.path().to_path_buf()),
        )
        .await
        .expect("read execution should succeed");

    assert!(
        output.output.contains("use start_line=4 to continue"),
        "output={:?}",
        output.output
    );
}

#[tokio::test]
async fn read_tool_supports_start_line_with_limit() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("sample.txt");
    std::fs::write(&path, "one\ntwo\nthree\nfour\nfive\n").expect("write sample file");

    let tool = ReadTool::new();
    let output = tool
        .execute(
            json!({
                "file_path": "sample.txt",
                "start_line": 2,
                "limit": 2
            }),
            make_ctx(temp.path().to_path_buf()),
        )
        .await
        .expect("read execution should succeed");

    assert!(
        output.output.contains("2\ttwo"),
        "output={:?}",
        output.output
    );
    assert!(
        output.output.contains("3\tthree"),
        "output={:?}",
        output.output
    );
    assert!(
        !output.output.contains("4\tfour"),
        "output={:?}",
        output.output
    );
    assert!(
        output.output.contains("use start_line=4 to continue"),
        "output={:?}",
        output.output
    );
}

#[tokio::test]
async fn read_tool_prefers_end_line_over_limit() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("sample.txt");
    std::fs::write(&path, "one\ntwo\nthree\nfour\nfive\n").expect("write sample file");

    let tool = ReadTool::new();
    let output = tool
        .execute(
            json!({
                "file_path": "sample.txt",
                "start_line": 2,
                "end_line": 3,
                "limit": 50
            }),
            make_ctx(temp.path().to_path_buf()),
        )
        .await
        .expect("read execution should succeed");

    assert!(
        output.output.contains("2\ttwo"),
        "output={:?}",
        output.output
    );
    assert!(
        output.output.contains("3\tthree"),
        "output={:?}",
        output.output
    );
    assert!(
        !output.output.contains("4\tfour"),
        "output={:?}",
        output.output
    );
    assert!(
        output.output.contains("use start_line=4 to continue"),
        "output={:?}",
        output.output
    );
}

fn tiny_bmp_bytes() -> Vec<u8> {
    // 2x2 24-bit BMP, rows padded to 4 bytes.
    let row: [u8; 8] = [0, 0, 255, 0, 255, 0, 0, 0];
    let pixels: Vec<u8> = row.iter().chain(row.iter()).copied().collect();
    let mut out = Vec::new();
    out.extend_from_slice(b"BM");
    out.extend_from_slice(&(54 + pixels.len() as u32).to_le_bytes());
    out.extend_from_slice(&[0, 0, 0, 0]);
    out.extend_from_slice(&54u32.to_le_bytes());
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&2i32.to_le_bytes());
    out.extend_from_slice(&2i32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&24u16.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&(pixels.len() as u32).to_le_bytes());
    out.extend_from_slice(&2835i32.to_le_bytes());
    out.extend_from_slice(&2835i32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&pixels);
    out
}

/// #1712: reading a BMP must attach a PNG, never `image/bmp`, which every
/// provider rejects and which used to brick the session.
#[tokio::test]
async fn read_tool_converts_bmp_to_png_image_block() {
    let temp = tempfile::tempdir().expect("tempdir");
    std::fs::write(temp.path().join("pic.bmp"), tiny_bmp_bytes()).expect("write bmp");

    let output = ReadTool::new()
        .execute(
            json!({"file_path": "pic.bmp"}),
            make_ctx(temp.path().to_path_buf()),
        )
        .await
        .expect("read execution should succeed");

    assert_eq!(output.images.len(), 1, "output={:?}", output.output);
    assert_eq!(output.images[0].media_type, "image/png");
    let bytes = base64::Engine::decode(
        &base64::engine::general_purpose::STANDARD,
        &output.images[0].data,
    )
    .expect("valid base64");
    assert!(bytes.starts_with(b"\x89PNG\r\n\x1a\n"));
    assert!(
        output.output.contains("Converted from BMP"),
        "{}",
        output.output
    );
}

/// Garbage behind an image extension produces text only, with a reason.
#[tokio::test]
async fn read_tool_refuses_undecodable_image_without_image_block() {
    let temp = tempfile::tempdir().expect("tempdir");
    std::fs::write(temp.path().join("fake.png"), b"not really a png").expect("write");

    let output = ReadTool::new()
        .execute(
            json!({"file_path": "fake.png"}),
            make_ctx(temp.path().to_path_buf()),
        )
        .await
        .expect("read execution should succeed");

    assert!(output.images.is_empty());
    assert!(
        output.output.contains("Not sent to the model"),
        "{}",
        output.output
    );
}
