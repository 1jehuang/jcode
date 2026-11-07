use super::*;
use crate::hooks::{hook_commands_for, hook_configured_for};

struct Env(&'static str, Option<std::ffi::OsString>);

impl Env {
    fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
        let previous = std::env::var_os(key);
        crate::env::set_var(key, value);
        crate::config::invalidate_config_cache();
        Self(key, previous)
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        match self.1.take() {
            Some(value) => crate::env::set_var(self.0, value),
            None => crate::env::remove_var(self.0),
        }
        crate::config::invalidate_config_cache();
    }
}

struct Fixture {
    _home: tempfile::TempDir,
    _env: Env,
    project: tempfile::TempDir,
}

fn fixture(project_toml: &str) -> Fixture {
    let home = tempfile::tempdir().unwrap();
    let env = Env::set("JCODE_HOME", home.path());
    let project = tempfile::tempdir().unwrap();
    std::fs::create_dir(project.path().join(".git")).unwrap();
    std::fs::create_dir(project.path().join(".jcode")).unwrap();
    std::fs::write(project.path().join(".jcode/config.toml"), project_toml).unwrap();
    clear_cache();
    Fixture {
        _home: home,
        _env: env,
        project,
    }
}

impl Fixture {
    fn cwd(&self) -> String {
        self.project.path().display().to_string()
    }

    fn grant(&self) {
        let (digest, _) = project_hooks_digest(self.project.path()).unwrap().unwrap();
        project_trust::grant(self.project.path(), Scope::Hooks, &digest).unwrap();
    }

    fn commands(&self, event: &str) -> Vec<String> {
        hook_commands_for(event, Some(&self.cwd()))
            .into_iter()
            .map(|c| c.command)
            .collect()
    }
}

const BASIC: &str = r#"
[hooks]
pre_tool = ["bash .jcode/hooks/guard.sh"]
post_tool_feedback = "echo feedback"
post_tool_feedback_timeout_ms = 999999
"#;

#[test]
fn untrusted_project_hooks_never_run() {
    let _lock = crate::storage::lock_test_env();
    let f = fixture(BASIC);
    assert!(f.commands("pre_tool").is_empty());
    assert!(!hook_configured_for("post_tool_feedback", Some(&f.cwd())));
}

#[test]
fn trusted_hooks_run_after_global_in_order() {
    let _lock = crate::storage::lock_test_env();
    let f = fixture(BASIC);
    let _global = Env::set("JCODE_HOOK_PRE_TOOL", "global-gate");
    f.grant();
    assert_eq!(
        f.commands("pre_tool"),
        ["global-gate", "bash .jcode/hooks/guard.sh"]
    );
    let scoped = hook_commands_for("pre_tool", Some(&f.cwd()));
    assert!(!scoped[0].scope.is_project());
    assert!(scoped[1].scope.is_project());
}

#[test]
fn editing_any_key_including_a_timeout_invalidates_trust() {
    let _lock = crate::storage::lock_test_env();
    let f = fixture(BASIC);
    f.grant();
    assert!(!f.commands("post_tool_feedback").is_empty());
    std::fs::write(
        f.project.path().join(".jcode/config.toml"),
        BASIC.replace("999999", "888888"),
    )
    .unwrap();
    clear_cache();
    assert!(
        f.commands("post_tool_feedback").is_empty(),
        "timeout edit must re-require trust"
    );
    std::fs::write(
        f.project.path().join(".jcode/config.toml"),
        BASIC.replace("echo feedback", "echo changed"),
    )
    .unwrap();
    clear_cache();
    assert!(f.commands("post_tool_feedback").is_empty());
}

#[test]
fn project_timeout_is_clamped_to_the_global_cap() {
    let _lock = crate::storage::lock_test_env();
    let f = fixture(BASIC);
    f.grant();
    let stop = &hook_commands_for("post_tool_feedback", Some(&f.cwd()))[0];
    assert_eq!(stop.timeout_ms, Some(999_999));
    assert_eq!(stop.effective_timeout_ms(10_000), 120_000);
    let pre = &hook_commands_for("pre_tool", Some(&f.cwd()))[0];
    assert_eq!(pre.timeout_ms, None);
    assert_eq!(pre.effective_timeout_ms(5_000), 5_000);
    let global = ScopedCommand {
        command: "x".into(),
        scope: CommandScope::global(),
        timeout_ms: None,
    };
    assert_eq!(global.effective_timeout_ms(7_000), 7_000);
}

#[test]
fn malformed_toml_turns_project_hooks_off_without_touching_global() {
    let _lock = crate::storage::lock_test_env();
    let f = fixture("[hooks\npre_tool = ");
    let _global = Env::set("JCODE_HOOK_PRE_TOOL", "global-gate");
    assert_eq!(f.commands("pre_tool"), ["global-gate"]);
}

#[test]
fn disable_switches_and_other_tables_are_ignored() {
    let _lock = crate::storage::lock_test_env();
    let f = fixture(&format!("{BASIC}\n[provider]\ndefault_model = \"x\"\n"));
    f.grant();
    assert!(!f.commands("pre_tool").is_empty());
    {
        let _off = Env::set("JCODE_PROJECT_HOOKS", "0");
        assert!(f.commands("pre_tool").is_empty());
    }
    assert!(!f.commands("pre_tool").is_empty());
}

#[test]
fn revoke_stops_project_hooks() {
    let _lock = crate::storage::lock_test_env();
    let f = fixture(BASIC);
    f.grant();
    assert!(!f.commands("pre_tool").is_empty());
    project_trust::revoke(f.project.path(), Some(Scope::Hooks)).unwrap();
    assert!(f.commands("pre_tool").is_empty());
}

#[test]
fn revoking_or_disabling_takes_effect_while_the_parse_is_cached() {
    let _lock = crate::storage::lock_test_env();
    let f = fixture(BASIC);
    f.grant();
    assert!(!f.commands("pre_tool").is_empty(), "warms the cache");
    project_trust::revoke(f.project.path(), Some(Scope::Hooks)).unwrap();
    assert!(
        f.commands("pre_tool").is_empty(),
        "a revoke must not wait for the cache to expire"
    );

    f.grant();
    assert!(!f.commands("pre_tool").is_empty());
    let _off = Env::set("JCODE_PROJECT_HOOKS", "0");
    assert!(
        f.commands("pre_tool").is_empty(),
        "disabling project hooks must not wait for the cache to expire"
    );
}

#[test]
fn two_projects_never_see_each_others_hooks() {
    let _lock = crate::storage::lock_test_env();
    let a = fixture("[hooks]\npre_tool = \"only-a\"\n");
    let b_dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(b_dir.path().join(".git")).unwrap();
    std::fs::create_dir(b_dir.path().join(".jcode")).unwrap();
    std::fs::write(
        b_dir.path().join(".jcode/config.toml"),
        "[hooks]\npre_tool = \"only-b\"\n",
    )
    .unwrap();
    a.grant();
    let (digest, _) = project_hooks_digest(b_dir.path()).unwrap().unwrap();
    project_trust::grant(b_dir.path(), Scope::Hooks, &digest).unwrap();
    let b_cwd = b_dir.path().display().to_string();
    assert_eq!(a.commands("pre_tool"), ["only-a"]);
    let b: Vec<_> = hook_commands_for("pre_tool", Some(&b_cwd))
        .into_iter()
        .map(|c| c.command)
        .collect();
    assert_eq!(b, ["only-b"]);
}

#[test]
fn worktree_with_identical_table_runs_and_edited_table_does_not() {
    let _lock = crate::storage::lock_test_env();
    let f = fixture(BASIC);
    f.grant();
    // A linked worktree of the main checkout.
    let admin = f.project.path().join(".git/worktrees/wt");
    std::fs::create_dir_all(&admin).unwrap();
    std::fs::write(admin.join("commondir"), "../..\n").unwrap();
    let wt = tempfile::tempdir().unwrap();
    std::fs::write(
        wt.path().join(".git"),
        format!("gitdir: {}\n", admin.display()),
    )
    .unwrap();
    std::fs::write(
        admin.join("gitdir"),
        format!("{}\n", wt.path().join(".git").display()),
    )
    .unwrap();
    std::fs::create_dir(wt.path().join(".jcode")).unwrap();
    std::fs::write(wt.path().join(".jcode/config.toml"), BASIC).unwrap();
    let cwd = wt.path().display().to_string();
    assert_eq!(hook_commands_for("pre_tool", Some(&cwd)).len(), 1);
    std::fs::write(
        wt.path().join(".jcode/config.toml"),
        BASIC.replace("guard.sh", "evil.sh"),
    )
    .unwrap();
    clear_cache();
    assert!(hook_commands_for("pre_tool", Some(&cwd)).is_empty());
}

#[test]
fn project_dir_is_exported_to_every_hook_and_relative_programs_resolve() {
    let _lock = crate::storage::lock_test_env();
    let f = fixture(BASIC);
    let root = std::fs::canonicalize(f.project.path()).unwrap();
    assert_eq!(project_dir_for(Some(&f.cwd())), Some(root.clone()));
    let scope = CommandScope {
        project_dir: Some(root.clone()),
    };
    let event = crate::hooks::HookEvent::new("pre_tool")
        .cwd(f.cwd())
        .with_scope(&scope);
    let cmd = crate::hooks::build_hook_process(".jcode/hooks/guard.sh --x", &event).unwrap();
    assert_eq!(
        cmd.get_program(),
        root.join(".jcode/hooks/guard.sh").as_os_str()
    );
    let envs: std::collections::HashMap<_, _> = cmd
        .get_envs()
        .filter_map(|(k, v)| {
            Some((
                k.to_string_lossy().into_owned(),
                v?.to_string_lossy().into_owned(),
            ))
        })
        .collect();
    assert_eq!(envs["JCODE_HOOK_SCOPE"], "project");
    assert_eq!(envs["JCODE_PROJECT_DIR"], root.display().to_string());
    assert_eq!(envs["CLAUDE_PROJECT_DIR"], root.display().to_string());

    // A global hook in the same session still learns the project dir.
    let global_event = crate::hooks::HookEvent::new("pre_tool").cwd(f.cwd());
    let gcmd = crate::hooks::build_hook_process("global-tool", &global_event).unwrap();
    let genvs: std::collections::HashMap<_, _> = gcmd
        .get_envs()
        .filter_map(|(k, v)| {
            Some((
                k.to_string_lossy().into_owned(),
                v?.to_string_lossy().into_owned(),
            ))
        })
        .collect();
    assert_eq!(genvs["JCODE_HOOK_SCOPE"], "global");
    assert_eq!(genvs["JCODE_PROJECT_DIR"], root.display().to_string());
}

#[test]
fn scope_review_lists_commands_and_timeouts() {
    let _lock = crate::storage::lock_test_env();
    let f = fixture(BASIC);
    let review = hooks_scope_review(f.project.path()).unwrap().unwrap();
    assert_eq!(review.scope, Scope::Hooks);
    assert!(review.digest.starts_with("sha256:"));
    assert!(
        review
            .lines
            .iter()
            .any(|l| l.contains("pre_tool: bash .jcode/hooks/guard.sh"))
    );
    assert!(
        review
            .lines
            .iter()
            .any(|l| l.contains("post_tool_feedback_timeout_ms = 999999"))
    );
    assert_eq!(review.item_count, 2);
}
