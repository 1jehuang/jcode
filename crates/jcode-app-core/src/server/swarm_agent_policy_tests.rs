use super::*;
use crate::agent_def::{AgentDefinition, DefinitionOrigin, DefinitionPolicy};
use crate::agent_def::{agents_digest, project_policy_definitions};
use std::path::PathBuf;

fn def(origin: DefinitionOrigin, policy: DefinitionPolicy) -> AgentDefinition {
    AgentDefinition {
        name: "worker".into(),
        description: "d".into(),
        persona: "p".into(),
        policy,
        origin,
        path: PathBuf::from("/x/worker.md"),
    }
}

fn policy(
    model: Option<&str>,
    effort: Option<&str>,
    tools: Option<&[&str]>,
    deny: &[&str],
) -> DefinitionPolicy {
    DefinitionPolicy {
        model: model.map(str::to_string),
        effort: effort.map(str::to_string),
        tools: tools.map(|t| t.iter().map(|s| s.to_string()).collect()),
        disallowed_tools: deny.iter().map(|s| s.to_string()).collect(),
    }
}

fn agents(force: bool) -> AgentsConfig {
    AgentsConfig {
        force_swarm_defaults: force,
        swarm_model: Some("global-model".into()),
        swarm_effort: Some("low".into()),
        ..AgentsConfig::default()
    }
}

#[test]
fn user_definition_policy_applies_and_beats_defaults() {
    let d = def(
        DefinitionOrigin::User,
        policy(
            Some("openai-api:gpt-5.5"),
            Some("HIGH"),
            Some(&["read", "bash"]),
            &["bash"],
        ),
    );
    let applied = applied_policy(&d, &agents(false), None);
    assert_eq!(applied.model.as_deref(), Some("openai-api:gpt-5.5"));
    assert_eq!(applied.effort.as_deref(), Some("high"));
    assert_eq!(
        applied.tools.as_deref(),
        Some(&["read".to_string(), "bash".into()][..])
    );
    assert_eq!(applied.disallowed_tools, ["bash"]);
    assert_eq!(applied.notice, None);

    let request = spawn_request(Some("requested".into()), Some("medium".into()), &applied);
    assert_eq!(
        request.model.as_deref(),
        Some("openai-api:gpt-5.5"),
        "definition beats per-spawn model"
    );
    assert_eq!(request.effort.as_deref(), Some("high"));
}

#[test]
fn force_swarm_defaults_makes_operator_defaults_win() {
    let d = def(
        DefinitionOrigin::User,
        policy(Some("m"), Some("high"), Some(&["read"]), &["bash"]),
    );
    let applied = applied_policy(&d, &agents(true), None);
    assert_eq!(
        applied,
        AppliedPolicy {
            notice: applied.notice.clone(),
            ..AppliedPolicy::default()
        }
    );
    assert!(
        applied
            .notice
            .as_deref()
            .unwrap()
            .contains("force_swarm_defaults")
    );
    let request = spawn_request(None, None, &applied);
    assert_eq!(
        request,
        SpawnRequest {
            model: None,
            effort: None
        }
    );
}

#[test]
fn inherit_selects_the_coordinator_past_a_pinned_default() {
    let d = def(
        DefinitionOrigin::User,
        policy(Some("inherit"), None, None, &[]),
    );
    let applied = applied_policy(&d, &agents(false), None);
    assert!(applied.inherit_model);
    assert_eq!(
        spawn_request(None, None, &applied).model.as_deref(),
        Some("inherit")
    );
}

#[test]
fn unknown_effort_is_dropped_and_empty_policy_passes_through() {
    let d = def(
        DefinitionOrigin::User,
        policy(None, Some("turbo"), None, &[]),
    );
    let applied = applied_policy(&d, &agents(false), None);
    assert_eq!(applied.effort, None);
    let none = applied_policy(
        &def(DefinitionOrigin::User, DefinitionPolicy::default()),
        &agents(false),
        None,
    );
    assert_eq!(none, AppliedPolicy::default());
    let request = spawn_request(Some("a".into()), Some("low".into()), &none);
    assert_eq!(
        request,
        SpawnRequest {
            model: Some("a".into()),
            effort: Some("low".into())
        }
    );
}

#[test]
fn tools_only_narrow_and_never_widen() {
    let mut base: HashSet<String> = ["read", "bash", "write"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let applied = AppliedPolicy {
        tools: Some(vec!["read".into(), "bash".into(), "webfetch".into()]),
        disallowed_tools: vec!["bash".into()],
        ..AppliedPolicy::default()
    };
    narrow_tools(&mut base, &applied);
    let mut got: Vec<_> = base.into_iter().collect();
    got.sort();
    assert_eq!(
        got,
        ["read"],
        "webfetch was not in the global set so it must not appear"
    );
}

struct Fx {
    _env: Env,
    home: tempfile::TempDir,
    project: tempfile::TempDir,
    _lock: std::sync::MutexGuard<'static, ()>,
}

struct Env(Option<std::ffi::OsString>);

impl Drop for Env {
    fn drop(&mut self) {
        match self.0.take() {
            Some(value) => crate::env::set_var("JCODE_HOME", value),
            None => crate::env::remove_var("JCODE_HOME"),
        }
    }
}

fn fx() -> Fx {
    let lock = crate::storage::lock_test_env();
    let home = tempfile::tempdir().unwrap();
    let previous = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", home.path());
    let project = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(project.path().join(".git")).unwrap();
    std::fs::create_dir_all(project.path().join(".claude/agents")).unwrap();
    std::fs::write(
        project.path().join(".claude/agents/worker.md"),
        "---\nname: worker\ndescription: d\nmodel: opus\neffort: high\ntools: Read\n---\nbody\n",
    )
    .unwrap();
    Fx {
        _env: Env(previous),
        home,
        project,
        _lock: lock,
    }
}

#[test]
fn project_definition_policy_needs_the_agents_trust_scope() {
    let f = fx();
    let _ = &f.home;
    let defs = crate::agent_def::load_agent_definitions(Some(f.project.path()));
    let d = &defs["worker"];
    assert_eq!(d.origin, DefinitionOrigin::Project);

    // Untrusted: persona only, with a notice. Nothing changes.
    let untrusted = applied_policy(d, &agents(false), Some(f.project.path()));
    assert_eq!(untrusted.model, None);
    assert_eq!(untrusted.effort, None);
    assert_eq!(untrusted.tools, None);
    assert!(untrusted.notice.as_deref().unwrap().contains("not trusted"));

    // Trusted: applies.
    let project_defs = project_policy_definitions(Some(f.project.path()));
    let refs: Vec<_> = project_defs.iter().collect();
    project_trust::grant(f.project.path(), Scope::Agents, &agents_digest(&refs)).unwrap();
    let trusted = applied_policy(d, &agents(false), Some(f.project.path()));
    assert_eq!(trusted.model.as_deref(), Some("opus"));
    assert_eq!(trusted.effort.as_deref(), Some("high"));
    assert_eq!(trusted.tools.as_deref(), Some(&["read".to_string()][..]));

    // Editing the policy invalidates the approval.
    std::fs::write(
        f.project.path().join(".claude/agents/worker.md"),
        "---\nname: worker\ndescription: d\nmodel: opus\neffort: max\ntools: Read\n---\nbody\n",
    )
    .unwrap();
    let defs = crate::agent_def::load_agent_definitions(Some(f.project.path()));
    let changed = applied_policy(&defs["worker"], &agents(false), Some(f.project.path()));
    assert_eq!(changed.effort, None);

    // Persona-only edits do not change the digest: persona never needs trust.
    let before = agents_digest(&[&defs["worker"]]);
    std::fs::write(
        f.project.path().join(".claude/agents/worker.md"),
        "---\nname: worker\ndescription: d2\nmodel: opus\neffort: max\ntools: Read\n---\nanother body\n",
    )
    .unwrap();
    let defs2 = crate::agent_def::load_agent_definitions(Some(f.project.path()));
    assert_eq!(before, agents_digest(&[&defs2["worker"]]));
}

#[test]
fn policy_digest_is_order_independent() {
    let a = def(
        DefinitionOrigin::Project,
        policy(Some("m"), None, None, &[]),
    );
    let mut b = def(
        DefinitionOrigin::Project,
        policy(None, Some("high"), None, &[]),
    );
    b.name = "other".into();
    assert_eq!(agents_digest(&[&a, &b]), agents_digest(&[&b, &a]));
    assert!(agents_digest(&[&a]).starts_with("sha256:"));
}
