// Read-only production sample, 2026-11-08 UTC: Swan snapshot 97,722,380 bytes,
// journal 264,881 bytes. Ran this exact embedded projector over existing managed
// SSH (python3 stdin only), three samples per mode. Snapshot/journal size and
// mtime_ns were unchanged before/after each sample. No daemon RPC or host writes.
// Preview: 80 messages, 181,277 output bytes including framing/telemetry.
// Remote projection ms: 2465.50, 1862.45, 1646.69.
// End-to-end fresh SSH ms: 8923.53, 5664.24, 7751.36.
// Local Python json.loads median ms (20 parses/sample): 0.969, 1.121, 0.904.
// This measures JSON decoding only, NOT Rust parse_preview/hydration. The prior
// ~49 MB cache/~300 ms parse baseline is therefore not an apples-to-apples timing
// comparison, though the transferred projection is now bounded and much smaller.
// Inventory: 278 output bytes; remote ms: 912.64, 1453.10, 736.12.
// End-to-end fresh SSH ms: 4342.86, 4807.31, 4282.34.
// Prior 97 MB inventory baseline was 702 ms: no inventory speedup demonstrated.
// Reproduction script/results: ~/.jcode/scratch/bounded-preview-measure.py and
// ~/.jcode/scratch/bounded-preview-measure-results.jsonl on the measurement host.
use super::*;
use std::io::Write;

fn run_projection(home: &std::path::Path, mode: &str, id: &str) -> std::process::Output {
    let mut child = std::process::Command::new("python3")
        .args(["-", mode, id])
        .env("JCODE_HOME", home)
        .env_remove("JCODE_RUNTIME_DIR")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("Python is required by the remote projection contract");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(REMOTE_PROJECTOR.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

#[test]
fn oversized_remote_snapshot_is_bounded_valid_and_keeps_late_journal_updates() {
    let home = tempfile::tempdir().unwrap();
    let sessions = home.path().join("sessions");
    std::fs::create_dir(&sessions).unwrap();
    let id = "oversized_preview";
    let session = Session::create_with_id(id.into(), None, Some("Old title".into()));
    let mut snapshot = serde_json::to_value(&session).unwrap();
    snapshot["migration_epoch"] = serde_json::json!(19);
    snapshot["messages"] = serde_json::json!([{
        "id": "huge", "role": "user", "content": [{"type": "text", "text": "x".repeat(65 * 1024 * 1024)}]
    }]);
    let path = sessions.join(format!("{id}.json"));
    serde_json::to_writer(std::fs::File::create(&path).unwrap(), &snapshot).unwrap();
    drop(snapshot);
    let before = std::fs::metadata(&path).unwrap();
    assert!(before.len() > 64 * 1024 * 1024);
    let first_preview = run_projection(home.path(), "preview", id);
    assert!(first_preview.stdout.len() < 2 * 1024 * 1024);
    let (first, _) = parse_preview(&first_preview.stdout)
        .expect("oversized individual message projection must remain valid");
    assert_eq!(first.messages.len(), 1);
    assert_eq!(first.messages[0].id, "huge");
    let mut journal = std::fs::File::create(sessions.join(format!("{id}.journal.jsonl"))).unwrap();
    for i in 0..100 {
        writeln!(journal, "{}", serde_json::json!({
            "meta": {"title": format!("Late journal title {i}"), "working_dir": "/latest/repository", "updated_at": "2026-11-08T05:00:00Z"},
            "append_messages": [{"id": format!("late-{i}"), "role": "user", "content": [{"type": "text", "text": format!("Late journal prompt {i}")}]}]
        })).unwrap();
    }
    drop(journal);
    let preview = run_projection(home.path(), "preview", id);
    assert!(preview.stdout.len() < 2 * 1024 * 1024);
    let (loaded, _) =
        parse_preview(&preview.stdout).expect("projection must remain valid Session JSON");
    assert_eq!(loaded.title.as_deref(), Some("Late journal title 99"));
    assert_eq!(loaded.messages.len(), 80);
    assert_eq!(loaded.messages.last().unwrap().id, "late-99");
    assert_eq!(loaded.migration_epoch, 19);
    assert!(
        build_messages_preview(&loaded)
            .iter()
            .any(|m| m.content.contains("Late journal prompt 99"))
    );
    let inventory = run_projection(home.path(), "inventory", id);
    let inventory: serde_json::Value = serde_json::from_slice(&inventory.stdout).unwrap();
    assert_eq!(inventory["sessions"][0]["title"], "Late journal title 99");
    assert_eq!(inventory["sessions"][0]["cwd"], "/latest/repository");
    assert_eq!(inventory["sessions"][0]["epoch"], 19);
    assert_eq!(
        inventory["sessions"][0]["updated_at"],
        "2026-11-08T05:00:00Z"
    );
    assert_eq!(std::fs::metadata(&path).unwrap().len(), before.len());
    assert_eq!(
        std::fs::metadata(&path).unwrap().modified().unwrap(),
        before.modified().unwrap()
    );
    assert_eq!(std::fs::read_dir(&sessions).unwrap().count(), 2);
}

#[test]
fn remote_projection_ignores_stale_sidecar_and_respects_away_lease() {
    let home = tempfile::tempdir().unwrap();
    let sessions = home.path().join("sessions");
    std::fs::create_dir(&sessions).unwrap();
    let session = Session::create_with_id("authority".into(), None, Some("Canonical title".into()));
    serde_json::to_writer(
        std::fs::File::create(sessions.join("authority.json")).unwrap(),
        &session,
    )
    .unwrap();
    std::fs::write(
        sessions.join("authority.meta.json"),
        br#"{"title":"Stale sidecar","migration_epoch":999}"#,
    )
    .unwrap();
    let result = run_projection(home.path(), "inventory", "authority");
    let result: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(result["sessions"][0]["title"], "Canonical title");
    assert_eq!(result["sessions"][0]["epoch"], 0);
    std::fs::create_dir(home.path().join("session_leases")).unwrap();
    std::fs::write(
        home.path().join("session_leases/authority.json"),
        br#"{"away_host":"other-owner","epoch":12}"#,
    )
    .unwrap();
    let result = run_projection(home.path(), "inventory", "authority");
    let result: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert!(result["sessions"].as_array().unwrap().is_empty());
}
