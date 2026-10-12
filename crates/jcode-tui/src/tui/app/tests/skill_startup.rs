fn with_skill_startup_home(f: impl FnOnce(&std::path::Path)) {
    let _lock = crate::storage::lock_test_env();
    let home = tempfile::tempdir().unwrap();
    struct Restore(Vec<(&'static str, Option<std::ffi::OsString>)>);
    impl Drop for Restore {
        fn drop(&mut self) {
            for (key, value) in self.0.drain(..) {
                match value {
                    Some(value) => crate::env::set_var(key, value),
                    None => crate::env::remove_var(key),
                }
            }
            crate::config::invalidate_config_cache();
            crate::tui::ui::prepare::invalidate_header_prep_cache();
        }
    }
    let _restore = Restore(
        [
            "HOME",
            "USERPROFILE",
            "JCODE_HOME",
            "JCODE_SSH_REMOTE",
            "JCODE_RELOAD_FAST_START",
        ]
        .into_iter()
        .map(|key| (key, std::env::var_os(key)))
        .collect(),
    );
    crate::env::set_var("HOME", home.path());
    crate::env::set_var("USERPROFILE", home.path());
    crate::env::set_var("JCODE_HOME", home.path().join(".jcode"));
    crate::env::remove_var("JCODE_SSH_REMOTE");
    crate::env::remove_var("JCODE_RELOAD_FAST_START");
    crate::config::invalidate_config_cache();
    // JCODE_HOME redirects global home-relative files into its external sandbox.
    // Exercise the same .agents/skills lookup as a normal unsandboxed home.
    f(&home.path().join(".jcode/external"));
}

fn write_startup_skill(root: &std::path::Path, name: &str) {
    let directory = root.join(".agents/skills").join(name);
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(
        directory.join("SKILL.md"),
        format!(
            "---\nname: {name}\ndescription: Startup regression fixture\n---\nUse this skill.\n"
        ),
    )
    .unwrap();
}

fn startup_skill_commands(app: &mut App, input: &str) -> Vec<String> {
    app.input = input.into();
    app.advance_command_suggestions_epoch();
    app.command_suggestions()
        .into_iter()
        .filter(|(_, help)| *help == "Activate skill")
        .map(|(command, _)| command)
        .collect()
}

#[test]
fn skill_startup_minimal_client_includes_global_skills_before_history() {
    with_skill_startup_home(|home| {
        write_startup_skill(home, "startup-global");
        let mut app = App::new_for_remote_with_options(None, false);
        // None intentionally falls back to process cwd, which other tests may
        // populate with project skills. Use an empty explicit project here.
        let project = home.join("empty-project");
        std::fs::create_dir_all(&project).unwrap();
        app.session.working_dir = Some(project.to_string_lossy().into_owned());
        assert!(app.remote_skills.is_empty());
        assert!(app.session.messages.is_empty());
        assert_eq!(
            startup_skill_commands(&mut app, "/"),
            vec!["/startup-global"]
        );
        assert_eq!(
            startup_skill_commands(&mut app, "/startup-g"),
            vec!["/startup-global"]
        );
        assert_eq!(
            crate::tui::TuiState::available_skills(&app),
            vec!["startup-global"]
        );
    });
}

#[test]
fn skill_startup_minimal_clients_keep_project_overlays_separate() {
    with_skill_startup_home(|home| {
        write_startup_skill(home, "startup-global");
        let a = home.join("project-a");
        let b = home.join("project-b");
        write_startup_skill(&a, "startup-project-a");
        write_startup_skill(&b, "startup-project-b");
        let mut first = App::new_for_remote_with_options(None, false);
        let mut second = App::new_for_remote_with_options(None, false);
        first.session.working_dir = Some(a.to_string_lossy().into_owned());
        second.session.working_dir = Some(b.to_string_lossy().into_owned());
        assert_eq!(
            startup_skill_commands(&mut first, "/"),
            vec!["/startup-global", "/startup-project-a"]
        );
        assert_eq!(
            startup_skill_commands(&mut second, "/"),
            vec!["/startup-global", "/startup-project-b"]
        );
        assert!(
            first
                .registry
                .skills()
                .try_read()
                .unwrap()
                .get("startup-project-a")
                .is_none()
        );
    });
}

#[test]
fn skill_startup_ssh_history_replaces_warmed_empty_candidates_without_local_skills() {
    with_skill_startup_home(|home| {
        write_startup_skill(home, "startup-local-secret");
        crate::env::set_var("JCODE_SSH_REMOTE", "fixture-host");
        let mut app = App::new_for_remote_with_options(None, false);
        assert!(startup_skill_commands(&mut app, "/").is_empty());
        assert!(crate::tui::TuiState::available_skills(&app).is_empty());
        assert!(app.current_skills_snapshot().list().is_empty());
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let _enter = runtime.enter();
        let mut remote = crate::tui::backend::RemoteConnection::dummy();
        let event = serde_json::from_value(serde_json::json!({
            "type": "history", "id": 1, "session_id": "skill-startup-remote",
            "messages": [], "skills": ["startup-remote"],
            "server_version": jcode_build_meta::version()
        }))
        .unwrap();
        app.handle_server_event(event, &mut remote);
        assert_eq!(
            startup_skill_commands(&mut app, "/"),
            vec!["/startup-remote"]
        );
        assert_eq!(
            startup_skill_commands(&mut app, "/startup-r"),
            vec!["/startup-remote"]
        );
        assert_eq!(
            crate::tui::TuiState::available_skills(&app),
            vec!["startup-remote"]
        );
    });
}
