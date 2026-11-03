//! Bounded, read-only cloud transcript reads. Never runs on the render thread.
use super::*;

mod cache;

type CloudSnapshot = (Session, Option<crate::session_telemetry::SessionTelemetry>);

const SCRIPT: &str = r#"
case "$1" in ''|*[!a-zA-Z0-9_-]*) exit 1 ;; esac
base="${JCODE_HOME:-$HOME/.jcode}/sessions/$1"
cat -- "$base.json" || exit 1
printf '\000'
if [ -f "$base.journal.jsonl" ]; then cat -- "$base.journal.jsonl" || exit 1; fi
printf '\000'
state="${JCODE_HOME:-$HOME/.jcode}/state"
if [ -n "$JCODE_RUNTIME_DIR" ]; then state="$JCODE_RUNTIME_DIR/durable-state"; fi
telemetry="$state/session-telemetry/$1.json"
if [ -f "$telemetry" ]; then cat -- "$telemetry" || :; fi
"#;

pub(super) fn load(session_id: &str, host: &str) -> Option<CloudSnapshot> {
    if session_id.is_empty()
        || !session_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        || host.is_empty()
        || host.starts_with('-')
    {
        return None;
    }
    let epoch = crate::storage::read_session_lease(session_id)
        .filter(|lease| lease.away_host.as_deref() == Some(host))
        .map_or(0, |lease| lease.epoch);
    let path = cache::path(session_id, host, epoch);
    let id = session_id.to_owned();
    let host = host.to_owned();
    load_cached_or_fetch(session_id, path, move || fetch_bytes(&id, &host))
}

/// A cloud cache is a preview replica, never a canonical local session. Even a
/// stale replica can be displayed offline while a single background reader
/// refreshes it. No SSH or workspace probe delays a cache hit.
fn load_cached_or_fetch(
    session_id: &str,
    path: Option<std::path::PathBuf>,
    fetch: impl FnOnce() -> Option<Vec<u8>> + Send + 'static,
) -> Option<CloudSnapshot> {
    let started = std::time::Instant::now();
    if let Some(path) = path.as_deref()
        && let Some(bytes) = cache::read(path)
        && let Some(snapshot) = parse_for_session(&bytes, session_id)
    {
        crate::logging::info(&format!(
            "CLOUD_PREVIEW cache=hit session={session_id} bytes={} elapsed_ms={}",
            bytes.len(),
            started.elapsed().as_millis(),
        ));
        if !cache::fresh(path)
            && let Some(guard) = cache::begin_refresh(path)
        {
            let path = path.to_owned();
            let id = session_id.to_owned();
            let refresh = std::thread::Builder::new()
                .name("jcode-cloud-preview-refresh".into())
                .spawn(move || {
                    let _guard = guard;
                    fetch_and_cache(&id, Some(&path), fetch);
                });
            preview_result(refresh, "cloud refresh worker");
        }
        return Some(snapshot);
    }
    fetch_and_cache(session_id, path.as_deref(), fetch)
}

fn fetch_and_cache(
    session_id: &str,
    path: Option<&std::path::Path>,
    fetch: impl FnOnce() -> Option<Vec<u8>>,
) -> Option<CloudSnapshot> {
    let started = std::time::Instant::now();
    let bytes = fetch()?;
    let fetched = started.elapsed();
    // Validate the full snapshot + journal before replacing the last good copy.
    let snapshot = parse_for_session(&bytes, session_id)?;
    if let Some(path) = path
        && let Err(error) = cache::write(path, &bytes)
    {
        crate::logging::debug(&format!("Cloud preview cache write unavailable: {error}"));
    }
    crate::logging::info(&format!(
        "CLOUD_PREVIEW cache=refresh session={session_id} bytes={} fetch_ms={} total_ms={}",
        bytes.len(),
        fetched.as_millis(),
        started.elapsed().as_millis(),
    ));
    Some(snapshot)
}

fn parse_for_session(bytes: &[u8], session_id: &str) -> Option<CloudSnapshot> {
    let snapshot = parse_preview(bytes)?;
    (snapshot.0.id == session_id).then_some(snapshot)
}

fn fetch_bytes(session_id: &str, host: &str) -> Option<Vec<u8>> {
    let config = crate::storage::jcode_dir()
        .map_or_else(
            |error| {
                crate::logging::debug(&format!("Cloud preview unavailable: {error}"));
                None
            },
            Some,
        )?
        .join("cloud/ssh_config");
    let command = crate::tui::cloud_git::compressed_script_command(
        Some(host),
        config.is_file().then_some(config.as_path()),
        session_id,
        SCRIPT,
    );
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_or_else(
            |error| {
                crate::logging::debug(&format!("Cloud preview unavailable: {error}"));
                None
            },
            Some,
        )?;
    let (status, bytes) = runtime.block_on(crate::tui::cloud_git::bounded_output(
        command,
        Duration::from_secs(15),
        cache::MAX_BYTES,
    ))?;
    if !status.success() {
        return None;
    }
    Some(bytes)
}

fn parse_preview(
    bytes: &[u8],
) -> Option<(Session, Option<crate::session_telemetry::SessionTelemetry>)> {
    let first = bytes.iter().position(|b| *b == 0)?;
    // Old hosts and missing/malformed optional telemetry still yield a usable
    // transcript. Neither case is permission to consult the local sidecar.
    let second = bytes[first + 1..]
        .iter()
        .position(|b| *b == 0)
        .map(|offset| first + 1 + offset);
    let session = parse_transcript(&bytes[..second.unwrap_or(bytes.len())])?;
    let telemetry = second.and_then(|split| {
        let bytes = &bytes[split + 1..];
        if bytes.is_empty() {
            None
        } else {
            preview_result(serde_json::from_slice(bytes), "remote telemetry decoding")
        }
    });
    Some((session, telemetry))
}

fn parse_transcript(bytes: &[u8]) -> Option<Session> {
    let split = bytes.iter().position(|b| *b == 0)?;
    let dir = tempfile::tempdir().map_or_else(
        |error| {
            crate::logging::debug(&format!("Cloud preview unavailable: {error}"));
            None
        },
        Some,
    )?;
    let path = dir.path().join("preview.json");
    std::fs::write(&path, &bytes[..split]).map_or_else(
        |error| {
            crate::logging::debug(&format!("Cloud preview unavailable: {error}"));
            None
        },
        Some,
    )?;
    std::fs::write(
        crate::session::session_journal_path_from_snapshot(&path),
        &bytes[split + 1..],
    )
    .map_or_else(
        |error| {
            crate::logging::debug(&format!("Cloud preview unavailable: {error}"));
            None
        },
        Some,
    )?;
    Session::load_from_path(&path).map_or_else(
        |error| {
            crate::logging::debug(&format!("Cloud preview unavailable: {error}"));
            None
        },
        Some,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wire(id: &str, title: &str) -> Vec<u8> {
        let session = Session::create_with_id(id.into(), None, Some(title.into()));
        let mut bytes = serde_json::to_vec(&session).unwrap();
        bytes.extend_from_slice(b"\0\0");
        bytes
    }

    fn make_stale(path: &std::path::Path) {
        std::fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_times(
                std::fs::FileTimes::new()
                    .set_modified(std::time::SystemTime::now() - Duration::from_secs(60)),
            )
            .unwrap();
    }

    #[test]
    fn fresh_cloud_cache_never_waits_for_or_starts_network_io() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("cloud.preview");
        cache::write(&path, &wire("cached", "Cloud copy")).unwrap();
        let (session, _) = load_cached_or_fetch("cached", Some(path), || {
            panic!("fresh cloud preview must be disk-only")
        })
        .unwrap();
        assert_eq!(session.title.as_deref(), Some("Cloud copy"));
    }

    #[test]
    fn stale_cloud_cache_is_immediate_and_single_flight_refreshes_in_background() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("cloud.preview");
        let old = wire("cached", "Old cloud copy");
        let new = wire("cached", "New cloud copy");
        cache::write(&path, &old).unwrap();
        make_stale(&path);
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let (session, _) = load_cached_or_fetch("cached", Some(path.clone()), move || {
            started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            Some(new)
        })
        .unwrap();
        assert_eq!(session.title.as_deref(), Some("Old cloud copy"));
        started_rx.recv_timeout(Duration::from_secs(3)).unwrap();
        let (session, _) = load_cached_or_fetch("cached", Some(path.clone()), || {
            panic!("must not duplicate a pending refresh")
        })
        .unwrap();
        assert_eq!(session.title.as_deref(), Some("Old cloud copy"));
        release_tx.send(()).unwrap();
        let started = std::time::Instant::now();
        while !cache::fresh(&path) {
            assert!(started.elapsed() < Duration::from_secs(3));
            std::thread::sleep(Duration::from_millis(5));
        }
        let (session, _) = load_cached_or_fetch("cached", Some(path), || {
            panic!("refreshed cloud preview must be disk-only")
        })
        .unwrap();
        assert_eq!(session.title.as_deref(), Some("New cloud copy"));
    }

    #[test]
    fn failed_cloud_refresh_preserves_last_good_transcript() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("cloud.preview");
        let old = wire("cached", "Last good cloud copy");
        cache::write(&path, &old).unwrap();
        // Exercise refresh validation directly, including network and identity failures.
        assert!(fetch_and_cache("cached", Some(&path), || None).is_none());
        assert!(fetch_and_cache("cached", Some(&path), || Some(b"broken".to_vec())).is_none());
        assert!(
            fetch_and_cache("cached", Some(&path), || Some(wire(
                "other",
                "Wrong session"
            )))
            .is_none()
        );
        assert_eq!(cache::read(&path).unwrap(), old);
    }

    #[test]
    fn corrupt_cloud_cache_is_repaired_and_cold_fetch_is_persisted() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("cloud.preview");
        cache::write(&path, b"broken").unwrap();
        let expected = wire("cached", "Repaired cloud copy");
        let copy = expected.clone();
        let (session, _) =
            load_cached_or_fetch("cached", Some(path.clone()), move || Some(copy)).unwrap();
        assert_eq!(session.title.as_deref(), Some("Repaired cloud copy"));
        assert_eq!(cache::read(&path).unwrap(), expected);
    }

    /// Needs the local SSH identity and an explicitly selected real cloud session.
    /// Run locally with --ignored --nocapture --test-threads=1, never on hosted CI.
    #[test]
    #[ignore = "requires explicit live cloud host/session and local SSH credentials"]
    fn live_cloud_preview_benchmark() {
        let id = std::env::var("JCODE_CLOUD_PREVIEW_BENCH_SESSION")
            .expect("set JCODE_CLOUD_PREVIEW_BENCH_SESSION");
        let host = std::env::var("JCODE_CLOUD_PREVIEW_BENCH_HOST")
            .expect("set JCODE_CLOUD_PREVIEW_BENCH_HOST");
        let template = load_sessions()
            .expect("load picker rows")
            .into_iter()
            .find(|session| session.id == id)
            .expect("cloud session row");
        for iteration in 0..3 {
            let started = std::time::Instant::now();
            let (session, snapshot) = load(&id, &host).expect("load real cloud transcript");
            let fetched = started.elapsed();
            let render = std::time::Instant::now();
            let (messages, hidden) = build_messages_preview_with_offset(&session);
            let projected = render.elapsed();
            let preview_count = messages.len();
            let first_frame = std::time::Instant::now();
            let mut picker = SessionPicker::new(vec![template.clone()]);
            picker.apply_session_preview(&id, messages);
            picker.preview_extras.insert(
                id.clone(),
                PreviewExtras {
                    hidden_prompts: hidden,
                    info_data: Some(preview_chrome::persisted_info_from_snapshot(
                        &session, snapshot,
                    )),
                    loaded_at: Some(std::time::Instant::now()),
                    ..Default::default()
                },
            );
            let mut terminal =
                ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 40)).unwrap();
            terminal
                .draw(|frame| picker.render_preview(frame, Rect::new(0, 0, 120, 40)))
                .unwrap();
            let frame_elapsed = first_frame.elapsed();
            let text: String = terminal
                .backend()
                .buffer()
                .content
                .iter()
                .map(|cell| cell.symbol())
                .collect();
            assert!(
                text.contains('☁'),
                "cloud marker missing from cached preview"
            );
            assert!(!text.contains("Loading preview"));
            eprintln!(
                "cloud-preview-bench iteration={iteration} load_ms={:.2} messages_ms={:.2} frame_ms={:.2} total_ms={:.2} stored_messages={} preview_messages={} hidden_prompts={hidden}",
                fetched.as_secs_f64() * 1000.0,
                projected.as_secs_f64() * 1000.0,
                frame_elapsed.as_secs_f64() * 1000.0,
                started.elapsed().as_secs_f64() * 1000.0,
                session.messages.len(),
                preview_count,
            );
            assert!(preview_count > 0);
        }
    }

    #[test]
    fn cloud_preview_hydrates_only_its_remote_telemetry() {
        let session = Session::create_with_id("remote_telemetry_preview".into(), None, None);
        let mut bytes = serde_json::to_vec(&session).unwrap();
        bytes.extend_from_slice(b"\0\0");
        let telemetry = serde_json::json!({
            "session_id": session.id,
            "model": session.model,
            "provider": session.provider_key,
            "measured_at": chrono::Utc::now(),
            "context_tokens": 12_345,
            "context_limit": 100_000,
            "compaction": null
        });
        bytes.extend_from_slice(&serde_json::to_vec(&telemetry).unwrap());
        let (loaded, snapshot) = parse_preview(&bytes).unwrap();
        let data = preview_chrome::persisted_info_from_snapshot(&loaded, snapshot);
        assert_eq!(data.observed_context_tokens, Some(12_345));
        assert_eq!(data.context_limit, Some(100_000));
        assert!(data.historical_context);

        let end = bytes.iter().rposition(|b| *b == 0).unwrap() + 1;
        bytes.truncate(end);
        bytes.extend_from_slice(b"malformed optional telemetry");
        let (loaded, snapshot) = parse_preview(&bytes).unwrap();
        assert!(snapshot.is_none());
        let data = preview_chrome::persisted_info_from_snapshot(&loaded, snapshot);
        assert!(data.observed_context_tokens.is_none());
    }

    #[test]
    fn invalid_targets_are_rejected_before_remote_io() {
        for (session, host) in [
            ("", "host"),
            ("../private", "host"),
            ("session", ""),
            ("session", "-oProxyCommand=bad"),
        ] {
            assert!(load(session, host).is_none());
        }
    }

    #[test]
    fn cloud_snapshot_is_loaded_without_persisting_locally() {
        let session = Session::create_with_id("session_cloudpreview_123".into(), None, None);
        let mut bytes = serde_json::to_vec(&session).unwrap();
        bytes.push(0);
        let mut meta = serde_json::to_value(&session).unwrap();
        meta["title"] = serde_json::json!("Latest cloud title");
        let entry = serde_json::json!({
            "meta": meta,
            "append_messages": [{
                "id": "cloud-message", "role": "user",
                "content": [{"type": "text", "text": "Latest cloud prompt"}]
            }]
        });
        bytes.extend_from_slice(&serde_json::to_vec(&entry).unwrap());
        bytes.push(b'\n');
        let loaded = parse_transcript(&bytes).unwrap();
        assert_eq!(loaded.id, session.id);
        assert_eq!(loaded.title.as_deref(), Some("Latest cloud title"));
        let preview = build_messages_preview(&loaded);
        assert!(
            preview
                .iter()
                .any(|message| message.content.contains("Latest cloud prompt"))
        );
        assert!(parse_transcript(b"invalid\0").is_none());
        assert!(parse_transcript(b"{}").is_none());
    }
}
