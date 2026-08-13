//! System prompt management

use std::path::{Path, PathBuf};
use std::process::Command;

/// Default system prompt for jcode (embedded at compile time)
pub const DEFAULT_SYSTEM_PROMPT: &str = include_str!("prompt/system_prompt.md");

/// Load the base system prompt, allowing the user to fully replace the built-in
/// [`DEFAULT_SYSTEM_PROMPT`]. Precedence: project `./.jcode/system-prompt.md`,
/// then global `~/.jcode/system-prompt.md`, then the built-in default.
///
/// This is a *replacement* hook. To merely add guidance on top of the default,
/// use `.jcode/prompt-overlay.md` instead.
pub fn load_base_system_prompt(working_dir: Option<&Path>) -> String {
    let project_dir = working_dir.unwrap_or(Path::new("."));
    let candidates = [
        Some(project_dir.join(".jcode").join("system-prompt.md")),
        crate::storage::jcode_dir()
            .ok()
            .map(|dir| dir.join("system-prompt.md")),
    ];
    for path in candidates.into_iter().flatten() {
        if let Ok(content) = std::fs::read_to_string(&path) {
            let trimmed = content.trim();
            if !trimmed.is_empty() {
                return trimmed.to_string();
            }
        }
    }
    DEFAULT_SYSTEM_PROMPT.to_string()
}

/// Prompt guidance for the optional Mermaid rendering capability.
pub const MERMAID_PROMPT: &str = "# Mermaid\n\nRender fenced `mermaid` blocks inline.";

/// Harness capabilities that conditionally contribute prompt modules.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PromptCapabilities {
    pub mermaid: bool,
}

impl Default for PromptCapabilities {
    fn default() -> Self {
        Self { mermaid: true }
    }
}

impl PromptCapabilities {
    fn current() -> Self {
        Self {
            mermaid: crate::config::config().features.mermaid,
        }
    }
}

fn base_system_prompt_parts(
    capabilities: PromptCapabilities,
    working_dir: Option<&Path>,
) -> Vec<String> {
    let mut parts = vec![load_base_system_prompt(working_dir)];
    if capabilities.mermaid {
        parts.push(MERMAID_PROMPT.to_string());
    }
    parts
}

/// Built-in default swarm prompt: model-routing guidance for spawned swarm
/// agents (which model/effort to pick per task kind). Users can override it by
/// creating `~/.jcode/swarm-prompt.md` (global) or `./.jcode/swarm-prompt.md`
/// (project). See [`load_swarm_prompt`].
pub const DEFAULT_SWARM_PROMPT: &str = include_str!("prompt/swarm_prompt.md");

/// Load the swarm prompt used to steer swarm model routing. Precedence:
/// project `./.jcode/swarm-prompt.md`, then global `~/.jcode/swarm-prompt.md`,
/// then the built-in [`DEFAULT_SWARM_PROMPT`].
pub fn load_swarm_prompt(working_dir: Option<&Path>) -> String {
    let project_dir = working_dir.unwrap_or(Path::new("."));
    let candidates = [
        Some(project_dir.join(".jcode").join("swarm-prompt.md")),
        crate::storage::jcode_dir()
            .ok()
            .map(|dir| dir.join("swarm-prompt.md")),
    ];
    for path in candidates.into_iter().flatten() {
        if let Ok(content) = std::fs::read_to_string(&path) {
            let trimmed = content.trim();
            if !trimmed.is_empty() {
                return trimmed.to_string();
            }
        }
    }
    DEFAULT_SWARM_PROMPT.trim().to_string()
}

/// Reasoning-effort sentinel that means "use the strongest reasoning the model
/// supports, AND actively orchestrate the work with the swarm tool". Providers
/// translate this to their strongest real effort when building API requests,
/// while the UI/session keep the literal `swarm` marker so the agent knows to
/// inject [`SWARM_EFFORT_DIRECTIVE`].
pub const SWARM_EFFORT: &str = "swarm";

/// Reasoning-effort sentinel for the **deep task graph** mode: strongest model
/// reasoning AND the comprehensive DAG-first swarm workflow (decompose into a
/// validated task graph, critique/verify gates, typed artifact handoffs). Sits
/// one rung above [`SWARM_EFFORT`] on the effort ladder: `... xhigh`, `swarm`
/// (light fan-out), `swarm-deep` (deep task graph). Providers translate this to
/// their strongest real effort, while the UI/session keep the literal marker so
/// the agent knows to inject [`SWARM_DEEP_EFFORT_DIRECTIVE`].
pub const SWARM_DEEP_EFFORT: &str = "swarm-deep";

/// System-prompt directive injected when the active reasoning effort is
/// [`SWARM_EFFORT`]. Instructs the agent to lean on the swarm tooling.
pub const SWARM_EFFORT_DIRECTIVE: &str = "# Swarm Effort\n\nYou are running at the maximum reasoning effort with swarm orchestration enabled. For any non-trivial task, decompose the work and use the `swarm` tool to spawn and coordinate parallel agents (spawn workers with concrete prompts, assign tasks, and collect their reports) instead of doing everything yourself in one thread. Prefer parallelizing independent subtasks across swarm members, and use a coordinator/plan when the work has multiple stages. Only skip the swarm for trivial, single-step requests.";

/// System-prompt directive injected when the active reasoning effort is
/// [`SWARM_DEEP_EFFORT`]. Instructs the agent to run the comprehensive DAG-first
/// task-graph workflow.
pub const SWARM_DEEP_EFFORT_DIRECTIVE: &str = "# Deep Task Graph\n\nYou are running at maximum reasoning effort with the deep task-graph swarm workflow. Treat the task DAG as the primary object, not ad hoc agent chat. Workflow:\n\n1. Seed a graph with `swarm task_graph` using `mode: \"deep\"`: lay out nodes (kind explore|implement|verify|fix|synthesize) and `depends_on` edges instead of answering directly. (At this effort the server already defaults the plan to deep, but pass `mode: \"deep\"` explicitly anyway.) The engine auto-inserts a plan-wide root gate over your seed: the plan cannot finish until a final adversarial audit passes, and that audit can inject new top-level work.\n2. For any node that is too big, `swarm expand_node` to decompose it into a child sub-DAG (you become its planner/integrator). In deep mode a critique/verify gate is auto-inserted before a composite node can close. The graph is EXPECTED to outgrow its seed, often by several times: growth (expansions and gate-injected gaps) is the system working, not scope creep. plan_status reports seeded-vs-grown counts.\n3. Finish each node with `swarm complete_node` and a typed artifact: `findings`, `evidence` (file:line / commit refs), `validation`, `open_questions`, a required `confidence` (low|medium|high; report low honestly, it routes follow-up work to shore up that scope), and an honest `what_i_did_not_check`. Downstream nodes are hydrated with these artifacts automatically. There is no other way to close a deep node: a turn ending without expand_node/complete_node re-queues the node to a fresh worker and fails it on repeat.\n4. When a critique/verify gate finds gaps or failures, use `swarm inject_gap` to add new nodes; the parent cannot close until they drain. A passing gate artifact must account for EVERY node it audited by id (the server rejects rubber stamps), and cannot pass over a low-confidence sibling without addressing it explicitly, so treat low-confidence siblings as priority probe targets.\n5. Use `swarm run_plan` to drive the graph to completion. It returns immediately and drives the plan as a background task (progress card + wake on completion), so keep working or answer the user while it runs; check `swarm plan_status` or `bg` for progress. Deep mode fans out wide automatically (many workers run in parallel, bounded only by the swarm member cap), so prefer decomposing into MANY independent sibling nodes rather than a few serial ones: keep the ready set wide so run_plan can dispatch lots of agents at once. Only add `depends_on` edges for real data dependencies.\n\nComprehensiveness is structural: prefer decomposition + gates over a single thorough answer, so it is very unlikely any nook or cranny is missed.";

/// Returns true when `effort` is either swarm sentinel (light or deep),
/// case-insensitive. Used by providers to map to the strongest real effort.
pub fn is_swarm_effort(effort: &str) -> bool {
    let trimmed = effort.trim();
    trimmed.eq_ignore_ascii_case(SWARM_EFFORT) || trimmed.eq_ignore_ascii_case(SWARM_DEEP_EFFORT)
}

/// Returns true when `effort` is specifically the deep task-graph sentinel.
pub fn is_deep_swarm_effort(effort: &str) -> bool {
    effort.trim().eq_ignore_ascii_case(SWARM_DEEP_EFFORT)
}

/// The user-facing "general effort" ladder is one list, but each rung is one of
/// two internal kinds: a plain reasoning level (mapped straight to the provider
/// wire effort) or a swarm orchestration mode (which also pins reasoning to the
/// model's max). [`EffortKind`] is the single classifier all consumers use so the
/// UI, providers, and scheduler never disagree about what a rung means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EffortKind {
    /// A plain reasoning level (none/low/medium/high/xhigh/max).
    Reasoning,
    /// Light swarm mode: max reasoning + parallel fan-out.
    SwarmLight,
    /// Deep swarm mode: max reasoning + DAG-first task graph.
    SwarmDeep,
}

impl EffortKind {
    /// True when this rung is a swarm orchestration mode rather than a plain
    /// reasoning level. Such rungs must not be treated as per-model effort
    /// variants (e.g. they should not generate `model (effort)` picker rows).
    pub fn is_swarm_mode(self) -> bool {
        matches!(self, EffortKind::SwarmLight | EffortKind::SwarmDeep)
    }
}

/// Classify a general-effort rung string into its [`EffortKind`].
pub fn classify_effort(effort: &str) -> EffortKind {
    let trimmed = effort.trim();
    if trimmed.eq_ignore_ascii_case(SWARM_DEEP_EFFORT) {
        EffortKind::SwarmDeep
    } else if trimmed.eq_ignore_ascii_case(SWARM_EFFORT) {
        EffortKind::SwarmLight
    } else {
        EffortKind::Reasoning
    }
}

/// True when an effort rung is a swarm orchestration mode (light or deep) rather
/// than a plain reasoning level. Convenience wrapper over [`classify_effort`].
pub fn is_swarm_mode_effort(effort: &str) -> bool {
    classify_effort(effort).is_swarm_mode()
}

/// Append the appropriate swarm directive to a split prompt's dynamic part when
/// the active reasoning effort is a swarm sentinel. The deep sentinel injects the
/// DAG-first task-graph directive; the light sentinel injects the fan-out
/// directive. No-op otherwise.
pub fn append_swarm_effort_directive(split: &mut SplitSystemPrompt, effort: Option<&str>) {
    let directive = match effort {
        Some(effort) if is_deep_swarm_effort(effort) => SWARM_DEEP_EFFORT_DIRECTIVE,
        Some(effort) if is_swarm_effort(effort) => SWARM_EFFORT_DIRECTIVE,
        _ => return,
    };
    if !split.dynamic_part.is_empty() {
        split.dynamic_part.push_str("\n\n");
    }
    split.dynamic_part.push_str(directive);
}
/// Mission-continuation template (embedded at compile time). Consumed by the
/// `mission` module in the upper `jcode-app-core` layer; the asset lives here
/// alongside the other prompt templates.
pub const MISSION_CONTINUATION_TEMPLATE: &str = include_str!("prompt/mission_continuation.md");
const SELFDEV_MODE_PROMPT: &str = include_str!("prompt/selfdev_mode.txt");
const SELFDEV_FOCUS_TUI_PROMPT: &str = include_str!("prompt/selfdev_focus_tui.txt");
const SELFDEV_FOCUS_DESKTOP2_PROMPT: &str = include_str!("prompt/selfdev_focus_desktop2.txt");
/// Split system prompt for efficient caching
/// Static content is cached, dynamic content is not
#[derive(Debug, Clone, Default)]
pub struct SplitSystemPrompt {
    /// Static content that should be cached (instruction files, base prompt, skills)
    pub static_part: String,
    /// Dynamic turn context that changes per request (memory, active skill, reminders)
    pub dynamic_part: String,
}

impl SplitSystemPrompt {
    pub fn chars(&self) -> usize {
        match (self.static_part.is_empty(), self.dynamic_part.is_empty()) {
            (true, true) => 0,
            (false, true) => self.static_part.len(),
            (true, false) => self.dynamic_part.len(),
            (false, false) => self.static_part.len() + 2 + self.dynamic_part.len(),
        }
    }

    pub fn estimated_tokens(&self) -> usize {
        crate::util::estimate_tokens(&if self.static_part.is_empty() {
            self.dynamic_part.clone()
        } else if self.dynamic_part.is_empty() {
            self.static_part.clone()
        } else {
            format!("{}\n\n{}", self.static_part, self.dynamic_part)
        })
    }
}

/// Skill info for system prompt
pub struct SkillInfo {
    pub name: String,
    pub description: String,
}

const SKILL_DESC_MAX_CHARS: usize = 120;

fn clip_skill_description(description: &str) -> String {
    let one_line = description.split_whitespace().collect::<Vec<_>>().join(" ");
    if one_line.chars().count() <= SKILL_DESC_MAX_CHARS {
        return one_line;
    }

    let mut clipped: String = one_line
        .chars()
        .take(SKILL_DESC_MAX_CHARS.saturating_sub(1))
        .collect();
    clipped.push('…');
    clipped
}

fn build_available_skills_section(available_skills: &[SkillInfo]) -> Option<String> {
    if available_skills.is_empty() {
        return None;
    }

    let mut section = "# Available Skills\n\nYou have access to the following skills that the user can invoke with `/skillname`:\n".to_string();
    for skill in available_skills {
        section.push_str(&format!(
            "\n- `/{} ` - {}",
            skill.name,
            clip_skill_description(&skill.description)
        ));
    }
    section.push_str(
        "\n\nWhen a user asks about available skills or capabilities, mention these skills.",
    );
    Some(section)
}

/// Information about what's loaded in the context window
#[derive(Debug, Clone, Default)]
pub struct ContextInfo {
    // === Static (System Prompt) ===
    /// Base system prompt size (chars)
    pub system_prompt_chars: usize,
    /// Immutable session context size (chars), when persisted in transcript history.
    pub session_context_chars: usize,
    /// Whether project AGENTS.md was loaded
    pub has_project_agents_md: bool,
    /// Project AGENTS.md size (chars)
    pub project_agents_md_chars: usize,
    /// Whether global ~/AGENTS.md was loaded
    pub has_global_agents_md: bool,
    /// Global AGENTS.md size (chars)
    pub global_agents_md_chars: usize,
    /// Skills section size (chars)
    pub skills_chars: usize,
    /// Self-dev section size (chars)
    pub selfdev_chars: usize,
    /// Memory section size (chars)
    pub memory_chars: usize,
    /// Prompt overlay section size (chars)
    pub prompt_overlay_chars: usize,
    /// Preferred tools section size (chars)
    pub preferred_tools_chars: usize,
    // === Dynamic (Conversation) ===
    /// Tool definitions sent to API (chars)
    pub tool_defs_chars: usize,
    /// Number of tool definitions
    pub tool_defs_count: usize,
    /// User messages total size (chars)
    pub user_messages_chars: usize,
    /// Number of user messages
    pub user_messages_count: usize,
    /// Assistant messages total size (chars)
    pub assistant_messages_chars: usize,
    /// Number of assistant messages
    pub assistant_messages_count: usize,
    /// Tool calls size (chars)
    pub tool_calls_chars: usize,
    /// Number of tool calls
    pub tool_calls_count: usize,
    /// Tool results size (chars)
    pub tool_results_chars: usize,
    /// Number of tool results
    pub tool_results_count: usize,

    /// Total system prompt size (chars)
    pub total_chars: usize,
}

impl ContextInfo {
    /// Rough estimate of tokens (chars / 4 is a common approximation)
    pub fn estimated_tokens(&self) -> usize {
        self.total_chars / 4
    }

    pub fn prompt_prefix_chars(&self) -> usize {
        self.system_prompt_chars
            + self.session_context_chars
            + self.project_agents_md_chars
            + self.global_agents_md_chars
            + self.skills_chars
            + self.selfdev_chars
            + self.memory_chars
            + self.prompt_overlay_chars
            + self.preferred_tools_chars
            + self.tool_defs_chars
    }

    pub fn prompt_prefix_tokens(&self) -> usize {
        self.prompt_prefix_chars() / 4
    }

    pub fn tool_definition_tokens(&self) -> usize {
        self.tool_defs_chars / 4
    }

    /// Get breakdown as (label, chars, icon) tuples for display
    pub fn breakdown(&self) -> Vec<(&'static str, usize, &'static str)> {
        let mut parts = vec![
            ("sys", self.system_prompt_chars, "⚙"),
            ("session", self.session_context_chars, "🌍"),
        ];
        if self.has_project_agents_md {
            parts.push(("agents", self.project_agents_md_chars, "📋"));
        }
        if self.has_global_agents_md {
            parts.push(("~agents", self.global_agents_md_chars, "📋"));
        }
        if self.skills_chars > 0 {
            parts.push(("skills", self.skills_chars, "🔧"));
        }
        if self.selfdev_chars > 0 {
            parts.push(("dev", self.selfdev_chars, "🛠"));
        }
        if self.memory_chars > 0 {
            parts.push(("mem", self.memory_chars, "🧠"));
        }
        if self.prompt_overlay_chars > 0 {
            parts.push(("overlay", self.prompt_overlay_chars, "🧩"));
        }
        if self.preferred_tools_chars > 0 {
            parts.push(("tools", self.preferred_tools_chars, "🧰"));
        }
        parts
    }
}

/// Build the full system prompt with static context.
pub fn build_system_prompt(skill_prompt: Option<&str>, available_skills: &[SkillInfo]) -> String {
    build_system_prompt_with_selfdev(skill_prompt, available_skills, false)
}

/// Build the full system prompt with optional self-dev tools
pub fn build_system_prompt_with_selfdev(
    skill_prompt: Option<&str>,
    available_skills: &[SkillInfo],
    is_selfdev: bool,
) -> String {
    let (prompt, _) = build_system_prompt_with_context(skill_prompt, available_skills, is_selfdev);
    prompt
}

/// Build the full system prompt and return context info about what was loaded
pub fn build_system_prompt_with_context(
    skill_prompt: Option<&str>,
    available_skills: &[SkillInfo],
    is_selfdev: bool,
) -> (String, ContextInfo) {
    build_system_prompt_with_context_and_memory(skill_prompt, available_skills, is_selfdev, None)
}

/// Build the full system prompt with optional memory section and return context info
pub fn build_system_prompt_with_context_and_memory(
    skill_prompt: Option<&str>,
    available_skills: &[SkillInfo],
    is_selfdev: bool,
    memory_prompt: Option<&str>,
) -> (String, ContextInfo) {
    build_system_prompt_full(
        skill_prompt,
        available_skills,
        is_selfdev,
        memory_prompt,
        None,
    )
}

/// Build the full system prompt with working directory support for loading context files
pub fn build_system_prompt_full(
    skill_prompt: Option<&str>,
    available_skills: &[SkillInfo],
    is_selfdev: bool,
    memory_prompt: Option<&str>,
    working_dir: Option<&Path>,
) -> (String, ContextInfo) {
    build_system_prompt_full_with_capabilities(
        skill_prompt,
        available_skills,
        is_selfdev,
        memory_prompt,
        working_dir,
        PromptCapabilities::current(),
    )
}

pub fn build_system_prompt_full_with_capabilities(
    skill_prompt: Option<&str>,
    available_skills: &[SkillInfo],
    is_selfdev: bool,
    memory_prompt: Option<&str>,
    working_dir: Option<&Path>,
    capabilities: PromptCapabilities,
) -> (String, ContextInfo) {
    let mut parts = base_system_prompt_parts(capabilities, working_dir);
    let mut info = ContextInfo {
        system_prompt_chars: parts.join("\n\n").len(),
        ..Default::default()
    };

    // Add self-dev guidance only in active self-dev sessions. Normal sessions
    // learn about the on-ramp from the mode-aware `selfdev` tool schema.
    if is_selfdev {
        let selfdev_prompt = build_selfdev_prompt_for_working_dir(working_dir);
        info.selfdev_chars = selfdev_prompt.len();
        parts.push(selfdev_prompt);
    }

    // Add AGENTS.md instructions with tracking (from working_dir or cwd)
    let (md_content, md_info) = load_agents_md_files_from_dir(working_dir);
    if let Some(content) = md_content {
        parts.push(content);
    }
    // Merge file info
    info.has_project_agents_md = md_info.has_project_agents_md;
    info.project_agents_md_chars = md_info.project_agents_md_chars;
    info.has_global_agents_md = md_info.has_global_agents_md;
    info.global_agents_md_chars = md_info.global_agents_md_chars;

    // Add optional prompt overlays from ~/.jcode/ and ./.jcode/
    let (overlay_content, overlay_chars) = load_prompt_overlay_files_from_dir(working_dir);
    if let Some(content) = overlay_content {
        info.prompt_overlay_chars = overlay_chars;
        parts.push(content);
    }

    // Add optional preferred-tool guidance from ~/.jcode/ and ./.jcode/
    let (preferred_tools_content, preferred_tools_chars) =
        load_preferred_tools_files_from_dir(working_dir);
    if let Some(content) = preferred_tools_content {
        info.preferred_tools_chars = preferred_tools_chars;
        parts.push(content);
    }

    if let Some(memory) = memory_prompt {
        info.memory_chars = memory.len();
        parts.push(memory.to_string());
    }

    // Add available skills list
    if let Some(skills_section) = build_available_skills_section(available_skills) {
        info.skills_chars = skills_section.len();
        parts.push(skills_section);
    }

    // Add active skill prompt
    if let Some(skill) = skill_prompt {
        parts.push(format!("# Active Skill\n\n{}", skill));
    }

    let prompt = parts.join("\n\n");
    info.total_chars = prompt.len();

    (prompt, info)
}

/// Build system prompt split into static (cacheable) and dynamic parts
/// This improves cache hit rate by keeping frequently-changing content separate
pub fn build_system_prompt_split(
    skill_prompt: Option<&str>,
    available_skills: &[SkillInfo],
    is_selfdev: bool,
    memory_prompt: Option<&str>,
    working_dir: Option<&Path>,
) -> (SplitSystemPrompt, ContextInfo) {
    build_system_prompt_split_with_capabilities(
        skill_prompt,
        available_skills,
        is_selfdev,
        memory_prompt,
        working_dir,
        PromptCapabilities::current(),
    )
}

pub fn build_system_prompt_split_with_capabilities(
    skill_prompt: Option<&str>,
    available_skills: &[SkillInfo],
    is_selfdev: bool,
    memory_prompt: Option<&str>,
    working_dir: Option<&Path>,
    capabilities: PromptCapabilities,
) -> (SplitSystemPrompt, ContextInfo) {
    let mut static_parts = base_system_prompt_parts(capabilities, working_dir);
    let mut dynamic_parts = Vec::new();
    let mut info = ContextInfo {
        system_prompt_chars: static_parts.join("\n\n").len(),
        ..Default::default()
    };

    // === STATIC CONTENT (cacheable) ===

    // Add self-dev guidance only in active self-dev sessions. Normal sessions
    // learn about the on-ramp from the mode-aware `selfdev` tool schema.
    if is_selfdev {
        let selfdev_prompt = build_selfdev_prompt_static_for_working_dir(working_dir);
        info.selfdev_chars = selfdev_prompt.len();
        static_parts.push(selfdev_prompt);
    }

    // Add AGENTS.md instructions (static per project)
    let (md_content, md_info) = load_agents_md_files_from_dir(working_dir);
    if let Some(content) = md_content {
        static_parts.push(content);
    }
    info.has_project_agents_md = md_info.has_project_agents_md;
    info.project_agents_md_chars = md_info.project_agents_md_chars;
    info.has_global_agents_md = md_info.has_global_agents_md;
    info.global_agents_md_chars = md_info.global_agents_md_chars;

    // Add optional prompt overlays from ~/.jcode/ and ./.jcode/
    let (overlay_content, overlay_chars) = load_prompt_overlay_files_from_dir(working_dir);
    if let Some(content) = overlay_content {
        info.prompt_overlay_chars = overlay_chars;
        static_parts.push(content);
    }

    // Add optional preferred-tool guidance (static per project/user)
    let (preferred_tools_content, preferred_tools_chars) =
        load_preferred_tools_files_from_dir(working_dir);
    if let Some(content) = preferred_tools_content {
        info.preferred_tools_chars = preferred_tools_chars;
        static_parts.push(content);
    }

    // Add available skills list (fairly static)
    if let Some(skills_section) = build_available_skills_section(available_skills) {
        info.skills_chars = skills_section.len();
        static_parts.push(skills_section);
    }

    // === TURN CONTEXT (not cached) ===

    // Memory prompt (changes per conversation)
    if let Some(memory) = memory_prompt {
        info.memory_chars = memory.len();
        dynamic_parts.push(memory.to_string());
    }

    // Active skill prompt (changes per skill invocation)
    if let Some(skill) = skill_prompt {
        dynamic_parts.push(format!("# Active Skill\n\n{}", skill));
    }

    let static_part = static_parts.join("\n\n");
    let dynamic_part = dynamic_parts.join("\n\n");
    info.total_chars = static_part.len() + dynamic_part.len();

    (
        SplitSystemPrompt {
            static_part,
            dynamic_part,
        },
        info,
    )
}

/// Build self-dev tools prompt section (static version without dynamic socket path)
#[cfg(test)]
fn build_selfdev_prompt_static() -> String {
    build_selfdev_prompt_static_for_context(SelfDevProductContext::Tui)
}

/// Build self-dev tools prompt section
#[cfg(test)]
fn build_selfdev_prompt() -> String {
    build_selfdev_prompt_for_context(SelfDevProductContext::Tui)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SelfDevProductContext {
    Tui,
    Desktop2,
}

impl SelfDevProductContext {
    fn from_working_dir(working_dir: Option<&Path>) -> Self {
        let Some(working_dir) = working_dir else {
            return Self::Tui;
        };

        let path = working_dir.to_string_lossy().replace('\\', "/");
        if path.contains("/crates/jcode-desktop") || path.ends_with("crates/jcode-desktop2") {
            Self::Desktop2
        } else {
            Self::Tui
        }
    }

    fn prompt_block(self) -> &'static str {
        match self {
            Self::Tui => SELFDEV_FOCUS_TUI_PROMPT,
            Self::Desktop2 => SELFDEV_FOCUS_DESKTOP2_PROMPT,
        }
    }
}

fn build_selfdev_prompt_static_for_working_dir(working_dir: Option<&Path>) -> String {
    build_selfdev_prompt_static_for_context(SelfDevProductContext::from_working_dir(working_dir))
}

fn build_selfdev_prompt_for_working_dir(working_dir: Option<&Path>) -> String {
    build_selfdev_prompt_for_context(SelfDevProductContext::from_working_dir(working_dir))
}

fn build_selfdev_prompt_static_for_context(context: SelfDevProductContext) -> String {
    build_selfdev_prompt_for_context(context).replace("__DEBUG_SOCKET_BLOCK__\n\n", "")
}

fn build_selfdev_prompt_for_context(context: SelfDevProductContext) -> String {
    SELFDEV_MODE_PROMPT.replace("__SELFDEV_PRODUCT_FOCUS__", context.prompt_block())
}

/// Build immutable session context captured once per session.
pub fn build_session_context(working_dir: Option<&Path>) -> String {
    let mut lines = vec!["# Session Context".to_string()];

    let now_utc = chrono::Utc::now();
    lines.push(format!("Date: {}", now_utc.format("%Y-%m-%d")));
    lines.push(format!("Time: {} UTC", now_utc.format("%H:%M:%S")));
    lines.push("Timezone: UTC".to_string());
    lines.push(format!("OS: {}", std::env::consts::OS));
    lines.push(format!("Architecture: {}", std::env::consts::ARCH));
    lines.push(format!(
        "Jcode version: {} ({})",
        jcode_build_meta::version(),
        jcode_build_meta::git_hash()
    ));

    if let Some(hardware) = hardware_context() {
        lines.push(hardware);
    }

    let cwd = working_dir.map(Path::to_path_buf);
    if let Some(cwd) = cwd.as_deref() {
        lines.push(format!("Working directory: {}", cwd.display()));
        if let Some(git_info) = get_git_info(Some(cwd)) {
            lines.push(git_info);
        }
    }

    lines.join("\n")
}

/// Get git branch and status summary
fn get_git_info(working_dir: Option<&Path>) -> Option<String> {
    let mut command = Command::new("git");
    if let Some(dir) = working_dir {
        command.current_dir(dir);
    }
    // Check if we're in a git repo
    let in_repo = command
        .args(["rev-parse", "--is-inside-work-tree"])
        .output()
        .ok()
        .map(|o| o.status.success())
        .unwrap_or(false);

    if !in_repo {
        return None;
    }

    let mut info = vec!["Git:".to_string()];

    // Current branch
    let mut branch_command = Command::new("git");
    if let Some(dir) = working_dir {
        branch_command.current_dir(dir);
    }
    if let Ok(output) = branch_command.args(["branch", "--show-current"]).output()
        && output.status.success()
    {
        let branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !branch.is_empty() {
            info.push(format!("  Branch: {}", branch));
        }
    }

    // Short status (modified files count)
    let mut status_command = Command::new("git");
    if let Some(dir) = working_dir {
        status_command.current_dir(dir);
    }
    if let Ok(output) = status_command.args(["status", "--porcelain"]).output()
        && output.status.success()
    {
        let status = String::from_utf8_lossy(&output.stdout);
        let modified: Vec<&str> = status.lines().take(5).collect();
        if !modified.is_empty() {
            info.push(format!("  Modified: {} files", status.lines().count()));
            for file in modified {
                info.push(format!("    {}", file));
            }
            if status.lines().count() > 5 {
                info.push("    ...".to_string());
            }
        }
    }

    if info.len() > 1 {
        Some(info.join("\n"))
    } else {
        None
    }
}

fn hardware_context() -> Option<String> {
    // Hardware never changes for the life of the process, but this used to be
    // rebuilt for every session create/attach, forking `lspci` each time. On a
    // busy shared server that meant one subprocess per client connection.
    static HARDWARE_CONTEXT: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    HARDWARE_CONTEXT
        .get_or_init(hardware_context_uncached)
        .clone()
}

fn hardware_context_uncached() -> Option<String> {
    let mut lines = Vec::new();

    if let Some(machine) = machine_model() {
        lines.push(format!("  Machine: {}", machine));
    }
    if let Some(cpu) = cpu_model() {
        lines.push(format!("  CPU: {}", cpu));
    }
    if let Some(gpu) = gpu_summary() {
        lines.push(format!("  GPU: {}", gpu));
    }
    if let Some(memory) = memory_summary() {
        lines.push(format!("  Memory: {}", memory));
    }

    if lines.is_empty() {
        None
    } else {
        let mut out = vec!["Hardware:".to_string()];
        out.extend(lines);
        Some(out.join("\n"))
    }
}

fn read_trimmed_file(path: impl Into<PathBuf>) -> Option<String> {
    std::fs::read_to_string(path.into())
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn machine_model() -> Option<String> {
    let vendor = read_trimmed_file("/sys/devices/virtual/dmi/id/sys_vendor");
    let product = read_trimmed_file("/sys/devices/virtual/dmi/id/product_name");
    match (vendor, product) {
        (Some(vendor), Some(product)) if product.contains(&vendor) => Some(product),
        (Some(vendor), Some(product)) => Some(format!("{} {}", vendor, product)),
        (None, Some(product)) => Some(product),
        (Some(vendor), None) => Some(vendor),
        (None, None) => None,
    }
}

fn cpu_model() -> Option<String> {
    let cpuinfo = std::fs::read_to_string("/proc/cpuinfo").ok()?;
    cpuinfo.lines().find_map(|line| {
        let (_, value) = line.split_once(':')?;
        if line.trim_start().starts_with("model name") {
            let value = value.trim();
            if value.is_empty() {
                None
            } else {
                Some(value.to_string())
            }
        } else {
            None
        }
    })
}

fn memory_summary() -> Option<String> {
    let meminfo = std::fs::read_to_string("/proc/meminfo").ok()?;
    let kb = meminfo.lines().find_map(|line| {
        let rest = line.strip_prefix("MemTotal:")?.trim();
        rest.split_whitespace().next()?.parse::<u64>().ok()
    })?;
    let gib = kb as f64 / 1024.0 / 1024.0;
    Some(format!("{:.1} GiB", gib))
}

fn gpu_summary() -> Option<String> {
    let output = Command::new("lspci").output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut gpus: Vec<String> = text
        .lines()
        .filter(|line| {
            line.contains(" VGA compatible controller")
                || line.contains(" 3D controller")
                || line.contains(" Display controller")
        })
        .filter_map(|line| {
            line.split_once(':')
                .map(|(_, rest)| rest.trim().to_string())
        })
        .collect();
    gpus.dedup();
    if gpus.is_empty() {
        None
    } else {
        Some(gpus.join("; "))
    }
}

fn load_agents_md_files_from_dirs(
    project_dir: &Path,
    global_agents_md: Option<&Path>,
) -> (Option<String>, ContextInfo) {
    let mut contents = vec![];
    let mut info = ContextInfo::default();

    // Helper to load a file if it exists, returns (formatted_content, raw_size)
    let load_file = |path: &Path, label: &str| -> Option<(String, usize)> {
        if path.exists() {
            std::fs::read_to_string(path).ok().map(|content| {
                let raw_size = content.len();
                let formatted = format!("# {}\n\n{}", label, content.trim());
                (formatted, raw_size)
            })
        } else {
            None
        }
    };

    let project_agents_md = project_dir.join("AGENTS.md");
    if let Some((content, size)) = load_file(&project_agents_md, "Project Instructions (AGENTS.md)")
    {
        info.has_project_agents_md = true;
        info.project_agents_md_chars = size;
        contents.push(content);
    }

    // Canonical file identity handles cwd=$HOME as well as symlinked aliases.
    // If either file is absent or cannot be resolved, loading below remains the
    // source of truth and simply skips unreadable files.
    let global_duplicates_project = global_agents_md.is_some_and(|global_agents_md| {
        match (
            std::fs::canonicalize(&project_agents_md),
            std::fs::canonicalize(global_agents_md),
        ) {
            (Ok(project), Ok(global)) => project == global,
            _ => false,
        }
    });

    if !global_duplicates_project
        && let Some(global_agents_md) = global_agents_md
        && let Some((content, size)) =
            load_file(global_agents_md, "Global Instructions (~/AGENTS.md)")
    {
        info.has_global_agents_md = true;
        info.global_agents_md_chars = size;
        contents.push(content);
    }

    if contents.is_empty() {
        (None, info)
    } else {
        (Some(contents.join("\n\n")), info)
    }
}

/// Load AGENTS.md files from a specific working directory.
pub fn load_agents_md_files_from_dir(working_dir: Option<&Path>) -> (Option<String>, ContextInfo) {
    let project_dir = working_dir.unwrap_or(Path::new("."));
    let global_agents_md = crate::storage::user_home_path("AGENTS.md").ok();
    load_agents_md_files_from_dirs(project_dir, global_agents_md.as_deref())
}

/// Load optional prompt overlay markdown from ~/.jcode/ and ./.jcode/
fn load_prompt_overlay_files_from_dir(working_dir: Option<&Path>) -> (Option<String>, usize) {
    let mut contents = vec![];
    let mut total_chars = 0usize;

    let load_file = |path: &Path, label: &str| -> Option<(String, usize)> {
        if path.exists() {
            std::fs::read_to_string(path).ok().map(|content| {
                let raw_size = content.len();
                let formatted = format!("# {}\n\n{}", label, content.trim());
                (formatted, raw_size)
            })
        } else {
            None
        }
    };

    let project_dir = working_dir.unwrap_or(Path::new("."));
    if let Some((content, size)) = load_file(
        &project_dir.join(".jcode").join("prompt-overlay.md"),
        "Project Prompt Overlay (.jcode/prompt-overlay.md)",
    ) {
        total_chars += size;
        contents.push(content);
    }

    if let Ok(global_overlay) = crate::storage::jcode_dir().map(|dir| dir.join("prompt-overlay.md"))
        && let Some((content, size)) = load_file(
            &global_overlay,
            "Global Prompt Overlay (~/.jcode/prompt-overlay.md)",
        )
    {
        total_chars += size;
        contents.push(content);
    }

    if contents.is_empty() {
        (None, 0)
    } else {
        (Some(contents.join("\n\n")), total_chars)
    }
}

/// Load optional preferred-tool guidance from ~/.jcode/ and ./.jcode/
fn load_preferred_tools_files_from_dir(working_dir: Option<&Path>) -> (Option<String>, usize) {
    let mut contents = vec![];
    let mut total_chars = 0usize;

    let load_file = |path: &Path, label: &str| -> Option<(String, usize)> {
        if path.exists() {
            std::fs::read_to_string(path).ok().map(|content| {
                let raw_size = content.len();
                let formatted = format!("# {}\n\n{}", label, content.trim());
                (formatted, raw_size)
            })
        } else {
            None
        }
    };

    let project_dir = working_dir.unwrap_or(Path::new("."));
    if let Some((content, size)) = load_file(
        &project_dir.join(".jcode").join("preferred-tools.md"),
        "Project Preferred Tools (.jcode/preferred-tools.md)",
    ) {
        total_chars += size;
        contents.push(content);
    }

    if let Ok(global_preferred_tools) =
        crate::storage::jcode_dir().map(|dir| dir.join("preferred-tools.md"))
        && let Some((content, size)) = load_file(
            &global_preferred_tools,
            "Global Preferred Tools (~/.jcode/preferred-tools.md)",
        )
    {
        total_chars += size;
        contents.push(content);
    }

    if contents.is_empty() {
        (None, 0)
    } else {
        (Some(contents.join("\n\n")), total_chars)
    }
}

#[cfg(test)]
#[path = "prompt_tests.rs"]
mod prompt_tests;
