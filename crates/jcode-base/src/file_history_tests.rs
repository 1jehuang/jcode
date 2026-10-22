use super::*;

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
