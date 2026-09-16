# Issue 2: Manager Mode for Jcode

## Problem

When the user asks the agent to "just coordinate" or "manage other agents" (orchestrator/verbal mode), the agent ignores the request and keeps doing implementation work itself. There's no real distinction between "do the work" and "direct others to do the work."

**Current state:**
- Jcode operates in a single mode: autonomous implementer
- The `swarm` tool exists but agents prefer to do work directly
- No CLI flag or config to enforce manager-only behavior
- User must repeatedly say "don't code, just coordinate" and the agent still codes
- The AGENTS.md says "delegate to subagents" but there's no enforcement

## User Experience

```
User: "Walk me through what needs to happen next"
Agent: *starts writing code*

User: "Stop. Don't code. Just tell me the plan."
Agent: *writes code anyway, adds "plan" in comments*

User: "I said MANAGER MODE. Coordinate, don't implement."
Agent: *finally stops, gives a plan, then asks "want me to implement?"*
```

## Proposed Design

### Mode Enum

```rust
pub enum AgentMode {
    /// Default: autonomous implementation
    Execute,
    /// Coordinator: plans, delegates, reviews, never codes
    Manager,
    /// Read-only: research, analyze, suggest, never modify files
    Researcher,
}
```

### Manager Mode Rules

When `AgentMode::Manager` is active:

1. **NEVER write code** — All file writes blocked at tool level
2. **NEVER run build commands** — `cargo`, `npm`, etc. blocked
3. **NEVER commit** — Git write operations blocked
4. **ALWAYS delegate** — Spawn workers for implementation
5. **ALWAYS plan first** — Output DAG/WBS before any action
6. **Review before approve** — Read diffs from workers, approve/reject
7. **Report status** — Always end with SESSION STATUS block

### Implementation Points

#### 1. CLI Flag
```bash
jcode --mode manager    # or --mgr
jcode --mode researcher # or --research
jcode                   # default: execute mode
```

#### 2. Tool Gating (crucial)
```rust
// In tool dispatch:
fn gate_tool(tool: &str, mode: &AgentMode) -> Result<(), ToolBlocked> {
    match mode {
        AgentMode::Manager => {
            if matches!(tool, "write" | "edit" | "multiedit" | "apply_patch" | "patch") {
                return Err(ToolBlocked::ManagerCannotImplement);
            }
            if matches!(tool, "bash") {
                // Allow read-only bash (git log, ls, cat, grep)
                // Block write bash (cargo build, npm install, git commit)
            }
            Ok(())
        }
        AgentMode::Researcher => {
            if matches!(tool, "write" | "edit" | "multiedit" | "apply_patch" | "patch" | "bash") {
                return Err(ToolBlocked::ResearcherReadOnly);
            }
            Ok(())
        }
        AgentMode::Execute => Ok(()),
    }
}
```

#### 3. System Prompt Injection
```
You are in MANAGER mode. You MUST NOT:
- Write or edit any files
- Run build/test commands  
- Make git commits
- Do any implementation work

You MUST:
- Create WBS/DAG for the task
- Spawn worker agents for each implementation step
- Review worker outputs
- Approve or reject changes
- Report progress to the user
```

#### 4. Swarm Integration
Manager mode naturally pairs with swarm:
- Manager decomposes tasks into DAG nodes
- Spawns workers via `swarm spawn` for each node
- Workers execute in Execute mode
- Manager reviews via `swarm report`
- Manager approves via `swarm complete_node`

### Config File Support
```toml
# .jcode/config.toml
[agent]
mode = "manager"  # or "execute" or "researcher"

[agent.manager]
max_workers = 8
auto_approve = false  # require human approval for merges
review_before_merge = true
```

### UX Improvements

1. **Mode indicator in status bar** — Show `[MGR]` or `[EXEC]` or `[RES]`
2. **Blocked action feedback** — When manager tries to write, show "Manager mode: use swarm spawn to delegate"
3. **Auto-decomposition** — Manager mode auto-creates DAG from user request
4. **Progress tracking** — Manager mode shows WBS progress bar automatically

## Files to Change

| File | Change |
|------|--------|
| `crates/jcode-cli/src/cli.rs` | Add `--mode` flag |
| `crates/jcode-tui/src/tui/status_bar.rs` | Show mode indicator |
| `crates/jcode-base/src/tools/` | Add tool gating by mode |
| `crates/jcode-base/src/agent/` | Mode-aware system prompt |
| `crates/jcode-tui/src/tui/app/commands.rs` | Mode-aware command dispatch |
