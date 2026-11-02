//! Bounded, read-only cloud transcript reads. Never runs on the render thread.
use super::*;

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

pub(super) fn load(
    session_id: &str,
    host: &str,
) -> Option<(Session, Option<crate::session_telemetry::SessionTelemetry>)> {
    if session_id.is_empty()
        || !session_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        || host.is_empty()
        || host.starts_with('-')
    {
        return None;
    }
    let config = crate::storage::jcode_dir()
        .map_or_else(
            |error| {
                crate::logging::debug(&format!("Cloud preview unavailable: {error}"));
                None
            },
            Some,
        )?
        .join("cloud/ssh_config");
    let command = crate::tui::cloud_git::script_command(
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
        64 * 1024 * 1024,
    ))?;
    if !status.success() {
        return None;
    }
    parse_preview(&bytes)
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
    let telemetry = second.and_then(|split| serde_json::from_slice(&bytes[split + 1..]).ok());
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
