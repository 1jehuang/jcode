//! Exercise the public SDK surface used by the desktop, not private API internals.
use jcode_sdk::{
    SessionEditStats, SessionInfo, enrich_sessions_from_edit_stats,
    enrich_sessions_from_local_edit_stats,
};
use serde_json::json;

fn session(id: &str) -> SessionInfo {
    serde_json::from_value(json!({"session_id": id, "status": "idle"})).unwrap()
}

#[test]
fn desktop_edit_stats_contract_is_additive() {
    let mut info = session("old_daemon");
    assert_eq!(info.edit_stats, None);
    assert!(
        serde_json::to_value(&info)
            .unwrap()
            .get("edit_stats")
            .is_none()
    );
    info.edit_stats = Some(SessionEditStats {
        added: 128,
        removed: 37,
        approximate: false,
    });
    let decoded: SessionInfo =
        serde_json::from_value(serde_json::to_value(&info).unwrap()).unwrap();
    assert_eq!(decoded.edit_stats, info.edit_stats);

    // Keep the local helper's desktop-facing signature covered without changing
    // process-global environment or reading the user's actual session records.
    let enrich: fn(&mut [SessionInfo]) = enrich_sessions_from_local_edit_stats;
    enrich(&mut []);
}

#[test]
fn desktop_enrichment_reads_sidecars_and_preserves_api_values() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("edit-stats")).unwrap();
    let sidecar = dir.path().join("edit-stats/local.json");
    std::fs::write(
        &sidecar,
        r#"{"added":128,"removed":37,"approximate":false}"#,
    )
    .unwrap();
    let mut sessions = [session("local"), session("unknown")];
    enrich_sessions_from_edit_stats(&mut sessions, dir.path());
    assert_eq!(
        sessions[0].edit_stats,
        Some(SessionEditStats {
            added: 128,
            removed: 37,
            approximate: false,
        })
    );
    assert_eq!(sessions[1].edit_stats, None);

    std::fs::write(&sidecar, r#"{"added":256,"removed":74,"approximate":true}"#).unwrap();
    // A server-supplied value wins, but a fresh old-daemon response sees updates.
    enrich_sessions_from_edit_stats(&mut sessions, dir.path());
    assert_eq!(sessions[0].edit_stats.unwrap().added, 128);
    let mut fresh = [session("local")];
    enrich_sessions_from_edit_stats(&mut fresh, dir.path());
    assert_eq!(fresh[0].edit_stats.unwrap().added, 256);
    assert!(fresh[0].edit_stats.unwrap().approximate);
}

#[test]
fn desktop_enrichment_rejects_invalid_ids_and_malformed_sidecars() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("edit-stats")).unwrap();
    std::fs::write(dir.path().join("edit-stats/bad.json"), "not json").unwrap();
    std::fs::write(
        dir.path().join("escape.json"),
        r#"{"added":99,"removed":0}"#,
    )
    .unwrap();
    let mut sessions = [session("bad"), session("../escape")];
    enrich_sessions_from_edit_stats(&mut sessions, dir.path());
    assert!(sessions.iter().all(|session| session.edit_stats.is_none()));
}
