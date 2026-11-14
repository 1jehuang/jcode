use super::*;
use serde::ser::SerializeSeq;

#[test]
fn streamed_json_matches_encoding_and_preserves_backup() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.json");
    let old = serde_json::json!({"old": "preserved"});
    write_json(&path, &old).unwrap();
    let value = serde_json::json!({
        "text": "λ 🧪\n\"\\".repeat(128 * 1024),
        "nested": [null, true, {"number": 42}],
    });
    write_json_fast(&path, &value).unwrap();
    assert_eq!(
        std::fs::read(&path).unwrap(),
        serde_json::to_vec(&value).unwrap()
    );
    assert_eq!(
        std::fs::read(path.with_extension("bak")).unwrap(),
        serde_json::to_vec(&old).unwrap()
    );
    assert_eq!(read_json::<serde_json::Value>(&path).unwrap(), value);
}

/// Fail after enough output to flush the file buffer. Neither a partial file
/// nor an updated backup may become visible when serialization fails late.
struct FailingDocument;

impl Serialize for FailingDocument {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        let mut seq = serializer.serialize_seq(None)?;
        seq.serialize_element(&"partial payload".repeat(2048))?;
        Err(serde::ser::Error::custom(
            "deliberate serialization failure",
        ))
    }
}

#[test]
fn failed_streamed_json_preserves_primary_backup_and_cleans_temp() {
    for durable in [false, true] {
        for secret in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("state.json");
            write_json(&path, &"backup").unwrap();
            write_json(&path, &"primary").unwrap();
            let primary = std::fs::read(&path).unwrap();
            let backup = std::fs::read(path.with_extension("bak")).unwrap();
            let error = write_json_inner(&path, &FailingDocument, durable, secret).unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains("deliberate serialization failure")
            );
            assert_eq!(std::fs::read(&path).unwrap(), primary);
            assert_eq!(std::fs::read(path.with_extension("bak")).unwrap(), backup);
            assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 2);
        }
    }
}

#[test]
fn json_serialization_streams_before_document_is_complete() {
    struct StreamingDocument<'a>(&'a Path);
    impl Serialize for StreamingDocument<'_> {
        fn serialize<S: serde::Serializer>(
            &self,
            serializer: S,
        ) -> std::result::Result<S::Ok, S::Error> {
            let mut seq = serializer.serialize_seq(Some(2))?;
            seq.serialize_element(&"streamed payload".repeat(4096))?;
            // A complete in-memory serde_json::to_vec would leave no file here.
            // This output must already have reached the unpublished temp file.
            let entries: Vec<_> = std::fs::read_dir(self.0).unwrap().collect();
            assert_eq!(entries.len(), 1);
            let entry = entries[0].as_ref().unwrap();
            assert!(entry.file_name().to_string_lossy().contains(".tmp."));
            assert!(entry.metadata().unwrap().len() > 8192);
            seq.serialize_element(&"final element")?;
            seq.end()
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.json");
    write_json_fast(&path, &StreamingDocument(dir.path())).unwrap();
    let value: Vec<String> = read_json(&path).unwrap();
    assert_eq!(value[0], "streamed payload".repeat(4096));
    assert_eq!(value[1], "final element");
}

#[test]
fn failed_streamed_json_does_not_publish_new_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.json");
    assert!(write_json_fast(&path, &FailingDocument).is_err());
    assert!(!path.exists());
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[cfg(unix)]
#[test]
fn streamed_secret_json_keeps_primary_and_backup_owner_only() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("secrets/state.json");
    write_json_secret(&path, &"old secret").unwrap();
    write_json_secret(&path, &"new secret").unwrap();
    for file in [&path, &path.with_extension("bak")] {
        assert_eq!(
            std::fs::metadata(file).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    assert_eq!(
        std::fs::metadata(path.parent().unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
}

#[test]
fn raw_byte_writes_still_preserve_exact_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("raw");
    let bytes: Vec<u8> = (0..=255).cycle().take(32768).collect();
    write_bytes(&path, &bytes).unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
}
