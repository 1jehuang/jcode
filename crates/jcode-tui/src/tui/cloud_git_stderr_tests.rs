use super::*;
use crate::tui::cloud_git::{bounded_output, script_command};

#[test]
fn workspace_operations_are_static_and_all_production_paths_are_assigned() {
    use crate::tui::cloud_git::WorkspaceOperation::*;
    assert!(include_str!("cloud_git.rs").contains("timing.operation(operation.label())"));
    assert_eq!(
        [
            GitProbe,
            InventoryPrepare,
            InventoryProjection,
            Preview,
            CloudStatus,
            CloudStatusDirectories
        ]
        .map(|operation| operation.label()),
        [
            "git_probe",
            "inventory_prepare",
            "inventory_projection",
            "preview",
            "cloud_status",
            "cloud_status_directories"
        ]
    );
    // These source contracts cover cfg(not(test)) workers too, without making
    // live SSH calls or putting any workspace/session content in timing labels.
    for (source, operations) in [
        (include_str!("cloud_git.rs"), &["GitProbe"][..]),
        (
            include_str!("cloud_status.rs"),
            &["CloudStatus", "CloudStatusDirectories"][..],
        ),
        (
            include_str!("session_picker/cloud_preview.rs"),
            &["Preview"][..],
        ),
        (
            include_str!("session_picker/loading/mobile_inventory.rs"),
            &["InventoryPrepare", "InventoryProjection"][..],
        ),
    ] {
        let boundary = if operations == ["Preview"] {
            "fn parse_preview("
        } else {
            "#[cfg(test)]"
        };
        let production = source.split(boundary).next().unwrap();
        assert!(!production.contains("bounded_output("));
        for operation in operations {
            assert!(production.contains(&format!("WorkspaceOperation::{operation}")));
        }
    }
}
use std::time::Duration;

const SAFE: &str = "[CLOUD_TIMING] stage=projection outcome=success duration_ms=17";

#[test]
fn exact_whitelist_rejects_auth_content_and_injection() {
    assert_eq!(timing_record(SAFE.as_bytes()).as_deref(), Some(SAFE));
    for line in [
        "Authorization: Bearer SECRET".to_string(),
        SAFE.replace("projection", "secret-session-id"),
        SAFE.replace("success", "private-error"),
        format!("{SAFE} token=SECRET"),
        format!("{SAFE}\r"),
        format!(" {SAFE}"),
        SAFE.replace("17", "NaN"),
        SAFE.replace("17", "-1"),
        SAFE.replace("17", "1.2"),
        SAFE.replace("17", "18446744073709551616"),
        SAFE.replace("17", "17\u{1b}[31m"),
    ] {
        assert!(timing_record(line.as_bytes()).is_none());
    }
    assert!(timing_record(b"\xff").is_none());
}

#[tokio::test]
async fn oversized_lines_and_stream_have_fixed_log_and_scan_budgets() {
    let mut input = vec![b'x'; MAX_LINE + 1];
    input.extend_from_slice(SAFE.as_bytes()); // A suffix must not masquerade as a line.
    input.push(b'\n');
    input.extend_from_slice(SAFE.as_bytes());
    input.push(b'\n');
    let mut records = Vec::new();
    drain(input.as_slice(), |record| records.push(record)).await;
    assert_eq!(records, [SAFE]);
    let input = format!("{SAFE}\n").repeat(RECORD_BUDGET * 100);
    records.clear();
    drain(input.as_bytes(), |record| records.push(record)).await;
    assert_eq!(records.len(), RECORD_BUDGET);
    let mut input = vec![b'x'; SCAN_BUDGET];
    input.push(b'\n');
    input.extend_from_slice(format!("{SAFE}\n").as_bytes());
    records.clear();
    drain(input.as_slice(), |record| records.push(record)).await;
    assert!(records.is_empty());
}

#[tokio::test]
async fn split_records_and_incomplete_eof_do_not_leak() {
    use tokio::io::AsyncWriteExt;
    let (mut writer, reader) = tokio::io::duplex(8);
    let producer = async {
        writer
            .write_all(format!("SECRET\n{SAFE}\n{SAFE}").as_bytes())
            .await
            .unwrap();
        drop(writer);
    };
    let mut records = Vec::new();
    tokio::join!(producer, drain(reader, |record| records.push(record)));
    assert_eq!(records, [SAFE]);
}

#[tokio::test]
async fn concurrent_large_stderr_cannot_deadlock_stdout_or_hide_exit_failure() {
    for exit in [0, 7] {
        let script = format!(
            "head -c 2097152 /dev/zero >&2; printf '%s\\n' '{SAFE}' >&2; printf payload; exit {exit}"
        );
        let command = script_command(None, None, "unused", &script);
        let (status, bytes) = bounded_output(command, Duration::from_secs(5), 64)
            .await
            .unwrap();
        assert_eq!(status.code(), Some(exit));
        assert_eq!(bytes, b"payload");
    }
}

#[tokio::test]
async fn oversized_stdout_cancels_infinite_stderr_without_waiting_for_deadline() {
    let command = script_command(
        None,
        None,
        "unused",
        "(while :; do printf secret >&2; done) & head -c 65 /dev/zero; wait",
    );
    let started = std::time::Instant::now();
    assert!(
        bounded_output(command, Duration::from_secs(10), 64)
            .await
            .is_none()
    );
    assert!(started.elapsed() < Duration::from_secs(2));
}

#[cfg(target_os = "linux")]
async fn assert_processes_stopped(path: &std::path::Path) {
    let pids = std::fs::read_to_string(path).unwrap();
    for _ in 0..100 {
        let alive = pids.split_whitespace().any(|pid| {
            std::fs::read_to_string(format!("/proc/{pid}/stat"))
                .ok()
                .and_then(|stat| {
                    stat.rsplit_once(") ")
                        .map(|(_, rest)| !rest.starts_with('Z'))
                })
                .unwrap_or(false)
        });
        if !alive {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("command process group survived cancellation");
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn deadline_and_cancellation_kill_pipe_holding_descendants() {
    for cancel in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let pids = temp.path().join("pids");
        let command = script_command(
            None,
            None,
            pids.to_str().unwrap(),
            "sleep 30 & printf '%s %s' \"$$\" \"$!\" > \"$1\"; wait",
        );
        let task = tokio::spawn(bounded_output(command, Duration::from_millis(500), 64));
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if std::fs::read_to_string(&pids).is_ok_and(|s| s.split_whitespace().count() == 2) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        if cancel {
            task.abort();
            assert!(task.await.unwrap_err().is_cancelled());
        } else {
            assert!(task.await.unwrap().is_none());
        }
        assert_processes_stopped(&pids).await;
    }
}
