//! End-to-end Claude Code import integration tests (swarm Worker C / dolphin).
//!
//! These drive the real `jcode_base::import` pipeline against synthetic
//! `.jsonl` fixtures written into a sandboxed `KCODE_HOME`, so they never touch
//! the developer's real `~/.claude`. They assert the *target* behavior agreed
//! with the swarm coordinator:
//!
//!   * a single malformed/odd block must never drop the whole line;
//!   * a tool_result image must import as a `[image]` placeholder, never base64;
//!   * sidechain entries are excluded from the imported transcript;
//!   * one malformed JSON line is skipped while good lines around it import;
//!   * empty / meta-only transcripts are filtered out of the session list;
//!   * a sessions-index.json pointing at a missing path falls back to the
//!     sibling `<id>.jsonl` file (resolve_claude_session_path).
//!
//! All tests serialize on `lock_test_env()` because they mutate the process
//! environment (`KCODE_HOME`).

use jcode_base::import::{
    import_session, import_session_from_file, list_claude_code_sessions,
    list_claude_code_sessions_lazy,
};
use jcode_base::message::ContentBlock;
use jcode_base::session::Session;
use jcode_import_core::imported_claude_code_session_id;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Serializes tests that mutate `KCODE_HOME`. This test binary has its own
/// address space, so a file-local mutex is sufficient (and avoids depending on
/// the `test-support`-gated `storage::lock_test_env`).
static ENV_LOCK: Mutex<()> = Mutex::new(());

/// RAII guard that sets/restores an env var, mirroring `import_tests.rs`.
struct EnvVarGuard {
    key: &'static str,
    prev: Option<std::ffi::OsString>,
}

impl EnvVarGuard {
    fn set_path(key: &'static str, value: &Path) -> Self {
        let prev = std::env::var_os(key);
        jcode_base::env::set_var(key, value);
        Self { key, prev }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        if let Some(prev) = self.prev.take() {
            jcode_base::env::set_var(self.key, prev);
        } else {
            jcode_base::env::remove_var(self.key);
        }
    }
}

/// Create the sandboxed Claude project dir under `$KCODE_HOME/external/...`.
fn make_project_dir(home: &Path) -> PathBuf {
    let dir = home.join("external/.claude/projects/-Users-demo");
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn all_text(session: &Session) -> String {
    session
        .messages
        .iter()
        .flat_map(|m| m.content.iter())
        .filter_map(|b| match b {
            ContentBlock::Text { text, .. } => Some(text.clone()),
            ContentBlock::ToolResult { content, .. } => Some(content.clone()),
            ContentBlock::Reasoning { text } => Some(text.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

// ---------------------------------------------------------------------------
// Image tool_result imports as a placeholder, never base64 (edge cases 1 + 9)
// ---------------------------------------------------------------------------

#[test]
fn import_array_tool_result_with_image_uses_placeholder_not_base64() {
    let _guard = ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let temp = tempfile::TempDir::new().unwrap();
    let _home = EnvVarGuard::set_path("KCODE_HOME", temp.path());
    let project_dir = make_project_dir(temp.path());

    let big = "Z".repeat(80_000);
    let path = project_dir.join("img-session.jsonl");
    let line_user = r#"{"type":"user","uuid":"u1","sessionId":"img-session","cwd":"/tmp/demo","message":{"role":"user","content":"take a screenshot"},"timestamp":"2026-05-01T10:00:00Z"}"#;
    // assistant requests the screenshot tool
    let line_tooluse = r#"{"type":"assistant","uuid":"a1","parentUuid":"u1","sessionId":"img-session","message":{"role":"assistant","model":"claude-sonnet-4-6","content":[{"type":"tool_use","id":"call_1","name":"screenshot","input":{}}]},"timestamp":"2026-05-01T10:00:01Z"}"#;
    // user delivers an ARRAY-form tool_result with text + a huge base64 image
    let line_toolresult = format!(
        r#"{{"type":"user","uuid":"u2","parentUuid":"a1","sessionId":"img-session","message":{{"role":"user","content":[{{"type":"tool_result","tool_use_id":"call_1","content":[{{"type":"text","text":"captured"}},{{"type":"image","source":{{"type":"base64","media_type":"image/png","data":"{big}"}}}}]}}]}},"timestamp":"2026-05-01T10:00:02Z"}}"#
    );
    std::fs::write(
        &path,
        format!("{line_user}\n{line_tooluse}\n{line_toolresult}\n"),
    )
    .unwrap();

    let session = import_session_from_file(&path, "img-session").unwrap();

    // All three message lines survived (none dropped).
    assert_eq!(
        session.messages.len(),
        3,
        "expected user+assistant+toolresult to all import, got {}",
        session.messages.len()
    );

    let text = all_text(&session);
    assert!(text.contains("captured"), "tool_result text must survive");
    assert!(text.contains("[image]"), "image must become a placeholder");
    assert!(
        !text.contains(&big),
        "base64 image data must NOT be embedded in the imported transcript"
    );

    // The persisted on-disk session must also be free of base64 bloat.
    let saved = std::fs::read_to_string(jcode_session_file(temp.path(), "img-session")).unwrap();
    assert!(
        !saved.contains(&big),
        "persisted session JSON must not contain base64 image data"
    );
}

/// Path to where jcode persists imported sessions under the sandbox.
/// With `KCODE_HOME` set, `storage::jcode_dir()` returns it directly, and
/// sessions live at `<KCODE_HOME>/sessions/<id>.json`.
fn jcode_session_file(home: &Path, claude_id: &str) -> PathBuf {
    let id = imported_claude_code_session_id(claude_id);
    home.join(format!("sessions/{id}.json"))
}

// ---------------------------------------------------------------------------
// null / missing tool_result content (edge case 1)
// ---------------------------------------------------------------------------

#[test]
fn import_tool_result_null_and_missing_content_does_not_drop_lines() {
    let _guard = ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let temp = tempfile::TempDir::new().unwrap();
    let _home = EnvVarGuard::set_path("KCODE_HOME", temp.path());
    let project_dir = make_project_dir(temp.path());

    let path = project_dir.join("nullc.jsonl");
    let lines = concat!(
        r#"{"type":"user","uuid":"u1","sessionId":"nullc","message":{"role":"user","content":"hi"},"timestamp":"2026-05-01T10:00:00Z"}"#,
        "\n",
        // tool_result content == null, plus a sibling text block so the message is non-empty
        r#"{"type":"user","uuid":"u2","parentUuid":"u1","sessionId":"nullc","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":null},{"type":"text","text":"after null"}]},"timestamp":"2026-05-01T10:00:01Z"}"#,
        "\n",
        // tool_result content missing entirely, plus sibling text
        r#"{"type":"assistant","uuid":"a1","parentUuid":"u2","sessionId":"nullc","message":{"role":"assistant","content":[{"type":"tool_result","tool_use_id":"t2"},{"type":"text","text":"after missing"}]},"timestamp":"2026-05-01T10:00:02Z"}"#,
        "\n",
    );
    std::fs::write(&path, lines).unwrap();

    let session = import_session_from_file(&path, "nullc").unwrap();
    assert_eq!(session.messages.len(), 3, "no line should be dropped");
    let text = all_text(&session);
    assert!(text.contains("after null"));
    assert!(text.contains("after missing"));
}

// ---------------------------------------------------------------------------
// Sidechain exclusion through the import pipeline (edge case 4)
// ---------------------------------------------------------------------------

#[test]
fn import_excludes_sidechain_messages() {
    let _guard = ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let temp = tempfile::TempDir::new().unwrap();
    let _home = EnvVarGuard::set_path("KCODE_HOME", temp.path());
    let project_dir = make_project_dir(temp.path());

    let path = project_dir.join("side.jsonl");
    let lines = concat!(
        r#"{"type":"user","uuid":"m1","sessionId":"side","message":{"role":"user","content":"main question"},"timestamp":"2026-05-01T10:00:00Z"}"#,
        "\n",
        r#"{"type":"assistant","uuid":"s1","parentUuid":"m1","isSidechain":true,"sessionId":"side","message":{"role":"assistant","content":"SIDECHAIN_SECRET"},"timestamp":"2026-05-01T10:00:01Z"}"#,
        "\n",
        r#"{"type":"assistant","uuid":"m2","parentUuid":"m1","sessionId":"side","message":{"role":"assistant","content":"main answer"},"timestamp":"2026-05-01T10:00:02Z"}"#,
        "\n",
    );
    std::fs::write(&path, lines).unwrap();

    let session = import_session_from_file(&path, "side").unwrap();
    let text = all_text(&session);
    assert!(
        !text.contains("SIDECHAIN_SECRET"),
        "sidechain message must be excluded from imported transcript"
    );
    assert!(text.contains("main question"));
    assert!(text.contains("main answer"));
    assert_eq!(session.messages.len(), 2);
}

// ---------------------------------------------------------------------------
// Malformed JSON lines skipped individually (edge case 6)
// ---------------------------------------------------------------------------

#[test]
fn import_skips_only_malformed_lines() {
    let _guard = ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let temp = tempfile::TempDir::new().unwrap();
    let _home = EnvVarGuard::set_path("KCODE_HOME", temp.path());
    let project_dir = make_project_dir(temp.path());

    let path = project_dir.join("mixed.jsonl");
    let lines = concat!(
        r#"{"type":"user","uuid":"u1","sessionId":"mixed","message":{"role":"user","content":"good one"},"timestamp":"2026-05-01T10:00:00Z"}"#,
        "\n",
        "{ this is not valid json at all }\n",
        "\n", // blank line
        r#"{"type":"assistant","uuid":"a1","parentUuid":"u1","sessionId":"mixed","message":{"role":"assistant","content":"good two"},"timestamp":"2026-05-01T10:00:02Z"}"#,
        "\n",
    );
    std::fs::write(&path, lines).unwrap();

    let session = import_session_from_file(&path, "mixed").unwrap();
    assert_eq!(
        session.messages.len(),
        2,
        "only the malformed line should be skipped"
    );
    let text = all_text(&session);
    assert!(text.contains("good one"));
    assert!(text.contains("good two"));
}

// ---------------------------------------------------------------------------
// Empty / meta-only transcripts filtered from the session list (edge case 7)
// ---------------------------------------------------------------------------

#[test]
fn list_filters_empty_and_meta_only_transcripts() {
    let _guard = ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let temp = tempfile::TempDir::new().unwrap();
    let _home = EnvVarGuard::set_path("KCODE_HOME", temp.path());
    let project_dir = make_project_dir(temp.path());

    // Empty file.
    std::fs::write(project_dir.join("empty.jsonl"), "").unwrap();
    // Only summary / system meta lines, no user/assistant messages.
    std::fs::write(
        project_dir.join("meta-only.jsonl"),
        concat!(
            r#"{"type":"summary","summary":"Some summary","leafUuid":"x"}"#,
            "\n",
            r#"{"type":"system","sessionId":"meta-only","content":"boot"}"#,
            "\n",
        ),
    )
    .unwrap();
    // A real session that SHOULD show up.
    std::fs::write(
        project_dir.join("real.jsonl"),
        concat!(
            r#"{"type":"user","uuid":"u1","sessionId":"real","message":{"role":"user","content":"real prompt"},"timestamp":"2026-05-01T10:00:00Z"}"#, "\n",
            r#"{"type":"assistant","uuid":"a1","parentUuid":"u1","sessionId":"real","message":{"role":"assistant","content":"real answer"},"timestamp":"2026-05-01T10:00:01Z"}"#, "\n",
        ),
    )
    .unwrap();

    let sessions = list_claude_code_sessions().unwrap();
    let ids: Vec<&str> = sessions.iter().map(|s| s.session_id.as_str()).collect();
    assert!(
        ids.contains(&"real"),
        "real session must be listed: {ids:?}"
    );
    assert!(
        !ids.contains(&"empty"),
        "empty transcript must be filtered out: {ids:?}"
    );
    assert!(
        !ids.contains(&"meta-only"),
        "meta-only transcript must be filtered out: {ids:?}"
    );
}

// ---------------------------------------------------------------------------
// sessions-index.json pointing at a missing path -> fallback to sibling file
// (edge case 8, resolve_claude_session_path)
// ---------------------------------------------------------------------------

#[test]
fn index_with_missing_full_path_falls_back_to_sibling_jsonl() {
    let _guard = ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let temp = tempfile::TempDir::new().unwrap();
    let _home = EnvVarGuard::set_path("KCODE_HOME", temp.path());
    let project_dir = make_project_dir(temp.path());

    // The actual transcript lives next to the index under <id>.jsonl ...
    let transcript = project_dir.join("renamed-session.jsonl");
    std::fs::write(
        &transcript,
        concat!(
            r#"{"type":"user","uuid":"u1","sessionId":"renamed-session","cwd":"/tmp/demo","message":{"role":"user","content":"recover me"},"timestamp":"2026-05-01T10:00:00Z"}"#, "\n",
            r#"{"type":"assistant","uuid":"a1","parentUuid":"u1","sessionId":"renamed-session","message":{"role":"assistant","model":"claude-sonnet-4-6","content":"recovered"},"timestamp":"2026-05-01T10:00:01Z"}"#, "\n",
        ),
    )
    .unwrap();

    // ... but the index points fullPath at a stale/missing location.
    std::fs::write(
        project_dir.join("sessions-index.json"),
        concat!(
            "{\"version\":1,\"entries\":[",
            "{\"sessionId\":\"renamed-session\",",
            "\"fullPath\":\"/nonexistent/old/renamed-session.jsonl\",",
            "\"firstPrompt\":\"recover me\",",
            "\"summary\":\"recover me\",",
            "\"messageCount\":2,",
            "\"created\":\"2026-05-01T10:00:00Z\",",
            "\"modified\":\"2026-05-01T10:00:01Z\",",
            "\"projectPath\":\"/tmp/demo\"",
            "}]}"
        ),
    )
    .unwrap();

    let sessions = list_claude_code_sessions().unwrap();
    let found = sessions
        .iter()
        .find(|s| s.session_id == "renamed-session")
        .expect("session must resolve via sibling-file fallback");
    assert_eq!(found.full_path, transcript.to_string_lossy());

    // And it must actually import from the recovered path.
    let imported = import_session("renamed-session").unwrap();
    assert_eq!(imported.messages.len(), 2);
    assert!(all_text(&imported).contains("recovered"));
}

// ---------------------------------------------------------------------------
// Lazy lister tolerates a huge/odd transcript without choking (robustness)
// ---------------------------------------------------------------------------

#[test]
fn lazy_lister_handles_array_tool_result_session() {
    let _guard = ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let temp = tempfile::TempDir::new().unwrap();
    let _home = EnvVarGuard::set_path("KCODE_HOME", temp.path());
    let project_dir = make_project_dir(temp.path());

    let big = "Y".repeat(40_000);
    let path = project_dir.join("lazy.jsonl");
    let line = format!(
        r#"{{"type":"user","uuid":"u1","sessionId":"lazy","message":{{"role":"user","content":[{{"type":"tool_result","tool_use_id":"t1","content":[{{"type":"image","source":{{"type":"base64","media_type":"image/png","data":"{big}"}}}}]}}]}},"timestamp":"2026-05-01T10:00:00Z"}}"#
    );
    std::fs::write(&path, format!("{line}\n")).unwrap();

    // Should not panic and should surface the session id.
    let sessions = list_claude_code_sessions_lazy(50).unwrap();
    assert!(
        sessions.iter().any(|s| s.session_id == "lazy"),
        "lazy lister should detect the session"
    );
}
