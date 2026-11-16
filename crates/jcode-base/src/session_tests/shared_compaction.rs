//! Regression coverage for shared compaction payloads. Hook as a child of session.rs.
use super::*;
use std::collections::HashSet;
use std::sync::Arc;

fn large_state(tag: &str) -> StoredCompactionState {
    StoredCompactionState {
        summary_text: format!("summary {tag}"),
        openai_encrypted_content: Some(
            format!("{tag}{}", "encrypted-body-".repeat(32 * 1024)).into(),
        ),
        covers_up_to_turn: 3,
        original_turn_count: 7,
        compacted_count: 2,
    }
}

fn fixture() -> Session {
    let mut session = Session::create_with_id("session_shared_compaction".into(), None, None);
    session.compaction = Some(Arc::new(large_state("original")));
    session.reset_persist_state(false);
    session
}

fn assert_metadata_shared(session: &Session) {
    let canonical = session.compaction.as_ref().expect("canonical compaction");
    let metadata = session
        .persist_state
        .last_meta
        .as_ref()
        .unwrap()
        .compaction
        .as_ref()
        .unwrap();
    assert!(Arc::ptr_eq(canonical, metadata));
    assert_eq!(
        canonical
            .openai_encrypted_content
            .as_ref()
            .unwrap()
            .as_ptr(),
        metadata.openai_encrypted_content.as_ref().unwrap().as_ptr()
    );
}

#[test]
fn reset_and_clone_retain_one_unique_encrypted_payload_until_mutation() {
    let session = fixture();
    assert_metadata_shared(&session);
    let mut cloned = session.clone();
    assert_metadata_shared(&cloned);
    assert!(Arc::ptr_eq(
        session.compaction.as_ref().unwrap(),
        cloned.compaction.as_ref().unwrap()
    ));

    // Count distinct retained encrypted-body allocations, not process RSS.
    let retained = [
        session.compaction.as_ref().unwrap(),
        session
            .persist_state
            .last_meta
            .as_ref()
            .unwrap()
            .compaction
            .as_ref()
            .unwrap(),
        cloned.compaction.as_ref().unwrap(),
        cloned
            .persist_state
            .last_meta
            .as_ref()
            .unwrap()
            .compaction
            .as_ref()
            .unwrap(),
    ];
    let unique: HashSet<_> = retained
        .iter()
        .map(|state| state.openai_encrypted_content.as_ref().unwrap().as_ptr())
        .collect();
    assert_eq!(
        unique.len(),
        1,
        "four references retain only one encrypted payload allocation"
    );

    Arc::make_mut(cloned.compaction.as_mut().unwrap())
        .summary_text
        .push_str(" changed");
    assert!(!Arc::ptr_eq(
        session.compaction.as_ref().unwrap(),
        cloned.compaction.as_ref().unwrap()
    ));
    assert_eq!(
        session
            .compaction
            .as_ref()
            .unwrap()
            .openai_encrypted_content
            .as_ref()
            .unwrap()
            .as_ptr(),
        cloned
            .compaction
            .as_ref()
            .unwrap()
            .openai_encrypted_content
            .as_ref()
            .unwrap()
            .as_ptr(),
        "copy-on-write of summary metadata must retain the shared encrypted body"
    );
    assert_eq!(
        session
            .compaction
            .as_ref()
            .unwrap()
            .openai_encrypted_content,
        cloned.compaction.as_ref().unwrap().openai_encrypted_content
    );
    assert_eq!(
        session.compaction.as_ref().unwrap().summary_text,
        "summary original"
    );
    assert_eq!(
        cloned
            .persist_state
            .last_meta
            .as_ref()
            .unwrap()
            .compaction
            .as_ref()
            .unwrap()
            .summary_text,
        "summary original"
    );
    assert_metadata_shared(&session);
    cloned.reset_persist_state(false);
    assert_metadata_shared(&cloned);
}

#[test]
fn redacted_export_cannot_mutate_original_or_persisted_snapshot() {
    let mut session = fixture();
    let secret = "ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZ0123";
    Arc::make_mut(session.compaction.as_mut().unwrap()).summary_text = format!("token {secret}");
    session.reset_persist_state(false);
    let before = serde_json::to_vec(&session).unwrap();
    let mut exported = session.redacted_for_export();
    assert!(
        !exported
            .compaction
            .as_ref()
            .unwrap()
            .summary_text
            .contains(secret)
    );
    assert!(!Arc::ptr_eq(
        session.compaction.as_ref().unwrap(),
        exported.compaction.as_ref().unwrap()
    ));
    Arc::make_mut(exported.compaction.as_mut().unwrap()).openai_encrypted_content = None;
    assert_eq!(serde_json::to_vec(&session).unwrap(), before);
    assert!(
        session
            .compaction
            .as_ref()
            .unwrap()
            .summary_text
            .contains(secret)
    );
    assert_metadata_shared(&session);
}

#[test]
fn arc_compaction_json_is_byte_identical_and_reads_owned_snapshot() -> anyhow::Result<()> {
    let owned = large_state("wire-format");
    let legacy_json = format!(
        "{{\"summary_text\":\"summary wire-format\",\"openai_encrypted_content\":{},\"covers_up_to_turn\":3,\"original_turn_count\":7,\"compacted_count\":2}}",
        serde_json::to_string(owned.openai_encrypted_content.as_deref().unwrap())?
    );
    assert_eq!(serde_json::to_vec(&owned)?, legacy_json.as_bytes());
    let legacy_state: StoredCompactionState = serde_json::from_str(&legacy_json)?;
    assert_eq!(legacy_state, owned);
    let shared = Arc::new(owned.clone());
    assert_eq!(serde_json::to_vec(&owned)?, serde_json::to_vec(&shared)?);
    assert_eq!(
        serde_json::to_vec(&Some(owned.clone()))?,
        serde_json::to_vec(&Some(shared.clone()))?
    );
    assert_eq!(
        serde_json::to_vec(&Option::<StoredCompactionState>::None)?,
        serde_json::to_vec(&Option::<Arc<StoredCompactionState>>::None)?
    );

    // Build the legacy snapshot with an owned state serialized into the original field.
    let session = fixture();
    let mut legacy = serde_json::to_value(&session)?;
    legacy["compaction"] = serde_json::to_value(&owned)?;
    let restored: Session = serde_json::from_slice(&serde_json::to_vec(&legacy)?)?;
    assert_eq!(restored.compaction.as_deref(), Some(&owned));
    let startup: RemoteStartupSessionSnapshot = serde_json::from_value(legacy)?;
    assert_eq!(startup.compaction.as_deref(), Some(&owned));

    let mut meta = session.journal_meta();
    meta.compaction = Some(shared);
    let bytes = serde_json::to_vec(&meta)?;
    let restored_meta: SessionJournalMeta = serde_json::from_slice(&bytes)?;
    assert_eq!(serde_json::to_vec(&restored_meta)?, bytes);
    assert_eq!(restored_meta.compaction.as_deref(), Some(&owned));
    Ok(())
}

#[test]
fn journal_replay_distinguishes_unchanged_changed_and_explicit_clear() -> anyhow::Result<()> {
    let mut session = fixture();
    let initial = session.compaction.clone();
    let mut unchanged = session.journal_meta();
    unchanged.compaction = None;
    unchanged.compaction_unchanged = true;
    session.apply_journal_meta(serde_json::from_slice(&serde_json::to_vec(&unchanged)?)?);
    assert!(Arc::ptr_eq(
        session.compaction.as_ref().unwrap(),
        initial.as_ref().unwrap()
    ));

    let replacement = large_state("replacement");
    let mut changed = session.journal_meta();
    changed.compaction = Some(Arc::new(replacement.clone()));
    changed.compaction_unchanged = false;
    let mut legacy = serde_json::to_value(changed)?;
    legacy
        .as_object_mut()
        .unwrap()
        .remove("compaction_unchanged");
    session.apply_journal_meta(serde_json::from_value(legacy)?);
    assert_eq!(session.compaction.as_deref(), Some(&replacement));

    let mut clear = session.journal_meta();
    clear.compaction = None;
    clear.compaction_unchanged = false;
    session.apply_journal_meta(serde_json::from_slice(&serde_json::to_vec(&clear)?)?);
    assert!(session.compaction.is_none());
    session.apply_journal_meta(unchanged);
    assert!(
        session.compaction.is_none(),
        "unchanged cannot resurrect cleared state"
    );
    Ok(())
}

struct HomeGuard(Option<std::ffi::OsString>);
impl Drop for HomeGuard {
    fn drop(&mut self) {
        if let Some(previous) = &self.0 {
            crate::env::set_var("JCODE_HOME", previous);
        } else {
            crate::env::remove_var("JCODE_HOME");
        }
    }
}

/// Repeat with `justrust test -p jcode-base shared_compaction_retention_microprobe -- --ignored --nocapture`.
/// Measures retained backing allocations and clone wall time, never RSS.
#[test]
#[ignore = "optional retained-allocation microprobe"]
fn shared_compaction_retention_microprobe() {
    let mut session = fixture();
    Arc::make_mut(session.compaction.as_mut().unwrap()).openai_encrypted_content =
        Some("encrypted".repeat(1024 * 1024).into());
    session.reset_persist_state(false);
    let started = std::time::Instant::now();
    let clones: Vec<_> = (0..128).map(|_| session.clone()).collect();
    let elapsed = started.elapsed();
    let unique: HashSet<_> = clones
        .iter()
        .chain(std::iter::once(&session))
        .flat_map(|session| {
            [
                session.compaction.as_ref().unwrap(),
                session
                    .persist_state
                    .last_meta
                    .as_ref()
                    .unwrap()
                    .compaction
                    .as_ref()
                    .unwrap(),
            ]
        })
        .map(|state| state.openai_encrypted_content.as_ref().unwrap().as_ptr())
        .collect();
    let payload_bytes = session
        .compaction
        .as_ref()
        .unwrap()
        .openai_encrypted_content
        .as_ref()
        .unwrap()
        .len();
    assert_eq!(unique.len(), 1);
    eprintln!(
        "sessions={} retained_references={} unique_payload_allocations={} unique_payload_bytes={} clone_elapsed_us={}",
        clones.len() + 1,
        2 * (clones.len() + 1),
        unique.len(),
        payload_bytes * unique.len(),
        elapsed.as_micros()
    );
}

#[test]
fn save_and_load_share_metadata_and_persist_compaction_transitions() -> anyhow::Result<()> {
    let _lock = crate::storage::lock_test_env();
    let home = tempfile::tempdir()?;
    let _restore = HomeGuard(std::env::var_os("JCODE_HOME"));
    crate::env::set_var("JCODE_HOME", home.path());
    let mut session = fixture();
    session.add_message(
        Role::User,
        vec![ContentBlock::Text {
            text: "keep transcript".into(),
            cache_control: None,
        }],
    );
    session.save()?;
    assert_metadata_shared(&session);
    let snapshot = session_path(&session.id)?;
    let original_snapshot = std::fs::read(&snapshot)?;
    let expected = session.compaction.clone();

    session.add_message(
        Role::User,
        vec![ContentBlock::Text {
            text: "append without changing compaction".into(),
            cache_control: None,
        }],
    );
    session.save()?;
    assert_metadata_shared(&session);
    let journal = session_journal_path(&session.id)?;
    let entries: Vec<serde_json::Value> = std::fs::read_to_string(&journal)?
        .lines()
        .map(serde_json::from_str)
        .collect::<Result<_, _>>()?;
    assert_eq!(
        entries.last().unwrap()["meta"]["compaction_unchanged"],
        true
    );
    assert!(entries.last().unwrap()["meta"]["compaction"].is_null());
    let loaded = Session::load(&session.id)?;
    assert_eq!(loaded.compaction, expected);
    assert_metadata_shared(&loaded);

    Arc::make_mut(session.compaction.as_mut().unwrap()).summary_text = "changed summary".into();
    session.save()?;
    assert_metadata_shared(&session);
    let changed = session.compaction.clone();
    assert_eq!(Session::load(&session.id)?.compaction, changed);
    assert_eq!(Session::load_startup_stub(&session.id)?.compaction, changed);
    assert_eq!(
        Session::load_for_remote_startup(&session.id)?.compaction,
        changed
    );
    assert_eq!(
        std::fs::read(&snapshot)?,
        original_snapshot,
        "changed state should exercise journal replay, not checkpoint replacement"
    );

    session.compaction = None;
    session.save()?;
    assert!(
        session
            .persist_state
            .last_meta
            .as_ref()
            .unwrap()
            .compaction
            .is_none()
    );
    assert!(Session::load(&session.id)?.compaction.is_none());
    assert!(
        Session::load_startup_stub(&session.id)?
            .compaction
            .is_none()
    );
    assert!(
        Session::load_for_remote_startup(&session.id)?
            .compaction
            .is_none()
    );
    let entries: Vec<serde_json::Value> = std::fs::read_to_string(journal)?
        .lines()
        .map(serde_json::from_str)
        .collect::<Result<_, _>>()?;
    let last = &entries.last().unwrap()["meta"];
    assert!(last["compaction"].is_null());
    assert_ne!(last["compaction_unchanged"], true);
    Ok(())
}
