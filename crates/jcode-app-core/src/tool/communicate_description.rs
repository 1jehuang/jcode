/// Description preamble for the swarm tool. The user-tunable swarm prompt is
/// appended per session by [`CommunicateTool::description_for`] rather than
/// baked in at registry construction.
const BASE_DESCRIPTION: &str =
    "Coordinate agents: spawn workers with a prompt, message them, and manage swarm plans.";

pub struct CommunicateTool {
    /// Description with no project swarm prompt attached. The per-session
    /// description is produced by [`Self::description_for`], which the agent's
    /// tool-definition builder applies on top of this so each project gets its
    /// own `swarm-prompt.md`.
    pub(super) description: String,
}

impl CommunicateTool {
    pub fn new() -> Self {
        Self {
            description: BASE_DESCRIPTION.to_string(),
        }
    }

    /// Build the swarm tool description for a session, resolving the project's
    /// `swarm-prompt.md` against that session's working directory.
    ///
    /// This lives here rather than in the `Tool::description` impl because the
    /// daemon shares one tool registry across every project, and the project
    /// swarm prompt is per-session data (P2.2). `working_dir` is the session's
    /// workspace root, never the daemon's: `None` resolves to no project prompt
    /// rather than the process cwd.
    pub fn description_for(working_dir: Option<&std::path::Path>) -> String {
        let swarm_prompt = crate::prompt::load_swarm_prompt(working_dir);
        if swarm_prompt.is_empty() {
            BASE_DESCRIPTION.to_string()
        } else {
            format!(
                "{BASE_DESCRIPTION}\n\nSwarm prompt (user-tunable via ~/.jcode/swarm-prompt.md):\n{swarm_prompt}"
            )
        }
    }
}
