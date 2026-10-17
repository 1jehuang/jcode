//! Durable, at-least-once delivery for spend events. The server must deduplicate
//! `(id, event_id)`: a lost HTTP acknowledgement can always cause a replay.
//! No age or capacity eviction is performed. Events rejected beyond the server's
//! replay horizon remain on disk for inspection, not silently counted or dropped.
//! Opt-out deliberately deletes pending reports. A crash before publication,
//! unavailable/full storage, or an unreported provider usage frame can still lose
//! usage. Staging files from interrupted publication are never treated as events.
use super::*;
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::Path;
#[cfg(not(test))]
use std::path::PathBuf;

#[cfg(not(test))]
fn directory() -> io::Result<PathBuf> {
    storage::jcode_dir()
        .map(|dir| dir.join("telemetry_usage_outbox"))
        .map_err(io::Error::other)
}

pub(super) fn acknowledged(response: reqwest::blocking::Response, payload: &Value) -> bool {
    let accepted = response.status().is_success()
        && response.json::<Value>().ok().is_some_and(|receipt| {
            receipt["durable"] == true
                && receipt["id"].as_str() == payload["id"].as_str()
                && receipt["event_id"].as_str() == payload["event_id"].as_str()
                && receipt["id"].as_str().is_some()
                && receipt["event_id"].as_str().is_some()
        });
    if !accepted {
        logging::warn(
            "usage telemetry acknowledgement missing durable matching receipt; retaining event",
        );
    }
    accepted
}

fn persist(dir: &Path, payload: &Value) -> io::Result<()> {
    let event_id = payload["event_id"]
        .as_str()
        .and_then(|value| uuid::Uuid::parse_str(value).ok())
        .ok_or_else(|| io::Error::other("usage report requires a UUID event_id"))?;
    std::fs::create_dir_all(dir)?;
    let path = dir.join(format!("{event_id}.json"));
    // A unique staging file avoids cross-process writers sharing a temporary
    // inode. Only fully synced files become visible to the delivery worker.
    let temporary = dir.join(format!("{}.pending", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary)?;
        file.write_all(&serde_json::to_vec(payload)?)?;
        file.sync_all()?;
        std::fs::rename(&temporary, &path)?;
        sync_directory(dir)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(temporary);
    }
    result
}

fn sync_directory(dir: &Path) -> io::Result<()> {
    #[cfg(unix)]
    File::open(dir)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = dir;
    Ok(())
}

fn drain(dir: &Path, mut deliver: impl FnMut(Value) -> bool) -> io::Result<()> {
    std::fs::create_dir_all(dir)?;
    // Separate from publishing: slow HTTP never holds up a newly recorded event.
    // OS locks release on process death, so another process can replay safely.
    let _lease = concurrency::lock_path(&dir.join("delivery.lock"))?;
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("json") {
            continue;
        }
        // A damaged file must not starve every other valid usage report.
        let payload: Value = match std::fs::read(&path)
            .map_err(io::Error::other)
            .and_then(|bytes| serde_json::from_slice(&bytes).map_err(io::Error::other))
        {
            Ok(payload) => payload,
            Err(error) => {
                logging::warn(&format!("unreadable usage telemetry outbox event: {error}"));
                continue;
            }
        };
        if payload["event"] != "usage_report" {
            continue;
        }
        if deliver(payload) {
            std::fs::remove_file(path)?;
            sync_directory(dir)?;
        }
    }
    Ok(())
}

fn clear(dir: &Path) -> io::Result<()> {
    if !dir.exists() {
        return Ok(());
    }
    let _lease = concurrency::lock_path(&dir.join("delivery.lock"))?;
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if matches!(
            path.extension().and_then(|value| value.to_str()),
            Some("json" | "pending")
        ) {
            std::fs::remove_file(path)?;
        }
    }
    sync_directory(dir)
}

#[cfg(not(test))]
pub(super) fn clear_pending() {
    if let Ok(dir) = directory()
        && let Err(error) = clear(&dir)
    {
        logging::warn(&format!(
            "could not remove opted-out usage telemetry backlog: {error}"
        ));
    }
}

#[cfg(not(test))]
pub(super) fn wake() {
    static WORKER: OnceLock<Option<SyncSender<()>>> = OnceLock::new();
    let sender = WORKER.get_or_init(|| {
        let (sender, receiver) = sync_channel(1);
        std::thread::Builder::new()
            .name("jcode-usage-outbox".into())
            .spawn(move || {
                loop {
                    if !is_enabled() {
                        // Opt-out intentionally deletes unsent telemetry rather
                        // than retaining it for a future opt-in.
                        clear_pending();
                    }
                    if is_enabled()
                        && let Ok(dir) = directory()
                        && let Err(error) = drain(&dir, |payload| {
                            is_enabled() && post_payload_with_retry(payload, ASYNC_SEND_TIMEOUT)
                        })
                    {
                        logging::warn(&format!("usage telemetry outbox replay failed: {error}"));
                    }
                    // A bounded notification channel cannot lose events: events are
                    // already on disk, and a periodic scan also covers process restarts.
                    match receiver.recv_timeout(Duration::from_secs(30)) {
                        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                        _ => {}
                    }
                }
            })
            .ok()?;
        Some(sender)
    });
    if let Some(sender) = sender {
        let _ = sender.try_send(());
    }
}

#[cfg(not(test))]
pub(super) fn has_pending() -> bool {
    is_enabled()
        && directory()
            .ok()
            .and_then(|dir| std::fs::read_dir(dir).ok())
            .is_some_and(|mut entries| {
                entries.any(|entry| {
                    entry.ok().is_some_and(|entry| {
                        entry.path().extension().and_then(|value| value.to_str()) == Some("json")
                    })
                })
            })
}

#[cfg(not(test))]
pub(super) fn enqueue(payload: Value) -> bool {
    match directory().and_then(|dir| persist(&dir, &payload)) {
        Ok(()) => {
            wake();
            true
        }
        Err(error) => {
            // Do not silently downgrade to a volatile queue and pretend the
            // spend signal was safely recorded.
            logging::warn(&format!("could not persist usage telemetry: {error}"));
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn payload() -> Value {
        serde_json::json!({"event": "usage_report", "event_id": new_event_id(),
            "id": "install", "input_tokens": 10, "total_tokens": 10})
    }

    #[test]
    fn retries_keep_identical_payload_until_acknowledged() {
        let dir = tempfile::tempdir().unwrap();
        let event = payload();
        persist(dir.path(), &event).unwrap();
        let mut attempts = Vec::new();
        drain(dir.path(), |value| {
            attempts.push(value);
            false
        })
        .unwrap();
        // Reconstruct from disk, just as a new process does after a crash.
        drain(dir.path(), |value| {
            attempts.push(value);
            true
        })
        .unwrap();
        drain(dir.path(), |_| panic!("acknowledged event replayed")).unwrap();
        assert_eq!(attempts, vec![event.clone(), event]);
    }

    #[test]
    fn independent_events_survive_and_staging_files_are_not_delivered() {
        let dir = tempfile::tempdir().unwrap();
        let first = payload();
        let second = payload();
        persist(dir.path(), &first).unwrap();
        persist(dir.path(), &second).unwrap();
        std::fs::write(dir.path().join("interrupted.pending"), b"{").unwrap();
        let mut delivered = Vec::new();
        drain(dir.path(), |value| {
            delivered.push(value);
            true
        })
        .unwrap();
        assert_eq!(delivered.len(), 2);
        assert!(delivered.contains(&first));
        assert!(delivered.contains(&second));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            persist(dir.path(), &first).unwrap();
            let path = dir
                .path()
                .join(format!("{}.json", first["event_id"].as_str().unwrap()));
            assert_eq!(
                std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn damaged_report_does_not_starve_valid_reports() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("corrupt.json"), b"{").unwrap();
        let event = payload();
        persist(dir.path(), &event).unwrap();
        let mut delivered = Vec::new();
        drain(dir.path(), |value| {
            delivered.push(value);
            true
        })
        .unwrap();
        assert_eq!(delivered, vec![event]);
        assert!(dir.path().join("corrupt.json").exists());
    }

    #[test]
    fn simultaneous_drainers_do_not_deliver_one_event_twice() {
        let dir = tempfile::tempdir().unwrap();
        persist(dir.path(), &payload()).unwrap();
        let attempts = std::sync::atomic::AtomicUsize::new(0);
        std::thread::scope(|scope| {
            for _ in 0..2 {
                scope.spawn(|| {
                    drain(dir.path(), |_| {
                        attempts.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        std::thread::sleep(Duration::from_millis(20));
                        true
                    })
                    .unwrap();
                });
            }
        });
        assert_eq!(attempts.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[test]
    fn opt_out_deletion_removes_published_and_staged_reports() {
        let dir = tempfile::tempdir().unwrap();
        persist(dir.path(), &payload()).unwrap();
        std::fs::write(dir.path().join("interrupted.pending"), b"{").unwrap();
        clear(dir.path()).unwrap();
        drain(dir.path(), |_| panic!("opted-out report replayed")).unwrap();
        assert!(!dir.path().join("interrupted.pending").exists());
    }

    #[test]
    fn http_receipt_must_be_durable_and_match_both_ids() {
        use std::io::Read;
        fn response(status: &str, body: &str) -> reqwest::blocking::Response {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            let wire = format!(
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let server = std::thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = [0; 4096];
                let _ = stream.read(&mut request).unwrap();
                stream.write_all(wire.as_bytes()).unwrap();
            });
            let response = reqwest::blocking::Client::builder()
                .no_proxy()
                .build()
                .unwrap()
                .get(format!("http://{address}/"))
                .timeout(Duration::from_secs(2))
                .send()
                .unwrap();
            server.join().unwrap();
            response
        }
        let event = payload();
        let receipt =
            serde_json::json!({"durable":true,"id":event["id"],"event_id":event["event_id"]});
        assert!(acknowledged(
            response("200 OK", &receipt.to_string()),
            &event
        ));
        for body in [
            "{}".to_string(),
            "not-json".to_string(),
            serde_json::json!({"durable":false,"id":event["id"],"event_id":event["event_id"]})
                .to_string(),
            serde_json::json!({"durable":true,"id":"other","event_id":event["event_id"]})
                .to_string(),
            serde_json::json!({"durable":true,"id":event["id"],"event_id":"other"}).to_string(),
        ] {
            assert!(!acknowledged(response("200 OK", &body), &event));
        }
        assert!(!acknowledged(
            response("503 Unavailable", &receipt.to_string()),
            &event
        ));
    }

    #[test]
    fn unsafe_event_id_never_becomes_a_path() {
        let dir = tempfile::tempdir().unwrap();
        let mut event = payload();
        event["event_id"] = Value::String("../escape".into());
        assert!(persist(dir.path(), &event).is_err());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }
}
