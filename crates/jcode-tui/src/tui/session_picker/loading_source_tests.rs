use super::tests::EnvVarGuard;
use super::*;

/// Native snapshots and external transcripts keep distinct picker sources even
/// when the native session uses the other application's provider.
#[test]
fn picker_source_tracks_session_origin() {
    let _env_lock = crate::storage::lock_test_env();
    let home = tempfile::tempdir().unwrap();
    let _home = EnvVarGuard::set_path("JCODE_HOME", home.path());
    let sessions = home.path().join("sessions");
    std::fs::create_dir(&sessions).unwrap();
    let catchup = crate::catchup::CatchupSeenSnapshot::load();
    let now = chrono::Utc::now();

    for (id, provider, model) in [
        ("session_native_opencode", "opencode-go", "glm-5"),
        ("session_native_codex_model", "openai", "gpt-5-codex"),
    ] {
        let snapshot = serde_json::json!({
            "created_at": now,
            "updated_at": now,
            "provider_key": provider,
            "model": model,
            "messages": [{"role": "user", "content": [{"type": "text", "text": "Hello"}]}]
        });
        std::fs::write(
            sessions.join(format!("{id}.json")),
            serde_json::to_vec(&snapshot).unwrap(),
        )
        .unwrap();
        let session = parse_jcode_session_info(&sessions, id, &catchup).unwrap();
        assert_eq!(session.provider_key.as_deref(), Some(provider));
        assert_eq!(session.source, SessionSource::Jcode, "{id}");
    }

    let external_path = home.path().join("opencode.json");
    std::fs::write(
        &external_path,
        r#"{"id":"ses_external","title":"External"}"#,
    )
    .unwrap();
    let external = load_opencode_session_stub(&external_path).unwrap().unwrap();
    assert_eq!(external.source, SessionSource::OpenCode);
}
