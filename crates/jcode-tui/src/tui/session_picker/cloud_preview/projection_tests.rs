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
// Scanner follow-up, 2026-11-08 UTC, same unchanged 97,722,380-byte snapshot:
// Whole-string regex alone did not improve latency. Coalescing scalar tokens
// plus retaining the last 80 message ranges during the first scan avoids the
// second full messages scan. Actual mmap preview remote ms: 768.34, 1381.58,
// 1376.54, fresh SSH ms: 4695.30, 5622.85, 5616.78. All retain 80 messages.
// Journal grew externally between samples (363,579 -> 384,539 bytes), but each
// individual read verified snapshot/journal size+mtime unchanged during projection.
// Paired comparison freezes both files in process RAM, no remote file writes:
// old preview ms 1419.64, 2489.48, 1444.72; new 1315.81, 1160.17, 772.47.
// Every pair is byte-identical (168,621 bytes, without framing/telemetry), SHA256
// cc39e053c4a2e80dd767fee2a4142f784a44c2cd10d6685a97493d5a04cf75fc.
// Inventory actual mmap ms 1019.11, 1507.74, 742.82: no consistent speedup.
// Evidence: ~/.jcode/scratch/bounded-preview-final-scanner-results.jsonl and
// paired-projection-final-results.txt, reproduced by paired-projection.py.
use super::*;
use std::io::Write;

#[test]
fn remote_scanner_handles_escaped_nested_strings_and_rejects_truncation() {
    let checks = r#"
import random
random.seed(37)
samples = ['', '"', '\\', '\\"', '\\\\"', '{}[]', '\n\t', '雪😀']
for _ in range(200):
    samples.append(''.join(random.choices('abc"\\{}[]\n雪', k=120)))
for text in samples:
    value = {'nested': [{'text': text, 'braces': ']}{['}, [text]], 'tail': text}
    encoded = json.dumps(value, ensure_ascii=False).encode()
    assert value_end(encoded + b',trailer', 0) == len(encoded)
    quoted = json.dumps(text, ensure_ascii=False).encode()
    assert string_end(quoted + b',trailer', 0) == len(quoted)
    assert json.loads(encoded) == value
messages = [{'id': str(i), 'content': [{'text': samples[i % len(samples)]}]} for i in range(300)]
encoded = json.dumps({'messages': messages}).encode()
ranges = collections.deque(maxlen=MAX_MESSAGES)
list(fields(encoded, message_ranges=ranges))
assert len(ranges) == MAX_MESSAGES
assert [json.loads(encoded[a:b]) for a,b in ranges] == messages[-MAX_MESSAGES:]
encoded = b'{"messages":[{"id":"old"}],"messages":[]}'
for key,a,b in fields(encoded, message_ranges=ranges):
    pass
assert not ranges
for malformed in (b'{', b'[', b'{"x":"unterminated}', b'["escaped\\"', b'{]', b'[{]}'):
    try:
        value_end(malformed, 0)
    except (ValueError, IndexError):
        pass
    else:
        raise AssertionError(repr(malformed))
"#;
    let mut child = std::process::Command::new("python3")
        .arg("-")
        .stdin(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let definitions = REMOTE_PROJECTOR.split("if __name__ ==").next().unwrap();
    let mut input = child.stdin.take().unwrap();
    write!(input, "{definitions}\n{checks}").unwrap();
    drop(input);
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

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
            "append_messages": [{"id": format!("late-{i}"), "role": "user", "content": [{"type": "text", "text": format!("Late journal prompt {i} escaped quote \" slash \\ nested {{[}}] 雪")}]}]
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
    assert!(build_messages_preview(&loaded).iter().any(|m| {
        m.content
            .contains("Late journal prompt 99 escaped quote \" slash \\ nested {[}] 雪")
    }));
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
