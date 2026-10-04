//! P2.2: the project swarm prompt must be resolved from the *session's* working
//! directory, not the daemon's.
//!
//! These tests drive the real seam (`Agent::tool_definitions`, which every turn
//! and the debug surface go through) rather than
//! `CommunicateTool::description_for` directly, because a helper test does not
//! prove the call site uses the helper. That is exactly how P1.3's dead opt-out
//! stayed hidden.

use super::*;

/// Redirect `JCODE_HOME` for the duration of a test so the global
/// `~/.jcode/swarm-prompt.md` cannot leak in from the developer's machine, and
/// restore it on drop so a failing assertion cannot poison later tests.
struct HomeGuard(Option<std::ffi::OsString>);

impl HomeGuard {
    fn new(home: &std::path::Path) -> Self {
        let previous = std::env::var_os("JCODE_HOME");
        crate::env::set_var("JCODE_HOME", home);
        Self(previous)
    }
}

impl Drop for HomeGuard {
    fn drop(&mut self) {
        match self.0.take() {
            Some(value) => crate::env::set_var("JCODE_HOME", value),
            None => crate::env::remove_var("JCODE_HOME"),
        }
    }
}

/// Build `<root>/<label>/.jcode/swarm-prompt.md` containing `text`.
fn repo_with_swarm_prompt(root: &std::path::Path, label: &str, text: &str) -> std::path::PathBuf {
    let repo = root.join(label);
    let prompt_dir = repo.join(".jcode");
    std::fs::create_dir_all(&prompt_dir).unwrap();
    std::fs::write(prompt_dir.join("swarm-prompt.md"), text).unwrap();
    repo
}

fn swarm_description(definitions: &[ToolDefinition]) -> String {
    definitions
        .iter()
        .find(|tool| tool.name == "swarm")
        .map(|tool| tool.description.clone())
        .expect("the swarm tool is registered for every session")
}

/// The plan's acceptance criterion: a session in project B sees B's swarm
/// prompt even though the daemon was started inside project A.
///
/// One shared `Registry` stands in for the single registry the daemon hands to
/// every session. That sharing is the whole point: one instance must serve two
/// projects' prompts correctly, so the fix cannot be "construct per session".
#[tokio::test]
async fn each_session_gets_its_own_projects_swarm_prompt_from_one_registry() {
    let _lock = crate::storage::lock_test_env();
    let home = tempfile::tempdir().unwrap();
    let _home_guard = HomeGuard::new(home.path());

    let project_a = repo_with_swarm_prompt(home.path(), "project-a", "project A routing");
    let project_b = repo_with_swarm_prompt(home.path(), "project-b", "project B routing");

    // The daemon's cwd is project A. That is the leak P2.2 describes.
    let prev_cwd = std::env::current_dir().unwrap();
    std::env::set_current_dir(&project_a).unwrap();

    let provider: Arc<dyn Provider> = Arc::new(NativeAutoCompactionProvider);
    let registry = Registry::new(provider.clone()).await;
    let mut agent_a =
        Agent::new_with_initial_working_dir(provider.clone(), registry.clone(), project_a.to_str());
    let mut agent_b = Agent::new_with_initial_working_dir(provider, registry, project_b.to_str());

    let a_description = swarm_description(&agent_a.tool_definitions().await);
    let b_description = swarm_description(&agent_b.tool_definitions().await);

    std::env::set_current_dir(prev_cwd).unwrap();

    assert!(
        a_description.contains("project A routing"),
        "session A lost its own prompt: {a_description:?}"
    );
    assert!(
        b_description.contains("project B routing"),
        "session B did not get project B's prompt: {b_description:?}"
    );
    assert!(
        !b_description.contains("project A routing"),
        "session B inherited the launching project's prompt: {b_description:?}"
    );
}

/// A session whose working directory is cleared must not pick up the daemon's
/// project prompt.
///
/// `Session::create` seeds `working_dir` from the process cwd, so a truly
/// `None` session is not reachable through `Agent::new`. Clearing it after
/// construction is how a session ends up unbound, and that is the state this
/// asserts on.
#[tokio::test]
async fn a_session_with_no_working_dir_gets_no_project_swarm_prompt() {
    let _lock = crate::storage::lock_test_env();
    let home = tempfile::tempdir().unwrap();
    let _home_guard = HomeGuard::new(home.path());
    let project_a = repo_with_swarm_prompt(home.path(), "project-a", "project A routing");

    let prev_cwd = std::env::current_dir().unwrap();
    std::env::set_current_dir(&project_a).unwrap();

    let provider: Arc<dyn Provider> = Arc::new(NativeAutoCompactionProvider);
    let registry = Registry::new(provider.clone()).await;
    let mut agent = Agent::new_with_initial_working_dir(provider, registry, project_a.to_str());

    // Positive control first: rooted at project A it does see A's prompt, so
    // the negative assertion below cannot pass because the file is unreadable.
    let rooted_description = swarm_description(&agent.tool_definitions().await);

    // `set_working_dir_for_pending_context` only applies a cwd when it is
    // `Some`, so going back to an unbound session needs the field plus an
    // explicit unlock: the tool snapshot is cached after the first read and the
    // seam would otherwise never run again.
    agent.session.working_dir = None;
    agent.unlock_tools();
    assert_eq!(agent.working_dir(), None);
    let description = swarm_description(&agent.tool_definitions().await);

    std::env::set_current_dir(prev_cwd).unwrap();

    assert!(
        rooted_description.contains("project A routing"),
        "positive control failed, the project prompt is not being read at all: {rooted_description:?}"
    );
    assert!(
        !description.contains("project A routing"),
        "a session with no working dir inherited the launching project's prompt: {description:?}"
    );
    assert!(
        description.starts_with("Coordinate agents"),
        "the swarm tool should still be present with its base description: {description:?}"
    );
}
