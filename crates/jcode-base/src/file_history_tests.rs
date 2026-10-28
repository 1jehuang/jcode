use super::*;
use crate::message::{ContentBlock, Role};
use jcode_session_types::RewindMode;

#[path = "file_history_assistant_tests.rs"]
mod assistant;

struct TestHome {
    _dir: tempfile::TempDir,
    old: Option<std::ffi::OsString>,
}

impl Drop for TestHome {
    fn drop(&mut self) {
        if let Some(old) = self.old.take() {
            crate::env::set_var("JCODE_HOME", old);
        } else {
            crate::env::remove_var("JCODE_HOME");
        }
    }
}

fn fixture() -> (TestHome, tempfile::TempDir) {
    let home = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let old = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", home.path());
    (TestHome { _dir: home, old }, work)
}

#[test]
fn raw_bytes_create_edit_delete_resume_and_repeated_restore() {
    let _env = crate::storage::lock_test_env();
    let (_home, work) = fixture();
    let path = work.path().join("binary");
    let first = checkpoint("s", 2, "first").unwrap();
    mutate("s", &path, Some(&[0, 255, 3])).unwrap();
    let second = checkpoint("s", 5, "second").unwrap();
    mutate("s", &path, Some(b"changed")).unwrap();
    mutate("s", &path, None).unwrap();
    assert_eq!(checkpoints("s").unwrap().len(), 2);
    assert_eq!(restore("s", &second.id).unwrap(), 1);
    assert_eq!(fs::read(&path).unwrap(), [0, 255, 3]);
    assert_eq!(restore("s", &second.id).unwrap(), 0);
    restore("s", &first.id).unwrap();
    assert!(!path.exists());
    restore("s", &second.id).unwrap();
    assert_eq!(fs::read(&path).unwrap(), [0, 255, 3]);
}

#[test]
fn legacy_history_without_metadata_remains_restorable() {
    let _env = crate::storage::lock_test_env();
    let (_home, work) = fixture();
    let path = fs::canonicalize(work.path()).unwrap().join("legacy");
    fs::write(&path, b"before").unwrap();
    let cp = checkpoint("legacy", 0, "edit").unwrap();
    mutate("legacy", &path, Some(b"after")).unwrap();
    let dir = directory("legacy").unwrap();
    let history_path = dir.join("history.json");
    let mut json: serde_json::Value =
        serde_json::from_slice(&fs::read(&history_path).unwrap()).unwrap();
    for state in json["expected"].as_object_mut().unwrap().values_mut() {
        state.as_object_mut().unwrap().remove("metadata");
    }
    for checkpoint in json["checkpoints"].as_array_mut().unwrap() {
        for state in checkpoint["before"].as_object_mut().unwrap().values_mut() {
            state.as_object_mut().unwrap().remove("metadata");
        }
    }
    crate::storage::write_json_secret(&history_path, &json).unwrap();
    let legacy = load(&dir).unwrap();
    let actual = state(&dir, &path).unwrap();
    let expected = &legacy.expected[&path];
    assert!(expected.matches(&actual));
    #[cfg(unix)]
    {
        assert_ne!(expected, &actual, "ordinary equality stays strict");
        assert!(!actual.matches(expected), "matching is directional");
    }
    // Metadata added since the old sidecar was written must survive rewind.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    write_xattrs(
        &path,
        &BTreeMap::from([(
            if cfg!(target_os = "macos") {
                "com.jcode.legacy"
            } else {
                "user.jcode-legacy"
            }
            .into(),
            b"live metadata".to_vec(),
        )]),
    )
    .unwrap();
    let live = state(&dir, &path).unwrap().metadata;
    let info = checkpoints("legacy").unwrap().remove(0);
    assert!(info.can_restore_files);
    assert_eq!(info.changed_file_count, 1);
    assert_eq!(restore("legacy", &cp.id).unwrap(), 1);
    assert_eq!(fs::read(&path).unwrap(), b"before");
    assert_eq!(state(&dir, &path).unwrap().metadata, live);
    assert_eq!(load(&dir).unwrap().expected[&path].metadata, live);
    assert_eq!(restore("legacy", &cp.id).unwrap(), 0);
    // Continuing tracked edits must not mark a legacy checkpoint as conflicted.
    mutate("legacy", &path, Some(b"next edit")).unwrap();
    assert!(checkpoints("legacy").unwrap()[0].can_restore_files);
    assert_eq!(restore("legacy", &cp.id).unwrap(), 1);
}

#[cfg(target_os = "macos")]
fn set_macos_test_xattr(path: &Path, name: &str, value: &[u8]) {
    let path = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
    let name = std::ffi::CString::new(name).unwrap();
    assert_eq!(
        unsafe {
            libc::setxattr(
                path.as_ptr(),
                name.as_ptr(),
                value.as_ptr().cast(),
                value.len(),
                0,
                libc::XATTR_NOFOLLOW,
            )
        },
        0,
        "{}",
        std::io::Error::last_os_error()
    );
}

#[cfg(target_os = "macos")]
fn get_macos_test_xattr(path: &Path, name: &str) -> Vec<u8> {
    let path = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
    let name = std::ffi::CString::new(name).unwrap();
    let mut value = vec![0u8; 256];
    let size = unsafe {
        libc::getxattr(
            path.as_ptr(),
            name.as_ptr(),
            value.as_mut_ptr().cast(),
            value.len(),
            0,
            libc::XATTR_NOFOLLOW,
        )
    };
    assert!(size >= 0, "{}", std::io::Error::last_os_error());
    value.truncate(size as usize);
    value
}

#[cfg(target_os = "macos")]
#[test]
fn macos_generated_provenance_does_not_block_new_file_rewind() {
    let _env = crate::storage::lock_test_env();
    let (_home, work) = fixture();
    let path = fs::canonicalize(work.path()).unwrap().join("new-metadata");
    let first = checkpoint("macos-new-metadata", 0, "create").unwrap();
    mutate("macos-new-metadata", &path, Some(b"created")).unwrap();
    let dir = directory("macos-new-metadata").unwrap();
    let history = load(&dir).unwrap();
    let actual = state(&dir, &path).unwrap();
    assert!(history.expected[&path].matches(&actual));
    assert!(
        !actual
            .metadata
            .unwrap()
            .xattrs
            .contains_key("com.apple.provenance")
    );
    assert!(history.checkpoints[0].blocked.is_empty());
    let second = checkpoint("macos-new-metadata", 1, "edit").unwrap();
    mutate("macos-new-metadata", &path, Some(b"edited")).unwrap();
    mutate("macos-new-metadata", &path, None).unwrap();
    assert_eq!(restore("macos-new-metadata", &second.id).unwrap(), 1);
    assert_eq!(restore("macos-new-metadata", &second.id).unwrap(), 0);
    assert_eq!(restore("macos-new-metadata", &first.id).unwrap(), 1);
    assert!(!path.exists());
}

#[cfg(target_os = "macos")]
#[test]
fn macos_edit_and_rewind_preserve_xattrs_and_detect_external_conflicts() {
    use std::os::unix::fs::MetadataExt;
    let _env = crate::storage::lock_test_env();
    let (_home, work) = fixture();
    let path = fs::canonicalize(work.path())
        .unwrap()
        .join("macos-metadata");
    fs::write(&path, b"before").unwrap();
    let original = fs::metadata(&path).unwrap();
    // macOS names need no Linux user. prefix; also cover binary and empty values.
    let attrs = [
        ("com.jcode.binary", &b"\0\xffpreserved"[..]),
        ("com.jcode.empty", &b""[..]),
    ];
    for (name, value) in attrs {
        set_macos_test_xattr(&path, name, value);
    }
    let cp = checkpoint("macos-metadata", 0, "edit").unwrap();
    mutate("macos-metadata", &path, Some(b"after")).unwrap();
    for (name, value) in attrs {
        assert_eq!(get_macos_test_xattr(&path, name), value);
    }
    assert_eq!(restore("macos-metadata", &cp.id).unwrap(), 1);
    assert_eq!(fs::read(&path).unwrap(), b"before");
    for (name, value) in attrs {
        assert_eq!(get_macos_test_xattr(&path, name), value);
    }
    let restored = fs::metadata(&path).unwrap();
    assert_eq!(
        (restored.uid(), restored.gid()),
        (original.uid(), original.gid())
    );
    mutate("macos-metadata", &path, Some(b"tracked edit")).unwrap();
    set_macos_test_xattr(&path, "com.jcode.binary", b"external");
    assert!(!checkpoints("macos-metadata").unwrap()[0].can_restore_files);
    assert!(restore("macos-metadata", &cp.id).is_err());
    assert_eq!(fs::read(&path).unwrap(), b"tracked edit");
    assert_eq!(get_macos_test_xattr(&path, "com.jcode.binary"), b"external");
}

#[cfg(target_os = "linux")]
#[test]
fn tracked_edit_and_rewind_preserve_unprivileged_xattrs_and_group() {
    use std::os::unix::fs::MetadataExt;
    let _env = crate::storage::lock_test_env();
    let (_home, work) = fixture();
    let path = work.path().join("metadata");
    fs::write(&path, b"before").unwrap();
    let group = fs::metadata(&path).unwrap().gid();
    let name = std::ffi::CString::new("user.jcode-test").unwrap();
    let c_path = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
    let value = b"preserved";
    assert_eq!(
        unsafe {
            libc::setxattr(
                c_path.as_ptr(),
                name.as_ptr(),
                value.as_ptr().cast(),
                value.len(),
                0,
            )
        },
        0
    );
    let checkpoint = checkpoint("metadata", 0, "edit").unwrap();
    mutate("metadata", &path, Some(b"after")).unwrap();
    assert_eq!(fs::metadata(&path).unwrap().gid(), group);
    // A fresh process/session read compares user xattrs as part of the state,
    // and restore materializes the before-image metadata from durable history.
    let durable: History = serde_json::from_slice(
        &fs::read(directory("metadata").unwrap().join("history.json")).unwrap(),
    )
    .unwrap();
    assert!(durable.expected[&path].metadata.is_some());
    let mut restored = [0u8; 32];
    let size = unsafe {
        libc::getxattr(
            c_path.as_ptr(),
            name.as_ptr(),
            restored.as_mut_ptr().cast(),
            restored.len(),
        )
    };
    assert_eq!(&restored[..size as usize], value);
    restore("metadata", &checkpoint.id).unwrap();
    assert_eq!(fs::read(&path).unwrap(), b"before");
    assert_eq!(fs::metadata(&path).unwrap().gid(), group);
    let size = unsafe {
        libc::getxattr(
            c_path.as_ptr(),
            name.as_ptr(),
            restored.as_mut_ptr().cast(),
            restored.len(),
        )
    };
    assert_eq!(&restored[..size as usize], value);
}

#[cfg(target_os = "linux")]
#[test]
fn external_user_xattr_change_blocks_rewind_even_when_bytes_are_unchanged() {
    let _env = crate::storage::lock_test_env();
    let (_home, work) = fixture();
    let path = work.path().join("metadata-conflict");
    fs::write(&path, b"same bytes").unwrap();
    let name = std::ffi::CString::new("user.jcode-conflict").unwrap();
    let c_path = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
    let initial = b"initial";
    assert_eq!(
        unsafe {
            libc::setxattr(
                c_path.as_ptr(),
                name.as_ptr(),
                initial.as_ptr().cast(),
                initial.len(),
                0,
            )
        },
        0
    );
    let cp = checkpoint("metadata-conflict", 0, "capture").unwrap();
    mutate("metadata-conflict", &path, Some(b"tracked edit")).unwrap();
    let changed = b"external";
    assert_eq!(
        unsafe {
            libc::setxattr(
                c_path.as_ptr(),
                name.as_ptr(),
                changed.as_ptr().cast(),
                changed.len(),
                0,
            )
        },
        0
    );
    assert!(restore("metadata-conflict", &cp.id).is_err());
    assert_eq!(fs::read(&path).unwrap(), b"tracked edit");
    let mut current = [0u8; 32];
    let size = unsafe {
        libc::getxattr(
            c_path.as_ptr(),
            name.as_ptr(),
            current.as_mut_ptr().cast(),
            current.len(),
        )
    };
    assert_eq!(&current[..size as usize], changed);
}

#[test]
fn external_changes_refuse_all_files_and_between_tool_writes() {
    let _env = crate::storage::lock_test_env();
    let (_home, work) = fixture();
    let a = work.path().join("a");
    let b = work.path().join("b");
    fs::write(&a, b"dirty user baseline").unwrap();
    let cp = checkpoint("s", 0, "go").unwrap();
    mutate("s", &a, Some(b"tool")).unwrap();
    mutate("s", &b, Some(b"tool b")).unwrap();
    fs::write(&a, b"human").unwrap();
    assert!(restore("s", &cp.id).is_err());
    assert_eq!(fs::read(&b).unwrap(), b"tool b");
    mutate("s", &a, Some(b"next tool")).unwrap();
    assert!(restore("s", &cp.id).is_err());
    let fresh = checkpoint("s", 4, "fresh").unwrap();
    mutate("s", &a, Some(b"safe new tool")).unwrap();
    restore("s", &fresh.id).unwrap();
    assert_eq!(fs::read(&a).unwrap(), b"next tool");
}

#[test]
fn max_one_hundred_and_failed_capture_never_mutates() {
    let _env = crate::storage::lock_test_env();
    let (_home, work) = fixture();
    for i in 0..103 {
        checkpoint("s", i, "prompt").unwrap();
    }
    assert_eq!(checkpoints("s").unwrap().len(), MAX_CHECKPOINTS);
    let directory = work.path().join("directory");
    fs::create_dir(&directory).unwrap();
    assert!(mutate("s", &directory, Some(b"bad")).is_err());
    assert!(directory.is_dir());
}

fn user(session: &mut crate::session::Session, prompt: &str) -> RewindCheckpoint {
    let cp = session.create_rewind_checkpoint(prompt).unwrap();
    let input_id = session.add_message(
        crate::message::Role::User,
        vec![crate::message::ContentBlock::Text {
            text: prompt.into(),
            cache_control: None,
        }],
    );
    session.bind_rewind_checkpoint(&cp.id, &input_id).unwrap();
    session.save().unwrap();
    cp
}

#[tokio::test]
async fn session_modes_resume_empty_first_prompt_and_consumed_ids() {
    use crate::session::Session;
    use jcode_session_types::RewindMode::*;
    let _env = crate::storage::lock_test_env();
    let (_home, work) = fixture();
    let path = work.path().join("f");
    let mut session = Session::create(None, None);
    let first = user(&mut session, "first");
    mutate(&session.id, &path, Some(b"first edit")).unwrap();
    let second = user(&mut session, "second");
    mutate(&session.id, &path, Some(b"second edit")).unwrap();
    let count = session.messages.len();
    let mut resumed = Session::load(&session.id).unwrap();
    assert_eq!(resumed.rewind_checkpoints().unwrap().len(), 2);
    let result = resumed
        .rewind_checkpoint(&second.id, FilesOnly)
        .await
        .unwrap();
    assert_eq!(result.restored_files, 1);
    assert_eq!(resumed.messages.len(), count);
    assert_eq!(fs::read(&path).unwrap(), b"first edit");
    assert_eq!(
        resumed
            .rewind_checkpoint(&second.id, FilesOnly)
            .await
            .unwrap()
            .restored_files,
        0
    );
    fs::write(&path, b"human").unwrap();
    assert!(
        resumed
            .rewind_checkpoint(&first.id, ConversationAndFiles)
            .await
            .is_err()
    );
    assert_eq!(resumed.messages.len(), count);
    assert_eq!(fs::read(&path).unwrap(), b"human");
    fs::write(&path, b"first edit").unwrap();
    resumed
        .rewind_checkpoint(&first.id, ConversationAndFiles)
        .await
        .unwrap();
    assert!(resumed.messages.is_empty());
    assert!(!path.exists());
    assert!(resumed.rewind_checkpoints().unwrap().is_empty());
    assert!(
        resumed
            .rewind_checkpoint(&second.id, FilesOnly)
            .await
            .is_err()
    );
    assert!(Session::load(&session.id).unwrap().messages.is_empty());

    let cp = user(&mut resumed, "new branch");
    mutate(&resumed.id, &path, Some(b"keep files")).unwrap();
    resumed.provider_session_id = Some("old provider".into());
    resumed.model_usage_turn_id = Some("old turn".into());
    resumed.compaction = Some(jcode_session_types::StoredCompactionState {
        summary_text: "discarded future".into(),
        openai_encrypted_content: None,
        covers_up_to_turn: 1,
        original_turn_count: 1,
        compacted_count: 1,
    });
    resumed
        .rewind_checkpoint(&cp.id, ConversationOnly)
        .await
        .unwrap();
    assert_eq!(fs::read(path).unwrap(), b"keep files");
    assert!(resumed.compaction.is_none());
    assert!(resumed.provider_session_id.is_none());
    assert!(resumed.model_usage_turn_id.is_none());
}

#[test]
fn rewind_marker_prevents_old_journal_resurrection() {
    use crate::session::Session;
    let _env = crate::storage::lock_test_env();
    let (_home, _work) = fixture();
    let mut session = Session::create(None, None);
    user(&mut session, "one");
    user(&mut session, "two");
    let snapshot = crate::session::session_path(&session.id).unwrap();
    let journal = crate::session::session_journal_path_from_snapshot(&snapshot);
    assert!(journal.exists());
    session.truncate_messages(0);
    session.provider_session_id = None;
    session.compaction = None;
    let pending = snapshot.with_extension("rewind-pending.json");
    crate::storage::write_json_secret(&pending, &session).unwrap();
    crate::storage::write_json(&snapshot, &session).unwrap();
    // Simulates death after snapshot replacement but before journal cutoff.
    assert!(
        Session::load_for_remote_startup(&session.id)
            .unwrap()
            .messages
            .is_empty()
    );
    assert!(Session::load(&session.id).unwrap().messages.is_empty());
    assert!(!journal.exists());
    assert!(!pending.exists());
}

#[tokio::test]
async fn finalize_failure_keeps_committed_memory_disk_and_files_coherent() {
    use crate::session::Session;
    use jcode_session_types::RewindMode::*;
    let _env = crate::storage::lock_test_env();
    let (_home, work) = fixture();
    let path = work.path().join("f");
    let mut session = Session::create(None, None);
    let cp = user(&mut session, "first");
    mutate(&session.id, &path, Some(b"edited")).unwrap();
    FAIL_NEXT_FINALIZE.with(|flag| flag.set(true));
    assert!(
        session
            .rewind_checkpoint(&cp.id, ConversationAndFiles)
            .await
            .is_err()
    );
    assert!(session.messages.is_empty());
    assert!(Session::load(&session.id).unwrap().messages.is_empty());
    assert!(!path.exists());
    assert!(
        session.rewind_checkpoints().unwrap().is_empty(),
        "durable transaction finishes on next read"
    );

    let cp = user(&mut session, "second");
    mutate(&session.id, &path, Some(b"keep files")).unwrap();
    FAIL_NEXT_FINALIZE.with(|flag| flag.set(true));
    assert!(
        session
            .rewind_checkpoint(&cp.id, ConversationOnly)
            .await
            .is_err()
    );
    assert!(session.messages.is_empty());
    assert!(Session::load(&session.id).unwrap().messages.is_empty());
    assert_eq!(fs::read(path).unwrap(), b"keep files");
    assert!(session.rewind_checkpoints().unwrap().is_empty());
}

#[test]
fn interrupted_restore_rolls_back_and_missing_backup_refuses_mutation() {
    let _env = crate::storage::lock_test_env();
    let (_home, work) = fixture();
    let path = fs::canonicalize(work.path()).unwrap().join("f");
    fs::write(&path, b"before").unwrap();
    let cp = checkpoint("s", 0, "go").unwrap();
    mutate("s", &path, Some(b"after")).unwrap();
    let dir = directory("s").unwrap();
    let mut history = load(&dir).unwrap();
    let change = Change {
        path: path.clone(),
        before: history.expected[&path].clone(),
        after: history.checkpoints[0].before[&path].clone(),
    };
    history.restoring = vec![change.clone()];
    save(&dir, &history).unwrap();
    apply(&dir, &change, false).unwrap();
    assert_eq!(fs::read(&path).unwrap(), b"before");
    load(&dir).unwrap();
    assert_eq!(fs::read(&path).unwrap(), b"after");
    fs::remove_file(dir.join("blobs").join(change.after.blob.unwrap())).unwrap();
    assert!(restore("s", &cp.id).is_err());
    assert_eq!(fs::read(path).unwrap(), b"after");
}

#[test]
fn intermediate_blobs_are_collected_and_external_unsafe_paths_do_not_block_prompts() {
    let _env = crate::storage::lock_test_env();
    let (_home, work) = fixture();
    let path = work.path().join("f");
    let cp = checkpoint("s", 0, "go").unwrap();
    for i in 0..12 {
        mutate("s", &path, Some(format!("version {i}").as_bytes())).unwrap();
    }
    assert_eq!(
        fs::read_dir(directory("s").unwrap().join("blobs"))
            .unwrap()
            .count(),
        1
    );
    fs::remove_file(&path).unwrap();
    fs::create_dir(&path).unwrap();
    checkpoint("s", 4, "continue anyway").unwrap();
    assert!(
        !checkpoints("s")
            .unwrap()
            .iter()
            .find(|checkpoint| checkpoint.id == cp.id)
            .unwrap()
            .can_restore_files
    );
}

#[cfg(unix)]
#[test]
fn links_preserve_edit_behavior_but_disable_unsafe_restore() {
    use std::os::unix::fs::symlink;
    let _env = crate::storage::lock_test_env();
    let (_home, work) = fixture();
    let cp = checkpoint("s", 0, "prompt").unwrap();
    let real = work.path().join("real");
    fs::write(&real, b"human").unwrap();
    let link = work.path().join("link");
    symlink(&real, &link).unwrap();
    mutate("s", &link, Some(b"edit through link")).unwrap();
    assert!(!checkpoints("s").unwrap()[0].can_restore_files);
    assert!(restore("s", &cp.id).is_err());
    let parent = work.path().join("parent");
    symlink(work.path(), &parent).unwrap();
    mutate("s", &parent.join("new"), Some(b"parent alias is safe")).unwrap();
    let physical = fs::canonicalize(work.path()).unwrap().join("new");
    let history = load(&directory("s").unwrap()).unwrap();
    assert!(history.expected.contains_key(&physical));
    fs::hard_link(&real, work.path().join("alias")).unwrap();
    mutate("s", &real, Some(b"hardlink edit")).unwrap();
    assert_eq!(fs::read(real).unwrap(), b"hardlink edit");
    assert!(restore("s", &cp.id).is_err());
}

#[tokio::test]
async fn transcript_only_targets_filter_synthetic_rows_and_reject_reused_branch_positions() {
    let _env = crate::storage::lock_test_env();
    let (_home, _work) = fixture();
    let mut session = crate::session::Session::create(None, None);
    let text = |text: &str| ContentBlock::Text {
        text: text.into(),
        cache_control: None,
    };
    session.add_message_with_display_role(
        Role::User,
        vec![text("internal")],
        Some(crate::session::StoredDisplayRole::System),
    );
    session.add_message(
        Role::User,
        vec![text("<system-reminder>hidden</system-reminder>")],
    );
    session.add_message(
        Role::User,
        vec![text("[Scheduled task]\nlegacy automation")],
    );
    session.add_message(Role::User, vec![text("[NOTIFICATION]\nYou received 1 notification(s) from other agents working in this codebase:\n\ninternal alert")]);
    let first_id = session.add_message(Role::User, vec![text("keep exact human text")]);
    session.add_message(
        Role::Assistant,
        vec![ContentBlock::ToolUse {
            id: "tool".into(),
            name: "read".into(),
            input: serde_json::json!({}),
            thought_signature: None,
        }],
    );
    session.add_message(
        Role::User,
        vec![
            ContentBlock::ToolResult {
                tool_use_id: "tool".into(),
                content: "result".into(),
                is_error: None,
            },
            ContentBlock::Image {
                media_type: "image/png".into(),
                data: "dG9vbA==".into(),
            },
            text("tool image label"),
        ],
    );
    let image_id = session.add_message(
        Role::User,
        vec![ContentBlock::Image {
            media_type: "image/png".into(),
            data: "aHVtYW4=".into(),
        }],
    );
    session.save().unwrap();
    let mut session = crate::session::Session::load(&session.id).unwrap();
    let targets = session.rewind_checkpoints().unwrap();
    assert_eq!(targets.len(), 2);
    assert_eq!(targets[0].id, format!("message:{first_id}"));
    assert_eq!(targets[1].id, format!("message:{image_id}"));
    assert!(
        targets
            .iter()
            .all(|target| !target.can_restore_files && target.changed_file_count == 0)
    );
    let count = session.messages.len();
    for mode in [RewindMode::FilesOnly, RewindMode::ConversationAndFiles] {
        assert!(
            session
                .rewind_checkpoint(&targets[1].id, mode)
                .await
                .is_err()
        );
        assert_eq!(session.messages.len(), count);
    }
    let result = session
        .rewind_checkpoint(&targets[1].id, RewindMode::ConversationOnly)
        .await
        .unwrap();
    assert_eq!(result.prompt, "");
    assert_eq!(result.images, vec![("image/png".into(), "aHVtYW4=".into())]);
    assert_eq!(result.removed_messages, 1);
    // A new prompt occupies the old raw index, but cannot inherit its identity.
    session.add_message(Role::User, vec![text("different branch")]);
    assert!(
        session
            .rewind_checkpoint(&targets[1].id, RewindMode::ConversationOnly)
            .await
            .is_err()
    );
    let forged = format!("message:{}", session.messages[1].id);
    assert!(
        session
            .rewind_checkpoint(&forged, RewindMode::ConversationOnly)
            .await
            .is_err()
    );
    let result = session
        .rewind_checkpoint(&targets[0].id, RewindMode::ConversationOnly)
        .await
        .unwrap();
    assert_eq!(result.prompt, "keep exact human text");
    assert!(result.images.is_empty());
    assert_eq!(session.messages.len(), 4);
}

#[tokio::test]
async fn message_targets_keep_snapshot_boundary_and_restore_original_image_order() {
    let _env = crate::storage::lock_test_env();
    let (_home, work) = fixture();
    let path = work.path().join("tracked");
    fs::write(&path, b"original").unwrap();
    let mut session = crate::session::Session::create(None, None);
    let checkpoint = session
        .create_rewind_checkpoint("  exact raw prompt  ")
        .unwrap();
    session.add_message_with_display_role(
        Role::User,
        vec![ContentBlock::Text {
            text: "late hidden context".into(),
            cache_control: None,
        }],
        Some(crate::session::StoredDisplayRole::System),
    );
    let input_id = session.add_message(
        Role::User,
        vec![
            ContentBlock::Image {
                media_type: "image/png".into(),
                data: "b25l".into(),
            },
            ContentBlock::Image {
                media_type: "image/jpeg".into(),
                data: "dHdv".into(),
            },
            ContentBlock::Text {
                text: "  exact raw prompt  ".into(),
                cache_control: None,
            },
        ],
    );
    session
        .bind_rewind_checkpoint(&checkpoint.id, &input_id)
        .unwrap();
    session.save().unwrap();
    mutate(&session.id, &path, Some(b"edited")).unwrap();
    let target = session.rewind_checkpoints().unwrap().remove(0);
    assert_eq!(target.id, format!("message:{input_id}"));
    assert_eq!(
        target.message_index, 0,
        "preserve before-injected-context snapshot boundary"
    );
    let result = session
        .rewind_checkpoint(&checkpoint.id, RewindMode::FilesOnly)
        .await
        .unwrap();
    assert!(result.images.is_empty());
    assert_eq!(session.messages.len(), 2);
    mutate(&session.id, &path, Some(b"edited again")).unwrap();
    let result = session
        .rewind_checkpoint(&target.id, RewindMode::ConversationAndFiles)
        .await
        .unwrap();
    assert_eq!(result.prompt, "  exact raw prompt  ");
    assert_eq!(
        result.images,
        vec![
            ("image/png".into(), "b25l".into()),
            ("image/jpeg".into(), "dHdv".into())
        ]
    );
    assert_eq!(fs::read(path).unwrap(), b"original");
    assert!(session.messages.is_empty());
}

#[tokio::test]
async fn file_snapshot_eviction_keeps_all_transcript_targets_and_stable_ids() {
    let _env = crate::storage::lock_test_env();
    let (_home, _work) = fixture();
    let mut session = crate::session::Session::create(None, None);
    for index in 0..=MAX_CHECKPOINTS {
        let prompt = format!("prompt {index}");
        let checkpoint = session.create_rewind_checkpoint(&prompt).unwrap();
        session.add_message_with_display_role(
            Role::User,
            vec![ContentBlock::Text {
                text: format!("hidden turn context {index}"),
                cache_control: None,
            }],
            Some(crate::session::StoredDisplayRole::System),
        );
        let input_id = session.add_message(
            Role::User,
            vec![ContentBlock::Text {
                text: prompt,
                cache_control: None,
            }],
        );
        session
            .bind_rewind_checkpoint(&checkpoint.id, &input_id)
            .unwrap();
        session.save().unwrap();
    }
    let first_id = format!("message:{}", session.messages[1].id);
    let mut session = crate::session::Session::load(&session.id).unwrap();
    let targets = session.rewind_checkpoints().unwrap();
    assert_eq!(targets.len(), MAX_CHECKPOINTS + 1);
    assert_eq!(targets[0].id, first_id);
    assert_eq!(
        targets[0].message_index, 0,
        "eviction must not move past hidden context"
    );
    assert!(!targets[0].can_restore_files);
    assert!(
        session
            .rewind_checkpoint(&first_id, RewindMode::FilesOnly)
            .await
            .is_err()
    );
    session
        .rewind_checkpoint(&first_id, RewindMode::ConversationOnly)
        .await
        .unwrap();
    assert!(session.messages.is_empty());
    assert!(
        load(&directory(&session.id).unwrap())
            .unwrap()
            .conversation_boundaries
            .is_empty()
    );
}

#[test]
fn legacy_history_without_any_sidecar_lists_more_than_one_hundred_prompts() {
    let _env = crate::storage::lock_test_env();
    let (_home, _work) = fixture();
    let mut session = crate::session::Session::create(None, None);
    for index in 0..125 {
        session.add_message(
            Role::User,
            vec![ContentBlock::Text {
                text: format!("legacy prompt {index}"),
                cache_control: None,
            }],
        );
    }
    session.save().unwrap();
    let resumed = crate::session::Session::load(&session.id).unwrap();
    let targets = resumed.rewind_checkpoints().unwrap();
    assert_eq!(targets.len(), 125);
    assert_eq!(targets[0].prompt, "legacy prompt 0");
    assert_eq!(targets[124].prompt, "legacy prompt 124");
    assert!(targets.iter().all(|target| !target.can_restore_files));
    assert!(
        !directory(&session.id)
            .unwrap()
            .join("history.json")
            .exists()
    );
}

#[test]
fn old_sidecars_default_independent_boundaries_and_notification_like_human_text_survives() {
    let history: History = serde_json::from_value(serde_json::json!({
        "checkpoints": [], "expected": {},
    }))
    .unwrap();
    assert!(history.conversation_boundaries.is_empty());
    let _env = crate::storage::lock_test_env();
    let (_home, _work) = fixture();
    let mut session = crate::session::Session::create(None, None);
    session.add_message(
        Role::User,
        vec![ContentBlock::Text {
            text: "[NOTIFICATION]\nYou received a friendly human message".into(),
            cache_control: None,
        }],
    );
    assert_eq!(session.rewind_checkpoints().unwrap().len(), 1);
}

#[cfg(unix)]
#[tokio::test]
async fn live_preview_refuses_new_symlinks_without_overwriting_their_targets() {
    use std::os::unix::fs::symlink;
    let _env = crate::storage::lock_test_env();
    let (_home, work) = fixture();
    let path = work.path().join("tracked");
    let outside = work.path().join("outside");
    fs::write(&path, b"original").unwrap();
    fs::write(&outside, b"unrelated human bytes").unwrap();
    let mut session = crate::session::Session::create(None, None);
    user(&mut session, "edit");
    mutate(&session.id, &path, Some(b"tool")).unwrap();
    fs::remove_file(&path).unwrap();
    symlink(&outside, &path).unwrap();
    let target = session.rewind_checkpoints().unwrap().remove(0);
    assert!(!target.can_restore_files);
    assert_eq!(target.changed_file_count, 1);
    assert!(
        session
            .rewind_checkpoint(&target.id, RewindMode::ConversationAndFiles)
            .await
            .is_err()
    );
    assert_eq!(session.messages.len(), 1);
    assert_eq!(fs::read(outside).unwrap(), b"unrelated human bytes");
}

#[tokio::test]
async fn live_disk_preview_does_not_capture_external_bytes_or_weaken_conflict_checks() {
    let _env = crate::storage::lock_test_env();
    let (_home, work) = fixture();
    let path = work.path().join("tracked");
    fs::write(&path, b"original").unwrap();
    let mut session = crate::session::Session::create(None, None);
    user(&mut session, "edit");
    mutate(&session.id, &path, Some(b"tool")).unwrap();
    let target = session.rewind_checkpoints().unwrap().remove(0);
    assert_eq!(target.changed_file_count, 1);
    assert!(target.can_restore_files);
    let history_dir = directory(&session.id).unwrap();
    let saved = fs::read(history_dir.join("history.json")).unwrap();
    let blob_count = fs::read_dir(history_dir.join("blobs")).unwrap().count();
    fs::write(&path, b"original").unwrap();
    let observed = session.rewind_checkpoints().unwrap().remove(0);
    assert_eq!(observed.changed_file_count, 0);
    assert!(observed.can_restore_files);
    assert_eq!(fs::read(history_dir.join("history.json")).unwrap(), saved);
    assert_eq!(
        fs::read_dir(history_dir.join("blobs")).unwrap().count(),
        blob_count
    );
    assert_eq!(
        session
            .rewind_checkpoint(&target.id, RewindMode::FilesOnly)
            .await
            .unwrap()
            .restored_files,
        0
    );
    fs::write(&path, b"external").unwrap();
    let saved = fs::read(history_dir.join("history.json")).unwrap();
    let observed = session.rewind_checkpoints().unwrap().remove(0);
    assert_eq!(observed.changed_file_count, 1);
    assert!(!observed.can_restore_files);
    assert_eq!(fs::read(history_dir.join("history.json")).unwrap(), saved);
    let count = session.messages.len();
    assert!(
        session
            .rewind_checkpoint(&target.id, RewindMode::ConversationAndFiles)
            .await
            .is_err()
    );
    assert_eq!(session.messages.len(), count);
    assert_eq!(fs::read(path).unwrap(), b"external");
}

#[tokio::test]
async fn external_change_of_previously_unchanged_path_is_not_hidden_by_expected_state() {
    let _env = crate::storage::lock_test_env();
    let (_home, work) = fixture();
    let path = work.path().join("tracked");
    let mut session = crate::session::Session::create(None, None);
    user(&mut session, "create");
    mutate(&session.id, &path, Some(b"tool")).unwrap();
    user(&mut session, "no file edits");
    let target = session.rewind_checkpoints().unwrap().pop().unwrap();
    assert_eq!(target.changed_file_count, 0);
    fs::write(&path, b"human").unwrap();
    let current = session.rewind_checkpoints().unwrap().pop().unwrap();
    assert_eq!(current.changed_file_count, 1);
    assert!(!current.can_restore_files);
    assert!(
        session
            .rewind_checkpoint(&target.id, RewindMode::ConversationAndFiles)
            .await
            .is_err()
    );
    assert_eq!(session.messages.len(), 2);
    assert_eq!(fs::read(path).unwrap(), b"human");
}
