//! Bounded, read-only cloud transcript reads. Never runs on the render thread.
use super::*;

const SCRIPT: &str = r#"
case "$1" in ''|*[!a-zA-Z0-9_-]*) exit 1 ;; esac
base="${JCODE_HOME:-$HOME/.jcode}/sessions/$1"
cat -- "$base.json" || exit 1
printf '\000'
if [ -f "$base.journal.jsonl" ]; then cat -- "$base.journal.jsonl" || exit 1; fi
"#;

pub(super) fn load(session_id: &str, host: &str) -> Option<Session> {
    if session_id.is_empty()
        || !session_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        || host.is_empty()
        || host.starts_with('-')
    {
        return None;
    }
    let config = crate::storage::jcode_dir().ok()?.join("cloud/ssh_config");
    let command = crate::tui::cloud_git::script_command(
        Some(host),
        config.is_file().then_some(config.as_path()),
        session_id,
        SCRIPT,
    );
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .ok()?;
    let (status, bytes) = runtime.block_on(crate::tui::cloud_git::bounded_output(
        command,
        Duration::from_secs(15),
        64 * 1024 * 1024,
    ))?;
    if !status.success() {
        return None;
    }
    parse_transcript(&bytes)
}

fn parse_transcript(bytes: &[u8]) -> Option<Session> {
    let split = bytes.iter().position(|b| *b == 0)?;
    let dir = tempfile::tempdir().ok()?;
    let path = dir.path().join("preview.json");
    std::fs::write(&path, &bytes[..split]).ok()?;
    std::fs::write(
        crate::session::session_journal_path_from_snapshot(&path),
        &bytes[split + 1..],
    )
    .ok()?;
    Session::load_from_path(&path).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

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
