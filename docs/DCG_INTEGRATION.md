# DCG Integration Plan — jcode Implementation

> What to build in `/data/projects/jcode`
> Synthesized from 9-repo research, dcg-core analysis, 3 rounds QA interview, discussion
> Date: 2026-05-30
> Branch: experiment/dcg-permission-modes
>
> **DCG Plan** (dcg-core enhancements) is at: `https://github.com/quangdang46/destructive_command_guard/blob/main/docs/DCG_PLAN.md`

---

## 0. Decisions Made

| Topic | Decision | Source |
|-------|----------|--------|
| **YOLO classifier** | ✅ Built in jcode only — `src/yolo_classifier.rs` | Discussion |
| **YOLO model** | Reuse active provider (zero extra cost). 2-stage: fast 64 tokens + thinking 4096 tokens. Fail closed. | Research + Discussion |
| **YOLO circuit breaker** | 3 consecutive YOLO denials → fallback to interactive prompt | Research + Discussion |
| **Mode cycling** | Yes, Shift+Tab full cycle (6 modes) in TUI | Round 2 QA |
| **TUI dialogs** | Permission dialogs for Prompt decisions | Round 2 QA |
| **Config loading** | TOML → dcg types, resolution chain | Discussion |
| **MCP permissions** | Unified pipeline — `mcp__server__tool` → `Engine::evaluate()` | Research + Discussion |
| **Subagent permissions** | Option A: inherit + restrict (opencode pattern) | Research + Discussion |
| **OS sandboxing** | ❌ Not doing — app-level only | Discussion |
| **dcg-core dependency** | Git URL (not local path, not crates.io) | Commit `29d937d4` |

---

## 1. Architecture

```
jcode (consumer)
─────────────────────────────────────────────────────
CLI flags: --permission-mode, --dangerously-skip-permissions
Config: .jcode/config.toml (TOML)
TUI: mode cycling (Shift+Tab), permission dialogs

┌───────────── yolo_classifier.rs ─────────────────┐
│  2-stage: fast (64 tokens) + thinking (4096t)  │
│  Reuse active provider (Claude/Gemini/OpenAI)   │
│  Fail closed on timeout/error                   │
│  3 consecutive denials → fallback to prompt     │
└────────────────────────────────────────────────┘
         │
         ▼
┌─────────────────── dcg_bridge.rs ───────────────────────┐
│  action_to_tool_call() → ToolCall + Effects               │
│  classify():                                              │
│    if Mode::Auto → ask YOLO first (if available)         │
│    else → Engine::evaluate() directly                    │
│    → BridgeDecision (Allow/Prompt/Deny)                │
│  set_mode() / current_mode()                            │
│  MCP: mcp__server__tool → Engine::evaluate()           │
└─────────────────────────────────────────────────────────┘
         │
         ▼
dcg-core (git URL: https://github.com/quangdang46/destructive_command_guard, branch=main)
         │
         ▼
Engine::evaluate(session, tool_call, mode, effects)
    ├─► Mode::pre_check()
    ├─► ProtectedPaths + PathAwareEscalation
    ├─► DangerousCommandRegistry (26-50 patterns)
    ├─► SafeCommandWhitelist (50+ commands)
    ├─► DenialEscalation (3/20)
    └─► Decision: Allow / Prompt / Deny
```

---

## 2. Phase Breakdown

### Phase 1 — Current State (DONE ✅)

| Item | Status | File |
|------|--------|------|
| dcg-core git dep (branch=main) | ✅ Done | `Cargo.toml` |
| `--permission-mode` CLI flag (6 modes) | ✅ Done | `src/cli/args.rs` |
| `--dangerously-skip-permissions` CLI flag | ✅ Done | `src/cli/args.rs` |
| `JCODE_PERMISSION_MODE` env var | ✅ Done | `src/cli/startup.rs` |
| dcg_bridge adapter | ✅ Done | `src/dcg_bridge.rs` |
| BridgeDecision → ActionTier mapping | ✅ Done | `src/safety.rs` |
| Engine + Session + ProtectedPaths integration | ✅ Done | `src/dcg_bridge.rs` |
| Legacy AUTO_ALLOWED compatibility | ✅ Done | `src/dcg_bridge.rs` |
| Tests for Default/Plan/Bypass modes | ✅ Done | `src/dcg_bridge.rs` |
| 9-repo research document | ✅ Done | `docs/PERMISSION_MODES_RESEARCH.md` |

### Phase 3.1 — TUI Mode Cycling [P0]

**What:** Runtime mode switching via Shift+Tab.

**Cycle order:**
```
default → acceptEdits → plan → auto → dontAsk → bypassPermissions → default
```

**Keybindings:**
- `Shift+Tab` — cycle forward
- `Ctrl+Shift+P M` — jump to specific mode (palette)

**Implementation:**
```rust
// In TUI event handler
fn handle_shift_tab(current_mode: Mode) -> Mode {
    match current_mode {
        Mode::Default    => Mode::AcceptEdits,
        Mode::AcceptEdits => Mode::Plan,
        Mode::Plan       => Mode::Auto,
        Mode::Auto       => Mode::DontAsk,
        Mode::DontAsk    => Mode::BypassPermissions,
        Mode::BypassPermissions => Mode::Default,
    }
}
```

**UI indicator:**
```
┌──────────────────────────────────────────────────────┐
│ jcode v0.13.0  │ 🔒 Plan Mode  │ Claude Opus 4.8    │
├──────────────────────────────────────────────────────┤
│  [Shift+Tab to change mode]                          │
└──────────────────────────────────────────────────────┘
```

**BypassPermissions confirmation dialog:**
```
⚠️  BypassPermissions disables all permission checks.
    Commands will execute without confirmation.
    Are you sure? [y/N]
```

---

### Phase 3.2 — TUI Permission Dialogs [P0]

**What:** Interactive approval dialogs for Prompt decisions.

**Dialog types:**

| Decision | Dialog |
|----------|--------|
| `Decision::Allow` | Auto-execute, no dialog |
| `Decision::Prompt { reason, alternatives }` | Show dialog: command + reason + alternatives |
| `Decision::Deny { reason, alternatives }` | Show blocked message + alternatives |

**Bash command dialog:**
```
┌──────────────────────────────────────────────────────┐
│ 🔧 Bash Permission Required                           │
├──────────────────────────────────────────────────────┤
│                                                      │
│  Command: rm -rf node_modules/                       │
│                                                      │
│  Reason: High-severity destructive pattern detected    │
│          (RecursiveDelete + DiskDestruction)         │
│                                                      │
│  Safer alternatives:                                │
│    • npm run clean                                  │
│    • rm -rf node_modules/.cache                    │
│                                                      │
│  [Approve Once] [Always Allow] [Deny] [Cancel]      │
└──────────────────────────────────────────────────────┘
```

**File edit dialog:**
```
┌──────────────────────────────────────────────────────┐
│ 📄 Edit Permission Required                          │
├──────────────────────────────────────────────────────┤
│                                                      │
│  File: src/main.rs                                  │
│                                                      │
│  +fn main() {                                       │
│  +    println!("Hello");                             │
│  +}                                                 │
│                                                      │
│  [Approve] [Always Allow] [Deny] [Cancel]           │
└──────────────────────────────────────────────────────┘
```

**Allow-once code input:**
```
┌──────────────────────────────────────────────────────┐
│ 🔑 Enter approval code from mobile device            │
│                                                      │
│  [______]  (6-char hex code)                       │
│                                                      │
│  Code expires in 24 hours                           │
│  [Submit] [Cancel]                                 │
└──────────────────────────────────────────────────────┘
```

---

### Phase 3.3 — Config Loading [P1]

**What:** Load TOML config and wire into dcg-core.

**Resolution chain:**
```
CLI --permission-mode > JCODE_PERMISSION_MODE env >
  project .jcode/config.toml > user ~/.jcode/config.toml > Mode::Default
```

**TOML config:**
```toml
[permissions]
default_mode = "default"

[permissions.protected_paths]
always_prompt = ["~/.ssh", "~/.aws", ".git", ".env"]
always_prompt_recursive = ["**/secrets/**", "**/.ssh/**"]

[permissions.tools]
bash = "prompt"
edit = "allow"
read = "allow"
webfetch = "prompt"
"bash:git *" = "allow"

[permissions.denial]
max_consecutive = 3
max_total = 20

[permissions.safe_commands]
enabled = true
extra = ["just", "make check"]
deny = ["git branch -D"]
```

**Implementation:**
```rust
impl DcgBridge {
    pub fn configure_from_toml(config: &PermissionsConfig) -> Result<(), ConfigError> {
        // Parse protected paths → EngineConfig
        // Parse per-tool overrides → apply after Engine::evaluate()
        // Parse denial limits → Session::DenialConfig
        // Parse safe command extras/denies → whitelist
    }
}
```

---

### Phase 3.4 — YOLO Classifier [P2]

**What:** LLM-based auto-approval when Mode::Auto is active.

**Why in jcode, NOT dcg-core:**
- dcg consumers use dcg as CLI hook (exit codes), not Rust library
- Only jcode links dcg-core as Cargo dependency
- YOLO needs LLM — consumer-specific

**Two-stage approach (from claude-code):**

```
Stage 1 — Fast (64 tokens, no thinking):
  "BLOCK if: irreversible, credential exfil, privilege escalation
   ALLOW if: read-only, in-CWD, tests/linters
   <block>yes</block> or <block>no</block>"

Stage 2 — Thinking (4096 tokens, CoT):
  Only if Stage 1 said BLOCK
  Uses tools to gather evidence (read files, git log, etc.
  Returns JSON: {risk_level, user_authorization, outcome}"
```

**Prompt (from claude-code):**
```
You are an automated security classifier for Claude Code...
BLOCK -- Always:
- Code from External (curl|bash, pip from unverified)
- Irreversible Local Destruction (rm -rf non-trivial)
- Unauthorized Persistence (.bashrc, cron, systemd)
- Security Weaken (disabling security tools)
- Privilege Escalation (sudo, su)

ALLOW -- Generally safe:
- Reading files, searching, read-only commands
- Creating/editing files in CWD
- Tests, linters, builds
```

**Fail closed:** All errors/timeouts/unavailable → Deny → fallback to interactive prompt.

**Circuit breaker:** 3 consecutive YOLO denials → stop calling YOLO, show interactive prompt.

---

### Phase 3.5 — Subagent Permissions [P2]

**What:** When spawning subagents, derive restricted permissions from parent.

**Pattern (from opencode `deriveSubagentSessionPermission`):**

```rust
fn derive_subagent_permissions(
    parent: &PermissionContext,
    subagent: &SubagentConfig,
) -> PermissionConfig {
    let mut rules = vec![];

    // (1) Inherit parent agent deny rules for edit
    //     If parent is Plan mode → subagent cannot edit
    for rule in &parent.agent_rules {
        if rule.action == "deny" && rule.permission == "edit" {
            rules.push(rule.clone());
        }
    }

    // (2) Inherit parent session denies + external_directory
    for rule in &parent.session_rules {
        if rule.permission == "external_directory" || rule.action == "deny" {
            rules.push(rule.clone());
        }
    }

    // (3) Default deny recursive capabilities (unless explicitly allowed)
    if !subagent.can_spawn {
        rules.push(Rule { permission: "task", pattern: "*", action: "deny" });
    }
    if !subagent.can_write_todos {
        rules.push(Rule { permission: "todowrite", pattern: "*", action: "deny" });
    }

    rules
}
```

**Key rule:** Children can do LESS than parent, never MORE.

**claude-code static deny-list (additional):**
```rust
const ALL_AGENT_DISALLOWED_TOOLS = [
    "Agent",           // no recursive nesting
    "AskUser",         // no ask-on-behalf
    "Workflow",         // no recursive workflow
    "VaultHttpFetch",   // user secrets stay on main thread
    "LocalMemoryRecall", // cross-session notes on main thread
];
```

---

### Phase 3.6 — MCP Permissions (Unified Pipeline) [P2]

**What:** MCP tools flow through the same `Engine::evaluate()` as builtin tools.

**Tool naming:** `mcp__serverName__toolName`

**Three matching levels (from claude-code):**
```
mcp__github          → matches ALL tools from github server
mcp__github__*       → wildcard, same as above
mcp__github__create_pull_request → exact tool
```

**Integration:**
```rust
fn action_to_tool_call(action: &str) -> (ToolCall, Vec<Effect>) {
    if action.starts_with("mcp__") {
        let parts: Vec<&str> = action.split("__").collect();
        // mcp__serverName__toolName
        let server = parts[1];
        let tool = parts.get(2).unwrap_or(&"*");
        return (ToolCall::Mcp { server, tool }, vec![Effect::Read, Effect::Write, Effect::Spawn]);
    }
    // ... builtin tool mapping
}
```

**Unknown MCP tools:** Default = Prompt (ask user).

**TOML config:**
```toml
[permissions.tools]
"mcp__github" = "allow"           # Allow all github MCP tools
"mcp__filesystem__write_file" = "prompt"  # Always prompt for filesystem writes
```

---

## 3. Dependency Map

```
Phase 1 (DONE)
    │
    ▼
Phase 3.1 TUI Mode Cycling
Phase 3.2 TUI Permission Dialogs
Phase 3.3 Config Loading (TOML)
    │
    ▼
Phase 3.4 YOLO Classifier (jcode-only, NOT dcg-core)
Phase 3.5 Subagent Permissions
Phase 3.6 MCP Unified Pipeline
```

---

## 4. Success Criteria

- [ ] Shift+Tab mode cycling works in TUI (6 modes)
- [ ] Permission dialogs show for Prompt decisions with alternatives
- [ ] Allow-once codes work (24h TTL, SHA-256 derived)
- [ ] TOML config loads and wires to dcg-core correctly
- [ ] YOLO: 2-stage (fast 64t + thinking 4096t), fail closed
- [ ] YOLO: 3 consecutive denials → circuit breaker triggers
- [ ] Subagent: inherits parent deny rules + default deny recursive
- [ ] MCP: `mcp__server__tool` → Engine::evaluate(), unknown → Prompt
- [ ] All 6 modes work end-to-end via CLI + env + config
- [ ] `cargo check` passes with zero errors
- [ ] Tests for all phases
