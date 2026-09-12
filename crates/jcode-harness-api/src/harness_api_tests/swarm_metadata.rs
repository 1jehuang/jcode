use super::*;
use serde_json::json;

struct TempDir(std::path::PathBuf);
impl TempDir {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "jcode-swarm-metadata-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn write(&self, name: &str, value: serde_json::Value) {
        std::fs::write(self.0.join(name), value.to_string()).unwrap();
    }
}
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn session(id: &str) -> SessionInfo {
    serde_json::from_value(json!({"session_id": id, "status": "idle"})).unwrap()
}

#[test]
fn old_session_wire_shape_defaults_and_omits_swarm_fields() {
    let session = session("legacy");
    assert_eq!(session.parent_session_id, None);
    assert_eq!(session.agent_label, None);
    assert_eq!(session.swarm_status, None);
    let value = serde_json::to_value(session).unwrap();
    for field in ["parent_session_id", "agent_label", "swarm_status"] {
        assert!(value.get(field).is_none());
    }
}

#[test]
fn new_session_wire_shape_roundtrips_unknown_status() {
    let value = json!({"session_id": "worker", "status": "idle", "parent_session_id": "root",
        "agent_label": "API reviewer", "swarm_status": "future_status"});
    let session: SessionInfo = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(serde_json::to_value(session).unwrap(), value);
}

#[test]
fn enrichment_maps_only_swarm_ownership_and_preserves_title() {
    let dir = TempDir::new();
    dir.write("swarm.json", json!({"updated_at_unix_ms": 1, "members": [
        {"session_id": "root", "status": "running"},
        {"session_id": "worker", "report_back_to_session_id": "root", "task_label": "review", "status": "ready"},
        {"session_id": "grandchild", "report_back_to_session_id": "worker", "status": "completed"}
    ]}));
    let mut sessions = vec![
        session("root"),
        session("worker"),
        session("grandchild"),
        serde_json::from_value(
            json!({"session_id": "fork", "status": "idle", "parent_id": "root"}),
        )
        .unwrap(),
    ];
    sessions[1].title = Some("Custom title".into());
    enrich_sessions_from_swarm_state(&mut sessions, &dir.0);
    assert_eq!(sessions[0].parent_session_id, None);
    assert_eq!(sessions[0].swarm_status.as_deref(), Some("running"));
    assert_eq!(sessions[1].parent_session_id.as_deref(), Some("root"));
    assert_eq!(sessions[1].agent_label.as_deref(), Some("review"));
    assert_eq!(sessions[1].swarm_status.as_deref(), Some("ready"));
    assert_eq!(sessions[1].title.as_deref(), Some("Custom title"));
    assert_eq!(sessions[1].status, "idle");
    assert_eq!(sessions[2].parent_session_id.as_deref(), Some("worker"));
    assert_eq!(sessions[3].parent_session_id, None);
    assert_eq!(sessions[3].swarm_status, None);
}

#[test]
fn enrichment_prefers_latest_snapshot_and_existing_api_values() {
    let dir = TempDir::new();
    dir.write("a-new.json", json!({"updated_at_unix_ms": 20, "members": [
        {"session_id": "worker", "report_back_to_session_id": "new-root", "task_label": "new", "status": "completed"}
    ]}));
    dir.write("z-old.json", json!({"updated_at_unix_ms": 10, "members": [
        {"session_id": "worker", "report_back_to_session_id": "old-root", "task_label": "old", "status": "running"}
    ]}));
    let mut sessions = vec![session("worker")];
    enrich_sessions_from_swarm_state(&mut sessions, &dir.0);
    assert_eq!(sessions[0].parent_session_id.as_deref(), Some("new-root"));
    assert_eq!(sessions[0].agent_label.as_deref(), Some("new"));
    assert_eq!(sessions[0].swarm_status.as_deref(), Some("completed"));
    sessions[0].parent_session_id = Some("api-root".into());
    sessions[0].agent_label = Some("api-label".into());
    sessions[0].swarm_status = Some("running".into());
    let before = sessions.clone();
    enrich_sessions_from_swarm_state(&mut sessions, &dir.0);
    assert_eq!(sessions, before);
}

#[test]
fn malformed_missing_and_legacy_snapshots_degrade_per_record() {
    let dir = TempDir::new();
    std::fs::write(dir.0.join("broken.json"), "{").unwrap();
    dir.write(
        "ignored.json.bak",
        json!({"members": [{"session_id":"ignored", "status":"running"}]}),
    );
    dir.write("legacy.json", json!({"members": [
        {"session_id": 123},
        {"session_id": "legacy", "report_back_to_session_id": "root"},
        {"session_id": "self", "report_back_to_session_id": "self", "task_label": "  ", "status": ""}
    ]}));
    let mut sessions = vec![session("legacy"), session("self"), session("ignored")];
    enrich_sessions_from_swarm_state(&mut sessions, dir.0.join("missing"));
    assert_eq!(sessions[0].parent_session_id, None);
    enrich_sessions_from_swarm_state(&mut sessions, &dir.0);
    assert_eq!(sessions[0].parent_session_id.as_deref(), Some("root"));
    assert_eq!(sessions[0].agent_label, None);
    assert_eq!(sessions[0].swarm_status, None);
    assert_eq!(sessions[1], session("self"));
    assert_eq!(sessions[2], session("ignored"));
}
