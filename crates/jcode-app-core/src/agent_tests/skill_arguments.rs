use super::*;

fn skill_texts(agent: &Agent) -> Vec<String> {
    agent
        .messages()
        .iter()
        .filter_map(|stored| {
            let text = content_text(&stored.content);
            text.contains("# Active Skill").then(|| text.to_string())
        })
        .collect()
}

#[test]
fn skill_arguments_are_substituted_and_reannounced_only_when_rendering_changes() {
    let _guard = crate::storage::lock_test_env();
    let home = tempfile::tempdir().expect("home");
    let project = tempfile::tempdir().expect("project");
    let previous = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", home.path());

    let skill_dir = project.path().join(".jcode/skills/triage");
    std::fs::create_dir_all(&skill_dir).expect("skill dir");
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: triage\ndescription: Triage\n---\nHandle: $ARGUMENTS in ${JCODE_PROJECT_DIR}\n",
    )
    .expect("write skill");

    let provider: Arc<dyn Provider> = Arc::new(NativeAutoCompactionProvider);
    let mut session = Session::create(None, None);
    session.working_dir = Some(project.path().display().to_string());
    let mut agent = Agent::new_with_session(provider, Registry::empty(), session, None);

    assert!(agent.set_remote_active_skill(Some("triage".into()), Some("issue 42".into())));
    let first = skill_texts(&agent);
    assert_eq!(first.len(), 1);
    assert!(first[0].contains(&format!("Handle: issue 42 in {}", project.path().display())));

    // Same skill, same arguments: nothing new is announced.
    assert!(agent.set_remote_active_skill(Some("triage".into()), Some("issue 42".into())));
    assert_eq!(skill_texts(&agent).len(), 1);

    // Same skill, different arguments: the new rendering is appended.
    assert!(agent.set_remote_active_skill(Some("triage".into()), Some("issue 43".into())));
    let third = skill_texts(&agent);
    assert_eq!(third.len(), 2);
    assert!(third[1].contains("Handle: issue 43"));

    // Follow-up messages without arguments keep the skill and add nothing.
    assert!(agent.set_remote_active_skill(Some("triage".into()), None));
    assert_eq!(skill_texts(&agent).len(), 2);

    match previous {
        Some(value) => crate::env::set_var("JCODE_HOME", value),
        None => crate::env::remove_var("JCODE_HOME"),
    }
}

#[test]
fn empty_arguments_replace_old_ones_and_rewind_allows_reannouncing() {
    let _guard = crate::storage::lock_test_env();
    let home = tempfile::tempdir().expect("home");
    let project = tempfile::tempdir().expect("project");
    let previous = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", home.path());

    let skill_dir = project.path().join(".jcode/skills/triage");
    std::fs::create_dir_all(&skill_dir).expect("skill dir");
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: triage\ndescription: Triage\n---\nHandle: [$ARGUMENTS]\n",
    )
    .expect("write skill");

    let provider: Arc<dyn Provider> = Arc::new(NativeAutoCompactionProvider);
    let mut session = Session::create(None, None);
    session.working_dir = Some(project.path().display().to_string());
    let mut agent = Agent::new_with_session(provider, Registry::empty(), session, None);

    assert!(agent.set_remote_active_skill(Some("triage".into()), Some("issue 42".into())));
    // An explicit empty string is a bare re-invocation and must not keep 42.
    assert!(agent.set_remote_active_skill(Some("triage".into()), Some(String::new())));
    let texts = skill_texts(&agent);
    assert_eq!(texts.len(), 2);
    assert!(texts[1].contains("Handle: []"));

    // Rewinding drops the last announcement, so repeating it must announce again.
    agent.add_message(
        Role::User,
        vec![ContentBlock::Text {
            text: "work on it".to_string(),
            cache_control: None,
        }],
    );
    assert!(agent.set_remote_active_skill(Some("triage".into()), Some("issue 43".into())));
    let before_rewind = skill_texts(&agent).len();
    agent
        .rewind_to_message(agent.session.rewind_target_count())
        .expect("rewind");
    assert_eq!(skill_texts(&agent).len(), before_rewind - 1);
    assert!(agent.set_remote_active_skill(Some("triage".into()), Some("issue 43".into())));
    assert_eq!(skill_texts(&agent).len(), before_rewind);

    match previous {
        Some(value) => crate::env::set_var("JCODE_HOME", value),
        None => crate::env::remove_var("JCODE_HOME"),
    }
}

#[test]
fn skill_is_reannounced_after_compaction_drops_the_announcement() {
    let _guard = crate::storage::lock_test_env();
    let home = tempfile::tempdir().expect("home");
    let project = tempfile::tempdir().expect("project");
    let previous = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", home.path());

    let skill_dir = project.path().join(".jcode/skills/triage");
    std::fs::create_dir_all(&skill_dir).expect("skill dir");
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: triage\ndescription: Triage\n---\nHandle: $ARGUMENTS\n",
    )
    .expect("write skill");

    let provider: Arc<dyn Provider> = Arc::new(NativeAutoCompactionProvider);
    let mut session = Session::create(None, None);
    session.working_dir = Some(project.path().display().to_string());
    let mut agent = Agent::new_with_session(provider, Registry::empty(), session, None);

    assert!(agent.set_remote_active_skill(Some("triage".into()), Some("issue 42".into())));
    assert_eq!(skill_texts(&agent).len(), 1);

    // Compaction may drop the announcement; the identical invocation must
    // then add the instructions back instead of hitting the duplicate check.
    agent.note_compaction_applied();
    assert!(agent.set_remote_active_skill(Some("triage".into()), Some("issue 42".into())));
    assert_eq!(skill_texts(&agent).len(), 2);

    match previous {
        Some(value) => crate::env::set_var("JCODE_HOME", value),
        None => crate::env::remove_var("JCODE_HOME"),
    }
}
