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
