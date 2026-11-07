use super::*;

struct Env(&'static str, Option<std::ffi::OsString>);

impl Env {
    fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
        let previous = std::env::var_os(key);
        crate::env::set_var(key, value);
        Self(key, previous)
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        match self.1.take() {
            Some(value) => crate::env::set_var(self.0, value),
            None => crate::env::remove_var(self.0),
        }
    }
}

fn write(dir: &Path, rel: &str, text: &str) {
    let path = dir.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn def(name: &str, description: &str, extra: &str, body: &str) -> String {
    format!("---\nname: {name}\ndescription: {description}\n{extra}---\n{body}\n")
}

/// Field order matters: the env var is restored and the directories removed
/// before the lock is released, so a following test never sees stale state.
struct Fx {
    _env: Env,
    home: tempfile::TempDir,
    project: tempfile::TempDir,
    _lock: std::sync::MutexGuard<'static, ()>,
}

fn fx() -> Fx {
    let lock = crate::storage::lock_test_env();
    let home = tempfile::tempdir().unwrap();
    let env = Env::set("JCODE_HOME", home.path());
    let project = tempfile::tempdir().unwrap();
    std::fs::create_dir(project.path().join(".git")).unwrap();
    Fx {
        _env: env,
        home,
        project,
        _lock: lock,
    }
}

#[test]
fn parses_a_real_aidlc_persona_from_claude_agents() {
    let f = fx();
    write(
        f.project.path(),
        ".claude/agents/aidlc-architect-agent.md",
        "---\nname: aidlc-architect-agent\ndisplay_name: Architect Agent\nexamples:\n  - tech-stack.md\ndescription: >\n  Solutions architect responsible for domain design.\n  Leads Feasibility stages.\ndisallowedTools: Task\ntier: judgment\n---\n\n# Architect Agent\n\nYou are a senior solutions architect.\n",
    );
    let defs = load_agent_definitions(Some(f.project.path()));
    let d = &defs["aidlc-architect-agent"];
    assert_eq!(
        d.description,
        "Solutions architect responsible for domain design. Leads Feasibility stages."
    );
    assert!(d.persona.starts_with("# Architect Agent"));
    assert_eq!(d.policy.disallowed_tools, ["swarm", "subagent"]);
    assert_eq!(d.origin, DefinitionOrigin::Project);
}

#[test]
fn precedence_across_the_four_directories() {
    let f = fx();
    let user_claude = tempfile::tempdir().unwrap();
    // `user_home_path` redirects under JCODE_HOME/external when JCODE_HOME is set.
    write(
        &f.home.path().join("external"),
        ".claude/agents/one.md",
        &def("one", "user claude", "", "u-claude"),
    );
    write(
        f.home.path(),
        "agents/one.md",
        &def("one", "user jcode", "", "u-jcode"),
    );
    write(
        f.project.path(),
        ".claude/agents/one.md",
        &def("one", "project claude", "", "p-claude"),
    );
    drop(user_claude);
    let pick = |f: &Fx| {
        let defs = load_agent_definitions(Some(f.project.path()));
        match defs.get("one") {
            Some(def) => def.description.clone(),
            None => format!("MISSING dirs={:?}", search_dirs(Some(f.project.path()))),
        }
    };
    assert_eq!(pick(&f), "project claude");
    write(
        f.project.path(),
        ".jcode/agents/one.md",
        &def("one", "project jcode", "", "p-jcode"),
    );
    assert_eq!(pick(&f), "project jcode");
    std::fs::remove_dir_all(f.project.path().join(".jcode")).unwrap();
    std::fs::remove_dir_all(f.project.path().join(".claude")).unwrap();
    assert_eq!(pick(&f), "user jcode");
    std::fs::remove_file(f.home.path().join("agents/one.md")).unwrap();
    assert_eq!(pick(&f), "user claude");
}

#[test]
fn closest_directory_wins_when_walking_up_to_the_project_root() {
    let f = fx();
    write(
        f.project.path(),
        ".jcode/agents/x.md",
        &def("x", "root", "", "r"),
    );
    write(
        f.project.path(),
        "sub/.jcode/agents/x.md",
        &def("x", "closest", "", "c"),
    );
    let nested = f.project.path().join("sub/deeper");
    std::fs::create_dir_all(&nested).unwrap();
    assert_eq!(
        load_agent_definitions(Some(&nested))["x"].description,
        "closest"
    );
    assert_eq!(
        load_agent_definitions(Some(f.project.path()))["x"].description,
        "root"
    );
}

#[test]
fn scan_is_recursive_and_identity_comes_from_name() {
    let f = fx();
    write(
        f.project.path(),
        ".jcode/agents/team/a/file-name-ignored.md",
        &def("real-name", "d", "", "p"),
    );
    let defs = load_agent_definitions(Some(f.project.path()));
    assert!(defs.contains_key("real-name"));
    assert_eq!(defs.len(), 1);
}

#[test]
fn skip_rules() {
    let f = fx();
    let agents = ".jcode/agents";
    write(
        f.project.path(),
        &format!("{agents}/readme.md"),
        "# Just docs\n",
    );
    write(
        f.project.path(),
        &format!("{agents}/noname.md"),
        "---\ndescription: d\n---\nbody\n",
    );
    write(
        f.project.path(),
        &format!("{agents}/nodesc.md"),
        "---\nname: nodesc\n---\nbody\n",
    );
    write(
        f.project.path(),
        &format!("{agents}/badyaml.md"),
        "---\nname: [unclosed\n---\nbody\n",
    );
    write(
        f.project.path(),
        &format!("{agents}/badname.md"),
        &def("Bad:Name", "d", "", "b"),
    );
    write(
        f.project.path(),
        &format!("{agents}/ok.md"),
        &def("ok", "d", "", "b"),
    );
    let defs = load_agent_definitions(Some(f.project.path()));
    assert_eq!(defs.keys().collect::<Vec<_>>(), ["ok"]);
}

#[test]
fn policy_fields_accept_both_key_spellings_and_map_tools() {
    let f = fx();
    write(
        f.project.path(),
        ".jcode/agents/p.md",
        &def(
            "p",
            "d",
            "model: inherit\neffort: HIGH\ntools: [Read, Grep, 'Bash(git push *)', Mystery]\ndisallowed-tools: Write, WebFetch\nunknown_key: ignored\n",
            "b",
        ),
    );
    write(
        f.project.path(),
        ".jcode/agents/q.md",
        &def(
            "q",
            "d",
            "disallowedTools: Edit\ntools: Read, Bash(a, b)\n",
            "b",
        ),
    );
    let defs = load_agent_definitions(Some(f.project.path()));
    let p = &defs["p"].policy;
    assert_eq!(p.model.as_deref(), Some("inherit"));
    assert_eq!(p.effort.as_deref(), Some("high"));
    assert_eq!(
        p.tools.as_deref(),
        Some(&["read".to_string(), "agentgrep".into(), "bash".into()][..])
    );
    assert_eq!(p.disallowed_tools, ["write", "webfetch"]);
    let q = &defs["q"].policy;
    assert_eq!(q.disallowed_tools, ["edit"]);
    assert_eq!(
        q.tools.as_deref(),
        Some(&["read".to_string(), "bash".into()][..])
    );
}

#[test]
fn name_validation() {
    assert!(valid_name("aidlc-quality_agent2"));
    assert!(!valid_name(""));
    assert!(!valid_name("UPPER"));
    assert!(!valid_name("a:b"));
    assert!(!valid_name(&"a".repeat(257)));
    assert!(valid_name(&"a".repeat(256)));
}

#[test]
fn summaries_and_lookup() {
    let f = fx();
    write(
        f.project.path(),
        ".jcode/agents/b.md",
        &def("b", "second", "", "pb"),
    );
    write(
        f.project.path(),
        ".jcode/agents/a.md",
        &def("a", "first", "", "pa"),
    );
    assert_eq!(
        agent_definition_summaries(Some(f.project.path())),
        [
            ("a".to_string(), "first".to_string()),
            ("b".to_string(), "second".to_string())
        ]
    );
    assert_eq!(
        find_agent_definition(" a ", Some(f.project.path()))
            .unwrap()
            .persona,
        "pa"
    );
    assert!(find_agent_definition("zzz", Some(f.project.path())).is_none());
    assert!(load_agent_definitions(None).is_empty());
}

#[test]
fn user_directories_reached_by_the_ancestor_walk_stay_user_definitions() {
    let f = fx();
    // `user_home_path` redirects under JCODE_HOME/external when JCODE_HOME is set.
    let user_home = f.home.path().join("external");
    write(
        &user_home,
        ".claude/agents/global.md",
        &def("global", "user claude", "model: opus\n", "g"),
    );
    std::fs::create_dir_all(user_home.join("work")).unwrap();
    // Launched from the user's home and from a non-git directory below it.
    for cwd in [user_home.clone(), user_home.join("work")] {
        let defs = load_agent_definitions(Some(&cwd));
        assert_eq!(defs["global"].origin, DefinitionOrigin::User, "{cwd:?}");
        assert!(project_policy_definitions(Some(&cwd)).is_empty(), "{cwd:?}");
    }
}
