use super::*;

#[test]
fn stripped_metadata_save_preserves_authoritative_compaction() -> anyhow::Result<()> {
    let _env_lock = lock_env();
    let home = tempfile::tempdir()?;
    let _home = EnvVarGuard::set("JCODE_HOME", home.path());
    let id = "session_stripped_compaction_regression";
    let mut session = Session::create_with_id(id.to_string(), None, None);
    session.add_message(
        Role::User,
        vec![ContentBlock::Text {
            text: "Keep this transcript".to_string(),
            cache_control: None,
        }],
    );
    let compaction = StoredCompactionState {
        summary_text: "Keep this summary".to_string(),
        openai_encrypted_content: None,
        covers_up_to_turn: 1,
        original_turn_count: 1,
        compacted_count: 1,
    };
    session.compaction = Some(compaction.clone());
    session.save()?;

    let mut remote = Session::load(id)?;
    remote.strip_transcript_for_remote_client();
    assert!(remote.compaction.is_none());
    remote.save()?;

    let restored = Session::load(id)?;
    assert_eq!(restored.compaction, Some(compaction));
    assert_eq!(restored.messages.len(), 1);
    Ok(())
}
